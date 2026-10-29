//! Deciding on a release against a profile, with every reason kept, so why a release was or
//! wasn't grabbed can be shown. The rules are Radarr's: the quality must be allowed, the custom
//! format score must reach the minimum, the audio must be the profile's language, and against an
//! existing file a release must be better quality (below the cutoff), a newer revision, or score
//! enough higher at the same quality while the file is below the upgrade-until score.

use std::cmp::Ordering;

use serde::Serialize;

use super::Loaded;
use super::formats::Facts;

/// A profile's verdict on a release, with what it was worked out from.
#[derive(Debug, Serialize)]
pub struct Decision {
    /// Whether the release may be taken: true only when `rejections` is empty.
    pub accepted: bool,
    /// The release's quality, by name.
    pub quality: String,
    /// The quality's rank in the profile, lowest 0; null when the profile doesn't list it.
    pub rank: Option<i64>,
    /// The audio languages it's taken to have (see [`Facts`]).
    pub languages: Vec<String>,
    /// Every custom format the release matches, with this profile's score for it.
    pub formats: Vec<MatchedFormat>,
    /// Its custom format score: the matched formats' scores added up.
    pub score: i64,
    /// Why it was turned down; empty when accepted.
    pub rejections: Vec<String>,
}

/// A custom format a release matches.
#[derive(Debug, Clone, Serialize)]
pub struct MatchedFormat {
    /// The format's name.
    pub name: String,
    /// The profile's score for it; 0 when the profile doesn't score it.
    pub score: i64,
}

/// The file already on disk.
pub struct Existing {
    /// Its quality, by name.
    pub quality: String,
    /// PROPER and REPACK count.
    pub proper: u8,
    /// Its custom format score under the same profile.
    pub score: i64,
}

impl Loaded {
    /// The custom formats a release matches, highest score first, and its total score.
    pub fn score(&self, facts: &Facts) -> Result<(Vec<MatchedFormat>, i64), String> {
        let mut matched = Vec::new();
        for (format, score) in &self.formats {
            if format.matches(facts)? {
                matched.push(MatchedFormat {
                    name: format.name.clone(),
                    score: *score,
                });
            }
        }
        // Highest score first: the breakdown answers "why this one?".
        matched.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
        let total = matched.iter().map(|m| m.score).sum();
        Ok((matched, total))
    }

    fn rank(&self, quality: &str) -> Option<i64> {
        self.profile
            .items
            .iter()
            .position(|item| item.qualities.iter().any(|q| q == quality))
            .map(|i| i as i64)
    }

    fn allowed(&self, quality: &str) -> bool {
        self.profile
            .items
            .iter()
            .any(|item| item.allowed && item.qualities.iter().any(|q| q == quality))
    }

    /// Decides on a release, against the file it would replace when there is one. Every rule is
    /// checked, so a refusal lists all its reasons.
    pub fn decide(&self, facts: &Facts, existing: Option<&Existing>) -> Decision {
        let profile = &self.profile;
        let quality = facts.quality.name.to_owned();
        let rank = self.rank(&quality);
        let mut rejections = Vec::new();
        let (formats, score) = self.score(facts).unwrap_or_else(|err| {
            rejections.push(format!("couldn't test the custom formats: {err}"));
            (Vec::new(), 0)
        });
        if !self.allowed(&quality) {
            rejections.push(format!("{quality} isn't allowed by {}", profile.name));
        }
        if let Some(wanted) = profile.language.as_deref().filter(|l| *l != "any") {
            let wanted = match (wanted, facts.original_language) {
                ("original", Some(original)) => original,
                ("original", None) => {
                    rejections.push("the title's original language isn't known".into());
                    "original"
                }
                (code, _) => code,
            };
            if wanted != "original" && !facts.languages.iter().any(|l| l == wanted) {
                rejections.push(format!(
                    "the audio is {}; {} wants {wanted}",
                    facts.languages.join(", "),
                    profile.name
                ));
            }
        }
        if score < profile.min_format_score {
            rejections.push(format!(
                "custom format score {score} is below the minimum {}",
                profile.min_format_score
            ));
        }
        if let Some(existing) = existing
            && let Some(reason) =
                self.upgrade_rejection(rank, facts.release.proper, score, existing)
        {
            rejections.push(reason);
        }
        Decision {
            accepted: rejections.is_empty(),
            quality,
            rank,
            languages: facts.languages.clone(),
            formats,
            score,
            rejections,
        }
    }

