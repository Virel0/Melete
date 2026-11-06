//! Singles and EPs whose songs are all on an album. An artist wanted whole wants every release
//! group, so a single would bring its songs in twice; before an artist's albums are searched for
//! (and when following finds new ones), each wanted single or EP not yet checked has its songs
//! looked up on MusicBrainz and compared with the artist's albums': by recording, or by title
//! with only an edit or version note set aside ("Radio Edit", "Single Version"). A remix, live or
//! acoustic take is a song of its own. One whose every song is on an album is no longer
//! monitored; one with a song on no album stays wanted whole. Albums asked for one by one are
//! never checked. What leaves the server: the release groups' MusicBrainz ids, to MusicBrainz.

use std::collections::{HashMap, HashSet};

use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::musicbrainz::{self, Song};

/// What a release group's songs are known by: their recordings and their titles, as compared.
#[derive(Default)]
struct Known {
    recordings: HashSet<String>,
    titles: HashSet<String>,
}

impl Known {
    fn add(&mut self, songs: &[Song]) {
        for song in songs {
            self.recordings.insert(song.recording_id.clone());
            self.titles.insert(song_key(&song.title));
        }
    }

    fn has(&self, song: &Song) -> bool {
        self.recordings.contains(&song.recording_id) || self.titles.contains(&song_key(&song.title))
    }
}

#[derive(sqlx::FromRow)]
struct Unchecked {
    id: i64,
    wanted_artist_id: i64,
    title: String,
    mb_release_group_id: String,
}

/// Checks the wanted singles and EPs not yet checked (of one artist, or all), not on disk or
/// downloading; leaves out each whose songs are all on the artist's albums. Says which were left
/// out, as "Single (on Album)". One MusicBrainz can't answer for is checked another time.
pub async fn leave_out_covered(
    pool: &SqlitePool,
    mb: &musicbrainz::Client,
    artist: Option<i64>,
) -> anyhow::Result<Vec<String>> {
    let unchecked: Vec<Unchecked> = sqlx::query_as(
        "SELECT id, wanted_artist_id, title, mb_release_group_id FROM wanted_albums
         WHERE songs_checked_at IS NULL AND monitored = 1 AND album_type IN ('Single', 'EP')
           AND (?1 IS NULL OR wanted_artist_id = ?1)
         ORDER BY wanted_artist_id, id",
    )
    .bind(artist)
    .fetch_all(pool)
    .await?;
    let mut albums: HashMap<i64, Vec<(String, Known)>> = HashMap::new();
    let mut left_out = Vec::new();
    for single in unchecked {
        if crate::album_search::taken(pool, single.id).await? {
            checked(pool, single.id, true).await?;
            continue;
        }
        if let std::collections::hash_map::Entry::Vacant(entry) =
            albums.entry(single.wanted_artist_id)
        {
            match albums_of(pool, mb, single.wanted_artist_id).await {
                Ok(found) => {
                    entry.insert(found);
                }
                Err(err) => {
                    tracing::warn!(error = %err, "couldn't look up an artist's albums' songs");
                    continue;
                }
            }
        }
        let songs = match mb.release_group_songs(&single.mb_release_group_id).await {
            Ok(songs) => songs,
            Err(err) => {
                tracing::warn!(error = %err, "couldn't look up a single's songs");
                continue;
            }
        };
        let on = albums
            .get(&single.wanted_artist_id)
            .and_then(|found| covering(found, &songs));
        checked(pool, single.id, on.is_none()).await?;
        if let Some(album) = on {
            left_out.push(format!("{} (on {album})", single.title));
        }
    }
    Ok(left_out)
}

/// The artist's wanted albums (Album release groups), each with its songs.
async fn albums_of(
    pool: &SqlitePool,
    mb: &musicbrainz::Client,
    artist: i64,
) -> anyhow::Result<Vec<(String, Known)>> {
    let groups: Vec<(String, String)> = sqlx::query_as(
        "SELECT title, mb_release_group_id FROM wanted_albums
         WHERE wanted_artist_id = ? AND album_type = 'Album' ORDER BY release_date, id",
    )
    .bind(artist)
    .fetch_all(pool)
    .await?;
    let mut albums = Vec::new();
    for (title, group) in groups {
        let mut known = Known::default();
        let songs = mb
            .release_group_songs(&group)
            .await
            .map_err(|err| anyhow::anyhow!("MusicBrainz: {err}"))?;
        known.add(&songs);
        albums.push((title, known));
    }
    Ok(albums)
}

/// The albums that have `songs` between them, when every one is on one of them (named once
/// each, as they're listed); none when a song is on no album, or there are no songs to go by.
fn covering(albums: &[(String, Known)], songs: &[Song]) -> Option<String> {
    if songs.is_empty() {
        return None;
    }
    let mut on: Vec<&str> = Vec::new();
    for song in songs {
        let (title, _) = albums.iter().find(|(_, known)| known.has(song))?;
        if !on.contains(&title.as_str()) {
            on.push(title);
        }
    }
    Some(on.join(", "))
}

