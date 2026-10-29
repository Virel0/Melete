//! Subtitles from OpenSubtitles.com, off unless an API key is set (`MELETE_OPENSUBTITLES_KEY`);
//! the only code that talks to it.
//!
//! What leaves the server: the video's OpenSubtitles hash (a checksum of its size and its first
//! and last 64 KB, which can't be turned back into the video), its TMDB or IMDb id with the
//! season and episode, the language wanted, and which subtitle to download. Nothing about the
//! household or who asked. The key goes in a header and the account's login in a request body,
//! never in a URL, a log or an error; requests name only Melete and its version.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::{OpenSubtitles, Secret};

/// OpenSubtitles wants the app's name and version, and nothing else is sent.
const USER_AGENT: &str = concat!("Melete v", env!("CARGO_PKG_VERSION"));
/// Where OpenSubtitles' API is.
pub const API: &str = "https://api.opensubtitles.com/api/v1";
/// Answers bigger than this aren't read; a subtitle file is far smaller.
const MAX_ANSWER: usize = 5 << 20;
/// A login is good for a day; it's done again well before.
const LOGIN_KEPT: Duration = Duration::from_secs(12 * 3600);

/// The OpenSubtitles hash of a video: its size plus the sum of its first and last 64 KB read as
/// little-endian 64-bit words, as 16 hex digits. None for a file under 64 KB.
pub fn hash(path: &Path) -> std::io::Result<Option<String>> {
    const CHUNK: u64 = 64 * 1024;
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    if size < CHUNK {
        return Ok(None);
    }
    let mut sum = size;
    let mut buffer = vec![0u8; CHUNK as usize];
    for at in [0, size - CHUNK] {
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(&mut buffer)?;
        for word in buffer.as_chunks::<8>().0 {
            sum = sum.wrapping_add(u64::from_le_bytes(*word));
        }
    }
    Ok(Some(format!("{sum:016x}")))
}

/// Which video subtitles are wanted for.
#[derive(Debug, Clone, PartialEq)]
pub struct Wanted {
    /// The video's OpenSubtitles hash, when it could be worked out.
    pub hash: Option<String>,
    /// The movie's TMDB id, or the series' for an episode.
    pub tmdb_id: Option<i64>,
    /// The movie's IMDb id ("tt0133093"), or the series'.
    pub imdb_id: Option<String>,
    /// For an episode, its season and number.
    pub episode: Option<(i64, i64)>,
    /// The language wanted, as OpenSubtitles names it: "en", "pt-br".
    pub language: String,
}

/// A subtitle OpenSubtitles has for a video.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Found {
    /// The file to download: OpenSubtitles' file id.
    pub id: i64,
    /// The release it was made for, as the uploader named it.
    pub release: Option<String>,
    /// Its language, as OpenSubtitles names it.
    pub language: String,
    /// For the hard of hearing.
    pub hearing_impaired: bool,
    /// How often it's been downloaded.
    pub downloads: i64,
    /// Made for this very file: its hash matched.
    pub matches_file: bool,
}

/// Why OpenSubtitles couldn't help, in words for the person who asked.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// The key or the login was refused.
    Refused,
    /// The day's downloads are used up; they come back at this Unix time.
    Quota(i64),
    /// Anything else: no answer, a bad one.
    Other(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused => f.write_str("OpenSubtitles refused the API key or the login"),
            Self::Quota(_) => f.write_str("OpenSubtitles' downloads for today are used up"),
            Self::Other(why) => write!(f, "OpenSubtitles: {why}"),
        }
    }
}

impl std::error::Error for Failure {}

/// A login kept between requests: whose, the token, and when it was got.
static LOGIN: Mutex<Option<(String, Secret, Instant)>> = Mutex::new(None);

/// OpenSubtitles as Melete talks to it, with the household's key and, when set, its account.
pub struct Client {
    http: reqwest::Client,
    settings: OpenSubtitles,
    api: String,
}

mod for_imports;
mod keeping;
mod talking;

pub use for_imports::*;
pub use keeping::*;

#[cfg(test)]
pub mod testing;

#[cfg(test)]
mod tests;
