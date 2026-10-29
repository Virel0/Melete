//! Each present track's artists, in credit order, worked out from every present track after each
//! scan and match, so a featured artist or a duet partner is found under their own name and not
//! only the album's artist. The evidence, strongest first:
//!
//! 1. MusicBrainz's credit for the track on its album's matched release;
//! 2. the artists its tags list apart (`ARTISTS`, or several artist values), paired with its
//!    MusicBrainz artist ids when there is one id per name;
//! 3. its artist string, whole.
//!
//! A name is never split on `/`, `;`, `,` or `&`: "AC/DC" and "Earth, Wind & Fire" are one
//! artist each. "A feat. B" is split on " feat. ", " ft. " or " featuring " only when its parts
//! match its MusicBrainz artist ids one for one, or every part is an artist known on its own
//! (an album artist, or one credited alone or listed apart on some track). A string the tags'
//! ids can't be paired with is whole, unless every id is an artist known by that id, who are
//! then its artists. A track with no artist tag is its album artist's.
//!
//! An artist with a MusicBrainz id is one row however tags spell the name. A name with no id is
//! the track's album artist when spelled the same (case aside), else the one artist known by
//! that name, else an artist of its own ("name:<lowercased>", as album artists are keyed).
//! Rebuilt whole each time; the track rows themselves (and so plays, pins and playlists, which
//! point at them) are never touched.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use sqlx::SqlitePool;

/// What working out a track's artists needs from it.
#[derive(Debug, Default, Clone, sqlx::FromRow)]
struct Track {
    id: i64,
    /// The artist string, as tagged.
    artist: Option<String>,
    /// The artists its tags list apart, as JSON.
    artist_names: String,
    /// Every MusicBrainz artist id its tags give, as JSON.
    mb_artist_ids: String,
    /// Its album's artist; none for a compilation.
    album_artist_id: Option<i64>,
    /// Its album's artist as shown.
    album_artist: String,
    /// MusicBrainz's credit for it on the matched release, as JSON, when known.
    mb_credit: Option<String>,
}

#[derive(Deserialize)]
struct MbCredit {
    name: String,
    id: String,
    artist: String,
}

/// One artist credited on a track, before it's linked to an artist row.
#[derive(Debug, Clone, PartialEq)]
struct Credit {
    /// The name as credited on the track.
    name: String,
    /// Their MusicBrainz id, when known.
    mb_artist_id: Option<String>,
    /// Their own name, when MusicBrainz gives it apart from the credited one.
    own_name: Option<String>,
}

impl Credit {
    fn named(name: &str, mb_artist_id: Option<&str>) -> Credit {
        Credit {
            name: name.to_owned(),
            mb_artist_id: mb_artist_id.map(str::to_owned),
            own_name: None,
        }
    }
}

/// What a track's own data says about its artists, before the rest of the library is consulted.
#[derive(Debug, Clone, PartialEq)]
enum Said {
    /// Artists listed apart (by MusicBrainz or a multi-value tag): taken as they are.
    Listed(Vec<Credit>),
    /// One artist string, with the tags' MusicBrainz artist ids: split or paired only on
    /// evidence.
    Whole(String, Vec<String>),
    /// No artist tag: the album's artist, if it has one.
    Untagged,
}

fn said(track: &Track) -> Said {
    let mb: Vec<MbCredit> = track
        .mb_credit
        .as_deref()
        .and_then(|c| serde_json::from_str(c).ok())
        .unwrap_or_default();
    if !mb.is_empty() {
        return Said::Listed(
            mb.into_iter()
                .map(|c| Credit {
                    own_name: (c.artist != c.name).then_some(c.artist),
                    name: c.name,
                    mb_artist_id: Some(c.id),
                })
                .collect(),
        );
    }
    let names: Vec<String> = serde_json::from_str(&track.artist_names).unwrap_or_default();
    let ids: Vec<String> = serde_json::from_str(&track.mb_artist_ids).unwrap_or_default();
    match names.len() {
        0 | 1 => match names.into_iter().next().or_else(|| track.artist.clone()) {
            Some(name) => Said::Whole(name, ids),
            None => Said::Untagged,
        },
        n => Said::Listed(
            names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    Credit::named(
                        name,
                        ids.get(i).filter(|_| ids.len() == n).map(String::as_str),
                    )
                })
                .collect(),
        ),
    }
}

