//! Which of YouTube's results a wanted song is, if any: its title and artist both there, its
//! length as long as the song's, and nothing that makes it another version. Taking none is
//! always better than taking the wrong one.

use serde::Deserialize;

/// Seconds a video's length may differ from the song's: an old record's transfer can start or
/// end a few seconds off.
pub const LENGTH_SLACK: f64 = 10.0;
/// The longest a video is taken for a song whose length isn't known, in seconds.
pub const LONGEST_UNKNOWN: f64 = 15.0 * 60.0;

/// Words that make a video another version of the song, unless the song's own title has them.
const OTHER_VERSIONS: &[&str] = &[
    "cover",
    "remix",
    "karaoke",
    "instrumental",
    "nightcore",
    "sped up",
    "speed up",
    "slowed",
    "reverb",
    "8d",
    "bass boosted",
    "live",
    "acoustic",
    "reaction",
    "tutorial",
    "lesson",
    "1 hour",
    "10 hours",
    "loop",
    "mashup",
    "parody",
    "teaser",
    "snippet",
];

/// One of YouTube's results, as the downloader gives it.
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
pub struct Found {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub channel: String,
    /// Its length in seconds, when YouTube says.
    pub duration: Option<f64>,
}

/// The wanted song, as choosing reads it.
pub struct Song<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub length_seconds: Option<i64>,
}

/// Words that join two names, read as one: "Simon & Garfunkel" is "Simon y Garfunkel".
const AND: &[&str] = &["and", "y", "e", "et", "und"];
/// Articles a title may start with or not ("El Corrido…" and "Corrido…").
const ARTICLES: &[&str] = &[
    "the", "a", "an", "el", "la", "los", "las", "le", "les", "il", "lo", "die", "der", "das",
];

/// A letter without its accent, for the Latin letters titles use: "Río" is "Rio".
fn unaccented(c: char) -> char {
    const FROM: &str = "àáâãäåāăąçćčďèéêëēėęěìíîïīįñńňòóôõöøōőùúûüūůűųýÿžźżšśłß";
    const TO: &str = "aaaaaaaaacccdeeeeeeeeiiiiiinnnooooooooouuuuuuuuyyzzzsslss";
    FROM.chars()
        .position(|f| f == c)
        .and_then(|at| TO.chars().nth(at))
        .unwrap_or(c)
}

/// A Cyrillic letter as Latin letters, as names are commonly written in them ("Волна" is
/// "Volna"); any other letter as it is.
fn latin(c: char) -> String {
    const CYRILLIC: [(char, &str); 37] = [
        ('а', "a"),
        ('б', "b"),
        ('в', "v"),
        ('г', "g"),
        ('д', "d"),
        ('е', "e"),
        ('ё', "e"),
        ('ж', "zh"),
        ('з', "z"),
        ('и', "i"),
        ('й', "y"),
        ('к', "k"),
        ('л', "l"),
        ('м', "m"),
        ('н', "n"),
        ('о', "o"),
        ('п', "p"),
        ('р', "r"),
        ('с', "s"),
        ('т', "t"),
        ('у', "u"),
        ('ф', "f"),
        ('х', "kh"),
        ('ц', "ts"),
        ('ч', "ch"),
        ('ш', "sh"),
        ('щ', "shch"),
        ('ъ', ""),
        ('ы', "y"),
        ('ь', ""),
        ('э', "e"),
        ('ю', "yu"),
        ('я', "ya"),
        ('і', "i"),
        ('ї', "yi"),
        ('є', "ye"),
        ('ґ', "g"),
    ];
    CYRILLIC
        .iter()
        .find(|(from, _)| *from == c)
        .map_or_else(|| c.to_string(), |(_, to)| (*to).to_owned())
}

/// Letters and digits only, lowercase, unaccented, Cyrillic as Latin letters, single-spaced,
/// with "&", "y", "et" and the like read as "and".
pub fn plain(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(unaccented)
        .map(latin)
        .collect::<String>()
        .replace('&', " and ")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| if AND.contains(&w) { "and" } else { w })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `words` are in `text` as whole words, in order (both plain).
fn has(text: &str, words: &str) -> bool {
    !words.is_empty() && format!(" {text} ").contains(&format!(" {words} "))
}

/// A channel's own name, without what YouTube adds to an artist's: " - Topic", "VEVO",
/// "Official".
fn channel_name(channel: &str) -> String {
    let plain = plain(channel);
    let plain = plain.strip_suffix(" topic").unwrap_or(&plain);
    let plain = plain.strip_suffix("vevo").unwrap_or(plain).trim();
    plain.strip_suffix(" official").unwrap_or(plain).to_owned()
}

/// Whether the video's length fits the song's: within [`LENGTH_SLACK`] of it, or, when the
/// song's isn't known, no longer than [`LONGEST_UNKNOWN`].
pub fn length_fits(song: Option<i64>, video: Option<f64>) -> bool {
    match (song, video) {
        (Some(song), Some(video)) => (video - song as f64).abs() <= LENGTH_SLACK,
        (None, Some(video)) => video > 0.0 && video <= LONGEST_UNKNOWN,
        (_, None) => false,
    }
}

