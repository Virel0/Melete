//! Setting aside a file an admin says is the wrong one for its title: only a file Melete itself
//! put in the library from a download, never one adopted from the existing library. It goes to
//! the library's trash as an upgrade's old file does (restorable, deleted after 30 days), the
//! release it came from is blocklisted, and the download counts as failed so the title can be
//! grabbed again.

use super::*;
use crate::search::Target;

/// Why a file wasn't set aside.
#[derive(Debug, PartialEq, Eq)]
pub enum NotSetAside {
    /// No such file in the library now.
    NotFound,
    /// Melete didn't bring it in from a download: it's part of the existing library.
    Adopted,
    /// It couldn't be moved; why.
    Failed(String),
}

/// A file set aside as the wrong one.
#[derive(Debug, PartialEq, Eq)]
pub struct SetAside {
    /// Its trash entry, for restoring it.
    pub trash_id: i64,
    /// The download it came from.
    pub download_id: i64,
    /// The release, now blocklisted.
    pub release: String,
    /// The search the download was for, to search again.
    pub target: Option<Target>,
}

#[derive(sqlx::FromRow)]
struct WrongRow {
    id: i64,
    library_id: i64,
    root: String,
    rel_path: String,
    size: i64,
    modified_at: i64,
}

#[derive(sqlx::FromRow)]
struct FromDownload {
    id: i64,
    title: String,
    wanted_movie_id: Option<i64>,
    wanted_series_id: Option<i64>,
    season: Option<i64>,
    episode: Option<i64>,
}

/// The download that put the file at `path` in the library, if one did: its record of the files
/// it placed names it.
pub async fn imported_by(pool: &SqlitePool, path: &Path) -> sqlx::Result<Option<i64>> {
    Ok(from_download(pool, path).await?.map(|d| d.id))
}

async fn from_download(pool: &SqlitePool, path: &Path) -> sqlx::Result<Option<FromDownload>> {
    sqlx::query_as(
        "SELECT id, title, wanted_movie_id, wanted_series_id, season, episode FROM downloads
         WHERE (wanted_movie_id IS NOT NULL OR wanted_series_id IS NOT NULL)
           AND EXISTS (SELECT 1 FROM json_each(placed) WHERE value = ?)
         ORDER BY id DESC LIMIT 1",
    )
    .bind(path.to_string_lossy())
    .fetch_optional(pool)
    .await
}

/// Sets a movie or episode file aside as the wrong one: moved, never over anything, to
/// `.melete-trash/<download>/` at the top of its library, gone from the library, its release
/// blocklisted and its download failed.
pub async fn set_aside(
    pool: &SqlitePool,
    file_id: i64,
) -> anyhow::Result<Result<SetAside, NotSetAside>> {
    let _one_at_a_time = TRASH_LOCK.lock().await;
    let row: Option<WrongRow> = sqlx::query_as(
        "SELECT f.id, f.library_id, l.path AS root, f.rel_path, f.size, f.modified_at
         FROM media_files f JOIN libraries l ON l.id = f.library_id
         WHERE f.id = ? AND f.missing_since IS NULL
           AND (f.movie_id IS NOT NULL OR f.series_id IS NOT NULL)",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(Err(NotSetAside::NotFound));
    };
    let root = PathBuf::from(&row.root);
    let Some(from) = inside(&root, &row.rel_path) else {
        return Ok(Err(NotSetAside::Adopted));
    };
    let Some(download) = from_download(pool, &from).await? else {
        return Ok(Err(NotSetAside::Adopted));
    };
    let trash_path = Path::new(TRASH)
        .join(download.id.to_string())
        .join(&row.rel_path)
        .to_string_lossy()
        .into_owned();
    let Some(to) = inside(&root, &trash_path) else {
        return Ok(Err(NotSetAside::Failed(
            "its path isn't a plain one inside the library".into(),
        )));
    };
    let (size, modified) = (row.size, row.modified_at);
    let (from_moved, to_moved) = (from.clone(), to.clone());
    let moved =
        tokio::task::spawn_blocking(move || move_as_scanned(&from, &to, size, modified)).await?;
    if let Err(why) = moved {
        return Ok(Err(NotSetAside::Failed(why)));
    }
    let trash_id = match record(pool, &row, &trash_path, download.id).await {
        Ok(id) => id,
        Err(err) => {
            // Unrecorded, it would sit in the trash forever: put it back.
            put_back(from_moved, to_moved, &err).await?;
            return Err(err.into());
        }
    };
    crate::blocklist::add(pool, crate::blocklist::From::Download(download.id)).await?;
    let target = match (
        download.wanted_movie_id,
        download.wanted_series_id,
        download.season,
    ) {
        (Some(id), ..) => Some(Target::Movie(id)),
        (None, Some(id), Some(season)) => Some(Target::Series {
            id,
            season,
            episode: download.episode,
        }),
        _ => None,
    };
    Ok(Ok(SetAside {
        trash_id,
        download_id: download.id,
        release: download.title,
        target,
    }))
}

/// Moves the file into the trash if it's still the regular file that was scanned; why not when
/// it isn't. Blocking.
fn move_as_scanned(from: &Path, to: &Path, size: i64, modified: i64) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(from).map_err(|err| err.to_string())?;
    let seconds = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    if !meta.file_type().is_file() || meta.len() as i64 != size || seconds != Some(modified) {
        return Err("on disk it isn't the file that was scanned; rescan its folder first".into());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("making the trash folder: {err}"))?;
    }
    crate::transfer::move_without_replacing(from, to)
}

/// Records the file as trashed and gone from the library, and its download as failed, in one
/// transaction; the trash entry's id.
async fn record(
    pool: &SqlitePool,
    row: &WrongRow,
    trash_path: &str,
    download_id: i64,
) -> sqlx::Result<i64> {
    let at = unix_now();
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO trashed_files (library_id, rel_path, trash_path, size, download_id,
             replaced_by, trashed_at, reason)
         VALUES (?, ?, ?, ?, ?, '', ?, 'wrong') RETURNING id",
    )
    .bind(row.library_id)
    .bind(&row.rel_path)
    .bind(trash_path)
    .bind(row.size)
    .bind(download_id)
    .bind(at)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE media_files SET missing_since = coalesce(missing_since, ?) WHERE id = ?")
        .bind(at)
        .bind(row.id)
        .execute(&mut *tx)
        .await?;
    // Failed, so it no longer counts as on its way and the title can be grabbed again.
    sqlx::query(
        "UPDATE downloads SET state = 'failed', updated_at = ?,
             message = 'an admin said its file is the wrong one: the file is in the trash and the release is blocklisted'
         WHERE id = ?",
    )
    .bind(at)
    .bind(download_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}
