//! Libraries from `MELETE_LIBRARY_<NAME>=<kind>[,<category>]:<path>` variables.

use super::*;

pub(super) const LIBRARY_PREFIX: &str = "MELETE_LIBRARY_";

#[cfg(test)]
pub(super) fn parse_libraries(
    vars: impl IntoIterator<Item = (String, String)>,
) -> anyhow::Result<Vec<LibraryConfig>> {
    Ok(parse_library_vars(vars)?
        .into_iter()
        .map(|(library, _)| library)
        .collect())
}

/// Every `MELETE_LIBRARY_<NAME>=<kind>[,<category>]:<path>`, sorted by name.
pub(super) fn parse_library_vars(
    vars: impl IntoIterator<Item = (String, String)>,
) -> anyhow::Result<Vec<(LibraryConfig, Option<LibraryCategory>)>> {
    let mut libraries = Vec::new();
    for (key, value) in vars {
        let Some(name) = key.strip_prefix(LIBRARY_PREFIX) else {
            continue;
        };
        let name = name.to_ascii_lowercase();
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            bail!("{key}: library names may only use letters, digits and underscores");
        }
        let Some((kind, path)) = value.split_once(':') else {
            bail!(
                "{key}={value:?} should look like movies:/media/movies, shows:/media/shows or music:/media/music"
            );
        };
        let (kind, category) = match kind.split_once(',') {
            Some((kind, category)) => (kind, Some(category)),
            None => (kind, None),
        };
        let Some(kind) = LibraryKind::parse(kind) else {
            bail!("{key}: unknown library kind {kind:?}; use movies or shows");
        };
        let category = match category {
            None => None,
            Some(category) => {
                let Some(category) = LibraryCategory::parse(category) else {
                    bail!("{key}: unknown library category {category:?}; use anime or kids");
                };
                if kind == LibraryKind::Music {
                    bail!("{key}: only movies and shows libraries have a category");
                }
                Some(category)
            }
        };
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            bail!("{key}: path {path:?} must be absolute");
        }
        libraries.push((LibraryConfig { name, kind, path }, category));
    }
    libraries.sort_by(|(a, _), (b, _)| a.name.cmp(&b.name));
    Ok(libraries)
}
