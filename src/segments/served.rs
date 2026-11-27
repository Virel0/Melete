//! What was found in a file, as it's served: its intro and its credits, and how each was found.

use sqlx::SqlitePool;

use super::Segment;

/// A file's intro and credits, when its current version has been looked at: `None` when it
/// hasn't been, by matching episodes or alone. Credits found by matching come first; those
/// found in the file alone count where matching found none. The API asks [`sourced`].
#[cfg(test)]
pub async fn of_file(
    pool: &SqlitePool,
    file_id: i64,
) -> anyhow::Result<Option<(Option<Segment>, Option<Segment>)>> {
    let found = sourced(pool, file_id).await?;
    Ok(found.map(|(intro, credits)| (intro, credits.map(|(c, _)| c))))
}

/// How a segment was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// By matching audio an episode shares with others of its season.
    Audio,
    /// By the black or silent ending of the file alone.
    Ending,
}

/// As [`of_file`], with how the credits were found (an intro is only found by matching).
pub async fn sourced(
    pool: &SqlitePool,
    file_id: i64,
) -> anyhow::Result<Option<(Option<Segment>, Option<(Segment, Found)>)>> {
    let scanned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM media_segment_scans s JOIN media_files f ON f.id = s.file_id
             WHERE s.file_id = ? AND s.size = f.size AND s.modified_at = f.modified_at)",
    )
    .bind(file_id)
    .fetch_one(pool)
    .await?;
    let alone: Option<(Option<f64>, Option<f64>)> = sqlx::query_as(
        "SELECT c.start_seconds, c.end_seconds FROM media_credit_scans c
         JOIN media_files f ON f.id = c.file_id
         WHERE c.file_id = ? AND c.size = f.size AND c.modified_at = f.modified_at",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await?;
    let alone_credits = match alone {
        Some((Some(start), Some(end))) => Some((Segment { start, end }, Found::Ending)),
        _ => None,
    };
    if !scanned {
        return Ok(alone.map(|_| (None, alone_credits)));
    }
    let rows: Vec<(String, f64, f64)> = sqlx::query_as(
        "SELECT kind, start_seconds, end_seconds FROM media_segments WHERE file_id = ?",
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?;
    let find = |kind: &str| {
        rows.iter()
            .find(|(k, _, _)| k == kind)
            .map(|(_, start, end)| Segment {
                start: *start,
                end: *end,
            })
    };
    let matched = find("credits").map(|c| (c, Found::Audio));
    Ok(Some((find("intro"), matched.or(alone_credits))))
}
