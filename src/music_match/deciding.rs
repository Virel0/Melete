//! Deciding an album's release: the one its tags name, else the one a search settles on, else
//! candidates when several release groups fit.

use super::*;

pub(super) async fn decide(
    mb: &musicbrainz::Client,
    album: &Album,
) -> Result<Outcome, musicbrainz::Error> {
    if let Some(id) = &album.mb_release_id {
        match mb.release(id).await {
            Ok(release) => {
                return Ok(Outcome::Matched {
                    release: Box::new(release),
                    method: "tags",
                });
            }
            // A stale or mistyped id: search as if there were none.
            Err(musicbrainz::Error::NotFound) => {}
            Err(err) => return Err(err),
        }
    }
    if album.title == "Unknown album" {
        return Ok(Outcome::Unmatched);
    }
    let artist = (!album.compilation && album.artist_name != "Unknown artist")
        .then_some(album.artist_name.as_str());
    let found = mb.search(&album.title, artist).await?;
    match choose(&found, &album.title, artist, album.tracks) {
        Choice::One(id) => Ok(Outcome::Matched {
            release: Box::new(mb.release(&id).await?),
            method: "search",
        }),
        Choice::Several(candidates) => Ok(Outcome::Ambiguous(candidates)),
        Choice::None => Ok(Outcome::Unmatched),
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum Choice {
    One(String),
    Several(serde_json::Value),
    None,
}

/// Which release a search settles on, if any.
pub(super) fn choose(found: &[Found], title: &str, artist: Option<&str>, tracks: i64) -> Choice {
    let good: Vec<&Found> = found
        .iter()
        .filter(|f| f.score >= 90 && fold(&f.title) == fold(title))
        .filter(|f| {
            artist.is_none_or(|a| {
                fold(&credit_line(&f.artist_credit)) == fold(a)
                    || f.artist_credit
                        .iter()
                        .any(|c| fold(&c.artist.name) == fold(a))
            })
        })
        .collect();
    if good.is_empty() {
        return Choice::None;
    }
    // Editions with the album's track count say most about which album it is.
    let same_count: Vec<&Found> = good
        .iter()
        .copied()
        .filter(|f| f.track_count == Some(tracks))
        .collect();
    let pool = if same_count.is_empty() {
        good
    } else {
        same_count
    };
    let mut groups: Vec<&str> = Vec::new();
    for f in &pool {
        if !groups.contains(&f.release_group.id.as_str()) {
            groups.push(&f.release_group.id);
        }
    }
    if let [_] = groups[..] {
        // The edition most like it: official, then the same track count, then the earliest.
        let best = pool.iter().min_by_key(|f| {
            (
                f.status.as_deref() != Some("Official"),
                f.track_count != Some(tracks),
                f.date
                    .clone()
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| "9999".into()),
            )
        });
        return Choice::One(best.map(|f| f.id.clone()).unwrap_or_default());
    }
    let candidates: Vec<serde_json::Value> = groups
        .iter()
        .take(5)
        .filter_map(|group| pool.iter().find(|f| f.release_group.id == *group))
        .map(|f| candidate(f))
        .collect();
    Choice::Several(json!(candidates))
}

/// A release group as a choice to match by hand, with an edition of it to match to.
pub(super) fn candidate(f: &Found) -> serde_json::Value {
    json!({
        "release_id": f.id,
        "release_group_id": f.release_group.id,
        "title": f.title,
        "artist": credit_line(&f.artist_credit),
        "year": f.date.as_deref().and_then(crate::library::tags::year_of),
        "primary_type": f.release_group.primary_type,
        "tracks": f.track_count,
    })
}
