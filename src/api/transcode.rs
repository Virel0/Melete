//! Transcoded playback over HLS, for devices that can't play a file as it is.
//!
//! Asking to transcode returns a playlist URL that carries its own token, like a stream URL;
//! the playlist and its segments need nothing else.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_playback};
use super::playback::{
    LINKS, LoadedFile, STREAM_PREFIX, create_play_session, holder_sees, load_file, load_progress,
    overall_bitrate, require_playable, resolve, way_limit, within_limit,
};
use super::subtitles::{embedded_key, sidecar_key, sidecars_of};
use crate::auth;
use crate::db::unix_now;
use crate::probe::MediaInfo;
use crate::subtitles::{self, Format, Source};
use crate::transcode::{self, Burn, Keyframes, Settings, TranscodeError, Transcoder, VideoCodec};

mod ass_burn;
use ass_burn::TextTrack;

/// Keyframe lists kept, by file version; a list is about 10 KB for a two-hour film.
const KEYFRAMES_KEPT: usize = 64;

/// File id, size and modification time.
type FileVersion = (i64, i64, i64);

#[derive(Clone)]
struct TranscodeState {
    pool: SqlitePool,
    ffprobe: Arc<PathBuf>,
    transcoder: Arc<Transcoder>,
    subtitle_cache: Arc<PathBuf>,
    keyframes: Arc<Mutex<HashMap<FileVersion, Arc<Keyframes>>>>,
    /// Whether each ASS/SSA track looked at is styled, by its subtitle cache key.
    styled: Arc<Mutex<HashMap<String, bool>>>,
}

/// What a client asks a transcode for: the settings, and the subtitle track the viewer chose
/// with whether the player draws ASS, so a styled ASS track can be burned in for it.
#[derive(serde::Deserialize)]
struct TranscodeAsk {
    #[serde(flatten)]
    settings: Settings,
    subtitle: Option<String>,
    renders_ass: Option<bool>,
}

/// Asking to transcode, for everyone signed in, and the HLS playlist and segments it gives,
/// behind their own token.
pub fn routes(
    pool: SqlitePool,
    ffprobe: PathBuf,
    transcoder: Arc<Transcoder>,
    subtitle_cache: PathBuf,
) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    let signed_in = Router::new()
        .route("/api/v1/files/{id}/transcode", post(start))
        .route_layer(middleware::from_fn_with_state(auth, require_playback));
    // The playlist URL's own token is the credential here.
    let hls = Router::new().route("/api/v1/hls/{token}/{name}", get(hls));
    signed_in.merge(hls).with_state(TranscodeState {
        pool,
        ffprobe: Arc::new(ffprobe),
        transcoder,
        subtitle_cache: Arc::new(subtitle_cache),
        keyframes: Arc::default(),
        styled: Arc::default(),
    })
}

/// A file's keyframes, found once per version of it.
async fn keyframes_of(
    state: &TranscodeState,
    file: &LoadedFile,
    video: &std::path::Path,
    media: &MediaInfo,
) -> Result<Arc<Keyframes>, ApiError> {
    let key = (file.id, file.size, file.modified_at);
    if let Some(found) = state.keyframes.lock().expect("no holder panics").get(&key) {
        return Ok(found.clone());
    }
    let stream = media.video.first().map_or(0, |v| v.index);
    let duration = media.duration_seconds.unwrap_or_default();
    let found = transcode::keyframes(&state.ffprobe, video, stream, duration)
        .await
        .map_err(|err| {
            tracing::warn!(
                rel_path = file.rel_path,
                error = format!("{err:#}"),
                "finding keyframes failed"
            );
            unprocessable(
                "this file's keyframes can't be found through its index, or are too far apart, so its video can't be copied; ask for h264 or hevc",
            )
        })?;
    let found = Arc::new(found);
    let mut kept = state.keyframes.lock().expect("no holder panics");
    if kept.len() >= KEYFRAMES_KEPT {
        kept.clear();
    }
    kept.insert(key, found.clone());
    Ok(found)
}

