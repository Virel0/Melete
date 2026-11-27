//! A watch party's guests over HTTP (`crate::watch_party::guests`): someone with no account,
//! in by an invite link, reaching their party alone. Joining gives them a pass, kept as a
//! cookie for `/api/v1/party` only (or sent as a bearer token by an app); with it they follow
//! the party (`GET`, and `events`), play, pause and seek it, see its file's tracks, get their own
//! stream of it (`play`, or a conversion under `/api/v1/party/transcode`), and leave. Nothing
//! here reaches the library, accounts or any other party. A pass sent by cookie changes nothing
//! without the `X-Melete-Web` header, as a household sign-in's doesn't.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::WEB_HEADER;
use crate::watch_party::Refused;
use crate::watch_party::guests::{self, Guest, PASS_PREFIX};

/// The cookie a guest's pass is kept in, sent back to the party's routes alone.
const COOKIE: &str = "melete_guest";
/// How long the cookie lasts at most: a party unchanged for a day ends anyway.
const COOKIE_SECONDS: i64 = 24 * 3600;
/// How often a guest's event stream looks for a change, and how long it stays open.
const EVERY: Duration = Duration::from_secs(1);
const LIFETIME: Duration = Duration::from_secs(10 * 60);

#[derive(Clone)]
struct GuestState {
    pool: SqlitePool,
    ffprobe: Arc<PathBuf>,
}

/// The guests' routes: joining by an invite, and what their pass reaches.
pub fn routes(pool: SqlitePool, ffprobe: PathBuf) -> Router {
    let with_pass = Router::new()
        .route("/api/v1/party", get(show).put(change))
        .route("/api/v1/party/leave", post(leave))
        .route("/api/v1/party/waiting", axum::routing::put(waiting))
        .route("/api/v1/party/file", get(file))
        .route("/api/v1/party/play", post(play))
        .route("/api/v1/party/events", get(events))
        .route_layer(middleware::from_fn_with_state(pool.clone(), require_guest));
    Router::new()
        .route("/api/v1/party/join", post(join))
        .merge(with_pass)
        .layer(middleware::from_fn(super::watch_parties::wake_streams))
        .with_state(GuestState {
            pool,
            ffprobe: Arc::new(ffprobe),
        })
}

/// The pass a request came with: the bearer token, else the cookie (and whether it was that).
fn pass_of(headers: &HeaderMap) -> Option<(&str, bool)> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token.trim());
    if let Some(pass) = bearer {
        return Some((pass, false));
    }
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('='))
        .filter(|pass| !pass.is_empty())
        .map(|pass| (pass, true))
}

/// Lets a request through with its guest, when its pass is one; refuses it otherwise.
pub(super) async fn require_guest(
    State(pool): State<SqlitePool>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    #[cfg(test)]
    super::auth::note_guest_route(&request);
    let ended = || {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "this watch party pass has ended; ask whoever invited you for a new link",
        )
    };
    let Some((pass, by_cookie)) = pass_of(request.headers()) else {
        return Err(ended());
    };
    if by_cookie && !request.method().is_safe() && !request.headers().contains_key(WEB_HEADER) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "requests with a guest's cookie that change something need the X-Melete-Web header",
        ));
    }
    let guest = guests::by_pass(&pool, pass).await?.ok_or_else(ended)?;
    // Through Melete's own tunnel, a guest's peer carries the guest who joined through it.
    if let Some(arrival) = request.extensions().get::<crate::tunnel::Arrival>()
        && !arrival.carries(guest.id)
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "this tunnel config is for another guest; use the link it came with",
        ));
    }
    request.extensions_mut().insert(guest);
    Ok(next.run(request).await)
}

/// The file the guest's party plays now.
pub(super) async fn playing(pool: &SqlitePool, guest: &Guest) -> Result<i64, ApiError> {
    sqlx::query_scalar("SELECT file_id FROM watch_parties WHERE id = ?")
        .bind(guest.party_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "the watch party has ended"))
}

