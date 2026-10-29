//! Bringing finished Soulseek downloads home. The files come from the download's own folders
//! in slskd's downloads folder (`melete/<id>/<n>`) and go into the album's folder in the music
//! library as a torrent album's do: each copied under a hidden name and put in place without
//! replacing anything, disc folders kept, then fixed. A song's one file goes into its album's
//! folder when the library has that album, else into a new folder for its album (or `Singles`)
//! beside the artist's albums, fixed as a single track. Once the download is imported, its
//! folder in slskd's is removed; nothing else there
//! is ever touched.

use std::io::ErrorKind;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;

use crate::album_fix::{self, Fixes};
use crate::db::unix_now;
use crate::library::AUDIO_EXTENSIONS;
use crate::soulseek_downloads::Stored;
use crate::transfer::{self, Plan, Planned, Stop};

mod planning;

use planning::*;

/// How often to look for finished downloads when there's nothing to do.
const IDLE: Duration = Duration::from_secs(60);

/// Imports finished Soulseek downloads for as long as the server runs. `downloads` is slskd's
/// downloads folder as mounted here.
pub async fn work(pool: SqlitePool, downloads: PathBuf, fixes: Arc<Fixes>) {
    // An import cut off by a restart starts again; what it placed is known.
    if let Err(err) = sqlx::query(
        "UPDATE downloads SET state = 'completed' WHERE state = 'transferring' AND source = 'soulseek'",
    )
    .execute(&pool)
    .await
    {
        tracing::warn!(error = %err, "couldn't reset interrupted Soulseek imports");
    }
    // Folders left by imports that finished just before a restart.
    if let Err(err) = sweep(&pool, &downloads, None).await {
        tracing::warn!(error = format!("{err:#}"), "couldn't tidy slskd's folder");
    }
    loop {
        match next_due(&pool).await {
            Ok(Some(id)) => {
                if let Err(err) = import(&pool, &downloads, Some(&fixes), id).await {
                    let _ = sqlx::query(
                        "UPDATE downloads SET state = 'completed', next_attempt_at = ?
                         WHERE id = ? AND state = 'transferring'",
                    )
                    .bind(unix_now() + IDLE.as_secs() as i64)
                    .bind(id)
                    .execute(&pool)
                    .await;
                    tracing::warn!(
                        download = id,
                        error = format!("{err:#}"),
                        "Soulseek import bookkeeping failed"
                    );
                    tokio::time::sleep(IDLE).await;
                }
            }
            Ok(None) => tokio::time::sleep(IDLE).await,
            Err(err) => {
                tracing::warn!(
                    error = format!("{err:#}"),
                    "couldn't look for finished Soulseek downloads"
                );
                tokio::time::sleep(IDLE).await;
            }
        }
    }
}

/// The next finished download to import: a song before any album, as one track
/// shouldn't wait while a whole album is tagged and measured; else the longest finished. The
/// downloads list shows each one's place in this order (`downloads::Download::import_position`).
async fn next_due(pool: &SqlitePool) -> anyhow::Result<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM downloads
         WHERE state = 'completed' AND source = 'soulseek'
           AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
         ORDER BY wanted_song_id IS NULL, completed_at, id LIMIT 1",
    )
    .bind(unix_now())
    .fetch_optional(pool)
    .await?)
}

#[derive(sqlx::FromRow)]
struct Row {
    title: String,
    wanted_album_id: Option<i64>,
    wanted_song_id: Option<i64>,
    soulseek: String,
    attempts: i64,
}

/// The download's own folder in slskd's downloads folder.
fn own_folder(downloads: &Path, id: i64) -> PathBuf {
    downloads.join("melete").join(id.to_string())
}

