//! Searching for everything missing in an area at once, as the *arrs' "Search all
//! missing" does: every monitored wanted movie that isn't in a library or on its way, every
//! monitored season of a monitored series still missing episodes, or every monitored album of a
//! monitored artist that isn't on disk or downloading. Each is searched as its own search would
//! be (indexers, and Soulseek for albums) and, when downloads are on, the best accepted result
//! is grabbed, as an RSS sync or an approved request does.
//!
//! Pacing: a run makes one search at a time, and starts the next no sooner than [`PACE`] after
//! the last one ended, so each indexer is asked at most once per pace from a run. Indexers
//! have no limits recorded in Melete, so this is the run's own ceiling; when an indexer answers
//! that it has reached its limit, the rest of the run leaves it out, and the run stops only when
//! every indexer has (an album search still asks Soulseek). One run at a
//! time per area (the database's own constraint); it can be asked to stop, and finishes the
//! search under way first.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::downloads::Pipeline;
use crate::progress::{self, Step};
use crate::rss::Grabbed;
use crate::search::{self, IndexerOutcome, Refused, Target};
use crate::{album_search, artist_search, slskd, tmdb, torznab};

mod control;
mod finding;
mod records;
mod running;
mod upgrades;

pub use control::*;
use finding::*;
pub use records::*;
pub(crate) use running::*;
use upgrades::*;

/// The least time from the end of one search in a run to the start of the next.
pub const PACE: Duration = Duration::from_secs(15);

/// Notes kept on a run at most; the rest are counted.
const NOTES_KEPT: usize = 50;

/// What a run needs from outside.
#[derive(Clone)]
pub struct Sources {
    /// TMDB, for movie and series details; None when it isn't set up.
    pub tmdb: Option<Arc<tmdb::Client>>,
    /// slskd, so album searches also ask Soulseek; None when it isn't set up.
    pub slskd: Option<Arc<slskd::Client>>,
    /// Deluge, for torrents; None when downloads are off.
    pub pipeline: Option<Arc<Pipeline>>,
    /// [`PACE`], but none in tests.
    pub pace: Duration,
}

/// Which kind of wanted title a run searches for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    Movies,
    Series,
    Albums,
}

impl Area {
    /// The area named "movies", "series" or "albums"; `None` for any other text.
    pub fn parse(text: &str) -> Option<Area> {
        match text {
            "movies" => Some(Area::Movies),
            "series" => Some(Area::Series),
            "albums" => Some(Area::Albums),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Area::Movies => "movies",
            Area::Series => "series",
            Area::Albums => "albums",
        }
    }
}

/// What a run searches for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Goal {
    /// What's missing.
    Missing,
    /// Upgrades of files below their profile's cutoff: movies and episodes, each on its own.
    CutoffUnmet,
}

