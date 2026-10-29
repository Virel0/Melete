//! Editing and deleting a profile.

use super::*;

/// What an admin may change in a profile: its name, upgrades, cutoff, and which of
/// its qualities are allowed. Ranks and groups stay as they are.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileChange {
    /// The new name, 1 to 100 characters; a profile from Lidarr must keep Lidarr's.
    pub name: String,
    /// Whether a better quality may replace one already had, until the cutoff is reached.
    pub upgrade_allowed: bool,
    /// A quality's name, or a group's.
    pub cutoff: String,
    /// The qualities allowed; a group's go together.
    pub allowed: Vec<String>,
}

/// What changing a profile came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Changed {
    /// Saved.
    Done,
    /// No profile has that id.
    NotFound,
    /// Another profile from the same place has that name.
    Taken,
    /// The change isn't one the profile can take, and why, in words.
    Invalid(String),
}

/// Changes a profile; an import from Lidarr leaves it as set from then on.
pub async fn change(pool: &SqlitePool, id: i64, change: &ProfileChange) -> anyhow::Result<Changed> {
    let Some(profile) = get(pool, id).await? else {
        return Ok(Changed::NotFound);
    };
    let name = change.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Ok(Changed::Invalid("a name is 1 to 100 characters".into()));
    }
    if profile.source == "lidarr" && name != profile.name {
        return Ok(Changed::Invalid(
            "a profile from Lidarr keeps Lidarr's name: artists from Lidarr find their profile by it"
                .into(),
        ));
    }
    if let Some(unknown) = change
        .allowed
        .iter()
        .find(|q| !profile.qualities.iter().any(|r| &r.quality == *q))
    {
        return Ok(Changed::Invalid(format!(
            "{unknown} isn't one of its qualities"
        )));
    }
    if change.allowed.is_empty() {
        return Ok(Changed::Invalid("allow at least one quality".into()));
    }
    let allows = |q: &RankedQuality| change.allowed.contains(&q.quality);
    // A group is one choice, as in Lidarr.
    for q in &profile.qualities {
        if let Some(group) = &q.group
            && profile
                .qualities
                .iter()
                .any(|o| o.group.as_ref() == Some(group) && allows(o) != allows(q))
        {
            return Ok(Changed::Invalid(format!(
                "{group}'s qualities are allowed or not together"
            )));
        }
    }
    let cutoff_allowed = profile.qualities.iter().any(|q| {
        (q.quality == change.cutoff || q.group.as_deref() == Some(change.cutoff.as_str()))
            && allows(q)
    });
    if !cutoff_allowed {
        return Ok(Changed::Invalid(
            "the cutoff is one of the qualities or groups it allows".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    let now = crate::db::unix_now();
    let renamed = sqlx::query(
        "UPDATE music_profiles SET name = ?, upgrade_allowed = ?, cutoff = ?, updated_at = ?,
             edited_at = ?
         WHERE id = ?",
    )
    .bind(name)
    .bind(change.upgrade_allowed)
    .bind(&change.cutoff)
    .bind(now)
    .bind(now)
    .bind(id)
    .execute(&mut *tx)
    .await;
    match renamed {
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => return Ok(Changed::Taken),
        other => {
            other?;
        }
    }
    for q in &profile.qualities {
        sqlx::query(
            "UPDATE music_profile_qualities SET allowed = ? WHERE profile_id = ? AND quality = ?",
        )
        .bind(allows(q))
        .bind(id)
        .bind(&q.quality)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Changed::Done)
}

/// What deleting a profile came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Deleted {
    /// Deleted.
    Done,
    /// No profile has that id.
    NotFound,
    /// What uses it, in words.
    InUse(String),
}

/// Deletes a profile nothing uses: no wanted artist (chosen, or by Lidarr's name for
/// it), no wanted song, and not where approved music requests go. Past searches keep their
/// results without it.
pub async fn delete(pool: &SqlitePool, id: i64) -> anyhow::Result<Deleted> {
    let mut tx = pool.begin().await?;
    let found: Option<(String, String)> =
        sqlx::query_as("SELECT source, name FROM music_profiles WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((source, name)) = found else {
        return Ok(Deleted::NotFound);
    };
    let (artists, songs, defaults): (i64, i64, bool) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM wanted_artists
                 WHERE profile_id = ?1
                    OR (profile_id IS NULL AND ?2 = 'lidarr' AND quality_profile = ?3)),
                (SELECT count(*) FROM wanted_songs WHERE music_profile_id = ?1),
                EXISTS (SELECT 1 FROM request_defaults WHERE music_profile_id = ?1)",
    )
    .bind(id)
    .bind(&source)
    .bind(&name)
    .fetch_one(&mut *tx)
    .await?;
    let mut uses = Vec::new();
    if artists > 0 {
        uses.push(format!(
            "{artists} wanted artist{}",
            if artists == 1 { "" } else { "s" }
        ));
    }
    if songs > 0 {
        uses.push(format!(
            "{songs} wanted song{}",
            if songs == 1 { "" } else { "s" }
        ));
    }
    let mut what = Vec::new();
    if !uses.is_empty() {
        what.push(format!("used by {}", uses.join(" and ")));
    }
    if defaults {
        what.push("the one approved music requests get".to_owned());
    }
    if !what.is_empty() {
        return Ok(Deleted::InUse(format!(
            "{name} is {}; choose another profile there first",
            what.join(" and ")
        )));
    }
    sqlx::query("DELETE FROM music_profiles WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Deleted::Done)
}
