//! A switch and what it can't renumber: episodes the new order leaves out, downloads on their
//! way, results searched for in the old order, and seasons asked for by TMDB's numbers.

use super::*;

/// A group with one season of all of TMDB's episodes but the 13th (TMDB id 1013, a recap, say):
/// the 14th is its episode 13.
async fn add_group_without_the_recap(pool: &SqlitePool) {
    sqlx::query(
        "INSERT INTO tmdb_episode_groups (id, series_tmdb_id, name, kind, fetched_at)
             VALUES ('rrrr', 100, 'No recap', 6, 10);
         INSERT INTO tmdb_episode_group_episodes (group_id, season, episode, episode_tmdb_id)
             SELECT 'rrrr', 1, row_number() OVER (ORDER BY tmdb_id), tmdb_id FROM tmdb_episodes
             WHERE series_tmdb_id = 100 AND season = 1 AND tmdb_id <> 1013",
    )
    .execute(pool)
    .await
    .unwrap();
}

/// A download of `title` for (season, episode) of the wanted series, in `state`.
async fn download(pool: &SqlitePool, hash: char, season: i64, episode: Option<i64>, state: &str) {
    sqlx::query(
        "INSERT INTO downloads (infohash, title, wanted_series_id, season, episode, decision,
             state, added_at, updated_at)
         VALUES (?, 'Some Anime', 1, ?, ?, '{}', ?, unixepoch(), 1)",
    )
    .bind(hash.to_string().repeat(40))
    .bind(season)
    .bind(episode)
    .bind(state)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn what_named_an_episode_the_new_order_leaves_out_names_no_other() {
    let pool = some_anime().await;
    add_group_without_the_recap(&pool).await;
    // The recap (S01E13 now) and the episode after it (S01E14) were each grabbed and searched
    // for; their grabs are in the history.
    download(&pool, 'a', 1, Some(13), "imported").await;
    download(&pool, 'b', 1, Some(14), "failed").await;
    sqlx::query(
        "INSERT INTO searches (wanted_series_id, season, episode, searched_at, indexers)
             VALUES (1, 1, 13, unixepoch(), '[]'), (1, 1, 14, unixepoch(), '[]');
         INSERT INTO wanted_episode_overrides (wanted_series_id, season, episode, monitored)
             VALUES (1, 1, 13, 0);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let rows = |table: &'static str| {
        let pool = pool.clone();
        async move {
            let rows: Vec<(Option<i64>, Option<i64>, Option<i64>)> =
                sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "SELECT season, episode, left_out_tmdb_id FROM {table}
                     WHERE {} ORDER BY id",
                    if table == "title_events" {
                        "kind <> 'reordered'"
                    } else {
                        "1"
                    }
                )))
                .fetch_all(&pool)
                .await
                .unwrap();
            rows
        }
    };
    assert_eq!(
        choose(&pool, 100, Some("rrrr"), ChosenBy::Auto, 50)
            .await
            .unwrap(),
        Chosen::Done
    );
    // The recap's rows lose their number, now the 14th's, and keep its TMDB id; the 14th's are
    // its episode 13.
    let expected = vec![(None, None, Some(1013)), (Some(1), Some(13), None)];
    assert_eq!(rows("downloads").await, expected);
    assert_eq!(rows("searches").await, expected);
    let history = rows("title_events").await;
    assert_eq!(history.len(), 4, "{history:?}");
    assert!(
        history.iter().filter(|r| r.2 == Some(1013)).count() == 2,
        "{history:?}"
    );
    assert!(history.iter().all(|r| r.2.is_some() || r.0 == Some(1)));
    // The recap being left out doesn't leave the 14th out.
    let off: Vec<(i64, i64)> =
        sqlx::query_as("SELECT season, episode FROM wanted_episode_overrides WHERE monitored = 0")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(off, vec![]);
}