impl Goal {
    fn as_str(self) -> &'static str {
        match self {
            Goal::Missing => "missing",
            Goal::CutoffUnmet => "cutoff_unmet",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Secret;
    use crate::db;
    use crate::deluge;
    use crate::downloads::Fetcher;
    use crate::rss::tests::{indexer, releases, setup, want_dune, want_frieren};

    pub(super) async fn admin(pool: &SqlitePool) -> i64 {
        crate::auth::create_user(pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap()
            .id
    }

    pub(super) fn sources(pipeline: Option<Arc<Pipeline>>) -> Sources {
        Sources {
            tmdb: None,
            slskd: None,
            pipeline,
            pace: Duration::ZERO,
        }
    }

    pub(super) async fn finished(pool: &SqlitePool, id: i64) -> MissingSearch {
        for _ in 0..500 {
            let run = get(pool, id).await.unwrap().unwrap();
            if !matches!(run.state.as_str(), "running" | "stopping") {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("missing search {id} didn't finish");
    }

    #[test]
    fn knows_an_indexer_at_its_limit() {
        for limited in [
            torznab::Error::Status(429).to_string(),
            torznab::Error::Indexer {
                code: "500".into(),
                description: "Request limit reached".into(),
            }
            .to_string(),
        ] {
            assert!(says_limit(&limited), "{limited}");
        }
        for other in [
            torznab::Error::Status(500).to_string(),
            torznab::Error::Rejected.to_string(),
            torznab::Error::Indexer {
                code: "201".into(),
                description: "Incorrect parameter".into(),
            }
            .to_string(),
        ] {
            assert!(!says_limit(&other), "{other}");
        }
    }

    #[tokio::test]
    async fn searches_every_missing_movie_once_and_grabs_the_best() {
        let pool = setup().await;
        let who = admin(&pool).await;
        let dune = want_dune(&pool).await;
        // Arrival is in a library, Alien isn't monitored, and Heat's download is on its way.
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
                 VALUES (5, 1, 'Arrival (2016)', 'Arrival', 1);
             INSERT INTO media_files (library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, movie_id)
                 VALUES (1, 'Arrival (2016)/Arrival.mkv', 1, 0, 1, 1, 5);
             INSERT INTO wanted_movies (tmdb_id, title, year, monitored, source, added_at,
                     quality_profile, movie_id) VALUES
                 (329865, 'Arrival', 2016, 1, 'radarr', 0, '4K Preferred', 5),
                 (348, 'Alien', 1979, 0, 'radarr', 0, '4K Preferred', NULL),
                 (949, 'Heat', 1995, 1, 'radarr', 0, '4K Preferred', NULL);
             INSERT INTO downloads (infohash, title, wanted_movie_id, decision, state,
                     added_at, updated_at)
                 SELECT 'a11bae70b2fdd9eb95b63e746f8594dce2650b0c', 'Heat', id, '{}',
                     'downloading', 0, 0 FROM wanted_movies WHERE tmdb_id = 949;",
        )
        .execute(&pool)
        .await
        .unwrap();
        let feed = indexer(&pool, "[2000]").await;
        feed.lock().unwrap().0 = releases(&[
            ("Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX", ""),
            (
                "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR",
                "",
            ),
        ]);
        let (url, _fake) = deluge::testing::start().await;
        let pipeline = Arc::new(Pipeline {
            deluge: deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
            fetcher: Fetcher::new().unwrap(),
            seedbox: None,
        });

        let run = start(&pool, sources(Some(pipeline.clone())), Area::Movies, who)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.state, "running");
        assert_eq!(run.started_by.as_deref(), Some("admin"));
        let run = finished(&pool, run.id).await;
        assert_eq!(run.state, "done", "{run:?}");
        assert_eq!(
            (run.total, run.done, run.searched, run.grabbed),
            (1, 1, 1, 1),
            "{run:?}"
        );
        let searched: Vec<i64> =
            sqlx::query_scalar("SELECT wanted_movie_id FROM searches ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            searched,
            [dune],
            "only Dune was missing with nothing on its way"
        );
        let grabbed: Vec<String> =
            sqlx::query_scalar("SELECT title FROM downloads WHERE wanted_movie_id = ?")
                .bind(dune)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            grabbed,
            ["Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR"]
        );

        // Dune is on its way now, so a second run finds nothing missing.
        let run = start(&pool, sources(Some(pipeline)), Area::Movies, who)
            .await
            .unwrap()
            .unwrap();
        let run = finished(&pool, run.id).await;
        assert_eq!((run.total, run.searched), (0, 0));
        assert_eq!(latest(&pool).await.unwrap(), [run]);
    }

    #[tokio::test]
    async fn searches_each_missing_season_of_a_series() {
        let pool = setup().await;
        let who = admin(&pool).await;
        // Season 1 monitored, season 2 not: only season 1 is searched for.
        let frieren = want_frieren(&pool, None).await;
        let feed = indexer(&pool, "[5000]").await;
        feed.lock().unwrap().0 = releases(&[]);
        let run = start(&pool, sources(None), Area::Series, who)
            .await
            .unwrap()
            .unwrap();
        let run = finished(&pool, run.id).await;
        assert_eq!(
            (run.state.as_str(), run.total, run.searched),
            ("done", 1, 1)
        );
        assert!(run.notes[0].starts_with("downloads are off"), "{run:?}");
        let searched: Vec<(i64, i64)> =
            sqlx::query_as("SELECT wanted_series_id, season FROM searches")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(searched, [(frieren, 1)]);
    }

    #[tokio::test]
    async fn searches_monitored_albums_not_on_disk_or_on_their_way() {
        let pool = db::open_in_memory().await;
        let who = admin(&pool).await;
        // OK Computer is missing; Kid A is downloading; Amnesiac isn't monitored; Hail to the
        // Thief is by an artist who isn't monitored.
        sqlx::query(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
                 VALUES (1, 'mb-1', 'Radiohead', 1, 'none', 'lidarr', 0),
                        (2, 'mb-2', 'Atoms', 0, 'none', 'lidarr', 0);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at) VALUES
                 (7, 1, 'rg-ok', 'OK Computer', 1, 'lidarr', 0),
                 (8, 1, 'rg-kid', 'Kid A', 1, 'lidarr', 0),
                 (9, 1, 'rg-am', 'Amnesiac', 0, 'lidarr', 0),
                 (10, 2, 'rg-ht', 'Hail to the Thief', 1, 'lidarr', 0);
             INSERT INTO downloads (infohash, title, wanted_album_id, decision, state, added_at, updated_at)
                 VALUES ('a11bae70b2fdd9eb95b63e746f8594dce2650b0c', 'Kid A', 8, '{}', 'downloading', 0, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let run = start(&pool, sources(None), Area::Albums, who)
            .await
            .unwrap()
            .unwrap();
        let run = finished(&pool, run.id).await;
        assert_eq!((run.state.as_str(), run.total, run.done), ("done", 1, 1));
        // The artist has no music profile, so it couldn't be searched for; the run says why.
        assert!(
            run.notes
                .iter()
                .any(|n| n.starts_with("OK Computer by Radiohead: ")),
            "{run:?}"
        );
    }

    #[tokio::test]
    async fn only_one_run_at_a_time_in_an_area_and_it_can_be_stopped() {
        let pool = setup().await;
        let who = admin(&pool).await;
        for id in 1..=3 {
            sqlx::query(
                "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at,
                     quality_profile) VALUES (?, ?, ?, 1, 'radarr', 0, '4K Preferred')",
            )
            .bind(id)
            .bind(id)
            .bind(format!("Title {id}"))
            .execute(&pool)
            .await
            .unwrap();
        }
        let feed = indexer(&pool, "[2000]").await;
        feed.lock().unwrap().0 = releases(&[]);
        // A long pace, so the run waits between searches and can be stopped there.
        let slow = Sources {
            pace: Duration::from_secs(60),
            ..sources(None)
        };
        let run = start(&pool, slow.clone(), Area::Movies, who)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            start(&pool, slow.clone(), Area::Movies, who).await.unwrap(),
            Err(Already(run.id))
        );
        // Series can run beside it.
        let series = start(&pool, sources(None), Area::Series, who)
            .await
            .unwrap()
            .unwrap();
        // Wait for the first search, then stop during the pause before the second.
        for _ in 0..500 {
            if get(&pool, run.id).await.unwrap().unwrap().searched == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let stopping = stop(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(stopping.state, "stopping");
        let run = finished(&pool, run.id).await;
        assert_eq!(run.state, "stopped");
        assert_eq!((run.total, run.searched), (3, 1));
        assert!(run.message.unwrap().starts_with("asked to stop"));
        assert_eq!(
            stop(&pool, run.id).await.unwrap(),
            Err(NotStopped::Finished)
        );
        assert_eq!(stop(&pool, 999).await.unwrap(), Err(NotStopped::NotFound));
        assert_eq!(finished(&pool, series.id).await.state, "done");

        // Free again once it's over.
        let next = start(&pool, sources(None), Area::Movies, who)
            .await
            .unwrap();
        assert!(next.is_ok());
    }

    #[tokio::test]
    async fn an_indexer_at_its_limit_stops_the_run() {
        let pool = setup().await;
        let who = admin(&pool).await;
        sqlx::query(
            "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at,
                 quality_profile) VALUES (1, 1, 'One', 1, 'radarr', 0, '4K Preferred'),
                                         (2, 2, 'Two', 1, 'radarr', 0, '4K Preferred')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let feed = indexer(&pool, "[2000]").await;
        feed.lock().unwrap().0 =
            r#"<?xml version="1.0"?><error code="500" description="Request limit reached"/>"#
                .into();
        let run = start(&pool, sources(None), Area::Movies, who)
            .await
            .unwrap()
            .unwrap();
        let run = finished(&pool, run.id).await;
        assert_eq!(run.state, "stopped");
        assert_eq!(run.searched, 1, "the second title isn't searched");
        assert!(run.message.unwrap().contains("reached its limit"));
    }

    #[tokio::test]
    async fn an_indexer_at_its_limit_is_left_out_and_the_rest_carry_on() {
        let pool = setup().await;
        let who = admin(&pool).await;
        sqlx::query(
            "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at,
                 quality_profile) VALUES (1, 1, 'One', 1, 'radarr', 0, '4K Preferred'),
                                         (2, 2, 'Two', 1, 'radarr', 0, '4K Preferred')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let limited = indexer(&pool, "[2000]").await;
        limited.lock().unwrap().0 =
            r#"<?xml version="1.0"?><error code="500" description="Request limit reached"/>"#
                .into();
        let second = indexer(&pool, "[2000]").await;
        let run = start(&pool, sources(None), Area::Movies, who)
            .await
            .unwrap()
            .unwrap();
        let run = finished(&pool, run.id).await;
        assert_eq!(run.state, "done", "{:?}", run.message);
        assert_eq!(run.searched, 2, "both titles are searched");
        assert!(
            run.notes.iter().any(|n| n.contains("reached its limit")),
            "{:?}",
            run.notes
        );
        // The one at its limit was asked once; the other, for both titles.
        assert_eq!(limited.lock().unwrap().1.len(), 1);
        assert_eq!(second.lock().unwrap().1.len(), 2);
    }

    #[tokio::test]
    async fn a_restart_ends_runs_it_cut_off() {
        let pool = db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO missing_searches (area, state, started_at) VALUES ('albums', 'running', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        recover(&pool).await.unwrap();
        let run = get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(run.state, "failed");
        assert!(run.finished_at.is_some());
    }
}
