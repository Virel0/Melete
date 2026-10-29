//! A release's group, from the end of its name, and its title cleaned of separators.

use super::*;

/// "-GROUP" at the end, ignoring a trailing "[site]" tag and Usenet poster suffixes.
pub(super) fn suffix_group(rest: &str) -> Option<String> {
    let mut trimmed = rest.trim_end();
    // "-[YTS.MX]" is the group; "[TGx]" and "(eztv)" after it are the site.
    if let Some(i) = trimmed.rfind(['[', '('])
        && trimmed.ends_with([']', ')'])
    {
        if trimmed[..i].ends_with('-') {
            let group = &trimmed[i + 1..trimmed.len() - 1];
            return (!group.is_empty() && group.len() <= 32).then(|| group.to_owned());
        }
        trimmed = trimmed[..i].trim_end();
    }
    while let Some((before, last)) = trimmed.rsplit_once('-') {
        if POSTER_SUFFIXES.contains(&last.to_ascii_lowercase().as_str()) {
            trimmed = before;
        } else {
            break;
        }
    }
    if let Some(dashed) = DASHED_GROUPS
        .iter()
        .find(|g| trimmed.ends_with(&format!("-{g}")))
    {
        return Some((*dashed).to_owned());
    }
    let (_, group) = trimmed.rsplit_once('-')?;
    let plain = !group.is_empty()
        && group.len() <= 32
        && group
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.');
    let first_part = group
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    (plain && !NOT_GROUPS.contains(&first_part.as_str())).then(|| group.to_owned())
}

pub(super) fn clean_title(raw: &str) -> String {
    let spaced = if raw.trim().contains(' ') {
        raw.replace('_', " ")
    } else {
        raw.replace(['.', '_'], " ")
    };
    spaced
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| c == '-' || c == '(' || c == '[' || c.is_whitespace())
        .to_owned()
}
