//! Classical music to browse: composers, their works, and a work's
//! recordings side by side, movement by movement. Only classical works are listed (ones with
//! movements, or with a track in a classical genre) and the composers who have one; pop songs
//! have composers and works too, and would bury them.

use std::collections::{BTreeMap, HashMap};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ApiError;
use super::auth::CurrentUser;
use super::music::{
    ALBUM_COLUMNS, AlbumRow, Liked, MusicState, Shown, TRACK_COLUMNS, TrackRow, bad_request, ids,
    paging, pattern,
};

pub(super) fn routes() -> Router<MusicState> {
    Router::new()
        .route("/api/v1/music/composers", get(list_composers))
        .route("/api/v1/music/composers/{id}", get(composer))
        .route("/api/v1/music/works", get(list_works))
        .route("/api/v1/music/works/{id}", get(work))
}

/// Classical works with their present tracks, and their composers.
fn works_from(access: &crate::access::Access) -> String {
    format!(
        "FROM music_works w
         LEFT JOIN music_composers c ON c.id = w.composer_id
         JOIN music_track_works tw ON tw.work_id = w.id
         JOIN music_tracks t ON t.id = tw.track_id AND t.missing_since IS NULL AND {}
         JOIN music_albums a ON a.id = t.album_id AND a.missing_since IS NULL
         WHERE w.classical = 1",
        access.track_sql("t")
    )
}

#[derive(sqlx::FromRow)]
struct ComposerRow {
    id: i64,
    name: String,
    sort_name: Option<String>,
    mb_artist_id: Option<String>,
    works: i64,
    tracks: i64,
}

impl ComposerRow {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "sort_name": self.sort_name,
            "mb_artist_id": self.mb_artist_id,
            "works": self.works,
            "tracks": self.tracks,
        })
    }
}

const COMPOSER_COLUMNS: &str = "c.id, c.name, c.sort_name, c.mb_artist_id,
    count(DISTINCT w.id) AS works, count(DISTINCT t.id) AS tracks";

#[derive(Deserialize)]
struct ComposerQuery {
    /// Part of the name, case-insensitive.
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_composers(
    State(state): State<MusicState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ComposerQuery>,
) -> Result<Json<Value>, ApiError> {
    let from = works_from(&current.access);
    let (limit, offset) = paging(query.limit, query.offset)?;
    let q = pattern(query.q.as_deref());
    let filter = "AND c.id IS NOT NULL AND (?1 IS NULL OR c.name LIKE ?1 ESCAPE '\\')";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(DISTINCT c.id) {from} {filter}"
    )))
    .bind(&q)
    .fetch_one(&state.pool)
    .await?;
    let rows: Vec<ComposerRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COMPOSER_COLUMNS} {from} {filter} GROUP BY c.id
         ORDER BY coalesce(c.sort_name, c.name) COLLATE NOCASE, c.id LIMIT ?2 OFFSET ?3"
    )))
    .bind(&q)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!({
        "total": total,
        "items": rows.iter().map(ComposerRow::to_json).collect::<Vec<_>>(),
    })))
}

async fn composer(
    State(state): State<MusicState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let from = works_from(&current.access);
    let row: Option<ComposerRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COMPOSER_COLUMNS} {from} AND c.id = ? GROUP BY c.id"
    )))
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such composer"));
    };
    let works: Vec<WorkRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {WORK_COLUMNS} {from} AND w.composer_id = ? GROUP BY w.id
         ORDER BY w.title COLLATE NOCASE, w.id"
    )))
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let mut body = row.to_json();
    body["work_list"] = json!(works.iter().map(WorkRow::to_json).collect::<Vec<_>>());
    Ok(Json(body))
}

#[derive(sqlx::FromRow)]
struct WorkRow {
    id: i64,
    title: String,
    composer_id: Option<i64>,
    composer: Option<String>,
    mb_work_id: Option<String>,
    recordings: i64,
    tracks: i64,
}

impl WorkRow {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "composer_id": self.composer_id,
            "composer": self.composer,
            "mb_work_id": self.mb_work_id,
            "recordings": self.recordings,
            "tracks": self.tracks,
        })
    }
}

/// A recording of a work is one album's tracks of it.
const WORK_COLUMNS: &str = "w.id, w.title, w.composer_id, c.name AS composer, w.mb_work_id,
    count(DISTINCT t.album_id) AS recordings, count(DISTINCT t.id) AS tracks";

