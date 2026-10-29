use axum::body::Body;
use axum::extract::Request;
use axum::http::header;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};

struct Fixture {
    pool: SqlitePool,
    app: Router,
    admin: String,
    kid: i64,
    kid_token: String,
}

async fn fixture() -> Fixture {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m'),
             (2, 'kids', 'movies', '/k'), (3, 'music', 'music', '/a');",
    )
    .execute(&pool)
    .await
    .unwrap();
    let token = |admin: bool, name: &'static str| {
        let pool = pool.clone();
        async move {
            let user = auth::create_user(&pool, name, "a-long-password".into(), admin)
                .await
                .unwrap()
                .unwrap();
            let token = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
                .await
                .unwrap()
                .1;
            (user.id, token)
        }
    };
    let (_, admin) = token(true, "parent").await;
    let (kid, kid_token) = token(false, "kid").await;
    Fixture {
        app: routes(pool.clone()),
        pool,
        admin,
        kid,
        kid_token,
    }
}

async fn send(
    f: &Fixture,
    token: &str,
    method: &str,
    uri: &str,
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

#[tokio::test]
async fn admins_set_which_libraries_and_ratings_a_user_sees() {
    let f = fixture().await;
    let uri = format!("/api/v1/users/{}/access", f.kid);
    let (status, body) = send(&f, &f.admin, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({ "user_id": f.kid, "admin": false, "libraries": null, "max_age": null })
    );
    let kids = json!({ "libraries": ["music", "kids", "kids"], "max_age": 7 });
    let (status, body) = send(&f, &f.admin, "PUT", &uri, Some(kids)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["libraries"], json!(["kids", "music"]));
    assert_eq!(body["max_age"], 7);
    assert_eq!(send(&f, &f.admin, "GET", &uri, None).await.1, body);
    assert_eq!(
        crate::access::Access::of(&f.pool, f.kid, false)
            .await
            .unwrap(),
        crate::access::Access::limited(Some(vec![2, 3]), Some(7))
    );

    let everything = json!({ "libraries": null, "max_age": null });
    let (_, body) = send(&f, &f.admin, "PUT", &uri, Some(everything)).await;
    assert_eq!(
        (&body["libraries"], &body["max_age"]),
        (&Value::Null, &Value::Null)
    );
    assert!(
        crate::access::Access::of(&f.pool, f.kid, false)
            .await
            .unwrap()
            == crate::access::Access::everything()
    );
}

#[tokio::test]
async fn wrong_settings_are_refused_and_only_admins_may_set_them() {
    let f = fixture().await;
    let uri = format!("/api/v1/users/{}/access", f.kid);
    for wrong in [
        json!({ "libraries": ["nowhere"], "max_age": null }),
        json!({ "libraries": null, "max_age": 19 }),
        json!({ "libraries": null, "max_age": -1 }),
    ] {
        assert_eq!(
            send(&f, &f.admin, "PUT", &uri, Some(wrong)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        send(&f, &f.admin, "GET", "/api/v1/users/99/access", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let open = json!({ "libraries": null, "max_age": null });
    assert_eq!(
        send(&f, &f.kid_token, "PUT", &uri, Some(open)).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&f, &f.kid_token, "GET", &uri, None).await.0,
        StatusCode::FORBIDDEN
    );
}
