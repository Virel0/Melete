//! Linking a wanted movie or series to its folder once the folder appears: a title wanted
//! before it was on disk (a request, an import list, adding it again) gets the folder its
//! download was imported into, or else the one folder it would have been adopted with had the
//! folder been there when it was added. Searches, upgrades, progress and the managed lists all
//! read this one link. A link once made is never changed, and nothing on disk is touched.

use super::*;

/// What a wanted title's folder is found by, per kind.
struct Kind {
    wanted: &'static str,
    link: &'static str,
    folders: &'static str,
    download: &'static str,
    /// The folder is the title's (for a series, also its season folder's parent).
    same_title: &'static str,
}

const MOVIES: Kind = Kind {
    wanted: "wanted_movies",
    link: "movie_id",
    folders: "movies",
    download: "wanted_movie_id",
    same_title: "(f.match_tmdb_id = w.tmdb_id OR f.arr_tmdb_id = w.tmdb_id OR f.tmdb_id = w.tmdb_id)",
};

const SERIES: Kind = Kind {
    wanted: "wanted_series",
    link: "series_id",
    folders: "series",
    download: "wanted_series_id",
    same_title: "(f.match_tmdb_id = w.tmdb_id OR f.arr_tmdb_id = w.tmdb_id OR f.tmdb_id = w.tmdb_id
                  OR f.arr_tvdb_id = w.tvdb_id OR f.tvdb_id = w.tvdb_id)",
};

/// Links every wanted movie and series without a folder to its folder, where exactly one is
/// found; how many were linked.
pub async fn link_folders(pool: &SqlitePool) -> anyhow::Result<u64> {
    let mut linked = 0;
    for kind in [&MOVIES, &SERIES] {
        linked += link(pool, kind, &by_download(kind)).await?;
        linked += link(pool, kind, &by_ids(kind)).await?;
    }
    Ok(linked)
}

/// The folder a download of the title was imported into: the folder itself, or for a series
/// the folder its season folder is in. One folder per title only.
fn by_download(kind: &Kind) -> String {
    let Kind {
        wanted,
        link,
        folders,
        download,
        ..
    } = kind;
    format!(
        "SELECT w.id, min(f.id) FROM {wanted} w
         JOIN downloads d ON d.{download} = w.id AND d.state = 'imported'
                         AND d.destination IS NOT NULL
         JOIN {folders} f ON f.missing_since IS NULL
         JOIN libraries l ON l.id = f.library_id
         WHERE w.{link} IS NULL
           AND (d.destination = rtrim(l.path, '/') || '/' || f.rel_path
                OR substr(d.destination, 1, length(rtrim(l.path, '/') || '/' || f.rel_path) + 1)
                   = rtrim(l.path, '/') || '/' || f.rel_path || '/')
         GROUP BY w.id HAVING count(DISTINCT f.id) = 1"
    )
}

/// The one present folder matched to the title's TMDB (or TheTVDB) id that no other wanted
/// title has, as adding the title would have adopted it.
fn by_ids(kind: &Kind) -> String {
    let Kind {
        wanted,
        link,
        folders,
        same_title,
        ..
    } = kind;
    format!(
        "SELECT w.id, min(f.id) FROM {wanted} w
         JOIN {folders} f ON f.missing_since IS NULL AND {same_title}
         WHERE w.{link} IS NULL
           AND NOT EXISTS (SELECT 1 FROM {wanted} o WHERE o.{link} = f.id)
         GROUP BY w.id HAVING count(DISTINCT f.id) = 1"
    )
}

/// Sets the links `found` gives, where none is set yet.
async fn link(pool: &SqlitePool, kind: &Kind, found: &str) -> anyhow::Result<u64> {
    let pairs: Vec<(i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(found.to_owned()))
        .fetch_all(pool)
        .await?;
    let mut linked = 0;
    for (wanted, folder) in pairs {
        linked += sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {} SET {link} = ?1 WHERE id = ?2 AND {link} IS NULL
               AND NOT EXISTS (SELECT 1 FROM {} WHERE {link} = ?1)",
            kind.wanted,
            kind.wanted,
            link = kind.link,
        )))
        .bind(folder)
        .bind(wanted)
        .execute(pool)
        .await?
        .rows_affected();
    }
    Ok(linked)
}

#[cfg(test)]
mod tests;
