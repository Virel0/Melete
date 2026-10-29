//! Text subtitles for the apps: embedded tracks and files beside the video, as WebVTT.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tokio::sync::Semaphore;

mod attached_fonts;
mod beside;
mod caching;
mod charset;
mod converting;

pub use attached_fonts::*;
pub use beside::*;
pub use caching::*;
pub use charset::*;
pub use converting::*;

const SIDECAR_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "vtt"];
/// Converting an embedded track reads the whole file, which over a NAS can take minutes.
const CONVERT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Conversions at once.
static CONVERSIONS: Semaphore = Semaphore::const_new(2);
/// How long to wait for one of those before answering "busy".
const SLOT_WAIT: Duration = Duration::from_secs(30);
/// A failed conversion isn't tried again for this long.
const RETRY_FAILED_AFTER: Duration = Duration::from_secs(10 * 60);
/// One lock per output file, so requests for the same track share a single conversion.
static IN_FLIGHT: LazyLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Mutex::default);

/// Both conversion slots stayed busy; worth trying again shortly.
#[derive(Debug)]
pub struct Busy;

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("subtitle conversions are busy; try again shortly")
    }
}

impl std::error::Error for Busy {}

/// Where a subtitle track comes from.
pub enum Source<'a> {
    /// A text stream inside the video, by stream index.
    Embedded {
        video: &'a Path,
        index: i64,
    },
    Sidecar(&'a Path),
}

