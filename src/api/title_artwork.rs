//! A title's artwork for an app's backdrop: the poster and backdrop, TMDB's best-voted
//! backdrop without text and logo, and where each backdrop's subject is, so apps crop around it.
//!
//! The textless backdrop and logo are picked from TMDB's images when a title's artwork is
//! first asked for, and again after 30 days. A backdrop's focus is worked out once, the first
//! time it's asked for: the image is then put in the artwork cache, as showing it would
//! and read through ffmpeg. Movie and series details carry the same, by
//! [`extras`].

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::media::{BACKDROP_SIZES, LOGO_SIZES, artwork_url, save_atomically};
use crate::db::unix_now;
use crate::tmdb;

/// How long picked images stand before TMDB is asked again.
const IMAGES_FOR: i64 = 30 * 24 * 3600;

/// What finding a title's artwork needs. Without `images`, only what's stored is given: no
/// TMDB requests, no downloads.
#[derive(Clone)]
pub(super) struct ArtworkState {
    pool: SqlitePool,
    tmdb: Option<Arc<tmdb::Client>>,
    images: Option<Arc<tmdb::Images>>,
    artwork_dir: PathBuf,
    ffmpeg: PathBuf,
}

impl ArtworkState {
    pub(super) fn new(
        pool: SqlitePool,
        tmdb: Option<Arc<tmdb::Client>>,
        images: tmdb::Images,
        artwork_dir: PathBuf,
        ffmpeg: PathBuf,
    ) -> Self {
        Self {
            pool,
            tmdb,
            images: Some(Arc::new(images)),
            artwork_dir,
            ffmpeg,
        }
    }

    /// TMDB, when it's set up.
    pub(super) fn tmdb(&self) -> Option<Arc<tmdb::Client>> {
        self.tmdb.clone()
    }

    /// The ffmpeg binary, which also resizes titles' own images.
    pub(super) fn ffmpeg(&self) -> &std::path::Path {
        &self.ffmpeg
    }

    #[cfg(test)]
    /// Only what's stored: what was picked and worked out before.
    pub(super) fn stored_only(pool: SqlitePool) -> Self {
        Self {
            pool,
            tmdb: None,
            images: None,
            artwork_dir: PathBuf::new(),
            ffmpeg: PathBuf::new(),
        }
    }
}

/// The title artwork routes, for tests.
#[cfg(test)]
pub fn routes(
    pool: SqlitePool,
    tmdb: Option<Arc<tmdb::Client>>,
    images: tmdb::Images,
    artwork_dir: PathBuf,
    ffmpeg: PathBuf,
) -> Router {
    routes_from(ArtworkState::new(pool, tmdb, images, artwork_dir, ffmpeg))
}

pub(super) fn routes_from(state: ArtworkState) -> Router {
    let auth = AppState {
        pool: state.pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/movies/{id}/artwork", get(movie_artwork))
        .route("/api/v1/series/{id}/artwork", get(series_artwork))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(state)
}

/// Table names for the two kinds of title. Constants only, never user input.
struct Kind {
    titles: &'static str,
    metadata: &'static str,
    tv: bool,
    noun: &'static str,
}

const MOVIES: Kind = Kind {
    titles: "movies",
    metadata: "tmdb_movies",
    tv: false,
    noun: "movie",
};

const SERIES: Kind = Kind {
    titles: "series",
    metadata: "tmdb_series",
    tv: true,
    noun: "series",
};

async fn movie_artwork(
    State(state): State<ArtworkState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let seen = current.access.sees_movie(&state.pool, id).await?;
    artwork(&state, &MOVIES, id, seen).await
}

async fn series_artwork(
    State(state): State<ArtworkState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let seen = current.access.sees_series(&state.pool, id).await?;
    artwork(&state, &SERIES, id, seen).await
}

#[derive(sqlx::FromRow)]
struct ArtworkRow {
    tmdb_id: Option<i64>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    textless_backdrop_path: Option<String>,
    logo_path: Option<String>,
    images_fetched_at: Option<i64>,
}

/// A title's artwork; `seen`: whether the user may see the title, which is a 404 if not.
async fn artwork(
    state: &ArtworkState,
    kind: &Kind,
    id: i64,
    seen: bool,
) -> Result<Json<Value>, ApiError> {
    let found = if seen {
        extras(state, kind.tv, id).await?
    } else {
        None
    };
    match found {
        Some(found) => Ok(Json(found)),
        None => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("no such {}", kind.noun),
        )),
    }
}

