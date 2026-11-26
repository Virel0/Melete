use serde_json::json;

use super::*;
use crate::watch_party::{self, State, guests};

/// A host's device in a party playing a film, and a guest in it.
async fn setup() -> (SqlitePool, (i64, i64), i64, guests::Guest) {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'films', 'movies', '/f');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'Film', 'Film', 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation) VALUES (1, 1, 'Film/a.mkv', 1, 0, 1, 1, 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let user = crate::auth::testing::user(&pool, "alice", false).await;
    let (token, _) =
        crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "tv")
            .await
            .unwrap();
    let host = (user.id, token);
    let film = State {
        file_id: 1,
        position_ms: 10_000,
        playing: true,
    };
    let party = watch_party::start(&pool, host, &film, None)
        .await
        .unwrap()
        .unwrap();
    let (invite, _) = guests::invite(&pool, host, party).await.unwrap().unwrap();
    let (_, guest) = guests::accept(&pool, &invite, "Sam")
        .await
        .unwrap()
        .unwrap();
    (pool, host, party, guest)
}

async fn state(pool: &SqlitePool, party: i64) -> (bool, bool, i64) {
    sqlx::query_as("SELECT playing, held, position_ms FROM watch_parties WHERE id = ?")
        .bind(party)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_party_pauses_for_whoever_is_loading_and_plays_on_once_theyre_ready() {
    let (pool, host, party, guest) = setup().await;
    sqlx::query("UPDATE watch_parties SET position_at = position_at - 2000")
        .execute(&pool)
        .await
        .unwrap();
    set(&pool, party, (Waiter::Guest(guest.id), "Sam"), true)
        .await
        .unwrap();
    let (playing, held, at) = state(&pool, party).await;
    assert!(!playing && held);
    // Paused where it had got to: about two seconds on from 10.
    assert!((12_000..13_000).contains(&at), "{at}");
    let seen = watch_party::one(&pool, host.1, party)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (seen["held"].clone(), seen["waiting"].clone()),
        (json!(true), json!(["Sam"]))
    );

    // The host's device loads too; the guest is ready first, so it still waits.
    set(&pool, party, (Waiter::Device(host.1), "alice"), true)
        .await
        .unwrap();
    set(&pool, party, (Waiter::Guest(guest.id), "Sam"), false)
        .await
        .unwrap();
    assert!(!state(&pool, party).await.0);
    // A guest is told someone of the household is loading, never who.
    let theirs = guests::view(&pool, &guest).await.unwrap().unwrap();
    assert_eq!(theirs["waiting"], json!([SOMEONE]));
    assert!(!theirs.to_string().contains("alice"));
    set(&pool, party, (Waiter::Device(host.1), "alice"), false)
        .await
        .unwrap();
    let (playing, held, at) = state(&pool, party).await;
    assert!(playing && !held);
    assert!(
        (12_000..13_000).contains(&at),
        "plays on from where it paused: {at}"
    );
}

#[tokio::test]
async fn a_wait_is_let_go_after_thirty_seconds_and_by_hand_play_or_pause() {
    let (pool, host, party, guest) = setup().await;
    set(&pool, party, (Waiter::Guest(guest.id), "Sam"), true)
        .await
        .unwrap();
    sqlx::query("UPDATE watch_party_waits SET since = since - 31000")
        .execute(&pool)
        .await
        .unwrap();
    // Seen again: too long, so the party plays on without them.
    assert!(names(&pool, party).await.unwrap().is_empty());
    assert!(state(&pool, party).await.0);

    // Paused by hand while waiting: it stays paused, and the wait is over.
    set(&pool, party, (Waiter::Guest(guest.id), "Sam"), true)
        .await
        .unwrap();
    let version: i64 = sqlx::query_scalar("SELECT version FROM watch_parties")
        .fetch_one(&pool)
        .await
        .unwrap();
    let paused = State {
        file_id: 1,
        position_ms: 5_000,
        playing: false,
    };
    watch_party::set(&pool, host, party, version, &paused)
        .await
        .unwrap()
        .unwrap();
    assert!(names(&pool, party).await.unwrap().is_empty());
    assert_eq!(state(&pool, party).await, (false, false, 5_000));
    // A wait on a paused party doesn't start it later.
    set(&pool, party, (Waiter::Guest(guest.id), "Sam"), true)
        .await
        .unwrap();
    set(&pool, party, (Waiter::Guest(guest.id), "Sam"), false)
        .await
        .unwrap();
    assert_eq!(state(&pool, party).await, (false, false, 5_000));
    // Someone who left isn't waited for.
    set(&pool, party, (Waiter::Device(host.1), "alice"), true)
        .await
        .unwrap();
    guests::leave(&pool, guest.id).await.unwrap();
    assert_eq!(
        names(&pool, party).await.unwrap(),
        [("alice".to_owned(), false)]
    );
}
