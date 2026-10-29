//! Going back to a copy (`melete restore <copy>`), only while no server uses the data folder.
//! The copy is copied in beside the database and checked there (SQLite's full integrity check,
//! and migrations this version knows), the current database is kept in the backups folder
//! first, and only then is the database replaced, its old WAL and shared-memory files removed
//! before the copy takes its name so SQLite never replays them onto it.

use std::fs::File;
use std::io::ErrorKind;

use super::*;

/// The file in the data folder a running server holds a lock on, and a restore too.
pub const LOCK_FILE: &str = "melete.lock";
/// The database's file name in the data folder.
const DATABASE: &str = "melete.db";
/// What the chosen copy is copied to while it's checked, beside the database.
const STAGED: &str = ".melete.db.restoring";

/// The lock on the data folder: held by the server for as long as it runs, and by a restore
/// while it works, so neither starts while the other has it. Let go when dropped.
#[derive(Debug)]
pub struct Lock {
    _file: File,
}

/// Takes the data folder's lock, or says why not: a server, or a restore, has it.
pub fn lock(data_dir: &Path) -> anyhow::Result<Lock> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = data_dir.join(LOCK_FILE);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Lock { _file: file }),
        Err(std::fs::TryLockError::WouldBlock) => bail!(
            "Melete is running on {} (or a restore is): {} is locked",
            data_dir.display(),
            path.display()
        ),
        Err(std::fs::TryLockError::Error(err)) => {
            Err(err).with_context(|| format!("locking {}", path.display()))
        }
    }
}

/// How the database that was there was kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kept {
    /// There was no database.
    Nothing,
    /// Copied by SQLite into the backups folder, as any copy is.
    Copied(Copy),
    /// SQLite couldn't copy it, so its files were moved into the backups folder as they were:
    /// each moved file's name there, and why it couldn't be copied.
    Moved {
        /// The names the files were moved to, in the backups folder.
        files: Vec<String>,
        /// Why SQLite couldn't copy it.
        why: String,
    },
}

/// What a restore did.
#[derive(Debug)]
pub struct Restored {
    /// The copy restored.
    pub from: PathBuf,
    /// The newest migration the copy holds.
    pub version: i64,
    /// Whether this version has migrations the copy hasn't had, applied at the next start.
    pub behind: bool,
    /// How the database that was there was kept.
    pub kept: Kept,
    /// The WAL and shared-memory files removed from beside the database.
    pub removed: Vec<String>,
}

/// The copies in the backups folder, newest first: each one's name, size and when it was
/// last changed (Unix seconds).
pub fn copies(data_dir: &Path) -> anyhow::Result<Vec<(String, u64, i64)>> {
    let dir = data_dir.join(FOLDER);
    let entries = match std::fs::read_dir(&dir) {
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        other => other.with_context(|| format!("reading {}", dir.display()))?,
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let meta = entry.metadata()?;
        if name.starts_with('.') || !name.ends_with(".db") || !meta.is_file() {
            continue;
        }
        let changed = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        found.push((name, meta.len(), changed));
    }
    found.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| b.0.cmp(&a.0)));
    Ok(found)
}

/// Where `chosen` is: a name in the backups folder, or a path when it has a `/`.
fn resolve(data_dir: &Path, chosen: &str) -> anyhow::Result<PathBuf> {
    let path = if chosen.contains('/') {
        PathBuf::from(chosen)
    } else {
        data_dir.join(FOLDER).join(chosen)
    };
    let meta = std::fs::metadata(&path).with_context(|| format!("reading {}", path.display()))?;
    if !meta.is_file() {
        bail!("{} isn't a file", path.display());
    }
    let database = data_dir.join(DATABASE);
    if database.exists() && std::fs::canonicalize(&path)? == std::fs::canonicalize(&database)? {
        bail!("{} is the database itself", path.display());
    }
    Ok(path)
}

/// Restores the copy `chosen` (a name in the backups folder, or a path) as the database in
/// `data_dir`, refusing while a server runs there, and a copy that fails SQLite's integrity
/// check or holds migrations `migrator` doesn't have. Nothing is replaced until the copy has
/// passed and the database there has been kept.
pub async fn restore(
    data_dir: &Path,
    chosen: &str,
    migrator: &Migrator,
) -> anyhow::Result<Restored> {
    let _lock = lock(data_dir)?;
    let from = resolve(data_dir, chosen)?;
    let staged = data_dir.join(STAGED);
    remove_staged(&staged)?;
    let checked = async {
        std::fs::copy(&from, &staged)
            .with_context(|| format!("copying {} beside the database", from.display()))?;
        check_restorable(&staged, migrator).await
    }
    .await;
    let (version, behind) = match checked {
        Ok(found) => found,
        Err(err) => {
            remove_staged(&staged)?;
            return Err(err.context(format!("{} wasn't restored", from.display())));
        }
    };
    let kept = keep_current(data_dir).await?;
    let mut removed = Vec::new();
    for suffix in ["-wal", "-shm"] {
        let name = format!("{DATABASE}{suffix}");
        let path = data_dir.join(&name);
        if path.exists() {
            remove_if_there(&path)?;
            removed.push(name);
        }
    }
    std::fs::rename(&staged, data_dir.join(DATABASE))
        .with_context(|| format!("putting the copy in place of {DATABASE}"))?;
    File::open(data_dir)
        .and_then(|dir| dir.sync_all())
        .with_context(|| format!("syncing {}", data_dir.display()))?;
    Ok(Restored {
        from,
        version,
        behind,
        kept,
        removed,
    })
}

