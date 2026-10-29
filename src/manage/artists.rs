//! The artists list: every wanted artist, and every artist in the music library no wanted
//! artist claims.

use std::collections::{HashMap, HashSet};

use super::*;

/// An artist's own fields in the list.
#[derive(Debug, Clone, Serialize)]
pub struct ArtistDetail {
    /// The artist in the music library, when any of theirs is on disk.
    pub artist_id: Option<i64>,
    /// MusicBrainz's id, when known.
    pub mb_artist_id: Option<String>,
    /// What happens to albums they release later (all, new or none); null when not wanted.
    pub monitor_new: Option<String>,
    /// Albums on disk: of those wanted when the artist is, else all of theirs.
    pub albums_on_disk: i64,
    /// Monitored albums that are out.
    pub albums_wanted: i64,
}

impl super::Folder for ArtistDetail {
    fn folder(&self) -> Option<i64> {
        self.artist_id
    }
}

#[derive(sqlx::FromRow)]
struct WantedRow {
    id: i64,
    mb_artist_id: String,
    name: String,
    monitored: bool,
    monitor_new: String,
    added_at: i64,
    profile_id: Option<i64>,
    profile: Option<String>,
    library: Option<String>,
}

#[derive(sqlx::FromRow)]
struct AlbumRow {
    id: i64,
    artist_id: Option<i64>,
    mb_artist_id: Option<String>,
    artist: Option<String>,
    mb_release_group_id: Option<String>,
    match_release_group_id: Option<String>,
    library: String,
    size: Option<i64>,
    first: Option<i64>,
}

/// A wanted album: its artist, id, release group, whether it's monitored and out.
type WantedAlbum = (i64, i64, String, bool, bool);

pub(super) const WANTED: &str = "
    SELECT w.id, w.mb_artist_id, w.name, w.monitored, w.monitor_new, w.added_at, w.profile_id,
           coalesce(p.name, w.quality_profile) AS profile, l.name AS library
    FROM wanted_artists w
    LEFT JOIN libraries l ON l.id = w.library_id
    LEFT JOIN music_profiles p ON p.id = w.profile_id";

/// Wanted albums; one dated by its year or month alone is out once that's over.
pub(super) const WANTED_ALBUMS: &str = "
    SELECT wanted_artist_id, id, mb_release_group_id, monitored,
           coalesce(CASE length(release_date) WHEN 4 THEN release_date || '-12-31'
                                               WHEN 7 THEN release_date || '-31'
                                               ELSE release_date END <= date('now'), 0)
    FROM wanted_albums";

pub(super) const ALBUMS: &str = "
    SELECT a.id, a.artist_id, ar.mb_artist_id, ar.name AS artist, a.mb_release_group_id,
           a.match_release_group_id, l.name AS library, t.size, t.first
    FROM music_albums a
    JOIN libraries l ON l.id = a.library_id
    LEFT JOIN music_artists ar ON ar.id = a.artist_id
    LEFT JOIN (SELECT album_id, sum(size) AS size, min(modified_at) AS first FROM music_tracks
               WHERE album_id IS NOT NULL AND missing_since IS NULL GROUP BY album_id) t
        ON t.album_id = a.id
    WHERE a.missing_since IS NULL AND t.size IS NOT NULL";

/// The library's albums, found by release group and by MusicBrainz artist.
struct Albums<'a> {
    by_group: HashMap<&'a str, Vec<&'a AlbumRow>>,
    by_mb_artist: HashMap<&'a str, Vec<&'a AlbumRow>>,
}

impl<'a> Albums<'a> {
    fn new(albums: &'a [AlbumRow]) -> Self {
        let mut by_group: HashMap<&str, Vec<&AlbumRow>> = HashMap::new();
        let mut by_mb_artist: HashMap<&str, Vec<&AlbumRow>> = HashMap::new();
        for album in albums {
            for group in [&album.mb_release_group_id, &album.match_release_group_id]
                .into_iter()
                .flatten()
            {
                by_group.entry(group).or_default().push(album);
            }
            if let Some(mb) = &album.mb_artist_id {
                by_mb_artist.entry(mb).or_default().push(album);
            }
        }
        Albums {
            by_group,
            by_mb_artist,
        }
    }
}

