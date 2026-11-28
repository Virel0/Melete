use serde_json::json;

use super::*;
use crate::watch_party::waits::{self, Waiter};
use crate::watch_party::{self, State, guests};

const FILM: State = State {
    file_id: 1,
    position_ms: 10_000,
    playing: true,
};

/// A party playing a film, started by alice's device, with bob's device and the guest Sam in it.
async fn setup() -> (SqlitePool, (i64, i64), (i64, i64), i64, guests::Guest) {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'films', 'movies', '/f');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'Film', 'Film', 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation)
             VALUES (1, 1, 'Film/a.mkv', 1, 0, 1, 1, 1), (2, 1, 'Film/b.mkv', 1, 0, 1, 1, 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut devices = Vec::new();
    for name in ["alice", "bob"] {
        let user = crate::auth::testing::user(&pool, name, false).await;
        let (token, _) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, name)
                .await
                .unwrap();
        devices.push((user.id, token));
    }
    let (alice, bob) = (devices[0], devices[1]);
    let party = watch_party::start(&pool, alice, &FILM, None)
        .await
        .unwrap()
        .unwrap();
    watch_party::join(&pool, bob.1, party).await.unwrap();
    let (invite, _) = guests::invite(&pool, alice, party).await.unwrap().unwrap();
    let (_, guest) = guests::accept(&pool, &invite, "Sam")
        .await
        .unwrap()
        .unwrap();
    (pool, alice, bob, party, guest)
}

async fn household(pool: &SqlitePool, token: i64, party: i64) -> serde_json::Value {
    watch_party::one(pool, token, party).await.unwrap().unwrap()
}

async fn version(pool: &SqlitePool, party: i64) -> i64 {
    sqlx::query_scalar("SELECT version FROM watch_parties WHERE id = ?")
        .bind(party)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[test]
fn a_change_is_what_it_changes() {
    let playing = Before {
        file_id: 1,
        position_ms: 10_000,
        position_at: 1_000,
        playing: true,
    };
    let now = 6_000;
    assert_eq!(kind(&playing, (2, 0, true), now), Some("next"));
    assert_eq!(kind(&playing, (1, 15_000, false), now), Some("pause"));
    // Where it would be by now, or near it: nothing.
    assert_eq!(kind(&playing, (1, 15_000, true), now), None);
    assert_eq!(kind(&playing, (1, 16_500, true), now), None);
    assert_eq!(kind(&playing, (1, 17_000, true), now), Some("seek"));
    assert_eq!(kind(&playing, (1, 3_000, true), now), Some("seek"));
    let paused = Before {
        playing: false,
        ..playing
    };
    assert_eq!(kind(&paused, (1, 10_000, true), now), Some("play"));
    assert_eq!(
        kind(&paused, (1, 10_000, false), now),
        None,
        "still where it was"
    );
    assert_eq!(kind(&paused, (1, 40_000, false), now), Some("seek"));
}

#[tokio::test]
async fn everyone_is_told_who_changed_what_and_a_guest_never_an_accounts_name() {
    let (pool, alice, bob, party, guest) = setup().await;
    assert_eq!(
        household(&pool, alice.1, party).await["last_change"],
        json!(null)
    );

    let paused = State {
        playing: false,
        ..FILM
    };
    let v = version(&pool, party).await;
    watch_party::set(&pool, alice, party, v, &paused)
        .await
        .unwrap()
        .unwrap();
    let theirs = household(&pool, alice.1, party).await["last_change"].clone();
    let at = theirs["at"].as_i64().unwrap();
    assert_eq!(
        theirs,
        json!({ "by": "alice", "guest": false, "you": true, "kind": "pause", "at": at })
    );
    assert_eq!(
        household(&pool, bob.1, party).await["last_change"]["you"],
        false
    );
    let seen = guests::view(&pool, &guest).await.unwrap().unwrap();
    assert_eq!(
        seen["last_change"],
        json!({ "by": waits::SOMEONE, "guest": false, "you": false, "kind": "pause", "at": at })
    );
    assert!(!seen.to_string().contains("alice"), "{seen}");

    // The guest jumps ahead.
    let v = version(&pool, party).await;
    guests::set(&pool, &guest, v, (600_000, false))
        .await
        .unwrap()
        .unwrap();
    let seen = guests::view(&pool, &guest).await.unwrap().unwrap();
    assert_eq!(seen["last_change"]["kind"], "seek");
    assert_eq!(seen["last_change"]["by"], "Sam");
    assert_eq!(seen["last_change"]["you"], true);
    let theirs = household(&pool, bob.1, party).await["last_change"].clone();
    assert_eq!(
        (theirs["by"].clone(), theirs["guest"].clone()),
        (json!("Sam"), json!(true))
    );

    // Setting it as it already is changes nothing to tell; the host moving on is `next`.
    let v = version(&pool, party).await;
    guests::set(&pool, &guest, v, (600_000, false))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        household(&pool, bob.1, party).await["last_change"]["by"],
        "Sam"
    );
    let next = State {
        file_id: 2,
        position_ms: 0,
        playing: true,
    };
    let v = version(&pool, party).await;
    watch_party::set(&pool, alice, party, v, &next)
        .await
        .unwrap()
        .unwrap();
    let theirs = household(&pool, bob.1, party).await["last_change"].clone();
    assert_eq!(
        (theirs["kind"].clone(), theirs["by"].clone()),
        (json!("next"), json!("alice"))
    );
}

#[tokio::test]
async fn each_wait_says_since_when_until_when_and_whether_its_yours() {
    let (pool, alice, bob, party, guest) = setup().await;
    waits::set(&pool, party, (Waiter::Device(bob.1), "bob"), true)
        .await
        .unwrap();
    waits::set(&pool, party, (Waiter::Guest(guest.id), "Sam"), true)
        .await
        .unwrap();
    let mine = household(&pool, bob.1, party).await;
    let all = mine["waits"].as_array().unwrap().clone();
    assert_eq!(all.len(), 2);
    let since = all[0]["since"].as_i64().unwrap();
    assert_eq!(
        all[0],
        json!({ "name": "bob", "guest": false, "you": true, "since": since,
                "ends_at": since + waits::MOST_MS })
    );
    assert_eq!(
        (
            all[1]["name"].clone(),
            all[1]["guest"].clone(),
            all[1]["you"].clone()
        ),
        (json!("Sam"), json!(true), json!(false))
    );
    assert_eq!(mine["waiting"], json!(["bob", "Sam"]), "as before");
    assert_eq!(
        household(&pool, alice.1, party).await["waits"][0]["you"],
        false
    );

    let seen = guests::view(&pool, &guest).await.unwrap().unwrap();
    let theirs = seen["waits"].as_array().unwrap();
    assert_eq!(theirs[0]["name"], waits::SOMEONE);
    assert_eq!(theirs[0]["you"], false);
    assert_eq!(
        (theirs[1]["name"].clone(), theirs[1]["you"].clone()),
        (json!("Sam"), json!(true))
    );
    assert!(!seen.to_string().contains("bob"), "{seen}");
}
