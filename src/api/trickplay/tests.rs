use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::auth::TokenKind;

struct Fixture {
    app: Router,
    pool: SqlitePool,
    token: String,
    token_id: i64,
    root: PathBuf,
    _dir: tempfile::TempDir,
}

/// File 1 with previews made: two sheets in its current version's folder.
async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'Film', 'Film', 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation) VALUES (1, 1, 'Film.mkv', 10, 7, 1, 1, 1);
         INSERT INTO media_trickplay (file_id, size, modified_at, made_at) VALUES (1, 10, 7, 0);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let root = dir.path().join("trickplay");
    let version = trickplay::version_dir(&root, 1, 10, 7);
    std::fs::create_dir_all(&version).unwrap();
    let index = Index {
        width: 320,
        height: 180,
        columns: 10,
        rows: 10,
        sheets: 2,
        times: (0..101).map(|i| f64::from(i) * 10.0).collect(),
    };
    std::fs::write(
        version.join("index.json"),
        serde_json::to_vec(&index).unwrap(),
    )
    .unwrap();
    for sheet in ["0.jpg", "1.jpg"] {
        std::fs::write(version.join(sheet), format!("jpeg {sheet}")).unwrap();
    }
    let user = auth::create_user(&pool, "alice", "alice-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    let (token_id, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
        .await
        .unwrap();
    Fixture {
        app: routes(pool.clone(), root.clone()),
        pool,
        token,
        token_id,
        root,
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
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, content_type, bytes.to_vec())
}

#[tokio::test]
async fn the_index_is_for_everyone_signed_in() {
    let f = fixture().await;
    let (status, _, body) = get(&f, "/api/v1/files/1/trickplay", true).await;
    assert_eq!(status, StatusCode::OK);
    let index: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(index["sheets"], 2);
    assert_eq!(index["times"][100], 1000.0);
    assert_eq!(index["columns"], 10);
    assert_eq!(
        get(&f, "/api/v1/files/1/trickplay", false).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&f, "/api/v1/files/2/trickplay", true).await.0,
        StatusCode::NOT_FOUND
    );
    // Made for a version that's gone: none.
    sqlx::query("UPDATE media_files SET size = 11 WHERE id = 1")
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        get(&f, "/api/v1/files/1/trickplay", true).await.0,
        StatusCode::NOT_FOUND
    );
    assert!(f.root.exists());
}

#[tokio::test]
async fn sheets_come_behind_a_stream_links_token() {
    let f = fixture().await;
    let Ok((token, _)) =
        crate::api::playback::create_play_session(&f.pool, f.token_id, 1, None).await
    else {
        panic!("creating a stream link failed");
    };
    let sheet = |n: &str| format!("/api/v1/stream/{token}/trickplay/{n}");
    let (status, content_type, body) = get(&f, &sheet("1.jpg"), false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("image/jpeg"));
    assert_eq!(body, b"jpeg 1.jpg");
    for missing in ["2.jpg", "x.jpg", "1.png", "-1.jpg", "01234567.jpg"] {
        assert_eq!(
            get(&f, &sheet(missing), false).await.0,
            StatusCode::NOT_FOUND,
            "{missing}"
        );
    }
    assert_eq!(
        get(&f, "/api/v1/stream/mls_unknown/trickplay/0.jpg", false)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&f, "/api/v1/stream/nope/trickplay/0.jpg", false)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}
