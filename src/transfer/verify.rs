//! Checking a copied movie or episode is the title it's for before it's put in place: how long
//! its video runs, read with ffprobe, against TMDB's runtime. A release can carry the right name
//! and hold another film; one that runs far longer or shorter than the title is held back.
//!
//! The tolerances are wide enough for real releases: a cut a few minutes off (credits, PAL
//! speed-up at 4%) and a longer one (an extended or director's cut) pass, a different film
//! rarely does. Music isn't checked.

use std::collections::HashMap;

use super::*;
use crate::release::{self, Episodes};

/// How long a download's videos should run, from TMDB's runtimes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Expected {
    /// A movie: its runtime in seconds, and whether the release names a longer cut.
    Movie { seconds: f64, longer_cut: bool },
    /// Episodes of one season: each one's runtime in seconds that TMDB has, and the episodes the
    /// release name holds, for a file whose own name says none.
    Episodes {
        season: i64,
        runtimes: HashMap<i64, f64>,
        release: Episodes,
    },
}

/// What a check needs: the runtimes to expect, and ffprobe to read the files' (None when it
/// isn't known, which holds a checked download back).
pub(crate) struct Check {
    pub(crate) expected: Expected,
    pub(crate) ffprobe: Option<PathBuf>,
    /// How long the longest video ran when it was checked, for an admin's later override.
    pub(crate) measured: std::sync::Mutex<Option<f64>>,
}

/// Edition words that mean a cut longer than the one TMDB times.
static LONGER_CUT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\b(extended|director'?s[ ._-]?cut|uncut|unrated|ultimate|redux|final[ ._-]?cut|special[ ._-]?edition|unabridged)\b",
    )
    .expect("a valid pattern")
});

/// The runtimes a movie or episode download is checked against; None when TMDB has none for it
/// (nothing to compare, so it's placed as before) or it's music.
pub(crate) async fn expected(pool: &SqlitePool, row: &Row) -> sqlx::Result<Option<Expected>> {
    // An admin said the runtime is right after all: the last held-back film's, or no check.
    if let Some(title) = row.target().map(crate::runtime_checks::title_of)
        && let Some(instead) = crate::runtime_checks::override_for(pool, title).await?
    {
        return Ok(match (instead, row.wanted_movie_id) {
            (Some(seconds), Some(_)) => Some(Expected::Movie {
                seconds,
                longer_cut: LONGER_CUT.is_match(&row.title),
            }),
            _ => None,
        });
    }
    if let Some(movie) = row.wanted_movie_id {
        let minutes: Option<i64> = sqlx::query_scalar(
            "SELECT t.runtime_minutes FROM wanted_movies w JOIN tmdb_movies t ON t.tmdb_id = w.tmdb_id
             WHERE w.id = ?",
        )
        .bind(movie)
        .fetch_optional(pool)
        .await?
        .flatten();
        return Ok(minutes.filter(|m| *m > 0).map(|m| Expected::Movie {
            seconds: m as f64 * 60.0,
            longer_cut: LONGER_CUT.is_match(&row.title),
        }));
    }
    let (Some(series), Some(season)) = (row.wanted_series_id, row.season) else {
        return Ok(None);
    };
    let runtimes: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT e.episode, e.runtime_minutes FROM wanted_series w
         JOIN tmdb_episodes e ON e.series_tmdb_id = w.tmdb_id
         WHERE w.id = ? AND e.season = ? AND e.runtime_minutes > 0",
    )
    .bind(series)
    .bind(season)
    .fetch_all(pool)
    .await?;
    if runtimes.is_empty() {
        return Ok(None);
    }
    Ok(Some(Expected::Episodes {
        season,
        runtimes: runtimes
            .into_iter()
            .map(|(e, m)| (e, m as f64 * 60.0))
            .collect(),
        release: release::parse(&row.title).episodes,
    }))
}

/// Whether a movie file of `file` seconds can be the film TMDB times at `film`: down to 10% or
/// 10 minutes shorter (whichever is more), and up to 20% or 20 minutes longer, or 60% or 30
/// minutes for a release that names a longer cut.
pub(crate) fn movie_fits(file: f64, film: f64, longer_cut: bool) -> bool {
    let shorter = (film * 0.10).max(600.0);
    let longer = if longer_cut {
        (film * 0.60).max(1800.0)
    } else {
        (film * 0.20).max(1200.0)
    };
    file >= film - shorter && file <= film + longer
}

