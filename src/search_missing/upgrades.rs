//! What's below its profile's cutoff in an area, and whether it still is when its turn comes:
//! a run for upgrades searches each such movie, and each such episode on its own, so every
//! result is judged against exactly the file it would replace.

use super::*;

/// Every monitored movie, or monitored episode, whose file is below its profile's cutoff and
/// has nothing on its way, as the managed lists judge it. Albums aren't upgraded.
pub(super) async fn below_cutoff(pool: &SqlitePool, area: Area) -> anyhow::Result<Vec<Item>> {
    let mut items = Vec::new();
    match area {
        Area::Movies => {
            for title in crate::manage::movies(pool).await? {
                if let (Some(id), Some(true), None) =
                    (title.wanted_id, title.cutoff_unmet, &title.download)
                {
                    items.push(Item::Movie {
                        id,
                        title: title.title,
                    });
                }
            }
        }
        Area::Series => {
            for anime in [false, true] {
                for title in crate::manage::series(pool, anime).await? {
                    let Some(id) = title.wanted_id else { continue };
                    for (season, episode) in title.detail.unmet_episodes {
                        items.push(Item::Episode {
                            id,
                            title: title.title.clone(),
                            season,
                            episode,
                        });
                    }
                }
            }
        }
        Area::Albums => {}
    }
    Ok(items)
}

/// Whether an item is still monitored, below its cutoff as its own search would judge the
/// file, and without a download on its way, since a sync may have grabbed it meanwhile.
pub(super) async fn still_below_cutoff(pool: &SqlitePool, item: &Item) -> anyhow::Result<bool> {
    let (target, monitored, under_way): (Target, bool, bool) = match item {
        Item::Movie { id, .. } => {
            let (monitored, under_way) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT w.monitored = 1, EXISTS (SELECT 1 FROM downloads d
                     WHERE d.wanted_movie_id = w.id AND d.state IN {UNDER_WAY})
                 FROM wanted_movies w WHERE w.id = ?"
            )))
            .bind(id)
            .fetch_optional(pool)
            .await?
            .unwrap_or((false, false));
            (Target::Movie(*id), monitored, under_way)
        }
        Item::Episode {
            id,
            season,
            episode,
            ..
        } => {
            let (monitored, under_way) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT w.monitored = 1
                        AND EXISTS (SELECT 1 FROM wanted_seasons s WHERE s.wanted_series_id = w.id
                                    AND s.season = ?2 AND s.monitored = 1)
                        AND NOT EXISTS (SELECT 1 FROM wanted_episode_overrides o
                                        WHERE o.wanted_series_id = w.id AND o.season = ?2
                                          AND o.episode = ?3 AND o.monitored = 0),
                        EXISTS (SELECT 1 FROM downloads d
                                WHERE d.wanted_series_id = w.id AND d.state IN {UNDER_WAY}
                                  AND (d.season IS NULL OR (d.season = ?2
                                       AND (d.episode IS NULL OR d.episode = ?3))))
                 FROM wanted_series w WHERE w.id = ?1"
            )))
            .bind(id)
            .bind(season)
            .bind(episode)
            .fetch_optional(pool)
            .await?
            .unwrap_or((false, false));
            let target = Target::Series {
                id: *id,
                season: *season,
                episode: Some(*episode),
            };
            (target, monitored, under_way)
        }
        Item::Season { .. } | Item::Album { .. } => return Ok(false),
    };
    Ok(monitored && !under_way && search::cutoff_unmet(pool, target).await? == Some(true))
}

/// The download states of one under way.
const UNDER_WAY: &str = crate::manage::UNDER_WAY;

#[cfg(test)]
mod tests {
    use super::super::tests::{admin, finished, sources};
    use super::*;
    use crate::config::Secret;
    use crate::deluge;
    use crate::downloads::Fetcher;
    use crate::rss::tests::{indexer, releases, setup, want_dune, want_frieren};

