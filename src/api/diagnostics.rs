//! Diagnostics reports: any signed-in device sends one (`POST /me/diagnostics`); admins
//! read them (`/diagnostics`).

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_admin, require_user};
use crate::db::unix_now;
use crate::diagnostics::{self, Report};

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

/// Sending a report, for everyone signed in; reading and deleting reports, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    let send = Router::new()
        .route("/api/v1/me/diagnostics", post(send_report))
        .layer(DefaultBodyLimit::max(diagnostics::MAX_BYTES))
        .route_layer(middleware::from_fn_with_state(auth.clone(), require_user));
    let read = Router::new()
        .route(
            "/api/v1/diagnostics",
            get(list_reports).delete(delete_device_reports),
        )
        .route(
            "/api/v1/diagnostics/{id}",
            get(one_report).delete(delete_report),
        )
        .route_layer(middleware::from_fn_with_state(auth, require_admin));
    send.merge(read).with_state(pool)
}

/// Keeps a report from the device that sends it. A body over 64 KiB is refused whole (413).
async fn send_report(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Json(report): Json<Report>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    match diagnostics::keep(&pool, current.user.id, current.token_id, report, unix_now()).await? {
        Ok(id) => Ok((StatusCode::CREATED, Json(json!({ "id": id })))),
        Err(why) => Err(ApiError::new(StatusCode::BAD_REQUEST, why)),
    }
}

#[derive(Deserialize)]
struct ListQuery {
    /// A user's name.
    user: Option<String>,
    /// A device's name.
    device: Option<String>,
    level: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct ReportRow {
    id: i64,
    user: String,
    device: String,
    app: String,
    app_version: Option<String>,
    platform: Option<String>,
    level: String,
    summary: String,
    received_at: i64,
    details: String,
}

impl ReportRow {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "user": self.user,
            "device": self.device,
            "app": self.app,
            "app_version": self.app_version,
            "platform": self.platform,
            "level": self.level,
            "summary": self.summary,
            "received_at": self.received_at,
        })
    }
}

const SELECT: &str = "SELECT d.id, u.username AS user, d.device, d.app, d.app_version,
         d.platform, d.level, d.summary, d.received_at, d.details
     FROM diagnostics d JOIN users u ON u.id = d.user_id";

/// Reports from the last 30 days, newest first, without their details.
async fn list_reports(
    State(pool): State<SqlitePool>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    let offset = query.offset.unwrap_or(0);
    if !(1..=MAX_LIMIT).contains(&limit) || offset < 0 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "limit is 1 to 200 and offset is 0 or more",
        ));
    }
    let since = unix_now() - diagnostics::KEEP_FOR;
    let filter = "WHERE d.received_at >= ?1 AND (?2 IS NULL OR u.username = ?2)
           AND (?3 IS NULL OR d.device = ?3) AND (?4 IS NULL OR d.level = ?4)";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM diagnostics d JOIN users u ON u.id = d.user_id {filter}"
    )))
    .bind(since)
    .bind(&query.user)
    .bind(&query.device)
    .bind(&query.level)
    .fetch_one(&pool)
    .await?;
    let rows: Vec<ReportRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} {filter} ORDER BY d.received_at DESC, d.id DESC LIMIT ?5 OFFSET ?6"
    )))
    .bind(since)
    .bind(&query.user)
    .bind(&query.device)
    .bind(&query.level)
    .bind(limit)
    .bind(offset)
    .fetch_all(&pool)
    .await?;
    Ok(Json(json!({
        "total": total,
        "items": rows.iter().map(ReportRow::to_json).collect::<Vec<_>>(),
    })))
}

/// One report with its details.
async fn one_report(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let row: Option<ReportRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE d.id = ? AND d.received_at >= ?"
    )))
    .bind(id)
    .bind(unix_now() - diagnostics::KEEP_FOR)
    .fetch_optional(&pool)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such report"));
    };
    let mut body = row.to_json();
    body["details"] = serde_json::from_str(&row.details)?;
    Ok(Json(body))
}

/// Deletes one report.
async fn delete_report(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let deleted = sqlx::query("DELETE FROM diagnostics WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such report"));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct DeviceQuery {
    /// The user's name; device names are per user.
    user: Option<String>,
    device: Option<String>,
}

