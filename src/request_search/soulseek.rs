//! Looking on Soulseek for music MusicBrainz doesn't know: the words go to slskd as they were
//! typed, and nothing else. What its users share is read as songs, each named by its files
//! ("Artist - Title", or a title in an artist's folder), with how many users have it and in
//! which formats, so one can be wanted by its title and artist. The same answer is read as
//! album folders too, to say which of MusicBrainz's albums for the words its users share.

use std::collections::{BTreeSet, HashMap};

use serde::Deserialize;

use super::*;
use crate::slskd;

/// A song as it's gathered: it, the users who share it, and its formats.
type Gathered = (Heard, BTreeSet<String>, BTreeSet<String>);

/// Songs listed at most.
const MOST: usize = 30;
/// What a file is called when it's music.
const AUDIO: [&str; 9] = [
    "flac", "mp3", "m4a", "ogg", "opus", "wav", "aac", "ape", "wv",
];

/// A song found on Soulseek, as its files name it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Heard {
    /// Its title.
    pub title: String,
    /// Its artist, when the file or its folder says.
    pub artist: Option<String>,
    /// How many users share it.
    pub users: usize,
    /// Its formats (FLAC, MP3…), as its files' names end.
    pub formats: Vec<String>,
}

/// The song a shared file's path names: its title, and its artist when the name or the folder
/// gives one. None for a file that isn't music.
fn song(path: &str) -> Option<(Option<String>, String, String)> {
    let parts: Vec<&str> = path.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
    let (name, ext) = parts.last()?.rsplit_once('.')?;
    let ext = ext.to_lowercase();
    if !AUDIO.contains(&ext.as_str()) {
        return None;
    }
    // A track number in front goes: "01 - ", "1. ", "03_".
    let name = name
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start_matches([' ', '.', '-', '_'])
        .trim();
    let folder = |back: usize| parts.len().checked_sub(back + 1).map(|i| parts[i]);
    let (artist, title) = match name.split_once(" - ") {
        Some((artist, title)) => (Some(artist.trim().to_owned()), title.trim().to_owned()),
        None => {
            // "Artist - Album" for a folder, else the folder above the album's.
            let artist = match folder(1).and_then(|f| f.split_once(" - ")) {
                Some((artist, _)) => Some(artist.trim().to_owned()),
                None => folder(2).map(|f| f.trim().to_owned()),
            };
            (artist, name.to_owned())
        }
    };
    (!title.is_empty()).then(|| (artist.filter(|a| !a.is_empty()), title, ext.to_uppercase()))
}

/// The songs Soulseek's answers share, most shared first.
pub fn songs(answers: &[slskd::Response]) -> Vec<Heard> {
    // By folded artist and title: the song, who shares it, and in which formats.
    let mut found: HashMap<(String, String), Gathered> = HashMap::new();
    for answer in answers {
        for file in &answer.files {
            let Some((artist, title, format)) = song(&file.filename) else {
                continue;
            };
            let key = (
                crate::search_keys::fold(artist.as_deref().unwrap_or_default()),
                crate::search_keys::fold(&title),
            );
            let entry = found.entry(key).or_insert_with(|| {
                let heard = Heard {
                    title,
                    artist,
                    users: 0,
                    formats: Vec::new(),
                };
                (heard, BTreeSet::new(), BTreeSet::new())
            });
            entry.1.insert(answer.username.clone());
            entry.2.insert(format);
        }
    }
    let mut songs: Vec<Heard> = found
        .into_values()
        .map(|(mut heard, users, formats)| {
            heard.users = users.len();
            heard.formats = formats.into_iter().collect();
            heard
        })
        .collect();
    songs.sort_by(|a, b| {
        b.users
            .cmp(&a.users)
            .then_with(|| a.artist.cmp(&b.artist))
            .then_with(|| a.title.cmp(&b.title))
    });
    songs.truncate(MOST);
    songs
}

