//! Tracks: looked up by id, listed, filtered and paged.

use super::*;

/// The tracks a user likes, from their sync log, for marking each track shown to them.
#[derive(Default)]
pub(in super::super) struct Liked(std::collections::HashSet<i64>);

impl Liked {
    /// What `user_id` likes now.
    pub(in super::super) async fn of(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Self> {
        Ok(Self(
            crate::aoide::likes::liked(pool, user_id)
                .await?
                .into_iter()
                .filter_map(|(track, _)| track)
                .collect(),
        ))
    }
}

#[derive(sqlx::FromRow)]
pub(in super::super) struct TrackRow {
    pub(super) id: i64,
    pub(super) title: String,
    pub(super) artist: Option<String>,
    pub(in super::super) album_id: i64,
    pub(super) album: String,
    pub(super) album_artist: String,
    pub(super) track_number: Option<i64>,
    pub(super) disc_number: Option<i64>,
    pub(super) year: Option<i64>,
    pub(super) duration_seconds: Option<f64>,
    pub(super) genres: String,
    pub(super) codec: Option<String>,
    pub(super) bitrate_kbps: Option<i64>,
    pub(super) sample_rate: Option<i64>,
    pub(super) bit_depth: Option<i64>,
    pub(super) channels: Option<i64>,
    /// ffprobe found Dolby Atmos.
    pub(super) atmos: Option<bool>,
    pub(super) isrc: Option<String>,
    pub(super) mb_recording_id: Option<String>,
    pub(super) mb_track_id: Option<String>,
    pub(super) mb_release_id: Option<String>,
    pub(super) mb_release_group_id: Option<String>,
    pub(super) mb_artist_id: Option<String>,
    /// Its artists in credit order, as JSON.
    pub(super) artists: String,
    pub(super) encoder_delay: Option<i64>,
    pub(super) padding: Option<i64>,
}

impl TrackRow {
    /// The track as the music API shows it to someone who likes `liked`.
    pub(in super::super) fn to_json(&self, liked: &Liked) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "artist": self.artist.as_deref().unwrap_or(&self.album_artist),
            "album_id": self.album_id,
            "album": self.album,
            "album_artist": self.album_artist,
            "track_number": self.track_number,
            "disc_number": self.disc_number,
            "year": self.year,
            "duration_seconds": self.duration_seconds,
            "genres": serde_json::from_str::<Value>(&self.genres).unwrap_or_else(|_| json!([])),
            "codec": self.codec,
            "bitrate_kbps": self.bitrate_kbps,
            "sample_rate": self.sample_rate,
            "bit_depth": self.bit_depth,
            "channels": self.channels,
            "spatial": crate::music_spatial::Spatial::of(self.atmos == Some(true), self.channels)
                .map(crate::music_spatial::Spatial::as_str),
            "isrc": self.isrc,
            "mb_recording_id": self.mb_recording_id,
            "mb_track_id": self.mb_track_id,
            "mb_release_id": self.mb_release_id,
            "mb_release_group_id": self.mb_release_group_id,
            "mb_artist_id": self.mb_artist_id,
            "artists": serde_json::from_str::<Value>(&self.artists).unwrap_or_else(|_| json!([])),
            "gapless": self.encoder_delay.map(|delay| json!({
                "encoder_delay": delay,
                "padding": self.padding,
            })),
            "liked": liked.0.contains(&self.id),
        })
    }
}

