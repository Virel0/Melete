use axum::body::Body;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::db;
use crate::two_factor::totp::{self, tests::from_base32};

const LOGIN: &str = "/api/v1/auth/login";
const WEB_LOGIN: &str = "/api/v1/auth/web-login";
const FINISH: &str = "/api/v1/auth/login/second-factor";
const WEB_FINISH: &str = "/api/v1/auth/web-login/second-factor";
const STATUS: &str = "/api/v1/me/two-factor";
const SETUP: &str = "/api/v1/me/two-factor/setup";
const CONFIRM: &str = "/api/v1/me/two-factor/confirm";
const OFF: &str = "/api/v1/me/two-factor/off";

struct Answer {
    status: StatusCode,
    body: Value,
    headers: HeaderMap,
}

async fn send(app: &Router, method: &str, uri: &str, token: Option<&str>, body: Value) -> Answer {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = if body.is_null() {
        request.body(Body::empty())
    } else {
        request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
    }
    .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Answer {
        status,
        body,
        headers,
    }
}

async fn post(app: &Router, uri: &str, token: Option<&str>, body: Value) -> Answer {
    send(app, "POST", uri, token, body).await
}

async fn get(app: &Router, uri: &str, token: &str) -> Answer {
    send(app, "GET", uri, Some(token), Value::Null).await
}

/// Passwords made when the tests run, rather than written in them.
static ALICE: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| auth::generate_password().unwrap());
static BOB: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| auth::generate_password().unwrap());

fn alice() -> &'static str {
    &ALICE
}

fn bob() -> &'static str {
    &BOB
}

/// A password that isn't anyone's.
fn wrong() -> String {
    auth::generate_password().unwrap()
}

async fn setup() -> (SqlitePool, Router) {
    let pool = db::open_in_memory().await;
    for (name, password) in [("alice", alice()), ("bob", bob())] {
        auth::create_user(&pool, name, password.into(), false)
            .await
            .unwrap();
    }
    let app = crate::api::app(pool.clone());
    (pool, app)
}

fn login(password: &str) -> Value {
    json!({ "username": "alice", "password": password, "device_name": "Living room iPad" })
}

async fn token(app: &Router) -> String {
    let answer = post(app, LOGIN, None, login(alice())).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    answer.body["token"].as_str().unwrap().to_owned()
}

/// The code an authenticator app shows now.
fn code(secret: &[u8]) -> String {
    totp::code_at(secret, totp::step_at(unix_now()))
}

/// Forgets which step was last taken, so a test can use the code of the step it's in again.
async fn forget_last_code(pool: &SqlitePool) {
    sqlx::query("UPDATE two_factor SET last_step = NULL")
        .execute(pool)
        .await
        .unwrap();
}

/// Turns alice's second factor on through the API, and answers her secret.
async fn turn_on(pool: &SqlitePool, app: &Router, token: &str) -> Vec<u8> {
    let password = json!({ "password": alice() });
    let answer = post(app, SETUP, Some(token), password).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let secret = from_base32(answer.body["secret"].as_str().unwrap());
    let confirm = json!({ "code": code(&secret) });
    let answer = post(app, CONFIRM, Some(token), confirm).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
    forget_last_code(pool).await;
    secret
}

/// A sign-in's challenge, after a right password at `route`.
async fn challenge(app: &Router, route: &str) -> String {
    let answer = post(app, route, None, login(alice())).await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{}", answer.body);
    assert_eq!(answer.body["second_factor"], "totp");
    assert_eq!(answer.body["expires_in"], 300);
    let body = &answer.body;
    assert!(body.get("token").is_none(), "no session before the code");
    assert!(body.get("user").is_none());
    assert!(!answer.headers.contains_key(header::SET_COOKIE));
    assert_documented("SecondFactorNeeded", body);
    body["challenge"].as_str().unwrap().to_owned()
}

fn finishing(challenge: &str, code: &str) -> Value {
    json!({ "challenge": challenge, "code": code })
}

