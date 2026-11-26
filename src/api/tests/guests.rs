//! Watch party guests' routes, held to what their pass may do: nothing an account's sign-in
//! or API key opens, and nothing without a guest's pass.

use axum::http::StatusCode;

use super::access::{guest_routes, key, status, sweep};
use super::openapi::full_server;

/// Every route a watch party guest's pass guards refuses every account's sign-in and API key,
/// whatever its scope, and answers to a guest's pass alone.
#[tokio::test]
async fn guest_routes_answer_only_to_a_guest_pass() {
    let (pool, app, _artwork) = full_server().await;
    let (answers, _) = sweep(&app).await;
    let guests = guest_routes();
    assert!(
        guests.len() >= 8,
        "only {} guest routes were found",
        guests.len()
    );
    // A party, and a guest in it.
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (91, 'party', 'movies', '/p');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (91, 91, 'Film', 'Film', 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation) VALUES (91, 91, 'Film/f.mkv', 1, 0, 91, 1, 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let host = crate::auth::testing::user(&pool, "host", false).await;
    let (device, session) =
        crate::auth::issue_token(&pool, host.id, crate::auth::TokenKind::Session, "tv")
            .await
            .unwrap();
    let film = crate::watch_party::State {
        file_id: 91,
        position_ms: 0,
        playing: false,
    };
    let party = crate::watch_party::start(&pool, (host.id, device), &film, None)
        .await
        .unwrap()
        .unwrap();
    // A guest let in afresh for each route: leaving ends a pass.
    let new_guest = || async {
        let (invite, _) = crate::watch_party::guests::invite(&pool, (host.id, device), party)
            .await
            .unwrap()
            .unwrap();
        crate::watch_party::guests::accept(&pool, &invite, "Sam")
            .await
            .unwrap()
            .unwrap()
            .0
    };
    let mut accounts = vec![session];
    use crate::auth::Scope;
    for scope in [Scope::Read, Scope::Playback, Scope::MusicSync, Scope::Full] {
        let (_, key) =
            crate::auth::issue_scoped(&pool, host.id, crate::auth::TokenKind::ApiKey, "k", scope)
                .await
                .unwrap();
        accounts.push(key);
    }
    let mut wrong = Vec::new();
    for (method, path) in answers.keys() {
        if !guests.contains(&key(method.as_str(), path)) {
            continue;
        }
        for token in &accounts {
            let answer = status(&app, method, path, Some(token)).await;
            if answer != StatusCode::UNAUTHORIZED {
                wrong.push(format!(
                    "{method} {path} answered {answer} to an account's token"
                ));
            }
        }
        let pass = new_guest().await;
        let answer = status(&app, method, path, Some(&pass)).await;
        if answer == StatusCode::UNAUTHORIZED {
            wrong.push(format!("{method} {path} refused a guest's pass"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
