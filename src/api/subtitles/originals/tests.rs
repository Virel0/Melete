use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

use super::super::routes;
use crate::auth::{self, TokenKind};
use crate::db;
use crate::library::{LibraryConfig, LibraryKind, sync_config};

/// A styled track: a positioned, fading sign in a font of its own, and karaoke.
const STYLED: &str = "[Script Info]\nScriptType: v4.00+\nPlayResX: 320\nPlayResY: 240\n\n\
[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, \
BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, \
Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
Style: Sign,Fancy,24,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,0,8,10,10,10,1\n\n\
[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
Dialogue: 0,0:00:00.00,0:00:01.00,Sign,,0,0,0,,{\\pos(160,40)\\fad(100,100)}Styled sign\n\
Dialogue: 1,0:00:00.50,0:00:01.50,Sign,,0,0,0,,{\\k20}Ka{\\k30}ra\n";

/// Bytes attached as a font; never parsed, only passed on.
const FONT: &[u8] = b"not really a font, but attached as one";

struct Fixture {
    pool: SqlitePool,
    app: Router,
    token: String,
    token_id: i64,
    dir: tempfile::TempDir,
}

/// File 1: a clip with video (0), the styled track (1), a font (2) and a text attachment (3),
/// with `clip.en.ass` and `clip.fr.srt` beside it.
async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("library");
    std::fs::create_dir(&library).unwrap();
    let styled = dir.path().join("styled.ass");
    std::fs::write(&styled, STYLED).unwrap();
    let font = dir.path().join("Fancy.ttf");
    std::fs::write(&font, FONT).unwrap();
    let notes = dir.path().join("notes.txt");
    std::fs::write(&notes, "not a font").unwrap();
    let clip = library.join("clip.mkv");
    let made = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc=d=2:s=64x64")
        .arg("-i")
        .arg(&styled)
        .args(["-map", "0", "-map", "1", "-c:v", "mpeg4", "-c:s", "copy"])
        .arg("-attach")
        .arg(&font)
        .args(["-metadata:s:t:0", "mimetype=application/x-truetype-font"])
        .args(["-metadata:s:t:0", "filename=../fonts/Fancy.ttf"])
        .arg("-attach")
        .arg(&notes)
        .args(["-metadata:s:t:1", "mimetype=text/plain"])
        .arg(&clip)
        .status()
        .expect("ffmpeg is installed");
    assert!(made.success());
    // Beside it: a styled file in a legacy encoding, sent in UTF-8, and a SubRip one.
    let mut beside = STYLED.replace("Styled sign", "Caf\u{e9}").into_bytes();
    let accent = beside.iter().position(|b| *b == 0xc3).unwrap();
    beside.splice(accent..accent + 2, [0xe9]);
    std::fs::write(library.join("clip.en.ass"), &beside).unwrap();
    std::fs::write(
        library.join("clip.fr.srt"),
        "1\n00:00:00,500 --> 00:00:01,500\nSalut\n",
    )
    .unwrap();
    let size = std::fs::metadata(&clip).unwrap().len() as i64;

    let pool = db::open_in_memory().await;
    let config = LibraryConfig {
        name: "anime".into(),
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
    let app = routes(
        pool.clone(),
        "ffprobe".into(),
        "ffmpeg".into(),
        dir.path().join("cache"),
    );
    let user = auth::create_user(&pool, "u", "user-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    let (token_id, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
        .await
        .unwrap();
    Fixture {
        pool,
        app,
        token,
        token_id,
        dir,
    }
}

struct Answer {
    status: StatusCode,
    content_type: Option<String>,
    nosniff: bool,
    body: Vec<u8>,
}

async fn get(f: &Fixture, uri: &str, bearer: Option<&str>) -> Answer {
    let mut request = Request::get(uri);
    if let Some(token) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = f
        .app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let header_value = |name| {
        response
            .headers()
            .get(name)
            .map(|v: &header::HeaderValue| v.to_str().unwrap().to_owned())
    };
    let content_type = header_value(header::CONTENT_TYPE);
    let nosniff = header_value(header::X_CONTENT_TYPE_OPTIONS).as_deref() == Some("nosniff");
    Answer {
        status: response.status(),
        content_type,
        nosniff,
        body: response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    }
}

/// A stream link for file 1, as `/api/v1/stream/{token}`.
async fn stream_url(f: &Fixture) -> String {
    let Ok((token, _)) =
        super::super::super::playback::create_play_session(&f.pool, f.token_id, 1, None).await
    else {
        panic!("creating a stream link failed");
    };
    format!("/api/v1/stream/{token}")
}

#[tokio::test]
async fn the_list_says_which_tracks_are_ass_and_where_they_are() {
    let f = fixture().await;
    let answer = get(&f, "/api/v1/files/1/subtitles", Some(&f.token)).await;
    assert_eq!(answer.status, StatusCode::OK);
    let tracks: Value = serde_json::from_slice(&answer.body).unwrap();
    let summary: Vec<(String, String, bool, Value)> = tracks
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["track"].as_str().unwrap().to_owned(),
                t["codec"].as_str().unwrap().to_owned(),
                t["ass"].as_bool().unwrap(),
                t["ass_url"].clone(),
            )
        })
        .collect();
    let ass_beside = summary[1].0.clone();
    assert_eq!(
        summary,
        vec![
            ("s1".into(), "ass".into(), true, json!("subtitles/s1.ass")),
            (
                ass_beside.clone(),
                "ass".into(),
                true,
                json!(format!("subtitles/{ass_beside}.ass"))
            ),
            (summary[2].0.clone(), "subrip".into(), false, Value::Null),
        ]
    );
}

