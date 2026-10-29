//! Posters of titles not in the library: a limited exception to serving artwork only for titles
//! in the library or asked for (which keeps the server from becoming a proxy for TMDB's images),
//! on only with `MELETE_DISCOVERY_POSTERS`. Only posters a discovery answer listed (trending,
//! similar, search, a title by its id: one page of TMDB's, 20 or so, at a time) are served, at
//! w342 only, for 30 days after they were last listed; at most [`MOST`] are remembered.
//! Housekeeping forgets the rest and deletes their cached files, unless a title in the library or
//! a request still names them. Turned off, every one is forgotten at the next housekeeping and
//! none is served. Each poster also remembers the titles it was listed for, so a user with a
//! highest rating is served it only for a title rated within their limit.

use std::path::Path;

use sqlx::SqlitePool;

/// How long a listed poster is served.
pub const SERVED_FOR: i64 = 30 * 24 * 3600;
/// Posters remembered at most; older ones are forgotten first.
pub const MOST: i64 = 2000;
/// The one size they're served at.
pub const SIZE: &str = "w342";

/// Whether the artwork route may fetch and serve `file` (as `/name.jpg`) as a listed poster.
pub async fn servable(pool: &SqlitePool, file: &str, now: i64) -> anyhow::Result<bool> {
    if !crate::settings::discovery_posters(pool).await? {
        return Ok(false);
    }
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM discover_posters WHERE file = ? AND listed_at >= ?)",
    )
    .bind(file)
    .bind(now - SERVED_FOR)
    .fetch_one(pool)
    .await?)
}

/// Whether the artwork route may serve `file` (as `/name.jpg`) as a listed poster to a user
/// with this access: as [`servable`], and for a user with a highest rating, only when one of the
/// titles it was listed for is rated within it (as discovery judges those titles).
pub async fn servable_to(
    pool: &SqlitePool,
    access: &crate::access::Access,
    file: &str,
    now: i64,
) -> anyhow::Result<bool> {
    if !servable(pool, file, now).await? {
        return Ok(false);
    }
    if !access.limits_rating() {
        return Ok(true);
    }
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT EXISTS (SELECT 1 FROM discover_poster_titles t
             WHERE t.file = ? AND ((t.tv = 0 AND {movie}) OR (t.tv = 1 AND {series})))",
        movie = access.rating_sql(false, "t.tmdb_id"),
        series = access.rating_sql(true, "t.tmdb_id"),
    )))
    .bind(file)
    .fetch_one(pool)
    .await?)
}

