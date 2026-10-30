//! Scanning a music library: every audio file becomes a track with what its tags say
//! (or, untagged, what its name and folders say), and tracks are grouped into albums and
//! artists. Tags are read again only for files that are new or changed size or time. Nothing on
//! disk is ever written.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use anyhow::Context;
use sqlx::SqlitePool;

use super::Library;
use super::scan::{FoundFile, Media, ScanReport, unix_seconds};
use super::tags::{self, Tags};

mod grouping;
mod merging;
mod naming;
mod reading;
mod storing;

use grouping::*;
use merging::*;
use naming::*;
use reading::*;
use storing::*;

/// Files read at once; the library usually sits on a network share.
const READERS: usize = 8;
/// Which reading of tags rows have: raised when a field is added, so files scanned before are
/// read again for it (1 added composers, works and movements; 2 stopped taking E-AC-3 in MP4
/// for AAC; 3 added the artists listed apart and every MusicBrainz artist id).
const TAGS_VERSION: i64 = 3;
/// What an album of several artists is called.
const VARIOUS: &str = "Various Artists";
/// File names (without extension) taken as an album's cover image, best first.
pub(crate) const COVER_NAMES: &[&str] = &["cover", "folder", "front", "album"];
pub(crate) const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];
/// File names taken as an album's motion cover (a looping video), best first.
const MOTION_NAMES: &[&str] = &["cover", "animated", "motion", "animated-cover"];
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "webm"];

/// An album folder's cover image and motion cover, relative to the library.
#[derive(Debug, Default, Clone, PartialEq)]
struct Artwork {
    cover: Option<String>,
    motion: Option<String>,
}

/// Scans a music library: walks it, reads the tags of new and changed files, finds each album
/// folder's artwork, and saves it all in one go. Nothing on disk is written.
///
/// # Errors
///
/// Fails when the library's folder can't be walked or the database can't be written. A file whose
/// tags can't be read isn't an error: it's saved with the reason, named from its folders.
pub async fn scan(pool: &SqlitePool, library: &Library) -> anyhow::Result<ScanReport> {
    let progress = super::progress::Progress::start(pool, library.id, true).await;
    let scanned = scan_noting(pool, library, &progress).await;
    progress.finish().await;
    scanned
}

