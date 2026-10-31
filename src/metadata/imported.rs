//! Folders Melete imported a download into, for a wanted title: matched at once to that title's
//! TMDB entry rather than searched for, since the download says which title it is. Only folders
//! not tried yet, and only when exactly one wanted title's download went there; anything else
//! goes through the normal, strict matching.

use super::*;

/// Each kind of title folder, with its wanted list, and the download column naming it.
struct Imported {
    kind: Kind,
    folders: &'static str,
    wanted: &'static str,
    download: &'static str,
}

const KINDS: [Imported; 2] = [
    Imported {
        kind: Kind::Movie,
        folders: "movies",
        wanted: "wanted_movies",
        download: "wanted_movie_id",
    },
    Imported {
        kind: Kind::Series,
        folders: "series",
        wanted: "wanted_series",
        download: "wanted_series_id",
    },
];

/// Matches the folders not tried yet that an imported download for a wanted title went into
/// (the folder itself, or for an episode the folder its season folder is in) to that title's
/// TMDB entry, fetched and stored as matching would; how many. One TMDB doesn't answer for is
/// left to normal matching.
pub(super) async fn match_imported(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    now: i64,
    report: &mut RefreshReport,
) -> anyhow::Result<()> {
    for imported in &KINDS {
        for (folder, tmdb_id) in imported_folders(pool, imported).await? {
            let stored =
                match imported.kind {
                    Kind::Movie => match tmdb.movie(tmdb_id).await {
                        Ok(Some(details)) => store_movie(pool, &details, now).await.map(|()| true),
                        Ok(None) => Ok(false),
                        Err(err) => skip(err, "imported movie", &tmdb_id.to_string(), report)
                            .map(|()| false),
                    },
                    Kind::Series => match fetch_series(tmdb, tmdb_id).await {
                        Ok(Some((details, seasons))) => store_series(pool, &details, &seasons, now)
                            .await
                            .map(|()| true),
                        Ok(None) => Ok(false),
                        Err(err) => skip(err, "imported series", &tmdb_id.to_string(), report)
                            .map(|()| false),
                    },
                }?;
            if stored && set_imported(pool, imported, folder, tmdb_id, now).await? {
                report.matched += 1;
            }
        }
    }
    Ok(())
}

/// The folders not tried yet that exactly one wanted title's imported download went into, with
/// that title's TMDB id.
async fn imported_folders(pool: &SqlitePool, imported: &Imported) -> sqlx::Result<Vec<(i64, i64)>> {
    let Imported {
        folders,
        wanted,
        download,
        ..
    } = imported;
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT f.id, min(w.tmdb_id) FROM {folders} f
         JOIN libraries l ON l.id = f.library_id
         JOIN downloads d ON d.state = 'imported' AND d.destination IS NOT NULL
             AND (d.destination = rtrim(l.path, '/') || '/' || f.rel_path
                  OR substr(d.destination, 1, length(rtrim(l.path, '/') || '/' || f.rel_path) + 1)
                     = rtrim(l.path, '/') || '/' || f.rel_path || '/')
         JOIN {wanted} w ON w.id = d.{download} AND w.tmdb_id IS NOT NULL
         WHERE f.missing_since IS NULL AND f.match_status IS NULL
           AND f.match_method IS NOT 'manual'
         GROUP BY f.id HAVING count(DISTINCT w.tmdb_id) = 1"
    )))
    .fetch_all(pool)
    .await
}

/// Records the folder matched to the wanted title's entry, unless it has been tried meanwhile.
async fn set_imported(
    pool: &SqlitePool,
    imported: &Imported,
    folder: i64,
    tmdb_id: i64,
    now: i64,
) -> sqlx::Result<bool> {
    let written = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {} SET match_status = 'matched', match_tmdb_id = ?, match_method = 'import',
             match_candidates = NULL, match_attempted_at = ?
         WHERE id = ? AND match_status IS NULL AND match_method IS NOT 'manual'",
        imported.folders
    )))
    .bind(tmdb_id)
    .bind(now)
    .bind(folder)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(written > 0)
}
