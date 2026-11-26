use super::*;
use crate::watch_party::{self, State};

/// A host signed in on a device, in a party playing file 1 of two.
async fn setup() -> (SqlitePool, (i64, i64), i64) {
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
    let user = crate::auth::testing::user(&pool, "alice", false).await;
    let (token, _) =
        crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "tv")
            .await
            .unwrap();
    let host = (user.id, token);
    let film = State {
        file_id: 1,
        position_ms: 0,
        playing: true,
    };
    let party = watch_party::start(&pool, host, &film, None)
        .await
        .unwrap()
        .unwrap();
    (pool, host, party)
}

#[tokio::test]
async fn an_invite_lets_one_guest_in_once_and_they_see_no_accounts() {
    let (pool, host, party) = setup().await;
    let (secret, _) = invite(&pool, host, party).await.unwrap().unwrap();
    assert!(secret.starts_with(INVITE_PREFIX));
    // A name is needed, and a long one isn't taken.
    assert!(matches!(
        accept(&pool, &secret, "  ").await.unwrap(),
        Err(Refused::Invalid(_))
    ));
    assert!(matches!(
        accept(&pool, &secret, &"x".repeat(41)).await.unwrap(),
        Err(Refused::Invalid(_))
    ));
    let (pass, guest) = accept(&pool, &secret, "  Sam \u{7}the  friend ")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(guest.name, "Sam the friend");
    assert!(pass.starts_with(PASS_PREFIX));
    // The same link doesn't let a second in.
    assert_eq!(
        accept(&pool, &secret, "Other").await.unwrap(),
        Err(Refused::NotFound)
    );
    assert_eq!(by_pass(&pool, &pass).await.unwrap(), Some(guest.clone()));
    assert_eq!(by_pass(&pool, "mpg_nope").await.unwrap(), None);
    assert_eq!(by_pass(&pool, "mls_x").await.unwrap(), None);

    let seen = view(&pool, &guest).await.unwrap().unwrap();
    assert_eq!(seen["title"], "Film");
    assert_eq!(seen["household"], 1);
    assert_eq!(
        seen["guests"],
        json!([{"name": "Sam the friend", "you": true}])
    );
    assert!(!seen.to_string().contains("alice"), "{seen}");
    // The household sees the guest, to send them away.
    let party_now = watch_party::one(&pool, host.1, party)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(party_now["guests"][0]["name"], "Sam the friend");
}

#[tokio::test]
async fn a_guest_plays_and_pauses_sees_only_what_plays_and_can_be_sent_away() {
    let (pool, host, party) = setup().await;
    let (secret, _) = invite(&pool, host, party).await.unwrap().unwrap();
    let (pass, guest) = accept(&pool, &secret, "Sam").await.unwrap().unwrap();
    assert_eq!(
        set(&pool, &guest, 1, (30_000, false)).await.unwrap(),
        Ok(())
    );
    assert_eq!(
        set(&pool, &guest, 1, (0, true)).await.unwrap(),
        Err(Refused::Stale)
    );
    let party_now = watch_party::one(&pool, host.1, party)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (party_now["playing"].clone(), party_now["file_id"].clone()),
        (json!(false), json!(1))
    );

    assert!(sees(&pool, guest.id, 1).await.unwrap().is_some());
    assert!(sees(&pool, guest.id, 2).await.unwrap().is_none());
    // The host moves the party on: the guest's way to the old file ends.
    let next = State {
        file_id: 2,
        position_ms: 0,
        playing: true,
    };
    watch_party::set(&pool, host, party, 2, &next)
        .await
        .unwrap()
        .unwrap();
    assert!(sees(&pool, guest.id, 1).await.unwrap().is_none());
    assert!(sees(&pool, guest.id, 2).await.unwrap().is_some());

    // Someone not in the party can't make invites or send guests away.
    assert_eq!(
        invite(&pool, (host.0, 999), party).await.unwrap(),
        Err(Refused::NotFound)
    );
    assert_eq!(
        send_away(&pool, 999, party, guest.id).await.unwrap(),
        Err(Refused::NotFound)
    );
    assert_eq!(
        send_away(&pool, host.1, party, guest.id).await.unwrap(),
        Ok(())
    );
    assert_eq!(by_pass(&pool, &pass).await.unwrap(), None);
    assert!(sees(&pool, guest.id, 2).await.unwrap().is_none());
}

#[tokio::test]
async fn guests_go_when_their_party_ends() {
    let (pool, host, party) = setup().await;
    let (secret, _) = invite(&pool, host, party).await.unwrap().unwrap();
    let (pass, guest) = accept(&pool, &secret, "Sam").await.unwrap().unwrap();
    // The household's last device leaves: the party ends, and its guests with it.
    watch_party::leave(&pool, host.1, party).await.unwrap();
    assert_eq!(by_pass(&pool, &pass).await.unwrap(), None);
    assert!(!still_in(&pool, guest.id).await.unwrap());
    // An invite to a party that ended lets no one in.
    let (pool, host, party) = setup().await;
    let (secret, _) = invite(&pool, host, party).await.unwrap().unwrap();
    watch_party::end(&pool, (host.0, host.1, false), party)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        accept(&pool, &secret, "Sam").await.unwrap(),
        Err(Refused::NotFound)
    );
}

#[tokio::test]
async fn a_guests_way_to_the_file_ends_with_the_party_even_before_its_cleared_away() {
    let (pool, host, party) = setup().await;
    let (secret, _) = invite(&pool, host, party).await.unwrap().unwrap();
    let (_, guest) = accept(&pool, &secret, "Sam").await.unwrap().unwrap();
    assert_eq!(sees(&pool, guest.id, 1).await.unwrap(), Some(host.0));
    // Quiet for over a day: over, though nothing has cleared it away yet.
    sqlx::query("UPDATE watch_parties SET updated_at = 0")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(sees(&pool, guest.id, 1).await.unwrap(), None);
    sqlx::query("UPDATE watch_parties SET updated_at = ?")
        .bind(now_ms())
        .execute(&pool)
        .await
        .unwrap();
    assert!(sees(&pool, guest.id, 1).await.unwrap().is_some());
    // The household's last device signs out: over at once.
    sqlx::query("DELETE FROM tokens WHERE id = ?")
        .bind(host.1)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(sees(&pool, guest.id, 1).await.unwrap(), None);
}
