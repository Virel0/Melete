//! AI picks as the API gives them (`crate::ai_picks`): "Featured for you" for the viewer, as its
//! own answer and as a home row, and titles like one title, from the AI while AI picks are on,
//! else by Melete's own likeness. Only what the viewer may see, and hasn't watched, is there.

use axum::extract::Path;
use axum::http::StatusCode;

use super::*;
use crate::ai_picks::{self, Like, Ranked, Requestable, Status, Title};

fn title_ref(title: Title) -> TitleRef {
    TitleRef {
        series: title.series,
        id: title.id,
    }
}

/// The viewer's AI picks, as a home row; none unless they're on and made.
pub(super) async fn featured(
    pool: &SqlitePool,
    outside: &because::Outside,
    me: i64,
    access: &Access,
) -> Result<Vec<Entry>, ApiError> {
    let (ai, tmdb) = (outside.ai.as_ref(), outside.tmdb.as_ref());
    let served = ai_picks::for_viewer_with(pool, ai, tmdb, me, access).await?;
    Ok(served
        .picks
        .into_iter()
        .map(|p| Entry::plain(title_ref(p.title)))
        .collect())
}

/// Titles like `title` from the AI, with its reasons, that the viewer may see and hasn't
/// watched, and where they stand: `ai` with them; else none, and `off` (AI picks off or not set
/// up), `making`, `limited` (the viewer's hourly AI calls are spent) or `busy` (the server is
/// running as many AI calls as it may).
pub(super) async fn like(
    pool: &SqlitePool,
    outside: &because::Outside,
    me: i64,
    access: &Access,
    title: TitleRef,
) -> Result<(&'static str, Option<Vec<(TitleRef, String)>>), ApiError> {
    let title = Title {
        series: title.series,
        id: title.id,
    };
    Ok(
        match ai_picks::like(pool, outside.ai.as_ref(), title, me, access).await? {
            Like::Made(picks) => {
                let picks = picks.into_iter().map(|p| (title_ref(p.title), p.reason));
                ("ai", Some(picks.collect()))
            }
            Like::Off => ("off", None),
            Like::Making => ("making", None),
            Like::Limited => ("limited", None),
            Like::Busy => ("busy", None),
        },
    )
}

/// The TMDB id a title in the library is matched to, when it is.
async fn tmdb_id_of(pool: &SqlitePool, title: TitleRef) -> sqlx::Result<Option<i64>> {
    let id: Option<Option<i64>> = sqlx::query_scalar(if title.series {
        "SELECT match_tmdb_id FROM series WHERE id = ?"
    } else {
        "SELECT match_tmdb_id FROM movies WHERE id = ?"
    })
    .bind(title.id)
    .fetch_optional(pool)
    .await?;
    Ok(id.flatten())
}

/// Titles in the library with their reasons as the answer gives them: what a tile shows of
/// each, from its card, and its TMDB id.
async fn items(
    pool: &SqlitePool,
    me: i64,
    access: &Access,
    titles: &[(TitleRef, Option<String>)],
) -> Result<Vec<Value>, ApiError> {
    let wanted: Vec<TitleRef> = titles.iter().map(|(t, _)| *t).collect();
    let cards = Cards::load(pool, me, access, &wanted).await?;
    let mut out = Vec::new();
    for (title, reason) in titles {
        let Some(card) = cards.item(&Entry::plain(*title)) else {
            continue;
        };
        out.push(json!({
            "kind": card["kind"], "id": card["id"], "tmdb_id": tmdb_id_of(pool, *title).await?,
            "requestable": false, "title": card["title"], "year": card["year"],
            "poster": card["poster"], "backdrop": card["backdrop"], "reason": reason,
        }));
    }
    Ok(out)
}

