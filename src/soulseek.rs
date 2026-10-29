//! Soulseek answers as album releases. Each user's album folder is
//! one result: its tracks (a CD1 or Disc 2 folder counts as part of its parent) and a cover
//! image beside them. Its quality is read from the files, which say what they are, rather than
//! from its name; its name is checked against the album as a torrent's is; and a folder with
//! clearly fewer tracks than another user's copy is taken for a part of the album, not all of
//! it.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::album_search::{AlbumDecision, WantedAlbum, mismatch};
use crate::library::{AUDIO_EXTENSIONS, COVER_NAMES, IMAGE_EXTENSIONS};
use crate::music_profiles::{self, MusicProfile};
use crate::music_release::{self, MusicRelease};
use crate::slskd::{File, Response};

/// A folder with fewer tracks than this share of the fullest copy found is taken for a part.
const COMPLETE_SHARE: f64 = 0.8;

static DISC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(cd|dis[ck])\s*[-_.]?\s*\d{1,2}\b")
        .expect("a literal pattern, valid as written")
});

/// One user's album folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The Soulseek user who has it.
    pub username: String,
    /// The album folder on the user's machine, as slskd gave it.
    pub folder: String,
    /// The folder's own name, and its parent's when it has one.
    pub name: String,
    /// The parent folder's name, for a disc folder (`CD1`) whose album name is its parent's.
    pub parent: Option<String>,
    /// What would be fetched: the tracks, and a cover image beside them.
    pub files: Vec<Fetch>,
    /// The audio files that make the album, as the user listed them.
    pub tracks: Vec<File>,
    /// Whether the user has an upload slot free now.
    pub free_slot: bool,
    /// How many downloads the user has queued.
    pub queue_length: i64,
    /// The user's upload speed, in bytes a second.
    pub upload_speed: i64,
}

/// A file to fetch, as slskd's download request takes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fetch {
    /// The path on the peer's machine.
    pub filename: String,
    /// Its size in bytes.
    pub size: i64,
}

/// What's stored with a Soulseek result: whom to fetch what from. Peers' names aren't shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// The Soulseek user to fetch from.
    pub username: String,
    /// The album folder on the user's machine.
    pub folder: String,
    /// The files to fetch: the tracks, and a cover image when there is one.
    pub files: Vec<Fetch>,
}

impl Candidate {
    /// The bytes to fetch, tracks and cover together.
    pub fn size(&self) -> i64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// What a grab stores to fetch this folder later.
    pub fn source(&self) -> Source {
        Source {
            username: self.username.clone(),
            folder: self.folder.clone(),
            files: self.files.clone(),
        }
    }
}

/// How asking slskd went, as a search keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    /// Album folders found.
    pub results: usize,
    /// Why asking slskd failed, when it did; none when it answered.
    pub error: Option<String>,
}

/// What asking slskd for an album gave: how it went, and each folder with the name it was
/// checked by and its decision.
pub struct Found {
    /// How asking went.
    pub outcome: Outcome,
    /// Each album folder found, with the name it was checked by and its decision.
    pub results: Vec<(Candidate, String, AlbumDecision)>,
}

/// Decides slskd's answer for `album`.
pub fn found(
    answer: anyhow::Result<Vec<Response>>,
    album: &WantedAlbum,
    profile: &MusicProfile,
) -> Found {
    match answer {
        Ok(responses) => {
            let candidates = candidates(&responses);
            let decided = decide_all(&candidates, &album.title, &album.artist, profile);
            Found {
                outcome: Outcome {
                    results: candidates.len(),
                    error: None,
                },
                results: candidates
                    .into_iter()
                    .zip(decided)
                    .map(|(c, (name, decision))| (c, name, decision))
                    .collect(),
            }
        }
        Err(err) => Found {
            outcome: Outcome {
                results: 0,
                error: Some(format!("{err:#}")),
            },
            results: Vec::new(),
        },
    }
}