/// Deletes every report from one device of one user.
async fn delete_device_reports(
    State(pool): State<SqlitePool>,
    Query(query): Query<DeviceQuery>,
) -> Result<Json<Value>, ApiError> {
    let (Some(user), Some(device)) = (query.user, query.device) else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "say whose device: user and device",
        ));
    };
    let deleted = sqlx::query(
        "DELETE FROM diagnostics WHERE device = ?
           AND user_id = (SELECT id FROM users WHERE username = ?)",
    )
    .bind(&device)
    .bind(&user)
    .execute(&pool)
    .await?
    .rows_affected();
    Ok(Json(json!({ "deleted": deleted })))
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

    struct Fixture {
        pool: SqlitePool,
        app: Router,
        admin: String,
        alice: String,
    }

    async fn fixture() -> Fixture {
        let pool = db::open_in_memory().await;
        let token = |name: &'static str, admin: bool, device: &'static str| {
            let pool = pool.clone();
            async move {
                let user = auth::create_user(&pool, name, "a-long-password".into(), admin)
                    .await
                    .unwrap()
                    .unwrap();
                auth::issue_token(&pool, user.id, TokenKind::Session, device)
                    .await
                    .unwrap()
                    .1
            }
        };
        Fixture {
            admin: token("admin", true, "Laptop").await,
            alice: token("alice", false, "Living room iPad").await,
            app: routes(pool.clone()),
            pool,
        }
    }

    async fn send(
        f: &Fixture,
        method: &str,
        uri: &str,
        token: &str,
        body: Option<String>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or_else(Body::empty, Body::from))
            .unwrap();
        let response = f.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn devices_send_reports_and_admins_read_them() {
        let f = fixture().await;
        let report = json!({
            "app": "Video app", "app_version": "2.1", "platform": "tvOS 26", "level": "error",
            "summary": "Playback stalled", "details": { "file_id": 4, "password": "nope" },
        });
        let (status, sent) = send(
            &f,
            "POST",
            "/api/v1/me/diagnostics",
            &f.alice,
            Some(report.to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, _) = send(
            &f,
            "POST",
            "/api/v1/me/diagnostics",
            &f.admin,
            Some(json!({ "app": "Web", "summary": "Fine" }).to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        let (status, page) = send(&f, "GET", "/api/v1/diagnostics", &f.admin, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["total"], 2);
        assert_eq!(page["items"][0]["summary"], "Fine", "newest first");
        assert_eq!(page["items"][1]["device"], "Living room iPad");
        assert_eq!(page["items"][1]["user"], "alice");
        let (_, only) = send(
            &f,
            "GET",
            "/api/v1/diagnostics?user=alice&level=error",
            &f.admin,
            None,
        )
        .await;
        assert_eq!(only["total"], 1);
        let (_, none) = send(
            &f,
            "GET",
            "/api/v1/diagnostics?device=Phone",
            &f.admin,
            None,
        )
        .await;
        assert_eq!(none["total"], 0);

        let (status, one) = send(
            &f,
            "GET",
            &format!("/api/v1/diagnostics/{}", sent["id"]),
            &f.admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            one["details"],
            json!({ "file_id": 4, "password": "[redacted]" })
        );
        assert_eq!(one["level"], "error");

        // Older than 30 days: no longer shown, even before housekeeping deletes it.
        sqlx::query("UPDATE diagnostics SET received_at = received_at - 31 * 86400")
            .execute(&f.pool)
            .await
            .unwrap();
        let (_, page) = send(&f, "GET", "/api/v1/diagnostics", &f.admin, None).await;
        assert_eq!(page["total"], 0);
        let (status, _) = send(
            &f,
            "GET",
            &format!("/api/v1/diagnostics/{}", sent["id"]),
            &f.admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn too_big_bad_or_unauthorised_reports_are_refused() {
        let f = fixture().await;
        let huge = json!({ "app": "a", "summary": "s", "details": { "log": "x".repeat(70_000) } });
        let (status, body) = send(
            &f,
            "POST",
            "/api/v1/me/diagnostics",
            &f.alice,
            Some(huge.to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
        let (status, body) = send(
            &f,
            "POST",
            "/api/v1/me/diagnostics",
            &f.alice,
            Some(json!({ "app": "a", "summary": "s", "level": "fatal" }).to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("level"));
        let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM diagnostics")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(kept, 0);
        for uri in ["/api/v1/diagnostics", "/api/v1/diagnostics/1"] {
            assert_eq!(
                send(&f, "GET", uri, &f.alice, None).await.0,
                StatusCode::FORBIDDEN,
                "{uri}"
            );
        }
        assert_eq!(
            send(&f, "GET", "/api/v1/diagnostics?limit=0", &f.admin, None)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn admins_delete_a_report_or_all_of_one_devices() {
        let f = fixture().await;
        let mut ids = Vec::new();
        for (token, summary) in [
            (&f.alice, "Stalled"),
            (&f.alice, "Stalled again"),
            (&f.alice, "And again"),
            (&f.admin, "Fine"),
        ] {
            let body = json!({ "app": "Video app", "summary": summary }).to_string();
            let (status, sent) =
                send(&f, "POST", "/api/v1/me/diagnostics", token, Some(body)).await;
            assert_eq!(status, StatusCode::CREATED);
            ids.push(sent["id"].as_i64().unwrap());
        }
        let one = format!("/api/v1/diagnostics/{}", ids[0]);
        assert_eq!(
            send(&f, "DELETE", &one, &f.alice, None).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            send(&f, "DELETE", &one, &f.admin, None).await.0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            send(&f, "DELETE", &one, &f.admin, None).await.0,
            StatusCode::NOT_FOUND
        );

        // A whole device: whose, and which, must both be said.
        assert_eq!(
            send(
                &f,
                "DELETE",
                "/api/v1/diagnostics?device=Laptop",
                &f.admin,
                None
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        let (status, body) = send(
            &f,
            "DELETE",
            "/api/v1/diagnostics?user=alice&device=Living%20room%20iPad",
            &f.admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({ "deleted": 2 }));
        let left: Vec<String> = sqlx::query_scalar("SELECT summary FROM diagnostics")
            .fetch_all(&f.pool)
            .await
            .unwrap();
        assert_eq!(left, vec!["Fine"]);
    }
}
