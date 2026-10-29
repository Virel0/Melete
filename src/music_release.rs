//! Reading a music release name: what it's like (format, bitrate, bit depth,
//! source, year). Quality names follow Lidarr's ("FLAC", "FLAC 24bit", "MP3-320",
//! "MP3-VBR-V0"…), so profiles imported from it line up. A name that says nothing about its
//! format is "Unknown", which no profile should allow: guessing a format is how a lossy album
//! gets taken for a lossless one.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

/// Every quality, as Lidarr lists them. A profile can rank only these.
pub const QUALITIES: &[&str] = &[
    "Unknown",
    "MP3-008",
    "MP3-016",
    "MP3-024",
    "MP3-032",
    "MP3-040",
    "MP3-048",
    "MP3-056",
    "MP3-064",
    "MP3-080",
    "MP3-096",
    "MP3-112",
    "MP3-128",
    "MP3-160",
    "MP3-192",
    "MP3-224",
    "MP3-256",
    "MP3-320",
    "MP3-VBR-V0",
    "MP3-VBR-V2",
    "AAC-192",
    "AAC-256",
    "AAC-320",
    "AAC-VBR",
    "WMA",
    "OGG Vorbis Q5",
    "OGG Vorbis Q6",
    "OGG Vorbis Q7",
    "OGG Vorbis Q8",
    "OGG Vorbis Q9",
    "OGG Vorbis Q10",
    "WAV",
    "WavPack",
    "APE",
    "FLAC",
    "ALAC",
    "FLAC 24bit",
    "ALAC 24bit",
];

/// Where a release was ripped from, as its name says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Web,
    Cd,
    Vinyl,
}

/// What a release's name says about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MusicRelease {
    /// One of [`QUALITIES`].
    pub quality: &'static str,
    /// The album's year, a bracketed one first; none when the name gives none.
    pub year: Option<i64>,
    /// Where it was ripped from; none when the name doesn't say.
    pub source: Option<Source>,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("only called with the literal patterns below, each valid as written")
}

static FLAC: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bflac\b|webflac"));
static ALAC: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\balac\b"));
// Only as a tag: "APE" in capitals or in brackets, not an ape in a title.
static APE: LazyLock<Regex> = LazyLock::new(|| re(r"\bAPE\b|(?i)[\[(]ape[\])]"));
static WAVPACK: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bwavpack\b|\bWV\b"));
static WAV: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bwav\b"));
static HI_RES: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)\b24[ -]?bits?\b|\b24[-/ ]?(44|48|88|96|176|192)([.,]\d)?\b|\bhi[- ]?res\b")
});
static MP3: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bmp3\b"));
static AAC: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\b(aac|m4a)\b"));
static OGG: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\b(ogg|vorbis)\b"));
static WMA: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bwma\b"));
static V0: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bv0\b"));
static V2: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bv2\b"));
static BITRATE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\b(320|256|224|192|160|128|112|96|80|64)\s?(kbps|kbit|k)?\b"));
static WEB: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bweb\b|webflac|\bweb-?dl\b"));
static CD: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bcd\b|\bcdrip\b|\bcdda\b"));
static VINYL: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bvinyl\b|\bvinylrip\b"));
static YEAR: LazyLock<Regex> =
    LazyLock::new(|| re(r"[\[(]((?:19|20)\d\d)[\])]|\b((?:19|20)\d\d)\b"));