pub(in super::super) const TRACK_COLUMNS: &str =
    "t.id, t.title, t.artist, t.album_id, a.title AS album,
    a.artist_name AS album_artist, t.track_number, t.disc_number, coalesce(t.year, a.year) AS year,
    coalesce(t.duration_seconds, (SELECT p.duration_seconds FROM music_track_probes p
        WHERE p.track_id = t.id AND p.size = t.size AND p.modified_at = t.modified_at))
        AS duration_seconds,
    t.genres,
    coalesce(t.codec, (SELECT p.codec FROM music_track_probes p
        WHERE p.track_id = t.id AND p.size = t.size AND p.modified_at = t.modified_at)) AS codec,
    t.bitrate_kbps, t.sample_rate, t.bit_depth,
    coalesce((SELECT p.channels FROM music_track_probes p
        WHERE p.track_id = t.id AND p.size = t.size AND p.modified_at = t.modified_at),
        t.channels) AS channels,
    (SELECT p.atmos FROM music_track_probes p
        WHERE p.track_id = t.id AND p.size = t.size AND p.modified_at = t.modified_at) AS atmos,
    t.isrc,
    coalesce(t.mb_recording_id, t.match_recording_id) AS mb_recording_id,
    coalesce(t.mb_track_id, t.match_track_id) AS mb_track_id,
    coalesce(t.mb_release_id, a.match_release_id) AS mb_release_id,
    coalesce(t.mb_release_group_id, a.match_release_group_id, a.mb_release_group_id)
        AS mb_release_group_id,
    coalesce(t.mb_artist_id, CASE WHEN t.artist IS NULL OR t.artist = a.artist_name
        THEN coalesce(t.mb_album_artist_id,
            (SELECT ar.mb_artist_id FROM music_artists ar WHERE ar.id = a.artist_id),
            (SELECT json_extract(c.artist_credit, '$[0].id') FROM mb_releases c
             WHERE c.id = a.match_release_id AND json_array_length(c.artist_credit) = 1))
        END) AS mb_artist_id,
    (SELECT json_group_array(json_object('id', ar.id, 'name', ta.name,
            'mb_artist_id', ar.mb_artist_id) ORDER BY ta.position)
        FROM music_track_artists ta JOIN music_artists ar ON ar.id = ta.artist_id
        WHERE ta.track_id = t.id) AS artists,
    t.encoder_delay, t.padding";
pub(super) const TRACK_ORDER: &str =
    "coalesce(a.artist_sort_key, a.artist_name) COLLATE NOCASE, a.year,
    coalesce(a.sort_key, a.title) COLLATE NOCASE, a.id,
    coalesce(t.disc_number, 1), t.track_number, t.rel_path";

pub(super) async fn tracks_where(
    pool: &SqlitePool,
    me: i64,
    filter: &str,
    value: Option<i64>,
) -> Result<Vec<Value>, ApiError> {
    let rows: Vec<TrackRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TRACK_COLUMNS} FROM music_tracks t JOIN music_albums a ON a.id = t.album_id
         WHERE t.missing_since IS NULL AND {filter} ORDER BY {TRACK_ORDER}"
    )))
    .bind(value)
    .fetch_all(pool)
    .await?;
    let liked = Liked::of(pool, me).await?;
    Ok(rows.iter().map(|r| r.to_json(&liked)).collect())
}

/// Present tracks by id that the user may see, as the music API shows them to `me`; other ids
/// are left out.
pub(in super::super) async fn tracks_by_id(
    pool: &SqlitePool,
    access: &Access,
    me: i64,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, Value>, ApiError> {
    let mut found = track_values(pool, me, ids).await?;
    if !access.is_everything() {
        let mut hidden = Vec::new();
        for id in found.keys() {
            if !access.sees_track(pool, *id).await? {
                hidden.push(*id);
            }
        }
        for id in hidden {
            found.remove(&id);
        }
    }
    Ok(found)
}

/// [`tracks_by_id`], for callers outside a request.
pub(in super::super) async fn track_values(
    pool: &SqlitePool,
    me: i64,
    ids: &[i64],
) -> sqlx::Result<std::collections::HashMap<i64, Value>> {
    let rows: Vec<TrackRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TRACK_COLUMNS} FROM music_tracks t JOIN music_albums a ON a.id = t.album_id
         WHERE t.missing_since IS NULL AND t.id IN (SELECT value FROM json_each(?))"
    )))
    .bind(serde_json::to_string(ids).unwrap_or_else(|_| "[]".into()))
    .fetch_all(pool)
    .await?;
    let liked = Liked::of(pool, me).await?;
    Ok(rows.iter().map(|r| (r.id, r.to_json(&liked))).collect())
}

