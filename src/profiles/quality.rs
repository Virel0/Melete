//! The qualities profiles rank, named as Radarr names them, with what custom formats test
//! about each (source, resolution, modifier).

use serde::{Deserialize, Serialize};

/// Where a quality comes from, as custom formats test it. Sonarr tells a remux (BlurayRaw)
/// and Raw-HD (TelevisionRaw) apart from the rest of their source; Radarr counts them in it,
/// so its Bluray and TV conditions are imported as both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Unknown,
    Workprint,
    Cam,
    Telesync,
    Telecine,
    Dvd,
    Tv,
    TvRaw,
    WebDl,
    WebRip,
    Bluray,
    BlurayRaw,
}

/// Radarr's quality modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modifier {
    None,
    Regional,
    Screener,
    RawHd,
    BrDisk,
    Remux,
}

/// A quality and what custom formats test about it.
pub struct Quality {
    /// Radarr's name for it, e.g. "Bluray-1080p".
    pub name: &'static str,
    /// Where it comes from.
    pub source: Source,
    /// Lines, e.g. 1080; 0 when unknown. See `resolution_in` for DVD.
    pub resolution: i64,
    /// Radarr's modifier, e.g. Remux.
    pub modifier: Modifier,
}

const fn q(name: &'static str, source: Source, resolution: i64, modifier: Modifier) -> Quality {
    Quality {
        name,
        source,
        resolution,
        modifier,
    }
}

use Modifier as M;
use Source as S;

/// Every quality, as Radarr 6 lists them, with DVD at 480p as in Sonarr (see `resolution_in`).
pub const QUALITIES: &[Quality] = &[
    q("Unknown", S::Unknown, 0, M::None),
    q("WORKPRINT", S::Workprint, 0, M::None),
    q("CAM", S::Cam, 0, M::None),
    q("TELESYNC", S::Telesync, 0, M::None),
    q("TELECINE", S::Telecine, 0, M::None),
    q("REGIONAL", S::Dvd, 480, M::Regional),
    q("DVDSCR", S::Dvd, 480, M::Screener),
    q("SDTV", S::Tv, 480, M::None),
    q("DVD", S::Dvd, 480, M::None),
    q("DVD-R", S::Dvd, 480, M::Remux),
    q("WEBDL-480p", S::WebDl, 480, M::None),
    q("WEBRip-480p", S::WebRip, 480, M::None),
    q("Bluray-480p", S::Bluray, 480, M::None),
    q("Bluray-576p", S::Bluray, 576, M::None),
    q("HDTV-720p", S::Tv, 720, M::None),
    q("WEBDL-720p", S::WebDl, 720, M::None),
    q("WEBRip-720p", S::WebRip, 720, M::None),
    q("Bluray-720p", S::Bluray, 720, M::None),
    q("HDTV-1080p", S::Tv, 1080, M::None),
    q("WEBDL-1080p", S::WebDl, 1080, M::None),
    q("WEBRip-1080p", S::WebRip, 1080, M::None),
    q("Bluray-1080p", S::Bluray, 1080, M::None),
    q("Remux-1080p", S::BlurayRaw, 1080, M::Remux),
    q("HDTV-2160p", S::Tv, 2160, M::None),
    q("WEBDL-2160p", S::WebDl, 2160, M::None),
    q("WEBRip-2160p", S::WebRip, 2160, M::None),
    q("Bluray-2160p", S::Bluray, 2160, M::None),
    q("Remux-2160p", S::BlurayRaw, 2160, M::Remux),
    q("BR-DISK", S::Bluray, 1080, M::BrDisk),
    q("Raw-HD", S::TvRaw, 1080, M::RawHd),
];

impl Quality {
    /// The resolution custom formats test. Radarr gives plain DVD none (DVD-R and DVDSCR are
    /// 480p there too); Sonarr gives it 480p.
    pub fn resolution_in(&self, app: super::App) -> i64 {
        match (app, self.name) {
            (super::App::Radarr, "DVD") => 0,
            _ => self.resolution,
        }
    }
}

/// The quality with this name, in Melete's (Radarr's) names; None for any other.
pub fn find(name: &str) -> Option<&'static Quality> {
    QUALITIES.iter().find(|q| q.name == name)
}

/// A quality as an app names it, in Melete's (Radarr's) names. Sonarr calls remuxes
/// "Bluray-1080p Remux".
pub fn from_app(name: &str) -> Option<&'static Quality> {
    match name {
        "Bluray-1080p Remux" => find("Remux-1080p"),
        "Bluray-2160p Remux" => find("Remux-2160p"),
        _ => find(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::{self, Release, Source as ReleaseSource};

    #[test]
    fn every_parsed_quality_is_a_known_quality() {
        let sources = [
            None,
            Some(ReleaseSource::Cam),
            Some(ReleaseSource::Telesync),
            Some(ReleaseSource::Telecine),
            Some(ReleaseSource::Screener),
            Some(ReleaseSource::Dvd),
            Some(ReleaseSource::Hdtv),
            Some(ReleaseSource::WebRip),
            Some(ReleaseSource::WebDl),
            Some(ReleaseSource::Bluray),
        ];
        let base: Release = release::parse("x");
        for source in sources {
            for resolution in [
                None,
                Some(480),
                Some(576),
                Some(720),
                Some(1080),
                Some(2160),
            ] {
                for (remux, disc) in [(false, false), (true, false), (false, true)] {
                    let r = Release {
                        source,
                        resolution,
                        remux,
                        disc,
                        ..base.clone()
                    };
                    let quality = r.quality();
                    assert!(find(&quality).is_some(), "{quality} from {r:?}");
                }
            }
        }
    }

    #[test]
    fn sonarr_names_map_to_radarr_names() {
        assert_eq!(from_app("Bluray-2160p Remux").unwrap().name, "Remux-2160p");
        assert_eq!(from_app("WEBDL-1080p").unwrap().name, "WEBDL-1080p");
        assert!(from_app("Bluray-4320p").is_none());
    }
}
