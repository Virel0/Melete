//! Music quality profiles: which formats an album may come in and which is
//! better, ranked lowest first as Lidarr ranks them, with qualities in a group sharing a rank.
//! Imported from Lidarr, or the built-in preset that prefers FLAC.

use std::collections::HashMap;

use anyhow::{anyhow, bail};
use serde::Serialize;
use sqlx::SqlitePool;

use crate::arr::{ArrClient, FullQualityProfile};
use crate::music_release::{MusicRelease, QUALITIES};

mod editing;

pub use editing::*;

/// The preset's name.
pub const LOSSLESS_PRESET: &str = "FLAC preferred";

/// One quality in a profile, with its place.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct RankedQuality {
    /// One of [`QUALITIES`].
    pub quality: String,
    /// Its place in the profile: higher is better; qualities in a group share one.
    pub rank: i64,
    /// The group it's in (Lidarr's, as "Lossless"), sharing its rank; none when on its own.
    #[sqlx(rename = "group_name")]
    pub group: Option<String>,
    /// Whether a release in this quality may be grabbed.
    pub allowed: bool,
}

/// A music quality profile: which qualities an album may come in, and which is better.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MusicProfile {
    /// The profile's row id.
    pub id: i64,
    /// Where it came from: `lidarr` or `preset`.
    pub source: String,
    /// Its name, unique among those from the same place.
    pub name: String,
    /// Whether a better quality may replace one already had, until the cutoff is reached.
    pub upgrade_allowed: bool,
    /// A quality's name, or a group's.
    pub cutoff: String,
    /// Every quality it ranks, lowest first.
    pub qualities: Vec<RankedQuality>,
}

/// A profile about to be written.
struct NewProfile {
    name: String,
    upgrade_allowed: bool,
    cutoff: String,
    qualities: Vec<RankedQuality>,
}

/// The preset: lossy at its best (MP3 320 or V0) when nothing better is out, and any lossless
/// format above it, FLAC the cutoff. Upgrades from lossy to lossless, never past it.
fn lossless_preset() -> NewProfile {
    let ranked = |qualities: &[&str], rank: i64, group: &str, allowed: bool| {
        qualities
            .iter()
            .map(|q| RankedQuality {
                quality: (*q).to_owned(),
                rank,
                group: Some(group.to_owned()),
                allowed,
            })
            .collect::<Vec<_>>()
    };
    let mut qualities = ranked(&["MP3-320", "MP3-VBR-V0"], 0, "Lossy", true);
    qualities.extend(ranked(
        &["FLAC", "FLAC 24bit", "ALAC", "ALAC 24bit", "APE", "WavPack"],
        1,
        "Lossless",
        true,
    ));
    NewProfile {
        name: LOSSLESS_PRESET.to_owned(),
        upgrade_allowed: true,
        cutoff: "Lossless".to_owned(),
        qualities,
    }
}

/// A Lidarr profile in Melete's terms. A quality Melete doesn't know is skipped when the
/// profile doesn't allow it (a newer Lidarr's extra), and refused when it does: dropping an
/// allowed quality silently would reject releases the profile wants.
fn convert(profile: &FullQualityProfile, skipped: &mut Vec<String>) -> anyhow::Result<NewProfile> {
    let mut qualities = Vec::new();
    let mut cutoff = None;
    for (rank, item) in profile.items.iter().enumerate() {
        let (members, group) = match &item.quality {
            Some(q) => (vec![q], None),
            None => {
                let name = item
                    .name
                    .clone()
                    .ok_or_else(|| anyhow!("a group has no name"))?;
                (
                    item.items
                        .iter()
                        .filter_map(|i| i.quality.as_ref())
                        .collect(),
                    Some(name),
                )
            }
        };
        let is_cutoff_group = group.is_some() && item.id == Some(profile.cutoff);
        if is_cutoff_group {
            cutoff = group.clone();
        }
        for member in members {
            let Some(known) = QUALITIES
                .iter()
                .find(|q| q.eq_ignore_ascii_case(&member.name))
            else {
                if item.allowed {
                    bail!("it allows {}, a quality Melete doesn't know", member.name);
                }
                skipped.push(member.name.clone());
                continue;
            };
            if group.is_none() && member.id == profile.cutoff {
                cutoff = Some((*known).to_owned());
            }
            qualities.push(RankedQuality {
                quality: (*known).to_owned(),
                rank: rank as i64,
                group: group.clone(),
                allowed: item.allowed,
            });
        }
    }
    let cutoff = cutoff
        .ok_or_else(|| anyhow!("the cutoff ({}) isn't one of its qualities", profile.cutoff))?;
    Ok(NewProfile {
        name: profile.name.clone(),
        upgrade_allowed: profile.upgrade_allowed,
        cutoff,
        qualities,
    })
}

