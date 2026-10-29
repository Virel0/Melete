//! What the backups folder holds, for the admins' System page: each copy, the kind its name
//! says, its size and when it was written. Copies being written (hidden part files), folders
//! and links are left out.

use std::time::UNIX_EPOCH;

use super::*;

/// One copy in the backups folder.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Listed {
    /// Its file name in the folder.
    pub file: String,
    /// What its name says it is: daily, before_migrating, by_hand, or other (a copy named
    /// otherwise, such as one put there by hand).
    pub kind: &'static str,
    /// Its size in bytes.
    pub bytes: u64,
    /// When it was last written, in Unix seconds.
    pub made_at: i64,
}

fn kind_of(name: &str) -> &'static str {
    if name.starts_with("daily-") {
        Kind::Daily.as_str()
    } else if name.starts_with("before-migrating-from-") {
        Kind::BeforeMigrating.as_str()
    } else if name.starts_with("by-hand-") {
        Kind::ByHand.as_str()
    } else {
        "other"
    }
}

/// The copies in `data_dir`'s backups folder, newest first; none when there's no folder yet.
pub fn list(data_dir: &Path) -> anyhow::Result<Vec<Listed>> {
    let dir = data_dir.join(FOLDER);
    let entries = match std::fs::read_dir(&dir) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        read => read.with_context(|| format!("listing {}", dir.display()))?,
    };
    let mut copies = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Ok(file) = entry.file_name().into_string() else {
            continue;
        };
        if file.starts_with('.') || !file.ends_with(".db") {
            continue;
        }
        // The entry's own metadata: a link is never followed.
        let meta = entry.metadata()?;
        if !meta.is_file() {
            continue;
        }
        let made_at = meta
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        copies.push(Listed {
            kind: kind_of(&file),
            file,
            bytes: meta.len(),
            made_at,
        });
    }
    copies.sort_by(|a, b| b.made_at.cmp(&a.made_at).then_with(|| b.file.cmp(&a.file)));
    Ok(copies)
}
