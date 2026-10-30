use axum::body::Body;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::db;

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    send_from(app, None, method, uri, token, body).await
}

async fn send_from(
    app: &Router,
    address: Option<&str>,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(address) = address {
        let peer: SocketAddr = format!("{address}:5000").parse().unwrap();
        request = request.extension(ConnectInfo(peer));
    }
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = match body {
        Some(body) => request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

async fn login_as(app: &Router, username: &str, password: &str) -> String {
    let body = json!({ "username": username, "password": password, "device_name": "test" });
    let (status, body) = send(app, "POST", "/api/v1/auth/login", None, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["token"].as_str().unwrap().to_owned()
}

async fn setup() -> (SqlitePool, Router) {
    let pool = db::open_in_memory().await;
    auth::create_user(&pool, "admin", "admin-password".into(), true)
        .await
        .unwrap();
    auth::create_user(&pool, "viewer", "viewer-password".into(), false)
        .await
        .unwrap();
    let app = super::super::app(pool.clone());
    (pool, app)
}

#[tokio::test]
async fn login_returns_a_working_token() {
    let (_, app) = setup().await;
    let body =
        json!({ "username": "Viewer", "password": "viewer-password", "device_name": "  iPhone  " });
    let (status, body) = send(&app, "POST", "/api/v1/auth/login", None, Some(body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["user"]["username"], "viewer");
    let token = body["token"].as_str().unwrap();

    let (status, me) = send(&app, "GET", "/api/v1/me", Some(token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        me,
        json!({ "id": 2, "username": "viewer", "is_admin": false, "sees_all_music": true })
    );
    let (_, tokens) = send(&app, "GET", "/api/v1/auth/tokens", Some(token), None).await;
    assert_eq!(tokens[0]["name"], "iPhone");
    assert_eq!(tokens[0]["kind"], "session");
    assert_eq!(tokens[0]["current"], true);
    assert!(tokens[0].get("token").is_none());
}

#[tokio::test]
async fn wrong_password_and_unknown_user_look_the_same() {
    let (_, app) = setup().await;
    for (username, password) in [("viewer", "wrong-password"), ("nobody", "viewer-password")] {
        let body = json!({ "username": username, "password": password, "device_name": "x" });
        let (status, body) = send(&app, "POST", "/api/v1/auth/login", None, Some(body)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "wrong username or password");
    }
}

#[tokio::test]
async fn repeated_failures_lock_even_the_right_password_out() {
    let (_, app) = setup().await;
    let attempt =
        |password: &str| json!({ "username": "viewer", "password": password, "device_name": "x" });
    for _ in 0..5 {
        send(
            &app,
            "POST",
            "/api/v1/auth/login",
            None,
            Some(attempt("guess")),
        )
        .await;
    }
    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(attempt("viewer-password")),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    login_as(&app, "admin", "admin-password").await;
}

#[tokio::test]
async fn parallel_guesses_get_no_more_tries_than_sequential_ones() {
    let (_, app) = setup().await;
    let mut tasks = Vec::new();
    for _ in 0..40 {
        let app = app.clone();
        tasks.push(tokio::spawn(async move {
            let body = json!({ "username": "viewer", "password": "guess", "device_name": "x" });
            send(&app, "POST", "/api/v1/auth/login", None, Some(body))
                .await
                .0
        }));
    }
    let mut checked = 0;
    for task in tasks {
        match task.await.unwrap() {
            StatusCode::UNAUTHORIZED => checked += 1,
            status => assert_eq!(status, StatusCode::TOO_MANY_REQUESTS),
        }
    }
    assert_eq!(checked, 5);
}

#[tokio::test]
async fn lockout_is_per_address_and_spares_password_changes() {
    let (_, app) = setup().await;
    let session = login_as(&app, "viewer", "viewer-password").await;
    let attempt =
        |password: &str| json!({ "username": "viewer", "password": password, "device_name": "x" });
    for _ in 0..5 {
        send_from(
            &app,
            Some("203.0.113.9"),
            "POST",
            "/api/v1/auth/login",
            None,
            Some(attempt("guess")),
        )
        .await;
    }
    let locked = send_from(
        &app,
        Some("203.0.113.9"),
        "POST",
        "/api/v1/auth/login",
        None,
        Some(attempt("viewer-password")),
    )
    .await;
    assert_eq!(locked.0, StatusCode::TOO_MANY_REQUESTS);
    let elsewhere = send_from(
        &app,
        Some("192.168.1.20"),
        "POST",
        "/api/v1/auth/login",
        None,
        Some(attempt("viewer-password")),
    )
    .await;
    assert_eq!(
        elsewhere.0,
        StatusCode::OK,
        "the real user, from their own address, gets in"
    );

    let change = json!({ "current_password": "viewer-password", "new_password": "new-password" });
    let (status, _) = send(
        &app,
        "PUT",
        "/api/v1/me/password",
        Some(&session),
        Some(change),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn impossible_usernames_are_refused_like_wrong_passwords() {
    let (_, app) = setup().await;
    for username in ["x".repeat(100_000), "--admin".into()] {
        let body = json!({ "username": username, "password": "anything", "device_name": "x" });
        let (status, body) = send(&app, "POST", "/api/v1/auth/login", None, Some(body)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "wrong username or password");
    }
}

#[tokio::test]
async fn library_data_needs_an_admin() {
    let (_, app) = setup().await;
    let viewer = login_as(&app, "viewer", "viewer-password").await;
    let admin = login_as(&app, "admin", "admin-password").await;
    for uri in [
        "/api/v1/libraries",
        "/api/v1/metadata/unmatched",
        "/api/v1/users",
    ] {
        assert_eq!(
            send(&app, "GET", uri, None, None).await.0,
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
        assert_eq!(
            send(&app, "GET", uri, Some("mlt_forged"), None).await.0,
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
        assert_eq!(
            send(&app, "GET", uri, Some(&viewer), None).await.0,
            StatusCode::FORBIDDEN,
            "{uri}"
        );
        assert_eq!(
            send(&app, "GET", uri, Some(&admin), None).await.0,
            StatusCode::OK,
            "{uri}"
        );
    }
    assert_eq!(
        send(&app, "GET", "/api/v1/health", None, None).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn logout_revokes_only_that_token() {
    let (_, app) = setup().await;
    let phone = login_as(&app, "viewer", "viewer-password").await;
    let tv = login_as(&app, "viewer", "viewer-password").await;
    assert_eq!(
        send(&app, "POST", "/api/v1/auth/logout", Some(&phone), None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(&app, "GET", "/api/v1/me", Some(&phone), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&app, "GET", "/api/v1/me", Some(&tv), None).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn api_keys_work_until_revoked() {
    let (_, app) = setup().await;
    let session = login_as(&app, "viewer", "viewer-password").await;
    let (status, created) = send(
        &app,
        "POST",
        "/api/v1/auth/keys",
        Some(&session),
        Some(json!({ "name": "Apple TV" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let key = created["token"].as_str().unwrap();
    assert_eq!(
        send(&app, "GET", "/api/v1/me", Some(key), None).await.0,
        StatusCode::OK
    );

    let uri = format!("/api/v1/auth/tokens/{}", created["id"]);
    assert_eq!(
        send(&app, "DELETE", &uri, Some(&session), None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(&app, "GET", "/api/v1/me", Some(key), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&app, "DELETE", &uri, Some(&session), None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn cannot_revoke_someone_elses_token() {
    let (_, app) = setup().await;
    let viewer = login_as(&app, "viewer", "viewer-password").await;
    let admin = login_as(&app, "admin", "admin-password").await;
    let (_, tokens) = send(&app, "GET", "/api/v1/auth/tokens", Some(&admin), None).await;
    let uri = format!("/api/v1/auth/tokens/{}", tokens[0]["id"]);
    assert_eq!(
        send(&app, "DELETE", &uri, Some(&viewer), None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, "GET", "/api/v1/me", Some(&admin), None).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn password_change_needs_the_current_one_and_signs_out_elsewhere() {
    let (_, app) = setup().await;
    let here = login_as(&app, "viewer", "viewer-password").await;
    let elsewhere = login_as(&app, "viewer", "viewer-password").await;
    let change =
        |current: &str, new: &str| json!({ "current_password": current, "new_password": new });

    let (status, _) = send(
        &app,
        "PUT",
        "/api/v1/me/password",
        Some(&here),
        Some(change("nope", "new-password")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &app,
        "PUT",
        "/api/v1/me/password",
        Some(&here),
        Some(change("viewer-password", "short")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(
        &app,
        "PUT",
        "/api/v1/me/password",
        Some(&here),
        Some(change("viewer-password", "new-password")),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert_eq!(
        send(&app, "GET", "/api/v1/me", Some(&here), None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        send(&app, "GET", "/api/v1/me", Some(&elsewhere), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    login_as(&app, "viewer", "new-password").await;
}

#[tokio::test]
async fn admins_create_users() {
    let (_, app) = setup().await;
    let admin = login_as(&app, "admin", "admin-password").await;
    let viewer = login_as(&app, "viewer", "viewer-password").await;
    let new_user = json!({ "username": "guest", "password": "guest-password" });

    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/users",
        Some(&viewer),
        Some(new_user.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, user) = send(
        &app,
        "POST",
        "/api/v1/users",
        Some(&admin),
        Some(new_user.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(user["is_admin"], false);
    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/users",
        Some(&admin),
        Some(json!({ "username": "GUEST", "password": "other-password" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/users",
        Some(&admin),
        Some(json!({ "username": "bad name", "password": "guest-password" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("usernames"));
    login_as(&app, "guest", "guest-password").await;
}

#[tokio::test]
async fn admins_reset_passwords_and_delete_users() {
    let (pool, app) = setup().await;
    let admin = login_as(&app, "admin", "admin-password").await;
    let viewer = login_as(&app, "viewer", "viewer-password").await;
    let viewer_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'viewer'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let reset = format!("/api/v1/users/{viewer_id}/password");

    // Only admins, and not on themselves.
    let (status, _) = send(&app, "POST", &reset, Some(&viewer), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/v1/users/{admin_id}/password"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // A reset: a new password, shown once, and the user signed out everywhere.
    let (status, body) = send(&app, "POST", &reset, Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["username"], "viewer");
    assert_eq!(
        body.as_object().unwrap().len(),
        2,
        "exactly PasswordReset's fields"
    );
    let password = body["password"].as_str().unwrap().to_owned();
    let (status, _) = send(&app, "GET", "/api/v1/me", Some(&viewer), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the old session is gone");
    let viewer = login_as(&app, "viewer", &password).await;

    // Deleting: never yourself, never the last admin; a user's data goes with them.
    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/api/v1/users/{admin_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        auth::delete_user(&pool, admin_id).await.unwrap(),
        auth::Deleted::LastAdmin
    );
    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/api/v1/users/{viewer_id}"),
        Some(&viewer),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/api/v1/users/{viewer_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(&app, "GET", "/api/v1/me", Some(&viewer), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM tokens WHERE user_id = ?")
        .bind(viewer_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/api/v1/users/{viewer_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(&app, "POST", &reset, Some(&admin), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admins_rename_users_and_make_or_take_away_admin() {
    let (pool, app) = setup().await;
    let admin = login_as(&app, "admin", "admin-password").await;
    let viewer = login_as(&app, "viewer", "viewer-password").await;
    let (admin_id, viewer_id) = (1, 2);
    let put = |id: i64, token: String, body: Value| {
        let app = app.clone();
        async move {
            send(
                &app,
                "PUT",
                &format!("/api/v1/users/{id}"),
                Some(&token),
                Some(body),
            )
            .await
        }
    };

    // Only admins, and with a good name.
    let (status, _) = put(
        viewer_id,
        viewer.clone(),
        json!({ "username": "v", "is_admin": false }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = put(
        viewer_id,
        admin.clone(),
        json!({ "username": "-x", "is_admin": false }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = put(
        99,
        admin.clone(),
        json!({ "username": "x", "is_admin": false }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Renaming: a taken name (case aside) is refused; the session carries on.
    let (status, body) = put(
        viewer_id,
        admin.clone(),
        json!({ "username": "ADMIN", "is_admin": false }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "that username is taken");
    let (status, body) = put(
        viewer_id,
        admin.clone(),
        json!({ "username": "carol", "is_admin": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({ "id": viewer_id, "username": "carol", "is_admin": false })
    );
    let (_, me) = send(&app, "GET", "/api/v1/me", Some(&viewer), None).await;
    assert_eq!(me["username"], "carol");
    login_as(&app, "carol", "viewer-password").await;

    // The last admin keeps it, and nobody takes their own away.
    let (status, body) = put(
        admin_id,
        admin.clone(),
        json!({ "username": "admin", "is_admin": false }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        body["error"],
        "you can't take away your own admin; ask another admin"
    );
    assert_eq!(
        auth::change_user(&pool, admin_id, None, Some(false))
            .await
            .unwrap(),
        auth::Changed::LastAdmin
    );

    // Made an admin, the user can manage users at once; then takes it from the first admin.
    let (status, body) = put(
        viewer_id,
        admin.clone(),
        json!({ "username": "carol", "is_admin": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["is_admin"], true);
    let (status, _) = put(
        admin_id,
        viewer.clone(),
        json!({ "username": "admin", "is_admin": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&app, "GET", "/api/v1/users", Some(&admin), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "no longer an admin");
    // Now carol is the last admin.
    assert_eq!(
        auth::change_user(&pool, viewer_id, None, Some(false))
            .await
            .unwrap(),
        auth::Changed::LastAdmin
    );
    // Taking it from someone who isn't an admin changes nothing and is fine.
    let (status, body) = put(
        admin_id,
        viewer.clone(),
        json!({ "username": "admin", "is_admin": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["is_admin"], false);
}

#[tokio::test]
async fn quick_connect_signs_a_device_in_once_its_code_is_approved() {
    let (_, app) = setup().await;
    let viewer = login_as(&app, "viewer", "viewer-password").await;
    let (status, started) = send(
        &app,
        "POST",
        "/api/v1/auth/quick-connect",
        None,
        Some(json!({ "device_name": "Living room TV" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{started}");
    assert_eq!(started["expires_in"], 300);
    let code = started["code"].as_str().unwrap().to_owned();
    let secret = json!({ "secret": started["secret"] });

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/auth/quick-connect/poll",
        None,
        Some(secret.clone()),
    )
    .await;
    assert_eq!(
        (status, body),
        (StatusCode::ACCEPTED, json!({ "status": "waiting" }))
    );

    // Looking up and approving need someone signed in; approving needs a lookup first,
    // which shows the device, where it asked from and how long ago.
    let approve = json!({ "code": code });
    for path in ["lookup", "approve"] {
        let (status, _) = send(
            &app,
            "POST",
            &format!("/api/v1/auth/quick-connect/{path}"),
            None,
            Some(approve.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/auth/quick-connect/approve",
        Some(&viewer),
        Some(approve.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "not before a lookup");
    let (status, seen) = send(
        &app,
        "POST",
        "/api/v1/auth/quick-connect/lookup",
        Some(&viewer),
        Some(approve.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert_eq!(seen["device_name"], "Living room TV");
    assert_eq!(seen["address"], "unknown", "no address in tests");
    assert!(seen["age_seconds"].as_u64().unwrap() < 5);
    assert!(seen["expires_in"].as_u64().unwrap() > 290);
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/auth/quick-connect/approve",
        Some(&viewer),
        Some(approve.clone()),
    )
    .await;
    assert_eq!(
        (status, body),
        (StatusCode::OK, json!({ "device_name": "Living room TV" }))
    );
    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/auth/quick-connect/approve",
        Some(&viewer),
        Some(approve),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a code is approved once");

    // The device gets a session of its own, once, as whoever approved it.
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/auth/quick-connect/poll",
        None,
        Some(secret.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["username"], "viewer");
    let token = body["token"].as_str().unwrap();
    assert!(token.starts_with("mlt_"));
    let (_, tokens) = send(&app, "GET", "/api/v1/auth/tokens", Some(token), None).await;
    let tv = tokens
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["current"] == true)
        .unwrap();
    assert_eq!(
        (tv["name"].as_str(), tv["kind"].as_str()),
        (Some("Living room TV"), Some("session"))
    );
    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/auth/quick-connect/poll",
        None,
        Some(secret),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the code is used up");
}

#[tokio::test]
async fn quick_connect_codes_alone_sign_nobody_in_and_guesses_are_limited() {
    let (_, app) = setup().await;
    let viewer = login_as(&app, "viewer", "viewer-password").await;
    let (_, started) = send(
        &app,
        "POST",
        "/api/v1/auth/quick-connect",
        None,
        Some(json!({ "device_name": "tv" })),
    )
    .await;
    let code = started["code"].as_str().unwrap();
    // Knowing the code someone's TV shows doesn't collect its session.
    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/auth/quick-connect/poll",
        None,
        Some(json!({ "secret": code })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/auth/quick-connect",
        None,
        Some(json!({ "device_name": "  " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Ten lookups an hour, found or not: approving a code of one's own doesn't buy more.
    let wrong = if code == "AAAAAA" { "BBBBBB" } else { "AAAAAA" };
    for n in 0..quick_connect::LOOKUPS_PER_HOUR {
        let (guess, secret) = if n % 2 == 0 {
            let (_, mine) = send(
                &app,
                "POST",
                "/api/v1/auth/quick-connect",
                None,
                Some(json!({ "device_name": "mine" })),
            )
            .await;
            (
                mine["code"].as_str().unwrap().to_owned(),
                mine["secret"].clone(),
            )
        } else {
            (wrong.to_owned(), Value::Null)
        };
        let (status, _) = send(
            &app,
            "POST",
            "/api/v1/auth/quick-connect/lookup",
            Some(&viewer),
            Some(json!({ "code": guess })),
        )
        .await;
        let expected = if n % 2 == 0 {
            StatusCode::OK
        } else {
            StatusCode::NOT_FOUND
        };
        assert_eq!(status, expected);
        if n % 2 == 0 {
            let (status, _) = send(
                &app,
                "POST",
                "/api/v1/auth/quick-connect/approve",
                Some(&viewer),
                Some(json!({ "code": guess })),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            // Collected, so it doesn't hold a place among the codes waiting.
            send(
                &app,
                "POST",
                "/api/v1/auth/quick-connect/poll",
                None,
                Some(json!({ "secret": secret })),
            )
            .await;
        }
    }
    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/auth/quick-connect/lookup",
        Some(&viewer),
        Some(json!({ "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let admin = login_as(&app, "admin", "admin-password").await;
    for path in ["lookup", "approve"] {
        let (status, _) = send(
            &app,
            "POST",
            &format!("/api/v1/auth/quick-connect/{path}"),
            Some(&admin),
            Some(json!({ "code": code })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "someone else isn't held up");
    }
}

#[test]
fn home_is_a_way_in_that_opens_everything() {
    assert!(from_home(None), "no ways in set");
    assert!(from_home(Some(Extension(Opens(None)))));
    assert!(from_home(Some(Extension(Opens(Some(vec![
        Part::Admin,
        Part::Music
    ]))))));
    assert!(!from_home(Some(Extension(Opens(Some(vec![
        Part::Music,
        Part::Requests
    ]))))));
}

#[tokio::test]
async fn quick_connect_gives_nothing_once_the_approver_is_gone() {
    let (pool, app) = setup().await;
    let viewer = login_as(&app, "viewer", "viewer-password").await;
    let (_, started) = send(
        &app,
        "POST",
        "/api/v1/auth/quick-connect",
        None,
        Some(json!({ "device_name": "tv" })),
    )
    .await;
    for path in ["lookup", "approve"] {
        send(
            &app,
            "POST",
            &format!("/api/v1/auth/quick-connect/{path}"),
            Some(&viewer),
            Some(json!({ "code": started["code"] })),
        )
        .await;
    }
    sqlx::query("DELETE FROM users WHERE username = 'viewer'")
        .execute(&pool)
        .await
        .unwrap();
    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/auth/quick-connect/poll",
        None,
        Some(json!({ "secret": started["secret"] })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A request with exactly these headers; answers status, headers and JSON body.
async fn send_raw(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = match body {
        Some(body) => request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, body)
}

#[tokio::test]
async fn the_web_ui_signs_in_to_a_cookie_its_script_cant_read() {
    let (_, app) = setup().await;
    let credentials = json!({ "username": "viewer", "password": "viewer-password", "device_name": "Firefox on Linux" });
    let (status, headers, body) = send_raw(
        &app,
        "POST",
        "/api/v1/auth/web-login",
        &[],
        Some(credentials.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "user": { "id": 2, "username": "viewer", "is_admin": false } })
    );
    let set = headers[header::SET_COOKIE].to_str().unwrap().to_owned();
    assert!(
        set.contains("; HttpOnly") && set.contains("; SameSite=Strict"),
        "{set}"
    );
    assert!(
        set.contains("; Max-Age=7776000") && !set.contains("Secure"),
        "{set}"
    );
    let token = set
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("melete_session=")
        .unwrap();
    assert!(token.starts_with("mlt_"));
    let cookie = format!("theme=dark; melete_session={token}");

    // Reading needs only the cookie; /me sets it again, so it lasts as long as the session.
    let (status, headers, body) =
        send_raw(&app, "GET", "/api/v1/me", &[("cookie", &cookie)], None).await;
    assert_eq!(
        (status, body["username"].as_str()),
        (StatusCode::OK, Some("viewer"))
    );
    assert_eq!(headers[header::SET_COOKIE].to_str().unwrap(), set);
    let tokens = send(&app, "GET", "/api/v1/auth/tokens", Some(token), None)
        .await
        .1;
    assert_eq!(tokens[0]["name"], "Firefox on Linux");

    // A change needs the header too, which another site's page can't add.
    let key = json!({ "name": "script" });
    let (status, _, body) = send_raw(
        &app,
        "POST",
        "/api/v1/auth/keys",
        &[("cookie", &cookie)],
        Some(key.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let web = [("cookie", cookie.as_str()), (WEB_HEADER, "1")];
    let (status, _, _) = send_raw(&app, "POST", "/api/v1/auth/keys", &web, Some(key)).await;
    assert_eq!(status, StatusCode::CREATED);

    // Signing out ends the session and clears the cookie.
    let (status, headers, _) = send_raw(&app, "POST", "/api/v1/auth/logout", &web, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let cleared = headers[header::SET_COOKIE].to_str().unwrap();
    assert!(
        cleared.starts_with("melete_session=;") && cleared.contains("Max-Age=0"),
        "{cleared}"
    );
    let (status, _, _) = send_raw(&app, "GET", "/api/v1/me", &[("cookie", &cookie)], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Over https the cookie is Secure; a wrong password sets none.
    let https = [("origin", "https://media.example")];
    let (_, headers, _) = send_raw(
        &app,
        "POST",
        "/api/v1/auth/web-login",
        &https,
        Some(credentials),
    )
    .await;
    assert!(
        headers[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .ends_with("; Secure")
    );
    let wrong = json!({ "username": "viewer", "password": "nope-nope-nope", "device_name": "x" });
    let (status, headers, _) =
        send_raw(&app, "POST", "/api/v1/auth/web-login", &[], Some(wrong)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!headers.contains_key(header::SET_COOKIE));
}

#[tokio::test]
async fn a_bearer_token_wins_over_a_cookie() {
    let (_, app) = setup().await;
    let admin = login_as(&app, "admin", "admin-password").await;
    let bearer = format!("Bearer {admin}");
    let headers = [
        ("cookie", "melete_session=mlt_stale"),
        ("authorization", bearer.as_str()),
    ];
    let (status, _, body) = send_raw(&app, "GET", "/api/v1/me", &headers, None).await;
    assert_eq!(
        (status, body["username"].as_str()),
        (StatusCode::OK, Some("admin"))
    );
    // And a bearer request that changes something needs no web header.
    let (status, _, _) = send_raw(
        &app,
        "POST",
        "/api/v1/auth/keys",
        &headers,
        Some(json!({ "name": "k" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _, _) = send_raw(
        &app,
        "GET",
        "/api/v1/me",
        &[("cookie", "melete_session=")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_key_is_made_for_what_its_for_and_says_so() {
    let (_, app) = setup().await;
    let admin = login_as(&app, "admin", "admin-password").await;
    let make = |body: Value| send(&app, "POST", "/api/v1/auth/keys", Some(&admin), Some(body));
    let (status, read) = make(json!({ "name": "Dashboard", "scope": "read" })).await;
    assert_eq!(
        (status, &read["scope"]),
        (StatusCode::CREATED, &json!("read"))
    );
    let (_, old_style) = make(json!({ "name": "Script" })).await;
    assert_eq!(
        old_style["scope"], "full",
        "a client older than scopes makes a full key"
    );
    let (status, _) = make(json!({ "name": "Odd", "scope": "admin" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, tokens) = send(&app, "GET", "/api/v1/auth/tokens", Some(&admin), None).await;
    let scopes: Vec<(&str, &str)> = tokens
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap(), t["scope"].as_str().unwrap()))
        .collect();
    assert_eq!(
        scopes,
        [("test", "full"), ("Dashboard", "read"), ("Script", "full")]
    );

    // An admin's read key reads, and reaches nothing the admin's session could change.
    let key = read["token"].as_str().unwrap();
    assert_eq!(
        send(&app, "GET", "/api/v1/me", Some(key), None).await.0,
        StatusCode::OK
    );
    for (method, uri) in [
        ("GET", "/api/v1/users"),
        ("POST", "/api/v1/auth/keys"),
        ("POST", "/api/v1/auth/logout"),
    ] {
        let body = (method == "POST").then(|| json!({ "name": "x", "scope": "full" }));
        assert_eq!(
            send(&app, method, uri, Some(key), body).await.0,
            StatusCode::FORBIDDEN,
            "{method} {uri}"
        );
    }
}

#[tokio::test]
async fn admins_see_a_users_api_keys_and_their_scopes_but_never_the_keys() {
    let (pool, app) = setup().await;
    let admin = login_as(&app, "admin", "admin-password").await;
    let viewer = login_as(&app, "viewer", "viewer-password").await;
    let made = send(
        &app,
        "POST",
        "/api/v1/auth/keys",
        Some(&viewer),
        Some(json!({ "name": "Dashboard", "scope": "read" })),
    )
    .await
    .1;
    let secret = made["token"].as_str().unwrap().to_owned();
    let id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'viewer'")
        .fetch_one(&pool)
        .await
        .unwrap();

    let uri = format!("/api/v1/users/{id}/keys");
    let (status, keys) = send(&app, "GET", &uri, Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let listed: Vec<(&str, &str)> = keys
        .as_array()
        .unwrap()
        .iter()
        .map(|k| (k["name"].as_str().unwrap(), k["scope"].as_str().unwrap()))
        .collect();
    assert_eq!(listed, [("Dashboard", "read")], "keys only, not sessions");
    assert!(keys[0]["created_at"].is_i64() && keys[0]["last_used_at"].is_null());
    assert!(!keys.to_string().contains(&secret));
    assert!(keys[0].get("token").is_none() && keys[0].get("token_hash").is_none());

    assert_eq!(
        send(&app, "GET", "/api/v1/users/999/keys", Some(&admin), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, "GET", &uri, Some(&viewer), None).await.0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn admins_revoke_one_of_a_users_api_keys_and_nothing_else() {
    let (pool, app) = setup().await;
    let admin = login_as(&app, "admin", "admin-password").await;
    let viewer = login_as(&app, "viewer", "viewer-password").await;
    let make = |name: &str| {
        let body = json!({ "name": name });
        let app = app.clone();
        let viewer = viewer.clone();
        async move {
            send(&app, "POST", "/api/v1/auth/keys", Some(&viewer), Some(body))
                .await
                .1
        }
    };
    let (tv, phone) = (make("TV").await, make("Phone").await);
    let tv_key = tv["token"].as_str().unwrap();
    let phone_key = phone["token"].as_str().unwrap();
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM users ORDER BY username")
        .fetch_all(&pool)
        .await
        .unwrap();
    let (admin_id, viewer_id) = (ids[0], ids[1]);
    let me = |token: &str| {
        let (app, token) = (app.clone(), token.to_owned());
        async move { send(&app, "GET", "/api/v1/me", Some(&token), None).await.0 }
    };

    // Not an admin, not even the key's owner: refused, and the key still works.
    let uri = format!("/api/v1/users/{viewer_id}/keys/{}", tv["id"]);
    assert_eq!(
        send(&app, "DELETE", &uri, Some(&viewer), None).await.0,
        StatusCode::FORBIDDEN
    );
    // The key under another user's id is no key of theirs.
    let wrong = format!("/api/v1/users/{admin_id}/keys/{}", tv["id"]);
    assert_eq!(
        send(&app, "DELETE", &wrong, Some(&admin), None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(me(tv_key).await, StatusCode::OK);

    assert_eq!(
        send(&app, "DELETE", &uri, Some(&admin), None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(me(tv_key).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        send(&app, "DELETE", &uri, Some(&admin), None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(me(phone_key).await, StatusCode::OK, "their other key works");
    assert_eq!(me(&viewer).await, StatusCode::OK, "their session works");

    // A session is never an admin's to revoke here; it stays the user's own.
    let session: i64 =
        sqlx::query_scalar("SELECT id FROM tokens WHERE user_id = ? AND kind = 'session'")
            .bind(viewer_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let uri = format!("/api/v1/users/{viewer_id}/keys/{session}");
    assert_eq!(
        send(&app, "DELETE", &uri, Some(&admin), None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(me(&viewer).await, StatusCode::OK);
}

#[tokio::test]
async fn me_says_whether_the_user_sees_every_music_library() {
    let (pool, app) = setup().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m'),
             (2, 'music', 'music', '/a'), (3, 'more', 'music', '/b');",
    )
    .execute(&pool)
    .await
    .unwrap();
    let admin = login_as(&app, "admin", "admin-password").await;
    let viewer = login_as(&app, "viewer", "viewer-password").await;
    let sees = |token: String| {
        let app = app.clone();
        async move {
            send(&app, "GET", "/api/v1/me", Some(&token), None).await.1["sees_all_music"].clone()
        }
    };
    assert_eq!(sees(admin).await, json!(true));
    assert_eq!(
        sees(viewer.clone()).await,
        json!(true),
        "nobody limited them"
    );
    // Limited to libraries, they see every music library only when the list has each one.
    for (libraries, all) in [
        (vec![1, 2, 3], true),
        (vec![2, 3], true),
        (vec![1, 2], false),
    ] {
        let limit = crate::access::Restriction {
            libraries: Some(libraries.clone()),
            max_age: None,
        };
        crate::access::set_restriction(&pool, 2, Some(&limit))
            .await
            .unwrap();
        assert_eq!(sees(viewer.clone()).await, json!(all), "{libraries:?}");
    }
    // A limit on ratings alone leaves every library.
    let limit = crate::access::Restriction {
        libraries: None,
        max_age: Some(12),
    };
    crate::access::set_restriction(&pool, 2, Some(&limit))
        .await
        .unwrap();
    assert_eq!(sees(viewer).await, json!(true));
}

mod during_scan;
