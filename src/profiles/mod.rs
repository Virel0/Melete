//! Quality profiles and custom formats: which releases are acceptable and how they
//! rank. Imported from Radarr and Sonarr, each app's kept apart.

pub mod decide;
pub mod edit;
pub mod formats;
pub mod import;
pub mod quality;

use std::collections::HashMap;

use serde::Serialize;
use sqlx::{SqliteConnection, SqlitePool};

use crate::arr::ArrClient;
use crate::db::unix_now;
use crate::import::RefusedToEmpty;
use formats::{Compiled, Condition};

/// The app a profile or custom format came from; each app's are kept apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum App {
    Radarr,
    Sonarr,
}

impl App {
    /// The name stored in `source` columns: "radarr" or "sonarr".
    pub fn as_str(self) -> &'static str {
        match self {
            App::Radarr => "radarr",
            App::Sonarr => "sonarr",
        }
    }

    /// The app a `source` column names; an error for any other value.
    pub fn from_source(source: &str) -> anyhow::Result<App> {
        match source {
            "radarr" => Ok(App::Radarr),
            "sonarr" => Ok(App::Sonarr),
            other => anyhow::bail!("unknown source {other}"),
        }
    }

    /// The app's name as people write it, for messages.
    pub fn name(self) -> &'static str {
        match self {
            App::Radarr => "Radarr",
            App::Sonarr => "Sonarr",
        }
    }
}

/// A quality's place in a profile, lowest rank first. A group's qualities share a rank.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedQuality {
    /// The quality's name, e.g. "Bluray-1080p".
    pub quality: String,
    /// Its place in the profile: higher is better.
    pub rank: i64,
    /// The group it's ranked in, if any.
    pub group: Option<String>,
    /// Whether a release of this quality may be taken at all.
    pub allowed: bool,
}

/// A profile as imported, before it has an id.
#[derive(Debug)]
pub struct NewProfile {
    /// The profile's name, unique within its app.
    pub name: String,
    /// Whether a file already in the library is replaced by a better release.
    pub upgrade_allowed: bool,
    /// The quality or group name upgrades stop at.
    pub cutoff: String,
    /// The custom format score a release needs to be taken.
    pub min_format_score: i64,
    /// Upgrades stop once a file's custom format score reaches this.
    pub cutoff_format_score: i64,
    /// How much an upgrade must raise the custom format score by.
    pub min_upgrade_format_score: i64,
    /// The audio language wanted: a language code or "original"; None or "any" for any.
    pub language: Option<String>,
    /// The profile's qualities, ranked, allowed or not.
    pub qualities: Vec<RankedQuality>,
    /// Custom format names and scores.
    pub scores: Vec<(String, i64)>,
}

/// What an import from Radarr or Sonarr changed.
#[derive(Debug, Serialize)]
pub struct ImportReport {
    /// How many custom formats the app has.
    pub formats: usize,
    /// How many quality profiles the app has.
    pub profiles: usize,
    /// Formats and profiles imported from the app before that it no longer has.
    pub removed_formats: u64,
    /// Profiles removed the same way.
    pub removed_profiles: u64,
}

/// Makes Melete's copy of the app's custom formats and quality profiles match the app's.
pub async fn import(
    pool: &SqlitePool,
    client: &ArrClient,
    app: App,
) -> anyhow::Result<ImportReport> {
    client.expect_app(app.name()).await?;
    let languages = client.languages().await?;
    let formats = client.custom_formats().await?;
    let profiles = client.full_quality_profiles().await?;
    // Both apps always have profiles; none means the wrong URL or a broken install.
    if profiles.is_empty() {
        return Err(RefusedToEmpty(format!("{} listed no quality profiles", app.name())).into());
    }
    let converted = import::convert(app, &languages, &formats, &profiles)
        .map_err(|err| import::Unsupported(format!("{err:#}")))?;
    save(pool, app, &converted).await
}

