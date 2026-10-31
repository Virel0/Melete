//! The Watch home in one request, as `/music/home` is for music: the hero's picks, then rows of
//! titles (continue watching, next up, favourites, "because you watched …" for the titles the
//! user finished most recently, and the newest shows, anime and movies), each title with
//! what a tile, a spotlight or the hero shows of it. A row with nothing in it is left out.
//!
//! Only what the user may see is there (`crate::access`).
//!
//! A series is anime when it's wanted as anime or sits in a library with the anime category;
//! anime has its own row and isn't among the shows. Artwork the rows carry is what's stored;
//! the hero's picks are worked out as a title's details are (`title_artwork::extras`), so the
//! hero has its textless backdrop and logo.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use futures_util::future::join_all;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::media::{Exclusions, anime_sql};
use super::title_artwork::{ArtworkState, extras};
use crate::access::Access;

mod because;
mod cards;
mod hero_trailer;
mod picks;

use cards::*;

/// The most titles in each row.
const ROW: i64 = 20;
/// The most picks the hero steps through.
const HERO: usize = 8;

#[derive(Clone)]
struct HomeState {
    pool: SqlitePool,
    artwork: ArtworkState,
    outside: because::Outside,
}

/// The Watch home, AI picks and titles like one, for everyone signed in. `artwork` works out
/// the hero's textless backdrops, logos and focal points, with TMDB when it's set up, which
/// also recommends titles like those the user finished while trending and similar titles are
/// on; `ai` makes AI picks while they're on.
pub(super) fn routes_with_ai(
    pool: SqlitePool,
    artwork: ArtworkState,
    ai: Option<Arc<crate::ai::Client>>,
) -> Router {
    let tmdb = artwork.tmdb();
    routes_from(pool, artwork, tmdb, ai)
}

/// The routes without OpenRouter, for tests.
#[cfg(test)]
fn routes(pool: SqlitePool, artwork: ArtworkState) -> Router {
    routes_with_ai(pool, artwork, None)
}

/// The routes with TMDB but without OpenRouter, for tests.
#[cfg(test)]
fn routes_with(
    pool: SqlitePool,
    artwork: ArtworkState,
    tmdb: Option<Arc<crate::tmdb::Client>>,
) -> Router {
    routes_from(pool, artwork, tmdb, None)
}

fn routes_from(
    pool: SqlitePool,
    artwork: ArtworkState,
    tmdb: Option<Arc<crate::tmdb::Client>>,
    ai: Option<Arc<crate::ai::Client>>,
) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    let outside = because::Outside {
        tmdb,
        recommended: Arc::default(),
        ai,
    };
    Router::new()
        .route("/api/v1/home", get(home))
        .route("/api/v1/ai/picks", get(picks::picks))
        .route("/api/v1/movies/{id}/similar", get(picks::movie_like))
        .route("/api/v1/series/{id}/similar", get(picks::series_like))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(HomeState {
            pool,
            artwork,
            outside,
        })
}

/// A title in a row: which one, and what the row says about it beyond the title itself.
#[derive(Clone)]
struct Entry {
    title: TitleRef,
    /// The episode the row is about (the one partway through, or the next one).
    episode: Option<Value>,
    /// The progress the row is about: the file partway through.
    progress: Option<Value>,
}

impl Entry {
    fn plain(title: TitleRef) -> Self {
        Self {
            title,
            episode: None,
            progress: None,
        }
    }
}

