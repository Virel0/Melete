//! Grouping tracks into albums and artists: which album a track belongs to, and what its tracks
//! agree on.

use super::*;

/// What grouping needs from a track.
#[derive(Debug, Default, Clone, sqlx::FromRow)]
pub(super) struct TrackFacts {
    pub(super) id: i64,
    pub(super) rel_path: String,
    pub(super) tagged: bool,
    pub(super) artist: Option<String>,
    pub(super) album_artist: Option<String>,
    pub(super) album: Option<String>,
    pub(super) year: Option<i64>,
    pub(super) compilation: bool,
    pub(super) mb_release_id: Option<String>,
    pub(super) mb_release_group_id: Option<String>,
    pub(super) mb_artist_id: Option<String>,
    pub(super) mb_album_artist_id: Option<String>,
}

/// An album as worked out from its tracks; `artist` is `None` for several artists.
#[derive(Debug, PartialEq)]
pub(super) struct AlbumFacts {
    pub(super) title: String,
    pub(super) artist: Option<String>,
    pub(super) year: Option<i64>,
    pub(super) folder: String,
    pub(super) mb_release_id: Option<String>,
    pub(super) mb_release_group_id: Option<String>,
    pub(super) mb_artist_id: Option<String>,
}

/// The folder an album's files are in: the file's folder, or a disc folder's parent
/// ("Album/CD2/01.flac" belongs with "Album/CD1"). Empty for a file at the library's root.
pub(super) fn album_folder(rel_path: &str) -> &str {
    let folder = rel_path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let (parent, last) = folder.rsplit_once('/').unwrap_or(("", folder));
    if is_disc_folder(last) { parent } else { folder }
}

/// "CD1", "CD 2", "Disc 1", "disk02", "Disc 1 - Live".
pub(super) fn is_disc_folder(name: &str) -> bool {
    let lower = name.to_lowercase();
    let rest = ["cd", "disc", "disk"]
        .iter()
        .find_map(|prefix| lower.strip_prefix(prefix));
    rest.map(|r| r.trim_start_matches([' ', '-', '_', '.']))
        .is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()))
}

/// Which album a track belongs to: its MusicBrainz release, else its album tag within its
/// folder (so two "Greatest Hits" stay apart), else its folder.
pub(super) fn album_key(track: &TrackFacts) -> String {
    let folder = album_folder(&track.rel_path);
    if let Some(release) = &track.mb_release_id {
        format!("mb:{release}")
    } else if let Some(album) = &track.album {
        format!("tag:{folder}\u{1f}{}", album.to_lowercase())
    } else {
        format!("dir:{folder}")
    }
}

/// Each track's album key, by [`album_key`], except that a track without a release id joins the
/// MusicBrainz release its folder's tracks with the same album tag carry, when there's exactly
/// one: a bonus track the release doesn't list stays with the album rather than making another.
pub(super) fn album_keys(tracks: &[TrackFacts]) -> Vec<String> {
    let mut releases: HashMap<(&str, String), Vec<&str>> = HashMap::new();
    for track in tracks {
        if let (Some(release), Some(album)) = (&track.mb_release_id, &track.album) {
            let found = releases
                .entry((album_folder(&track.rel_path), album.to_lowercase()))
                .or_default();
            if !found.contains(&release.as_str()) {
                found.push(release);
            }
        }
    }
    tracks
        .iter()
        .map(|track| {
            let joined = match (&track.mb_release_id, &track.album) {
                (None, Some(album)) => releases
                    .get(&(album_folder(&track.rel_path), album.to_lowercase()))
                    .and_then(|found| match found.as_slice() {
                        [one] => Some(format!("mb:{one}")),
                        _ => None,
                    }),
                _ => None,
            };
            joined.unwrap_or_else(|| album_key(track))
        })
        .collect()
}

pub(super) fn artist_key(name: &str, mb_artist_id: Option<&str>) -> String {
    match mb_artist_id {
        Some(id) => format!("mb:{id}"),
        None => format!("name:{}", name.to_lowercase()),
    }
}

/// The value most tracks agree on (the first such, for ties).
pub(super) fn most_common<T: PartialEq + Clone>(values: impl Iterator<Item = T>) -> Option<T> {
    let mut counts: Vec<(T, usize)> = Vec::new();
    for value in values {
        match counts.iter_mut().find(|(v, _)| *v == value) {
            Some((_, n)) => *n += 1,
            None => counts.push((value, 1)),
        }
    }
    let best = counts.iter().map(|(_, n)| *n).max()?;
    counts.into_iter().find(|(_, n)| *n == best).map(|(v, _)| v)
}