/// What Soulseek is asked: the artist and the album, as plain words. A leading "-" would
/// exclude a word, and punctuation only narrows what matches.
pub fn search_text(artist: &str, album: &str) -> String {
    format!("{artist} {album}")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// A path's parts, split at whichever separator it uses.
pub(crate) fn parts(path: &str) -> Vec<&str> {
    let separator = if path.contains('\\') { '\\' } else { '/' };
    path.split(separator).filter(|p| !p.is_empty()).collect()
}

/// Every user's album folders, best placed to send first: a free upload slot, then the
/// shortest queue, then the fastest.
pub fn candidates(responses: &[Response]) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = Vec::new();
    for response in responses {
        // Keyed by the album folder's parts; disc folders fold into their parent.
        let mut folders: BTreeMap<Vec<&str>, (Vec<File>, Vec<Fetch>)> = BTreeMap::new();
        let mut covers: Vec<(Vec<&str>, &File)> = Vec::new();
        for file in &response.files {
            let parts = parts(&file.filename);
            let Some((name, mut folder)) = parts.split_last().map(|(n, f)| (*n, f.to_vec())) else {
                continue;
            };
            let extension = extension(name);
            if AUDIO_EXTENSIONS.contains(&extension.as_str()) {
                if folder.len() > 1 && folder.last().is_some_and(|f| DISC.is_match(f)) {
                    folder.pop();
                }
                let entry = folders.entry(folder).or_default();
                entry.0.push(file.clone());
                entry.1.push(Fetch {
                    filename: file.filename.clone(),
                    size: file.size,
                });
            } else if IMAGE_EXTENSIONS.contains(&extension.as_str()) {
                let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
                if COVER_NAMES.contains(&stem.to_ascii_lowercase().as_str()) {
                    covers.push((folder, file));
                }
            }
        }
        for (folder, (tracks, mut files)) in folders {
            let Some(name) = folder.last() else { continue };
            // The first cover name in order of preference, beside the tracks.
            let cover = COVER_NAMES.iter().find_map(|wanted| {
                covers.iter().find(|(f, file)| {
                    *f == folder
                        && parts(&file.filename)
                            .last()
                            .and_then(|n| n.rsplit_once('.'))
                            .is_some_and(|(stem, _)| stem.eq_ignore_ascii_case(wanted))
                })
            });
            if let Some((_, file)) = cover {
                files.push(Fetch {
                    filename: file.filename.clone(),
                    size: file.size,
                });
            }
            let separator = if response.files[0].filename.contains('\\') {
                "\\"
            } else {
                "/"
            };
            found.push(Candidate {
                username: response.username.clone(),
                folder: folder.join(separator),
                name: (*name).to_owned(),
                parent: folder
                    .len()
                    .checked_sub(2)
                    .map(|i| folder[i].to_owned())
                    .filter(|p| !p.starts_with("@@")),
                files,
                tracks,
                free_slot: response.has_free_upload_slot,
                queue_length: response.queue_length,
                upload_speed: response.upload_speed,
            });
        }
    }
    found.sort_by(|a, b| {
        b.free_slot
            .cmp(&a.free_slot)
            .then(a.queue_length.cmp(&b.queue_length))
            .then(b.upload_speed.cmp(&a.upload_speed))
    });
    found
}

