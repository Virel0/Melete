//! The ways back when stored secrets can't be read: what the refused start says, and
//! `melete secrets clear`.

use std::path::Path;

use sqlx::SqlitePool;

use super::startup::{Unreadables, unreadable};
use super::{KEY_FILE, Unreadable, key_file};

/// Clears the stored secrets the key in `data_dir` can't open (all sealed ones, without a key):
/// outside services' settings and indexers' keys, to be entered again, and search results'
/// links, which a new search finds again. Two-factor secrets are left for
/// `melete user reset-password`. Returns what couldn't be read.
pub async fn clear(pool: &SqlitePool, data_dir: &Path) -> anyhow::Result<Unreadables> {
    let key = key_file::read(&data_dir.join(KEY_FILE))?;
    let found = unreadable(pool, key.as_ref()).await?;
    let mut tx = pool.begin().await?;
    for (name, _) in &found.settings {
        sqlx::query("DELETE FROM settings WHERE name = ?")
            .bind(name)
            .execute(&mut *tx)
            .await?;
    }
    for (id, _, _) in &found.indexers {
        sqlx::query("UPDATE indexers SET api_key = '' WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "UPDATE release_decisions SET download_url = NULL, guid = NULL
         WHERE id IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(&found.links)?)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(found)
}

/// Why the server won't start, which secrets, and the ways back.
pub(super) fn refusal(path: &Path, missing: bool, found: &Unreadables) -> String {
    let because = if missing {
        format!(
            "the key they were sealed with, {}, isn't there",
            path.display()
        )
    } else {
        format!(
            "they don't open with the key in {}: it isn't the key they were sealed with",
            path.display()
        )
    };
    let named = |name: &str, why: &Unreadable| match why {
        Unreadable::Unsealed => format!("{name} (kept unsealed)"),
        _ => name.to_owned(),
    };
    let mut text = format!("Melete won't start: stored secrets can't be read, because {because}.");
    let mut list = |label: &str, names: Vec<String>| {
        if !names.is_empty() {
            text.push_str(&format!("\n  {label}: {}", names.join(", ")));
        }
    };
    list(
        "Outside services' settings",
        found.settings.iter().map(|(n, w)| named(n, w)).collect(),
    );
    list(
        "Indexers' keys",
        found.indexers.iter().map(|(_, n, w)| named(n, w)).collect(),
    );
    list(
        "Two-factor sign-in of",
        found.two_factor.iter().map(|(n, w)| named(n, w)).collect(),
    );
    if !found.links.is_empty() {
        text.push_str(&format!(
            "\n  Download links of {} search results",
            found.links.len()
        ));
    }
    text.push_str(&format!(
        "\nTo recover, either:
  - put back the {KEY_FILE} they were sealed with (from where you keep its copy), then start Melete again; or
  - clear them and enter them again. With Melete stopped (docker compose stop melete), run
      docker compose run --rm melete melete secrets clear
    for the services' settings, indexers' keys and search results' links, and for each user named under two-factor sign-in
      docker compose run --rm melete melete user reset-password <name> --remove-second-factor
    then start Melete again{new}.",
        new = if missing {
            ": it makes a new key once nothing sealed is left"
        } else {
            ""
        },
    ));
    text
}