/// The answer's fields are exactly those `openapi.json` gives `schema`.
fn assert_documented(schema: &str, value: &Value) {
    let spec: Value = serde_json::from_str(crate::api::OPENAPI).unwrap();
    let mut documented: Vec<&String> = spec["components"]["schemas"][schema]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    let mut actual: Vec<&String> = value.as_object().unwrap().keys().collect();
    actual.sort();
    documented.sort();
    assert_eq!(actual, documented, "{schema} differs from openapi.json");
}

#[tokio::test]
async fn a_right_password_gives_a_challenge_and_the_code_a_session() {
    let (pool, app) = setup().await;
    let before = token(&app).await;
    let secret = turn_on(&pool, &app, &before).await;

    // A wrong password is answered as ever, with nothing about a second factor.
    let wrong = post(&app, LOGIN, None, login(&wrong())).await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.body, json!({ "error": "wrong username or password" }));

    let challenge = challenge(&app, LOGIN).await;
    let answer = post(&app, FINISH, None, finishing(&challenge, "000000")).await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert_eq!(answer.body["error"], "wrong code");
    assert!(answer.body.get("token").is_none());

    let right = code(&secret);
    let answer = post(&app, FINISH, None, finishing(&challenge, &right)).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    assert_eq!(answer.body["user"]["username"], "alice");
    let session = answer.body["token"].as_str().unwrap();
    let tokens = get(&app, "/api/v1/auth/tokens", session).await;
    let tokens = tokens.body.as_array().unwrap().clone();
    let current = tokens.iter().find(|t| t["current"] == true).unwrap();
    assert_eq!(current["name"], "Living room iPad", "named as asked");

    // The challenge is used up, and the code was taken: neither signs in again.
    let again = post(&app, FINISH, None, finishing(&challenge, &right)).await;
    assert_eq!(again.status, StatusCode::GONE);
    let fresh = self::challenge(&app, LOGIN).await;
    let reused = post(&app, FINISH, None, finishing(&fresh, &right)).await;
    assert_eq!(
        reused.status,
        StatusCode::UNAUTHORIZED,
        "a code is taken once"
    );

    // Turning it on signed nothing out.
    assert_eq!(
        get(&app, "/api/v1/me", &before).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn the_web_finishes_to_a_cookie_and_only_its_own_challenges() {
    let (pool, app) = setup().await;
    let before = token(&app).await;
    let secret = turn_on(&pool, &app, &before).await;
    let web = challenge(&app, WEB_LOGIN).await;
    let apps = challenge(&app, LOGIN).await;

    let crossed = post(&app, FINISH, None, finishing(&web, &code(&secret))).await;
    assert_eq!(
        crossed.status,
        StatusCode::GONE,
        "a web sign-in isn't an app's"
    );
    let crossed = post(&app, WEB_FINISH, None, finishing(&apps, &code(&secret))).await;
    assert_eq!(crossed.status, StatusCode::GONE, "nor the other way round");

    let answer = post(&app, WEB_FINISH, None, finishing(&web, &code(&secret))).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    assert_eq!(
        answer.body,
        json!({ "user": { "id": 1, "username": "alice", "is_admin": false } })
    );
    let cookie = answer.headers[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with("melete_session=mlt_") && cookie.contains("HttpOnly"));
}

#[tokio::test]
async fn a_challenge_runs_out_after_five_minutes() {
    let (pool, app) = setup().await;
    let before = token(&app).await;
    let secret = turn_on(&pool, &app, &before).await;
    let challenge = challenge(&app, LOGIN).await;
    sqlx::query("UPDATE sign_in_challenges SET expires_at = ?")
        .bind(unix_now())
        .execute(&pool)
        .await
        .unwrap();
    let answer = post(&app, FINISH, None, finishing(&challenge, &code(&secret))).await;
    assert_eq!(answer.status, StatusCode::GONE);
    assert!(answer.body.get("token").is_none());
    for made_up in ["", "m2f_00", "mlt_00"] {
        let answer = post(&app, FINISH, None, finishing(made_up, "123456")).await;
        assert_eq!(answer.status, StatusCode::GONE, "{made_up}");
    }
}

#[tokio::test]
async fn wrong_codes_are_limited_like_wrong_passwords() {
    let (pool, app) = setup().await;
    let before = token(&app).await;
    let secret = turn_on(&pool, &app, &before).await;
    let challenge = challenge(&app, LOGIN).await;
    for _ in 0..5 {
        let answer = post(&app, FINISH, None, finishing(&challenge, "000000")).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    }
    let answer = post(&app, FINISH, None, finishing(&challenge, &code(&secret))).await;
    assert_eq!(
        answer.status,
        StatusCode::TOO_MANY_REQUESTS,
        "even the right code"
    );
    // Codes count per user, wherever they're typed: turning it off is locked too.
    let off = json!({ "password": alice(), "code": code(&secret) });
    let answer = post(&app, OFF, Some(&before), off).await;
    assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
    // Someone else's sign-in is untouched.
    let bob = json!({ "username": "bob", "password": bob(), "device_name": "tv" });
    assert_eq!(post(&app, LOGIN, None, bob).await.status, StatusCode::OK);
}

#[tokio::test]
async fn turning_it_on_needs_the_password_and_a_first_code_and_signs_nothing_out() {
    let (_, app) = setup().await;
    let session = token(&app).await;
    let key = post(
        &app,
        "/api/v1/auth/keys",
        Some(&session),
        json!({ "name": "script" }),
    )
    .await;
    let key = key.body["token"].as_str().unwrap().to_owned();
    let status = get(&app, STATUS, &session).await;
    assert_eq!(
        status.body,
        json!({ "on": false, "on_since": null, "changes": [] })
    );
    assert_documented("TwoFactorStatus", &status.body);

    let wrong = json!({ "password": wrong() });
    assert_eq!(
        post(&app, SETUP, Some(&session), wrong).await.status,
        StatusCode::FORBIDDEN
    );
    let password = json!({ "password": alice() });
    let answer = post(&app, SETUP, Some(&session), password.clone()).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.headers[header::CACHE_CONTROL], "no-store");
    assert_documented("TwoFactorSetup", &answer.body);
    let encoded = answer.body["secret"].as_str().unwrap().to_owned();
    let uri = answer.body["uri"].as_str().unwrap();
    assert!(uri.starts_with("otpauth://totp/Melete:alice?secret="));
    let secret = from_base32(&encoded);
    let status = get(&app, STATUS, &session).await;
    assert_eq!(
        status.body["on"], false,
        "not until a first code confirms it"
    );
    assert!(
        !status.body.to_string().contains(&encoded),
        "never shown again"
    );
    let plain = post(&app, LOGIN, None, login(alice())).await;
    assert_eq!(
        plain.status,
        StatusCode::OK,
        "sign-in is unchanged meanwhile"
    );

    let confirm = |code: &str| json!({ "code": code });
    let answer = post(&app, CONFIRM, Some(&session), confirm("000000")).await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN);
    let answer = post(&app, CONFIRM, Some(&session), confirm(&code(&secret))).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    let status = get(&app, STATUS, &session).await;
    assert_eq!(status.body["on"], true);
    assert_eq!(status.body["changes"][0]["change"], "two_factor_on");
    assert!(!status.body.to_string().contains(&encoded));
    let again = post(&app, SETUP, Some(&session), password).await;
    assert_eq!(
        again.status,
        StatusCode::CONFLICT,
        "no second secret while it's on"
    );
    assert!(again.body.get("secret").is_none());
    assert_eq!(
        get(&app, "/api/v1/me", &key).await.status,
        StatusCode::OK,
        "keys work"
    );
    assert_eq!(
        get(&app, "/api/v1/me", &session).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn turning_it_off_needs_the_password_and_a_code_and_is_recorded() {
    let (pool, app) = setup().await;
    let session = token(&app).await;
    let secret = turn_on(&pool, &app, &session).await;
    let off = |password: &str, code: &str| json!({ "password": password, "code": code });
    let answer = post(&app, OFF, Some(&session), off(alice(), "000000")).await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN);
    let answer = post(&app, OFF, Some(&session), off(&wrong(), &code(&secret))).await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN);
    let answer = post(&app, OFF, Some(&session), off(alice(), &code(&secret))).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
    let answer = post(&app, OFF, Some(&session), off(alice(), &code(&secret))).await;
    assert_eq!(answer.status, StatusCode::CONFLICT);

    let status = get(&app, STATUS, &session).await;
    assert_eq!(status.body["on"], false);
    let changes: Vec<(&str, &str)> = status.body["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["change"].as_str().unwrap(),
                c["made_from"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        changes,
        [("two_factor_off", "account"), ("two_factor_on", "account")]
    );
    token(&app).await;
    let me = get(&app, "/api/v1/me", &session).await;
    assert_eq!(
        me.status,
        StatusCode::OK,
        "turning it off signed nothing out"
    );
}

