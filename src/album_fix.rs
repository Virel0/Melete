//! Fixing an album at import: each track's loudness measured and
//! written as ReplayGain, the album's name, artist and MusicBrainz ids filled in where its tags
//! lack them, and a front cover fetched from the Cover Art Archive when the release brought
//! none (only with MusicBrainz matching on). With lyrics turned on, each track's synced lyrics
//! from LRCLIB go beside it as an .lrc file.
//!
//! Only files this import has just put in place are changed. Each is tagged as a hidden copy
//! beside it, which then takes its place in one rename, so the library never holds a
//! half-written file. Fixing again gives the same result, so an import cut off mid-fix simply
//! fixes again.

use std::io::ErrorKind;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lofty::config::{ParseOptions, WriteOptions};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::Tag;
use sqlx::SqlitePool;

use crate::coverart::{self, Fetched};
use crate::lrclib;
use crate::sound::loudness::{self, Loudness};
use crate::sound::{self, MeasureError};

mod cover_and_lyrics;
mod measuring;
mod songs;
mod tagging;

use cover_and_lyrics::*;
use measuring::*;
pub use songs::*;
use tagging::*;

/// ReplayGain 2.0's reference loudness.
const REFERENCE_LUFS: f64 = -18.0;
/// A fetched cover's size in pixels: for the library's own file, not a grid.
const COVER_SIZE: &str = "1200";

/// What fixing an album needs from outside.
pub struct Fixes {
    /// ffmpeg and ffprobe, for measuring loudness.
    pub tools: sound::Tools,
    /// The Cover Art Archive, when MusicBrainz matching is on.
    pub coverart: Option<Arc<coverart::Client>>,
    /// LRCLIB, when lyrics are turned on (`MELETE_LYRICS=lrclib`).
    pub lyrics: Option<lrclib::Client>,
}

/// The album as it's wanted: what fills its tags' gaps.
#[derive(Debug, sqlx::FromRow)]
struct Wanted {
    title: String,
    artist: String,
    mb_release_group_id: String,
    mb_artist_id: String,
}