#[derive(Deserialize)]
struct WorkQuery {
    /// Part of the title or composer, case-insensitive.
    q: Option<String>,
    /// Comma-separated composer ids.
    composer_id: Option<String>,
    /// `composer` (default: by composer, then title) or `title`.
    sort: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_works(
    State(state): State<MusicState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<WorkQuery>,
) -> Result<Json<Value>, ApiError> {
    let from = works_from(&current.access);
    let (limit, offset) = paging(query.limit, query.offset)?;
    let composers = ids(query.composer_id.as_deref(), "composer_id")?;
    let order = match query.sort.as_deref().unwrap_or("composer") {
        "composer" => {
            "coalesce(c.sort_name, c.name) IS NULL, coalesce(c.sort_name, c.name) COLLATE NOCASE,
             w.title COLLATE NOCASE, w.id"
        }
        "title" => "w.title COLLATE NOCASE, w.id",
        _ => return Err(bad_request("sort is composer or title")),
    };
    let q = pattern(query.q.as_deref());
    let filter = "AND (?1 IS NULL OR w.title LIKE ?1 ESCAPE '\\' OR c.name LIKE ?1 ESCAPE '\\')
        AND (?2 IS NULL OR w.composer_id IN (SELECT value FROM json_each(?2)))";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(DISTINCT w.id) {from} {filter}"
    )))
    .bind(&q)
    .bind(&composers)
    .fetch_one(&state.pool)
    .await?;
    let rows: Vec<WorkRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {WORK_COLUMNS} {from} {filter} GROUP BY w.id ORDER BY {order}
         LIMIT ?3 OFFSET ?4"
    )))
    .bind(&q)
    .bind(&composers)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!({
        "total": total,
        "items": rows.iter().map(WorkRow::to_json).collect::<Vec<_>>(),
    })))
}

#[derive(sqlx::FromRow)]
struct PartRow {
    #[sqlx(flatten)]
    track: TrackRow,
    movement: Option<String>,
    movement_number: Option<i64>,
}

/// The most common of some names; the first of those tied.
fn most_common(names: &[&str]) -> Option<String> {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for name in names {
        match counts.iter_mut().find(|(n, _)| n == name) {
            Some((_, count)) => *count += 1,
            None => counts.push((name, 1)),
        }
    }
    let most = counts.iter().map(|(_, c)| *c).max()?;
    counts
        .into_iter()
        .find(|(_, c)| *c == most)
        .map(|(n, _)| n.to_owned())
}