#[tokio::test]
async fn every_new_route_refuses_a_caller_without_a_token_or_challenge() {
    let (pool, app) = setup().await;
    let session = token(&app).await;
    turn_on(&pool, &app, &session).await;
    let password = json!({ "password": alice() });
    let code = json!({ "code": "123456" });
    let both = json!({ "password": alice(), "code": "123456" });
    let answer = send(&app, "GET", STATUS, None, Value::Null).await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    for (route, body) in [(SETUP, password), (CONFIRM, code), (OFF, both.clone())] {
        let answer = post(&app, route, None, body).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{route}");
    }
    for route in [FINISH, WEB_FINISH] {
        let answer = post(&app, route, None, finishing("", "123456")).await;
        assert_eq!(answer.status, StatusCode::GONE, "{route}");
        assert!(!answer.headers.contains_key(header::SET_COOKIE));
        let answer = post(&app, route, None, Value::Null).await;
        assert!(
            answer.status.is_client_error(),
            "{route}: {}",
            answer.status
        );
    }
    // A read-only API key may see whether it's on, and change nothing.
    let key = json!({ "name": "reader", "scope": "read" });
    let key = post(&app, "/api/v1/auth/keys", Some(&session), key).await;
    let key = key.body["token"].as_str().unwrap();
    assert_eq!(get(&app, STATUS, key).await.status, StatusCode::OK);
    let answer = post(&app, OFF, Some(key), both).await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN);
}

