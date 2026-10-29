//! Which of a download's files are fetched, and where it goes: the plan, by extension, track and
//! cover.

use super::*;

/// Works out where a download goes and which of its files are fetched.
pub(super) async fn plan(
    pool: &SqlitePool,
    seedbox: &Seedbox,
    row: &Row,
) -> anyhow::Result<Result<Plan, Stop>> {
    let Some((root, dir)) = destination(pool, row).await? else {
        return Ok(Err(Stop::Fail(
            "it isn't for a wanted title with a library any more, so there's nowhere to put it"
                .into(),
        )));
    };
    let (Some(save_path), Some(files)) = (row.save_path.as_deref(), row.files.as_deref()) else {
        return Ok(Err(Stop::Fail(
            "Deluge didn't say where its files are".into(),
        )));
    };
    let Some(remote_dir) = seedbox.sftp_path(save_path) else {
        return Ok(Err(Stop::Fail(format!(
            "Deluge keeps it in {save_path}, outside the folder MELETE_SEEDBOX_PATHS maps"
        ))));
    };
    let files: Vec<TorrentFile> = serde_json::from_str(files)?;
    let mut paths: Vec<(&TorrentFile, Vec<&str>)> = Vec::new();
    for file in &files {
        let parts: Vec<&str> = file.path.split('/').collect();
        if file.path.starts_with('/')
            || parts
                .iter()
                .any(|p| p.is_empty() || *p == "." || *p == "..")
        {
            return Ok(Err(Stop::Fail(format!(
                "Deluge lists a file with an odd path: {:?}",
                file.path
            ))));
        }
        paths.push((file, parts));
    }
    // An album keeps its disc folders: a track goes in its folder below the one all the tracks
    // share. Anything else goes straight in the title's folder.
    let shared: Option<&[&str]> = row.wanted_album_id.map(|_| {
        paths
            .iter()
            .filter(|(_, parts)| is_track(parts))
            .map(|(_, parts)| &parts[..parts.len() - 1])
            .reduce(|shared, folder| {
                let same = shared
                    .iter()
                    .zip(folder)
                    .take_while(|(a, b)| a == b)
                    .count();
                &shared[..same]
            })
            .unwrap_or_default()
    });
    let mut planned: Vec<Planned> = Vec::new();
    for (file, parts) in &paths {
        let (folders, name) = parts.split_at(parts.len() - 1);
        let name = name[0];
        let folder = match shared {
            None if wanted_file(parts) => String::new(),
            Some(shared) if is_track(parts) => folders[shared.len()..].join("/"),
            Some(shared) if is_cover(name) && folders == shared => String::new(),
            _ => continue,
        };
        if planned.iter().any(|p| p.folder == folder && p.name == name) {
            return Ok(Err(Stop::Fail(format!(
                "two of its files are named {name}"
            ))));
        }
        let remote = match remote_dir.as_str() {
            "/" => format!("/{}", file.path),
            dir => format!("{dir}/{}", file.path),
        };
        planned.push(Planned {
            remote,
            size: u64::try_from(file.size).unwrap_or_default(),
            folder,
            name: name.to_owned(),
        });
    }
    let (needed, kind) = match shared {
        Some(_) => (AUDIO_EXTENSIONS, "audio"),
        None => (VIDEO, "video"),
    };
    if !planned.iter().any(|p| has_extension(&p.name, needed)) {
        return Ok(Err(Stop::Fail(format!(
            "{} has no {kind} files to import",
            row.title
        ))));
    }
    Ok(Ok(Plan {
        root,
        dir,
        files: planned,
    }))
}

pub(crate) fn has_extension(name: &str, extensions: &[&str]) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.contains(&e.to_ascii_lowercase().as_str()))
}

/// Videos and subtitles, but not samples or what's in a Sample or Screens folder.
pub(super) fn wanted_file(parts: &[&str]) -> bool {
    let name = parts.last().copied().unwrap_or_default();
    let in_skipped_folder = parts[..parts.len().saturating_sub(1)]
        .iter()
        .any(|p| ["sample", "samples", "screens"].contains(&p.to_ascii_lowercase().as_str()));
    let sample = name.to_ascii_lowercase().contains("sample");
    (has_extension(name, VIDEO) || has_extension(name, SUBTITLES)) && !in_skipped_folder && !sample
}

/// An album's track: an audio file, not one in a sample folder, nor a Mac's hidden copy of one
/// ("._01.flac", or anything in __MACOSX).
pub(super) fn is_track(parts: &[&str]) -> bool {
    let name = parts.last().copied().unwrap_or_default();
    let skipped = parts.iter().any(|p| {
        p.starts_with('.')
            || ["sample", "samples", "__macosx"].contains(&p.to_ascii_lowercase().as_str())
    });
    has_extension(name, AUDIO_EXTENSIONS) && !skipped
}

/// An album's cover image, named as a library scan looks for one ("cover.jpg", "folder.png"…).
pub(crate) fn is_cover(name: &str) -> bool {
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    COVER_NAMES.contains(&stem.as_str()) && has_extension(name, IMAGE_EXTENSIONS)
}
