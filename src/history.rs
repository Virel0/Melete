//! What happened to each wanted title, newest first: searched, grabbed, imported, failed,
//! upgraded, taken off, blocklisted. The database writes each event where the thing is recorded
//! (migration `title_events`); this lists them per service, per kind and per title, and a
//! title's past searches.

use serde::Serialize;
use sqlx::SqlitePool;

/// The kinds of event.
pub const KINDS: [&str; 10] = [
    "searched",
    "grabbed",
    "imported",
    "failed",
    "upgraded",
    "taken_off",
    "blocklisted",
    "extra_found",
    "renamed",
    "removed_from_deluge",
];

/// A managed service, as its history is kept: its name there, and which wanted titles are its
/// own rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    Movies,
    Series,
    Anime,
    /// Artists, with their albums and songs.
    Music,
}

impl Service {
    /// The service a managed list's path names: movies, series, anime or artists.
    pub fn parse(path: &str) -> Option<Service> {
        match path {
            "movies" => Some(Service::Movies),
            "series" => Some(Service::Series),
            "anime" => Some(Service::Anime),
            "artists" => Some(Service::Music),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Service::Movies => "movies",
            Service::Series => "series",
            Service::Anime => "anime",
            Service::Music => "music",
        }
    }

    /// The kind of wanted title a managed list's row is.
    fn row_kind(self) -> &'static str {
        match self {
            Service::Movies => "movie",
            Service::Series | Service::Anime => "series",
            Service::Music => "artist",
        }
    }
}

/// Records that a wanted title's folder (`table` `movies` or `series`, row `id`) was renamed
/// from `from` to `to`, as a scan found at `now`. Nothing when the title isn't wanted.
pub(crate) async fn folder_renamed(
    conn: &mut sqlx::SqliteConnection,
    (table, id): (&str, i64),
    (from, to): (&str, &str),
    now: i64,
) -> sqlx::Result<()> {
    let (kind, wanted) = match table {
        "series" => (
            "series",
            "SELECT id FROM wanted_series WHERE series_id = ?1",
        ),
        _ => ("movie", "SELECT id FROM wanted_movies WHERE movie_id = ?1"),
    };
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO title_events (at, kind, service, key, title, release, message)
         SELECT ?2, 'renamed', t.service, t.key, t.title, ?4, 'the folder took the place of ' || ?3
         FROM title_event_titles t WHERE t.kind = ?5 AND t.wanted_id IN ({wanted})"
    )))
    .bind(id)
    .bind(now)
    .bind(from)
    .bind(to)
    .bind(kind)
    .execute(conn)
    .await?;
    Ok(())
}

/// Records that file `new` took the place of `old` in a wanted title's folder (renamed, or
/// replaced by another tool), as a scan found at `now`. Nothing when the folder's title isn't
/// wanted.
pub(crate) async fn renamed(
    conn: &mut sqlx::SqliteConnection,
    old: i64,
    new: i64,
    now: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO title_events (at, kind, service, key, title, season, episode, release,
                                   message)
         SELECT ?3, 'renamed', t.service, t.key, t.title, n.season, n.episode, n.rel_path,
                'took the place of ' || o.rel_path
         FROM media_files n JOIN media_files o ON o.id = ?1
         JOIN title_event_titles t
             ON (t.kind = 'movie'
                 AND t.wanted_id IN (SELECT id FROM wanted_movies WHERE movie_id = n.movie_id))
             OR (t.kind = 'series'
                 AND t.wanted_id IN (SELECT id FROM wanted_series WHERE series_id = n.series_id))
         WHERE n.id = ?2",
    )
    .bind(old)
    .bind(new)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}

/// One thing that happened to a title.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct Event {
    /// The event's id.
    pub id: i64,
    /// When, in Unix seconds.
    pub at: i64,
    /// One of [`KINDS`].
    pub kind: String,
    /// The title as it was called then; for music, the album or song with its artist.
    pub title: String,
    /// For a series: the season, when the event was for one.
    pub season: Option<i64>,
    /// And the episode, when it was for one.
    pub episode: Option<i64>,
    /// The search, while it's kept (30 days); see `GET /api/v1/searches/{id}`.
    pub search_id: Option<i64>,
    /// The download, while it's kept.
    pub download_id: Option<i64>,
    /// The release or file involved.
    pub release: Option<String>,
    /// The latest word on it, in plain words.
    pub message: Option<String>,
}

