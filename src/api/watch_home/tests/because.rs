//! "Because you watched …" rows.

use super::*;
use crate::tmdb::testing::{FakeTmdb, Reply};

/// Besides the main fixture: Film A (603) is in collection 1, with people 1 and 2. Sequel (604)
/// is in that collection too; Heist (605) is Action like it, with person 1, from its decade;
/// Thriller (606) is Action only; Romance (607) shares nothing with it; Seen (608) is Action
/// but watched; Elsewhere (609) is Action in a library of its own. Show (10) is a Drama like
/// Romance.
async fn alike_fixture() -> Fixture {
    let f = fixture().await;
    sqlx::raw_sql(
        r#"INSERT INTO libraries (id, name, kind, path) VALUES (4, 'other', 'movies', '/o');
         INSERT INTO tmdb_collections (id, name) VALUES (1, 'A Collection');
         UPDATE tmdb_movies SET collection_id = 1 WHERE tmdb_id = 603;
         INSERT INTO tmdb_movies (tmdb_id, title, year, genres, collection_id, fetched_at) VALUES
             (604, 'Sequel', 2003, '["Action"]', 1, 0),
             (605, 'Heist', 1998, '["Action", "Crime"]', NULL, 0),
             (606, 'Thriller', 2015, '["Action"]', NULL, 0),
             (607, 'Romance', 2016, '["Romance", "Drama"]', NULL, 0),
             (608, 'Seen', 2017, '["Action"]', NULL, 0),
             (609, 'Elsewhere', 2018, '["Action"]', NULL, 0);
         INSERT INTO tmdb_people (id, name) VALUES (1, 'Person One'), (2, 'Person Two');
         INSERT INTO tmdb_credits (movie_tmdb_id, person_id, role, job, position) VALUES
             (603, 1, 'crew', 'Director', 0), (603, 2, 'cast', NULL, 1),
             (605, 1, 'crew', 'Director', 0);
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
             match_status, match_tmdb_id) VALUES
             (20, 3, 'Sequel', 'Sequel', 1, 'matched', 604),
             (21, 3, 'Heist', 'Heist', 1, 'matched', 605),
             (22, 3, 'Thriller', 'Thriller', 1, 'matched', 606),
             (23, 3, 'Romance', 'Romance', 1, 'matched', 607),
             (24, 3, 'Seen', 'Seen', 1, 'matched', 608),
             (25, 4, 'Elsewhere', 'Elsewhere', 1, 'matched', 609);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, added_at,
             movie_id, first_seen_generation, seen_generation) VALUES
             (20, 3, 'Sequel/s.mkv', 1, 1, 20, 20, 1, 1),
             (21, 3, 'Heist/h.mkv', 1, 1, 21, 21, 1, 1),
             (22, 3, 'Thriller/t.mkv', 1, 1, 22, 22, 1, 1),
             (23, 3, 'Romance/r.mkv', 1, 1, 23, 23, 1, 1),
             (24, 3, 'Seen/s.mkv', 1, 1, 24, 24, 1, 1),
             (25, 4, 'Elsewhere/e.mkv', 1, 1, 25, 25, 1, 1);"#,
    )
    .execute(&f.pool)
    .await
    .unwrap();
    f
}