/// One track's quality, by Lidarr's names, from what its file says.
pub(crate) fn track_quality(file: &File) -> &'static str {
    let name = parts(&file.filename).last().copied().unwrap_or_default();
    let bitrate = file.bit_rate.unwrap_or(0);
    let hi_res = file.bit_depth.is_some_and(|d| d >= 24);
    match extension(name).as_str() {
        "flac" if hi_res => "FLAC 24bit",
        "flac" => "FLAC",
        // An m4a is ALAC when it has a bit depth, which lossy files don't.
        "m4a" if hi_res => "ALAC 24bit",
        "m4a" if file.bit_depth.is_some() => "ALAC",
        "m4a" | "aac" if file.is_variable_bit_rate == Some(true) => "AAC-VBR",
        "m4a" | "aac" if bitrate >= 315 => "AAC-320",
        "m4a" | "aac" if bitrate >= 250 => "AAC-256",
        "m4a" | "aac" if bitrate >= 188 => "AAC-192",
        "mp3" if file.is_variable_bit_rate == Some(true) => match bitrate {
            220.. => "MP3-VBR-V0",
            170.. => "MP3-VBR-V2",
            _ => "Unknown",
        },
        "mp3" => match bitrate {
            315.. => "MP3-320",
            250.. => "MP3-256",
            220.. => "MP3-224",
            188.. => "MP3-192",
            156.. => "MP3-160",
            125.. => "MP3-128",
            _ => "Unknown",
        },
        // Vorbis files don't say their level; the lowest is the safe reading.
        "ogg" | "oga" => "OGG Vorbis Q5",
        "wav" => "WAV",
        "wv" => "WavPack",
        "ape" => "APE",
        _ => "Unknown",
    }
}

/// The folder's quality: its tracks' when they share one (FLAC with some 24-bit tracks counts
/// as FLAC), else None.
fn quality(tracks: &[File]) -> Option<&'static str> {
    let mut qualities: Vec<&'static str> = tracks.iter().map(track_quality).collect();
    qualities.sort_unstable();
    qualities.dedup();
    match qualities.as_slice() {
        [one] => Some(one),
        ["FLAC", "FLAC 24bit"] => Some("FLAC"),
        ["ALAC", "ALAC 24bit"] => Some("ALAC"),
        _ => None,
    }
}

/// The name a folder is checked by: its own when that names the album ("Radiohead - OK
/// Computer (1997)"), else its parent's and its own ("Radiohead" / "OK Computer").
fn name_checked(candidate: &Candidate, artist: &str, album: &str) -> (String, Option<String>) {
    let own = candidate.name.clone();
    if mismatch(&own, artist, album).is_none() {
        return (own, None);
    }
    let Some(parent) = &candidate.parent else {
        let why = mismatch(&own, artist, album);
        return (own, why);
    };
    let joined = format!("{parent} - {own}");
    let why = mismatch(&joined, artist, album);
    (joined, why)
}