    /// Why a release isn't an upgrade over `existing`, following Radarr.
    fn upgrade_rejection(
        &self,
        rank: Option<i64>,
        proper: u8,
        score: i64,
        existing: &Existing,
    ) -> Option<String> {
        let profile = &self.profile;
        // Qualities the profile doesn't list rank below all it does.
        let new_rank = rank.unwrap_or(-1);
        let old_rank = self.rank(&existing.quality).unwrap_or(-1);
        let quality = new_rank.cmp(&old_rank);
        let revision = proper.cmp(&existing.proper);
        // Radarr compares revisions as part of the quality here, so without upgrades not even
        // a PROPER replaces a file. (Not confirmed against Radarr; this is the side that
        // downloads less.)
        if !profile.upgrade_allowed {
            return Some(format!(
                "a file exists and {} doesn't upgrade",
                profile.name
            ));
        }
        match quality {
            Ordering::Greater if self.below_cutoff(old_rank) => return None,
            Ordering::Less => {
                return Some(format!(
                    "the existing file's quality ({}) is better",
                    existing.quality
                ));
            }
            _ => {}
        }
        match revision {
            Ordering::Greater => return None,
            Ordering::Less => return Some("the existing file is a newer revision".into()),
            Ordering::Equal => {}
        }
        if quality == Ordering::Greater {
            return Some(format!(
                "the existing file already reaches the cutoff ({})",
                profile.cutoff
            ));
        }
        if !self.below_cutoff_score(existing.score) {
            return Some(format!(
                "the existing file's score {} already reaches the upgrade-until score {}",
                existing.score, profile.cutoff_format_score
            ));
        }
        if score <= existing.score || score < existing.score + profile.min_upgrade_format_score {
            return Some(format!(
                "score {score} isn't at least {} above the existing file's {}",
                profile.min_upgrade_format_score, existing.score
            ));
        }
        None
    }

    /// A quality's rank in the profile: its item's place, lowest 0; one the profile doesn't
    /// list ranks below all it does (-1).
    pub fn rank_of(&self, quality: &str) -> i64 {
        self.rank(quality).unwrap_or(-1)
    }

    /// Whether a file of this rank is below the cutoff's quality, so a better quality is an
    /// upgrade. An unknown cutoff (imports check it) counts as reached.
    pub fn below_cutoff(&self, rank: i64) -> bool {
        rank < self.rank_of_item(&self.profile.cutoff).unwrap_or(-1)
    }

    /// Whether a file of this custom format score is below the upgrade-until score, so a
    /// higher score at the same quality is an upgrade.
    pub fn below_cutoff_score(&self, score: i64) -> bool {
        score < self.profile.cutoff_format_score
    }

    /// Whether the file on disk hasn't reached the cutoff: the profile upgrades, and the file's
    /// quality is below the cutoff's or its score below the upgrade-until score. Exactly the
    /// files [`Loaded::decide`] lets a better release replace (a newer revision aside).
    pub fn cutoff_unmet(&self, existing: &Existing) -> bool {
        self.profile.upgrade_allowed
            && (self.below_cutoff(self.rank_of(&existing.quality))
                || self.below_cutoff_score(existing.score))
    }