/// For a text track to burn in, renders it to an ASS file once and returns `Burn::Text`.
/// `None` without burn-in, or for a picture track, which the plan already covers.
async fn text_burn(
    state: &TranscodeState,
    file: &LoadedFile,
    video: PathBuf,
    settings: &Settings,
) -> Result<Option<Burn>, ApiError> {
    let Some(track) = settings.burn_subtitle.as_deref() else {
        return Ok(None);
    };
    if !track.starts_with(['s', 'x']) {
        return Err(unprocessable("no such subtitle track"));
    }
    let ffmpeg = state.transcoder.ffmpeg();
    let Some(text) = TextTrack::find(file, &video, track).await? else {
        return Ok(None);
    };
    let path = text.render(state, file, &video).await?;
    let burn = Burn::text(path).map_err(|reason| anyhow::anyhow!(reason))?;
    // The fonts the video carries, so styled subtitles look as made; without them a standard
    // font stands in, so a failure here doesn't stop the burn.
    let key = subtitles::fonts_key(file.id, file.size, file.modified_at);
    let fonts = subtitles::fonts(&state.ffprobe, ffmpeg, &state.subtitle_cache, &key, &video).await;
    Ok(Some(match fonts {
        Ok(Some(dir)) => burn
            .with_fonts(dir)
            .map_err(|reason| anyhow::anyhow!(reason))?,
        Ok(None) => burn,
        Err(err) => {
            tracing::warn!(
                rel_path = file.rel_path,
                error = format!("{err:#}"),
                "couldn't get the video's fonts"
            );
            burn
        }
    }))
}

fn unprocessable(reason: &str) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, reason)
}

/// Leaves this much of a way's video limit for the audio beside the video: AAC's 384 kbit/s for
/// surround sound, the most a conversion's audio takes.
const AUDIO_ALLOWANCE: i64 = 384_000;

/// Holds a conversion to the video limit of the way it was asked through: encoded video at the
/// limit less [`AUDIO_ALLOWANCE`], whatever was asked; copied video only when the file is
/// within the limit as it is.
fn hold_to_limit(
    settings: &mut Settings,
    limit: Option<i64>,
    file: &LoadedFile,
    media: &MediaInfo,
) -> Result<(), ApiError> {
    let Some(limit) = limit else {
        return Ok(());
    };
    if settings.video_codec == Some(VideoCodec::Copy) {
        return within_limit(limit, overall_bitrate(file.size, Some(media)));
    }
    let video = (limit - AUDIO_ALLOWANCE).max(100_000);
    settings.max_bitrate = Some(settings.max_bitrate.map_or(video, |asked| asked.min(video)));
    Ok(())
}