/// The song's title as a video's must have it: without what follows " - " (" - Remastered"),
/// a featured artist, bracketed parts ("(The Ballad of…)", "[2011 Mix]") or a leading article.
fn core_title(title: &str) -> String {
    let mut core = title.split(" - ").next().unwrap_or(title).to_owned();
    for open in ["feat.", "ft."] {
        if let Some(at) = core.to_lowercase().find(open) {
            core.truncate(at);
        }
    }
    let mut kept = String::with_capacity(core.len());
    let mut depth = 0usize;
    for c in core.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => kept.push(c),
            _ => {}
        }
    }
    let plain = plain(&kept);
    match plain.split_once(' ') {
        Some((first, rest)) if ARTICLES.contains(&first) => rest.to_owned(),
        _ => plain,
    }
}

/// A name without a leading article ("the red army choir" is "red army choir").
fn without_article(name: &str) -> &str {
    match name.split_once(' ') {
        Some((first, rest)) if ARTICLES.contains(&first) => rest,
        _ => name,
    }
}

/// Whether the artist is named in `text`: their whole name, with or without its leading article
/// ("Harbour Choir" for "The Harbour Choir"), or the first of several credited ("Dueto Sol
/// and Luna" for "Dueto Sol & Luna with Mariachi") when it's more than a short word.
fn names_artist(text: &str, artist: &str) -> bool {
    if has(text, artist) || has(text, without_article(artist)) {
        return true;
    }
    let first = [" and ", " with ", " con ", " feat ", " ft ", " x "]
        .iter()
        .filter_map(|joint| artist.split_once(joint).map(|(first, _)| first))
        .min_by_key(|first| first.len());
    first.is_some_and(|first| first.len() >= 4 && has(text, first))
}

/// Whether the video's title makes it another version (a cover, a remix, sped up) that the
/// song's own title doesn't ask for.
fn other_version(title: &str, asked: &str) -> bool {
    OTHER_VERSIONS
        .iter()
        .any(|w| has(title, w) && !has(asked, w))
}

/// The artist's own "- Topic" channel (YouTube's copy of the release) first, then their
/// channel, then anyone's; then the nearest in length.
fn rank(song: &Song<'_>, found: &Found, theirs: bool) -> (u8, i64) {
    let channel = if theirs && plain(&found.channel).ends_with(" topic") {
        3
    } else if theirs {
        2
    } else {
        1
    };
    let gap = match (song.length_seconds, found.duration) {
        (Some(s), Some(d)) => ((d - s as f64).abs() * 10.0) as i64,
        _ => 0,
    };
    (channel, -gap)
}

/// What keeps a result from being the song, in words: its title, its artist (named in the
/// title, or theirs is the channel), another version, its length. None when it is the song.
pub fn misses(song: &Song<'_>, found: &Found) -> Vec<String> {
    let title = plain(&found.title);
    let artist = plain(song.artist);
    let wanted = core_title(song.title);
    let mut misses = Vec::new();
    if wanted.is_empty() || !has(&title, &wanted) {
        misses.push("its title isn't the song's".to_owned());
    }
    if artist.is_empty() || !(theirs(found, &artist) || names_artist(&title, &artist)) {
        misses.push("it doesn't name the artist".to_owned());
    }
    if other_version(&title, &plain(song.title)) {
        misses.push("it's another version".to_owned());
    }
    if !length_fits(song.length_seconds, found.duration) {
        misses.push(match (found.duration, song.length_seconds) {
            (Some(d), Some(s)) => format!("it runs {}, the song {}", minutes(d), minutes(s as f64)),
            (None, _) => "YouTube gives no length".to_owned(),
            (Some(d), None) => format!("it runs {}, too long for a song", minutes(d)),
        });
    }
    misses
}

/// Seconds as m:ss.
fn minutes(seconds: f64) -> String {
    let s = seconds.round() as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Whether the channel is the artist's own.
fn theirs(found: &Found, artist: &str) -> bool {
    let channel = channel_name(&found.channel);
    channel == artist
        || without_article(&channel) == without_article(artist)
        || (!channel.is_empty() && names_artist(&channel, artist))
}

/// How well a result fits, best highest; None when it isn't the song ([`misses`]).
fn fit(song: &Song<'_>, found: &Found) -> Option<(u8, i64)> {
    misses(song, found)
        .is_empty()
        .then(|| rank(song, found, theirs(found, &plain(song.artist))))
}

/// The result that came closest to being the song, and what it missed; None when there were
/// no results.
pub fn closest<'a>(song: &Song<'_>, found: &'a [Found]) -> Option<(&'a Found, Vec<String>)> {
    found
        .iter()
        .map(|f| (f, misses(song, f)))
        .min_by_key(|(_, misses)| misses.len())
}

/// Every result that is the song, the best fitting first (YouTube's order among equals).
pub fn ranked<'a>(song: &Song<'_>, found: &'a [Found]) -> Vec<&'a Found> {
    let mut fitting: Vec<((u8, i64), usize, &Found)> = found
        .iter()
        .enumerate()
        .filter_map(|(at, f)| fit(song, f).map(|score| (score, at, f)))
        .collect();
    fitting.sort_by(|(a, at_a, _), (b, at_b, _)| b.cmp(a).then(at_a.cmp(at_b)));
    fitting.into_iter().map(|(_, _, f)| f).collect()
}