/// Writes a profile, replacing one of the same source and name. Returns its id. A Lidarr
/// import leaves one an admin changed in Melete as they set it.
async fn save(pool: &SqlitePool, source: &str, profile: &NewProfile) -> sqlx::Result<i64> {
    let mut tx = pool.begin().await?;
    if source == "lidarr" {
        let edited: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM music_profiles
             WHERE source = 'lidarr' AND name = ? AND edited_at IS NOT NULL",
        )
        .bind(&profile.name)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(id) = edited {
            return Ok(id);
        }
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO music_profiles (source, name, upgrade_allowed, cutoff, updated_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (source, name) DO UPDATE SET upgrade_allowed = excluded.upgrade_allowed,
             cutoff = excluded.cutoff, updated_at = excluded.updated_at, edited_at = NULL
         RETURNING id",
    )
    .bind(source)
    .bind(&profile.name)
    .bind(profile.upgrade_allowed)
    .bind(&profile.cutoff)
    .bind(crate::db::unix_now())
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM music_profile_qualities WHERE profile_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for q in &profile.qualities {
        sqlx::query(
            "INSERT INTO music_profile_qualities (profile_id, quality, rank, group_name, allowed)
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
    tx.commit().await?;
    Ok(id)
}

/// Creates the preset, or puts it back as it ships. Returns its id.
pub async fn lossless(pool: &SqlitePool) -> sqlx::Result<i64> {
    save(pool, "preset", &lossless_preset()).await
}

/// What importing Lidarr's quality profiles did.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    /// How many profiles were imported.
    pub profiles: usize,
    /// Qualities Lidarr ranks that Melete doesn't know, all of them not allowed.
    pub skipped_qualities: Vec<String>,
}

/// Imports Lidarr's quality profiles, replacing earlier imports of the same name. Nothing is
/// written unless every profile converts.
pub async fn import_lidarr(pool: &SqlitePool, lidarr: &ArrClient) -> anyhow::Result<ImportReport> {
    lidarr.expect_app("Lidarr").await?;
    let profiles = lidarr.lidarr_full_quality_profiles().await?;
    let mut skipped = Vec::new();
    let converted = profiles
        .iter()
        .map(|p| {
            convert(p, &mut skipped).map_err(|err| {
                crate::profiles::import::Unsupported(format!("quality profile {}: {err}", p.name))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    for profile in &converted {
        save(pool, "lidarr", profile).await?;
    }
    skipped.sort();
    skipped.dedup();
    Ok(ImportReport {
        profiles: converted.len(),
        skipped_qualities: skipped,
    })
}

/// Every profile, by name.
pub async fn list(pool: &SqlitePool) -> sqlx::Result<Vec<MusicProfile>> {
    let profiles: Vec<(i64, String, String, bool, String)> = sqlx::query_as(
        "SELECT id, source, name, upgrade_allowed, cutoff FROM music_profiles
         ORDER BY name COLLATE NOCASE, source",
    )
    .fetch_all(pool)
    .await?;
    let qualities: Vec<(i64, String, i64, Option<String>, bool)> = sqlx::query_as(
        "SELECT profile_id, quality, rank, group_name, allowed FROM music_profile_qualities
         ORDER BY rank, quality",
    )
    .fetch_all(pool)
    .await?;
    let mut of: HashMap<i64, Vec<RankedQuality>> = HashMap::new();
    for (profile, quality, rank, group, allowed) in qualities {
        of.entry(profile).or_default().push(RankedQuality {
            quality,
            rank,
            group,
            allowed,
        });
    }
    Ok(profiles
        .into_iter()
        .map(|(id, source, name, upgrade_allowed, cutoff)| MusicProfile {
            qualities: of.remove(&id).unwrap_or_default(),
            id,
            source,
            name,
            upgrade_allowed,
            cutoff,
        })
        .collect())
}

/// A profile by its id; none when there's no such profile.
pub async fn get(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<MusicProfile>> {
    Ok(list(pool).await?.into_iter().find(|p| p.id == id))
}

/// A wanted artist's profile: one chosen in Melete, else the one imported from Lidarr under
/// the artist's profile name. None when neither is there.
pub async fn for_artist(
    pool: &SqlitePool,
    wanted_artist_id: i64,
) -> sqlx::Result<Option<MusicProfile>> {
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT coalesce(a.profile_id, (SELECT p.id FROM music_profiles p
                                        WHERE p.source = 'lidarr' AND p.name = a.quality_profile))
         FROM wanted_artists a WHERE a.id = ?",
    )
    .bind(wanted_artist_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    match id {
        Some(id) => get(pool, id).await,
        None => Ok(None),
    }
}

/// Whether a release's quality is one a profile takes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Decision {
    /// Whether the profile allows the quality.
    pub accepted: bool,
    /// The release's quality, one of [`QUALITIES`].
    pub quality: &'static str,
    /// The quality's rank in the profile, when it's ranked at all.
    pub rank: Option<i64>,
    /// Why it was refused; empty when accepted.
    pub reasons: Vec<String>,
}

/// Whether a release's quality is one the profile takes.
pub fn decide(profile: &MusicProfile, release: &MusicRelease) -> Decision {
    let ranked = profile
        .qualities
        .iter()
        .find(|q| q.quality == release.quality);
    let mut reasons = Vec::new();
    match ranked {
        None => reasons.push(format!("{} isn't in the profile", release.quality)),
        Some(q) if !q.allowed => {
            reasons.push(format!("the profile doesn't allow {}", release.quality))
        }
        Some(_) => {}
    }
    Decision {
        accepted: reasons.is_empty(),
        quality: release.quality,
        rank: ranked.map(|q| q.rank),
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::import::testing::FakeArr;
    use crate::music_release::parse;

    fn lidarr_profiles() -> serde_json::Value {
        json!([
            { "id": 1, "name": "Lossless", "upgradeAllowed": true, "cutoff": 1005, "items": [
                { "quality": { "id": 1, "name": "MP3-192" }, "items": [], "allowed": false },
                { "quality": { "id": 99, "name": "Opus 512" }, "items": [], "allowed": false },
                { "name": "High Quality Lossy", "id": 1004, "allowed": true, "items": [
                    { "quality": { "id": 2, "name": "MP3-VBR-V0" }, "items": [], "allowed": true },
                    { "quality": { "id": 4, "name": "MP3-320" }, "items": [], "allowed": true } ] },
                { "name": "Lossless", "id": 1005, "allowed": true, "items": [
                    { "quality": { "id": 6, "name": "FLAC" }, "items": [], "allowed": true },
                    { "quality": { "id": 21, "name": "FLAC 24bit" }, "items": [], "allowed": true } ] } ] },
            { "id": 2, "name": "Any", "upgradeAllowed": false, "cutoff": 4, "items": [
                { "quality": { "id": 4, "name": "MP3-320" }, "items": [], "allowed": true },
                { "quality": { "id": 6, "name": "FLAC" }, "items": [], "allowed": true } ] },
        ])
    }

    async fn start(profiles: serde_json::Value) -> ArrClient {
        let fake = FakeArr::default();
        fake.reply("/api/v1/system/status", json!({ "appName": "Lidarr" }))
            .reply("/api/v1/qualityprofile", profiles);
        ArrClient::new(&fake.start().await, "key".into()).unwrap()
    }

    #[tokio::test]
    async fn imports_lidarrs_profiles_with_their_groups_and_cutoffs() {
        let pool = crate::db::open_in_memory().await;
        let report = import_lidarr(&pool, &start(lidarr_profiles()).await)
            .await
            .unwrap();
        assert_eq!(
            report,
            ImportReport {
                profiles: 2,
                skipped_qualities: vec!["Opus 512".into()]
            }
        );
        let profiles = list(&pool).await.unwrap();
        let any = &profiles[0];
        assert_eq!(
            (any.name.as_str(), any.cutoff.as_str(), any.upgrade_allowed),
            ("Any", "MP3-320", false)
        );
        let lossless = &profiles[1];
        assert_eq!(lossless.cutoff, "Lossless");
        let flac = lossless
            .qualities
            .iter()
            .find(|q| q.quality == "FLAC")
            .unwrap();
        assert_eq!(
            (flac.rank, flac.group.as_deref(), flac.allowed),
            (3, Some("Lossless"), true)
        );
        // Running it again replaces rather than adds.
        import_lidarr(&pool, &start(lidarr_profiles()).await)
            .await
            .unwrap();
        assert_eq!(list(&pool).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn refuses_a_profile_that_allows_a_quality_melete_doesnt_know() {
        let pool = crate::db::open_in_memory().await;
        let profiles = json!([{ "id": 1, "name": "New", "upgradeAllowed": true, "cutoff": 9, "items": [
            { "quality": { "id": 9, "name": "Opus 512" }, "items": [], "allowed": true } ] }]);
        let err = import_lidarr(&pool, &start(profiles).await)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Opus 512"), "{err:#}");
        assert!(list(&pool).await.unwrap().is_empty(), "nothing written");
    }

    #[tokio::test]
    async fn the_preset_takes_lossless_first_and_lossy_at_its_best() {
        let pool = crate::db::open_in_memory().await;
        let id = lossless(&pool).await.unwrap();
        assert_eq!(
            lossless(&pool).await.unwrap(),
            id,
            "made once, put back as it ships"
        );
        let profile = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(
            (profile.name.as_str(), profile.cutoff.as_str()),
            (LOSSLESS_PRESET, "Lossless")
        );
        let decided = |name: &str| decide(&profile, &parse(name));
        let flac = decided("Artist - Album (2020) [FLAC 24-96]");
        assert_eq!(
            (flac.accepted, flac.quality, flac.rank),
            (true, "FLAC 24bit", Some(1))
        );
        assert_eq!(decided("Artist - Album (2020) [MP3 320]").rank, Some(0));
        let low = decided("Artist - Album (2020) [MP3 192]");
        assert!(
            !low.accepted && low.reasons[0].contains("isn't in the profile"),
            "{low:?}"
        );
        assert!(
            !decided("Artist - Album (2020)").accepted,
            "an unknown format is never taken"
        );
    }

    #[tokio::test]
    async fn an_artist_gets_its_chosen_profile_else_lidarrs_by_name() {
        let pool = crate::db::open_in_memory().await;
        import_lidarr(&pool, &start(lidarr_profiles()).await)
            .await
            .unwrap();
        let preset = lossless(&pool).await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, quality_profile, source, added_at)
                 VALUES (1, 'a', 'A', 1, 'all', 'Any', 'lidarr', 0), (2, 'b', 'B', 1, 'all', 'Gone', 'lidarr', 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(for_artist(&pool, 1).await.unwrap().unwrap().name, "Any");
        assert_eq!(for_artist(&pool, 2).await.unwrap(), None);
        sqlx::query("UPDATE wanted_artists SET profile_id = ? WHERE id = 2")
            .bind(preset)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            for_artist(&pool, 2).await.unwrap().unwrap().name,
            LOSSLESS_PRESET
        );
    }
}
