//! Lists as they're recorded, with their last sync's result.

use super::*;

/// A list as stored, with how its last sync went.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct List {
    /// The list's row id.
    pub id: i64,
    /// Its name, 1 to 100 characters, unique among lists.
    pub name: String,
    /// Where it's read from: "tmdb", "imdb" or "rottentomatoes".
    pub provider: String,
    /// Which list: for TMDB a chart ("popular", "trending_week"…) or "list:<id>"; for IMDb
    /// "top", "popular" or a public list's "ls…" id; for Rotten Tomatoes a browse page's
    /// address.
    pub list: String,
    /// "movies" or "series".
    pub kind: String,
    /// The library its titles go in; one for its kind.
    pub library_id: i64,
    /// Whether titles it adds are monitored.
    pub monitored: bool,
    /// The Radarr or Sonarr quality profile its titles get, by name.
    pub quality_profile: String,
    /// How many titles from the top of the list are read, 1 to 250.
    pub max_items: i64,
    /// For series: which seasons a new title monitors, "all", "latest" or "none".
    pub monitor_seasons: String,
    /// Whether it's synced every 12 hours.
    pub enabled: bool,
    /// When it was last synced, in Unix seconds; `None` before its first sync.
    pub synced_at: Option<i64>,
    /// The last sync's outcome: what it found, added, already wanted and skipped (with why),
    /// or `{"error"}`; `None` before its first sync.
    #[sqlx(skip)]
    pub last_result: Option<Value>,
}

#[derive(sqlx::FromRow)]
pub(super) struct Row {
    #[sqlx(flatten)]
    pub(super) list: List,
    pub(super) last_result: Option<String>,
}

pub(super) const COLUMNS: &str = "SELECT id, name, provider, list, kind, library_id, monitored,
    quality_profile, max_items, monitor_seasons, enabled, synced_at, last_result FROM import_lists";

pub(super) fn view(row: Row) -> List {
    List {
        last_result: row.last_result.and_then(|r| serde_json::from_str(&r).ok()),
        ..row.list
    }
}

/// Every list, by name.
pub async fn all(pool: &SqlitePool) -> anyhow::Result<Vec<List>> {
    let rows: Vec<Row> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{COLUMNS} ORDER BY name, id")))
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(view).collect())
}

/// A list by its id; `None` when there's no such list.
pub async fn get(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<List>> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{COLUMNS} WHERE id = ?")))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(view))
}