/// Every managed artist, unsorted. A wanted artist claims the library's albums by the same
/// MusicBrainz artist, and every album on disk of a release group it wants; an artist in the
/// library no wanted artist claims is a row of its own.
pub async fn artists(pool: &SqlitePool) -> anyhow::Result<Vec<Title<ArtistDetail>>> {
    let wanted: Vec<WantedRow> = sqlx::query_as(WANTED).fetch_all(pool).await?;
    let wanted_albums: Vec<WantedAlbum> = sqlx::query_as(WANTED_ALBUMS).fetch_all(pool).await?;
    let albums: Vec<AlbumRow> = sqlx::query_as(ALBUMS).fetch_all(pool).await?;
    let downloads = Downloads::load(pool, "wanted_album_id").await?;
    let library = Albums::new(&albums);
    let mut albums_of: HashMap<i64, Vec<&WantedAlbum>> = HashMap::new();
    for album in &wanted_albums {
        albums_of.entry(album.0).or_default().push(album);
    }

    let mut titles = Vec::new();
    let mut claimed: HashSet<i64> = HashSet::new();
    for w in wanted {
        let theirs = albums_of.remove(&w.id).unwrap_or_default();
        let (title, artists) = wanted_title(w, &theirs, &library, &downloads);
        claimed.extend(artists);
        titles.push(title);
    }
    titles.extend(unclaimed(&albums, &claimed));
    Ok(titles)
}

/// A wanted artist's row, and the library's artists it claims. Missing are its monitored
/// albums that are out and not on disk, while the artist is monitored; a download counts as
/// bringing what it lacks when its album isn't on disk.
fn wanted_title(
    w: WantedRow,
    theirs: &[&WantedAlbum],
    library: &Albums,
    downloads: &Downloads,
) -> (Title<ArtistDetail>, Vec<i64>) {
    let mut present: HashMap<i64, &AlbumRow> = HashMap::new();
    for album in library
        .by_mb_artist
        .get(w.mb_artist_id.as_str())
        .into_iter()
        .flatten()
    {
        present.insert(album.id, album);
    }
    let (mut wanted_count, mut found, mut under_way) = (0, 0, false);
    for (_, id, group, monitored, released) in theirs.iter().copied() {
        let here = library
            .by_group
            .get(group.as_str())
            .map_or(&[][..], Vec::as_slice);
        for album in here {
            present.insert(album.id, album);
        }
        under_way |= here.is_empty() && downloads.under_way(*id).is_some();
        if *monitored && *released && w.monitored {
            wanted_count += 1;
            found += i64::from(!here.is_empty());
        }
    }
    let facts = Facts {
        on_disk: !present.is_empty(),
        missing: wanted_count - found,
        under_way,
        monitored: w.monitored,
    };
    let shown = present.values().min_by_key(|a| a.id);
    let title = Title {
        wanted_id: Some(w.id),
        title: w.name,
        year: None,
        poster: None,
        state: facts.state(),
        monitored: w.monitored,
        wanted: true,
        profile_id: w.profile_id,
        profile: w.profile,
        library: shown.map(|a| a.library.clone()).or(w.library),
        category: None,
        size: present.values().filter_map(|a| a.size).sum(),
        added_at: Some(w.added_at),
        download: theirs.iter().find_map(|a| downloads.under_way(a.1)),
        cutoff_unmet: None,
        detail: ArtistDetail {
            artist_id: shown.and_then(|a| a.artist_id),
            mb_artist_id: Some(w.mb_artist_id),
            monitor_new: Some(w.monitor_new),
            albums_on_disk: found,
            albums_wanted: wanted_count,
        },
    };
    (
        title,
        present.values().filter_map(|a| a.artist_id).collect(),
    )
}

/// The library's artists no wanted artist claims, each with its albums on disk.
fn unclaimed(albums: &[AlbumRow], claimed: &HashSet<i64>) -> Vec<Title<ArtistDetail>> {
    let mut by_artist: HashMap<i64, Vec<&AlbumRow>> = HashMap::new();
    for album in albums {
        if let Some(artist) = album.artist_id.filter(|a| !claimed.contains(a)) {
            by_artist.entry(artist).or_default().push(album);
        }
    }
    by_artist
        .into_iter()
        .map(|(artist, theirs)| Title {
            wanted_id: None,
            title: theirs[0].artist.clone().unwrap_or_default(),
            year: None,
            poster: None,
            state: State::InLibrary,
            monitored: false,
            wanted: false,
            profile_id: None,
            profile: None,
            library: Some(theirs[0].library.clone()),
            category: None,
            size: theirs.iter().filter_map(|a| a.size).sum(),
            added_at: theirs.iter().filter_map(|a| a.first).min(),
            download: None,
            cutoff_unmet: None,
            detail: ArtistDetail {
                artist_id: Some(artist),
                mb_artist_id: theirs[0].mb_artist_id.clone(),
                monitor_new: None,
                albums_on_disk: theirs.len() as i64,
                albums_wanted: 0,
            },
        })
        .collect()
}
