//! Owned or requestable: movies and series in one shape, whether they're in the library,
//! requested or only on TMDB. Each has one stable id, `tmdb:movie:603` or `tmdb:tv:1399`, and a
//! state: `playable` (in the library), `requested` (wanted, or asked for by the caller) or
//! `requestable`.
//!
//! Trending and similar titles, looking up a title that isn't in the library, and a collection's
//! films that aren't, ask TMDB, so they're on only with `MELETE_DISCOVERY`; only which chart or
//! a title's or collection's TMDB id leaves the server. Search sends the words searched for, as requests' search does, and
//! needs only TMDB.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::media::artwork_url;
use crate::tmdb::{self, Listed};
use crate::want::year_of;

mod collections;

use collections::collection;

#[derive(Clone)]
struct DiscoverState {
    pool: SqlitePool,
    tmdb: Option<Arc<tmdb::Client>>,
}

/// Trending, search, similar titles and one item, for everyone signed in. Without `tmdb`, the
/// routes that need TMDB refuse.
pub fn routes(pool: SqlitePool, tmdb: Option<Arc<tmdb::Client>>) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/discover/trending", get(trending))
        .route("/api/v1/discover/search", get(search))
        .route("/api/v1/discover/similar/{id}", get(similar))
        .route("/api/v1/items/{id}", get(item))
        .route("/api/v1/collections/{id}", get(collection))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(DiscoverState { pool, tmdb })
}

/// A movie or a series, as ids name them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Movie,
    Series,
}

impl Kind {
    fn parse(kind: Option<&str>) -> Result<Self, ApiError> {
        match kind.unwrap_or("movie") {
            "movie" => Ok(Self::Movie),
            "series" => Ok(Self::Series),
            _ => Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "kind is movie or series",
            )),
        }
    }

    fn tv(self) -> bool {
        self == Self::Series
    }

    /// As the API names it.
    fn name(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Series => "series",
        }
    }

    /// As TMDB and ids name it.
    fn tmdb(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Series => "tv",
        }
    }
}

/// `tmdb:movie:603` or `tmdb:tv:1399`.
fn parse_id(id: &str) -> Option<(Kind, i64)> {
    let rest = id.strip_prefix("tmdb:")?;
    let (kind, number) = rest.split_once(':')?;
    let kind = match kind {
        "movie" => Kind::Movie,
        "tv" => Kind::Series,
        _ => return None,
    };
    // Digits only, without a leading zero, so one title has one id.
    if number.starts_with('0') || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((kind, number.parse().ok()?))
}

fn bad_id() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "an id is tmdb:movie:<number> or tmdb:tv:<number>",
    )
}

/// Asking TMDB for charts and titles is off unless an admin turned it on.
async fn discovery(state: &DiscoverState) -> Result<&tmdb::Client, ApiError> {
    if !crate::settings::discovery(&state.pool).await? {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "trending and similar titles are off; an admin turns them on in Settings",
        ));
    }
    metadata(state)
}

fn metadata(state: &DiscoverState) -> Result<&tmdb::Client, ApiError> {
    state.tmdb.as_deref().ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "TMDB isn't set up, so there's nothing to look titles up in",
        )
    })
}

/// TMDB didn't answer: logged, and the caller told only that.
fn outside(err: anyhow::Error) -> ApiError {
    tracing::warn!(error = format!("{err:#}"), "TMDB discovery request failed");
    ApiError::new(StatusCode::BAD_GATEWAY, "TMDB didn't answer")
}

#[derive(Deserialize)]
struct TrendingQuery {
    kind: Option<String>,
    /// `day` or `week` (the default).
    window: Option<String>,
}

