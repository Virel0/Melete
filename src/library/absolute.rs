//! Episodes numbered from the first ("Frieren - 05"): the scanner records the number, and
//! once a show is matched, this works out its season and episode by counting through TMDB's
//! episode list, seasons in order and specials (season 0) left out.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use sqlx::SqlitePool;

/// Gives season and episode numbers to files that only have an absolute one, for matched
/// shows. A number past the end of TMDB's list is left alone. Returns how many files it
/// numbered (or renumbered, when TMDB's list changed).
pub async fn number(pool: &SqlitePool) -> anyhow::Result<usize> {
    #[derive(sqlx::FromRow)]
    struct File {
        id: i64,
        show: i64,
        absolute: i64,
        season: Option<i64>,
        episode: Option<i64>,
    }
    let files: Vec<File> = sqlx::query_as(
        "SELECT f.id, s.match_tmdb_id AS show, f.absolute_episode AS absolute, f.season, f.episode
         FROM media_files f JOIN series s ON s.id = f.series_id
         WHERE f.absolute_episode IS NOT NULL AND (f.season IS NULL OR f.mapped = 1)
           AND f.missing_since IS NULL AND s.match_tmdb_id IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;
    let mut lists: HashMap<i64, Vec<(i64, i64)>> = HashMap::new();
    let mut numbered = 0;
    let mut tx = pool.begin().await?;
    for file in files {
        let list = match lists.entry(file.show) {
            Entry::Occupied(known) => known.into_mut(),
            Entry::Vacant(new) => new.insert(
                sqlx::query_as(
                    "SELECT season, episode FROM tmdb_episodes
                     WHERE series_tmdb_id = ? AND season > 0 ORDER BY season, episode",
                )
                .bind(file.show)
                .fetch_all(&mut *tx)
                .await?,
            ),
        };
        let Some(&(season, episode)) = usize::try_from(file.absolute - 1)
            .ok()
            .and_then(|i| list.get(i))
        else {
            continue;
        };
        if (file.season, file.episode) == (Some(season), Some(episode)) {
            continue;
        }
        sqlx::query("UPDATE media_files SET season = ?, episode = ?, episode_end = NULL, mapped = 1 WHERE id = ?")
            .bind(season)
            .bind(episode)
            .bind(file.id)
            .execute(&mut *tx)
            .await?;
        numbered += 1;
    }
    tx.commit().await?;
    Ok(numbered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Library, LibraryKind, scan};

    #[tokio::test]
    async fn numbers_anime_through_tmdbs_episode_list_and_keeps_it_across_scans() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "Frieren/[Group] Frieren - 05 [1080p].mkv",
            "Frieren/[Group] Frieren - 30 [1080p].mkv",
            "Frieren/[Group] Frieren - 99 [1080p].mkv",
            "Frieren/Frieren - S01E01.mkv",
            "Spy x Family/[SubsPlease] Spy x Family Season 2 - 05 (1080p).mkv",
        ] {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
        let pool = crate::db::open_in_memory().await;
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'anime', 'shows', ?)")
            .bind(dir.path().to_string_lossy().into_owned())
            .execute(&pool)
            .await
            .unwrap();
        let library = Library {
            id: 1,
            name: "anime".into(),
            kind: LibraryKind::Shows,
            path: dir.path().to_path_buf(),
        };
        scan(&pool, &library).await.unwrap();
        // TMDB: season 1 has 28 episodes, season 2 has 10, plus a special.
        sqlx::query("INSERT INTO tmdb_series (tmdb_id, name, genres, fetched_at) VALUES (209867, 'Frieren', '[]', 0)")
            .execute(&pool)
            .await
            .unwrap();
        for (season, count) in [(0, 1), (1, 28), (2, 10)] {
            for episode in 1..=count {
                sqlx::query("INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id) VALUES (209867, ?, ?, ?)")
                    .bind(season)
                    .bind(episode)
                    .bind(season * 1000 + episode)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }
        let numbers = || async {
            let rows: Vec<(String, Option<i64>, Option<i64>)> = sqlx::query_as(
                "SELECT rel_path, season, episode FROM media_files ORDER BY rel_path",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            rows
        };
        // Not matched yet: nothing to count through.
        assert_eq!(number(&pool).await.unwrap(), 0);
        sqlx::query("UPDATE series SET match_tmdb_id = 209867 WHERE parsed_title = 'Frieren'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(number(&pool).await.unwrap(), 2);
        let expected = vec![
            ("Frieren/Frieren - S01E01.mkv".to_owned(), Some(1), Some(1)),
            (
                "Frieren/[Group] Frieren - 05 [1080p].mkv".to_owned(),
                Some(1),
                Some(5),
            ),
            // Past season 1's 28, specials skipped: season 2, episode 2.
            (
                "Frieren/[Group] Frieren - 30 [1080p].mkv".to_owned(),
                Some(2),
                Some(2),
            ),
            // Past the end of TMDB's list: left alone.
            (
                "Frieren/[Group] Frieren - 99 [1080p].mkv".to_owned(),
                None,
                None,
            ),
            // An anime season in the name is read at scan time.
            (
                "Spy x Family/[SubsPlease] Spy x Family Season 2 - 05 (1080p).mkv".to_owned(),
                Some(2),
                Some(5),
            ),
        ];
        assert_eq!(numbers().await, expected);
        // A rescan keeps what was worked out, and there's nothing new to number.
        scan(&pool, &library).await.unwrap();
        assert_eq!(numbers().await, expected);
        assert_eq!(number(&pool).await.unwrap(), 0);
    }
}