/// Asks slskd for `q`, sending only the words.
async fn ask(slskd: Option<&slskd::Client>, q: &str) -> Result<Vec<slskd::Response>, Refused> {
    let q = q.trim();
    if q.is_empty() || q.chars().count() > 200 {
        return Err(Refused::Invalid("q is 1 to 200 characters".into()));
    }
    let Some(slskd) = slskd else {
        return Err(Refused::Conflict(
            "Soulseek is searched through slskd, which isn't set up (MELETE_SLSKD_URL)".into(),
        ));
    };
    slskd.search(q).await.map_err(|err| outside("slskd", err))
}

/// Looks on Soulseek for `q`, sending slskd only the words.
///
/// # Errors
///
/// The inner [`Refused`] when `q` isn't usable, slskd isn't set up, or it didn't answer.
pub async fn search_soulseek(
    slskd: Option<&slskd::Client>,
    q: &str,
) -> Result<Vec<Heard>, Refused> {
    ask(slskd, q).await.map(|answers| songs(&answers))
}

/// Soulseek's songs for `q` as search results, closest first ([`how_close`]), then most shared:
/// each a song wanted by its title and artist, saying who shares it in which formats, and
/// marked wanted or asked for already as MusicBrainz's are. Only the words go to slskd.
///
/// # Errors
///
/// The inner [`Refused`] as [`search_soulseek`] gives one; the outer error when the database
/// fails.
pub async fn soulseek_candidates(
    pool: &SqlitePool,
    slskd: Option<&slskd::Client>,
    user_id: i64,
    q: &str,
) -> anyhow::Result<Result<Vec<Candidate>, Refused>> {
    match search_soulseek(slskd, q).await {
        Ok(heard) => Ok(Ok(song_candidates(pool, user_id, q, heard).await?)),
        Err(refused) => Ok(Err(refused)),
    }
}

/// Who shares it, as a result's detail says: "Shared by 2 users, FLAC, MP3".
fn shared_by(users: usize, formats: &[String]) -> String {
    let who = if users == 1 { "user" } else { "users" };
    format!("Shared by {users} {who}, {}", formats.join(", "))
}

/// The songs heard for `q` as search results, closest first, then most shared.
async fn song_candidates(
    pool: &SqlitePool,
    user_id: i64,
    q: &str,
    heard: Vec<Heard>,
) -> anyhow::Result<Vec<Candidate>> {
    let mut found = Vec::with_capacity(heard.len());
    for song in heard {
        let candidate = Candidate {
            kind: "song".into(),
            closeness: how_close(q.trim(), &song.title, song.artist.as_deref()),
            detail: Some(shared_by(song.users, &song.formats)),
            title: song.title,
            artist: song.artist,
            ..Candidate::default()
        };
        found.push(mark(pool, user_id, candidate).await?);
    }
    found.sort_by_key(|c| std::cmp::Reverse(c.closeness));
    Ok(found)
}

/// An album MusicBrainz found for the words, to be looked for among Soulseek's folders.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlbumShown {
    /// Its release group, as the search gave it.
    pub mb_release_group_id: String,
    /// Its title.
    pub title: String,
    /// Its credited artists, as one line.
    pub artist: String,
}

/// A MusicBrainz album that Soulseek's users share as a whole album folder.
#[derive(Debug, PartialEq, Serialize)]
pub struct AlbumShared {
    /// The album's release group.
    pub mb_release_group_id: String,
    /// How many users share it.
    pub users: usize,
    /// Its formats (FLAC, MP3…), as its tracks' names end.
    pub formats: Vec<String>,
    /// The same, as a result's detail says it.
    pub detail: String,
}

/// What one Soulseek search for the words found: its songs as search results, and which of
/// MusicBrainz's albums for the same words its users share.
#[derive(Debug, Serialize)]
pub struct SoulseekFound {
    /// As [`soulseek_candidates`] gives them.
    pub songs: Vec<Candidate>,
    /// The albums shared, in the order they were given.
    pub albums: Vec<AlbumShared>,
}

/// Albums looked for at most in one search: more than MusicBrainz lists for words.
const MOST_ALBUMS: usize = 50;