#[tokio::test]
async fn a_switch_waits_while_a_download_comes_for_what_it_would_renumber_away() {
    let pool = some_anime().await;
    add_group_without_the_recap(&pool).await;
    // The recap on its way: imported now, it would be taken for the 14th.
    download(&pool, 'a', 1, Some(13), "downloading").await;
    assert_eq!(
        choose(&pool, 100, Some("rrrr"), ChosenBy::Auto, 50)
            .await
            .unwrap(),
        Chosen::Busy((1, 13))
    );
    assert_eq!(current(&pool, 100).await.unwrap().group_id, None);
    let kept: (Option<i64>, Option<i64>) = sqlx::query_as("SELECT season, episode FROM downloads")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kept, (Some(1), Some(13)));
    // A season pack on its way for a season the group makes up of other episodes waits too.
    sqlx::query("UPDATE downloads SET state = 'imported'")
        .execute(&pool)
        .await
        .unwrap();
    download(&pool, 'b', 1, None, "transferring").await;
    assert_eq!(
        choose(&pool, 100, Some("rrrr"), ChosenBy::Auto, 50)
            .await
            .unwrap(),
        Chosen::Busy((1, 0))
    );
    // Once it's in, the switch goes ahead.
    sqlx::query("UPDATE downloads SET state = 'imported'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        choose(&pool, 100, Some("rrrr"), ChosenBy::Auto, 50)
            .await
            .unwrap(),
        Chosen::Done
    );
}

#[tokio::test]
async fn results_searched_for_before_a_switch_arent_grabbed() {
    let pool = some_anime().await;
    add_group(&pool, "aaaa", "Seasons", &[24, 23, 12]).await;
    // Results of a search for S01E30 under TMDB's own order (the group's S02E06), and of one
    // after the switch.
    let result = |searched_at: &'static str| {
        let pool = pool.clone();
        async move {
            let search: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "INSERT INTO searches (wanted_series_id, season, episode, searched_at, indexers)
                 VALUES (1, 1, 30, {searched_at}, '[]') RETURNING id"
            )))
            .fetch_one(&pool)
            .await
            .unwrap();
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO release_decisions (search_id, title, accepted, decision)
                 VALUES (?, 'Some Anime - 30', 1, '{}') RETURNING id",
            )
            .bind(search)
            .fetch_one(&pool)
            .await
            .unwrap();
            id
        }
    };
    let before = result("unixepoch() - 60").await;
    assert_eq!(decide(&pool).await.unwrap(), 1);
    let after = result("unixepoch() + 60").await;
    let (url, _fake) = crate::deluge::testing::start().await;
    let deluge = crate::deluge::Client::new(&url, crate::config::Secret::for_tests("pw")).unwrap();
    let fetcher = crate::downloads::Fetcher::new().unwrap();
    let grab = |id| crate::downloads::grab(&pool, &deluge, &fetcher, id);
    assert_eq!(
        grab(before).await.unwrap().err(),
        Some(crate::downloads::Refused::Reordered)
    );
    // A result from after it goes on (here, to find it has no link).
    assert_eq!(
        grab(after).await.unwrap().err(),
        Some(crate::downloads::Refused::NoLink)
    );
}

/// "Some Anime" with TMDB's own order in two seasons, of 24 and 35, following its "Seasons"
/// group of 24, 23 and 12.
async fn in_two_tmdb_seasons_following_the_group() -> SqlitePool {
    let pool = some_anime().await;
    sqlx::query(
        "UPDATE tmdb_episodes SET season = 2, episode = episode - 24
         WHERE series_tmdb_id = 100 AND season = 1 AND episode > 24",
    )
    .execute(&pool)
    .await
    .unwrap();
    add_group(&pool, "aaaa", "Seasons", &[24, 23, 12]).await;
    assert_eq!(decide(&pool).await.unwrap(), 1);
    sqlx::query("UPDATE wanted_seasons SET monitored = 0")
        .execute(&pool)
        .await
        .unwrap();
    pool
}

