//! Lyrics on the server, so the phone and the desktop don't each look them up: an `.lrc`
//! file beside the track first, then the track's own tags, then LRCLIB when lyrics are turned
//! on, asked once per track and kept. Synced lyrics come as lines with their
//! times; plain ones as lines without. Library files are only read, never written.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::lrclib;

mod finding;
mod lrc;
mod reading;
mod searching;
mod translating;

pub use finding::*;
pub use lrc::*;
pub use reading::*;
pub use searching::*;
pub use translating::*;

/// How long LRCLIB having nothing for a track holds before it's asked again.
pub const RETRY_AFTER_SECONDS: i64 = 30 * 86_400;
/// Lyrics are text; a bigger sidecar isn't lyrics.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Where a track's lyrics were found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// An `.lrc` file beside the track, with its name.
    File,
    /// The track's own tags.
    Tags,
    /// LRCLIB, when lyrics are fetched.
    Lrclib,
}

impl Source {
    /// Its name, as stored and served: `file`, `tags` or `lrclib`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Tags => "tags",
            Self::Lrclib => "lrclib",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "file" => Some(Self::File),
            "tags" => Some(Self::Tags),
            "lrclib" => Some(Self::Lrclib),
            _ => None,
        }
    }
}

/// A track's lyrics: LRC text when synced, plain lines otherwise.
#[derive(Debug, Clone, PartialEq)]
pub struct Lyrics {
    /// Where they were found.
    pub source: Source,
    /// Whether they're LRC text, timed line by line.
    pub synced: bool,
    /// The lyrics as kept: LRC text when synced, else plain lines with Unix line ends.
    pub text: String,
}

/// One line as the apps show it: when it starts (synced lyrics only), and its words.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// When the line starts, in seconds into the track; none for plain lyrics.
    pub start: Option<f64>,
    /// The line's words.
    pub text: String,
}

impl Lyrics {
    /// Text found locally or from LRCLIB, as kept: synced when it's LRC, else plain.
    fn from_text(source: Source, text: &str) -> Option<Self> {
        let text = text.trim_start_matches('\u{feff}');
        if let Some(lrc) = lrclib::lrc(text) {
            return Some(Self {
                source,
                synced: true,
                text: lrc,
            });
        }
        let plain = text.replace("\r\n", "\n").trim().to_owned();
        (!plain.is_empty()).then_some(Self {
            source,
            synced: false,
            text: plain,
        })
    }

    /// The lyrics line by line: timed for synced lyrics, else each plain line with no time.
    pub fn lines(&self) -> Vec<Line> {
        if self.synced {
            lrc_lines(&self.text)
        } else {
            self.text
                .lines()
                .map(|line| Line {
                    start: None,
                    text: line.trim_end().to_owned(),
                })
                .collect()
        }
    }
}

/// What a request for a track's lyrics finds.
#[derive(Debug, PartialEq)]
pub enum Answer {
    /// No present track has that id.
    NoTrack,
    /// The track has no lyrics anywhere asked.
    Nothing,
    /// Its lyrics.
    Found(Lyrics),
}

/// Why lyrics couldn't be found or translated.
#[derive(Debug)]
pub enum Error {
    /// LRCLIB couldn't be asked, or answered wrongly; nothing was kept.
    Lrclib(anyhow::Error),
    /// LibreTranslate couldn't be asked, or answered wrongly; nothing was kept.
    Translation(anyhow::Error),
    /// The database couldn't be read or written.
    Database(sqlx::Error),
}

