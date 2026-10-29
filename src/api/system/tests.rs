use axum::body::Body;
use axum::extract::Request;
use axum::http::header;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};
use crate::db;

async fn send(app: &Router, method: &str, uri: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
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
async fn admins_see_disk_space_and_tasks_and_run_one() {
    let pool = db::open_in_memory().await;
    let (films, data, transcode) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    sqlx::query(
        "INSERT INTO libraries (name, kind, path) VALUES ('films', 'movies', ?),
                                                         ('gone', 'shows', '/no/such/folder')",
    )
    .bind(films.path().to_str().unwrap())
    .execute(&pool)
    .await
    .unwrap();
    let mut tokens = Vec::new();
    for (name, admin) in [("alice", true), ("bob", false)] {
        let user = auth::create_user(&pool, name, format!("{name}-password"), admin)
            .await
            .unwrap()
            .unwrap();
        tokens.push(
            auth::issue_token(&pool, user.id, TokenKind::Session, "t")
                .await
                .unwrap()
                .1,
        );
    }
    let (admin, viewer) = (&tokens[0], &tokens[1]);
    let app = routes(pool, data.path().into(), transcode.path().into());

    let (status, places) = send(&app, "GET", "/api/v1/admin/system/disk", admin).await;
    assert_eq!(status, StatusCode::OK);
    let summary: Vec<(&str, &str, bool)> = places
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["kind"].as_str().unwrap(),
                p["name"].as_str().unwrap(),
                p["total"].as_u64().is_some_and(|t| t > 0),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("library", "films", true),
            ("library", "gone", false),
            ("data", "data", true),
            ("transcode", "transcode", true),
        ]
    );
    assert!(
        places[1]["error"].is_string(),
        "a folder that isn't there says why"
    );

    let (status, all) = send(&app, "GET", "/api/v1/admin/system/tasks", admin).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(all.as_array().unwrap().len(), tasks::TASKS.len());

    // Segments has run once here; cover colours never started (as with its service off).
    tasks::started("segments");
    tasks::finished("segments", None, None);
    let run = |name: &str| format!("/api/v1/admin/system/tasks/{name}/run");
    for (name, expected) in [
        ("segments", StatusCode::ACCEPTED),
        ("cover-colours", StatusCode::CONFLICT),
        ("backup", StatusCode::CONFLICT),
        ("nothing", StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            send(&app, "POST", &run(name), admin).await.0,
            expected,
            "{name}"
        );
    }
    for (method, uri) in [
        ("GET", "/api/v1/admin/system/disk".to_owned()),
        ("GET", "/api/v1/admin/system/tasks".to_owned()),
        ("POST", run("segments")),
    ] {
        let (status, _) = send(&app, method, &uri, viewer).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
}

/// A file of `bytes` in `dir`, last written `age` seconds ago.
fn aged(dir: &std::path::Path, name: &str, bytes: usize, age: u64) {
    let path = dir.join(name);
    std::fs::write(&path, vec![0; bytes]).unwrap();
    let at = std::time::SystemTime::now() - std::time::Duration::from_secs(age);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

#[tokio::test]
async fn admins_see_the_backups_and_the_latest_try_of_each_kind() {
    let pool = db::open_in_memory().await;
    let (data, transcode) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut tokens = Vec::new();
    for (name, admin) in [("alice", true), ("bob", false)] {
        let user = auth::create_user(&pool, name, format!("{name}-password"), admin)
            .await
            .unwrap()
            .unwrap();
        tokens.push(
            auth::issue_token(&pool, user.id, TokenKind::Session, "t")
                .await
                .unwrap()
                .1,
        );
    }
    let (admin, viewer) = (&tokens[0], &tokens[1]);
    let app = routes(pool.clone(), data.path().into(), transcode.path().into());
    let uri = "/api/v1/admin/system/backups";

    // No folder yet: no copies, and nothing tried.
    let (status, none) = send(&app, "GET", uri, admin).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(none["copies"], serde_json::json!([]));
    assert_eq!(none["latest"], serde_json::json!([]));
    assert_eq!(none["kept"]["daily"], backup::DAILY_KEPT);

    let dir = data.path().join(backup::FOLDER);
    std::fs::create_dir(&dir).unwrap();
    aged(&dir, "daily-2026-10-27.db", 10, 2 * 86_400);
    aged(&dir, "daily-2026-10-28.db", 20, 86_400);
    aged(&dir, "by-hand-2026-10-28-120000.db", 30, 3_600);
    aged(
        &dir,
        "before-migrating-from-0075-2026-10-20-101500.db",
        40,
        9 * 86_400,
    );
    aged(&dir, "kept-aside.db", 50, 10 * 86_400);
    // Left out: a copy being written, a file that isn't a copy, a folder and a link.
    aged(&dir, ".daily-2026-10-29.db.part", 5, 0);
    aged(&dir, "notes.txt", 5, 0);
    std::fs::create_dir(dir.join("old.db")).unwrap();
    std::os::unix::fs::symlink(dir.join("daily-2026-10-28.db"), dir.join("link.db")).unwrap();

    let failed: anyhow::Result<backup::Copy> = Err(anyhow::anyhow!("disk full at /a/secret/path"));
    backup::record(&pool, backup::Kind::Daily, &failed)
        .await
        .unwrap();
    let made = backup::Copy {
        file: "by-hand-2026-10-28-120000.db".into(),
        bytes: 30,
    };
    backup::record(&pool, backup::Kind::ByHand, &Ok(made))
        .await
        .unwrap();

    let (status, all) = send(&app, "GET", uri, admin).await;
    assert_eq!(status, StatusCode::OK);
    let copies: Vec<(&str, &str, u64)> = all["copies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["file"].as_str().unwrap(),
                c["kind"].as_str().unwrap(),
                c["bytes"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        copies,
        [
            ("by-hand-2026-10-28-120000.db", "by_hand", 30),
            ("daily-2026-10-28.db", "daily", 20),
            ("daily-2026-10-27.db", "daily", 10),
            (
                "before-migrating-from-0075-2026-10-20-101500.db",
                "before_migrating",
                40
            ),
            ("kept-aside.db", "other", 50),
        ]
    );
    let latest: Vec<(&str, bool)> = all["latest"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["kind"].as_str().unwrap(), a["ok"].as_bool().unwrap()))
        .collect();
    assert_eq!(latest.len(), 2);
    assert!(latest.contains(&("daily", false)) && latest.contains(&("by_hand", true)));
    assert!(
        !all.to_string().contains("secret"),
        "why a copy failed stays out of the answer"
    );

    let (status, _) = send(&app, "GET", uri, viewer).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
