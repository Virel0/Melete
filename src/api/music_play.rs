//! Playing music. A play request gives a stream link for one track, as it is
//! or converted for the device (ALAC or FLAC keep lossless exact; AAC, Opus, MP3), with what a
//! player needs for gapless joins. Links carry their own token, last a day, and end when
//! the session that asked signs out, as video's do.

use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tower_http::services::ServeFile;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_playback};
use super::playback::{MAX_STREAMS_PER_TOKEN, STREAM_LIFETIME, STREAM_PREFIX, resolve};
use crate::auth;
use crate::db::unix_now;
use crate::library::gapless::{self, Gapless};
use crate::music_convert::{Converter, Format};
use crate::music_spatial::{self, Spatial};

#[derive(Clone)]
struct PlayState {
    pool: SqlitePool,
    converter: Arc<Converter>,
}

/// Asking to play a track, for everyone signed in, and the stream link it gives, which carries
/// its own token.
pub fn routes(pool: SqlitePool, converter: Arc<Converter>) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    let state = PlayState { pool, converter };
    let play = Router::new()
        .route("/api/v1/music/tracks/{id}/play", post(play))
        .route_layer(middleware::from_fn_with_state(auth, require_playback))
        .with_state(state.clone());
    // The link is the credential: players that can't send headers use it as it is.
    let stream = Router::new()
        .route("/api/v1/music/stream/{token}", get(stream))
        .with_state(state);
    play.merge(stream)
}

#[derive(Deserialize, Default)]
struct PlayRequest {
    /// `original` (default), `alac`, `flac`, `aac`, `opus` or `mp3`.
    format: Option<String>,
    /// For AAC, Opus and MP3: 64 to 320.
    bitrate_kbps: Option<i64>,
    /// The device plays Dolby Atmos and multichannel audio. Without it, such a track is
    /// folded into stereo.
    #[serde(default)]
    spatial: bool,
}

#[derive(sqlx::FromRow)]
struct TrackFile {
    root: String,
    rel_path: String,
    size: i64,
    modified_at: i64,
    duration_seconds: Option<f64>,
    encoder_delay: Option<i64>,
    padding: Option<i64>,
    codec: Option<String>,
    channels: Option<i64>,
    atmos: Option<bool>,
}

fn gapless_json(gapless: Option<Gapless>) -> Value {
    match gapless {
        Some(g) => json!({ "encoder_delay": g.encoder_delay, "padding": g.padding }),
        None => Value::Null,
    }
}

/// The MIME type of what's served: the converted format's, else the original file's by its
/// extension.
fn content_type(format: Option<Format>, rel_path: &str) -> &'static str {
    let extension = match format {
        Some(format) => format.extension(),
        None => rel_path.rsplit_once('.').map_or("", |(_, e)| e),
    };
    match extension.to_ascii_lowercase().as_str() {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" => "audio/mp4",
        "ec3" | "eac3" => "audio/eac3",
        "ac3" => "audio/ac3",
        "thd" | "mlp" => "audio/vnd.dolby.mlp",
        "mka" => "audio/x-matroska",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "aif" | "aiff" => "audio/aiff",
        "wv" => "audio/x-wavpack",
        "ape" => "audio/x-ape",
        "mpc" => "audio/x-musepack",
        _ => "application/octet-stream",
    }
}

async fn play(
    State(state): State<PlayState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    body: Option<Json<PlayRequest>>,
) -> Result<Json<Value>, ApiError> {
    let request = body.map(|Json(b)| b).unwrap_or_default();
    let (format, kbps) = requested_format(&request)?;
    if !current.access.sees_track(&state.pool, id).await? {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such track"));
    }
    let track = track_file(&state.pool, id).await?;
    // Dolby Atmos and multichannel tracks go as they are only to devices that play them; for
    // the rest they're folded into stereo: FLAC from a lossless source, else AAC, unless the
    // device asked for a format of its own.
    let spatial = Spatial::of(track.atmos == Some(true), track.channels);
    let stereo = spatial.is_some() && !request.spatial;
    let (format, kbps) = match format {
        None if stereo => {
            let lossless = track.codec.as_deref().is_some_and(music_spatial::lossless);
            let format = if lossless { Format::Flac } else { Format::Aac };
            (Some(format), format.default_kbps())
        }
        _ => (format, kbps),
    };
    let served = match (format, stereo) {
        (_, true) => None,
        (None, false) => spatial,
        // A conversion keeps the channels but not Atmos, which no encoder here writes.
        (Some(_), false) => Spatial::of(false, track.channels),
    };
    let (converted, gapless) = match format {
        None => (
            None,
            track.encoder_delay.map(|encoder_delay| Gapless {
                encoder_delay,
                padding: track.padding,
            }),
        ),
        Some(format) => {
            let (name, gapless) = convert(&state, id, &track, format, kbps, stereo).await?;
            (Some(name), gapless)
        }
    };

    let (token, expires_at) =
        new_stream_link(&state.pool, current.token_id, id, format, &converted).await?;
    Ok(Json(json!({
        "stream_url": format!("/api/v1/music/stream/{token}"),
        "expires_at": expires_at,
        "format": format.map_or("original", Format::as_str),
        "content_type": content_type(format, &track.rel_path),
        "duration_seconds": track.duration_seconds,
        "gapless": gapless_json(gapless),
        "spatial": served.map(Spatial::as_str),
    })))
}

