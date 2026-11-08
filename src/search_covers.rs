//! Covers of music not in the library, for the music results of a search for a request: an
//! exception to fetching covers only for albums in the library, on only with
//! `MELETE_SEARCH_COVERS` (and MusicBrainz). Kept to discovery posters' terms
//! ([`crate::discover_posters`]): only release groups a search listed are served, for 30 days
//! after they were last listed, at most [`MOST`] remembered. Each is fetched once from the
//! Cover Art Archive, which learns the release group's id and nothing else, and cached under
//! that id; housekeeping forgets the rest and deletes their files. Turned off, every one is
//! forgotten at the next housekeeping and none is served. A user an admin limited to some
//! libraries isn't given or served the cover of an album in a library hidden from them.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use crate::access::Access;

/// How long a listed cover is served.
pub const SERVED_FOR: i64 = 30 * 24 * 3600;
/// Covers remembered at most; older ones are forgotten first.
pub const MOST: i64 = 2000;
/// How long the archive having no cover for a release group is believed without asking again.
pub const MISS_FOR: i64 = 24 * 3600;

/// The address a search result gives for `group`'s cover.
pub fn url(group: &str) -> String {
    format!("/api/v1/music/release-groups/{group}/cover")
}

/// Where `group`'s cover and the sizes made from it are kept, under the music covers' folder.
/// `group` must be a MusicBrainz id, which can't climb out of it.
pub fn folder(covers: &Path, group: &str) -> PathBuf {
    covers.join("search").join(group)
}

/// Whether `group` isn't an album in a library hidden from a user with this access.
async fn open_to(pool: &SqlitePool, access: &Access, group: &str) -> sqlx::Result<bool> {
    if !access.limits_libraries() {
        return Ok(true);
    }
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT NOT EXISTS (SELECT 1 FROM music_albums a WHERE a.missing_since IS NULL
             AND ?1 IN (a.mb_release_group_id, a.match_release_group_id) AND NOT {})",
        access.album_sql("a")
    )))
    .bind(group)
    .fetch_one(pool)
    .await
}

/// A search result's cover, for the release group `group`: with the setting on, one the
/// archive isn't known to lack lately, and that the user may be given, is noted as listed and
/// its address given. Nothing is fetched here.
pub async fn offer(
    pool: &SqlitePool,
    access: &Access,
    group: Option<&str>,
    now: i64,
) -> anyhow::Result<Option<String>> {
    let Some(group) = group.filter(|g| crate::library::tags::is_mbid(g)) else {
        return Ok(None);
    };
    if !crate::settings::search_covers(pool).await? || !open_to(pool, access, group).await? {
        return Ok(None);
    }
    let lacking: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM search_covers
             WHERE release_group_id = ? AND missing_at >= ?)",
    )
    .bind(group)
    .bind(now - MISS_FOR)
    .fetch_one(pool)
    .await?;
    if lacking {
        return Ok(None);
    }
    sqlx::query(
        "INSERT INTO search_covers (release_group_id, listed_at) VALUES (?, ?)
         ON CONFLICT (release_group_id) DO UPDATE SET listed_at = excluded.listed_at",
    )
    .bind(group)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(Some(url(group)))
}

/// Whether the cover route may fetch and serve `group`'s cover to a user with this access: the
/// setting on, listed in the last 30 days, not lacking one lately, and open to them.
pub async fn servable_to(
    pool: &SqlitePool,
    access: &Access,
    group: &str,
    now: i64,
) -> anyhow::Result<bool> {
    if !crate::library::tags::is_mbid(group) || !crate::settings::search_covers(pool).await? {
        return Ok(false);
    }
    let listed: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM search_covers WHERE release_group_id = ?
             AND listed_at >= ? AND coalesce(missing_at, 0) < ?)",
    )
    .bind(group)
    .bind(now - SERVED_FOR)
    .bind(now - MISS_FOR)
    .fetch_one(pool)
    .await?;
    Ok(listed && open_to(pool, access, group).await?)
}

/// Notes that the archive has no front cover for `group`.
pub async fn lacking(pool: &SqlitePool, group: &str, now: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE search_covers SET missing_at = ? WHERE release_group_id = ?")
        .bind(now)
        .bind(group)
        .execute(pool)
        .await?;
    Ok(())
}

/// Forgets covers past their time, beyond [`MOST`], or all of them when the setting is off,
/// and deletes their cached files (under `covers`, the music covers' folder). How many were
/// forgotten.
pub async fn prune(pool: &SqlitePool, covers: &Path, now: i64) -> anyhow::Result<usize> {
    let on = crate::settings::search_covers(pool).await?;
    let gone: Vec<String> = sqlx::query_scalar(
        "SELECT release_group_id FROM search_covers
         WHERE NOT ?1 OR listed_at < ?2 OR release_group_id NOT IN (
             SELECT release_group_id FROM search_covers
             ORDER BY listed_at DESC, release_group_id LIMIT ?3)",
    )
    .bind(on)
    .bind(now - SERVED_FOR)
    .bind(MOST)
    .fetch_all(pool)
    .await?;
    for group in &gone {
        sqlx::query("DELETE FROM search_covers WHERE release_group_id = ?")
            .bind(group)
            .execute(pool)
            .await?;
        if !crate::library::tags::is_mbid(group) {
            continue;
        }
        match tokio::fs::remove_dir_all(folder(covers, group)).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(gone.len())
}

#[cfg(test)]
mod tests;
