//! The MusicBrainz recording a track is: by its ISRC first, then by title, artist and length.
//! A track with an artist may also be a recording whose title is close to its own, by the same
//! artist, when none has its title; one without an artist needs its title.

use super::*;

/// Letters and digits only, lowercase, single-spaced: "AC/DC" and "ac dc" compare equal.
pub(super) fn plain(text: &str) -> String {
    text.to_lowercase()
        .replace('&', " and ")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A title without what streaming services add to it: " - Remastered 2011", " - Live",
/// "(feat. Someone)".
pub(super) fn core_title(title: &str) -> String {
    let mut core = title.split(" - ").next().unwrap_or(title).to_owned();
    for open in ["(feat.", "(ft.", "(with ", "[feat."] {
        if let Some(at) = core.to_lowercase().find(open) {
            core.truncate(at);
        }
    }
    plain(&core)
}

/// A title as it's compared loosely: its core, without bracketed parts or a leading "the",
/// "a" or "an".
fn loose_title(title: &str) -> String {
    let mut kept = String::with_capacity(title.len());
    let mut depth = 0usize;
    for c in title.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => kept.push(c),
            _ => {}
        }
    }
    let core = core_title(&kept);
    ["the ", "a ", "an "]
        .iter()
        .find_map(|article| core.strip_prefix(article))
        .map_or_else(|| core.clone(), str::to_owned)
}

/// Whether a recording's title is close to the track's: the same loosely, or alike by the
/// share of the library matcher's title words.
fn close_title(track: &str, recording: &str) -> bool {
    let (a, b) = (loose_title(track), loose_title(recording));
    (!a.is_empty() && a == b) || crate::aoide::matcher::titles_share_words(track, recording)
}

pub(super) fn artist_fits(track: &Track, recording: &RecordingDetail) -> bool {
    if track.artists.is_empty() {
        return true;
    }
    let credited: Vec<String> = recording
        .artist_credit
        .iter()
        .flat_map(|c| [plain(&c.name), plain(&c.artist.name)])
        .collect();
    // "Tyler, The Creator" arrives split at its comma; the whole line still matches.
    let line = plain(&musicbrainz::credit_line(&recording.artist_credit));
    track.artists.iter().any(|a| credited.contains(&plain(a)))
        || plain(&track.artists.join(", ")) == line
}

/// How far its length is from the track's; 0 when either isn't known, None when too far.
pub(super) fn length_gap(track: &Track, recording: &RecordingDetail) -> Option<i64> {
    match (track.length_ms, recording.length) {
        (Some(a), Some(b)) => Some((a - b).abs()).filter(|gap| *gap <= LENGTH_SLACK_MS),
        _ => Some(0),
    }
}

/// The best fitting of `found` by artist and length (and title, for a search): the nearest in
/// length.
pub(super) fn best(track: &Track, found: &[RecordingDetail], titled: bool) -> Option<String> {
    found
        .iter()
        .filter(|r| artist_fits(track, r))
        .filter(|r| !titled || core_title(&r.title) == core_title(&track.title))
        .filter_map(|r| length_gap(track, r).map(|gap| (gap, r)))
        .min_by_key(|(gap, _)| *gap)
        .map(|(_, r)| r.id.clone())
}

/// Of `found`, the recording by the track's artist with a title close to its own, within the
/// length slack, the nearest in length; never for a track without an artist.
pub(super) fn near(track: &Track, found: &[RecordingDetail]) -> Option<String> {
    if track.artists.is_empty() {
        return None;
    }
    found
        .iter()
        .filter(|r| artist_fits(track, r))
        .filter(|r| close_title(&track.title, &r.title))
        .filter_map(|r| length_gap(track, r).map(|gap| (gap, r)))
        .min_by_key(|(gap, _)| *gap)
        .map(|(_, r)| r.id.clone())
}

/// The recording of the same title, else of a close one (see [`near`]).
pub(super) fn fitting(track: &Track, found: &[RecordingDetail]) -> Option<String> {
    best(track, found, true).or_else(|| near(track, found))
}

/// The MusicBrainz recording a track is, or why none could be told.
pub async fn recording_for(
    mb: &musicbrainz::Client,
    track: &Track,
) -> Result<Result<String, String>, musicbrainz::Error> {
    if let Some(isrc) = &track.isrc {
        // One recording with the code is it; several are told apart by artist and length. A
        // code MusicBrainz won't look up is left for the title to find.
        let found = match mb.isrc(isrc).await {
            Ok(found) => found,
            Err(musicbrainz::Error::Refused(_)) => Vec::new(),
            Err(err) => return Err(err),
        };
        if let Some(id) = best(track, &found, false) {
            return Ok(Ok(id));
        }
    }
    let artist = track.artists.first().map(String::as_str);
    let title = track.title.split(" - ").next().unwrap_or(&track.title);
    let found = mb.search_songs(title, artist).await?;
    let mut fits = fitting(track, &found);
    // The title as a phrase found nothing that fits: its words once more, by the same artist.
    // A track without an artist isn't looked for more loosely.
    if fits.is_none()
        && let Some(artist) = artist
    {
        fits = fitting(track, &mb.search_song_words(title, artist).await?);
    }
    Ok(fits.ok_or_else(|| {
        if track.artists.is_empty() {
            "the playlist doesn't say who it's by, and its title alone found nothing that fits"
                .to_owned()
        } else {
            "no recording on MusicBrainz has its title, artist and length".to_owned()
        }
    }))
}