#[derive(Deserialize)]
pub(super) struct TrackQuery {
    /// Part of the title, artist or album, case-insensitive.
    pub(super) q: Option<String>,
    /// What `q` is matched against: `all` (default) or `track`, the track's own title and
    /// artists only, so an album's name doesn't bring every track on it.
    pub(super) q_in: Option<String>,
    /// Comma-separated track ids.
    pub(super) id: Option<String>,
    /// Comma-separated album ids.
    pub(super) album_id: Option<String>,
    /// Comma-separated artist ids: their albums' tracks, and the tracks crediting them.
    pub(super) artist_id: Option<String>,
    pub(super) genre: Option<String>,
    /// `album` (default: by artist, album, disc and number), `title` or `recent`.
    pub(super) sort: Option<String>,
    pub(super) limit: Option<i64>,
    pub(super) offset: Option<i64>,
}

pub(super) async fn list_tracks(
    State(state): State<MusicState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<TrackQuery>,
) -> Result<Json<Value>, ApiError> {
    let (limit, offset) = paging(query.limit, query.offset)?;
    let tracks = ids(query.id.as_deref(), "id")?;
    let albums = ids(query.album_id.as_deref(), "album_id")?;
    let artists = ids(query.artist_id.as_deref(), "artist_id")?;
    let text = match query.q_in.as_deref().unwrap_or("all") {
        "all" => {
            "t.title LIKE ?1 ESCAPE '\\' OR t.artist LIKE ?1 ESCAPE '\\'
             OR a.title LIKE ?1 ESCAPE '\\' OR t.search_key LIKE ?5 OR a.search_key LIKE ?5"
        }
        "track" => {
            "t.title LIKE ?1 ESCAPE '\\' OR t.artist LIKE ?1 ESCAPE '\\'
             OR t.album_artist LIKE ?1 ESCAPE '\\' OR a.artist_name LIKE ?1 ESCAPE '\\'
             OR t.search_key LIKE ?5 OR EXISTS (SELECT 1 FROM music_artists ar
                 WHERE ar.id = a.artist_id AND ar.search_key LIKE ?5)"
        }
        _ => return Err(bad_request("q_in is all or track")),
    };
    let order = match query.sort.as_deref().unwrap_or("album") {
        "album" => TRACK_ORDER,
        "title" => "coalesce(t.sort_key, t.title) COLLATE NOCASE, t.id",
        "recent" => "t.first_seen_generation DESC, t.id DESC",
        _ => return Err(bad_request("sort is album, title or recent")),
    };
    let from = format!(
        "FROM music_tracks t JOIN music_albums a ON a.id = t.album_id
         WHERE t.missing_since IS NULL AND a.missing_since IS NULL AND {}
           AND (?1 IS NULL OR {text})
           AND (?2 IS NULL OR t.album_id IN (SELECT value FROM json_each(?2)))
           AND (?3 IS NULL OR a.artist_id IN (SELECT value FROM json_each(?3))
                OR EXISTS (SELECT 1 FROM music_track_artists ta WHERE ta.track_id = t.id
                    AND ta.artist_id IN (SELECT value FROM json_each(?3))))
           AND (?4 IS NULL OR EXISTS (SELECT 1 FROM json_each(t.genres) g
               WHERE g.value = ?4 COLLATE NOCASE))
           AND (?6 IS NULL OR t.id IN (SELECT value FROM json_each(?6)))",
        current.access.track_sql("t"),
    );
    let q = pattern(query.q.as_deref());
    let folded = crate::search_keys::pattern(query.q.as_deref());
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {from}")))
        .bind(&q)
        .bind(&albums)
        .bind(&artists)
        .bind(&query.genre)
        .bind(&folded)
        .bind(&tracks)
        .fetch_one(&state.pool)
        .await?;
    let rows: Vec<TrackRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TRACK_COLUMNS} {from} ORDER BY {order} LIMIT ?7 OFFSET ?8"
    )))
    .bind(&q)
    .bind(&albums)
    .bind(&artists)
    .bind(&query.genre)
    .bind(&folded)
    .bind(&tracks)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;
    let liked = Liked::of(&state.pool, current.user.id).await?;
    Ok(Json(json!({
        "total": total,
        "items": rows.iter().map(|r| r.to_json(&liked)).collect::<Vec<_>>(),
    })))
}

// ---------- Artists and genres ----------