    /// A file in the library: in folder `folder` (a movie's or a series'), at `name`.
    async fn file(pool: &SqlitePool, folder: (&str, i64), name: &str, episode: Option<(i64, i64)>) {
        let (column, id) = folder;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO media_files (library_id, rel_path, size, modified_at, {column}, season,
                 episode, first_seen_generation, seen_generation) VALUES (1, ?, 1, 0, ?, ?, ?, 1, 1)"
        )))
        .bind(name)
        .bind(id)
        .bind(episode.map(|e| e.0))
        .bind(episode.map(|e| e.1))
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn searches_and_grabs_upgrades_for_movies_below_their_cutoff() {
        let pool = setup().await;
        let who = admin(&pool).await;
        let dune = want_dune(&pool).await;
        // Dune's file is a 1080p web release, below 4K Preferred's cutoff; Arrival's reaches it.
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
                 VALUES (5, 1, 'Dune', 'Dune', 1), (6, 1, 'Arrival', 'Arrival', 1);
             UPDATE wanted_movies SET movie_id = 5;
             INSERT INTO wanted_movies (tmdb_id, title, year, monitored, source, added_at,
                     quality_profile, movie_id)
                 VALUES (329865, 'Arrival', 2016, 1, 'radarr', 0, '4K Preferred', 6);",
        )
        .execute(&pool)
        .await
        .unwrap();
        file(
            &pool,
            ("movie_id", 5),
            "Dune/Dune.Part.Two.2024.1080p.WEB-DL.mkv",
            None,
        )
        .await;
        let arrival = "Arrival/Arrival.2016.2160p.UHD.BluRay.REMUX.DV.HDR10.TrueHD.7.1.mkv";
        file(&pool, ("movie_id", 6), arrival, None).await;
        let feed = indexer(&pool, "[2000]").await;
        feed.lock().unwrap().0 = releases(&[(
            "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR",
            "",
        )]);
        let (url, _fake) = deluge::testing::start().await;
        let pipeline = Arc::new(Pipeline {
            deluge: deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
            fetcher: Fetcher::new().unwrap(),
            seedbox: None,
        });
        let start = |pipeline| {
            start_for(
                &pool,
                sources(Some(pipeline)),
                Area::Movies,
                Goal::CutoffUnmet,
                who,
            )
        };

        let run = start(pipeline.clone()).await.unwrap().unwrap();
        assert_eq!(run.goal, "cutoff_unmet");
        let run = finished(&pool, run.id).await;
        assert_eq!(
            (run.state.as_str(), run.total, run.searched, run.grabbed),
            ("done", 1, 1, 1),
            "{run:?}"
        );
        let searched: Vec<i64> = sqlx::query_scalar("SELECT wanted_movie_id FROM searches")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(searched, [dune], "Arrival reaches its cutoff");

        // The upgrade is on its way now, so a second run has nothing to do.
        let run = start(pipeline).await.unwrap().unwrap();
        let run = finished(&pool, run.id).await;
        assert_eq!((run.total, run.searched), (0, 0), "{run:?}");
    }

    #[tokio::test]
    async fn searches_each_monitored_episode_below_its_cutoff_on_its_own() {
        let pool = setup().await;
        let who = admin(&pool).await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'shows', 'shows', '/s');
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
                 VALUES (3, 1, 'Frieren', 'Frieren', 1);",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Season 1 is monitored but for episode 7; season 2 isn't.
        let frieren = want_frieren(&pool, Some(3)).await;
        let folder = ("series_id", 3);
        file(
            &pool,
            folder,
            "Frieren/Frieren.S01E01.1080p.WEB-DL.mkv",
            Some((1, 1)),
        )
        .await;
        let remux = "Frieren/Frieren.S01E02.2160p.BluRay.REMUX.DV.HDR10.TrueHD.7.1.mkv";
        file(&pool, folder, remux, Some((1, 2))).await;
        file(
            &pool,
            folder,
            "Frieren/Frieren.S01E07.720p.HDTV.mkv",
            Some((1, 7)),
        )
        .await;
        file(
            &pool,
            folder,
            "Frieren/Frieren.S02E01.720p.HDTV.mkv",
            Some((2, 1)),
        )
        .await;
        let feed = indexer(&pool, "[5000]").await;
        feed.lock().unwrap().0 = releases(&[]);

        let run = start_for(&pool, sources(None), Area::Series, Goal::CutoffUnmet, who)
            .await
            .unwrap()
            .unwrap();
        let run = finished(&pool, run.id).await;
        assert_eq!(
            (run.state.as_str(), run.total, run.searched),
            ("done", 1, 1),
            "{run:?}"
        );
        let searched: Vec<(i64, i64, Option<i64>)> =
            sqlx::query_as("SELECT wanted_series_id, season, episode FROM searches")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(searched, [(frieren, 1, Some(1))]);
    }
}
