//! Tagging a track: the wanted names and ids where its tags lack them, and its ReplayGain,
//! written to a hidden copy beside it which then takes its place.

use super::*;

pub(super) fn duration(path: &Path) -> Option<f64> {
    let file = open(path).ok()?;
    let seconds = file.properties().duration().as_secs_f64();
    (seconds > 0.0).then_some(seconds)
}

pub(super) fn open(path: &Path) -> Result<lofty::file::TaggedFile, String> {
    Probe::open(path)
        .map_err(|err| err.to_string())?
        .options(ParseOptions::new())
        .guess_file_type()
        .map_err(|err| err.to_string())?
        .read()
        .map_err(|err| err.to_string())
}

pub(super) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The hidden copy a track is tagged as, beside it.
pub(super) fn temporary(track: &Path, download: i64) -> PathBuf {
    track.with_file_name(format!(".{}.{download}.melete-fix", file_name(track)))
}

/// What [`retag`] writes.
pub(super) struct Writes {
    /// Written only where the tag has no value.
    pub(super) fill: Vec<(ItemKey, String)>,
    /// Always written.
    pub(super) set: Vec<(ItemKey, String)>,
}

/// The wanted album's names and ids, and the track's and album's ReplayGain when measured. A
/// track without its own gain gets no album gain either: a player would apply it alone.
pub(super) fn writes(wanted: &Wanted, track: Option<Loudness>, album: Option<Loudness>) -> Writes {
    let mut set = Vec::new();
    if let Some(track) = track {
        set.push((ItemKey::ReplayGainTrackGain, gain(track)));
        set.push((ItemKey::ReplayGainTrackPeak, peak(track)));
    }
    if let (Some(album), Some(_)) = (album, track) {
        set.push((ItemKey::ReplayGainAlbumGain, gain(album)));
        set.push((ItemKey::ReplayGainAlbumPeak, peak(album)));
    }
    Writes {
        fill: vec![
            (ItemKey::AlbumTitle, wanted.title.clone()),
            (ItemKey::AlbumArtist, wanted.artist.clone()),
            (
                ItemKey::MusicBrainzReleaseGroupId,
                wanted.mb_release_group_id.clone(),
            ),
            (
                ItemKey::MusicBrainzReleaseArtistId,
                wanted.mb_artist_id.clone(),
            ),
        ],
        set,
    }
}

#[derive(Debug)]
pub(super) enum Retag {
    /// Writing to the library failed.
    Library(std::io::Error),
    /// The file's tags couldn't be read or written; it's left as it came.
    Tags(String),
}

/// Writes `writes` into `path`'s tags by way of `temp`. Returns whether anything changed.
pub(super) fn retag(path: &Path, temp: &Path, writes: &Writes) -> Result<bool, Retag> {
    copy_to(path, temp).map_err(Retag::Library)?;
    let result = edit(temp, writes);
    if !matches!(result, Ok(true)) {
        let _ = std::fs::remove_file(temp);
        return result.map_err(Retag::Tags);
    }
    let done = (|| {
        std::fs::File::open(temp)?.sync_all()?;
        std::fs::set_permissions(temp, std::fs::Permissions::from_mode(0o664))?;
        // Replaces only the file this import just placed.
        std::fs::rename(temp, path)
    })();
    if let Err(err) = done {
        let _ = std::fs::remove_file(temp);
        return Err(Retag::Library(err));
    }
    Ok(true)
}

/// A fresh copy of `path` at `temp`, never written through a link.
pub(super) fn copy_to(path: &Path, temp: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(temp) {
        Ok(_) => std::fs::remove_file(temp)?,
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let mut from = std::fs::File::open(path)?;
    let mut to = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(temp)?;
    std::io::copy(&mut from, &mut to)?;
    to.sync_all()
}

/// Edits the tag a scan reads (the file's main one, else the first it has, else a new main
/// one), saving only when something changed.
pub(super) fn edit(path: &Path, writes: &Writes) -> Result<bool, String> {
    let mut file = open(path)?;
    if file.primary_tag().is_none() && file.first_tag().is_none() {
        file.insert_tag(Tag::new(file.primary_tag_type()));
    }
    let tag = match file.primary_tag().is_some() {
        true => file.primary_tag_mut(),
        false => file.first_tag_mut(),
    }
    .ok_or("no tag to write")?;
    let mut changed = false;
    for (key, value) in &writes.fill {
        if tag.get_string(*key).is_none_or(|v| v.trim().is_empty()) {
            changed |= tag.insert_text(*key, value.clone());
        }
    }
    for (key, value) in &writes.set {
        if tag.get_string(*key) != Some(value.as_str()) {
            changed |= tag.insert_text(*key, value.clone());
        }
    }
    if changed {
        tag.save_to_path(path, WriteOptions::default())
            .map_err(|err| err.to_string())?;
    }
    Ok(changed)
}