/// [`scan`], its walk noted in `progress`.
async fn scan_noting(
    pool: &SqlitePool,
    library: &Library,
    progress: &super::progress::Progress,
) -> anyhow::Result<ScanReport> {
    let root = library.path.clone();
    let folders = progress.folders();
    let walk = tokio::task::spawn_blocking(move || {
        super::walk::walk_from(&root, Media::Audio, None, &folders)
    });
    let walked = progress
        .walking(super::progress::TICK, walk)
        .await?
        .with_context(|| format!("scanning {}", library.path.display()))?;
    let (found, skipped, ignored) = (walked.found, walked.skipped, walked.ignored);

    // Only new files, ones whose size or time changed, and ones read before a field was added
    // have their tags read.
    let known: HashMap<String, (i64, i64)> = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT rel_path, size, modified_at FROM music_tracks
         WHERE library_id = ? AND tags_version >= ?",
    )
    .bind(library.id)
    .bind(TAGS_VERSION)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(path, size, modified)| (path, (size, modified)))
    .collect();
    let stale: Vec<String> = found
        .iter()
        .filter(|f| known.get(&f.rel_path) != Some(&(f.size, f.modified)))
        .map(|f| f.rel_path.clone())
        .collect();
    let root = library.path.clone();
    let read = tokio::task::spawn_blocking(move || read_all(&root, stale)).await?;
    let mut folders: Vec<String> = found
        .iter()
        .map(|f| album_folder(&f.rel_path).to_owned())
        .collect();
    folders.sort_unstable();
    folders.dedup();
    let root = library.path.clone();
    let artwork = tokio::task::spawn_blocking(move || find_artwork(&root, &folders)).await?;
    store(pool, library, &found, read, &artwork, (skipped, &ignored)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(rel_path: &str) -> TrackFacts {
        TrackFacts {
            rel_path: rel_path.into(),
            ..TrackFacts::default()
        }
    }

    #[test]
    fn disc_folders_belong_to_their_album() {
        assert_eq!(album_folder("Artist/Album/CD2/01.flac"), "Artist/Album");
        assert_eq!(
            album_folder("Artist/Album/Disc 1 - Live/01.flac"),
            "Artist/Album"
        );
        assert_eq!(album_folder("Artist/Album/01.flac"), "Artist/Album");
        assert_eq!(album_folder("Artist/CDs/01.flac"), "Artist/CDs");
        assert_eq!(album_folder("loose.mp3"), "");
    }

    #[test]
    fn albums_are_keyed_by_release_then_tag_then_folder() {
        let mut t = track("A/Greatest Hits/01.flac");
        assert_eq!(album_key(&t), "dir:A/Greatest Hits");
        t.album = Some("Greatest Hits".into());
        assert_eq!(album_key(&t), "tag:A/Greatest Hits\u{1f}greatest hits");
        t.mb_release_id = Some("b84ee12a-09ef-421b-82de-0441a926375b".into());
        assert_eq!(album_key(&t), "mb:b84ee12a-09ef-421b-82de-0441a926375b");
    }

    #[test]
    fn untagged_albums_come_from_their_folders() {
        let a = track("Radiohead/OK Computer (1997) [FLAC]/01 - Airbag.flac");
        assert_eq!(
            album_of(&[&a]),
            AlbumFacts {
                title: "OK Computer".into(),
                artist: Some("Radiohead".into()),
                year: Some(1997),
                folder: "Radiohead/OK Computer (1997) [FLAC]".into(),
                mb_release_id: None,
                mb_release_group_id: None,
                mb_artist_id: None,
            }
        );
        let b = track("Portishead - Dummy (1994)/01 Mysterons.mp3");
        let album = album_of(&[&b]);
        assert_eq!(
            (album.artist.as_deref(), album.title.as_str(), album.year),
            (Some("Portishead"), "Dummy", Some(1994))
        );
        let c = track("loose.mp3");
        assert_eq!(album_of(&[&c]).title, "Unknown album");
    }

    #[test]
    fn tags_win_and_several_artists_make_a_compilation() {
        let mut a = track("x/01.flac");
        a.album = Some("Now 1".into());
        a.artist = Some("One".into());
        a.year = Some(1983);
        let mut b = track("x/02.flac");
        b.album = Some("Now 1".into());
        b.artist = Some("Two".into());
        let album = album_of(&[&a, &b]);
        assert_eq!(
            (album.title.as_str(), album.artist, album.year),
            ("Now 1", None, Some(1983))
        );

        // An album artist settles it, even with guests on some tracks.
        a.album_artist = Some("One".into());
        b.album_artist = Some("One".into());
        a.mb_album_artist_id = Some("a74b1b7f-71a5-4011-9441-d0b5e4122711".into());
        let album = album_of(&[&a, &b]);
        assert_eq!(album.artist.as_deref(), Some("One"));
        assert_eq!(
            album.mb_artist_id.as_deref(),
            Some("a74b1b7f-71a5-4011-9441-d0b5e4122711")
        );
        // "Various Artists" as the album artist is a compilation.
        a.album_artist = Some("Various Artists".into());
        b.album_artist = Some("Various Artists".into());
        assert_eq!(album_of(&[&a, &b]).artist, None);
    }

    #[test]
    fn untagged_titles_and_numbers_come_from_names() {
        let named = |p: &str| {
            let n = from_name(p);
            (n.title, n.track_number)
        };
        assert_eq!(named("a/01 - Airbag.flac"), ("Airbag".into(), Some(1)));
        assert_eq!(named("a/1-03 Lucky.flac"), ("Lucky".into(), Some(3)));
        assert_eq!(
            named("a/07. Climbing Up the Walls.mp3"),
            ("Climbing Up the Walls".into(), Some(7))
        );
        assert_eq!(named("a/Exit Music.mp3"), ("Exit Music".into(), None));
        assert_eq!(named("a/1979.mp3"), ("1979".into(), None));
        assert_eq!(named("a/99.mp3"), ("99".into(), None));
    }

    /// A one-second tone at `rel`, tagged with `tags` (ffmpeg's metadata keys).
    fn clip(root: &Path, rel: &str, tags: &[(&str, &str)]) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut ffmpeg = std::process::Command::new("ffmpeg");
        ffmpeg.args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=d=1"]);
        for (key, value) in tags {
            ffmpeg.args(["-metadata", &format!("{key}={value}")]);
        }
        assert!(
            ffmpeg.arg(&path).status().unwrap().success(),
            "ffmpeg made {rel}"
        );
    }

    async fn library(root: &Path) -> (SqlitePool, Library) {
        let pool = crate::db::open_in_memory().await;
        let config = super::super::LibraryConfig {
            name: "music".into(),
            kind: super::super::LibraryKind::Music,
            path: root.to_path_buf(),
        };
        let library = super::super::sync_config(&pool, &[config])
            .await
            .unwrap()
            .remove(0);
        (pool, library)
    }

    /// Title, artist, year, compilation, release id and track count.
    type AlbumRow = (String, String, Option<i64>, bool, Option<String>, i64);

    const RELEASE: &str = "b84ee12a-09ef-421b-82de-0441a926375b";
    const ARTIST: &str = "a74b1b7f-71a5-4011-9441-d0b5e4122711";

    #[tokio::test]
    async fn adopts_tagged_untagged_and_broken_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let ok = [
            ("ALBUMARTIST", "Radiohead"),
            ("artist", "Radiohead"),
            ("album", "OK Computer"),
            ("date", "1997"),
            ("genre", "Rock;Alternative"),
            ("MUSICBRAINZ_ALBUMID", RELEASE),
            ("MUSICBRAINZ_ALBUMARTISTID", ARTIST),
        ];
        clip(
            root,
            "Radiohead/OK Computer (1997)/01 - Airbag.flac",
            &[&ok[..], &[("title", "Airbag"), ("track", "1")]].concat(),
        );
        clip(
            root,
            "Radiohead/OK Computer (1997)/02 - Paranoid Android.flac",
            &[&ok[..], &[("title", "Paranoid Android"), ("track", "2")]].concat(),
        );
        clip(
            root,
            "Portishead/Dummy/CD1/01.mp3",
            &[
                ("title", "Mysterons"),
                ("artist", "Portishead"),
                ("album", "Dummy"),
                ("track", "1"),
            ],
        );
        clip(
            root,
            "Portishead/Dummy/CD2/01.mp3",
            &[
                ("title", "Sour Times"),
                ("artist", "Portishead"),
                ("album", "Dummy"),
                ("track", "1"),
            ],
        );
        clip(
            root,
            "Compilations/Now/01.mp3",
            &[("title", "A"), ("artist", "One"), ("album", "Now")],
        );
        clip(
            root,
            "Compilations/Now/02.mp3",
            &[("title", "B"), ("artist", "Two"), ("album", "Now")],
        );
        clip(root, "Some Band/First Album (2003)/02 - Song.wav", &[]);
        std::fs::create_dir_all(root.join("Broken")).unwrap();
        std::fs::write(root.join("Broken/03 - Noise.mp3"), b"not audio at all").unwrap();
        std::fs::write(root.join("Radiohead/OK Computer (1997)/cover.jpg"), b"jpg").unwrap();
        let (pool, library) = library(root).await;

        let report = scan(&pool, &library).await.unwrap();
        assert_eq!((report.files, report.added, report.unparsed), (8, 8, 2));

        let albums: Vec<AlbumRow> = sqlx::query_as(
            "SELECT a.title, a.artist_name, a.year, a.compilation, a.mb_release_id,
                 (SELECT count(*) FROM music_tracks t WHERE t.album_id = a.id)
             FROM music_albums a ORDER BY a.title",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            albums,
            [
                // A folder at the top with no artist folder above it.
                (
                    "Broken".into(),
                    "Unknown artist".into(),
                    None,
                    false,
                    None,
                    1
                ),
                ("Dummy".into(), "Portishead".into(), None, false, None, 2),
                (
                    "First Album".into(),
                    "Some Band".into(),
                    Some(2003),
                    false,
                    None,
                    1
                ),
                ("Now".into(), "Various Artists".into(), None, true, None, 2),
                (
                    "OK Computer".into(),
                    "Radiohead".into(),
                    Some(1997),
                    false,
                    Some(RELEASE.into()),
                    2
                ),
            ]
        );
        let radiohead: (String, Option<String>) =
            sqlx::query_as("SELECT key, mb_artist_id FROM music_artists WHERE name = 'Radiohead'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(radiohead, (format!("mb:{ARTIST}"), Some(ARTIST.into())));
        let airbag: (
            String,
            Option<i64>,
            String,
            Option<String>,
            Option<i64>,
            bool,
        ) = sqlx::query_as(
            "SELECT title, track_number, genres, codec, sample_rate, duration_seconds > 0.9
             FROM music_tracks WHERE rel_path LIKE '%Airbag%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            airbag,
            (
                "Airbag".into(),
                Some(1),
                r#"["Rock","Alternative"]"#.into(),
                Some("flac".into()),
                Some(44100),
                true
            )
        );
        let untagged: (String, Option<i64>, bool, Option<String>) = sqlx::query_as(
            "SELECT title, track_number, tagged, codec FROM music_tracks WHERE rel_path LIKE '%Song.wav'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            untagged,
            ("Song".into(), Some(2), false, Some("wav".into()))
        );
        let broken: (bool, bool) = sqlx::query_as(
            "SELECT tagged, read_error IS NOT NULL FROM music_tracks WHERE rel_path LIKE 'Broken/%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(broken, (false, true));

        // An unchanged file isn't read again: what's stored stays.
        sqlx::query("UPDATE music_tracks SET title = 'kept' WHERE rel_path LIKE '%Airbag%'")
            .execute(&pool)
            .await
            .unwrap();
        let again = scan(&pool, &library).await.unwrap();
        assert_eq!((again.files, again.added, again.newly_missing), (8, 0, 0));
        let title: String =
            sqlx::query_scalar("SELECT title FROM music_tracks WHERE rel_path LIKE '%Airbag%'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(title, "kept");

        // A retagged file is read again and moves album; a removed one goes missing with its album.
        clip(
            root,
            "Compilations/Now/02.mp3",
            &[("title", "B"), ("artist", "Two"), ("album", "Then")],
        );
        std::fs::remove_file(root.join("Some Band/First Album (2003)/02 - Song.wav")).unwrap();
        let third = scan(&pool, &library).await.unwrap();
        assert_eq!((third.files, third.newly_missing), (7, 1));
        let present: Vec<(String, String)> = sqlx::query_as(
            "SELECT title, artist_name FROM music_albums WHERE missing_since IS NULL ORDER BY title",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let names: Vec<String> = present.iter().map(|(t, a)| format!("{t} / {a}")).collect();
        assert_eq!(
            names,
            [
                "Broken / Unknown artist",
                "Dummy / Portishead",
                "Now / One",
                "OK Computer / Radiohead",
                "Then / Two"
            ]
        );
    }

    #[test]
    fn finds_cover_images_and_motion_covers_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (rel, _) in [
            ("a/Folder.JPG", ()),
            ("a/cover.png", ()),
            ("a/animated.webm", ()),
            ("a/cover.mp4", ()),
            ("b/back.jpg", ()),
            ("b/notes.txt", ()),
            ("c/front.webp", ()),
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
        std::fs::create_dir_all(root.join("c/cover.jpg")).unwrap();
        let found = find_artwork(
            root,
            &["a".into(), "b".into(), "c".into(), "".into(), "gone".into()],
        );
        assert_eq!(
            found.get("a"),
            Some(&Artwork {
                cover: Some("a/cover.png".into()),
                motion: Some("a/cover.mp4".into()),
            }),
            "cover before folder, cover before animated"
        );
        assert_eq!(found.get("b"), None, "a back cover isn't the cover");
        assert_eq!(
            found.get("c").and_then(|a| a.cover.as_deref()),
            Some("c/front.webp"),
            "a folder named like a cover isn't one"
        );
        assert_eq!(found.len(), 2);
    }

    /// Rewrites an MP3's ID3 tag with lofty, as a tagger would.
    fn id3(path: &Path, items: &[(lofty::prelude::ItemKey, &str)]) {
        use lofty::prelude::*;
        let mut tag = lofty::tag::Tag::new(lofty::tag::TagType::Id3v2);
        for (key, value) in items {
            assert!(tag.insert_text(*key, (*value).to_owned()), "{key:?}");
        }
        tag.save_to_path(path, lofty::config::WriteOptions::default())
            .unwrap();
    }

    type Classical = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
    );

    async fn classical(pool: &SqlitePool, like: &str) -> Classical {
        sqlx::query_as(
            "SELECT composer, work, movement, movement_number, movement_total FROM music_tracks
             WHERE rel_path LIKE ?",
        )
        .bind(like)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn reads_composers_works_and_movements_and_reads_old_rows_again_for_them() {
        use lofty::prelude::ItemKey;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        clip(
            root,
            "Beethoven/Symphony 5/01.flac",
            &[
                ("title", "I. Allegro con brio"),
                ("album", "Symphony 5"),
                ("COMPOSER", "Ludwig van Beethoven"),
                ("WORK", "Symphony No. 5 in C minor, Op. 67"),
                ("MOVEMENTNAME", "Allegro con brio"),
                ("MOVEMENT", "1"),
                ("MOVEMENTTOTAL", "4"),
            ],
        );
        // Apple's way in MP3s: the work in TIT1 with the grouping moved to GRP1.
        clip(root, "Satie/Gymnopedies/01.mp3", &[]);
        id3(
            &root.join("Satie/Gymnopedies/01.mp3"),
            &[
                (ItemKey::TrackTitle, "Gymnopédie No. 1"),
                (ItemKey::Composer, "Erik Satie; Claude Debussy"),
                (ItemKey::ContentGroup, "Gymnopédies"),
                (ItemKey::AppleId3v2ContentGroup, "Piano"),
                (ItemKey::Movement, "Lent et douloureux"),
                (ItemKey::MovementNumber, "1/3"),
            ],
        );
        // Without GRP1, TIT1 is just the grouping.
        clip(root, "Pop/Song/01.mp3", &[]);
        id3(
            &root.join("Pop/Song/01.mp3"),
            &[
                (ItemKey::TrackTitle, "Song"),
                (ItemKey::ContentGroup, "Summer"),
            ],
        );
        let (pool, library) = library(root).await;
        scan(&pool, &library).await.unwrap();
        assert_eq!(
            classical(&pool, "Beethoven%").await,
            (
                Some("Ludwig van Beethoven".into()),
                Some("Symphony No. 5 in C minor, Op. 67".into()),
                Some("Allegro con brio".into()),
                Some(1),
                Some(4)
            )
        );
        assert_eq!(
            classical(&pool, "Satie%").await,
            (
                Some("Erik Satie".into()),
                Some("Gymnopédies".into()),
                Some("Lent et douloureux".into()),
                Some(1),
                Some(3)
            )
        );
        assert_eq!(
            classical(&pool, "Pop%").await,
            (None, None, None, None, None)
        );

        // A row read before these fields were: read again, keeping what matching linked.
        sqlx::query(
            "UPDATE music_tracks SET tags_version = 0, composer = NULL, work = NULL,
                 match_recording_id = 'r', match_track_id = 't'",
        )
        .execute(&pool)
        .await
        .unwrap();
        scan(&pool, &library).await.unwrap();
        assert_eq!(
            classical(&pool, "Beethoven%").await.0.as_deref(),
            Some("Ludwig van Beethoven")
        );
        let kept: Vec<(Option<String>, i64)> =
            sqlx::query_as("SELECT match_track_id, tags_version FROM music_tracks")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(
            kept.iter().all(|k| k == &(Some("t".into()), TAGS_VERSION)),
            "{kept:?}"
        );
    }

    #[tokio::test]
    async fn a_changed_file_loses_the_ids_matching_gave_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        clip(root, "A/B/01.flac", &[("title", "One"), ("album", "B")]);
        clip(root, "A/B/02.flac", &[("title", "Two"), ("album", "B")]);
        let (pool, library) = library(root).await;
        scan(&pool, &library).await.unwrap();
        sqlx::query("UPDATE music_tracks SET match_recording_id = 'r', match_track_id = 't'")
            .execute(&pool)
            .await
            .unwrap();
        clip(
            root,
            "A/B/02.flac",
            &[("title", "Two, retagged"), ("album", "B")],
        );
        scan(&pool, &library).await.unwrap();
        let ids: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT title, match_recording_id, match_track_id FROM music_tracks ORDER BY rel_path",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            ids,
            [
                ("One".into(), Some("r".into()), Some("t".into())),
                ("Two, retagged".into(), None, None),
            ]
        );
    }

    /// Rewrites a file's tags with lofty as a tagger would, repeated keys kept as several values.
    fn retag(path: &Path, kind: lofty::tag::TagType, items: &[(lofty::prelude::ItemKey, &str)]) {
        use lofty::prelude::*;
        let mut tag = lofty::tag::Tag::new(kind);
        for (key, value) in items {
            tag.push(lofty::tag::TagItem::new(
                *key,
                lofty::tag::ItemValue::Text((*value).to_owned()),
            ));
        }
        tag.save_to_path(path, lofty::config::WriteOptions::default())
            .unwrap();
    }

    /// Artist, artists listed apart and MusicBrainz artist ids, as stored.
    type Artists = (Option<String>, String, String, Option<String>);

    async fn artists(pool: &SqlitePool, like: &str) -> Artists {
        sqlx::query_as(
            "SELECT artist, artist_names, mb_artist_ids, mb_artist_id FROM music_tracks
             WHERE rel_path LIKE ?",
        )
        .bind(like)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn reads_several_artists_from_each_kind_of_tag_and_keeps_tracks_when_reading_again() {
        use lofty::prelude::ItemKey::{MusicBrainzArtistId, TrackArtist, TrackArtists, TrackTitle};
        use lofty::tag::TagType::{Id3v2, Mp4Ilst, VorbisComments};
        const OTHER: &str = "8f6bd1e4-fbe1-4f50-aa9b-94c450ec0f11";
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for rel in ["v/1.flac", "v/2.flac", "i/1.mp3", "m/1.m4a", "a/1.flac"] {
            clip(root, rel, &[]);
        }
        // Vorbis: an ARTISTS tag, and ids one per artist.
        retag(
            &root.join("v/1.flac"),
            VorbisComments,
            &[
                (TrackTitle, "Duet"),
                (TrackArtist, "Alder feat. Birch"),
                (TrackArtists, "Alder"),
                (TrackArtists, "Birch"),
                (MusicBrainzArtistId, ARTIST),
                (MusicBrainzArtistId, OTHER),
            ],
        );
        // Vorbis: ARTIST twice.
        retag(
            &root.join("v/2.flac"),
            VorbisComments,
            &[
                (TrackTitle, "Two"),
                (TrackArtist, "Alder"),
                (TrackArtist, "Birch"),
            ],
        );
        // ID3v2.4: TPE1 with two values, and Picard's ids in one frame.
        retag(
            &root.join("i/1.mp3"),
            Id3v2,
            &[
                (TrackTitle, "Three"),
                (TrackArtist, "Alder"),
                (TrackArtist, "Birch"),
                (MusicBrainzArtistId, &format!("{ARTIST}/{OTHER}")),
            ],
        );
        // MP4: Apple's ARTISTS atom.
        retag(
            &root.join("m/1.m4a"),
            Mp4Ilst,
            &[
                (TrackTitle, "Four"),
                (TrackArtist, "Alder & Birch"),
                (TrackArtists, "Alder"),
                (TrackArtists, "Birch"),
            ],
        );
        // One name with a slash, an ampersand and a comma: one artist.
        retag(
            &root.join("a/1.flac"),
            VorbisComments,
            &[(TrackTitle, "Five"), (TrackArtist, "AC/DC, Earth & Fire")],
        );
        let (pool, library) = library(root).await;
        scan(&pool, &library).await.unwrap();
        let both = r#"["Alder","Birch"]"#.to_owned();
        let ids = format!(r#"["{ARTIST}","{OTHER}"]"#);
        let alder = Some(ARTIST.to_owned());
        assert_eq!(
            artists(&pool, "v/1%").await,
            (
                Some("Alder feat. Birch".into()),
                both.clone(),
                ids.clone(),
                alder.clone()
            )
        );
        assert_eq!(
            artists(&pool, "v/2%").await,
            (Some("Alder; Birch".into()), both.clone(), "[]".into(), None)
        );
        assert_eq!(
            artists(&pool, "i/1%").await,
            (Some("Alder; Birch".into()), both.clone(), ids, alder)
        );
        assert_eq!(
            artists(&pool, "m/1%").await,
            (Some("Alder & Birch".into()), both, "[]".into(), None)
        );
        assert_eq!(
            artists(&pool, "a/1%").await,
            (
                Some("AC/DC, Earth & Fire".into()),
                "[]".into(),
                "[]".into(),
                None
            )
        );
    }

    #[tokio::test]
    async fn rows_read_before_artists_were_listed_are_read_again_as_the_same_tracks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        clip(root, "v/1.flac", &[]);
        clip(root, "v/2.flac", &[]);
        retag(
            &root.join("v/2.flac"),
            lofty::tag::TagType::VorbisComments,
            &[
                (lofty::prelude::ItemKey::TrackArtist, "Alder"),
                (lofty::prelude::ItemKey::TrackArtist, "Birch"),
            ],
        );
        let (pool, library) = library(root).await;
        scan(&pool, &library).await.unwrap();
        // Read again in place: the same track, so what points at it (plays, pins, playlists)
        // still does.
        let before: Vec<(i64, String)> =
            sqlx::query_as("SELECT id, rel_path FROM music_tracks ORDER BY rel_path")
                .fetch_all(&pool)
                .await
                .unwrap();
        sqlx::query("UPDATE music_tracks SET tags_version = 2, artist_names = '[]'")
            .execute(&pool)
            .await
            .unwrap();
        scan(&pool, &library).await.unwrap();
        let after: Vec<(i64, String)> =
            sqlx::query_as("SELECT id, rel_path FROM music_tracks ORDER BY rel_path")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(after, before);
        assert_eq!(artists(&pool, "v/2%").await.1, r#"["Alder","Birch"]"#);
    }

    #[tokio::test]
    async fn records_what_gapless_playback_needs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        clip(root, "A/B/01.flac", &[("title", "Lossless")]);
        clip(root, "A/B/02.mp3", &[("title", "Lossy")]);
        clip(root, "A/B/03.ogg", &[("title", "Vorbis")]);
        let (pool, library) = library(root).await;
        scan(&pool, &library).await.unwrap();
        let rows: Vec<(String, Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT title, encoder_delay, padding FROM music_tracks ORDER BY rel_path",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows[0], ("Lossless".into(), Some(0), Some(0)));
        assert_eq!((rows[1].0.as_str(), rows[1].1), ("Lossy", Some(576 + 529)));
        assert_eq!(rows[2], ("Vorbis".into(), None, None));
    }

    #[tokio::test]
    async fn an_empty_folder_fails_the_scan_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (pool, library) = library(dir.path()).await;
        assert!(scan(&pool, &library).await.is_err());
    }

    /// Each album's title and its artist's id, by title.
    async fn album_artists(pool: &SqlitePool) -> Vec<(String, i64)> {
        sqlx::query_as("SELECT title, artist_id FROM music_albums ORDER BY title")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    /// The artist's row id for a key.
    async fn artist_row(pool: &SqlitePool, key: &str) -> (i64, Option<i64>) {
        sqlx::query_as("SELECT id, merged_into FROM music_artists WHERE key = ?")
            .bind(key)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn an_artist_with_one_tagged_and_one_untagged_album_is_one_artist() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Read in either order: the untagged album's folder sorts first.
        let by = |album: &'static str| [("artist", "Radiohead"), ("album", album)];
        clip(root, "A/Kid A/01.flac", &by("Kid A"));
        clip(
            root,
            "B/OK Computer/01.flac",
            &[
                &by("OK Computer")[..],
                &[("MUSICBRAINZ_ALBUMARTISTID", ARTIST)],
            ]
            .concat(),
        );
        // Spelled another way, case and punctuation aside.
        clip(
            root,
            "C/Amnesiac/01.flac",
            &[("artist", "radio-head"), ("album", "Amnesiac")],
        );
        let (pool, library) = library(root).await;
        scan(&pool, &library).await.unwrap();

        let (tagged, _) = artist_row(&pool, &format!("mb:{ARTIST}")).await;
        let (named, joined) = artist_row(&pool, "name:radiohead").await;
        assert_eq!(
            joined,
            Some(tagged),
            "the old address leads to the one it joined"
        );
        assert_eq!(
            album_artists(&pool).await,
            [
                ("Amnesiac".into(), tagged),
                ("Kid A".into(), tagged),
                ("OK Computer".into(), tagged)
            ]
        );
        // Its tracks' credits join too.
        crate::track_artists::refresh(&pool).await.unwrap();
        let credited: Vec<i64> =
            sqlx::query_scalar("SELECT DISTINCT artist_id FROM music_track_artists")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(credited, [tagged]);

        // A database the old scan left split heals on the next scan.
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "UPDATE music_albums SET artist_id = {named} WHERE title = 'Kid A';
             UPDATE music_artists SET merged_into = NULL;"
        )))
        .execute(&pool)
        .await
        .unwrap();
        scan(&pool, &library).await.unwrap();
        assert!(album_artists(&pool).await.iter().all(|(_, a)| *a == tagged));
        assert_eq!(artist_row(&pool, "name:radiohead").await.1, Some(tagged));
    }

    #[tokio::test]
    async fn two_musicbrainz_artists_of_one_name_stay_two() {
        const OTHER: &str = "5b11f4ce-a62d-471e-81fc-a69a8278c7da";
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let by = |album: &'static str, id: &'static str| {
            [
                ("artist", "Nirvana"),
                ("album", album),
                ("MUSICBRAINZ_ALBUMARTISTID", id),
            ]
        };
        clip(root, "A/Nevermind/01.flac", &by("Nevermind", ARTIST));
        clip(root, "B/Local/01.flac", &by("Local", OTHER));
        clip(
            root,
            "C/Untagged/01.flac",
            &[("artist", "Nirvana"), ("album", "Untagged")],
        );
        let (pool, library) = library(root).await;
        scan(&pool, &library).await.unwrap();

        let (first, _) = artist_row(&pool, &format!("mb:{ARTIST}")).await;
        let (second, _) = artist_row(&pool, &format!("mb:{OTHER}")).await;
        let (named, joined) = artist_row(&pool, "name:nirvana").await;
        assert_eq!(joined, None, "a name matching several stays on its own");
        assert_eq!(
            album_artists(&pool).await,
            [
                ("Local".into(), second),
                ("Nevermind".into(), first),
                ("Untagged".into(), named)
            ]
        );
    }
}