async fn home(
    State(state): State<HomeState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let pool = &state.pool;
    let me = current.user.id;
    let access = &current.access;
    let continuing = continue_watching(pool, me, access).await?;
    let next_up = next_up(pool, me, access).await?;
    let favourites = favourites(pool, me, access).await?;
    let shows = latest_series(pool, access, false).await?;
    let anime = latest_series(pool, access, true).await?;
    let movies = latest_movies(pool, access).await?;
    let because = because::rows(pool, &state.outside, me, access).await?;
    let featured = picks::featured(pool, &state.outside, me, access).await?;

    // The hero: what's being watched first, then the newest of everything.
    let mut newest: Vec<&(TitleRef, i64)> = shows.iter().chain(&anime).chain(&movies).collect();
    newest.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));
    let mut seen = HashSet::new();
    let hero: Vec<Entry> = continuing
        .iter()
        .cloned()
        .chain(newest.into_iter().map(|(t, _)| Entry::plain(*t)))
        .filter(|e| seen.insert(e.title))
        .take(HERO)
        .collect();

    let rows: [(&str, Vec<Entry>); 6] = [
        ("continue_watching", continuing),
        ("next_up", next_up),
        ("favourites", favourites),
        ("latest_movies", plain(&movies)),
        ("latest_shows", plain(&shows)),
        ("latest_anime", plain(&anime)),
    ];
    let wanted: Vec<TitleRef> = hero
        .iter()
        .chain(rows.iter().flat_map(|(_, entries)| entries))
        .chain(because.iter().flat_map(|(_, entries)| entries))
        .chain(&featured)
        .map(|e| e.title)
        .chain(because.iter().map(|(seed, _)| *seed))
        .collect();
    let mut cards = Cards::load(pool, me, access, &wanted).await?;
    // The hero's artwork as a title's details give it, picked and worked out if it's due.
    let hero: Vec<Entry> = hero.into_iter().filter(|e| cards.has(e.title)).collect();
    let found = join_all(
        hero.iter()
            .map(|e| extras(&state.artwork, e.title.series, e.title.id)),
    )
    .await;
    for (entry, found) in hero.iter().zip(found) {
        if let Some(found) = found? {
            cards.overlay_artwork(entry.title, &found);
        }
    }
    // Each pick says where its trailer comes from, so an app needn't ask.
    let online = crate::settings::trailers_online(pool).await?;
    let mut picks = Vec::new();
    for entry in &hero {
        if let Some(mut pick) = cards.item(entry) {
            pick["trailer"] = hero_trailer::hero_trailer(pool, entry.title, online).await?;
            picks.push(pick);
        }
    }
    Ok(Json(json!({
        "hero": picks,
        "rows": rows_json(&cards, &rows, &featured, &because),
    })))
}

/// The rows as the answer gives them, "featured for you" and the "because you watched" ones
/// after the user's own (continue watching, next up, favourites); each left out when nothing
/// in it is there to show, and a "because you watched" one also when its title isn't.
fn rows_json(
    cards: &Cards,
    rows: &[(&str, Vec<Entry>)],
    featured: &[Entry],
    because: &[(TitleRef, Vec<Entry>)],
) -> Vec<Value> {
    let row = |key: &str, seed: Value, entries: &[Entry]| {
        let items: Vec<Value> = entries.iter().filter_map(|e| cards.item(e)).collect();
        (!items.is_empty()).then(|| json!({ "key": key, "seed": seed, "items": items }))
    };
    let alike = || {
        because.iter().filter_map(|(title, entries)| {
            let card = cards.item(&Entry::plain(*title))?;
            let seed = json!({ "kind": card["kind"], "id": card["id"], "title": card["title"] });
            row("because_you_watched", seed, entries)
        })
    };
    let (own, rest): (Vec<_>, Vec<_>) = rows
        .iter()
        .partition(|(key, _)| matches!(*key, "continue_watching" | "next_up" | "favourites"));
    let draw = |(key, entries): &&(&str, Vec<Entry>)| row(key, Value::Null, entries);
    own.iter()
        .filter_map(draw)
        .chain(row("featured_for_you", Value::Null, featured))
        .chain(alike())
        .chain(rest.iter().filter_map(draw))
        .collect()
}

fn plain(titles: &[(TitleRef, i64)]) -> Vec<Entry> {
    titles.iter().map(|(t, _)| Entry::plain(*t)).collect()
}

fn none_excluded() -> Result<Exclusions, ApiError> {
    Exclusions::parse(None, None, None)
}

/// What the user is partway through, one entry per title (its latest file), most recent first.
async fn continue_watching(
    pool: &SqlitePool,
    me: i64,
    access: &Access,
) -> Result<Vec<Entry>, ApiError> {
    let rows = super::playback::continue_rows(pool, me, access, &none_excluded()?, ROW).await?;
    let mut seen = HashSet::new();
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let title = match (r.movie_id, r.series_id) {
                (Some(id), _) => TitleRef::movie(id),
                (None, Some(id)) => TitleRef::series(id),
                (None, None) => return None,
            };
            let episode = r.series_id.map(|_| {
                episode_json(
                    r.file_id,
                    r.season,
                    r.episode,
                    r.episode_name.as_deref(),
                    r.still_path,
                )
            });
            let progress = json!({
                "file_id": r.file_id,
                "position_seconds": r.position_seconds,
                "duration_seconds": r.duration_seconds,
                "watched": false,
                "updated_at": r.updated_at,
            });
            seen.insert(title).then_some(Entry {
                title,
                episode,
                progress: Some(progress),
            })
        })
        .collect())
}

