//! A track's parts, for the apps' decks: what the separator made (the
//! voice for the whole track; drums, bass, guitar and other for its first and last minute and a
//! half)
//! with the sample each region starts at, and each part's FLAC, served as stored with ranges.
//! Playing `track + Σ (gain − 1) × part` over a region changes only that part, as the parts
//! add back to the track exactly.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use crate::stems;

#[derive(Clone)]
struct StemState {
    pool: SqlitePool,
    /// Where the parts are kept; None when the separator isn't set up.
    dir: Option<PathBuf>,
}

/// A track's parts, for everyone signed in. `dir` is where they're kept; None when the separator
/// isn't set up.
pub fn routes(pool: SqlitePool, dir: Option<PathBuf>) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/tracks/{id}/stems", get(parts))
        .route("/api/v1/music/tracks/{id}/stems/{file}", get(part))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(StemState { pool, dir })
}

fn none() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "this track hasn't been split into parts",
    )
}

/// The track's parts' folder and what the separator wrote about them, when it's been split.
async fn manifest(pool: &SqlitePool, id: i64) -> Result<Option<(String, Value)>, ApiError> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT s.folder, s.manifest FROM music_stems s JOIN music_tracks t ON t.id = s.track_id
         WHERE s.track_id = ? AND s.state = 'done' AND t.missing_since IS NULL
           AND s.size = t.size AND s.modified_at = t.modified_at",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some((folder, manifest)) => Some((
            folder,
            serde_json::from_str(&manifest).map_err(anyhow::Error::from)?,
        )),
        None => None,
    })
}

/// The files a manifest names: the voice, and each region's parts.
fn files(manifest: &Value) -> Vec<String> {
    let mut names: Vec<String> = manifest["voice"]
        .as_str()
        .map(str::to_owned)
        .into_iter()
        .collect();
    for region in manifest["regions"].as_array().into_iter().flatten() {
        for file in region["parts"]
            .as_object()
            .into_iter()
            .flat_map(|p| p.values())
        {
            if let Some(file) = file.as_str() {
                names.push(file.to_owned());
            }
        }
    }
    names
}

async fn parts(
    State(state): State<StemState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    if !current.access.sees_track(&state.pool, id).await? {
        return Err(none());
    }
    let Some((_, m)) = manifest(&state.pool, id).await? else {
        return Err(none());
    };
    let url = |file: &str| format!("/api/v1/music/tracks/{id}/stems/{file}");
    let regions: Vec<Value> = m["regions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            let parts: serde_json::Map<String, Value> = r["parts"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(part, file)| Some((part.clone(), json!(url(file.as_str()?)))))
                .collect();
            json!({ "name": r["name"], "start": r["start"], "end": r["end"], "parts": parts })
        })
        .collect();
    Ok(Json(json!({
        "rate": m["rate"],
        "channels": m["channels"],
        "frames": m["frames"],
        "bits": m["bits"],
        "voice": m["voice"].as_str().map(url),
        "regions": regions,
    })))
}