fn refused(refused: Refused) -> ApiError {
    match refused {
        Refused::NotFound => ApiError::new(
            StatusCode::NOT_FOUND,
            "this invite has been used, has run out, or its party has ended; ask for a new link",
        ),
        Refused::Stale => ApiError::new(
            StatusCode::CONFLICT,
            "it changed since; get it again and make the change against its new version",
        ),
        Refused::NotHost(why) => ApiError::new(StatusCode::FORBIDDEN, why),
        Refused::Invalid(why) => ApiError::new(StatusCode::BAD_REQUEST, why),
    }
}

/// The guest's party with the server's clock beside it.
async fn answer(pool: &SqlitePool, guest: &Guest) -> Result<Json<Value>, ApiError> {
    let party = guests::view(pool, guest)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "the watch party has ended"))?;
    Ok(Json(
        json!({ "party": party, "server_time": crate::db::now_ms() }),
    ))
}

/// The pass's cookie, or one clearing it; `Secure` when the page is on https.
fn cookie(pass: Option<&str>, headers: &HeaderMap) -> String {
    let https = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|origin| origin.starts_with("https://"));
    let (value, max_age) = pass.map_or(("", 0), |p| (p, COOKIE_SECONDS));
    format!(
        "{COOKIE}={value}; Path=/api/v1/party; HttpOnly; SameSite=Strict; Max-Age={max_age}{}",
        if https { "; Secure" } else { "" }
    )
}

#[derive(Deserialize)]
struct JoinBody {
    /// The invite link's secret.
    invite: String,
    /// The name to show the others.
    name: String,
}

/// Joins with an invite: the pass comes back as a cookie and, for an app, in the answer.
async fn join(
    State(state): State<GuestState>,
    arrival: Option<Extension<crate::tunnel::Arrival>>,
    headers: HeaderMap,
    Json(body): Json<JoinBody>,
) -> Result<Response, ApiError> {
    if !body.invite.starts_with(guests::INVITE_PREFIX) {
        return Err(refused(Refused::NotFound));
    }
    // Through Melete's own tunnel, a guest's peer takes the invite it was made with alone.
    if let Some(Extension(arrival)) = &arrival
        && !arrival.may_join(&crate::auth::token_hash(&body.invite))
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "this tunnel config is for another invite; use the link it came with",
        ));
    }
    let (pass, guest) = guests::accept(&state.pool, &body.invite, &body.name)
        .await?
        .map_err(refused)?;
    // Through Melete's own tunnel, the guest's peer now lasts as long as they're in.
    if let Some(Extension(arrival)) = arrival {
        arrival.joined(guest.party_id, guest.id);
    }
    debug_assert!(pass.starts_with(PASS_PREFIX));
    let Json(mut answer) = answer(&state.pool, &guest).await?;
    answer["pass"] = json!(pass);
    let set = cookie(Some(&pass), &headers);
    Ok((
        StatusCode::CREATED,
        [(header::SET_COOKIE, set)],
        Json(answer),
    )
        .into_response())
}

async fn show(
    State(state): State<GuestState>,
    Extension(guest): Extension<Guest>,
) -> Result<Json<Value>, ApiError> {
    answer(&state.pool, &guest).await
}

#[derive(Deserialize)]
struct ChangeBody {
    version: i64,
    position_ms: i64,
    playing: bool,
}

/// Plays, pauses or seeks the party; a guest never changes what it plays.
async fn change(
    State(state): State<GuestState>,
    Extension(guest): Extension<Guest>,
    Json(body): Json<ChangeBody>,
) -> Result<Json<Value>, ApiError> {
    guests::set(
        &state.pool,
        &guest,
        body.version,
        (body.position_ms, body.playing),
    )
    .await?
    .map_err(refused)?;
    answer(&state.pool, &guest).await
}

#[derive(Deserialize)]
struct WaitingBody {
    waiting: bool,
}

