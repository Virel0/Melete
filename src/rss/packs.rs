//! Taking part of a season pack: when some of the season is on disk or on its way, a pack holding
//! episodes that are missing is grabbed for those, and for any whose copy in it is an upgrade of
//! the file there, each judged against that file. Its other episodes are left out (the pack is
//! downloaded whole, and only the chosen episodes' files are brought home), so better files on
//! disk stay where they are.

use super::*;
use crate::upgrades::Replaced;

/// The episodes of a season a pack is taken for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Part {
    /// Every episode taken, in order.
    pub(crate) episodes: Vec<i64>,
    /// The files on disk the pack's copies replace, once confirmed (decision 37).
    pub(crate) replaces: Vec<Replaced>,
    /// Episodes left on disk as they are.
    pub(super) kept: Vec<i64>,
    /// Episodes left out because a download for them is on its way or not settled.
    pub(super) on_the_way: Vec<i64>,
}

impl Part {
    /// What a run says it took: "took episodes 6, 7 from a pack; kept 1–5 on disk".
    pub(super) fn said(&self) -> String {
        let mut said = format!("took episodes {} from a pack", numbers(&self.episodes));
        let upgraded: Vec<i64> = self.replaces.iter().filter_map(|r| r.episode).collect();
        if !upgraded.is_empty() {
            said.push_str(&format!(
                " ({} replacing the file on disk)",
                numbers(&upgraded)
            ));
        }
        if !self.kept.is_empty() {
            said.push_str(&format!("; kept {} on disk", numbers(&self.kept)));
        }
        if !self.on_the_way.is_empty() {
            said.push_str(&format!(
                "; left out {}, on its way",
                numbers(&self.on_the_way)
            ));
        }
        said
    }
}

/// Episode numbers in order, runs of three or more joined: "1–5, 8, 10, 11".
pub(crate) fn numbers(episodes: &[i64]) -> String {
    let mut sorted = episodes.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut runs: Vec<(i64, i64)> = Vec::new();
    for e in sorted {
        match runs.last_mut() {
            Some((_, last)) if *last + 1 == e => *last = e,
            _ => runs.push((e, e)),
        }
    }
    let mut said = Vec::new();
    for (first, last) in runs {
        match last - first {
            0 => said.push(first.to_string()),
            1 => said.extend([first.to_string(), last.to_string()]),
            _ => said.push(format!("{first}–{last}")),
        }
    }
    said.join(", ")
}

/// A file on disk in the season, with whether it's in the wanted title's own folder.
#[derive(sqlx::FromRow)]
struct OnDiskFile {
    #[sqlx(flatten)]
    file: Replaced,
    linked: bool,
}

/// The part of a season pack to take for `season` of a wanted series: the episodes TMDB lists
/// as aired (those left out on their own aside) that are missing, and those whose file the
/// pack's copy is an upgrade of (`decision` being the pack's, judged by the title's profile), leaving out
/// any on their way. Why none, when it holds no missing episode or the season's episodes
/// aren't known.
pub(super) async fn part_of(
    pool: &SqlitePool,
    wanted_series_id: i64,
    season: i64,
    wanted: &Wanted,
    release: &Release,
    decision: &serde_json::Value,
) -> anyhow::Result<Result<Part, String>> {
    let listed = aired(pool, wanted_series_id, season, wanted).await?;
    if listed.is_empty() {
        return Ok(Err(format!(
            "season {season}'s episode list isn't known, so which of its episodes are missing \
             can't be told"
        )));
    }
    let Some(on_the_way) = on_the_way(pool, wanted_series_id, season).await? else {
        return Ok(Err(
            "a download for the whole season is on its way or not yet in the library".into(),
        ));
    };
    let target = Target::Series {
        id: wanted_series_id,
        season,
        episode: None,
    };
    let profile = match search::profile_for(pool, target, wanted).await? {
        Ok(profile) => profile,
        Err(refused) => return Ok(Err(refused.to_string())),
    };
    let pack = Judged {
        profile: &profile,
        wanted,
        season,
        rank: decision["rank"].as_i64(),
        proper: release.proper,
        score: decision["score"].as_i64().unwrap_or(i64::MIN),
    };
    let files = files_in(pool, wanted, season).await?;
    let mut part = Part {
        episodes: Vec::new(),
        replaces: Vec::new(),
        kept: Vec::new(),
        on_the_way: Vec::new(),
    };
    let mut missing = 0;
    for e in listed {
        if on_the_way.contains(&e) {
            part.on_the_way.push(e);
            continue;
        }
        match pack.against(pool, &files, e).await? {
            Against::Missing => {
                missing += 1;
                part.episodes.push(e);
            }
            Against::Upgrade(file) => {
                part.episodes.push(e);
                part.replaces.push(file);
            }
            Against::Kept => part.kept.push(e),
        }
    }
    if missing > 0 {
        return Ok(Ok(part));
    }
    Ok(Err(if part.on_the_way.is_empty() {
        format!("no episode of season {season} is missing")
    } else {
        format!(
            "no episode of season {season} is missing that isn't on its way already ({} on \
             the way)",
            numbers(&part.on_the_way)
        )
    }))
}

