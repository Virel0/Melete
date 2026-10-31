//! Grabbing from an RSS sync: the best accepted release for a title or album, and whether anything
//! is on disk or on its way where it goes.

use super::*;

/// A grab an RSS sync made or tried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grabbed {
    /// The result's row id (its decision's).
    pub release_id: i64,
    /// The release name.
    pub title: String,
    /// It replaces files on disk once confirmed.
    #[serde(default)]
    pub upgrade: bool,
    /// The download it started; `None` when it was refused.
    pub download_id: Option<i64>,
    /// Why it wasn't grabbed.
    pub refused: Option<String>,
}

/// Grabs the best accepted release of any search for `target`, as a sync would (requests do,
/// once approved).
pub(crate) async fn grab_for(
    pool: &SqlitePool,
    pipeline: &Pipeline,
    target: Target,
    search_id: i64,
    grabbed: &mut Vec<Grabbed>,
) -> anyhow::Result<()> {
    match search::load_wanted(pool, target).await? {
        Some(wanted) => grab_best(pool, pipeline, target, &wanted, search_id, grabbed).await,
        None => Ok(()),
    }
}

/// Grabs the best accepted release of each search a sync kept: the titles', then the albums'.
pub(super) async fn grab_all(
    pool: &SqlitePool,
    pipeline: &Pipeline,
    candidates: &[Candidate],
    stored: &[(Target, usize, i64)],
    stored_albums: &[(i64, i64)],
) -> anyhow::Result<Vec<Grabbed>> {
    let mut grabbed = Vec::new();
    for (target, n, search_id) in stored {
        grab_best(
            pool,
            pipeline,
            *target,
            &candidates[*n].wanted,
            *search_id,
            &mut grabbed,
        )
        .await?;
    }
    for (album_id, search_id) in stored_albums {
        grab_album(pool, pipeline, *album_id, *search_id, &mut grabbed).await?;
    }
    Ok(grabbed)
}

/// Grabs the best accepted release of a sync's search for `target`, when [`grab_kind`] allows.
pub(super) async fn grab_best(
    pool: &SqlitePool,
    pipeline: &Pipeline,
    target: Target,
    wanted: &Wanted,
    search_id: i64,
    grabbed: &mut Vec<Grabbed>,
) -> anyhow::Result<()> {
    // Its downloads kept failing the running-time check: nothing more until an admin looks.
    if crate::runtime_checks::stopped(pool, crate::runtime_checks::title_of(target)).await? {
        return Ok(());
    }
    let best: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, title FROM release_decisions WHERE search_id = ? AND accepted = 1
           AND lower(title) NOT IN (SELECT lower(title) FROM blocklist)
         ORDER BY json_extract(decision, '$.rank') DESC NULLS LAST,
                  json_extract(decision, '$.score') DESC, seeders DESC NULLS LAST, id
         LIMIT ?",
    )
    .bind(search_id)
    .bind(GRAB_TRIES)
    .fetch_all(pool)
    .await?;
    for (release_id, title) in best {
        // Accepted against a file there means better than it, below the cutoff.
        let Some(upgrade) = grab_kind(pool, target, wanted, &title).await? else {
            continue;
        };
        let outcome =
            downloads::grab(pool, &pipeline.deluge, &pipeline.fetcher, release_id).await?;
        let (download_id, refused, try_next) = match outcome {
            Ok(download) => (Some(download.id), None, false),
            // The indexer or Deluge failed for this one: the next may do.
            Err(
                refused @ (Refused::Fetch(_)
                | Refused::Deluge(_)
                | Refused::NoLink
                | Refused::Blocklisted),
            ) => (None, Some(refused.to_string()), true),
            Err(refused) => (None, Some(refused.to_string()), false),
        };
        grabbed.push(Grabbed {
            release_id,
            title,
            upgrade,
            download_id,
            refused,
        });
        if !try_next {
            break;
        }
    }
    Ok(())
}

/// A release listed in music categories only.
pub(super) fn is_music(item: &torznab::Item) -> bool {
    !item.categories.is_empty() && item.categories.iter().all(|c| (3000..4000).contains(c))
}

/// Grabs the best accepted release of a sync's search for a wanted album, when the album
/// isn't on disk and nothing is downloading for it.
pub(super) async fn grab_album(
    pool: &SqlitePool,
    pipeline: &Pipeline,
    album_id: i64,
    search_id: i64,
    grabbed: &mut Vec<Grabbed>,
) -> anyhow::Result<()> {
    if album_search::taken(pool, album_id).await? {
        return Ok(());
    }
    let best: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, title FROM release_decisions WHERE search_id = ? AND accepted = 1
           AND lower(title) NOT IN (SELECT lower(title) FROM blocklist)
         ORDER BY json_extract(decision, '$.rank') DESC NULLS LAST, seeders DESC NULLS LAST, id
         LIMIT ?",
    )
    .bind(search_id)
    .bind(GRAB_TRIES)
    .fetch_all(pool)
    .await?;
    for (release_id, title) in best {
        let outcome =
            downloads::grab(pool, &pipeline.deluge, &pipeline.fetcher, release_id).await?;
        let (download_id, refused, try_next) = match outcome {
            Ok(download) => (Some(download.id), None, false),
            Err(
                refused @ (Refused::Fetch(_)
                | Refused::Deluge(_)
                | Refused::NoLink
                | Refused::Blocklisted),
            ) => (None, Some(refused.to_string()), true),
            Err(refused) => (None, Some(refused.to_string()), false),
        };
        grabbed.push(Grabbed {
            release_id,
            title,
            upgrade: false,
            download_id,
            refused,
        });
        if !try_next {
            break;
        }
    }
    Ok(())
}

