//! A file's intro, credits, recap and preview, for a Skip button.

use std::sync::Arc;

use axum::extract::{Path, State};
#[cfg(test)]
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use crate::probe::{Chapter, chapters_of, named_recap_and_preview, named_segments};
use crate::segments::{self, Found, Segment};

/// The intro and credits route, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/files/{id}/segments", get(file_segments))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(pool)
}

/// A segment as the API gives it, saying how it was found (`source`).
fn segment(found: Option<(Segment, Found)>) -> Value {
    found.map_or(Value::Null, |(s, how)| {
        let source = match how {
            Found::Audio => "audio",
            Found::Ending => "ending",
        };
        json!({ "start_seconds": s.start, "end_seconds": s.end, "source": source })
    })
}

/// The file's intro and credits, each null when none was found; `scanned` says whether its
/// current version has been looked at yet.
async fn file_segments(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    super::playback::require_file(&pool, &current.access, id).await?;
    Ok(Json(of_file(&pool, id).await?))
}

/// A segment from a chapter the file's author named, which says so (`source`).
fn from_chapter(chapter: &Chapter) -> Value {
    json!({ "start_seconds": chapter.start, "end_seconds": chapter.end, "source": "chapter" })
}

/// A file's intro and credits as the API gives them, each saying how it was found; file info and
/// play answers carry it too. Chapters named as an intro or credits come ahead of what listening
/// found: the file's author marked them.
pub(super) async fn of_file(pool: &SqlitePool, id: i64) -> Result<Value, ApiError> {
    let found = segments::sourced(pool, id).await?;
    let (intro, credits) = found.unwrap_or((None, None));
    let intro = intro.map(|s| (s, Found::Audio));
    let chapters = chapters_of(pool, id).await?;
    let (intro_chapter, credits_chapter) = named_segments(&chapters);
    // Only chapters name a recap or a preview: listening finds neither.
    let (recap, preview) = named_recap_and_preview(&chapters);
    Ok(json!({
        "scanned": found.is_some(),
        "intro": intro_chapter.map_or_else(|| segment(intro), from_chapter),
        "credits": credits_chapter.map_or_else(|| segment(credits), from_chapter),
        "recap": recap.map_or(Value::Null, from_chapter),
        "preview": preview.map_or(Value::Null, from_chapter),
    }))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;

    #[tokio::test]
    async fn segments_of_the_files_current_version_only() {
        let pool = db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'shows', 'shows', '/s');
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
                 VALUES (1, 1, 'Show', 'Show', 1);
             INSERT INTO media_files (id, library_id, rel_path, size, modified_at, series_id,
                 season, episode, first_seen_generation, seen_generation, missing_since) VALUES
                 (1, 1, 'S01E01.mkv', 10, 1, 1, 1, 1, 1, 1, NULL),
                 (2, 1, 'S01E02.mkv', 10, 1, 1, 1, 2, 1, 1, NULL),
                 (3, 1, 'S01E03.mkv', 10, 1, 1, 1, 3, 1, 1, 5);
             INSERT INTO media_segment_scans (file_id, size, modified_at, scanned_at)
                 VALUES (1, 10, 1, 0);
             INSERT INTO media_segments (file_id, kind, start_seconds, end_seconds)
                 VALUES (1, 'intro', 20.5, 110.0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = auth::create_user(&pool, "viewer", "viewer-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap();
        let app = routes(pool.clone());
        let get = |uri: &str, token: Option<&str>| {
            let mut request = Request::get(uri);
            if let Some(token) = token {
                request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
            }
            let request = request.body(Body::empty()).unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                let status = response.status();
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                (
                    status,
                    serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
                )
            }
        };
        assert_eq!(
            get("/api/v1/files/1/segments", Some(&token)).await,
            (
                StatusCode::OK,
                json!({
                    "scanned": true,
                    "intro": { "start_seconds": 20.5, "end_seconds": 110.0, "source": "audio" },
                    "credits": null,
                    "recap": null,
                    "preview": null,
                })
            )
        );
        assert_eq!(
            get("/api/v1/files/2/segments", Some(&token)).await.1,
            json!({ "scanned": false, "intro": null, "credits": null, "recap": null,
                    "preview": null })
        );
        sqlx::query("UPDATE media_files SET size = 11 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            get("/api/v1/files/1/segments", Some(&token)).await.1,
            json!({ "scanned": false, "intro": null, "credits": null, "recap": null,
                    "preview": null }),
            "found in a version that's gone"
        );
        for uri in ["/api/v1/files/3/segments", "/api/v1/files/9/segments"] {
            assert_eq!(
                get(uri, Some(&token)).await.0,
                StatusCode::NOT_FOUND,
                "{uri}"
            );
        }
        assert_eq!(
            get("/api/v1/files/1/segments", None).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn named_chapters_come_ahead_of_what_listening_found() {
        let pool = db::open_in_memory().await;
        let chapters = r#"[{"start_seconds": 0.0, "end_seconds": 30.0, "title": "Recap"},
            {"start_seconds": 30.0, "end_seconds": 120.0, "title": "OP"},
            {"start_seconds": 120.0, "end_seconds": 1300.0, "title": "Part A"},
            {"start_seconds": 1300.0, "end_seconds": 1390.0, "title": "ED"}]"#;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'shows', 'shows', '/s');
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
                 VALUES (1, 1, 'Show', 'Show', 1);
             INSERT INTO media_files (id, library_id, rel_path, size, modified_at, series_id,
                 season, episode, first_seen_generation, seen_generation) VALUES
                 (1, 1, 'S01E01.mkv', 10, 1, 1, 1, 1, 1, 1),
                 (2, 1, 'S01E02.mkv', 10, 1, 1, 1, 2, 1, 1);
             INSERT INTO media_segment_scans (file_id, size, modified_at, scanned_at)
                 VALUES (1, 10, 1, 0);
             INSERT INTO media_segments (file_id, kind, start_seconds, end_seconds)
                 VALUES (1, 'intro', 31.5, 118.0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        for file in [1, 2] {
            sqlx::query(
                "INSERT INTO media_probes (file_id, size, modified_at, probed_at, info, chapters)
                 VALUES (?, 10, 1, 0, '{}', ?)",
            )
            .bind(file)
            .bind(chapters)
            .execute(&pool)
            .await
            .unwrap();
        }
        let intro = json!({ "start_seconds": 30.0, "end_seconds": 120.0, "source": "chapter" });
        let credits =
            json!({ "start_seconds": 1300.0, "end_seconds": 1390.0, "source": "chapter" });
        let recap = json!({ "start_seconds": 0.0, "end_seconds": 30.0, "source": "chapter" });
        assert_eq!(
            of_file(&pool, 1).await.ok().unwrap(),
            json!({ "scanned": true, "intro": intro, "credits": credits, "recap": recap,
                    "preview": null })
        );
        assert_eq!(
            of_file(&pool, 2).await.ok().unwrap(),
            json!({ "scanned": false, "intro": intro, "credits": credits, "recap": recap,
                    "preview": null }),
            "reading chapters needs no listening"
        );
        // Chapters of a version that's gone don't count.
        sqlx::query("UPDATE media_files SET size = 11 WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            of_file(&pool, 2).await.ok().unwrap(),
            json!({ "scanned": false, "intro": null, "credits": null, "recap": null,
                    "preview": null })
        );
    }

    #[tokio::test]
    async fn every_segment_says_how_it_was_found() {
        let pool = db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
                 VALUES (1, 1, 'Film', 'Film', 1);
             INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation) VALUES (1, 1, 'Film.mkv', 10, 1, 1, 1, 1);
             INSERT INTO media_credit_scans (file_id, size, modified_at, scanned_at,
                 start_seconds, end_seconds) VALUES (1, 10, 1, 0, 5400.0, 5600.0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            of_file(&pool, 1).await.ok().unwrap(),
            json!({ "scanned": true, "intro": null, "credits":
                { "start_seconds": 5400.0, "end_seconds": 5600.0, "source": "ending" },
                "recap": null, "preview": null })
        );
    }
}