/// The next episode of each series the user is watching, as `/me/next-up` gives them.
async fn next_up(pool: &SqlitePool, me: i64, access: &Access) -> Result<Vec<Entry>, ApiError> {
    let rows = super::next_up::rows(pool, me, access, &none_excluded()?, ROW).await?;
    Ok(rows
        .into_iter()
        .map(|r| Entry {
            title: TitleRef::series(r.series_id),
            episode: Some(episode_json(
                r.file_id,
                Some(r.season),
                Some(r.episode),
                r.episode_name.as_deref(),
                r.still_path,
            )),
            progress: None,
        })
        .collect())
}

/// The user's favourite movies and series they may see, newest favourite first.
async fn favourites(pool: &SqlitePool, me: i64, access: &Access) -> Result<Vec<Entry>, ApiError> {
    let rows: Vec<(Option<i64>, Option<i64>)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT v.movie_id, v.series_id FROM favourites v
         LEFT JOIN movies m ON m.id = v.movie_id
         LEFT JOIN series s ON s.id = v.series_id
         WHERE v.user_id = ?1
           AND ((m.id IS NOT NULL AND m.missing_since IS NULL AND {})
             OR (s.id IS NOT NULL AND s.missing_since IS NULL AND {}))
         ORDER BY v.added_at DESC, v.id DESC LIMIT ?2",
        access.movie_sql("m"),
        access.series_sql("s")
    )))
    .bind(me)
    .bind(ROW)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| match row {
            (Some(id), _) => Some(Entry::plain(TitleRef::movie(id))),
            (None, Some(id)) => Some(Entry::plain(TitleRef::series(id))),
            (None, None) => None,
        })
        .collect())
}

/// The present series (only anime, or only not), newest episode first, with when it came in.
async fn latest_series(
    pool: &SqlitePool,
    access: &Access,
    anime: bool,
) -> Result<Vec<(TitleRef, i64)>, ApiError> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT s.id, max(coalesce(f.added_at, f.modified_at)) AS latest
         FROM series s
         JOIN libraries l ON l.id = s.library_id
         JOIN media_files f ON f.series_id = s.id AND f.missing_since IS NULL
         WHERE s.missing_since IS NULL AND {} = ?1 AND {}
         GROUP BY s.id ORDER BY latest DESC, s.id DESC LIMIT ?2",
        anime_sql("s", "l"),
        access.series_sql("s")
    )))
    .bind(anime)
    .bind(ROW)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, at)| (TitleRef::series(id), at))
        .collect())
}

/// The present movies, newest first, with when each came in.
async fn latest_movies(
    pool: &SqlitePool,
    access: &Access,
) -> Result<Vec<(TitleRef, i64)>, ApiError> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT m.id, max(coalesce(f.added_at, f.modified_at)) AS latest
         FROM movies m
         JOIN media_files f ON f.movie_id = m.id AND f.missing_since IS NULL
         WHERE m.missing_since IS NULL AND {}
         GROUP BY m.id ORDER BY latest DESC, m.id DESC LIMIT ?",
        access.movie_sql("m")
    )))
    .bind(ROW)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, at)| (TitleRef::movie(id), at))
        .collect())
}

fn episode_json(
    file_id: i64,
    season: Option<i64>,
    episode: Option<i64>,
    name: Option<&str>,
    still: Option<String>,
) -> Value {
    json!({
        "file_id": file_id,
        "season": season,
        "episode": episode,
        "name": name,
        "still": super::media::artwork_url("w300", still),
    })
}

/// Titles by kind, for looking their cards up together.
fn ids_of(titles: &[TitleRef], series: bool) -> Vec<i64> {
    let mut ids: Vec<i64> = titles
        .iter()
        .filter(|t| t.series == series)
        .map(|t| t.id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Cards by title.
type ByTitle = HashMap<TitleRef, Value>;

#[cfg(test)]
mod tests;
