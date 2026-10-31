//! Artists and genres: the lists and one artist with their albums and the albums they appear on.

use super::*;

#[derive(Deserialize)]
pub(super) struct ArtistQuery {
    pub(super) q: Option<String>,
    /// `name` (default) or `recent` (newest album first).
    pub(super) sort: Option<String>,
    pub(super) limit: Option<i64>,
    pub(super) offset: Option<i64>,
}

#[derive(sqlx::FromRow)]
pub(super) struct ArtistRow {
    pub(super) id: i64,
    pub(super) name: String,
    pub(super) mb_artist_id: Option<String>,
    pub(super) albums: i64,
    /// Other artists' albums with a track crediting them.
    pub(super) appears_on: i64,
}

impl ArtistRow {
    pub(super) fn to_json(&self) -> Value {
        json!({ "id": self.id, "name": self.name, "mb_artist_id": self.mb_artist_id,
                "albums": self.albums, "appears_on": self.appears_on })
    }
}

/// An artist's MusicBrainz id: from the tags, else the one most of their matched albums'
/// releases credit alone.
pub(super) const ARTIST_MBID: &str = "coalesce(ar.mb_artist_id,
    (SELECT json_extract(c.artist_credit, '$[0].id') FROM music_albums a2
     JOIN mb_releases c ON c.id = a2.match_release_id
     WHERE a2.artist_id = ar.id AND a2.missing_since IS NULL
       AND json_array_length(c.artist_credit) = 1
     GROUP BY 1 ORDER BY count(*) DESC LIMIT 1)) AS mb_artist_id";

/// Whether album `a3` is one the artist `ar` appears on: present and seen by the user, not
/// their own, with a present track crediting them.
fn appears_on(access: &Access) -> String {
    format!(
        "a3.missing_since IS NULL AND a3.artist_id IS NOT ar.id AND {}
         AND EXISTS (SELECT 1 FROM music_track_artists ta
             JOIN music_tracks t3 ON t3.id = ta.track_id AND t3.missing_since IS NULL AND {}
             WHERE ta.artist_id = ar.id AND t3.album_id = a3.id)",
        access.album_sql("a3"),
        access.track_sql("t3")
    )
}

/// The columns of an artist item, over [`artist_from`], counting what the user may see.
pub(super) fn artist_columns(access: &Access) -> String {
    format!(
        "ar.id, ar.name, {ARTIST_MBID}, count(a.id) AS albums,
         (SELECT count(*) FROM music_albums a3 WHERE {}) AS appears_on",
        appears_on(access)
    )
}

/// Artists with an album in the library now that the user may see, or a track there crediting
/// them (a featured artist, a duet's other half). Callers add their conditions with `AND`.
pub(super) fn artist_from(access: &Access) -> String {
    format!(
        "FROM music_artists ar
         LEFT JOIN music_albums a ON a.artist_id = ar.id AND a.missing_since IS NULL AND {}
         WHERE (a.id IS NOT NULL OR EXISTS (SELECT 1 FROM music_track_artists ta
             JOIN music_tracks t ON t.id = ta.track_id AND t.missing_since IS NULL AND {}
             WHERE ta.artist_id = ar.id))",
        access.album_sql("a"),
        access.track_sql("t")
    )
}

/// The artist an id stands for now: the one it joined, for an artist known only by name whose
/// albums joined the MusicBrainz artist of that name; else itself.
async fn joined(pool: &SqlitePool, id: i64) -> sqlx::Result<i64> {
    let joined: Option<i64> =
        sqlx::query_scalar("SELECT merged_into FROM music_artists WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .flatten();
    Ok(joined.unwrap_or(id))
}

/// Artists in the library now that the user may see, by id (an artist that joined another by
/// the one it joined), as the music API shows them.
pub(in super::super) async fn artists_by_id(
    pool: &SqlitePool,
    access: &Access,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, Value>, ApiError> {
    let from = artist_from(access);
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT id, coalesce(merged_into, id) FROM music_artists
         WHERE id IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(ids).unwrap_or_else(|_| "[]".into()))
    .fetch_all(pool)
    .await?;
    let now: Vec<i64> = rows.iter().map(|(_, now)| *now).collect();
    let found: Vec<ArtistRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} {from}
         AND ar.id IN (SELECT value FROM json_each(?)) GROUP BY ar.id",
        artist_columns(access)
    )))
    .bind(serde_json::to_string(&now).unwrap_or_else(|_| "[]".into()))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .filter_map(|(asked, now)| {
            let row = found.iter().find(|r| r.id == *now)?;
            Some((*asked, row.to_json()))
        })
        .collect())
}

