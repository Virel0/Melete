//! A track's lyrics for the apps: found beside it, in its tags, or from LRCLIB when
//! lyrics are turned on, and kept, so every device shows the same without looking them up.
//! Translated through LibreTranslate when it's set up and an app asks.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use crate::lyrics::{self, Answer};
use crate::{libretranslate, lrclib};

#[derive(Clone)]
struct LyricsState {
    pool: SqlitePool,
    lrclib: Option<Arc<lrclib::Client>>,
    translator: Option<Arc<libretranslate::Client>>,
}

/// The routes without translation.
#[cfg(test)]
pub fn routes(pool: SqlitePool, lrclib: Option<Arc<lrclib::Client>>) -> Router {
    with_translation(pool, lrclib, None)
}

/// The routes, with LibreTranslate for translations when it's set up.
pub fn with_translation(
    pool: SqlitePool,
    lrclib: Option<Arc<lrclib::Client>>,
    translator: Option<Arc<libretranslate::Client>>,
) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/tracks/{id}/lyrics", get(track_lyrics))
        .route("/api/v1/music/lyrics/search", get(search))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(LyricsState {
            pool,
            lrclib,
            translator,
        })
}

#[derive(Deserialize)]
struct LyricsQuery {
    /// A language to translate the lyrics into ("en", "pt-BR").
    translate: Option<String>,
    /// Each line's pronunciation in Latin letters too, when romanisation is on.
    #[serde(default)]
    pronounce: bool,
}

async fn track_lyrics(
    State(state): State<LyricsState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Query(query): Query<LyricsQuery>,
) -> Result<Json<Value>, ApiError> {
    if !current.access.sees_track(&state.pool, id).await? {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such track"));
    }
    let language = query.translate.filter(|l| !l.is_empty());
    let translator = match &language {
        None => None,
        Some(code) if !lyrics::language_code(code) => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "translate is a language code such as en or pt-BR",
            ));
        }
        Some(_) => Some(state.translator.clone().ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "translating lyrics isn't set up on this server",
            )
        })?),
    };
    if query.pronounce && !crate::settings::romanisation(&state.pool).await? {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "lyrics' pronunciation isn't turned on on this server",
        ));
    }
    let failed = |err: lyrics::Error| match err {
        lyrics::Error::Lrclib(err) => {
            tracing::warn!(error = format!("{err:#}"), track = id, "LRCLIB failed");
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "LRCLIB didn't answer; try again later",
            )
        }
        lyrics::Error::Translation(err) => {
            tracing::warn!(
                error = format!("{err:#}"),
                track = id,
                "LibreTranslate failed"
            );
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "LibreTranslate didn't translate the lyrics; try again later",
            )
        }
        lyrics::Error::Database(err) => err.into(),
    };
    let answer = lyrics::for_track(&state.pool, state.lrclib.as_deref(), id)
        .await
        .map_err(failed)?;
    let found = match answer {
        Answer::NoTrack => return Err(ApiError::new(StatusCode::NOT_FOUND, "no such track")),
        Answer::Nothing => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "no lyrics are known for this track",
            ));
        }
        Answer::Found(found) => found,
    };
    let translated = match (&translator, &language) {
        (Some(client), Some(code)) => Some(
            lyrics::translation(&state.pool, client, id, &found, code)
                .await
                .map_err(failed)?,
        ),
        _ => None,
    };
    // Translated, each line has its translation beside it (an empty line, itself); with its
    // pronunciation asked for, that too (none for a line already in Latin letters).
    let japanese = crate::romanise::japanese(&found.text);
    let lines: Vec<Value> = found
        .lines()
        .into_iter()
        .map(|line| {
            let mut shown = json!({ "start_seconds": line.start, "text": line.text });
            if let Some(t) = &translated {
                shown["translation"] = json!(t.get(&line.text).unwrap_or(&line.text));
            }
            if query.pronounce {
                shown["pronunciation"] = json!(crate::romanise::line(&line.text, japanese));
            }
            shown
        })
        .collect();
    let mut body = json!({
        "track_id": id,
        "source": found.source.as_str(),
        "synced": found.synced,
        "lines": lines,
    });
    if translated.is_some() {
        body["language"] = json!(language);
    }
    Ok(Json(body))
}

