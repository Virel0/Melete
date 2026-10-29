//! Reading titles, years, ID hints and episode numbers out of folder and file names.

use std::sync::LazyLock;

use regex::Regex;

/// What a movie or series folder's name says about the title.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ParsedTitle {
    /// The title, with ID tags, the year and anything after it taken off; dots and underscores
    /// become spaces in release-style names.
    pub title: String,
    /// The year, from 1900 to 2099; None when the name has none, or only one that's part of the
    /// title ("Blade Runner 2049").
    pub year: Option<i64>,
    /// A TMDB id from a tag such as `[tmdbid-603]`; None without one.
    pub tmdb_id: Option<i64>,
    /// A TVDB id from a tag such as `{tvdb-81189}`; None without one.
    pub tvdb_id: Option<i64>,
    /// An IMDb id from a tag such as `[imdbid-tt0133093]`, in lower case; None without one.
    pub imdb_id: Option<String>,
}

/// The season and episode an episode file's name gives.
#[derive(Debug, PartialEq, Eq)]
pub struct EpisodeNumber {
    /// The season's number; 0 is specials.
    pub season: i64,
    /// The (first) episode's number within the season.
    pub episode: i64,
    /// The last episode of a file holding several (`S01E02-E03`); None for a single episode.
    pub episode_end: Option<i64>,
}

/// `[tmdbid-603]`, `{tmdb-603}`, `[imdbid-tt0133093]`, `{tvdb=81189}` (Jellyfin, Radarr and Sonarr styles).
static ID_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[\[{]\s*(tmdb|tvdb|imdb)(?:id)?\s*[-=]\s*([a-z0-9]+)\s*[\]}]")
        .expect("a valid pattern")
});
static PAREN_YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[(\[]((?:19|20)\d{2})[)\]]").expect("a valid pattern"));
static BARE_YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b((?:19|20)\d{2})\b").expect("a valid pattern"));
static SEASON_EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])s(\d{1,3})[ ._-]?e(\d{1,4})").expect("a valid pattern")
});
static CROSS_EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(\d{1,2})x(\d{2,3})(?:[^a-z0-9]|$)").expect("a valid pattern")
});

/// Parses a movie or series folder name (or a loose movie file's stem).
pub fn parse_title(name: &str) -> ParsedTitle {
    let mut parsed = ParsedTitle::default();
    for caps in ID_TAG.captures_iter(name) {
        let value = &caps[2];
        match caps[1].to_ascii_lowercase().as_str() {
            "tmdb" => parsed.tmdb_id = value.parse().ok(),
            "tvdb" => parsed.tvdb_id = value.parse().ok(),
            _ => {
                let value = value.to_ascii_lowercase();
                if value.strip_prefix("tt").is_some_and(is_digits) {
                    parsed.imdb_id = Some(value);
                }
            }
        }
    }
    let name = ID_TAG.replace_all(name, " ");

    // Release-style names use dots or underscores for spaces; "Mr. Robot" does not.
    let name = if name.trim().contains(' ') {
        name.replace('_', " ")
    } else {
        name.replace(['.', '_'], " ")
    };

    let mut title = name.as_str();
    if let Some(m) = PAREN_YEAR.captures_iter(&name).last() {
        parsed.year = m[1].parse().ok();
        title = &name[..m.get(0).expect("group 0 is the whole match").start()];
    } else if let Some(m) = BARE_YEAR.captures_iter(&name).last() {
        // A bare year only counts with something after it ("Movie 1999 1080p BluRay");
        // otherwise it is part of the title ("Blade Runner 2049").
        let year = m.get(1).expect("the year group isn't optional");
        if year.start() > 0 && !name[year.end()..].trim().is_empty() {
            parsed.year = year.as_str().parse().ok();
            title = &name[..year.start()];
        }
    }

    parsed.title = title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches([' ', '-', '(', '['])
        .to_string();
    parsed
}

/// Parses `S01E02`, `S01E02E03`, `S01E02-E03`, `S01E02-03` or `1x02` from an episode file's stem.
/// Returns `None` for anything else (absolute anime numbering, dates), which stays unparsed.
pub fn parse_episode(stem: &str) -> Option<EpisodeNumber> {
    if let Some(caps) = SEASON_EPISODE.captures(stem) {
        let season = caps[1].parse().ok()?;
        let episode = caps[2].parse().ok()?;
        let rest = &stem[caps.get(2).expect("the episode group isn't optional").end()..];
        return Some(EpisodeNumber {
            season,
            episode,
            episode_end: episode_end(rest, episode),
        });
    }
    let caps = CROSS_EPISODE.captures(stem)?;
    Some(EpisodeNumber {
        season: caps[1].parse().ok()?,
        episode: caps[2].parse().ok()?,
        episode_end: None,
    })
}

