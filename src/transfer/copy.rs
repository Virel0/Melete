//! Copying files over SFTP under a hidden part name that resumes, with each part file recorded so
//! removing the download deletes only its own.

use super::*;

/// Copies every file of the plan under its hidden part name, checks a movie's or episode's
/// running time when `check` is given, and only then puts the files in place: a download that
/// fails the check leaves nothing in the library.
pub(super) async fn copy_all(
    pool: &SqlitePool,
    seedbox: &Seedbox,
    id: i64,
    plan: &Plan,
    check: Option<&verify::Check>,
) -> Result<(), Stop> {
    // An unmounted share looks like an empty folder; writing there would fill the host's disk.
    let root = plan.root.clone();
    let mounted = blocking(move || {
        Ok(std::fs::read_dir(&root)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false))
    })
    .await?;
    if !mounted {
        return Err(Stop::Retry(format!(
            "the library folder {} is missing or empty; is the share mounted?",
            plan.root.display()
        )));
    }
    let connection = seedbox
        .connect()
        .await
        .map_err(|err| Stop::Retry(err.to_string()))?;
    make_dirs(pool, id, &plan.dir).await?;
    let db = |err: sqlx::Error| Stop::Retry(format!("the database: {err}"));
    let placed: String = sqlx::query_scalar("SELECT placed FROM downloads WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(db)?;
    let placed: Vec<String> = serde_json::from_str(&placed).unwrap_or_default();
    let mut staged = Vec::with_capacity(plan.files.len());
    for (n, file) in plan.files.iter().enumerate() {
        let dir = match file.folder.as_str() {
            "" => plan.dir.clone(),
            folder => {
                let dir = plan.dir.join(folder);
                make_dirs(pool, id, &dir).await?;
                dir
            }
        };
        let message = format!("copying {} of {}: {}", n + 1, plan.files.len(), file.name);
        say(pool, id, &message).await?;
        staged.push(fetch(pool, &connection, file, &dir, id, &placed).await?);
    }
    if let Some(check) = check {
        say(pool, id, "checking how long it runs").await?;
        verify::check_staged(check, &staged).await?;
    }
    place_all(pool, id, staged, placed).await
}

/// Puts each copied file in place, recording each as soon as it is.
async fn place_all(
    pool: &SqlitePool,
    id: i64,
    staged: Vec<Staged>,
    mut placed: Vec<String>,
) -> Result<(), Stop> {
    for file in staged {
        let Some(partial) = file.partial else {
            continue;
        };
        let target = file.target.clone();
        blocking(move || {
            std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o664))?;
            place(&partial, &target)
        })
        .await?;
        // Remembered at once, so a restart knows this file is this download's own.
        placed.push(file.target.to_string_lossy().into_owned());
        sqlx::query("UPDATE downloads SET placed = ? WHERE id = ?")
            .bind(serde_json::to_string(&placed).unwrap_or_default())
            .bind(id)
            .execute(pool)
            .await
            .map_err(|err| Stop::Retry(format!("the database: {err}")))?;
        // And which file it is, so it's known for this download's only while it's still there.
        record_placed(pool, id, &file.target).await?;
    }
    Ok(())
}

/// Updates the download's message.
async fn say(pool: &SqlitePool, id: i64, message: &str) -> Result<(), Stop> {
    sqlx::query("UPDATE downloads SET message = ?, updated_at = ? WHERE id = ?")
        .bind(message)
        .bind(unix_now())
        .bind(id)
        .execute(pool)
        .await
        .map_err(|err| Stop::Retry(format!("the database: {err}")))?;
    Ok(())
}

/// Runs file-system work off the async threads: a NAS can take seconds to answer.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Stop> + Send + 'static,
) -> Result<T, Stop> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|err| Stop::Retry(format!("a file-system task failed: {err}")))?
}

/// Creates the destination and any folders above it that don't exist, as u=rwx,g=rwx,o=rx
/// (set after creating, since the umask would take group write away). Existing folders are
/// left as they are.
pub(crate) fn create_dirs(dir: &Path) -> Result<(), Stop> {
    if dir.is_file() {
        return Err(Stop::Fail(format!(
            "{} is a file, not a folder",
            dir.display()
        )));
    }
    let missing: Vec<&Path> = dir.ancestors().take_while(|d| !d.exists()).collect();
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o775)
        .create(dir)?;
    for created in missing {
        std::fs::set_permissions(created, std::fs::Permissions::from_mode(0o775))?;
    }
    Ok(())
}

