//! The title check, which fails closed: comparable titles, episode order and the second look at a
//! release, and why one isn't the wanted title.

use super::*;

/// A title as compared: lower case, accents dropped ("Amélie" is "Amelie"), apostrophes
/// dropped ("Charlotte's" is "Charlottes"), "&" as "and", other punctuation as spaces, and a
/// leading "the", "a" or "an" left out.
pub(crate) fn comparable(title: &str) -> String {
    let folded: String = title.to_lowercase().chars().filter_map(fold).collect();
    let spaced = folded.replace('&', " and ");
    let mut words: Vec<&str> = spaced
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if words.len() > 1 && ["the", "a", "an"].contains(&words[0]) {
        words.remove(0);
    }
    words.join(" ")
}

/// A lower-case letter without its accent; apostrophes go.
pub(super) fn fold(c: char) -> Option<char> {
    Some(match c {
        '\'' | '’' | '`' => return None,
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'ď' | 'đ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => 'i',
        'ł' | 'ľ' | 'ĺ' => 'l',
        'ñ' | 'ń' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => 'o',
        'ŕ' | 'ř' => 'r',
        'ś' | 'š' | 'ş' | 'ș' => 's',
        'ť' | 'ţ' | 'ț' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => 'u',
        'ý' | 'ÿ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    })
}

/// A wanted title without a disambiguating year: Sonarr's "Doctor Who (2005)" is "Doctor Who".
pub(crate) fn without_year(title: &str) -> &str {
    let trimmed = title.trim_end();
    match trimmed.strip_suffix(')').and_then(|t| t.rsplit_once(" (")) {
        Some((name, year)) if year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => trimmed,
    }
}

/// What a second look found.
pub(super) enum Look {
    /// The rejection doesn't hold after all.
    Lifted,
    /// It doesn't hold for the release numbered this way.
    Renumbered(Episodes),
    /// It holds, perhaps with more to say.
    Kept(Option<String>),
}

/// TMDB answers and season episode lists, asked once per search.
#[derive(Default)]
pub(super) struct Looked {
    /// For each year: whether TMDB has another film of the wanted title from it.
    pub(super) other_film: HashMap<i64, Result<Option<String>, String>>,
    /// For each season: the episodes TMDB lists.
    pub(super) season_episodes: HashMap<i64, Vec<i64>>,
    /// The show's episodes in order, specials left out.
    pub(super) order: Option<Vec<(i64, i64)>>,
}

/// A show's episodes as TMDB lists them, in order and without specials: absolute number n is
/// the nth, as for files on disk (`library::absolute`).
pub(super) async fn episode_order(
    pool: &SqlitePool,
    wanted: &Wanted,
) -> anyhow::Result<Vec<(i64, i64)>> {
    Ok(sqlx::query_as(
        "SELECT e.season, e.episode FROM tmdb_episodes e
         WHERE e.season > 0 AND e.series_tmdb_id = (
             SELECT tmdb_id FROM tmdb_series
             WHERE tmdb_id = ?1 OR (?1 IS NULL AND tvdb_id = ?2) ORDER BY tmdb_id LIMIT 1)
         ORDER BY e.season, e.episode",
    )
    .bind(wanted.tmdb_id)
    .bind(wanted.tvdb_id)
    .fetch_all(pool)
    .await?)
}

/// Absolute episode numbers as one season and its episodes; why not, when they can't be.
pub(super) fn in_season(order: &[(i64, i64)], numbers: &[i64]) -> Result<(i64, Vec<i64>), String> {
    if order.is_empty() {
        return Err("TMDB's episode list for the show isn't known to map it".into());
    }
    let mut season = None;
    let mut episodes = Vec::new();
    for &n in numbers {
        let Some(&(s, e)) = usize::try_from(n - 1).ok().and_then(|i| order.get(i)) else {
            return Err(format!("TMDB lists {} episodes, not {n}", order.len()));
        };
        if season.is_some_and(|known| known != s) {
            return Err("its episodes span seasons".into());
        }
        season = Some(s);
        episodes.push(e);
    }
    season
        .map(|s| (s, episodes))
        .ok_or_else(|| "it names no episode".into())
}

/// For a release numbered from the first episode: the season and episodes it is, by TMDB's
/// list.
pub(crate) async fn absolute_in_season(
    pool: &SqlitePool,
    wanted: &Wanted,
    numbers: &[i64],
) -> anyhow::Result<Result<(i64, Vec<i64>), String>> {
    Ok(in_season(&episode_order(pool, wanted).await?, numbers))
}

/// Rejections a closer look can lift. A movie named with a year one off (a festival year,
/// say) is the wanted film unless TMDB has another film of that title from that year ("Little
/// Women", 2018 and 2019). And in a season search, an episode range ("S01E01-E12") is the
/// season when it holds every episode TMDB lists for it.
pub(super) async fn second_look(
    pool: &SqlitePool,
    tmdb: Option<&crate::tmdb::Client>,
    wanted: &Wanted,
    target: Target,
    release: &Release,
    looked: &mut Looked,
) -> anyhow::Result<Look> {
    match (target, &release.episodes) {
        (Target::Movie(_), _) => year_off(tmdb, wanted, target, release, looked).await,
        (
            Target::Series {
                id,
                season,
                episode: None,
            },
            Episodes::Standard {
                season: s,
                episodes,
            },
        ) if *s == season => {
            episode_range(pool, wanted, release, looked, id, season, episodes).await
        }
        (Target::Series { .. }, Episodes::Absolute(numbers)) => {
            numbered_from_first(pool, tmdb, wanted, target, release, looked, numbers).await
        }
        _ => Ok(Look::Kept(None)),
    }
}

/// A movie named with a year one off: lifted unless TMDB has another film of the title from
/// that year.
async fn year_off(
    tmdb: Option<&crate::tmdb::Client>,
    wanted: &Wanted,
    target: Target,
    release: &Release,
    looked: &mut Looked,
) -> anyhow::Result<Look> {
    let (Some(year), Some(expected)) = (release.year, wanted.year) else {
        return Ok(Look::Kept(None));
    };
    let as_named = Wanted {
        year: Some(year),
        ..wanted.clone()
    };
    if (year - expected).abs() != 1 || mismatch(release, &as_named, target).is_some() {
        return Ok(Look::Kept(None));
    }
    let Some(tmdb) = tmdb else {
        return Ok(Look::Kept(Some(
            "without TMDB, a year off can't be checked".into(),
        )));
    };
    let other = match looked.other_film.entry(year) {
        Entry::Occupied(known) => known.into_mut(),
        Entry::Vacant(new) => new.insert(other_film(tmdb, wanted, year).await),
    };
    Ok(match other {
        Ok(None) => Look::Lifted,
        Ok(Some(title)) => Look::Kept(Some(format!("TMDB has another \"{title}\" from {year}"))),
        Err(err) => Look::Kept(Some(format!("couldn't check with TMDB: {err}"))),
    })
}

/// An episode range in a season search: lifted when it holds every episode TMDB lists for the
/// season.
async fn episode_range(
    pool: &SqlitePool,
    wanted: &Wanted,
    release: &Release,
    looked: &mut Looked,
    id: i64,
    season: i64,
    episodes: &[i64],
) -> anyhow::Result<Look> {
    let Some(first) = episodes.iter().min() else {
        return Ok(Look::Kept(None));
    };
    // The title and year must fit as for one of its episodes.
    let as_episode = Target::Series {
        id,
        season,
        episode: Some(*first),
    };
    if mismatch(release, wanted, as_episode).is_some() {
        return Ok(Look::Kept(None));
    }
    // TMDB's entry by its id, or by the TVDB id when Sonarr didn't know the TMDB one.
    let listed = match looked.season_episodes.entry(season) {
        Entry::Occupied(known) => known.into_mut(),
        Entry::Vacant(new) => new.insert(
            sqlx::query_scalar(
                "SELECT episode FROM tmdb_episodes WHERE season = ?3 AND series_tmdb_id = (
                             SELECT tmdb_id FROM tmdb_series
                             WHERE tmdb_id = ?1 OR (?1 IS NULL AND tvdb_id = ?2)
                             ORDER BY tmdb_id LIMIT 1)",
            )
            .bind(wanted.tmdb_id)
            .bind(wanted.tvdb_id)
            .bind(season)
            .fetch_all(pool)
            .await?,
        ),
    };
    let whole = !listed.is_empty() && listed.iter().all(|e| episodes.contains(e));
    Ok(if whole {
        Look::Lifted
    } else {
        Look::Kept(None)
    })
}

/// A release numbered from the first episode, renumbered by TMDB's list into the season.
async fn numbered_from_first(
    pool: &SqlitePool,
    tmdb: Option<&crate::tmdb::Client>,
    wanted: &Wanted,
    target: Target,
    release: &Release,
    looked: &mut Looked,
    numbers: &[i64],
) -> anyhow::Result<Look> {
    if looked.order.is_none() {
        looked.order = Some(episode_order(pool, wanted).await?);
    }
    let order = looked.order.as_deref().unwrap_or_default();
    let (season, episodes) = match in_season(order, numbers) {
        Ok(mapped) => mapped,
        Err(why) => return Ok(Look::Kept(Some(why))),
    };
    let renumbered = Release {
        episodes: Episodes::Standard { season, episodes },
        ..release.clone()
    };
    if mismatch(&renumbered, wanted, target).is_none() {
        return Ok(Look::Renumbered(renumbered.episodes));
    }
    // A batch may still be the whole season searched for, as episode ranges are.
    Ok(
        match Box::pin(second_look(pool, tmdb, wanted, target, &renumbered, looked)).await? {
            Look::Lifted | Look::Renumbered(_) => Look::Renumbered(renumbered.episodes),
            Look::Kept(_) => Look::Kept(Some(format!(
                "as numbered it's {}",
                describe_episodes(&renumbered.episodes)
            ))),
        },
    )
}

/// "S02E03" or "S02E01-E12".
pub(super) fn describe_episodes(episodes: &Episodes) -> String {
    match episodes {
        Episodes::Standard { season, episodes } => {
            let (first, last) = (
                episodes.iter().min().copied().unwrap_or(0),
                episodes.iter().max().copied().unwrap_or(0),
            );
            if first == last {
                format!("S{season:02}E{first:02}")
            } else {
                format!("S{season:02}E{first:02}-E{last:02}")
            }
        }
        other => format!("{other:?}"),
    }
}

/// Another film TMDB lists with the wanted title, first released in `year`.
pub(super) async fn other_film(
    tmdb: &crate::tmdb::Client,
    wanted: &Wanted,
    year: i64,
) -> Result<Option<String>, String> {
    let title = wanted.titles.first().cloned().unwrap_or_default();
    let results = tmdb
        .search_movies(&title, Some(year))
        .await
        .map_err(|err| format!("{err:#}"))?;
    let comparables: Vec<String> = wanted.titles.iter().map(|t| comparable(t)).collect();
    Ok(results
        .into_iter()
        .filter(|r| Some(r.id) != wanted.tmdb_id)
        .filter(|r| {
            r.release_date
                .as_deref()
                .and_then(|d| d.get(..4))
                .and_then(|y| y.parse().ok())
                == Some(year)
        })
        .find(|r| {
            comparables.contains(&comparable(&r.title))
                || r.original_title
                    .as_deref()
                    .is_some_and(|o| comparables.contains(&comparable(o)))
        })
        .map(|r| r.title))
}

/// Why a release isn't the wanted title, or isn't what was searched for.
pub(crate) fn mismatch(release: &Release, wanted: &Wanted, target: Target) -> Option<String> {
    let title = comparable(&release.title);
    if title.is_empty()
        || !wanted
            .titles
            .iter()
            .any(|t| comparable(without_year(t)) == title)
    {
        return Some(format!(
            "it's {:?}, not {:?}",
            release.title,
            wanted
                .titles
                .first()
                .map(String::as_str)
                .unwrap_or_default()
        ));
    }
    // Exactly the year: remakes come a year apart ("Little Women", 2018 and 2019).
    match (release.year, wanted.year, target) {
        (None, _, Target::Movie(_)) => return Some("the name gives no year".into()),
        (_, None, Target::Movie(_)) => {
            return Some("the wanted movie has no year to check against".into());
        }
        (Some(year), Some(expected), _) if year != expected => {
            return Some(format!("it's from {year}, not {expected}"));
        }
        _ => {}
    }
    match (target, &release.episodes) {
        (Target::Movie(_), Episodes::None) => None,
        (Target::Movie(_), _) => Some("it's a TV release".into()),
        (
            Target::Series {
                season,
                episode: Some(e),
                ..
            },
            Episodes::Standard {
                season: s,
                episodes,
            },
        ) => (*s != season || !episodes.contains(&e))
            .then(|| format!("it isn't S{season:02}E{e:02}")),
        (
            Target::Series {
                episode: Some(_), ..
            },
            Episodes::Seasons(_) | Episodes::CompleteSeries,
        ) => Some("it's a season pack; this searched for one episode".into()),
        (
            Target::Series {
                season,
                episode: None,
                ..
            },
            Episodes::Seasons(seasons),
        ) => (!seasons.contains(&season)).then(|| format!("it doesn't hold season {season}")),
        (Target::Series { episode: None, .. }, Episodes::Standard { .. }) => {
            Some("it's single episodes; this searched for a season".into())
        }
        (Target::Series { episode: None, .. }, Episodes::CompleteSeries) => {
            Some("it's the complete series; this searched for one season".into())
        }
        (Target::Series { .. }, Episodes::Absolute(_)) => Some(
            "it's numbered from the first episode, which takes TMDB's episode list to map".into(),
        ),
        (Target::Series { .. }, Episodes::Daily(_)) => {
            Some("it's dated, which Melete can't map to episodes yet".into())
        }
        (Target::Series { .. }, Episodes::None) => Some("it names no season or episode".into()),
    }
}
