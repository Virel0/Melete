//! Which wanted title or album an RSS release is for, checked harder than a search result: the
//! releases not looked at before, candidates and groups, other ids, unmonitored titles, and each
//! release's key.

use super::*;

/// A wanted title an RSS release may be for.
pub(super) struct Candidate {
    pub(super) title: Title,
    pub(super) wanted: Wanted,
    /// Its titles as compared.
    pub(super) comparables: Vec<String>,
}

/// New releases, each with its indexer and a reason to refuse it found here.
pub(super) type Found<'a> = Vec<(i64, &'a torznab::Item, Option<String>)>;

/// The new releases naming one target, each with a reason to refuse it found here.
pub(super) struct Group<'a> {
    pub(super) target: Target,
    /// Which candidate it is.
    pub(super) candidate: usize,
    pub(super) releases: Found<'a>,
}

/// What a release names, for one candidate.
pub(super) fn target_for(title: Title, release: &Release) -> Option<Target> {
    match (title, &release.episodes) {
        (Title::Movie(id), Episodes::None) => Some(Target::Movie(id)),
        (Title::Series(id), Episodes::Standard { season, episodes }) => Some(Target::Series {
            id,
            season: *season,
            episode: Some(*episodes.iter().min()?),
        }),
        (Title::Series(id), Episodes::Seasons(seasons)) if seasons.len() == 1 => {
            Some(Target::Series {
                id,
                season: seasons[0],
                episode: None,
            })
        }
        // Multi-season packs, the complete series and dated numbering are left to searches;
        // absolute numbering is mapped by the caller.
        _ => None,
    }
}

/// Why the indexer's ids say a release is another title.
pub(super) fn other_ids(item: &torznab::Item, wanted: &Wanted) -> Option<String> {
    let imdb = |id: &str| {
        id.trim_start_matches("tt")
            .trim_start_matches('0')
            .to_owned()
    };
    if let (Some(given), Some(own)) = (&item.imdb_id, &wanted.imdb_id)
        && imdb(given) != imdb(own)
    {
        return Some(format!("the indexer says it's IMDb tt{given}, not {own}"));
    }
    if let (Some(given), Some(own)) = (item.tvdb_id, wanted.tvdb_id)
        && given != own
    {
        return Some(format!("the indexer says it's TVDB {given}, not {own}"));
    }
    if let (Some(given), Some(own)) = (item.tmdb_id, wanted.tmdb_id)
        && given != own
    {
        return Some(format!("the indexer says it's TMDB {given}, not {own}"));
    }
    None
}

/// Why a season or episode isn't wanted: Sonarr's season and episode choices.
pub(super) async fn unmonitored(
    pool: &SqlitePool,
    target: Target,
    release: &Release,
) -> anyhow::Result<Option<String>> {
    let Target::Series { id, season, .. } = target else {
        return Ok(None);
    };
    let monitored: Option<bool> = sqlx::query_scalar(
        "SELECT monitored FROM wanted_seasons WHERE wanted_series_id = ? AND season = ?",
    )
    .bind(id)
    .bind(season)
    .fetch_optional(pool)
    .await?;
    if monitored != Some(true) {
        return Ok(Some(format!("season {season} isn't monitored")));
    }
    if let Episodes::Standard { episodes, .. } = &release.episodes {
        let off: Option<i64> = sqlx::query_scalar(
            "SELECT episode FROM wanted_episode_overrides
             WHERE wanted_series_id = ? AND season = ? AND monitored = 0
               AND episode IN (SELECT value FROM json_each(?))
             ORDER BY episode LIMIT 1",
        )
        .bind(id)
        .bind(season)
        .bind(serde_json::to_string(episodes)?)
        .fetch_optional(pool)
        .await?;
        if let Some(episode) = off {
            return Ok(Some(format!("S{season:02}E{episode:02} isn't monitored")));
        }
    }
    Ok(None)
}

/// Releases are known by a hash of their guid, or of their title without one.
pub(super) fn release_key(item: &torznab::Item) -> String {
    let source = item.guid.as_deref().unwrap_or(&item.title);
    Sha256::digest(source.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// New releases, each with its indexer and key.
pub(super) type Fresh<'a> = Vec<(i64, &'a torznab::Item, String)>;

/// The releases not looked at before, each once.
pub(super) async fn fresh_releases<'a>(
    pool: &SqlitePool,
    outcomes: &'a Outcomes,
) -> sqlx::Result<Fresh<'a>> {
    let mut fresh: Fresh = Vec::new();
    for (outcome, items) in outcomes {
        for item in items {
            let key = release_key(item);
            let seen: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM rss_seen WHERE indexer_id = ? AND release_key = ?",
            )
            .bind(outcome.indexer_id)
            .bind(&key)
            .fetch_optional(pool)
            .await?;
            let twice = fresh
                .iter()
                .any(|(i, _, k)| *i == outcome.indexer_id && *k == key);
            if seen.is_none() && !twice {
                fresh.push((outcome.indexer_id, item, key));
            }
        }
    }
    Ok(fresh)
}