/// What's where a release would go: the movie, the episodes it covers, or the whole season
/// for a pack. Files count in the wanted title's adopted folder and in any other folder known
/// by the title's ids.
pub(super) struct Occupied {
    pub(super) files: i64,
    /// Downloads for it that haven't failed.
    pub(super) downloads: i64,
    /// Of those, ones an upgrade must wait for: still on the way, or imported and not settled
    /// (an upgrade whose old file hasn't been dealt with, or a new file not scanned yet).
    pub(super) unsettled: i64,
}

/// Downloads an upgrade waits for, as a condition on `downloads`.
pub(super) const UNSETTLED: &str = "(state IN ('downloading', 'completed', 'transferring')
    OR (state = 'imported' AND replaces <> '[]' AND retired_at IS NULL)
    OR (state = 'imported' AND replaces = '[]'
        AND updated_at >= coalesce((SELECT min(last_scan_at) FROM libraries), 0)))";

pub(super) async fn occupied(
    pool: &SqlitePool,
    target: Target,
    wanted: &Wanted,
    release: &Release,
) -> anyhow::Result<Occupied> {
    Ok(match target {
        Target::Movie(id) => {
            let files: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM media_files f JOIN movies m ON m.id = f.movie_id
                 WHERE f.missing_since IS NULL
                   AND (m.id = ? OR m.match_tmdb_id = ? OR m.arr_tmdb_id = ? OR m.tmdb_id = ?)",
            )
            .bind(wanted.movie_id)
            .bind(wanted.tmdb_id)
            .bind(wanted.tmdb_id)
            .bind(wanted.tmdb_id)
            .fetch_one(pool)
            .await?;
            let (downloads, unsettled): (i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT count(*), coalesce(sum({UNSETTLED}), 0) FROM downloads
                 WHERE wanted_movie_id = ? AND state <> 'failed'"
            )))
            .bind(id)
            .fetch_one(pool)
            .await?;
            Occupied {
                files,
                downloads,
                unsettled,
            }
        }
        Target::Series { id, season, .. } => {
            // The episodes it covers; a season pack covers them all.
            let (pack, first, last) = match &release.episodes {
                Episodes::Standard { episodes, .. } => (
                    false,
                    episodes.iter().min().copied().unwrap_or(0),
                    episodes.iter().max().copied().unwrap_or(0),
                ),
                _ => (true, i64::MIN, i64::MAX),
            };
            let files: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM media_files f JOIN series s ON s.id = f.series_id
                 WHERE f.season = ? AND f.missing_since IS NULL
                   AND f.episode <= ? AND coalesce(f.episode_end, f.episode) >= ?
                   AND (s.id = ? OR s.match_tmdb_id = ? OR s.arr_tmdb_id = ? OR s.tmdb_id = ?
                        OR s.arr_tvdb_id = ? OR s.tvdb_id = ?)",
            )
            .bind(season)
            .bind(last)
            .bind(first)
            .bind(wanted.series_id)
            .bind(wanted.tmdb_id)
            .bind(wanted.tmdb_id)
            .bind(wanted.tmdb_id)
            .bind(wanted.tvdb_id)
            .bind(wanted.tvdb_id)
            .fetch_one(pool)
            .await?;
            let (downloads, unsettled): (i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT count(*), coalesce(sum({UNSETTLED}), 0) FROM downloads
                 WHERE wanted_series_id = ? AND season = ? AND state <> 'failed'
                   AND (? OR episode IS NULL OR episode BETWEEN ? AND ?)"
            )))
            .bind(id)
            .bind(season)
            .bind(pack)
            .bind(first)
            .bind(last)
            .fetch_one(pool)
            .await?;
            Occupied {
                files,
                downloads,
                unsettled,
            }
        }
    })
}

/// Why a release may be grabbed: nothing is where it goes, or it's an upgrade of exactly the
/// files there, with nothing on the way or unsettled. `None` when it may not.
pub(super) async fn grab_kind(
    pool: &SqlitePool,
    target: Target,
    wanted: &Wanted,
    title: &str,
) -> anyhow::Result<Option<bool>> {
    let release = release::parse(title);
    let here = occupied(pool, target, wanted, &release).await?;
    if here.files == 0 && here.downloads == 0 {
        return Ok(Some(false));
    }
    if here.files == 0 || here.unsettled > 0 {
        return Ok(None);
    }
    let (movie, series, season) = match target {
        Target::Movie(id) => (Some(id), None, None),
        Target::Series { id, season, .. } => (None, Some(id), Some(season)),
    };
    let replaced = crate::upgrades::replaced_by(pool, movie, series, season, title).await?;
    // Every file there is one it replaces: none in another folder, none left beside it.
    Ok((!replaced.is_empty() && replaced.len() as i64 == here.files).then_some(true))
}
