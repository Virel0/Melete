//! Albums: the list, one album with its tracks, and the columns other music routes share.

use super::*;

/// The columns of an album item, from `music_albums a LEFT JOIN mb_releases r`.
pub(in super::super) const ALBUM_COLUMNS: &str =
    "a.id, a.title, a.artist_id, a.artist_name, a.year, a.compilation,
    (SELECT count(*) FROM music_tracks t WHERE t.album_id = a.id AND t.missing_since IS NULL)
        AS tracks,
    (SELECT sum(t.duration_seconds) FROM music_tracks t
        WHERE t.album_id = a.id AND t.missing_since IS NULL) AS duration_seconds,
    a.cover_file IS NOT NULL OR EXISTS (SELECT 1 FROM music_tracks t
        WHERE t.album_id = a.id AND t.missing_since IS NULL AND t.has_picture = 1) AS own_cover,
    coalesce(r.cover_front, 0) AS archive_cover,
    a.motion_file IS NOT NULL AS motion,
    EXISTS (SELECT 1 FROM music_motion_covers m
        JOIN music_cover_colours mc ON mc.album_id = m.album_id
        WHERE m.album_id = a.id AND m.state = 'done' AND m.source = mc.source) AS generated_motion,
    (SELECT c.colours FROM music_cover_colours c WHERE c.album_id = a.id) AS colours,
    coalesce(a.match_release_id, a.mb_release_id) AS mb_release_id,
    coalesce(a.match_release_group_id, a.mb_release_group_id) AS mb_release_group_id,
    coalesce((SELECT ar.mb_artist_id FROM music_artists ar WHERE ar.id = a.artist_id),
        (SELECT json_extract(c.artist_credit, '$[0].id') FROM mb_releases c
         WHERE c.id = a.match_release_id AND json_array_length(c.artist_credit) = 1))
        AS mb_artist_id,
    a.match_status";

#[derive(sqlx::FromRow)]
pub(in super::super) struct AlbumRow {
    pub(in super::super) id: i64,
    pub(super) title: String,
    pub(super) artist_id: Option<i64>,
    pub(super) artist_name: String,
    pub(super) year: Option<i64>,
    pub(super) compilation: bool,
    pub(super) tracks: i64,
    pub(super) duration_seconds: Option<f64>,
    pub(super) own_cover: bool,
    pub(super) archive_cover: bool,
    pub(super) motion: bool,
    /// A motion cover made from the cover it has now.
    pub(super) generated_motion: bool,
    /// The cover's colours, as the cover-colouring job worked them out.
    pub(super) colours: Option<String>,
    pub(super) mb_release_id: Option<String>,
    pub(super) mb_release_group_id: Option<String>,
    /// The album artist's: from the tags, else from the matched release when one artist is
    /// credited.
    pub(super) mb_artist_id: Option<String>,
    pub(super) match_status: Option<String>,
}

/// What a caller sees of an album: the Cover Art Archive's covers (with MusicBrainz matching
/// on), and motion covers made from covers (while they're on, unless the caller turned them off
/// for themselves).
#[derive(Debug, Clone, Copy)]
pub(in super::super) struct Shown {
    pub(in super::super) archive: bool,
    pub(in super::super) generated_motion: bool,
}

impl Shown {
    pub(in super::super) async fn to(
        pool: &SqlitePool,
        archive: bool,
        user_id: i64,
    ) -> sqlx::Result<Shown> {
        Ok(Shown {
            archive,
            generated_motion: crate::motion::shown_to(pool, user_id).await?,
        })
    }
}

impl AlbumRow {
    pub(in super::super) fn to_json(&self, shown: Shown) -> Value {
        let has_cover = self.own_cover || (shown.archive && self.archive_cover);
        let motion = self.motion || (shown.generated_motion && self.generated_motion);
        json!({
            "id": self.id,
            "title": self.title,
            "artist_id": self.artist_id,
            "artist": self.artist_name,
            "year": self.year,
            "compilation": self.compilation,
            "tracks": self.tracks,
            "duration_seconds": self.duration_seconds,
            "cover": has_cover.then(|| format!("/api/v1/music/albums/{}/cover", self.id)),
            "motion_cover": motion.then(|| format!("/api/v1/music/albums/{}/motion", self.id)),
            // Only beside the cover they came from.
            "colours": self
                .colours
                .as_deref()
                .filter(|_| has_cover)
                .and_then(|c| serde_json::from_str::<Value>(c).ok()),
            "mb_release_id": self.mb_release_id,
            "mb_release_group_id": self.mb_release_group_id,
            "mb_artist_id": self.mb_artist_id,
            "match_status": self.match_status,
        })
    }
}

/// Present albums by id that `access` lets the user see, as the music API shows them; other
/// ids are left out.
pub(in super::super) async fn albums_by_id(
    pool: &SqlitePool,
    access: &Access,
    shown: Shown,
    ids: &[i64],
) -> sqlx::Result<std::collections::HashMap<i64, Value>> {
    let rows: Vec<AlbumRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ALBUM_COLUMNS} FROM music_albums a
         LEFT JOIN mb_releases r ON r.id = a.match_release_id
         WHERE a.missing_since IS NULL AND a.id IN (SELECT value FROM json_each(?)) AND {}",
        access.album_sql("a")
    )))
    .bind(serde_json::to_string(ids).unwrap_or_else(|_| "[]".into()))
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(|r| (r.id, r.to_json(shown))).collect())
}

