//! Picks as the database keeps them, and claiming the right to make them afresh.

use super::*;

/// A viewer's picks, or titles like one, as kept.
#[derive(Debug, Default)]
pub(super) struct Kept {
    /// What the viewer had watched when they were made; empty for titles like one.
    pub history: String,
    pub picks: Vec<Pick>,
    pub made_at: Option<i64>,
    pub error: Option<String>,
    /// When making them was last started.
    pub tried_at: i64,
}

type KeptRow = (String, String, Option<i64>, Option<String>, i64);

fn kept((history, picks, made_at, error, tried_at): KeptRow) -> Kept {
    Kept {
        history,
        picks: serde_json::from_str(&picks).unwrap_or_default(),
        made_at,
        error,
        tried_at,
    }
}

fn kind(title: Title) -> &'static str {
    if title.series { "series" } else { "movie" }
}

/// A viewer's picks as kept.
pub(super) async fn viewer_row(pool: &SqlitePool, user: i64) -> sqlx::Result<Option<Kept>> {
    let row: Option<KeptRow> = sqlx::query_as(
        "SELECT history, picks, made_at, error, tried_at FROM ai_picks WHERE user_id = ?",
    )
    .bind(user)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(kept))
}

/// Starts making a viewer's picks unless that was started after `since`: true when this call
/// may make them.
pub(super) async fn claim_viewer(
    pool: &SqlitePool,
    user: i64,
    history: &str,
    since: i64,
) -> sqlx::Result<bool> {
    let claimed = sqlx::query(
        "INSERT INTO ai_picks (user_id, history, picks, tried_at) VALUES (?1, ?2, '[]', ?3)
         ON CONFLICT (user_id) DO UPDATE SET tried_at = excluded.tried_at
         WHERE ai_picks.tried_at <= ?4",
    )
    .bind(user)
    .bind(history)
    .bind(unix_now())
    .bind(since)
    .execute(pool)
    .await?;
    Ok(claimed.rows_affected() > 0)
}

/// Keeps a viewer's new picks, made from `history`.
pub(super) async fn keep_viewer(
    pool: &SqlitePool,
    user: i64,
    history: &str,
    picks: &[Pick],
) -> anyhow::Result<()> {
    let now = unix_now();
    sqlx::query(
        "INSERT INTO ai_picks (user_id, history, picks, made_at, tried_at) VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT (user_id) DO UPDATE SET history = excluded.history,
             picks = excluded.picks, made_at = excluded.made_at, error = NULL",
    )
    .bind(user)
    .bind(history)
    .bind(serde_json::to_string(picks)?)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Keeps why making a viewer's picks failed; the picks from before stay.
pub(super) async fn fail_viewer(pool: &SqlitePool, user: i64, why: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE ai_picks SET error = ? WHERE user_id = ?")
        .bind(why)
        .bind(user)
        .execute(pool)
        .await?;
    Ok(())
}

/// Titles like one, as kept for those who see `seen`.
pub(super) async fn similar_row(
    pool: &SqlitePool,
    title: Title,
    seen: &str,
) -> sqlx::Result<Option<Kept>> {
    let row: Option<KeptRow> = sqlx::query_as(
        "SELECT '', picks, made_at, error, tried_at FROM ai_similar
         WHERE kind = ? AND title_id = ? AND seen = ?",
    )
    .bind(kind(title))
    .bind(title.id)
    .bind(seen)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(kept))
}

/// Starts making the titles like one unless that was started after `since`.
pub(super) async fn claim_similar(
    pool: &SqlitePool,
    title: Title,
    seen: &str,
    since: i64,
) -> sqlx::Result<bool> {
    let claimed = sqlx::query(
        "INSERT INTO ai_similar (kind, title_id, seen, picks, tried_at) VALUES (?1, ?2, ?3, '[]', ?4)
         ON CONFLICT (kind, title_id, seen) DO UPDATE SET tried_at = excluded.tried_at
         WHERE ai_similar.tried_at <= ?5",
    )
    .bind(kind(title))
    .bind(title.id)
    .bind(seen)
    .bind(unix_now())
    .bind(since)
    .execute(pool)
    .await?;
    Ok(claimed.rows_affected() > 0)
}

/// Keeps the titles like one, for those who see `seen`.
pub(super) async fn keep_similar(
    pool: &SqlitePool,
    title: Title,
    seen: &str,
    picks: &[Pick],
) -> anyhow::Result<()> {
    let now = unix_now();
    sqlx::query(
        "INSERT INTO ai_similar (kind, title_id, seen, picks, made_at, tried_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT (kind, title_id, seen) DO UPDATE SET picks = excluded.picks,
             made_at = excluded.made_at, error = NULL",
    )
    .bind(kind(title))
    .bind(title.id)
    .bind(seen)
    .bind(serde_json::to_string(picks)?)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Keeps why making the titles like one failed; those from before stay.
pub(super) async fn fail_similar(
    pool: &SqlitePool,
    title: Title,
    seen: &str,
    why: &str,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE ai_similar SET error = ? WHERE kind = ? AND title_id = ? AND seen = ?")
        .bind(why)
        .bind(kind(title))
        .bind(title.id)
        .bind(seen)
        .execute(pool)
        .await?;
    Ok(())
}