/// "A feat. B" as its parts; one part when there's no such separator.
fn featuring_parts(name: &str) -> Vec<String> {
    const SEPARATORS: [&str; 3] = [" feat. ", " ft. ", " featuring "];
    // ASCII lowercasing keeps every byte where it was, so positions in it are positions in name.
    let lower = name.to_ascii_lowercase();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < lower.len() {
        match SEPARATORS.iter().find(|s| lower[i..].starts_with(*s)) {
            Some(separator) => {
                parts.push(name[start..i].trim().to_owned());
                i += separator.len();
                start = i;
            }
            None => i += lower[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    parts.push(name[start..].trim().to_owned());
    if parts.iter().any(String::is_empty) {
        return vec![name.to_owned()];
    }
    parts
}

/// Artists known across the library: by lowercased name, and by MusicBrainz id with a name.
#[derive(Debug, Default)]
struct Known {
    names: HashSet<String>,
    ids: HashMap<String, String>,
}

impl Known {
    fn learn(&mut self, name: &str, id: Option<&str>) {
        self.names.insert(name.to_lowercase());
        if let Some(id) = id {
            self.ids
                .entry(id.to_owned())
                .or_insert_with(|| name.to_owned());
        }
    }

    /// What every track's own data says, and the album artists, as known artists.
    fn from(said: &[Said], album_artists: &[String], rows: &[ArtistRow]) -> Known {
        let mut known = Known::default();
        for name in album_artists {
            known.learn(name, None);
        }
        for row in rows {
            if let Some(id) = &row.mb_artist_id {
                known.learn(&row.name, Some(id));
            }
        }
        for one in said {
            match one {
                Said::Listed(credits) => credits
                    .iter()
                    .for_each(|c| known.learn(&c.name, c.mb_artist_id.as_deref())),
                Said::Whole(name, ids) if ids.len() <= 1 => {
                    known.learn(name, ids.first().map(String::as_str));
                }
                Said::Whole(..) | Said::Untagged => {}
            }
        }
        known
    }
}

/// A track's credits, the rest of the library consulted for an artist string's.
fn credits(said: &Said, known: &Known, album_artist: Option<&str>) -> Vec<Credit> {
    let (name, ids) = match said {
        Said::Listed(credits) => return credits.clone(),
        Said::Untagged => {
            return album_artist
                .map(|a| vec![Credit::named(a, None)])
                .unwrap_or_default();
        }
        Said::Whole(name, ids) => (name, ids),
    };
    let parts = featuring_parts(name);
    if parts.len() > 1 && parts.len() == ids.len() {
        return parts
            .iter()
            .zip(ids)
            .map(|(part, id)| Credit::named(part, Some(id)))
            .collect();
    }
    if parts.len() > 1
        && parts
            .iter()
            .all(|p| known.names.contains(&p.to_lowercase()))
    {
        return parts.iter().map(|p| Credit::named(p, None)).collect();
    }
    match &ids[..] {
        // One id for "A feat. B" is A's, the main artist, whom the tagger gave the id.
        [id] => vec![Credit::named(&parts[0], Some(id))],
        [_, _, ..] if ids.iter().all(|id| known.ids.contains_key(id)) => ids
            .iter()
            .map(|id| Credit::named(&known.ids[id], Some(id)))
            .collect(),
        _ => vec![Credit::named(name, None)],
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ArtistRow {
    id: i64,
    key: String,
    name: String,
    mb_artist_id: Option<String>,
    /// Artist of an album in the library now.
    present: bool,
}

/// The artist rows, to link credits to.
#[derive(Debug, Default)]
struct Artists {
    rows: HashMap<i64, ArtistRow>,
    by_id: HashMap<String, i64>,
    by_name: HashMap<String, Vec<i64>>,
}

impl Artists {
    fn new(rows: &[ArtistRow]) -> Artists {
        let mut artists = Artists::default();
        // Album artists first, then rows keyed by the id, then the oldest: the first added wins
        // an id that several rows have.
        let mut ordered: Vec<&ArtistRow> = rows.iter().collect();
        ordered.sort_by_key(|r| (!r.present, !r.key.starts_with("mb:"), r.id));
        for row in ordered {
            artists.add(row.clone());
        }
        artists
    }

    fn add(&mut self, row: ArtistRow) {
        if let Some(id) = &row.mb_artist_id {
            self.by_id.entry(id.clone()).or_insert(row.id);
        }
        let named = self.by_name.entry(row.name.to_lowercase()).or_default();
        named.push(row.id);
        named.sort_unstable();
        self.rows.insert(row.id, row);
    }

    /// The row a credit is, if there's one already.
    fn find(&self, credit: &Credit, album_artist: Option<(i64, &str)>) -> Option<i64> {
        let lower = credit.name.to_lowercase();
        let album = album_artist
            .filter(|(_, name)| name.to_lowercase() == lower)
            .and_then(|(id, _)| self.rows.get(&id));
        if let Some(mb) = &credit.mb_artist_id {
            // An id no row has yet, spelled as the album's artist whose own tags gave no id, is
            // the album's artist.
            return self
                .by_id
                .get(mb)
                .copied()
                .or_else(|| album.filter(|r| r.mb_artist_id.is_none()).map(|r| r.id));
        }
        if let Some(row) = album {
            return Some(row.id);
        }
        let rows: Vec<&ArtistRow> = self
            .by_name
            .get(&lower)?
            .iter()
            .filter_map(|id| self.rows.get(id))
            .collect();
        let present: Vec<&ArtistRow> = rows.iter().copied().filter(|r| r.present).collect();
        if let [only] = present[..] {
            return Some(only.id);
        }
        if let [only] = rows[..] {
            return Some(only.id);
        }
        let key = format!("name:{lower}");
        rows.iter().find(|r| r.key == key).map(|r| r.id)
    }
}

/// Works out every present track's artists and stores them, replacing what was there; returns
/// how many tracks have artists.
///
/// # Errors
///
/// Fails when the database can't be read or written; nothing is changed then.
pub async fn refresh(pool: &SqlitePool) -> anyhow::Result<usize> {
    // Read before the transaction, which then starts with a write: one that read first and
    // wrote later would fail if a scan wrote in between. Anything a scan changes meanwhile is
    // caught by the run it wakes.
    let tracks: Vec<Track> = sqlx::query_as(
        "SELECT t.id, t.artist, t.artist_names, t.mb_artist_ids,
             a.artist_id AS album_artist_id, a.artist_name AS album_artist,
             rt.artist_credit AS mb_credit
         FROM music_tracks t
         JOIN music_albums a ON a.id = t.album_id
         LEFT JOIN mb_release_tracks rt ON a.match_status = 'matched'
             AND rt.release_id = a.match_release_id
             AND rt.track_id = coalesce(t.mb_track_id, t.match_track_id)
         WHERE t.missing_since IS NULL AND a.missing_since IS NULL ORDER BY t.id",
    )
    .fetch_all(pool)
    .await?;
    let rows: Vec<ArtistRow> = sqlx::query_as(
        "SELECT ar.id, ar.key, ar.name, ar.mb_artist_id,
             EXISTS (SELECT 1 FROM music_albums a
                 WHERE a.artist_id = ar.id AND a.missing_since IS NULL) AS present
         FROM music_artists ar ORDER BY ar.id",
    )
    .fetch_all(pool)
    .await?;
    let saids: Vec<Said> = tracks.iter().map(said).collect();
    let album_artists: Vec<String> = tracks
        .iter()
        .filter(|t| t.album_artist_id.is_some())
        .map(|t| t.album_artist.clone())
        .collect();
    let known = Known::from(&saids, &album_artists, &rows);
    let all: Vec<Vec<Credit>> = tracks
        .iter()
        .zip(&saids)
        .map(|(track, said)| credits(said, &known, album_artist(track).map(|(_, name)| name)))
        .collect();
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM music_track_artists")
        .execute(&mut *tx)
        .await?;
    let linked = link(&mut tx, &tracks, &all, Artists::new(&rows)).await?;
    let mut credited = 0;
    for ((track, credits), ids) in tracks.iter().zip(&all).zip(&linked) {
        let mut stored: Vec<i64> = Vec::new();
        for (credit, id) in credits.iter().zip(ids) {
            if stored.contains(id) {
                continue;
            }
            sqlx::query(
                "INSERT INTO music_track_artists (track_id, position, artist_id, name)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(track.id)
            .bind(i64::try_from(stored.len())?)
            .bind(id)
            .bind(&credit.name)
            .execute(&mut *tx)
            .await?;
            stored.push(*id);
        }
        credited += usize::from(!stored.is_empty());
    }
    tx.commit().await?;
    // Artists added here are found by the same folded search as album artists.
    crate::search_keys::fill(pool).await?;
    Ok(credited)
}

fn album_artist(track: &Track) -> Option<(i64, &str)> {
    track
        .album_artist_id
        .map(|id| (id, track.album_artist.as_str()))
}

/// Each credit's artist row, adding the ones not there yet. Credits with an id go first, so a
/// name alone finds the artist an id made, whichever track comes first.
async fn link(
    tx: &mut sqlx::SqliteConnection,
    tracks: &[Track],
    all: &[Vec<Credit>],
    mut artists: Artists,
) -> anyhow::Result<Vec<Vec<i64>>> {
    let mut linked: Vec<Vec<i64>> = all.iter().map(|c| vec![0; c.len()]).collect();
    for with_id in [true, false] {
        for ((track, credits), ids) in tracks.iter().zip(all).zip(&mut linked) {
            for (credit, id) in credits.iter().zip(ids.iter_mut()) {
                if credit.mb_artist_id.is_some() != with_id {
                    continue;
                }
                *id = match artists.find(credit, album_artist(track)) {
                    Some(found) => found,
                    None => insert_artist(&mut *tx, &mut artists, credit).await?,
                };
            }
        }
    }
    Ok(linked)
}

/// Adds an artist for a credit no row is yet: keyed by MusicBrainz id, else by name.
async fn insert_artist(
    tx: &mut sqlx::SqliteConnection,
    artists: &mut Artists,
    credit: &Credit,
) -> anyhow::Result<i64> {
    let name = credit.own_name.as_deref().unwrap_or(&credit.name);
    let key = match &credit.mb_artist_id {
        Some(id) => format!("mb:{id}"),
        None => format!("name:{}", credit.name.to_lowercase()),
    };
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO music_artists (key, name, mb_artist_id) VALUES (?, ?, ?)
         ON CONFLICT (key) DO UPDATE SET
             mb_artist_id = coalesce(music_artists.mb_artist_id, excluded.mb_artist_id)
         RETURNING id",
    )
    .bind(&key)
    .bind(name)
    .bind(&credit.mb_artist_id)
    .fetch_one(&mut *tx)
    .await?;
    artists.add(ArtistRow {
        id,
        key,
        name: name.to_owned(),
        mb_artist_id: credit.mb_artist_id.clone(),
        present: false,
    });
    Ok(id)
}

#[cfg(test)]
mod tests;