/// The guest's player is loading or buffering, or ready again: a playing party pauses for
/// them and plays on once everyone's ready, or after 30 seconds.
async fn waiting(
    State(state): State<GuestState>,
    Extension(guest): Extension<Guest>,
    Json(body): Json<WaitingBody>,
) -> Result<Json<Value>, ApiError> {
    let who = (
        crate::watch_party::waits::Waiter::Guest(guest.id),
        guest.name.as_str(),
    );
    crate::watch_party::waits::set(&state.pool, guest.party_id, who, body.waiting).await?;
    answer(&state.pool, &guest).await
}

async fn leave(
    State(state): State<GuestState>,
    Extension(guest): Extension<Guest>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    guests::leave(&state.pool, guest.id).await?;
    let clear = cookie(None, &headers);
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, clear)]).into_response())
}

/// The party's file as a player needs it: its tracks and length, and the cap guests' streams
/// are held to (with the way's own limit).
async fn file(
    State(state): State<GuestState>,
    Extension(guest): Extension<Guest>,
    limit: Option<Extension<crate::ways_in::VideoLimit>>,
) -> Result<Json<Value>, ApiError> {
    let id = playing(&state.pool, &guest).await?;
    let f = super::playback::load_file(&state.pool, &state.ffprobe, id).await?;
    let cap = super::playback::guest_cap(&state.pool, guest.id).await?;
    Ok(Json(json!({
        "id": f.id,
        "media": f.media,
        "max_bitrate": super::playback::lower(super::playback::way_limit(limit), cap),
    })))
}

/// A stream link for the file the party plays, as it is; one over the cap is refused when
/// streamed, and asked for as a conversion instead.
async fn play(
    State(state): State<GuestState>,
    Extension(guest): Extension<Guest>,
    limit: Option<Extension<crate::ways_in::VideoLimit>>,
) -> Result<Json<Value>, ApiError> {
    let id = playing(&state.pool, &guest).await?;
    let (token, expires_at) =
        super::playback::create_guest_link(&state.pool, guest.id, id, None).await?;
    let cap = super::playback::guest_cap(&state.pool, guest.id).await?;
    Ok(Json(json!({
        "file_id": id,
        "stream_url": format!("/api/v1/stream/{token}"),
        "expires_at": expires_at,
        "max_bitrate": super::playback::lower(super::playback::way_limit(limit), cap),
    })))
}

/// The guest's party as it changes, as `watch_party` events (`{party, server_time}`), and
/// `null` once it has ended or they've left; then the stream ends. It ends after 10 minutes
/// too, for the browser to open again.
async fn events(
    State(state): State<GuestState>,
    Extension(guest): Extension<Guest>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let until = tokio::time::Instant::now() + LIFETIME;
    // Woken by each change to a party, so a change reaches the guest at once; looked at every
    // [`EVERY`] besides, for what changes without a request (a party ending unused).
    let start = (
        state.pool,
        guest,
        None::<String>,
        true,
        crate::watch_party::changes(),
    );
    let stream = futures_util::stream::unfold(Some(start), move |state| async move {
        let (pool, guest, mut sent, mut first, mut wake) = state?;
        if tokio::time::Instant::now() >= until {
            return None;
        }
        loop {
            if !first {
                tokio::select! {
                    () = tokio::time::sleep(EVERY) => {}
                    _ = wake.changed() => {}
                }
            }
            first = false;
            let still = guests::still_in(&pool, guest.id).await.unwrap_or(false);
            let seen = if still {
                guests::view(&pool, &guest).await.ok().flatten()
            } else {
                None
            };
            let Some(seen) = seen else {
                let event = Event::default().event("watch_party").data("null");
                return Some((Ok(event), None));
            };
            let data = json!({ "party": seen, "server_time": crate::db::now_ms() });
            let key = seen.to_string();
            if sent.as_deref() != Some(&key) {
                sent = Some(key);
                let event = Event::default().event("watch_party").data(data.to_string());
                return Some((Ok(event), Some((pool, guest, sent, false, wake))));
            }
            if tokio::time::Instant::now() >= until {
                return None;
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests;