/// One part's FLAC, as the separator wrote it; ranges as asked.
async fn part(
    State(state): State<StemState>,
    Extension(current): Extension<CurrentUser>,
    Path((id, file)): Path<(i64, String)>,
    request: Request,
) -> Result<Response, ApiError> {
    if !current.access.sees_track(&state.pool, id).await? {
        return Err(none());
    }
    let (Some(dir), Some((folder, m))) = (&state.dir, manifest(&state.pool, id).await?) else {
        return Err(none());
    };
    // Only a file the separator named for this track: nothing else under the folder is served.
    if !files(&m).contains(&file) {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such part"));
    }
    let path = stems::parts_dir(dir, &folder).join(&file);
    // This MIME type and the Cache-Control value below are constants, so both always parse.
    let mut response = ServeFile::new_with_mime(&path, &"audio/flac".parse().expect("a MIME type"))
        .oneshot(request)
        .await
        .map_err(|err| anyhow::anyhow!("serving a part: {err}"))?
        .into_response();
    if response.status() == StatusCode::NOT_FOUND {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "the part's file is missing",
        ));
    }
    // The bytes behind a name change only when the track is split again; private, as it needs
    // a signed-in user.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "private, max-age=86400".parse().expect("a header"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use http_body_util::BodyExt;

    use super::*;
    use crate::auth::{self, TokenKind};

    async fn send(
        app: &Router,
        token: &str,
        uri: &str,
        range: Option<&str>,
    ) -> (StatusCode, Vec<u8>, Option<String>) {
        let mut request =
            HttpRequest::get(uri).header(header::AUTHORIZATION, format!("Bearer {token}"));
        if let Some(range) = range {
            request = request.header(header::RANGE, range);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let kind = response
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_owned());
        (
            status,
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
            kind,
        )
    }

    #[tokio::test]
    async fn serves_a_tracks_parts_and_nothing_else() {
        let pool = crate::db::open_in_memory().await;
        let user = auth::create_user(&pool, "alice", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let token = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap()
            .1;
        let manifest = json!({
            "version": 1, "rate": 44100, "channels": 2, "frames": 9000000, "bits": 16, "voice": "voice.flac",
            "regions": [
                {"name": "start", "start": 0, "end": 2646000, "parts": {"drums": "start_drums.flac", "bass": "start_bass.flac"}},
                {"name": "end", "start": 6354000, "end": 9000000, "parts": {"drums": "end_drums.flac"}},
            ],
        });
        sqlx::query(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k1', 'One', 'Alder', 'a', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, genres) VALUES
                 (7, 1, 'a/7', 10, 1, 1, 1, 1, 1, 'Seven', '[]'), (8, 1, 'a/8', 10, 1, 1, 1, 1, 1, 'Eight', '[]');
             INSERT INTO music_stems (track_id, state, size, modified_at, folder, manifest, queued_at, finished_at)
                 VALUES (7, 'done', 10, 1, '07/7', ?, 0, 1);"#,
        )
        .bind(manifest.to_string())
        .execute(&pool)
        .await
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("07/7");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("voice.flac"), b"fLaC-voice-bytes").unwrap();
        std::fs::write(folder.join("start_drums.flac"), b"fLaC-drums").unwrap();
        std::fs::write(folder.join("stems.json"), b"{}").unwrap();
        let app = routes(pool.clone(), Some(dir.path().to_owned()));

        let (status, body, _) = send(&app, &token, "/api/v1/music/tracks/7/stems", None).await;
        assert_eq!(status, StatusCode::OK);
        let view: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(view["voice"], "/api/v1/music/tracks/7/stems/voice.flac");
        assert_eq!(
            view["regions"][0]["parts"]["drums"],
            "/api/v1/music/tracks/7/stems/start_drums.flac"
        );
        assert_eq!(
            (&view["regions"][1]["start"], &view["frames"]),
            (&json!(6354000), &json!(9000000))
        );

        let (status, bytes, kind) = send(
            &app,
            &token,
            "/api/v1/music/tracks/7/stems/voice.flac",
            None,
        )
        .await;
        assert_eq!(
            (status, bytes.as_slice(), kind.as_deref()),
            (StatusCode::OK, &b"fLaC-voice-bytes"[..], Some("audio/flac"))
        );
        let (status, bytes, _) = send(
            &app,
            &token,
            "/api/v1/music/tracks/7/stems/voice.flac",
            Some("bytes=5-9"),
        )
        .await;
        assert_eq!(
            (status, bytes.as_slice()),
            (StatusCode::PARTIAL_CONTENT, &b"voice"[..])
        );

        for refused in [
            "/api/v1/music/tracks/7/stems/stems.json",
            "/api/v1/music/tracks/7/stems/..%2F..%2Fetc%2Fpasswd",
            "/api/v1/music/tracks/7/stems/end_drums.flac",
            "/api/v1/music/tracks/8/stems",
            "/api/v1/music/tracks/8/stems/voice.flac",
        ] {
            assert_eq!(
                send(&app, &token, refused, None).await.0,
                StatusCode::NOT_FOUND,
                "{refused}"
            );
        }
        // A file changed since it was split has no parts until it's split again.
        sqlx::query("UPDATE music_tracks SET size = 11 WHERE id = 7")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            send(&app, &token, "/api/v1/music/tracks/7/stems", None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        // Without the separator set up, nothing's served.
        let off = routes(pool, None);
        assert_eq!(
            send(
                &off,
                &token,
                "/api/v1/music/tracks/7/stems/voice.flac",
                None
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
}
