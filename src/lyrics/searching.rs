//! Full-text search over the library's lyrics, best match first.

use super::*;

/// A line that matched a lyrics search, and the track it's in.
#[derive(Debug, PartialEq)]
pub struct Hit {
    /// The track the line is in.
    pub track_id: i64,
    /// When the line starts, in seconds; none for plain lyrics.
    pub start: Option<f64>,
    /// The line's words.
    pub text: String,
}

/// Words typed as a full-text query: the words as one phrase, the last one a prefix, so
/// "next world w" finds "in the next world war". None when there are no words.
pub fn phrase(words: &str) -> Option<String> {
    let tokens: Vec<String> = words
        .split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '\u{2019}')
        .filter(|w| !w.is_empty())
        .map(|w| w.replace('"', ""))
        .collect();
    (!tokens.is_empty()).then(|| format!("\"{}\" *", tokens.join(" ")))
}

/// Tracks in the library whose lyrics have a line matching `words`, best match first: each
/// track once, with its best-matching line (the earliest of equals). Also how many tracks
/// matched in all.
pub async fn search(
    pool: &SqlitePool,
    words: &str,
    limit: usize,
    offset: usize,
) -> sqlx::Result<(usize, Vec<Hit>)> {
    let Some(query) = phrase(words) else {
        return Ok((0, Vec::new()));
    };
    let rows: Vec<(i64, Option<f64>, String)> = sqlx::query_as(
        "SELECT l.track_id, l.start_seconds, l.text
         FROM music_lyric_search s
         JOIN music_lyric_lines l ON l.id = s.rowid
         JOIN music_tracks t ON t.id = l.track_id AND t.missing_since IS NULL
         WHERE music_lyric_search MATCH ?
         ORDER BY bm25(music_lyric_search), l.track_id, l.position",
    )
    .bind(query)
    .fetch_all(pool)
    .await?;
    let mut seen = std::collections::HashSet::new();
    let hits: Vec<Hit> = rows
        .into_iter()
        .filter(|(track_id, _, _)| seen.insert(*track_id))
        .map(|(track_id, start, text)| Hit {
            track_id,
            start,
            text,
        })
        .collect();
    let total = hits.len();
    Ok((total, hits.into_iter().skip(offset).take(limit).collect()))
}