/// Whether an episode file of `file` seconds can be episodes TMDB times at `episodes` in all:
/// within a third or 8 minutes either way (TMDB's episode runtimes are rounder than a film's).
pub(crate) fn episode_fits(file: f64, episodes: f64) -> bool {
    (file - episodes).abs() <= (episodes / 3.0).max(480.0)
}

/// "2 h 50 m", "42 m", or "12 s" for under a minute.
pub(crate) fn running_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0).round() as i64;
    let (hours, minutes) = (seconds / 3600, seconds % 3600 / 60);
    match (hours, minutes) {
        (0, 0) => format!("{seconds} s"),
        (0, m) => format!("{m} m"),
        (h, m) => format!("{h} h {m} m"),
    }
}

/// A video to check: its name, and where it is now (its part file, or the library once this
/// download placed it).
pub(crate) struct Video<'a> {
    pub(crate) name: &'a str,
    pub(crate) path: &'a Path,
}

/// Checks the download's videos run as long as the title: Ok when they do, a stop that holds it
/// back as not the title when they don't or a running time can't be read, and a retry when
/// ffprobe can't run or the file can't be opened (which says nothing about the file).
pub(crate) async fn check(check: &Check, videos: &[Video<'_>]) -> Result<(), Stop> {
    let Some(ffprobe) = check.ffprobe.as_deref() else {
        return Err(Stop::Retry(
            "ffprobe isn't available to check how long it runs".into(),
        ));
    };
    let mut durations = Vec::with_capacity(videos.len());
    for video in videos {
        let seconds = match crate::probe::probe(ffprobe, video.path).await {
            Ok(info) => info.duration_seconds.filter(|s| *s > 0.0),
            Err(crate::probe::ProbeError::Failed(why)) => {
                return Err(unreadable(video.name, &why));
            }
            Err(other) => {
                return Err(Stop::Retry(format!(
                    "checking how long {} runs: {other}",
                    video.name
                )));
            }
        };
        let Some(seconds) = seconds else {
            return Err(unreadable(video.name, "ffprobe found no running time"));
        };
        durations.push((video.name, seconds));
    }
    if let Ok(mut measured) = check.measured.lock() {
        *measured = durations.iter().map(|d| d.1).reduce(f64::max);
    }
    match &check.expected {
        Expected::Movie {
            seconds: film,
            longer_cut,
        } => movie_matches(&durations, *film, *longer_cut),
        Expected::Episodes {
            season,
            runtimes,
            release,
        } => episodes_match(&durations, *season, runtimes, release),
    }
}

fn unreadable(name: &str, why: &str) -> Stop {
    let why: String = why
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect();
    Stop::NotTheTitle(format!("couldn't read how long {name} runs ({why})"))
}

/// A movie matches when its longest video does on its own: extras may sit beside the film, but
/// videos never add up to it (a film in parts is held back).
fn movie_matches(durations: &[(&str, f64)], film: f64, longer_cut: bool) -> Result<(), Stop> {
    let Some(&(name, longest)) = durations.iter().max_by(|a, b| a.1.total_cmp(&b.1)) else {
        return Ok(());
    };
    if movie_fits(longest, film, longer_cut) {
        return Ok(());
    }
    Err(Stop::NotTheTitle(format!(
        "{name} runs {}; the film runs {}",
        running_time(longest),
        running_time(film)
    )))
}

/// Each video whose episodes are known, by its own name or else the release's when it's the
/// only video, must run as long as they do together; one whose runtimes TMDB lacks isn't judged.
/// Among several videos, one whose name says no episode of the season is held back: it can't be
/// told apart from another film.
fn episodes_match(
    durations: &[(&str, f64)],
    season: i64,
    runtimes: &HashMap<i64, f64>,
    release: &Episodes,
) -> Result<(), Stop> {
    let only = durations.len() == 1;
    for &(name, seconds) in durations {
        let parsed = release::parse(name).episodes;
        let Some(episodes) = episodes_of(&parsed, release, season, only) else {
            if only {
                continue;
            }
            return Err(Stop::NotTheTitle(format!(
                "{name}'s name doesn't say which episode of season {season} it is, so it can't be checked"
            )));
        };
        let Some(expected) = episodes
            .iter()
            .map(|e| runtimes.get(e).copied())
            .sum::<Option<f64>>()
        else {
            continue;
        };
        if episode_fits(seconds, expected) {
            continue;
        }
        let which = match episodes {
            [one] => format!("episode {one} runs"),
            [first, .., last] => format!("episodes {first} to {last} run"),
            [] => continue,
        };
        return Err(Stop::NotTheTitle(format!(
            "{name} runs {}; {which} {}",
            running_time(seconds),
            running_time(expected)
        )));
    }
    Ok(())
}

/// The season's episodes a video holds: by its own name, or the release's when it's the only
/// video and its name says none.
fn episodes_of<'a>(
    parsed: &'a Episodes,
    release: &'a Episodes,
    season: i64,
    only: bool,
) -> Option<&'a [i64]> {
    match (parsed, release) {
        (
            Episodes::Standard {
                season: s,
                episodes,
            },
            _,
        ) if *s == season => Some(episodes.as_slice()),
        (
            Episodes::None,
            Episodes::Standard {
                season: s,
                episodes,
            },
        ) if *s == season && only => Some(episodes.as_slice()),
        _ => None,
    }
    .filter(|e| !e.is_empty())
}