#[derive(Deserialize)]
struct SearchQuery {
    /// Words from a line of the lyrics.
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

/// Tracks whose lyrics have a line with these words in this order, best match first,
/// each with that line and when it starts, so a player can go straight to it.
async fn search(
    State(state): State<LyricsState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Value>, ApiError> {
    let (limit, offset) = super::music::paging(query.limit, query.offset)?;
    let words = query.q.unwrap_or_default();
    if lyrics::phrase(&words).is_none() {
        return Err(super::music::bad_request(
            "q is words from a line of the lyrics",
        ));
    }
    let (total, hits) = lyrics::search(
        &state.pool,
        &words,
        usize::try_from(limit).unwrap_or(0),
        usize::try_from(offset).unwrap_or(0),
    )
    .await?;
    let ids: Vec<i64> = hits.iter().map(|h| h.track_id).collect();
    let tracks =
        super::music::tracks_by_id(&state.pool, &current.access, current.user.id, &ids).await?;
    let items: Vec<Value> = hits
        .iter()
        .filter_map(|hit| {
            Some(json!({
                "track": tracks.get(&hit.track_id)?,
                "line": { "start_seconds": hit.start, "text": hit.text },
            }))
        })
        .collect();
    Ok(Json(json!({ "total": total, "items": items })))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};

    struct Fixture {
        pool: SqlitePool,
        token: String,
        library: tempfile::TempDir,
    }

    /// A library of three silent tracks on disk: Airbag, No Surprises and Lucky.
    async fn fixture() -> Fixture {
        let pool = crate::db::open_in_memory().await;
        let library = tempfile::tempdir().unwrap();
        std::fs::create_dir(library.path().join("ok")).unwrap();
        for name in ["01 Airbag.flac", "02 No Surprises.flac", "03 Lucky.flac"] {
            std::fs::write(library.path().join("ok").join(name), b"not really audio").unwrap();
        }
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', ?)")
            .bind(library.path().to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(
            r#"INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                   seen_generation) VALUES (1, 1, 'k', 'OK Computer', 'Radiohead', 'ok', 1);
               INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                   first_seen_generation, seen_generation, album_id, tagged, title, genres,
                   duration_seconds) VALUES
                   (1, 1, 'ok/01 Airbag.flac', 1, 1, 1, 1, 1, 1, 'Airbag', '[]', 284.2),
                   (2, 1, 'ok/02 No Surprises.flac', 1, 1, 1, 1, 1, 1, 'No Surprises', '[]', 229),
                   (3, 1, 'ok/03 Lucky.flac', 1, 1, 1, 1, 1, 1, 'Lucky', '[]', 259);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = auth::create_user(&pool, "alice", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let token = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap()
            .1;
        Fixture {
            pool,
            token,
            library,
        }
    }

    async fn get(app: &Router, token: &str, uri: &str) -> (StatusCode, Value) {
        let request = Request::get(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
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
    async fn a_file_beside_the_track_comes_first_and_lrclib_is_asked_once() {
        let f = fixture().await;
        let (client, log) = lrclib::testing::start().await;
        let app = routes(f.pool.clone(), Some(Arc::new(client)));

        // Airbag has an .lrc beside it: LRCLIB isn't asked.
        std::fs::write(
            f.library.path().join("ok/01 Airbag.lrc"),
            "[00:01.00]From the file\n[00:03.00]Second line\n",
        )
        .unwrap();
        let (status, body) = get(&app, &f.token, "/api/v1/music/tracks/1/lyrics").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!({ "track_id": 1, "source": "file", "synced": true, "lines": [
                { "start_seconds": 1.0, "text": "From the file" },
                { "start_seconds": 3.0, "text": "Second line" },
            ]})
        );
        assert!(log.lock().unwrap().is_empty());

        // No Surprises: LRCLIB's plain lyrics, asked once, with only the track's details.
        for _ in 0..2 {
            let (status, body) = get(&app, &f.token, "/api/v1/music/tracks/2/lyrics").await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(
                (&body["source"], &body["synced"]),
                (&json!("lrclib"), &json!(false))
            );
            assert_eq!(body["lines"][1]["text"], "A job that slowly kills you");
            assert_eq!(body["lines"][1]["start_seconds"], Value::Null);
        }
        {
            let log = log.lock().unwrap();
            assert_eq!(log.len(), 1, "kept after the first time");
            let mut keys: Vec<&String> = log[0].keys().collect();
            keys.sort();
            assert_eq!(
                keys,
                ["album_name", "artist_name", "duration", "track_name"]
            );
            assert_eq!(
                (
                    &log[0]["artist_name"],
                    &log[0]["album_name"],
                    &log[0]["duration"]
                ),
                (
                    &"Radiohead".to_owned(),
                    &"OK Computer".to_owned(),
                    &"229".to_owned()
                )
            );
        }

        // Lucky: LRCLIB has nothing, and isn't asked again soon.
        for _ in 0..2 {
            let (status, _) = get(&app, &f.token, "/api/v1/music/tracks/3/lyrics").await;
            assert_eq!(status, StatusCode::NOT_FOUND);
        }
        assert_eq!(log.lock().unwrap().len(), 2);
        // A changed file is looked at afresh.
        sqlx::query("UPDATE music_tracks SET modified_at = 2 WHERE id = 3")
            .execute(&f.pool)
            .await
            .unwrap();
        get(&app, &f.token, "/api/v1/music/tracks/3/lyrics").await;
        assert_eq!(log.lock().unwrap().len(), 3);

        assert_eq!(
            get(&app, &f.token, "/api/v1/music/tracks/99/lyrics")
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn without_lrclib_only_the_tracks_own_lyrics_are_shown() {
        let f = fixture().await;
        let app = routes(f.pool.clone(), None);
        assert_eq!(
            get(&app, &f.token, "/api/v1/music/tracks/2/lyrics").await.0,
            StatusCode::NOT_FOUND
        );
        std::fs::write(
            f.library.path().join("ok/02 No Surprises.lrc"),
            "Plain words\nin a file\n",
        )
        .unwrap();
        let (status, body) = get(&app, &f.token, "/api/v1/music/tracks/2/lyrics").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (&body["source"], &body["synced"]),
            (&json!("file"), &json!(false))
        );
        // While the library can't be reached, the lyrics last found stand.
        std::fs::remove_dir_all(f.library.path().join("ok")).unwrap();
        let (status, body) = get(&app, &f.token, "/api/v1/music/tracks/2/lyrics").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["lines"][0]["text"], "Plain words");
    }

    #[tokio::test]
    async fn translations_are_asked_for_once_and_only_when_set_up() {
        let f = fixture().await;
        std::fs::write(
            f.library.path().join("ok/01 Airbag.lrc"),
            "[00:01.00]in the next world war\n[00:02.00]\n[00:03.00]in the next world war\n",
        )
        .unwrap();
        let without = routes(f.pool.clone(), None);
        assert_eq!(
            get(
                &without,
                &f.token,
                "/api/v1/music/tracks/1/lyrics?translate=de"
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        let (base, log) = libretranslate::testing::start(false).await;
        let app = with_translation(
            f.pool.clone(),
            None,
            Some(Arc::new(libretranslate::Client::new(&base, None).unwrap())),
        );
        for _ in 0..2 {
            let (status, body) =
                get(&app, &f.token, "/api/v1/music/tracks/1/lyrics?translate=de").await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["language"], "de");
            let shown: Vec<(&Value, &Value)> = body["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| (&l["text"], &l["translation"]))
                .collect();
            assert_eq!(
                shown,
                [
                    (
                        &json!("in the next world war"),
                        &json!("IN THE NEXT WORLD WAR")
                    ),
                    (&json!(""), &json!("")),
                    (
                        &json!("in the next world war"),
                        &json!("IN THE NEXT WORLD WAR")
                    ),
                ]
            );
        }
        {
            let log = log.lock().unwrap();
            assert_eq!(log.len(), 1, "kept after the first time");
            assert_eq!(
                log[0]["q"],
                json!(["in the next world war"]),
                "each line once, no empty ones"
            );
        }
        // Changed lyrics are translated afresh; untranslated, the answer is as before.
        std::fs::write(
            f.library.path().join("ok/01 Airbag.lrc"),
            "[00:01.00]airbag\n",
        )
        .unwrap();
        get(&app, &f.token, "/api/v1/music/tracks/1/lyrics?translate=de").await;
        assert_eq!(log.lock().unwrap().len(), 2);
        let (_, plain) = get(&app, &f.token, "/api/v1/music/tracks/1/lyrics").await;
        assert_eq!(plain.get("language"), None);
        assert_eq!(plain["lines"][0].get("translation"), None);
        for (uri, want) in [
            (
                "/api/v1/music/tracks/1/lyrics?translate=../x",
                StatusCode::BAD_REQUEST,
            ),
            (
                "/api/v1/music/tracks/1/lyrics?translate=fr",
                StatusCode::BAD_GATEWAY,
            ),
        ] {
            assert_eq!(get(&app, &f.token, uri).await.0, want, "{uri}");
        }
    }

    #[tokio::test]
    async fn finds_tracks_by_a_line_of_their_lyrics() {
        let f = fixture().await;
        std::fs::write(
            f.library.path().join("ok/01 Airbag.lrc"),
            "[00:01.00]In the next world war\n[00:03.50]In a jackknifed juggernaut\n",
        )
        .unwrap();
        // LRCLIB's lyrics, once kept, are found too.
        let (client, _) = lrclib::testing::start().await;
        let app = routes(f.pool.clone(), Some(Arc::new(client)));
        get(&app, &f.token, "/api/v1/music/tracks/2/lyrics").await;
        crate::lyrics::read_library(&f.pool).await.unwrap();

        let (status, found) = get(
            &app,
            &f.token,
            "/api/v1/music/lyrics/search?q=jackknifed%20jugg",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{found}");
        assert_eq!(found["total"], 1);
        assert_eq!(found["items"][0]["track"]["title"], "Airbag");
        assert_eq!(
            found["items"][0]["line"],
            json!({ "start_seconds": 3.5, "text": "In a jackknifed juggernaut" })
        );
        let (_, landfill) = get(&app, &f.token, "/api/v1/music/lyrics/search?q=landfill").await;
        assert_eq!(landfill["items"][0]["track"]["title"], "No Surprises");
        let (_, none) = get(&app, &f.token, "/api/v1/music/lyrics/search?q=karma").await;
        assert_eq!(none, json!({ "total": 0, "items": [] }));
        for bad in [
            "/api/v1/music/lyrics/search",
            "/api/v1/music/lyrics/search?q=%20*%20",
            "/api/v1/music/lyrics/search?q=war&limit=0",
        ] {
            assert_eq!(
                get(&app, &f.token, bad).await.0,
                StatusCode::BAD_REQUEST,
                "{bad}"
            );
        }
    }

    #[tokio::test]
    async fn pronunciation_comes_beside_each_line_when_turned_on() {
        let f = fixture().await;
        std::fs::write(
            f.library.path().join("ok/01 Airbag.lrc"),
            "[00:01.00]東京の夜は眠らない\n[00:02.00]Hello\n[00:03.00]사랑해\n",
        )
        .unwrap();
        let app = routes(f.pool.clone(), None);
        let uri = "/api/v1/music/tracks/1/lyrics?pronounce=true";
        assert_eq!(
            get(&app, &f.token, uri).await.0,
            StatusCode::CONFLICT,
            "off by default"
        );
        sqlx::query(
            "INSERT INTO settings (name, value, updated_at)
             VALUES ('MELETE_LYRICS_ROMANISATION', 'on', 0)",
        )
        .execute(&f.pool)
        .await
        .unwrap();
        let (status, body) = get(&app, &f.token, uri).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let shown: Vec<&Value> = body["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| &l["pronunciation"])
            .collect();
        assert_eq!(
            shown,
            [
                &json!("toukyou no yoru wa nemura nai"),
                &Value::Null,
                &json!("saranghae")
            ]
        );
        // Not asked for, the lines are as they always were.
        let (_, plain) = get(&app, &f.token, "/api/v1/music/tracks/1/lyrics").await;
        assert_eq!(plain["lines"][0].get("pronunciation"), None);
    }

    #[test]
    fn language_codes_are_checked() {
        for good in ["en", "de", "pt-BR", "zh-Hans", "yue"] {
            assert!(lyrics::language_code(good), "{good}");
        }
        for bad in ["", "e", "EN", "en-", "en-US-x", "../x", "english"] {
            assert!(!lyrics::language_code(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn lrclib_away_is_a_bad_gateway_and_nothing_is_kept() {
        let f = fixture().await;
        let client = lrclib::Client::with_base("http://127.0.0.1:9".into()).unwrap();
        let app = routes(f.pool.clone(), Some(Arc::new(client)));
        assert_eq!(
            get(&app, &f.token, "/api/v1/music/tracks/3/lyrics").await.0,
            StatusCode::BAD_GATEWAY
        );
        let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM music_lyrics")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(kept, 0);
    }

    #[tokio::test]
    async fn lyrics_in_the_tracks_tags_are_read() {
        let f = fixture().await;
        // A real FLAC with lyrics in its Vorbis comments.
        let path = f.library.path().join("ok/03 Lucky.flac");
        let status = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
            .arg("anullsrc=r=44100:cl=mono")
            .args(["-t", "0.5", "-metadata", "LYRICS=[00:02.00]From the tags"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let app = routes(f.pool.clone(), None);
        let (status, body) = get(&app, &f.token, "/api/v1/music/tracks/3/lyrics").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            (&body["source"], &body["synced"]),
            (&json!("tags"), &json!(true))
        );
        assert_eq!(
            body["lines"][0],
            json!({ "start_seconds": 2.0, "text": "From the tags" })
        );
    }
}
