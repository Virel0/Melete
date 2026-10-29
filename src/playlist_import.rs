//! Playlists imported as song requests: a CSV export (Exportify's, with
//! the column names other exporters use too) whose every track is matched to a MusicBrainz
//! recording, by its ISRC first, then by title, artist and length, and asked for as a song by
//! whoever imported it. Imports run one at a time in the background, a track at a time, as
//! MusicBrainz answers one request a second; a track asked for at once (an admin's, or a user's
//! whose requests go straight through) is fetched before the next is matched. What can't be
//! matched is listed with why. Importing the same playlist again asks only for what's new.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::auth::User;
use crate::db::unix_now;
use crate::fulfil;
use crate::musicbrainz::{self, RecordingDetail};
use crate::requests::{self, Choice, NewRequest, Refused};

mod deciding;
mod matching;
mod reading;
mod records;
mod running;

pub use deciding::*;
pub use matching::*;
pub use reading::*;
pub use records::*;
pub use running::*;

/// Tracks one import may hold.
pub const MAX_TRACKS: usize = 5_000;
/// How long to wait before asking MusicBrainz again when it doesn't answer, and how often.
const RETRY: Duration = Duration::from_secs(60);
const TRIES: u32 = 3;
/// How often to look for imports to work on when there are none.
const IDLE: Duration = Duration::from_secs(5);
/// A track's length on MusicBrainz may differ from the playlist's by this much.
const LENGTH_SLACK_MS: i64 = 10_000;