/// Checks the copied videos run as long as the title. When one doesn't and an earlier try already
/// put some of the download's files in the library, it's held for an admin, naming them: nothing
/// more is placed, and what's there stays until someone looks.
pub(super) async fn check_staged(check: &Check, staged: &[Staged]) -> Result<(), Stop> {
    let videos: Vec<Video> = staged
        .iter()
        .filter(|s| has_extension(&s.name, VIDEO))
        .map(|s| Video {
            name: &s.name,
            path: s.partial.as_deref().unwrap_or(&s.target),
        })
        .collect();
    let placed: Vec<String> = staged
        .iter()
        .filter(|s| s.partial.is_none())
        .map(|s| s.target.display().to_string())
        .collect();
    match self::check(check, &videos).await {
        Err(Stop::NotTheTitle(why)) if !placed.is_empty() => Err(Stop::Held(format!(
            "{why}; an earlier try already put {} in the library, so it's held for an admin to check {}",
            placed.join(", "),
            if placed.len() == 1 { "it" } else { "them" }
        ))),
        checked => checked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: f64 = 60.0;

    #[test]
    fn real_cuts_fit_and_other_films_dont() {
        // PAL speed-up (4% shorter), a cut without long credits, a slightly longer cut.
        assert!(movie_fits(163.0 * MIN, 170.0 * MIN, false));
        assert!(movie_fits(160.0 * MIN, 170.0 * MIN, false));
        assert!(movie_fits(190.0 * MIN, 170.0 * MIN, false));
        // An extended cut 30% longer passes only when the name says so.
        assert!(!movie_fits(263.0 * MIN, 201.0 * MIN, false));
        assert!(movie_fits(263.0 * MIN, 201.0 * MIN, true));
        // Another film: far shorter, or far longer.
        assert!(!movie_fits(92.0 * MIN, 170.0 * MIN, false));
        assert!(!movie_fits(92.0 * MIN, 170.0 * MIN, true));
        assert!(!movie_fits(150.0 * MIN, 95.0 * MIN, false));
        // A short film: 10 minutes either side at least.
        assert!(movie_fits(12.0 * MIN, 20.0 * MIN, false));
        assert!(!movie_fits(9.0 * MIN, 20.0 * MIN, false));
    }

    #[test]
    fn episodes_get_a_third_or_eight_minutes() {
        assert!(episode_fits(42.0 * MIN, 60.0 * MIN), "a 60-minute slot");
        assert!(episode_fits(23.5 * MIN, 24.0 * MIN));
        assert!(episode_fits(16.5 * MIN, 24.0 * MIN));
        assert!(!episode_fits(2.0 * MIN, 24.0 * MIN));
        assert!(
            !episode_fits(100.0 * MIN, 45.0 * MIN),
            "a film, not an episode"
        );
    }

    #[test]
    fn longer_cuts_are_read_from_the_name() {
        for name in [
            "Some.Movie.2001.EXTENDED.1080p.BluRay.x264-GRP",
            "Some Movie 2001 Directors Cut 2160p",
            "Some.Movie.2001.Director's.Cut.1080p",
            "Some.Movie.1979.Final.Cut.REMASTERED.1080p",
        ] {
            assert!(LONGER_CUT.is_match(name), "{name}");
        }
        assert!(!LONGER_CUT.is_match("Some.Movie.2024.1080p.WEBRip.x265-GRP"));
    }

    #[test]
    fn running_times_read_plainly() {
        assert_eq!(running_time(10_200.0), "2 h 50 m");
        assert_eq!(running_time(5_520.0), "1 h 32 m");
        assert_eq!(running_time(2_520.0), "42 m");
        assert_eq!(running_time(12.4), "12 s");
    }
}
