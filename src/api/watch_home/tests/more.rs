//! The rows beyond the user's own: new episodes, the next film in a collection, highly rated
//! titles not seen yet, and titles to watch again.

use super::*;

/// Besides the main fixture: Film A (603) is in collection 1 with Sequel (604, out
/// 2003-05-15) and Third (605, known only by its year, 2010); Other (606) is in collection 2.
/// Sequel scores 8.0 from 500 votes, Third 9.0 from only 50, Other 7.9 from 800, Romance
/// (607, Romance and Drama) 8.5 from 1,000, and Show (a Drama) 9.0 from 200.
async fn more_fixture() -> Fixture {
    let f = fixture().await;
    sqlx::raw_sql(
        r#"INSERT INTO tmdb_collections (id, name) VALUES (1, 'A Collection'), (2, 'Another');
         UPDATE tmdb_movies SET collection_id = 1, score = 6.0, score_votes = 900
             WHERE tmdb_id = 603;
         UPDATE tmdb_series SET score = 9.0, score_votes = 200 WHERE tmdb_id = 10;
         INSERT INTO tmdb_movies (tmdb_id, title, year, release_date, genres, collection_id,
             score, score_votes, fetched_at) VALUES
             (604, 'Sequel', 2003, '2003-05-15', '["Action"]', 1, 8.0, 500, 0),
             (605, 'Third', 2010, NULL, '["Action"]', 1, 9.0, 50, 0),
             (606, 'Other', 2000, NULL, '["Action"]', 2, 7.9, 800, 0),
             (607, 'Romance', 2016, NULL, '["Romance", "Drama"]', NULL, 8.5, 1000, 0);
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
             match_status, match_tmdb_id) VALUES
             (20, 3, 'Sequel', 'Sequel', 1, 'matched', 604),
             (21, 3, 'Third', 'Third', 1, 'matched', 605),
             (22, 3, 'Other', 'Other', 1, 'matched', 606),
             (23, 3, 'Romance', 'Romance', 1, 'matched', 607);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, added_at,
             movie_id, first_seen_generation, seen_generation) VALUES
             (20, 3, 'Sequel/s.mkv', 1, 1, 20, 20, 1, 1),
             (21, 3, 'Third/t.mkv', 1, 1, 21, 21, 1, 1),
             (22, 3, 'Other/o.mkv', 1, 1, 22, 22, 1, 1),
             (23, 3, 'Romance/r.mkv', 1, 1, 23, 23, 1, 1);"#,
    )
    .execute(&f.pool)
    .await
    .unwrap();
    f
}

