//! The patterns a release name is read with: separators, tokens, languages, and the regexes for
//! episodes, seasons, groups and tags.

use super::*;

/// Separators between words in a release name.
pub(super) fn is_separator(c: char) -> bool {
    matches!(
        c,
        ' ' | '.' | '_' | '-' | '[' | ']' | '(' | ')' | '{' | '}' | ',' | '+' | '/' | '~'
    )
}

/// A feature regex: `pattern` standing alone between separators.
pub(super) fn token(pattern: &str) -> Regex {
    Regex::new(&format!(
        r"(?i)(?:^|[\s._\-\[\](){{}},+/~])(?:{pattern})(?:$|[\s._\-\[\](){{}},+/~])"
    ))
    // `pattern` is always built from the literals in this file, which the tests all use.
    .expect("valid pattern")
}

macro_rules! tokens {
    ($($name:ident = $pattern:expr;)*) => {
        $(pub(super) static $name: LazyLock<Regex> = LazyLock::new(|| token($pattern));)*
    };
}

pub(super) const CH: &str = r"(?:[\s.]?[1-9][\s.][01])?";

tokens! {
    // Strong: these end a title wherever they appear.
    R2160 = r"2160p|3840x2160";
    R1080 = r"1080[pi]|1920x1080";
    R720 = r"720p|1280x720";
    R576 = r"576[pi]";
    R480 = r"480[pi]|640x480";
    REMUX = r"remux|bdremux";
    BLURAY = r"blu-?ray|bdrip|brrip|bd25|bd50|bd66|bd100|uhd-?bluray";
    WEBRIP = r"web-?rip";
    WEBDL = r"web-?dl";
    HDTV = r"hdtv|pdtv|dsr|tvrip|sdtv";
    DVDRIP = r"dvd-?rip|dvd5|dvd9|dvdr";
    CAMRIP = r"hdcam|camrip|hqcam|hd-?cam";
    TELESYNCS = r"telesync|hd-?ts";
    TELECINES = r"telecine|hd-?tc";
    SCREENERS = r"dvdscr|screener";
    H264 = r"x264|h[\s.]?264|avc";
    H265 = r"x265|h[\s.]?265|hevc";
    // Weak: only once the title is over.
    R4K = r"4k|uhd";
    BD = r"bd";
    WEB = r"web|amzn|nf|dsnp|hmax|max|atvp|pcok|hulu|itunes|cr|stan|it";
    DVD = r"dvd|ntsc|pal";
    CAM = r"cam";
    TELESYNC = r"ts";
    TELECINE = r"tc";
    SCREENER = r"scr";
    HDRIP = r"hdrip";
    COMPLETE = r"complete";
    DISC_SIZE = r"bd25|bd50|bd66|bd100";
    DVD_DISC = r"dvd-?r|dvd5|dvd9";
    DISC_CODEC = r"avc|vc-?1|mpeg-?2";
    ISO = r"iso";
    DOLBY_VISION = r"dv|dovi|dolby[\s._-]?vision";
    HDR10_PLUS = r"hdr10\+|hdr10plus|hdr10p";
    HDR10 = r"hdr10|hdr";
    HLG = r"hlg";
    AV1 = r"av1";
    XVID = r"xvid|divx";
    MPEG2 = r"mpeg-?2";
    TRUEHD = &format!(r"truehd{CH}");
    ATMOS = &format!(r"atmos{CH}|ddpa{CH}");
    DTS_X = &format!(r"dts[\s._-]?x{CH}");
    DTS_HRA = &format!(r"dts[\s._-]?hd[\s._-]?hra{CH}|dts[\s._-]?hra{CH}");
    DTS_HD = &format!(r"dts[\s._-]?hd(?:[\s._-]?ma)?{CH}|dts[\s._-]?ma{CH}");
    DTS = &format!(r"dts{CH}");
    EAC3 = &format!(r"ddpa?{CH}|dd\+{CH}|e-?ac-?3{CH}");
    AC3 = &format!(r"dd{CH}|ac-?3{CH}|dolby[\s._-]?digital");
    AAC = &format!(r"aac{CH}");
    FLAC = &format!(r"flac{CH}");
    OPUS = &format!(r"opus{CH}");
    PCM = &format!(r"l?pcm{CH}");
    MP3 = r"mp3";
    DUAL_AUDIO = r"dual[\s._-]?audio|dual";
    DUAL_LANGUAGE = r"(?:german|french|italian|spanish)[\s._-]dl|dl[\s._-]audio";
    MULTI_SUBS = r"multi[\s._-]?subs?|multisubs?|multiple[\s._-]?subtitles?";
    MULTI = r"multi|multi[\s._-]?audio";
    VOSTFR = r"vostfr|subfrench|subfr";
    HARDSUB = r"hardsubs?|hc|hardcoded|hardcoded[\s._-]?subs?|korsubs?";
    DUB = r"english[\s._-]?dub(?:bed)?|eng[\s._-]?dub|dub(?:bed)?";
}

