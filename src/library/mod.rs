pub mod absolute;
pub mod extras;
pub mod gapless;
mod music;
pub mod parse;
pub mod progress;
mod renamed;
mod scan;
mod successors;
pub mod tags;
mod title_files;
pub mod unparsed;
mod walk;
pub mod watch;

pub(crate) use music::{COVER_NAMES, IMAGE_EXTENSIONS};
pub(crate) use scan::AUDIO_EXTENSIONS;
pub(crate) use title_files::NfoIds;

use std::path::PathBuf;

use anyhow::bail;
use sqlx::SqlitePool;

pub use scan::{ScanReport, scan, scan_folder};
pub use successors::carry_progress;

/// Asks for the folder something was just put in to be scanned at once: its title folder in a
/// movies or shows library, or its whole music library; every library when it's in none.
pub async fn ask_scan_of(pool: &SqlitePool, dir: &std::path::Path) -> sqlx::Result<()> {
    match locate(pool, dir).await? {
        Some((id, folder)) => crate::jobs::request_folder_scan(id, folder),
        None => crate::jobs::request_rescan(),
    }
    Ok(())
}

/// The library `dir` is in, by id, and its title folder there (none for a music library, which
/// is scanned whole); `None` when it's in no library.
async fn locate(
    pool: &SqlitePool,
    dir: &std::path::Path,
) -> sqlx::Result<Option<(i64, Option<String>)>> {
    let libraries: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id, kind, path FROM libraries")
            .fetch_all(pool)
            .await?;
    Ok(libraries.into_iter().find_map(|(id, kind, path)| {
        let rest = dir.strip_prefix(&path).ok()?;
        let folder = rest
            .components()
            .next()
            .and_then(|c| c.as_os_str().to_str())
            .filter(|_| kind != "music")
            .map(str::to_owned);
        Some((id, folder))
    }))
}

/// What a library holds, which decides how it's scanned and matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryKind {
    Movies,
    Shows,
    Music,
}

impl LibraryKind {
    /// The kind as configured and stored: "movies", "shows" or "music".
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Movies => "movies",
            Self::Shows => "shows",
            Self::Music => "music",
        }
    }

    /// The kind named by "movies", "shows" or "music"; None for anything else.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "movies" => Some(Self::Movies),
            "shows" => Some(Self::Shows),
            "music" => Some(Self::Music),
            _ => None,
        }
    }
}

/// What a movies or shows library holds beyond its kind, so apps needn't guess from its
/// name: `MELETE_LIBRARY_<NAME>=<kind>,<category>:<path>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryCategory {
    Anime,
    Kids,
}

impl LibraryCategory {
    /// The category as configured and stored: "anime" or "kids".
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anime => "anime",
            Self::Kids => "kids",
        }
    }

    /// The category named by "anime" or "kids"; None for anything else.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "anime" => Some(Self::Anime),
            "kids" => Some(Self::Kids),
            _ => None,
        }
    }
}

/// A library as configured (`MELETE_LIBRARY_<NAME>=<kind>:<path>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryConfig {
    /// The library's name, from `MELETE_LIBRARY_<NAME>`, which identifies it across restarts.
    pub name: String,
    /// What it holds.
    pub kind: LibraryKind,
    /// Its folder, as Melete sees it.
    pub path: PathBuf,
}

/// A configured library with its database id.
#[derive(Debug, Clone)]
pub struct Library {
    /// The library's row id.
    pub id: i64,
    /// The library's name, from `MELETE_LIBRARY_<NAME>`.
    pub name: String,
    /// What it holds.
    pub kind: LibraryKind,
    /// Its folder, as Melete sees it.
    pub path: PathBuf,
}

