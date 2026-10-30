//! Reading a scan's files: each album folder's artwork by name, and tags a few files at a time.

use std::collections::BTreeMap;
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

/// Files a scan's summary of tag problems names.
const EXAMPLES: usize = 3;

/// The files whose tags had problems in one scan: how many, and the first few by path with
/// the first problem each had. A problem is a warning from the tag reader, or tags that
/// couldn't be read at all.
#[derive(Debug, Default, PartialEq)]
pub(super) struct TagProblems {
    /// How many files had problems.
    pub(super) files: usize,
    /// The first few of them by path, with the first problem each had.
    pub(super) examples: BTreeMap<String, String>,
}

impl TagProblems {
    /// Counts a file with a problem, keeping it as an example if it's among the first by path.
    pub(super) fn note(&mut self, rel_path: &str, problem: String) {
        self.files += 1;
        self.examples.insert(rel_path.to_owned(), problem);
        if self.examples.len() > EXAMPLES {
            self.examples.pop_last();
        }
    }

    /// One line in the log for the whole scan, naming a few files; none when there were no
    /// problems.
    pub(super) fn log(&self, library: &str) {
        if self.files == 0 {
            return;
        }
        let examples: Vec<String> = self
            .examples
            .iter()
            .map(|(path, problem)| format!("{path}: {problem}"))
            .collect();
        tracing::warn!(
            library,
            files = self.files,
            examples = examples.join("; "),
            "some files' tags had problems (RUST_LOG=info,lofty=warn names each)"
        );
    }
}

/// Reads tags for each file, a few at a time, noting the files whose tags had problems.
pub(super) fn read_all(
    root: &Path,
    files: Vec<String>,
) -> (HashMap<String, Result<Tags, String>>, TagProblems) {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(HashMap::with_capacity(files.len()));
    let problems = Mutex::new(TagProblems::default());
    std::thread::scope(|scope| {
        for _ in 0..READERS.min(files.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(rel_path) = files.get(i) else { break };
                    let path = root.join(rel_path);
                    // A file that trips up the tag reader is that file's problem, not the scan's.
                    let (result, warnings) = crate::logging::noting(|| {
                        std::panic::catch_unwind(|| tags::read(&path)).unwrap_or_else(|_| {
                            Err("the tag reader failed on this file".to_owned())
                        })
                    });
                    let problem = match &result {
                        Err(err) => Some(err.clone()),
                        Ok(_) => warnings.into_iter().next(),
                    };
                    if let Some(problem) = problem {
                        problems
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .note(rel_path, problem);
                    }
                    results
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(rel_path.clone(), result);
                }
            });
        }
    });
    (
        results.into_inner().unwrap_or_else(PoisonError::into_inner),
        problems
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner),
    )
}
