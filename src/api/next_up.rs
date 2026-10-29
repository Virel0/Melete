//! Next Up: the next episode to watch of each series the user is watching.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::media::{Exclusions, artwork_url};
use super::playback::{CONTINUE_LIMIT, ExcludeQuery};

/// The Next Up route, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/me/next-up", get(next_up))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(pool)
}

/// The next episode of one series.
#[derive(sqlx::FromRow)]
pub(super) struct NextRow {
    /// The episode's file, to play.
    pub file_id: i64,
    /// The series.
    pub series_id: i64,
    /// The episode's season.
    pub season: i64,
    /// The episode's number.
    pub episode: i64,
    /// The series' title.
    pub title: String,
    /// The series' year.
    pub year: Option<i64>,
    /// The episode's name, from TMDB.
    pub episode_name: Option<String>,
    /// The series' poster on TMDB's image server.
    pub poster_path: Option<String>,
    /// The episode's still on TMDB's image server.
    pub still_path: Option<String>,
    /// When the user last finished an episode of the series, in Unix seconds.
    pub last_watched_at: i64,
}

/// For each series with an episode the user has watched, the first episode on disk after the
/// furthest one watched (by season and episode; specials aside) that they haven't watched,
/// series watched most recently first. An episode already started is left out: it's in
/// `/me/continue`.
async fn next_up(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ExcludeQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let excluded = query.parse()?;
    let rows = rows(
        &pool,
        current.user.id,
        &current.access,
        &excluded,
        CONTINUE_LIMIT,
    )
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| {
                json!({
                    "file_id": r.file_id,
                    "series_id": r.series_id,
                    "season": r.season,
                    "episode": r.episode,
                    "title": r.title,
                    "year": r.year,
                    "episode_name": r.episode_name,
                    "poster": artwork_url("w342", r.poster_path),
                    "still": artwork_url("w300", r.still_path),
                    "last_watched_at": r.last_watched_at,
                })
            })
            .collect(),
    ))
}

