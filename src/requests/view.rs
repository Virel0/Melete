//! A request as its requester and admins see it: one, or a page of them.

use super::*;

/// A request as shown to its requester and to admins.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestView {
    /// The request's row id.
    pub id: i64,
    /// Who asked for it.
    pub user_id: i64,
    /// Their username.
    pub username: String,
    /// "movie", "series", "artist", "album" or "song".
    pub kind: String,
    /// The Manage service it belongs to: "movies", "series", "anime" or "music" (see
    /// [`SERVICE`]).
    pub service: String,
    /// A movie's or series' TMDB id.
    pub tmdb_id: Option<i64>,
    /// For a series: the one season asked for; null for all of it.
    pub season: Option<i64>,
    /// The artist's MusicBrainz id, for music.
    pub mb_artist_id: Option<String>,
    /// An album's MusicBrainz release group.
    pub mb_release_group_id: Option<String>,
    /// A song's MusicBrainz recording; null for a song asked for by name.
    pub mb_recording_id: Option<String>,
    /// The title as TMDB or MusicBrainz gave it, or an artist's name.
    pub title: String,
    /// The year it came out, when known.
    pub year: Option<i64>,
    /// An album's or song's artist.
    pub artist: Option<String>,
    /// The poster's address on this server, when there's one.
    pub poster: Option<String>,
    /// "requested" (waiting for a decision), "approved" or "declined".
    pub state: String,
    /// Why it was declined; null otherwise.
    pub reason: Option<String>,
    /// When it was asked for, in Unix seconds.
    pub created_at: i64,
    /// When it was approved or declined, in Unix seconds; null while it waits.
    pub decided_at: Option<i64>,
    /// Who decided; null when it was approved at once.
    pub decided_by: Option<String>,
    /// What it became on the wanted list, once approved.
    pub wanted_id: Option<i64>,
    /// How it's getting on there, once approved.
    pub progress: Option<Progress>,
    /// For an approved series: each of its seasons as it stands, whoever asked for
    /// them; null otherwise.
    pub seasons: Option<Vec<SeasonProgress>>,
    /// Whether this answer started its search: an approval (or an admin's request, approved at
    /// once) that isn't told to wait; false everywhere else.
    pub search_started: bool,
}

#[derive(sqlx::FromRow)]
pub(super) struct Row {
    pub(super) id: i64,
    pub(super) user_id: i64,
    pub(super) username: String,
    pub(super) kind: String,
    pub(super) service: String,
    pub(super) tmdb_id: Option<i64>,
    pub(super) season: Option<i64>,
    pub(super) mb_artist_id: Option<String>,
    pub(super) mb_release_group_id: Option<String>,
    pub(super) mb_recording_id: Option<String>,
    pub(super) title: String,
    pub(super) year: Option<i64>,
    pub(super) artist: Option<String>,
    pub(super) poster: Option<String>,
    pub(super) state: String,
    pub(super) reason: Option<String>,
    pub(super) created_at: i64,
    pub(super) decided_at: Option<i64>,
    pub(super) decided_by: Option<String>,
    pub(super) wanted_id: Option<i64>,
}

/// The Manage service a request `r` belongs to, as SQL: a movie to Movies and music to Music;
/// a series to Anime when the wanted list places it there (wanted as anime, or its folder's
/// library has the anime category, as history and the managed lists decide), else, before
/// it's on the list, when TMDB had it as anime when it was asked for; otherwise to Series.
pub const SERVICE: &str = "CASE r.kind WHEN 'movie' THEN 'movies'
        WHEN 'series' THEN coalesce(
            (SELECT t.service FROM title_event_titles t
             WHERE t.kind = 'series' AND t.wanted_id = r.wanted_series_id),
            CASE WHEN r.anime = 1 THEN 'anime' ELSE 'series' END)
        ELSE 'music' END";

/// Every request with the service it belongs to, as `r`.
fn requests_with_service() -> String {
    format!("(SELECT *, ({SERVICE}) AS service FROM requests r) r")
}

