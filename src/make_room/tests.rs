use super::*;

const NOW: i64 = 1_000 * DAY;
const GB: i64 = 1_000_000_000;

/// A two-hour film of `gb` gigabytes in H.265, never watched, added two years ago.
fn film(gb: i64) -> Facts {
    Facts {
        id: 1,
        title: "Film".into(),
        size: gb * GB,
        seconds: Some(7200.0),
        codec: Some("hevc".into()),
        added: Some(NOW - 730 * DAY),
        ..Facts::default()
    }
}

fn of(f: &Facts) -> Option<(Action, i64, i64, Vec<&'static str>)> {
    suggest(f, NOW, &[]).map(|s| (s.action, s.saves, s.score, s.reasons))
}

#[test]
fn a_film_nobody_watched_or_rated_well_is_suggested_for_deleting() {
    assert_eq!(
        of(&film(8)),
        Some((
            Action::Delete,
            8 * GB,
            50,
            vec!["never_watched", "untouched"]
        ))
    );
    // Low on TMDB too: suggested more strongly.
    let low = Facts {
        tmdb_score: Some(5.1),
        tmdb_votes: Some(400),
        ..film(8)
    };
    assert_eq!(of(&low).unwrap().2, 70);
}

#[test]
fn what_someone_is_watching_has_on_a_list_or_asked_for_is_never_suggested() {
    let protected = Facts {
        protected: true,
        ..film(80)
    };
    assert_eq!(of(&protected), None);
}

#[test]
fn a_title_watched_lately_is_left_alone() {
    let watched = Facts {
        watchers: 1,
        last_watched: Some(NOW - 30 * DAY),
        ..film(8)
    };
    assert_eq!(of(&watched), None);
}

#[test]
fn a_title_worth_keeping_with_a_large_copy_is_swapped_or_compressed_not_deleted() {
    // A 60 GB remux of a well-rated film nobody's seen in two years: swapped for a 1080p
    // release of about 6 GB (two hours at 3 GB an hour).
    let remux = Facts {
        wanted: true,
        watchers: 1,
        last_watched: Some(NOW - 730 * DAY),
        tmdb_score: Some(7.8),
        tmdb_votes: Some(5000),
        ..film(60)
    };
    let (action, saves, _, reasons) = of(&remux).unwrap();
    assert_eq!((action, saves), (Action::Swap, 54 * GB));
    assert!(reasons.contains(&"large_copy"), "{reasons:?}");
    // The same in H.264: compressed, saving about half.
    let h264 = Facts {
        codec: Some("h264".into()),
        ..remux
    };
    let (action, saves, _, reasons) = of(&h264).unwrap();
    assert_eq!((action, saves), (Action::Compress, 30 * GB));
    assert!(reasons.contains(&"compressible"));
    // Not on the wanted list: there's no profile to search with, so it isn't swapped.
    let unwanted = Facts {
        wanted: false,
        codec: Some("hevc".into()),
        ..h264
    };
    assert_eq!(of(&unwanted), None);
}

#[test]
fn a_major_studios_title_is_kept_longer() {
    let indie = Facts {
        companies: Some(r#"["Small Films"]"#.into()),
        ..film(8)
    };
    let studio = Facts {
        companies: Some(r#"["Warner Bros. Pictures"]"#.into()),
        ..film(8)
    };
    assert_eq!(of(&indie).unwrap().2, 50);
    // 15 less, and worth keeping: its small copy isn't worth swapping, so it's left alone.
    assert_eq!(of(&studio), None);
    // An admin's addition counts the same.
    assert_eq!(suggest(&indie, NOW, &["Small Films".into()]), None);
}

#[tokio::test]
async fn suggestions_come_from_the_library_strongest_first_and_protected_ones_left_out() {
    let pool = crate::db::open_in_memory().await;
    let old = unix_now() - 730 * DAY;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO libraries (id, name, kind, path, category) VALUES
             (1, 'movies', 'movies', '/m', NULL), (2, 'anime', 'shows', '/a', 'anime');
         INSERT INTO tmdb_movies (tmdb_id, title, year, runtime_minutes, fetched_at, score,
             score_votes, companies) VALUES
             (11, 'Bad Film', 2001, 120, 0, 4.0, 900, '[\"Small Films\"]'),
             (12, 'Kept Film', 2002, 120, 0, 4.0, 900, '[]'),
             (13, 'Plain Film', 2003, 120, 0, NULL, 0, '[]');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
             match_status, match_tmdb_id) VALUES
             (1, 1, 'Bad', 'Bad', 1, 'matched', 11), (2, 1, 'Kept', 'Kept', 1, 'matched', 12),
             (3, 1, 'Plain', 'Plain', 1, 'matched', 13);
         INSERT INTO series (id, library_id, rel_path, parsed_title, parsed_year,
             seen_generation) VALUES (1, 2, 'Anime', 'Anime', 2010, 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, added_at,
             movie_id, series_id, season, episode, first_seen_generation, seen_generation)
             VALUES (1, 1, 'Bad/b.mkv', 8000000000, 1, {old}, 1, NULL, NULL, NULL, 1, 1),
                 (2, 1, 'Kept/k.mkv', 8000000000, 1, {old}, 2, NULL, NULL, NULL, 1, 1),
                 (3, 1, 'Plain/p.mkv', 8000000000, 1, {old}, 3, NULL, NULL, NULL, 1, 1),
                 (4, 2, 'Anime/S01E01.mkv', 2000000000, 1, {old}, NULL, 1, 1, 1, 1, 1);"
    )))
    .execute(&pool)
    .await
    .unwrap();
    // Someone has the kept film on their list.
    let user = crate::auth::testing::user(&pool, "u", false).await;
    sqlx::query("INSERT INTO favourites (user_id, movie_id, added_at) VALUES (?, 2, 0)")
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();
    let found = suggestions(&pool, &[]).await.unwrap();
    let summary: Vec<(&str, i64, bool, Action, i64)> = found
        .iter()
        .map(|s| (s.kind, s.id, s.anime, s.action, s.score))
        .collect();
    assert_eq!(
        summary,
        [
            ("movie", 1, false, Action::Delete, 70),
            ("movie", 3, false, Action::Delete, 50),
            ("series", 1, true, Action::Delete, 50),
        ]
    );
    assert_eq!(found[0].saves, 8 * GB);
    assert_eq!(found[0].title, "Bad Film");

    // Someone partway through the plain film, and a request for the bad one lately: neither.
    sqlx::query(
        "INSERT INTO playback_progress (user_id, file_id, position_seconds, watched, play_count,
             updated_at) VALUES (?, 3, 600, 0, 0, ?)",
    )
    .bind(user.id)
    .bind(unix_now())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO requests (user_id, kind, tmdb_id, title, state, created_at)
         VALUES (?, 'movie', 11, 'Bad Film', 'requested', ?)",
    )
    .bind(user.id)
    .bind(unix_now() - 10 * DAY)
    .execute(&pool)
    .await
    .unwrap();
    let left: Vec<(&str, i64)> = suggestions(&pool, &[])
        .await
        .unwrap()
        .iter()
        .map(|s| (s.kind, s.id))
        .collect();
    assert_eq!(left, [("series", 1)]);
}
