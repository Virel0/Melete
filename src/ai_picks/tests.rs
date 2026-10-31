use std::time::Duration;

use serde_json::{Value, json};

use super::*;
use crate::ai::fake;

/// Two movie libraries, the second for grown-ups. In the first: Watched (matched, Drama),
/// Calm (Drama, rated 6), Loud (Action, rated 16) and Unmatched; in the second: Late (Drama).
/// Users: Zebediah, who sees everything, and Quillon, who sees the first library up to 12.
async fn library() -> (SqlitePool, i64, Access, i64, Access) {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        r#"INSERT INTO libraries (id, name, kind, path) VALUES
             (1, 'films', 'movies', '/srv/household-films'), (2, 'late', 'movies', '/srv/late');
         INSERT INTO tmdb_movies (tmdb_id, title, year, overview, genres, fetched_at) VALUES
             (1, 'Watched', 1990, 'Someone watched this one already.', '["Drama"]', 0),
             (2, 'Calm', 2001, 'A quiet story about a lighthouse keeper and the sea, told over one long winter.',
                 '["Drama", "Family", "History", "Romance"]', 0),
             (3, 'Loud', 2002, 'Explosions.', '["Action"]', 0),
             (5, 'Late', 2004, 'After dark.', '["Drama"]', 0);
         INSERT INTO tmdb_ratings (tv, tmdb_id, certifications, age, fetched_at) VALUES
             (0, 1, '{}', 6, 0), (0, 2, '{}', 6, 0), (0, 3, '{}', 16, 0), (0, 5, '{}', 6, 0);
         INSERT INTO movies (id, library_id, rel_path, parsed_title, parsed_year, seen_generation,
             match_status, match_tmdb_id) VALUES
             (1, 1, 'Watched (1990)', 'Watched', 1990, 1, 'matched', 1),
             (2, 1, 'Calm (2001)', 'Calm', 2001, 1, 'matched', 2),
             (3, 1, 'Loud (2002)', 'Loud', 2002, 1, 'matched', 3),
             (4, 1, 'Unmatched (2003)', 'Unmatched', 2003, 1, NULL, NULL),
             (5, 2, 'Late (2004)', 'Late', 2004, 1, 'matched', 5);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, added_at, movie_id,
             first_seen_generation, seen_generation) VALUES
             (1, 1, 'Watched (1990)/w.mkv', 1, 1, 1, 1, 1, 1),
             (2, 1, 'Calm (2001)/c.mkv', 1, 1, 2, 2, 1, 1),
             (3, 1, 'Loud (2002)/l.mkv', 1, 1, 3, 3, 1, 1),
             (4, 1, 'Unmatched (2003)/u.mkv', 1, 1, 4, 4, 1, 1),
             (5, 2, 'Late (2004)/l.mkv', 1, 1, 5, 5, 1, 1);"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut users = Vec::new();
    for name in ["Zebediah", "Quillon"] {
        let password = crate::auth::generate_password().unwrap();
        let user = crate::auth::create_user(&pool, name, password, false)
            .await
            .unwrap()
            .unwrap();
        // Each watched Watched.
        sqlx::query(
            "INSERT INTO playback_progress (user_id, file_id, position_seconds, watched,
                 play_count, updated_at) VALUES (?, 1, 0, 1, 1, 10)",
        )
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();
        users.push(user.id);
    }
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO user_access (user_id, all_libraries, max_age, updated_at)
             VALUES ({0}, 0, 12, 0);
         INSERT INTO user_libraries (user_id, library_id) VALUES ({0}, 1);",
        users[1]
    )))
    .execute(&pool)
    .await
    .unwrap();
    let child = Access::of(&pool, users[1], false).await.unwrap();
    (pool, users[0], Access::everything(), users[1], child)
}

async fn turn_on(pool: &SqlitePool) {
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_AI_PICKS', 'on', 0)",
    )
    .execute(pool)
    .await
    .unwrap();
}