    fn rank_of_item(&self, name: &str) -> Option<i64> {
        self.profile
            .items
            .iter()
            .position(|item| item.name == name)
            .map(|i| i as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::profiles::import::tests::converted;
    use crate::profiles::{self, App};
    use crate::release;

    /// The profile `name` as imported from the captured Radarr.
    async fn radarr_profile(name: &str) -> Loaded {
        let pool = db::open_in_memory().await;
        profiles::save(&pool, App::Radarr, &converted(App::Radarr))
            .await
            .unwrap();
        let id = profiles::list(&pool)
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.name == name)
            .unwrap()
            .id;
        profiles::load(&pool, id).await.unwrap().unwrap()
    }

    fn decide_with(
        profile: &Loaded,
        name: &str,
        original: Option<&str>,
        existing: Option<&str>,
    ) -> Decision {
        let existing = existing.map(|file| {
            let parsed = release::parse(file);
            let facts = Facts::new(file, &parsed, None, &[], original);
            Existing {
                quality: facts.quality.name.to_owned(),
                proper: parsed.proper,
                score: profile.score(&facts).unwrap().1,
            }
        });
        let parsed = release::parse(name);
        let facts = Facts::new(name, &parsed, None, &[], original);
        profile.decide(&facts, existing.as_ref())
    }

    fn decide(profile: &Loaded, name: &str, existing: Option<&str>) -> Decision {
        decide_with(profile, name, Some("en"), existing)
    }

    #[tokio::test]
    async fn scores_with_the_profiles_formats() {
        let profile = radarr_profile("4K Preferred").await;
        let d = decide(
            &profile,
            "Movie.2023.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR",
            None,
        );
        assert!(d.accepted, "{:?}", d.rejections);
        assert_eq!(
            (d.quality.as_str(), d.score),
            ("Remux-2160p", 150 + 100 + 75)
        );
        let scored: Vec<(&str, i64)> = d
            .formats
            .iter()
            .filter(|f| f.score != 0)
            .map(|f| (f.name.as_str(), f.score))
            .collect();
        assert_eq!(scored, [("DV", 150), ("HDR10", 100), ("7.1 Surround", 75)]);
        // Formats the profile doesn't score still show, at 0.
        assert!(d.formats.iter().any(|f| f.name == "Remux" && f.score == 0));
    }

    #[tokio::test]
    async fn rejects_with_every_reason() {
        let profile = radarr_profile("4K Preferred").await;
        let d = decide(&profile, "Movie.2023.720p.WEB-DL.DDP5.1.H.264-GRP", None);
        assert_eq!(d.rejections, ["WEBDL-720p isn't allowed by 4K Preferred"]);
        let d = decide(&profile, "Movie.2023.FRENCH.1080p.WEB.H264-GRP", None);
        assert_eq!(d.rejections, ["the audio is fr; 4K Preferred wants en"]);
        // A dub in any language Radarr knows is that language, not the original's.
        for dub in [
            "POLISH",
            "PLDUB",
            "SWEDISH",
            "HUNGARIAN",
            "CZECH",
            "TURKISH",
        ] {
            let d = decide(
                &profile,
                &format!("Movie.2023.{dub}.1080p.WEB-DL.H264-GRP"),
                None,
            );
            assert!(
                !d.accepted && !d.languages.contains(&"en".to_owned()),
                "{dub}: {d:?}"
            );
        }
        let d = decide(&profile, "Movie.2023.1080p.WEB.H264.VOSTFR-GRP", None);
        assert_eq!(
            d.rejections,
            ["custom format score -9900 is below the minimum 0"]
        );
        // Without the title's original language, an unlabelled release's audio is unknown.
        let d = decide_with(
            &profile,
            "Movie.2023.1080p.WEB-DL.DDP5.1.H.264-GRP",
            None,
            None,
        );
        assert_eq!(
            d.rejections,
            ["the audio is unknown; 4K Preferred wants en"]
        );
        let d = decide(&profile, "Movie.2023.FRENCH.720p.HDTV.x264-GRP", None);
        assert_eq!(d.rejections.len(), 2, "{:?}", d.rejections);
    }

    #[tokio::test]
    async fn upgrades_follow_radarr() {
        let profile = radarr_profile("4K Preferred").await;
        let web_1080 = "Movie.2023.1080p.WEB-DL.DDP5.1.H.264-GRP";
        let web_2160_hdr = "Movie.2023.2160p.WEB-DL.DDP5.1.HDR.H.265-GRP";
        let web_2160_dv = "Movie.2023.2160p.WEB-DL.DDP5.1.DV.HDR10Plus.H.265-GRP";
        let remux_2160 = "Movie.2023.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-GRP";
        // Better quality below the cutoff.
        assert!(decide(&profile, web_2160_hdr, Some(web_1080)).accepted);
        assert!(decide(&profile, remux_2160, Some(web_2160_hdr)).accepted);
        // Nothing replaces the cutoff quality.
        let d = decide(&profile, remux_2160, Some(remux_2160));
        assert!(!d.accepted);
        // Worse quality never.
        let d = decide(&profile, web_1080, Some(web_2160_hdr));
        assert_eq!(
            d.rejections,
            ["the existing file's quality (WEBDL-2160p) is better"]
        );
        // A proper of the same quality does.
        assert!(
            decide(
                &profile,
                "Movie.2023.PROPER.1080p.WEB-DL.DDP5.1.H.264-GRP",
                Some(web_1080)
            )
            .accepted
        );
        // Same quality, higher score: the file already scores 100, past the upgrade-until
        // score of 0, so the profile stops there.
        let d = decide(&profile, web_2160_dv, Some(web_2160_hdr));
        assert_eq!(
            d.rejections,
            ["the existing file's score 100 already reaches the upgrade-until score 0"]
        );
    }

    #[tokio::test]
    async fn without_upgrades_nothing_replaces_a_file() {
        // Radarr's default "HD-1080p": no upgrades, original language.
        let profile = radarr_profile("HD-1080p").await;
        let bluray = "Movie.2023.1080p.BluRay.x264-GRP";
        assert!(decide(&profile, bluray, None).accepted);
        let d = decide(
            &profile,
            "Movie.2023.1080p.BluRay.x264.DTS-HD.MA.7.1-GRP",
            Some("Movie.2023.1080p.WEB-DL.H.264-GRP"),
        );
        assert_eq!(d.rejections, ["a file exists and HD-1080p doesn't upgrade"]);
        let d = decide(
            &profile,
            "Movie.2023.REPACK.1080p.BluRay.x264-GRP",
            Some(bluray),
        );
        assert_eq!(d.rejections, ["a file exists and HD-1080p doesn't upgrade"]);
        let d = decide_with(&profile, bluray, None, None);
        assert_eq!(d.rejections, ["the title's original language isn't known"]);
    }
}
