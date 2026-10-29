//! Downloads (admins only): grabbing an accepted search result, the list of downloads, and
//! testing the seedbox connection.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router, middleware};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::downloads::{self, Download, NotRemoved, Pipeline, Refused};
use crate::seedbox::TestReport;
use crate::upgrades::{self, Emptied, NotRestored, Trashed};

#[derive(Clone)]
struct DownloadState {
    pool: SqlitePool,
    pipeline: Option<Arc<Pipeline>>,
    /// For Soulseek results.
    slskd: Option<Arc<crate::slskd::Client>>,
}

/// The download routes, for admins only. Without `pipeline` (downloads off) or `slskd` (no
/// Soulseek), the routes that need them refuse.
pub fn routes(
    pool: SqlitePool,
    pipeline: Option<Arc<Pipeline>>,
    slskd: Option<Arc<crate::slskd::Client>>,
) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/releases/{id}/grab", post(grab))
        .route("/api/v1/downloads", get(list))
        .route("/api/v1/downloads/{id}", delete(remove))
        .route("/api/v1/downloads/clear-failed", post(clear_failed))
        .route("/api/v1/seedbox/test", post(test_seedbox))
        .route("/api/v1/trash", get(trash).delete(empty_trash))
        .route("/api/v1/trash/{id}", delete(delete_trashed))
        .route("/api/v1/trash/{id}/restore", post(restore))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(DownloadState {
            pool,
            pipeline,
            slskd,
        })
}

async fn grab(
    State(state): State<DownloadState>,
    Path(id): Path<i64>,
) -> Result<Json<Download>, ApiError> {
    let soulseek: bool =
        sqlx::query_scalar("SELECT soulseek IS NOT NULL FROM release_decisions WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .unwrap_or(false);
    if soulseek {
        let Some(slskd) = state.slskd.as_deref() else {
            return Err(refused(Refused::Soulseek));
        };
        let grabbed = crate::soulseek_downloads::grab(&state.pool, slskd, id).await?;
        return grabbed.map(Json).map_err(refused);
    }
    let Some(pipeline) = state.pipeline.as_deref() else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "downloads are off; MELETE_DOWNLOADS=on with Deluge's URL and password turns them on",
        ));
    };
    downloads::grab(&state.pool, &pipeline.deluge, &pipeline.fetcher, id)
        .await?
        .map(Json)
        .map_err(refused)
}

fn refused(refused: Refused) -> ApiError {
    let status = match refused {
        Refused::NoRelease => StatusCode::NOT_FOUND,
        Refused::NotAccepted(_)
        | Refused::NoLink
        | Refused::Already(_)
        | Refused::Soulseek
        | Refused::Blocklisted => StatusCode::CONFLICT,
        Refused::Fetch(_) | Refused::Deluge(_) | Refused::Slskd(_) => StatusCode::BAD_GATEWAY,
    };
    ApiError::new(status, refused.to_string())
}

async fn test_seedbox(State(state): State<DownloadState>) -> Result<Json<TestReport>, ApiError> {
    let Some(seedbox) = state.pipeline.as_deref().and_then(|p| p.seedbox.as_ref()) else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "the seedbox isn't set up; MELETE_DOWNLOADS=on with the MELETE_SEEDBOX_* settings sets it up",
        ));
    };
    let deluge_paths: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT save_path FROM downloads WHERE state = 'completed' AND save_path IS NOT NULL
         ORDER BY save_path",
    )
    .fetch_all(&state.pool)
    .await?;
    seedbox
        .test(&deluge_paths)
        .await
        .map(Json)
        .map_err(|err| ApiError::new(StatusCode::BAD_GATEWAY, err.to_string()))
}

#[derive(serde::Deserialize)]
struct RemoveOptions {
    /// Also remove the torrent from Deluge (what it downloaded stays on the seedbox).
    #[serde(default)]
    from_deluge: bool,
    /// Remove the torrent from Deluge and have Deluge delete what it downloaded on the
    /// seedbox, for good. Never anything in a library.
    #[serde(default)]
    delete_data: bool,
}

