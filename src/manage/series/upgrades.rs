//! Which of a wanted series' episodes are below its profile's cutoff.

use std::collections::HashSet;

use super::*;

/// Which seasons and episodes are monitored, as searches and RSS decide it: a season listed as
/// monitored, and an episode in it not left out on its own.
pub(super) struct Rules {
    seasons: HashSet<(i64, i64)>,
    left_out: HashSet<(i64, i64, i64)>,
}

impl Rules {
    pub(super) async fn load(pool: &SqlitePool) -> anyhow::Result<Self> {
        let seasons: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT wanted_series_id, season FROM wanted_seasons WHERE monitored = 1",
        )
        .fetch_all(pool)
        .await?;
        let left_out: Vec<(i64, i64, i64)> = sqlx::query_as(
            "SELECT wanted_series_id, season, episode FROM wanted_episode_overrides
             WHERE monitored = 0",
        )
        .fetch_all(pool)
        .await?;
        Ok(Rules {
            seasons: seasons.into_iter().collect(),
            left_out: left_out.into_iter().collect(),
        })
    }

    fn monitored(&self, series: i64, season: i64, episode: i64) -> bool {
        self.seasons.contains(&(series, season))
            && !self.left_out.contains(&(series, season, episode))
    }
}

/// Whether a monitored wanted series has a monitored episode below its profile's cutoff
/// (None when none could be judged), and which, judged on the files of the folder linked to it.
pub(super) async fn series_cutoff(
    pool: &SqlitePool,
    profiles: &mut cutoff::Profiles,
    w: &WantedRow,
    linked: Option<&Vec<EpisodeFile>>,
    rules: &Rules,
) -> anyhow::Result<(Option<bool>, Vec<(i64, i64)>)> {
    let Some(files) = linked.filter(|_| w.monitored) else {
        return Ok((None, Vec::new()));
    };
    let Some(profile) = profiles
        .get(pool, w.profile_id, w.arr_profile.as_deref())
        .await?
    else {
        return Ok((None, Vec::new()));
    };
    let files: Vec<&EpisodeFile> = files.iter().collect();
    let (judged, below) = cutoff::episodes_unmet(
        pool,
        profile,
        &files,
        |season, episode| rules.monitored(w.id, season, episode),
        w.original_language.as_deref(),
    )
    .await?;
    Ok((judged.then_some(!below.is_empty()), below))
}
