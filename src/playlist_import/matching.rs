//! The MusicBrainz recording a track is: by its ISRC first, then by title, artist and length.

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

/// The MusicBrainz recording a track is, or why none could be told.
pub async fn recording_for(
    mb: &musicbrainz::Client,
    track: &Track,
) -> Result<Result<String, String>, musicbrainz::Error> {
    if let Some(isrc) = &track.isrc {
        // One recording with the code is it; several are told apart by artist and length.
        let found = mb.isrc(isrc).await?;
        if let Some(id) = best(track, &found, false) {
            return Ok(Ok(id));
        }
    }
    let artist = track.artists.first().map(String::as_str);
    let found = mb
        .search_songs(
            track.title.split(" - ").next().unwrap_or(&track.title),
            artist,
        )
        .await?;
    Ok(best(track, &found, true).ok_or_else(|| {
        if track.artists.is_empty() {
            "the playlist doesn't say who it's by, and its title alone found nothing that fits"
                .to_owned()
        } else {
            "no recording on MusicBrainz has its title, artist and length".to_owned()
        }
    }))
}