#[derive(Deserialize)]
pub(super) struct AlbumQuery {
    /// Part of the title or artist; case, accents, spaces and punctuation aside.
    pub(super) q: Option<String>,
    /// Comma-separated artist ids.
    pub(super) artist_id: Option<String>,
    pub(super) genre: Option<String>,
    /// `artist` (default), `title`, `year` (newest first) or `recent`.
    pub(super) sort: Option<String>,
    pub(super) limit: Option<i64>,
    pub(super) offset: Option<i64>,
}

pub(super) async fn list_albums(
    State(state): State<MusicState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<AlbumQuery>,
) -> Result<Json<Value>, ApiError> {
    let (limit, offset) = paging(query.limit, query.offset)?;
    let artists = ids(query.artist_id.as_deref(), "artist_id")?;
    let order = match query.sort.as_deref().unwrap_or("artist") {
        "artist" => {
            "coalesce(a.artist_sort_key, a.artist_name) COLLATE NOCASE, a.year,
             coalesce(a.sort_key, a.title) COLLATE NOCASE, a.id"
        }
        "title" => "coalesce(a.sort_key, a.title) COLLATE NOCASE, a.id",
        "year" => "a.year IS NULL, a.year DESC, coalesce(a.sort_key, a.title) COLLATE NOCASE, a.id",
        "recent" => "a.id DESC",
        _ => return Err(bad_request("sort is artist, title, year or recent")),
    };
    let from = format!(
        "FROM music_albums a LEFT JOIN mb_releases r ON r.id = a.match_release_id
         WHERE a.missing_since IS NULL AND {}
           AND (?1 IS NULL OR a.title LIKE ?1 ESCAPE '\\' OR a.artist_name LIKE ?1 ESCAPE '\\'
                OR a.search_key LIKE ?4)
           AND (?2 IS NULL OR a.artist_id IN (SELECT value FROM json_each(?2)))
           AND (?3 IS NULL OR EXISTS (SELECT 1 FROM music_tracks t, json_each(t.genres) g
               WHERE t.album_id = a.id AND t.missing_since IS NULL AND g.value = ?3 COLLATE NOCASE))",
        current.access.album_sql("a")
    );
    let q = pattern(query.q.as_deref());
    let folded = crate::search_keys::pattern(query.q.as_deref());
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {from}")))
        .bind(&q)
        .bind(&artists)
        .bind(&query.genre)
        .bind(&folded)
        .fetch_one(&state.pool)
        .await?;
    let rows: Vec<AlbumRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ALBUM_COLUMNS} {from} ORDER BY {order} LIMIT ?5 OFFSET ?6"
    )))
    .bind(&q)
    .bind(&artists)
    .bind(&query.genre)
    .bind(&folded)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;
    let shown = Shown::to(&state.pool, state.coverart.is_some(), current.user.id).await?;
    Ok(Json(json!({
        "total": total,
        "items": rows.iter().map(|r| r.to_json(shown)).collect::<Vec<_>>(),
    })))
}

pub(super) async fn album(
    State(state): State<MusicState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    if !current.access.sees_album(&state.pool, id).await? {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such album"));
    }
    let row: Option<AlbumRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ALBUM_COLUMNS} FROM music_albums a
         LEFT JOIN mb_releases r ON r.id = a.match_release_id
         WHERE a.id = ? AND a.missing_since IS NULL"
    )))
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such album"));
    };
    let (library, release): (String, Option<String>) = sqlx::query_as(
        "SELECT l.name,
             (SELECT json_object('title', r.title, 'date', r.date, 'country', r.country,
                  'status', r.status, 'primary_type', r.primary_type,
                  'secondary_types', json(r.secondary_types),
                  'first_release_date', r.first_release_date)
              FROM mb_releases r WHERE r.id = a.match_release_id)
         FROM music_albums a JOIN libraries l ON l.id = a.library_id WHERE a.id = ?",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await?;
    let tracks = tracks_where(&state.pool, current.user.id, "t.album_id = ?1", Some(id)).await?;
    let mut genres: Vec<String> = Vec::new();
    for track in &tracks {
        for genre in track["genres"].as_array().into_iter().flatten() {
            let genre = genre.as_str().unwrap_or_default();
            if !genres.iter().any(|g| g.eq_ignore_ascii_case(genre)) {
                genres.push(genre.to_owned());
            }
        }
    }
    let shown = Shown::to(&state.pool, state.coverart.is_some(), current.user.id).await?;
    let mut body = row.to_json(shown);
    let liked =
        crate::aoide::likes::liked_albums(&state.pool, current.user.id, &current.access).await?;
    body["liked"] = json!(liked.iter().any(|(album, _)| *album == Some(id)));
    body["motion_cover_generation"] = json!(crate::motion::status(&state.pool, id).await?);
    body["library"] = json!(library);
    body["genres"] = json!(genres);
    body["release"] = match release {
        Some(release) => serde_json::from_str(&release)?,
        None => Value::Null,
    };
    body["track_list"] = json!(tracks);
    Ok(Json(body))
}

// ---------- Tracks ----------
