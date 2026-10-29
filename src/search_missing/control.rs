//! Starting a run, one per area at a time, and asking one to stop.

use super::*;

/// A run under way in the area already: its id.
#[derive(Debug, PartialEq)]
pub struct Already(pub i64);

pub(super) async fn running_in(pool: &SqlitePool, area: Area) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar(
        "SELECT id FROM missing_searches WHERE area = ? AND state IN ('running', 'stopping')",
    )
    .bind(area.as_str())
    .fetch_optional(pool)
    .await
}

/// Starts a run for what's missing in the area in the background; returns it as it starts.
#[cfg(test)]
pub async fn start(
    pool: &SqlitePool,
    sources: Sources,
    area: Area,
    started_by: i64,
) -> anyhow::Result<Result<MissingSearch, Already>> {
    start_for(pool, sources, area, Goal::Missing, started_by).await
}

/// Starts a run for the goal in the area in the background; returns it as it starts. A run
/// for either goal is the area's one run at a time.
pub async fn start_for(
    pool: &SqlitePool,
    sources: Sources,
    area: Area,
    goal: Goal,
    started_by: i64,
) -> anyhow::Result<Result<MissingSearch, Already>> {
    if let Some(id) = running_in(pool, area).await? {
        return Ok(Err(Already(id)));
    }
    let inserted: sqlx::Result<i64> = sqlx::query_scalar(
        "INSERT INTO missing_searches (area, goal, state, started_by, started_at)
         VALUES (?, ?, 'running', ?, ?) RETURNING id",
    )
    .bind(area.as_str())
    .bind(goal.as_str())
    .bind(started_by)
    .bind(unix_now())
    .fetch_one(pool)
    .await;
    let id = match inserted {
        Ok(id) => id,
        // Another start won the race: the index allows one at a time.
        Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
            let id = running_in(pool, area).await?.unwrap_or_default();
            return Ok(Err(Already(id)));
        }
        Err(err) => return Err(err.into()),
    };
    let started = get(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("missing search {id} vanished"))?;
    let pool = pool.clone();
    tokio::spawn(async move {
        let outcome = run(&pool, &sources, id, area, goal).await;
        let (state, message) = match outcome {
            Ok(Ended::Done(message)) => ("done", message),
            Ok(Ended::Stopped(message)) => ("stopped", message),
            Err(err) => ("failed", format!("{err:#}")),
        };
        if let Err(err) = sqlx::query(
            "UPDATE missing_searches SET state = ?, message = ?, current = NULL, finished_at = ?
             WHERE id = ?",
        )
        .bind(state)
        .bind(message)
        .bind(unix_now())
        .bind(id)
        .execute(&pool)
        .await
        {
            tracing::warn!(error = %err, "couldn't record how a search for missing titles ended");
        }
    });
    Ok(Ok(started))
}

/// Why a run wasn't asked to stop.
#[derive(Debug, PartialEq)]
pub enum NotStopped {
    NotFound,
    /// It has finished already.
    Finished,
}

/// Asks a run to stop: it finishes the search under way, then ends.
pub async fn stop(pool: &SqlitePool, id: i64) -> anyhow::Result<Result<MissingSearch, NotStopped>> {
    sqlx::query(
        "UPDATE missing_searches SET state = 'stopping' WHERE id = ? AND state = 'running'",
    )
    .bind(id)
    .execute(pool)
    .await?;
    match get(pool, id).await? {
        None => Ok(Err(NotStopped::NotFound)),
        Some(run) if run.state == "stopping" => Ok(Ok(run)),
        Some(_) => Ok(Err(NotStopped::Finished)),
    }
}

pub(super) async fn stopping(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT state = 'stopping' FROM missing_searches WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map(|s| s.unwrap_or(true))
}