/// The first page of TMDB's trending movies or series.
async fn trending(
    State(state): State<DiscoverState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<TrendingQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let kind = Kind::parse(query.kind.as_deref())?;
    let week = match query.window.as_deref().unwrap_or("week") {
        "week" => true,
        "day" => false,
        _ => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "window is day or week",
            ));
        }
    };
    let tmdb = discovery(&state).await?;
    let listed = tmdb.trending(kind.tv(), week).await.map_err(outside)?;
    Ok(Json(items(&state, &current, kind, listed).await?))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
    kind: Option<String>,
}

/// TMDB's first page of movies or series called `q`.
async fn search(
    State(state): State<DiscoverState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let kind = Kind::parse(query.kind.as_deref())?;
    let q = query.q.as_deref().unwrap_or_default().trim();
    if q.is_empty() || q.chars().count() > 200 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "q is 1 to 200 characters",
        ));
    }
    let tmdb = metadata(&state)?;
    let listed = tmdb.search_listed(kind.tv(), q).await.map_err(outside)?;
    Ok(Json(items(&state, &current, kind, listed).await?))
}

/// Titles TMDB recommends with this one.
async fn similar(
    State(state): State<DiscoverState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let (kind, tmdb_id) = parse_id(&id).ok_or_else(bad_id)?;
    let tmdb = discovery(&state).await?;
    let Some(listed) = tmdb
        .recommendations(kind.tv(), tmdb_id)
        .await
        .map_err(outside)?
    else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "TMDB has no such title",
        ));
    };
    Ok(Json(items(&state, &current, kind, listed).await?))
}

#[derive(sqlx::FromRow)]
struct LocalTitle {
    title: String,
    year: Option<i64>,
    overview: Option<String>,
    poster_path: Option<String>,
}

/// One title by its id. A title in the library is answered from the library; any other is
/// looked up on TMDB, with discovery on.
async fn item(
    State(state): State<DiscoverState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (kind, tmdb_id) = parse_id(&id).ok_or_else(bad_id)?;
    let access = &current.access;
    let local: Option<LocalTitle> = sqlx::query_as(sqlx::AssertSqlSafe(match kind {
        Kind::Movie => format!(
            "SELECT COALESCE(t.title, x.parsed_title) AS title,
                 COALESCE(t.year, x.parsed_year) AS year, t.overview, t.poster_path
             FROM movies x LEFT JOIN tmdb_movies t ON t.tmdb_id = x.match_tmdb_id
             WHERE x.match_tmdb_id = ? AND x.missing_since IS NULL AND {}
             ORDER BY x.id LIMIT 1",
            access.movie_sql("x")
        ),
        Kind::Series => format!(
            "SELECT COALESCE(t.name, x.parsed_title) AS title,
                 COALESCE(t.year, x.parsed_year) AS year, t.overview, t.poster_path
             FROM series x LEFT JOIN tmdb_series t ON t.tmdb_id = x.match_tmdb_id
             WHERE x.match_tmdb_id = ? AND x.missing_since IS NULL AND {}
             ORDER BY x.id LIMIT 1",
            access.series_sql("x")
        ),
    }))
    .bind(tmdb_id)
    .fetch_optional(&state.pool)
    .await?;
    let listed = match local {
        Some(local) => Listed {
            id: tmdb_id,
            title: Some(local.title),
            release_date: local.year.map(|y| format!("{y}")),
            overview: local.overview,
            poster_path: local.poster_path,
            ..Listed::default()
        },
        // A title not in the library: looked up, then left out by `items` for a user with a
        // highest rating when it's rated above it or not rated.
        None => {
            let tmdb = discovery(&state).await?;
            let found = match kind {
                Kind::Movie => tmdb.movie(tmdb_id).await.map_err(outside)?.map(|m| Listed {
                    id: m.id,
                    title: Some(m.title),
                    release_date: m.release_date,
                    overview: m.overview,
                    poster_path: m.poster_path,
                    ..Listed::default()
                }),
                Kind::Series => tmdb
                    .series(tmdb_id)
                    .await
                    .map_err(outside)?
                    .map(|s| Listed {
                        id: s.id,
                        name: Some(s.name),
                        first_air_date: s.first_air_date,
                        overview: s.overview,
                        poster_path: s.poster_path,
                        ..Listed::default()
                    }),
            };
            found.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "TMDB has no such title"))?
        }
    };
    let mut answer = items(&state, &current, kind, vec![listed]).await?;
    if answer.is_empty() {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such title"));
    }
    Ok(Json(answer.remove(0)))
}