pub(super) async fn list_artists(
    State(state): State<MusicState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ArtistQuery>,
) -> Result<Json<Value>, ApiError> {
    let from = artist_from(&current.access);
    let (limit, offset) = paging(query.limit, query.offset)?;
    let order = match query.sort.as_deref().unwrap_or("name") {
        "name" => "coalesce(ar.sort_key, ar.name) COLLATE NOCASE, ar.id",
        "recent" => "max(a.id) DESC",
        _ => return Err(bad_request("sort is name or recent")),
    };
    let q = pattern(query.q.as_deref());
    let folded = crate::search_keys::pattern(query.q.as_deref());
    let filter = "AND (?1 IS NULL OR ar.name LIKE ?1 ESCAPE '\\' OR ar.search_key LIKE ?2)";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(DISTINCT ar.id) {from} {filter}"
    )))
    .bind(&q)
    .bind(&folded)
    .fetch_one(&state.pool)
    .await?;
    let rows: Vec<ArtistRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} {from} {filter}
         GROUP BY ar.id ORDER BY {order} LIMIT ?3 OFFSET ?4",
        artist_columns(&current.access)
    )))
    .bind(&q)
    .bind(&folded)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!({
        "total": total,
        "items": rows.iter().map(ArtistRow::to_json).collect::<Vec<_>>(),
    })))
}

pub(super) async fn artist(
    State(state): State<MusicState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    // An artist whose albums joined another answers as that one; its `id` says which.
    let id = joined(&state.pool, id).await?;
    let from = artist_from(&current.access);
    let row: Option<ArtistRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} {from} AND ar.id = ? GROUP BY ar.id",
        artist_columns(&current.access)
    )))
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such artist"));
    };
    let albums: Vec<AlbumRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ALBUM_COLUMNS} FROM music_albums a
         LEFT JOIN mb_releases r ON r.id = a.match_release_id
         WHERE a.artist_id = ? AND a.missing_since IS NULL AND {}
         ORDER BY a.year IS NULL, a.year, a.title COLLATE NOCASE",
        current.access.album_sql("a")
    )))
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let appears_on: Vec<AlbumRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ALBUM_COLUMNS} FROM music_albums a
         LEFT JOIN mb_releases r ON r.id = a.match_release_id
         WHERE a.id IN (SELECT a3.id FROM music_albums a3, music_artists ar
             WHERE ar.id = ? AND {})
         ORDER BY a.year IS NULL, a.year, a.title COLLATE NOCASE",
        appears_on(&current.access)
    )))
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let mut body = row.to_json();
    let shown = Shown::to(&state.pool, state.coverart.is_some(), current.user.id).await?;
    body["album_list"] = json!(albums.iter().map(|a| a.to_json(shown)).collect::<Vec<_>>());
    body["appears_on_list"] = json!(
        appears_on
            .iter()
            .map(|a| a.to_json(shown))
            .collect::<Vec<_>>()
    );
    Ok(Json(body))
}

/// Every genre with how many tracks and albums have it, most tracks first (one request for a
/// genre row, as for movies and series).
pub(super) async fn genres(
    State(state): State<MusicState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT min(g.value), count(DISTINCT t.id), count(DISTINCT t.album_id)
         FROM music_tracks t, json_each(t.genres) g
         WHERE t.missing_since IS NULL AND {}
         GROUP BY lower(g.value) ORDER BY count(DISTINCT t.id) DESC, lower(g.value)",
        current.access.track_sql("t")
    )))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!(
        rows.iter()
            .map(
                |(name, tracks, albums)| json!({ "name": name, "tracks": tracks, "albums": albums })
            )
            .collect::<Vec<_>>()
    )))
}

// ---------- Covers ----------
