//! What each API key's scope reaches, checked against the router itself: every route the code
//! guards notes what it needs of a token's scope, `openapi.json` says it in `x-melete-scope`,
//! and a key of every scope is refused wherever its scope doesn't reach.

use std::collections::BTreeMap;

use axum::http::Method;

use super::access::{key, public, spec, status, sweep};
use super::openapi::full_server;
use super::*;
use crate::auth::{Scope, TokenKind};

/// Every route and method a sweep of `app` asked, and what each route the code guards needs
/// of a token's scope.
async fn needed(app: &Router) -> (Vec<(Method, String)>, BTreeMap<String, Scope>) {
    auth::SCOPE_NEEDED.with(|seen| seen.borrow_mut().clear());
    let (answers, _) = sweep(app).await;
    let seen = auth::SCOPE_NEEDED.with(|seen| seen.borrow().clone());
    let needed = seen
        .into_iter()
        .map(|(route, scope)| {
            let (method, path) = route.split_once(' ').unwrap();
            (key(method, path), scope)
        })
        .collect();
    (answers.into_keys().collect(), needed)
}

/// An admin's API keys, one of each scope.
async fn keys(pool: &SqlitePool) -> Vec<(Scope, String)> {
    let admin = crate::auth::create_user(pool, "alice", "a-long-password".into(), true)
        .await
        .unwrap()
        .unwrap();
    let mut keys = Vec::new();
    for scope in [Scope::Read, Scope::Playback, Scope::MusicSync, Scope::Full] {
        let (_, token) = crate::auth::issue_scoped(pool, admin.id, TokenKind::ApiKey, "k", scope)
            .await
            .unwrap();
        keys.push((scope, token));
    }
    keys
}

#[tokio::test]
async fn every_operation_documents_the_scope_it_needs() {
    let (_, app, _artwork) = full_server().await;
    let (_, needed) = needed(&app).await;
    let mut wrong = Vec::new();
    for (path, operations) in spec()["paths"].as_object().unwrap() {
        for (method, operation) in operations.as_object().unwrap() {
            let method = Method::from_bytes(method.to_uppercase().as_bytes()).unwrap();
            let code = match public(&method, path) {
                Some(_) => None,
                None => Some(needed[&key(method.as_str(), path)].as_str()),
            };
            let documented = operation["x-melete-scope"].as_str();
            if documented != code {
                wrong.push(format!(
                    "{method} {path}: documented {documented:?}, the code says {code:?}"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Every route, with every method, refuses a key whose scope doesn't reach what it needs, even
/// an admin's: a key below full reaches no admin route.
#[tokio::test]
async fn a_key_is_refused_wherever_its_scope_does_not_reach() {
    let (pool, app, _artwork) = full_server().await;
    let (routes, needed) = needed(&app).await;
    let keys = keys(&pool).await;
    let (mut refused, mut wrong) = (0, Vec::new());
    for (method, path) in &routes {
        let Some(&needs) = needed.get(&key(method.as_str(), path)) else {
            continue;
        };
        for (scope, token) in &keys {
            if scope.reaches(needs) {
                continue;
            }
            let answer = status(&app, method, path, Some(token)).await;
            if answer != StatusCode::FORBIDDEN {
                wrong.push(format!(
                    "{method} {path} answered {answer} to a {scope:?} key"
                ));
            }
            refused += 1;
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(refused > 1000, "only {refused} refusals were checked");
    // The scopes nest as they're described: playback reaches what read does, and full all.
    assert!(Scope::Playback.reaches(Scope::Read) && !Scope::Read.reaches(Scope::Playback));
    assert!(!Scope::MusicSync.reaches(Scope::Read) && !Scope::Read.reaches(Scope::MusicSync));
    assert!(Scope::Full.reaches(Scope::MusicSync) && !Scope::Playback.reaches(Scope::Full));
}

#[tokio::test]
async fn each_key_does_what_its_for() {
    let (pool, app, _artwork) = full_server().await;
    let keys: BTreeMap<&str, String> = keys(&pool)
        .await
        .into_iter()
        .map(|(scope, token)| (scope.as_str(), token))
        .collect();
    let ask = |method: Method, path: &'static str, scope: &str| {
        let token = keys[scope].clone();
        let app = app.clone();
        async move { status(&app, &method, path, Some(&token)).await }
    };
    // Reading: what anyone signed in may read, but nothing written, Aoide's or admins'.
    assert_eq!(ask(Method::GET, "/api/v1/me", "read").await, StatusCode::OK);
    assert_eq!(
        ask(Method::GET, "/api/v1/movies", "read").await,
        StatusCode::OK
    );
    assert_eq!(
        ask(Method::POST, "/api/v1/requests", "read").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ask(Method::PUT, "/api/v1/files/1/watched", "read").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ask(Method::GET, "/aoide/sync/status", "read").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ask(Method::GET, "/api/v1/users", "read").await,
        StatusCode::FORBIDDEN
    );
    // Playback: marking watched gets past the key (then wants its body).
    assert_eq!(
        ask(Method::PUT, "/api/v1/files/1/watched", "playback").await,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(
        ask(Method::POST, "/api/v1/auth/keys", "playback").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ask(Method::DELETE, "/api/v1/music/plays", "playback").await,
        StatusCode::FORBIDDEN
    );
    // Music sync: Aoide's routes and nothing else.
    assert_eq!(
        ask(Method::GET, "/aoide/sync/status", "music-sync").await,
        StatusCode::OK
    );
    assert_eq!(
        ask(Method::GET, "/api/v1/movies", "music-sync").await,
        StatusCode::FORBIDDEN
    );
    // Full: as every key was before scopes, an admin's reaching admin routes too.
    assert_eq!(
        ask(Method::GET, "/api/v1/users", "full").await,
        StatusCode::OK
    );
    assert_eq!(
        ask(Method::GET, "/aoide/sync/status", "full").await,
        StatusCode::OK
    );
}
