//! Reading a release name: what it is (title, year, episodes) and what it's like (quality,
//! HDR, audio, languages, group). The download side decides on these facts.
//!
//! Quality names follow Radarr and Sonarr ("WEBDL-1080p", "Remux-2160p"), so profiles imported
//! from them line up.
//!
//! Two rules keep titles and episode titles from being read as tags. Only unambiguous markers
//! (an explicit resolution, a named source, a codec) end a title; words like "web", "max" or
//! "uhd" count only once the title is over. And between the title and the first such marker,
//! where years, episode titles and scene tags sit, only ALL-CAPS words count as tags: scene
//! tags are written "FRENCH", "MULTi", "HDCAM", episode titles "The.Cam" or "Dual.Citizenship".

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

mod episodes;
mod naming;
mod patterns;
mod tags;

use episodes::*;
use naming::*;
use patterns::*;
use tags::*;

/// The episodes a release name holds: which seasons and episodes, in the numbering the name uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Episodes {
    /// A movie, or nothing recognisable.
    None,
    /// A season and its episodes: one ("S01E02") or several in a row ("S01E02-E03").
    Standard {
        /// The season number.
        season: i64,
        /// The episode numbers in that season, in order.
        episodes: Vec<i64>,
    },
    /// Whole seasons (a pack).
    Seasons(Vec<i64>),
    /// Every season ("Complete Series").
    CompleteSeries,
    /// Anime absolute numbering: one episode, or a batch.
    Absolute(Vec<i64>),
    /// YYYY-MM-DD, for daily shows.
    Daily(String),
}

/// Where a release was taken from, from the worst (a camera in a cinema) to the best (a
/// Blu-ray). The order is the derived `Ord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Filmed in a cinema ("CAM", "HDCAM").
    Cam,
    /// Filmed in a cinema with the sound taken directly ("TS", "TELESYNC").
    Telesync,
    /// Copied from the cinema's film reel ("TC", "TELECINE").
    Telecine,
    /// A review copy sent out before release ("SCREENER", "DVDSCR").
    Screener,
    /// A DVD, ripped or whole.
    Dvd,
    /// Recorded from television.
    Hdtv,
    /// Captured from a streaming service's player and re-encoded.
    WebRip,
    /// Downloaded untouched from a streaming service.
    WebDl,
    /// A Blu-ray: an encode, a remux or the whole disc.
    Bluray,
}

/// What a release name says: the title it's for and what the download is like. Built by
/// [`parse`]; every field is read from the name alone, so a name that doesn't say leaves it
/// `None`, `false` or empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Release {
    /// The title, with separators turned into spaces ("The Matrix").
    pub title: String,
    /// The year after the title, if the name has one.
    pub year: Option<i64>,
    /// The episodes the name holds; [`Episodes::None`] for a movie.
    pub episodes: Episodes,
    /// 480, 576, 720, 1080 or 2160.
    pub resolution: Option<i64>,
    /// Where it was taken from; `None` when the name names no source.
    pub source: Option<Source>,
    /// A remux: the disc's video and audio repackaged, not re-encoded.
    pub remux: bool,
    /// A full disc rather than an encode (Radarr's BR-DISK, or DVD-R for a DVD).
    pub disc: bool,
    /// Dolby Vision HDR ("DV", "DoVi").
    pub dolby_vision: bool,
    /// HDR10+.
    pub hdr10_plus: bool,
    /// HDR10; also set for HDR10+, which builds on it.
    pub hdr10: bool,
    /// HLG HDR, the broadcast kind.
    pub hlg: bool,
    /// "h264", "h265", "av1", "xvid" or "mpeg2".
    pub video_codec: Option<&'static str>,
    /// "truehd", "dts-x", "dts-hd ma", "dts-hd hra", "dts", "eac3", "ac3", "aac", "flac", "opus",
    /// "pcm" or "mp3".
    pub audio_codec: Option<&'static str>,
    /// Dolby Atmos.
    pub atmos: bool,
    /// "7.1", "5.1", "2.0"…
    pub audio_channels: Option<String>,
    /// Audio languages the name mentions, as ISO 639-1 codes.
    pub languages: Vec<&'static str>,
    /// Two audio languages, usually the original and a dub ("Dual Audio", "German DL").
    pub dual_audio: bool,
    /// Several audio languages ("MULTi").
    pub multi_audio: bool,
    /// Subtitle languages the name mentions ("en" for EngSub, "fr" for VOSTFR).
    pub subtitles: Vec<&'static str>,
    /// Subtitles in several languages ("Multi-Subs").
    pub multi_subs: bool,
    /// Subtitles burned into the picture ("HC", "HARDSUB").
    pub hardcoded_subs: bool,
    /// How many PROPER, REPACK and RERIP (REPACK2 counts 2).
    pub proper: u8,
    /// Anime re-releases: 2 for "05v2".
    pub version: Option<u8>,
    /// The release group: "[Group]" at the start or "-GROUP" at the end.
    pub group: Option<String>,
}