/// Removes the staged copy and any files SQLite kept beside it.
fn remove_staged(staged: &Path) -> anyhow::Result<()> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut name = staged.as_os_str().to_owned();
        name.push(suffix);
        remove_if_there(Path::new(&name))?;
    }
    Ok(())
}

/// Checks the copy at `staged` can be the database: whole by SQLite's integrity check, made by
/// Melete, its migrations all ones `migrator` has, unchanged and none failed part way. Leaves
/// it as one file (no WAL). Answers its newest migration and whether it's behind `migrator`.
async fn check_restorable(staged: &Path, migrator: &Migrator) -> anyhow::Result<(i64, bool)> {
    let mut conn: SqliteConnection = SqliteConnectOptions::new()
        .filename(staged)
        .connect()
        .await
        .context("opening the copy")?;
    let verdict: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&mut conn)
        .await
        .context("checking the copy")?;
    if verdict != ["ok"] {
        bail!(
            "the copy fails SQLite's integrity check: {}",
            verdict.join("; ")
        );
    }
    let found = check_migrations(&mut conn, migrator).await?;
    sqlx::query("PRAGMA journal_mode = DELETE")
        .execute(&mut conn)
        .await
        .context("making the copy one file")?;
    conn.close().await?;
    Ok(found)
}

/// The copy's newest migration and whether it's behind `migrator`, refusing one Melete didn't
/// make, one with a migration `migrator` doesn't have or has changed, and one failed part way.
async fn check_migrations(
    conn: &mut SqliteConnection,
    migrator: &Migrator,
) -> anyhow::Result<(i64, bool)> {
    let melete: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
    )
    .fetch_one(&mut *conn)
    .await?;
    if !melete {
        bail!("the copy isn't a Melete database: it has no migrations");
    }
    let applied: Vec<(i64, bool, Vec<u8>)> =
        sqlx::query_as("SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut *conn)
            .await?;
    for (version, success, checksum) in &applied {
        if !success {
            bail!("migration {version:04} failed part way in the copy");
        }
        match migrator.iter().find(|m| m.version == *version) {
            None => bail!(
                "the copy was made by a newer Melete: this version doesn't have its migration {version:04}"
            ),
            Some(m) if m.checksum.as_ref() != checksum.as_slice() => {
                bail!("the copy's migration {version:04} isn't this version's")
            }
            Some(_) => {}
        }
    }
    let Some(version) = applied.iter().map(|a| a.0).max() else {
        bail!("the copy isn't a Melete database: it has no migrations");
    };
    let behind = migrator
        .iter()
        .any(|m| !applied.iter().any(|a| a.0 == m.version));
    Ok((version, behind))
}

/// Keeps the database in `data_dir` in the backups folder: copied by SQLite when it can be,
/// else its files moved there as they are.
async fn keep_current(data_dir: &Path) -> anyhow::Result<Kept> {
    let database = data_dir.join(DATABASE);
    let wal = data_dir.join(format!("{DATABASE}-wal"));
    if !database.exists() && !wal.exists() {
        return Ok(Kept::Nothing);
    }
    let name = format!(
        "before-restoring-{}.db",
        dates::stamp(crate::db::unix_now())
    );
    let why = if database.exists() {
        match copy_current(data_dir, &name).await {
            Ok(copy) => return Ok(Kept::Copied(copy)),
            Err(err) => format!("{err:#}"),
        }
    } else {
        format!("there was no {DATABASE}, only its WAL")
    };
    let dir = folder(data_dir)?;
    let mut files = Vec::new();
    for suffix in ["", "-wal", "-shm"] {
        let from = data_dir.join(format!("{DATABASE}{suffix}"));
        if !from.exists() {
            continue;
        }
        let to = format!("{name}{suffix}");
        if dir.join(&to).exists() {
            bail!("{} is there already", dir.join(&to).display());
        }
        std::fs::rename(&from, dir.join(&to))
            .with_context(|| format!("moving {} into {}", from.display(), dir.display()))?;
        files.push(to);
    }
    Ok(Kept::Moved { files, why })
}

/// Copies the database in `data_dir` to `name` in the backups folder, through its WAL.
async fn copy_current(data_dir: &Path, name: &str) -> anyhow::Result<Copy> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(data_dir.join(DATABASE))
                .create_if_missing(false),
        )
        .await
        .context("opening the database there")?;
    let copied = copy(&pool, data_dir, name).await;
    // The last connection closing writes the WAL into the database and removes it.
    pool.close().await;
    copied
}

#[cfg(test)]
mod tests;
