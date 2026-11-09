use serde_json::json;

use super::*;
use crate::tmdb::testing::{FakeTmdb, Reply};

/// "Some Anime" as TMDB lists it by default: one season of 59 episodes (TMDB ids 1001 to 1059)
/// and a special; its folder holds files named S01 (24), S02 (23) and S03 (12), as other sites
/// number it; and it's wanted, season 1 monitored. Returns the pool.
async fn some_anime() -> SqlitePool {
    let pool = crate::db::open_in_memory().await;
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'anime', 'shows', '/a');
         INSERT INTO tmdb_series (tmdb_id, tvdb_id, name, fetched_at) VALUES (100, 200, 'Some Anime', 10);
         INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, name)
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 59)
             SELECT 100, 1, i, 1000 + i, 'Episode ' || i FROM n;
         INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, name)
             VALUES (100, 0, 1, 1900, 'A special');
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation, match_tmdb_id)
             VALUES (1, 1, 'Some Anime', 'Some Anime', 1, 100);
         INSERT INTO wanted_series (id, tvdb_id, tmdb_id, title, monitored, series_type, source,
                 added_at, series_id)
             VALUES (1, 200, 100, 'Some Anime', 1, 'anime', 'manual', 0, 1);
         INSERT INTO wanted_seasons (wanted_series_id, season, monitored) VALUES (1, 0, 0), (1, 1, 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    for (season, count) in [(1, 24), (2, 23), (3, 12)] {
        for episode in 1..=count {
            add_file(
                &pool,
                &format!("Some Anime - S{season:02}E{episode:02}.mkv"),
                season,
                episode,
            )
            .await;
        }
    }
    pool
}

async fn add_file(pool: &SqlitePool, name: &str, season: i64, episode: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, season,
             episode, first_seen_generation, seen_generation)
         VALUES (1, 'Some Anime/' || ?, 1, 0, 1, ?, ?, 1, 1) RETURNING id",
    )
    .bind(name)
    .bind(season)
    .bind(episode)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Keeps a group of `parts` episode counts over TMDB's episodes in order, from 1001.
