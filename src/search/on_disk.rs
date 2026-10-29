//! What's on disk where a release would go, to judge whether it's better.

use super::*;

/// What's on disk where a release would go.
pub(crate) enum OnDisk {
    Nothing,
    File(Existing),
    /// A file is there but can't be judged, so no release can be called better.
    Unclear(String),
}

/// The best file already on disk for a movie, or for episodes `first..=last` of a season, for
/// upgrade decisions. Season packs aren't compared against files: they replace several at
/// once.
pub(super) async fn on_disk(
    pool: &SqlitePool,
    profile: &Loaded,
    wanted: &Wanted,
    episodes: Option<(i64, i64, i64)>,
) -> anyhow::Result<OnDisk> {
    // Each present file, with what ffprobe found in it (when its probe is current).
    let files: Vec<(String, Option<String>)> = match (episodes, wanted.movie_id, wanted.series_id) {
        (None, Some(movie_id), _) => {
            sqlx::query_as(
                "SELECT f.rel_path, p.info FROM media_files f
                 LEFT JOIN media_probes p ON p.file_id = f.id AND p.size = f.size AND p.modified_at = f.modified_at
                 WHERE f.movie_id = ? AND f.missing_since IS NULL",
            )
            .bind(movie_id)
            .fetch_all(pool)
            .await?
        }
        (Some((season, first, last)), _, Some(series_id)) => {
            sqlx::query_as(
                "SELECT f.rel_path, p.info FROM media_files f
                 LEFT JOIN media_probes p ON p.file_id = f.id AND p.size = f.size AND p.modified_at = f.modified_at
                 WHERE f.series_id = ? AND f.season = ? AND f.episode <= ? AND coalesce(f.episode_end, f.episode) >= ?
                   AND f.missing_since IS NULL",
            )
            .bind(series_id)
            .bind(season)
            .bind(last)
            .bind(first)
            .fetch_all(pool)
            .await?
        }
        _ => Vec::new(),
    };
    // No numbered file for these episodes, but files Melete couldn't number that look numbered
    // ("Show - 05.mkv"): one of them may be this episode.
    if files.is_empty()
        && let (Some(_), Some(series_id)) = (episodes, wanted.series_id)
    {
        let unnumbered: Vec<String> = sqlx::query_scalar(
            "SELECT rel_path FROM media_files WHERE series_id = ? AND season IS NULL AND missing_since IS NULL",
        )
        .bind(series_id)
        .fetch_all(pool)
        .await?;
        if let Some(example) = unnumbered
            .iter()
            .map(|p| file_name(p))
            .find(|n| looks_numbered(n))
        {
            return Ok(OnDisk::Unclear(format!(
                "the show has files Melete couldn't number, like {example}, so it can't tell whether this episode is already there"
            )));
        }
    }
    Ok(best_of(
        profile,
        &files,
        wanted.original_language.as_deref(),
    ))
}

/// Whether a wanted movie's file, or one episode's, is below the title's profile's cutoff, as
/// an upgrade search for it would judge the file it compares with; None when there's no such
/// file, the title has no profile here, or the file can't be judged yet.
pub(crate) async fn cutoff_unmet(
    pool: &SqlitePool,
    target: Target,
) -> anyhow::Result<Option<bool>> {
    let episodes = match target {
        Target::Movie(_) => None,
        Target::Series {
            season,
            episode: Some(episode),
            ..
        } => Some((season, episode, episode)),
        Target::Series { episode: None, .. } => return Ok(None),
    };
    let Some(wanted) = load_wanted(pool, target).await? else {
        return Ok(None);
    };
    let Ok(profile) = profile_for(pool, target, &wanted).await? else {
        return Ok(None);
    };
    Ok(match on_disk(pool, &profile, &wanted, episodes).await? {
        OnDisk::File(existing) => Some(profile.cutoff_unmet(&existing)),
        OnDisk::Nothing | OnDisk::Unclear(_) => None,
    })
}

/// The best of a title's files on disk (paths, with their probes' JSON when current) under its
/// profile: the highest-ranked quality, then the highest custom format score. Unclear when any
/// of them can't be judged. The managed lists judge cutoff unmet with this too, so they and
/// the upgrades never disagree.
pub(crate) fn best_of(
    profile: &Loaded,
    files: &[(String, Option<String>)],
    original: Option<&str>,
) -> OnDisk {
    let mut read = Vec::with_capacity(files.len());
    for (path, probe) in files {
        match Read::file(path, probe.as_deref()) {
            Ok(file) => read.push(file),
            Err(unclear) => return unclear,
        }
    }
    best_read(profile, &read, original)
}

