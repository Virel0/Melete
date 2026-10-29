//! Who's in a title and who made it, and a person's titles in the library, from TMDB's data for
//! the titles in the library. Kept out of the browse lists, so those stay as cheap as they are.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::media::artwork_url;
use crate::access::Access;

/// The one size people's pictures are given at.
pub(super) const PROFILE_SIZE: &str = "w185";

/// A title's credits and a person's titles, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/movies/{id}/credits", get(movie_credits))
        .route("/api/v1/series/{id}/credits", get(series_credits))
        .route("/api/v1/people/{id}", get(person))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(pool)
}

/// A movie's TMDB id, and its collection's id, name and poster.
type MovieRow = (Option<i64>, Option<i64>, Option<String>, Option<String>);
/// A credit: the person's id, name and picture, and their role, character and job.
type CreditRow = (
    i64,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
);
/// A person's title: its kind, id, title, year and poster, and their role and what they did.
type TitleRow = (
    String,
    i64,
    String,
    Option<i64>,
    Option<String>,
    String,
    Option<String>,
);

fn not_found(what: &str) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, format!("no such {what}"))
}

async fn movie_credits(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let row: Option<MovieRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT t.tmdb_id, c.id, c.name, c.poster_path FROM movies m
         LEFT JOIN tmdb_movies t ON t.tmdb_id = m.match_tmdb_id
         LEFT JOIN tmdb_collections c ON c.id = t.collection_id
         WHERE m.id = ? AND m.missing_since IS NULL AND {}",
        current.access.movie_sql("m")
    )))
    .bind(id)
    .fetch_optional(&pool)
    .await?;
    let Some((tmdb_id, collection, name, poster)) = row else {
        return Err(not_found("movie"));
    };
    let collection = match (collection, name) {
        (Some(id), Some(name)) => {
            json!({ "id": id, "name": name, "poster": artwork_url("w342", poster) })
        }
        _ => Value::Null,
    };
    credits(&pool, "movie_tmdb_id", tmdb_id, collection).await
}

async fn series_credits(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let row: Option<(Option<i64>,)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT t.tmdb_id FROM series s LEFT JOIN tmdb_series t ON t.tmdb_id = s.match_tmdb_id
         WHERE s.id = ? AND s.missing_since IS NULL AND {}",
        current.access.series_sql("s")
    )))
    .bind(id)
    .fetch_optional(&pool)
    .await?;
    let Some((tmdb_id,)) = row else {
        return Err(not_found("series"));
    };
    credits(&pool, "series_tmdb_id", tmdb_id, Value::Null).await
}

/// A title's cast and crew in billing order (none when it isn't matched or they aren't
/// fetched yet), with the collection given.
async fn credits(
    pool: &SqlitePool,
    column: &str,
    tmdb_id: Option<i64>,
    collection: Value,
) -> Result<Json<Value>, ApiError> {
    let rows: Vec<CreditRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT p.id, p.name, p.profile_path, c.role, c.character, c.job
             FROM tmdb_credits c JOIN tmdb_people p ON p.id = c.person_id
             WHERE c.{column} = ? ORDER BY c.position"
    )))
    .bind(tmdb_id)
    .fetch_all(pool)
    .await?;
    let (mut cast, mut crew) = (Vec::new(), Vec::new());
    for (id, name, profile, role, character, job) in rows {
        let profile = artwork_url(PROFILE_SIZE, profile);
        if role == "cast" {
            cast.push(json!({ "person_id": id, "name": name, "character": character, "profile": profile }));
        } else {
            crew.push(json!({ "person_id": id, "name": name, "job": job, "profile": profile }));
        }
    }
    Ok(Json(
        json!({ "cast": cast, "crew": crew, "collection": collection }),
    ))
}

/// A person and their titles in the library that the user may see, newest first, with what they
/// did in each; 404 when the user may see none of them.
async fn person(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let found: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT name, profile_path FROM tmdb_people WHERE id = ?")
            .bind(id)
            .fetch_optional(&pool)
            .await?;
    let Some((name, profile)) = found else {
        return Err(not_found("person"));
    };
    let rows = titles_of(&pool, &current.access, id).await?;
    if rows.is_empty() {
        return Err(not_found("person"));
    }
    let mut titles: Vec<Value> = Vec::with_capacity(rows.len());
    for (kind, title_id, title, year, poster, role, detail) in rows {
        let entry = match titles
            .iter_mut()
            .find(|t| t["kind"] == kind && t["id"] == title_id)
        {
            Some(entry) => entry,
            None => {
                titles.push(
                    json!({ "kind": kind, "id": title_id, "title": title, "year": year,
                    "poster": artwork_url("w342", poster), "characters": [], "jobs": [] }),
                );
                titles.last_mut().expect("just pushed")
            }
        };
        let list = if role == "cast" { "characters" } else { "jobs" };
        if let (Some(detail), Some(items)) = (detail, entry[list].as_array_mut())
            && !items.contains(&Value::from(detail.as_str()))
        {
            items.push(Value::from(detail));
        }
    }
    Ok(Json(json!({
        "id": id,
        "name": name,
        "profile": artwork_url(PROFILE_SIZE, profile),
        "titles": titles,
    })))
}