/// The hidden name a file is copied under in `dir` before it's put in place: one
/// per download, so two releases with the same file name never mix. Scans skip it.
pub(crate) fn partial_path(dir: &Path, name: &str, id: i64) -> Result<PathBuf, Stop> {
    let partial_name = format!(".{name}.{id}.melete-part");
    if partial_name.len() > 255 {
        return Err(Stop::Fail(format!(
            "{name} is too long a name to copy under a temporary one"
        )));
    }
    Ok(dir.join(partial_name))
}

/// Records a part file of the download's, before it's created (`downloads.partials`).
pub(crate) async fn record_partial(pool: &SqlitePool, id: i64, partial: &Path) -> Result<(), Stop> {
    let path = partial.to_string_lossy();
    sqlx::query(
        "UPDATE downloads SET partials = json_insert(partials, '$[#]', ?1)
         WHERE id = ?2 AND NOT EXISTS (SELECT 1 FROM json_each(partials) WHERE value = ?1)",
    )
    .bind(path.as_ref())
    .bind(id)
    .execute(pool)
    .await
    .map_err(|err| Stop::Retry(format!("the database: {err}")))?;
    Ok(())
}

/// Whether `path` is download `id`'s own part file by its name alone: absolute, nothing like
/// "..", and named ".<file>.<id>.melete-part" for a non-empty file name.
pub(super) fn own_partial(path: &Path, id: i64) -> bool {
    use std::path::Component;
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix('.'))
        .and_then(|n| n.strip_suffix(&format!(".{id}.melete-part")))
        .is_some_and(|file| !file.is_empty())
}

/// Deletes a removed download's part files, and nothing else: each must be one it
/// recorded, named as its own ([`own_partial`]), inside a library folder, and a regular file
/// (never a link or a folder, whatever it points at). One that's gone already is fine. Returns
/// how many it deleted, and what it left and why.
pub async fn delete_partials(
    pool: &SqlitePool,
    id: i64,
    partials: &[String],
) -> anyhow::Result<(usize, Vec<String>)> {
    let roots: Vec<String> = sqlx::query_scalar("SELECT path FROM libraries")
        .fetch_all(pool)
        .await?;
    let mut sure = Vec::new();
    let mut left = Vec::new();
    for partial in partials {
        let path = PathBuf::from(partial);
        let in_a_library = roots
            .iter()
            .any(|root| path.parent().is_some_and(|p| p.starts_with(root)));
        if own_partial(&path, id) && in_a_library {
            sure.push(path);
        } else {
            left.push(format!("{partial}: not this download's part file"));
        }
    }
    let (deleted, mut not) = tokio::task::spawn_blocking(move || {
        let mut deleted = 0;
        let mut not = Vec::new();
        for path in sure {
            match std::fs::symlink_metadata(&path) {
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => not.push(format!("{}: {err}", path.display())),
                Ok(meta) if !meta.file_type().is_file() => {
                    not.push(format!("{}: not a regular file", path.display()));
                }
                Ok(_) => match std::fs::remove_file(&path) {
                    Ok(()) => deleted += 1,
                    Err(err) if err.kind() == ErrorKind::NotFound => {}
                    Err(err) => not.push(format!("{}: {err}", path.display())),
                },
            }
        }
        (deleted, not)
    })
    .await?;
    left.append(&mut not);
    Ok((deleted, left))
}

/// A file copied home under its part name and not yet in place.
pub(super) struct Staged {
    /// Its name in the library.
    pub(super) name: String,
    /// Its part file; None when this download had placed it already.
    pub(super) partial: Option<PathBuf>,
    /// Where it goes.
    pub(super) target: PathBuf,
}

