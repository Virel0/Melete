//! Reading a playlist export: RFC 4180 rows, with columns found by the names exporters give them.

use super::*;

pub(super) const TITLE: &[&str] = &["track name", "title", "track", "name", "song"];
pub(super) const ARTIST: &[&str] = &[
    "artist name(s)",
    "artist name",
    "artist",
    "artists",
    "artist(s)",
];
pub(super) const ALBUM: &[&str] = &["album name", "album"];
pub(super) const LENGTH: &[&str] = &[
    "track duration (ms)",
    "duration (ms)",
    "duration_ms",
    "duration",
];
pub(super) const ISRC: &[&str] = &["isrc"];

/// RFC 4180's rows: quoted fields, doubled quotes, line breaks inside quotes.
pub(super) fn rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let (mut row, mut field) = (Vec::new(), String::new());
    let (mut quoted, mut was_quoted) = (false, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                c => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() && !was_quoted => {
                quoted = true;
                was_quoted = true;
            }
            ',' => {
                row.push(std::mem::take(&mut field));
                was_quoted = false;
            }
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                was_quoted = false;
            }
            c => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// The playlist's tracks, or why the file isn't one.
pub fn parse(text: &str) -> Result<Vec<Track>, String> {
    // A byte-order mark would stick to the first column's name.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows = rows(text)
        .into_iter()
        .filter(|row| row.iter().any(|cell| !cell.trim().is_empty()));
    let Some(header) = rows.next() else {
        return Err("the file is empty".into());
    };
    let columns: Vec<String> = header.iter().map(|c| c.trim().to_lowercase()).collect();
    let find = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| columns.iter().position(|c| c == name))
    };
    let Some(title) = find(TITLE) else {
        return Err(
            "the file has no column of track names (like Exportify's \"Track Name\")".into(),
        );
    };
    let (artist, album, length, isrc) = (find(ARTIST), find(ALBUM), find(LENGTH), find(ISRC));
    let mut tracks = Vec::new();
    for row in rows {
        let cell = |column: Option<usize>| {
            column
                .and_then(|c| row.get(c))
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let Some(name) = cell(Some(title)) else {
            continue;
        };
        if tracks.len() == MAX_TRACKS {
            return Err(format!("the playlist has more than {MAX_TRACKS} tracks"));
        }
        tracks.push(Track {
            title: name,
            // Exportify joins several artists with commas.
            artists: cell(artist)
                .map(|a| {
                    a.split(',')
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            album: cell(album),
            length_ms: cell(length).and_then(|l| length_ms(&l)),
            isrc: cell(isrc).map(|i| i.to_uppercase()),
        });
    }
    if tracks.is_empty() {
        return Err("the file lists no tracks".into());
    }
    Ok(tracks)
}

/// Milliseconds as Exportify writes them, or m:ss as a hand-made sheet might.
pub(super) fn length_ms(text: &str) -> Option<i64> {
    if let Ok(ms) = text.parse::<i64>() {
        return (ms > 0).then_some(ms);
    }
    let (m, s) = text.split_once(':')?;
    let (m, s) = (m.parse::<i64>().ok()?, s.parse::<i64>().ok()?);
    (s < 60 && (m, s) != (0, 0)).then_some((m * 60 + s) * 1000)
}