/// A person's titles in the library that the user may see, with what they did in each.
async fn titles_of(pool: &SqlitePool, access: &Access, id: i64) -> Result<Vec<TitleRow>, ApiError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT 'movie', x.id, t.title, t.year, COALESCE(x.local_poster_key, t.poster_path),
             c.role, COALESCE(c.character, c.job)
         FROM tmdb_credits c JOIN tmdb_movies t ON t.tmdb_id = c.movie_tmdb_id
         JOIN movies x ON x.match_tmdb_id = t.tmdb_id AND x.missing_since IS NULL
         WHERE c.person_id = ?1 AND {}
         UNION ALL
         SELECT 'series', x.id, t.name, t.year, COALESCE(x.local_poster_key, t.poster_path),
             c.role, COALESCE(c.character, c.job)
         FROM tmdb_credits c JOIN tmdb_series t ON t.tmdb_id = c.series_tmdb_id
         JOIN series x ON x.match_tmdb_id = t.tmdb_id AND x.missing_since IS NULL
         WHERE c.person_id = ?1 AND {}
         ORDER BY 4 DESC, 3, 2",
        access.movie_sql("x"),
        access.series_sql("x"),
    )))
    .bind(id)
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    async fn get(app: &Router, token: &str, uri: &str) -> (StatusCode, Value) {
        let request = Request::get(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn titles_say_who_made_them_and_people_their_titles() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'm', 'movies', '/m'),
                 (2, 's', 'shows', '/s');
             INSERT INTO tmdb_collections (id, name, poster_path)
                 VALUES (2344, 'The Matrix Collection', '/coll.jpg');
             INSERT INTO tmdb_movies (tmdb_id, title, year, poster_path, fetched_at, collection_id)
                 VALUES (603, 'The Matrix', 1999, '/m.jpg', 0, 2344),
                        (604, 'The Matrix Reloaded', 2003, NULL, 0, 2344);
             INSERT INTO tmdb_series (tmdb_id, name, year, fetched_at)
                 VALUES (1396, 'A Show', 2008, 0);
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
                 match_status, match_tmdb_id) VALUES
                 (1, 1, 'The Matrix', 'The Matrix', 1, 'matched', 603),
                 (2, 1, 'Unmatched', 'Unmatched', 1, NULL, NULL);
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation,
                 match_status, match_tmdb_id) VALUES (1, 2, 'A Show', 'A Show', 1, 'matched', 1396);
             INSERT INTO tmdb_people (id, name, profile_path) VALUES
                 (6384, 'Keanu Reeves', '/keanu.jpg'), (9339, 'Lana Wachowski', NULL);
             INSERT INTO tmdb_credits (movie_tmdb_id, series_tmdb_id, person_id, role, character,
                 job, position) VALUES
                 (603, NULL, 9339, 'crew', NULL, 'Director', 0),
                 (603, NULL, 6384, 'cast', 'Neo', NULL, 1),
                 (604, NULL, 6384, 'cast', 'Neo', NULL, 0),
                 (NULL, 1396, 6384, 'cast', 'Himself', NULL, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let app = routes(pool.clone());

        let (status, credits) = get(&app, &token, "/api/v1/movies/1/credits").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            credits,
            json!({
                "cast": [{ "person_id": 6384, "name": "Keanu Reeves", "character": "Neo",
                           "profile": "/api/v1/artwork/w185/keanu.jpg" }],
                "crew": [{ "person_id": 9339, "name": "Lana Wachowski", "job": "Director",
                           "profile": null }],
                "collection": { "id": 2344, "name": "The Matrix Collection",
                                "poster": "/api/v1/artwork/w342/coll.jpg" },
            })
        );
        let (_, unmatched) = get(&app, &token, "/api/v1/movies/2/credits").await;
        assert_eq!(
            unmatched,
            json!({ "cast": [], "crew": [], "collection": null })
        );
        let (_, show) = get(&app, &token, "/api/v1/series/1/credits").await;
        assert_eq!(show["cast"][0]["character"], "Himself");
        assert_eq!(show["collection"], Value::Null);

        let (status, keanu) = get(&app, &token, "/api/v1/people/6384").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(keanu["name"], "Keanu Reeves");
        let titles: Vec<(&str, i64, &str)> = keanu["titles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                (
                    t["kind"].as_str().unwrap(),
                    t["id"].as_i64().unwrap(),
                    t["title"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            titles,
            [("series", 1, "A Show"), ("movie", 1, "The Matrix")],
            "newest first, and only what's in the library"
        );
        assert_eq!(keanu["titles"][1]["characters"], json!(["Neo"]));
        for missing in [
            "/api/v1/people/1",
            "/api/v1/movies/9/credits",
            "/api/v1/series/9/credits",
        ] {
            assert_eq!(
                get(&app, &token, missing).await.0,
                StatusCode::NOT_FOUND,
                "{missing}"
            );
        }
    }
}
