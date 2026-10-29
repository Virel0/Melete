//! The video limit of the way a request came in by: what it means for stream links and
//! conversions.

use super::*;
use crate::ways_in::VideoLimit;

/// The most bits per second a video may take through the way a request came in by; None when
/// that way sets no limit (or no ways are set).
pub(in super::super) fn way_limit(limit: Option<Extension<VideoLimit>>) -> Option<i64> {
    limit.and_then(|Extension(VideoLimit(max))| max)
}

/// A file's overall bits per second: what its container says, else its size over its length.
pub(in super::super) fn overall_bitrate(
    size: i64,
    media: Option<&probe::MediaInfo>,
) -> Option<i64> {
    let media = media?;
    media.bit_rate.filter(|b| *b > 0).or_else(|| {
        let seconds = media
            .duration_seconds
            .filter(|d| d.is_finite() && *d > 0.0)?;
        Some((size as f64 * 8.0 / seconds).round() as i64)
    })
}

/// A file's (or an extra's) overall bits per second from its stored probe; None when it hasn't
/// been read, or its bitrate can't be told.
pub(in super::super) async fn stored_bitrate(
    pool: &SqlitePool,
    id: i64,
) -> sqlx::Result<Option<i64>> {
    let query = if extras::is_extra_id(id) {
        "SELECT size, probe_info FROM media_extras WHERE id = ?"
    } else {
        "SELECT f.size, p.info FROM media_files f
         LEFT JOIN media_probes p ON p.file_id = f.id
             AND p.size = f.size AND p.modified_at = f.modified_at
         WHERE f.id = ?"
    };
    let row: Option<(i64, Option<String>)> =
        sqlx::query_as(query).bind(id).fetch_optional(pool).await?;
    Ok(row.and_then(|(size, info)| {
        let media: probe::MediaInfo = serde_json::from_str(&info?).ok()?;
        overall_bitrate(size, Some(&media))
    }))
}

/// Bits per second in words: "8 Mbit/s", "2.5 Mbit/s", "800 kbit/s".
pub(in super::super) fn rate_words(bits: i64) -> String {
    if bits >= 1_000_000 {
        let mbits = format!("{:.1}", bits as f64 / 1e6);
        format!("{} Mbit/s", mbits.trim_end_matches(".0"))
    } else {
        format!("{} kbit/s", (bits as f64 / 1e3).round())
    }
}

/// Refuses a file whose overall bitrate is over `limit`, or unknown, saying what to do instead:
/// it would take more than the way allows as it is.
pub(in super::super) fn within_limit(limit: i64, bitrate: Option<i64>) -> Result<(), ApiError> {
    let refused = |why: String| ApiError::new(StatusCode::FORBIDDEN, why);
    match bitrate {
        Some(bits) if bits <= limit => Ok(()),
        Some(bits) => Err(refused(format!(
            "this file is {}, over the {} limit of the way you came in; ask for a conversion",
            rate_words(bits),
            rate_words(limit)
        ))),
        None => Err(refused(format!(
            "this file's bitrate is unknown, so it can't be held to the {} limit of the way you came in; ask for a conversion",
            rate_words(limit)
        ))),
    }
}

#[cfg(test)]
mod tests;
