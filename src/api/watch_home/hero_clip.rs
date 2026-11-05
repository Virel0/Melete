//! The clip of the title itself each hero pick can play instead of a trailer: which file, where
//! to start and for how long, worked out as the web UI has always done it, so every app plays
//! the same clip and chooses per screen between it and the pick's `trailer`.
//!
//! The file is what Play would start: the episode the pick names, else the movie's file the
//! viewer has progress in, else the movie's first file or the series' first episode on disk
//! (specials last). It starts where the viewer left that file, else an eighth of the way in
//! (past most films' titles and most episodes' recaps).

use super::*;

/// How long a clip plays.
const CLIP_SECONDS: f64 = 40.0;
/// The length taken for a file whose length isn't known yet.
const UNKNOWN_LENGTH: f64 = 600.0;

/// A hero pick's clip: `{file_id, start_seconds, duration_seconds}`; null when the title has
/// no file to play.
pub(super) async fn hero_clip(
    pool: &SqlitePool,
    title: TitleRef,
    pick: &Value,
) -> Result<Value, ApiError> {
    let progress = &pick["progress"];
    let named = if title.series {
        pick["episode"]["file_id"].as_i64()
    } else {
        progress["file_id"].as_i64()
    };
    let file = match named {
        Some(id) => Some(id),
        None => first_file(pool, title).await?,
    };
    let Some(file) = file else {
        return Ok(Value::Null);
    };
    let partway = progress["watched"] != json!(true)
        && progress["position_seconds"]
            .as_f64()
            .is_some_and(|p| p > 0.0)
        && progress["file_id"].as_i64() == Some(file);
    let start = if partway {
        progress["position_seconds"].as_f64().unwrap_or(0.0)
    } else {
        let runtime = pick["runtime_minutes"]
            .as_f64()
            .filter(|_| !title.series)
            .map(|m| m * 60.0);
        let length = match runtime {
            Some(length) => Some(length),
            None => probed_length(pool, file).await?,
        };
        length.unwrap_or(UNKNOWN_LENGTH) / 8.0
    };
    Ok(json!({
        "file_id": file,
        "start_seconds": (start * 10.0).round() / 10.0,
        "duration_seconds": CLIP_SECONDS,
    }))
}

/// The movie's first file, or the series' first episode on disk, specials last.
async fn first_file(pool: &SqlitePool, title: TitleRef) -> sqlx::Result<Option<i64>> {
    let query = if title.series {
        "SELECT id FROM media_files
         WHERE series_id = ? AND missing_since IS NULL AND season IS NOT NULL
         ORDER BY season = 0, season, episode, rel_path LIMIT 1"
    } else {
        "SELECT id FROM media_files WHERE movie_id = ? AND missing_since IS NULL
         ORDER BY rel_path LIMIT 1"
    };
    sqlx::query_scalar(query)
        .bind(title.id)
        .fetch_optional(pool)
        .await
}

/// A file's length, as its probe found it.
async fn probed_length(pool: &SqlitePool, file: i64) -> sqlx::Result<Option<f64>> {
    let length: Option<Option<f64>> = sqlx::query_scalar(
        "SELECT CAST(json_extract(info, '$.duration_seconds') AS REAL) FROM media_probes
         WHERE file_id = ?",
    )
    .bind(file)
    .fetch_optional(pool)
    .await?;
    Ok(length.flatten())
}
