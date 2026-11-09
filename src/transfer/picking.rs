//! Which of a download's files are fetched, and where it goes: the plan, by extension, track and
//! cover; a movie's or season's extras go in its `Extras` folder.

use super::*;

/// Works out where a download goes and which of its files are fetched.
pub(super) async fn plan(
    pool: &SqlitePool,
    seedbox: &Seedbox,
    row: &Row,
) -> anyhow::Result<Result<Plan, Stop>> {
    // Every series download names its season; one that doesn't (its episode left out by the
    // order its series follows now) would go loose in the series' folder, its running time
    // unchecked.
    if row.wanted_series_id.is_some() && row.season.is_none() {
        return Ok(Err(Stop::Fail(
            "it names no season of its series (the order the series follows now leaves its \
             episode out), so it isn't brought home"
                .into(),
        )));
    }
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
    let shared = shared_folder(row, &paths);
    let film = film(row, &paths);
    let only = match taken(pool, row).await? {
        Ok(only) => only,
        Err(stop) => return Ok(Err(stop)),
    };
    let mut planned: Vec<Planned> = Vec::new();
    for (file, parts) in &paths {
        let (folders, name) = parts.split_at(parts.len() - 1);
        let name = name[0];
        if let Some(only) = &only
            && !only.holds(pool, parts).await?
        {
            continue;
        }
        let folder = match shared {
            None if wanted_file(parts) && is_extra(parts) && film != Some(file.path.as_str()) => {
                EXTRAS.to_owned()
            }
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
    if let Some(stop) = nothing_to_import(row, &planned, shared.is_some(), only.as_ref()) {
        return Ok(Err(stop));
    }
    Ok(Ok(Plan {
        root,
        dir,
        files: planned,
    }))
}

/// An album keeps its disc folders: a track goes in its folder below the one all the tracks
/// share, which this is. Anything else goes straight in the title's folder (None).
fn shared_folder<'a>(
    row: &Row,
    paths: &'a [(&TorrentFile, Vec<&'a str>)],
) -> Option<&'a [&'a str]> {
    row.wanted_album_id.map(|_| {
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
    })
}

/// The season a pack is brought home for, and the series it's of: the episodes of it the pack
/// was taken for, or every episode of it from a pack of several seasons.
struct Taken {
    wanted: crate::search::Wanted,
    season: i64,
    /// None: any episode of the season (a pack of several seasons, recorded for this one).
    episodes: Option<Vec<i64>>,
}

/// What a season pack taken in part, or a pack of several seasons, is brought home for; None
/// for any other download, all of whose files are brought home. A pack of several seasons is
/// recorded for the season it was searched for, and only that season's files come home (each
/// is checked against that season's runtimes); the rest stay on the seedbox.
async fn taken(pool: &SqlitePool, row: &Row) -> anyhow::Result<Result<Option<Taken>, Stop>> {
    let episodes: Option<Vec<i64>> = match row.episodes.as_deref() {
        Some(episodes) => Some(serde_json::from_str(episodes)?),
        None if row.wanted_series_id.is_some() && several_seasons(&row.title) => None,
        None => return Ok(Ok(None)),
    };
    let target = row.target();
    let wanted = match target {
        Some(target @ Target::Series { .. }) => crate::search::load_wanted(pool, target).await?,
        _ => None,
    };
    let (Some(wanted), Some(Target::Series { season, .. })) = (wanted, target) else {
        return Ok(Err(Stop::Fail(
            "it was taken for part of a series that isn't wanted any more".into(),
        )));
    };
    Ok(Ok(Some(Taken {
        wanted,
        season,
        episodes,
    })))
}

impl Taken {
    /// Whether a file is one of the episodes taken, for certain: its own name gives the season
    /// and episodes (or numbers from the first episode that map to them), every one of them
    /// taken (any, from a pack of several seasons). Extras, anything in an extras folder whatever its name, and anything else stay
    /// behind.
    async fn holds(&self, pool: &SqlitePool, parts: &[&str]) -> anyhow::Result<bool> {
        let Some((name, folders)) = parts.split_last() else {
            return Ok(false);
        };
        if is_extra(parts) || in_extras_folder(folders) {
            return Ok(false);
        }
        let named = crate::release::parse(name).episodes;
        Ok(
            match crate::search::numbered_in_order(pool, &self.wanted, &named).await? {
                Ok(crate::release::Episodes::Standard { season, episodes }) => {
                    season == self.season
                        && !episodes.is_empty()
                        && self
                            .episodes
                            .as_ref()
                            .is_none_or(|taken| episodes.iter().all(|e| taken.contains(e)))
                }
                _ => false,
            },
        )
    }
}

/// Whether a release's name says it holds several seasons ("S01-S03", "Complete Series").
fn several_seasons(title: &str) -> bool {
    match crate::release::parse(title).episodes {
        crate::release::Episodes::Seasons(seasons) => seasons.len() > 1,
        crate::release::Episodes::CompleteSeries => true,
        _ => false,
    }
}

/// A movie's largest video, which is the film whatever its name says ("Trailer Park…").
fn film<'a>(row: &Row, paths: &[(&'a TorrentFile, Vec<&str>)]) -> Option<&'a str> {
    row.wanted_movie_id?;
    paths
        .iter()
        .filter(|(file, _)| has_extension(&file.path, VIDEO))
        .max_by_key(|(file, _)| file.size)
        .map(|(file, _)| file.path.as_str())
}

/// Why a plan has nothing to import: no audio for an album, no video, only extras, or none of
/// the episodes a season pack was taken for.
fn nothing_to_import(
    row: &Row,
    planned: &[Planned],
    album: bool,
    only: Option<&Taken>,
) -> Option<Stop> {
    if let Some(only) = only
        && !planned.iter().any(|p| has_extension(&p.name, VIDEO))
    {
        return Some(Stop::Fail(match &only.episodes {
            Some(episodes) => format!(
                "none of its files can be told to be one of the episodes it was taken for ({} of \
                 season {})",
                crate::rss::numbers(episodes),
                only.season
            ),
            None => format!(
                "it holds several seasons, and none of its files can be told to be an episode of \
                 season {}, the one it was taken for",
                only.season
            ),
        }));
    }
    let (needed, kind) = if album {
        (AUDIO_EXTENSIONS, "audio")
    } else {
        (VIDEO, "video")
    };
    if !planned.iter().any(|p| has_extension(&p.name, needed)) {
        return Some(Stop::Fail(format!(
            "{} has no {kind} files to import",
            row.title
        )));
    }
    let own_video = |p: &Planned| has_extension(&p.name, VIDEO) && p.folder != EXTRAS;
    (!album && !planned.iter().any(own_video)).then(|| {
        Stop::Fail(format!(
            "{} holds only extras (creditless songs, trailers and the like), none of the title's own videos",
            row.title
        ))
    })
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
