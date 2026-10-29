//! What else a title folder holds, read and never written: the ids in its `.nfo` files (as
//! Kodi, Jellyfin, Radarr and Sonarr write them) and its own poster and backdrop images.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::scan::FoundFile;

mod recording;

pub(super) use recording::{ReadIds, read_changed_nfo, record};

/// An `.nfo` file larger than this isn't read: real ones are a few kilobytes.
const MAX_NFO_BYTES: u64 = 1 << 20;

/// Image file types a title folder's artwork may have.
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// Names of a title's poster, best first, then of its backdrop.
const POSTER_NAMES: &[&str] = &["poster", "folder"];
const BACKDROP_NAMES: &[&str] = &["fanart", "backdrop"];

/// The ids a title folder's `.nfo` files give, each kind deduplicated. More than one of a kind
/// means the files disagree.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NfoIds {
    /// TMDB ids.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub tmdb: BTreeSet<i64>,
    /// IMDb ids, as `tt` and digits.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub imdb: BTreeSet<String>,
    /// TheTVDB ids.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub tvdb: BTreeSet<i64>,
}

impl NfoIds {
    /// Whether no id was found.
    pub fn is_empty(&self) -> bool {
        self.tmdb.is_empty() && self.imdb.is_empty() && self.tvdb.is_empty()
    }

    fn add(&mut self, kind: &str, value: &str) {
        let value = value.trim();
        match kind.to_ascii_lowercase().as_str() {
            "tmdb" | "tmdbid" => {
                if let Ok(id) = value.parse::<i64>()
                    && id > 0
                {
                    self.tmdb.insert(id);
                }
            }
            "tvdb" | "tvdbid" => {
                if let Ok(id) = value.parse::<i64>()
                    && id > 0
                {
                    self.tvdb.insert(id);
                }
            }
            "imdb" | "imdbid" | "id" => {
                let digits = value.strip_prefix("tt").unwrap_or_default();
                if (7..=10).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()) {
                    self.imdb.insert(value.to_owned());
                }
            }
            _ => {}
        }
    }

    fn merge(&mut self, other: NfoIds) {
        self.tmdb.extend(other.tmdb);
        self.imdb.extend(other.imdb);
        self.tvdb.extend(other.tvdb);
    }
}

/// The ids in one `.nfo` file: `<tmdbid>`, `<imdbid>`, `<tvdbid>`, `<uniqueid type="…">` and an
/// `<id>` that is an IMDb id, read only directly under its `<movie>` or `<tvshow>` (an actor's
/// or a set's ids are someone else's). Not XML, or another kind of `.nfo` (a release's text
/// file, an episode's), gives none. Read as UTF-8, any other byte read as a stand-in, since the
/// ids are plain ASCII either way.
pub(crate) fn parse_nfo(bytes: &[u8]) -> NfoIds {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}');
    let mut ids = NfoIds::default();
    let mut reader = Reader::from_str(text);
    let mut depth = 0usize;
    // The id field being read (its kind), and its text so far.
    let mut field: Option<String> = None;
    let mut value = String::new();
    loop {
        let Ok(event) = reader.read_event() else {
            return ids;
        };
        match event {
            Event::Start(e) => {
                let name = e.local_name().as_ref().to_ascii_lowercase();
                depth += 1;
                if depth == 1 && name != "movie" && name != "tvshow" {
                    return ids;
                }
                if depth == 2 {
                    field = match name.as_str() {
                        "uniqueid" => e
                            .try_get_attribute("type")
                            .ok()
                            .flatten()
                            .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok())
                            .map(|v| v.into_owned()),
                        "tmdbid" | "imdbid" | "tvdbid" | "id" => Some(name),
                        _ => None,
                    };
                    value.clear();
                }
            }
            Event::Text(t) if field.is_some() => value.push_str(&t.xml10_content()),
            Event::End(_) => {
                if depth == 2
                    && let Some(kind) = field.take()
                {
                    ids.add(&kind, &value);
                }
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return ids;
                }
            }
            Event::Eof => return ids,
            _ => {}
        }
    }
}

