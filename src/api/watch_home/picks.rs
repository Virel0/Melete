//! AI picks as the API gives them (`crate::ai_picks`): "Featured for you" for the viewer, as its
//! own answer and as a home row, and titles like one title, from the AI while AI picks are on,
//! else by Melete's own likeness. Only what the viewer may see, and hasn't watched, is there.

use axum::extract::Path;
use axum::http::StatusCode;

use super::*;
use crate::ai_picks::{self, Like, Status, Title};

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
    let served = ai_picks::for_viewer(pool, outside.ai.as_ref(), me, access).await?;
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

/// Titles with their reasons as the answer gives them: what a tile shows of each, from its card.
async fn items(
    pool: &SqlitePool,
    me: i64,
    access: &Access,
    titles: &[(TitleRef, Option<String>)],
) -> Result<Vec<Value>, ApiError> {
    let wanted: Vec<TitleRef> = titles.iter().map(|(t, _)| *t).collect();
    let cards = Cards::load(pool, me, access, &wanted).await?;
    Ok(titles
        .iter()
        .filter_map(|(title, reason)| {
            let card = cards.item(&Entry::plain(*title))?;
            Some(json!({
                "kind": card["kind"], "id": card["id"], "title": card["title"],
                "year": card["year"], "poster": card["poster"], "backdrop": card["backdrop"],
                "reason": reason,
            }))
        })
        .collect())
}

/// The viewer's "Featured for you": the picks as they stand, and where they stand.
pub(super) async fn picks(
    State(state): State<HomeState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let pool = &state.pool;
    let me = current.user.id;
    let served = ai_picks::for_viewer(pool, state.outside.ai.as_ref(), me, &current.access).await?;
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
    let titles: Vec<(TitleRef, Option<String>)> = served
        .picks
        .into_iter()
        .map(|p| (title_ref(p.title), Some(p.reason)))
        .collect();
    Ok(Json(json!({
        "status": status,
        "message": message,
        "made_at": served.made_at,
        "picks": items(pool, me, &current.access, &titles).await?,
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