/// Fixes the tracks a download of wanted album `album` placed, and the cover of its folder
/// `dir`. `has_cover` says whether the release brought a cover file. Returns the download's message.
/// Fails only when the library can't be written, which is worth another try; a track that
/// can't be measured or tagged stays as it came, and the message says so.
pub async fn fix(
    pool: &SqlitePool,
    fixes: &Fixes,
    download: i64,
    album: i64,
    dir: &Path,
    tracks: &[PathBuf],
    has_cover: bool,
) -> Result<String, String> {
    let wanted: Option<Wanted> = sqlx::query_as(
        "SELECT al.title, ar.name AS artist, al.mb_release_group_id, ar.mb_artist_id
         FROM wanted_albums al JOIN wanted_artists ar ON ar.id = al.wanted_artist_id
         WHERE al.id = ?",
    )
    .bind(album)
    .fetch_optional(pool)
    .await
    .map_err(|err| format!("the database: {err}"))?;
    let Some(wanted) = wanted else {
        return Ok("in the library".into());
    };
    let mut notes: Vec<String> = Vec::new();

    let (track_loudness, album_loudness) = measure(&fixes.tools, tracks, &mut notes).await;
    let mut untagged = Vec::new();
    for (track, loudness) in tracks.iter().zip(track_loudness) {
        let writes = writes(&wanted, loudness, album_loudness);
        let (path, temp) = (track.clone(), temporary(track, download));
        let retagged = tokio::task::spawn_blocking(move || retag(&path, &temp, &writes))
            .await
            .map_err(|err| format!("a tagging task failed: {err}"))?;
        match retagged {
            Ok(_) => {}
            Err(Retag::Tags(why)) => {
                tracing::warn!(file = %track.display(), error = why, "couldn't tag a track");
                untagged.push(file_name(track));
            }
            Err(Retag::Library(err)) => {
                return Err(format!("tagging {}: {err}", track.display()));
            }
        }
    }
    if album_loudness.is_some() {
        notes.insert(0, "ReplayGain added".into());
    }
    if !untagged.is_empty() {
        notes.push(format!("couldn't tag {}", untagged.join(", ")));
    }
    if !has_cover && let Some(note) = cover(fixes, &wanted, dir, tracks).await? {
        notes.push(note);
    }
    if let Some(client) = &fixes.lyrics {
        notes.push(lyrics(client, &wanted.artist, &wanted.title, download, tracks).await?);
    }
    Ok(match notes.as_slice() {
        [] => "in the library".into(),
        notes => format!("in the library; {}", notes.join("; ")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GROUP: &str = "b1392450-e666-3926-a536-22c65f834433";
    const ARTIST: &str = "a74b1b7f-71a5-4011-9441-d0b5e4122711";

    fn fixes(ffmpeg: &str, coverart: Option<coverart::Client>) -> Fixes {
        Fixes {
            tools: sound::Tools {
                ffmpeg: ffmpeg.into(),
                ffprobe: "ffprobe".into(),
            },
            coverart: coverart.map(Arc::new),
            lyrics: None,
        }
    }

    /// A tone of `seconds` at `volume`, tagged with ffmpeg's metadata keys.
    fn tone(dir: &Path, name: &str, seconds: f64, volume: f64, tags: &[(&str, &str)]) -> PathBuf {
        let path = dir.join(name);
        let mut ffmpeg = std::process::Command::new("ffmpeg");
        ffmpeg
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!("sine=f=440:d={seconds}"))
            .args(["-af", &format!("volume={volume}")]);
        for (key, value) in tags {
            ffmpeg.args(["-metadata", &format!("{key}={value}")]);
        }
        assert!(
            ffmpeg.arg(&path).status().unwrap().success(),
            "ffmpeg made {name}"
        );
        path
    }

    async fn wanted() -> SqlitePool {
        let pool = crate::db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
                 VALUES (1, ?, 'Radiohead', 1, 'all', 'lidarr', 0);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at)
                 VALUES (7, 1, ?, 'OK Computer', 1, 'lidarr', 0);",
        )
        .bind(ARTIST)
        .bind(GROUP)
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn tag(path: &Path, key: ItemKey) -> Option<String> {
        let file = open(path).unwrap();
        let tag = file.primary_tag().or_else(|| file.first_tag())?;
        tag.get_string(key).map(str::to_owned)
    }

    fn decibels(value: &str) -> f64 {
        value.trim_end_matches(" dB").parse().unwrap()
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.'))
            .collect()
    }

    #[test]
    fn the_albums_loudness_is_its_tracks_energy_over_its_length() {
        let loud = |lufs, true_peak_dbfs| Loudness {
            lufs,
            true_peak_dbfs,
        };
        let album = album_of(&[(loud(-10.0, -1.0), 100.0), (loud(-20.0, -3.0), 100.0)]).unwrap();
        // 10·log10((10^-1 + 10^-2) / 2)
        assert!((album.lufs - -12.596).abs() < 0.001, "{}", album.lufs);
        assert_eq!(album.true_peak_dbfs, -1.0);
        // A longer track weighs more: 10·log10((300·10^-1 + 100·10^-2) / 400)
        let album = album_of(&[(loud(-10.0, -1.0), 300.0), (loud(-20.0, -3.0), 100.0)]).unwrap();
        assert!((album.lufs - -11.107).abs() < 0.001, "{}", album.lufs);
        assert_eq!(album_of(&[]), None);
        assert_eq!(gain(loud(-9.68, 0.0)), "-8.32 dB");
        assert_eq!(peak(loud(-9.68, -6.0206)), "0.500000");
    }

    #[tokio::test]
    async fn writes_replaygain_and_fills_only_the_gaps_in_the_tags() {
        let pool = wanted().await;
        let dir = tempfile::tempdir().unwrap();
        let loud = tone(
            dir.path(),
            "01.flac",
            5.0,
            0.5,
            &[("album", "OK Computer (Collector's Edition)")],
        );
        let quiet = tone(dir.path(), "02.flac", 5.0, 0.05, &[]);
        let short = tone(dir.path(), "03.flac", 1.0, 0.5, &[]);
        let tracks = vec![loud.clone(), quiet.clone(), short.clone()];

        let message = fix(
            &pool,
            &fixes("ffmpeg", None),
            9,
            7,
            dir.path(),
            &tracks,
            true,
        )
        .await
        .unwrap();
        assert_eq!(message, "in the library; ReplayGain added");
        assert_eq!(
            tag(&loud, ItemKey::AlbumTitle).as_deref(),
            Some("OK Computer (Collector's Edition)"),
            "a name the tags have is kept"
        );
        assert_eq!(
            tag(&quiet, ItemKey::AlbumTitle).as_deref(),
            Some("OK Computer")
        );
        for track in &tracks {
            assert_eq!(
                tag(track, ItemKey::AlbumArtist).as_deref(),
                Some("Radiohead")
            );
            assert_eq!(
                tag(track, ItemKey::MusicBrainzReleaseGroupId).as_deref(),
                Some(GROUP)
            );
            assert_eq!(
                tag(track, ItemKey::MusicBrainzReleaseArtistId).as_deref(),
                Some(ARTIST)
            );
        }
        let loud_gain = decibels(&tag(&loud, ItemKey::ReplayGainTrackGain).unwrap());
        let quiet_gain = decibels(&tag(&quiet, ItemKey::ReplayGainTrackGain).unwrap());
        // A tenth of the volume is 20 dB quieter.
        assert!(
            (quiet_gain - loud_gain - 20.0).abs() < 0.5,
            "{loud_gain} {quiet_gain}"
        );
        let album_gain = tag(&loud, ItemKey::ReplayGainAlbumGain).unwrap();
        assert_eq!(
            tag(&quiet, ItemKey::ReplayGainAlbumGain),
            Some(album_gain.clone())
        );
        assert!(decibels(&album_gain) > loud_gain && decibels(&album_gain) < quiet_gain);
        // ffmpeg's sine is at an eighth of full scale; halved, 0.0625.
        let peak: f64 = tag(&loud, ItemKey::ReplayGainTrackPeak)
            .unwrap()
            .parse()
            .unwrap();
        assert!((0.06..0.065).contains(&peak), "{peak}");
        assert_eq!(
            tag(&short, ItemKey::ReplayGainTrackGain),
            None,
            "too short to measure"
        );
        assert_eq!(tag(&short, ItemKey::ReplayGainAlbumGain), None);
        assert!(
            leftovers(dir.path()).is_empty(),
            "{:?}",
            leftovers(dir.path())
        );
        assert_eq!(
            std::fs::metadata(&loud).unwrap().permissions().mode() & 0o777,
            0o664
        );

        // Fixing again, as after a restart, changes nothing.
        let before = std::fs::read(&loud).unwrap();
        fix(
            &pool,
            &fixes("ffmpeg", None),
            9,
            7,
            dir.path(),
            &tracks,
            true,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&loud).unwrap(), before);
    }

    #[tokio::test]
    async fn without_ffmpeg_or_readable_tags_the_rest_is_still_fixed() {
        let pool = wanted().await;
        let dir = tempfile::tempdir().unwrap();
        let good = tone(dir.path(), "01.flac", 4.0, 0.5, &[]);
        let message = fix(
            &pool,
            &fixes("/nonexistent/ffmpeg", None),
            9,
            7,
            dir.path(),
            std::slice::from_ref(&good),
            true,
        )
        .await
        .unwrap();
        assert_eq!(message, "in the library; no ReplayGain: ffmpeg can't run");
        assert_eq!(tag(&good, ItemKey::ReplayGainTrackGain), None);
        assert_eq!(
            tag(&good, ItemKey::MusicBrainzReleaseGroupId).as_deref(),
            Some(GROUP)
        );

        let broken = dir.path().join("02.flac");
        std::fs::write(&broken, b"not audio").unwrap();
        let message = fix(
            &pool,
            &fixes("ffmpeg", None),
            9,
            7,
            dir.path(),
            &[good.clone(), broken.clone()],
            true,
        )
        .await
        .unwrap();
        assert_eq!(
            message,
            "in the library; couldn't measure 02.flac; couldn't tag 02.flac"
        );
        assert_eq!(
            std::fs::read(&broken).unwrap(),
            b"not audio",
            "left as it came"
        );
        assert!(tag(&good, ItemKey::ReplayGainTrackGain).is_some());
        assert_eq!(
            tag(&good, ItemKey::ReplayGainAlbumGain),
            None,
            "not from part of the album"
        );
        assert!(leftovers(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn fetches_a_cover_only_when_the_release_brought_none() {
        let pool = wanted().await;
        let (client, log) = coverart::testing::start().await;
        let fixes = fixes("ffmpeg", Some(client));
        let dir = tempfile::tempdir().unwrap();
        let track = tone(dir.path(), "01.flac", 1.0, 0.5, &[]);

        let message = fix(
            &pool,
            &fixes,
            9,
            7,
            dir.path(),
            std::slice::from_ref(&track),
            true,
        )
        .await
        .unwrap();
        assert_eq!(message, "in the library");
        assert!(log.lock().unwrap().is_empty(), "it brought one");

        let message = fix(
            &pool,
            &fixes,
            9,
            7,
            dir.path(),
            std::slice::from_ref(&track),
            false,
        )
        .await
        .unwrap();
        assert_eq!(message, "in the library; cover fetched");
        assert_eq!(
            std::fs::read(dir.path().join("cover.jpg")).unwrap(),
            coverart::testing::JPEG
        );
        assert_eq!(*log.lock().unwrap(), [format!("{GROUP} front-1200")]);

        // Whatever is already at the name stays.
        std::fs::write(dir.path().join("cover.jpg"), b"mine").unwrap();
        let message = fix(&pool, &fixes, 9, 7, dir.path(), &[track], false)
            .await
            .unwrap();
        assert_eq!(message, "in the library");
        assert_eq!(
            std::fs::read(dir.path().join("cover.jpg")).unwrap(),
            b"mine"
        );
    }

    #[tokio::test]
    async fn writes_synced_lyrics_beside_the_tracks_lrclib_knows() {
        let pool = wanted().await;
        let (client, log) = lrclib::testing::start().await;
        let mut fixes = fixes("ffmpeg", None);
        fixes.lyrics = Some(client);
        let dir = tempfile::tempdir().unwrap();
        let tracks = vec![
            tone(
                dir.path(),
                "01 - Airbag.flac",
                4.0,
                0.5,
                &[("title", "Airbag"), ("artist", "Radiohead")],
            ),
            tone(
                dir.path(),
                "02.flac",
                4.0,
                0.5,
                &[("title", "Fitter Happier")],
            ),
            tone(dir.path(), "03.flac", 4.0, 0.5, &[("title", "Lucky")]),
            tone(dir.path(), "04.flac", 4.0, 0.5, &[]),
        ];
        let message = fix(&pool, &fixes, 9, 7, dir.path(), &tracks, true)
            .await
            .unwrap();
        assert_eq!(
            message,
            "in the library; ReplayGain added; synced lyrics for 1 of 4 tracks"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("01 - Airbag.lrc")).unwrap(),
            lrclib::testing::AIRBAG
        );
        assert!(!dir.path().join("02.lrc").exists(), "instrumental");
        assert!(leftovers(dir.path()).is_empty());
        {
            let asked = log.lock().unwrap();
            assert_eq!(asked.len(), 3, "a track without a title isn't asked about");
            assert_eq!(asked[0]["album_name"], "OK Computer", "the album filled in");
            assert_eq!(asked[0]["duration"], "4");
            assert_eq!(
                asked[1]["artist_name"], "Radiohead",
                "the album artist filled in"
            );
        }

        // Lyrics already there are kept and not asked for again.
        std::fs::write(dir.path().join("03.lrc"), b"mine").unwrap();
        let message = fix(&pool, &fixes, 9, 7, dir.path(), &tracks, true)
            .await
            .unwrap();
        assert!(
            message.ends_with("synced lyrics for 2 of 4 tracks"),
            "{message}"
        );
        assert_eq!(std::fs::read(dir.path().join("03.lrc")).unwrap(), b"mine");
        assert_eq!(
            log.lock().unwrap().len(),
            4,
            "only Fitter Happier asked again"
        );
    }

    #[tokio::test]
    async fn lrclib_away_leaves_the_album_imported() {
        let pool = wanted().await;
        let mut fixes = fixes("ffmpeg", None);
        fixes.lyrics = Some(lrclib::Client::with_base("http://127.0.0.1:9".into()).unwrap());
        let dir = tempfile::tempdir().unwrap();
        let track = tone(dir.path(), "01.flac", 4.0, 0.5, &[("title", "Airbag")]);
        let message = fix(&pool, &fixes, 9, 7, dir.path(), &[track], true)
            .await
            .unwrap();
        assert_eq!(
            message,
            "in the library; ReplayGain added; no lyrics: LRCLIB didn't answer"
        );
    }
}