/// A trending pick not in the library as the answer gives it: by kind and TMDB id, with no id
/// in the library, its poster only when the artwork route would serve it.
async fn requestable_item(pool: &SqlitePool, pick: &Requestable) -> Result<Value, ApiError> {
    let poster =
        crate::api::discover::poster(pool, pick.poster_path.as_deref(), pick.series, pick.tmdb_id)
            .await?;
    Ok(json!({
        "kind": if pick.series { "series" } else { "movie" }, "id": null,
        "tmdb_id": pick.tmdb_id, "requestable": true, "title": pick.title, "year": pick.year,
        "poster": poster, "backdrop": null, "reason": pick.reason,
    }))
}

/// The viewer's picks in the model's order, those in the library and those to ask for.
async fn ranked_items(
    pool: &SqlitePool,
    me: i64,
    access: &Access,
    ranked: &[Ranked],
) -> Result<Vec<Value>, ApiError> {
    let mut out = Vec::new();
    for pick in ranked {
        match pick {
            Ranked::Library(p) => {
                let titles = [(title_ref(p.title), Some(p.reason.clone()))];
                out.extend(items(pool, me, access, &titles).await?);
            }
            Ranked::Requestable(r) => out.push(requestable_item(pool, r).await?),
        }
    }
    Ok(out)
}

/// The viewer's "Featured for you": the picks as they stand, and where they stand.
pub(super) async fn picks(
    State(state): State<HomeState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let pool = &state.pool;
    let me = current.user.id;
    let (ai, tmdb) = (state.outside.ai.as_ref(), state.outside.tmdb.as_ref());
    let served = ai_picks::for_viewer_with(pool, ai, tmdb, me, &current.access).await?;
    let (status, message) = match &served.status {
        Status::Off => (
            "off",
            Some("AI picks are off; an admin turns them on in Settings".to_owned()),
        ),
        Status::Unavailable => (
            "unavailable",
            Some(
                "AI features aren't set up; an admin can add an OpenRouter key in Settings".into(),
            ),
        ),
        Status::NoHistory => (
            "no_history",
            Some("Picks are made from what you watch; watch something first".into()),
        ),
        Status::Working => (
            "working",
            Some("Your picks are being made; ask again in a minute".into()),
        ),
        Status::Limited => (
            "limited",
            Some("You've asked for as many AI picks as you may this hour; try again later".into()),
        ),
        Status::Busy => (
            "busy",
            Some("Too many AI picks are being made just now; ask again in a minute".into()),
        ),
        Status::Ready => ("ready", None),
        Status::Failed(why) => (
            "failed",
            Some(format!("Your picks couldn't be made: {why}")),
        ),
    };
    Ok(Json(json!({
        "status": status,
        "message": message,
        "made_at": served.made_at,
        "picks": ranked_items(pool, me, &current.access, &served.ranked).await?,
    })))
}

/// Titles like a movie.
pub(super) async fn movie_like(
    state: State<HomeState>,
    current: Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    like_answer(state, current, TitleRef::movie(id)).await
}

/// Titles like a series.
pub(super) async fn series_like(
    state: State<HomeState>,
    current: Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    like_answer(state, current, TitleRef::series(id)).await
}

/// Titles like `title` the viewer may see and hasn't watched, most alike first, and where they
/// came from: the AI, or the library's own likeness.
async fn like_answer(
    State(state): State<HomeState>,
    Extension(current): Extension<CurrentUser>,
    title: TitleRef,
) -> Result<Json<Value>, ApiError> {
    let pool = &state.pool;
    let me = current.user.id;
    let access = &current.access;
    let cards = Cards::load(pool, me, access, &[title]).await?;
    if !cards.has(title) {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such title"));
    }
    let (status, from_ai) = like(pool, &state.outside, me, access, title).await?;
    let (source, alike) = match from_ai {
        Some(picks) => ("ai", picks.into_iter().map(|(t, r)| (t, Some(r))).collect()),
        None => {
            let discovery =
                state.outside.tmdb.is_some() && crate::settings::discovery(pool).await?;
            let own = because::own_like(pool, &state.outside, discovery, me, access, title);
            ("library", own.await?)
        }
    };
    Ok(Json(json!({
        "status": status,
        "source": source,
        "items": items(pool, me, access, &alike).await?,
    })))
}
