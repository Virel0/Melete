//! The episodes a release name holds: single, crossed, daily, absolute, batches and whole seasons.

use super::*;

/// Episodes found in a name, and where in it they start and end.
type Found = (Episodes, Option<(usize, usize)>);

/// The episodes a name holds, and where in it they start and end.
pub(super) fn episodes(rest: &str, anime_style: bool) -> Found {
    season_episodes(rest)
        .or_else(|| dated_or_bare(rest))
        .or_else(|| anime_numbers(rest, anime_style))
        .or_else(|| whole_seasons(rest))
        .unwrap_or((Episodes::None, None))
}

/// A season and episodes: "S01E02", "S01E02-E03", "Season 1 Episode 2", "1x02".
fn season_episodes(rest: &str) -> Option<Found> {
    if let Some(caps) = SEASON_EPISODE.captures(rest) {
        let season = caps[1].parse().unwrap_or_default();
        let first: i64 = caps[2].parse().unwrap_or_default();
        let (more, end) = more_episodes(
            rest,
            caps.get(2)
                .expect("the episode group is not optional")
                .end(),
            first,
        );
        let start = caps.get(0).expect("a match has group 0").start();
        let episodes = std::iter::once(first).chain(more).collect();
        return Some((Episodes::Standard { season, episodes }, Some((start, end))));
    }
    if let Some(caps) = SEASON_EPISODE_WORDS.captures(rest) {
        let whole = caps.get(0).expect("a match has group 0");
        let episodes = vec![caps[2].parse().unwrap_or_default()];
        return Some((
            Episodes::Standard {
                season: caps[1].parse().unwrap_or_default(),
                episodes,
            },
            Some((whole.start(), whole.end())),
        ));
    }
    if let Some(caps) = CROSS_EPISODE.captures(rest) {
        let whole = caps.get(0).expect("a match has group 0");
        let episodes = vec![caps[2].parse().unwrap_or_default()];
        return Some((
            Episodes::Standard {
                season: caps[1].parse().unwrap_or_default(),
                episodes,
            },
            Some((whole.start(), whole.end())),
        ));
    }
    None
}

/// A date for a daily show, or an episode number with no season.
fn dated_or_bare(rest: &str) -> Option<Found> {
    if let Some(caps) = DAILY.captures(rest) {
        let whole = caps.get(0).expect("a match has group 0");
        return Some((
            Episodes::Daily(format!("{}-{}-{}", &caps[1], &caps[2], &caps[3])),
            Some((whole.start(), whole.end())),
        ));
    }
    // An episode number with no season ("E05", "Ep05", "Episode 5"): numbered from the first
    // episode, and never a movie.
    if let Some(caps) = BARE_EPISODE.captures(rest) {
        let whole = caps.get(0).expect("a match has group 0");
        let number = caps
            .get(1)
            .or(caps.get(2))
            .and_then(|m| m.as_str().parse().ok());
        if let Some(number) = number {
            return Some((
                Episodes::Absolute(vec![number]),
                Some((whole.start(), whole.end())),
            ));
        }
    }
    None
}

/// An anime batch ("01-12") or absolute episode, where the name allows anime numbering.
fn anime_numbers(rest: &str, anime_style: bool) -> Option<Found> {
    // Anime numbering: a fansub name ("[Group] Show - 05"), or a name with no year at all
    // (a year means "Mission Impossible - 7 (2023)" could be a movie).
    if anime_style || years(rest).is_empty() {
        let resolution_like =
            |n: i64| matches!(n, 480 | 576 | 720 | 1080 | 2160) || (1900..=2099).contains(&n);
        if let Some(caps) = BATCH.captures(rest) {
            let number = |i: usize, j: usize| {
                caps.get(i)
                    .or(caps.get(j))
                    .and_then(|m| m.as_str().parse::<i64>().ok())
            };
            if let (Some(first), Some(last)) = (number(1, 3), number(2, 4))
                && last > first
                && !resolution_like(first)
                && !resolution_like(last)
            {
                let whole = caps.get(0).expect("a match has group 0");
                return Some((
                    Episodes::Absolute((first..=last).collect()),
                    Some((whole.start(), whole.end())),
                ));
            }
        }
        if let Some(caps) = ABSOLUTE.captures(rest) {
            let number: i64 = caps["number"].parse().unwrap_or_default();
            if !resolution_like(number) {
                let whole = caps.get(0).expect("a match has group 0");
                // A half episode matches no numbered episode; the title still ends here.
                let episodes = match caps.name("half") {
                    Some(_) => Episodes::None,
                    None => Episodes::Absolute(vec![number]),
                };
                return Some((episodes, Some((whole.start(), whole.end()))));
            }
        }
    }
    None
}

/// Whole seasons: every season, a range, a list, or one season.
fn whole_seasons(rest: &str) -> Option<Found> {
    if let Some(m) = COMPLETE_SERIES.find(rest) {
        return Some((Episodes::CompleteSeries, Some((m.start(), m.end()))));
    }
    if let Some(caps) = SEASON_RANGE.captures(rest) {
        let (first, last): (i64, i64) = (
            caps[1].parse().unwrap_or_default(),
            caps[2].parse().unwrap_or_default(),
        );
        if last > first {
            let whole = caps.get(0).expect("a match has group 0");
            return Some((
                Episodes::Seasons((first..=last).collect()),
                Some((whole.start(), whole.end())),
            ));
        }
    }
    if let Some(caps) = SEASON_LIST.captures(rest) {
        let seasons = caps[1]
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|n| n.parse().ok())
            .collect();
        let whole = caps.get(0).expect("a match has group 0");
        return Some((
            Episodes::Seasons(seasons),
            Some((whole.start(), whole.end())),
        ));
    }
    if let Some(caps) = SEASON_ONE.captures(rest) {
        let whole = caps.get(0).expect("a match has group 0");
        // "Open.Season.2.2008" is a movie: a year right after the number.
        let after = rest[whole.end()..].trim_start_matches(is_separator);
        let year_follows = years(after).first().is_some_and(|(at, _)| *at == 0);
        if !year_follows {
            let season = caps[1].parse().unwrap_or_default();
            return Some((
                Episodes::Seasons(vec![season]),
                Some((whole.start(), whole.end())),
            ));
        }
    }
    None
}

/// More episodes after the first: "E02", "-E03", "-03", "+E02", repeated. A bare number needs a
/// dash right before it and nothing alphanumeric after ("S01E02-720p" isn't a range), and
/// anything else ("S03E07.42", "S01E01.1080i") ends the list.
pub(super) fn more_episodes(rest: &str, mut at: usize, first: i64) -> (Vec<i64>, usize) {
    let mut found = Vec::new();
    let mut last = first;
    loop {
        let tail = &rest[at..];
        let (dash, after_dash) = match tail.strip_prefix(['-', '+']) {
            Some(r) => (true, r),
            None => (false, tail),
        };
        let (e, after_e) = match after_dash.strip_prefix(['e', 'E']) {
            Some(r) => (true, r),
            None => (false, after_dash),
        };
        let digits = after_e.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 4 || (!e && !dash) {
            break;
        }
        let after_number = &after_e[digits..];
        if !e && after_number.starts_with(|c: char| c.is_ascii_alphanumeric()) {
            break;
        }
        let Ok(n) = after_e[..digits].parse::<i64>() else {
            break;
        };
        if n <= last {
            break;
        }
        // "E01-03" and "E01-E03" are ranges; "E01E02" lists.
        if dash && n > last + 1 {
            found.extend(last + 1..n);
        }
        found.push(n);
        last = n;
        at = rest.len() - after_number.len();
    }
    (found, at)
}
