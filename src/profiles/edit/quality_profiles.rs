//! Making, changing and deleting quality profiles. A profile is checked before it's kept, and its
//! items turned into ranked qualities.

use super::*;

/// Checks a profile and turns its items into ranked qualities.
pub(super) async fn check_profile(
    pool: &SqlitePool,
    input: &ProfileInput,
) -> anyhow::Result<Result<Checked, Refused>> {
    let name = match check_name(&input.name) {
        Ok(name) => name,
        Err(refused) => return Ok(Err(refused)),
    };
    let app = match app(&input.source) {
        Ok(app) => app,
        Err(refused) => return Ok(Err(refused)),
    };
    let qualities = match ranked_qualities(input)? {
        Ok(qualities) => qualities,
        Err(refused) => return Ok(Err(refused)),
    };
    let cutoff = match cutoff(input)? {
        Ok(cutoff) => cutoff,
        Err(refused) => return Ok(Err(refused)),
    };
    if let Err(refused) = check_scores(input)? {
        return Ok(Err(refused));
    }
    if let Err(refused) = check_formats(pool, input, app).await? {
        return Ok(Err(refused));
    }
    Ok(Ok(Checked {
        app,
        name,
        cutoff: cutoff.name.trim().to_owned(),
        qualities,
    }))
}

/// A profile refused as invalid, for why.
fn refuse<T>(why: String) -> anyhow::Result<Result<T, Refused>> {
    Ok(Err(invalid(why)))
}

/// The profile's items as ranked qualities: each item a rank, each quality known and in one item.
fn ranked_qualities(input: &ProfileInput) -> anyhow::Result<Result<Vec<RankedQuality>, Refused>> {
    if input.items.is_empty() || input.items.len() > 64 {
        return refuse("a profile has 1 to 64 items".into());
    }
    let mut qualities = Vec::new();
    let mut seen_items = HashSet::new();
    let mut seen_qualities = HashSet::new();
    for (rank, item) in input.items.iter().enumerate() {
        let item_name = item.name.trim();
        if item_name.is_empty() || !seen_items.insert(item_name.to_owned()) {
            return refuse(format!(
                "item names must be given and different: {item_name:?}"
            ));
        }
        if item.qualities.is_empty() {
            return refuse(format!("{item_name} has no qualities"));
        }
        let group = (item.qualities.len() > 1 || item.qualities[0] != item_name)
            .then(|| item_name.to_owned());
        for name in &item.qualities {
            let Some(known) = quality::find(name) else {
                return refuse(format!("{name} isn't a quality Melete knows"));
            };
            if !seen_qualities.insert(known.name) {
                return refuse(format!("{name} is in more than one item"));
            }
            qualities.push(RankedQuality {
                quality: known.name.to_owned(),
                rank: rank as i64,
                group: group.clone(),
                allowed: item.allowed,
            });
        }
    }
    Ok(Ok(qualities))
}

/// The item the profile's cutoff names, which must be allowed.
fn cutoff(input: &ProfileInput) -> anyhow::Result<Result<&ItemInput, Refused>> {
    let Some(cutoff) = input
        .items
        .iter()
        .find(|i| i.name.trim() == input.cutoff.trim())
    else {
        return refuse(format!(
            "the cutoff {:?} isn't one of the items",
            input.cutoff
        ));
    };
    if !cutoff.allowed {
        return refuse(format!("the cutoff {} isn't allowed", cutoff.name.trim()));
    }
    Ok(Ok(cutoff))
}

/// Checks the profile's scores and language.
fn check_scores(input: &ProfileInput) -> anyhow::Result<Result<(), Refused>> {
    if input.min_upgrade_format_score < 1 {
        return refuse("min_upgrade_format_score is at least 1".into());
    }
    let scores = [
        input.min_format_score,
        input.cutoff_format_score,
        input.min_upgrade_format_score,
    ];
    if scores
        .iter()
        .chain(input.formats.iter().map(|f| &f.score))
        .any(|s| s.abs() > 1_000_000)
    {
        return refuse("scores are within ±1,000,000".into());
    }
    if let Some(language) = &input.language
        && (language.is_empty() || language.len() > 16)
    {
        return refuse("language is an ISO 639-1 code, original or any".into());
    }
    Ok(Ok(()))
}

/// Checks that each format the profile scores exists, is for its app, and is scored once.
async fn check_formats(
    pool: &SqlitePool,
    input: &ProfileInput,
    app: App,
) -> anyhow::Result<Result<(), Refused>> {
    let mut seen_formats = HashSet::new();
    for score in &input.formats {
        if !seen_formats.insert(score.format_id) {
            return refuse(format!("format {} is scored twice", score.format_id));
        }
        let source: Option<String> =
            sqlx::query_scalar("SELECT source FROM custom_formats WHERE id = ?")
                .bind(score.format_id)
                .fetch_optional(pool)
                .await?;
        match source {
            None => return refuse(format!("there's no format {}", score.format_id)),
            Some(source) if source != app.as_str() => {
                return refuse(format!(
                    "format {} is for {source}, not {}",
                    score.format_id,
                    app.as_str()
                ));
            }
            Some(_) => {}
        }
    }
    Ok(Ok(()))
}