/// Every monitored wanted title, with its titles as compared.
pub(super) async fn wanted_candidates(pool: &SqlitePool) -> anyhow::Result<Vec<Candidate>> {
    Ok(search::monitored_wanted(pool)
        .await?
        .into_iter()
        .map(|(title, wanted)| Candidate {
            title,
            comparables: wanted
                .titles
                .iter()
                .map(|t| search::comparable(search::without_year(t)))
                .collect(),
            wanted,
        })
        .collect())
}

/// Which wanted titles each new release names, grouped by target, each with a reason to refuse
/// it: one that fits several wanted titles fully is refused for all of them.
pub(super) async fn group_by_title<'a>(
    pool: &SqlitePool,
    candidates: &[Candidate],
    fresh: &Fresh<'a>,
) -> anyhow::Result<Vec<Group<'a>>> {
    let mut groups: Vec<Group> = Vec::new();
    for &(indexer_id, item, _) in fresh {
        // Music is for the albums.
        if is_music(item) {
            continue;
        }
        let parsed = release::parse(&item.title);
        let named = search::comparable(&parsed.title);
        if named.is_empty() {
            continue;
        }
        let fitting = fitting(pool, candidates, &parsed, &named).await?;
        let fits_fully = fitting
            .iter()
            .filter(|(n, target, judged)| {
                search::mismatch(judged, &candidates[*n].wanted, *target).is_none()
            })
            .count();
        for (n, target, judged) in fitting {
            let candidate = &candidates[n];
            let mut why = if fits_fully > 1 {
                Some(format!(
                    "it could be any of {fits_fully} wanted titles named {:?}",
                    parsed.title
                ))
            } else {
                other_ids(item, &candidate.wanted)
            };
            if why.is_none() {
                why = unmonitored(pool, target, &judged).await?;
            }
            match groups.iter_mut().find(|g| g.target == target) {
                Some(group) => group.releases.push((indexer_id, item, why)),
                None => groups.push(Group {
                    target,
                    candidate: n,
                    releases: vec![(indexer_id, item, why)],
                }),
            }
        }
    }
    Ok(groups)
}

/// Each candidate a release named `named` may be for, with the release as judged for it: one
/// numbered from the first episode is the season and episodes TMDB's list makes it.
async fn fitting(
    pool: &SqlitePool,
    candidates: &[Candidate],
    parsed: &Release,
    named: &str,
) -> anyhow::Result<Vec<(usize, Target, Release)>> {
    let mut fitting: Vec<(usize, Target, Release)> = Vec::new();
    for (n, c) in candidates.iter().enumerate() {
        if !c.comparables.iter().any(|title| title == named) {
            continue;
        }
        if let Some(target) = target_for(c.title, parsed) {
            fitting.push((n, target, parsed.clone()));
        } else if let (Title::Series(id), Episodes::Absolute(numbers)) = (c.title, &parsed.episodes)
            && let Ok((season, episodes)) =
                search::absolute_in_season(pool, &c.wanted, numbers).await?
            && let Some(&first) = episodes.iter().min()
        {
            let renumbered = Release {
                episodes: Episodes::Standard { season, episodes },
                ..parsed.clone()
            };
            let target = Target::Series {
                id,
                season,
                episode: Some(first),
            };
            fitting.push((n, target, renumbered));
        }
    }
    Ok(fitting)
}

/// Which wanted albums each new music release is; one that could be several is refused for
/// all of them.
pub(super) fn group_by_album<'a>(
    albums: &[album_search::WantedAlbum],
    fresh: &Fresh<'a>,
) -> Vec<(usize, Found<'a>)> {
    let mut album_groups: Vec<(usize, Found)> = Vec::new();
    for &(indexer_id, item, _) in fresh.iter().filter(|(_, item, _)| is_music(item)) {
        let fitting: Vec<usize> = albums
            .iter()
            .enumerate()
            .filter(|(_, a)| album_search::mismatch(&item.title, &a.artist, &a.title).is_none())
            .map(|(n, _)| n)
            .collect();
        let why = (fitting.len() > 1)
            .then(|| format!("it could be any of {} wanted albums", fitting.len()));
        for n in fitting {
            match album_groups.iter_mut().find(|(album, _)| *album == n) {
                Some((_, releases)) => releases.push((indexer_id, item, why.clone())),
                None => album_groups.push((n, vec![(indexer_id, item, why.clone())])),
            }
        }
    }
    album_groups
}