pub(crate) async fn save(
    pool: &SqlitePool,
    app: App,
    converted: &import::Converted,
) -> anyhow::Result<ImportReport> {
    let now = unix_now();
    let mut tx = pool.begin().await?;
    let format_ids = save_formats(&mut tx, app, &converted.formats, now).await?;
    let kept: Vec<i64> = format_ids.values().copied().collect();
    let removed_formats = sqlx::query(
        "DELETE FROM custom_formats
         WHERE source = ? AND origin = 'imported' AND id NOT IN (SELECT value FROM json_each(?))",
    )
    .bind(app.as_str())
    .bind(serde_json::to_string(&kept)?)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    let mut profile_ids = Vec::new();
    for p in &converted.profiles {
        let Some(id) = save_profile(&mut tx, app, p, &format_ids, now).await? else {
            continue;
        };
        profile_ids.push(id);
    }
    let removed_profiles = sqlx::query(
        "DELETE FROM quality_profiles
         WHERE source = ? AND origin = 'imported' AND id NOT IN (SELECT value FROM json_each(?))",
    )
    .bind(app.as_str())
    .bind(serde_json::to_string(&profile_ids)?)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(ImportReport {
        formats: converted.formats.len(),
        profiles: converted.profiles.len(),
        removed_formats,
        removed_profiles,
    })
}

/// Saves the app's custom formats, leaving those made or edited in Melete as they are; each
/// format's id by name.
async fn save_formats<'a>(
    tx: &mut SqliteConnection,
    app: App,
    formats: &'a [(String, Vec<Condition>)],
    now: i64,
) -> anyhow::Result<HashMap<&'a str, i64>> {
    let mut format_ids = HashMap::new();
    for (name, conditions) in formats {
        // A format made or edited in Melete stays as it is; the app's profiles score it.
        let updated: Option<i64> = sqlx::query_scalar(
            "INSERT INTO custom_formats (source, name, specifications, updated_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (source, name) DO UPDATE
             SET specifications = excluded.specifications, updated_at = excluded.updated_at
             WHERE custom_formats.origin = 'imported'
             RETURNING id",
        )
        .bind(app.as_str())
        .bind(name)
        .bind(serde_json::to_string(conditions)?)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        let id = match updated {
            Some(id) => id,
            None => {
                sqlx::query_scalar("SELECT id FROM custom_formats WHERE source = ? AND name = ?")
                    .bind(app.as_str())
                    .bind(name)
                    .fetch_one(&mut *tx)
                    .await?
            }
        };
        format_ids.insert(name.as_str(), id);
    }
    Ok(format_ids)
}