/// Languages by name, as (pattern, ISO 639-1): every language Radarr and Sonarr know, by full
/// name and the tags scene names use. Regional variants come before their language, and a
/// match is taken out before the next is looked for, so "POR-BR" isn't Portuguese as well.
pub(super) const LANGUAGE_NAMES: &[(&str, &str)] = &[
    (r"english|eng", "en"),
    (r"japanese|jpn|jap|jp", "ja"),
    (r"french|truefrench|vff|vfq|vf2|vfi", "fr"),
    (r"german|ger|deutsch", "de"),
    (r"brazilian|pt[\s._-]?br|por[\s._-]?br|dublado", "pt-br"),
    (r"latino|spa[\s._-]la|es[\s._-]la|latam", "es-419"),
    (r"spanish|spa|esp|castellano", "es"),
    (r"italian|ita", "it"),
    (r"korean|kor", "ko"),
    (r"chinese|chi|chs|cht|mandarin|cantonese", "zh"),
    (r"russian|rus", "ru"),
    (r"portuguese|por", "pt"),
    (r"hindi|hin", "hi"),
    (r"flemish", "nl-be"),
    (r"dutch|nld", "nl"),
    (r"polish|pldub|pol", "pl"),
    (r"swedish|swe", "sv"),
    (r"danish", "da"),
    (r"norwegian|nor", "no"),
    (r"finnish", "fi"),
    (r"icelandic", "is"),
    (r"turkish", "tr"),
    (r"greek", "el"),
    (r"hungarian|hun", "hu"),
    (r"hebrew|heb", "he"),
    (r"lithuanian", "lt"),
    (r"latvian", "lv"),
    (r"estonian", "et"),
    (r"czech|cze", "cs"),
    (r"slovak", "sk"),
    (r"slovenian", "sl"),
    (r"croatian|hrv", "hr"),
    (r"serbian|srb", "sr"),
    (r"bosnian", "bs"),
    (r"bulgarian|bgaudio", "bg"),
    (r"romanian|rum", "ro"),
    (r"ukrainian|ukr", "uk"),
    (r"arabic|ara", "ar"),
    (r"persian|farsi", "fa"),
    (r"thai", "th"),
    (r"vietnamese|vie", "vi"),
    (r"indonesian", "id"),
    (r"bengali", "bn"),
    (r"tamil", "ta"),
    (r"telugu", "te"),
    (r"kannada", "kn"),
    (r"malayalam", "ml"),
    (r"marathi", "mr"),
    (r"macedonian", "mk"),
    (r"albanian", "sq"),
    (r"afrikaans", "af"),
    (r"georgian", "ka"),
    (r"mongolian", "mn"),
    (r"romansh", "rm"),
    (r"tagalog|filipino", "tl"),
    (r"urdu", "ur"),
    (r"catalan", "ca"),
];

/// Each language as audio, and as subtitles ("Eng Subs", "Sub ITA", "SUBBED.ENG").
pub(super) static LANGUAGES: LazyLock<Vec<(Regex, Regex, &'static str)>> = LazyLock::new(|| {
    LANGUAGE_NAMES
        .iter()
        .map(|(names, code)| {
            let subtitle = format!(
                r"(?:{names})[\s._-]?(?:subs?|subbed|subtitles?)|(?:subs?|subbed|subtitles?)[\s._-]?(?:{names})|(?:{names})subs?"
            );
            (token(names), token(&subtitle), *code)
        })
        .collect()
});

