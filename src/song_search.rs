//! Searching Soulseek for a wanted song. Each audio file a user
//! shares is one result. Its name is checked as an album's is (the artist, then the song's
//! title, then nothing but a year, a tag or an edition word), by the file's own name or by a
//! folder above it and the file's name ("Radiohead" / "OK Computer" / "01 Paranoid Android");
//! a leading track number is tried with and without. When the song's length is known, a file
//! more than a few seconds off is another version (a radio edit, a live take). Its quality is
//! read from the file and decided with the song's music profile.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::album_search::{AlbumDecision, mismatch_as};
use crate::db::unix_now;
use crate::library::AUDIO_EXTENSIONS;
use crate::music_profiles::{self, MusicProfile};
use crate::music_release::{self, MusicRelease};
use crate::search::Refused;
use crate::slskd::{self, File, Response};
use crate::soulseek::{self, Fetch, Outcome, Source};

/// Seconds a file's length may differ from the song's.
const LENGTH_SLACK: i64 = 4;
/// Results whose name isn't the song's are kept up to this many, to show why nothing fit.
const KEEP_MISFITS: usize = 25;
/// Searches are kept this long, as video's are.
const KEEP_SECONDS: i64 = 30 * 24 * 3600;

static TRACK_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\d{1,2}[-.])?\d{1,3}(?:\s*[-._)\]]\s*|\s+)")
        .expect("a literal pattern, valid as written")
});

/// A wanted song, as searched for.
#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct WantedSong {
    /// The wanted song's row id.
    pub id: i64,
    /// The song's title.
    pub title: String,
    /// Its artist.
    pub artist: String,
    /// The album it's on, for its folder and tags; none when it's wanted on its own.
    pub album: Option<String>,
    /// Its length in seconds, which tells a radio edit or a live take from it; none when unknown.
    pub length_seconds: Option<i64>,
    /// Its MusicBrainz recording id, when known.
    pub mb_recording_id: Option<String>,
    /// Its artist's MusicBrainz id, when known.
    pub mb_artist_id: Option<String>,
    /// The library it goes into; none when none is set, as after that library is removed.
    pub library_id: Option<i64>,
    /// The music profile its results are decided with; without one (as after it is removed)
    /// searches are refused.
    pub music_profile_id: Option<i64>,
    /// When it was added, in seconds since the epoch.
    pub added_at: i64,
}

/// The query reading [`WantedSong`] rows, for a `WHERE` or `ORDER BY` to finish.
pub const WANTED_SONG: &str = "SELECT id, title, artist, album, length_seconds, mb_recording_id,
        mb_artist_id, library_id, music_profile_id, added_at
     FROM wanted_songs";

/// One user's file.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The Soulseek user who has it.
    pub username: String,
    /// The file, as the user listed it.
    pub file: File,
    /// Whether the user has an upload slot free now.
    pub free_slot: bool,
    /// How many downloads the user has queued.
    pub queue_length: i64,
    /// The user's upload speed, in bytes a second.
    pub upload_speed: i64,
}

impl Candidate {
    /// What a grab fetches: the one file, from the folder it's in.
    pub fn source(&self) -> Source {
        let parts = soulseek::parts(&self.file.filename);
        let separator = if self.file.filename.contains('\\') {
            "\\"
        } else {
            "/"
        };
        Source {
            username: self.username.clone(),
            folder: parts[..parts.len().saturating_sub(1)].join(separator),
            files: vec![Fetch {
                filename: self.file.filename.clone(),
                size: self.file.size,
            }],
        }
    }
}

/// Every audio file in the answers, best placed to send first: a free upload slot, then the
/// shortest queue, then the fastest.
pub fn candidates(responses: &[Response]) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = responses
        .iter()
        .flat_map(|response| {
            response.files.iter().filter_map(move |file| {
                let parts = soulseek::parts(&file.filename);
                let name = parts.last()?;
                let audio = name.rsplit_once('.').is_some_and(|(_, e)| {
                    AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str())
                });
                // A Mac's hidden copy, or a sample.
                let skipped = parts.iter().any(|p| {
                    p.starts_with('.')
                        || ["sample", "samples", "__macosx"]
                            .contains(&p.to_ascii_lowercase().as_str())
                });
                (audio && !skipped).then(|| Candidate {
                    username: response.username.clone(),
                    file: file.clone(),
                    free_slot: response.has_free_upload_slot,
                    queue_length: response.queue_length,
                    upload_speed: response.upload_speed,
                })
            })
        })
        .collect();
    found.sort_by(|a, b| {
        b.free_slot
            .cmp(&a.free_slot)
            .then(a.queue_length.cmp(&b.queue_length))
            .then(b.upload_speed.cmp(&a.upload_speed))
    });
    found
}