/// Saves one of the app's profiles with its qualities and scores; its id, or None when it was
/// made or edited in Melete and left as it is.
async fn save_profile(
    tx: &mut SqliteConnection,
    app: App,
    p: &NewProfile,
    format_ids: &HashMap<&str, i64>,
    now: i64,
) -> anyhow::Result<Option<i64>> {
    let updated: Option<i64> = sqlx::query_scalar(
        "INSERT INTO quality_profiles (source, name, upgrade_allowed, cutoff, min_format_score,
                 cutoff_format_score, min_upgrade_format_score, language, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (source, name) DO UPDATE
             SET upgrade_allowed = excluded.upgrade_allowed, cutoff = excluded.cutoff,
                 min_format_score = excluded.min_format_score,
                 cutoff_format_score = excluded.cutoff_format_score,
                 min_upgrade_format_score = excluded.min_upgrade_format_score,
                 language = excluded.language, updated_at = excluded.updated_at
             WHERE quality_profiles.origin = 'imported'
             RETURNING id",
    )
    .bind(app.as_str())
    .bind(&p.name)
    .bind(p.upgrade_allowed)
    .bind(&p.cutoff)
    .bind(p.min_format_score)
    .bind(p.cutoff_format_score)
    .bind(p.min_upgrade_format_score)
    .bind(&p.language)
    .bind(now)
    .fetch_optional(&mut *tx)
    .await?;
    // A profile made or edited in Melete keeps its own qualities and scores.
    let Some(id) = updated else {
        return Ok(None);
    };
    for delete in [
        "DELETE FROM quality_profile_qualities WHERE profile_id = ?",
        "DELETE FROM quality_profile_formats WHERE profile_id = ?",
    ] {
        sqlx::query(delete).bind(id).execute(&mut *tx).await?;
    }
    for q in &p.qualities {
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
    for (format, score) in &p.scores {
        let format_id = format_ids[format.as_str()];
        sqlx::query(
            "INSERT INTO quality_profile_formats (profile_id, format_id, score) VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(format_id)
        .bind(score)
        .execute(&mut *tx)
        .await?;
    }
    Ok(Some(id))
}

/// A quality profile as kept, with its qualities and custom format scores.
#[derive(Debug, Serialize)]
pub struct Profile {
    /// The profile's row id.
    pub id: i64,
    /// The app it belongs to ([`App::as_str`]).
    pub source: String,
    /// The profile's name, unique within its app.
    pub name: String,
    /// Whether a file already in the library is replaced by a better release.
    pub upgrade_allowed: bool,
    /// The quality or group name upgrades stop at.
    pub cutoff: String,
    /// The custom format score a release needs to be taken.
    pub min_format_score: i64,
    /// Upgrades stop once a file's custom format score reaches this.
    pub cutoff_format_score: i64,
    /// How much an upgrade must raise the custom format score by.
    pub min_upgrade_format_score: i64,
    /// The audio language wanted: a language code or "original"; None or "any" for any.
    pub language: Option<String>,
    /// For a movie profile, how far out a film must be before its releases are taken:
    /// "announced", "in_cinemas" or "released".
    pub minimum_availability: String,
    /// "imported" from Radarr or Sonarr, or "melete" when made or edited here (re-imports
    /// leave those alone).
    pub origin: String,
    /// Lowest first.
    pub items: Vec<ProfileItem>,
    /// The custom formats it scores, in name order; any other scores 0.
    pub formats: Vec<FormatScore>,
}

/// A quality, or a group of qualities ranked together.
#[derive(Debug, Serialize)]
pub struct ProfileItem {
    /// The quality's name, or the group's.
    pub name: String,
    /// The qualities in it: one for a quality on its own.
    pub qualities: Vec<String>,
    /// Whether a release of these qualities may be taken at all.
    pub allowed: bool,
}

/// A custom format's score in a profile.
#[derive(Debug, Clone, Serialize)]
pub struct FormatScore {
    /// The custom format's row id.
    pub format_id: i64,
    /// The custom format's name.
    pub name: String,
    /// Added to a release's score when it matches; may be negative.
    pub score: i64,
}

#[derive(sqlx::FromRow)]
struct ProfileRow {
    id: i64,
    source: String,
    name: String,
    upgrade_allowed: bool,
    cutoff: String,
    min_format_score: i64,
    cutoff_format_score: i64,
    min_upgrade_format_score: i64,
    language: Option<String>,
    origin: String,
    minimum_availability: String,
}

/// Every quality profile, both apps', by app then name.
pub async fn list(pool: &SqlitePool) -> anyhow::Result<Vec<Profile>> {
    let rows: Vec<ProfileRow> = sqlx::query_as(
        "SELECT id, source, name, upgrade_allowed, cutoff, min_format_score, cutoff_format_score,
                min_upgrade_format_score, language, origin, minimum_availability
         FROM quality_profiles ORDER BY source, name",
    )
    .fetch_all(pool)
    .await?;
    let mut profiles = Vec::new();
    for row in rows {
        profiles.push(fill(pool, row).await?);
    }
    Ok(profiles)
}

/// One quality profile; None when there's no such id.
pub async fn get(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<Profile>> {
    let row: Option<ProfileRow> = sqlx::query_as(
        "SELECT id, source, name, upgrade_allowed, cutoff, min_format_score, cutoff_format_score,
                min_upgrade_format_score, language, origin, minimum_availability
         FROM quality_profiles WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some(row) => Ok(Some(fill(pool, row).await?)),
        None => Ok(None),
    }
}

async fn fill(pool: &SqlitePool, row: ProfileRow) -> anyhow::Result<Profile> {
    let qualities: Vec<(String, i64, Option<String>, bool)> = sqlx::query_as(
        "SELECT quality, rank, group_name, allowed FROM quality_profile_qualities
         WHERE profile_id = ? ORDER BY rank, rowid",
    )
    .bind(row.id)
    .fetch_all(pool)
    .await?;
    let mut items: Vec<(i64, ProfileItem)> = Vec::new();
    for (quality, rank, group, allowed) in qualities {
        match items.last_mut() {
            Some((last_rank, item)) if *last_rank == rank => item.qualities.push(quality),
            _ => items.push((
                rank,
                ProfileItem {
                    name: group.unwrap_or_else(|| quality.clone()),
                    qualities: vec![quality],
                    allowed,
                },
            )),
        }
    }
    let formats: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT f.id, f.name, s.score FROM quality_profile_formats s
         JOIN custom_formats f ON f.id = s.format_id
         WHERE s.profile_id = ? ORDER BY f.name",
    )
    .bind(row.id)
    .fetch_all(pool)
    .await?;
    Ok(Profile {
        id: row.id,
        source: row.source,
        name: row.name,
        upgrade_allowed: row.upgrade_allowed,
        cutoff: row.cutoff,
        min_format_score: row.min_format_score,
        cutoff_format_score: row.cutoff_format_score,
        min_upgrade_format_score: row.min_upgrade_format_score,
        language: row.language,
        minimum_availability: row.minimum_availability,
        origin: row.origin,
        items: items.into_iter().map(|(_, item)| item).collect(),
        formats: formats
            .into_iter()
            .map(|(format_id, name, score)| FormatScore {
                format_id,
                name,
                score,
            })
            .collect(),
    })
}

/// A custom format as kept: what a release must match to score it.
#[derive(Debug, Serialize)]
pub struct CustomFormat {
    /// The format's row id.
    pub id: i64,
    /// The app it belongs to ([`App::as_str`]).
    pub source: String,
    /// The format's name, unique within its app.
    pub name: String,
    /// "imported", or "melete" when made or edited here (re-imports leave those alone).
    pub origin: String,
    /// What a release is tested against.
    pub conditions: Vec<Condition>,
}

/// Every custom format, both apps', by app then name.
pub async fn custom_formats(pool: &SqlitePool) -> anyhow::Result<Vec<CustomFormat>> {
    let rows: Vec<(i64, String, String, String, String)> = sqlx::query_as(
        "SELECT id, source, name, origin, specifications FROM custom_formats ORDER BY source, name",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|(id, source, name, origin, specifications)| {
            Ok(CustomFormat {
                id,
                source,
                name,
                origin,
                conditions: serde_json::from_str(&specifications)?,
            })
        })
        .collect()
}

/// A profile ready to decide on releases: its app's custom formats compiled, with this
/// profile's scores (0 for formats it doesn't score).
pub struct Loaded {
    /// The profile itself.
    pub profile: Profile,
    formats: Vec<(Compiled, i64)>,
}

/// A profile with its app's custom formats compiled; None when there's no such id.
///
/// # Errors
/// Also when a stored format no longer compiles, such as a pattern that isn't valid.
pub async fn load(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<Loaded>> {
    let Some(profile) = get(pool, id).await? else {
        return Ok(None);
    };
    let app = App::from_source(&profile.source)?;
    let scores: HashMap<i64, i64> = profile
        .formats
        .iter()
        .map(|f| (f.format_id, f.score))
        .collect();
    let formats = custom_formats(pool)
        .await?
        .into_iter()
        .filter(|f| f.source == profile.source)
        .map(|f| {
            let score = scores.get(&f.id).copied().unwrap_or(0);
            Ok((Compiled::new(app, f.name, f.conditions)?, score))
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(Some(Loaded { profile, formats }))
}
