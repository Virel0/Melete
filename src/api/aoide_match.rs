//! `POST /aoide/match`: an imported track list matched against the music
//! library in one call, by the rules Aoide's apps share (`crate::aoide::matcher`).

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Extension, Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_music_sync};
use crate::aoide::matcher::{Imported, LibraryTrack, Matcher};

/// Rows per call; an import longer than this is split by the client.
const MAX_ROWS: usize = 5000;

/// The track-list matching route, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route(
            "/aoide/match",
            post(match_list).layer(DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route_layer(middleware::from_fn_with_state(
            auth.pool.clone(),
            super::access::require_all_music,
        ))
        .route_layer(middleware::from_fn_with_state(auth, require_music_sync))
        .with_state(pool)
}

#[derive(sqlx::FromRow)]
struct TrackRow {
    id: i64,
    title: String,
    artist: Option<String>,
    album_artist: String,
    album: String,
    duration_seconds: Option<f64>,
    isrc: Option<String>,
}

/// Every present track, with its own artist and its album's (each once).
async fn library(pool: &SqlitePool) -> sqlx::Result<Vec<LibraryTrack>> {
    let rows: Vec<TrackRow> = sqlx::query_as(
        "SELECT t.id, t.title, t.artist, a.artist_name AS album_artist, a.title AS album,
             t.duration_seconds, t.isrc
         FROM music_tracks t JOIN music_albums a ON a.id = t.album_id
         WHERE t.missing_since IS NULL AND a.missing_since IS NULL
         ORDER BY t.id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let mut artists: Vec<String> = Vec::new();
            for name in row.artist.into_iter().chain([row.album_artist]) {
                if !name.trim().is_empty() && !artists.iter().any(|a| a.eq_ignore_ascii_case(&name))
                {
                    artists.push(name);
                }
            }
            LibraryTrack {
                id: row.id.to_string(),
                title: Some(row.title),
                artists,
                album: Some(row.album),
                duration_ms: row.duration_seconds.map(|s| (s * 1000.0).round() as i64),
                isrc: row.isrc,
            }
        })
        .collect())
}

/// A row as sent; anything of the wrong type counts as absent.
fn imported(row: &Value) -> Imported {
    let text = |name: &str| row[name].as_str().map(str::to_owned);
    Imported {
        title: text("title"),
        artists: row["artists"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        duration_ms: row["durationMs"].as_i64(),
        isrc: text("isrc"),
    }
}

/// Results line up with the rows; an unmatched row is null. Each match carries the track's
/// own data and the three scores, so a surprising one can be explained. The track's id comes
/// as `trackId` and, for apps that still say it, `jellyfinId`.
async fn match_list(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let Value::Array(rows) = body else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "send a JSON array of rows",
        ));
    };
    if rows.len() > MAX_ROWS {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "{} rows is over the limit of {MAX_ROWS}; split the import",
                rows.len()
            ),
        ));
    }
    let tracks = library(&pool).await?;
    let user = current.user.id;
    // Seconds of work for a long list against a big library: off the async workers.
    let reply = tokio::task::spawn_blocking(move || match_rows(user, &tracks, &rows)).await?;
    Ok(Json(reply))
}

fn match_rows(user: i64, tracks: &[LibraryTrack], rows: &[Value]) -> Value {
    let matcher = Matcher::new(tracks);
    let mut matched = 0;
    let results: Vec<Value> = rows
        .iter()
        .map(|row| match matcher.find(&imported(row)) {
            Some(found) => {
                matched += 1;
                json!({
                    "trackId": found.track.id,
                    "jellyfinId": found.track.id,
                    "confidence": found.confidence,
                    "titleScore": found.title_score,
                    "artistScore": found.artist_score,
                    "durationScore": found.duration_score,
                    "title": found.track.title,
                    "artists": found.track.artists,
                    "album": found.track.album,
                    "durationMs": found.track.duration_ms,
                })
            }
            None => Value::Null,
        })
        .collect();
    tracing::info!(
        user,
        rows = rows.len(),
        matched,
        library = matcher.library_size(),
        "matched an imported track list"
    );
    json!({
        "results": results,
        "matched": matched,
        "librarySize": matcher.library_size(),
    })
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    async fn send(app: &Router, token: &str, body: String) -> (StatusCode, Value) {
        let request = Request::post("/aoide/match")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn matches_rows_in_order_against_present_tracks() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k', 'OK Computer', 'Radiohead', 'r', 1),
                        (2, 1, 'k2', 'Now 1', 'Various Artists', 'n', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, artist,
                 duration_seconds, isrc, missing_since) VALUES
                 (1, 1, 'r/1.flac', 1, 1, 1, 1, 1, 1, 'Karma Police', NULL, 264.07, NULL, NULL),
                 (2, 1, 'n/1.flac', 1, 1, 1, 1, 2, 1, 'Crazy In Love', 'Beyonce', 235.0, NULL, NULL),
                 (3, 1, 'r/2.flac', 1, 1, 1, 1, 1, 1, 'Gone', 'Radiohead', 200.0, NULL, 9);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "u", "a-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let app = routes(pool);
        let rows = json!([
            { "title": "Karma Police - Remastered 2009", "artists": ["Radiohead"], "durationMs": 264000 },
            { "title": "Nothing Like It", "artists": ["Nobody"] },
            null,
            { "title": "Crazy in Love (feat. Jay-Z)", "artists": ["Beyoncé", "JAY-Z"], "durationMs": 236000 },
            { "title": "Gone", "artists": ["Radiohead"] },
        ]);
        let (status, body) = send(&app, &token, rows.to_string()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            (body["matched"].clone(), body["librarySize"].clone()),
            (json!(2), json!(2))
        );
        let results = body["results"].as_array().unwrap();
        assert_eq!(results.len(), 5);
        assert_eq!(results[0]["trackId"], "1");
        assert_eq!(results[0]["jellyfinId"], "1");
        assert_eq!(
            results[0]["artists"],
            json!(["Radiohead"]),
            "the album's artist stands in"
        );
        assert_eq!(results[0]["durationMs"], 264070);
        assert_eq!(
            (results[1].clone(), results[2].clone(), results[4].clone()),
            (Value::Null, Value::Null, Value::Null)
        );
        assert_eq!(results[3]["artists"], json!(["Beyonce", "Various Artists"]));
        let spec: Value = serde_json::from_str(super::super::OPENAPI).unwrap();
        for (schema, value) in [("AoideMatchResponse", &body), ("AoideMatch", &results[0])] {
            let mut documented: Vec<&String> = spec["components"]["schemas"][schema]["properties"]
                .as_object()
                .unwrap()
                .keys()
                .collect();
            let mut actual: Vec<&String> = value.as_object().unwrap().keys().collect();
            documented.sort();
            actual.sort();
            assert_eq!(actual, documented, "{schema}");
        }

        assert_eq!(
            send(&app, &token, "{}".into()).await.0,
            StatusCode::BAD_REQUEST
        );
        let many = Value::Array(vec![json!({ "title": "x" }); MAX_ROWS + 1]).to_string();
        assert_eq!(send(&app, &token, many).await.0, StatusCode::BAD_REQUEST);
    }
}
