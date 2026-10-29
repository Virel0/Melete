use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use super::*;
use crate::oidc::fake;

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
    text: String,
}

impl Answer {
    fn location(&self) -> &str {
        self.headers[header::LOCATION].to_str().unwrap()
    }

    /// The value of a cookie this answer sets.
    fn cookie(&self, name: &str) -> Option<String> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|c| c.strip_prefix(&format!("{name}="))?.split(';').next())
            .map(str::to_owned)
    }
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> Answer {
    let mut request = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        request = request.header(*k, *v);
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
    Answer {
        status,
        headers,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// What the confirmation page's form sends, with the browser's cookie.
async fn confirm_form(app: &Router, cookie: Option<&str>, form: &str, action: &str) -> Answer {
    let mut request = Request::post("/api/v1/auth/oidc/confirm")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, format!("melete_oidc_confirm={cookie}"));
    }
    let request = request
        .body(Body::from(format!("form={form}&action={action}")))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Answer {
        status,
        headers,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// The form token in a confirmation page.
fn form_token(page: &str) -> String {
    page.split("name=\"form\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("{page}"))
        .to_owned()
}

/// An app's sign-in brought to its confirmation page: the page's answer.
async fn to_confirmation(app: &Router, fake: &fake::Fake, return_to: &str) -> (String, Answer) {
    let started = send(
        app,
        "POST",
        "/api/v1/auth/oidc/start",
        &[],
        Some(json!({ "device_name": "Living room <TV>", "return_to": return_to })),
    )
    .await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let secret = started.body["secret"].as_str().unwrap().to_owned();
    let url = started.body["authorize_url"].as_str().unwrap().to_owned();
    let page = arrive(app, fake, &url, None, "viewer-at-provider").await;
    (secret, page)
}

async fn setup() -> (SqlitePool, Router, fake::Fake, String) {
    let pool = crate::db::open_in_memory().await;
    for (name, admin) in [("admin", true), ("viewer", false)] {
        auth::create_user(&pool, name, format!("{name}-password"), admin)
            .await
            .unwrap();
    }
    let fake = fake::start().await;
    let client = Arc::new(Client::new(fake::settings(&fake.issuer)).unwrap());
    let limiter: Arc<LoginLimiter> = Arc::default();
    let app = super::super::app_with(pool.clone(), limiter.clone()).merge(routes(
        pool.clone(),
        Some(client),
        limiter,
    ));
    let login =
        json!({ "username": "viewer", "password": "viewer-password", "device_name": "phone" });
    let token = send(&app, "POST", "/api/v1/auth/login", &[], Some(login))
        .await
        .body["token"]
        .as_str()
        .unwrap()
        .to_owned();
    (pool, app, fake, token)
}

/// Plays the provider's part: the browser comes back from it with a code, carrying
/// `cookie`. The ID token it hands over is for this sign-in and says `subject`.
async fn come_back(
    app: &Router,
    fake: &fake::Fake,
    authorize_url: &str,
    cookie: Option<&str>,
    subject: &str,
) -> Answer {
    let answer = arrive(app, fake, authorize_url, cookie, subject).await;
    assert_eq!(answer.status, StatusCode::SEE_OTHER);
    answer
}

/// As `come_back`, whatever the answer.
async fn arrive(
    app: &Router,
    fake: &fake::Fake,
    authorize_url: &str,
    cookie: Option<&str>,
    subject: &str,
) -> Answer {
    let query = fake::query(authorize_url);
    *fake.nonce.lock().unwrap() = query["nonce"].clone();
    *fake.subject.lock().unwrap() = subject.to_owned();
    let uri = format!(
        "/api/v1/auth/oidc/callback?state={}&code=the-code",
        query["state"]
    );
    let cookie = cookie.map(|c| format!("melete_oidc={c}"));
    let headers: Vec<(&str, &str)> = cookie.iter().map(|c| ("cookie", c.as_str())).collect();
    send(app, "GET", &uri, &headers, None).await
}

async fn link(
    app: &Router,
    fake: &fake::Fake,
    token: &str,
    password: &str,
    subject: &str,
) -> String {
    let bearer = format!("Bearer {token}");
    let started = send(
        app,
        "POST",
        "/api/v1/auth/oidc/link",
        &[("authorization", &bearer)],
        Some(json!({ "password": password })),
    )
    .await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let bound = started.cookie("melete_oidc").unwrap();
    let url = started.body["authorize_url"].as_str().unwrap();
    come_back(app, fake, url, Some(&bound), subject)
        .await
        .location()
        .to_owned()
}

#[tokio::test]
async fn the_web_ui_signs_in_as_the_user_who_linked_the_identity() {
    let (_, app, fake, token) = setup().await;
    let bearer = format!("Bearer {token}");
    let status = send(
        &app,
        "GET",
        "/api/v1/auth/oidc/link",
        &[("authorization", &bearer)],
        None,
    )
    .await;
    assert_eq!(status.body["linked"], false);
    assert_eq!(
        link(&app, &fake, &token, "viewer-password", "viewer-at-provider").await,
        "/#/settings/account?oidc=linked"
    );
    let status = send(
        &app,
        "GET",
        "/api/v1/auth/oidc/link",
        &[("authorization", &bearer)],
        None,
    )
    .await;
    assert_eq!(status.body["linked"], true);

    let about = send(&app, "GET", "/api/v1/auth/oidc", &[], None).await;
    assert_eq!(about.body, json!({ "enabled": true, "name": "Example ID" }));
    let started = send(
        &app,
        "POST",
        "/api/v1/auth/oidc/start",
        &[],
        Some(json!({ "device_name": "Firefox on Linux" })),
    )
    .await;
    let bound = started.cookie("melete_oidc").unwrap();
    let set = started.headers[header::SET_COOKIE].to_str().unwrap();
    assert!(
        set.contains("SameSite=Lax") && set.contains("HttpOnly"),
        "{set}"
    );
    let url = started.body["authorize_url"].as_str().unwrap().to_owned();

    // Finished in another browser, without the cookie: refused.
    let elsewhere = come_back(&app, &fake, &url, None, "viewer-at-provider").await;
    assert_eq!(elsewhere.location(), "/#/?oidc=browser");
    assert!(elsewhere.cookie("melete_session").is_none());

    let started = send(
        &app,
        "POST",
        "/api/v1/auth/oidc/start",
        &[],
        Some(json!({ "device_name": "Firefox on Linux" })),
    )
    .await;
    let bound2 = started.cookie("melete_oidc").unwrap();
    assert_ne!(bound, bound2);
    let url = started.body["authorize_url"].as_str().unwrap().to_owned();
    let done = come_back(&app, &fake, &url, Some(&bound2), "viewer-at-provider").await;
    assert_eq!(done.location(), "/#/");
    let session = done.cookie("melete_session").unwrap();
    assert!(session.starts_with("mlt_"));
    assert_eq!(done.cookie("melete_oidc").as_deref(), Some(""), "cleared");
    let me = send(
        &app,
        "GET",
        "/api/v1/me",
        &[("cookie", &format!("melete_session={session}"))],
        None,
    )
    .await;
    assert_eq!(me.body["username"], "viewer");

    // The same sign-in can't be finished twice.
    let again = come_back(&app, &fake, &url, Some(&bound2), "viewer-at-provider").await;
    assert_eq!(again.location(), "/#/?oidc=expired");
}

#[tokio::test]
async fn only_a_linked_identity_signs_in_and_one_identity_is_one_user() {
    let (pool, app, fake, token) = setup().await;
    // Nobody linked it: the browser is told so, and gets no session.
    let started = send(
        &app,
        "POST",
        "/api/v1/auth/oidc/start",
        &[],
        Some(json!({ "device_name": "Firefox" })),
    )
    .await;
    let bound = started.cookie("melete_oidc").unwrap();
    let url = started.body["authorize_url"].as_str().unwrap().to_owned();
    let back = come_back(&app, &fake, &url, Some(&bound), "stranger").await;
    assert_eq!(back.location(), "/#/?oidc=unlinked");
    assert!(back.cookie("melete_session").is_none());

    link(&app, &fake, &token, "viewer-password", "shared").await;
    let admin_login =
        json!({ "username": "admin", "password": "admin-password", "device_name": "x" });
    let admin = send(&app, "POST", "/api/v1/auth/login", &[], Some(admin_login))
        .await
        .body["token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        link(&app, &fake, &admin, "admin-password", "shared").await,
        "/#/settings/account?oidc=taken"
    );
    // Linking again replaces the user's own link.
    link(&app, &fake, &token, "viewer-password", "viewer-new").await;
    let subjects: Vec<String> = sqlx::query_scalar("SELECT subject FROM oidc_identities")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(subjects, ["viewer-new"]);

    let bearer = format!("Bearer {token}");
    let gone = send(
        &app,
        "DELETE",
        "/api/v1/auth/oidc/link",
        &[("authorization", &bearer)],
        None,
    )
    .await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT);
    let gone = send(
        &app,
        "DELETE",
        "/api/v1/auth/oidc/link",
        &[("authorization", &bearer)],
        None,
    )
    .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_refusal_at_the_provider_signs_nobody_in() {
    let (_, app, fake, token) = setup().await;
    link(&app, &fake, &token, "viewer-password", "viewer-at-provider").await;
    let started = send(
        &app,
        "POST",
        "/api/v1/auth/oidc/start",
        &[],
        Some(json!({ "device_name": "tv" })),
    )
    .await;
    let state = fake::query(started.body["authorize_url"].as_str().unwrap())["state"].clone();
    let uri = format!("/api/v1/auth/oidc/callback?state={state}&error=access_denied");
    let back = send(&app, "GET", &uri, &[], None).await;
    assert_eq!(back.location(), "/#/?oidc=denied");
    assert!(back.cookie("melete_session").is_none());
    let back = send(&app, "GET", "/api/v1/auth/oidc/callback", &[], None).await;
    assert_eq!(back.location(), "/#/?oidc=expired");
}

#[tokio::test]
async fn off_until_set_up() {
    let pool = crate::db::open_in_memory().await;
    let app = routes(pool, None, Arc::default());
    let about = send(&app, "GET", "/api/v1/auth/oidc", &[], None).await;
    assert_eq!(about.body, json!({ "enabled": false, "name": null }));
    let started = send(
        &app,
        "POST",
        "/api/v1/auth/oidc/start",
        &[],
        Some(json!({ "device_name": "tv" })),
    )
    .await;
    assert_eq!(started.status, StatusCode::SERVICE_UNAVAILABLE);
    let back = send(
        &app,
        "GET",
        "/api/v1/auth/oidc/callback?state=x&code=y",
        &[],
        None,
    )
    .await;
    assert_eq!(back.location(), "/#/?oidc=off");
}

#[tokio::test]
async fn an_app_redeems_its_code_with_its_secret_once() {
    let (_, app, fake, token) = setup().await;
    link(&app, &fake, &token, "viewer-password", "viewer-at-provider").await;
    let (secret, page) = to_confirmation(&app, &fake, "com.example.app:/signed-in").await;
    // The browser is asked first: which device, as whom, going back where.
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.text
            .contains("Sign in Living room &lt;TV&gt; as viewer?"),
        "{}",
        page.text
    );
    assert!(page.text.contains("com.example.app:/signed-in"));
    assert!(!page.text.contains("code="), "no code before Continue");
    let policy = page.headers[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(policy.contains("frame-ancestors 'none'"), "{policy}");
    let cookie = page.cookie("melete_oidc_confirm").unwrap();
    let set = page.headers.get_all(header::SET_COOKIE);
    assert!(set.iter().any(|c| {
        let c = c.to_str().unwrap();
        c.starts_with("melete_oidc_confirm=")
            && c.contains("SameSite=Strict")
            && c.contains("HttpOnly")
    }));
    let form = form_token(&page.text);
    assert_eq!(
        confirm_form(&app, None, &form, "continue").await.status,
        StatusCode::BAD_REQUEST,
        "not from another browser"
    );
    assert_eq!(
        confirm_form(&app, Some(&cookie), "forged", "continue")
            .await
            .status,
        StatusCode::BAD_REQUEST,
        "not without the page's own token"
    );
    let back = confirm_form(&app, Some(&cookie), &form, "continue").await;
    assert_eq!(back.status, StatusCode::SEE_OTHER);
    assert_eq!(
        confirm_form(&app, Some(&cookie), &form, "continue")
            .await
            .status,
        StatusCode::BAD_REQUEST,
        "once"
    );
    let location = back.location();
    let code = location
        .strip_prefix("com.example.app:/signed-in?code=")
        .unwrap_or_else(|| panic!("{location}"))
        .to_owned();

    let redeem = |secret: &str| json!({ "secret": secret, "code": code });
    let wrong = send(
        &app,
        "POST",
        "/api/v1/auth/oidc/redeem",
        &[],
        Some(redeem("mos_nope")),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::NOT_FOUND);
    let right = send(
        &app,
        "POST",
        "/api/v1/auth/oidc/redeem",
        &[],
        Some(redeem(&secret)),
    )
    .await;
    assert_eq!(right.status, StatusCode::OK, "{}", right.body);
    assert_eq!(right.body["user"]["username"], "viewer");
    let tv = right.body["token"].as_str().unwrap();
    let tokens = send(
        &app,
        "GET",
        "/api/v1/auth/tokens",
        &[("authorization", &format!("Bearer {tv}"))],
        None,
    )
    .await;
    let current = tokens
        .body
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["current"] == true)
        .unwrap()
        .clone();
    assert_eq!(current["name"], "Living room <TV>");
    let again = send(
        &app,
        "POST",
        "/api/v1/auth/oidc/redeem",
        &[],
        Some(redeem(&secret)),
    )
    .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_app_is_sent_back_only_to_itself_and_told_what_went_wrong() {
    let (_, app, fake, token) = setup().await;
    for return_to in [
        "https://evil.example/",
        "javascript:alert(1)",
        "x-safari-https://attacker.example/cb",
        "googlechromes://attacker.example/cb",
        "com.example.app:/elsewhere",
    ] {
        let started = send(
            &app,
            "POST",
            "/api/v1/auth/oidc/start",
            &[],
            Some(json!({ "device_name": "tv", "return_to": return_to })),
        )
        .await;
        assert_eq!(started.status, StatusCode::BAD_REQUEST, "{return_to}");
    }
    let start = || {
        send(
            &app,
            "POST",
            "/api/v1/auth/oidc/start",
            &[],
            Some(json!({ "device_name": "tv", "return_to": "aoide://signed-in" })),
        )
    };
    // Nobody linked it: the app is told so, and gets nothing.
    let started = start().await;
    let url = started.body["authorize_url"].as_str().unwrap().to_owned();
    let back = come_back(&app, &fake, &url, None, "stranger").await;
    assert_eq!(back.location(), "aoide://signed-in?error=unlinked");
    // Refused at the provider.
    link(&app, &fake, &token, "viewer-password", "viewer-at-provider").await;
    let started = start().await;
    let state = fake::query(started.body["authorize_url"].as_str().unwrap())["state"].clone();
    let uri = format!("/api/v1/auth/oidc/callback?state={state}&error=access_denied");
    let back = send(&app, "GET", &uri, &[], None).await;
    assert_eq!(back.location(), "aoide://signed-in?error=denied");
    // Cancelled on the confirmation page: no code.
    let (_, page) = to_confirmation(&app, &fake, "aoide://signed-in").await;
    let cookie = page.cookie("melete_oidc_confirm").unwrap();
    let back = confirm_form(&app, Some(&cookie), &form_token(&page.text), "cancel").await;
    assert_eq!(back.location(), "aoide://signed-in?error=denied");
}

#[tokio::test]
async fn linking_needs_the_password_and_a_new_password_unlinks() {
    let (pool, app, fake, token) = setup().await;
    let bearer = format!("Bearer {token}");
    let auth = [("authorization", bearer.as_str())];
    let refused = send(&app, "POST", "/api/v1/auth/oidc/link", &auth, None).await;
    assert_eq!(
        refused.status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "no password, no link"
    );
    let wrong = json!({ "password": "not-it" });
    let refused = send(&app, "POST", "/api/v1/auth/oidc/link", &auth, Some(wrong)).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert!(refused.cookie("melete_oidc").is_none());
    let identities = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM oidc_identities")
            .fetch_one(&pool)
            .await
            .unwrap()
    };

    // A reset by an admin signs the user out and unlinks the provider too.
    link(&app, &fake, &token, "viewer-password", "viewer-at-provider").await;
    assert_eq!(identities().await, 1);
    let viewer_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'viewer'")
        .fetch_one(&pool)
        .await
        .unwrap();
    crate::auth::set_password(&pool, viewer_id, "fresh-password".into(), None)
        .await
        .unwrap();
    assert_eq!(identities().await, 0);

    // So does the user's own change of password.
    let login = json!({ "username": "viewer", "password": "fresh-password", "device_name": "x" });
    let token = send(&app, "POST", "/api/v1/auth/login", &[], Some(login))
        .await
        .body["token"]
        .as_str()
        .unwrap()
        .to_owned();
    link(&app, &fake, &token, "fresh-password", "viewer-at-provider").await;
    assert_eq!(identities().await, 1);
    let change = json!({ "current_password": "fresh-password", "new_password": "newer-password" });
    let bearer = format!("Bearer {token}");
    let changed = send(
        &app,
        "PUT",
        "/api/v1/me/password",
        &[("authorization", &bearer)],
        Some(change),
    )
    .await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT);
    assert_eq!(identities().await, 0);
}

#[tokio::test]
async fn wrong_passwords_at_linking_and_changing_add_up() {
    let (_, app, _fake, token) = setup().await;
    let bearer = format!("Bearer {token}");
    let auth = [("authorization", bearer.as_str())];
    for _ in 0..3 {
        let change = json!({ "current_password": "guess", "new_password": "new-password" });
        let refused = send(&app, "PUT", "/api/v1/me/password", &auth, Some(change)).await;
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
    }
    for _ in 0..2 {
        let guess = json!({ "password": "guess" });
        let refused = send(&app, "POST", "/api/v1/auth/oidc/link", &auth, Some(guess)).await;
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
    }
    // Five in all: even the right password is refused now, at either.
    let right = json!({ "password": "viewer-password" });
    let locked = send(&app, "POST", "/api/v1/auth/oidc/link", &auth, Some(right)).await;
    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS);
    let change = json!({ "current_password": "viewer-password", "new_password": "new-password" });
    let locked = send(&app, "PUT", "/api/v1/me/password", &auth, Some(change)).await;
    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS);
}