/// What the model was told in the `n`th request.
fn told(fake: &fake::Fake, n: usize) -> (Value, Value) {
    let seen = fake.seen.lock().unwrap();
    let body = seen[n].0.clone();
    let content = body["messages"][1]["content"].as_str().unwrap().to_owned();
    (body, serde_json::from_str(&content).unwrap())
}

fn titles_told(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn off_by_default_nothing_is_sent() {
    let (pool, me, everything, _, _) = library().await;
    let (fake, api) = fake::start().await;
    let client = Arc::new(fake::client(&api));
    let served = for_viewer(&pool, Some(&client), me, &everything)
        .await
        .unwrap();
    assert_eq!(served, Served::none(Status::Off));
    let title = Title {
        series: false,
        id: 1,
    };
    assert_eq!(
        like(&pool, Some(&client), title, me, &everything)
            .await
            .unwrap(),
        Like::Off
    );
    turn_on(&pool).await;
    let served = for_viewer(&pool, None, me, &everything).await.unwrap();
    assert_eq!(served.status, Status::Unavailable, "no OpenRouter key");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(fake.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn only_titles_genres_years_and_premises_leave_and_only_what_the_viewer_may_see() {
    let (pool, _, _, child, limited) = library().await;
    turn_on(&pool).await;
    let (fake, api) = fake::start().await;
    *fake.content.lock().unwrap() =
        r#"{"picks": [{"id": "c1", "reason": "Quiet like Watched"}]}"#.into();
    let client = fake::client(&api);
    make_for_viewer(&pool, &client, child, &limited)
        .await
        .unwrap();
    let (body, told) = told(&fake, 0);
    assert_eq!(body["provider"]["data_collection"], "deny");
    assert!(body.get("user").is_none(), "nobody is named");
    let sent = body.to_string();
    for private in ["Quillon", "Zebediah", "/srv", "household", "films"] {
        assert!(!sent.contains(private), "{private} left the server");
    }
    assert_eq!(
        told["watched"],
        json!([{ "title": "Watched", "year": 1990, "genres": ["Drama"] }])
    );
    // Loud is rated 16 and Late is in a library they can't see; Unmatched has nothing to tell.
    assert_eq!(titles_told(&told["candidates"]), ["Calm"]);
    assert_eq!(
        told["candidates"][0],
        json!({ "id": "c1", "title": "Calm", "year": 2001,
                "genres": ["Drama", "Family", "History"],
                "premise": "A quiet story about a lighthouse keeper and the sea, told over…" })
    );
    let served = for_viewer(&pool, Some(&Arc::new(client)), child, &limited)
        .await
        .unwrap();
    assert_eq!(served.status, Status::Ready);
    let calm = Title {
        series: false,
        id: 2,
    };
    assert_eq!(
        served.picks,
        [Pick {
            title: calm,
            reason: "Quiet like Watched".into()
        }]
    );
}

/// Waits for the picks made in the background to be kept.
async fn made(pool: &SqlitePool, user: i64) -> Kept {
    for _ in 0..100 {
        if let Some(kept) = viewer_row(pool, user).await.unwrap()
            && (kept.made_at.is_some() || kept.error.is_some())
        {
            return kept;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the picks weren't made");
}

#[tokio::test]
async fn picks_are_made_in_the_background_and_again_when_what_was_watched_changes() {
    let (pool, me, everything, _, _) = library().await;
    turn_on(&pool).await;
    let (fake, api) = fake::start().await;
    *fake.content.lock().unwrap() = r#"{"picks": [{"id": "c2", "reason": "Why"}]}"#.into();
    let client = Arc::new(fake::client(&api));
    let first = for_viewer(&pool, Some(&client), me, &everything)
        .await
        .unwrap();
    assert_eq!(first.status, Status::Working);
    assert!(first.picks.is_empty());
    made(&pool, me).await;
    let served = for_viewer(&pool, Some(&client), me, &everything)
        .await
        .unwrap();
    assert_eq!(served.status, Status::Ready);
    assert_eq!(served.picks.len(), 1);
    assert_eq!(fake.seen.lock().unwrap().len(), 1, "kept, not asked again");

    // Watching what was picked: it's no longer served, and picks are made again from the new
    // history, though not more often than every few minutes.
    let picked = served.picks[0].title.id;
    sqlx::query(
        "INSERT INTO playback_progress (user_id, file_id, position_seconds, watched, play_count,
             updated_at) VALUES (?, ?, 0, 1, 1, 20)",
    )
    .bind(me)
    .bind(picked)
    .execute(&pool)
    .await
    .unwrap();
    *fake.content.lock().unwrap() = "not what was asked for".into();
    let meanwhile = for_viewer(&pool, Some(&client), me, &everything)
        .await
        .unwrap();
    assert!(
        meanwhile.picks.is_empty(),
        "a title watched since is left out"
    );
    assert_eq!(fake.seen.lock().unwrap().len(), 1, "tried a moment ago");
    sqlx::query("UPDATE ai_picks SET tried_at = tried_at - 3600")
        .execute(&pool)
        .await
        .unwrap();
    for_viewer(&pool, Some(&client), me, &everything)
        .await
        .unwrap();
    for _ in 0..100 {
        let kept = viewer_row(&pool, me).await.unwrap().unwrap();
        if kept.error.is_some() {
            // Failing keeps why and the picks from before.
            assert_eq!(kept.picks.len(), 1);
            let served = for_viewer(&pool, Some(&client), me, &everything)
                .await
                .unwrap();
            assert_eq!(served.status, Status::Ready);
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("not made again");
}

#[tokio::test]
async fn a_failure_with_nothing_from_before_says_why_without_the_key() {
    let (pool, me, everything, _, _) = library().await;
    turn_on(&pool).await;
    let (fake, api) = fake::start().await;
    *fake.status.lock().unwrap() = 402;
    let client = Arc::new(fake::client(&api));
    for_viewer(&pool, Some(&client), me, &everything)
        .await
        .unwrap();
    made(&pool, me).await;
    let served = for_viewer(&pool, Some(&client), me, &everything)
        .await
        .unwrap();
    assert_eq!(
        served.status,
        Status::Failed("the OpenRouter account is out of credit".into())
    );
    assert!(served.picks.is_empty());
}

#[tokio::test]
async fn titles_like_one_are_made_once_for_those_who_see_the_same() {
    let (pool, me, everything, child, limited) = library().await;
    turn_on(&pool).await;
    let (fake, api) = fake::start().await;
    *fake.content.lock().unwrap() = r#"{"picks": [{"id": "c1", "reason": "Also quiet"}]}"#.into();
    let client = fake::client(&api);
    let watched = Title {
        series: false,
        id: 1,
    };
    make_like(&pool, &client, watched, &limited).await.unwrap();
    make_like(&pool, &client, watched, &everything)
        .await
        .unwrap();
    let (_, for_child) = told(&fake, 0);
    assert_eq!(
        for_child["title"],
        json!({ "title": "Watched", "year": 1990, "genres": ["Drama"],
                "premise": "Someone watched this one already." })
    );
    assert_eq!(titles_told(&for_child["candidates"]), ["Calm"]);
    let (_, for_all) = told(&fake, 1);
    assert_eq!(
        titles_told(&for_all["candidates"]),
        ["Late", "Calm", "Loud"],
        "most like it first, then newest"
    );
    let client = Arc::new(client);
    let Like::Made(child_sees) = like(&pool, Some(&client), watched, child, &limited)
        .await
        .unwrap()
    else {
        panic!("not made");
    };
    assert_eq!(child_sees[0].title.id, 2);
    // Those who see everything got Late, which the child never sees, nor is it sent for them.
    let Like::Made(all_see) = like(&pool, Some(&client), watched, me, &everything)
        .await
        .unwrap()
    else {
        panic!("not made");
    };
    assert_eq!(all_see[0].title.id, 5);
    assert_eq!(fake.seen.lock().unwrap().len(), 2, "kept a week");
}

#[tokio::test]
async fn a_title_not_matched_on_tmdb_never_leaves_even_when_watched() {
    let (pool, me, everything, _, _) = library().await;
    turn_on(&pool).await;
    // Unmatched's only name is its folder's.
    sqlx::query(
        "INSERT INTO playback_progress (user_id, file_id, position_seconds, watched, play_count,
             updated_at) VALUES (?, 4, 0, 1, 1, 30)",
    )
    .bind(me)
    .execute(&pool)
    .await
    .unwrap();
    let (fake, api) = fake::start().await;
    *fake.content.lock().unwrap() = r#"{"picks": []}"#.into();
    let client = fake::client(&api);
    make_for_viewer(&pool, &client, me, &everything)
        .await
        .unwrap();
    let (body, told) = told(&fake, 0);
    assert_eq!(titles_told(&told["watched"]), ["Watched"]);
    assert!(!body.to_string().contains("Unmatched"), "{body}");
}

/// Asks for titles like `title` until the call started for it is over, as it fails.
async fn tried(pool: &SqlitePool, client: &Arc<Client>, title: Title, user: i64) -> Like {
    sqlx::query("UPDATE ai_similar SET tried_at = 0, error = NULL")
        .execute(pool)
        .await
        .unwrap();
    let answer = like(pool, Some(client), title, user, &Access::everything())
        .await
        .unwrap();
    if answer == Like::Making {
        for _ in 0..200 {
            let kept = similar_row(pool, title, "").await.unwrap().unwrap();
            if kept.error.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // The place is let go just after the failure is kept.
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    answer
}

#[tokio::test]
async fn each_user_starts_at_most_30_calls_an_hour_retries_included() {
    let (pool, me, _, other, _) = library().await;
    turn_on(&pool).await;
    let (fake, api) = fake::start().await;
    *fake.status.lock().unwrap() = 500;
    let client = Arc::new(fake::client(&api));
    let calm = Title {
        series: false,
        id: 2,
    };
    for n in 0..crate::ai::STARTS_PER_HOUR {
        assert_eq!(
            tried(&pool, &client, calm, me).await,
            Like::Making,
            "call {n}"
        );
    }
    assert_eq!(tried(&pool, &client, calm, me).await, Like::Limited);
    assert_eq!(fake.seen.lock().unwrap().len(), crate::ai::STARTS_PER_HOUR);
    // Someone else still may.
    assert_eq!(tried(&pool, &client, calm, other).await, Like::Making);
    assert_eq!(
        fake.seen.lock().unwrap().len(),
        crate::ai::STARTS_PER_HOUR + 1
    );
}

#[tokio::test]
async fn at_most_two_calls_run_at_once_and_the_rest_wait() {
    let (pool, me, everything, _, _) = library().await;
    turn_on(&pool).await;
    let (fake, api) = fake::start().await;
    *fake.content.lock().unwrap() = r#"{"picks": []}"#.into();
    *fake.delay_ms.lock().unwrap() = 500;
    let client = Arc::new(fake::client(&api));
    let title = |id| Title { series: false, id };
    let ask = |id| like(&pool, Some(&client), title(id), me, &everything);
    assert_eq!(ask(1).await.unwrap(), Like::Making);
    assert_eq!(ask(2).await.unwrap(), Like::Making);
    assert_eq!(ask(3).await.unwrap(), Like::Busy, "two are running");
    // A viewer's own picks wait their turn too.
    let served = for_viewer(&pool, Some(&client), me, &everything)
        .await
        .unwrap();
    assert_eq!(served.status, Status::Busy);
    assert!(
        viewer_row(&pool, me).await.unwrap().is_none(),
        "nothing claimed"
    );
    for _ in 0..100 {
        if similar_row(&pool, title(2), "")
            .await
            .unwrap()
            .unwrap()
            .made_at
            .is_some()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        ask(3).await.unwrap(),
        Like::Making,
        "their places are free again"
    );
}