/// Records a single as checked: still wanted, or left out.
async fn checked(pool: &SqlitePool, id: i64, wanted: bool) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE wanted_albums SET songs_checked_at = ?, monitored = monitored AND ? WHERE id = ?",
    )
    .bind(unix_now())
    .bind(wanted)
    .bind(id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Words that make a bracketed note a take of its own rather than an edit of the same song.
const OWN_TAKE: &[&str] = &[
    "live",
    "remix",
    "mix",
    "acoustic",
    "demo",
    "instrumental",
    "cover",
    "session",
];
/// Words that make a note an edit or version of the same song.
const SAME_SONG: &[&str] = &[
    "edit",
    "version",
    "remaster",
    "remastered",
    "mono",
    "stereo",
];

/// A song's title as compared: lower case, letters and digits, with notes that only say it's an
/// edit or version of the song set aside ("Karma Police (Radio Edit)", "Creep - Single Version").
pub(crate) fn song_key(title: &str) -> String {
    let same_song = |note: &str| {
        let words: Vec<String> = note
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect();
        words.iter().any(|w| SAME_SONG.contains(&w.as_str()))
            && !words.iter().any(|w| OWN_TAKE.contains(&w.as_str()))
    };
    let mut kept = String::new();
    let mut rest = title;
    while let Some(open) = rest.find(['(', '[']) {
        let close = if rest[open..].starts_with('(') {
            ')'
        } else {
            ']'
        };
        let Some(len) = rest[open..].find(close) else {
            break;
        };
        let note = &rest[open + 1..open + len];
        kept.push_str(&rest[..open]);
        if !same_song(note) {
            kept.push(' ');
            kept.push_str(note);
        }
        rest = &rest[open + len + 1..];
    }
    kept.push_str(rest);
    if let Some((song, note)) = kept.rsplit_once(" - ")
        && same_song(note)
    {
        kept = song.to_owned();
    }
    kept.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::musicbrainz::testing::{
        self, CREEP_SINGLE, KARMA_SINGLE, OK_COMPUTER, PARANOID_SINGLE, RADIOHEAD,
    };

    /// Radiohead wanted whole: OK Computer and Kid A, and its three singles waiting to be
    /// compared; one more single asked for by itself, and one already downloading.
    async fn radiohead(pool: &SqlitePool) {
        sqlx::query(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source,
                 added_at) VALUES (1, ?, 'Radiohead', 1, 'all', 'manual', 0)",
        )
        .bind(RADIOHEAD)
        .execute(pool)
        .await
        .unwrap();
        for (group, title, kind, checked) in [
            (OK_COMPUTER, "OK Computer", "Album", Some(1)),
            (
                "2a0981fb-9593-3019-864b-ce934d97a16e",
                "Kid A",
                "Album",
                Some(1),
            ),
            (CREEP_SINGLE, "Creep", "Single", None),
            (KARMA_SINGLE, "Karma Police", "Single", None),
            (PARANOID_SINGLE, "Paranoid Android", "EP", None),
            ("rg-asked", "No Surprises", "Single", Some(1)),
            ("rg-coming", "Lucky", "Single", None),
        ] {
            sqlx::query(
                "INSERT INTO wanted_albums (wanted_artist_id, mb_release_group_id, title,
                     album_type, monitored, source, added_at, songs_checked_at)
                 VALUES (1, ?, ?, ?, 1, 'manual', 0, ?)",
            )
            .bind(group)
            .bind(title)
            .bind(kind)
            .bind(checked)
            .execute(pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO downloads (infohash, title, decision, state, added_at, updated_at,
                 wanted_album_id)
             VALUES ('abababababababababababababababababababab', 'Radiohead - Lucky', '{}', 'downloading', 0, 0,
                 (SELECT id FROM wanted_albums WHERE title = 'Lucky'))",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_single_whose_songs_are_all_on_an_album_is_left_out() {
        let pool = crate::db::open_in_memory().await;
        radiohead(&pool).await;
        let (mb, log) = testing::start(0).await;

        let left = leave_out_covered(&pool, &mb, Some(1)).await.unwrap();

        assert_eq!(left, ["Karma Police (on OK Computer)"]);
        let wanted: Vec<(String, bool, bool)> = sqlx::query_as(
            "SELECT title, monitored, songs_checked_at IS NOT NULL FROM wanted_albums ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let wanted: Vec<(&str, bool, bool)> = wanted
            .iter()
            .map(|(t, m, c)| (t.as_str(), *m, *c))
            .collect();
        assert_eq!(
            wanted,
            [
                ("OK Computer", true, true),
                ("Kid A", true, true),
                ("Creep", true, true),
                ("Karma Police", false, true),
                ("Paranoid Android", true, true),
                ("No Surprises", true, true),
                ("Lucky", true, true),
            ],
            "the radio edit's single goes; one with a B-side, one on no album, one asked for by \
             itself and one downloading stay"
        );
        let asked = log.lock().unwrap().len();
        assert_eq!(asked, 5, "two albums' songs and three singles'");

        // Checked once: nothing is asked again.
        assert!(
            leave_out_covered(&pool, &mb, None)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(log.lock().unwrap().len(), asked);
    }

    #[test]
    fn edits_and_versions_are_the_same_song_but_other_takes_arent() {
        let same = |a: &str, b: &str| song_key(a) == song_key(b);
        assert!(same("Karma Police (Radio Edit)", "Karma Police"));
        assert!(same("Creep - Single Version", "Creep"));
        assert!(same("No Surprises [2009 Remaster]", "No Surprises"));
        assert!(same("Let Down", "let down"));
        assert!(!same("Karma Police (Live)", "Karma Police"));
        assert!(!same("Creep (Acoustic Version)", "Creep"));
        assert!(!same(
            "Everything in Its Right Place (Four Tet Remix)",
            "Everything in Its Right Place"
        ));
        assert!(!same("Polyethylene (Parts 1 & 2)", "Polyethylene"));
    }
}