/// Brings one finished Soulseek download into the library and records how it went.
pub async fn import(
    pool: &SqlitePool,
    downloads: &Path,
    fixes: Option<&Fixes>,
    id: i64,
) -> anyhow::Result<()> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT title, wanted_album_id, wanted_song_id, soulseek, attempts FROM downloads
         WHERE id = ? AND state = 'completed' AND source = 'soulseek'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(()) };
    let plan = match plan(pool, downloads, id, &row).await? {
        Ok(plan) => plan,
        Err(stop) => return transfer::finish(pool, id, row.attempts, Err(stop)).await,
    };
    let claimed = sqlx::query(
        "UPDATE downloads SET state = 'transferring', message = 'copying from slskd''s folder',
             updated_at = ?
         WHERE id = ? AND state = 'completed'",
    )
    .bind(unix_now())
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    if claimed == 0 {
        return Ok(());
    }
    let outcome = match copy_all(pool, id, &plan).await {
        Ok(()) => match (row.wanted_album_id, row.wanted_song_id, fixes) {
            (Some(album), _, Some(fixes)) => transfer::fix(pool, fixes, id, album, &plan).await,
            (None, Some(song), Some(fixes)) => {
                // A song's plan holds its one track.
                let track = plan
                    .files
                    .first()
                    .map(|f| plan.dir.join(&f.folder).join(&f.name));
                match track {
                    Some(track) => album_fix::fix_song(pool, fixes, id, song, &track)
                        .await
                        .map_err(Stop::Retry),
                    None => Ok("in the library".to_owned()),
                }
            }
            _ => Ok("in the library".to_owned()),
        },
        Err(stop) => Err(stop),
    };
    let imported = outcome.is_ok();
    transfer::finish(
        pool,
        id,
        row.attempts,
        outcome.map(|note| (plan.dir.clone(), note)),
    )
    .await?;
    if imported {
        sweep(pool, downloads, Some(id)).await?;
    }
    Ok(())
}

/// Works out where the download goes and which of the files in its folders are brought.
async fn plan(
    pool: &SqlitePool,
    downloads: &Path,
    id: i64,
    row: &Row,
) -> anyhow::Result<Result<Plan, Stop>> {
    let destination = match (row.wanted_album_id, row.wanted_song_id) {
        (Some(album), _) => transfer::album_destination(pool, album).await?,
        (None, Some(song)) => transfer::song_destination(pool, song).await?,
        (None, None) => {
            return Ok(Err(Stop::Fail(
                "it isn't for a wanted album or song any more, so there's nowhere to put it".into(),
            )));
        }
    };
    let Some((root, dir)) = destination else {
        return Ok(Err(Stop::Fail(
            "it has no music library, so there's nowhere to put it".into(),
        )));
    };
    let stored: Stored = serde_json::from_str(&row.soulseek)?;
    let downloads = downloads.to_path_buf();
    let title = row.title.clone();
    let song = row.wanted_song_id.is_some();
    Ok(transfer::blocking(move || {
        let plan = local_plan(&downloads, id, &stored, &title, root, dir)?;
        let tracks = plan
            .files
            .iter()
            .filter(|f| transfer::has_extension(&f.name, AUDIO_EXTENSIONS))
            .count();
        if song && tracks != 1 {
            return Ok(Err(Stop::Fail(format!(
                "a song is one track, and slskd's folder holds {tracks}"
            ))));
        }
        Ok(Ok(plan))
    })
    .await
    .unwrap_or_else(Err))
}