async fn add_group(pool: &SqlitePool, id: &str, name: &str, parts: &[i64]) {
    sqlx::query(
        "INSERT INTO tmdb_episode_groups (id, series_tmdb_id, name, kind, fetched_at)
         VALUES (?, 100, ?, 6, 10)",
    )
    .bind(id)
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
    let mut next = 1001;
    for (i, count) in parts.iter().enumerate() {
        for episode in 1..=*count {
            sqlx::query(
                "INSERT INTO tmdb_episode_group_episodes (group_id, season, episode, episode_tmdb_id)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(id)
            .bind(i as i64 + 1)
            .bind(episode)
            .bind(next)
            .execute(pool)
            .await
            .unwrap();
            next += 1;
        }
    }
}

/// The TMDB episode numbered (season, episode) in the order the series follows.
async fn episode_at(pool: &SqlitePool, season: i64, episode: i64) -> Option<(i64, String)> {
    sqlx::query_as(
        "SELECT tmdb_id, name FROM episodes_in_order
         WHERE series_tmdb_id = 100 AND season = ? AND episode = ?",
    )
    .bind(season)
    .bind(episode)
    .fetch_optional(pool)
    .await
    .unwrap()
}

async fn events(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT message FROM title_events WHERE kind = 'reordered' AND key = '200' ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_series_switches_to_the_one_group_its_files_fit_exactly() {
    let pool = some_anime().await;
    add_group(&pool, "aaaa", "Seasons", &[24, 23, 12]).await;
    add_group(&pool, "bbbb", "Story arcs", &[24, 23, 9, 3]).await;
    // Under TMDB's own order the second season's files are no episode TMDB has.
    assert_eq!(episode_at(&pool, 2, 1).await, None);
    assert_eq!(decide(&pool).await.unwrap(), 1);
    let order = current(&pool, 100).await.unwrap();
    assert_eq!(order.group_id.as_deref(), Some("aaaa"));
    assert_eq!(order.name.as_deref(), Some("Seasons"));
    assert_eq!(order.chosen_by.as_deref(), Some("auto"));
    // The file named S02E01 is TMDB's 25th episode, with its title.
    assert_eq!(
        episode_at(&pool, 2, 1).await,
        Some((1025, "Episode 25".into()))
    );
    assert_eq!(episode_at(&pool, 1, 25).await, None);
    // Specials the group leaves out stay as they are.
    assert_eq!(
        episode_at(&pool, 0, 1).await,
        Some((1900, "A special".into()))
    );
    let said = events(&pool).await;
    assert_eq!(said.len(), 1);
    assert!(said[0].contains("\u{201c}Seasons\u{201d}") && said[0].contains("fit it exactly"));
    // Chosen once: nothing changes on the next run, and nothing is suggested.
    assert_eq!(decide(&pool).await.unwrap(), 0);
    assert_eq!(suggestion(&pool, 100).await.unwrap(), None);
}

#[tokio::test]
async fn without_an_exact_fit_nothing_switches_and_the_closest_is_suggested() {
    let pool = some_anime().await;
    sqlx::query("UPDATE media_files SET missing_since = 5 WHERE season = 2 AND episode = 23")
        .execute(&pool)
        .await
        .unwrap();
    add_group(&pool, "aaaa", "Seasons", &[24, 23, 12]).await;
    add_group(&pool, "bbbb", "Story arcs", &[24, 23, 9, 3]).await;
    assert_eq!(decide(&pool).await.unwrap(), 0);
    assert_eq!(current(&pool, 100).await.unwrap().chosen_by, None);
    let suggested = suggestion(&pool, 100).await.unwrap().unwrap();
    assert_eq!(suggested.group_id, "aaaa");
    assert!(!suggested.exact);
    assert_eq!(suggested.covered, 58);
    // Two groups fitting exactly is no fit either.
    let pool = some_anime().await;
    add_group(&pool, "aaaa", "Seasons", &[24, 23, 12]).await;
    add_group(&pool, "cccc", "Seasons (again)", &[24, 23, 12]).await;
    assert_eq!(decide(&pool).await.unwrap(), 0);
    assert!(suggestion(&pool, 100).await.unwrap().unwrap().exact);
    // Files TMDB's own order has: no other order is looked for.
    let pool = some_anime().await;
    sqlx::query("DELETE FROM media_files WHERE season > 1")
        .execute(&pool)
        .await
        .unwrap();
    add_group(&pool, "dddd", "Seasons", &[24]).await;
    assert_eq!(decide(&pool).await.unwrap(), 0);
    assert_eq!(suggestion(&pool, 100).await.unwrap(), None);
}

#[tokio::test]
async fn a_switch_keeps_monitoring_downloads_searches_history_and_progress_on_their_episodes() {
    let pool = some_anime().await;
    add_group(&pool, "aaaa", "Seasons", &[24, 23, 12]).await;
    let file = add_file(&pool, "Some Anime - S02E06 (v2).mkv", 2, 6).await;
    let user = crate::auth::testing::user(&pool, "alice", false).await;
    // Under TMDB's own order: S01E30 (the group's S02E06) left out, S01E40 (S02E16) grabbed,
    // searched for and in the history; alice is partway through the file named S02E06.
    sqlx::query(
        "INSERT INTO wanted_episode_overrides (wanted_series_id, season, episode, monitored)
             VALUES (1, 1, 30, 0);
         INSERT INTO downloads (infohash, title, wanted_series_id, season, episode, decision,
                 state, added_at, updated_at)
             VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'Some Anime - 40', 1, 1, 40, '{}',
                 'downloading', unixepoch(), 1),
                    ('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'Some Anime S01', 1, 1, NULL, '{}',
                 'imported', unixepoch(), 1);
         INSERT INTO searches (wanted_series_id, season, episode, searched_at, indexers)
             VALUES (1, 1, 40, unixepoch(), '[]');",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO playback_progress (user_id, file_id, position_seconds, watched, updated_at)
         VALUES (?, ?, 300, 0, 1)",
    )
    .bind(user.id)
    .bind(file)
    .execute(&pool)
    .await
    .unwrap();
    let monitored = || async {
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT e.season, e.episode FROM episodes_in_order e
             JOIN wanted_seasons ws ON ws.wanted_series_id = 1 AND ws.season = e.season
             LEFT JOIN wanted_episode_overrides o
                 ON o.wanted_series_id = 1 AND o.season = e.season AND o.episode = e.episode
             WHERE e.series_tmdb_id = 100 AND ws.monitored = 1 AND coalesce(o.monitored, 1) = 1
             ORDER BY e.season, e.episode",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        rows
    };
    let numbered = |table: &'static str| {
        let pool = pool.clone();
        async move {
            let rows: Vec<(Option<i64>, Option<i64>)> = sqlx::query_as(sqlx::AssertSqlSafe(
                format!("SELECT season, episode FROM {table} ORDER BY id"),
            ))
            .fetch_all(&pool)
            .await
            .unwrap();
            rows
        }
    };
    let before = monitored().await;
    assert_eq!(before.len(), 58);
    assert!(!before.contains(&(1, 30)));

    assert_eq!(decide(&pool).await.unwrap(), 1);
    let after = monitored().await;
    assert_eq!(after.len(), 58, "every episode monitored as it was");
    assert!(!after.contains(&(2, 6)) && after.contains(&(2, 7)) && after.contains(&(3, 12)));
    assert_eq!(
        numbered("downloads").await,
        vec![(Some(2), Some(16)), (Some(1), None)],
        "a season pack keeps its season"
    );
    assert_eq!(numbered("searches").await, vec![(Some(2), Some(16))]);
    // History, as the grabs and the search wrote it.
    let history: Vec<(String, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT kind, season, episode FROM title_events WHERE kind <> 'reordered' ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let kinds = |k: &str| {
        history
            .iter()
            .filter(|h| h.0 == k)
            .map(|h| (h.1, h.2))
            .collect::<Vec<_>>()
    };
    assert_eq!(kinds("grabbed"), vec![(Some(2), Some(16)), (Some(1), None)]);
    assert_eq!(kinds("searched"), vec![(Some(2), Some(16))]);
    // Files keep their names and ids, and so their progress.
    let progress: (i64, f64, String) = sqlx::query_as(
        "SELECT p.file_id, p.position_seconds, f.rel_path FROM playback_progress p
         JOIN media_files f ON f.id = p.file_id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        progress,
        (
            file,
            300.0,
            "Some Anime/Some Anime - S02E06 (v2).mkv".into()
        )
    );
}

#[tokio::test]
async fn an_admins_choice_and_its_undoing_stand() {
    let pool = some_anime().await;
    add_group(&pool, "aaaa", "Seasons", &[24, 23, 12]).await;
    add_group(&pool, "bbbb", "Story arcs", &[24, 23, 9, 3]).await;
    assert_eq!(
        choose(&pool, 100, Some("bbbb"), ChosenBy::Admin, 50)
            .await
            .unwrap(),
        Chosen::Done
    );
    assert_eq!(episode_at(&pool, 4, 1).await.unwrap().0, 1057);
    // Melete never overrides it, though another group fits the files exactly.
    assert_eq!(decide(&pool).await.unwrap(), 0);
    assert_eq!(
        choose(&pool, 100, Some("aaaa"), ChosenBy::Auto, 60)
            .await
            .unwrap(),
        Chosen::EarlierStands
    );
    assert_eq!(
        current(&pool, 100).await.unwrap().group_id.as_deref(),
        Some("bbbb")
    );
    // Back to TMDB's own order, by hand: that stands too.
    assert_eq!(
        choose(&pool, 100, None, ChosenBy::Admin, 70).await.unwrap(),
        Chosen::Done
    );
    let order = current(&pool, 100).await.unwrap();
    assert_eq!(
        (order.group_id, order.chosen_by.as_deref()),
        (None, Some("admin"))
    );
    assert_eq!(episode_at(&pool, 1, 59).await.unwrap().0, 1059);
    assert_eq!(decide(&pool).await.unwrap(), 0);
    assert_eq!(suggestion(&pool, 100).await.unwrap(), None);
    let said = events(&pool).await;
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(said[1].contains("TMDB's own order") && said[1].contains("an admin chose it"));
    // A group the series doesn't have isn't chosen.
    assert_eq!(
        choose(&pool, 100, Some("zzzz"), ChosenBy::Admin, 80)
            .await
            .unwrap(),
        Chosen::NoSuchGroup
    );
}

#[tokio::test]
async fn switching_back_by_hand_puts_everything_where_it_was() {
    let pool = some_anime().await;
    add_group(&pool, "aaaa", "Seasons", &[24, 23, 12]).await;
    sqlx::query(
        "INSERT INTO wanted_episode_overrides (wanted_series_id, season, episode, monitored)
             VALUES (1, 1, 30, 0);
         INSERT INTO downloads (infohash, title, wanted_series_id, season, episode, decision,
                 state, added_at, updated_at)
             VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'Some Anime - 40', 1, 1, 40, '{}',
                 'downloading', unixepoch(), 1);
         INSERT INTO searches (wanted_series_id, season, episode, searched_at, indexers)
             VALUES (1, 1, 40, unixepoch(), '[]');",
    )
    .execute(&pool)
    .await
    .unwrap();
    let state = || async {
        let off: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT season, episode FROM wanted_episode_overrides WHERE monitored = 0",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let grabbed: (i64, i64) = sqlx::query_as("SELECT season, episode FROM downloads")
            .fetch_one(&pool)
            .await
            .unwrap();
        let searched: (i64, i64) = sqlx::query_as("SELECT season, episode FROM searches")
            .fetch_one(&pool)
            .await
            .unwrap();
        (off, grabbed, searched)
    };
    assert_eq!(decide(&pool).await.unwrap(), 1);
    assert_eq!(state().await, (vec![(2, 6)], (2, 16), (2, 16)));
    choose(&pool, 100, None, ChosenBy::Admin, 90).await.unwrap();
    assert_eq!(state().await, (vec![(1, 30)], (1, 40), (1, 40)));
    assert_eq!(episode_at(&pool, 2, 1).await, None);
}

