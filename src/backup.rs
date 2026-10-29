//! Copies of the database, made by SQLite itself (`VACUUM INTO`), so a copy taken while Melete
//! runs is whole even with writes still in the WAL; copying the file would not be. One is made
//! before any migration runs (startup stops when it can't be), one a day, and one whenever
//! `melete backup` is run. They go in the data folder's `backups`, readable by Melete alone,
//! and each is checked by SQLite before it takes its name. Kept: the daily copies of the last
//! [`DAILY_KEPT`] days and one a week for [`WEEKLY_KEPT`] weeks before them, and the last
//! [`BEFORE_MIGRATING_KEPT`] made before migrating; copies made by hand are the admin's to
//! remove. Going back to one is `melete restore`, with no server running (see [`restore`]).

use std::collections::BTreeSet;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{ConnectOptions, Connection, SqlitePool};

mod dates;
mod listing;
mod pruning;
pub mod restore;

pub use listing::{Listed, list};
pub use pruning::prune;

/// The folder in the data folder the copies go in.
pub const FOLDER: &str = "backups";
/// Days whose daily copy is kept.
pub const DAILY_KEPT: usize = 7;
/// Weeks before those that keep one daily copy each.
pub const WEEKLY_KEPT: usize = 4;
/// Copies made before migrating that are kept, the newest.
pub const BEFORE_MIGRATING_KEPT: usize = 5;
/// How old the newest copy may get before admins are told.
pub const STALE_AFTER: i64 = 2 * 86_400;
/// How often the daily copy is looked for, and made when today's isn't there.
const LOOK_EVERY: Duration = Duration::from_secs(3600);
/// How long a record of a copy is kept.
const RECORDS_KEPT: i64 = 90 * 86_400;

/// Why a copy was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Before applying migrations, holding the schema as it was.
    BeforeMigrating,
    /// The day's copy.
    Daily,
    /// Asked for with `melete backup`.
    ByHand,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::BeforeMigrating => "before_migrating",
            Kind::Daily => "daily",
            Kind::ByHand => "by_hand",
        }
    }
}

/// A copy that was made: its file name in [`FOLDER`] and its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copy {
    /// The file's name, in the backups folder.
    pub file: String,
    /// Its size in bytes.
    pub bytes: u64,
}

/// The backups folder in `data_dir`, made (for Melete's user alone) when it isn't there.
pub fn folder(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let dir = data_dir.join(FOLDER);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .with_context(|| format!("making {}", dir.display()))?;
    Ok(dir)
}

/// Copies the database `pool` holds to `name` in the backups folder: written under a hidden
/// name, checked, then given its own. Never replaces a copy already there.
pub async fn copy(pool: &SqlitePool, data_dir: &Path, name: &str) -> anyhow::Result<Copy> {
    let dir = folder(data_dir)?;
    let target = dir.join(name);
    if target.exists() {
        bail!("{} is there already", target.display());
    }
    let part = dir.join(format!(".{name}.part"));
    // A copy cut off before (Melete stopped part way) is no use; SQLite won't write over it.
    remove_if_there(&part)?;
    if let Err(err) = write_and_check(pool, &part).await {
        remove_if_there(&part)?;
        return Err(err);
    }
    std::fs::rename(&part, &target)
        .with_context(|| format!("naming the copy {}", target.display()))?;
    let bytes = std::fs::metadata(&target)?.len();
    Ok(Copy {
        file: name.to_owned(),
        bytes,
    })
}

fn remove_if_there(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            Err(err).with_context(|| format!("removing {}", path.display()))
        }
        _ => Ok(()),
    }
}

async fn write_and_check(pool: &SqlitePool, part: &Path) -> anyhow::Result<()> {
    let path = part
        .to_str()
        .context("the data folder's path isn't UTF-8, which SQLite needs")?;
    sqlx::query("VACUUM INTO ?")
        .bind(path)
        .execute(pool)
        .await
        .context("copying the database")?;
    check(part).await
}

/// Whether the database at `path` is whole, by SQLite's quick check, opened read-only.
pub async fn check(path: &Path) -> anyhow::Result<()> {
    let mut conn: SqliteConnection = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .connect()
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    let verdict: String = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_one(&mut conn)
        .await
        .with_context(|| format!("checking {}", path.display()))?;
    conn.close().await?;
    if verdict != "ok" {
        bail!("{} fails SQLite's check: {verdict}", path.display());
    }
    Ok(())
}