/// Creates a transcoding session for a file and returns its playlist URL.
async fn start(
    State(state): State<TranscodeState>,
    Extension(current): Extension<CurrentUser>,
    Path(file_id): Path<i64>,
    limit: Option<Extension<crate::ways_in::VideoLimit>>,
    Json(ask): Json<TranscodeAsk>,
) -> Result<Json<Value>, ApiError> {
    let TranscodeAsk {
        mut settings,
        subtitle,
        renders_ass,
    } = ask;
    settings
        .validate()
        .and(ass_burn::check_subtitle(subtitle.as_deref()))
        .map_err(|reason| ApiError::new(StatusCode::BAD_REQUEST, reason))?;
    require_playable(&state.pool, &current.access, file_id).await?;
    let file = load_file(&state.pool, &state.ffprobe, file_id).await?;
    let Some(media) = &file.media else {
        return Err(unprocessable(
            "ffprobe couldn't read this file, so it can't be transcoded",
        ));
    };
    let limit = way_limit(limit);
    hold_to_limit(&mut settings, limit, &file, media)?;
    let burn_reason = ass_burn::decide(
        &state,
        &file,
        &mut settings,
        subtitle.as_deref(),
        renders_ass,
    )
    .await?;
    let duration_seconds = media.duration_seconds;
    // Checks the file can be transcoded with these settings before handing out a URL.
    transcode::plan(
        PathBuf::new(),
        media,
        &settings,
        state.transcoder.hardware(),
    )
    .map_err(unprocessable)?;
    if settings.video_codec == Some(VideoCodec::Copy) {
        let video = resolve(&file.root, &file.rel_path).await?;
        keyframes_of(&state, &file, &video, media).await?;
    }
    if let Some(id) = settings
        .burn_subtitle
        .as_deref()
        .and_then(|t| t.strip_prefix('x'))
    {
        let video = resolve(&file.root, &file.rel_path).await?;
        if !sidecars_of(video).await?.iter().any(|s| s.id == id) {
            return Err(unprocessable("no such subtitle track"));
        }
    }
    if settings.burn_subtitle.is_some()
        && Burn::text(state.subtitle_cache.join("check.ass")).is_err()
    {
        // Text tracks are drawn from the cache folder, whose path goes into an ffmpeg filter.
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the data folder's path has characters ffmpeg's filters can't take, so subtitles can't be burned in",
        ));
    }
    if settings.burn_subtitle.is_some() {
        // Render a text track now, so it's ready by the first segment; reading an embedded
        // track means reading the whole file. Failures show up again when a segment asks.
        let (state, settings) = (state.clone(), settings.clone());
        tokio::spawn(async move {
            if let Ok(video) = resolve(&file.root, &file.rel_path).await {
                let _ = text_burn(&state, &file, video, &settings).await;
            }
        });
    }
    let settings_json = serde_json::to_string(&settings)?;
    let (token, expires_at) =
        create_play_session(&state.pool, current.token_id, file_id, Some(&settings_json)).await?;
    let progress = load_progress(&state.pool, current.user.id, file_id)
        .await?
        .unwrap_or_default();
    Ok(Json(json!({
        "playlist_url": format!("/api/v1/hls/{token}/main.m3u8"),
        "expires_at": expires_at,
                "duration_seconds": duration_seconds,
        "resume_seconds": progress.position_seconds,
        "watched": progress.watched,
        "segments": super::segments::of_file(&state.pool, file_id).await?,
        "max_bitrate": limit,
        "burned_subtitle": settings.burn_subtitle,
        "burn_reason": burn_reason,
    })))
}

/// Serves a transcoding session's playlist (`main.m3u8`), one of its segments (`{n}.ts`, or
/// `{n}.m4s` for HEVC), or for HEVC the segments' init section (`init.mp4`).
async fn hls(
    State(state): State<TranscodeState>,
    Path((token, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    if !token.starts_with(STREAM_PREFIX) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "not a stream link"));
    }
    // The token's hash also keys its running ffmpeg: unlike row ids, it is never reused.
    let key = auth::token_hash(&token);
    let row: Option<(i64, String, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT file_id, transcode, token_id FROM {LINKS}
         WHERE token_hash = ? AND expires_at > ? AND transcode IS NOT NULL"
    )))
    .bind(&key)
    .bind(unix_now())
    .fetch_optional(&state.pool)
    .await?;
    let gone = || {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "this playlist link is unknown or has expired; ask to transcode again",
        )
    };
    let Some((file_id, settings, token_id)) = row else {
        return Err(gone());
    };
    if !holder_sees(&state.pool, token_id, file_id).await? {
        return Err(gone());
    }
    let settings: Settings = serde_json::from_str(&settings)?;
    let file = load_file(&state.pool, &state.ffprobe, file_id).await?;
    let media = file
        .media
        .clone()
        .ok_or_else(|| unprocessable("ffprobe can no longer read this file"))?;
    let path = resolve(&file.root, &file.rel_path).await?;
    let mut plan = transcode::plan(path.clone(), &media, &settings, state.transcoder.hardware())
        .map_err(unprocessable)?;
    if plan.codec == VideoCodec::Copy {
        plan.keyframes = Some(keyframes_of(&state, &file, &path, &media).await?);
    }

    if name == "main.m3u8" {
        let headers = [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")];
        return Ok((headers, plan.playlist()).into_response());
    }
    let ask = if name == "init.mp4" && plan.fmp4 {
        Ask::Init
    } else {
        let segment = name
            .strip_suffix(plan.extension())
            .and_then(|n| n.strip_suffix('.'))
            .and_then(|n| n.parse::<i64>().ok())
            .filter(|n| (0..plan.segment_count()).contains(n));
        let Some(n) = segment else {
            return Err(ApiError::new(StatusCode::NOT_FOUND, "no such segment"));
        };
        Ask::Segment(n)
    };
    if plan.burn.is_none() {
        plan.burn = text_burn(&state, &file, path, &settings).await?;
    }
    let made = match ask {
        Ask::Init => state.transcoder.init(&key, &plan).await,
        Ask::Segment(n) => state.transcoder.segment(&key, &plan, n).await,
    };
    let segment_file = made.map_err(|err| match err {
        TranscodeError::Busy => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "too many transcodes are running; try again later",
        ),
        TranscodeError::TimedOut => ApiError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "the segment wasn't ready in time; try again",
        ),
        TranscodeError::Failed(log) => {
            tracing::warn!(rel_path = file.rel_path, log, "transcoding failed");
            ApiError::new(StatusCode::BAD_GATEWAY, "transcoding this file failed")
        }
    })?;
    // The supervisor may delete a segment the viewer has long passed; ask again restarts it.
    let bytes = tokio::fs::read(&segment_file).await.map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "the segment is gone; try again",
        )
    })?;
    let content_type = if plan.fmp4 { "video/mp4" } else { "video/mp2t" };
    Ok(([(header::CONTENT_TYPE, content_type)], bytes).into_response())
}

