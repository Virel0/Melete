//! Fixing matches by hand (admins only): pick the right TMDB entry for a title, or record that
//! it has none. Manual decisions are final; automatic matching never revisits them.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::metadata::{self, Kind};
use crate::tmdb::{self, RateLimited, Unauthorized};

#[derive(Clone)]
struct MatchState {
    pool: SqlitePool,
    tmdb: Option<Arc<tmdb::Client>>,
}

impl MatchState {
    fn tmdb(&self) -> Result<&tmdb::Client, ApiError> {
        self.tmdb.as_deref().ok_or_else(|| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "TMDB is not configured; set MELETE_TMDB_TOKEN",
            )
        })
    }
}

/// The matching routes, for admins only.
pub fn routes(pool: SqlitePool, tmdb: Option<Arc<tmdb::Client>>) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route(
            "/api/v1/movies/{id}/match",
            put(set_movie).delete(clear_movie),
        )
        .route(
            "/api/v1/series/{id}/match",
            put(set_series).delete(clear_series),
        )
        .route("/api/v1/metadata/search", get(search))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(MatchState { pool, tmdb })
}

#[derive(Deserialize)]
struct MatchRequest {
    tmdb_id: i64,
}

/// 404 unless the title exists and is present on disk.
async fn require_title(pool: &SqlitePool, kind: Kind, id: i64) -> Result<(), ApiError> {
    let (query, what) = match kind {
        Kind::Movie => (
            "SELECT 1 FROM movies WHERE id = ? AND missing_since IS NULL",
            "movie",
        ),
        Kind::Series => (
            "SELECT 1 FROM series WHERE id = ? AND missing_since IS NULL",
            "series",
        ),
    };
    let found: Option<i64> = sqlx::query_scalar(query)
        .bind(id)
        .fetch_optional(pool)
        .await?;
    match found {
        Some(_) => Ok(()),
        None => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("no such {what}"),
        )),
    }
}

/// TMDB trouble is the gateway's fault (502/503), not ours (500).
fn tmdb_error(err: anyhow::Error) -> ApiError {
    if err.is::<Unauthorized>() {
        ApiError::new(StatusCode::BAD_GATEWAY, err.to_string())
    } else if err.is::<RateLimited>() {
        ApiError::new(StatusCode::SERVICE_UNAVAILABLE, err.to_string())
    } else if err.is::<sqlx::Error>() {
        ApiError::Internal(err)
    } else {
        tracing::warn!(error = format!("{err:#}"), "TMDB request failed");
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "TMDB request failed; try again later",
        )
    }
}

async fn set_movie(
    State(state): State<MatchState>,
    Path(id): Path<i64>,
    Json(body): Json<MatchRequest>,
) -> Result<Json<Value>, ApiError> {
    require_title(&state.pool, Kind::Movie, id).await?;
    let found = metadata::match_movie_manually(&state.pool, state.tmdb()?, id, body.tmdb_id)
        .await
        .map_err(tmdb_error)?;
    matched(id, body.tmdb_id, found, "movie")
}

async fn set_series(
    State(state): State<MatchState>,
    Path(id): Path<i64>,
    Json(body): Json<MatchRequest>,
) -> Result<Json<Value>, ApiError> {
    require_title(&state.pool, Kind::Series, id).await?;
    let found = metadata::match_series_manually(&state.pool, state.tmdb()?, id, body.tmdb_id)
        .await
        .map_err(tmdb_error)?;
    matched(id, body.tmdb_id, found, "series")
}

fn matched(id: i64, tmdb_id: i64, found: bool, what: &str) -> Result<Json<Value>, ApiError> {
    if !found {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("TMDB has no {what} {tmdb_id}"),
        ));
    }
    Ok(Json(json!({
        "id": id,
        "match_status": "matched",
        "match_method": "manual",
        "tmdb_id": tmdb_id,
    })))
}

