//! The ways back when stored secrets can't be read: what the refused start says, and
//! `melete secrets clear`.

use std::path::Path;

use anyhow::bail;
use sqlx::SqlitePool;

use super::key_file::{self, Found};
use super::startup::{Unreadables, unreadable};
use super::{KEY_FILE, Unreadable};

/// Clears the stored secrets the key in `data_dir` can't open (all sealed ones, without a key):
/// outside services' settings and indexers' keys, to be entered again; two-factor setups still
/// waiting for their first code, to be started again; and search results' links, which a new
/// search finds again. Two-factor sign-in that's on is left for `melete user reset-password`.
/// Returns what couldn't be read.
pub async fn clear(pool: &SqlitePool, data_dir: &Path) -> anyhow::Result<Unreadables> {
    let path = data_dir.join(KEY_FILE);
    let key = match key_file::read(&path)? {
        Found::Key(key) => Some(key),
        Found::Missing => None,
        Found::Garbled => bail!("{}", garbled(&path, pool).await?),
    };
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
    for (user_id, _) in &found.setups {
        sqlx::query("DELETE FROM two_factor WHERE user_id = ? AND confirmed_at IS NULL")
            .bind(user_id)
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

/// What to say when the key file isn't a key: safe to delete when nothing is sealed yet (a
/// start that stopped while making it), else to be put back from its copy.
pub(super) async fn garbled(path: &Path, pool: &SqlitePool) -> anyhow::Result<String> {
    let nothing_sealed = unreadable(pool, None).await?.is_empty();
    let start = format!(
        "{} isn't a secrets key Melete made: it should be 64 hexadecimal characters, and it's empty, cut short or another file.",
        path.display()
    );
    Ok(if nothing_sealed {
        format!(
            "{start} Nothing in the database is sealed with it yet, so it's safe to delete it and start Melete again, which makes a new one."
        )
    } else {
        format!(
            "{start} Secrets in the database are sealed with the key it should hold: put back its copy from where you keep it, then start Melete again."
        )
    })
}

/// Why the server won't start, which secrets, and the ways back.
pub(super) fn refusal(path: &Path, missing: bool, found: &Unreadables) -> String {
    let unsealed = |why: &Unreadable| *why == Unreadable::Unsealed;
    let only_unsealed = found.links.is_empty()
        && found.setups.is_empty()
        && found.settings.iter().all(|(_, w)| unsealed(w))
        && found.indexers.iter().all(|(_, _, w)| unsealed(w))
        && found.two_factor.iter().all(|(_, w)| unsealed(w));
    let because = if missing {
        format!(
            "because the key they were sealed with, {}, isn't there",
            path.display()
        )
    } else if only_unsealed {
        "because they're kept unsealed in a database that keeps secrets sealed, which only a change made to the database outside Melete does".to_owned()
    } else {
        format!(
            "because they don't open with the key in {}: it isn't the key they were sealed with",
            path.display()
        )
    };
    let mut text = format!("Melete won't start: stored secrets can't be read, {because}.");
    text.push_str(&listed(found));
    let put_back = if only_unsealed {
        String::new()
    } else {
        format!(
            "\n  - put back the {KEY_FILE} they were sealed with (from where you keep its copy), then start Melete again; or"
        )
    };
    text.push_str(&format!(
        "\nTo recover, either:{put_back}
  - clear them and enter them again. With Melete stopped (docker compose stop melete), run
      docker compose run --rm melete melete secrets clear
    for the services' settings, indexers' keys, unconfirmed two-factor setups and search results' links, and for each user named under two-factor sign-in
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

/// The lines naming what can't be read.
fn listed(found: &Unreadables) -> String {
    let named = |name: &str, why: &Unreadable| match why {
        Unreadable::Unsealed => format!("{name} (kept unsealed)"),
        _ => name.to_owned(),
    };
    let mut text = String::new();
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
    list(
        "Unconfirmed two-factor setups of",
        found.setups.iter().map(|(_, n)| n.clone()).collect(),
    );
    if !found.links.is_empty() {
        text.push_str(&format!(
            "\n  Download links of {} search results",
            found.links.len()
        ));
    }
    text
}
