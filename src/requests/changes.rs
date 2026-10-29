//! Changing a request after it's made: declining, reopening, changing the season, withdrawing.

use super::*;

/// Declines a request waiting for a decision, saying why (1 to 500 characters), and returns it
/// as it is now. Nothing is added to the wanted list.
///
/// # Errors
///
/// The inner [`Refused`]: `Invalid` for a missing or long reason, `Decided` when it isn't
/// waiting any more, `NotFound` when there's no such request.
pub async fn decline(
    pool: &SqlitePool,
    id: i64,
    admin: &User,
    reason: &str,
) -> anyhow::Result<Result<RequestView, Refused>> {
    let reason = reason.trim();
    if reason.is_empty() || reason.chars().count() > 500 {
        return Ok(Err(Refused::Invalid(
            "say why, in 1 to 500 characters".into(),
        )));
    }
    let changed = sqlx::query(
        "UPDATE requests SET state = 'declined', reason = ?, decided_at = ?, decided_by = ?
         WHERE id = ? AND state = 'requested'",
    )
    .bind(reason)
    .bind(unix_now())
    .bind(admin.id)
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    if changed == 0 {
        return Ok(Err(match get(pool, id).await? {
            Some(_) => Refused::Decided,
            None => Refused::NotFound,
        }));
    }
    Ok(Ok(get(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("request {id} vanished"))?))
}

/// Undoes a decline: the request waits for a decision again, its reason gone. Refused
/// when its requester has since asked for the same thing again, which is the one to decide.
pub async fn reopen(pool: &SqlitePool, id: i64) -> anyhow::Result<Result<RequestView, Refused>> {
    let Some(found) = get(pool, id).await? else {
        return Ok(Err(Refused::NotFound));
    };
    if found.state != "declined" {
        return Ok(Err(Refused::Conflict(
            "only a declined request can be reopened".into(),
        )));
    }
    let reopened = sqlx::query(
        "UPDATE requests SET state = 'requested', reason = NULL, decided_at = NULL, decided_by = NULL
         WHERE id = ? AND state = 'declined'",
    )
    .bind(id)
    .execute(pool)
    .await;
    match reopened {
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Ok(Err(Refused::Conflict(format!(
                "{} has asked for it again since; decide on that request instead",
                found.username
            ))));
        }
        other => {
            other?;
        }
    }
    Ok(Ok(get(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("request {id} vanished"))?))
}

/// Changes which season a series request still waiting for a decision asks for: one,
/// or none for all of it. Its requester's or an admin's to do.
pub async fn change_season(
    pool: &SqlitePool,
    id: i64,
    user: &User,
    season: Option<i64>,
) -> anyhow::Result<Result<RequestView, Refused>> {
    let Some(found) = get(pool, id).await? else {
        return Ok(Err(Refused::NotFound));
    };
    if found.user_id != user.id && !user.is_admin {
        return Ok(Err(Refused::NotYours));
    }
    if found.kind != "series" {
        return Ok(Err(Refused::Invalid(
            "a season is only for a series".into(),
        )));
    }
    if season.is_some_and(|s| !(0..=10_000).contains(&s)) {
        return Ok(Err(Refused::Invalid("season is out of range".into())));
    }
    let changed =
        sqlx::query("UPDATE requests SET season = ? WHERE id = ? AND state = 'requested'")
            .bind(season)
            .bind(id)
            .execute(pool)
            .await;
    match changed {
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            let other: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM requests WHERE user_id = ? AND kind = 'series' AND tmdb_id = ?
                   AND coalesce(season, -1) = coalesce(?, -1) AND state != 'declined'",
            )
            .bind(found.user_id)
            .bind(found.tmdb_id)
            .bind(season)
            .fetch_optional(pool)
            .await?;
            return Ok(Err(Refused::Already(other.unwrap_or_default())));
        }
        Ok(done) if done.rows_affected() == 0 => return Ok(Err(Refused::Decided)),
        other => {
            other?;
        }
    }
    Ok(Ok(get(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("request {id} vanished"))?))
}

/// Takes back a request still waiting for a decision; its requester's or an admin's to do.
pub async fn withdraw(
    pool: &SqlitePool,
    id: i64,
    user: &User,
) -> anyhow::Result<Result<(), Refused>> {
    let found: Option<(i64, String)> =
        sqlx::query_as("SELECT user_id, state FROM requests WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    match found {
        None => Ok(Err(Refused::NotFound)),
        Some((owner, _)) if owner != user.id && !user.is_admin => Ok(Err(Refused::NotYours)),
        Some((_, state)) if state != "requested" => Ok(Err(Refused::Decided)),
        Some(_) => {
            sqlx::query("DELETE FROM requests WHERE id = ? AND state = 'requested'")
                .bind(id)
                .execute(pool)
                .await?;
            Ok(Ok(()))
        }
    }
}