/// The name a file is checked by, and why it isn't the song when it isn't.
fn name_checked(filename: &str, artist: &str, title: &str) -> (String, Option<String>) {
    let parts = soulseek::parts(filename);
    let Some((name, folders)) = parts.split_last() else {
        return (String::new(), Some("it has no name".into()));
    };
    let stem = name.rsplit_once('.').map_or(*name, |(s, _)| s);
    // Without a leading track number first, for a cleaner name; with it next, for a title
    // that starts with a number ("99 Problems").
    let mut stems = Vec::new();
    if let Some(found) = TRACK_NUMBER.find(stem)
        && found.end() < stem.len()
    {
        stems.push(stem[found.end()..].to_owned());
    }
    stems.push(stem.to_owned());
    // The folders above it, nearest first, leaving out share roots ("@@music").
    let folders: Vec<&str> = folders
        .iter()
        .rev()
        .filter(|f| !f.starts_with("@@"))
        .take(3)
        .copied()
        .collect();
    let mut names: Vec<String> = Vec::new();
    for stem in &stems {
        names.push(stem.clone());
        names.extend(folders.iter().map(|f| format!("{f} - {stem}")));
    }
    let mut tried: Vec<(String, String)> = Vec::new();
    for name in names {
        match mismatch_as(&name, artist, title, "song") {
            None => return (name, None),
            Some(why) => tried.push((name, why)),
        }
    }
    // A reason past the artist says more than one that never found it.
    let pick = tried
        .iter()
        .position(|(_, why)| !why.starts_with("it doesn't name the artist"))
        .unwrap_or(0);
    let (name, why) = tried.swap_remove(pick);
    (name, Some(why))
}