/// A title folder's own files beside its videos, as a scan found them.
#[derive(Default)]
pub(super) struct TitleFiles<'a> {
    /// Its `.nfo` files: `movie.nfo` or `tvshow.nfo`, and those named after one of its videos.
    pub(super) nfo: Vec<&'a FoundFile>,
    /// Its poster, `poster.*` before `folder.*`.
    pub(super) poster: Option<&'a FoundFile>,
    /// Its backdrop, `fanart.*` before `backdrop.*`.
    pub(super) backdrop: Option<&'a FoundFile>,
}

/// Each title folder's own files, by the folder's path in the library.
pub(super) type ByTitle<'a> = HashMap<&'a str, TitleFiles<'a>>;

impl TitleFiles<'_> {
    /// What the `.nfo` files were when read, to read them again only when one changed.
    pub(super) fn nfo_signature(&self) -> Option<String> {
        if self.nfo.is_empty() {
            return None;
        }
        let mut parts: Vec<String> = self
            .nfo
            .iter()
            .map(|f| format!("{}:{}:{}", f.rel_path, f.size, f.modified))
            .collect();
        parts.sort();
        Some(parts.join("\n"))
    }
}

/// Whether a file directly in a title folder may be one of its `.nfo` files or its artwork,
/// which the scan then lists beside the videos.
pub(super) fn is_title_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some((stem, extension)) = lower.rsplit_once('.') else {
        return false;
    };
    extension == "nfo"
        || (IMAGE_EXTENSIONS.contains(&extension)
            && (POSTER_NAMES.contains(&stem) || BACKDROP_NAMES.contains(&stem)))
}

/// Sorts the title files a scan found into their title folders, keeping only the `.nfo` files
/// that belong to the title (`tvshow.nfo` for a series; `movie.nfo` or one named after a video
/// for a movie) and the best poster and backdrop.
pub(super) fn by_title<'a>(
    found: &'a [FoundFile],
    videos: &[FoundFile],
    series: bool,
) -> ByTitle<'a> {
    let video_stems: BTreeSet<(&str, String)> = videos
        .iter()
        .filter_map(|v| {
            let (folder, rest) = v.rel_path.split_once('/')?;
            let stem = Path::new(rest).file_stem()?.to_str()?;
            Some((folder, stem.to_ascii_lowercase()))
        })
        .collect();
    let mut titles: HashMap<&str, TitleFiles> = HashMap::new();
    for file in found {
        let Some((folder, name)) = file.rel_path.split_once('/') else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        let Some((stem, extension)) = lower.rsplit_once('.') else {
            continue;
        };
        let entry = titles.entry(folder).or_default();
        if extension == "nfo" {
            let belongs = if series {
                stem == "tvshow"
            } else {
                stem == "movie" || video_stems.contains(&(folder, stem.to_owned()))
            };
            if belongs {
                entry.nfo.push(file);
            }
        } else if let Some(rank) = POSTER_NAMES.iter().position(|n| *n == stem) {
            keep_best(&mut entry.poster, file, rank, POSTER_NAMES);
        } else if let Some(rank) = BACKDROP_NAMES.iter().position(|n| *n == stem) {
            keep_best(&mut entry.backdrop, file, rank, BACKDROP_NAMES);
        }
    }
    titles
}

/// Keeps `file` if its name ranks above the one kept (or equal and sorting first, so the
/// choice doesn't depend on the order a folder lists its files in).
fn keep_best<'a>(
    kept: &mut Option<&'a FoundFile>,
    file: &'a FoundFile,
    rank: usize,
    names: &[&str],
) {
    let rank_of = |f: &FoundFile| {
        let name = f
            .rel_path
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let stem = name
            .rsplit_once('.')
            .map_or(name.clone(), |(s, _)| s.to_owned());
        names.iter().position(|n| *n == stem).unwrap_or(usize::MAX)
    };
    let better = match kept {
        None => true,
        Some(old) => (rank, &file.rel_path) < (rank_of(old), &old.rel_path),
    };
    if better {
        *kept = Some(file);
    }
}

/// Reads and merges the ids of a title's `.nfo` files (by path in the library at `root`, with
/// their sizes). A file that can't be read, or is too large, gives none.
fn read_nfo_ids(root: &Path, files: &[(String, i64)]) -> NfoIds {
    let mut ids = NfoIds::default();
    for (rel_path, size) in files {
        if u64::try_from(*size).unwrap_or(u64::MAX) > MAX_NFO_BYTES {
            continue;
        }
        match std::fs::read(root.join(rel_path)) {
            Ok(bytes) if bytes.len() as u64 <= MAX_NFO_BYTES => ids.merge(parse_nfo(&bytes)),
            Ok(_) => {}
            Err(err) => tracing::warn!(
                rel_path,
                error = %err,
                "couldn't read an .nfo file; its ids are left out"
            ),
        }
    }
    ids
}