async fn monitored_seasons(pool: &SqlitePool, wanted: i64) -> Vec<i64> {
    sqlx::query_scalar(
        "SELECT season FROM wanted_seasons WHERE wanted_series_id = ? AND monitored = 1
         ORDER BY season",
    )
    .bind(wanted)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_season_asked_for_by_tmdbs_number_monitors_its_episodes_in_the_group() {
    let pool = in_two_tmdb_seasons_following_the_group().await;
    // TMDB's season 2 is the group's seasons 2 and 3.
    crate::want::monitor_asked(&pool, 1, crate::want::Seasons::Only(2))
        .await
        .unwrap();
    assert_eq!(monitored_seasons(&pool, 1).await, vec![2, 3]);
    // TMDB's season 1 with a group made of other episodes: the episodes not asked for stay out.
    let pool = some_anime().await;
    add_group(&pool, "bbbb", "Halves", &[30, 29]).await;
    sqlx::query(
        "UPDATE tmdb_episodes SET season = 2, episode = episode - 24
         WHERE series_tmdb_id = 100 AND season = 1 AND episode > 24;
         INSERT INTO series_orders (series_tmdb_id, group_id, chosen_by, chosen_at)
             VALUES (100, 'bbbb', 'auto', 0);
         UPDATE wanted_seasons SET monitored = 0;",
    )
    .execute(&pool)
    .await
    .unwrap();
    crate::want::monitor_asked(&pool, 1, crate::want::Seasons::Only(1))
        .await
        .unwrap();
    assert_eq!(monitored_seasons(&pool, 1).await, vec![1]);
    let off: Vec<i64> = sqlx::query_scalar(
        "SELECT episode FROM wanted_episode_overrides
         WHERE wanted_series_id = 1 AND season = 1 AND monitored = 0 ORDER BY episode",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(off, (25..=30).collect::<Vec<_>>());
}

#[tokio::test]
async fn a_series_wanted_while_it_follows_a_group_gets_the_groups_seasons() {
    let pool = in_two_tmdb_seasons_following_the_group().await;
    sqlx::query("DELETE FROM wanted_series")
        .execute(&pool)
        .await
        .unwrap();
    let fake = FakeTmdb::default();
    fake.reply(
        "/tv/100?append_to_response=external_ids",
        vec![Reply::ok(json!({
            "id": 100, "name": "Some Anime", "first_air_date": "2020-10-03",
            "external_ids": {"tvdb_id": 200},
            "seasons": [{"season_number": 0}, {"season_number": 1}, {"season_number": 2}]
        }))],
    );
    let tmdb = fake.start().await;
    let placement = |seasons| crate::want::Placement {
        monitored: true,
        quality_profile: None,
        profile_id: None,
        library_id: 1,
        seasons,
        list_id: None,
        keep_removals: false,
    };
    let id = match crate::want::series(&pool, &tmdb, 100, &placement(crate::want::Seasons::Only(2)))
        .await
        .unwrap()
    {
        crate::want::Wanted::Added(id) => id,
        other => panic!("{other:?}"),
    };
    let listed: Vec<i64> = sqlx::query_scalar(
        "SELECT season FROM wanted_seasons WHERE wanted_series_id = ? ORDER BY season",
    )
    .bind(id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(listed, vec![0, 1, 2, 3], "the group's seasons, not TMDB's");
    assert_eq!(monitored_seasons(&pool, id).await, vec![2, 3]);
}

#[tokio::test]
async fn a_file_numbered_from_the_first_with_no_place_in_the_new_order_loses_its_number() {
    let pool = some_anime().await;
    add_group(&pool, "aaaa", "Short", &[24, 23]).await;
    sqlx::query(
        "INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id,
             absolute_episode, first_seen_generation, seen_generation)
         VALUES (1, 'Some Anime/Some Anime - 55.mkv', 1, 0, 1, 55, 1, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    crate::library::absolute::number(&pool).await.unwrap();
    choose(&pool, 100, Some("aaaa"), ChosenBy::Auto, 50)
        .await
        .unwrap();
    // The group has 47: S01E55's number would be no episode, or another one later.
    let numbered: (Option<i64>, Option<i64>, i64) = sqlx::query_as(
        "SELECT season, episode, mapped FROM media_files WHERE absolute_episode = 55",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(numbered, (None, None, 0));
}