/// What a list of events is narrowed to.
#[derive(Debug, Default)]
pub struct Filter {
    /// Only this kind.
    pub kind: Option<String>,
    /// Only this wanted title's (a managed list's `wanted_id`).
    pub wanted_id: Option<i64>,
}

/// A page of a service's events, newest first, and how many match; None when the wanted title
/// asked for isn't one of the service's.
pub async fn list(
    pool: &SqlitePool,
    service: Service,
    filter: &Filter,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Option<(i64, Vec<Event>)>> {
    let key = match filter.wanted_id {
        Some(id) => match key_of(pool, service, id).await? {
            Some(key) => Some(key),
            None => return Ok(None),
        },
        None => None,
    };
    const WHERE: &str =
        "WHERE service = ?1 AND (?2 IS NULL OR key = ?2) AND (?3 IS NULL OR kind = ?3)";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM title_events {WHERE}"
    )))
    .bind(service.name())
    .bind(&key)
    .bind(&filter.kind)
    .fetch_one(pool)
    .await?;
    let events: Vec<Event> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT id, at, kind, title, season, episode, search_id, download_id, release, message
         FROM title_events {WHERE} ORDER BY at DESC, id DESC LIMIT ?4 OFFSET ?5"
    )))
    .bind(service.name())
    .bind(&key)
    .bind(&filter.kind)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(Some((total, events)))
}

/// The key a wanted title's events are kept under, when it's one of the service's.
async fn key_of(pool: &SqlitePool, service: Service, id: i64) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "SELECT key FROM title_event_titles WHERE kind = ? AND wanted_id = ? AND service = ?",
    )
    .bind(service.row_kind())
    .bind(id)
    .bind(service.name())
    .fetch_optional(pool)
    .await
}

/// One of a title's searches, still kept.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct PastSearch {
    /// The search's id; its results are at `GET /api/v1/searches/{id}`.
    pub id: i64,
    /// When, in Unix seconds.
    pub searched_at: i64,
    /// For a series: the season searched, and the episode unless it was a season search.
    pub season: Option<i64>,
    /// The episode searched.
    pub episode: Option<i64>,
    /// For an artist: the album searched, by title.
    pub album: Option<String>,
    /// Whether an RSS sync made it, rather than a search.
    pub rss: bool,
    /// Results decided on.
    pub results: i64,
    /// Of those, the ones accepted.
    pub accepted: i64,
}

/// A wanted title's searches still kept, newest first (an artist's are its albums'); None when
/// the title isn't one of the service's.
pub async fn searches(
    pool: &SqlitePool,
    service: Service,
    wanted_id: i64,
) -> anyhow::Result<Option<Vec<PastSearch>>> {
    if key_of(pool, service, wanted_id).await?.is_none() {
        return Ok(None);
    }
    let whose = match service {
        Service::Movies => "s.wanted_movie_id = ?1",
        Service::Series | Service::Anime => "s.wanted_series_id = ?1",
        Service::Music => {
            "s.wanted_album_id IN (SELECT id FROM wanted_albums WHERE wanted_artist_id = ?1)"
        }
    };
    let found: Vec<PastSearch> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT s.id, s.searched_at, s.season, s.episode,
                (SELECT title FROM wanted_albums WHERE id = s.wanted_album_id) AS album,
                s.rss_sync_id IS NOT NULL AS rss,
                (SELECT count(*) FROM release_decisions d WHERE d.search_id = s.id) AS results,
                (SELECT count(*) FROM release_decisions d
                 WHERE d.search_id = s.id AND d.accepted = 1) AS accepted
         FROM searches s WHERE {whose} ORDER BY s.searched_at DESC, s.id DESC"
    )))
    .bind(wanted_id)
    .fetch_all(pool)
    .await?;
    Ok(Some(found))
}

#[cfg(test)]
mod tests;