/// Copies one file into `dir` under its part name, resuming a partial copy; it's put in place
/// once every file is copied and checked.
pub(super) async fn fetch(
    pool: &SqlitePool,
    connection: &Connection,
    file: &Planned,
    dir: &Path,
    id: i64,
    placed: &[String],
) -> Result<Staged, Stop> {
    let target = dir.join(&file.name);
    let target_name = target.to_string_lossy().into_owned();
    let staged = |partial| Staged {
        name: file.name.clone(),
        partial,
        target: target.clone(),
    };
    // Anything at the name (a file, a folder, even a dangling link) stops the copy, unless this
    // download put it there itself (and a restart came before the database heard).
    match std::fs::symlink_metadata(&target) {
        Ok(_) if placed.contains(&target_name) => return Ok(staged(None)),
        Ok(_) => {
            return Err(Stop::Fail(format!(
                "{} is already there and isn't this file; Melete doesn't replace files",
                target.display()
            )));
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    let partial = partial_path(dir, &file.name, id)?;
    // Recorded before it exists, so removing the download can delete it.
    record_partial(pool, id, &partial).await?;

    let sftp = &connection.sftp;
    check_remote(sftp, file).await?;

    let (local, done) = open_partial(&partial, file).await?;
    let done = fetch_rest(sftp, file, local, done).await?;
    let written = std::fs::symlink_metadata(&partial)?.len();
    if done != file.size || written != file.size {
        return Err(Stop::Retry(format!(
            "{} came out {written} bytes, not {}",
            file.name, file.size
        )));
    }
    Ok(staged(Some(partial)))
}

/// An error from the seedbox, as a stop that's retried later.
fn seedbox_error(err: russh_sftp::client::error::Error) -> Stop {
    Stop::Retry(format!("the seedbox: {err}"))
}

/// Checks the file on the seedbox is a regular file of the size Deluge reported.
async fn check_remote(sftp: &russh_sftp::client::SftpSession, file: &Planned) -> Result<(), Stop> {
    let remote = sftp
        .symlink_metadata(file.remote.clone())
        .await
        .map_err(seedbox_error)?;
    // file_type compares the type exactly; is_regular() also says yes for links (their type
    // bits include the regular file's).
    if !remote.file_type().is_file() {
        return Err(Stop::Fail(format!(
            "{} on the seedbox isn't a regular file (a link, say); Melete only fetches files",
            file.remote
        )));
    }
    if remote.len() != file.size {
        return Err(Stop::Retry(format!(
            "{} on the seedbox is {} bytes, not the {} Deluge reported",
            file.remote,
            remote.len(),
            file.size
        )));
    }
    Ok(())
}

/// Opens the part file for writing, never through a link, cut back to what an earlier copy
/// left of this file; returns it with how many bytes it keeps.
async fn open_partial(partial: &Path, file: &Planned) -> Result<(tokio::fs::File, u64), Stop> {
    let mut done = std::fs::symlink_metadata(partial)
        .map(|m| m.len())
        .unwrap_or(0);
    if done > file.size {
        done = 0;
    }
    // Never through a link: the partial is Melete's own file or nothing.
    let mut local = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .custom_flags(libc::O_NOFOLLOW)
        .open(partial)
        .await
        .map_err(|err| match err.raw_os_error() {
            Some(libc::ELOOP) => Stop::Fail(format!(
                "{} is a link; Melete won't write through it",
                partial.display()
            )),
            _ => Stop::from(err),
        })?;
    local.set_len(done).await?;
    local.seek(std::io::SeekFrom::Start(done)).await?;
    Ok((local, done))
}

/// Copies the rest of the file from the seedbox into the part file and makes sure it reached
/// the disk; returns how many bytes were copied in all.
async fn fetch_rest(
    sftp: &russh_sftp::client::SftpSession,
    file: &Planned,
    mut local: tokio::fs::File,
    mut done: u64,
) -> Result<u64, Stop> {
    let mut source = sftp
        .open(file.remote.clone())
        .await
        .map_err(seedbox_error)?;
    source
        .seek(std::io::SeekFrom::Start(done))
        .await
        .map_err(|err| Stop::Retry(format!("the seedbox: {err}")))?;
    let mut buffer = vec![0u8; 256 * 1024];
    while done < file.size {
        let read = source.read(&mut buffer).await.map_err(|err| {
            Stop::Retry(format!("reading {} from the seedbox: {err}", file.remote))
        })?;
        if read == 0 {
            break;
        }
        let take = read.min((file.size - done) as usize);
        local.write_all(&buffer[..take]).await?;
        done += take as u64;
    }
    // tokio writes in the background and reports a failed last write only on flush (sync_all
    // doesn't): a full disk must not pass for a finished copy.
    local.flush().await?;
    local.sync_all().await?;
    drop(local);
    Ok(done)
}
