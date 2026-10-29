//! Subtitle tracks for the apps: a file's list of embedded and external tracks, and text tracks
//! as WebVTT behind the stream link's token, so players can load them without headers.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::playback::{LINKS, STREAM_PREFIX, holder_sees, load_file, require_playable, resolve};
use crate::auth;
use crate::db::unix_now;
use crate::subtitles::{self, Sidecar, Source};

#[derive(Clone)]
struct SubtitleState {
    pool: SqlitePool,
    ffprobe: Arc<PathBuf>,
    ffmpeg: Arc<PathBuf>,
    cache: Arc<PathBuf>,
}

/// A file's subtitle list, for everyone signed in, and text tracks behind the stream link's own
/// token.
pub fn routes(pool: SqlitePool, ffprobe: PathBuf, ffmpeg: PathBuf, cache: PathBuf) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    let signed_in = Router::new()
        .route("/api/v1/files/{id}/subtitles", get(list))
        .route_layer(middleware::from_fn_with_state(auth, require_user));
    // The stream link's own token is the credential here.
    let tracks = Router::new().route("/api/v1/stream/{token}/subtitles/{track}", get(track));
    signed_in.merge(tracks).with_state(SubtitleState {
        pool,
        ffprobe: Arc::new(ffprobe),
        ffmpeg: Arc::new(ffmpeg),
        cache: Arc::new(cache),
    })
}

pub(super) use crate::subtitles::{embedded_key, sidecar_key};

pub(super) async fn sidecars_of(video: PathBuf) -> Result<Vec<Sidecar>, ApiError> {
    tokio::task::spawn_blocking(move || subtitles::sidecars(&video))
        .await?
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "the file's folder can't be read right now",
            )
        })
}

/// Every subtitle track of a file: embedded (`s{index}`) and files beside it (`x{n}`).
async fn list(
    State(state): State<SubtitleState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_playable(&state.pool, &current.access, id).await?;
    let file = load_file(&state.pool, &state.ffprobe, id).await?;
    let video = resolve(&file.root, &file.rel_path).await?;
    let mut tracks: Vec<Value> = file
        .media
        .iter()
        .flat_map(|m| &m.subtitles)
        .map(|s| {
            json!({
                "track": format!("s{}", s.index),
                "source": "embedded",
                "codec": s.codec,
                "language": s.language,
                "title": s.title,
                "default": s.default,
                "forced": s.forced,
                "hearing_impaired": false,
                "text": s.text,
                "file": null,
                "encoding": null,
            })
        })
        .collect();
    let fetched = crate::opensubtitles::fetched_for(&state.pool, id).await?;
    let sidecars = sidecars_of(video).await?;
    // Guessed from each file as converting it does, so a wrong guess can be seen.
    let paths: Vec<PathBuf> = sidecars.iter().map(|s| s.path.clone()).collect();
    let encodings: Vec<Option<&str>> = tokio::task::spawn_blocking(move || {
        paths.iter().map(|p| subtitles::file_encoding(p)).collect()
    })
    .await?;
    for (s, encoding) in sidecars.into_iter().zip(encodings) {
        tracks.push(json!({
                        "track": format!("x{}", s.id),
            "source": "external",
            "codec": s.codec,
            "language": s.language,
            "title": null,
            "default": false,
            "forced": s.forced,
            "hearing_impaired": s.hearing_impaired,
            "text": true,
            "file": s.file,
            "encoding": encoding,
        }));
    }
    let version = (file.id, file.size, file.modified_at);
    let fetched_paths: Vec<PathBuf> = fetched
        .iter()
        .map(|f| crate::opensubtitles::fetched_path(&state.cache, version, &f.language))
        .collect();
    let fetched_encodings: Vec<Option<&str>> = tokio::task::spawn_blocking(move || {
        fetched_paths
            .iter()
            .map(|p| subtitles::file_encoding(p))
            .collect()
    })
    .await?;
    for (f, encoding) in fetched.iter().zip(fetched_encodings) {
        tracks.push(super::subtitle_search::fetched_track(f, encoding));
    }
    Ok(Json(tracks))
}