impl Release {
    /// The quality as Radarr names it, always one of its qualities. A resolution without a
    /// known source counts as HDTV, as in Sonarr; standard definition is SDTV, 576p web joins
    /// 480p, and a remux below 1080p counts as the Blu-ray encode (Radarr has none).
    pub fn quality(&self) -> String {
        match (self.source, self.resolution) {
            (Some(Source::Cam), _) => "CAM".into(),
            (Some(Source::Telesync), _) => "TELESYNC".into(),
            (Some(Source::Telecine), _) => "TELECINE".into(),
            (Some(Source::Screener), _) => "DVDSCR".into(),
            // A full DVD; a DVD remux is DVD, as in Radarr.
            (Some(Source::Dvd), _) if self.disc => "DVD-R".into(),
            (Some(Source::Dvd), _) => "DVD".into(),
            (Some(Source::Bluray), _) if self.disc => "BR-DISK".into(),
            (Some(Source::Bluray), Some(r @ (1080 | 2160))) if self.remux => format!("Remux-{r}p"),
            (Some(Source::Bluray), Some(r)) => format!("Bluray-{r}p"),
            (Some(Source::WebDl), Some(576)) => "WEBDL-480p".into(),
            (Some(Source::WebDl), Some(r)) => format!("WEBDL-{r}p"),
            (Some(Source::WebRip), Some(576)) => "WEBRip-480p".into(),
            (Some(Source::WebRip), Some(r)) => format!("WEBRip-{r}p"),
            (Some(Source::Hdtv) | None, Some(480 | 576)) | (Some(Source::Hdtv), None) => {
                "SDTV".into()
            }
            (Some(Source::Hdtv) | None, Some(r)) => format!("HDTV-{r}p"),
            (Some(Source::Bluray), None) if self.remux => "Remux-1080p".into(),
            (Some(Source::Bluray), None) => "Bluray-1080p".into(),
            (Some(Source::WebDl | Source::WebRip), None) | (None, None) => "Unknown".into(),
        }
    }
}

/// Every standalone four-digit year in `text`, with its position. Scanned by hand: a regex
/// that consumes the separator after one year would miss a year right after it ("2049.2017").
fn years(text: &str) -> Vec<(usize, i64)> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    for start in 0..bytes.len().saturating_sub(3) {
        let before_ok = start == 0 || is_separator(bytes[start - 1] as char);
        let after_ok = start + 4 == bytes.len() || is_separator(bytes[start + 4] as char);
        let digits = &bytes[start..start + 4];
        if before_ok && after_ok && digits.iter().all(u8::is_ascii_digit) {
            let year: i64 = text[start..start + 4].parse().unwrap_or_default();
            if (1900..=2099).contains(&year) {
                found.push((start, year));
            }
        }
    }
    found
}

/// The words of `text` written like scene tags: nearly all capitals ("FRENCH", "MULTi",
/// "iNTERNAL"), or starting with a digit ("4K", "10bit"). "The", "Cam" and "Dual" don't pass.
/// Bracketed words pass whatever their case, since fansubs tag "(English Dub)", and so do
/// propers and "German DL", which scene names write in mixed case.
fn tag_like_words(text: &str) -> String {
    static BRACKETED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[[^\]]*\]|\([^)]*\)").expect("a valid pattern"));
    let scene_case = |word: &str| {
        let letters = word.chars().filter(char::is_ascii_alphabetic).count();
        let capitals = word.chars().filter(char::is_ascii_uppercase).count();
        word.starts_with(|c: char| c.is_ascii_digit())
            || (capitals >= 2 && capitals * 4 >= letters * 3)
            || PROPER_WORD.is_match(word)
    };
    let unbracketed = BRACKETED.replace_all(text, " ");
    let words = unbracketed
        .split(is_separator)
        .filter(|word| scene_case(word));
    let bracketed = BRACKETED.find_iter(text).map(|m| m.as_str());
    let dual_language = DUAL_LANGUAGE.find_iter(&unbracketed).map(|m| m.as_str());
    words
        .chain(bracketed)
        .chain(dual_language)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The first strong marker in `text` at or after `from`.