/// A present movie's (or with `series`, a series') artwork as `/artwork` gives it, picking and
/// working out what's due first; `None` when there's no such title.
pub(super) async fn extras(
    state: &ArtworkState,
    series: bool,
    id: i64,
) -> Result<Option<Value>, ApiError> {
    let kind = if series { &SERIES } else { &MOVIES };
    let Kind {
        titles,
        metadata,
        tv,
        noun: _,
    } = kind;
    let load = || {
        sqlx::query_as::<_, ArtworkRow>(sqlx::AssertSqlSafe(format!(
            "SELECT t.tmdb_id, COALESCE(x.local_poster_key, t.poster_path) AS poster_path,
                 COALESCE(x.local_backdrop_key, t.backdrop_path) AS backdrop_path,
                 t.textless_backdrop_path, t.logo_path, t.images_fetched_at
             FROM {titles} x LEFT JOIN {metadata} t ON t.tmdb_id = x.match_tmdb_id
             WHERE x.id = ? AND x.missing_since IS NULL"
        )))
        .bind(id)
        .fetch_optional(&state.pool)
    };
    let Some(mut row) = load().await? else {
        return Ok(None);
    };
    let now = unix_now();
    let due = row.images_fetched_at.is_none_or(|at| at < now - IMAGES_FOR);
    if let (Some(tmdb_id), Some(client), true) = (row.tmdb_id, &state.tmdb, due) {
        // A failed request leaves what's stored; the next ask tries again.
        match client.images(*tv, tmdb_id).await {
            Ok(list) => {
                let list = list.unwrap_or_default();
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "UPDATE {metadata} SET textless_backdrop_path = ?, logo_path = ?,
                         images_fetched_at = ?
                     WHERE tmdb_id = ?"
                )))
                .bind(list.textless_backdrop())
                .bind(list.logo_in(crate::tmdb::language::code(client.language())))
                .bind(now)
                .bind(tmdb_id)
                .execute(&state.pool)
                .await?;
                row = load().await?.unwrap_or(row);
            }
            Err(err) => {
                tracing::warn!(error = format!("{err:#}"), tmdb_id, "TMDB images failed");
            }
        }
    }
    let backdrop_focus = focus(state, row.backdrop_path.as_deref()).await?;
    let textless_focus = focus(state, row.textless_backdrop_path.as_deref()).await?;
    let [backdrop_size] = BACKDROP_SIZES else {
        unreachable!("one backdrop size")
    };
    let [logo_size] = LOGO_SIZES else {
        unreachable!("one logo size")
    };
    Ok(Some(json!({
        "poster": artwork_url("w500", row.poster_path),
        "backdrop": artwork_url(backdrop_size, row.backdrop_path),
        "backdrop_focus": backdrop_focus,
        "textless_backdrop": artwork_url(backdrop_size, row.textless_backdrop_path),
        "textless_backdrop_focus": textless_focus,
        "logo": artwork_url(logo_size, row.logo_path),
    })))
}