/// A work with its movements and each recording of it (an album's tracks of the work), oldest
/// album first: movement by movement, to compare.
async fn work(
    State(state): State<MusicState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let from = works_from(&current.access);
    let row: Option<WorkRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {WORK_COLUMNS} {from} AND w.id = ? GROUP BY w.id"
    )))
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such work"));
    };
    let parts: Vec<PartRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TRACK_COLUMNS}, tw.movement, tw.movement_number
         FROM music_track_works tw
         JOIN music_tracks t ON t.id = tw.track_id AND t.missing_since IS NULL
         JOIN music_albums a ON a.id = t.album_id AND a.missing_since IS NULL
         WHERE tw.work_id = ? AND {}
         ORDER BY a.year IS NULL, a.year, a.title COLLATE NOCASE, a.id,
             tw.movement_number IS NULL, tw.movement_number, coalesce(t.disc_number, 1),
             t.track_number, t.rel_path",
        current.access.track_sql("t")
    )))
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let mut album_ids: Vec<i64> = parts.iter().map(|p| p.track.album_id).collect();
    album_ids.dedup();
    let albums: Vec<AlbumRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ALBUM_COLUMNS} FROM music_albums a
         LEFT JOIN mb_releases r ON r.id = a.match_release_id
         WHERE a.id IN (SELECT value FROM json_each(?))"
    )))
    .bind(json!(album_ids).to_string())
    .fetch_all(&state.pool)
    .await?;
    let shown = Shown::to(&state.pool, state.coverart.is_some(), current.user.id).await?;
    let albums: HashMap<i64, Value> = albums.iter().map(|a| (a.id, a.to_json(shown))).collect();

    // The movements, by number, with the name most recordings give each.
    let mut names: BTreeMap<i64, Vec<&str>> = BTreeMap::new();
    for part in &parts {
        if let Some(number) = part.movement_number {
            let entry = names.entry(number).or_default();
            if let Some(name) = &part.movement {
                entry.push(name);
            }
        }
    }
    let movements: Vec<Value> = names
        .iter()
        .map(|(number, names)| json!({ "number": number, "name": most_common(names) }))
        .collect();

    let liked = Liked::of(&state.pool, current.user.id).await?;
    let recordings: Vec<Value> = album_ids
        .iter()
        .filter_map(|album_id| {
            let tracks: Vec<(&PartRow, Value)> = parts
                .iter()
                .filter(|p| p.track.album_id == *album_id)
                .map(|p| (p, p.track.to_json(&liked)))
                .collect();
            let artists: Vec<&str> = tracks
                .iter()
                .filter_map(|(_, t)| t["artist"].as_str())
                .collect();
            Some(json!({
                "album": albums.get(album_id)?,
                // Who plays it: the artist most of its tracks name, else the album's.
                "artist": most_common(&artists),
                "tracks": tracks
                    .iter()
                    .map(|(p, track)| json!({
                        "movement_number": p.movement_number,
                        "movement": p.movement,
                        "track": track,
                    }))
                    .collect::<Vec<_>>(),
            }))
        })
        .collect();
    let mut body = row.to_json();
    body["movements"] = json!(movements);
    body["recording_list"] = json!(recordings);
    Ok(Json(body))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use sqlx::SqlitePool;
    use tower::ServiceExt;

    use super::*;

    /// Album, artist, work, movement, its number, composer.
    type Row<'a> = (
        i64,
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        Option<i64>,
        &'a str,
    );

    /// Two recordings of Beethoven's Fifth (one with his Seventh's first movement too), Satie's
    /// Gymnopédies with no movements or classical genre, and a pop song with a composer.
    async fn library() -> (SqlitePool, Router, String) {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
               INSERT INTO music_albums (id, library_id, key, title, artist_name, year, folder,
                   seen_generation) VALUES
                   (1, 1, 'k1', 'Symphony No. 5', 'Ludwig van Beethoven', 2021, 'a', 1),
                   (2, 1, 'k2', 'Symphonies 5 & 7', 'Ludwig van Beethoven', 1975, 'b', 1),
                   (3, 1, 'k3', 'Gymnopédies', 'Erik Satie', 1990, 'c', 1),
                   (4, 1, 'k4', 'Help!', 'The Beatles', 1965, 'd', 1);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let five = "Symphony No. 5 in C minor, Op. 67";
        let rows: [Row; 12] = [
            (
                1,
                "Wiener Philharmoniker",
                Some(five),
                Some("Allegro con brio"),
                Some(1),
                "Beethoven",
            ),
            (
                1,
                "Wiener Philharmoniker",
                Some(five),
                Some("Andante con moto"),
                Some(2),
                "Beethoven",
            ),
            (
                1,
                "Wiener Philharmoniker",
                Some(five),
                Some("Scherzo. Allegro"),
                Some(3),
                "Beethoven",
            ),
            (
                1,
                "Wiener Philharmoniker",
                Some(five),
                Some("Allegro"),
                Some(4),
                "Beethoven",
            ),
            (
                2,
                "Wiener Philharmoniker",
                Some(five),
                Some("Allegro con brio"),
                Some(1),
                "Beethoven",
            ),
            (
                2,
                "Wiener Philharmoniker",
                Some(five),
                Some("Andante con moto"),
                Some(2),
                "Beethoven",
            ),
            (
                2,
                "Wiener Philharmoniker",
                Some(five),
                Some("Allegro"),
                Some(3),
                "Beethoven",
            ),
            (
                2,
                "Wiener Philharmoniker",
                Some(five),
                Some("Allegro – Presto"),
                Some(4),
                "Beethoven",
            ),
            (
                2,
                "Wiener Philharmoniker",
                Some("Symphony No. 7 in A major, Op. 92"),
                Some("Poco sostenuto – Vivace"),
                Some(1),
                "Beethoven",
            ),
            (
                3,
                "Pascal Rogé",
                Some("Gymnopédies"),
                None,
                None,
                "Erik Satie",
            ),
            (
                3,
                "Pascal Rogé",
                Some("Gymnopédies"),
                None,
                None,
                "Erik Satie",
            ),
            (4, "The Beatles", Some("Help!"), None, None, "Lennon"),
        ];
        for (n, (album, artist, work, movement, number, composer)) in rows.into_iter().enumerate() {
            sqlx::query(
                "INSERT INTO music_tracks (library_id, rel_path, size, modified_at,
                     first_seen_generation, seen_generation, album_id, tagged, title, artist,
                     track_number, genres, composer, work, movement, movement_number)
                 VALUES (1, ?, 1, 1, 1, 1, ?, 1, ?, ?, ?, '[]', ?, ?, ?, ?)",
            )
            .bind(format!("{album}/{n}.flac"))
            .bind(album)
            .bind(movement.unwrap_or("Track"))
            .bind(artist)
            .bind(i64::try_from(n).unwrap() + 1)
            .bind(if composer == "Beethoven" {
                "Ludwig van Beethoven"
            } else {
                composer
            })
            .bind(work)
            .bind(movement)
            .bind(number)
            .execute(&pool)
            .await
            .unwrap();
        }
        crate::classical::refresh(&pool).await.unwrap();
        let user = crate::auth::create_user(&pool, "alice", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let app =
            super::super::music::routes(pool.clone(), "/nowhere".into(), "/nowhere".into(), None);
        (pool, app, token)
    }

    async fn get(app: &Router, token: &str, uri: &str) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::get(uri)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn browses_classical_composers_and_works_side_by_side() {
        let (pool, app, token) = library().await;
        let (status, composers) = get(&app, &token, "/api/v1/music/composers").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            composers["total"], 1,
            "Satie's and Lennon's works aren't classical here"
        );
        let beethoven = &composers["items"][0];
        assert_eq!(
            (
                &beethoven["name"],
                &beethoven["works"],
                &beethoven["tracks"]
            ),
            (&json!("Ludwig van Beethoven"), &json!(2), &json!(9))
        );
        let id = beethoven["id"].as_i64().unwrap();
        let (_, page) = get(&app, &token, &format!("/api/v1/music/composers/{id}")).await;
        let works: Vec<(&str, i64)> = page["work_list"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["title"].as_str().unwrap(),
                    w["recordings"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            works,
            [
                ("Symphony No. 5 in C minor, Op. 67", 2),
                ("Symphony No. 7 in A major, Op. 92", 1)
            ]
        );

        let (_, found) = get(&app, &token, "/api/v1/music/works?q=op.%2092").await;
        assert_eq!(found["total"], 1);
        let (_, all) = get(
            &app,
            &token,
            &format!("/api/v1/music/works?composer_id={id}&sort=title"),
        )
        .await;
        assert_eq!(all["total"], 2);
        let five = all["items"][0]["id"].as_i64().unwrap();

        let (status, work) = get(&app, &token, &format!("/api/v1/music/works/{five}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(work["composer"], "Ludwig van Beethoven");
        assert_eq!(work["movements"].as_array().unwrap().len(), 4);
        assert_eq!(
            work["movements"][0],
            json!({ "number": 1, "name": "Allegro con brio" })
        );
        // Recordings oldest first, each movement by movement; the Seventh's isn't among them.
        let recordings = work["recording_list"].as_array().unwrap();
        assert_eq!(recordings.len(), 2);
        assert_eq!(recordings[0]["album"]["year"], 1975);
        assert_eq!(recordings[0]["artist"], "Wiener Philharmoniker");
        let numbers: Vec<i64> = recordings[0]["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["movement_number"].as_i64().unwrap())
            .collect();
        assert_eq!(numbers, [1, 2, 3, 4]);
        assert_eq!(recordings[0]["tracks"][3]["movement"], "Allegro – Presto");
        assert_eq!(recordings[0]["tracks"][3]["track"]["album_id"], 2);

        // What isn't classical isn't browsed; a gone album's recording goes with it.
        let satie: i64 =
            sqlx::query_scalar("SELECT id FROM music_works WHERE title = 'Gymnopédies'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            get(&app, &token, &format!("/api/v1/music/works/{satie}"))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        sqlx::query("UPDATE music_albums SET missing_since = 1 WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();
        let (_, work) = get(&app, &token, &format!("/api/v1/music/works/{five}")).await;
        assert_eq!(work["recording_list"].as_array().unwrap().len(), 1);
        assert_eq!(work["recordings"], 1);

        for bad in [
            "/api/v1/music/works?composer_id=x",
            "/api/v1/music/works?sort=year",
            "/api/v1/music/composers?limit=0",
        ] {
            assert_eq!(
                get(&app, &token, bad).await.0,
                StatusCode::BAD_REQUEST,
                "{bad}"
            );
        }
        assert_eq!(
            get(&app, &token, "/api/v1/music/composers/999").await.0,
            StatusCode::NOT_FOUND
        );
    }
}