/// A track as the playlist lists it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track {
    /// The track's title.
    pub title: String,
    /// Its artists, as the playlist lists them.
    pub artists: Vec<String>,
    /// Its album's title, when given.
    pub album: Option<String>,
    /// Its length in milliseconds, when given.
    pub length_ms: Option<i64>,
    /// Its ISRC, when given; matched on first.
    pub isrc: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth;
    use crate::db;
    use crate::musicbrainz::testing::{self, PARANOID_ANDROID, PARANOID_ISRC};

    #[test]
    fn reads_exportify_and_other_exporters_files() {
        let csv = "\u{feff}\"Track URI\",\"Track Name\",\"Artist Name(s)\",\"Album Name\",\"Track Duration (ms)\",\"ISRC\"\r\n\
                   \"spotify:track:1\",\"Paranoid Android\",\"Radiohead\",\"OK Computer\",\"383000\",\"gbaye9700245\"\r\n\
                   \r\n\
                   \"spotify:track:2\",\"Say \"\"Hi\"\"\",\"Tyler, The Creator,Kali Uchis\",\"\",\"\",\"\"\r\n\
                   \"spotify:track:3\",\"\",\"Nobody\",\"\",\"\",\"\"\r\n";
        let tracks = parse(csv).unwrap();
        assert_eq!(
            tracks,
            [
                Track {
                    title: "Paranoid Android".into(),
                    artists: vec!["Radiohead".into()],
                    album: Some("OK Computer".into()),
                    length_ms: Some(383_000),
                    isrc: Some("GBAYE9700245".into()),
                },
                Track {
                    title: "Say \"Hi\"".into(),
                    artists: vec!["Tyler".into(), "The Creator".into(), "Kali Uchis".into()],
                    ..Track::default()
                },
            ],
            "a row without a title is skipped"
        );
        // Another exporter's names, and lengths as m:ss.
        let other = parse("title,artist,duration\nCreep,Radiohead,3:58\n").unwrap();
        assert_eq!(
            (other[0].length_ms, other[0].artists.len()),
            (Some(238_000), 1)
        );
        assert_eq!(parse(""), Err("the file is empty".into()));
        assert_eq!(
            parse("Track Name\n"),
            Err("the file lists no tracks".into())
        );
        assert!(
            parse("Artist\nRadiohead\n")
                .unwrap_err()
                .contains("no column of track names")
        );
    }

    fn recording(title: &str, artist: &str, length: Option<i64>) -> RecordingDetail {
        serde_json::from_value(serde_json::json!({
            "id": format!("{title}/{artist}/{length:?}"), "title": title, "length": length,
            "artist-credit": [{"name": artist, "joinphrase": "", "artist": {"id": "a", "name": artist}}],
        }))
        .unwrap()
    }

    #[test]
    fn a_track_is_the_recording_that_fits_its_title_artist_and_length() {
        let track = Track {
            title: "Paranoid Android - Remastered 2017".into(),
            artists: vec!["Radiohead".into()],
            length_ms: Some(386_000),
            ..Track::default()
        };
        let found = [
            recording("Paranoid Android", "Radiohead", Some(383_000)),
            recording("Paranoid Android (live)", "Radiohead", Some(386_000)),
            recording("Paranoid Android", "A Tribute Band", Some(386_000)),
            recording("Paranoid Android", "Radiohead", Some(500_000)),
        ];
        assert_eq!(
            best(&track, &found, true).as_deref(),
            Some("Paranoid Android/Radiohead/Some(383000)"),
            "the same title by the same artist, close enough in length"
        );
        let comma = Track {
            title: "Earfquake".into(),
            artists: vec!["Tyler".into(), "The Creator".into()],
            ..Track::default()
        };
        assert!(
            best(
                &comma,
                &[recording("EARFQUAKE", "Tyler, The Creator", None)],
                true
            )
            .is_some()
        );
        assert_eq!(core_title("Song (feat. Someone) - Live"), "song");
    }

    async fn user(pool: &SqlitePool, name: &str, admin: bool) -> User {
        auth::create_user(pool, name, "a-long-password".into(), admin)
            .await
            .unwrap()
            .unwrap()
    }

    const PLAYLIST: &str = "Track Name,Artist Name(s),Track Duration (ms),ISRC\n\
                            Paranoid Android,Radiohead,383000,GBAYE9700245\n\
                            Karma Police,Radiohead,264000,\n\
                            Paranoid Android,Radiohead,,\n";

    #[tokio::test]
    async fn an_import_asks_for_each_track_it_can_match_and_lists_the_rest() {
        let pool = db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (4, 'music', 'music', '/music')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (alice, admin) = (
            user(&pool, "alice", false).await,
            user(&pool, "admin", true).await,
        );
        let (mb, log) = testing::start(0).await;
        let sources = fulfil::Sources::default();
        let made = create(&pool, &alice, "Road trip", &parse(PLAYLIST).unwrap())
            .await
            .unwrap();
        assert_eq!((made.state.as_str(), made.tracks), ("queued", 3));

        run(&pool, &mb, &sources, made.id).await.unwrap();
        let view = get(&pool, made.id).await.unwrap().unwrap();
        let s = &view.summary;
        assert_eq!(
            (s.state.as_str(), s.asked, s.already, s.unmatched, s.waiting),
            ("done", 1, 1, 1, 1)
        );
        assert_eq!(
            s.message.as_deref(),
            Some("1 asked for, 1 asked for already, 1 not found on MusicBrainz")
        );
        let items: Vec<(&str, Option<&str>)> = view
            .items
            .iter()
            .map(|t| (t.state.as_str(), t.why.as_deref()))
            .collect();
        assert_eq!(
            items,
            [
                ("asked", None),
                (
                    "unmatched",
                    Some("no recording on MusicBrainz has its title, artist and length")
                ),
                ("already", None),
            ]
        );
        assert!(
            log.lock()
                .unwrap()
                .contains(&format!("isrc {PARANOID_ISRC}")),
            "the ISRC first"
        );
        let asked = requests::get(&pool, view.items[0].request_id.unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                asked.kind.as_str(),
                asked.mb_recording_id.as_deref(),
                asked.state.as_str()
            ),
            ("song", Some(PARANOID_ANDROID), "requested")
        );
        assert_eq!(view.items[2].request_id, Some(asked.id));

        // An admin approves the whole playlist at once.
        let (approved, refused) = approve_all(&pool, Some(&mb), &sources, made.id, &admin)
            .await
            .unwrap();
        assert_eq!((approved, refused), (vec![asked.id], vec![]));
        assert_eq!(summary(&pool, made.id).await.unwrap().unwrap().waiting, 0);

        // Imported again: nothing new to ask for.
        let again = create(&pool, &alice, "Road trip", &parse(PLAYLIST).unwrap())
            .await
            .unwrap();
        run(&pool, &mb, &sources, again.id).await.unwrap();
        let again = summary(&pool, again.id).await.unwrap().unwrap();
        assert_eq!((again.asked, again.already, again.unmatched), (0, 2, 1));
        assert_eq!(list(&pool, Some(alice.id)).await.unwrap().len(), 2);
        assert!(list(&pool, Some(admin.id)).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_let_through_users_tracks_are_approved_as_theyre_asked_for() {
        let pool = db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (4, 'music', 'music', '/music')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let bob = user(&pool, "bob", false).await;
        sqlx::query("UPDATE users SET requests_approved = 1 WHERE id = ?")
            .bind(bob.id)
            .execute(&pool)
            .await
            .unwrap();
        let (mb, _) = testing::start(0).await;
        let made = create(&pool, &bob, "Mine", &parse(PLAYLIST).unwrap())
            .await
            .unwrap();
        run(&pool, &mb, &fulfil::Sources::default(), made.id)
            .await
            .unwrap();
        let view = get(&pool, made.id).await.unwrap().unwrap();
        let asked = requests::get(&pool, view.items[0].request_id.unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(asked.state, "approved");
        assert_eq!(view.summary.waiting, 0);
    }
}