/// A backdrop's focus as `{x, y}`, worked out and kept the first time; null when there's no
/// backdrop or it can't be had or read now (the next ask tries again).
async fn focus(state: &ArtworkState, path: Option<&str>) -> Result<Value, ApiError> {
    let Some(path) = path else {
        return Ok(Value::Null);
    };
    let known: Option<(f64, f64)> = sqlx::query_as("SELECT x, y FROM artwork_focus WHERE file = ?")
        .bind(path)
        .fetch_optional(&state.pool)
        .await?;
    if let Some((x, y)) = known {
        return Ok(json!({ "x": x, "y": y }));
    }
    let Some(images) = &state.images else {
        return Ok(Value::Null);
    };
    let Some(file) = path
        .strip_prefix('/')
        .filter(|f| super::media::artwork_type(f).is_some())
    else {
        return Ok(Value::Null);
    };
    let [size] = BACKDROP_SIZES else {
        unreachable!("one backdrop size")
    };
    let cached = state.artwork_dir.join(size).join(file);
    let bytes = if super::local_artwork::is_local(file) {
        let local =
            super::local_artwork::image(&state.pool, &state.artwork_dir, &state.ffmpeg, size, file);
        match local.await? {
            Some(bytes) => bytes,
            None => return Ok(Value::Null),
        }
    } else {
        match tokio::fs::read(&cached).await {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                match images.fetch(size, file).await {
                    Ok(Some(bytes)) => {
                        if let Err(err) = save_atomically(&cached, &bytes).await {
                            tracing::warn!(error = format!("{err:#}"), "could not cache artwork");
                        }
                        bytes
                    }
                    Ok(None) => return Ok(Value::Null),
                    Err(err) => {
                        tracing::warn!(error = format!("{err:#}"), "artwork download failed");
                        return Ok(Value::Null);
                    }
                }
            }
            Err(err) => return Err(err.into()),
        }
    };
    match crate::backdrop_focus::of_image(&state.ffmpeg, &bytes).await {
        Ok((x, y)) => {
            sqlx::query("INSERT OR REPLACE INTO artwork_focus (file, x, y) VALUES (?, ?, ?)")
                .bind(path)
                .bind(x)
                .bind(y)
                .execute(&state.pool)
                .await?;
            Ok(json!({ "x": x, "y": y }))
        }
        Err(err) => {
            tracing::warn!(
                error = format!("{err:#}"),
                "could not read a backdrop's focus"
            );
            Ok(Value::Null)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

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
        downloads: Arc<AtomicUsize>,
        artwork: tempfile::TempDir,
    }

    /// The Matrix (matched, with a backdrop) and an unmatched series. TMDB lists a textless
    /// backdrop and a logo for The Matrix; the image server serves one real backdrop for any
    /// name and counts what it sends.
    async fn fixture(tmdb_on: bool) -> Fixture {
        let pool = db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES
                 (1, 'movies', 'movies', '/m'), (2, 'shows', 'shows', '/s');
             INSERT INTO tmdb_movies (tmdb_id, title, poster_path, backdrop_path, fetched_at)
                 VALUES (603, 'The Matrix', '/poster.jpg', '/backdrop.jpg', 0);
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
                 match_status, match_tmdb_id, missing_since) VALUES
                 (1, 1, 'The Matrix (1999)', 'The Matrix', 1, 'matched', 603, NULL),
                 (2, 1, 'Gone', 'Gone', 1, NULL, NULL, 5);
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
                 VALUES (1, 2, 'Show', 'Show', 1);",
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

        let fake = FakeTmdb::default();
        fake.reply(
            "/movie/603/images?include_image_language=en,null",
            vec![Reply::ok(json!({
                "backdrops": [
                    { "file_path": "/titled.jpg", "iso_639_1": "en", "vote_average": 9.0, "vote_count": 5 },
                    { "file_path": "/textless.jpg", "iso_639_1": null, "vote_average": 6.0, "vote_count": 5 }
                ],
                "logos": [{ "file_path": "/logo.png", "iso_639_1": "en", "vote_average": 5.0, "vote_count": 1 }]
            }))],
        );
        let client = fake.start().await;

        let artwork = tempfile::tempdir().unwrap();
        let picture = artwork.path().join("source.jpg");
        crate::backdrop_focus::tests::backdrop(&picture, 1000, 100);
        let picture = std::fs::read(&picture).unwrap();
        let downloads = Arc::new(AtomicUsize::new(0));
        let counter = downloads.clone();
        let server = Router::new().fallback(move || {
            let counter = counter.clone();
            let picture = picture.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                ([(header::CONTENT_TYPE, "image/jpeg")], picture)
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
        let images = tmdb::Images::with_base(format!("http://{addr}")).unwrap();

        Fixture {
            app: routes(
                pool.clone(),
                tmdb_on.then(|| Arc::new(client)),
                images,
                artwork.path().join("cache"),
                "ffmpeg".into(),
            ),
            pool,
            token,
            fake,
            downloads,
            artwork,
        }
    }

    async fn get(f: &Fixture, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
        let mut request = Request::get(uri);
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = f
            .app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn in_top_right(focus: &Value) -> bool {
        let (x, y) = (focus["x"].as_f64().unwrap(), focus["y"].as_f64().unwrap());
        // The busy pattern's centre is at (1100, 200) of 1280 by 720.
        (0.75..0.95).contains(&x) && (0.15..0.4).contains(&y)
    }

    #[tokio::test]
    async fn picks_textless_backdrops_and_logos_and_finds_each_backdrops_focus() {
        let f = fixture(true).await;
        let (status, body) = get(&f, "/api/v1/movies/1/artwork", Some(&f.token)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["poster"], "/api/v1/artwork/w500/poster.jpg");
        assert_eq!(body["backdrop"], "/api/v1/artwork/w1280/backdrop.jpg");
        assert_eq!(
            body["textless_backdrop"],
            "/api/v1/artwork/w1280/textless.jpg"
        );
        assert_eq!(body["logo"], "/api/v1/artwork/w500/logo.png");
        assert!(in_top_right(&body["backdrop_focus"]), "{body}");
        assert!(in_top_right(&body["textless_backdrop_focus"]), "{body}");
        assert_eq!(f.downloads.load(Ordering::SeqCst), 2, "each backdrop once");
        assert!(f.artwork.path().join("cache/w1280/textless.jpg").exists());

        // Asked again: nothing more from TMDB or its image server.
        let (_, again) = get(&f, "/api/v1/movies/1/artwork", Some(&f.token)).await;
        assert_eq!(again, body);
        assert_eq!(f.downloads.load(Ordering::SeqCst), 2);
        assert_eq!(f.fake.log.lock().unwrap().len(), 1);

        // After 30 days TMDB is asked again.
        sqlx::query("UPDATE tmdb_movies SET images_fetched_at = images_fetched_at - 31 * 86400")
            .execute(&f.pool)
            .await
            .unwrap();
        get(&f, "/api/v1/movies/1/artwork", Some(&f.token)).await;
        assert_eq!(f.fake.log.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn without_tmdb_or_a_match_there_is_what_there_is() {
        let f = fixture(false).await;
        let (status, body) = get(&f, "/api/v1/movies/1/artwork", Some(&f.token)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["textless_backdrop"], Value::Null);
        assert_eq!(body["logo"], Value::Null);
        assert!(in_top_right(&body["backdrop_focus"]), "{body}");

        let (status, body) = get(&f, "/api/v1/series/1/artwork", Some(&f.token)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!({
                "poster": null, "backdrop": null, "backdrop_focus": null,
                "textless_backdrop": null, "textless_backdrop_focus": null, "logo": null,
            })
        );
        for uri in [
            "/api/v1/movies/2/artwork",
            "/api/v1/movies/9/artwork",
            "/api/v1/series/9/artwork",
        ] {
            assert_eq!(
                get(&f, uri, Some(&f.token)).await.0,
                StatusCode::NOT_FOUND,
                "{uri}"
            );
        }
        assert_eq!(
            get(&f, "/api/v1/movies/1/artwork", None).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn a_failing_tmdb_leaves_what_is_stored() {
        let f = fixture(true).await;
        f.fake.reply(
            "/movie/603/images?include_image_language=en,null",
            vec![Reply::status(StatusCode::BAD_REQUEST)],
        );
        sqlx::query("UPDATE tmdb_movies SET logo_path = '/old.png'")
            .execute(&f.pool)
            .await
            .unwrap();
        let (status, body) = get(&f, "/api/v1/movies/1/artwork", Some(&f.token)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["logo"], "/api/v1/artwork/w500/old.png");
        let fetched: Option<i64> = sqlx::query_scalar("SELECT images_fetched_at FROM tmdb_movies")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(fetched, None, "tried again next time");
    }
}