enum Ask {
    Init,
    Segment(i64),
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
    use crate::transcode::Hardware;

    struct Fixture {
        pool: SqlitePool,
        app: Router,
        token: String,
        token_id: i64,
        _dir: tempfile::TempDir,
    }

    /// File 1: a 20-second clip. File 2: junk ffprobe rejects.
    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("library");
        std::fs::create_dir(&library).unwrap();
        let clip = crate::probe::testing::make_long_clip(dir.path(), 20);
        std::fs::rename(&clip, library.join("clip.mkv")).unwrap();
        std::fs::write(library.join("junk.mkv"), b"junk").unwrap();
        let size = |name: &str| std::fs::metadata(library.join(name)).unwrap().len() as i64;

        let pool = db::open_in_memory().await;
        let config = LibraryConfig {
            name: "movies".into(),
            kind: LibraryKind::Movies,
            path: library.clone(),
        };
        sync_config(&pool, &[config]).await.unwrap();
        sqlx::query("INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation) VALUES (1, 1, 'clip.mkv', 'Clip', 1)")
            .execute(&pool)
            .await
            .unwrap();
        for (id, name) in [(1, "clip.mkv"), (2, "junk.mkv")] {
            sqlx::query(
                "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
                     first_seen_generation, seen_generation) VALUES (?, 1, ?, ?, 1, 1, 1, 1)",
            )
            .bind(id)
            .bind(name)
            .bind(size(name))
            .execute(&pool)
            .await
            .unwrap();
        }
        let user = auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (token_id, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap();
        let transcoder = Transcoder::new(
            "ffmpeg".into(),
            Hardware::Software,
            dir.path().join("transcode"),
            4,
        );
        Fixture {
            app: routes(
                pool.clone(),
                "ffprobe".into(),
                transcoder,
                dir.path().join("subtitles"),
            ),
            pool,
            token,
            token_id,
            _dir: dir,
        }
    }

    async fn call(f: &Fixture, method: &str, uri: &str, body: Option<Value>) -> Response {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {}", f.token));
        if body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        let request = request
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap();
        f.app.clone().oneshot(request).await.unwrap()
    }

    async fn json_body(response: Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    async fn playlist_url(f: &Fixture, settings: Value) -> String {
        let response = call(f, "POST", "/api/v1/files/1/transcode", Some(settings)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert!(
            (body["duration_seconds"].as_f64().unwrap() - 20.0).abs() < 0.2,
            "{body}"
        );
        body["playlist_url"].as_str().unwrap().to_owned()
    }

    #[tokio::test]
    async fn serves_a_playlist_and_transcoded_segments() {
        let f = fixture().await;
        let url = playlist_url(&f, json!({ "max_height": 180, "audio_channels": 2 })).await;
        assert!(
            url.starts_with("/api/v1/hls/mls_") && url.ends_with("/main.m3u8"),
            "{url}"
        );

        let response = call(&f, "GET", &url, None).await;
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/vnd.apple.mpegurl"
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let playlist = String::from_utf8(bytes.to_vec()).unwrap();
        let segments: Vec<&str> = playlist.lines().filter(|l| l.ends_with(".ts")).collect();
        assert_eq!(segments, ["0.ts", "1.ts", "2.ts", "3.ts"]);

        let segment_url = url.replace("main.m3u8", "1.ts");
        let response = call(&f, "GET", &segment_url, None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "video/mp2t");
        let segment = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(segment[0], 0x47, "an MPEG-TS sync byte");

        let saved = f._dir.path().join("segment.ts");
        std::fs::write(&saved, &segment).unwrap();
        let info = crate::probe::probe(std::path::Path::new("ffprobe"), &saved)
            .await
            .unwrap();
        assert_eq!(info.video[0].codec, "h264");
        assert_eq!(info.video[0].height, Some(180), "scaled down as asked");
    }

    #[tokio::test]
    async fn refuses_bad_requests() {
        let f = fixture().await;
        let response = call(
            &f,
            "POST",
            "/api/v1/files/1/transcode",
            Some(json!({ "max_height": 50 })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = call(&f, "POST", "/api/v1/files/2/transcode", Some(json!({}))).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let response = call(
            &f,
            "POST",
            "/api/v1/files/1/transcode",
            Some(json!({ "audio_stream": 7 })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json_body(response).await["error"], "no such audio stream");

        let url = playlist_url(&f, json!({})).await;
        let past_the_end = url.replace("main.m3u8", "4.ts");
        assert_eq!(
            call(&f, "GET", &past_the_end, None).await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(&f, "GET", &url.replace("main.m3u8", "x.ts"), None)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(&f, "GET", "/api/v1/hls/mlt_x/main.m3u8", None)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(&f, "GET", "/api/v1/hls/mls_unknown/main.m3u8", None)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn burns_in_a_subtitle_file_beside_the_video() {
        let f = fixture().await;
        std::fs::write(
            f._dir.path().join("library/clip.en.srt"),
            "1\n00:00:00,000 --> 00:00:19,000\nBURNED IN\n",
        )
        .unwrap();
        let url = playlist_url(&f, json!({ "burn_subtitle": "xed0587fb" })).await;
        let response = call(&f, "GET", &url.replace("main.m3u8", "0.ts"), None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let segment = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(segment[0], 0x47);
        assert!(
            std::fs::read_dir(f._dir.path().join("subtitles"))
                .unwrap()
                .any(|e| e.unwrap().file_name().to_str().unwrap().ends_with(".ass")),
            "rendered to ASS in the cache"
        );
    }

    async fn get_bytes(f: &Fixture, url: &str, content_type: &str) -> Vec<u8> {
        let response = call(f, "GET", url, None).await;
        assert_eq!(response.status(), StatusCode::OK, "{url}");
        assert_eq!(response.headers()[header::CONTENT_TYPE], content_type);
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec()
    }

    #[tokio::test]
    async fn copies_the_video_as_it_is() {
        let f = fixture().await;
        let url = playlist_url(&f, json!({ "video_codec": "copy" })).await;
        let playlist =
            String::from_utf8(get_bytes(&f, &url, "application/vnd.apple.mpegurl").await).unwrap();
        // The clip has keyframes at 0 and 10.42 s only.
        let segments: Vec<&str> = playlist.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(segments, ["0.ts", "1.ts"], "{playlist}");
        assert!(playlist.contains("#EXTINF:10.41"), "{playlist}");

        let segment = get_bytes(&f, &url.replace("main.m3u8", "1.ts"), "video/mp2t").await;
        let saved = f._dir.path().join("copied.ts");
        std::fs::write(&saved, &segment).unwrap();
        let info = crate::probe::probe(std::path::Path::new("ffprobe"), &saved)
            .await
            .unwrap();
        assert_eq!(info.video[0].codec, "h264");
        assert_eq!(info.video[0].height, Some(240), "the picture as it was");
        assert_eq!(info.audio[0].codec, "aac");
        assert_eq!(
            call(&f, "GET", &url.replace("main.m3u8", "2.ts"), None)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn makes_hevc_in_fragmented_mp4() {
        let f = fixture().await;
        let url = playlist_url(&f, json!({ "video_codec": "hevc", "max_height": 180 })).await;
        let playlist =
            String::from_utf8(get_bytes(&f, &url, "application/vnd.apple.mpegurl").await).unwrap();
        assert!(
            playlist.contains("#EXT-X-MAP:URI=\"init.mp4\"\n"),
            "{playlist}"
        );
        let segments: Vec<&str> = playlist.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(segments, ["0.m4s", "1.m4s", "2.m4s", "3.m4s"]);

        let mut whole = get_bytes(&f, &url.replace("main.m3u8", "init.mp4"), "video/mp4").await;
        whole.extend(get_bytes(&f, &url.replace("main.m3u8", "1.m4s"), "video/mp4").await);
        let saved = f._dir.path().join("hevc.mp4");
        std::fs::write(&saved, &whole).unwrap();
        let info = crate::probe::probe(std::path::Path::new("ffprobe"), &saved)
            .await
            .unwrap();
        assert_eq!(info.video[0].codec, "hevc");
        assert_eq!(info.video[0].height, Some(180));
        assert_eq!(
            call(&f, "GET", &url.replace("main.m3u8", "1.ts"), None)
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "segments are .m4s here"
        );
        let h264 = playlist_url(&f, json!({})).await;
        assert_eq!(
            call(&f, "GET", &h264.replace("main.m3u8", "init.mp4"), None)
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "MPEG-TS has no init section"
        );
    }

    #[tokio::test]
    async fn refuses_copying_with_picture_changes() {
        let f = fixture().await;
        let response = call(
            &f,
            "POST",
            "/api/v1/files/1/transcode",
            Some(json!({ "video_codec": "copy", "max_height": 180 })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = call(
            &f,
            "POST",
            "/api/v1/files/1/transcode",
            Some(json!({ "video_codec": "av1" })),
        )
        .await;
        assert!(response.status().is_client_error());
    }

    #[tokio::test]
    async fn refuses_unknown_burn_tracks() {
        let f = fixture().await;
        for (settings, status) in [
            (json!({ "burn_subtitle": "nope" }), StatusCode::BAD_REQUEST),
            (
                json!({ "burn_subtitle": "s9" }),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                json!({ "burn_subtitle": "x00000000" }),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            let response = call(
                &f,
                "POST",
                "/api/v1/files/1/transcode",
                Some(settings.clone()),
            )
            .await;
            assert_eq!(response.status(), status, "{settings}");
        }
    }

    #[tokio::test]
    async fn direct_stream_links_dont_open_playlists_and_sign_out_ends_them() {
        let f = fixture().await;
        let Ok((direct, _)) = create_play_session(&f.pool, f.token_id, 1, None).await else {
            panic!("creating a direct stream link failed");
        };
        let response = call(&f, "GET", &format!("/api/v1/hls/{direct}/main.m3u8"), None).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let url = playlist_url(&f, json!({})).await;
        assert_eq!(call(&f, "GET", &url, None).await.status(), StatusCode::OK);
        sqlx::query("DELETE FROM tokens WHERE id = ?")
            .bind(f.token_id)
            .execute(&f.pool)
            .await
            .unwrap();
        let anonymous = Request::get(&url).body(Body::empty()).unwrap();
        assert_eq!(
            f.app.clone().oneshot(anonymous).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }

    /// An ASS script with one style, `Default`, and these events.
    fn ass_script(events: &str) -> String {
        format!(
            "[Script Info]\nScriptType: v4.00+\nPlayResX: 320\nPlayResY: 240\n\n[V4+ Styles]\n\
             Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, \
             BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, \
             BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
             Style: Default,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,\
             0,0,1,2,0,2,10,10,10,1\n\n[Events]\n\
             Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
             {events}"
        )
    }

    /// A placed sign and karaoke.
    fn styled_ass() -> String {
        ass_script(
            "Dialogue: 0,0:00:00.00,0:00:19.00,Default,,0,0,0,,{\\pos(160,40)}A SIGN\n\
             Dialogue: 0,0:00:01.00,0:00:05.00,Default,,0,0,0,,{\\k50}Ka{\\k50}ra\n",
        )
    }

    /// Dialogue only, with italics.
    fn plain_ass() -> String {
        ass_script("Dialogue: 0,0:00:00.00,0:00:19.00,Default,,0,0,0,,Hello, {\\i1}there{\\i0}\n")
    }

    /// Remuxes file 1 with a styled ASS track (stream 2) and a plain one (stream 3).
    async fn add_ass_tracks(f: &Fixture) {
        let dir = f._dir.path();
        std::fs::write(dir.join("styled.ass"), styled_ass()).unwrap();
        std::fs::write(dir.join("plain.ass"), plain_ass()).unwrap();
        let clip = dir.join("library/clip.mkv");
        let made = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-i"])
            .arg(&clip)
            .arg("-i")
            .arg(dir.join("styled.ass"))
            .arg("-i")
            .arg(dir.join("plain.ass"))
            .args(["-map", "0", "-map", "1", "-map", "2", "-c", "copy"])
            .arg(dir.join("with-ass.mkv"))
            .status()
            .expect("ffmpeg is installed");
        assert!(made.success());
        std::fs::rename(dir.join("with-ass.mkv"), &clip).unwrap();
        sqlx::query("UPDATE media_files SET size = ? WHERE id = 1")
            .bind(std::fs::metadata(&clip).unwrap().len() as i64)
            .execute(&f.pool)
            .await
            .unwrap();
    }

    async fn transcode_answer(f: &Fixture, settings: &Value) -> (StatusCode, Value) {
        let response = call(
            f,
            "POST",
            "/api/v1/files/1/transcode",
            Some(settings.clone()),
        )
        .await;
        (response.status(), json_body(response).await)
    }

    /// The track the latest transcoding session burns in, as its segments are made.
    async fn stored_burn(f: &Fixture) -> Option<String> {
        let stored: String =
            sqlx::query_scalar("SELECT transcode FROM play_sessions ORDER BY rowid DESC LIMIT 1")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        serde_json::from_str::<Settings>(&stored)
            .unwrap()
            .burn_subtitle
    }

    #[tokio::test]
    async fn burns_in_a_styled_ass_track_for_a_player_that_cant_draw_ass() {
        let f = fixture().await;
        add_ass_tracks(&f).await;
        let asked = json!({ "subtitle": "s2", "renders_ass": false });
        let (status, body) = transcode_answer(&f, &asked).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["burned_subtitle"], "s2", "{body}");
        assert_eq!(body["burn_reason"], "styled_ass");
        assert_eq!(stored_burn(&f).await.as_deref(), Some("s2"));
        let url = body["playlist_url"].as_str().unwrap();
        let segment = get_bytes(&f, &url.replace("main.m3u8", "0.ts"), "video/mp2t").await;
        assert_eq!(segment[0], 0x47);
    }

    #[tokio::test]
    async fn leaves_plain_ass_and_players_that_draw_ass_alone() {
        let f = fixture().await;
        add_ass_tracks(&f).await;
        for asked in [
            json!({ "subtitle": "s3", "renders_ass": false }),
            json!({ "subtitle": "s2", "renders_ass": true }),
            json!({ "subtitle": "s2" }),
            json!({ "renders_ass": false }),
            json!({ "subtitle": "o0123abcd", "renders_ass": false }),
        ] {
            let (status, body) = transcode_answer(&f, &asked).await;
            assert_eq!(status, StatusCode::OK, "{asked}: {body}");
            assert_eq!(body["burned_subtitle"], Value::Null, "{asked}");
            assert_eq!(body["burn_reason"], Value::Null, "{asked}");
            assert_eq!(stored_burn(&f).await, None, "{asked}");
        }
        // Decided once: the plain track's rendering, gone from the cache, isn't made again.
        let cache = f._dir.path().join("subtitles");
        let plain_rendering = || {
            std::fs::read_dir(&cache)
                .unwrap()
                .map(|e| e.unwrap().path())
                .find(|p| {
                    let name = p.file_name().unwrap().to_str().unwrap();
                    name.starts_with("1-s3-") && name.ends_with(".ass")
                })
        };
        std::fs::remove_file(plain_rendering().expect("rendered to decide")).unwrap();
        let asked = json!({ "subtitle": "s3", "renders_ass": false });
        let (_, body) = transcode_answer(&f, &asked).await;
        assert_eq!(body["burn_reason"], Value::Null);
        assert_eq!(plain_rendering(), None, "not read again");

        // burn_subtitle still decides on its own.
        let asked = json!({ "burn_subtitle": "s3", "subtitle": "s2", "renders_ass": true });
        let (_, body) = transcode_answer(&f, &asked).await;
        assert_eq!(body["burned_subtitle"], "s3");
        assert_eq!(body["burn_reason"], "asked");
    }

    #[tokio::test]
    async fn keeps_copy_and_says_why() {
        let f = fixture().await;
        add_ass_tracks(&f).await;
        let asked = json!({ "video_codec": "copy", "subtitle": "s2", "renders_ass": false });
        let (status, body) = transcode_answer(&f, &asked).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["burned_subtitle"], Value::Null);
        assert_eq!(body["burn_reason"], "copy_kept");
        assert_eq!(stored_burn(&f).await, None);
    }

    #[tokio::test]
    async fn burns_in_a_styled_ass_file_beside_the_video() {
        let f = fixture().await;
        let library = f._dir.path().join("library");
        std::fs::write(library.join("clip.en.ass"), styled_ass()).unwrap();
        std::fs::write(library.join("clip.fr.ass"), plain_ass()).unwrap();
        let sidecars = crate::subtitles::sidecars(&library.join("clip.mkv")).unwrap();
        let id = |language: &str| {
            let found = sidecars
                .iter()
                .find(|s| s.language.as_deref() == Some(language));
            format!("x{}", found.unwrap().id)
        };
        let asked = json!({ "subtitle": id("en"), "renders_ass": false });
        let (status, body) = transcode_answer(&f, &asked).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["burned_subtitle"], id("en"));
        assert_eq!(body["burn_reason"], "styled_ass");
        let asked = json!({ "subtitle": id("fr"), "renders_ass": false });
        let (_, body) = transcode_answer(&f, &asked).await;
        assert_eq!(body["burned_subtitle"], Value::Null);
        assert_eq!(body["burn_reason"], Value::Null);
    }

    #[tokio::test]
    async fn refuses_chosen_tracks_that_arent_the_files() {
        let f = fixture().await;
        for (asked, status) in [
            (json!({ "subtitle": "nope" }), StatusCode::BAD_REQUEST),
            (
                json!({ "subtitle": "s9", "renders_ass": false }),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                json!({ "subtitle": "x00000000", "renders_ass": false }),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            // Without renders_ass false the track isn't looked at, as before.
            (json!({ "subtitle": "s9" }), StatusCode::OK),
        ] {
            let (got, body) = transcode_answer(&f, &asked).await;
            assert_eq!(got, status, "{asked}: {body}");
        }
    }
}