/// The `kind:id` of a row's items, or nothing when the row is left out.
fn row_titles(home: &Value, key: &str) -> Vec<String> {
    home["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == key)
        .map(|r| titles(&r["items"]))
        .unwrap_or_default()
}

async fn added_at(f: &Fixture, file: i64, at: i64) {
    sqlx::query("UPDATE media_files SET added_at = ?2 WHERE id = ?1")
        .bind(file)
        .bind(at)
        .execute(&f.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_next_episode_come_in_lately_moves_from_next_up_to_new_episodes() {
    let f = more_fixture().await;
    let (me, token) = user(&f, "viewer").await;
    let now = crate::db::unix_now();
    // S01E02 came in after S01E01 was watched, but long ago.
    progress(&f, me, 1, 0.0, 10).await;
    let home = home_of(&f, &token).await;
    assert_eq!(row_titles(&home, "next_up"), ["series:1"]);
    assert!(row_titles(&home, "new_episodes").is_empty());

    // It came in yesterday: the show moves, keeping its episode to play.
    added_at(&f, 2, now - 86_400).await;
    let home = home_of(&f, &token).await;
    assert_eq!(row_titles(&home, "new_episodes"), ["series:1"]);
    assert!(row_titles(&home, "next_up").is_empty());
    assert_eq!(row(&home, "new_episodes")[0]["episode"]["file_id"], 2);
    // With nothing partway, the user's own rows start with it.
    assert_eq!(keys(&home).first(), Some(&"new_episodes"));

    // Watched since it came in (another episode, say), it's no longer new.
    sqlx::query("UPDATE playback_progress SET updated_at = ?1 WHERE file_id = 1")
        .bind(now)
        .execute(&f.pool)
        .await
        .unwrap();
    let home = home_of(&f, &token).await;
    assert_eq!(row_titles(&home, "next_up"), ["series:1"]);
    assert!(row_titles(&home, "new_episodes").is_empty());
}

#[tokio::test]
async fn the_next_film_in_a_collection_is_its_earliest_not_watched() {
    let f = more_fixture().await;
    let (me, token) = user(&f, "viewer").await;
    let (other, other_token) = user(&f, "other").await;
    progress(&f, me, 6, 0.0, 10).await;
    let home = home_of(&f, &token).await;
    // Sequel's full date comes before Third's year; nothing of the other collection is watched.
    assert_eq!(row_titles(&home, "next_in_collection"), ["movie:20"]);

    progress(&f, me, 20, 0.0, 20).await;
    let home = home_of(&f, &token).await;
    assert_eq!(row_titles(&home, "next_in_collection"), ["movie:21"]);

    // Partway through a film isn't having watched it.
    progress(&f, other, 6, 60.0, 10).await;
    let home = home_of(&f, &other_token).await;
    assert!(row_titles(&home, "next_in_collection").is_empty());

    // A shows page has no collections.
    let (_, shows) = because::home_at(&f, &token, "/api/v1/home?kind=shows").await;
    assert!(row_titles(&shows, "next_in_collection").is_empty());
}

#[tokio::test]
async fn highly_rated_titles_are_in_genres_watched_and_not_started() {
    let f = more_fixture().await;
    let (me, token) = user(&f, "viewer").await;
    progress(&f, me, 6, 0.0, 10).await;
    let home = home_of(&f, &token).await;
    // Action ones scored 7.5 or more from enough votes, best first; Film A is watched, Third
    // has too few votes, Romance and Show share no genre with it.
    assert_eq!(row_titles(&home, "highly_rated"), ["movie:20", "movie:22"]);

    // A Drama started brings Romance in; Show itself is started.
    progress(&f, me, 1, 60.0, 20).await;
    let home = home_of(&f, &token).await;
    assert_eq!(
        row_titles(&home, "highly_rated"),
        ["movie:23", "movie:20", "movie:22"]
    );
    let (_, movies) = because::home_at(&f, &token, "/api/v1/home?kind=shows").await;
    assert!(row_titles(&movies, "highly_rated").is_empty());
}

#[tokio::test]
async fn watch_again_holds_what_was_finished_long_ago() {
    let f = more_fixture().await;
    let (me, token) = user(&f, "viewer").await;
    let now = crate::db::unix_now();
    progress(&f, me, 6, 0.0, 10).await;
    // Watched lately.
    progress(&f, me, 20, 0.0, now).await;
    // Partway, and a show with an episode left (the special doesn't count).
    progress(&f, me, 21, 60.0, 10).await;
    progress(&f, me, 1, 0.0, 10).await;
    progress(&f, me, 3, 0.0, 10).await;
    let home = home_of(&f, &token).await;
    assert_eq!(row_titles(&home, "watch_again"), ["movie:1"]);
    assert_eq!(keys(&home).last(), Some(&"watch_again"));

    // Every episode watched: the show, played more, comes first.
    progress(&f, me, 2, 0.0, 20).await;
    let home = home_of(&f, &token).await;
    assert_eq!(row_titles(&home, "watch_again"), ["series:1", "movie:1"]);

    // A favourite comes first of all.
    sqlx::query("INSERT INTO favourites (user_id, movie_id, added_at) VALUES (?1, 1, 0)")
        .bind(me)
        .execute(&f.pool)
        .await
        .unwrap();
    let home = home_of(&f, &token).await;
    assert_eq!(row_titles(&home, "watch_again"), ["movie:1", "series:1"]);
}