/// A library file with its quality read: its name, and what that was read from, parsed.
pub(crate) struct Read {
    name: String,
    facts_name: String,
    release: Release,
}

impl Read {
    /// Reads a file's quality from its path, and its probe's JSON when the name doesn't say;
    /// Unclear when neither does.
    pub(crate) fn file(path: &str, probe: Option<&str>) -> Result<Read, OnDisk> {
        let name = file_name(path);
        match judge_file(&name, probe) {
            Some((facts_name, release)) => Ok(Read {
                name,
                facts_name,
                release,
            }),
            None => Err(OnDisk::Unclear(format!(
                "the file on disk ({name}) doesn't name its quality and hasn't been probed yet, so Melete can't tell whether this is better"
            ))),
        }
    }
}

/// [`best_of`] for files already read.
pub(crate) fn best_read<'a>(
    profile: &Loaded,
    files: impl IntoIterator<Item = &'a Read>,
    original: Option<&str>,
) -> OnDisk {
    let mut best: Option<(i64, Existing)> = None;
    for file in files {
        let facts = Facts::new(&file.facts_name, &file.release, None, &[], original);
        let score = match profile.score(&facts) {
            Ok((_, score)) => score,
            Err(err) => {
                let name = &file.name;
                return OnDisk::Unclear(format!("couldn't judge the file on disk ({name}): {err}"));
            }
        };
        let rank = profile.rank_of(facts.quality.name);
        let candidate = Existing {
            quality: facts.quality.name.to_owned(),
            proper: file.release.proper,
            score,
        };
        if best
            .as_ref()
            .is_none_or(|(r, e)| (rank, score) > (*r, e.score))
        {
            best = Some((rank, candidate));
        }
    }
    best.map_or(OnDisk::Nothing, |(_, existing)| OnDisk::File(existing))
}

/// What a library file's quality is read from: its name, parsed; or, when the name doesn't
/// say, the name with what ffprobe found in it (`probe`, the probe's JSON). With no source
/// known, the file counts as the best quality at its resolution, so only a higher resolution
/// replaces it. None when the name doesn't say and there's no probe.
pub(crate) fn judge_file(name: &str, probe: Option<&str>) -> Option<(String, Release)> {
    let parsed = release::parse(name);
    if parsed.quality() != "Unknown" {
        return Some((name.to_owned(), parsed));
    }
    let described = probe.and_then(describe)?;
    let facts_name = format!("{name} {described}");
    let judged = release::parse(&facts_name);
    Some((facts_name, judged))
}

pub(crate) fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_owned()
}

/// What ffprobe found, written like a release name's tags ("2160p BluRay REMUX DV"): the
/// resolution from the picture's size (width first, so cropped films count at their real
/// resolution), the best source at that resolution, and the HDR type.
pub(super) fn describe(probe: &str) -> Option<String> {
    let info: crate::probe::MediaInfo = serde_json::from_str(probe).ok()?;
    let video = info.video.first()?;
    let (width, height) = (video.width.unwrap_or(0), video.height.unwrap_or(0));
    let resolution = match (width, height) {
        (w, h) if w >= 3200 || h >= 1800 => 2160,
        (w, h) if w >= 1700 || h >= 900 => 1080,
        (w, h) if w >= 1200 || h >= 600 => 720,
        (w, h) if w > 0 || h > 0 => 480,
        _ => return None,
    };
    let source = if resolution >= 1080 {
        "BluRay REMUX"
    } else {
        "BluRay"
    };
    let hdr = match video.hdr.as_deref() {
        Some("Dolby Vision") => " DV HDR10",
        Some("HDR10") => " HDR10",
        Some("HLG") => " HLG",
        _ => "",
    };
    Some(format!("{resolution}p {source}{hdr}"))
}

/// Whether a file name holds what looks like an episode number: a standalone number of up to
/// four digits that isn't a year or a resolution ("Show - 05.mkv", but not "Behind the
/// Scenes.mkv" or "Making of (2019).mkv").
pub(super) fn looks_numbered(name: &str) -> bool {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    stem.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| {
            !word.is_empty() && word.len() <= 4 && word.bytes().all(|b| b.is_ascii_digit())
        })
        .any(|word| {
            let n: i64 = word.parse().unwrap_or(0);
            !(word.len() == 4 && (1900..=2099).contains(&n))
        })
}
