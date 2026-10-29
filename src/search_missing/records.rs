//! Runs as they're recorded: looked up, the latest listed, and those a restart cut off marked.

use super::*;

/// A run, and how far it has got.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MissingSearch {
    /// The run's row id.
    pub id: i64,
    /// "movies", "series" or "albums".
    pub area: String,
    /// "missing", or "cutoff_unmet" for upgrades of files below their profile's cutoff.
    pub goal: String,
    /// "running"; "stopping" (asked to stop, finishing the search under way); "done";
    /// "stopped"; or "failed", with why in `message`.
    pub state: String,
    /// Searches the run set out to make; a series counts each season.
    pub total: i64,
    /// How many of those it has dealt with, searched for or not.
    pub done: i64,
    /// How many searches it has made.
    pub searched: i64,
    /// How many of those it grabbed a release from.
    pub grabbed: i64,
    /// What it's searching for now; `None` once it has ended.
    pub current: Option<String>,
    /// How it ended, in plain words; `None` while it runs.
    pub message: Option<String>,
    /// Why some weren't searched for or grabbed: "Title: why", the first 50 kept.
    pub notes: Vec<String>,
    /// The username of who started it; `None` once that user is removed.
    pub started_by: Option<String>,
    /// When it started, in Unix seconds.
    pub started_at: i64,
    /// When it ended, in Unix seconds; `None` while it runs.
    pub finished_at: Option<i64>,
}

#[derive(sqlx::FromRow)]
pub(super) struct Row {
    pub(super) id: i64,
    pub(super) area: String,
    pub(super) goal: String,
    pub(super) state: String,
    pub(super) total: i64,
    pub(super) done: i64,
    pub(super) searched: i64,
    pub(super) grabbed: i64,
    pub(super) current: Option<String>,
    pub(super) message: Option<String>,
    pub(super) notes: String,
    pub(super) started_by: Option<String>,
    pub(super) started_at: i64,
    pub(super) finished_at: Option<i64>,
}

impl From<Row> for MissingSearch {
    fn from(r: Row) -> Self {
        MissingSearch {
            id: r.id,
            area: r.area,
            goal: r.goal,
            state: r.state,
            total: r.total,
            done: r.done,
            searched: r.searched,
            grabbed: r.grabbed,
            current: r.current,
            message: r.message,
            // Written only here, always as a list.
            notes: serde_json::from_str(&r.notes).unwrap_or_default(),
            started_by: r.started_by,
            started_at: r.started_at,
            finished_at: r.finished_at,
        }
    }
}

pub(super) const COLUMNS: &str =
    "SELECT s.id, s.area, s.goal, s.state, s.total, s.done, s.searched, s.grabbed,
        s.current, s.message, s.notes, u.username AS started_by, s.started_at, s.finished_at
     FROM missing_searches s LEFT JOIN users u ON u.id = s.started_by";

/// A run by its id; `None` when there's no such run.
pub async fn get(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<MissingSearch>> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{COLUMNS} WHERE s.id = ?")))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(MissingSearch::from))
}

/// Each area's latest run, movies, series, then albums.
pub async fn latest(pool: &SqlitePool) -> sqlx::Result<Vec<MissingSearch>> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{COLUMNS} WHERE s.id IN (SELECT max(id) FROM missing_searches GROUP BY area)
         ORDER BY CASE s.area WHEN 'movies' THEN 0 WHEN 'series' THEN 1 ELSE 2 END"
    )))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(MissingSearch::from).collect())
}

/// Marks runs a restart cut off.
pub async fn recover(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE missing_searches SET state = 'failed', message = 'a restart cut it off',
             current = NULL, finished_at = ?
         WHERE state IN ('running', 'stopping')",
    )
    .bind(unix_now())
    .execute(pool)
    .await?;
    Ok(())
}
