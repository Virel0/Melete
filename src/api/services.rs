//! Outside services set up from the web UI, for admins: each service with its settings
//! (secrets only said to be set), and saving them, which takes effect without a restart.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router, middleware};
use serde::Deserialize;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::live::Reload;
use crate::service_check::{self, Checked};
use crate::settings::{self, Refused, ServiceView};

#[derive(Clone)]
struct ServicesState {
    pool: SqlitePool,
    reload: Reload,
}

/// The outside service settings routes, for admins only. Saving asks `reload` to set the
/// services up afresh, so a change takes effect without a restart.
pub fn routes(pool: SqlitePool, reload: Reload) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/settings/services", get(list))
        .route("/api/v1/settings/services/{key}", put(save))
        .route("/api/v1/settings/services/{key}/test", post(test))
        .route("/api/v1/settings/keys", post(new_key))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(ServicesState { pool, reload })
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

async fn list(State(state): State<ServicesState>) -> Result<Json<Vec<ServiceView>>, ApiError> {
    let config = settings::load(&state.pool).await?;
    let stored = settings::stored(&state.pool).await?;
    Ok(Json(settings::views(&config, &stored, &env)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Changes {
    /// Each setting to change, by its environment variable: a value, or null to clear it.
    values: HashMap<String, Option<String>>,
}

fn refused(refused: Refused) -> ApiError {
    match refused {
        Refused::NoService => ApiError::new(StatusCode::NOT_FOUND, "no such service"),
        Refused::Invalid(why) => ApiError::new(StatusCode::BAD_REQUEST, why),
    }
}

/// Tests a service's connection as it's set up, or as it would be with the body's values
/// (`{values: {...}}`, as saving takes them; nothing is saved).
async fn test(
    State(state): State<ServicesState>,
    Path(key): Path<String>,
    body: Bytes,
) -> Result<Json<Checked>, ApiError> {
    let changes: Changes = if body.iter().all(u8::is_ascii_whitespace) {
        Changes {
            values: HashMap::new(),
        }
    } else {
        serde_json::from_slice(&body)
            .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, format!("the body: {err}")))?
    };
    let (config, _) = settings::candidate(&state.pool, &key, &changes.values, &env)
        .await?
        .map_err(refused)?;
    Ok(Json(
        service_check::check(&state.pool, &key, &config).await?,
    ))
}

/// A new random key, for an API key that Melete and another program share (slskd's, say):
/// 32 bytes as hex. It's made here and kept nowhere.
async fn new_key() -> Result<Json<serde_json::Value>, ApiError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness available: {e}"))?;
    let key: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(Json(serde_json::json!({ "key": key })))
}