/// The format and bitrate the device asked for, or why they won't do.
fn requested_format(request: &PlayRequest) -> Result<(Option<Format>, Option<i64>), ApiError> {
    let bad = |message: &str| ApiError::new(StatusCode::BAD_REQUEST, message);
    let format = match request.format.as_deref().unwrap_or("original") {
        "original" => None,
        name => Some(
            Format::parse(name)
                .ok_or_else(|| bad("format is original, alac, flac, aac, opus or mp3"))?,
        ),
    };
    let kbps = match (format, request.bitrate_kbps) {
        (Some(f), Some(kbps)) if !f.lossless() && (64..=320).contains(&kbps) => Some(kbps),
        (Some(f), None) if !f.lossless() => f.default_kbps(),
        (_, None) => None,
        (Some(f), Some(_)) if !f.lossless() => return Err(bad("bitrate_kbps is 64 to 320")),
        (_, Some(_)) => {
            return Err(bad(
                "bitrate_kbps is only for aac, opus and mp3; the original and lossless formats keep every bit",
            ));
        }
    };
    Ok((format, kbps))
}

/// The track's file and what's known of its sound, unless it's missing.
async fn track_file(pool: &SqlitePool, id: i64) -> Result<TrackFile, ApiError> {
    let track: Option<TrackFile> = sqlx::query_as(
        "SELECT l.path AS root, t.rel_path, t.size, t.modified_at,
             coalesce(t.duration_seconds, p.duration_seconds) AS duration_seconds,
             t.encoder_delay, t.padding, coalesce(t.codec, p.codec) AS codec,
             coalesce(p.channels, t.channels) AS channels, p.atmos
         FROM music_tracks t JOIN libraries l ON l.id = t.library_id
         LEFT JOIN music_track_probes p
             ON p.track_id = t.id AND p.size = t.size AND p.modified_at = t.modified_at
         WHERE t.id = ? AND t.missing_since IS NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some(track) = track else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such track"));
    };
    Ok(track)
}

/// Converts the track for the device, the conversion named by the file's version, with what
/// the converted file says for gapless joins.
async fn convert(
    state: &PlayState,
    id: i64,
    track: &TrackFile,
    format: Format,
    kbps: Option<i64>,
    stereo: bool,
) -> Result<(String, Option<Gapless>), ApiError> {
    let source = resolve(&track.root, &track.rel_path).await?;
    // A new version of the file gets a new conversion.
    let digest = Sha256::digest(format!(
        "{}\0{}\0{}",
        track.rel_path, track.size, track.modified_at
    ));
    let version: String = digest[..6].iter().map(|b| format!("{b:02x}")).collect();
    let name = state
        .converter
        .convert_channels(
            &source,
            &format!("track{id}-{version}"),
            format,
            kbps,
            stereo,
        )
        .await
        .map_err(|err| {
            tracing::warn!(
                track = id,
                error = format!("{err:#}"),
                "couldn't convert a track"
            );
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the track couldn't be converted",
            )
        })?;
    let path = state.converter.path(&name);
    let codec = match format {
        Format::Alac => "alac",
        Format::Flac => "flac",
        Format::Aac => "aac",
        Format::Opus => "opus",
        Format::Mp3 => "mp3",
    };
    let gapless = tokio::task::spawn_blocking(move || gapless::read(&path, Some(codec)))
        .await
        .map_err(|err| anyhow::anyhow!(err))?;
    Ok((name, gapless))
}