pub(super) fn album_of(tracks: &[&TrackFacts]) -> AlbumFacts {
    let folder = album_folder(&tracks[0].rel_path).to_owned();
    let (parent, folder_name) = folder.rsplit_once('/').unwrap_or(("", &folder));
    let named = album_from_folder(folder_name);

    let album_artist = most_common(tracks.iter().filter_map(|t| t.album_artist.clone()));
    let mut artists: Vec<&str> = tracks.iter().filter_map(|t| t.artist.as_deref()).collect();
    artists.sort_unstable();
    artists.dedup();
    let artist = if let Some(name) = album_artist.filter(|a| !a.eq_ignore_ascii_case(VARIOUS)) {
        Some(name)
    } else if tracks.iter().any(|t| t.compilation) || artists.len() > 1 {
        None
    } else if let [only] = artists[..] {
        Some(only.to_owned())
    } else {
        // Untagged: Artist/Album folders, else "Artist - Album" in one folder name.
        let from_parent = parent.rsplit('/').next().filter(|p| !p.is_empty());
        Some(
            from_parent
                .map(str::to_owned)
                .or(named.artist.clone())
                .unwrap_or_else(|| "Unknown artist".to_owned()),
        )
    };
    let title = most_common(tracks.iter().filter_map(|t| t.album.clone()))
        .or_else(|| (!named.title.is_empty()).then(|| named.title.clone()))
        .unwrap_or_else(|| "Unknown album".to_owned());
    let mb_artist_id = match &artist {
        Some(_) => {
            most_common(tracks.iter().filter_map(|t| t.mb_album_artist_id.clone())).or_else(|| {
                // One artist throughout: their own id stands for the album's.
                let ids: Vec<&String> = tracks
                    .iter()
                    .filter_map(|t| t.mb_artist_id.as_ref())
                    .collect();
                (artists.len() == 1
                    && ids.len() == tracks.len()
                    && ids.windows(2).all(|w| w[0] == w[1]))
                .then(|| ids[0].clone())
            })
        }
        None => None,
    };
    AlbumFacts {
        title,
        artist,
        year: most_common(tracks.iter().filter_map(|t| t.year)).or(named.year),
        folder,
        mb_release_id: most_common(tracks.iter().filter_map(|t| t.mb_release_id.clone())),
        mb_release_group_id: most_common(
            tracks.iter().filter_map(|t| t.mb_release_group_id.clone()),
        ),
        mb_artist_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(rel_path: &str, album: &str, release: Option<&str>) -> TrackFacts {
        TrackFacts {
            rel_path: rel_path.into(),
            tagged: true,
            album: Some(album.into()),
            mb_release_id: release.map(Into::into),
            ..TrackFacts::default()
        }
    }

    #[test]
    fn a_track_without_a_release_id_stays_with_its_album() {
        let mut tracks: Vec<TrackFacts> = (1..=16)
            .map(|n| {
                track(
                    &format!("A/Halcyon (2025)/{n:02}.flac"),
                    "Halcyon",
                    Some("rel-1"),
                )
            })
            .collect();
        tracks.push(track("A/Halcyon (2025)/17 Bonus.flac", "HALCYON", None));
        let keys = album_keys(&tracks);
        assert!(keys.iter().all(|k| k == "mb:rel-1"), "{keys:?}");
    }

    #[test]
    fn two_releases_or_another_album_tag_stay_apart() {
        let tracks = [
            track("A/Both/01.flac", "Greatest Hits", Some("rel-1")),
            track("A/Both/02.flac", "Greatest Hits", Some("rel-2")),
            track("A/Both/03.flac", "Greatest Hits", None),
            track("A/Both/04.flac", "Other", None),
            track("A/Elsewhere/01.flac", "Greatest Hits", None),
        ];
        assert_eq!(
            album_keys(&tracks),
            [
                "mb:rel-1",
                "mb:rel-2",
                "tag:A/Both\u{1f}greatest hits",
                "tag:A/Both\u{1f}other",
                "tag:A/Elsewhere\u{1f}greatest hits",
            ]
        );
    }
}
