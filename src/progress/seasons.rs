//! Where each season of a wanted series stands, for its request.

use super::*;

/// A download under way for a series: its season, state, progress, message, and the other
/// seasons of a pack of several it brings home.
type Moving = (Option<i64>, String, f64, Option<String>, Option<String>);

/// One season of a wanted series, as it stands: whether it's looked for, how much of it
/// is in the library, and a download of it under way.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeasonProgress {
    /// The season's number.
    pub season: i64,
    /// Whether it's looked for (monitored), whoever asked for it.
    pub monitored: bool,
    /// Episodes TMDB lists for it that have aired; null when TMDB's list isn't known here.
    pub episodes: Option<i64>,
    /// How many of its episodes are in the library.
    pub in_library: i64,
    /// Where the season has got to.
    pub step: Step,
    /// How much of the download is done, while downloading.
    pub percent: Option<f64>,
    /// Not all here and nothing on its way, and a download for what's missing was found dead:
    /// no one was sharing it, and nothing else could be grabbed.
    pub dead: bool,
}

/// Each season of a wanted series, in order; empty when it isn't wanted.
pub async fn seasons(
    pool: &SqlitePool,
    wanted_series_id: i64,
) -> anyhow::Result<Vec<SeasonProgress>> {
    let listed: Vec<(i64, bool)> = sqlx::query_as(
        "SELECT season, monitored = 1 FROM wanted_seasons WHERE wanted_series_id = ?
         ORDER BY season",
    )
    .bind(wanted_series_id)
    .fetch_all(pool)
    .await?;
    if listed.is_empty() {
        return Ok(Vec::new());
    }
    // Episodes on disk, a file holding several counted for each.
    let files: Vec<(i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT f.season, f.episode, f.episode_end FROM media_files f
           JOIN wanted_series w ON w.series_id = f.series_id
         WHERE w.id = ? AND f.missing_since IS NULL AND f.season IS NOT NULL",
    )
    .bind(wanted_series_id)
    .fetch_all(pool)
    .await?;
    let mut on_disk = std::collections::BTreeSet::new();
    for (season, first, last) in files {
        for episode in first..=last.unwrap_or(first).min(first + 1000) {
            on_disk.insert((season, episode));
        }
    }
    // Aired episodes, and whether each is left out on its own.
    let aired: Vec<(i64, i64, bool)> = sqlx::query_as(
        "SELECT e.season, e.episode,
                EXISTS (SELECT 1 FROM wanted_episode_overrides o
                        WHERE o.wanted_series_id = w.id AND o.season = e.season
                          AND o.episode = e.episode AND o.monitored = 0)
         FROM episodes_in_order e
           JOIN wanted_series w ON w.tmdb_id = e.series_tmdb_id
         WHERE w.id = ? AND e.air_date IS NOT NULL AND e.air_date <= date('now')",
    )
    .bind(wanted_series_id)
    .fetch_all(pool)
    .await?;
    let known = !aired.is_empty();
    // Downloads under way, newest first; one without a season (a whole-series pack) is every
    // season's, and a pack of several seasons each of its seasons'.
    let moving: Vec<Moving> = sqlx::query_as(
        "SELECT season, state, progress, message, seasons FROM downloads
         WHERE wanted_series_id = ? AND state IN ('downloading', 'completed', 'transferring')
         ORDER BY id DESC",
    )
    .bind(wanted_series_id)
    .fetch_all(pool)
    .await?;
    // Downloads found dead: a pack (no episode) or the episode each was for.
    let died: Vec<(Option<i64>, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT season, episode, seasons FROM downloads
         WHERE wanted_series_id = ? AND dead_at IS NOT NULL",
    )
    .bind(wanted_series_id)
    .fetch_all(pool)
    .await?;
    Ok(listed
        .into_iter()
        .map(|(season, monitored)| {
            let in_library = on_disk.iter().filter(|(s, _)| *s == season).count() as i64;
            let of_season = || aired.iter().filter(|(s, ..)| *s == season);
            let episodes = known.then(|| of_season().count() as i64);
            // Every aired episode that isn't left out on its own is here.
            let complete = episodes.is_some_and(|n| n > 0)
                && of_season().all(|(s, e, left_out)| *left_out || on_disk.contains(&(*s, *e)));
            // A pack of several seasons is each season's it brings home.
            let brings = |s: &Option<i64>, others: &Option<String>| {
                s.is_none_or(|s| s == season)
                    || others.as_deref().is_some_and(|o| {
                        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(o)
                            .is_ok_and(|o| o.contains_key(&season.to_string()))
                    })
            };
            let download = moving.iter().find(|(s, .., others)| brings(s, others));
            let (step, percent) = match download {
                Some((_, state, done, message, _)) if !complete => {
                    let step = download_step(state, message.as_deref());
                    (step, (step == Step::Downloading).then_some(*done))
                }
                _ if complete => (Step::InLibrary, None),
                _ if monitored => (Step::Searching, None),
                // Not looked for: what's here is all there'll be.
                _ if in_library > 0 => (Step::InLibrary, None),
                _ => (Step::Wanted, None),
            };
            // What a dead download was for is still missing, and nothing is on its way.
            let dead = !complete
                && download.is_none()
                && died.iter().any(|(s, e, others)| {
                    brings(s, others) && e.is_none_or(|e| !on_disk.contains(&(season, e)))
                });
            SeasonProgress {
                season,
                monitored,
                episodes,
                in_library,
                step,
                percent,
                dead,
            }
        })
        .collect())
}
