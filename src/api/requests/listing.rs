//! Searching for something to ask for, listing requests, and following one's own newest requests as
//! events.

use super::*;

#[derive(Deserialize)]
pub(super) struct SearchQuery {
    pub(super) kind: String,
    pub(super) q: String,
    /// Narrows an album or song search.
    pub(super) artist: Option<String>,
}

/// Movies and series on TMDB, music on MusicBrainz, each marked in the library, wanted, or
/// asked for already.
pub(super) async fn search(
    State(state): State<RequestState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<Candidate>>, ApiError> {
    let found = request_search::search(
        &state.pool,
        state.sources.tmdb.as_deref(),
        state.musicbrainz.as_deref(),
        current.user.id,
        &query.kind,
        &query.q,
        query.artist.as_deref(),
    )
    .await?
    .map_err(refused)?;
    // A movie or series the user may not be shown is left out, as discovery leaves it out; for a
    // user with a highest rating, by its rating, fetched first when it isn't known.
    if current.access.limits_rating()
        && let Some(tmdb) = state.sources.tmdb.as_deref()
    {
        let titles: Vec<(bool, i64)> = found
            .iter()
            .filter_map(|c| c.tmdb_id.map(|id| (c.kind == "series", id)))
            .collect();
        crate::ratings::fetch_unknown(&state.pool, tmdb, &titles).await?;
    }
    let mut shown = Vec::with_capacity(found.len());
    for candidate in found {
        let tv = candidate.kind == "series";
        let keep = match candidate.tmdb_id {
            Some(id) => current.access.shows_tmdb(&state.pool, tv, id).await?,
            None => true,
        };
        if keep {
            shown.push(request_search::as_seen_by(&state.pool, &current.access, candidate).await?);
        }
    }
    Ok(Json(shown))
}

#[derive(Deserialize)]
pub(super) struct ListQuery {
    /// requested, approved or declined.
    pub(super) state: Option<String>,
    /// Everyone's, for an admin.
    #[serde(default)]
    pub(super) all: bool,
    /// movies, series, anime or music.
    pub(super) service: Option<String>,
    pub(super) limit: Option<i64>,
    pub(super) offset: Option<i64>,
}

#[derive(Serialize)]
pub(super) struct Page {
    pub(super) total: i64,
    pub(super) items: Vec<RequestView>,
}

pub(super) async fn list(
    State(state): State<RequestState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page>, ApiError> {
    if query.all && !current.user.is_admin {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "everyone's requests are for admins",
        ));
    }
    if let Some(s) = query.state.as_deref()
        && !["requested", "approved", "declined"].contains(&s)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "state is requested, approved or declined",
        ));
    }
    if let Some(s) = query.service.as_deref()
        && !["movies", "series", "anime", "music"].contains(&s)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "service is movies, series, anime or music",
        ));
    }
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = query.offset.unwrap_or(0).max(0);
    let filter = requests::Filter {
        user_id: (!query.all).then_some(current.user.id),
        state: query.state.as_deref(),
        service: query.service.as_deref(),
    };
    let (total, items) = requests::list_by(&state.pool, filter, limit, offset).await?;
    Ok(Json(Page { total, items }))
}

/// How many of a user's newest requests the stream follows.
pub(super) const FOLLOWED: i64 = 50;

/// The user's own newest requests as `requests` events, sent again whenever one of them
/// changes: its state, or how it's getting on.
pub(super) async fn events(
    State(state): State<RequestState>,
    Extension(current): Extension<CurrentUser>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let user = current.user.id;
    super::super::progress::stream(
        state.pool,
        &current,
        false,
        "requests",
        move |pool| async move {
            Ok(requests::list(&pool, Some(user), None, FOLLOWED, 0)
                .await?
                .1)
        },
    )
}