/// Reads the continuation after a first episode: `E03`, `-E03` or `-03`, repeated.
/// A bare `-03` must not run into letters, so `S01E02-720p` is not a range.
fn episode_end(mut rest: &str, first: i64) -> Option<i64> {
    let mut end: Option<i64> = None;
    loop {
        let after_dash = rest.strip_prefix('-');
        let after_e = after_dash.unwrap_or(rest).strip_prefix(['e', 'E']);
        let number_start = after_e.or(after_dash).unwrap_or(rest);
        let digits = number_start.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 4 || (after_e.is_none() && after_dash.is_none()) {
            break;
        }
        let after_number = &number_start[digits..];
        if after_e.is_none() && after_number.starts_with(|c: char| c.is_ascii_alphanumeric()) {
            break;
        }
        let n: i64 = number_start[..digits].parse().ok()?;
        if n <= end.unwrap_or(first) {
            break;
        }
        end = Some(n);
        rest = after_number;
    }
    end
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title(name: &str) -> (String, Option<i64>) {
        let parsed = parse_title(name);
        (parsed.title, parsed.year)
    }

    #[test]
    fn plain_folder_with_year() {
        assert_eq!(
            title("The Matrix (1999)"),
            ("The Matrix".into(), Some(1999))
        );
    }

    #[test]
    fn underscores_become_spaces() {
        assert_eq!(
            title("A_Knight_of_the_Seven_Kingdoms_(2026)"),
            ("A Knight of the Seven Kingdoms".into(), Some(2026))
        );
    }

    #[test]
    fn dots_kept_when_name_has_spaces() {
        assert_eq!(title("Mr. Robot (2015)"), ("Mr. Robot".into(), Some(2015)));
    }

    #[test]
    fn release_style_name() {
        assert_eq!(
            title("Blade.Runner.2049.2017.1080p.BluRay.x264-GROUP"),
            ("Blade Runner 2049".into(), Some(2017))
        );
    }

    #[test]
    fn number_in_title_is_not_a_year() {
        assert_eq!(
            title("Blade Runner 2049"),
            ("Blade Runner 2049".into(), None)
        );
        assert_eq!(title("1917 (2019)"), ("1917".into(), Some(2019)));
        assert_eq!(
            title("2001 A Space Odyssey (1968)"),
            ("2001 A Space Odyssey".into(), Some(1968))
        );
    }

    #[test]
    fn no_year() {
        assert_eq!(title("Severance"), ("Severance".into(), None));
    }

    #[test]
    fn id_tags_are_read_and_removed() {
        let parsed = parse_title("The Matrix (1999) [tmdbid-603] {imdb-tt0133093}");
        assert_eq!(
            parsed,
            ParsedTitle {
                title: "The Matrix".into(),
                year: Some(1999),
                tmdb_id: Some(603),
                tvdb_id: None,
                imdb_id: Some("tt0133093".into()),
            }
        );
        let parsed = parse_title("Severance {tvdb-371980}");
        assert_eq!(parsed.title, "Severance");
        assert_eq!(parsed.tvdb_id, Some(371980));
    }

    #[test]
    fn malformed_imdb_tag_is_ignored() {
        assert_eq!(parse_title("Movie [imdbid-12345]").imdb_id, None);
    }

    fn ep(stem: &str) -> Option<(i64, i64, Option<i64>)> {
        parse_episode(stem).map(|e| (e.season, e.episode, e.episode_end))
    }

    #[test]
    fn season_episode_forms() {
        assert_eq!(ep("Severance - S01E02 - Half Loop"), Some((1, 2, None)));
        assert_eq!(ep("severance.s01e02.1080p"), Some((1, 2, None)));
        assert_eq!(ep("Show_S02E10_720p"), Some((2, 10, None)));
        assert_eq!(ep("Show S00E01 Special"), Some((0, 1, None)));
        assert_eq!(ep("Show 1x02 Title"), Some((1, 2, None)));
    }

    #[test]
    fn multi_episode_files() {
        assert_eq!(ep("Show S01E01E02"), Some((1, 1, Some(2))));
        assert_eq!(ep("Show S01E01-E03 Title"), Some((1, 1, Some(3))));
        assert_eq!(ep("Show S01E01-02"), Some((1, 1, Some(2))));
        assert_eq!(ep("Show S01E01E02E03"), Some((1, 1, Some(3))));
    }

    #[test]
    fn quality_tags_are_not_episode_ranges() {
        assert_eq!(ep("Show S01E02-720p"), Some((1, 2, None)));
        assert_eq!(ep("Show S01E02-1080p.WEB"), Some((1, 2, None)));
        assert_eq!(ep("Show S01E02-10bit"), Some((1, 2, None)));
    }

    #[test]
    fn resolution_is_not_season_cross_episode() {
        assert_eq!(ep("Movie 1920x1080"), None);
    }

    #[test]
    fn unparseable_names() {
        assert_eq!(ep("[Group] Frieren - 05 [1080p]"), None);
        assert_eq!(ep("The Daily Show 2024-03-05"), None);
    }
}
