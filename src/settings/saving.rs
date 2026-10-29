//! Saving a service's settings, refused unless they hold together, and putting them back after a
//! change that couldn't be set up.

use super::*;

/// Puts `names` back as they were in `before`, after a change that couldn't be set up.
pub async fn restore<'a>(
    pool: &SqlitePool,
    names: impl Iterator<Item = &'a String>,
    before: &HashMap<String, String>,
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    for name in names {
        match before.get(name) {
            Some(value) => {
                sqlx::query(
                    "INSERT INTO settings (name, value, updated_at) VALUES (?, ?, ?)
                     ON CONFLICT (name) DO UPDATE SET value = excluded.value",
                )
                .bind(name)
                .bind(value)
                .bind(unix_now())
                .execute(&mut *tx)
                .await?;
            }
            None => {
                sqlx::query("DELETE FROM settings WHERE name = ?")
                    .bind(name)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    tx.commit().await
}

/// A change refused, and why.
#[derive(Debug, PartialEq)]
pub enum Refused {
    NoService,
    Invalid(String),
}

/// Why a value can't be what a setting entered this way takes; None when it can.
fn refused_value(name: &str, input: Input, value: &str) -> Option<String> {
    match input {
        Input::Toggle { on } if value != on && value != "off" => {
            return Some(format!("{name} is {on} or off"));
        }
        Input::Choice { options } if !options.iter().any(|o| o.value == value) => {
            let all: Vec<&str> = options.iter().map(|o| o.value).collect();
            return Some(format!("{name} is one of {}", all.join(", ")));
        }
        _ => {}
    }
    (value.chars().count() > 2000 || value.contains(['\n', '\r', '\0']))
        .then(|| format!("{name} must be one line of at most 2000 characters"))
}

/// The configuration a service's settings would make with `changes` (each named one set to its
/// value, or cleared when it's None or empty; ones left out as they are), and the settings
/// stored then. Refused unless every name is the service's, none is set in the environment, and
/// the configuration holds together.
pub async fn candidate(
    pool: &SqlitePool,
    service: &str,
    changes: &HashMap<String, Option<String>>,
    env: &(dyn Fn(&str) -> Option<String> + Sync),
) -> anyhow::Result<Result<(Config, HashMap<String, String>), Refused>> {
    let Some(service) = SERVICES.iter().find(|s| s.key == service) else {
        return Ok(Err(Refused::NoService));
    };
    let mut next = stored(pool).await?;
    for (name, value) in changes {
        let Some(field) = service.fields.iter().find(|f| f.name == name.as_str()) else {
            return Ok(Err(Refused::Invalid(format!(
                "{name} isn't one of {}'s settings",
                service.name
            ))));
        };
        if env(name).is_some_and(|v| !v.trim().is_empty()) {
            return Ok(Err(Refused::Invalid(format!(
                "{name} is set in .env, which wins; change or remove it there"
            ))));
        }
        let value = value.as_deref().map(str::trim).filter(|v| !v.is_empty());
        if let Some(why) = value.and_then(|v| refused_value(name, field.input, v)) {
            return Ok(Err(Refused::Invalid(why)));
        }
        match value {
            Some(v) => next.insert(name.clone(), v.to_owned()),
            None => next.remove(name),
        };
    }
    // The whole configuration has to hold together before anything is kept or tried.
    match Config::from_env_and(lookup(&next, env)) {
        Ok(config) => Ok(Ok((config, next))),
        Err(err) => Ok(Err(Refused::Invalid(format!("{err:#}")))),
    }
}

/// Saves a service's settings, as [`candidate`] takes them. Returns the configuration to run
/// with.
pub async fn save(
    pool: &SqlitePool,
    service: &str,
    changes: &HashMap<String, Option<String>>,
    env: &(dyn Fn(&str) -> Option<String> + Sync),
) -> anyhow::Result<Result<Config, Refused>> {
    let (config, next) = match candidate(pool, service, changes, env).await? {
        Ok(made) => made,
        Err(refused) => return Ok(Err(refused)),
    };
    let mut tx = pool.begin().await?;
    for name in changes.keys() {
        match next.get(name) {
            Some(value) => {
                sqlx::query(
                    "INSERT INTO settings (name, value, updated_at) VALUES (?, ?, ?)
                     ON CONFLICT (name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                )
                .bind(name)
                .bind(value)
                .bind(unix_now())
                .execute(&mut *tx)
                .await?;
            }
            None => {
                sqlx::query("DELETE FROM settings WHERE name = ?")
                    .bind(name)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(Ok(config))
}
