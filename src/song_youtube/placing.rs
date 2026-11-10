//! Putting a song from YouTube in the library: beside its album, or in `Singles`, as a Soulseek
//! song goes, never replacing a file, then tagged as the song.

use super::*;
use crate::transfer::Stop;

/// Puts the downloaded audio where a Soulseek song of it would go, under the song's title,
/// never replacing a file (a second copy gets " (2)", and so on), tags it as the song, and asks
/// for a scan of its folder. Where it went, from the library's root, and the tagging's note.
pub(super) async fn import(
    pool: &SqlitePool,
    fixes: &Fixes,
    song: &Wanted,
    file: &Path,
) -> Result<(String, String), String> {
    let destination = crate::transfer::song_destination(pool, song.id)
        .await
        .map_err(|err| format!("the database: {err:#}"))?;
    let Some((root, dir)) = destination else {
        return Err("it has no music library, so there's nowhere to put it".into());
    };
    let extension = file
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| ["opus", "m4a"].contains(e))
        .ok_or("the downloader gave neither Opus nor AAC audio")?
        .to_owned();
    let stem = crate::transfer::folder_name(&song.title, None);
    if stem.is_empty() {
        return Err("its title has nothing a file name can hold".into());
    }
    let (from, into) = (file.to_path_buf(), dir.clone());
    let target = crate::transfer::blocking(move || place(&from, &into, &stem, &extension))
        .await
        .map_err(why)??;
    // Tagged under an id of its own, apart from downloads': the song's, negated.
    let note = crate::album_fix::fix_song(pool, fixes, -song.id, song.id, &target).await?;
    if let Err(err) = crate::library::ask_scan_of(pool, &dir).await {
        tracing::warn!(error = %err, "couldn't ask for a scan of a song from YouTube");
    }
    let placed = target
        .strip_prefix(&root)
        .unwrap_or(&target)
        .to_string_lossy()
        .into_owned();
    Ok((placed, note))
}

/// Copies `from` into `dir` as `<stem>.<extension>`, or `<stem> (2).<extension>` and so on when
/// a file has the name, never replacing one.
fn place(
    from: &Path,
    dir: &Path,
    stem: &str,
    extension: &str,
) -> Result<Result<PathBuf, String>, Stop> {
    crate::transfer::create_dirs(dir)?;
    for n in 1..10 {
        let name = if n == 1 {
            format!("{stem}.{extension}")
        } else {
            format!("{stem} ({n}).{extension}")
        };
        let target = dir.join(&name);
        if target.exists() {
            continue;
        }
        let partial = crate::transfer::partial_path(dir, &name, 0)?;
        std::fs::copy(from, &partial)?;
        return match crate::transfer::place(&partial, &target) {
            Ok(()) => Ok(Ok(target)),
            Err(stop) => {
                let _ = std::fs::remove_file(&partial);
                Err(stop)
            }
        };
    }
    Ok(Err(format!("{stem} has nine copies in its folder already")))
}

/// Why placing stopped, in words.
fn why(stop: Stop) -> String {
    match stop {
        Stop::Retry(why) | Stop::Fail(why) | Stop::NotTheTitle(why) | Stop::Held(why) => why,
    }
}
