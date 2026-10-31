//! Looking on Soulseek for music MusicBrainz doesn't know: the words go to slskd as they were
//! typed, and nothing else. What its users share is read as songs, each named by its files
//! ("Artist - Title", or a title in an artist's folder), with how many users have it and in
//! which formats, so one can be wanted by its title and artist.

use std::collections::{BTreeSet, HashMap};

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

/// Looks on Soulseek for `q`, sending slskd only the words.
///
/// # Errors
///
/// The inner [`Refused`] when `q` isn't usable, slskd isn't set up, or it didn't answer.
pub async fn search_soulseek(
    slskd: Option<&slskd::Client>,
    q: &str,
) -> Result<Vec<Heard>, Refused> {
    let q = q.trim();
    if q.is_empty() || q.chars().count() > 200 {
        return Err(Refused::Invalid("q is 1 to 200 characters".into()));
    }
    let Some(slskd) = slskd else {
        return Err(Refused::Conflict(
            "Soulseek is searched through slskd, which isn't set up (MELETE_SLSKD_URL)".into(),
        ));
    };
    match slskd.search(q).await {
        Ok(answers) => Ok(songs(&answers)),
        Err(err) => Err(outside("slskd", err)),
    }
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
