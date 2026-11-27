use super::*;

/// Two users each signed in on a device, and two present files (a film and an episode).
async fn setup() -> (SqlitePool, (i64, i64), (i64, i64)) {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'films', 'movies', '/f');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'A', 'A', 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation)
             VALUES (1, 1, 'A/a.mkv', 1, 0, 1, 1, 1), (2, 1, 'A/b.mkv', 1, 0, 1, 1, 1),
                    (3, 1, 'A/c.mkv', 1, 0, 1, 1, 1);
         UPDATE media_files SET missing_since = 5 WHERE id = 3;",
    )
    .execute(&pool)
    .await
    .unwrap();
    let device = |name: &'static str| {
        let pool = pool.clone();
        async move {
            let user = crate::auth::testing::user(&pool, name, false).await;
            let (token, _) =
                crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, name)
                    .await
                    .unwrap();
            (user.id, token)
        }
    };
    let host = device("alice").await;
    let friend = device("bob").await;
    (pool, host, friend)
}

const FILM: State = State {
    file_id: 1,
    position_ms: 0,
    playing: true,
};

#[tokio::test]
async fn a_party_plays_in_step_and_only_its_host_changes_what_plays() {
    let (pool, host, friend) = setup().await;
    let id = start(&pool, host, &FILM, Some(8_000_000))
        .await
        .unwrap()
        .unwrap();
    let party = one(&pool, host.1, id).await.unwrap().unwrap();
    assert_eq!(party["guest_max_bitrate"], 8_000_000);
    let code = party["code"].as_str().unwrap().to_owned();
    // Typed loosely, the code still finds it.
    let loose = format!("{}-{}", &code[..3], code[3..].to_lowercase());
    assert_eq!(by_code(&pool, &loose).await.unwrap(), Some((id, 1)));
    assert_eq!(by_code(&pool, "ZZZZZZ").await.unwrap(), None);
    join(&pool, friend.1, id).await.unwrap();
    let party = one(&pool, friend.1, id).await.unwrap().unwrap();
    assert_eq!(party["members"].as_array().unwrap().len(), 2);
    assert_eq!(party["host"], "alice");

    // The friend pauses and seeks; a change against an older version is refused.
    let paused = State {
        position_ms: 60_000,
        playing: false,
        ..FILM
    };
    assert_eq!(set(&pool, friend, id, 1, &paused).await.unwrap(), Ok(()));
    assert_eq!(
        set(&pool, host, id, 1, &FILM).await.unwrap(),
        Err(Refused::Stale)
    );
    let party = one(&pool, host.1, id).await.unwrap().unwrap();
    assert_eq!(
        (party["version"].clone(), party["playing"].clone()),
        (json!(2), json!(false))
    );
    assert_eq!(party["position_ms"], 60_000);

    // Only the host moves it on to another file, and only to one in the library.
    let next = State {
        file_id: 2,
        position_ms: 0,
        playing: true,
    };
    assert!(matches!(
        set(&pool, friend, id, 2, &next).await.unwrap(),
        Err(Refused::NotHost(_))
    ));
    let gone = State { file_id: 3, ..next };
    assert!(matches!(
        set(&pool, host, id, 2, &gone).await.unwrap(),
        Err(Refused::Invalid(_))
    ));
    assert_eq!(set(&pool, host, id, 2, &next).await.unwrap(), Ok(()));
    assert_eq!(
        one(&pool, friend.1, id).await.unwrap().unwrap()["file_id"],
        2
    );
}

#[tokio::test]
async fn only_the_host_ends_a_party_and_the_last_to_leave_ends_it_too() {
    let (pool, host, friend) = setup().await;
    let id = start(&pool, host, &FILM, None).await.unwrap().unwrap();
    join(&pool, friend.1, id).await.unwrap();
    assert!(matches!(
        end(&pool, (friend.0, friend.1, false), id).await.unwrap(),
        Err(Refused::NotHost(_))
    ));
    // An admin may end anyone's.
    assert_eq!(
        end(&pool, (friend.0, friend.1, true), id).await.unwrap(),
        Ok(())
    );
    assert!(for_device(&pool, host.1).await.unwrap().is_empty());

    let id = start(&pool, host, &FILM, None).await.unwrap().unwrap();
    join(&pool, friend.1, id).await.unwrap();
    assert!(leave(&pool, host.1, id).await.unwrap());
    assert_eq!(
        one(&pool, friend.1, id).await.unwrap().unwrap()["members"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(leave(&pool, friend.1, id).await.unwrap());
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM watch_parties")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    // A bad guest cap isn't taken.
    assert!(matches!(
        start(&pool, host, &FILM, Some(0)).await.unwrap(),
        Err(Refused::Invalid(_))
    ));
}

#[tokio::test]
async fn a_party_set_playing_starts_a_moment_ahead_and_a_pause_at_once() {
    let (pool, host, _) = setup().await;
    let id = start(&pool, host, &FILM, None).await.unwrap().unwrap();
    let at = |pool: SqlitePool| async move {
        let at: i64 = sqlx::query_scalar("SELECT position_at FROM watch_parties")
            .fetch_one(&pool)
            .await
            .unwrap();
        at - now_ms()
    };
    let paused = State {
        playing: false,
        ..FILM
    };
    set(&pool, host, id, 1, &paused).await.unwrap().unwrap();
    assert!(at(pool.clone()).await <= 0, "a pause takes effect at once");
    set(&pool, host, id, 2, &FILM).await.unwrap().unwrap();
    let ahead = at(pool.clone()).await;
    assert!((300..=START_LEAD_MS).contains(&ahead), "{ahead}");
}
