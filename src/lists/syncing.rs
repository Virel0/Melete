//! Syncing one list: each title it names pinned to TMDB and added to the wanted list, and the
//! outcome recorded.

use super::*;

/// What a sync did.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct SyncResult {
    /// Titles on the list (up to its max_items).
    pub found: usize,
    /// Titles it added to the wanted list.
    pub added: usize,
    /// Titles on the wanted list already, left as they were.
    pub already_wanted: usize,
    /// Titles it didn't add, with why.
    pub skipped: Vec<Skipped>,
}

/// A title a sync didn't add.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Skipped {
    /// The title as the list names it.
    pub title: String,
    /// Why it wasn't added, in plain words.
    pub why: String,
}

/// A title as a list names it, before TMDB says which it is.
pub(super) enum Named {
    Tmdb(i64, String),
    Imdb(String),
    Titled(rottentomatoes::Listed),
}

/// Syncs one list and records the outcome; `None` when there's no such list.
pub async fn sync(
    pool: &SqlitePool,
    sources: &Sources,
    id: i64,
) -> anyhow::Result<Option<SyncResult>> {
    // One sync at a time, so two can't add the same title.
    static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one_at_a_time = RUNNING.lock().await;
    let Some(list) = get(pool, id).await? else {
        return Ok(None);
    };
    let outcome = sync_list(pool, sources, &list).await;
    let recorded = match &outcome {
        Ok(result) => serde_json::to_value(result)?,
        Err(err) => json!({ "error": format!("{err:#}") }),
    };
    sqlx::query("UPDATE import_lists SET synced_at = ?, last_result = ? WHERE id = ?")
        .bind(unix_now())
        .bind(recorded.to_string())
        .bind(id)
        .execute(pool)
        .await?;
    outcome.map(Some)
}

pub(super) async fn sync_list(
    pool: &SqlitePool,
    sources: &Sources,
    list: &List,
) -> anyhow::Result<SyncResult> {
    let series = list.kind == "series";
    let max = list.max_items as usize;
    let named = names(sources, list, series, max).await?;
    let mut result = SyncResult {
        found: named.len(),
        ..SyncResult::default()
    };
    for name in named {
        let (tmdb_id, title) = match pin(&sources.tmdb, name, series).await? {
            Ok(found) => found,
            Err(skipped) => {
                result.skipped.push(skipped);
                continue;
            }
        };
        let outcome = if series {
            add_series(pool, &sources.tmdb, list, tmdb_id).await?
        } else {
            add_movie(pool, &sources.tmdb, list, tmdb_id).await?
        };
        match outcome {
            Outcome::Added => result.added += 1,
            Outcome::Already => result.already_wanted += 1,
            Outcome::Skipped(why) => result.skipped.push(Skipped { title, why }),
        }
    }
    Ok(result)
}
