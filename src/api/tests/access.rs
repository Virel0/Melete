//! Who may call each route, checked against the router itself: every route the API's source
//! declares, with every method, is asked without a token, and every route the code puts behind
//! `require_admin` is asked by a signed-in user who isn't an admin. `openapi.json` says each
//! operation's access in `x-melete-access`, and that is held to what the code does.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use axum::http::Method;

use super::openapi::full_server;
use super::*;

/// What a request is sent with: every method a route might answer. HEAD is answered by GET.
const METHODS: [Method; 6] = [
    Method::GET,
    Method::HEAD,
    Method::POST,
    Method::PUT,
    Method::PATCH,
    Method::DELETE,
];

/// A route anyone may call without a token, and why it's open.
pub(super) struct Public {
    method: &'static str,
    /// The route's path as the code writes it; one ending in `/*` covers everything under it.
    path: &'static str,
    /// Its URL carries a token of its own, so a made-up one must be refused.
    own_token: bool,
    /// Why it's open; needed for every route on the list.
    why: &'static str,
}

const fn open(method: &'static str, path: &'static str, why: &'static str) -> Public {
    Public {
        method,
        path,
        own_token: false,
        why,
    }
}

const fn by_link(path: &'static str, why: &'static str) -> Public {
    Public {
        method: "GET",
        path,
        own_token: true,
        why,
    }
}

/// Every route open without a token. Nothing goes here without its reason.
const PUBLIC: &[Public] = &[
    open(
        "GET",
        "/api/v1/health",
        "a liveness check for the container; says only that it's up and its version",
    ),
    open(
        "GET",
        "/api/v1/openapi.json",
        "the API's description, the same for everyone",
    ),
    open("POST", "/api/v1/auth/login", "signing in"),
    open("POST", "/api/v1/auth/web-login", "the web UI signing in"),
    open(
        "POST",
        "/api/v1/auth/quick-connect",
        "a device that isn't signed in asks for a code to show",
    ),
    open(
        "POST",
        "/api/v1/auth/quick-connect/poll",
        "that device waits for a signed-in user to approve its code",
    ),
    open(
        "GET",
        "/api/v1/auth/oidc",
        "whether signing in through a provider is on, for the sign-in screen",
    ),
    open(
        "POST",
        "/api/v1/auth/oidc/start",
        "signing in through a provider begins before anyone is signed in",
    ),
    open(
        "GET",
        "/api/v1/auth/oidc/callback",
        "the provider sends the browser back here",
    ),
    open(
        "POST",
        "/api/v1/auth/oidc/confirm",
        "the browser confirms the sign-in the provider vouched for",
    ),
    open(
        "POST",
        "/api/v1/auth/oidc/redeem",
        "an app trades the code and its own secret for a session",
    ),
    by_link(
        "/api/v1/stream/{token}",
        "players that can't send headers; the link is one file's, for a day",
    ),
    by_link(
        "/api/v1/stream/{token}/subtitles/{track}",
        "subtitles for that stream, under its link",
    ),
    by_link(
        "/api/v1/stream/{token}/trickplay/{sheet}",
        "seek-bar previews for that stream, under its link",
    ),
    by_link(
        "/api/v1/offline/download/{token}",
        "an offline copy's download, under its own link, so a download can resume",
    ),
    by_link(
        "/api/v1/offline/download/{token}/{track}",
        "that copy's subtitles, under its link",
    ),
    by_link(
        "/api/v1/hls/{token}/{name}",
        "a transcode's playlist and segments, under its own link",
    ),
    by_link(
        "/api/v1/music/stream/{token}",
        "a track's stream link, as the video one",
    ),
    open("GET", "/", "the web UI's page, which signs in by itself"),
    open(
        "GET",
        "/web/*",
        "the web UI's scripts, styles and icon, the same for everyone",
    ),
];

pub(super) fn public(method: &Method, path: &str) -> Option<&'static Public> {
    let method = if method == Method::HEAD {
        "GET"
    } else {
        method.as_str()
    };
    PUBLIC.iter().find(|p| {
        let same_path = match p.path.strip_suffix('*') {
            Some(prefix) => path.starts_with(prefix),
            None => p.path == path,
        };
        p.method == method && same_path
    })
}

/// A route as "METHOD /path/{}/…", whatever its parameters are named.
pub(super) fn key(method: &str, path: &str) -> String {
    let path: Vec<String> = path
        .split('/')
        .map(|part| match part.starts_with('{') {
            true => "{}".to_owned(),
            false => part.to_owned(),
        })
        .collect();
    format!("{method} {}", path.join("/"))
}

/// Every literal path the API's source hands to `.route(`, outside test modules. Read from
/// the source because axum can't list a router's routes.
fn routes_in_source() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut dirs = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src/api")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if !path.ends_with("tests") {
                    dirs.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "rs")
                && path.file_name().is_some_and(|n| n != "tests.rs")
            {
                let text = std::fs::read_to_string(&path).unwrap();
                found.extend(route_literals(&text));
            }
        }
    }
    found
}

fn route_literals(text: &str) -> Vec<String> {
    let code = text.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
    code.split(".route(")
        .skip(1)
        .filter_map(|after| {
            let literal = after.trim_start().strip_prefix('"')?;
            Some(literal[..literal.find('"')?].to_owned())
        })
        .collect()
}