// Every pattern from here on is a literal, and the tests read names with each of them, so
// none of the `expect`s below can fire.
pub(super) static EXTENSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\.(mkv|mp4|m4v|avi|ts|m2ts|wmv|webm|nzb|torrent)$").expect("a valid pattern")
});
/// "[ www.Torrenting.com ] - " and "www.UIndex.org - " in front of a name.
pub(super) static SITE_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*\[?\s*www\.[a-z0-9.\-]+\.[a-z]{2,}\s*\]?\s*-?\s*")
        .expect("a valid pattern")
});
pub(super) static PREFIX_GROUP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\[([^\]]{1,40})\]\s*").expect("a valid pattern"));
pub(super) static SEASON_EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])S(\d{1,4})[\s._-]?E(\d{1,4})").expect("a valid pattern")
});
pub(super) static SEASON_EPISODE_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])Season[\s._-]?(\d{1,4})[\s._-]?Episode[\s._-]?(\d{1,4})")
        .expect("a valid pattern")
});
pub(super) static CROSS_EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(\d{1,2})x(\d{2,3})(?:[^a-z0-9]|$)").expect("a valid pattern")
});
/// "E05" or "Ep05" (two digits or more, so "E3" isn't one), or "Episode 5".
pub(super) static BARE_EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:Ep?(\d{2,4})|Episode[\s._-]?(\d{1,4}))(?:[^a-z0-9]|$)")
        .expect("a valid pattern")
});
pub(super) static DAILY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^0-9])((?:19|20)\d\d)[.\-\s](0[1-9]|1[0-2])[.\-\s](0[1-9]|[12]\d|3[01])(?:[^0-9]|$)").expect("a valid pattern")
});
/// " - 05", " - 05v2", " - 1085", and " - 05.5", a recap between episodes.
pub(super) static ABSOLUTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s-\s(?<number>\d{1,4})(?<half>\.5)?(?:v(?<version>\d))?(?:\s|\[|\(|$)")
        .expect("a valid pattern")
});
/// "(01-12)", "[01~12]", " - 01 ~ 12", " - 01-12". A bare "8 - 12" is not a batch.
pub(super) static BATCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:[\[(](\d{1,4})\s*[-~]\s*(\d{1,4})[\])]|\s-\s(\d{1,4})\s*[-~]\s*(\d{1,4})(?:\s|\[|\(|$))").expect("a valid pattern")
});
/// A season at the end of an anime title: "Season 2", "2nd Season", "S2".
pub(super) static TITLE_SEASON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s+(?:Season\s*(\d{1,2})|(\d{1,2})(?:st|nd|rd|th)\s+Season|S(\d{1,2}))$")
        .expect("a valid pattern")
});
pub(super) static COMPLETE_SERIES: LazyLock<Regex> =
    LazyLock::new(|| token(r"(?:the[\s._-]+)?complete[\s._-]+(?:series|collection)"));
/// "S01-S05", "Season 1-3", "Seasons.1-3".
pub(super) static SEASON_RANGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:S|Seasons?[\s._-]?)(\d{1,3})[\s._-]*-[\s._-]*(?:S|Seasons?[\s._-]?)?(\d{1,3})(?:[^a-z0-9]|$)").expect("a valid pattern")
});
/// "S01.S02.S03".
pub(super) static SEASON_LIST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(S\d{1,3}(?:[\s._-]+S\d{1,3})+)(?:[^a-z0-9]|$)")
        .expect("a valid pattern")
});
pub(super) static SEASON_ONE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:S|Season[\s._-]?)(\d{1,3})(?:[^a-z0-9]|$)")
        .expect("a valid pattern")
});
pub(super) static PROPER_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:proper|repack|rerip)(\d)?$").expect("a valid pattern"));
pub(super) static CHANNELS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^0-9])([1-9])[\s.]([01])(?:$|[^0-9])").expect("a valid pattern")
});
pub(super) static CHANNEL_COUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^0-9a-z])([268])ch(?:$|[^a-z])").expect("a valid pattern")
});
pub(super) static VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\d|[\s._\-\[(])v([2-9])(?:$|[^0-9a-z])").expect("a valid pattern")
});

/// Words that end a name after a dash but aren't a group.
pub(super) const NOT_GROUPS: &[&str] = &[
    "dl", "rip", "hd", "ma", "x264", "x265", "h264", "h265", "hevc", "avc", "web", "audio", "subs",
    "sub", "dub", "remux", "hdr", "dv", "atmos", "uhd", "bluray", "1080p", "2160p", "720p",
];
/// Usenet posters' suffixes after the real group.
pub(super) const POSTER_SUFFIXES: &[&str] = &[
    "obfuscated",
    "xpost",
    "scrambled",
    "postbot",
    "rakuv",
    "sample",
];
/// Groups whose name has a dash in it.
pub(super) const DASHED_GROUPS: &[&str] = &["D-Z0N3"];