/// The "because you watched" rows: each seed's `kind:id`, with its items' `kind:id`.
fn because(home: &Value) -> Vec<(String, Vec<String>)> {
    home["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["key"] == "because_you_watched")
        .map(|r| {
            let seed = format!(
                "{}:{}",
                r["seed"]["kind"].as_str().unwrap(),
                r["seed"]["id"]
            );
            (seed, titles(&r["items"]))
        })
        .collect()
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[tokio::test]
async fn rows_hold_titles_like_those_finished_most_alike_first() {
    let f = alike_fixture().await;
    let (me, token) = user(&f, "viewer").await;
    progress(&f, me, 24, 0.0, 5).await;
    progress(&f, me, 6, 0.0, 10).await;
    progress(&f, me, 1, 0.0, 20).await;
    let home = home_of(&f, &token).await;
    assert_eq!(
        because(&home),
        [
            // The series last: an episode counts as its series.
            ("series:1".to_owned(), strings(&["movie:23"])),
            // Its collection, then a genre, a person and the decade, then a genre alone; the
            // watched film, the one sharing nothing and the film itself left out.
            (
                "movie:1".to_owned(),
                strings(&["movie:20", "movie:21", "movie:25", "movie:22"])
            ),
            // The decade orders those sharing as much.
            (
                "movie:24".to_owned(),
                strings(&["movie:25", "movie:22", "movie:20", "movie:21"])
            ),
        ]
    );
    let seed = home["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "because_you_watched")
        .unwrap();
    assert_eq!(
        seed["seed"],
        json!({ "kind": "series", "id": 1, "title": "Show" })
    );
    assert_eq!(seed["items"][0]["title"], "Romance", "items are full cards");
    // After the user's own rows, before the newest titles; the others carry no seed.
    let keys = keys(&home);
    let first = keys
        .iter()
        .position(|k| *k == "because_you_watched")
        .unwrap();
    assert_eq!(keys[first - 1], "next_up");
    assert!(keys[first + 3].starts_with("latest_"));
    assert!(home["rows"][0]["seed"].is_null());
}

#[tokio::test]
async fn only_titles_the_user_may_see_are_there() {
    let f = alike_fixture().await;
    let (me, token) = user(&f, "child").await;
    // The movies library only, and nothing over 12: Sequel is rated 15, Heist 12, and
    // Thriller has no rating Melete can read.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO user_access (user_id, all_libraries, max_age, updated_at) VALUES ({me}, 0, 12, 0);
         INSERT INTO user_libraries (user_id, library_id) VALUES ({me}, 3);
         INSERT INTO tmdb_ratings (tv, tmdb_id, certifications, age, fetched_at) VALUES
             (0, 603, '{{}}', 12, 0), (0, 604, '{{}}', 15, 0), (0, 605, '{{}}', 12, 0),
             (0, 609, '{{}}', 0, 0);"
    )))
    .execute(&f.pool)
    .await
    .unwrap();
    progress(&f, me, 6, 0.0, 10).await;
    let home = home_of(&f, &token).await;
    assert_eq!(
        because(&home),
        [("movie:1".to_owned(), strings(&["movie:21"]))]
    );
    // Not even picked to be dropped later, so they never take the place of one they may see.
    let access = Access::of(&f.pool, me, false).await.unwrap();
    let outside = super::super::because::Outside::default();
    let Ok(rows) = super::super::because::rows(&f.pool, &outside, me, &access).await else {
        panic!("couldn't work the rows out");
    };
    let picked: Vec<TitleRef> = rows[0].1.iter().map(|e| e.title).collect();
    assert_eq!(picked, [TitleRef::movie(21)]);
    // A title they may no longer see isn't a row of its own.
    sqlx::query("UPDATE tmdb_ratings SET age = 16 WHERE tmdb_id = 603")
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(because(&home_of(&f, &token).await).is_empty());
}

/// The home with TMDB, which recommends Romance (607) and a film not in the library (999)
/// with Film A.
async fn with_tmdb(f: &Fixture) -> (Router, FakeTmdb) {
    let fake = FakeTmdb::default();
    let listed = json!({ "results": [
        { "id": 999, "title": "Not Here", "release_date": "2001-01-01" },
        { "id": 607, "title": "Romance", "release_date": "2016-01-01" },
    ] });
    fake.reply("/movie/603/recommendations", vec![Reply::ok(listed)]);
    let client = Arc::new(fake.start().await);
    let app = routes_with(
        f.pool.clone(),
        ArtworkState::stored_only(f.pool.clone()),
        Some(client),
    );
    (app, fake)
}

#[tokio::test]
async fn tmdb_recommends_only_with_trending_and_similar_titles_on() {
    let f = alike_fixture().await;
    let (app, fake) = with_tmdb(&f).await;
    let f = Fixture {
        pool: f.pool.clone(),
        app,
    };
    let (me, token) = user(&f, "viewer").await;
    progress(&f, me, 6, 0.0, 10).await;
    let alike = strings(&["movie:20", "movie:21", "movie:25", "movie:24", "movie:22"]);
    assert_eq!(
        because(&home_of(&f, &token).await),
        [("movie:1".to_owned(), alike)]
    );
    assert!(
        fake.log.lock().unwrap().is_empty(),
        "nothing sent while it's off"
    );

    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_DISCOVERY', 'on', 0)",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let home = home_of(&f, &token).await;
    // Romance, recommended, comes in after the collection; the film not in the library doesn't.
    assert_eq!(
        because(&home),
        [(
            "movie:1".to_owned(),
            strings(&[
                "movie:20", "movie:23", "movie:21", "movie:25", "movie:24", "movie:22"
            ])
        )]
    );
    home_of(&f, &token).await;
    assert_eq!(
        *fake.log.lock().unwrap(),
        ["/movie/603/recommendations"],
        "only the title's id, once a day"
    );
}
