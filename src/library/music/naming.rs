//! What names say when tags don't: an album folder's artist, title and year, and a file's title and
//! number.

use super::*;

#[derive(Debug, Default, PartialEq)]
pub(super) struct FolderName {
    pub(super) artist: Option<String>,
    pub(super) title: String,
    pub(super) year: Option<i64>,
}

/// What an album folder's name says: "Artist - Album (2001) [FLAC]", "Album (2001)", "Album".
pub(super) fn album_from_folder(name: &str) -> FolderName {
    // Bracketed groups are source and format notes, or the year.
    let mut year = None;
    let mut text = String::new();
    let mut depth = 0;
    let mut group = String::new();
    for c in name.chars() {
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                group.clear();
            }
            ')' | ']' | '}' if depth > 0 => {
                depth -= 1;
                if group.len() == 4 && year.is_none() {
                    year = tags::year_of(&group).filter(|y| (1900..=2100).contains(y));
                }
            }
            _ if depth > 0 => group.push(c),
            _ => text.push(c),
        }
    }
    let text = text.trim().trim_end_matches(['-', ' ']).trim();
    let (artist, title) = match text.split_once(" - ") {
        Some((artist, title)) => (Some(artist.trim().to_owned()), title.trim()),
        None => (None, text),
    };
    FolderName {
        artist,
        title: title.to_owned(),
        year,
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct NamedTrack {
    pub(super) title: String,
    pub(super) track_number: Option<i64>,
}

/// An untagged file's title and number from its name: "01 - Song", "1-03 Song", "07. Song".
pub(super) fn from_name(rel_path: &str) -> NamedTrack {
    let stem = Path::new(rel_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(rel_path);
    let digits: String = stem.chars().take_while(|c| c.is_ascii_digit()).collect();
    let mut rest = &stem[digits.len()..];
    let mut number = digits.parse::<i64>().ok().filter(|_| digits.len() <= 3);
    // "1-03": a disc and a track number.
    if number.is_some() && rest.starts_with('-') {
        let more: String = rest[1..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !more.is_empty() && more.len() <= 3 {
            number = more.parse().ok();
            rest = &rest[1 + more.len()..];
        }
    }
    if number.is_none() {
        rest = stem;
    }
    let title = rest.trim_start_matches([' ', '.', '-', '_', ')']).trim();
    NamedTrack {
        title: if title.is_empty() {
            stem.to_owned()
        } else {
            title.to_owned()
        },
        track_number: number.filter(|_| !title.is_empty()),
    }
}