/// Notes that a discovery answer listed `file` for the title with TMDB id `tmdb_id` (a
/// series' with `tv`), as [`listed`] does, keeping which title it was.
pub async fn listed_for(
    pool: &SqlitePool,
    file: &str,
    tv: bool,
    tmdb_id: i64,
    now: i64,
) -> anyhow::Result<()> {
    listed(pool, file, now).await?;
    sqlx::query(
        "INSERT INTO discover_poster_titles (file, tv, tmdb_id) VALUES (?, ?, ?)
         ON CONFLICT DO NOTHING",
    )
    .bind(file)
    .bind(tv)
    .bind(tmdb_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Notes that a discovery answer listed `file`.
pub async fn listed(pool: &SqlitePool, file: &str, now: i64) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO discover_posters (file, listed_at) VALUES (?, ?)
         ON CONFLICT (file) DO UPDATE SET listed_at = excluded.listed_at",
    )
    .bind(file)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Forgets posters past their time, beyond [`MOST`], or all of them when the setting is off,
/// and deletes their cached files that nothing else names. How many were forgotten.
pub async fn prune(pool: &SqlitePool, artwork_dir: &Path, now: i64) -> anyhow::Result<usize> {
    let on = crate::settings::discovery_posters(pool).await?;
    let gone: Vec<String> = sqlx::query_scalar(
        "SELECT file FROM discover_posters
         WHERE NOT ?1 OR listed_at < ?2 OR file NOT IN (
             SELECT file FROM discover_posters ORDER BY listed_at DESC, file LIMIT ?3)",
    )
    .bind(on)
    .bind(now - SERVED_FOR)
    .bind(MOST)
    .fetch_all(pool)
    .await?;
    for file in &gone {
        sqlx::query("DELETE FROM discover_posters WHERE file = ?")
            .bind(file)
            .execute(pool)
            .await?;
        let named: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM tmdb_movies WHERE poster_path = ?1
                            UNION ALL SELECT 1 FROM tmdb_series WHERE poster_path = ?1
                            UNION ALL SELECT 1 FROM requests WHERE poster = ?1)",
        )
        .bind(file)
        .fetch_one(pool)
        .await?;
        let name = file.trim_start_matches('/');
        if !named && !name.is_empty() && !name.contains(['/', '\\']) && name != ".." {
            match tokio::fs::remove_file(artwork_dir.join(SIZE).join(name)).await {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
    }
    Ok(gone.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn turn(pool: &SqlitePool, on: bool) {
        sqlx::query(
            "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_DISCOVERY_POSTERS', ?, 0)
             ON CONFLICT (name) DO UPDATE SET value = excluded.value",
        )
        .bind(if on { "on" } else { "off" })
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn served_for_thirty_days_while_on_then_forgotten_with_their_files() {
        let pool = db::open_in_memory().await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(SIZE)).unwrap();
        for name in ["old.jpg", "new.jpg", "kept.jpg"] {
            std::fs::write(dir.path().join(SIZE).join(name), b"jpeg").unwrap();
        }
        let now = 100 * 24 * 3600;
        listed(&pool, "/old.jpg", now - SERVED_FOR - 1)
            .await
            .unwrap();
        listed(&pool, "/kept.jpg", now - SERVED_FOR - 1)
            .await
            .unwrap();
        listed(&pool, "/new.jpg", now - 10).await.unwrap();
        // A library title still names this one, so its file stays.
        sqlx::query("INSERT INTO tmdb_movies (tmdb_id, title, poster_path, fetched_at) VALUES (1, 'T', '/kept.jpg', 0)")
            .execute(&pool)
            .await
            .unwrap();

        assert!(
            !servable(&pool, "/new.jpg", now).await.unwrap(),
            "off by default"
        );
        turn(&pool, true).await;
        assert!(servable(&pool, "/new.jpg", now).await.unwrap());
        assert!(
            !servable(&pool, "/old.jpg", now).await.unwrap(),
            "past its time"
        );
        assert!(
            !servable(&pool, "/other.jpg", now).await.unwrap(),
            "never listed"
        );

        assert_eq!(prune(&pool, dir.path(), now).await.unwrap(), 2);
        assert!(!dir.path().join("w342/old.jpg").exists());
        assert!(
            dir.path().join("w342/kept.jpg").exists(),
            "named by a title"
        );
        assert!(dir.path().join("w342/new.jpg").exists());

        turn(&pool, false).await;
        assert_eq!(
            prune(&pool, dir.path(), now).await.unwrap(),
            1,
            "off: all go"
        );
        assert!(!dir.path().join("w342/new.jpg").exists());
    }

    #[tokio::test]
    async fn at_most_two_thousand_are_remembered() {
        let pool = db::open_in_memory().await;
        let dir = tempfile::tempdir().unwrap();
        turn(&pool, true).await;
        let now = 100 * 24 * 3600;
        for n in 0..MOST + 3 {
            listed(&pool, &format!("/{n}.jpg"), now - 1000 + n)
                .await
                .unwrap();
        }
        assert_eq!(prune(&pool, dir.path(), now).await.unwrap(), 3);
        assert!(
            !servable(&pool, "/0.jpg", now).await.unwrap(),
            "the oldest went"
        );
        assert!(
            servable(&pool, &format!("/{}.jpg", MOST + 2), now)
                .await
                .unwrap()
        );
    }
}