async fn remove(
    State(state): State<DownloadState>,
    Path(id): Path<i64>,
    Query(options): Query<RemoveOptions>,
) -> Result<StatusCode, ApiError> {
    let removed = match downloads::removable(&state.pool, id).await? {
        Ok(removed) => removed,
        Err(NotRemoved::NotFound) => {
            return Err(ApiError::new(StatusCode::NOT_FOUND, "no such download"));
        }
        Err(NotRemoved::Busy(now)) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                match now.as_str() {
                    "transferring" => {
                        "it's being copied home; remove it once that's done or failed"
                    }
                    "imported" => "it's in the library already",
                    _ => "a Soulseek download can be removed once it has failed",
                },
            ));
        }
    };
    // Deluge first: if it can't be told, nothing changes here, and the admin can try again.
    // Deleting the data is Deluge's own remove-with-data, on the seedbox only; nothing in a
    // library is touched, and one being copied home or imported was refused above.
    if options.from_deluge || options.delete_data {
        let Some(hash) = &removed.infohash else {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "it isn't a torrent, so Deluge doesn't have it",
            ));
        };
        let Some(pipeline) = &state.pipeline else {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "downloading is off, so Melete can't reach Deluge; remove it there yourself",
            ));
        };
        pipeline
            .deluge
            .remove(hash, options.delete_data)
            .await
            .map_err(|err| ApiError::new(StatusCode::BAD_GATEWAY, format!("Deluge: {err}")))?;
    }
    if downloads::remove(&state.pool, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(
            StatusCode::CONFLICT,
            "it moved on while being removed; look again",
        ))
    }
}

#[derive(serde::Serialize)]
struct Cleared {
    removed: u64,
}

#[derive(serde::Deserialize)]
struct ClearOptions {
    /// Only failed downloads for movies, series or music.
    #[serde(rename = "for")]
    only: Option<downloads::For>,
}

async fn clear_failed(
    State(state): State<DownloadState>,
    Query(options): Query<ClearOptions>,
) -> Result<Json<Cleared>, ApiError> {
    Ok(Json(Cleared {
        removed: downloads::clear_failed(&state.pool, options.only).await?,
    }))
}

async fn list(State(state): State<DownloadState>) -> Result<Json<Vec<Download>>, ApiError> {
    Ok(Json(downloads::list(&state.pool).await?))
}

async fn trash(State(state): State<DownloadState>) -> Result<Json<Vec<Trashed>>, ApiError> {
    Ok(Json(upgrades::list(&state.pool).await?))
}

/// Empties the trash now rather than after 30 days.
async fn empty_trash(State(state): State<DownloadState>) -> Result<Json<Emptied>, ApiError> {
    Ok(Json(
        upgrades::delete_now(&state.pool, None)
            .await?
            .unwrap_or_default(),
    ))
}

/// Deletes one file from the trash now.
async fn delete_trashed(
    State(state): State<DownloadState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    match upgrades::delete_now(&state.pool, Some(id)).await? {
        None => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such file in the trash",
        )),
        Some(Emptied { deleted: 1, .. }) => Ok(StatusCode::NO_CONTENT),
        Some(_) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "the trash doesn't hold the file it put there now (or the library's share is away); \
             it was left alone",
        )),
    }
}