async fn clear_movie(
    State(state): State<MatchState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    require_title(&state.pool, Kind::Movie, id).await?;
    metadata::unmatch_manually(&state.pool, Kind::Movie, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_series(
    State(state): State<MatchState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    require_title(&state.pool, Kind::Series, id).await?;
    metadata::unmatch_manually(&state.pool, Kind::Series, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SearchQuery {
    /// `movie` or `series`.
    kind: String,
    q: String,
    year: Option<i64>,
}

/// Searches TMDB, for picking a match the automatic search didn't offer.
async fn search(
    State(state): State<MatchState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let q = query.q.trim();
    if q.is_empty() || q.chars().count() > 200 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "q is 1 to 200 characters",
        ));
    }
    if !matches!(query.kind.as_str(), "movie" | "series") {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "kind is movie or series",
        ));
    }
    let tmdb = state.tmdb()?;
    let year_of = |date: Option<&str>| {
        date.and_then(|d| d.get(..4))
            .and_then(|y| y.parse::<i64>().ok())
    };
    let results: Vec<Value> = match query.kind.as_str() {
        "movie" => tmdb
            .search_movies(q, query.year)
            .await
            .map_err(tmdb_error)?
            .into_iter()
            .map(|r| {
                json!({
                    "tmdb_id": r.id,
                    "title": r.title,
                    "original_title": r.original_title,
                    "year": year_of(r.release_date.as_deref()),
                })
            })
            .collect(),
        "series" => tmdb
            .search_series(q, query.year)
            .await
            .map_err(tmdb_error)?
            .into_iter()
            .map(|r| {
                json!({
                    "tmdb_id": r.id,
                    "title": r.name,
                    "original_title": r.original_name,
                    "year": year_of(r.first_air_date.as_deref()),
                })
            })
            .collect(),
        _ => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "kind is movie or series",
            ));
        }
    };
    Ok(Json(results))
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
    use crate::library::{LibraryConfig, LibraryKind, sync_config};
    use crate::tmdb::testing::{FakeTmdb, Reply};

    struct Fixture {
        pool: SqlitePool,
        fake: FakeTmdb,
        app: Router,
        admin: String,
        viewer: String,
    }

    async fn fixture(with_tmdb: bool) -> Fixture {
        let pool = db::open_in_memory().await;
        sync_config(
            &pool,
            &[
                LibraryConfig {
                    name: "movies".into(),
                    kind: LibraryKind::Movies,
                    path: "/m".into(),
                },
                LibraryConfig {
                    name: "shows".into(),
                    kind: LibraryKind::Shows,
                    path: "/s".into(),
                },
            ],
        )
        .await
        .unwrap();
        sqlx::raw_sql(
            "INSERT INTO movies (id, library_id, rel_path, parsed_title, parsed_year, seen_generation,
                 match_status, match_candidates, missing_since) VALUES
                 (1, 1, 'Dune', 'Dune', NULL, 1, 'ambiguous', '[]', NULL),
                 (2, 1, 'Gone (2000)', 'Gone', 2000, 1, NULL, NULL, 99);
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 2, 'Severance', 'Severance', 1);",
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

        let fake = FakeTmdb::default();
        let tmdb = if with_tmdb {
            Some(Arc::new(fake.start().await))
        } else {
            None
        };
        Fixture {
            app: routes(pool.clone(), tmdb),
            pool,
            fake,
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

    async fn movie_match(
        pool: &SqlitePool,
        id: i64,
    ) -> (Option<String>, Option<i64>, Option<String>) {
        sqlx::query_as("SELECT match_status, match_tmdb_id, match_method FROM movies WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn admin_picks_the_right_movie() {
        let f = fixture(true).await;
        f.fake.reply(
            "/movie/438631",
            vec![Reply::ok(
                json!({ "id": 438631, "title": "Dune", "release_date": "2021-09-15" }),
            )],
        );
        let (status, body) = send(
            &f,
            "PUT",
            "/api/v1/movies/1/match",
            &f.admin,
            Some(json!({ "tmdb_id": 438631 })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["match_method"], "manual");
        assert_eq!(
            movie_match(&f.pool, 1).await,
            (Some("matched".into()), Some(438631), Some("manual".into()))
        );
        let title: String =
            sqlx::query_scalar("SELECT title FROM tmdb_movies WHERE tmdb_id = 438631")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(title, "Dune");
    }

    #[tokio::test]
    async fn admin_picks_a_series_and_gets_its_episodes() {
        let f = fixture(true).await;
        f.fake
            .reply(
                "/tv/95396?append_to_response=external_ids",
                vec![Reply::ok(json!({ "id": 95396, "name": "Severance", "seasons": [{ "season_number": 1 }] }))],
            )
            .reply(
                "/tv/95396?append_to_response=season/1",
                vec![Reply::ok(json!({ "id": 95396, "name": "Severance", "season/1": { "episodes": [
                    { "id": 1, "season_number": 1, "episode_number": 1 }
                ]}}))],
            );
        let (status, _) = send(
            &f,
            "PUT",
            "/api/v1/series/1/match",
            &f.admin,
            Some(json!({ "tmdb_id": 95396 })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let episodes: i64 =
            sqlx::query_scalar("SELECT count(*) FROM tmdb_episodes WHERE series_tmdb_id = 95396")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(episodes, 1);
    }

    #[tokio::test]
    async fn unknown_tmdb_ids_and_titles_are_404() {
        let f = fixture(true).await;
        let (status, body) = send(
            &f,
            "PUT",
            "/api/v1/movies/1/match",
            &f.admin,
            Some(json!({ "tmdb_id": 999 })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "TMDB has no movie 999");
        assert_eq!(
            movie_match(&f.pool, 1).await.0.as_deref(),
            Some("ambiguous"),
            "unchanged"
        );

        f.fake.log.lock().unwrap().clear();
        for uri in [
            "/api/v1/movies/2/match",
            "/api/v1/movies/42/match",
            "/api/v1/series/42/match",
        ] {
            let (status, _) = send(&f, "PUT", uri, &f.admin, Some(json!({ "tmdb_id": 1 }))).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        }
        assert!(
            f.fake.log.lock().unwrap().is_empty(),
            "TMDB isn't asked about titles we don't have"
        );
    }

    #[tokio::test]
    async fn admin_says_a_title_has_no_match() {
        let f = fixture(true).await;
        let (status, _) = send(&f, "DELETE", "/api/v1/movies/1/match", &f.admin, None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(
            movie_match(&f.pool, 1).await,
            (Some("unmatched".into()), None, Some("manual".into()))
        );
        let candidates: Option<String> =
            sqlx::query_scalar("SELECT match_candidates FROM movies WHERE id = 1")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(candidates, None);
    }

    #[tokio::test]
    async fn only_admins_fix_matches() {
        let f = fixture(true).await;
        for (method, uri) in [
            ("PUT", "/api/v1/movies/1/match"),
            ("DELETE", "/api/v1/series/1/match"),
            ("GET", "/api/v1/metadata/search?kind=movie&q=Dune"),
        ] {
            let (status, _) = send(&f, method, uri, &f.viewer, Some(json!({ "tmdb_id": 1 }))).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
        }
    }

    #[tokio::test]
    async fn bad_search_kind_is_400_even_without_tmdb() {
        let f = fixture(false).await;
        let (status, _) = send(
            &f,
            "GET",
            "/api/v1/metadata/search?kind=film&q=x",
            &f.admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn without_tmdb_matching_is_unavailable_but_unmatching_works() {
        let f = fixture(false).await;
        let (status, body) = send(
            &f,
            "PUT",
            "/api/v1/movies/1/match",
            &f.admin,
            Some(json!({ "tmdb_id": 1 })),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("MELETE_TMDB_TOKEN")
        );
        let (status, _) = send(&f, "DELETE", "/api/v1/movies/1/match", &f.admin, None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn search_offers_candidates() {
        let f = fixture(true).await;
        f.fake.reply(
            "/search/tv?first_air_date_year=2022&include_adult=false&query=Severance",
            vec![Reply::ok(json!({ "results": [
                { "id": 95396, "name": "Severance", "original_name": "Severance", "first_air_date": "2022-02-17" }
            ]}))],
        );
        let (status, body) = send(
            &f,
            "GET",
            "/api/v1/metadata/search?kind=series&q=%20Severance%20&year=2022",
            &f.admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!([{ "tmdb_id": 95396, "title": "Severance", "original_title": "Severance", "year": 2022 }])
        );
        for bad in ["kind=film&q=x", "kind=movie&q=%20"] {
            let (status, _) = send(
                &f,
                "GET",
                &format!("/api/v1/metadata/search?{bad}"),
                &f.admin,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        }
    }

    #[tokio::test]
    async fn rejected_token_is_a_gateway_error() {
        let f = fixture(true).await;
        f.fake
            .reply("/movie/5", vec![Reply::status(StatusCode::UNAUTHORIZED)]);
        let (status, body) = send(
            &f,
            "PUT",
            "/api/v1/movies/1/match",
            &f.admin,
            Some(json!({ "tmdb_id": 5 })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("rejected the token")
        );
    }
}