fn first_strong(text: &str, from: usize) -> Option<usize> {
    let strong = [
        &*R2160,
        &*R1080,
        &*R720,
        &*R576,
        &*R480,
        &*REMUX,
        &*BLURAY,
        &*WEBRIP,
        &*WEBDL,
        &*HDTV,
        &*DVDRIP,
        &*CAMRIP,
        &*TELESYNCS,
        &*TELECINES,
        &*SCREENERS,
        &*H264,
        &*H265,
    ];
    let rest = text.get(from..)?;
    let at = strong
        .iter()
        .filter_map(|r| r.find(rest).map(|m| from + m.start()))
        .min()?;
    // Inside "[BD 1080p]", the tags start at the bracket.
    let opener = text[from..at].rfind(['[', '(']).map(|i| from + i);
    let closed = opener.is_some_and(|i| text[i..at].contains([']', ')']));
    Some(opener.filter(|_| !closed).unwrap_or(at))
}

/// Reads a release name: an indexer's title or a download's file or folder name. Never fails:
/// what the name doesn't say is left empty.
pub fn parse(name: &str) -> Release {
    let (prefix_group, rest) = split_prefix_group(name);

    let (mut episodes, episode_span) = episodes(&rest, prefix_group.is_some());
    let (title_part, year) = title_and_year(&rest, episode_span);
    let mut title = clean_title(title_part);
    season_in_title(&mut title, &mut episodes);

    let tags = tag_text(&rest, title_part, episode_span);
    let has = |r: &Regex| r.is_match(&tags);

    let resolution = resolution(&has);
    let (remux, source, disc) = source_and_disc(&tags, &has);

    let video_codec = video_codec(&has);
    let audio_codec = audio_codec(&has);
    let audio_channels = audio_channels(&tags);

    // Subtitle phrases first; what's left names the audio. With a subtitle list ("Multiple
    // Subtitle", "Multi-Subs"), bare language words are that list, not audio.
    let multi_subs = has(&MULTI_SUBS);
    let (languages, subtitles, audio_tags) = languages(&tags, multi_subs, &has);

    let version = version(&tags, &episodes, &rest);

    Release {
        title,
        year,
        episodes,
        resolution,
        source,
        remux,
        disc,
        dolby_vision: has(&DOLBY_VISION),
        hdr10_plus: has(&HDR10_PLUS),
        hdr10: has(&HDR10) || has(&HDR10_PLUS),
        hlg: has(&HLG),
        video_codec,
        audio_codec,
        atmos: has(&ATMOS),
        audio_channels,
        languages,
        dual_audio: has(&DUAL_AUDIO) || has(&DUAL_LANGUAGE),
        multi_audio: MULTI.is_match(&audio_tags),
        subtitles,
        multi_subs,
        hardcoded_subs: has(&HARDSUB),
        proper: propers(&tags),
        version,
        group: prefix_group.or_else(|| suffix_group(&rest)),
    }
}

/// The name without its extension and site prefix, split into the group written before it
/// ("[Group] Show - 05") and the rest.
fn split_prefix_group(name: &str) -> (Option<String>, String) {
    let name = EXTENSION.replace(name.trim(), "");
    let name = SITE_PREFIX.replace(&name, "").into_owned();
    match PREFIX_GROUP.captures(&name) {
        Some(caps) => (
            Some(caps[1].trim().to_owned()),
            // Group 0, the whole match, is always there.
            name[caps.get(0).expect("a match has group 0").end()..].to_owned(),
        ),
        None => (None, name.clone()),
    }
}

/// The title part of `rest`, which ends at the episodes, the first strong marker or the year,
/// and that year.
fn title_and_year(rest: &str, episode_span: Option<(usize, usize)>) -> (&str, Option<i64>) {
    let cut = [episode_span.map(|(start, _)| start), first_strong(rest, 0)]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(rest.len());
    let head = &rest[..cut];

    // The last year in the head that isn't its first word ends the title.
    let year_match = years(head).into_iter().rfind(|(at, _)| *at > 0);
    match year_match {
        Some((at, year)) => (&head[..at], Some(year)),
        None => (head, None),
    }
}

/// Moves an anime season written in the title into the episodes.
fn season_in_title(title: &mut String, episodes: &mut Episodes) {
    // An anime season in the title ("Season 2 - 05", "2nd Season [01-12]") numbers the episodes.
    if let Episodes::Absolute(numbers) = &*episodes
        && let Some(caps) = TITLE_SEASON.captures(title)
    {
        let season = caps
            .iter()
            .skip(1)
            .flatten()
            .next()
            .and_then(|m| m.as_str().parse().ok());
        if let Some(season) = season {
            *episodes = Episodes::Standard {
                season,
                episodes: numbers.clone(),
            };
            *title = title[..caps.get(0).expect("a match has group 0").start()]
                .trim()
                .to_owned();
        }
    }
}

