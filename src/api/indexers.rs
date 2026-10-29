//! Indexers (admins only): importing them from Prowlarr, listing them (never with their key),
//! testing, renaming, turning off and removing one.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router, middleware};
use serde::Deserialize;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use super::import::import_error;
use crate::arr::ArrClient;
use crate::indexers::{
    self, Indexer, NotAdded, NotAddedOwn, NotChanged, ProwlarrReport, TestReport,
};
use crate::torznab;

/// The indexer routes, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/import/prowlarr", post(import_prowlarr))
        .route("/api/v1/indexers", get(list).post(add_torznab))
        .route("/api/v1/indexers/{id}", put(update).delete(remove))
        .route("/api/v1/indexers/{id}/test", post(test))
        .route("/api/v1/indexers/presets/{key}", post(add_preset))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(pool)
}

#[derive(Deserialize)]
struct Connection {
    url: String,
    api_key: String,
}

async fn import_prowlarr(
    State(pool): State<SqlitePool>,
    Json(connection): Json<Connection>,
) -> Result<Json<ProwlarrReport>, ApiError> {
    let client = ArrClient::new(&connection.url, connection.api_key)
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))?;
    let report = indexers::import_prowlarr(&pool, &client)
        .await
        .map_err(|err| import_error("Prowlarr", err))?;
    Ok(Json(report))
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<Indexer>>, ApiError> {
    Ok(Json(indexers::list(&pool).await?))
}

/// What an admin can change: the name, and whether Melete searches it.
#[derive(Deserialize)]
struct Change {
    name: String,
    enabled: bool,
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "no such indexer")
}

async fn update(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(change): Json<Change>,
) -> Result<Json<Indexer>, ApiError> {
    match indexers::update(&pool, id, &change.name, change.enabled).await? {
        Ok(()) => {}
        Err(NotChanged::NotFound) => return Err(not_found()),
        Err(NotChanged::BadName) => {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "give the indexer a name of 1 to 100 characters",
            ));
        }
        Err(NotChanged::OffInSource) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "it's turned off in Prowlarr; turn it on there, then import from Prowlarr again",
            ));
        }
    }
    indexers::list(&pool)
        .await?
        .into_iter()
        .find(|i| i.id == id)
        .map(Json)
        .ok_or_else(not_found)
}

async fn remove(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if indexers::remove(&pool, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found())
    }
}

async fn test(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<TestReport>, ApiError> {
    let client = torznab::Client::new()?;
    match indexers::test(&pool, &client, id).await? {
        None => Err(not_found()),
        Some(Ok(report)) => Ok(Json(report)),
        Some(Err(err)) => Err(ApiError::new(StatusCode::BAD_GATEWAY, err.to_string())),
    }
}

/// Adds a public tracker Melete reaches itself: thepiratebay, nyaa or animetosho.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewTorznab {
    name: String,
    url: String,
    #[serde(default)]
    api_key: String,
}

/// Adds a Torznab indexer by hand; it's asked what it can do before it's kept.
async fn add_torznab(
    State(pool): State<SqlitePool>,
    Json(body): Json<NewTorznab>,
) -> Result<(StatusCode, Json<Indexer>), ApiError> {
    let client = torznab::Client::new()?;
    let id = match indexers::add_torznab(&pool, &client, &body.name, &body.url, &body.api_key)
        .await?
    {
        Ok(id) => id,
        Err(NotAddedOwn::Invalid(why)) => return Err(ApiError::new(StatusCode::BAD_REQUEST, why)),
        Err(NotAddedOwn::Already(id)) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                format!("it's indexer {id} already"),
            ));
        }
        Err(NotAddedOwn::Unanswered(why)) => {
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                format!("it didn't answer as a Torznab indexer, so it wasn't added: {why}"),
            ));
        }
    };
    let indexer = indexers::list(&pool)
        .await?
        .into_iter()
        .find(|i| i.id == id)
        .ok_or_else(|| anyhow::anyhow!("indexer {id} vanished"))?;
    Ok((StatusCode::CREATED, Json(indexer)))
}