pub(super) struct Checked {
    pub(super) app: App,
    pub(super) name: String,
    pub(super) cutoff: String,
    pub(super) qualities: Vec<RankedQuality>,
}

/// Makes a quality profile as Melete's own: its id, or why it was refused.
pub async fn create_profile(
    pool: &SqlitePool,
    input: &ProfileInput,
) -> anyhow::Result<Result<i64, Refused>> {
    let checked = match check_profile(pool, input).await? {
        Ok(checked) => checked,
        Err(refused) => return Ok(Err(refused)),
    };
    write_profile(pool, None, input, &checked).await
}

/// Changes a quality profile, which becomes Melete's own, so re-imports leave it alone: its id,
/// or why it was refused. Its source can't change.
pub async fn update_profile(
    pool: &SqlitePool,
    id: i64,
    input: &ProfileInput,
) -> anyhow::Result<Result<i64, Refused>> {
    let source: Option<String> =
        sqlx::query_scalar("SELECT source FROM quality_profiles WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let checked = match (source, check_profile(pool, input).await?) {
        (None, _) => return Ok(Err(Refused::NotFound)),
        (_, Err(refused)) => return Ok(Err(refused)),
        (Some(source), Ok(checked)) if source != checked.app.as_str() => {
            return Ok(Err(invalid(
                "a profile's source can't change; make a new one",
            )));
        }
        (_, Ok(checked)) => checked,
    };
    write_profile(pool, Some(id), input, &checked).await
}

pub(super) async fn write_profile(
    pool: &SqlitePool,
    id: Option<i64>,
    input: &ProfileInput,
    checked: &Checked,
) -> anyhow::Result<Result<i64, Refused>> {
    let mut tx = pool.begin().await?;
    let written: Result<i64, sqlx::Error> = match id {
        None => {
            sqlx::query_scalar(
                "INSERT INTO quality_profiles (source, name, upgrade_allowed, cutoff, min_format_score,
                     cutoff_format_score, min_upgrade_format_score, language, updated_at, origin)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'melete') RETURNING id",
            )
            .bind(checked.app.as_str())
            .bind(&checked.name)
            .bind(input.upgrade_allowed)
            .bind(&checked.cutoff)
            .bind(input.min_format_score)
            .bind(input.cutoff_format_score)
            .bind(input.min_upgrade_format_score)
            .bind(&input.language)
            .bind(unix_now())
            .fetch_one(&mut *tx)
            .await
        }
        Some(id) => sqlx::query(
            "UPDATE quality_profiles SET name = ?, upgrade_allowed = ?, cutoff = ?, min_format_score = ?,
                 cutoff_format_score = ?, min_upgrade_format_score = ?, language = ?, updated_at = ?,
                 origin = 'melete'
             WHERE id = ?",
        )
        .bind(&checked.name)
        .bind(input.upgrade_allowed)
        .bind(&checked.cutoff)
        .bind(input.min_format_score)
        .bind(input.cutoff_format_score)
        .bind(input.min_upgrade_format_score)
        .bind(&input.language)
        .bind(unix_now())
        .bind(id)
        .execute(&mut *tx)
        .await
        .map(|_| id),
    };
    let id = match written {
        Ok(id) => id,
        Err(err) if unique_violation(&err) => return Ok(Err(Refused::Taken)),
        Err(err) => return Err(err.into()),
    };
    for delete in [
        "DELETE FROM quality_profile_qualities WHERE profile_id = ?",
        "DELETE FROM quality_profile_formats WHERE profile_id = ?",
    ] {
        sqlx::query(delete).bind(id).execute(&mut *tx).await?;
    }
    for q in &checked.qualities {
        sqlx::query(
            "INSERT INTO quality_profile_qualities (profile_id, quality, rank, group_name, allowed)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(&q.quality)
        .bind(q.rank)
        .bind(&q.group)
        .bind(q.allowed)
        .execute(&mut *tx)
        .await?;
    }
    for score in &input.formats {
        sqlx::query(
            "INSERT INTO quality_profile_formats (profile_id, format_id, score) VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(score.format_id)
        .bind(score.score)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Ok(id))
}

/// Deletes a quality profile; false when there was none with that id.
pub async fn delete_profile(pool: &SqlitePool, id: i64) -> anyhow::Result<bool> {
    let deleted = sqlx::query("DELETE FROM quality_profiles WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(deleted.rows_affected() > 0)
}