/// The artwork name a title's own image is served under: unique to the file as it is now
/// (its library, path, size and date), so a changed image gets a new URL, and saying nothing
/// about the path.
fn artwork_name(library_id: i64, file: &FoundFile) -> String {
    let digest = Sha256::digest(format!(
        "{library_id}\0{}\0{}\0{}",
        file.rel_path, file.size, file.modified
    ));
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("/local-{hex}.jpg")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(rel_path: &str) -> FoundFile {
        FoundFile {
            rel_path: rel_path.into(),
            size: 10,
            modified: 1,
        }
    }

    #[test]
    fn reads_ids_as_kodi_jellyfin_and_the_arrs_write_them() {
        let radarr = br#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
            <movie><title>The Matrix</title><tmdbid>603</tmdbid><imdbid>tt0133093</imdbid>
            <actor><name>Keanu Reeves</name><tmdbid>6384</tmdbid></actor></movie>"#;
        let ids = parse_nfo(radarr);
        assert_eq!(
            ids.tmdb,
            BTreeSet::from([603]),
            "an actor's id isn't the film's"
        );
        assert_eq!(ids.imdb, BTreeSet::from(["tt0133093".to_owned()]));

        let kodi = "\u{feff}<tvshow><title>Les Revenants</title>\
            <uniqueid type=\"tvdb\" default=\"true\">261742</uniqueid>\
            <uniqueid type=\"TMDB\">46896</uniqueid><id>261742</id></tvshow>";
        let ids = parse_nfo(kodi.as_bytes());
        assert_eq!(ids.tvdb, BTreeSet::from([261742]));
        assert_eq!(ids.tmdb, BTreeSet::from([46896]));
        assert!(
            ids.imdb.is_empty(),
            "a bare number in <id> isn't an IMDb id"
        );
    }

    #[test]
    fn anything_else_gives_no_ids() {
        for nfo in [
            &b"RELEASE.NAME.1999.1080p\nimdb: http://www.imdb.com/title/tt0133093/"[..],
            b"<episodedetails><tmdbid>1</tmdbid></episodedetails>",
            b"<movie><tmdbid>603",
            b"<movie><tmdbid>-4</tmdbid><imdbid>nm0000206</imdbid></movie>",
            b"\xff\xfe<\x00",
        ] {
            assert!(
                parse_nfo(nfo).is_empty(),
                "{}",
                String::from_utf8_lossy(nfo)
            );
        }
        let latin1 = b"<movie><title>Am\xe9lie</title><tmdbid>194</tmdbid></movie>";
        assert_eq!(parse_nfo(latin1).tmdb, BTreeSet::from([194]));
    }

    #[test]
    fn sorts_out_each_folders_own_files() {
        let videos = [found("Film (2000)/Film.mkv"), found("Show/S01E01.mkv")];
        let files = [
            found("Film (2000)/movie.nfo"),
            found("Film (2000)/Film.nfo"),
            found("Film (2000)/Other.nfo"),
            found("Film (2000)/folder.jpg"),
            found("Film (2000)/Poster.PNG"),
            found("Film (2000)/backdrop.webp"),
        ];
        let titles = by_title(&files, &videos, false);
        let film = &titles["Film (2000)"];
        let nfo: Vec<&str> = film.nfo.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(nfo, ["Film (2000)/movie.nfo", "Film (2000)/Film.nfo"]);
        assert_eq!(film.poster.unwrap().rel_path, "Film (2000)/Poster.PNG");
        assert_eq!(film.backdrop.unwrap().rel_path, "Film (2000)/backdrop.webp");

        let show_files = [found("Show/tvshow.nfo"), found("Show/S01E01.nfo")];
        let shows = by_title(&show_files, &videos, true);
        assert_eq!(
            shows["Show"].nfo.len(),
            1,
            "an episode's .nfo isn't the show's"
        );

        assert!(is_title_file("fanart.jpeg"));
        assert!(is_title_file("Movie.NFO"));
        assert!(!is_title_file("poster.gif"));
        assert!(!is_title_file("cover.jpg"));
    }

    #[test]
    fn artwork_names_change_with_the_file_and_hide_its_path() {
        let a = artwork_name(1, &found("Film/poster.jpg"));
        assert!(a.starts_with("/local-") && a.ends_with(".jpg") && !a.contains("Film"));
        let mut changed = found("Film/poster.jpg");
        changed.modified = 2;
        assert_ne!(a, artwork_name(1, &changed));
        assert_ne!(a, artwork_name(2, &found("Film/poster.jpg")));
    }

    type Row = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );

    async fn row(pool: &sqlx::SqlitePool) -> Row {
        sqlx::query_as(
            "SELECT nfo_ids, local_poster, local_backdrop, local_poster_key, match_status
             FROM movies",
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn scans_read_nfo_ids_and_artwork_and_write_nothing() {
        use crate::library::{LibraryConfig, LibraryKind, sync_config};
        let dir = tempfile::tempdir().unwrap();
        let film = dir.path().join("Film (2000)");
        std::fs::create_dir_all(&film).unwrap();
        std::fs::write(film.join("Film.mkv"), b"video").unwrap();
        std::fs::write(
            film.join("movie.nfo"),
            b"<movie><tmdbid>603</tmdbid></movie>",
        )
        .unwrap();
        std::fs::write(film.join("folder.jpg"), b"jpeg").unwrap();
        std::fs::write(film.join("fanart.png"), b"png").unwrap();
        let pool = crate::db::open_in_memory().await;
        let config = LibraryConfig {
            name: "m".into(),
            kind: LibraryKind::Movies,
            path: dir.path().into(),
        };
        let library = sync_config(&pool, &[config]).await.unwrap().remove(0);
        let listing = |p: &Path| {
            let mut names: Vec<String> = std::fs::read_dir(p)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        };
        let before = listing(&film);

        let report = crate::library::scan(&pool, &library).await.unwrap();
        assert_eq!(
            report.files, 1,
            "neither the .nfo nor the images are videos"
        );
        let (ids, poster, backdrop, key, _) = row(&pool).await;
        assert_eq!(ids.as_deref(), Some(r#"{"tmdb":[603]}"#));
        assert_eq!(poster.as_deref(), Some("Film (2000)/folder.jpg"));
        assert_eq!(backdrop.as_deref(), Some("Film (2000)/fanart.png"));
        assert!(key.unwrap().starts_with("/local-"));
        assert_eq!(listing(&film), before, "nothing is written into the folder");

        // A title matched by searching is matched again when its .nfo changes...
        sqlx::query("UPDATE movies SET match_status = 'matched', match_method = 'search'")
            .execute(&pool)
            .await
            .unwrap();
        crate::library::scan(&pool, &library).await.unwrap();
        assert_eq!(
            row(&pool).await.4.as_deref(),
            Some("matched"),
            "unchanged, unread"
        );
        std::fs::write(
            film.join("movie.nfo"),
            b"<movie><tmdbid>604</tmdbid></movie>",
        )
        .unwrap();
        // Edited later than the scan before: a file's date is what says it changed.
        std::fs::File::options()
            .write(true)
            .open(film.join("movie.nfo"))
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(60))
            .unwrap();
        std::fs::write(film.join("poster.jpg"), b"a better poster").unwrap();
        crate::library::scan(&pool, &library).await.unwrap();
        let (ids, poster, _, _, status) = row(&pool).await;
        assert_eq!(ids.as_deref(), Some(r#"{"tmdb":[604]}"#));
        assert_eq!(poster.as_deref(), Some("Film (2000)/poster.jpg"));
        assert_eq!(status, None);

        // ...but never one matched by hand.
        sqlx::query("UPDATE movies SET match_status = 'matched', match_method = 'manual'")
            .execute(&pool)
            .await
            .unwrap();
        std::fs::remove_file(film.join("movie.nfo")).unwrap();
        std::fs::remove_file(film.join("poster.jpg")).unwrap();
        crate::library::scan(&pool, &library).await.unwrap();
        let (ids, poster, _, _, status) = row(&pool).await;
        assert_eq!(
            (ids, poster.as_deref()),
            (None, Some("Film (2000)/folder.jpg"))
        );
        assert_eq!(status.as_deref(), Some("matched"));
    }
}
