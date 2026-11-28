//! What a watch party plays next: as the host's player counts down to the next episode, it
//! says which file and when it starts, so everyone's player shows the same countdown (read
//! only, for all but the host, who alone moves the party on). Cleared when the party moves on.

use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::{Refused, host_for};
use crate::db::now_ms;

/// What plays next, as the host's player sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpNext {
    /// The next film's or episode's file.
    pub file_id: i64,
    /// When it starts, in Unix milliseconds on the server's clock.
    pub starts_at: i64,
}

/// Sets or clears (`None`) what party `id` plays next, for its host's device.
pub async fn set(
    pool: &SqlitePool,
    (user_id, token_id): (i64, i64),
    id: i64,
    next: Option<UpNext>,
) -> anyhow::Result<Result<(), Refused>> {
    match host_for(pool, token_id, id).await? {
        None => return Ok(Err(Refused::NotFound)),
        Some(host) if host != user_id => {
            return Ok(Err(Refused::NotHost(
                "only whoever started the party says what it plays next",
            )));
        }
        Some(_) => {}
    }
    if let Some(next) = next {
        let present: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM media_files WHERE id = ? AND missing_since IS NULL)",
        )
        .bind(next.file_id)
        .fetch_one(pool)
        .await?;
        if !present {
            return Ok(Err(Refused::Invalid(
                "file_id is a film's or episode's file in the library".into(),
            )));
        }
    }
    sqlx::query(
        "UPDATE watch_parties SET up_next_file_id = ?, up_next_at = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(next.map(|n| n.file_id))
    .bind(next.map(|n| n.starts_at))
    .bind(now_ms())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(Ok(()))
}

/// Clears what plays next: the party moved on to another file.
pub async fn moved_on(pool: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE watch_parties SET up_next_file_id = NULL, up_next_at = NULL WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct Next {
    file_id: Option<i64>,
    starts_at: Option<i64>,
    movie: Option<String>,
    series: Option<String>,
    season: Option<i64>,
    episode: Option<i64>,
}

/// What party `id` plays next, as everyone in it is shown it: `{file_id, title, starts_at}`,
/// or null when nothing is set (or its file has gone).
pub async fn view(pool: &SqlitePool, id: i64) -> sqlx::Result<Value> {
    let next: Option<Next> = sqlx::query_as(
        "SELECT p.up_next_file_id AS file_id, p.up_next_at AS starts_at,
             m.parsed_title AS movie, s.parsed_title AS series, f.season, f.episode
         FROM watch_parties p JOIN media_files f ON f.id = p.up_next_file_id
         LEFT JOIN movies m ON m.id = f.movie_id LEFT JOIN series s ON s.id = f.series_id
         WHERE p.id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some(Next {
        file_id: Some(file_id),
        starts_at: Some(starts_at),
        movie,
        series,
        season,
        episode,
    }) = next
    else {
        return Ok(Value::Null);
    };
    Ok(json!({
        "file_id": file_id,
        "title": title(movie, series, season, episode),
        "starts_at": starts_at,
    }))
}

/// A file's title as a party shows it: the film's name, or the series' with its season and
/// episode.
pub fn title(
    movie: Option<String>,
    series: Option<String>,
    season: Option<i64>,
    episode: Option<i64>,
) -> String {
    match (movie, series, season, episode) {
        (Some(movie), ..) => movie,
        (None, Some(series), Some(season), Some(episode)) => {
            format!("{series}, season {season} episode {episode}")
        }
        (None, Some(series), ..) => series,
        _ => "What's playing".to_owned(),
    }
}

#[cfg(test)]
mod tests;