/// A new stream link for the track, after forgetting expired ones; a session keeps only its
/// latest links. The link's token, and when it expires.
async fn new_stream_link(
    pool: &SqlitePool,
    token_id: i64,
    id: i64,
    format: Option<Format>,
    converted: &Option<String>,
) -> Result<(String, i64), ApiError> {
    let now = unix_now();
    sqlx::query("DELETE FROM music_streams WHERE expires_at <= ?")
        .bind(now)
        .execute(pool)
        .await?;
    let token = auth::random_token(STREAM_PREFIX)?;
    let expires_at = now + STREAM_LIFETIME;
    sqlx::query(
        "INSERT INTO music_streams (token_hash, token_id, track_id, format, converted, created_at,
             expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(auth::token_hash(&token))
    .bind(token_id)
    .bind(id)
    .bind(format.map_or("original", Format::as_str))
    .bind(converted)
    .bind(now)
    .bind(expires_at)
    .execute(pool)
    .await?;
    // A device that asks again and again keeps only its latest links.
    sqlx::query(
        "DELETE FROM music_streams WHERE token_id = ?1 AND id NOT IN
             (SELECT id FROM music_streams WHERE token_id = ?1 ORDER BY id DESC LIMIT ?2)",
    )
    .bind(token_id)
    .bind(MAX_STREAMS_PER_TOKEN)
    .execute(pool)
    .await?;
    Ok((token, expires_at))
}

async fn stream(
    State(state): State<PlayState>,
    Path(token): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    let gone = || {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "this stream link is unknown or has expired; ask to play again",
        )
    };
    if !token.starts_with(STREAM_PREFIX) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "not a stream link"));
    }
    let row: Option<(String, String, String, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT l.path, t.rel_path, s.format, s.converted, s.token_id, s.track_id
         FROM music_streams s
         JOIN music_tracks t ON t.id = s.track_id
         JOIN libraries l ON l.id = t.library_id
         WHERE s.token_hash = ? AND s.expires_at > ? AND t.missing_since IS NULL",
    )
    .bind(auth::token_hash(&token))
    .bind(unix_now())
    .fetch_optional(&state.pool)
    .await?;
    let Some((root, rel_path, format, converted, token_id, track_id)) = row else {
        return Err(gone());
    };
    let holder = super::playback::holder_access(&state.pool, token_id).await?;
    match holder {
        Some(access) if access.sees_track(&state.pool, track_id).await? => {}
        _ => return Err(gone()),
    }
    let format = Format::parse(&format);
    let path = match &converted {
        Some(name) => {
            let path = state.converter.path(name);
            if tokio::fs::metadata(&path).await.is_err() {
                return Err(gone());
            }
            path
        }
        None => resolve(&root, &rel_path).await?,
    };
    let mut response = ServeFile::new(path)
        .try_call(request)
        .await?
        .map(axum::body::Body::new);
    if response.status().is_success() {
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(content_type(format, &rel_path)),
        );
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    struct Fixture {
        app: Router,
        pool: SqlitePool,
        token: String,
        transcode: tempfile::TempDir,
        _library: tempfile::TempDir,
    }

    /// Track 1 is a FLAC tone (gapless 0 and 0), track 2 the same but missing.
    async fn fixture() -> Fixture {
        let library = tempfile::tempdir().unwrap();
        let path = library.path().join("a/01.flac");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let status = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=d=2"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let pool = crate::db::open_in_memory().await;
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', ?)")
            .bind(library.path().to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(
            "INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'dir:a', 'A', 'X', 'a', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, duration_seconds,
                 encoder_delay, padding, missing_since) VALUES
                 (1, 1, 'a/01.flac', 1, 1, 1, 1, 1, 1, 'One', 2.0, 0, 0, NULL),
                 (2, 1, 'a/02.flac', 1, 1, 1, 1, 1, 1, 'Two', 2.0, 0, 0, 5);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let transcode = tempfile::tempdir().unwrap();
        let converter = Arc::new(Converter::new("ffmpeg".into(), transcode.path()));
        Fixture {
            app: routes(pool.clone(), converter),
            pool,
            token,
            transcode,
            _library: library,
        }
    }

    async fn play(f: &Fixture, id: i64, body: Option<Value>) -> (StatusCode, Value) {
        let mut request = Request::post(format!("/api/v1/music/tracks/{id}/play"))
            .header(header::AUTHORIZATION, format!("Bearer {}", f.token));
        let body = match body {
            Some(body) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(body.to_string())
            }
            None => Body::empty(),
        };
        let response = f
            .app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn fetch(f: &Fixture, url: &str, range: Option<&str>) -> Response {
        let mut request = Request::get(url);
        if let Some(range) = range {
            request = request.header(header::RANGE, range);
        }
        f.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn plays_the_original_with_byte_ranges() {
        let f = fixture().await;
        let (status, body) = play(&f, 1, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let spec: Value = serde_json::from_str(super::super::OPENAPI).unwrap();
        let documented: BTreeSet<&str> =
            spec["components"]["schemas"]["MusicPlayResponse"]["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
        let actual: BTreeSet<&str> = body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(actual, documented);
        assert_eq!(body["format"], "original");
        assert_eq!(body["content_type"], "audio/flac");
        assert_eq!(body["gapless"], json!({ "encoder_delay": 0, "padding": 0 }));
        let url = body["stream_url"].as_str().unwrap();
        let part = fetch(&f, url, Some("bytes=0-3")).await;
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(part.headers()[header::CONTENT_TYPE], "audio/flac");
        let bytes = part.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"fLaC");
    }

    #[tokio::test]
    async fn converts_for_the_device_once_and_says_how_to_join_it() {
        let f = fixture().await;
        let (_, mp3) = play(&f, 1, Some(json!({ "format": "mp3", "bitrate_kbps": 128 }))).await;
        assert_eq!(mp3["format"], "mp3");
        assert_eq!(mp3["content_type"], "audio/mpeg");
        assert_eq!(mp3["gapless"]["encoder_delay"], 576 + 529);
        let whole = fetch(&f, mp3["stream_url"].as_str().unwrap(), None).await;
        assert_eq!(whole.status(), StatusCode::OK);
        assert_eq!(whole.headers()[header::CONTENT_TYPE], "audio/mpeg");

        let (_, alac) = play(&f, 1, Some(json!({ "format": "alac" }))).await;
        assert_eq!(alac["content_type"], "audio/mp4");
        assert_eq!(alac["gapless"], json!({ "encoder_delay": 0, "padding": 0 }));
        let (_, opus) = play(&f, 1, Some(json!({ "format": "opus" }))).await;
        assert_eq!(
            opus["gapless"],
            json!({ "encoder_delay": 312, "padding": null })
        );
        let (_, aac) = play(&f, 1, Some(json!({ "format": "aac" }))).await;
        assert_eq!(aac["gapless"], Value::Null);

        // Asking again reuses the conversion.
        play(&f, 1, Some(json!({ "format": "mp3", "bitrate_kbps": 128 }))).await;
        let made = std::fs::read_dir(f.transcode.path().join("music"))
            .unwrap()
            .count();
        assert_eq!(made, 4);

        // A conversion cleaned away ends its links; asking again makes it anew.
        for entry in std::fs::read_dir(f.transcode.path().join("music")).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
        let gone = fetch(&f, mp3["stream_url"].as_str().unwrap(), None).await;
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
        let (status, _) = play(&f, 1, Some(json!({ "format": "mp3", "bitrate_kbps": 128 }))).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn refuses_what_it_cant_do() {
        let f = fixture().await;
        for body in [
            json!({ "format": "wma" }),
            json!({ "format": "flac", "bitrate_kbps": 128 }),
            json!({ "bitrate_kbps": 128 }),
            json!({ "format": "aac", "bitrate_kbps": 500 }),
        ] {
            assert_eq!(
                play(&f, 1, Some(body.clone())).await.0,
                StatusCode::BAD_REQUEST,
                "{body}"
            );
        }
        assert_eq!(
            play(&f, 2, None).await.0,
            StatusCode::NOT_FOUND,
            "a missing track"
        );
        assert_eq!(play(&f, 9, None).await.0, StatusCode::NOT_FOUND);
        assert_eq!(
            fetch(&f, "/api/v1/music/stream/mls_nope", None)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            fetch(&f, "/api/v1/music/stream/other", None).await.status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn links_end_when_the_session_signs_out() {
        let f = fixture().await;
        let (_, body) = play(&f, 1, None).await;
        let url = body["stream_url"].as_str().unwrap().to_owned();
        assert_eq!(fetch(&f, &url, None).await.status(), StatusCode::OK);
        sqlx::query("DELETE FROM tokens")
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(fetch(&f, &url, None).await.status(), StatusCode::NOT_FOUND);
    }

    /// Channels of a stream as ffprobe counts them.
    fn channels_of(path: &std::path::Path) -> i64 {
        let output = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "a:0", "-show_entries"])
            .args(["stream=channels", "-of", "csv=p=0"])
            .arg(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap()
    }

    #[tokio::test]
    async fn atmos_and_multichannel_go_as_they_are_only_to_devices_that_play_them() {
        let library = tempfile::tempdir().unwrap();
        for (name, codec, extra) in [
            ("a/01.ec3", "eac3", &["-b:a", "384k"][..]),
            ("a/02.flac", "flac", &[][..]),
        ] {
            let path = library.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let status = std::process::Command::new("ffmpeg")
                .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=d=2"])
                .args([
                    "-af",
                    "pan=5.1|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0",
                    "-c:a",
                    codec,
                ])
                .args(extra)
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
        }
        let pool = crate::db::open_in_memory().await;
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', ?)")
            .bind(library.path().to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        // Track 1 is Dolby Atmos as ffprobe would report it (ffmpeg can't write JOC itself);
        // track 2 a 5.1 FLAC the tag reader read.
        sqlx::raw_sql(
            "INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'dir:a', 'A', 'X', 'a', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, codec, channels)
                 VALUES (1, 1, 'a/01.ec3', 1, 1, 1, 1, 1, 0, 'One', NULL, NULL),
                        (2, 1, 'a/02.flac', 1, 1, 1, 1, 1, 1, 'Two', 'flac', 6);
             INSERT INTO music_track_probes (track_id, size, modified_at, codec, profile, channels,
                 atmos, duration_seconds, probed_at)
                 VALUES (1, 1, 1, 'eac3', 'Dolby Digital Plus + Dolby Atmos', 6, 1, 2.0, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let transcode = tempfile::tempdir().unwrap();
        let converter = Arc::new(Converter::new("ffmpeg".into(), transcode.path()));
        let f = Fixture {
            app: routes(pool.clone(), converter),
            pool,
            token,
            transcode,
            _library: library,
        };
        let served = |body: &Value| -> i64 {
            let name = body["stream_url"].as_str().unwrap();
            assert!(name.starts_with("/api/v1/music/stream/"));
            let made: Vec<_> = std::fs::read_dir(f.transcode.path().join("music"))
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            let newest = made
                .iter()
                .max_by_key(|p| std::fs::metadata(p).unwrap().modified().unwrap())
                .unwrap();
            channels_of(newest)
        };

        // A device that plays Atmos gets the file untouched.
        let (status, atmos) = play(&f, 1, Some(json!({ "spatial": true }))).await;
        assert_eq!(status, StatusCode::OK, "{atmos}");
        assert_eq!(
            (&atmos["format"], &atmos["spatial"], &atmos["content_type"]),
            (&json!("original"), &json!("atmos"), &json!("audio/eac3"))
        );
        assert_eq!(atmos["duration_seconds"], 2.0, "from ffprobe");
        // Any other gets it folded into stereo AAC (it's lossy).
        let (_, folded) = play(&f, 1, None).await;
        assert_eq!(
            (&folded["format"], &folded["spatial"]),
            (&json!("aac"), &Value::Null)
        );
        assert_eq!(served(&folded), 2);

        // A 5.1 FLAC folds into stereo FLAC, or into what the device asked for.
        let (_, flac) = play(&f, 2, None).await;
        assert_eq!(
            (&flac["format"], &flac["spatial"]),
            (&json!("flac"), &Value::Null)
        );
        assert_eq!(served(&flac), 2);
        let (_, opus) = play(&f, 2, Some(json!({ "format": "opus" }))).await;
        assert_eq!(served(&opus), 2);
        // Converted for a device that plays multichannel: the channels stay.
        let (_, kept) = play(&f, 2, Some(json!({ "format": "opus", "spatial": true }))).await;
        assert_eq!(kept["spatial"], "multichannel");
        assert_eq!(served(&kept), 6);
        let (_, original) = play(&f, 2, Some(json!({ "spatial": true }))).await;
        assert_eq!(
            (&original["format"], &original["spatial"]),
            (&json!("original"), &json!("multichannel"))
        );
    }
}
