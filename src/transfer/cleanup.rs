//! The folders a download's copy makes, recorded before each is made, and taking a download's
//! part files and empty folders away again when it fails or is held back.

use super::*;

/// Makes `dir` and any folders above it that are missing, recording each one before it's made
/// (`downloads.created_dirs`), so taking the download away again removes only folders it made,
/// across retries and restarts.
pub(super) async fn make_dirs(pool: &SqlitePool, id: i64, dir: &Path) -> Result<(), Stop> {
    let probe = dir.to_path_buf();
    let missing: Vec<PathBuf> = blocking(move || {
        Ok(probe
            .ancestors()
            .take_while(|d| std::fs::symlink_metadata(d).is_err())
            .map(Path::to_path_buf)
            .collect())
    })
    .await?;
    for folder in &missing {
        sqlx::query(
            "UPDATE downloads SET created_dirs = json_insert(created_dirs, '$[#]', ?1)
             WHERE id = ?2 AND NOT EXISTS (SELECT 1 FROM json_each(created_dirs) WHERE value = ?1)",
        )
        .bind(folder.to_string_lossy())
        .bind(id)
        .execute(pool)
        .await
        .map_err(|err| Stop::Retry(format!("the database: {err}")))?;
    }
    let dir = dir.to_path_buf();
    blocking(move || create_dirs(&dir)).await
}

/// Deletes the download's part files ([`delete_partials`]) and then the folders its copy made,
/// deepest first, each only if it's an empty folder inside a library; what it can't is logged
/// (scans skip part files, and an empty folder is in nobody's way).
pub(super) async fn clean_up(pool: &SqlitePool, id: i64) {
    let recorded: Option<(String, String)> =
        sqlx::query_as("SELECT partials, created_dirs FROM downloads WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    let Some((partials, created)) = recorded else {
        return;
    };
    let partials: Vec<String> = serde_json::from_str(&partials).unwrap_or_default();
    match delete_partials(pool, id, &partials).await {
        Ok((_, left)) if !left.is_empty() => {
            tracing::warn!(download = id, ?left, "left a download's part files");
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(
            download = id,
            error = format!("{err:#}"),
            "couldn't delete a download's part files"
        ),
    }
    let roots: Vec<String> = sqlx::query_scalar("SELECT path FROM libraries")
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    let mut created: Vec<PathBuf> = serde_json::from_str::<Vec<String>>(&created)
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .filter(|d| {
            d.is_absolute()
                && roots
                    .iter()
                    .any(|root| d.starts_with(root) && d != Path::new(root))
        })
        .collect();
    // Deepest first, so a folder is empty once the ones inside it are gone.
    created.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    let _ = blocking(move || {
        for dir in created {
            // A real folder only (never through a link), and remove_dir refuses a full one.
            if std::fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_dir()) {
                let _ = std::fs::remove_dir(&dir);
            }
        }
        Ok(())
    })
    .await;
}