/// What to convert a track to.
#[derive(Debug, Clone, Copy)]
pub enum Format {
    /// For the apps.
    WebVtt,
    /// For burning in: keeps styling, which matters for anime.
    Ass,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Self::WebVtt => "vtt",
            Self::Ass => "ass",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"1\n00:00:00,000 --> 00:00:01,000\nHi\n").unwrap();
    }

    #[test]
    fn finds_sidecars_with_language_and_flags() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("Movie (2001).mkv");
        std::fs::write(&video, b"").unwrap();
        for name in [
            "Movie (2001).srt",
            "Movie (2001).en.srt",
            "Movie (2001).fr.forced.ASS",
            "Movie (2001).eng.sdh.srt",
            "Movie (2001).hi.srt",
            "Movie (2001).en.hi.srt",
            "Movie (2001).nfo",
            "Movie (2001) 2.srt",
            "Other.en.srt",
        ] {
            touch(dir.path(), name);
        }
        std::os::unix::fs::symlink(
            dir.path().join("Other.en.srt"),
            dir.path().join("Movie (2001).de.srt"),
        )
        .unwrap();

        let found = sidecars(&video).unwrap();
        // (file, codec, language, forced, hearing impaired)
        type Row<'a> = (&'a str, &'a str, Option<&'a str>, bool, bool);
        let summary: Vec<Row> = found
            .iter()
            .map(|s| {
                (
                    s.file.as_str(),
                    s.codec.as_str(),
                    s.language.as_deref(),
                    s.forced,
                    s.hearing_impaired,
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("Movie (2001).en.hi.srt", "subrip", Some("en"), false, true),
                ("Movie (2001).en.srt", "subrip", Some("en"), false, false),
                (
                    "Movie (2001).eng.sdh.srt",
                    "subrip",
                    Some("eng"),
                    false,
                    true
                ),
                ("Movie (2001).fr.forced.ASS", "ass", Some("fr"), true, false),
                ("Movie (2001).hi.srt", "subrip", Some("hi"), false, false),
                ("Movie (2001).srt", "subrip", None, false, false),
            ]
        );
        assert!(
            found
                .iter()
                .all(|s| s.id.len() == 8 && s.id.bytes().all(|b| b.is_ascii_hexdigit()))
        );

        // Ids come from names, so a new file doesn't change the others'.
        let ids: Vec<String> = found.iter().map(|s| s.id.clone()).collect();
        touch(dir.path(), "Movie (2001).de.forced.srt");
        let after: Vec<String> = sidecars(&video)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert!(ids.iter().all(|id| after.contains(id)));
        assert_eq!(after.len(), ids.len() + 1);
    }

    #[test]
    fn a_sidecars_language_keeps_its_region() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("Show S01E01.mkv");
        std::fs::write(&video, b"").unwrap();
        for name in [
            "Show S01E01.pt-BR.srt",
            "Show S01E01.es-419.forced.srt",
            "Show S01E01.pt-Brazil.srt",
        ] {
            touch(dir.path(), name);
        }
        let languages: Vec<(String, Option<String>)> = sidecars(&video)
            .unwrap()
            .into_iter()
            .map(|s| (s.file, s.language))
            .collect();
        assert_eq!(
            languages,
            vec![
                (
                    "Show S01E01.es-419.forced.srt".into(),
                    Some("es-419".into())
                ),
                ("Show S01E01.pt-BR.srt".into(), Some("pt-br".into())),
                ("Show S01E01.pt-Brazil.srt".into(), None),
            ]
        );
    }

    #[tokio::test]
    async fn converts_embedded_and_sidecar_tracks_and_caches_them() {
        let dir = tempfile::tempdir().unwrap();
        let clip = crate::probe::testing::make_clip(dir.path());
        let cache = dir.path().join("cache");

        let embedded = webvtt(
            Path::new("ffmpeg"),
            &cache,
            "1-s2",
            Source::Embedded {
                video: &clip,
                index: 2,
            },
        )
        .await
        .unwrap();
        let text = String::from_utf8(embedded).unwrap();
        assert!(
            text.starts_with("WEBVTT") && text.contains("Hello"),
            "{text}"
        );
        assert!(cache.join("1-s2.vtt").exists());

        // Served from the cache: works with ffmpeg gone.
        let again = webvtt(
            Path::new("/nonexistent/ffmpeg"),
            &cache,
            "1-s2",
            Source::Embedded {
                video: &clip,
                index: 2,
            },
        )
        .await
        .unwrap();
        assert_eq!(String::from_utf8(again).unwrap(), text);

        let srt = dir.path().join("clip.en.srt");
        std::fs::write(&srt, "1\n00:00:01,000 --> 00:00:02,500\nBonjour\n").unwrap();
        let sidecar = webvtt(Path::new("ffmpeg"), &cache, "1-x0", Source::Sidecar(&srt))
            .await
            .unwrap();
        let sidecar = String::from_utf8(sidecar).unwrap();
        assert!(
            sidecar.contains("00:01.000 --> 00:02.500") && sidecar.contains("Bonjour"),
            "{sidecar}"
        );
    }

    #[tokio::test]
    async fn converts_to_ass_for_burning_in() {
        let dir = tempfile::tempdir().unwrap();
        let srt = dir.path().join("a.srt");
        std::fs::write(&srt, "1\n00:00:01,000 --> 00:00:02,000\nBurned\n").unwrap();
        let cache = dir.path().join("cache");
        let path = convert(
            Path::new("ffmpeg"),
            &cache,
            "1-x0",
            Source::Sidecar(&srt),
            Format::Ass,
        )
        .await
        .unwrap();
        assert_eq!(path, cache.join("1-x0.ass"));
        let ass = std::fs::read_to_string(&path).unwrap();
        assert!(
            ass.contains("[Script Info]") && ass.contains("Burned"),
            "{ass}"
        );
    }

    #[tokio::test]
    async fn a_failure_is_remembered_instead_of_retried_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.srt");
        std::fs::write(&junk, b"\x00\x01 not subtitles").unwrap();
        let cache = dir.path().join("cache");
        assert!(
            webvtt(Path::new("ffmpeg"), &cache, "k", Source::Sidecar(&junk))
                .await
                .is_err()
        );
        // A second request doesn't run ffmpeg (here: one that doesn't exist) again.
        let again = webvtt(
            Path::new("/nonexistent/ffmpeg"),
            &cache,
            "k",
            Source::Sidecar(&junk),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{again:#}").contains("failed recently"),
            "{again:#}"
        );
    }

    #[tokio::test]
    async fn concurrent_requests_for_one_track_share_a_conversion() {
        let dir = tempfile::tempdir().unwrap();
        let srt = dir.path().join("a.srt");
        std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:01,000\nShared\n").unwrap();
        let cache = dir.path().join("cache");
        let requests = (0..6).map(|_| {
            let (srt, cache) = (srt.clone(), cache.clone());
            tokio::spawn(async move {
                webvtt(Path::new("ffmpeg"), &cache, "shared", Source::Sidecar(&srt)).await
            })
        });
        for request in requests {
            let vtt = request.await.unwrap().unwrap();
            assert!(String::from_utf8(vtt).unwrap().contains("Shared"));
        }
        // Other tests run in parallel and share the table, so only check this track's lock.
        let key = cache.join("shared.vtt");
        assert!(
            !IN_FLIGHT.lock().unwrap().contains_key(&key),
            "its lock is cleaned up"
        );
    }

    #[tokio::test]
    async fn a_failed_conversion_is_an_error_and_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.srt");
        std::fs::write(&junk, b"\x00\x01 not subtitles").unwrap();
        let cache = dir.path().join("cache");
        assert!(
            webvtt(Path::new("ffmpeg"), &cache, "k", Source::Sidecar(&junk))
                .await
                .is_err()
        );
        assert!(!cache.join("k.vtt").exists());
    }

    #[tokio::test]
    async fn pulls_out_a_videos_fonts_once_under_names_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let font = dir.path().join("Fancy.ttf");
        std::fs::write(&font, b"not really a font, but attached as one").unwrap();
        let with_fonts = dir.path().join("styled.mkv");
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=d=1:s=64x64",
                "-c:v",
                "mpeg4",
            ])
            .arg("-attach")
            .arg(&font)
            .args([
                "-metadata:s:t:0",
                "mimetype=application/x-truetype-font",
                "-metadata:s:t:0",
                "filename=../../evil.ttf",
            ])
            .arg(&with_fonts)
            .status()
            .unwrap();
        assert!(made.success());
        let cache = dir.path().join("cache");
        let ffmpeg = Path::new("ffmpeg");
        let ffprobe = Path::new("ffprobe");
        let fonts_dir = fonts(ffprobe, ffmpeg, &cache, "1-2-3", &with_fonts)
            .await
            .unwrap()
            .unwrap();
        let saved: Vec<String> = std::fs::read_dir(&fonts_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(saved.len(), 1, "{saved:?}");
        assert!(
            saved[0].starts_with("font"),
            "a name of Melete's own: {saved:?}"
        );
        assert_eq!(
            std::fs::read(fonts_dir.join(&saved[0])).unwrap(),
            std::fs::read(&font).unwrap()
        );
        assert!(!dir.path().join("evil.ttf").exists() && !cache.join("evil.ttf").exists());
        // Once saved, nothing runs again.
        let again = fonts(
            Path::new("/nonexistent/ffprobe"),
            Path::new("/nonexistent/ffmpeg"),
            &cache,
            "1-2-3",
            &with_fonts,
        )
        .await
        .unwrap();
        assert_eq!(again.as_deref(), Some(fonts_dir.as_path()));

        // A video without fonts is remembered as such.
        let plain = dir.path().join("plain.mkv");
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=d=1:s=64x64",
                "-c:v",
                "mpeg4",
            ])
            .arg(&plain)
            .status()
            .unwrap();
        assert!(made.success());
        assert_eq!(
            fonts(ffprobe, ffmpeg, &cache, "4-5-6", &plain)
                .await
                .unwrap(),
            None
        );
        let remembered = fonts(
            Path::new("/nonexistent/ffprobe"),
            ffmpeg,
            &cache,
            "4-5-6",
            &plain,
        )
        .await
        .unwrap();
        assert_eq!(remembered, None);
    }

    #[tokio::test]
    async fn cleaning_removes_only_what_nothing_will_ask_for() {
        let library = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        // A present video with a subtitle file beside it, and a file now gone.
        std::fs::write(library.path().join("Movie.mkv"), b"video").unwrap();
        std::fs::write(
            library.path().join("Movie.en.srt"),
            b"1\n00:00:01,000 --> 00:00:02,000\nHi\n",
        )
        .unwrap();
        let pool = crate::db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', ?)",
        )
        .bind(library.path().to_string_lossy().into_owned())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation) VALUES (1, 1, 'Movie', 'Movie', 1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id, first_seen_generation, seen_generation, missing_since)
             VALUES (7, 1, 'Movie.mkv', 5, 100, 1, 1, 1, NULL), (8, 1, 'Gone.mkv', 5, 100, 1, 1, 1, 50)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let sidecar = sidecars(&library.path().join("Movie.mkv"))
            .unwrap()
            .remove(0);
        let current_sidecar = sidecar_key(7, &sidecar);
        let mut stale_sidecar = sidecar.clone();
        stale_sidecar.size += 1;
        let stale_sidecar = sidecar_key(7, &stale_sidecar);
        let keep = [
            format!("{}.vtt", embedded_key(7, 2, 5, 100)),
            format!("{}.ass", embedded_key(7, 2, 5, 100)),
            format!("{current_sidecar}.vtt"),
            format!("{}.vtt.failed", embedded_key(7, 3, 5, 100)),
            "notes.txt".to_owned(),
        ];
        let remove = [
            format!("{}.vtt", embedded_key(7, 2, 4, 90)),
            format!("{stale_sidecar}.vtt"),
            format!("{}.vtt", embedded_key(8, 2, 5, 100)),
            format!("{}.vtt", embedded_key(99, 0, 1, 1)),
        ];
        for name in keep.iter().chain(&remove) {
            std::fs::write(cache.path().join(name), b"x").unwrap();
        }
        // An old failure marker and an abandoned temporary file.
        let old = std::time::SystemTime::now() - Duration::from_secs(2 * 24 * 3600);
        for name in [
            format!("{}.vtt.failed", embedded_key(7, 4, 5, 100)),
            ".123.part".to_owned(),
        ] {
            let file = std::fs::File::create(cache.path().join(&name)).unwrap();
            file.set_modified(old).unwrap();
        }

        assert_eq!(
            clean_cache(&pool, cache.path()).await.unwrap(),
            remove.len() + 2
        );
        let mut left: Vec<String> = std::fs::read_dir(cache.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        let mut expected = keep.to_vec();
        expected.sort();
        assert_eq!(left, expected);
        // A cache folder not made yet is fine.
        assert_eq!(
            clean_cache(&pool, &cache.path().join("none"))
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn cleaning_removes_fonts_of_files_gone_or_changed() {
        let library = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        std::fs::write(library.path().join("Movie.mkv"), b"video").unwrap();
        let pool = crate::db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', ?)",
        )
        .bind(library.path().to_string_lossy().into_owned())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation) VALUES (1, 1, 'Movie', 'Movie', 1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id, first_seen_generation, seen_generation, missing_since)
             VALUES (7, 1, 'Movie.mkv', 5, 100, 1, 1, 1, NULL), (8, 1, 'Gone.mkv', 5, 100, 1, 1, 1, 50)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let folder = |name: &str| {
            let dir = cache.path().join(name);
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join("font3"), b"x").unwrap();
            dir
        };
        folder("fonts-7-5-100");
        folder("fonts-7-4-90");
        std::fs::write(cache.path().join("fonts-8-5-100.none"), b"").unwrap();
        // An extraction abandoned long ago.
        let partial = folder(".fonts-7-5-100.42.part");
        let old = std::time::SystemTime::now() - Duration::from_secs(2 * 3600);
        std::fs::File::open(&partial)
            .unwrap()
            .set_modified(old)
            .unwrap();

        assert_eq!(clean_cache(&pool, cache.path()).await.unwrap(), 3);
        let left: Vec<String> = std::fs::read_dir(cache.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(left, ["fonts-7-5-100"]);
        assert!(cache.path().join("fonts-7-5-100/font3").exists());
    }
}