/// One text track as WebVTT, for a stream link's file.
async fn track(
    State(state): State<SubtitleState>,
    Path((token, track)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    if !token.starts_with(STREAM_PREFIX) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "not a stream link"));
    }
    let session: Option<(i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT file_id, token_id FROM {LINKS} WHERE token_hash = ? AND expires_at > ?"
    )))
    .bind(auth::token_hash(&token))
    .bind(unix_now())
    .fetch_optional(&state.pool)
    .await?;
    let mut file_id = None;
    if let Some((id, token_id)) = session
        && holder_sees(&state.pool, token_id, id).await?
    {
        file_id = Some(id);
    }
    let Some(file_id) = file_id else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "this stream link is unknown or has expired; ask to play again",
        ));
    };
    let no_track = || ApiError::new(StatusCode::NOT_FOUND, "no such subtitle track");
    let name = track.strip_suffix(".vtt").ok_or_else(no_track)?;
    let file = load_file(&state.pool, &state.ffprobe, file_id).await?;
    let video = resolve(&file.root, &file.rel_path).await?;

    let vtt = if let Some(index) = name.strip_prefix('s').and_then(|i| i.parse::<i64>().ok()) {
        let stream = file
            .media
            .iter()
            .flat_map(|m| &m.subtitles)
            .find(|s| s.index == index)
            .ok_or_else(no_track)?;
        if !stream.text {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "picture subtitles can't be turned into text; burn them in when transcoding",
            ));
        }
        // Keyed to the file's version, so a replaced file is converted afresh.
        let key = embedded_key(file.id, index, file.size, file.modified_at);
        subtitles::webvtt(
            &state.ffmpeg,
            &state.cache,
            &key,
            Source::Embedded {
                video: &video,
                index,
            },
        )
        .await
    } else if name.starts_with('o') {
        let fetched = crate::opensubtitles::fetched_for(&state.pool, file.id).await?;
        let fetched = fetched
            .iter()
            .find(|f| crate::opensubtitles::track_id(&f.language) == name)
            .ok_or_else(no_track)?;
        let version = (file.id, file.size, file.modified_at);
        let path = crate::opensubtitles::fetched_path(&state.cache, version, &fetched.language);
        // Keyed to the video's version, as embedded tracks are.
        let key = format!("{}-{name}-{}-{}", file.id, file.size, file.modified_at);
        subtitles::webvtt(&state.ffmpeg, &state.cache, &key, Source::Sidecar(&path)).await
    } else if let Some(id) = name.strip_prefix('x') {
        let sidecars = sidecars_of(video).await?;
        let sidecar = sidecars.iter().find(|s| s.id == id).ok_or_else(no_track)?;
        let key = sidecar_key(file.id, sidecar);
        subtitles::webvtt(
            &state.ffmpeg,
            &state.cache,
            &key,
            Source::Sidecar(&sidecar.path),
        )
        .await
    } else {
        return Err(no_track());
    };
    let vtt = vtt.map_err(|err| {
        if err.is::<subtitles::Busy>() {
            return ApiError::new(StatusCode::SERVICE_UNAVAILABLE, err.to_string());
        }
        tracing::warn!(
            rel_path = file.rel_path,
            error = format!("{err:#}"),
            "subtitle conversion failed"
        );
        ApiError::new(StatusCode::BAD_GATEWAY, "converting the subtitles failed")
    })?;
    Ok(([(header::CONTENT_TYPE, "text/vtt; charset=utf-8")], vtt).into_response())
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::TokenKind;
    use crate::db;
    use crate::library::{LibraryConfig, LibraryKind, sync_config};

    struct Fixture {
        pool: SqlitePool,
        app: Router,
        token: String,
        token_id: i64,
        _dir: tempfile::TempDir,
    }

    /// File 1: the test clip (embedded forced French SRT at stream 2) with `clip.en.srt` beside it.
    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("library");
        std::fs::create_dir(&library).unwrap();
        let clip = crate::probe::testing::make_clip(dir.path());
        std::fs::rename(&clip, library.join("clip.mkv")).unwrap();
        std::fs::write(
            library.join("clip.en.srt"),
            "1\n00:00:00,500 --> 00:00:01,500\nHi there\n",
        )
        .unwrap();
        let size = std::fs::metadata(library.join("clip.mkv")).unwrap().len() as i64;

        let pool = db::open_in_memory().await;
        let config = LibraryConfig {
            name: "movies".into(),
            kind: LibraryKind::Movies,
            path: library,
        };
        sync_config(&pool, &[config]).await.unwrap();
        sqlx::query("INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation) VALUES (1, 1, 'clip.mkv', 'Clip', 1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation) VALUES (1, 1, 'clip.mkv', ?, 1, 1, 1, 1)",
        )
        .bind(size)
        .execute(&pool)
        .await
        .unwrap();
        let user = auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (token_id, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap();
        let app = routes(
            pool.clone(),
            "ffprobe".into(),
            "ffmpeg".into(),
            dir.path().join("cache"),
        );
        Fixture {
            pool,
            app,
            token,
            token_id,
            _dir: dir,
        }
    }

    async fn get(f: &Fixture, uri: &str, signed_in: bool) -> (StatusCode, Option<String>, Vec<u8>) {
        let mut request = Request::get(uri);
        if signed_in {
            request = request.header(header::AUTHORIZATION, format!("Bearer {}", f.token));
        }
        let response = f
            .app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_owned());
        (
            status,
            content_type,
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
    }

    async fn stream_token(f: &Fixture) -> String {
        let Ok((token, _)) =
            super::super::playback::create_play_session(&f.pool, f.token_id, 1, None).await
        else {
            panic!("creating a stream link failed");
        };
        token
    }

    #[tokio::test]
    async fn lists_embedded_and_external_tracks() {
        let f = fixture().await;
        let (status, _, body) = get(&f, "/api/v1/files/1/subtitles", true).await;
        assert_eq!(status, StatusCode::OK);
        let tracks: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            tracks,
            json!([
                { "track": "s2", "source": "embedded", "codec": "subrip", "language": "fre", "title": null,
                  "default": false, "forced": true, "hearing_impaired": false, "text": true, "file": null,
                  "encoding": null },
                { "track": "xed0587fb", "source": "external", "codec": "subrip", "language": "en", "title": null,
                  "default": false, "forced": false, "hearing_impaired": false, "text": true, "file": "clip.en.srt",
                  "encoding": "UTF-8" },
            ])
        );
        assert_eq!(
            get(&f, "/api/v1/files/1/subtitles", false).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn serves_text_tracks_as_webvtt_behind_the_stream_token() {
        let f = fixture().await;
        let token = stream_token(&f).await;

        let (status, content_type, body) = get(
            &f,
            &format!("/api/v1/stream/{token}/subtitles/s2.vtt"),
            false,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type.as_deref(), Some("text/vtt; charset=utf-8"));
        let embedded = String::from_utf8(body).unwrap();
        assert!(
            embedded.starts_with("WEBVTT") && embedded.contains("Hello"),
            "{embedded}"
        );

        let (_, _, body) = get(
            &f,
            &format!("/api/v1/stream/{token}/subtitles/xed0587fb.vtt"),
            false,
        )
        .await;
        let external = String::from_utf8(body).unwrap();
        assert!(
            external.contains("00:00.500 --> 00:01.500") && external.contains("Hi there"),
            "{external}"
        );
    }

    #[tokio::test]
    async fn refuses_unknown_tracks_picture_tracks_and_bad_links() {
        let f = fixture().await;
        let token = stream_token(&f).await;
        for track in [
            "s0.vtt",
            "s9.vtt",
            "x00000000.vtt",
            "s2.srt",
            "y0.vtt",
            "../x.vtt",
        ] {
            let uri = format!("/api/v1/stream/{token}/subtitles/{track}");
            assert_eq!(
                get(&f, &uri, false).await.0,
                StatusCode::NOT_FOUND,
                "{track}"
            );
        }
        assert_eq!(
            get(&f, "/api/v1/stream/mlt_x/subtitles/s2.vtt", false)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            get(&f, "/api/v1/stream/mls_unknown/subtitles/s2.vtt", false)
                .await
                .0,
            StatusCode::NOT_FOUND
        );

        // A picture track (PGS), as ffprobe would describe it.
        let (size,): (i64,) = sqlx::query_as("SELECT size FROM media_files WHERE id = 1")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        let info = json!({ "container": "matroska,webm", "duration_seconds": 2.0, "bit_rate": null,
            "video": [], "audio": [],
            "subtitles": [{ "index": 3, "codec": "hdmv_pgs_subtitle", "language": "eng", "title": null,
                            "default": false, "forced": false, "text": false }] });
        sqlx::query("INSERT OR REPLACE INTO media_probes (file_id, size, modified_at, probed_at, info) VALUES (1, ?, 1, 1, ?)")
            .bind(size)
            .bind(info.to_string())
            .execute(&f.pool)
            .await
            .unwrap();
        let (status, _, body) = get(
            &f,
            &format!("/api/v1/stream/{token}/subtitles/s3.vtt"),
            false,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(String::from_utf8(body).unwrap().contains("burn them in"));

        sqlx::query("DELETE FROM tokens WHERE id = ?")
            .bind(f.token_id)
            .execute(&f.pool)
            .await
            .unwrap();
        let uri = format!("/api/v1/stream/{token}/subtitles/xed0587fb.vtt");
        assert_eq!(
            get(&f, &uri, false).await.0,
            StatusCode::NOT_FOUND,
            "signing out ends it"
        );
    }
}
