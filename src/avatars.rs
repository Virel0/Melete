//! Profile pictures: each user's own, kept in the database, and the gallery they can pick one
//! from: Melete's own pictures, shipped with it, and a folder on the server the household fills
//! (`MELETE_AVATARS_DIR`), one subfolder per category. Nothing is fetched from anywhere.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use crate::aoide::library::{MAX_IMAGE_BYTES, sha256_hex};

/// Where Melete's own pictures are in its image.
pub const OFFICIAL_DIR: &str = "/usr/share/melete/avatars";

/// The gallery's two sources: Melete's own pictures, and the household's folder.
#[derive(Debug, Clone, Default)]
pub struct Gallery {
    /// Melete's own pictures, files directly in it; none when the folder isn't there.
    pub official: Option<PathBuf>,
    /// The household's folder (`MELETE_AVATARS_DIR`), a subfolder per category.
    pub folder: Option<PathBuf>,
}

/// A category of the gallery: its id ("official", or "gallery/" and the subfolder's name), the
/// name shown, and how many pictures it holds.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub count: usize,
}

/// A picture's file extension says it's one Melete takes.
fn is_picture(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !lower.starts_with('.')
        && [".png", ".jpg", ".jpeg", ".webp"]
            .iter()
            .any(|e| lower.ends_with(e))
}

/// A name that stays inside its folder: no separators, not "." or "..", not hidden.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\', '\0'])
        && name.len() <= 255
}

/// The picture files directly in `dir`, by name; none when it can't be read. Links are left
/// out, so nothing outside the folder is reached.
fn pictures_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| is_picture(n))
        .collect();
    names.sort();
    names
}

impl Gallery {
    /// The categories, Melete's own first, then the folder's by name; empty ones left out.
    pub fn categories(&self) -> Vec<Category> {
        let mut found = Vec::new();
        if let Some(official) = &self.official {
            let count = pictures_in(official).len();
            if count > 0 {
                found.push(Category {
                    id: "official".into(),
                    name: "Melete".into(),
                    count,
                });
            }
        }
        if let Some(folder) = &self.folder {
            let mut subfolders: Vec<String> = std::fs::read_dir(folder)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| plain_name(n))
                .collect();
            subfolders.sort_by_key(|n| n.to_lowercase());
            for name in subfolders {
                let count = pictures_in(&folder.join(&name)).len();
                if count > 0 {
                    found.push(Category {
                        id: format!("gallery/{name}"),
                        name,
                        count,
                    });
                }
            }
        }
        found
    }

    /// The folder a category id names, when it's one of the gallery's.
    fn dir_of(&self, category: &str) -> Option<PathBuf> {
        if category == "official" {
            return self.official.clone();
        }
        let name = category.strip_prefix("gallery/")?;
        plain_name(name).then(|| self.folder.as_ref().map(|f| f.join(name)))?
    }

    /// A page of a category's pictures, as ids ("<category>/<file>"), and how many it holds.
    pub fn pictures(
        &self,
        category: &str,
        offset: usize,
        limit: usize,
    ) -> Option<(usize, Vec<String>)> {
        let dir = self.dir_of(category).filter(|d| d.is_dir())?;
        let all = pictures_in(&dir);
        let page = all
            .iter()
            .skip(offset)
            .take(limit)
            .map(|n| format!("{category}/{n}"))
            .collect();
        Some((all.len(), page))
    }

    /// The file a picture id names, when it's a picture in the gallery: the name checked, and
    /// a plain file (not a link) found where it says.
    pub fn file(&self, id: &str) -> Option<PathBuf> {
        let (category, name) = id.rsplit_once('/')?;
        if !plain_name(name) || !is_picture(name) {
            return None;
        }
        let path = self.dir_of(category)?.join(name);
        std::fs::symlink_metadata(&path)
            .ok()
            .filter(|m| m.file_type().is_file())
            .map(|_| path)
    }
}

/// A user's picture: its bytes, type and hash.
pub struct Avatar {
    pub bytes: Vec<u8>,
    pub mime: String,
    pub hash: String,
}

/// Sets a user's picture; `bytes` were checked to be of type `mime`.
///
/// # Errors
///
/// A database error, or a picture over the size limit.
pub async fn set(pool: &SqlitePool, user_id: i64, mime: &str, bytes: &[u8]) -> anyhow::Result<()> {
    anyhow::ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_IMAGE_BYTES,
        "a picture is 1 byte to 5 MB"
    );
    sqlx::query(
        "INSERT INTO user_avatars (user_id, image_hash, mime_type, bytes, size, updated_at)
         VALUES (?, ?, ?, ?, ?, unixepoch())
         ON CONFLICT (user_id) DO UPDATE SET image_hash = excluded.image_hash,
             mime_type = excluded.mime_type, bytes = excluded.bytes, size = excluded.size,
             updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(sha256_hex(bytes))
    .bind(mime)
    .bind(bytes)
    .bind(bytes.len() as i64)
    .execute(pool)
    .await?;
    Ok(())
}

/// Removes a user's picture; whether there was one.
///
/// # Errors
///
/// A database error.
pub async fn clear(pool: &SqlitePool, user_id: i64) -> sqlx::Result<bool> {
    let done = sqlx::query("DELETE FROM user_avatars WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// A user's picture, when they have one.
///
/// # Errors
///
/// A database error.
pub async fn get(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Option<Avatar>> {
    let row: Option<(Vec<u8>, String, String)> =
        sqlx::query_as("SELECT bytes, mime_type, image_hash FROM user_avatars WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(bytes, mime, hash)| Avatar { bytes, mime, hash }))
}

/// Where a user's picture is fetched from, its hash in the address so a new one isn't cached
/// as the old.
pub fn url(user_id: i64, hash: &str) -> String {
    format!(
        "/api/v1/users/{user_id}/avatar?v={}",
        &hash[..hash.len().min(12)]
    )
}

/// A user's picture address; none without a picture.
///
/// # Errors
///
/// A database error.
pub async fn url_of(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Option<String>> {
    let hash: Option<String> =
        sqlx::query_scalar("SELECT image_hash FROM user_avatars WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(hash.map(|h| url(user_id, &h)))
}

/// Every user's picture address, by user id, for lists of people.
///
/// # Errors
///
/// A database error.
pub async fn urls(pool: &SqlitePool) -> sqlx::Result<std::collections::HashMap<i64, String>> {
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT user_id, image_hash FROM user_avatars")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, hash)| (id, url(id, &hash)))
        .collect())
}

#[cfg(test)]
pub(crate) mod tests;