async fn add_preset(
    State(pool): State<SqlitePool>,
    Path(key): Path<String>,
) -> Result<(StatusCode, Json<Indexer>), ApiError> {
    let id = match indexers::add_preset(&pool, &key).await? {
        Ok(id) => id,
        Err(NotAdded::Unknown) => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "no such tracker; Melete knows thepiratebay, nyaa, animetosho and uindex",
            ));
        }
        Err(NotAdded::NeedsFlareSolverr) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "Uindex sits behind Cloudflare's check; set MELETE_FLARESOLVERR_URL first",
            ));
        }
        Err(NotAdded::Already(id)) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                format!("it's indexer {id} already"),
            ));
        }
    };
    let indexer = indexers::list(&pool)
        .await?
        .into_iter()
        .find(|i| i.id == id)
        .ok_or_else(|| anyhow::anyhow!("indexer {id} vanished"))?;
    Ok((StatusCode::CREATED, Json(indexer)))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::header;
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;
    use crate::import::testing::FakeArr;

    struct Fixture {
        app: Router,
        admin: String,
        viewer: String,
    }

    async fn fixture() -> Fixture {
        let pool = db::open_in_memory().await;
        let mut tokens = Vec::new();
        for (name, admin) in [("admin", true), ("viewer", false)] {
            let user = auth::create_user(&pool, name, format!("{name}-password"), admin)
                .await
                .unwrap()
                .unwrap();
            let (_, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
                .await
                .unwrap();
            tokens.push(token);
        }
        let viewer = tokens.pop().unwrap();
        Fixture {
            app: routes(pool),
            admin: tokens.pop().unwrap(),
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

    /// A Prowlarr listing the captured indexers, changed by `edit`.
    async fn prowlarr(edit: impl FnOnce(&mut Value)) -> String {
        let mut listed: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/prowlarr/indexers.json"))
                .unwrap();
        edit(&mut listed);
        let fake = FakeArr::default();
        fake.reply("/api/v1/system/status", json!({ "appName": "Prowlarr" }))
            .reply("/api/v1/indexer", listed);
        fake.start().await
    }

    async fn import(f: &Fixture, url: &str) -> (StatusCode, Value) {
        send(
            f,
            "POST",
            "/api/v1/import/prowlarr",
            &f.admin,
            Some(json!({ "url": url, "api_key": "key" })),
        )
        .await
    }

    #[tokio::test]
    async fn imports_torrent_indexers_as_prowlarr_feeds() {
        let f = fixture().await;
        let url = prowlarr(|listed| {
            let mut usenet = listed[1].clone();
            usenet["id"] = 3.into();
            usenet["name"] = "Some Usenet".into();
            usenet["protocol"] = "usenet".into();
            listed.as_array_mut().unwrap().push(usenet);
        })
        .await;
        let (status, report) = import(&f, &url).await;
        assert_eq!(status, StatusCode::OK, "{report}");
        assert_eq!(
            report,
            json!({ "indexers": 2, "skipped_usenet": 1, "removed": 0 })
        );
        let (status, listed) = send(&f, "GET", "/api/v1/indexers", &f.admin, None).await;
        assert_eq!(status, StatusCode::OK);
        let added_at = listed[0]["added_at"].clone();
        assert_eq!(
            listed,
            json!([
                { "id": 1, "name": "Disabled Tracker", "url": format!("{url}/2/api"), "enabled": false,
                  "categories": [], "movie_search_params": [], "tv_search_params": [], "source": "prowlarr", "kind": "torznab",
                  "added_at": added_at },
                { "id": 2, "name": "Fake Tracker", "url": format!("{url}/1/api"), "enabled": true,
                  "categories": [2000, 2040, 2045, 5000, 5040], "movie_search_params": ["q", "imdbid", "tmdbid"],
                  "tv_search_params": ["q", "season", "ep", "tvdbid"], "source": "prowlarr", "kind": "torznab", "added_at": added_at }
            ])
        );
        assert!(
            !listed.to_string().contains("\"key\""),
            "the API key never shows"
        );

        // Prowlarr dropped one: so does Melete. An empty list is refused.
        let url_one = prowlarr(|listed| listed.as_array_mut().unwrap().truncate(1)).await;
        let (_, report) = import(&f, &url_one).await;
        assert_eq!(report["removed"], 1);
        let empty = prowlarr(|listed| *listed = json!([])).await;
        let (status, _) = import(&f, &empty).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn renames_turns_off_and_removes_an_indexer() {
        let f = fixture().await;
        let url = prowlarr(|_| {}).await;
        assert_eq!(import(&f, &url).await.0, StatusCode::OK);
        let (_, listed) = send(&f, "GET", "/api/v1/indexers", &f.admin, None).await;
        let id = |listed: &Value, name: &str| {
            listed
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["name"] == name)
                .map(|i| i["id"].as_i64().unwrap())
        };
        let (fake, disabled) = (
            id(&listed, "Fake Tracker").unwrap(),
            id(&listed, "Disabled Tracker").unwrap(),
        );
        let put = |id: i64, body: Value| {
            let f = &f;
            async move {
                send(
                    f,
                    "PUT",
                    &format!("/api/v1/indexers/{id}"),
                    &f.admin,
                    Some(body),
                )
                .await
            }
        };

        // Renamed and turned off; the answer is the indexer as listed.
        let (status, changed) = put(fake, json!({ "name": "  Films  ", "enabled": false })).await;
        assert_eq!(status, StatusCode::OK, "{changed}");
        assert_eq!(
            (
                changed["id"].as_i64(),
                changed["name"].clone(),
                changed["enabled"].clone()
            ),
            (Some(fake), json!("Films"), json!(false))
        );
        assert!(!changed.to_string().contains("\"key\""));

        // Another import from Prowlarr keeps both.
        assert_eq!(import(&f, &url).await.0, StatusCode::OK);
        let (_, listed) = send(&f, "GET", "/api/v1/indexers", &f.admin, None).await;
        assert_eq!(id(&listed, "Films"), Some(fake));
        assert_eq!(id(&listed, "Fake Tracker"), None);
        let films = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == fake)
            .unwrap();
        assert_eq!(films["enabled"], false);

        // Turned on again.
        let (status, changed) = put(fake, json!({ "name": "Films", "enabled": true })).await;
        assert_eq!(
            (status, changed["enabled"].clone()),
            (StatusCode::OK, json!(true))
        );

        // One Prowlarr has off can't be turned on here, but can be renamed.
        let (status, body) = put(disabled, json!({ "name": "Off", "enabled": true })).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        let (status, _) = put(disabled, json!({ "name": "Off", "enabled": false })).await;
        assert_eq!(status, StatusCode::OK);

        // Names must be 1 to 100 characters.
        for name in ["", "   ", &"x".repeat(101)] {
            let (status, _) = put(fake, json!({ "name": name, "enabled": true })).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{name:?}");
        }
        let (status, _) = put(9999, json!({ "name": "x", "enabled": true })).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(
            &f,
            "PUT",
            &format!("/api/v1/indexers/{fake}"),
            &f.viewer,
            Some(json!({ "name": "x", "enabled": true })),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Removed: gone from the list, and a second removal finds nothing.
        let path = format!("/api/v1/indexers/{fake}");
        let (status, _) = send(&f, "DELETE", &path, &f.viewer, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = send(&f, "DELETE", &path, &f.admin, None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, listed) = send(&f, "GET", "/api/v1/indexers", &f.admin, None).await;
        assert_eq!(id(&listed, "Films"), None);
        let (status, _) = send(&f, "DELETE", &path, &f.admin, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Prowlarr still has it, so the next import brings it back under its own name.
        assert_eq!(import(&f, &url).await.0, StatusCode::OK);
        let (_, listed) = send(&f, "GET", "/api/v1/indexers", &f.admin, None).await;
        assert!(id(&listed, "Fake Tracker").is_some());
    }

    #[tokio::test]
    async fn adds_public_trackers_melete_reaches_itself() {
        let f = fixture().await;
        let (status, nyaa) =
            send(&f, "POST", "/api/v1/indexers/presets/nyaa", &f.admin, None).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(
            (
                nyaa["name"].clone(),
                nyaa["kind"].clone(),
                nyaa["source"].clone(),
                nyaa["url"].clone()
            ),
            (
                json!("Nyaa"),
                json!("nyaa"),
                json!("melete"),
                json!("https://nyaa.si")
            )
        );
        let (status, _) = send(&f, "POST", "/api/v1/indexers/presets/nyaa", &f.admin, None).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) = send(
            &f,
            "POST",
            "/api/v1/indexers/presets/worldtorrent",
            &f.admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(
            &f,
            "POST",
            "/api/v1/indexers/presets/thepiratebay",
            &f.viewer,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn tests_an_indexer_through_its_feed() {
        use axum::extract::Query;
        use axum::response::IntoResponse;
        use std::collections::HashMap;

        let f = fixture().await;
        // A Prowlarr that also serves the per-indexer feeds, as the real one does.
        let feed = axum::Router::new()
            .route(
                "/api/v1/system/status",
                axum::routing::get(|| async { Json(json!({ "appName": "Prowlarr" })) }),
            )
            .route(
                "/api/v1/indexer",
                axum::routing::get(|| async {
                    Json(
                        serde_json::from_str::<Value>(include_str!(
                            "../../tests/fixtures/prowlarr/indexers.json"
                        ))
                        .unwrap(),
                    )
                }),
            )
            .route(
                "/{id}/api",
                axum::routing::get(
                    |Path(id): Path<i64>, Query(q): Query<HashMap<String, String>>| async move {
                        match (
                            id,
                            q.get("apikey").map(String::as_str),
                            q.get("t").map(String::as_str),
                        ) {
                            (_, Some(key), _) if key != "key" => {
                                StatusCode::UNAUTHORIZED.into_response()
                            }
                            (2, _, _) => StatusCode::GONE.into_response(),
                            (_, _, Some("caps")) => {
                                include_str!("../../tests/fixtures/prowlarr/caps.xml")
                                    .into_response()
                            }
                            _ => include_str!("../../tests/fixtures/prowlarr/search.xml")
                                .into_response(),
                        }
                    },
                ),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, feed).await.unwrap() });
        assert_eq!(import(&f, &url).await.0, StatusCode::OK);

        // Melete's ids are its own: look them up by name.
        let (_, listed) = send(&f, "GET", "/api/v1/indexers", &f.admin, None).await;
        let id = |name: &str| {
            listed
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["name"] == name)
                .unwrap()["id"]
                .clone()
        };
        let (fake, disabled) = (id("Fake Tracker"), id("Disabled Tracker"));
        let (status, report) = send(
            &f,
            "POST",
            &format!("/api/v1/indexers/{fake}/test"),
            &f.admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{report}");
        assert_eq!(
            report,
            json!({ "categories": [2000, 2040, 2045, 5000, 5040], "recent_releases": 1 })
        );
        let (status, body) = send(
            &f,
            "POST",
            &format!("/api/v1/indexers/{disabled}/test"),
            &f.admin,
            None,
        )
        .await;
        assert_eq!(
            (status, body["error"].as_str()),
            (
                StatusCode::BAD_GATEWAY,
                Some("the indexer is turned off in Prowlarr")
            )
        );
        let (status, _) = send(&f, "POST", "/api/v1/indexers/9/test", &f.admin, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(&f, "GET", "/api/v1/indexers", &f.viewer, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
}