/// Saves a service's settings and sets the services up afresh with them; puts them back as
/// they were when that can't be done.
async fn save(
    State(state): State<ServicesState>,
    Path(key): Path<String>,
    Json(changes): Json<Changes>,
) -> Result<Json<ServiceView>, ApiError> {
    let before = settings::stored(&state.pool).await?;
    settings::save(&state.pool, &key, &changes.values, &env)
        .await?
        .map_err(refused)?;
    let (answer, answered) = tokio::sync::oneshot::channel();
    let rebuilt = match state.reload.send(answer).await {
        Ok(()) => answered
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the services weren't set up afresh"))),
        Err(_) => Err(anyhow::anyhow!("the services weren't set up afresh")),
    };
    if let Err(err) = rebuilt {
        settings::restore(&state.pool, changes.values.keys(), &before).await?;
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("couldn't set it up, so nothing changed: {err:#}"),
        ));
    }
    // Turned on, the search for intros and credits, the making of seek-bar previews and
    // subtitles for new imports start now rather than after the next scan.
    crate::jobs::SEGMENTS.notify_one();
    crate::jobs::TRICKPLAY.notify_one();
    crate::jobs::IMPORT_SUBTITLES.notify_one();
    // Another country's ratings: every title is judged again now, from what was fetched.
    if changes.values.contains_key(crate::ratings::COUNTRY_SETTING) {
        crate::ratings::reread(&state.pool).await?;
    }
    let config = settings::load(&state.pool).await?;
    let stored = settings::stored(&state.pool).await?;
    settings::views(&config, &stored, &env)
        .into_iter()
        .find(|s| s.key == key)
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such service"))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;

    async fn token(pool: &SqlitePool, name: &str, admin: bool) -> String {
        let user = auth::create_user(pool, name, "a-long-password".into(), admin)
            .await
            .unwrap()
            .unwrap();
        auth::issue_token(pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap()
            .1
    }

    /// A stand-in for the services, answering each rebuild with `works`.
    fn rebuilds(works: bool) -> Reload {
        let (reload, mut asked) =
            tokio::sync::mpsc::channel::<tokio::sync::oneshot::Sender<anyhow::Result<()>>>(4);
        tokio::spawn(async move {
            while let Some(answer) = asked.recv().await {
                let _ = answer.send(if works {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("Deluge's address won't do"))
                });
            }
        });
        reload
    }

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        if body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn admins_set_up_services_and_secrets_stay_hidden() {
        let pool = db::open_in_memory().await;
        let (admin, someone) = (
            token(&pool, "admin", true).await,
            token(&pool, "someone", false).await,
        );
        let app = routes(pool.clone(), rebuilds(true));

        let (status, all) = send(&app, "GET", "/api/v1/settings/services", &admin, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(all.as_array().unwrap().len(), settings::SERVICES.len());
        assert_eq!(
            send(&app, "GET", "/api/v1/settings/services", &someone, None)
                .await
                .0,
            StatusCode::FORBIDDEN
        );

        let (status, tmdb) = send(
            &app,
            "PUT",
            "/api/v1/settings/services/tmdb",
            &admin,
            Some(json!({"values": {"MELETE_TMDB_TOKEN": "a-secret-token"}})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{tmdb}");
        assert_eq!(
            (
                &tmdb["on"],
                &tmdb["fields"][0]["set"],
                &tmdb["fields"][0]["value"]
            ),
            (&json!(true), &json!(true), &Value::Null)
        );
        let (_, all) = send(&app, "GET", "/api/v1/settings/services", &admin, None).await;
        assert!(!all.to_string().contains("a-secret-token"));

        for (uri, body, status) in [
            (
                "/api/v1/settings/services/nothing",
                json!({"values": {}}),
                StatusCode::NOT_FOUND,
            ),
            (
                "/api/v1/settings/services/lyrics",
                json!({"values": {"MELETE_LYRICS": "yes"}}),
                StatusCode::BAD_REQUEST,
            ),
            (
                "/api/v1/settings/services/lyrics",
                json!({"values": {}, "extra": 1}),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            assert_eq!(
                send(&app, "PUT", uri, &admin, Some(body)).await.0,
                status,
                "{uri}"
            );
        }
        assert_eq!(
            send(
                &app,
                "PUT",
                "/api/v1/settings/services/tmdb",
                &someone,
                Some(json!({"values": {}}))
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn tests_a_connection_with_unsaved_values_and_makes_keys() {
        let pool = db::open_in_memory().await;
        let admin = token(&pool, "admin", true).await;
        let app = routes(pool.clone(), rebuilds(true));
        let (status, checked) = send(
            &app,
            "POST",
            "/api/v1/settings/services/tmdb/test",
            &admin,
            None,
        )
        .await;
        assert_eq!(
            (status, &checked["ok"]),
            (StatusCode::OK, &json!(false)),
            "{checked}"
        );
        // As it would be: the address is tried, and nothing's kept.
        let (status, checked) = send(
            &app,
            "POST",
            "/api/v1/settings/services/flaresolverr/test",
            &admin,
            Some(json!({"values": {"MELETE_FLARESOLVERR_URL": "http://127.0.0.1:9"}})),
        )
        .await;
        assert_eq!(
            (status, &checked["ok"]),
            (StatusCode::OK, &json!(false)),
            "{checked}"
        );
        assert!(
            !settings::stored(&pool)
                .await
                .unwrap()
                .contains_key("MELETE_FLARESOLVERR_URL")
        );
        assert_eq!(
            send(
                &app,
                "POST",
                "/api/v1/settings/services/nothing/test",
                &admin,
                None
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );

        let (status, first) = send(&app, "POST", "/api/v1/settings/keys", &admin, None).await;
        let (_, second) = send(&app, "POST", "/api/v1/settings/keys", &admin, None).await;
        let key = first["key"].as_str().unwrap();
        assert_eq!(status, StatusCode::OK);
        assert!(key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn another_ratings_country_counts_for_a_limited_user_at_once() {
        use crate::access::{self, Access, Restriction};
        let pool = db::open_in_memory().await;
        let admin = token(&pool, "admin", true).await;
        let child = auth::create_user(&pool, "child", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let limit = Restriction {
            libraries: None,
            max_age: Some(15),
        };
        access::set_restriction(&pool, child.id, Some(&limit))
            .await
            .unwrap();
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
               INSERT INTO tmdb_movies (tmdb_id, title, fetched_at) VALUES (603, 'A', 0);
               INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
                   match_status, match_tmdb_id) VALUES (1, 1, 'a', 'A', 1, 'matched', 603);
               INSERT INTO tmdb_ratings (tv, tmdb_id, certifications, age, fetched_at)
                   VALUES (0, 603, '{"US": "R", "GB": "15"}', 17, 0);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let sees = || async {
            Access::of(&pool, child.id, false)
                .await
                .unwrap()
                .sees_movie(&pool, 1)
                .await
                .unwrap()
        };
        assert!(!sees().await, "rated R in the United States");
        let app = routes(pool.clone(), rebuilds(true));
        let put = |country: &'static str| {
            send(
                &app,
                "PUT",
                "/api/v1/settings/services/tmdb",
                &admin,
                Some(json!({"values": {"MELETE_RATINGS_COUNTRY": country}})),
            )
        };
        let (status, said) = put("XX").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{said}");
        assert!(!sees().await);
        let (status, tmdb) = put("GB").await;
        assert_eq!(status, StatusCode::OK, "{tmdb}");
        assert_eq!(tmdb["fields"][2]["value"], json!("GB"));
        assert_eq!(
            tmdb["fields"][2]["input"]["choice"]["options"][0]["value"],
            json!("US")
        );
        assert!(
            sees().await,
            "15 in the United Kingdom, with nothing fetched"
        );
    }

    #[tokio::test]
    async fn a_change_that_cant_be_set_up_is_put_back() {
        let pool = db::open_in_memory().await;
        let admin = token(&pool, "admin", true).await;
        let app = routes(pool.clone(), rebuilds(false));
        let (status, said) = send(
            &app,
            "PUT",
            "/api/v1/settings/services/lyrics",
            &admin,
            Some(json!({"values": {"MELETE_LYRICS": "lrclib"}})),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(
            said["error"]
                .as_str()
                .unwrap()
                .contains("nothing changed: Deluge's address won't do")
        );
        assert!(
            !settings::stored(&pool)
                .await
                .unwrap()
                .contains_key("MELETE_LYRICS")
        );
    }
}