/// How a password changes: by its owner, by an admin, or on the server's command line.
#[derive(Debug, Clone, Copy)]
enum NewPassword {
    Owner,
    Admin,
    CommandLine,
}

#[tokio::test]
async fn a_new_password_ends_sign_ins_waiting_for_their_code() {
    for how in [
        NewPassword::Owner,
        NewPassword::Admin,
        NewPassword::CommandLine,
    ] {
        let (pool, app) = setup().await;
        let session = token(&app).await;
        let secret = turn_on(&pool, &app, &session).await;
        // Someone with the old password gets as far as the code.
        let waiting = challenge(&app, LOGIN).await;
        match how {
            NewPassword::Owner => {
                let change = json!({ "current_password": alice(), "new_password": wrong() });
                let answer = send(&app, "PUT", "/api/v1/me/password", Some(&session), change).await;
                assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
            }
            NewPassword::Admin => {
                let admin = auth::create_user(&pool, "carol", wrong(), true)
                    .await
                    .unwrap()
                    .unwrap();
                let (_, admin) = auth::issue_token(&pool, admin.id, TokenKind::Session, "t")
                    .await
                    .unwrap();
                let answer =
                    post(&app, "/api/v1/users/1/password", Some(&admin), Value::Null).await;
                assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
            }
            NewPassword::CommandLine => {
                let keep = crate::cli::SecondFactor::Keep;
                crate::cli::reset_password(&pool, "alice", keep, || unreachable!())
                    .await
                    .unwrap();
            }
        }
        let answer = post(&app, FINISH, None, finishing(&waiting, &code(&secret))).await;
        assert_eq!(answer.status, StatusCode::GONE, "{how:?}: {}", answer.body);
        assert!(answer.body.get("token").is_none());
    }
}

