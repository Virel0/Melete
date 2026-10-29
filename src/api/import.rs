//! Importing Radarr's and Sonarr's lists (admins only), and the wanted list they fill.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router, middleware};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_admin};
use crate::arr::{ArrClient, Rejected, WrongApp};
use crate::wanted_edit::{self, EpisodeNotSet, Kind, NotRemoved};
use crate::{import, metadata, tmdb};

mod albums;
mod artists;
mod choosing;
mod importing;
mod monitoring;
mod video;

use albums::*;
use artists::*;
use choosing::*;
pub(super) use importing::*;
use monitoring::*;
use video::*;

#[derive(Clone)]
struct ImportState {
    pool: SqlitePool,
    tmdb: Option<Arc<tmdb::Client>>,
}

/// The import and wanted-list routes, for admins only.
pub fn routes(pool: SqlitePool, tmdb: Option<Arc<tmdb::Client>>) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/import/radarr", post(import_radarr))
        .route("/api/v1/wanted/movies", get(wanted_movies))
        .route("/api/v1/import/sonarr", post(import_sonarr))
        .route("/api/v1/wanted/series", get(wanted_series))
        .route("/api/v1/import/lidarr", post(import_lidarr))
        .route("/api/v1/wanted/artists", get(wanted_artists))
        .route("/api/v1/wanted/artists/follows", get(follows))
        .route(
            "/api/v1/wanted/movies/{id}",
            delete(|s, u, p| unwant(Kind::Movie, s, u, p)),
        )
        .route(
            "/api/v1/wanted/series/{id}",
            delete(|s, u, p| unwant(Kind::Series, s, u, p)),
        )
        .route(
            "/api/v1/wanted/artists/{id}",
            delete(|s, u, p| unwant(Kind::Artist, s, u, p)).put(change_artist),
        )
        .route(
            "/api/v1/wanted/series/{id}/seasons/{season}",
            put(monitor_season),
        )
        .route(
            "/api/v1/wanted/series/{id}/seasons/{season}/episodes/{episode}",
            put(monitor_episode),
        )
        .route(
            "/api/v1/wanted/albums/{id}",
            delete(|s, u, p| unwant(Kind::Album, s, u, p)),
        )
        .route(
            "/api/v1/wanted/movies/{id}/monitored",
            put(|s, p, j| monitor(Kind::Movie, s, p, j)),
        )
        .route(
            "/api/v1/wanted/series/{id}/monitored",
            put(|s, p, j| monitor(Kind::Series, s, p, j)),
        )
        .route(
            "/api/v1/wanted/artists/{id}/monitored",
            put(|s, p, j| monitor(Kind::Artist, s, p, j)),
        )
        .route(
            "/api/v1/wanted/albums/{id}/monitored",
            put(|s, p, j| monitor(Kind::Album, s, p, j)),
        )
        .route("/api/v1/wanted/artists/{id}/albums", get(wanted_albums))
        .route(
            "/api/v1/wanted/movies/{id}/profile",
            put(choose_movie_profile),
        )
        .route(
            "/api/v1/wanted/series/{id}/profile",
            put(choose_series_profile),
        )
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(ImportState { pool, tmdb })
}

#[derive(Deserialize)]
struct Monitored {
    monitored: bool,
}

/// Monitors a wanted entry or stops: monitored ones are searched for and grabbed.
async fn monitor(
    kind: Kind,
    State(state): State<ImportState>,
    Path(id): Path<i64>,
    Json(body): Json<Monitored>,
) -> Result<StatusCode, ApiError> {
    if wanted_edit::set_monitored(&state.pool, kind, id, body.monitored).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(StatusCode::NOT_FOUND, "no such wanted entry"))
    }
}