impl From<sqlx::Error> for Error {
    fn from(err: sqlx::Error) -> Self {
        Self::Database(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(start: f64, text: &str) -> Line {
        Line {
            start: Some(start),
            text: text.into(),
        }
    }

    #[test]
    fn reads_lrc_into_timed_lines() {
        let lines = lrc_lines(
            "[ar: Alice]\n[offset:+500]\n[00:10.00][01:00.00]Chorus\n[00:01.50]<00:01.50>First \
             <00:02.00>line\n[00:05:250]\n",
        );
        assert_eq!(
            lines,
            [
                line(1.0, "First line"),
                line(4.75, ""),
                line(9.5, "Chorus"),
                line(59.5, "Chorus"),
            ]
        );
        assert_eq!(timestamp("1:02"), Some(62.0));
        assert_eq!(timestamp("ar: x"), None);
        assert_eq!(timestamp("00:61"), None);
    }

    #[test]
    fn words_become_one_phrase_ending_in_a_prefix() {
        assert_eq!(
            phrase(" next \"world\" w ").as_deref(),
            Some("\"next world w\" *")
        );
        assert_eq!(phrase("don't stop").as_deref(), Some("\"don't stop\" *"));
        assert_eq!(
            phrase("NEAR(a b) OR c*").as_deref(),
            Some("\"NEAR a b OR c\" *")
        );
        assert_eq!(phrase(" - * "), None);
    }

    async fn search_texts(pool: &SqlitePool, words: &str) -> Vec<(i64, Option<f64>, String)> {
        search(pool, words, 10, 0)
            .await
            .unwrap()
            .1
            .into_iter()
            .map(|h| (h.track_id, h.start, h.text))
            .collect()
    }

    #[tokio::test]
    async fn reads_the_librarys_lyrics_into_the_search_once_and_again_when_changed() {
        let pool = crate::db::open_in_memory().await;
        let library = tempfile::tempdir().unwrap();
        for name in ["1.flac", "2.flac", "3.flac"] {
            std::fs::write(library.path().join(name), b"not audio").unwrap();
        }
        let lrc = library.path().join("1.lrc");
        std::fs::write(
            &lrc,
            "[00:01.00]In the next world war\n[00:03.00]In a jackknifed juggernaut\n\
             [00:09.00]In the next world war\n",
        )
        .unwrap();
        std::fs::write(
            library.path().join("2.lrc"),
            "Café society\nis where we meet\n",
        )
        .unwrap();
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', ?)")
            .bind(library.path().to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(
            r#"INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                   seen_generation) VALUES (1, 1, 'k', 'Album', 'Alice', '', 1);
               INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                   first_seen_generation, seen_generation, album_id, tagged, title, genres)
                   VALUES (1, 1, '1.flac', 1, 1, 1, 1, 1, 1, 'One', '[]'),
                          (2, 1, '2.flac', 1, 1, 1, 1, 1, 1, 'Two', '[]'),
                          (3, 1, '3.flac', 1, 1, 1, 1, 1, 1, 'Three', '[]');"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            read_library(&pool).await.unwrap(),
            Report { found: 2, gone: 0 }
        );
        assert_eq!(
            search_texts(&pool, "next WORLD w").await,
            [(1, Some(1.0), "In the next world war".to_owned())],
            "each track once, at its first matching line"
        );
        assert_eq!(
            search_texts(&pool, "cafe society").await,
            [(2, None, "Café society".to_owned())],
            "accents folded; plain lyrics have no start"
        );
        assert!(
            search_texts(&pool, "world next").await.is_empty(),
            "in order"
        );
        assert_eq!(
            read_library(&pool).await.unwrap(),
            Report::default(),
            "nothing changed, nothing read"
        );

        // An edited .lrc is read again; one taken away takes its lyrics with it.
        std::fs::write(&lrc, "[00:02.00]Airbag saved my life\n").unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&lrc)
            .unwrap()
            .set_modified(later)
            .unwrap();
        std::fs::remove_file(library.path().join("2.lrc")).unwrap();
        assert_eq!(
            read_library(&pool).await.unwrap(),
            Report { found: 1, gone: 1 }
        );
        assert!(search_texts(&pool, "next world").await.is_empty());
        assert_eq!(search_texts(&pool, "saved").await[0].0, 1);
        assert!(search_texts(&pool, "cafe").await.is_empty());

        // A library that can't be reached is left as it was.
        let moved = library.path().with_extension("moved");
        std::fs::rename(library.path(), &moved).unwrap();
        assert_eq!(read_library(&pool).await.unwrap(), Report::default());
        assert_eq!(search_texts(&pool, "airbag").await[0].0, 1);
        std::fs::rename(&moved, library.path()).unwrap();
        // A track gone from the library isn't found.
        sqlx::query("UPDATE music_tracks SET missing_since = 1 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        assert!(search_texts(&pool, "airbag").await.is_empty());
    }

    #[test]
    fn keeps_lrc_as_synced_and_anything_else_as_plain() {
        let synced = Lyrics::from_text(Source::File, "\u{feff}[00:01.00]One\r\n").unwrap();
        assert!(synced.synced);
        assert_eq!(synced.lines(), [line(1.0, "One")]);
        let plain = Lyrics::from_text(Source::Tags, "\n One\r\n\r\nTwo  \n").unwrap();
        assert!(!plain.synced);
        let texts: Vec<String> = plain.lines().into_iter().map(|l| l.text).collect();
        assert_eq!(texts, ["One", "", "Two"]);
        assert_eq!(Lyrics::from_text(Source::Tags, " \n "), None);
    }
}