/// For a user with a highest rating, fetches the ratings Melete doesn't have yet of the listed
/// titles, so each can be judged; nothing for anyone else.
async fn rate_first(
    state: &DiscoverState,
    access: &crate::access::Access,
    kind: Kind,
    listed: &[Listed],
) -> Result<(), ApiError> {
    if let (true, Some(tmdb)) = (access.limits_rating(), state.tmdb.as_deref()) {
        let titles: Vec<(bool, i64)> = listed.iter().map(|t| (kind.tv(), t.id)).collect();
        crate::ratings::fetch_unknown(&state.pool, tmdb, &titles).await?;
    }
    Ok(())
}

/// Listed titles in the API's one shape, each with its state for this user. Left out: a title
/// in the library the user may not see, and with a rating limit, one not in the library rated
/// above it or not rated (its rating fetched from TMDB first when it isn't known).
async fn items(
    state: &DiscoverState,
    current: &CurrentUser,
    kind: Kind,
    listed: Vec<Listed>,
) -> Result<Vec<Value>, ApiError> {
    let pool = &state.pool;
    let user_id = current.user.id;
    let access = &current.access;
    rate_first(state, access, kind, &listed).await?;
    let (titles, seen, wanted) = match kind {
        Kind::Movie => (
            "movies",
            access.movie_sql("x"),
            "SELECT EXISTS (SELECT 1 FROM wanted_movies WHERE tmdb_id = ?)",
        ),
        Kind::Series => (
            "series",
            access.series_sql("x"),
            "SELECT EXISTS (SELECT 1 FROM wanted_series WHERE tmdb_id = ?)",
        ),
    };
    let mut out = Vec::with_capacity(listed.len());
    for title in listed {
        // The first present title with this id, and whether the user may see it.
        let library: Option<(i64, bool)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT x.id, {seen} FROM {titles} x WHERE x.match_tmdb_id = ? AND x.missing_since IS NULL
             ORDER BY {seen} DESC, x.id LIMIT 1"
        )))
        .bind(title.id)
        .fetch_optional(pool)
        .await?;
        let library_id = match library {
            Some((id, true)) => Some(id),
            Some((_, false)) => continue,
            None if !access.rates_tmdb(pool, kind.tv(), title.id).await? => continue,
            None => None,
        };
        let wanted: bool = sqlx::query_scalar(wanted)
            .bind(title.id)
            .fetch_one(pool)
            .await?;
        let request_id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM requests WHERE user_id = ? AND kind = ? AND tmdb_id = ?
                 AND state IN ('requested', 'approved')
             ORDER BY id DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(kind.name())
        .bind(title.id)
        .fetch_optional(pool)
        .await?;
        let state = if library_id.is_some() {
            "playable"
        } else if wanted || request_id.is_some() {
            "requested"
        } else {
            "requestable"
        };
        let poster = poster(pool, title.poster_path.as_deref(), kind.tv(), title.id).await?;
        let (name, date) = match kind {
            Kind::Movie => (title.title, title.release_date),
            Kind::Series => (
                title.name.or(title.title),
                title.first_air_date.or(title.release_date),
            ),
        };
        out.push(json!({
            "id": format!("tmdb:{}:{}", kind.tmdb(), title.id),
            "kind": kind.name(),
            "tmdb_id": title.id,
            "title": name.unwrap_or_default(),
            "year": date.as_deref().and_then(year_of),
            "overview": title.overview.filter(|o| !o.is_empty()),
            "poster": poster,
            "state": state,
            "library_id": library_id,
            "request_id": request_id,
        }));
    }
    Ok(out)
}

