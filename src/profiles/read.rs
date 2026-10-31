//! Reading quality profiles as kept, with their qualities and custom format scores.

use super::*;

/// A quality profile as kept, with its qualities and custom format scores.
#[derive(Debug, Serialize)]
pub struct Profile {
    /// The profile's row id.
    pub id: i64,
    /// The app it belongs to ([`App::as_str`]).
    pub source: String,
    /// The Manage service whose titles it's for: movies, series or anime.
    pub service: String,
    /// The profile's name, unique within its service.
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
    service: String,
    name: String,
    upgrade_allowed: bool,
    cutoff: String,
    min_format_score: i64,
    cutoff_format_score: i64,
    min_upgrade_format_score: i64,
    language: Option<String>,
    origin: String,
}

/// Every quality profile, both apps', by app then name.
pub async fn list(pool: &SqlitePool) -> anyhow::Result<Vec<Profile>> {
    let rows: Vec<ProfileRow> = sqlx::query_as(
        "SELECT id, source, service, name, upgrade_allowed, cutoff, min_format_score, cutoff_format_score,
                min_upgrade_format_score, language, origin
         FROM quality_profiles ORDER BY source, service DESC, name",
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
        "SELECT id, source, service, name, upgrade_allowed, cutoff, min_format_score, cutoff_format_score,
                min_upgrade_format_score, language, origin
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
        service: row.service,
        name: row.name,
        upgrade_allowed: row.upgrade_allowed,
        cutoff: row.cutoff,
        min_format_score: row.min_format_score,
        cutoff_format_score: row.cutoff_format_score,
        min_upgrade_format_score: row.min_upgrade_format_score,
        language: row.language,
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