/// Makes the `libraries` table match the configuration and returns the configured libraries.
///
/// Libraries dropped from the configuration keep their rows; they are just no longer scanned.
/// A new name for a library's folder and kind is a rename (`take_over_renamed`). Changing a
/// library's kind is refused, since its rows were built for the old kind.
pub async fn sync_config(
    pool: &SqlitePool,
    configs: &[LibraryConfig],
) -> anyhow::Result<Vec<Library>> {
    let mut libraries = Vec::with_capacity(configs.len());
    for config in configs {
        let existing: Option<String> =
            sqlx::query_scalar("SELECT kind FROM libraries WHERE name = ?")
                .bind(&config.name)
                .fetch_optional(pool)
                .await?;
        if let Some(kind) = &existing
            && kind != config.kind.as_str()
        {
            bail!(
                "library {:?} is configured as {} but was created as {kind}; use a new name instead",
                config.name,
                config.kind.as_str()
            );
        }
        let Some(path) = config.path.to_str() else {
            bail!("library {:?} path is not valid UTF-8", config.name);
        };
        if existing.is_none() {
            take_over_renamed(pool, config, path, configs).await?;
        }
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO libraries (name, kind, path) VALUES (?, ?, ?)
             ON CONFLICT (name) DO UPDATE SET path = excluded.path
             RETURNING id",
        )
        .bind(&config.name)
        .bind(config.kind.as_str())
        .bind(path)
        .fetch_one(pool)
        .await?;
        libraries.push(Library {
            id,
            name: config.name.clone(),
            kind: config.kind,
            path: config.path.clone(),
        });
    }
    Ok(libraries)
}

/// A library configured under a new name takes over the one no longer configured that was at
/// the same folder with the same kind, so its titles, files and everyone's progress carry on
/// rather than being adopted afresh. Where several were, which it is is unclear, and it's
/// adopted afresh as before.
async fn take_over_renamed(
    pool: &SqlitePool,
    config: &LibraryConfig,
    path: &str,
    configs: &[LibraryConfig],
) -> anyhow::Result<()> {
    let names: Vec<&str> = configs.iter().map(|c| c.name.as_str()).collect();
    let earlier: Vec<String> =
        sqlx::query_scalar("SELECT name FROM libraries WHERE kind = ? AND path = ?")
            .bind(config.kind.as_str())
            .bind(path)
            .fetch_all(pool)
            .await?;
    let earlier: Vec<String> = earlier
        .into_iter()
        .filter(|name| !names.contains(&name.as_str()))
        .collect();
    match earlier.as_slice() {
        [] => {}
        [old] => {
            sqlx::query("UPDATE libraries SET name = ? WHERE name = ?")
                .bind(&config.name)
                .bind(old)
                .execute(pool)
                .await?;
            tracing::info!(
                from = old,
                to = config.name,
                "library renamed; it keeps everything"
            );
        }
        several => tracing::warn!(
            library = config.name,
            earlier = ?several,
            "several libraries no longer configured were at this folder; adopting it afresh"
        ),
    }
    Ok(())
}