/// The columns of a [`Row`], from [`requests_with_service`] and the users who asked and decided.
pub(super) fn columns() -> String {
    format!(
        "SELECT r.id, r.user_id, u.username, r.kind, r.service, r.tmdb_id, r.season,
            r.mb_artist_id, r.mb_release_group_id, r.mb_recording_id, r.title, r.year, r.artist,
            r.poster, r.state, r.reason, r.created_at, r.decided_at, d.username AS decided_by,
            coalesce(r.wanted_movie_id, r.wanted_series_id, r.wanted_artist_id, r.wanted_album_id,
                     r.wanted_song_id) AS wanted_id
         FROM {} JOIN users u ON u.id = r.user_id LEFT JOIN users d ON d.id = r.decided_by",
        requests_with_service()
    )
}

pub(super) async fn view(pool: &SqlitePool, row: Row) -> anyhow::Result<RequestView> {
    // An artist has no one step of its own; its albums do.
    let kind: Option<&'static str> = match row.kind.as_str() {
        "movie" => Some("movie"),
        "series" => Some("series"),
        "album" => Some("album"),
        "song" => Some("song"),
        _ => None,
    };
    let progress = match (row.state.as_str(), row.wanted_id, kind) {
        ("approved", Some(id), Some(kind)) => progress::of(pool, kind, id).await?,
        _ => None,
    };
    let seasons = match (row.state.as_str(), row.wanted_id, kind) {
        ("approved", Some(id), Some("series")) => Some(progress::seasons(pool, id).await?),
        _ => None,
    };
    let poster = match row.kind.as_str() {
        // TMDB's path, served through the artwork route.
        "movie" | "series" => row.poster.map(|p| format!("/api/v1/artwork/w342{p}")),
        _ => row.poster,
    };
    Ok(RequestView {
        id: row.id,
        user_id: row.user_id,
        username: row.username,
        kind: row.kind,
        service: row.service,
        tmdb_id: row.tmdb_id,
        season: row.season,
        mb_artist_id: row.mb_artist_id,
        mb_release_group_id: row.mb_release_group_id,
        mb_recording_id: row.mb_recording_id,
        title: row.title,
        year: row.year,
        artist: row.artist,
        poster,
        state: row.state,
        reason: row.reason,
        created_at: row.created_at,
        decided_at: row.decided_at,
        decided_by: row.decided_by,
        wanted_id: row.wanted_id,
        progress,
        seasons,
        search_started: false,
    })
}

/// A request by its id, whoever asked for it; `None` when there's no such request.
pub async fn get(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<RequestView>> {
    let row: Option<Row> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{} WHERE r.id = ?", columns())))
            .bind(id)
            .fetch_optional(pool)
            .await?;
    match row {
        Some(row) => Ok(Some(view(pool, row).await?)),
        None => Ok(None),
    }
}

/// Which requests [`list`] gives: a user's own (or everyone's), in one state, for one service.
#[derive(Debug, Default, Clone, Copy)]
pub struct Filter<'a> {
    /// Only this user's; None for everyone's, which only an admin may ask for.
    pub user_id: Option<i64>,
    /// requested, approved or declined.
    pub state: Option<&'a str>,
    /// movies, series, anime or music, as [`SERVICE`] places a request.
    pub service: Option<&'a str>,
}

/// A user's own requests, or everyone's for an admin who asks; newest first. `state` narrows
/// it to requested, approved or declined.
pub async fn list(
    pool: &SqlitePool,
    user_id: Option<i64>,
    state: Option<&str>,
    limit: i64,
    offset: i64,
) -> anyhow::Result<(i64, Vec<RequestView>)> {
    let filter = Filter {
        user_id,
        state,
        service: None,
    };
    list_by(pool, filter, limit, offset).await
}

/// The requests `filter` picks, newest first.
pub async fn list_by(
    pool: &SqlitePool,
    filter: Filter<'_>,
    limit: i64,
    offset: i64,
) -> anyhow::Result<(i64, Vec<RequestView>)> {
    let filter_sql = "WHERE (?1 IS NULL OR r.user_id = ?1) AND (?2 IS NULL OR r.state = ?2)
                        AND (?3 IS NULL OR r.service = ?3)";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM {} {filter_sql}",
        requests_with_service()
    )))
    .bind(filter.user_id)
    .bind(filter.state)
    .bind(filter.service)
    .fetch_one(pool)
    .await?;
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} {filter_sql} ORDER BY r.created_at DESC, r.id DESC LIMIT ?4 OFFSET ?5",
        columns()
    )))
    .bind(filter.user_id)
    .bind(filter.state)
    .bind(filter.service)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let mut views = Vec::with_capacity(rows.len());
    for row in rows {
        views.push(view(pool, row).await?);
    }
    Ok((total, views))
}