#[tokio::test]
async fn confirming_with_nothing_to_confirm_doesnt_count_as_a_wrong_code() {
    let (pool, app) = setup().await;
    let session = token(&app).await;
    for _ in 0..6 {
        let answer = post(&app, CONFIRM, Some(&session), json!({ "code": "000000" })).await;
        assert_eq!(answer.status, StatusCode::CONFLICT, "nothing waiting");
    }
    let secret = turn_on(&pool, &app, &session).await;
    for _ in 0..6 {
        let answer = post(&app, CONFIRM, Some(&session), json!({ "code": "000000" })).await;
        assert_eq!(answer.status, StatusCode::CONFLICT, "on already");
    }
    // The code step at sign-in isn't locked by any of that.
    let waiting = challenge(&app, LOGIN).await;
    let answer = post(&app, FINISH, None, finishing(&waiting, &code(&secret))).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
}

#[tokio::test]
async fn refusals_say_whether_the_password_or_the_code_was_wrong() {
    let (pool, app) = setup().await;
    let session = token(&app).await;
    let reason = |answer: &Answer| answer.body["reason"].as_str().unwrap_or("none").to_owned();
    let refused = |status, why: &str| (status, why.to_owned());
    let answer = post(&app, SETUP, Some(&session), json!({ "password": wrong() })).await;
    let wrong_password = refused(StatusCode::FORBIDDEN, "wrong_password");
    assert_eq!((answer.status, reason(&answer)), wrong_password);
    assert_documented("Refusal", &answer.body);
    let answer = post(&app, SETUP, Some(&session), json!({ "password": alice() })).await;
    let secret = from_base32(answer.body["secret"].as_str().unwrap());
    let answer = post(&app, CONFIRM, Some(&session), json!({ "code": "000000" })).await;
    let wrong_code = refused(StatusCode::FORBIDDEN, "wrong_code");
    assert_eq!((answer.status, reason(&answer)), wrong_code);
    let answer = post(
        &app,
        CONFIRM,
        Some(&session),
        json!({ "code": code(&secret) }),
    )
    .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    forget_last_code(&pool).await;
    let off = |password: &str, code: &str| json!({ "password": password, "code": code });
    let answer = post(&app, OFF, Some(&session), off(&wrong(), &code(&secret))).await;
    assert_eq!((answer.status, reason(&answer)), wrong_password);
    let answer = post(&app, OFF, Some(&session), off(alice(), "000000")).await;
    assert_eq!((answer.status, reason(&answer)), wrong_code);
    let waiting = challenge(&app, LOGIN).await;
    let answer = post(&app, FINISH, None, finishing(&waiting, "000000")).await;
    let at_sign_in = refused(StatusCode::UNAUTHORIZED, "wrong_code");
    assert_eq!((answer.status, reason(&answer)), at_sign_in);
}

#[tokio::test]
async fn wrong_codes_at_sign_in_show_in_the_users_record_without_where_from() {
    let (pool, app) = setup().await;
    let session = token(&app).await;
    turn_on(&pool, &app, &session).await;
    let waiting = challenge(&app, LOGIN).await;
    for _ in 0..2 {
        let answer = post(&app, FINISH, None, finishing(&waiting, "000000")).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    }
    let status = get(&app, STATUS, &session).await;
    let newest = &status.body["changes"][0];
    assert_eq!(newest["change"], "wrong_sign_in_code");
    assert_eq!(newest["made_from"], "sign_in");
    assert_eq!(newest["count"], 2);
    assert_eq!(status.body["changes"][1]["count"], 1, "turning it on");
    let keys: Vec<&String> = newest.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["at", "change", "count", "made_from"], "no address");
}

#[tokio::test]
async fn setup_answers_the_link_drawn_as_a_qr_code_and_nowhere_else() {
    let (_, app) = setup().await;
    let session = token(&app).await;
    let answer = post(&app, SETUP, Some(&session), json!({ "password": alice() })).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.headers[header::CACHE_CONTROL], "no-store");
    assert_documented("TwoFactorSetup", &answer.body);
    let uri = answer.body["uri"].as_str().unwrap();
    let qr = answer.body["qr"].as_str().expect("a QR code");
    let drawn = crate::two_factor::qr::encode(uri.as_bytes()).unwrap();
    assert_eq!(qr, drawn.svg(), "the link, drawn");
    assert!(qr.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
    // Shown with the setup only: the status never has it.
    let status = get(&app, STATUS, &session).await;
    assert!(status.body.get("qr").is_none());
    assert!(!status.body.to_string().contains("<svg"));
}
