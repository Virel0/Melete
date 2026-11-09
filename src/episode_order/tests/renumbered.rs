//! A switch that keeps a season's episodes but numbers them differently, and what's kept by
//! number while it would.

use super::switching::download;
use super::*;

/// A group with TMDB's one season, its fifth and sixth episodes the other way round.
async fn add_group_swapping_five_and_six(pool: &SqlitePool) {
    sqlx::query(
        "INSERT INTO tmdb_episode_groups (id, series_tmdb_id, name, kind, fetched_at)
             VALUES ('ssss', 100, 'Swapped', 6, 10);
         INSERT INTO tmdb_episode_group_episodes (group_id, season, episode, episode_tmdb_id)
             SELECT 'ssss', 1, CASE episode WHEN 5 THEN 6 WHEN 6 THEN 5 ELSE episode END, tmdb_id
             FROM tmdb_episodes WHERE series_tmdb_id = 100 AND season = 1",
    )
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_switch_waits_for_what_is_kept_by_number_within_a_season() {
    let pool = some_anime().await;
    add_group_swapping_five_and_six(&pool).await;
    // A pack of season 1 taken for its fifth episode only, on its way: renumbered, it would
    // bring home the sixth instead.
    download(&pool, 'a', 1, None, "downloading").await;
    sqlx::query("UPDATE downloads SET episodes = '[5]'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        choose(&pool, 100, Some("ssss"), ChosenBy::Auto, 50)
            .await
            .unwrap(),
        Chosen::Busy((1, 0))
    );
    assert_eq!(current(&pool, 100).await.unwrap().group_id, None);
    // An upgrade of the fifth episode, in the library with its old file still to go: that file
    // is kept by number too.
    sqlx::query(
        "UPDATE downloads SET state = 'imported', episodes = NULL, episode = 5,
             replaces = '[{\"file_id\": 5, \"library_id\": 1, \"rel_path\": \"x\", \"size\": 1,
                 \"modified_at\": 0, \"season\": 1, \"episode\": 5, \"episode_end\": null}]'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        choose(&pool, 100, Some("ssss"), ChosenBy::Auto, 50)
            .await
            .unwrap(),
        Chosen::Busy((1, 5))
    );
    // Once its old file is dealt with, the switch goes ahead.
    sqlx::query("UPDATE downloads SET retired_at = 60")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        choose(&pool, 100, Some("ssss"), ChosenBy::Auto, 50)
            .await
            .unwrap(),
        Chosen::Done
    );
}

/// Two groups of two seasons of twelve, the second's first season with TMDB's 30th episode
/// added as its 13th: its second season is the same, but counted from the first episode it
/// starts one later.
async fn add_groups_one_adding_an_episode_to_season_one(pool: &SqlitePool) {
    sqlx::query(
        "INSERT INTO tmdb_episode_groups (id, series_tmdb_id, name, kind, fetched_at)
             VALUES ('g1', 100, 'Two seasons', 6, 10), ('g2', 100, 'Two seasons and one', 6, 10);
         INSERT INTO tmdb_episode_group_episodes (group_id, season, episode, episode_tmdb_id)
             SELECT g.id, CASE WHEN e.episode <= 12 THEN 1 ELSE 2 END,
                    CASE WHEN e.episode <= 12 THEN e.episode ELSE e.episode - 12 END, e.tmdb_id
             FROM tmdb_episodes e, (SELECT 'g1' AS id UNION ALL SELECT 'g2') g
             WHERE e.series_tmdb_id = 100 AND e.season = 1 AND e.episode <= 24;
         INSERT INTO tmdb_episode_group_episodes (group_id, season, episode, episode_tmdb_id)
             VALUES ('g2', 1, 13, 1030)",
    )
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_switch_shifting_what_absolute_numbers_count_to_waits_for_later_seasons() {
    let pool = some_anime().await;
    add_groups_one_adding_an_episode_to_season_one(&pool).await;
    assert_eq!(
        choose(&pool, 100, Some("g1"), ChosenBy::Admin, 50)
            .await
            .unwrap(),
        Chosen::Done
    );
    // The 16th episode on disk is S02E04, and an upgrade of it is on its way, its old file kept
    // by that number.
    sqlx::query(
        "INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, season,
             episode, absolute_episode, mapped, first_seen_generation, seen_generation)
         VALUES (1, 'Some Anime/Some Anime - 16.mkv', 1, 0, 1, 2, 4, 16, 1, 1, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    download(&pool, 'a', 2, Some(4), "downloading").await;
    sqlx::query(
        "UPDATE downloads SET replaces = '[{\"file_id\": 1, \"library_id\": 1, \"rel_path\": \"x\",
             \"size\": 1, \"modified_at\": 0, \"season\": 2, \"episode\": 4, \"episode_end\": null}]'",
    )
    .execute(&pool)
    .await
    .unwrap();
    // Season 2 keeps its episodes and their numbers, but the 16th would be S02E03.
    assert_eq!(
        choose(&pool, 100, Some("g2"), ChosenBy::Admin, 60)
            .await
            .unwrap(),
        Chosen::Busy((2, 4))
    );
}