/// A URI for `path`, with a value in each parameter.
fn concrete(path: &str) -> String {
    path.split('/')
        .map(|part| match part {
            "{size}" => "w342",
            "{file}" => "x.jpg",
            _ if part.starts_with('{') => "1",
            _ => part,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) async fn status(
    app: &Router,
    method: &Method,
    path: &str,
    token: Option<&str>,
) -> StatusCode {
    let mut request = Request::builder()
        .method(method.clone())
        .uri(concrete(path));
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = request.body(Body::empty()).unwrap();
    app.clone().oneshot(request).await.unwrap().status()
}

pub(super) fn spec() -> Value {
    serde_json::from_str(OPENAPI).expect("openapi.json is valid JSON")
}

/// Every route in the source and in `openapi.json`, which must agree on the paths.
fn every_route() -> BTreeSet<String> {
    let mut routes = routes_in_source();
    let in_source: BTreeSet<String> = routes.iter().map(|p| key("", p)).collect();
    for path in spec()["paths"].as_object().unwrap().keys() {
        assert!(
            in_source.contains(&key("", path)),
            "{path} is documented but no `.route(\"{path}\"` was found in src/api"
        );
        routes.insert(path.clone());
    }
    assert!(routes.len() > 150, "found only {} routes", routes.len());
    routes
}

/// Asks every route with every method without a token, and answers what each said along with
/// the routes `require_admin` guarded meanwhile.
pub(super) async fn sweep(
    app: &Router,
) -> (BTreeMap<(Method, String), StatusCode>, BTreeSet<String>) {
    auth::ADMIN_GUARDED.with(|guarded| guarded.borrow_mut().clear());
    let mut answers = BTreeMap::new();
    for path in every_route() {
        for method in &METHODS {
            let answer = status(app, method, &path, None).await;
            answers.insert((method.clone(), path.clone()), answer);
        }
    }
    let guarded = auth::ADMIN_GUARDED.with(|guarded| guarded.borrow().clone());
    let guarded = guarded
        .iter()
        .map(|route| {
            let (method, path) = route.split_once(' ').unwrap();
            key(method, path)
        })
        .collect();
    (answers, guarded)
}

#[tokio::test]
async fn every_route_refuses_a_caller_without_a_token() {
    let (_, app, _artwork) = full_server().await;
    let (answers, _) = sweep(&app).await;
    let mut wrong = Vec::new();
    let mut answered_openly = BTreeSet::new();
    for ((method, path), answer) in &answers {
        let shut = [StatusCode::UNAUTHORIZED, StatusCode::METHOD_NOT_ALLOWED];
        match public(method, path) {
            // Not public: 401, or 405 for a method the route doesn't answer.
            None if !shut.contains(answer) => {
                wrong.push(format!("{method} {path} answered {answer} without a token"));
            }
            None => {}
            Some(open) if open.own_token && !answer.is_client_error() => {
                wrong.push(format!(
                    "{method} {path} answered {answer} to a made-up link"
                ));
            }
            Some(open) if shut.contains(answer) => {
                wrong.push(format!(
                    "{method} {path} is listed as public ({}) but answered {answer}",
                    open.why
                ));
                answered_openly.insert((open.method, open.path));
            }
            Some(open) => {
                answered_openly.insert((open.method, open.path));
            }
        }
    }
    for open in PUBLIC {
        if !answered_openly.contains(&(open.method, open.path)) {
            wrong.push(format!(
                "{} {} is listed as public but no route answers it",
                open.method, open.path
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[tokio::test]
async fn every_admin_route_refuses_a_signed_in_user_who_isnt_an_admin() {
    let (pool, app, _artwork) = full_server().await;
    let (answers, guarded) = sweep(&app).await;
    let user = crate::auth::create_user(&pool, "viewer", "user-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    let mut tokens = Vec::new();
    for kind in [
        crate::auth::TokenKind::Session,
        crate::auth::TokenKind::ApiKey,
    ] {
        let (_, token) = crate::auth::issue_token(&pool, user.id, kind, "t")
            .await
            .unwrap();
        tokens.push(token);
    }
    let mut asked = 0;
    let mut wrong = Vec::new();
    for (method, path) in answers.keys() {
        if !guarded.contains(&key(method.as_str(), path)) {
            continue;
        }
        for token in &tokens {
            let answer = status(&app, method, path, Some(token)).await;
            if answer != StatusCode::FORBIDDEN {
                wrong.push(format!(
                    "{method} {path} answered {answer} to a user who isn't an admin"
                ));
            }
            asked += 1;
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(asked > 200, "only {asked} admin requests were made");
}

/// `x-melete-access` on every documented operation says what the code does: `public` for a
/// route on the list above (and `security: []`), `admin` where `require_admin` guards it, and
/// `user` otherwise.
#[tokio::test]
async fn every_operation_documents_who_may_call_it() {
    let (_, app, _artwork) = full_server().await;
    let (_, guarded) = sweep(&app).await;
    let mut wrong = Vec::new();
    for (path, operations) in spec()["paths"].as_object().unwrap() {
        for (method, operation) in operations.as_object().unwrap() {
            let method = Method::from_bytes(method.to_uppercase().as_bytes()).unwrap();
            let open = public(&method, path).is_some();
            let code = if open {
                "public"
            } else if guarded.contains(&key(method.as_str(), path)) {
                "admin"
            } else {
                "user"
            };
            let documented = operation["x-melete-access"].as_str().unwrap_or("missing");
            if documented != code {
                wrong.push(format!(
                    "{method} {path}: documented {documented}, the code says {code}"
                ));
            }
            let unsecured = operation["security"].as_array().is_some_and(Vec::is_empty);
            if unsecured != open {
                wrong.push(format!(
                    "{method} {path}: `security: []` should be there only for a public route"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