async fn restore(
    State(state): State<DownloadState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    match upgrades::restore(&state.pool, id).await? {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(NotRestored::NotInTrash) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such file in the trash",
        )),
        Err(NotRestored::Failed(why)) => Err(ApiError::new(StatusCode::CONFLICT, why)),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::header;
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::config::Secret;
    use crate::db;
    use crate::deluge::{self, testing};

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

    async fn admin(pool: &SqlitePool) -> String {
        let user = auth::create_user(pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        auth::issue_token(pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap()
            .1
    }

    #[tokio::test]
    async fn grabbing_needs_downloads_turned_on() {
        let pool = db::open_in_memory().await;
        let token = admin(&pool).await;
        let app = routes(pool, None, None);
        let (status, body) = send(&app, "POST", "/api/v1/releases/1/grab", &token).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("MELETE_DOWNLOADS=on")
        );
        let (status, body) = send(&app, "GET", "/api/v1/downloads", &token).await;
        assert_eq!((status, body), (StatusCode::OK, serde_json::json!([])));
    }

    #[tokio::test]
    async fn a_soulseek_result_goes_to_slskd_even_with_deluge_off() {
        let pool = db::open_in_memory().await;
        let token = admin(&pool).await;
        sqlx::raw_sql(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
                 VALUES (1, 'mb-a', 'Radiohead', 1, 'all', 'lidarr', 0);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at)
                 VALUES (7, 1, 'rg', 'OK Computer', 1, 'lidarr', 0);
             INSERT INTO searches (id, wanted_album_id, searched_at, indexers) VALUES (1, 7, 0, '[]');
             INSERT INTO release_decisions (id, search_id, title, accepted, decision, soulseek)
                 VALUES (5, 1, 'Radiohead - OK Computer', 1, '{\"accepted\":true,\"reasons\":[]}',
                         '{\"username\":\"alice\",\"folder\":\"a\",\"files\":[{\"filename\":\"a\\\\01.flac\",\"size\":9}]}');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (slskd, seen) = crate::slskd::testing::start(serde_json::json!([]), 1).await;
        let app = routes(pool.clone(), None, Some(Arc::new(slskd)));
        let (status, body) = send(&app, "POST", "/api/v1/releases/5/grab", &token).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["source"], "soulseek");
        assert_eq!(body["infohash"], serde_json::Value::Null);
        assert_eq!(seen.lock().unwrap().enqueued.len(), 1);
        let (status, body) = send(&app, "GET", "/api/v1/downloads", &token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body[0]["source"], "soulseek");

        // Without slskd set up, it says so, rather than that Deluge is off.
        let app = routes(pool, None, None);
        let (status, body) = send(&app, "POST", "/api/v1/releases/5/grab", &token).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("slskd isn't set up"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn grabs_through_deluge_and_lists_the_download() {
        let pool = db::open_in_memory().await;
        let token = admin(&pool).await;
        // An indexer serving the test torrent.
        let feed = axum::Router::new().fallback(|| async {
            include_bytes!("../../tests/fixtures/deluge/movie.torrent").to_vec()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let link = format!("http://{}/t", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, feed).await.unwrap() });
        sqlx::query("INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at) VALUES (1, 1, 'M', 1, 'radarr', 0)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO searches (id, wanted_movie_id, searched_at, indexers) VALUES (1, 1, 0, '[]')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO release_decisions (id, search_id, title, download_url, accepted, decision)
             VALUES (7, 1, 'Some.Movie.2024.1080p.WEB-DL.H264-GRP', ?, 1, '{\"accepted\":true,\"rejections\":[]}')",
        )
        .bind(link)
        .execute(&pool)
        .await
        .unwrap();
        let (url, _fake) = testing::start().await;
        let pipeline = Arc::new(Pipeline {
            deluge: deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
            fetcher: downloads::Fetcher::new().unwrap(),
            seedbox: None,
        });
        let app = routes(pool, Some(pipeline), None);
        let (status, download) = send(&app, "POST", "/api/v1/releases/7/grab", &token).await;
        assert_eq!(status, StatusCode::OK, "{download}");
        assert_eq!(download["state"], "downloading");
        let (status, again) = send(&app, "POST", "/api/v1/releases/7/grab", &token).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            again["error"],
            format!("it's already download {}", download["id"])
        );
        let (_, listed) = send(&app, "GET", "/api/v1/downloads", &token).await;
        assert_eq!(listed, serde_json::json!([download]));
        let (status, _) = send(&app, "POST", "/api/v1/releases/99/grab", &token).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn removing_a_download_can_delete_its_data_on_the_seedbox_only() {
        let pool = db::open_in_memory().await;
        let token = admin(&pool).await;
        let hash = |n: u8| format!("{:040x}", n);
        sqlx::query("INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at) VALUES (1, 1, 'M', 1, 'manual', 0)")
            .execute(&pool)
            .await
            .unwrap();
        let (url, fake) = testing::start().await;
        fake.lock().unwrap().connected = true;
        for (id, state) in [
            (1, "completed"),
            (2, "transferring"),
            (3, "imported"),
            (4, "downloading"),
        ] {
            sqlx::query(
                "INSERT INTO downloads (id, infohash, title, wanted_movie_id, decision, state, added_at, updated_at)
                 VALUES (?, ?, 'M.2024', 1, '{}', ?, 0, 0)",
            )
            .bind(id)
            .bind(hash(id))
            .bind(state)
            .execute(&pool)
            .await
            .unwrap();
            fake.lock()
                .unwrap()
                .torrents
                .insert(hash(id), testing::status("M", "Seeding", 100.0, true));
        }
        let pipeline = Arc::new(Pipeline {
            deluge: deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
            fetcher: downloads::Fetcher::new().unwrap(),
            seedbox: None,
        });
        let app = routes(pool.clone(), Some(pipeline), None);

        // Being copied home, or in the library already: refused, and Deluge isn't asked.
        for id in [2, 3] {
            let (status, _) = send(
                &app,
                "DELETE",
                &format!("/api/v1/downloads/{id}?delete_data=true"),
                &token,
            )
            .await;
            assert_eq!(status, StatusCode::CONFLICT, "{id}");
        }
        assert!(fake.lock().unwrap().data_removed.is_empty());
        assert_eq!(fake.lock().unwrap().torrents.len(), 4);

        // Removing it from Deluge alone keeps its data.
        let (status, _) = send(
            &app,
            "DELETE",
            "/api/v1/downloads/4?from_deluge=true",
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(fake.lock().unwrap().data_removed.is_empty());

        // Finished on the seedbox and not yet copied home: Deluge deletes it with its data.
        let (status, body) = send(
            &app,
            "DELETE",
            "/api/v1/downloads/1?delete_data=true",
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        assert_eq!(fake.lock().unwrap().data_removed, [hash(1)]);
        assert!(!fake.lock().unwrap().torrents.contains_key(&hash(1)));
        let left: Vec<i64> = sqlx::query_scalar("SELECT id FROM downloads ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(left, [2, 3]);
    }

    #[tokio::test]
    async fn removes_downloads_and_clears_failed_ones_but_never_mid_copy() {
        let pool = db::open_in_memory().await;
        let token = admin(&pool).await;
        let hash = |n: u8| format!("{:040x}", n);
        sqlx::query("INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at) VALUES (1, 1, 'M', 1, 'manual', 0)")
            .execute(&pool)
            .await
            .unwrap();
        for (id, state) in [
            (1, "downloading"),
            (2, "transferring"),
            (3, "failed"),
            (4, "failed"),
            (5, "imported"),
            (6, "downloading"),
        ] {
            sqlx::query(
                "INSERT INTO downloads (id, infohash, title, wanted_movie_id, decision, state, added_at, updated_at)
                 VALUES (?, ?, 'M.2024', 1, '{}', ?, 0, 0)",
            )
            .bind(id)
            .bind(hash(id))
            .bind(state)
            .execute(&pool)
            .await
            .unwrap();
        }
        let (url, fake) = testing::start().await;
        fake.lock().unwrap().connected = true;
        fake.lock()
            .unwrap()
            .torrents
            .insert(hash(1), testing::status("M", "Downloading", 10.0, false));
        let pipeline = Arc::new(Pipeline {
            deluge: deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
            fetcher: downloads::Fetcher::new().unwrap(),
            seedbox: None,
        });
        let app = routes(pool.clone(), Some(pipeline), None);
        let ids = |listed: &Value| -> Vec<i64> {
            listed
                .as_array()
                .unwrap()
                .iter()
                .map(|d| d["id"].as_i64().unwrap())
                .collect()
        };

        // Downloading, and removed from Deluge too; what it downloaded stays on the seedbox.
        let (status, body) = send(
            &app,
            "DELETE",
            "/api/v1/downloads/1?from_deluge=true",
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        assert!(fake.lock().unwrap().torrents.is_empty());
        assert!(
            fake.lock()
                .unwrap()
                .calls
                .contains(&"core.remove_torrent".to_owned())
        );
        // Deluge no longer having one isn't a failure.
        let (status, _) = send(
            &app,
            "DELETE",
            "/api/v1/downloads/6?from_deluge=true",
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        // Mid-copy and imported ones stay.
        for id in [2, 5] {
            let (status, _) =
                send(&app, "DELETE", &format!("/api/v1/downloads/{id}"), &token).await;
            assert_eq!(status, StatusCode::CONFLICT, "{id}");
        }
        let (status, _) = send(&app, "DELETE", "/api/v1/downloads/99", &token).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        // A failed one on its own, then the rest of the failed ones at once.
        let (status, _) = send(&app, "DELETE", "/api/v1/downloads/3", &token).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        // Only series' failed downloads: none of these.
        let (status, cleared) = send(
            &app,
            "POST",
            "/api/v1/downloads/clear-failed?for=series",
            &token,
        )
        .await;
        assert_eq!(
            (status, cleared),
            (StatusCode::OK, serde_json::json!({ "removed": 0 }))
        );
        let (status, cleared) = send(
            &app,
            "POST",
            "/api/v1/downloads/clear-failed?for=movies",
            &token,
        )
        .await;
        assert_eq!(
            (status, cleared),
            (StatusCode::OK, serde_json::json!({ "removed": 1 }))
        );
        let (_, listed) = send(&app, "GET", "/api/v1/downloads", &token).await;
        assert_eq!(ids(&listed), [5, 2]);
        // The wanted title stays wanted.
        let wanted: i64 = sqlx::query_scalar("SELECT count(*) FROM wanted_movies")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(wanted, 1);

        // With downloading off, Deluge can't be asked, and nothing is removed.
        sqlx::query("UPDATE downloads SET state = 'failed' WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();
        let app = routes(pool, None, None);
        let (status, _) = send(
            &app,
            "DELETE",
            "/api/v1/downloads/2?from_deluge=true",
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (_, listed) = send(&app, "GET", "/api/v1/downloads", &token).await;
        assert_eq!(ids(&listed), [5, 2]);
    }

    #[tokio::test]
    async fn tests_the_seedbox_and_its_folders() {
        let pool = db::open_in_memory().await;
        let token = admin(&pool).await;
        let (status, body) = send(
            &routes(pool.clone(), None, None),
            "POST",
            "/api/v1/seedbox/test",
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body["error"].as_str().unwrap().contains("MELETE_SEEDBOX_"));

        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("Completed")).unwrap();
        let server = crate::seedbox::testing::start(root.path()).await;
        sqlx::query(
            "INSERT INTO downloads (infohash, title, decision, state, save_path, added_at, updated_at)
             VALUES ('a11bae70b2fdd9eb95b63e746f8594dce2650b0c', 'M', '{}', 'completed', '/home/user/Completed', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (url, _fake) = testing::start().await;
        let pipeline = Arc::new(Pipeline {
            deluge: deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
            fetcher: downloads::Fetcher::new().unwrap(),
            seedbox: Some(crate::seedbox::Seedbox::new(crate::config::Seedbox {
                host: "127.0.0.1".into(),
                port: server.port,
                user: "user".into(),
                password: Secret::for_tests("pw"),
                host_key: server.fingerprint.clone(),
                deluge_dir: "/home/user/Completed".into(),
                sftp_dir: "/Completed".into(),
            })),
        });
        let (status, report) = send(
            &routes(pool, Some(pipeline), None),
            "POST",
            "/api/v1/seedbox/test",
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{report}");
        assert_eq!(
            report,
            serde_json::json!({ "host_key": server.fingerprint, "folder": "/Completed", "entries": 0,
                                "completed": [{ "deluge_path": "/home/user/Completed", "sftp_path": "/Completed", "found": true }] })
        );
    }
}
