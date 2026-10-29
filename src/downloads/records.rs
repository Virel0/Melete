//! A download as the list shows it, looked up by id.

use super::*;

/// A download as the list shows it: a torrent on the seedbox or something slskd fetches from
/// Soulseek, and how far it has got.
#[derive(Debug, PartialEq, Serialize)]
pub struct Download {
    /// The download's row id.
    pub id: i64,
    /// "torrent", or "soulseek" for a Soulseek user's album folder fetched by slskd.
    pub source: String,
    /// A torrent's hash; None for a Soulseek download.
    pub infohash: Option<String>,
    /// The release's title, as found in the search.
    pub title: String,
    /// The wanted movie it's for; None when it's for something else or that entry was removed.
    pub wanted_movie_id: Option<i64>,
    /// The wanted series it's for; None when it's for something else or that entry was removed.
    pub wanted_series_id: Option<i64>,
    /// The wanted album it's for; None when it's for something else or that entry was removed.
    pub wanted_album_id: Option<i64>,
    /// The wanted song it's for; None when it's for something else or that entry was removed.
    pub wanted_song_id: Option<i64>,
    /// The season it holds; None when it isn't one season of a series.
    pub season: Option<i64>,
    /// The episode it holds within `season`; None for a whole season or anything else.
    pub episode: Option<i64>,
    /// downloading (on the seedbox or with slskd), completed (all of it is there), transferring
    /// (being copied home), imported (in the library) or failed (see `message`).
    pub state: String,
    /// How much is downloaded, in percent (0 to 100).
    pub progress: f64,
    /// Its size in bytes; None until Deluge or slskd says.
    pub size: Option<i64>,
    /// Deluge's, slskd's or Melete's latest word on it; None when there is none.
    pub message: Option<String>,
    /// When it was grabbed, in Unix seconds.
    pub added_at: i64,
    /// When all of it was downloaded, in Unix seconds; None until then.
    pub completed_at: Option<i64>,
    /// The library folder it was copied into.
    pub destination: Option<String>,
    /// The library files it replaces once it's confirmed in place (relative to their library).
    pub replaces: Vec<String>,
    /// A finished Soulseek download waiting to be imported: its place in line (1 is next), as
    /// songs go first, so a requested track never waits behind album work.
    pub import_position: Option<i64>,
}

/// A download as stored: `replaces` is JSON.
#[derive(sqlx::FromRow)]
pub(super) struct DownloadRow {
    pub(super) id: i64,
    pub(super) source: String,
    pub(super) infohash: Option<String>,
    pub(super) title: String,
    pub(super) wanted_movie_id: Option<i64>,
    pub(super) wanted_series_id: Option<i64>,
    pub(super) wanted_album_id: Option<i64>,
    pub(super) wanted_song_id: Option<i64>,
    pub(super) season: Option<i64>,
    pub(super) episode: Option<i64>,
    pub(super) state: String,
    pub(super) progress: f64,
    pub(super) size: Option<i64>,
    pub(super) message: Option<String>,
    pub(super) added_at: i64,
    pub(super) completed_at: Option<i64>,
    pub(super) destination: Option<String>,
    pub(super) replaces: String,
    pub(super) import_position: Option<i64>,
}

impl From<DownloadRow> for Download {
    fn from(r: DownloadRow) -> Self {
        Download {
            id: r.id,
            source: r.source,
            infohash: r.infohash,
            title: r.title,
            wanted_movie_id: r.wanted_movie_id,
            wanted_series_id: r.wanted_series_id,
            wanted_album_id: r.wanted_album_id,
            wanted_song_id: r.wanted_song_id,
            season: r.season,
            episode: r.episode,
            state: r.state,
            progress: r.progress,
            size: r.size,
            message: r.message,
            added_at: r.added_at,
            completed_at: r.completed_at,
            destination: r.destination,
            // Written by the query above as an array of strings.
            replaces: serde_json::from_str(&r.replaces).unwrap_or_default(),
            import_position: r.import_position,
        }
    }
}

pub(super) const DOWNLOAD_COLUMNS: &str =
    "SELECT id, source, infohash, title, wanted_movie_id, wanted_series_id, wanted_album_id,
        wanted_song_id, season, episode, state, progress, size, message, added_at, completed_at, destination,
        (SELECT json_group_array(json_extract(value, '$.rel_path')) FROM json_each(replaces))
            AS replaces,
        -- In the order soulseek_import takes them: songs first, then the longest finished.
        CASE WHEN d.state = 'completed' AND d.source = 'soulseek' THEN 1 + (
            SELECT count(*) FROM downloads q WHERE q.state = 'completed' AND q.source = 'soulseek'
              AND (q.wanted_song_id IS NULL, q.completed_at, q.id)
                < (d.wanted_song_id IS NULL, d.completed_at, d.id)) END AS import_position
     FROM downloads d";

/// One download by id; None when there's no such download.
pub async fn get(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<Download>> {
    let row: Option<DownloadRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{DOWNLOAD_COLUMNS} WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(Download::from))
}