/// Decides every folder for the album. A folder whose name fits but holds fewer than
/// [`COMPLETE_SHARE`] of the tracks of the fullest fitting copy is refused as partial.
pub fn decide_all(
    candidates: &[Candidate],
    album: &str,
    artist: &str,
    profile: &MusicProfile,
) -> Vec<(String, AlbumDecision)> {
    let named: Vec<(String, Option<String>)> = candidates
        .iter()
        .map(|c| name_checked(c, artist, album))
        .collect();
    let fullest = candidates
        .iter()
        .zip(&named)
        .filter(|(_, (_, why))| why.is_none())
        .map(|(c, _)| c.tracks.len())
        .max()
        .unwrap_or(0);
    candidates
        .iter()
        .zip(named)
        .map(|(candidate, (name, why))| {
            let parsed = music_release::parse(&name);
            let quality = quality(&candidate.tracks);
            let release = MusicRelease {
                quality: quality.unwrap_or("Unknown"),
                year: parsed.year,
                source: parsed.source,
            };
            let decided = music_profiles::decide(profile, &release);
            let mut reasons: Vec<String> = why.into_iter().collect();
            let tracks = candidate.tracks.len();
            if reasons.is_empty() && (tracks as f64) < fullest as f64 * COMPLETE_SHARE {
                reasons.push(format!(
                    "it looks like part of the album: {tracks} tracks, where another copy has {fullest}"
                ));
            }
            if quality.is_none() {
                reasons.push("its tracks aren't all the same format".into());
            } else {
                reasons.extend(decided.reasons);
            }
            let decision = AlbumDecision {
                accepted: reasons.is_empty(),
                quality: release.quality,
                rank: decided.rank,
                year: release.year,
                source: release.source,
                reasons,
            };
            (name, decision)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut file = serde_json::json!({"filename": path, "size": 1000});
        file.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        file
    }

    fn response(user: &str, slot: bool, queue: i64, files: Vec<serde_json::Value>) -> Response {
        serde_json::from_value(serde_json::json!({
            "username": user, "hasFreeUploadSlot": slot, "queueLength": queue,
            "uploadSpeed": 100, "files": files,
        }))
        .unwrap()
    }

    fn flac(path: &str) -> serde_json::Value {
        file(
            path,
            serde_json::json!({"bitDepth": 16, "sampleRate": 44100}),
        )
    }

    #[test]
    fn the_search_text_is_plain_words() {
        assert_eq!(
            search_text("AC/DC", "Back in Black (Remastered) -Live"),
            "AC DC Back in Black Remastered Live"
        );
        assert_eq!(search_text("Sigur Rós", "( )"), "Sigur Rós");
    }

    #[test]
    fn groups_each_users_files_into_album_folders() {
        let found = candidates(&[
            response(
                "bob",
                false,
                5,
                vec![flac("@@lossless/Radiohead/OK Computer/01 Airbag.flac")],
            ),
            response(
                "alice",
                true,
                0,
                vec![
                    flac("@@music\\Radiohead\\OK Computer (1997)\\CD1\\01 Airbag.flac"),
                    flac("@@music\\Radiohead\\OK Computer (1997)\\Disc 2\\01 Lucky.flac"),
                    file(
                        "@@music\\Radiohead\\OK Computer (1997)\\back.jpg",
                        serde_json::json!({}),
                    ),
                    file(
                        "@@music\\Radiohead\\OK Computer (1997)\\Folder.JPG",
                        serde_json::json!({}),
                    ),
                    file(
                        "@@music\\Radiohead\\OK Computer (1997)\\cover.png",
                        serde_json::json!({}),
                    ),
                    file(
                        "@@music\\Radiohead\\OK Computer (1997)\\rip.log",
                        serde_json::json!({}),
                    ),
                    flac("@@music\\Radiohead\\Kid A\\01 Everything.flac"),
                ],
            ),
        ]);
        let summary: Vec<(&str, &str, Option<&str>, usize, usize)> = found
            .iter()
            .map(|c| {
                (
                    c.username.as_str(),
                    c.name.as_str(),
                    c.parent.as_deref(),
                    c.tracks.len(),
                    c.files.len(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("alice", "Kid A", Some("Radiohead"), 1, 1),
                ("alice", "OK Computer (1997)", Some("Radiohead"), 2, 3),
                ("bob", "OK Computer", Some("Radiohead"), 1, 1),
            ],
            "a free slot first; discs folded in; the preferred cover only"
        );
        let ok = &found[1];
        assert_eq!(ok.folder, "@@music\\Radiohead\\OK Computer (1997)");
        assert_eq!(
            ok.files.last().unwrap().filename,
            "@@music\\Radiohead\\OK Computer (1997)\\cover.png"
        );
        assert_eq!(ok.size(), 3000);
    }

    #[test]
    fn reads_the_quality_from_the_files() {
        let one = |extra: serde_json::Value, name: &str| -> &'static str {
            track_quality(&serde_json::from_value(file(name, extra)).unwrap())
        };
        use serde_json::json;
        assert_eq!(one(json!({"bitDepth": 16}), "a.flac"), "FLAC");
        assert_eq!(one(json!({"bitDepth": 24}), "a.FLAC"), "FLAC 24bit");
        assert_eq!(one(json!({}), "a.flac"), "FLAC");
        assert_eq!(one(json!({"bitDepth": 16}), "a.m4a"), "ALAC");
        assert_eq!(one(json!({"bitRate": 256}), "a.m4a"), "AAC-256");
        assert_eq!(one(json!({"bitRate": 320}), "a.mp3"), "MP3-320");
        assert_eq!(
            one(json!({"bitRate": 245, "isVariableBitRate": true}), "a.mp3"),
            "MP3-VBR-V0"
        );
        assert_eq!(
            one(json!({"bitRate": 190, "isVariableBitRate": true}), "a.mp3"),
            "MP3-VBR-V2"
        );
        assert_eq!(one(json!({"bitRate": 192}), "a.mp3"), "MP3-192");
        assert_eq!(
            one(json!({}), "a.mp3"),
            "Unknown",
            "an MP3 that doesn't say"
        );
        assert_eq!(one(json!({}), "a.opus"), "Unknown");
        let tracks = |names: &[(&str, i64)]| -> Vec<File> {
            names
                .iter()
                .map(|(n, depth)| {
                    serde_json::from_value(file(n, json!({"bitDepth": depth}))).unwrap()
                })
                .collect()
        };
        assert_eq!(
            quality(&tracks(&[("1.flac", 16), ("2.flac", 24)])),
            Some("FLAC")
        );
        assert_eq!(
            quality(&tracks(&[("1.flac", 24), ("2.flac", 24)])),
            Some("FLAC 24bit")
        );
        assert_eq!(quality(&tracks(&[("1.flac", 16), ("2.mp3", 0)])), None);
    }

    #[tokio::test]
    async fn decides_by_name_completeness_and_profile() {
        let pool = crate::db::open_in_memory().await;
        let id = music_profiles::lossless(&pool).await.unwrap();
        let profile = music_profiles::get(&pool, id).await.unwrap().unwrap();
        let mut whole: Vec<serde_json::Value> = (1..=12)
            .map(|n| flac(&format!("@@m/Radiohead/OK Computer (1997)/{n:02}.flac")))
            .collect();
        whole.push(flac("@@m/Radiohead - OK Computer [FLAC]/01.flac"));
        let partial: Vec<serde_json::Value> = (1..=3)
            .map(|n| flac(&format!("@@x/Radiohead - OK Computer/{n:02}.flac")))
            .collect();
        let lossy: Vec<serde_json::Value> = (1..=12)
            .map(|n| {
                file(
                    &format!("@@y/Radiohead/OK Computer/{n:02}.mp3"),
                    serde_json::json!({"bitRate": 320}),
                )
            })
            .collect();
        let other = vec![flac("@@z/Radiohead/Kid A/01.flac")];
        let found = candidates(&[
            response("alice", true, 0, whole),
            response("bob", true, 1, partial),
            response("carol", true, 2, lossy),
            response("dave", true, 3, other),
        ]);
        let decided = decide_all(&found, "OK Computer", "Radiohead", &profile);
        let summary: Vec<(&str, bool, &str, String)> = found
            .iter()
            .zip(&decided)
            .map(|(c, (name, d))| {
                (
                    c.username.as_str(),
                    d.accepted,
                    d.quality,
                    format!("{name} | {}", d.reasons.join("; ")),
                )
            })
            .collect();
        assert_eq!(
            summary[0],
            (
                "alice",
                true,
                "FLAC",
                "Radiohead - OK Computer (1997) | ".into()
            )
        );
        assert_eq!(
            summary[1],
            ("alice", false, "FLAC", "Radiohead - OK Computer [FLAC] | it looks like part of the album: 1 tracks, where another copy has 12".into())
        );
        assert_eq!(
            summary[2],
            ("bob", false, "FLAC", "Radiohead - OK Computer | it looks like part of the album: 3 tracks, where another copy has 12".into())
        );
        // The preset takes MP3-320 too, ranked below FLAC.
        assert_eq!(
            summary[3],
            (
                "carol",
                true,
                "MP3-320",
                "Radiohead - OK Computer | ".into()
            )
        );
        assert!(decided[3].1.rank < decided[0].1.rank);
        assert!(!summary[4].1);
        assert!(summary[4].3.contains("Kid A"), "{:?}", summary[4]);
        assert_eq!(decided[0].1.year, Some(1997));
    }
}