/// The episodes of the season TMDB lists as aired, in the order the series follows, those left
/// out on their own aside.
async fn aired(
    pool: &SqlitePool,
    wanted_series_id: i64,
    season: i64,
    wanted: &Wanted,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT e.episode FROM episodes_in_order e
         WHERE e.season = ?3 AND e.air_date IS NOT NULL AND e.air_date <= date('now')
           AND e.series_tmdb_id = (SELECT tmdb_id FROM tmdb_series
                                   WHERE tmdb_id = ?1 OR (?1 IS NULL AND tvdb_id = ?2)
                                   ORDER BY tmdb_id LIMIT 1)
           AND NOT EXISTS (SELECT 1 FROM wanted_episode_overrides o
                           WHERE o.wanted_series_id = ?4 AND o.season = ?3
                             AND o.episode = e.episode AND o.monitored = 0)
         ORDER BY e.episode",
    )
    .bind(wanted.tmdb_id)
    .bind(wanted.tvdb_id)
    .bind(season)
    .bind(wanted_series_id)
    .fetch_all(pool)
    .await
}

/// A pack as its decision judged it, to compare with each episode's file.
struct Judged<'a> {
    profile: &'a crate::profiles::Loaded,
    wanted: &'a Wanted,
    season: i64,
    rank: Option<i64>,
    proper: u8,
    score: i64,
}

/// What the pack's copy of an episode is against what's on disk.
enum Against {
    Missing,
    /// An upgrade of this file, the episode's only one.
    Upgrade(Replaced),
    Kept,
}

impl Judged<'_> {
    async fn against(
        &self,
        pool: &SqlitePool,
        files: &[OnDiskFile],
        e: i64,
    ) -> anyhow::Result<Against> {
        let here: Vec<&OnDiskFile> = files
            .iter()
            .filter(|f| {
                f.file.episode.is_some_and(|first| first <= e)
                    && f.file
                        .episode_end
                        .or(f.file.episode)
                        .is_some_and(|last| last >= e)
            })
            .collect();
        // Only one file, in the title's own folder, holding this episode alone, can be replaced;
        // anything else stays.
        let only = match here.as_slice() {
            [] => return Ok(Against::Missing),
            [only]
                if only.linked
                    && only.file.episode == Some(e)
                    && only.file.episode_end.is_none_or(|end| end == e) =>
            {
                only
            }
            _ => return Ok(Against::Kept),
        };
        let key = Some((self.season, e, e));
        Ok(
            match search::on_disk(pool, self.profile, self.wanted, key).await? {
                search::OnDisk::File(existing)
                    if self
                        .profile
                        .upgrades(self.rank, self.proper, self.score, &existing) =>
                {
                    Against::Upgrade(only.file.clone())
                }
                _ => Against::Kept,
            },
        )
    }
}

/// The episodes of the season a download is on its way for, or not yet settled in the library;
/// None when one is for the whole season.
async fn on_the_way(
    pool: &SqlitePool,
    wanted_series_id: i64,
    season: i64,
) -> anyhow::Result<Option<Vec<i64>>> {
    let rows: Vec<(Option<i64>, Option<String>)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT episode, episodes FROM downloads
         WHERE wanted_series_id = ? AND season = ? AND state <> 'failed' AND {UNSETTLED}"
    )))
    .bind(wanted_series_id)
    .bind(season)
    .fetch_all(pool)
    .await?;
    let mut episodes = Vec::new();
    for (episode, part) in rows {
        match (episode, part) {
            (Some(e), _) => episodes.push(e),
            (None, Some(part)) => episodes.extend(serde_json::from_str::<Vec<i64>>(&part)?),
            (None, None) => return Ok(None),
        }
    }
    Ok(Some(episodes))
}

/// The files on disk in the season, in the wanted title's folder or any other known by its ids.
async fn files_in(
    pool: &SqlitePool,
    wanted: &Wanted,
    season: i64,
) -> anyhow::Result<Vec<OnDiskFile>> {
    Ok(sqlx::query_as(
        "SELECT f.id AS file_id, f.library_id, f.rel_path, f.size, f.modified_at, f.season,
                f.episode, f.episode_end, f.series_id IS ?1 AS linked
         FROM media_files f JOIN series s ON s.id = f.series_id
         WHERE f.season = ?2 AND f.missing_since IS NULL
           AND (s.id = ?1 OR s.match_tmdb_id = ?3 OR s.arr_tmdb_id = ?3 OR s.tmdb_id = ?3
                OR s.arr_tvdb_id = ?4 OR s.tvdb_id = ?4)",
    )
    .bind(wanted.series_id)
    .bind(season)
    .bind(wanted.tmdb_id)
    .bind(wanted.tvdb_id)
    .fetch_all(pool)
    .await?)
}