/// The text the tags are read from.
fn tag_text(rest: &str, title_part: &str, episode_span: Option<(usize, usize)>) -> String {
    // Tags: everything from the first strong marker after the title (and its episode marker),
    // plus the tag-like words before it.
    let after_title = title_part.len().max(episode_span.map_or(0, |(_, end)| end));
    let region_start = first_strong(rest, after_title).unwrap_or(rest.len());
    let zone = rest
        .get(title_part.len()..region_start.max(title_part.len()))
        .unwrap_or("");
    format!(
        "{} {}",
        tag_like_words(zone),
        &rest[region_start.max(title_part.len())..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(name: &str) -> (String, Option<i64>, Episodes, String, Option<String>) {
        let r = parse(name);
        let quality = r.quality();
        (r.title, r.year, r.episodes, quality, r.group)
    }

    fn standard(season: i64, episodes: &[i64]) -> Episodes {
        Episodes::Standard {
            season,
            episodes: episodes.to_vec(),
        }
    }

    #[test]
    fn movies() {
        // (name, (title, year, quality, group))
        type Case<'a> = (&'a str, (&'a str, Option<i64>, &'a str, Option<&'a str>));
        let cases: &[Case] = &[
            (
                "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR10.HEVC.TrueHD.Atmos.7.1-FraMeSToR",
                (
                    "Dune Part Two",
                    Some(2024),
                    "Remux-2160p",
                    Some("FraMeSToR"),
                ),
            ),
            (
                "The.Matrix.1999.1080p.BluRay.x264-SPARKS",
                ("The Matrix", Some(1999), "Bluray-1080p", Some("SPARKS")),
            ),
            (
                "Oppenheimer 2023 1080p WEB-DL DDP5.1 Atmos H 264-FLUX",
                ("Oppenheimer", Some(2023), "WEBDL-1080p", Some("FLUX")),
            ),
            (
                "Blade.Runner.2049.2017.720p.WEBRip.x265-RARBG",
                (
                    "Blade Runner 2049",
                    Some(2017),
                    "WEBRip-720p",
                    Some("RARBG"),
                ),
            ),
            (
                "2001.A.Space.Odyssey.1968.2160p.UHD.BluRay.x265-TERMiNAL",
                (
                    "2001 A Space Odyssey",
                    Some(1968),
                    "Bluray-2160p",
                    Some("TERMiNAL"),
                ),
            ),
            (
                "1917.2019.1080p.AMZN.WEB-DL.DDP5.1.H.264-NTG",
                ("1917", Some(2019), "WEBDL-1080p", Some("NTG")),
            ),
            (
                "Barbie (2023) [1080p] [WEBRip] [5.1] [YTS.MX]",
                ("Barbie", Some(2023), "WEBRip-1080p", None),
            ),
            (
                "Some.Movie.2022.HDCAM.x264-GRP",
                ("Some Movie", Some(2022), "CAM", Some("GRP")),
            ),
            (
                "Old.Film.1985.DVDRip.XviD-XYZ.avi",
                ("Old Film", Some(1985), "DVD", Some("XYZ")),
            ),
        ];
        for (name, (title, year, quality, group)) in cases {
            let (t, y, e, q, g) = summary(name);
            assert_eq!(
                (t.as_str(), y, q.as_str(), g.as_deref()),
                (*title, *year, *quality, *group),
                "{name}"
            );
            assert_eq!(e, Episodes::None, "{name}");
        }
    }

    #[test]
    fn episodes_and_packs() {
        let cases: &[(&str, &str, Episodes)] = &[
            (
                "Severance.S02E01.Hello.Ms.Cobel.2160p.ATVP.WEB-DL.DDP5.1.Atmos.DV.HDR.H.265-FLUX",
                "Severance",
                standard(2, &[1]),
            ),
            (
                "The.Office.US.S05E14E15.720p.HDTV.x264-LOL",
                "The Office US",
                standard(5, &[14, 15]),
            ),
            (
                "Show.Name.S01E01-E03.1080p.WEB.h264-GRP",
                "Show Name",
                standard(1, &[1, 2, 3]),
            ),
            (
                "Show Name - S03E07 - Episode Title [1080p]",
                "Show Name",
                standard(3, &[7]),
            ),
            ("show.name.3x07.hdtv-lol", "show name", standard(3, &[7])),
            (
                "Doctor.Who.2005.S13E01.1080p.WEB-DL",
                "Doctor Who",
                standard(13, &[1]),
            ),
            (
                "Breaking.Bad.S01.1080p.BluRay.x264-ROVERS",
                "Breaking Bad",
                Episodes::Seasons(vec![1]),
            ),
            (
                "Breaking.Bad.S01-S05.Complete.1080p.BluRay-GRP",
                "Breaking Bad",
                Episodes::Seasons(vec![1, 2, 3, 4, 5]),
            ),
            (
                "The.Daily.Show.2024.03.05.Guest.720p.WEB.h264-EDITH",
                "The Daily Show",
                Episodes::Daily("2024-03-05".into()),
            ),
        ];
        for (name, title, episodes) in cases {
            let r = parse(name);
            assert_eq!(
                (r.title.as_str(), &r.episodes),
                (*title, episodes),
                "{name}"
            );
        }
    }

    #[test]
    fn anime() {
        let r = parse("[SubsPlease] Sousou no Frieren - 05 (1080p) [ABCD1234].mkv");
        assert_eq!(
            (r.title.as_str(), r.episodes.clone(), r.group.as_deref()),
            (
                "Sousou no Frieren",
                Episodes::Absolute(vec![5]),
                Some("SubsPlease")
            )
        );
        assert_eq!(
            r.quality(),
            "HDTV-1080p",
            "a resolution without a source counts as HDTV, as in Sonarr"
        );

        let v2 = parse("[Erai-raws] One Piece - 1085v2 [1080p][Multiple Subtitle].mkv");
        assert_eq!(
            (v2.title.as_str(), v2.episodes.clone(), v2.version),
            ("One Piece", Episodes::Absolute(vec![1085]), Some(2))
        );

        let batch =
            parse("[Judas] Spy x Family (01-12) [BD 1080p][HEVC x265 10bit][Dual-Audio][Eng-Subs]");
        assert_eq!(batch.episodes, Episodes::Absolute((1..=12).collect()));
        assert_eq!(batch.title, "Spy x Family");
        assert_eq!(
            (batch.quality(), batch.video_codec, batch.dual_audio),
            ("Bluray-1080p".to_string(), Some("h265"), true)
        );
        assert_eq!(batch.subtitles, vec!["en"]);

        let seasonal = parse("[ASW] Mushoku Tensei S2 - 05 [1080p HEVC][1A2B3C4D].mkv");
        assert_eq!(seasonal.episodes, standard(2, &[5]));
        assert_eq!(seasonal.title, "Mushoku Tensei");

        let western =
            parse("Frieren.Beyond.Journeys.End.S01E05.1080p.CR.WEB-DL.DUAL.AAC2.0.H.264-VARYG");
        assert_eq!(western.episodes, standard(1, &[5]));
        assert!(western.dual_audio);
        assert_eq!(
            (western.audio_codec, western.audio_channels.as_deref()),
            (Some("aac"), Some("2.0"))
        );
    }

    #[test]
    fn hdr_codecs_and_audio() {
        let r = parse("Movie.2023.2160p.WEB-DL.DDP5.1.Atmos.DV.HDR10Plus.H.265-GRP");
        assert!(r.dolby_vision && r.hdr10_plus && r.hdr10 && r.atmos && !r.hlg);
        assert_eq!(
            (r.video_codec, r.audio_codec, r.audio_channels.as_deref()),
            (Some("h265"), Some("eac3"), Some("5.1"))
        );

        let r = parse("Movie.2020.1080p.BluRay.DTS-HD.MA.7.1.x264-GRP");
        assert_eq!(
            (r.audio_codec, r.audio_channels.as_deref(), r.dolby_vision),
            (Some("dts-hd ma"), Some("7.1"), false)
        );

        let r = parse("Movie.2020.1080p.WEB-DL.AAC2.0.H.264-GRP");
        assert_eq!(
            (r.audio_codec, r.audio_channels.as_deref(), r.hdr10),
            (Some("aac"), Some("2.0"), false)
        );

        let r = parse("Movie.2019.2160p.UHD.BluRay.HLG.HEVC-GRP");
        assert!(r.hlg);
    }

    #[test]
    fn languages_and_subtitles() {
        let r = parse("Movie.2020.MULTi.1080p.BluRay.x264-GRP");
        assert!(r.multi_audio);
        let r = parse("Movie.2020.FRENCH.1080p.WEB.H264-GRP");
        assert_eq!(r.languages, vec!["fr"]);
        let r = parse("Movie.2020.1080p.WEB.H264.VOSTFR-GRP");
        assert_eq!(
            (r.subtitles.clone(), r.languages.clone()),
            (vec!["fr"], Vec::<&str>::new()),
            "VOSTFR is subtitles"
        );
        let r = parse("[Group] Show - 03 [1080p][Multi-Subs]");
        assert!(
            r.multi_subs && !r.multi_audio,
            "multi subs aren't multi audio"
        );
        let r = parse("[Group] Show - 03 (English Dub) [1080p]");
        assert_eq!(r.languages, vec!["en"]);
        let r = parse("Show.S01E01.JAPANESE.1080p.WEB.H264.ENG.SUBS-GRP");
        assert_eq!(
            (r.languages.clone(), r.subtitles.clone()),
            (vec!["ja"], vec!["en"])
        );
        let r = parse("Movie.2021.1080p.WEB-DL.HC.x264-GRP");
        assert!(r.hardcoded_subs);
    }

    #[test]
    fn propers_and_groups() {
        assert_eq!(
            parse("Show.S01E01.PROPER.REPACK.720p.HDTV.x264-GRP").proper,
            2
        );
        assert_eq!(parse("Show.S01E01.720p.HDTV.x264-GRP").proper, 0);
        assert_eq!(
            parse("Movie.2020.1080p.WEB-DL").group,
            None,
            "WEB-DL's DL isn't a group"
        );
        assert_eq!(
            parse("Movie.2020.1080p.BluRay.x264-GROUP[rarbg]")
                .group
                .as_deref(),
            Some("GROUP")
        );
    }

    #[test]
    fn titles_with_numbers_and_words_that_look_like_tags() {
        assert_eq!(
            parse("Planet.51.2009.1080p.BluRay.x264-GRP").audio_channels,
            None,
            "51 in the title isn't 5.1"
        );
        assert_eq!(
            parse("Planet.51.2009.1080p.BluRay.x264-GRP").title,
            "Planet 51"
        );
        assert!(
            !parse("The.Dual.2023.1080p.WEB.H264-GRP").dual_audio,
            "a title word isn't a tag"
        );
        assert_eq!(
            parse("Cam.Girl.2020.1080p.WEB.H264-GRP").quality(),
            "WEBDL-1080p"
        );
    }

    // The cases below came from a stress test of the first version of this parser.

    #[test]
    fn source_and_service_words_dont_end_titles() {
        let cases = [
            (
                "Mad.Max.Fury.Road.2015.1080p.BluRay.x264-SPARKS",
                "Mad Max Fury Road",
            ),
            ("Max.Payne.2008.1080p.BluRay.x264-GRP", "Max Payne"),
            (
                "Charlottes.Web.2006.1080p.BluRay.x264-GRP",
                "Charlottes Web",
            ),
            (
                "Unfriended.Dark.Web.2018.1080p.BluRay.x264-GRP",
                "Unfriended Dark Web",
            ),
            ("Web.Therapy.S01E01.720p.HDTV.x264-GRP", "Web Therapy"),
            (
                "Mobile.Suit.Gundam.00.S01E05.1080p.BluRay.x264-GRP",
                "Mobile Suit Gundam 00",
            ),
        ];
        for (name, title) in cases {
            assert_eq!(parse(name).title, title, "{name}");
        }
        assert_eq!(
            summary("Open.Season.2.2008.1080p.BluRay.x264-GRP"),
            (
                "Open Season 2".into(),
                Some(2008),
                Episodes::None,
                "Bluray-1080p".into(),
                Some("GRP".into())
            )
        );
    }

    #[test]
    fn episode_titles_arent_tags() {
        let r = parse("Show.S02E03.The.Cam.1080p.WEB.H264-GRP");
        assert_eq!(
            (r.quality(), r.source),
            ("WEBDL-1080p".into(), Some(Source::WebDl))
        );
        let r = parse("Show.S02E03.Dual.Citizenship.720p.HDTV.x264-GRP");
        assert!(!r.dual_audio);
        let r = parse("Show.S01E05.The.Web.720p.HDTV.x264-GRP");
        assert_eq!(r.quality(), "HDTV-720p");
        let r = parse("Show.S01E05.The.German.1080p.WEB.H264-GRP");
        assert!(r.languages.is_empty());
        let r = parse("Mr.Robot.S02E01.eps2.0_unm4sk-pt1.tc.720p.WEB-DL.DD5.1-GRP");
        assert_eq!(
            (r.quality(), r.audio_channels),
            ("WEBDL-720p".into(), Some("5.1".into()))
        );
        // Scene tags in mixed case still count.
        assert_eq!(parse("Show.S01E01.Proper.720p.HDTV").proper, 1);
        let r = parse("Show.S01E01.4K.HDR.WEB.H265-GRP");
        assert_eq!((r.quality(), r.hdr10), ("WEBDL-2160p".into(), true));
    }

    #[test]
    fn numbers_after_an_episode_arent_more_episodes() {
        for name in [
            "Doctor.Who.2005.S03E07.42.1080p.BluRay.x264-GRP",
            "Doctor Who (2005) - S03E07 - 42 [Bluray-1080p]",
        ] {
            let r = parse(name);
            assert_eq!(
                (r.title.as_str(), r.episodes),
                ("Doctor Who", standard(3, &[7])),
                "{name}"
            );
        }
        let r = parse("Show.S01E01.1080i.HDTV.MPEG2-GRP");
        assert_eq!(
            (r.quality(), r.episodes),
            ("HDTV-1080p".into(), standard(1, &[1]))
        );
        assert_eq!(
            parse("Show.S01E01.10bit.HEVC-GRP").episodes,
            standard(1, &[1])
        );
        assert_eq!(
            parse("Show.S01E01+E02.1080p.WEB.H264-GRP").episodes,
            standard(1, &[1, 2])
        );
        assert_eq!(
            parse("Show.S2024E05.1080p.WEB.H264-GRP").episodes,
            standard(2024, &[5])
        );
        assert_eq!(
            parse("Show.Season.2.Episode.5.1080p.WEB.H264-GRP").episodes,
            standard(2, &[5])
        );
    }

    #[test]
    fn anime_numbers_and_seasons() {
        let cases = [
            (
                "[SubsPlease] Kaiju No. 8 - 12 (1080p) [ABCD1234].mkv",
                "Kaiju No. 8",
                Episodes::Absolute(vec![12]),
            ),
            (
                "[Group] Mobile Suit Gundam 00 - 05 [1080p]",
                "Mobile Suit Gundam 00",
                Episodes::Absolute(vec![5]),
            ),
            (
                "[SubsPlease] Spy x Family Season 2 - 05 (1080p) [ABCD1234].mkv",
                "Spy x Family",
                standard(2, &[5]),
            ),
            (
                "[SubsPlease] Oshi no Ko 2nd Season - 05 (1080p) [ABCD1234].mkv",
                "Oshi no Ko",
                standard(2, &[5]),
            ),
            (
                "[Group] Show 2nd Season [01-12] [BD 1080p]",
                "Show",
                standard(2, &(1..=12).collect::<Vec<_>>()),
            ),
            (
                "[Group] Show S2 - 01-12 [1080p]",
                "Show",
                standard(2, &(1..=12).collect::<Vec<_>>()),
            ),
            (
                "[Group] Show - 01 ~ 12 [1080p]",
                "Show",
                Episodes::Absolute((1..=12).collect()),
            ),
            (
                "Sousou no Frieren - 05 [1080p].mkv",
                "Sousou no Frieren",
                Episodes::Absolute(vec![5]),
            ),
            // A recap between episodes: no numbered episode, but the title still ends there.
            (
                "[SubsPlease] Frieren - 05.5 (1080p) [ABCD1234].mkv",
                "Frieren",
                Episodes::None,
            ),
        ];
        for (name, title, episodes) in cases {
            let r = parse(name);
            assert_eq!((r.title.as_str(), r.episodes), (title, episodes), "{name}");
        }
        let r = parse("[Group] Show Vol. 1-3 [BD 1080p]");
        assert_eq!(
            (r.title.as_str(), r.quality()),
            ("Show Vol. 1-3", "Bluray-1080p".into())
        );
    }

    #[test]
    fn cam_telesync_and_telecine() {
        let cases = [
            ("Movie.2024.1080p.TS.x264-GRP", "TELESYNC"),
            ("Movie.2024.1080p.HD-TS.x264-GRP", "TELESYNC"),
            ("Movie.2024.720p.HDTC.x264-GRP", "TELECINE"),
            ("Movie.2024.720p.TC.x264-GRP", "TELECINE"),
            ("Movie.2024.1080p.HQCAM.x264-GRP", "CAM"),
        ];
        for (name, quality) in cases {
            assert_eq!(parse(name).quality(), quality, "{name}");
        }
    }

    #[test]
    fn discs_dvd_remuxes_and_uhd() {
        let cases = [
            ("Friends.S01.NTSC.DVD.REMUX.DD5.1-GRP", "DVD"),
            ("Movie.1999.DVD.REMUX.DD5.1-GRP", "DVD"),
            (
                "Dune.2021.1080p.UHD.BluRay.DDP.7.1.HDR.x265-NCmt",
                "Bluray-1080p",
            ),
            ("Movie.2019.COMPLETE.UHD.BLURAY-GRP", "BR-DISK"),
            ("Movie.2010.1080p.Blu-ray.AVC.DTS-HD.MA.5.1-GRP", "BR-DISK"),
            (
                "Movie.2023.2160p.BluRay.REMUX.HEVC.TrueHD.Atmos.7.1-GRP",
                "Remux-2160p",
            ),
            (
                "Movie.2020.1080p.BluRay.x264.DTS-HD.HRA.7.1-GRP",
                "Bluray-1080p",
            ),
        ];
        for (name, quality) in cases {
            assert_eq!(parse(name).quality(), quality, "{name}");
        }
    }

    #[test]
    fn season_packs_and_complete_series() {
        let cases = [
            (
                "Breaking.Bad.Complete.Series.1080p.BluRay.x264-GRP",
                "Breaking Bad",
                Episodes::CompleteSeries,
            ),
            (
                "Show.S01.S02.S03.1080p.BluRay.x264-GRP",
                "Show",
                Episodes::Seasons(vec![1, 2, 3]),
            ),
            (
                "Show.Seasons.1-3.1080p.BluRay.x264-GRP",
                "Show",
                Episodes::Seasons(vec![1, 2, 3]),
            ),
            (
                "Friends.S01.NTSC.DVD.REMUX.DD5.1-GRP",
                "Friends",
                Episodes::Seasons(vec![1]),
            ),
        ];
        for (name, title, episodes) in cases {
            let r = parse(name);
            assert_eq!((r.title.as_str(), r.episodes), (title, episodes), "{name}");
        }
    }

    #[test]
    fn site_prefixes_and_suffixes() {
        let r = parse("[ www.Torrenting.com ] - Show.S01E01.1080p.WEB.H264-GRP");
        assert_eq!(
            (r.title.as_str(), r.group.as_deref()),
            ("Show", Some("GRP"))
        );
        let r = parse("www.UIndex.org    -    Movie.2024.1080p.WEB-DL.DDP5.1.H.264-GRP");
        assert_eq!(
            (r.title.as_str(), r.year, r.group.as_deref()),
            ("Movie", Some(2024), Some("GRP"))
        );
        assert_eq!(
            parse("Movie.2020.1080p.BluRay.x264-GRP (eztv)")
                .group
                .as_deref(),
            Some("GRP")
        );
        assert_eq!(
            parse("Show.S01E01.1080p.WEB.H264-NTb[TGx]")
                .group
                .as_deref(),
            Some("NTb")
        );
        assert_eq!(
            parse("Movie.2024.1080p.WEBRip.x265.10bit.AAC5.1-[YTS.MX]")
                .group
                .as_deref(),
            Some("YTS.MX")
        );
    }

    #[test]
    fn subtitle_lists_and_dual_language() {
        let r = parse(
            "[Erai-raws] Tensei shitara Slime Datta Ken 3rd Season - 05 [1080p][Multiple Subtitle][ENG][POR-BR][SPA-LA][SPA][ARA][FRE][GER][ITA][RUS]",
        );
        assert!(
            r.languages.is_empty(),
            "a subtitle list isn't audio: {:?}",
            r.languages
        );
        assert!(r.multi_subs && r.subtitles.contains(&"en") && r.subtitles.contains(&"de"));
        let r = parse("[Group] Show - 05 [Sub ITA][1080p]");
        assert_eq!((r.languages, r.subtitles), (vec![], vec!["it"]));
        let r = parse("Movie.2023.German.DL.1080p.BluRay.x264-GRP");
        assert_eq!((r.dual_audio, r.languages), (true, vec!["de"]));
        let r = parse("Movie.2024.1080p.HDRip.KORSUB.x264-GRP");
        assert!(r.hardcoded_subs);
    }

    #[test]
    fn audio_formats_propers_and_groups() {
        let r = parse("Movie.2023.2160p.WEB-DL.DDPA5.1.DV.HDR.H.265-GRP");
        assert_eq!((r.audio_codec, r.atmos), (Some("eac3"), true));
        let r = parse("Movie.2020.1080p.BluRay.x264.6CH-PSA");
        assert_eq!(
            (r.audio_channels.as_deref(), r.group.as_deref()),
            (Some("5.1"), Some("PSA"))
        );
        let r = parse("Movie.2020.1080p.BluRay.x264.DTS-HD.HRA.7.1-GRP");
        assert_eq!(r.audio_codec, Some("dts-hd hra"));
        assert_eq!(parse("Show.S01E01.REPACK2.1080p.WEB.H264-GRP").proper, 2);
        let cases = [
            ("Movie.2020.1080p.WEB-DL.DDP5.1.H.264", None),
            ("Movie.2020.1080p.BluRay.DTS-HD.MA.5.1.x264", None),
            ("Movie.2020.1080p.BluRay.DTS.x264-D-Z0N3", Some("D-Z0N3")),
            (
                "Movie.2020.1080p.WEB-DL.DDP5.1.H.264-FLUX-Obfuscated",
                Some("FLUX"),
            ),
            (
                "Movie.2020.1080p.WEB-DL.DDP5.1.H.264-FLUX-xpost",
                Some("FLUX"),
            ),
        ];
        for (name, group) in cases {
            assert_eq!(parse(name).group.as_deref(), group, "{name}");
        }
    }
}