fn minutes(seconds: i64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// Decides every file for the song.
pub fn decide_all(
    candidates: &[Candidate],
    song: &WantedSong,
    profile: &MusicProfile,
) -> Vec<(String, AlbumDecision)> {
    candidates
        .iter()
        .map(|candidate| {
            let (name, why) = name_checked(&candidate.file.filename, &song.artist, &song.title);
            let parsed = music_release::parse(&name);
            let release = MusicRelease {
                quality: soulseek::track_quality(&candidate.file),
                year: parsed.year,
                source: parsed.source,
            };
            let decided = music_profiles::decide(profile, &release);
            let mut reasons: Vec<String> = why.into_iter().collect();
            if let (Some(wanted), Some(length)) = (song.length_seconds, candidate.file.length)
                && (wanted - length).abs() > LENGTH_SLACK
            {
                reasons.push(format!(
                    "it's {} long, not {}: another version",
                    minutes(length),
                    minutes(wanted)
                ));
            }
            reasons.extend(decided.reasons);
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

/// Searches Soulseek for a wanted song and stores what it found, decided on; returns the
/// search's id.
pub async fn search(
    pool: &SqlitePool,
    slskd: &slskd::Client,
    wanted_song_id: i64,
) -> anyhow::Result<Result<i64, Refused>> {
    let song: Option<WantedSong> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{WANTED_SONG} WHERE id = ?")))
            .bind(wanted_song_id)
            .fetch_optional(pool)
            .await?;
    let Some(song) = song else {
        return Ok(Err(Refused::NotWanted));
    };
    let profile = match song.music_profile_id {
        Some(id) => music_profiles::get(pool, id).await?,
        None => None,
    };
    let Some(profile) = profile else {
        return Ok(Err(Refused::NoProfile(
            "the song's music profile was removed; choose another".into(),
        )));
    };
    let answer = soulseek::search_loosely(slskd, &song.artist, &song.title).await;
    let (outcome, decided) = match answer {
        Ok(responses) => {
            let found = candidates(&responses);
            let decided = decide_all(&found, &song, &profile);
            let outcome = Outcome {
                results: found.len(),
                error: None,
            };
            (outcome, found.into_iter().zip(decided).collect())
        }
        Err(err) => (
            Outcome {
                results: 0,
                error: Some(format!("{err:#}")),
            },
            Vec::new(),
        ),
    };
    Ok(Ok(store(pool, &song, profile.id, &outcome, decided).await?))
}

/// Keeps a song search: every file whose name is the song's, and a few that aren't, to show
/// why nothing fit.
async fn store(
    pool: &SqlitePool,
    song: &WantedSong,
    profile_id: i64,
    outcome: &Outcome,
    decided: Vec<(Candidate, (String, AlbumDecision))>,
) -> anyhow::Result<i64> {
    let now = unix_now();
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM searches WHERE searched_at < ?")
        .bind(now - KEEP_SECONDS)
        .execute(&mut *tx)
        .await?;
    let search_id: i64 = sqlx::query_scalar(
        "INSERT INTO searches (wanted_song_id, music_profile_id, searched_at, indexers, soulseek)
         VALUES (?, ?, ?, '[]', ?) RETURNING id",
    )
    .bind(song.id)
    .bind(profile_id)
    .bind(now)
    .bind(serde_json::to_string(outcome)?)
    .fetch_one(&mut *tx)
    .await?;
    let mut misfits = 0;
    for (candidate, (name, decision)) in decided {
        let fits = !decision
            .reasons
            .first()
            .is_some_and(|r| r.starts_with("it doesn't name") || r.starts_with("it names another"));
        if !fits {
            misfits += 1;
            if misfits > KEEP_MISFITS {
                continue;
            }
        }
        sqlx::query(
            "INSERT INTO release_decisions (search_id, title, size, accepted, decision, soulseek)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(search_id)
        .bind(&name)
        .bind(candidate.file.size)
        .bind(decision.accepted)
        .bind(serde_json::to_string(&decision)?)
        .bind(serde_json::to_string(&candidate.source())?)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(search_id)
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

    fn answer(user: &str, slot: bool, files: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({
            "username": user, "hasFreeUploadSlot": slot, "queueLength": 0,
            "uploadSpeed": 100, "files": files,
        })
    }

    fn checked(path: &str, artist: &str, title: &str) -> (String, Option<String>) {
        name_checked(path, artist, title)
    }

    #[test]
    fn a_file_is_the_song_when_its_name_or_its_folders_say_so() {
        for (path, artist, title, name) in [
            (
                "@@m\\Radiohead\\OK Computer\\02 Paranoid Android.flac",
                "Radiohead",
                "Paranoid Android",
                "Radiohead - Paranoid Android",
            ),
            (
                "@@m/Singles/Radiohead - Paranoid Android.mp3",
                "Radiohead",
                "Paranoid Android",
                "Radiohead - Paranoid Android",
            ),
            (
                "@@m\\Jay-Z\\The Black Album\\04 99 Problems.flac",
                "Jay-Z",
                "99 Problems",
                "Jay-Z - 99 Problems",
            ),
            (
                "@@m\\Jay-Z\\The Black Album\\99 Problems.flac",
                "Jay-Z",
                "99 Problems",
                "Jay-Z - 99 Problems",
            ),
            (
                "@@m\\Radiohead\\OK Computer\\1-02 Paranoid Android (Remastered).flac",
                "Radiohead",
                "Paranoid Android",
                "Radiohead - Paranoid Android (Remastered)",
            ),
        ] {
            assert_eq!(
                checked(path, artist, title),
                (name.to_owned(), None),
                "{path}"
            );
        }
    }

    #[test]
    fn another_version_or_artist_isnt_the_song() {
        let (_, why) = checked(
            "@@m\\Radiohead\\Live\\Paranoid Android (Live).flac",
            "Radiohead",
            "Paranoid Android",
        );
        assert_eq!(
            why.as_deref(),
            Some("it names another song: \"Paranoid Android\" goes on with \"live\"")
        );
        let (_, why) = checked(
            "@@m\\Covers\\Paranoid Android.flac",
            "Radiohead",
            "Paranoid Android",
        );
        assert_eq!(
            why.as_deref(),
            Some("it doesn't name the artist (Radiohead)")
        );
        let (_, why) = checked(
            "@@m\\Radiohead\\Creep.flac",
            "Radiohead",
            "Paranoid Android",
        );
        assert_eq!(
            why.as_deref(),
            Some("it doesn't name the song (Paranoid Android)"),
            "past the artist, the more telling reason"
        );
    }

    #[test]
    fn keeps_audio_files_best_placed_first() {
        let responses: Vec<Response> = serde_json::from_value(serde_json::json!([
            answer(
                "bob",
                false,
                vec![file(
                    "@@b\\Radiohead\\01 Airbag.flac",
                    serde_json::json!({})
                )]
            ),
            answer(
                "alice",
                true,
                vec![
                    file("@@a\\Radiohead\\01 Airbag.mp3", serde_json::json!({})),
                    file("@@a\\Radiohead\\cover.jpg", serde_json::json!({})),
                    file("@@a\\Radiohead\\._01 Airbag.mp3", serde_json::json!({})),
                    file(
                        "@@a\\Radiohead\\Sample\\01 Airbag.mp3",
                        serde_json::json!({})
                    ),
                ]
            ),
        ]))
        .unwrap();
        let kept = candidates(&responses);
        let found: Vec<(&str, &str)> = kept
            .iter()
            .map(|c| (c.username.as_str(), c.file.filename.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                ("alice", "@@a\\Radiohead\\01 Airbag.mp3"),
                ("bob", "@@b\\Radiohead\\01 Airbag.flac"),
            ]
        );
        let source = candidates(&responses)[0].source();
        assert_eq!(source.folder, "@@a\\Radiohead");
        assert_eq!(source.files.len(), 1);
    }

    async fn wanted(pool: &SqlitePool, length: Option<i64>) -> WantedSong {
        let profile = music_profiles::lossless(pool).await.unwrap();
        sqlx::query_as(
            "INSERT INTO wanted_songs (title, artist, album, length_seconds, music_profile_id, source, added_at)
             VALUES ('Paranoid Android', 'Radiohead', 'OK Computer', ?, ?, 'manual', 0)
             RETURNING id, title, artist, album, length_seconds, mb_recording_id, mb_artist_id,
                 library_id, music_profile_id, added_at",
        )
        .bind(length)
        .bind(profile)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn decides_by_name_length_and_profile() {
        let pool = crate::db::open_in_memory().await;
        let song = wanted(&pool, Some(383)).await;
        let profile = music_profiles::get(&pool, song.music_profile_id.unwrap())
            .await
            .unwrap()
            .unwrap();
        let responses: Vec<Response> = serde_json::from_value(serde_json::json!([answer(
            "alice",
            true,
            vec![
                file(
                    "@@a\\Radiohead\\OK Computer\\02 Paranoid Android.flac",
                    serde_json::json!({"bitDepth": 16, "length": 385})
                ),
                file(
                    "@@a\\Radiohead\\Edits\\Paranoid Android.flac",
                    serde_json::json!({"bitDepth": 16, "length": 230})
                ),
                file(
                    "@@a\\Radiohead\\OK Computer\\02 Paranoid Android.mp3",
                    serde_json::json!({"bitRate": 128, "length": 383})
                ),
            ],
        )]))
        .unwrap();
        let found = candidates(&responses);
        let decided = decide_all(&found, &song, &profile);
        let summary: Vec<(bool, &str, String)> = decided
            .iter()
            .map(|(_, d)| (d.accepted, d.quality, d.reasons.join("; ")))
            .collect();
        assert_eq!(
            summary,
            [
                (true, "FLAC", String::new()),
                (
                    false,
                    "FLAC",
                    "it's 3:50 long, not 6:23: another version".into()
                ),
                (false, "MP3-128", "MP3-128 isn't in the profile".into()),
            ]
        );
    }

    #[tokio::test]
    async fn searches_soulseek_and_keeps_what_fits_and_a_few_that_dont() {
        let pool = crate::db::open_in_memory().await;
        let song = wanted(&pool, None).await;
        let mut files = vec![file(
            "@@a\\Radiohead\\OK Computer\\02 Paranoid Android.flac",
            serde_json::json!({"bitDepth": 16}),
        )];
        files.extend((0..40).map(|n| {
            file(
                &format!("@@a\\Radiohead\\Other\\{n:02} Track.flac"),
                serde_json::json!({"bitDepth": 16}),
            )
        }));
        let (slskd, seen) =
            crate::slskd::testing::start(serde_json::json!([answer("alice", true, files)]), 1)
                .await;
        let id = search(&pool, &slskd, song.id).await.unwrap().unwrap();
        assert_eq!(seen.lock().unwrap().texts, ["Radiohead Paranoid Android"]);
        let view = crate::search::get(&pool, id).await.unwrap().unwrap();
        assert_eq!(
            (
                view.wanted_song_id,
                view.wanted_album_id,
                view.music_profile_id
            ),
            (Some(song.id), None, song.music_profile_id)
        );
        assert_eq!(view.soulseek.as_ref().map(|o| o.results), Some(41));
        assert_eq!(view.releases.len(), 1 + KEEP_MISFITS);
        assert!(view.releases[0].accepted);
        assert_eq!(view.releases[0].title, "Radiohead - Paranoid Android");
        assert_eq!(view.releases[0].source, "soulseek");

        assert_eq!(
            search(&pool, &slskd, 999).await.unwrap(),
            Err(Refused::NotWanted)
        );
    }
}
