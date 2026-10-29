use super::*;

/// Libraries 1 (movies) and 2 (shows) and 3 (kids' movies). Movies: 1 rated 12, 2 rated 17,
/// 3 unmatched, 4 matched but with ratings that can't be read, 5 in the kids' library rated
/// 0. Series 1 rated 14. Files: 1 of movie 1, 2 of movie 2, 3 an episode of series 1, 4 of
/// movie 5.
async fn fixture() -> SqlitePool {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m'),
             (2, 'shows', 'shows', '/s'), (3, 'kids', 'movies', '/k');
         INSERT INTO tmdb_movies (tmdb_id, title, fetched_at) VALUES (101, 'A', 0), (102, 'B', 0),
             (104, 'D', 0), (105, 'E', 0);
         INSERT INTO tmdb_series (tmdb_id, name, fetched_at) VALUES (201, 'S', 0);
         INSERT INTO tmdb_ratings (tv, tmdb_id, certifications, age, fetched_at) VALUES
             (0, 101, '{"GB": "12"}', 12, 0), (0, 102, '{"US": "R"}', 17, 0),
             (0, 104, '{"US": "NR"}', NULL, 0), (0, 105, '{"US": "G"}', 0, 0),
             (1, 201, '{"US": "TV-14"}', 14, 0);
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
             match_tmdb_id) VALUES (1, 1, 'a', 'A', 1, 101), (2, 1, 'b', 'B', 1, 102),
             (3, 1, 'c', 'C', 1, NULL), (4, 1, 'd', 'D', 1, 104), (5, 3, 'e', 'E', 1, 105);
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation,
             match_tmdb_id) VALUES (1, 2, 's', 'S', 1, 201);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id, series_id,
             season, episode, first_seen_generation, seen_generation) VALUES
             (1, 1, 'a/a.mkv', 1, 1, 1, NULL, NULL, NULL, 1, 1),
             (2, 1, 'b/b.mkv', 1, 1, 2, NULL, NULL, NULL, 1, 1),
             (3, 2, 's/e.mkv', 1, 1, NULL, 1, 1, 1, 1, 1),
             (4, 3, 'e/e.mkv', 1, 1, 5, NULL, NULL, NULL, 1, 1);"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

async fn seen(pool: &SqlitePool, access: &Access) -> (Vec<i64>, Vec<i64>) {
    let mut movies = Vec::new();
    for id in 1..=5 {
        if access.sees_movie(pool, id).await.unwrap() {
            movies.push(id);
        }
    }
    let series = if access.sees_series(pool, 1).await.unwrap() {
        vec![1]
    } else {
        vec![]
    };
    (movies, series)
}

#[tokio::test]
async fn everything_is_seen_without_a_restriction() {
    let pool = fixture().await;
    let all = Access::everything();
    assert_eq!(seen(&pool, &all).await, (vec![1, 2, 3, 4, 5], vec![1]));
}

#[tokio::test]
async fn only_the_libraries_given_are_seen() {
    let pool = fixture().await;
    let kids = Access::limited(Some(vec![3]), None);
    assert_eq!(seen(&pool, &kids).await, (vec![5], vec![]));
    let none = Access::limited(Some(vec![]), None);
    assert_eq!(seen(&pool, &none).await, (vec![], vec![]));
}

#[tokio::test]
async fn a_highest_rating_hides_what_is_above_it_or_unrated() {
    let pool = fixture().await;
    let twelve = Access::limited(None, Some(12));
    // Movie 3 has no match and movie 4 a rating that can't be read: both hidden.
    assert_eq!(seen(&pool, &twelve).await, (vec![1, 5], vec![]));
    let fourteen = Access::limited(None, Some(14));
    assert_eq!(seen(&pool, &fourteen).await, (vec![1, 5], vec![1]));
    let zero_in_movies = Access::limited(Some(vec![1, 2]), Some(0));
    assert_eq!(seen(&pool, &zero_in_movies).await, (vec![], vec![]));
}

#[tokio::test]
async fn admins_see_everything_and_restrictions_are_kept_and_forgotten() {
    let pool = fixture().await;
    let user = crate::auth::create_user(&pool, "kid", "a-long-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restriction(&pool, user.id).await.unwrap(), None);
    let kids = Restriction {
        libraries: Some(vec![3]),
        max_age: Some(7),
    };
    set_restriction(&pool, user.id, Some(&kids)).await.unwrap();
    assert_eq!(restriction(&pool, user.id).await.unwrap(), Some(kids));
    assert_eq!(
        Access::of(&pool, user.id, false).await.unwrap(),
        Access::limited(Some(vec![3]), Some(7))
    );
    assert!(Access::of(&pool, user.id, true).await.unwrap() == Access::everything());
    let rating_only = Restriction {
        libraries: None,
        max_age: Some(12),
    };
    set_restriction(&pool, user.id, Some(&rating_only))
        .await
        .unwrap();
    assert_eq!(
        restriction(&pool, user.id).await.unwrap(),
        Some(rating_only)
    );
    let nothing = Restriction {
        libraries: None,
        max_age: None,
    };
    set_restriction(&pool, user.id, Some(&nothing))
        .await
        .unwrap();
    assert_eq!(restriction(&pool, user.id).await.unwrap(), None);
    assert!(Access::of(&pool, user.id, false).await.unwrap() == Access::everything());
}

#[tokio::test]
async fn a_file_goes_by_its_library_and_an_episode_by_its_series_rating() {
    let pool = fixture().await;
    let files = |access: Access| {
        let pool = pool.clone();
        async move {
            let mut seen = Vec::new();
            for id in 1..=4 {
                if access.sees_file(&pool, id).await.unwrap() {
                    seen.push(id);
                }
            }
            seen
        }
    };
    assert_eq!(files(Access::everything()).await, [1, 2, 3, 4]);
    assert_eq!(files(Access::limited(Some(vec![3]), None)).await, [4]);
    assert_eq!(files(Access::limited(None, Some(12))).await, [1, 4]);
    assert_eq!(files(Access::limited(None, Some(14))).await, [1, 3, 4]);
    assert_eq!(
        files(Access::limited(Some(vec![1, 2]), Some(14))).await,
        [1, 3]
    );
}