/// Gives each configured library the category its configuration names, or none. Unlike its
/// kind, a library's category may change: nothing stored depends on it.
pub async fn sync_categories(
    pool: &SqlitePool,
    configs: &[LibraryConfig],
    categories: &[(String, LibraryCategory)],
) -> anyhow::Result<()> {
    for config in configs {
        let category = categories
            .iter()
            .find(|(name, _)| *name == config.name)
            .map(|(_, category)| category.as_str());
        sqlx::query("UPDATE libraries SET category = ? WHERE name = ?")
            .bind(category)
            .bind(&config.name)
            .execute(pool)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn categories(pool: &SqlitePool) -> Vec<(String, Option<String>)> {
        sqlx::query_as("SELECT name, category FROM libraries ORDER BY name")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn categories_follow_the_configuration() {
        let pool = db::open_in_memory().await;
        let configs = [
            config("anime", LibraryKind::Shows, "/a"),
            config("shows", LibraryKind::Shows, "/s"),
        ];
        sync_config(&pool, &configs).await.unwrap();
        sync_categories(&pool, &configs, &[("anime".into(), LibraryCategory::Anime)])
            .await
            .unwrap();
        assert_eq!(
            categories(&pool).await,
            [
                ("anime".to_owned(), Some("anime".to_owned())),
                ("shows".to_owned(), None)
            ]
        );
        // Changed or taken away in the configuration, changed in the database.
        sync_categories(&pool, &configs, &[("shows".into(), LibraryCategory::Kids)])
            .await
            .unwrap();
        assert_eq!(
            categories(&pool).await,
            [
                ("anime".to_owned(), None),
                ("shows".to_owned(), Some("kids".to_owned()))
            ]
        );
    }

    #[tokio::test]
    async fn music_libraries_have_no_category() {
        let pool = db::open_in_memory().await;
        let configs = [config("music", LibraryKind::Music, "/m")];
        sync_config(&pool, &configs).await.unwrap();
        let err = sync_categories(&pool, &configs, &[("music".into(), LibraryCategory::Kids)])
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("CHECK"), "{err:#}");
    }

    fn config(name: &str, kind: LibraryKind, path: &str) -> LibraryConfig {
        LibraryConfig {
            name: name.into(),
            kind,
            path: path.into(),
        }
    }

    #[tokio::test]
    async fn sync_creates_then_updates_path() {
        let pool = db::open_in_memory().await;
        let first = sync_config(&pool, &[config("movies", LibraryKind::Movies, "/a")])
            .await
            .unwrap();
        let second = sync_config(&pool, &[config("movies", LibraryKind::Movies, "/b")])
            .await
            .unwrap();
        assert_eq!(first[0].id, second[0].id);
        let path: String = sqlx::query_scalar("SELECT path FROM libraries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(path, "/b");
    }

    #[tokio::test]
    async fn sync_refuses_kind_change() {
        let pool = db::open_in_memory().await;
        sync_config(&pool, &[config("anime", LibraryKind::Shows, "/a")])
            .await
            .unwrap();
        let err = sync_config(&pool, &[config("anime", LibraryKind::Movies, "/a")])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("created as shows"));
    }

    #[tokio::test]
    async fn a_library_given_a_new_name_keeps_everything() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Show")).unwrap();
        std::fs::write(dir.path().join("Show/S01E01.mkv"), b"pilot").unwrap();
        let path = dir.path().to_str().unwrap();
        let pool = db::open_in_memory().await;
        let before = sync_config(&pool, &[config("tv", LibraryKind::Shows, path)])
            .await
            .unwrap();
        scan(&pool, &before[0]).await.unwrap();
        let user = crate::auth::create_user(&pool, "alice", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        sqlx::query(
            "INSERT INTO playback_progress (user_id, file_id, position_seconds, watched, updated_at)
             SELECT ?, id, 0, 1, 0 FROM media_files",
        )
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();

        // Renamed, beside another library: the same folder and kind is the same library.
        let configs = [
            config("movies", LibraryKind::Movies, "/m"),
            config("shows", LibraryKind::Shows, path),
        ];
        let after = sync_config(&pool, &configs).await.unwrap();
        assert_eq!(after[1].id, before[0].id);
        let report = scan(&pool, &after[1]).await.unwrap();
        assert_eq!(
            (report.files, report.added),
            (1, 0),
            "nothing adopted afresh"
        );
        let watched: Vec<(String, bool)> = sqlx::query_as(
            "SELECT l.name, p.watched FROM playback_progress p
             JOIN media_files f ON f.id = p.file_id JOIN libraries l ON l.id = f.library_id
             WHERE f.missing_since IS NULL",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(watched, [("shows".to_owned(), true)]);

        // A different kind at that folder, or a different folder, is another library.
        let other = sync_config(&pool, &[config("films", LibraryKind::Movies, path)])
            .await
            .unwrap();
        assert_ne!(other[0].id, before[0].id);
        let elsewhere = sync_config(&pool, &[config("tv", LibraryKind::Shows, "/elsewhere")])
            .await
            .unwrap();
        assert_ne!(elsewhere[0].id, before[0].id);
    }
}
