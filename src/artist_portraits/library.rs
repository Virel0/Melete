//! Portraits for the artists in the library: a picture of their own beside their albums, else
//! Commons', looked up in the background a few a run rather than while anyone waits.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::library::ARTIST_MBID;

/// Artists one background run looks up at most. Each is a MusicBrainz request, at its one a
/// second shared with matching, so a run holds matching up by 20 seconds at most.
pub const LIBRARY_LOOKUPS: usize = 20;

/// How often a background run looks up library artists.
const EVERY: Duration = Duration::from_secs(3600);

/// The address the music API gives for the portrait of the library's artist `id`.
pub fn library_url(id: i64) -> String {
    format!("/api/v1/music/artists/{id}/portrait")
}

/// SQL to select from: the albums that give the artist whose id is `artist` (an SQL expression)
/// a picture of their own. Those present that match `visible` (a condition on `a5`), when every
/// one has the same picture in the same library, the one in the folder above it (`artist_file`,
/// which scans note), so they share that folder; no row when they don't, or there are none.
pub fn own_picture(artist: &str, visible: &str) -> String {
    format!(
        "FROM music_albums a5 JOIN libraries l5 ON l5.id = a5.library_id
         WHERE a5.artist_id = {artist} AND a5.missing_since IS NULL AND {visible}
         GROUP BY a5.artist_id HAVING count(a5.artist_file) = count(*)
             AND count(DISTINCT a5.library_id || '/' || a5.artist_file) = 1"
    )
}

/// SQL for the artists in the library now (an album there, or a track there crediting them)
/// that have a MusicBrainz id: `mb_artist_id`, and `own_picture`, whether they have one of
/// their own ([`own_picture`], whoever may see it).
pub(super) fn in_library() -> String {
    format!(
        "SELECT * FROM (SELECT {ARTIST_MBID}, EXISTS (SELECT 1 {}) AS own_picture
             FROM music_artists ar
             WHERE EXISTS (SELECT 1 FROM music_albums a
                     WHERE a.artist_id = ar.id AND a.missing_since IS NULL)
                 OR EXISTS (SELECT 1 FROM music_track_artists ta
                     JOIN music_tracks t ON t.id = ta.track_id AND t.missing_since IS NULL
                     WHERE ta.artist_id = ar.id))
         WHERE mb_artist_id IS NOT NULL",
        own_picture("ar.id", "1")
    )
}

/// What a background run did.
#[derive(Debug, Default, PartialEq)]
pub struct Looked {
    /// Artists looked up, whatever was found.
    pub looked_up: usize,
    /// Of those, the lookups that failed, kept nowhere so a later run asks again.
    pub failed: usize,
}

/// Looks up, with the setting on, at most [`LIBRARY_LOOKUPS`] artists in the library without a
/// picture of their own whose portrait isn't known or is out of date, never looked up first,
/// then the longest ago; none once Wikimedia asks for a pause, until it's over. What's found is kept as a search's lookups are, but not as listed.
///
/// # Errors
///
/// Fails when the database can't be read or written; a failed lookup is only counted.
pub async fn look_up_library(
    pool: &SqlitePool,
    services: &Services<'_>,
    now: i64,
) -> anyhow::Result<Looked> {
    if !crate::settings::artist_portraits(pool).await? {
        return Ok(Looked::default());
    }
    let artists: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT l.mb_artist_id FROM ({}) l
         LEFT JOIN artist_portraits p ON p.mb_artist_id = l.mb_artist_id
         GROUP BY l.mb_artist_id HAVING NOT max(l.own_picture)
         ORDER BY min(p.looked_up_at) IS NOT NULL, min(p.looked_up_at), l.mb_artist_id",
        in_library()
    )))
    .fetch_all(pool)
    .await?;
    let mut lookups = LIBRARY_LOOKUPS;
    let mut looked = Looked::default();
    for artist in artists.iter().filter(|a| crate::library::tags::is_mbid(a)) {
        // Out of lookups, or Wikimedia asked for a pause: the next run carries on.
        if lookups == 0 || services.wikimedia.held() {
            break;
        }
        let before = lookups;
        let known = known(pool, services, artist, &mut lookups, now).await?;
        if lookups < before {
            looked.looked_up += 1;
            looked.failed += usize::from(!known);
        }
    }
    Ok(looked)
}

/// Looks up library artists' portraits ([`look_up_library`]) once an hour and when an admin
/// asks, through the MusicBrainz client matching uses, so together they keep to its pace.
pub async fn run(
    pool: SqlitePool,
    mb: Arc<musicbrainz::Client>,
    wikimedia: Arc<wikimedia::Client>,
) {
    crate::jobs::repeat_task(
        "artist-portraits",
        EVERY,
        &crate::jobs::PORTRAITS,
        || async {
            let services = Services {
                musicbrainz: &mb,
                wikimedia: &wikimedia,
            };
            match look_up_library(&pool, &services, crate::db::unix_now()).await {
                Ok(looked) if looked == Looked::default() => {}
                Ok(looked) => tracing::info!(
                    looked_up = looked.looked_up,
                    failed = looked.failed,
                    "looked up library artists' portraits"
                ),
                Err(err) => tracing::error!(
                    error = format!("{err:#}"),
                    "looking up artists' portraits stopped"
                ),
            }
        },
    )
    .await;
}

/// The credits of the Commons portraits of `artists` (MusicBrainz ids) that the library route
/// may serve: with the setting on, those with a portrait not failed in the last day.
///
/// # Errors
///
/// Fails when the database can't be read.
pub async fn credits(
    pool: &SqlitePool,
    artists: &[&str],
    now: i64,
) -> sqlx::Result<HashMap<String, Credit>> {
    if artists.is_empty() || !crate::settings::artist_portraits(pool).await? {
        return Ok(HashMap::new());
    }
    #[derive(sqlx::FromRow)]
    struct Row {
        mb_artist_id: String,
        #[sqlx(flatten)]
        credit: Credit,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT mb_artist_id, author, licence, licence_url, source_url FROM artist_portraits
         WHERE mb_artist_id IN (SELECT value FROM json_each(?))
             AND file IS NOT NULL AND coalesce(failed_at, 0) < ?",
    )
    .bind(serde_json::to_string(artists).unwrap_or_else(|_| "[]".into()))
    .bind(now - FAIL_FOR)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.mb_artist_id, r.credit))
        .collect())
}