/// The newest migration applied, or `None` for a database Melete has never migrated (a new
/// one, with nothing in it to lose).
async fn schema_version(pool: &SqlitePool) -> anyhow::Result<Option<i64>> {
    let migrated: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await?;
    if !migrated {
        return Ok(None);
    }
    Ok(
        sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success")
            .fetch_one(pool)
            .await?,
    )
}

/// Copies the database when `migrator` has migrations it hasn't had yet, before they run,
/// named after the newest migration it holds. Nothing for a new database or one up to date.
pub async fn before_migrating(
    pool: &SqlitePool,
    data_dir: &Path,
    migrator: &Migrator,
) -> anyhow::Result<Option<Copy>> {
    let Some(version) = schema_version(pool).await? else {
        return Ok(None);
    };
    let applied: BTreeSet<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success")
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();
    if migrator.iter().all(|m| applied.contains(&m.version)) {
        return Ok(None);
    }
    let now = dates::stamp(crate::db::unix_now());
    let copy = copy(
        pool,
        data_dir,
        &format!("before-migrating-from-{version:04}-{now}.db"),
    )
    .await?;
    prune(&folder(data_dir)?, crate::db::unix_now())?;
    Ok(Some(copy))
}

/// Keeps what came of a copy (`result`), for the admins' health page, forgetting records past
/// [`RECORDS_KEPT`].
pub async fn record(
    pool: &SqlitePool,
    kind: Kind,
    result: &anyhow::Result<Copy>,
) -> anyhow::Result<()> {
    let now = crate::db::unix_now();
    let (file, bytes, error) = match result {
        Ok(copy) => (
            Some(copy.file.as_str()),
            Some(i64::try_from(copy.bytes)?),
            None,
        ),
        Err(err) => (None, None, Some(format!("{err:#}"))),
    };
    sqlx::query("INSERT INTO backups (kind, file, bytes, error, made_at) VALUES (?, ?, ?, ?, ?)")
        .bind(kind.as_str())
        .bind(file)
        .bind(bytes)
        .bind(error)
        .bind(now)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM backups WHERE made_at < ?")
        .bind(now - RECORDS_KEPT)
        .execute(pool)
        .await?;
    Ok(())
}

/// Today's copy, unless it's there already, then the old ones pruned. Answers the copy made.
pub async fn daily_once(pool: &SqlitePool, data_dir: &Path) -> anyhow::Result<Option<Copy>> {
    let now = crate::db::unix_now();
    let name = format!("daily-{}.db", dates::date(now));
    let made = async {
        let dir = folder(data_dir)?;
        if dir.join(&name).exists() {
            return Ok(None);
        }
        let copy = copy(pool, data_dir, &name).await?;
        prune(&dir, now)?;
        Ok(Some(copy))
    }
    .await;
    match made {
        Ok(None) => Ok(None),
        Ok(Some(copy)) => {
            record(pool, Kind::Daily, &Ok(copy.clone())).await?;
            Ok(Some(copy))
        }
        Err(err) => {
            let failed: anyhow::Result<Copy> = Err(err);
            record(pool, Kind::Daily, &failed).await?;
            failed.map(Some)
        }
    }
}

/// Makes the day's copy at startup and then whenever a day has turned, for as long as Melete
/// runs.
pub async fn daily(pool: SqlitePool, data_dir: PathBuf) {
    loop {
        crate::tasks::started("backup");
        let made = daily_once(&pool, &data_dir).await;
        let result = match &made {
            Ok(Some(_)) => Some("made the day's copy".into()),
            Ok(None) => Some("the day's copy is made already".into()),
            Err(_) => crate::tasks::failed(),
        };
        crate::tasks::finished("backup", result, Some(LOOK_EVERY));
        match made {
            Ok(Some(copy)) => tracing::info!(
                file = copy.file,
                bytes = copy.bytes,
                "backed up the database"
            ),
            Ok(None) => {}
            Err(err) => tracing::error!(
                error = format!("{err:#}"),
                "couldn't back up the database; trying again in an hour"
            ),
        }
        tokio::time::sleep(LOOK_EVERY).await;
    }
}

/// A copy made by hand (`melete backup`), kept until the admin removes it.
pub async fn by_hand(pool: &SqlitePool, data_dir: &Path) -> anyhow::Result<Copy> {
    let name = format!("by-hand-{}.db", dates::stamp(crate::db::unix_now()));
    let result = copy(pool, data_dir, &name).await;
    record(pool, Kind::ByHand, &result).await?;
    result
}

#[cfg(test)]
mod tests;