/// Looks on Soulseek for `q` once, sending slskd only the words, and reads the answer twice: as
/// songs ([`soulseek_candidates`]) and as album folders, each of `albums` marked shared when a
/// whole folder is named as it is ([`crate::soulseek::shared`]). An album folder matching none
/// of them isn't listed: an album is wanted by its MusicBrainz release group.
///
/// # Errors
///
/// The inner [`Refused`] as [`search_soulseek`] gives one, or when there are too many albums;
/// the outer error when the database fails.
pub async fn soulseek_found(
    pool: &SqlitePool,
    slskd: Option<&slskd::Client>,
    user_id: i64,
    q: &str,
    albums: &[AlbumShown],
) -> anyhow::Result<Result<SoulseekFound, Refused>> {
    if albums.len() > MOST_ALBUMS {
        let why = format!("at most {MOST_ALBUMS} albums are looked for at once");
        return Ok(Err(Refused::Invalid(why)));
    }
    let answers = match ask(slskd, q).await {
        Ok(answers) => answers,
        Err(refused) => return Ok(Err(refused)),
    };
    let songs = song_candidates(pool, user_id, q, songs(&answers)).await?;
    let folders = crate::soulseek::candidates(&answers);
    let albums = albums
        .iter()
        .filter_map(|album| {
            let (users, formats) = crate::soulseek::shared(&folders, &album.title, &album.artist)?;
            Some(AlbumShared {
                mb_release_group_id: album.mb_release_group_id.clone(),
                detail: shared_by(users, &formats),
                users,
                formats,
            })
        })
        .collect();
    Ok(Ok(SoulseekFound { songs, albums }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_read_as_songs() {
        assert_eq!(
            song(r"Music\Radiohead\OK Computer\02 - Paranoid Android.flac"),
            Some((
                Some("Radiohead".into()),
                "Paranoid Android".into(),
                "FLAC".into()
            ))
        );
        assert_eq!(
            song("shared/singles/07. Radiohead - Creep.mp3"),
            Some((Some("Radiohead".into()), "Creep".into(), "MP3".into()))
        );
        assert_eq!(
            song("Radiohead - Kid A (2000)/01_Everything In Its Right Place.ogg"),
            Some((
                Some("Radiohead".into()),
                "Everything In Its Right Place".into(),
                "OGG".into()
            ))
        );
        assert_eq!(
            song("Lone.flac"),
            Some((None, "Lone".into(), "FLAC".into()))
        );
        assert_eq!(song("Radiohead/OK Computer/cover.jpg"), None);
        assert_eq!(song("Radiohead/OK Computer/01.flac"), None);
    }

    #[test]
    fn songs_are_counted_by_who_shares_them() {
        let answer = |user: &str, files: &[&str]| slskd::Response {
            username: user.into(),
            files: files
                .iter()
                .map(|f| slskd::File {
                    filename: (*f).into(),
                    size: 1,
                    bit_rate: None,
                    bit_depth: None,
                    sample_rate: None,
                    length: None,
                    is_variable_bit_rate: None,
                })
                .collect(),
            has_free_upload_slot: true,
            queue_length: 0,
            upload_speed: 0,
        };
        let found = songs(&[
            answer(
                "a",
                &[
                    r"Radiohead\OK Computer\02 Paranoid Android.flac",
                    r"Radiohead\OK Computer\02 Paranoid Android.mp3",
                ],
            ),
            answer(
                "b",
                &[
                    "Radiohead - Paranoid Android.mp3",
                    "Radiohead - Creep.mp3",
                    "notes.txt",
                ],
            ),
        ]);
        assert_eq!(
            found,
            [
                Heard {
                    title: "Paranoid Android".into(),
                    artist: Some("Radiohead".into()),
                    users: 2,
                    formats: vec!["FLAC".into(), "MP3".into()]
                },
                Heard {
                    title: "Creep".into(),
                    artist: Some("Radiohead".into()),
                    users: 1,
                    formats: vec!["MP3".into()]
                },
            ]
        );
    }
}