/// A poster's URL when the artwork route serves it: a title in the library's, or one asked
/// for, or with `MELETE_DISCOVERY_POSTERS` on, any listed here, which is noted with the title
/// it was listed for (TMDB id `tmdb_id`, a series' with `tv`) so the route serves it for a
/// while, to a user with a highest rating only for a title within it
/// ([`crate::discover_posters`]).
pub(super) async fn poster(
    pool: &SqlitePool,
    path: Option<&str>,
    tv: bool,
    tmdb_id: i64,
) -> Result<Value, ApiError> {
    let Some(path) = path.filter(|p| p.starts_with('/')) else {
        return Ok(Value::Null);
    };
    let served: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM tmdb_movies WHERE poster_path = ?1
                        UNION ALL SELECT 1 FROM tmdb_series WHERE poster_path = ?1
                        UNION ALL SELECT 1 FROM requests WHERE poster = ?1
                            AND kind IN ('movie', 'series'))",
    )
    .bind(path)
    .fetch_one(pool)
    .await?;
    let artwork_ok = path
        .strip_prefix('/')
        .is_some_and(|f| super::media::artwork_type(f).is_some());
    let served = served
        || (artwork_ok && crate::settings::discovery_posters(pool).await? && {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            crate::discover_posters::listed_for(pool, path, tv, tmdb_id, now).await?;
            true
        });
    Ok(if served {
        json!(artwork_url("w342", Some(path.to_owned())))
    } else {
        Value::Null
    })
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;
    use crate::tmdb::testing::{FakeTmdb, Reply};

    struct Fixture {
        pool: SqlitePool,
        app: Router,
        token: String,
        fake: FakeTmdb,
    }

    fn listed(id: i64, title: &str, poster: &str) -> Value {
        json!({ "id": id, "title": title, "release_date": "1999-03-31", "overview": "",
                "poster_path": poster })
    }

    /// The Matrix (603) in the library, Reloaded (604) wanted, Revolutions (605) asked for
    /// by the viewer and Resurrections (606) by someone else, and TMDB's chart and
    /// recommendations listing all four and one more.
    async fn fixture(on: bool) -> Fixture {
        let pool = db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
             INSERT INTO tmdb_movies (tmdb_id, title, year, overview, poster_path, fetched_at)
                 VALUES (603, 'The Matrix', 1999, 'A hacker learns the truth.', '/matrix.jpg', 0);
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
                 match_status, match_tmdb_id)
                 VALUES (7, 1, 'The Matrix (1999)', 'The Matrix', 1, 'matched', 603);
             INSERT INTO wanted_movies (tmdb_id, title, year, monitored, source, added_at)
                 VALUES (604, 'The Matrix Reloaded', 2003, 1, 'manual', 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = |name: &'static str| {
            let pool = pool.clone();
            async move {
                auth::create_user(&pool, name, "a-long-password".into(), false)
                    .await
                    .unwrap()
                    .unwrap()
            }
        };
        let (viewer, other) = (user("viewer").await, user("other").await);
        for (who, tmdb_id) in [(viewer.id, 605), (other.id, 606)] {
            sqlx::query(
                "INSERT INTO requests (user_id, kind, tmdb_id, title, poster, state, created_at)
                 VALUES (?, 'movie', ?, 'Asked', '/asked.jpg', 'requested', 0)",
            )
            .bind(who)
            .bind(tmdb_id)
            .execute(&pool)
            .await
            .unwrap();
        }
        if on {
            sqlx::query(
                "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_DISCOVERY', 'on', 0)",
            )
            .execute(&pool)
            .await
            .unwrap();
        }
        let (_, token) = auth::issue_token(&pool, viewer.id, TokenKind::Session, "t")
            .await
            .unwrap();
        let chart = json!({ "results": [
            listed(603, "The Matrix", "/matrix.jpg"),
            listed(604, "The Matrix Reloaded", "/reloaded.jpg"),
            listed(605, "The Matrix Revolutions", "/asked.jpg"),
            listed(606, "The Matrix Resurrections", "/res.jpg"),
            listed(607, "Something Else", "/else.jpg"),
        ]});
        let fake = FakeTmdb::default();
        fake.reply("/trending/movie/week", vec![Reply::ok(chart.clone())])
            .reply("/movie/603/recommendations", vec![Reply::ok(chart.clone())])
            .reply(
                "/search/movie?include_adult=false&query=matrix",
                vec![Reply::ok(chart)],
            )
            .reply(
                "/tv/1399?append_to_response=external_ids",
                vec![Reply::ok(json!({ "id": 1399, "name": "Game of Thrones",
                    "first_air_date": "2011-04-17", "overview": "Winter.", "poster_path": "/got.jpg" }))],
            );
        let client = fake.start().await;
        Fixture {
            app: routes(pool.clone(), Some(Arc::new(client))),
            pool: pool.clone(),
            token,
            fake,
        }
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

    fn states(items: &Value) -> Vec<(String, String)> {
        items
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                (
                    i["id"].as_str().unwrap().to_owned(),
                    i["state"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn trending_titles_say_whether_theyre_playable_requested_or_requestable() {
        let f = fixture(true).await;
        let (status, items) = get(&f, "/api/v1/discover/trending").await;
        assert_eq!(status, StatusCode::OK, "{items}");
        let expected = [
            ("tmdb:movie:603", "playable"),
            ("tmdb:movie:604", "requested"),
            ("tmdb:movie:605", "requested"),
            ("tmdb:movie:606", "requestable"),
            ("tmdb:movie:607", "requestable"),
        ]
        .map(|(id, state)| (id.to_owned(), state.to_owned()));
        assert_eq!(states(&items), expected);
        assert_eq!(
            items[0],
            json!({
                "id": "tmdb:movie:603", "kind": "movie", "tmdb_id": 603, "title": "The Matrix",
                "year": 1999, "overview": null, "poster": "/api/v1/artwork/w342/matrix.jpg",
                "state": "playable", "library_id": 7, "request_id": null,
            })
        );
        assert!(items[2]["request_id"].is_i64());
        assert_eq!(
            items[2]["poster"], "/api/v1/artwork/w342/asked.jpg",
            "asked for"
        );
        assert_eq!(
            items[4]["poster"],
            Value::Null,
            "TMDB's posters aren't served"
        );
        sqlx::query(
            "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_DISCOVERY_POSTERS', 'on', 0)",
        )
        .execute(&f.pool)
        .await
        .unwrap();
        let (_, items) = get(&f, "/api/v1/discover/trending").await;
        assert_eq!(
            items[4]["poster"], "/api/v1/artwork/w342/else.jpg",
            "allowed now"
        );
        let noted: Vec<String> =
            sqlx::query_scalar("SELECT file FROM discover_posters ORDER BY file")
                .fetch_all(&f.pool)
                .await
                .unwrap();
        assert_eq!(
            noted,
            ["/else.jpg", "/reloaded.jpg", "/res.jpg"],
            "only those not served already"
        );
        let (_, similar) = get(&f, "/api/v1/discover/similar/tmdb:movie:603").await;
        assert_eq!(states(&similar), expected);
        let (_, found) = get(&f, "/api/v1/discover/search?q=matrix").await;
        assert_eq!(states(&found), expected);
    }

    /// The viewer limited to titles rated up to 12, and TMDB's ratings for the chart's titles:
    /// The Matrix rated already, Reloaded PG (10), Revolutions PG-13, Resurrections R, and the
    /// last unknown to TMDB.
    async fn limited(f: &Fixture) {
        let viewer: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'viewer'")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        let limit = crate::access::Restriction {
            libraries: None,
            max_age: Some(12),
        };
        crate::access::set_restriction(&f.pool, viewer, Some(&limit))
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO tmdb_ratings (tv, tmdb_id, certifications, age, fetched_at)
             VALUES (0, 603, '{\"US\": \"PG\"}', 10, ?)",
        )
        .bind(crate::db::unix_now())
        .execute(&f.pool)
        .await
        .unwrap();
        for (id, cert) in [(604, "PG"), (605, "PG-13"), (606, "R")] {
            f.fake.reply(
                &format!("/movie/{id}/release_dates"),
                vec![Reply::ok(json!({ "id": id, "results": [
                    { "iso_3166_1": "US", "release_dates": [{ "certification": cert, "type": 3 }] }
                ]}))],
            );
        }
        f.fake.reply(
            "/movie/604",
            vec![Reply::ok(
                json!({ "id": 604, "title": "The Matrix Reloaded",
                "release_date": "2003-05-15", "poster_path": "/reloaded.jpg" }),
            )],
        );
    }

    fn rating_requests(f: &Fixture) -> Vec<String> {
        let log = f.fake.log.lock().unwrap();
        log.iter()
            .filter(|r| r.ends_with("/release_dates"))
            .cloned()
            .collect()
    }

    #[tokio::test]
    async fn a_rating_limited_user_sees_titles_outside_the_library_within_their_rating() {
        let f = fixture(true).await;
        // Without a highest rating, no rating is asked for.
        get(&f, "/api/v1/discover/trending").await;
        assert!(rating_requests(&f).is_empty());
        limited(&f).await;
        let (status, items) = get(&f, "/api/v1/discover/trending").await;
        assert_eq!(status, StatusCode::OK, "{items}");
        let expected = [
            ("tmdb:movie:603", "playable"),
            ("tmdb:movie:604", "requested"),
        ]
        .map(|(id, state)| (id.to_owned(), state.to_owned()));
        assert_eq!(states(&items), expected);
        assert_eq!(
            items[1]["poster"],
            Value::Null,
            "the route wouldn't serve it"
        );
        let asked = rating_requests(&f);
        assert_eq!(
            asked,
            [604, 605, 606, 607].map(|id| format!("/movie/{id}/release_dates")),
            "each unknown one once, the library's known one not"
        );
        // Kept: neither another list nor one title asks again.
        let (_, found) = get(&f, "/api/v1/discover/search?q=matrix").await;
        assert_eq!(states(&found), expected);
        let (status, one) = get(&f, "/api/v1/items/tmdb:movie:604").await;
        assert_eq!(
            (status, &one["title"]),
            (StatusCode::OK, &json!("The Matrix Reloaded"))
        );
        for above in [
            "/api/v1/items/tmdb:movie:606",
            "/api/v1/items/tmdb:movie:607",
        ] {
            assert_eq!(get(&f, above).await.0, StatusCode::NOT_FOUND, "{above}");
        }
        assert_eq!(rating_requests(&f).len(), 4);
    }

    #[tokio::test]
    async fn one_title_by_its_id() {
        let f = fixture(true).await;
        let (status, matrix) = get(&f, "/api/v1/items/tmdb:movie:603").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (&matrix["state"], &matrix["overview"], &matrix["year"]),
            (
                &json!("playable"),
                &json!("A hacker learns the truth."),
                &json!(1999)
            )
        );
        assert!(f.fake.log.lock().unwrap().is_empty(), "the library answers");
        let (_, got) = get(&f, "/api/v1/items/tmdb:tv:1399").await;
        assert_eq!(
            (
                &got["id"],
                &got["kind"],
                &got["title"],
                &got["year"],
                &got["state"]
            ),
            (
                &json!("tmdb:tv:1399"),
                &json!("series"),
                &json!("Game of Thrones"),
                &json!(2011),
                &json!("requestable")
            )
        );
        assert_eq!(
            get(&f, "/api/v1/items/tmdb:movie:999").await.0,
            StatusCode::NOT_FOUND
        );
        for bad in [
            "603",
            "tmdb:show:1",
            "tmdb:movie:x",
            "tmdb:movie:-1",
            "tmdb:movie:007",
        ] {
            assert_eq!(
                get(&f, &format!("/api/v1/items/{bad}")).await.0,
                StatusCode::BAD_REQUEST,
                "{bad}"
            );
        }
    }

    #[tokio::test]
    async fn trending_similar_and_lookups_wait_for_discovery_to_be_on() {
        let f = fixture(false).await;
        for uri in [
            "/api/v1/discover/trending",
            "/api/v1/discover/similar/tmdb:movie:603",
            "/api/v1/items/tmdb:tv:1399",
        ] {
            assert_eq!(get(&f, uri).await.0, StatusCode::CONFLICT, "{uri}");
        }
        assert!(
            f.fake.log.lock().unwrap().is_empty(),
            "nothing asked of TMDB"
        );
        assert_eq!(
            get(&f, "/api/v1/items/tmdb:movie:603").await.0,
            StatusCode::OK,
            "the library's titles still answer"
        );
        assert_eq!(
            get(&f, "/api/v1/discover/search?q=matrix").await.0,
            StatusCode::OK,
            "search only needs TMDB"
        );
        for bad in [
            "/api/v1/discover/search?q=",
            "/api/v1/discover/search?q=matrix&kind=album",
            "/api/v1/discover/trending?window=year",
        ] {
            assert_eq!(get(&f, bad).await.0, StatusCode::BAD_REQUEST, "{bad}");
        }
    }

    async fn collected(on: bool) -> Fixture {
        let f = fixture(on).await;
        sqlx::raw_sql(
            "INSERT INTO tmdb_collections (id, name, poster_path)
                 VALUES (2344, 'The Matrix Collection', '/coll.jpg'), (9, 'Elsewhere', NULL);
             UPDATE tmdb_movies SET collection_id = 2344, release_date = '1999-03-31'
                 WHERE tmdb_id = 603;",
        )
        .execute(&f.pool)
        .await
        .unwrap();
        let film = |id: i64, title: &str, date: &str| json!({ "id": id, "title": title, "release_date": date, "poster_path": null });
        f.fake.reply(
            "/collection/2344",
            vec![Reply::ok(json!({ "id": 2344, "parts": [
                film(606, "The Matrix Resurrections", "2021-12-16"),
                film(604, "The Matrix Reloaded", "2003-05-15"),
                film(603, "The Matrix", "1999-03-31"),
                film(605, "The Matrix Revolutions", "2003-11-05"),
            ]}))],
        );
        f
    }

    #[tokio::test]
    async fn a_collection_lists_its_films_in_the_library_and_with_discovery_the_rest() {
        let off = collected(false).await;
        let (status, body) = get(&off, "/api/v1/collections/2344").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (body["name"].as_str(), body["all_films"].as_bool()),
            (Some("The Matrix Collection"), Some(false))
        );
        assert_eq!(
            states(&body["items"]),
            [("tmdb:movie:603".into(), "playable".into())]
        );
        assert!(
            off.fake.log.lock().unwrap().is_empty(),
            "nothing leaves the server while discovery is off"
        );
        assert_eq!(
            get(&off, "/api/v1/collections/9").await.0,
            StatusCode::NOT_FOUND,
            "only collections with a film in the library"
        );

        let on = collected(true).await;
        let (_, body) = get(&on, "/api/v1/collections/2344").await;
        assert_eq!(body["all_films"], true);
        assert_eq!(
            states(&body["items"]),
            [
                ("tmdb:movie:603".into(), "playable".into()),
                ("tmdb:movie:604".into(), "requested".into()),
                ("tmdb:movie:605".into(), "requested".into()),
                ("tmdb:movie:606".into(), "requestable".into()),
            ],
            "in release order"
        );
    }
}
