//! Reading a scan's files: each album folder's artwork by name, and tags a few files at a time.

use std::sync::PoisonError;

use super::*;

/// Each album folder's cover image and motion cover, by name. A folder that can't be read just
/// has none; the library's root isn't an album's folder.
pub(super) fn find_artwork(root: &Path, folders: &[String]) -> HashMap<String, Artwork> {
    let mut found = HashMap::new();
    for folder in folders.iter().filter(|f| !f.is_empty()) {
        let Ok(entries) = std::fs::read_dir(root.join(folder)) else {
            continue;
        };
        let names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        let pick = |stems: &[&str], extensions: &[&str]| {
            stems.iter().find_map(|stem| {
                names.iter().find(|name| {
                    let lower = name.to_lowercase();
                    lower
                        .rsplit_once('.')
                        .is_some_and(|(s, e)| s == *stem && extensions.contains(&e))
                })
            })
        };
        let artwork = Artwork {
            cover: pick(COVER_NAMES, IMAGE_EXTENSIONS).map(|n| format!("{folder}/{n}")),
            motion: pick(MOTION_NAMES, VIDEO_EXTENSIONS).map(|n| format!("{folder}/{n}")),
        };
        if artwork != Artwork::default() {
            found.insert(folder.clone(), artwork);
        }
    }
    found
}

/// Reads tags for each file, a few at a time.
pub(super) fn read_all(root: &Path, files: Vec<String>) -> HashMap<String, Result<Tags, String>> {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(HashMap::with_capacity(files.len()));
    std::thread::scope(|scope| {
        for _ in 0..READERS.min(files.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(rel_path) = files.get(i) else { break };
                    let path = root.join(rel_path);
                    // A file that trips up the tag reader is that file's problem, not the scan's.
                    let result = std::panic::catch_unwind(|| tags::read(&path))
                        .unwrap_or_else(|_| Err("the tag reader failed on this file".to_owned()));
                    results
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(rel_path.clone(), result);
                }
            });
        }
    });
    results.into_inner().unwrap_or_else(PoisonError::into_inner)
}