async fn copy_all(pool: &SqlitePool, id: i64, plan: &Plan) -> Result<(), Stop> {
    let root = plan.root.clone();
    let mounted = transfer::blocking(move || {
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
    let db = |err: sqlx::Error| Stop::Retry(format!("the database: {err}"));
    let placed: String = sqlx::query_scalar("SELECT placed FROM downloads WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(db)?;
    let mut placed: Vec<String> = serde_json::from_str(&placed).unwrap_or_default();
    for (n, file) in plan.files.iter().enumerate() {
        sqlx::query("UPDATE downloads SET message = ?, updated_at = ? WHERE id = ?")
            .bind(format!(
                "copying {} of {}: {}",
                n + 1,
                plan.files.len(),
                file.name
            ))
            .bind(unix_now())
            .bind(id)
            .execute(pool)
            .await
            .map_err(db)?;
        let dir = match file.folder.as_str() {
            "" => plan.dir.clone(),
            folder => plan.dir.join(folder),
        };
        let target = dir.join(&file.name);
        let target_name = target.to_string_lossy().into_owned();
        let source = PathBuf::from(&file.remote);
        let (size, mine) = (file.size, placed.contains(&target_name));
        let partial = transfer::partial_path(&dir, &file.name, id)?;
        // Recorded before it exists, so removing the download can delete it.
        transfer::record_partial(pool, id, &partial).await?;
        let copied = transfer::blocking(move || {
            transfer::create_dirs(&dir)?;
            copy_one(&source, &partial, &target, size, mine)
        })
        .await?;
        if copied {
            // Remembered at once, so a restart knows this file is this download's own.
            placed.push(target_name);
            sqlx::query("UPDATE downloads SET placed = ? WHERE id = ?")
                .bind(serde_json::to_string(&placed).unwrap_or_default())
                .bind(id)
                .execute(pool)
                .await
                .map_err(db)?;
        }
    }
    Ok(())
}

/// Copies `source` to `target` by way of `partial`, never replacing anything at `target`
/// unless this download put it there (`mine`), in which case it's left. Returns whether it
/// copied.
fn copy_one(
    source: &Path,
    partial: &Path,
    target: &Path,
    size: u64,
    mine: bool,
) -> Result<bool, Stop> {
    match std::fs::symlink_metadata(target) {
        Ok(_) if mine => return Ok(false),
        Ok(_) => {
            return Err(Stop::Fail(format!(
                "{} is already there and isn't this file; Melete doesn't replace files",
                target.display()
            )));
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    match std::fs::symlink_metadata(partial) {
        Ok(_) => std::fs::remove_file(partial)?,
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    let mut from = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(source)?;
    let mut to = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(partial)?;
    let written = std::io::copy(&mut from, &mut to)?;
    to.sync_all()?;
    drop(to);
    if written != size {
        let _ = std::fs::remove_file(partial);
        return Err(Stop::Fail(format!(
            "{} changed while it was copied ({written} bytes, not {size})",
            source.display()
        )));
    }
    std::fs::set_permissions(partial, std::fs::Permissions::from_mode(0o664))?;
    transfer::place(partial, target)?;
    Ok(true)
}

/// Removes imported downloads' folders from slskd's downloads folder: download `only`'s, or
/// every imported one's that's still there.
async fn sweep(pool: &SqlitePool, downloads: &Path, only: Option<i64>) -> anyhow::Result<()> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM downloads
         WHERE source = 'soulseek' AND state = 'imported' AND (? IS NULL OR id = ?)",
    )
    .bind(only)
    .bind(only)
    .fetch_all(pool)
    .await?;
    let downloads = downloads.to_path_buf();
    tokio::task::spawn_blocking(move || {
        // Never through a link: the folder is Melete's own or nothing.
        let melete = downloads.join("melete");
        if !std::fs::symlink_metadata(&melete).is_ok_and(|m| m.is_dir()) {
            return;
        }
        for id in ids {
            let folder = own_folder(&downloads, id);
            match std::fs::symlink_metadata(&folder) {
                Ok(meta) if meta.is_dir() => {
                    if let Err(err) = std::fs::remove_dir_all(&folder) {
                        tracing::warn!(download = id, error = %err, "couldn't remove slskd's copy");
                    }
                }
                _ => {}
            }
        }
    })
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::slskd::Wanted;
    use crate::soulseek_downloads::Part;

    /// A part's folder below the album's, and its files' names and bytes.
    type DiscFiles<'a> = (&'a str, &'a [(&'a str, &'a [u8])]);

    struct Rig {
        pool: SqlitePool,
        /// slskd's downloads folder.
        downloads: tempfile::TempDir,
        /// The music library's parent: music/.
        local: tempfile::TempDir,
    }

    /// A music library, Radiohead wanting OK Computer in it, and finished download 4 whose
    /// parts are (folder, [(name, bytes)]), written into slskd's folder as slskd would.
    async fn rig(parts: &[DiscFiles<'_>]) -> Rig {
        let downloads = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        let music = local.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        std::fs::write(music.join(".keep"), b"").unwrap();
        // Someone else's download, which must be left alone.
        std::fs::create_dir_all(downloads.path().join("other")).unwrap();
        std::fs::write(downloads.path().join("other/x.flac"), b"x").unwrap();
        let mut stored = Stored {
            username: "alice".into(),
            folder: "@@m\\Radiohead\\OK Computer (1997)".into(),
            parts: Vec::new(),
        };
        for (n, (folder, files)) in parts.iter().enumerate() {
            let dir = own_folder(downloads.path(), 4).join((n + 1).to_string());
            std::fs::create_dir_all(&dir).unwrap();
            let mut wanted = Vec::new();
            for (name, bytes) in *files {
                std::fs::write(dir.join(name), bytes).unwrap();
                wanted.push(Wanted {
                    filename: format!("{}\\{folder}\\{name}", stored.folder),
                    size: bytes.len() as i64,
                });
            }
            stored.parts.push(Part {
                batch: format!("batch-{n}"),
                folder: (*folder).into(),
                files: wanted,
            });
        }
        let pool = db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (3, 'music', 'music', ?);
             INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, library_id, source, added_at)
                 VALUES (1, 'mb-radiohead', 'Radiohead', 1, 'all', 3, 'lidarr', 0);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, release_date, monitored, source, added_at)
                 VALUES (7, 1, 'rg', 'OK Computer', '1997-05-21', 1, 'lidarr', 0);
             INSERT INTO downloads (id, source, soulseek, title, wanted_album_id, decision, state, added_at, updated_at, completed_at)
                 VALUES (4, 'soulseek', ?, 'Radiohead - OK Computer (1997)', 7, '{}', 'completed', 0, 0, 1);",
        )
        .bind(music.to_string_lossy().into_owned())
        .bind(serde_json::to_string(&stored).unwrap())
        .execute(&pool)
        .await
        .unwrap();
        Rig {
            pool,
            downloads,
            local,
        }
    }

    impl Rig {
        async fn import(&self) {
            import(&self.pool, self.downloads.path(), None, 4)
                .await
                .unwrap();
        }

        async fn state(&self) -> (String, Option<String>, Option<String>, i64) {
            sqlx::query_as(
                "SELECT state, message, destination, attempts FROM downloads WHERE id = 4",
            )
            .fetch_one(&self.pool)
            .await
            .unwrap()
        }

        fn album(&self) -> PathBuf {
            self.local.path().join("music/Radiohead/OK Computer (1997)")
        }
    }

    /// Every file under `dir`, relative to it, in order.
    fn tree(dir: &Path) -> Vec<String> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                found.extend(tree(&path).into_iter().map(|f| format!("{name}/{f}")));
            } else {
                found.push(name);
            }
        }
        found.sort();
        found
    }

    const DISCS: &[DiscFiles] = &[
        (
            "CD1",
            &[
                ("01 Airbag.flac", b"airbag"),
                ("02 Paranoid Android.flac", b"paranoid"),
            ],
        ),
        ("CD2", &[("01 Lucky.flac", b"lucky")]),
        ("", &[("cover.jpg", b"jpg")]),
    ];

    #[tokio::test]
    async fn brings_the_album_home_with_its_discs_then_removes_its_own_folder() {
        let r = rig(DISCS).await;
        r.import().await;
        let (state, message, destination, _) = r.state().await;
        assert_eq!(state, "imported", "{message:?}");
        assert_eq!(destination.as_deref(), Some(r.album().to_str().unwrap()));
        assert_eq!(
            tree(&r.album()),
            [
                "CD1/01 Airbag.flac",
                "CD1/02 Paranoid Android.flac",
                "CD2/01 Lucky.flac",
                "cover.jpg"
            ]
        );
        assert_eq!(
            std::fs::read(r.album().join("CD2/01 Lucky.flac")).unwrap(),
            b"lucky"
        );
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&r.album().join("cover.jpg")), 0o664);
        assert!(
            !own_folder(r.downloads.path(), 4).exists(),
            "its folder is removed"
        );
        assert_eq!(
            std::fs::read(r.downloads.path().join("other/x.flac")).unwrap(),
            b"x",
            "nothing else in slskd's folder is touched"
        );
    }

    #[tokio::test]
    async fn refuses_a_folder_that_isnt_what_was_asked_for() {
        let r = rig(DISCS).await;
        std::fs::write(
            own_folder(r.downloads.path(), 4).join("2/01 Lucky.flac"),
            b"luck",
        )
        .unwrap();
        r.import().await;
        let (state, message, _, _) = r.state().await;
        assert_eq!(state, "failed");
        assert_eq!(
            message.as_deref(),
            Some("slskd's folder holds 21 bytes where 22 were asked for")
        );
        assert!(!r.album().exists(), "nothing placed");
        assert!(
            own_folder(r.downloads.path(), 4).exists(),
            "kept for a look"
        );
    }

    #[tokio::test]
    async fn replaces_nothing_and_takes_no_links() {
        let r = rig(DISCS).await;
        std::fs::create_dir_all(r.album().join("CD2")).unwrap();
        std::fs::write(r.album().join("CD2/01 Lucky.flac"), b"mine").unwrap();
        r.import().await;
        let (state, message, _, _) = r.state().await;
        assert_eq!(state, "failed");
        assert!(
            message
                .unwrap()
                .contains("is already there and isn't this file")
        );
        assert_eq!(
            std::fs::read(r.album().join("CD2/01 Lucky.flac")).unwrap(),
            b"mine"
        );
        assert!(own_folder(r.downloads.path(), 4).exists());

        let r = rig(DISCS).await;
        let part = own_folder(r.downloads.path(), 4).join("3");
        std::fs::remove_file(part.join("cover.jpg")).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", part.join("cover.jpg")).unwrap();
        r.import().await;
        let (state, message, _, _) = r.state().await;
        assert_eq!(state, "failed");
        assert!(message.unwrap().contains("isn't a plain file"), "a link");
    }

    #[tokio::test]
    async fn waits_while_slskds_folder_isnt_mounted() {
        let r = rig(DISCS).await;
        let empty = tempfile::tempdir().unwrap();
        import(&r.pool, empty.path(), None, 4).await.unwrap();
        let (state, message, _, attempts) = r.state().await;
        assert_eq!((state.as_str(), attempts), ("completed", 1));
        assert!(message.unwrap().contains("is it mounted?"));
    }

    #[tokio::test]
    async fn picks_up_where_a_restart_left_it() {
        let r = rig(DISCS).await;
        // The first file was placed, and recorded, before the restart.
        let first = r.album().join("CD1/01 Airbag.flac");
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::write(&first, b"airbag").unwrap();
        sqlx::query("UPDATE downloads SET placed = ? WHERE id = 4")
            .bind(serde_json::json!([first.to_string_lossy()]).to_string())
            .execute(&r.pool)
            .await
            .unwrap();
        r.import().await;
        assert_eq!(r.state().await.0, "imported");
        assert_eq!(tree(&r.album()).len(), 4);
    }

    #[tokio::test]
    async fn the_album_is_fixed_before_it_counts_as_imported() {
        let made = tempfile::tempdir().unwrap();
        let clip = made.path().join("01.flac");
        let status = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=d=4"])
            .arg(&clip)
            .status()
            .expect("ffmpeg is installed");
        assert!(status.success());
        let audio = std::fs::read(&clip).unwrap();
        let r = rig(&[("", &[("01 Airbag.flac", &audio)])]).await;
        let fixes = Fixes {
            tools: crate::sound::Tools {
                ffmpeg: "ffmpeg".into(),
                ffprobe: "ffprobe".into(),
            },
            coverart: None,
            lyrics: None,
        };
        import(&r.pool, r.downloads.path(), Some(&fixes), 4)
            .await
            .unwrap();
        let (state, message, _, _) = r.state().await;
        assert_eq!(state, "imported");
        assert_eq!(message.as_deref(), Some("in the library; ReplayGain added"));
        assert_ne!(
            std::fs::read(r.album().join("01 Airbag.flac")).unwrap(),
            audio,
            "tagged"
        );
    }

    /// A music library, a wanted song (on `album`, when given), and its finished download 4
    /// holding `files` in slskd's folder.
    async fn song_rig(album: Option<&str>, files: &[(&str, &[u8])]) -> Rig {
        let downloads = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        let music = local.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        std::fs::write(music.join(".keep"), b"").unwrap();
        let dir = own_folder(downloads.path(), 4).join("1");
        std::fs::create_dir_all(&dir).unwrap();
        let mut wanted = Vec::new();
        for (name, bytes) in files {
            std::fs::write(dir.join(name), bytes).unwrap();
            wanted.push(Wanted {
                filename: format!("@@m\\Radiohead\\{name}"),
                size: bytes.len() as i64,
            });
        }
        let stored = Stored {
            username: "alice".into(),
            folder: "@@m\\Radiohead".into(),
            parts: vec![Part {
                batch: "batch-0".into(),
                folder: String::new(),
                files: wanted,
            }],
        };
        let pool = db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (3, 'music', 'music', ?);
             INSERT INTO wanted_songs (id, title, artist, album, mb_recording_id, library_id, source, added_at)
                 VALUES (2, 'Paranoid Android', 'Radiohead', ?, '7d3a3d0b-0f4b-4b6f-9c8e-0a9b1f2c3d4e', 3, 'manual', 0);
             INSERT INTO downloads (id, source, soulseek, title, wanted_song_id, decision, state, added_at, updated_at, completed_at)
                 VALUES (4, 'soulseek', ?, 'Radiohead - Paranoid Android', 2, '{}', 'completed', 0, 0, 1);",
        )
        .bind(music.to_string_lossy().into_owned())
        .bind(album)
        .bind(serde_json::to_string(&stored).unwrap())
        .execute(&pool)
        .await
        .unwrap();
        Rig {
            pool,
            downloads,
            local,
        }
    }

    impl Rig {
        async fn album_in_library(&self, title: &str, folder: &str) {
            sqlx::query(
                "INSERT INTO music_albums (library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (3, ?, ?, 'Radiohead', ?, 1)",
            )
            .bind(format!("dir:{folder}"))
            .bind(title)
            .bind(folder)
            .execute(&self.pool)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn a_song_joins_its_album_when_the_library_has_it() {
        let r = song_rig(
            Some("ok computer"),
            &[("02 Paranoid Android.flac", b"paranoid")],
        )
        .await;
        r.album_in_library("OK Computer", "Radiohead/OK Computer (1997)")
            .await;
        r.import().await;
        let (state, message, destination, _) = r.state().await;
        assert_eq!(state, "imported", "{message:?}");
        assert_eq!(destination.as_deref(), Some(r.album().to_str().unwrap()));
        assert_eq!(tree(&r.album()), ["02 Paranoid Android.flac"]);
        assert!(!own_folder(r.downloads.path(), 4).exists());
    }

    #[tokio::test]
    async fn a_song_goes_to_its_album_or_singles_in_its_artists_folder() {
        let r = song_rig(
            Some("OK Computer"),
            &[("02 Paranoid Android.flac", b"paranoid")],
        )
        .await;
        r.album_in_library("Kid A", "R/Radiohead (band)/Kid A")
            .await;
        r.import().await;
        assert_eq!(r.state().await.0, "imported");
        let folder = r.local.path().join("music/R/Radiohead (band)/OK Computer");
        assert_eq!(tree(&folder), ["02 Paranoid Android.flac"]);

        let r = song_rig(None, &[("Paranoid Android.mp3", b"paranoid")]).await;
        r.import().await;
        assert_eq!(r.state().await.0, "imported");
        let singles = r.local.path().join("music/Radiohead/Singles");
        assert_eq!(tree(&singles), ["Paranoid Android.mp3"]);
    }

    #[tokio::test]
    async fn a_song_is_one_track() {
        let r = song_rig(None, &[("01.flac", b"one"), ("02.flac", b"two")]).await;
        r.import().await;
        let (state, message, _, _) = r.state().await;
        assert_eq!(state, "failed");
        assert_eq!(
            message.as_deref(),
            Some("a song is one track, and slskd's folder holds 2")
        );
    }

    #[tokio::test]
    async fn a_song_is_fixed_as_a_single_track() {
        let made = tempfile::tempdir().unwrap();
        let clip = made.path().join("clip.flac");
        let status = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=d=4"])
            .arg(&clip)
            .status()
            .expect("ffmpeg is installed");
        assert!(status.success());
        let audio = std::fs::read(&clip).unwrap();
        let r = song_rig(None, &[("Paranoid Android.flac", &audio)]).await;
        let fixes = Fixes {
            tools: crate::sound::Tools {
                ffmpeg: "ffmpeg".into(),
                ffprobe: "ffprobe".into(),
            },
            coverart: None,
            lyrics: None,
        };
        import(&r.pool, r.downloads.path(), Some(&fixes), 4)
            .await
            .unwrap();
        let (state, message, _, _) = r.state().await;
        assert_eq!(state, "imported");
        assert_eq!(message.as_deref(), Some("in the library; ReplayGain added"));
        let track = r
            .local
            .path()
            .join("music/Radiohead/Singles/Paranoid Android.flac");
        let tags = crate::library::tags::read(&track).unwrap();
        assert_eq!(
            (
                tags.title.as_deref(),
                tags.artist.as_deref(),
                tags.album.as_deref()
            ),
            (Some("Paranoid Android"), Some("Radiohead"), None)
        );
        assert_eq!(
            tags.mb_recording_id.as_deref(),
            Some("7d3a3d0b-0f4b-4b6f-9c8e-0a9b1f2c3d4e")
        );
    }

    #[tokio::test]
    async fn a_song_is_imported_before_an_album_that_finished_first() {
        let pool = db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
                 VALUES (1, 'mb-a', 'Radiohead', 1, 'none', 'manual', 0);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at)
                 VALUES (7, 1, 'rg-ok', 'OK Computer', 1, 'manual', 0);
             INSERT INTO wanted_songs (id, title, artist, source, added_at)
                 VALUES (2, 'Creep', 'Radiohead', 'manual', 0);
             INSERT INTO downloads (id, source, soulseek, title, wanted_album_id, wanted_song_id, decision, state,
                     added_at, updated_at, completed_at) VALUES
                 (4, 'soulseek', '{}', 'Radiohead - OK Computer', 7, NULL, '{}', 'completed', 0, 0, 1),
                 (5, 'soulseek', '{}', 'Radiohead - Creep', NULL, 2, '{}', 'completed', 0, 0, 5);",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            next_due(&pool).await.unwrap(),
            Some(5),
            "the song, though it finished later"
        );
        sqlx::query("UPDATE downloads SET state = 'imported' WHERE id = 5")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(next_due(&pool).await.unwrap(), Some(4));
    }

    #[tokio::test]
    async fn the_downloads_list_shows_the_import_line_in_the_order_its_taken() {
        let pool = db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
                 VALUES (1, 'mb-a', 'Radiohead', 1, 'none', 'manual', 0);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at)
                 VALUES (7, 1, 'rg-ok', 'OK Computer', 1, 'manual', 0), (8, 1, 'rg-kid', 'Kid A', 1, 'manual', 0);
             INSERT INTO wanted_songs (id, title, artist, source, added_at)
                 VALUES (2, 'Creep', 'Radiohead', 'manual', 0);
             INSERT INTO downloads (id, source, soulseek, title, wanted_album_id, wanted_song_id, decision, state,
                     added_at, updated_at, completed_at) VALUES
                 (4, 'soulseek', '{}', 'Radiohead - Kid A', 8, NULL, '{}', 'completed', 0, 0, 3),
                 (5, 'soulseek', '{}', 'Radiohead - OK Computer', 7, NULL, '{}', 'completed', 0, 0, 1),
                 (6, 'soulseek', '{}', 'Radiohead - Creep', NULL, 2, '{}', 'completed', 0, 0, 5),
                 (7, 'soulseek', '{}', 'Radiohead - Airbag', NULL, 2, '{}', 'downloading', 0, 0, NULL);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut line: Vec<(i64, Option<i64>)> = crate::downloads::list(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|d| (d.id, d.import_position))
            .collect();
        line.sort();
        assert_eq!(line, [(4, Some(3)), (5, Some(2)), (6, Some(1)), (7, None)]);
        assert_eq!(
            next_due(&pool).await.unwrap(),
            Some(6),
            "the first in line is the next taken"
        );
    }
}
