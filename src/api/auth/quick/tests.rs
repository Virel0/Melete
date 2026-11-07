use axum::body::Body;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::super::*;
use crate::db;
use crate::logging::testing::capture;

/// The sign-in routes over a Quick Connect store the test holds, so it can run codes out, and
/// the viewer's password.
async fn setup() -> (SqlitePool, Router, Arc<QuickConnect>, String) {
    let pool = db::open_in_memory().await;
    auth::testing::user(&pool, "admin", true).await;
    let (_, viewer_password) = auth::testing::user_with_password(&pool, "viewer", false).await;
    let quick = Arc::new(QuickConnect::default());
    let state = AppState {
        pool: pool.clone(),
        limiter: Arc::new(LoginLimiter::default()),
    };
    (pool, routes(state, quick.clone()), quick, viewer_password)
}

/// A request with these headers and JSON body; answers status, headers and the body's text.
async fn send(
    app: &Router,
    uri: &str,
    headers: &[(&str, &str)],
    body: Value,
) -> (StatusCode, HeaderMap, String) {
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request.body(Body::from(body.to_string())).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

const WEB: [(&str, &str); 1] = [(WEB_HEADER, "1")];
const POLL: &str = "/api/v1/auth/quick-connect/web-poll";

async fn bearer(app: &Router, username: &str, password: &str) -> String {
    let body = json!({ "username": username, "password": password, "device_name": "test" });
    let (status, _, text) = send(app, "/api/v1/auth/login", &[], body).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let body: Value = serde_json::from_str(&text).unwrap();
    format!("Bearer {}", body["token"].as_str().unwrap())
}

/// A code started as the web UI starts one: (code, secret).
async fn start(app: &Router) -> (String, String) {
    let body = json!({ "device_name": "Firefox on Linux" });
    let (status, _, text) = send(app, "/api/v1/auth/quick-connect", &WEB, body).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let started: Value = serde_json::from_str(&text).unwrap();
    let field = |name: &str| started[name].as_str().unwrap().to_owned();
    (field("code"), field("secret"))
}

/// `user` (a bearer header) looks the code up and approves it.
async fn approve(app: &Router, user: &str, code: &str) {
    for step in ["lookup", "approve"] {
        let uri = format!("/api/v1/auth/quick-connect/{step}");
        let headers = [("authorization", user)];
        let (status, _, text) = send(app, &uri, &headers, json!({ "code": code })).await;
        assert_eq!(status, StatusCode::OK, "{step}: {text}");
    }
}

#[tokio::test]
async fn an_approved_code_signs_the_web_ui_in_to_the_password_sign_ins_cookie_once() {
    let (_, app, _, password) = setup().await;
    let viewer = bearer(&app, "viewer", &password).await;
    let (code, secret) = start(&app).await;
    let poll = json!({ "secret": secret });

    // Not approved yet: waiting, and no cookie.
    let (status, headers, text) = send(&app, POLL, &WEB, poll.clone()).await;
    assert_eq!(
        (status, text.as_str()),
        (StatusCode::ACCEPTED, r#"{"status":"waiting"}"#)
    );
    assert!(!headers.contains_key(header::SET_COOKIE));

    approve(&app, &viewer, &code).await;
    let (status, headers, text) = send(&app, POLL, &WEB, poll.clone()).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    // The user, and the session only in the cookie: never in the body.
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        json!({ "user": { "id": 2, "username": "viewer", "is_admin": false } })
    );
    let set = headers[header::SET_COOKIE].to_str().unwrap().to_owned();
    let token = set
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("melete_session=")
        .unwrap()
        .to_owned();
    assert!(token.starts_with("mlt_") && !text.contains(&token));
    // Exactly the cookie a password sign-in sets, attribute for attribute.
    let (_, signed_in, _) = send(
        &app,
        "/api/v1/auth/web-login",
        &[],
        json!({ "username": "viewer", "password": password, "device_name": "x" }),
    )
    .await;
    let by_password = signed_in[header::SET_COOKIE].to_str().unwrap();
    let attributes = |cookie: &str| cookie.split_once(';').unwrap().1.to_owned();
    assert_eq!(attributes(&set), attributes(by_password));
    assert_eq!(set, cookie_for(Some(&token), false));

    // The cookie signs the page in as the approver, named as the browser asked.
    let request = Request::get("/api/v1/auth/tokens")
        .header("cookie", format!("melete_session={token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let tokens: Value = serde_json::from_slice(&bytes).unwrap();
    let this = tokens
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["current"] == true)
        .unwrap();
    assert_eq!(this["name"], "Firefox on Linux");

    // Used once: asking again gets nothing, from either poll.
    let (status, headers, _) = send(&app, POLL, &WEB, poll.clone()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!headers.contains_key(header::SET_COOKIE));
    let app_poll = "/api/v1/auth/quick-connect/poll";
    let (status, _, _) = send(&app, app_poll, &[], poll).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Over https the cookie is Secure, as a password sign-in's is.
    let (code, secret) = start(&app).await;
    approve(&app, &viewer, &code).await;
    let https = [(WEB_HEADER, "1"), ("origin", "https://media.example")];
    let (_, headers, _) = send(&app, POLL, &https, json!({ "secret": secret })).await;
    assert!(
        headers[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .ends_with("; Secure")
    );
}

#[tokio::test]
async fn an_unapproved_or_run_out_code_gives_no_cookie() {
    let (_, app, quick, password) = setup().await;
    let viewer = bearer(&app, "viewer", &password).await;
    // Looked up but never approved: still waiting, whatever the lookup showed.
    let (code, secret) = start(&app).await;
    let headers = [("authorization", viewer.as_str())];
    let lookup = "/api/v1/auth/quick-connect/lookup";
    let (status, _, _) = send(&app, lookup, &headers, json!({ "code": code })).await;
    assert_eq!(status, StatusCode::OK);
    let (status, set, _) = send(&app, POLL, &WEB, json!({ "secret": secret })).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(!set.contains_key(header::SET_COOKIE));

    // Approved, but run out before it was collected: lost, as for an app.
    approve(&app, &viewer, &code).await;
    quick.run_out_all();
    let (status, set, _) = send(&app, POLL, &WEB, json!({ "secret": secret })).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!set.contains_key(header::SET_COOKIE));

    // A run-out code can't be approved either.
    let (code, secret) = start(&app).await;
    quick.run_out_all();
    let approve_uri = "/api/v1/auth/quick-connect/approve";
    for uri in [lookup, approve_uri] {
        let (status, _, _) = send(&app, uri, &headers, json!({ "code": code })).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
    let (status, _, _) = send(&app, POLL, &WEB, json!({ "secret": secret })).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn only_the_browser_that_asked_collects_its_code() {
    let (_, app, _, password) = setup().await;
    let viewer = bearer(&app, "viewer", &password).await;
    let (theirs, their_secret) = start(&app).await;
    let (_, my_secret) = start(&app).await;
    approve(&app, &viewer, &theirs).await;
    // Someone else's approved code: not mine to collect, by my secret or by the code it shows.
    let (status, set, _) = send(&app, POLL, &WEB, json!({ "secret": my_secret })).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(!set.contains_key(header::SET_COOKIE));
    for guess in [theirs.as_str(), "mqc_00", ""] {
        let (status, set, _) = send(&app, POLL, &WEB, json!({ "secret": guess })).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{guess}");
        assert!(!set.contains_key(header::SET_COOKIE));
    }
    // A cross-site page can't make the request: the web header is needed, as for changes.
    let (status, set, _) = send(&app, POLL, &[], json!({ "secret": their_secret })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!set.contains_key(header::SET_COOKIE));
    // Refused, it's still there for the browser that asked.
    let (status, _, _) = send(&app, POLL, &WEB, json!({ "secret": their_secret })).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn the_web_poll_keeps_the_apps_limits() {
    let (_, app, _, password) = setup().await;
    let viewer = bearer(&app, "viewer", &password).await;
    // As many codes wait at once from one address as for the apps.
    let start_uri = "/api/v1/auth/quick-connect";
    let mut busy = None;
    for _ in 0..10 {
        let (status, _, _) = send(&app, start_uri, &WEB, json!({ "device_name": "tv" })).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            busy = Some(status);
            break;
        }
    }
    assert_eq!(busy, Some(StatusCode::TOO_MANY_REQUESTS));
    // Ten lookups an hour, found or not, whichever kind of device the code is for.
    let headers = [("authorization", viewer.as_str())];
    let lookup = "/api/v1/auth/quick-connect/lookup";
    for _ in 0..quick_connect::LOOKUPS_PER_HOUR {
        send(&app, lookup, &headers, json!({ "code": "AAAAAA" })).await;
    }
    let (status, _, _) = send(&app, lookup, &headers, json!({ "code": "BBBBBB" })).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn no_secret_reaches_the_log_or_a_response_it_doesnt_belong_in() {
    let log = capture("trace");
    let (_, app, _, password) = setup().await;
    let viewer = bearer(&app, "viewer", &password).await;
    let (code, secret) = start(&app).await;
    let (_, _, waiting) = send(&app, POLL, &WEB, json!({ "secret": secret })).await;
    approve(&app, &viewer, &code).await;
    let (status, headers, body) = send(&app, POLL, &WEB, json!({ "secret": secret })).await;
    assert_eq!(status, StatusCode::OK);
    let set = headers[header::SET_COOKIE].to_str().unwrap();
    let token = set
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("melete_session=")
        .unwrap();
    for answer in [&waiting, &body] {
        assert!(
            !answer.contains(&secret) && !answer.contains(token),
            "{answer}"
        );
    }
    let (_, _, gone) = send(&app, POLL, &WEB, json!({ "secret": secret })).await;
    assert!(!gone.contains(&secret), "{gone}");
    let written = log.written.text();
    for kept in [secret.as_str(), token, code.as_str()] {
        assert!(!written.contains(kept), "{written}");
    }
}
