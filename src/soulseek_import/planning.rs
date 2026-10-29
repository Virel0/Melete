//! Planning a Soulseek download's import from its own folders in slskd's downloads folder.

use super::*;

/// What to bring home from the download's own folders: its tracks and cover images, once the
/// folders hold exactly the bytes asked for.
pub(super) fn local_plan(
    downloads: &Path,
    id: i64,
    stored: &Stored,
    title: &str,
    root: PathBuf,
    dir: PathBuf,
) -> Result<Plan, Stop> {
    let own = checked_own_folder(downloads, id)?;
    let expected: i64 = stored
        .parts
        .iter()
        .flat_map(|p| &p.files)
        .map(|f| f.size)
        .sum();
    let (found, mut planned) = read_parts(stored, &own)?;
    // What slskd wrote must be what was asked for: names may differ, sizes can't.
    if found != expected {
        return Err(Stop::Fail(format!(
            "slskd's folder holds {found} bytes where {expected} were asked for"
        )));
    }
    for (i, file) in planned.iter().enumerate() {
        if planned[..i]
            .iter()
            .any(|p| p.folder == file.folder && p.name == file.name)
        {
            return Err(Stop::Fail(format!(
                "two of its files are named {}",
                file.name
            )));
        }
    }
    if !planned
        .iter()
        .any(|p| transfer::has_extension(&p.name, AUDIO_EXTENSIONS))
    {
        return Err(Stop::Fail(format!("{title} has no audio files to import")));
    }
    planned.sort_by(|a, b| (&a.folder, &a.name).cmp(&(&b.folder, &b.name)));
    Ok(Plan {
        root,
        dir,
        files: planned,
    })
}

/// Checks slskd's downloads folder is mounted and the download's folders there are plain
/// folders of Melete's own; returns the download's own folder.
fn checked_own_folder(downloads: &Path, id: i64) -> Result<PathBuf, Stop> {
    // An unmounted folder looks empty, not missing, inside a container; either way, wait.
    let mounted = std::fs::read_dir(downloads)
        .map(|mut d| d.next().is_some())
        .unwrap_or(false);
    if !mounted {
        return Err(Stop::Retry(format!(
            "slskd's downloads folder {} is missing or empty; is it mounted?",
            downloads.display()
        )));
    }
    let own = own_folder(downloads, id);
    // Only real folders of Melete's own: never a link to somewhere else.
    for folder in [downloads.join("melete"), own.clone()] {
        match std::fs::symlink_metadata(&folder) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(Stop::Fail(format!(
                    "{} isn't a plain folder; Melete won't read through it",
                    folder.display()
                )));
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                return Err(Stop::Fail(format!(
                    "slskd's folder for it, {}, isn't there",
                    own.display()
                )));
            }
            Err(err) => return Err(err.into()),
        }
    }
    Ok(own)
}

/// Lists the tracks and cover images in each part's folder, refusing anything but plain
/// files with text names; returns them with the bytes the folders hold in all.
fn read_parts(stored: &Stored, own: &Path) -> Result<(i64, Vec<Planned>), Stop> {
    let mut found: i64 = 0;
    let mut planned: Vec<Planned> = Vec::new();
    for (n, part) in stored.parts.iter().enumerate() {
        let folder = Path::new(&part.folder);
        if folder
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(Stop::Fail(format!("an odd folder name: {:?}", part.folder)));
        }
        let local = own.join((n + 1).to_string());
        let entries = match std::fs::read_dir(&local) {
            Ok(entries) => entries,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                return Err(Stop::Fail(format!("{} isn't there", local.display())));
            }
            Err(err) => return Err(err.into()),
        };
        for entry in entries {
            let entry = entry?;
            let meta = std::fs::symlink_metadata(entry.path())?;
            if !meta.file_type().is_file() {
                return Err(Stop::Fail(format!(
                    "{} isn't a plain file; Melete only takes files",
                    entry.path().display()
                )));
            }
            let Ok(name) = entry.file_name().into_string() else {
                return Err(Stop::Fail(format!(
                    "{} has a name that isn't text",
                    entry.path().display()
                )));
            };
            found += meta.len() as i64;
            let track = transfer::has_extension(&name, AUDIO_EXTENSIONS) && !name.starts_with('.');
            let cover = part.folder.is_empty() && transfer::is_cover(&name);
            if !track && !cover {
                continue;
            }
            planned.push(Planned {
                remote: entry.path().to_string_lossy().into_owned(),
                size: meta.len(),
                folder: part.folder.clone(),
                name,
            });
        }
    }
    Ok((found, planned))
}