/// Reads what a release name says about itself.
pub fn parse(name: &str) -> MusicRelease {
    // Scene names separate words with dots and underscores.
    let name = name.replace(['_', '.'], " ");
    let hi_res = HI_RES.is_match(&name);
    let bitrate = || {
        BITRATE
            .captures(&name)
            .and_then(|c| c[1].parse::<u32>().ok())
    };
    let quality = if FLAC.is_match(&name) {
        if hi_res { "FLAC 24bit" } else { "FLAC" }
    } else if ALAC.is_match(&name) {
        if hi_res { "ALAC 24bit" } else { "ALAC" }
    } else if APE.is_match(&name) {
        "APE"
    } else if WAVPACK.is_match(&name) {
        "WavPack"
    } else if WAV.is_match(&name) {
        "WAV"
    } else if MP3.is_match(&name) || V0.is_match(&name) || V2.is_match(&name) {
        if V0.is_match(&name) {
            "MP3-VBR-V0"
        } else if V2.is_match(&name) {
            "MP3-VBR-V2"
        } else {
            match bitrate() {
                Some(320) => "MP3-320",
                Some(256) => "MP3-256",
                Some(224) => "MP3-224",
                Some(192) => "MP3-192",
                Some(160) => "MP3-160",
                Some(128) => "MP3-128",
                Some(112) => "MP3-112",
                Some(96) => "MP3-096",
                Some(80) => "MP3-080",
                Some(64) => "MP3-064",
                // An MP3 that doesn't say its bitrate could be anything.
                _ => "Unknown",
            }
        }
    } else if AAC.is_match(&name) {
        match bitrate() {
            Some(320) => "AAC-320",
            Some(256) => "AAC-256",
            Some(192) => "AAC-192",
            _ => "AAC-VBR",
        }
    } else if OGG.is_match(&name) {
        // Vorbis names rarely give a level; the lowest is the safe reading.
        "OGG Vorbis Q5"
    } else if WMA.is_match(&name) {
        "WMA"
    } else {
        "Unknown"
    };
    let source = if VINYL.is_match(&name) {
        Some(Source::Vinyl)
    } else if WEB.is_match(&name) {
        Some(Source::Web)
    } else if CD.is_match(&name) {
        Some(Source::Cd)
    } else {
        None
    };
    // A bracketed year first: "(2016)" is the album's, a bare 1999 might be its title.
    let year = YEAR
        .captures_iter(&name)
        .find_map(|c| c.get(1))
        .or_else(|| YEAR.captures(&name).and_then(|c| c.get(2)))
        .and_then(|m| m.as_str().parse().ok());
    MusicRelease {
        quality,
        year,
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(name: &str) -> (&'static str, Option<i64>, Option<Source>) {
        let release = parse(name);
        (release.quality, release.year, release.source)
    }

    #[test]
    fn reads_formats_bit_depths_and_bitrates() {
        use Source::*;
        for (name, expected) in [
            (
                "Radiohead - OK Computer (1997) [FLAC]",
                ("FLAC", Some(1997), None),
            ),
            (
                "Radiohead - OK Computer (1997) [FLAC 24-96]",
                ("FLAC 24bit", Some(1997), None),
            ),
            (
                "Radiohead - OK Computer OKNOTOK (2017) 24bit Hi-Res FLAC",
                ("FLAC 24bit", Some(2017), None),
            ),
            (
                "Radiohead-OK_Computer-WEBFLAC-2016-GROUP",
                ("FLAC", Some(2016), Some(Web)),
            ),
            (
                "Radiohead-OK_Computer-(Remastered)-CD-FLAC-1997-GROUP",
                ("FLAC", Some(1997), Some(Cd)),
            ),
            (
                "Radiohead - OK Computer (1997) [Vinyl 24-192 FLAC]",
                ("FLAC 24bit", Some(1997), Some(Vinyl)),
            ),
            (
                "Radiohead - OK Computer (1997) ALAC",
                ("ALAC", Some(1997), None),
            ),
            (
                "Radiohead - OK Computer (1997) [APE]",
                ("APE", Some(1997), None),
            ),
            (
                "Radiohead - OK Computer (1997) Mp3 320kbps",
                ("MP3-320", Some(1997), None),
            ),
            (
                "Radiohead - OK Computer (1997) [MP3 V0]",
                ("MP3-VBR-V0", Some(1997), None),
            ),
            (
                "Radiohead-OK_Computer-WEB-2016-GROUP V0",
                ("MP3-VBR-V0", Some(2016), Some(Web)),
            ),
            (
                "Radiohead - OK Computer (1997) [MP3 192]",
                ("MP3-192", Some(1997), None),
            ),
            (
                "Radiohead - OK Computer (1997) MP3",
                ("Unknown", Some(1997), None),
            ),
            (
                "Radiohead - OK Computer (1997) [AAC 256]",
                ("AAC-256", Some(1997), None),
            ),
            (
                "Radiohead - OK Computer (1997) m4a",
                ("AAC-VBR", Some(1997), None),
            ),
            (
                "Radiohead - OK Computer (1997) [OGG]",
                ("OGG Vorbis Q5", Some(1997), None),
            ),
            (
                "Radiohead - OK Computer (1997)",
                ("Unknown", Some(1997), None),
            ),
        ] {
            assert_eq!(read(name), expected, "{name}");
        }
    }

    #[test]
    fn a_word_in_a_title_isnt_a_tag() {
        // "Ape" in a title isn't APE; a bare 1999 that is the title gives way to the bracketed year.
        assert_eq!(
            read("Planet of the Ape Kings - Ape (2001) [MP3 320]").0,
            "MP3-320"
        );
        assert_eq!(read("Prince - 1999 (1982) [FLAC]").1, Some(1982));
        assert_eq!(read("Prince - 1999 [FLAC]").1, Some(1999));
    }

    #[test]
    fn every_quality_it_reads_is_one_lidarr_knows() {
        for name in [
            "x FLAC 24bit",
            "x ALAC 24bit",
            "x WavPack",
            "x WAV",
            "x MP3 160",
            "x MP3 V2",
            "x AAC 320",
            "x AAC 192",
            "x WMA",
            "x MP3 64",
            "x MP3 96",
            "x MP3 112",
            "x MP3 224",
        ] {
            assert!(
                QUALITIES.contains(&parse(name).quality),
                "{name}: {}",
                parse(name).quality
            );
        }
    }
}
