//! Making and changing custom formats and quality profiles in Melete. Each is checked as
//! an import checks it, then marked as Melete's own, which re-imports from Radarr and Sonarr
//! leave alone.

use std::collections::HashSet;

use serde::Deserialize;
use sqlx::SqlitePool;

use super::formats::{Compiled, Condition};
use super::{App, RankedQuality, quality};
use crate::db::unique_violation;
use crate::db::unix_now;

mod anime;
mod custom_formats;
mod quality_profiles;

pub use anime::*;
pub use custom_formats::*;
pub use quality_profiles::*;

/// Why a change wasn't made.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    Invalid(String),
    NotFound,
    /// Another format or profile for the same kind of titles has the name.
    Taken,
}

/// A custom format as an admin sends it.
#[derive(Debug, Deserialize)]
pub struct FormatInput {
    /// "radarr" for movies, "sonarr" for series.
    pub source: String,
    /// Its name, unique within its source.
    pub name: String,
    /// 1 to 50 conditions; every pattern must compile.
    pub conditions: Vec<Condition>,
}

/// A quality profile as an admin sends it.
#[derive(Debug, Deserialize)]
pub struct ProfileInput {
    /// "radarr" for movies, "sonarr" for series; it can't change once made.
    pub source: String,
    /// The Manage service whose titles it's for: "movies" for Radarr's, "series" or "anime"
    /// for Sonarr's; when left out, movies or series. It can't change once made.
    #[serde(default)]
    pub service: Option<String>,
    /// Its name, unique within its service.
    pub name: String,
    /// Whether a file already in the library is replaced by a better release.
    pub upgrade_allowed: bool,
    /// The item (quality or group) upgrades stop at.
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
    /// "announced", "in_cinemas" or "released". Left out, a new profile waits until a film is
    /// released and a changed one keeps its setting.
    #[serde(default)]
    pub minimum_availability: Option<String>,
    /// Lowest first.
    pub items: Vec<ItemInput>,
    /// Scores for custom formats of the same source; any other scores 0.
    pub formats: Vec<ScoreInput>,
}

/// A quality, or a group of qualities ranked together.
#[derive(Debug, Deserialize)]
pub struct ItemInput {
    /// The quality's name, or the group's.
    pub name: String,
    /// The qualities in it: one for a quality on its own.
    pub qualities: Vec<String>,
    /// Whether a release of these qualities may be taken at all.
    pub allowed: bool,
}

/// A custom format's score in a profile.
#[derive(Debug, Deserialize)]
pub struct ScoreInput {
    /// The custom format's row id.
    pub format_id: i64,
    /// Added to a release's score when it matches; may be negative.
    pub score: i64,
}

fn invalid(why: impl Into<String>) -> Refused {
    Refused::Invalid(why.into())
}

fn check_name(name: &str) -> Result<String, Refused> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(invalid("name is 1 to 100 characters"));
    }
    Ok(name.to_owned())
}

fn app(source: &str) -> Result<App, Refused> {
    App::from_source(source)
        .map_err(|_| invalid("source is radarr (for movies) or sonarr (for series)"))
}

/// The service a profile of `app` is for: the one asked for, which must take the app's
/// profiles, else the app's own.
fn service_of(app: App, asked: Option<&str>) -> Result<&'static str, Refused> {
    let Some(asked) = asked else {
        return Ok(super::service::default_for(app));
    };
    super::service::SERVICES
        .into_iter()
        .find(|s| *s == asked && super::service::app_of(s) == Some(app))
        .ok_or_else(|| {
            invalid(match app {
                App::Radarr => "a radarr profile's service is movies",
                App::Sonarr => "a sonarr profile's service is series or anime",
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::profiles::formats::Facts;
    use crate::release;

    /// Accepted, score and first reason for a release of a Japanese title.
    fn judge(profile: &super::super::Loaded, name: &str) -> (bool, i64, Option<String>) {
        let parsed = release::parse(name);
        let facts = Facts::new(name, &parsed, None, &[], Some("ja"));
        let decision = profile.decide(&facts, None);
        (
            decision.accepted,
            decision.score,
            decision.rejections.first().cloned(),
        )
    }

    #[tokio::test]
    async fn the_anime_preset_ranks_dual_audio_then_english_then_japanese() {
        let pool = db::open_in_memory().await;
        let id = anime_preset(&pool, "sonarr").await.unwrap().unwrap();
        let profile = super::super::load(&pool, id).await.unwrap().unwrap();
        assert_eq!(profile.profile.name, "Anime");
        let dual = judge(
            &profile,
            "[Judas] Frieren (01-28) [BD 1080p][HEVC x265 10bit][Dual-Audio][Eng-Subs]",
        );
        let english = judge(&profile, "Frieren.S01E05.1080p.WEB.H264.English.Dub-GRP");
        let subbed = judge(&profile, "[Group] Frieren - 05 [1080p][Eng-Subs]");
        let fansub = judge(&profile, "[SubsPlease] Frieren - 05 (1080p) [ABCD1234].mkv");
        assert!(
            dual.0 && english.0 && subbed.0 && fansub.0,
            "{dual:?} {english:?} {subbed:?} {fansub:?}"
        );
        assert!(
            dual.1 > english.1 && english.1 > subbed.1 && subbed.1 > fansub.1,
            "dual {} > English {} > Japanese with English subtitles {} > unnamed {}",
            dual.1,
            english.1,
            subbed.1,
            fansub.1
        );
        let raw = judge(&profile, "[Raws] Frieren - 05 (1080p).mkv");
        assert!(!raw.0, "{raw:?}");
        let sd = judge(&profile, "[SubsPlease] Frieren - 05 (720p) [ABCD1234].mkv");
        assert!(!sd.0 && sd.2.unwrap().contains("isn't allowed"));
        let uhd = judge(&profile, "Frieren.S01E05.2160p.WEB-DL.DDP5.1.H.265-GRP");
        assert!(uhd.0);

        // Made once: a second time is refused and leaves no formats behind.
        let formats = || async {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM custom_formats")
                .fetch_one(&pool)
                .await
                .unwrap()
        };
        let before = formats().await;
        assert_eq!(
            anime_preset(&pool, "sonarr").await.unwrap(),
            Err(Refused::Taken)
        );
        assert_eq!(formats().await, before);
        // Movies get their own.
        assert!(anime_preset(&pool, "radarr").await.unwrap().is_ok());
        assert!(matches!(
            anime_preset(&pool, "lidarr").await.unwrap(),
            Err(Refused::Invalid(_))
        ));
    }
}
