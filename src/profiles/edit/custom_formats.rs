//! Making, changing and deleting custom formats. A format is checked before it's kept: its name,
//! its source, and its patterns, compiled as an import compiles them.

use super::*;

pub(super) fn check_format(input: &FormatInput) -> Result<(App, String), Refused> {
    let name = check_name(&input.name)?;
    let app = app(&input.source)?;
    if input.conditions.is_empty() || input.conditions.len() > 50 {
        return Err(invalid("a format has 1 to 50 conditions"));
    }
    // Compiling checks every pattern, as an import does.
    Compiled::new(app, name.clone(), input.conditions.clone())
        .map_err(|err| invalid(format!("{err:#}")))?;
    Ok((app, name))
}

/// Makes a custom format as Melete's own: its id, or why it was refused.
pub async fn create_format(
    pool: &SqlitePool,
    input: &FormatInput,
) -> anyhow::Result<Result<i64, Refused>> {
    let (app, name) = match check_format(input) {
        Ok(checked) => checked,
        Err(refused) => return Ok(Err(refused)),
    };
    let created = sqlx::query_scalar(
        "INSERT INTO custom_formats (source, name, specifications, updated_at, origin)
         VALUES (?, ?, ?, ?, 'melete') RETURNING id",
    )
    .bind(app.as_str())
    .bind(&name)
    .bind(serde_json::to_string(&input.conditions)?)
    .bind(unix_now())
    .fetch_one(pool)
    .await;
    match created {
        Ok(id) => Ok(Ok(id)),
        Err(err) if unique_violation(&err) => Ok(Err(Refused::Taken)),
        Err(err) => Err(err.into()),
    }
}

/// Changes a custom format, which becomes Melete's own, so re-imports leave it alone. Its
/// source can't change.
pub async fn update_format(
    pool: &SqlitePool,
    id: i64,
    input: &FormatInput,
) -> anyhow::Result<Result<(), Refused>> {
    let (app, name) = match check_format(input) {
        Ok(checked) => checked,
        Err(refused) => return Ok(Err(refused)),
    };
    let source: Option<String> =
        sqlx::query_scalar("SELECT source FROM custom_formats WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    match source {
        None => return Ok(Err(Refused::NotFound)),
        // Profiles of the other kind score it; it can't change sides.
        Some(source) if source != app.as_str() => {
            return Ok(Err(invalid(
                "a format's source can't change; make a new one",
            )));
        }
        Some(_) => {}
    }
    let updated = sqlx::query(
        "UPDATE custom_formats SET name = ?, specifications = ?, updated_at = ?, origin = 'melete' WHERE id = ?",
    )
    .bind(&name)
    .bind(serde_json::to_string(&input.conditions)?)
    .bind(unix_now())
    .bind(id)
    .execute(pool)
    .await;
    match updated {
        Ok(_) => Ok(Ok(())),
        Err(err) if unique_violation(&err) => Ok(Err(Refused::Taken)),
        Err(err) => Err(err.into()),
    }
}

/// Deletes a format; profiles stop scoring it.
pub async fn delete_format(pool: &SqlitePool, id: i64) -> anyhow::Result<bool> {
    let deleted = sqlx::query("DELETE FROM custom_formats WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(deleted.rows_affected() > 0)
}