/// Up to `limit` next episodes for `user_id`, as `/me/next-up` lists them, of what `access`
/// sees, leaving out what `excluded` names.
pub(super) async fn rows(
    pool: &SqlitePool,
    user_id: i64,
    access: &crate::access::Access,
    excluded: &Exclusions,
    limit: i64,
) -> sqlx::Result<Vec<NextRow>> {
    // Episode numbers stay far below the multiplier, so season * it + episode orders episodes.
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH furthest AS (
             SELECT f.series_id, max(f.season * 1000000 + COALESCE(f.episode_end, f.episode))
                     AS mark,
                 max(p.updated_at) AS watched_at
             FROM playback_progress p JOIN media_files f ON f.id = p.file_id
             WHERE p.user_id = ?1 AND p.watched = 1 AND f.series_id IS NOT NULL
               AND f.season > 0 AND f.episode IS NOT NULL
             GROUP BY f.series_id
         ),
         following AS (
             SELECT f.id, f.series_id, f.season, f.episode, w.watched_at,
                 COALESCE(p.position_seconds, 0) AS position,
                 row_number() OVER (PARTITION BY f.series_id
                                    ORDER BY f.season, f.episode, f.id) AS n
             FROM media_files f
             JOIN furthest w ON w.series_id = f.series_id
             JOIN libraries l ON l.id = f.library_id
             LEFT JOIN playback_progress p ON p.file_id = f.id AND p.user_id = ?1
             WHERE f.missing_since IS NULL AND f.season > 0 AND f.episode IS NOT NULL
               AND f.season * 1000000 + f.episode > w.mark
               AND COALESCE(p.watched, 0) = 0
               AND {} AND {}
         )
         SELECT n.id AS file_id, n.series_id, n.season, n.episode,
             COALESCE(ts.name, s.parsed_title) AS title,
             COALESCE(ts.year, s.parsed_year) AS year,
             te.name AS episode_name, COALESCE(s.local_poster_key, ts.poster_path) AS poster_path,
             te.still_path,
             n.watched_at AS last_watched_at
         FROM following n
         JOIN series s ON s.id = n.series_id
         LEFT JOIN tmdb_series ts ON ts.tmdb_id = s.match_tmdb_id
         LEFT JOIN tmdb_episodes te ON te.series_tmdb_id = ts.tmdb_id
             AND te.season = n.season AND te.episode = n.episode
         WHERE n.n = 1 AND n.position = 0 AND s.missing_since IS NULL
         ORDER BY n.watched_at DESC, n.series_id DESC
         LIMIT ?2",
        Exclusions::clause("f.series_id", 3),
        access.file_sql("f"),
    )))
    .bind(user_id)
    .bind(limit)
    .bind(&excluded.series)
    .bind(&excluded.libraries)
    .bind(&excluded.categories)
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;

    struct Fixture {
        pool: SqlitePool,
        app: Router,
        token: String,
        user_id: i64,
    }

    /// Two series in a shows library and one in an anime library, each with season 1 on disk
    /// (Show has E01, E02-E03 in one file, E05 and a special), and a movie.
    async fn fixture() -> Fixture {
        let pool = db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path, category) VALUES
                 (1, 'shows', 'shows', '/s', NULL), (2, 'anime', 'shows', '/a', 'anime'),
                 (3, 'movies', 'movies', '/m', NULL);
             INSERT INTO tmdb_series (tmdb_id, name, year, poster_path, fetched_at)
                 VALUES (10, 'Show', 2020, '/show.jpg', 0);
             INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, name, still_path)
                 VALUES (10, 1, 5, 105, 'Five', '/s1e5.jpg');
             INSERT INTO series (id, library_id, rel_path, parsed_title, parsed_year,
                 seen_generation, match_status, match_tmdb_id) VALUES
                 (1, 1, 'Show', 'Show', NULL, 1, 'matched', 10),
                 (2, 1, 'Other', 'Other', 2001, 1, NULL, NULL),
                 (3, 2, 'Anime', 'Anime', 2022, 1, NULL, NULL);
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
                 VALUES (1, 3, 'Film', 'Film', 1);
             INSERT INTO media_files (id, library_id, rel_path, size, modified_at, series_id,
                 movie_id, season, episode, episode_end, first_seen_generation, seen_generation)
                 VALUES
                 (1, 1, 'Show/S01E01.mkv', 1, 1, 1, NULL, 1, 1, NULL, 1, 1),
                 (2, 1, 'Show/S01E02E03.mkv', 1, 1, 1, NULL, 1, 2, 3, 1, 1),
                 (3, 1, 'Show/S01E05.mkv', 1, 1, 1, NULL, 1, 5, NULL, 1, 1),
                 (4, 1, 'Show/S00E01.mkv', 1, 1, 1, NULL, 0, 1, NULL, 1, 1),
                 (5, 1, 'Other/S01E01.mkv', 1, 1, 2, NULL, 1, 1, NULL, 1, 1),
                 (6, 1, 'Other/S01E02.mkv', 1, 1, 2, NULL, 1, 2, NULL, 1, 1),
                 (7, 2, 'Anime/S01E01.mkv', 1, 1, 3, NULL, 1, 1, NULL, 1, 1),
                 (8, 2, 'Anime/S01E02.mkv', 1, 1, 3, NULL, 1, 2, NULL, 1, 1),
                 (9, 3, 'Film/film.mkv', 1, 1, NULL, 1, NULL, NULL, NULL, 1, 1);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = auth::create_user(&pool, "viewer", "viewer-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "test")
            .await
            .unwrap();
        Fixture {
            app: routes(pool.clone()).merge(super::super::playback::routes(
                pool.clone(),
                "ffprobe".into(),
            )),
            pool,
            token,
            user_id: user.id,
        }
    }

    /// Records file `file` as watched (or `position` seconds in) at `at`.
    async fn watched(f: &Fixture, file: i64, position: f64, at: i64) {
        sqlx::query(
            "INSERT INTO playback_progress (user_id, file_id, position_seconds, watched,
                 play_count, updated_at)
             VALUES (?1, ?2, ?3, ?3 = 0, ?3 = 0, ?4)
             ON CONFLICT (user_id, file_id) DO UPDATE SET position_seconds = ?3,
                 watched = ?3 = 0, updated_at = ?4",
        )
        .bind(f.user_id)
        .bind(file)
        .bind(position)
        .bind(at)
        .execute(&f.pool)
        .await
        .unwrap();
    }

    async fn get(f: &Fixture, uri: &str) -> (StatusCode, Value) {
        let request = Request::get(uri)
            .header(header::AUTHORIZATION, format!("Bearer {}", f.token))
            .body(Body::empty())
            .unwrap();
        let response = f.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn files(body: &Value) -> Vec<i64> {
        body.as_array()
            .unwrap()
            .iter()
            .map(|i| i["file_id"].as_i64().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn the_next_episode_after_the_furthest_watched() {
        let f = fixture().await;
        let (status, body) = get(&f, "/api/v1/me/next-up").await;
        assert_eq!(
            (status, body),
            (StatusCode::OK, json!([])),
            "nothing watched"
        );

        // Show: E01 then the E02-E03 file watched, so next is E05 (E04 isn't on disk).
        watched(&f, 1, 0.0, 10).await;
        watched(&f, 2, 0.0, 11).await;
        // Other: E01 watched; E02 already started, so it's in Continue Watching instead.
        watched(&f, 5, 0.0, 30).await;
        watched(&f, 6, 42.0, 31).await;
        // Anime: E01 watched later than Show.
        watched(&f, 7, 0.0, 20).await;
        // A watched special and a watched movie lead nowhere.
        watched(&f, 4, 0.0, 40).await;
        watched(&f, 9, 0.0, 41).await;

        let (_, body) = get(&f, "/api/v1/me/next-up").await;
        assert_eq!(files(&body), [8, 3], "most recently watched series first");
        assert_eq!(
            body[1],
            json!({
                "file_id": 3, "series_id": 1, "season": 1, "episode": 5, "title": "Show",
                "year": 2020, "episode_name": "Five", "poster": "/api/v1/artwork/w342/show.jpg",
                "still": "/api/v1/artwork/w300/s1e5.jpg", "last_watched_at": 11,
            })
        );

        // Rewatching an early episode doesn't move the series back.
        watched(&f, 1, 0.0, 50).await;
        assert_eq!(files(&get(&f, "/api/v1/me/next-up").await.1), [3, 8]);
        // Finishing the season leaves nothing next.
        watched(&f, 3, 0.0, 60).await;
        assert_eq!(files(&get(&f, "/api/v1/me/next-up").await.1), [8]);
    }

    #[tokio::test]
    async fn series_libraries_and_categories_can_be_left_out() {
        let f = fixture().await;
        watched(&f, 1, 0.0, 10).await;
        watched(&f, 5, 0.0, 30).await;
        watched(&f, 7, 0.0, 20).await;
        assert_eq!(files(&get(&f, "/api/v1/me/next-up").await.1), [6, 8, 2]);
        for (query, expected) in [
            ("exclude_category=anime", vec![6, 2]),
            ("exclude_library=shows", vec![8]),
            ("exclude_library=shows,anime", vec![]),
            ("exclude_series=2,%201", vec![8]),
        ] {
            let (status, body) = get(&f, &format!("/api/v1/me/next-up?{query}")).await;
            assert_eq!(status, StatusCode::OK, "{query}");
            assert_eq!(files(&body), expected, "{query}");
        }

        // Continue Watching takes the same lists, and movies stay unless their library goes.
        watched(&f, 6, 42.0, 31).await;
        watched(&f, 8, 42.0, 21).await;
        watched(&f, 9, 42.0, 5).await;
        assert_eq!(files(&get(&f, "/api/v1/me/continue").await.1), [6, 8, 9]);
        for (query, expected) in [
            ("exclude_category=anime", vec![6, 9]),
            ("exclude_series=2", vec![8, 9]),
            ("exclude_library=movies,anime", vec![6]),
        ] {
            let (status, body) = get(&f, &format!("/api/v1/me/continue?{query}")).await;
            assert_eq!(status, StatusCode::OK, "{query}");
            assert_eq!(files(&body), expected, "{query}");
        }

        for bad in [
            "exclude_series=x",
            "exclude_category=cartoons",
            "exclude_series=1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98,99,100,101",
        ] {
            for route in ["next-up", "continue"] {
                let (status, _) = get(&f, &format!("/api/v1/me/{route}?{bad}")).await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "{route} {bad}");
            }
        }
    }
}