/// Takes a wanted entry off the list; nothing on disk is touched.
async fn unwant(
    kind: Kind,
    State(state): State<ImportState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    // Who took it off is kept, to show beside it under Taken off.
    match wanted_edit::remove_by(&state.pool, kind, id, Some(current.user.id)).await? {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(NotRemoved::NotFound) => {
            Err(ApiError::new(StatusCode::NOT_FOUND, "no such wanted entry"))
        }
        Err(NotRemoved::Downloading(n)) => Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("{n} download(s) for it are on their way; remove them under Downloads first"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::header;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;
    use crate::import::testing::FakeArr;
    use crate::library::{LibraryConfig, LibraryKind, sync_config};

    struct Fixture {
        app: Router,
        admin: String,
        viewer: String,
    }

    async fn fixture() -> Fixture {
        let pool = db::open_in_memory().await;
        let config = LibraryConfig {
            name: "movies".into(),
            kind: LibraryKind::Movies,
            path: "/media/movies".into(),
        };
        sync_config(&pool, &[config]).await.unwrap();
        let admin = auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let viewer = auth::create_user(&pool, "viewer", "viewer-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, admin) = auth::issue_token(&pool, admin.id, TokenKind::Session, "t")
            .await
            .unwrap();
        let (_, viewer) = auth::issue_token(&pool, viewer.id, TokenKind::Session, "t")
            .await
            .unwrap();
        Fixture {
            app: routes(pool, None),
            admin,
            viewer,
        }
    }

    async fn send(
        f: &Fixture,
        method: &str,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap();
        let response = f.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn radarr() -> String {
        let fake = FakeArr::default();
        fake.reply("/api/v3/system/status", json!({ "appName": "Radarr" }))
            .reply("/api/v3/qualityprofile", json!([{ "id": 1, "name": "Any" }]))
            .reply(
                "/api/v3/movie",
                json!([{ "id": 3, "title": "Dune", "year": 2021, "tmdbId": 438631, "monitored": true,
                         "qualityProfileId": 1, "minimumAvailability": "released",
                         "path": "/data/movies/Dune (2021)", "rootFolderPath": "/data/movies" }]),
            );
        fake.start().await
    }

    #[tokio::test]
    async fn imports_and_lists_the_wanted_movies() {
        let f = fixture().await;
        let url = radarr().await;
        let (status, report) = send(
            &f,
            "POST",
            "/api/v1/import/radarr",
            &f.admin,
            Some(json!({ "url": url, "api_key": "key" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{report}");
        assert_eq!(report["movies"], 1);
        let (status, wanted) = send(&f, "GET", "/api/v1/wanted/movies", &f.admin, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            wanted,
            json!([{ "id": 1, "tmdb_id": 438631, "imdb_id": null, "title": "Dune", "year": 2021, "monitored": true,
                     "quality_profile": "Any", "profile_id": null, "minimum_availability": "released", "library": "movies",
                     "movie_id": null, "source": "radarr" }])
        );
    }

    #[tokio::test]
    async fn explains_what_went_wrong() {
        let f = fixture().await;
        let url = radarr().await;
        let cases = [
            (
                json!({ "url": "not a url", "api_key": "key" }),
                StatusCode::BAD_REQUEST,
                "isn't valid",
            ),
            (
                json!({ "url": url, "api_key": "wrong" }),
                StatusCode::BAD_REQUEST,
                "rejected the API key",
            ),
            (
                json!({ "url": url, "api_key": "key", "root_folders": { "/data/movies": "films" } }),
                StatusCode::BAD_REQUEST,
                "no movies library named films",
            ),
            // Nothing listens on port 9 (discard).
            (
                json!({ "url": "http://127.0.0.1:9", "api_key": "key" }),
                StatusCode::BAD_GATEWAY,
                "can't reach",
            ),
        ];
        for (body, status, message) in cases {
            let (got, reply) = send(
                &f,
                "POST",
                "/api/v1/import/radarr",
                &f.admin,
                Some(body.clone()),
            )
            .await;
            assert_eq!(got, status, "{body}");
            assert!(
                reply["error"].as_str().unwrap().contains(message),
                "{body}: {reply}"
            );
            assert!(
                !reply.to_string().contains("\"key\""),
                "the API key is never echoed"
            );
        }
        let sonarr = FakeArr::default();
        sonarr.reply("/api/v3/system/status", json!({ "appName": "Sonarr" }));
        let sonarr = sonarr.start().await;
        let (status, reply) = send(
            &f,
            "POST",
            "/api/v1/import/radarr",
            &f.admin,
            Some(json!({ "url": sonarr, "api_key": "key" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(reply["error"], "that URL is Sonarr, not Radarr");
    }

    #[tokio::test]
    async fn imports_and_lists_the_wanted_series() {
        let f = fixture().await;
        let fake = FakeArr::default();
        fake.reply("/api/v3/system/status", json!({ "appName": "Sonarr" }))
            .reply("/api/v3/qualityprofile", json!([]))
            .reply(
                "/api/v3/series",
                json!([{ "id": 9, "title": "Frieren", "year": 2023, "tvdbId": 424536, "monitored": true,
                         "seriesType": "anime", "path": "/tv/anime/Frieren", "rootFolderPath": "/tv/anime",
                         "seasons": [{ "seasonNumber": 1, "monitored": true }] }]),
            )
            .reply("/api/v3/episode?seriesId=9", json!([{ "seasonNumber": 1, "episodeNumber": 5, "monitored": false }]));
        let url = fake.start().await;
        let (status, report) = send(
            &f,
            "POST",
            "/api/v1/import/sonarr",
            &f.admin,
            Some(json!({ "url": url, "api_key": "key" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{report}");
        assert_eq!(
            (
                report["series"].as_i64(),
                report["without_library"].as_i64()
            ),
            (Some(1), Some(1))
        );
        let (_, wanted) = send(&f, "GET", "/api/v1/wanted/series", &f.admin, None).await;
        assert_eq!(
            wanted,
            json!([{ "id": 1, "tvdb_id": 424536, "tmdb_id": null, "imdb_id": null, "title": "Frieren", "year": 2023,
                     "monitored": true, "series_type": "anime", "quality_profile": null, "profile_id": null, "library": null,
                     "series_id": null, "source": "sonarr",
                     "seasons": [{ "season": 1, "monitored": true }],
                     "unmonitored_episodes": [{ "season": 1, "episode": 5 }] }])
        );
    }

    #[tokio::test]
    async fn an_empty_list_doesnt_wipe_the_wanted_list() {
        let f = fixture().await;
        let url = radarr().await;
        let body = json!({ "url": url, "api_key": "key" });
        send(&f, "POST", "/api/v1/import/radarr", &f.admin, Some(body)).await;
        let empty = FakeArr::default();
        empty
            .reply("/api/v3/system/status", json!({ "appName": "Radarr" }))
            .reply("/api/v3/qualityprofile", json!([]))
            .reply("/api/v3/movie", json!([]));
        let url = empty.start().await;
        let (status, reply) = send(
            &f,
            "POST",
            "/api/v1/import/radarr",
            &f.admin,
            Some(json!({ "url": url, "api_key": "key" })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{reply}");
        let (_, wanted) = send(&f, "GET", "/api/v1/wanted/movies", &f.admin, None).await;
        assert_eq!(wanted.as_array().unwrap().len(), 1, "nothing removed");
    }

    #[tokio::test]
    async fn admins_only() {
        let f = fixture().await;
        let body = json!({ "url": "http://x", "api_key": "key" });
        assert_eq!(
            send(&f, "POST", "/api/v1/import/radarr", &f.viewer, Some(body))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            send(&f, "GET", "/api/v1/wanted/movies", &f.viewer, None)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn imports_and_lists_wanted_music() {
        let pool = db::open_in_memory().await;
        let config = LibraryConfig {
            name: "music".into(),
            kind: LibraryKind::Music,
            path: "/media/music".into(),
        };
        sync_config(&pool, &[config]).await.unwrap();
        sqlx::query(
            "INSERT INTO music_albums (library_id, key, title, artist_name, folder, seen_generation,
                 mb_release_group_id) VALUES (1, 'k', 'One', 'Alder', 'a', 1, 'rg-1')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let admin = auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let viewer = auth::create_user(&pool, "viewer", "viewer-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, admin) = auth::issue_token(&pool, admin.id, TokenKind::Session, "t")
            .await
            .unwrap();
        let (_, viewer) = auth::issue_token(&pool, viewer.id, TokenKind::Session, "t")
            .await
            .unwrap();
        let f = Fixture {
            app: routes(pool, None),
            admin,
            viewer,
        };
        let fake = FakeArr::default();
        fake.reply("/api/v1/system/status", json!({ "appName": "Lidarr" }))
            .reply("/api/v1/qualityprofile", json!([{ "id": 1, "name": "Lossless" }]))
            .reply(
                "/api/v1/artist",
                json!([{ "id": 10, "artistName": "Alder", "foreignArtistId": "mb-a1", "monitored": true,
                         "qualityProfileId": 1, "rootFolderPath": "/music", "monitorNewItems": "all" }]),
            )
            .reply(
                "/api/v1/album",
                json!([
                    { "id": 100, "title": "One", "foreignAlbumId": "rg-1", "artistId": 10, "monitored": true,
                      "releaseDate": "2016-04-29T00:00:00Z", "albumType": "Album", "secondaryTypes": [] },
                    { "id": 101, "title": "Two", "foreignAlbumId": "rg-2", "artistId": 10, "monitored": false,
                      "releaseDate": "2019-01-01T00:00:00Z", "albumType": "EP", "secondaryTypes": ["Live"] },
                ]),
            );
        let url = fake.start().await;
        let body = json!({ "url": url, "api_key": "key" });
        assert_eq!(
            send(
                &f,
                "POST",
                "/api/v1/import/lidarr",
                &f.viewer,
                Some(body.clone())
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        let (status, report) =
            send(&f, "POST", "/api/v1/import/lidarr", &f.admin, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{report}");
        assert_eq!(
            (
                report["artists"].clone(),
                report["albums"].clone(),
                report["on_disk"].clone()
            ),
            (json!(1), json!(2), json!(1))
        );
        let wrong = json!({ "url": url, "api_key": "key", "root_folders": { "/music": "nope" } });
        assert_eq!(
            send(&f, "POST", "/api/v1/import/lidarr", &f.admin, Some(wrong))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );

        let (status, artists) = send(&f, "GET", "/api/v1/wanted/artists", &f.admin, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            artists,
            json!([{ "id": 1, "mb_artist_id": "mb-a1", "name": "Alder", "monitored": true, "monitor_new": "all",
                     "quality_profile": "Lossless", "profile_id": null, "library": "music", "source": "lidarr",
                     "albums": 2, "monitored_albums": 1, "on_disk": 1 }])
        );
        let (status, albums) =
            send(&f, "GET", "/api/v1/wanted/artists/1/albums", &f.admin, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            albums,
            json!([
                { "id": 2, "mb_release_group_id": "rg-2", "title": "Two", "album_type": "EP", "secondary_types": ["Live"],
                  "release_date": "2019-01-01", "monitored": false, "source": "lidarr", "album_id": null },
                { "id": 1, "mb_release_group_id": "rg-1", "title": "One", "album_type": "Album", "secondary_types": [],
                  "release_date": "2016-04-29", "monitored": true, "source": "lidarr", "album_id": 1 },
            ])
        );
        let spec: Value = serde_json::from_str(super::super::OPENAPI).unwrap();
        for (schema, value) in [
            ("LidarrReport", &report),
            ("WantedArtist", &artists[0]),
            ("WantedAlbum", &albums[0]),
        ] {
            let mut documented: Vec<&String> = spec["components"]["schemas"][schema]["properties"]
                .as_object()
                .unwrap()
                .keys()
                .collect();
            let mut actual: Vec<&String> = value.as_object().unwrap().keys().collect();
            documented.sort();
            actual.sort();
            assert_eq!(actual, documented, "{schema}");
        }
        assert_eq!(
            send(&f, "GET", "/api/v1/wanted/artists/9/albums", &f.admin, None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(&f, "GET", "/api/v1/wanted/artists", &f.viewer, None)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn lists_followed_artists_with_how_their_last_look_went() {
        let pool = db::open_in_memory().await;
        let admin = auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) = auth::issue_token(&pool, admin.id, TokenKind::Session, "t")
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
             VALUES (1, 'a1', 'Bjork', 1, 'new', 'manual', 0),
                    (2, 'a2', 'Arca', 1, 'all', 'manual', 0),
                    (3, 'a3', 'Not followed', 1, 'none', 'manual', 0),
                    (4, 'a4', 'Unmonitored', 0, 'all', 'manual', 0);
             INSERT INTO followed_artist_checks (wanted_artist_id, since, checked_at, looked, error)
             VALUES (1, '2026-10-01', 100, 1, NULL), (2, '2026-10-02', 200, 0, 'MusicBrainz answered 503');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let viewer = auth::create_user(&pool, "viewer", "viewer-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, viewer) = auth::issue_token(&pool, viewer.id, TokenKind::Session, "t")
            .await
            .unwrap();
        let f = Fixture {
            app: routes(pool, None),
            admin: token,
            viewer,
        };
        let (status, body) =
            send(&f, "GET", "/api/v1/wanted/artists/follows", &f.admin, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!([
                { "wanted_artist_id": 2, "name": "Arca", "monitor_new": "all", "since": "2026-10-02",
                  "checked_at": 200, "error": "MusicBrainz answered 503" },
                { "wanted_artist_id": 1, "name": "Bjork", "monitor_new": "new", "since": "2026-10-01",
                  "checked_at": 100, "error": null }
            ])
        );
        let (status, _) = send(&f, "GET", "/api/v1/wanted/artists/follows", &f.viewer, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn monitors_and_takes_titles_off_the_wanted_list() {
        let pool = db::open_in_memory().await;
        let mut tokens = Vec::new();
        for (name, admin) in [("admin", true), ("viewer", false)] {
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
        sqlx::query(
            "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at)
             VALUES (1, 1, 'Dune', 1, 'manual', 0), (2, 2, 'Arrival', 1, 'manual', 0);
             INSERT INTO downloads (infohash, title, wanted_movie_id, decision, state, added_at, updated_at)
             VALUES ('a11bae70b2fdd9eb95b63e746f8594dce2650b0c', 'Dune', 1, '{}', 'downloading', 0, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let viewer = tokens.pop().unwrap();
        let f = Fixture {
            app: routes(pool, None),
            admin: tokens.pop().unwrap(),
            viewer,
        };
        let (status, _) = send(
            &f,
            "PUT",
            "/api/v1/wanted/movies/2/monitored",
            &f.admin,
            Some(json!({ "monitored": false })),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, listed) = send(&f, "GET", "/api/v1/wanted/movies", &f.admin, None).await;
        let arrival = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == 2)
            .unwrap()
            .clone();
        assert_eq!(arrival["monitored"], false);
        let (status, _) = send(
            &f,
            "PUT",
            "/api/v1/wanted/albums/9/monitored",
            &f.admin,
            Some(json!({ "monitored": true })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Dune's download is on its way, so it stays; Arrival goes.
        let (status, body) = send(&f, "DELETE", "/api/v1/wanted/movies/1", &f.admin, None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        let (status, _) = send(&f, "DELETE", "/api/v1/wanted/movies/2", &f.admin, None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, listed) = send(&f, "GET", "/api/v1/wanted/movies", &f.admin, None).await;
        assert_eq!(listed.as_array().unwrap().len(), 1);
        for (method, uri) in [
            ("DELETE", "/api/v1/wanted/series/1"),
            ("PUT", "/api/v1/wanted/artists/1/monitored"),
        ] {
            let (status, _) = send(
                &f,
                method,
                uri,
                &f.viewer,
                Some(json!({ "monitored": true })),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        }
    }
}