#[tokio::test]
async fn files_numbered_from_the_first_episode_count_through_the_group() {
    let pool = some_anime().await;
    add_group(&pool, "aaaa", "Seasons", &[24, 23, 12]).await;
    let file: i64 = sqlx::query_scalar(
        "INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id,
             absolute_episode, first_seen_generation, seen_generation)
         VALUES (1, 'Some Anime/[Group] Some Anime - 30 [1080p].mkv', 1, 0, 1, 30, 1, 1)
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let numbers = || async {
        let row: (Option<i64>, Option<i64>) =
            sqlx::query_as("SELECT season, episode FROM media_files WHERE id = ?")
                .bind(file)
                .fetch_one(&pool)
                .await
                .unwrap();
        row
    };
    crate::library::absolute::number(&pool).await.unwrap();
    assert_eq!(numbers().await, (Some(1), Some(30)));
    // The switch counts it again: 24 in season 1, so the 30th is season 2's 6th.
    assert_eq!(decide(&pool).await.unwrap(), 1);
    assert_eq!(numbers().await, (Some(2), Some(6)));
}

fn group_reply(id: &str, parts: &[(&str, std::ops::RangeInclusive<i64>)]) -> Reply {
    let groups: Vec<_> = parts
        .iter()
        .enumerate()
        .map(|(i, (name, ids))| {
            json!({
                "name": name,
                "order": i,
                "episodes": ids.clone().enumerate()
                    .map(|(n, id)| json!({"id": id, "order": n}))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    Reply::ok(json!({"id": id, "name": "Seasons", "type": 6, "groups": groups}))
}

#[tokio::test]
async fn groups_are_fetched_only_when_needed_and_again_with_the_metadata() {
    let pool = some_anime().await;
    // A second series whose files TMDB's own order fits: its groups aren't asked for.
    sqlx::query(
        "INSERT INTO tmdb_series (tmdb_id, name, fetched_at) VALUES (300, 'Another Show', 10);
         INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id) VALUES (300, 1, 1, 3001);
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation, match_tmdb_id)
             VALUES (2, 1, 'Another Show', 'Another Show', 1, 300);
         INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, season,
                 episode, first_seen_generation, seen_generation)
             VALUES (1, 'Another Show/S01E01.mkv', 1, 0, 2, 1, 1, 1, 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let fake = FakeTmdb::default();
    fake.reply(
        "/tv/100/episode_groups",
        vec![Reply::ok(json!({"results": [
            {"id": "aaaa", "name": "Seasons", "type": 6},
            {"id": "bbbb", "name": "Story arcs", "type": 5}
        ]}))],
    );
    fake.reply(
        "/tv/episode_group/aaaa",
        vec![group_reply(
            "aaaa",
            &[
                ("Season 1", 1001..=1024),
                ("Season 2", 1025..=1047),
                ("Season 3", 1048..=1059),
            ],
        )],
    );
    fake.reply(
        "/tv/episode_group/bbbb",
        vec![group_reply("bbbb", &[("Part 1", 1001..=1030)])],
    );
    let tmdb = fake.start().await;
    assert_eq!(refresh(&pool, &tmdb).await.unwrap(), 1);
    assert_eq!(
        *fake.log.lock().unwrap(),
        vec![
            "/tv/100/episode_groups",
            "/tv/episode_group/aaaa",
            "/tv/episode_group/bbbb"
        ]
    );
    let kept: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT g.id, g.name, count(*) FROM tmdb_episode_groups g
         JOIN tmdb_episode_group_episodes e ON e.group_id = g.id GROUP BY g.id ORDER BY g.id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        kept,
        vec![
            ("aaaa".into(), "Seasons".into(), 59),
            ("bbbb".into(), "Seasons".into(), 30)
        ]
    );
    // Not again until the series' metadata is newer.
    assert_eq!(refresh(&pool, &tmdb).await.unwrap(), 0);
    assert_eq!(decide(&pool).await.unwrap(), 1);
    // TMDB stops listing the group the series follows, and the other: the one followed is kept.
    fake.reply(
        "/tv/100/episode_groups",
        vec![Reply::ok(json!({"results": []}))],
    );
    sqlx::query("UPDATE tmdb_series SET fetched_at = unixepoch() + 100 WHERE tmdb_id = 100")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(refresh(&pool, &tmdb).await.unwrap(), 1);
    let kept: Vec<String> = sqlx::query_scalar("SELECT id FROM tmdb_episode_groups")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(kept, vec!["aaaa"]);
    assert_eq!(episode_at(&pool, 3, 12).await.unwrap().0, 1059);
}

mod by_hand;
mod renumbered;
mod switching;