#[tokio::test]
async fn serves_ass_tracks_byte_for_byte_behind_the_stream_token() {
    let f = fixture().await;
    let stream = stream_url(&f).await;

    let embedded = get(&f, &format!("{stream}/subtitles/s1.ass"), None).await;
    assert_eq!(embedded.status, StatusCode::OK);
    assert_eq!(
        embedded.content_type.as_deref(),
        Some("text/x-ass; charset=UTF-8")
    );
    assert!(embedded.nosniff);
    assert_eq!(String::from_utf8(embedded.body).unwrap(), STYLED);
    // Cached: served again with ffmpeg's copy kept, not the burn-in rendering's file.
    let cached: Vec<String> = std::fs::read_dir(f.dir.path().join("cache"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        cached
            .iter()
            .any(|n| n.starts_with("1-s1-") && n.ends_with(".orig.ass")),
        "{cached:?}"
    );

    let sidecars = crate::subtitles::sidecars(&f.dir.path().join("library/clip.mkv")).unwrap();
    let ass = sidecars.iter().find(|s| s.codec == "ass").unwrap();
    let beside = get(&f, &format!("{stream}/subtitles/x{}.ass", ass.id), None).await;
    assert_eq!(beside.status, StatusCode::OK);
    assert_eq!(
        beside.content_type.as_deref(),
        Some("text/x-ass; charset=utf-8")
    );
    assert_eq!(
        String::from_utf8(beside.body).unwrap(),
        STYLED.replace("Styled sign", "Caf\u{e9}")
    );
}

/// Puts `clip.{tag}.ass` beside the clip with these bytes and fetches it as ASS, with the file.
async fn beside_as_ass(
    f: &Fixture,
    stream: &str,
    tag: &str,
    bytes: &[u8],
) -> (Answer, crate::subtitles::Sidecar) {
    let name = format!("clip.{tag}.ass");
    std::fs::write(f.dir.path().join("library").join(&name), bytes).unwrap();
    let sidecars = crate::subtitles::sidecars(&f.dir.path().join("library/clip.mkv")).unwrap();
    let sidecar = sidecars.into_iter().find(|s| s.file == name).unwrap();
    let answer = get(f, &format!("{stream}/subtitles/x{}.ass", sidecar.id), None).await;
    (answer, sidecar)
}

/// The names in the cache.
fn cached(f: &Fixture) -> Vec<String> {
    std::fs::read_dir(f.dir.path().join("cache"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect()
}

#[tokio::test]
async fn a_file_beside_the_video_in_shift_jis_comes_back_in_utf8_converted_once() {
    let f = fixture().await;
    let stream = stream_url(&f).await;
    let text = STYLED
        .replace(
            "Styled sign",
            "\u{5b57}\u{5e55}\u{306e}\u{770b}\u{677f}\u{3067}\u{3059}\u{3002}",
        )
        .replace(
            "{\\k20}Ka{\\k30}ra",
            "{\\k20}\u{30ab}{\\k30}\u{30e9}\u{30aa}\u{30b1}",
        );
    let (shift_jis, _, unmappable) = encoding_rs::SHIFT_JIS.encode(&text);
    assert!(!unmappable && std::str::from_utf8(&shift_jis).is_err());
    let (answer, sidecar) = beside_as_ass(&f, &stream, "ja", &shift_jis).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(
        answer.content_type.as_deref(),
        Some("text/x-ass; charset=utf-8")
    );
    assert!(answer.nosniff);
    assert_eq!(String::from_utf8(answer.body).unwrap(), text);

    // Cached under the file's version; a changed file is converted again, and the old copy
    // is cleaned up.
    let first = format!("{}.orig.ass", crate::subtitles::sidecar_key(1, &sidecar));
    assert!(cached(&f).contains(&first), "{:?}", cached(&f));
    let changed = text.replace("\u{3067}\u{3059}", "\u{3060}\u{3063}\u{305f}");
    let (shift_jis, _, _) = encoding_rs::SHIFT_JIS.encode(&changed);
    let (answer, sidecar) = beside_as_ass(&f, &stream, "ja", &shift_jis).await;
    assert_eq!(String::from_utf8(answer.body).unwrap(), changed);
    let second = format!("{}.orig.ass", crate::subtitles::sidecar_key(1, &sidecar));
    assert_ne!(first, second);
    crate::subtitles::clean_cache(&f.pool, &f.dir.path().join("cache"))
        .await
        .unwrap();
    let left = cached(&f);
    assert!(left.contains(&second) && !left.contains(&first), "{left:?}");
}

#[tokio::test]
async fn a_utf8_file_beside_the_video_is_sent_byte_for_byte_less_a_byte_order_mark() {
    let f = fixture().await;
    let stream = stream_url(&f).await;
    let text = STYLED.replace("Styled sign", "Gr\u{fc}\u{df}e \u{2014} \u{5b57}\u{5e55}");

    let (plain, _) = beside_as_ass(&f, &stream, "de", text.as_bytes()).await;
    assert_eq!(plain.status, StatusCode::OK);
    assert_eq!(
        plain.content_type.as_deref(),
        Some("text/x-ass; charset=utf-8")
    );
    assert_eq!(plain.body, text.as_bytes());

    let marked = [b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat();
    let (unmarked, _) = beside_as_ass(&f, &stream, "es", &marked).await;
    assert_eq!(unmarked.status, StatusCode::OK);
    assert_eq!(
        unmarked.content_type.as_deref(),
        Some("text/x-ass; charset=utf-8")
    );
    assert_eq!(unmarked.body, text.as_bytes());
}

#[tokio::test]
async fn other_tracks_and_bad_links_get_no_ass() {
    let f = fixture().await;
    let stream = stream_url(&f).await;
    let sidecars = crate::subtitles::sidecars(&f.dir.path().join("library/clip.mkv")).unwrap();
    let srt = sidecars.iter().find(|s| s.codec == "subrip").unwrap();
    for track in [
        format!("x{}.ass", srt.id),
        "s0.ass".into(),
        "s9.ass".into(),
        "x00000000.ass".into(),
        "o00000000.ass".into(),
        "y1.ass".into(),
    ] {
        let answer = get(&f, &format!("{stream}/subtitles/{track}"), None).await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{track}");
    }
    let (status, unknown) = (
        get(&f, "/api/v1/stream/mlt_x/subtitles/s1.ass", None)
            .await
            .status,
        get(&f, "/api/v1/stream/mls_unknown/subtitles/s1.ass", None)
            .await
            .status,
    );
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(unknown, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn lists_and_serves_only_the_videos_fonts() {
    let f = fixture().await;
    let stream = stream_url(&f).await;
    let answer = get(&f, &format!("{stream}/fonts"), None).await;
    assert_eq!(answer.status, StatusCode::OK);
    let fonts: Value = serde_json::from_slice(&answer.body).unwrap();
    assert_eq!(
        fonts,
        json!([{
            "id": 2,
            "file_name": "Fancy.ttf",
            "mime_type": "font/ttf",
            "size": FONT.len(),
            "url": format!("{stream}/fonts/2"),
        }])
    );
    // The fields the API contract documents, no more and no fewer.
    let spec: Value = serde_json::from_str(crate::api::OPENAPI).unwrap();
    let documented: Vec<&String> = spec["components"]["schemas"]["AttachedFont"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    let mut sent: Vec<&String> = fonts[0].as_object().unwrap().keys().collect();
    sent.sort();
    let mut documented = documented;
    documented.sort();
    assert_eq!(sent, documented);

    let font = get(&f, &format!("{stream}/fonts/2"), None).await;
    assert_eq!(font.status, StatusCode::OK);
    assert_eq!(font.content_type.as_deref(), Some("font/ttf"));
    assert!(font.nosniff);
    assert_eq!(font.body, FONT);
}

#[tokio::test]
async fn fonts_are_given_only_by_their_id_and_under_a_current_link() {
    let f = fixture().await;
    let stream = stream_url(&f).await;
    // The text attachment, streams that aren't attachments, and anything that isn't an index.
    for id in [
        "3",
        "1",
        "0",
        "99",
        "-1",
        "+2",
        "02x",
        "123456",
        "..%2Ffont2",
    ] {
        let answer = get(&f, &format!("{stream}/fonts/{id}"), None).await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{id}");
    }
    for uri in [
        "/api/v1/stream/mls_unknown/fonts",
        "/api/v1/stream/mls_unknown/fonts/2",
    ] {
        assert_eq!(
            get(&f, uri, None).await.status,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
    assert_eq!(
        get(&f, "/api/v1/stream/mlt_x/fonts", None).await.status,
        StatusCode::BAD_REQUEST
    );

    // Signing out ends the link, fonts included.
    sqlx::query("DELETE FROM tokens WHERE id = ?")
        .bind(f.token_id)
        .execute(&f.pool)
        .await
        .unwrap();
    for uri in [format!("{stream}/fonts"), format!("{stream}/fonts/2")] {
        assert_eq!(
            get(&f, &uri, None).await.status,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn a_video_without_fonts_lists_none() {
    let f = fixture().await;
    let library = f.dir.path().join("library");
    let plain = crate::probe::testing::make_clip(f.dir.path());
    std::fs::rename(&plain, library.join("plain.mkv")).unwrap();
    let size = std::fs::metadata(library.join("plain.mkv")).unwrap().len() as i64;
    sqlx::query(
        "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation) VALUES (2, 1, 'plain.mkv', ?, 1, 1, 1, 1)",
    )
    .bind(size)
    .execute(&f.pool)
    .await
    .unwrap();
    let Ok((token, _)) =
        super::super::super::playback::create_play_session(&f.pool, f.token_id, 2, None).await
    else {
        panic!("creating a stream link failed");
    };
    let answer = get(&f, &format!("/api/v1/stream/{token}/fonts"), None).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&answer.body).unwrap(),
        json!([])
    );
    let font = get(&f, &format!("/api/v1/stream/{token}/fonts/2"), None).await;
    assert_eq!(font.status, StatusCode::NOT_FOUND);
}
