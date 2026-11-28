use super::*;
use crate::watch_party::{self, State, guests};

const FILM: State = State {
    file_id: 1,
    position_ms: 0,
    playing: true,
};

/// A party playing episode 1, started by alice's device, with bob's device and the guest Sam.
async fn setup() -> (SqlitePool, (i64, i64), (i64, i64), i64, guests::Guest) {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'shows', 'shows', '/s');
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'Show', 'Show', 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, series_id,
             season, episode, first_seen_generation, seen_generation)
             VALUES (1, 1, 'Show/1.mkv', 1, 0, 1, 1, 1, 1, 1),
                    (2, 1, 'Show/2.mkv', 1, 0, 1, 1, 2, 1, 1),
                    (3, 1, 'Show/3.mkv', 1, 0, 1, 1, 3, 1, 1);
         UPDATE media_files SET missing_since = 5 WHERE id = 3;",
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
    let party = watch_party::start(&pool, devices[0], &FILM, None)
        .await
        .unwrap()
        .unwrap();
    watch_party::join(&pool, devices[1].1, party).await.unwrap();
    let (invite, _) = guests::invite(&pool, devices[0], party)
        .await
        .unwrap()
        .unwrap();
    let (_, guest) = guests::accept(&pool, &invite, "Sam")
        .await
        .unwrap()
        .unwrap();
    (pool, devices[0], devices[1], party, guest)
}

#[tokio::test]
async fn the_host_says_what_plays_next_and_everyone_sees_it_until_it_moves_on() {
    let (pool, alice, bob, party, guest) = setup().await;
    let next = UpNext {
        file_id: 2,
        starts_at: 1_800_000_000_000,
    };
    let shown = |pool: SqlitePool, token: i64| async move {
        watch_party::one(&pool, token, party)
            .await
            .unwrap()
            .unwrap()["up_next"]
            .clone()
    };
    assert_eq!(shown(pool.clone(), bob.1).await, Value::Null);
    assert_eq!(
        set(&pool, bob, party, Some(next)).await.unwrap(),
        Err(Refused::NotHost(
            "only whoever started the party says what it plays next"
        ))
    );
    let gone = UpNext { file_id: 3, ..next };
    assert!(matches!(
        set(&pool, alice, party, Some(gone)).await.unwrap(),
        Err(Refused::Invalid(_))
    ));
    set(&pool, alice, party, Some(next)).await.unwrap().unwrap();
    let said = json!({
        "file_id": 2,
        "title": "Show, season 1 episode 2",
        "starts_at": 1_800_000_000_000_i64,
    });
    assert_eq!(shown(pool.clone(), bob.1).await, said);
    let theirs = guests::view(&pool, &guest).await.unwrap().unwrap();
    assert_eq!(theirs["up_next"], said);

    // Cleared by the host, or by the party moving on.
    set(&pool, alice, party, None).await.unwrap().unwrap();
    assert_eq!(shown(pool.clone(), alice.1).await, Value::Null);
    set(&pool, alice, party, Some(next)).await.unwrap().unwrap();
    let version: i64 = sqlx::query_scalar("SELECT version FROM watch_parties")
        .fetch_one(&pool)
        .await
        .unwrap();
    let second = State { file_id: 2, ..FILM };
    watch_party::set(&pool, alice, party, version, &second)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(shown(pool.clone(), bob.1).await, Value::Null);
}

#[tokio::test]
async fn why_a_guest_went_is_kept_for_their_last_word() {
    let (pool, alice, _, party, sam) = setup().await;
    let invite = || async {
        let (invite, _) = guests::invite(&pool, alice, party).await.unwrap().unwrap();
        guests::accept(&pool, &invite, "Kim")
            .await
            .unwrap()
            .unwrap()
            .1
    };
    assert_eq!(
        guests::gone_why(&pool, sam.id).await.unwrap(),
        None,
        "still in"
    );
    guests::leave(&pool, sam.id).await.unwrap();
    assert_eq!(
        guests::gone_why(&pool, sam.id).await.unwrap().as_deref(),
        Some("left")
    );
    let kim = invite().await;
    if kim.id == sam.id {
        assert_eq!(
            guests::gone_why(&pool, kim.id).await.unwrap(),
            None,
            "id used again"
        );
    }
    guests::send_away(&pool, alice.1, party, kim.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        guests::gone_why(&pool, kim.id).await.unwrap().as_deref(),
        Some("sent_away")
    );
    let lee = invite().await;
    watch_party::end(&pool, (alice.0, alice.1, false), party)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        guests::gone_why(&pool, lee.id).await.unwrap().as_deref(),
        Some("ended")
    );
}
