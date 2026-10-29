//! Turning Radarr's and Sonarr's custom formats and quality profiles into Melete's. Anything
//! Melete can't test exactly as the app would (an unknown condition type, a pattern its regex
//! engine can't compile, an unknown quality) fails the import with its name, rather than being
//! dropped: a missing -10000 format would quietly let through what the profile was built to
//! keep out.

use std::collections::HashMap;

use anyhow::{Context, anyhow, bail};
use serde_json::Value;

use super::formats::{Condition, ReleaseType, Test, compile};
use super::quality::{self, Modifier, Source};
use super::{App, NewProfile, RankedQuality};
use crate::arr;

/// The app has something Melete can't import faithfully.
#[derive(Debug)]
pub struct Unsupported(pub String);

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unsupported {}

/// An app's custom formats and quality profiles in Melete's terms, ready to save.
#[derive(Debug)]
pub struct Converted {
    /// Each custom format's name and conditions.
    pub formats: Vec<(String, Vec<Condition>)>,
    /// The quality profiles.
    pub profiles: Vec<NewProfile>,
}

/// Converts what an app lists into Melete's terms.
///
/// # Errors
/// When a format or profile uses something Melete doesn't support or can't understand; the
/// caller turns that into [`Unsupported`].
pub fn convert(
    app: App,
    languages: &[arr::IdName],
    formats: &[arr::CustomFormat],
    profiles: &[arr::FullQualityProfile],
) -> anyhow::Result<Converted> {
    let languages: HashMap<i64, &str> = languages.iter().map(|l| (l.id, l.name.as_str())).collect();
    let converted_formats = formats
        .iter()
        .map(|format| {
            let conditions = format
                .specifications
                .iter()
                .map(|spec| {
                    condition(app, &languages, spec).with_context(|| {
                        format!("custom format {}, condition {}", format.name, spec.name)
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok((format.name.clone(), conditions))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let format_names: HashMap<i64, &str> =
        formats.iter().map(|f| (f.id, f.name.as_str())).collect();
    let converted_profiles = profiles
        .iter()
        .map(|p| {
            profile(&languages, &format_names, p)
                .with_context(|| format!("quality profile {}", p.name))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(Converted {
        formats: converted_formats,
        profiles: converted_profiles,
    })
}

fn field<'a>(spec: &'a arr::Specification, name: &str) -> Option<&'a Value> {
    spec.fields
        .iter()
        .find(|f| f.name == name)
        .map(|f| &f.value)
}

fn int(spec: &arr::Specification, name: &str) -> anyhow::Result<i64> {
    let value = field(spec, name).ok_or_else(|| anyhow!("no {name}"))?;
    value
        .as_i64()
        .or_else(|| {
            value
                .as_f64()
                .filter(|f| f.fract() == 0.0)
                .map(|f| f as i64)
        })
        .ok_or_else(|| anyhow!("{name} isn't a whole number: {value}"))
}

fn condition(
    app: App,
    languages: &HashMap<i64, &str>,
    spec: &arr::Specification,
) -> anyhow::Result<Condition> {
    let text = |name: &str| -> anyhow::Result<String> {
        let value = field(spec, name)
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("no {name}"))?;
        compile(value)
            .map_err(|err| anyhow!("Melete can't compile the pattern {value:?}: {err}"))?;
        Ok(value.to_owned())
    };
    let test = match (spec.implementation.as_str(), app) {
        ("ReleaseTitleSpecification", _) => Test::ReleaseTitle {
            pattern: text("value")?,
        },
        ("EditionSpecification", App::Radarr) => Test::Edition {
            pattern: text("value")?,
        },
        ("ReleaseGroupSpecification", _) => Test::ReleaseGroup {
            pattern: text("value")?,
        },
        ("LanguageSpecification", _) => {
            let id = int(spec, "value")?;
            let except = field(spec, "exceptLanguage")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Test::Language {
                language: language(languages, id)?,
                except,
            }
        }
        ("SourceSpecification", _) => Test::Source {
            sources: sources(app, int(spec, "value")?)?,
        },
        ("ResolutionSpecification", _) => {
            let resolution = int(spec, "value")?;
            if ![0, 360, 480, 540, 576, 720, 1080, 2160].contains(&resolution) {
                bail!("unknown resolution {resolution}");
            }
            Test::Resolution { resolution }
        }
        ("QualityModifierSpecification", App::Radarr) => {
            let modifier = match int(spec, "value")? {
                0 => Modifier::None,
                1 => Modifier::Regional,
                2 => Modifier::Screener,
                3 => Modifier::RawHd,
                4 => Modifier::BrDisk,
                5 => Modifier::Remux,
                other => bail!("unknown quality modifier {other}"),
            };
            Test::Modifier { modifier }
        }
        ("SizeSpecification", _) => {
            let number = |name| {
                field(spec, name)
                    .and_then(Value::as_f64)
                    .ok_or_else(|| anyhow!("no {name}"))
            };
            Test::Size {
                min: number("min")?,
                max: number("max")?,
            }
        }
        ("YearSpecification", App::Radarr) => Test::Year {
            min: int(spec, "min")?,
            max: int(spec, "max")?,
        },
        ("ReleaseTypeSpecification", App::Sonarr) => {
            let release_type = match int(spec, "value")? {
                0 => ReleaseType::Unknown,
                1 => ReleaseType::SingleEpisode,
                2 => ReleaseType::MultiEpisode,
                3 => ReleaseType::SeasonPack,
                other => bail!("unknown release type {other}"),
            };
            Test::ReleaseType { release_type }
        }
        ("IndexerFlagSpecification", _) => Test::IndexerFlag {
            flag: indexer_flag(app, int(spec, "value")?)?.into(),
        },
        (other, _) => bail!("Melete can't test {other} conditions"),
    };
    Ok(Condition {
        name: spec.name.clone(),
        negate: spec.negate,
        required: spec.required,
        test,
    })
}

/// The app's source numbers. Radarr counts remuxes as Blu-ray and Raw-HD as TV; Sonarr keeps
/// them apart (BlurayRaw, TelevisionRaw).
fn sources(app: App, value: i64) -> anyhow::Result<Vec<Source>> {
    use Source as S;
    Ok(match (app, value) {
        (_, 0) => vec![S::Unknown],
        (App::Radarr, 1) => vec![S::Cam],
        (App::Radarr, 2) => vec![S::Telesync],
        (App::Radarr, 3) => vec![S::Telecine],
        (App::Radarr, 4) => vec![S::Workprint],
        (App::Radarr, 5) | (App::Sonarr, 5) => vec![S::Dvd],
        (App::Radarr, 6) => vec![S::Tv, S::TvRaw],
        (App::Radarr, 7) | (App::Sonarr, 3) => vec![S::WebDl],
        (App::Radarr, 8) | (App::Sonarr, 4) => vec![S::WebRip],
        (App::Radarr, 9) => vec![S::Bluray, S::BlurayRaw],
        (App::Sonarr, 1) => vec![S::Tv],
        (App::Sonarr, 2) => vec![S::TvRaw],
        (App::Sonarr, 6) => vec![S::Bluray],
        (App::Sonarr, 7) => vec![S::BlurayRaw],
        (_, other) => bail!("unknown source {other}"),
    })
}

fn indexer_flag(app: App, value: i64) -> anyhow::Result<&'static str> {
    Ok(match (app, value) {
        (_, 1) => "freeleech",
        (_, 2) => "halfleech",
        (_, 4) => "double_upload",
        (App::Radarr, 8) => "ptp_golden",
        (App::Radarr, 16) => "ptp_approved",
        (App::Radarr, 32) | (App::Sonarr, 8) => "internal",
        (App::Radarr, 128) | (App::Sonarr, 16) => "scene",
        (App::Radarr, 256) | (App::Sonarr, 32) => "freeleech75",
        (App::Radarr, 512) | (App::Sonarr, 64) => "freeleech25",
        (App::Radarr, 2048) | (App::Sonarr, 128) => "nuked",
        (_, other) => bail!("unknown indexer flag {other}"),
    })
}

/// An app's language id as Melete names it. The apps number languages differently (Arabic is
/// 31 in Radarr and 26 in Sonarr), so ids go through the app's own list to a name.
fn language(languages: &HashMap<i64, &str>, id: i64) -> anyhow::Result<String> {
    match id {
        -1 => return Ok("any".into()),
        -2 => return Ok("original".into()),
        0 => return Ok("unknown".into()),
        _ => {}
    }
    let name = languages
        .get(&id)
        .ok_or_else(|| anyhow!("unknown language {id}"))?;
    LANGUAGE_CODES
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, code)| (*code).to_owned())
        .ok_or_else(|| anyhow!("Melete doesn't know the language {name}"))
}

/// A language as the apps name it ("English", "Portuguese (Brazil)"), as Melete's code.
pub fn language_code(name: &str) -> Option<&'static str> {
    LANGUAGE_CODES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, code)| *code)
}

/// Every language Radarr 6 and Sonarr 4 list, as ISO 639-1 codes (with a region where the
/// apps split one).
const LANGUAGE_CODES: &[(&str, &str)] = &[
    ("Afrikaans", "af"),
    ("Albanian", "sq"),
    ("Arabic", "ar"),
    ("Bengali", "bn"),
    ("Bosnian", "bs"),
    ("Bulgarian", "bg"),
    ("Catalan", "ca"),
    ("Chinese", "zh"),
    ("Croatian", "hr"),
    ("Czech", "cs"),
    ("Danish", "da"),
    ("Dutch", "nl"),
    ("English", "en"),
    ("Estonian", "et"),
    ("Finnish", "fi"),
    ("Flemish", "nl-be"),
    ("French", "fr"),
    ("Georgian", "ka"),
    ("German", "de"),
    ("Greek", "el"),
    ("Hebrew", "he"),
    ("Hindi", "hi"),
    ("Hungarian", "hu"),
    ("Icelandic", "is"),
    ("Indonesian", "id"),
    ("Italian", "it"),
    ("Japanese", "ja"),
    ("Kannada", "kn"),
    ("Korean", "ko"),
    ("Latvian", "lv"),
    ("Lithuanian", "lt"),
    ("Macedonian", "mk"),
    ("Malayalam", "ml"),
    ("Marathi", "mr"),
    ("Mongolian", "mn"),
    ("Norwegian", "no"),
    ("Persian", "fa"),
    ("Polish", "pl"),
    ("Portuguese", "pt"),
    ("Portuguese (Brazil)", "pt-br"),
    ("Romanian", "ro"),
    ("Romansh", "rm"),
    ("Russian", "ru"),
    ("Serbian", "sr"),
    ("Slovak", "sk"),
    ("Slovenian", "sl"),
    ("Spanish", "es"),
    ("Spanish (Latino)", "es-419"),
    ("Swedish", "sv"),
    ("Tagalog", "tl"),
    ("Tamil", "ta"),
    ("Telugu", "te"),
    ("Thai", "th"),
    ("Turkish", "tr"),
    ("Ukrainian", "uk"),
    ("Urdu", "ur"),
    ("Vietnamese", "vi"),
];

fn profile(
    languages: &HashMap<i64, &str>,
    format_names: &HashMap<i64, &str>,
    p: &arr::FullQualityProfile,
) -> anyhow::Result<NewProfile> {
    let mut qualities = Vec::new();
    let mut cutoff = None;
    for (rank, item) in p.items.iter().enumerate() {
        let rank = rank as i64;
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
        for member in members {
            let known = quality::from_app(&member.name)
                .ok_or_else(|| anyhow!("unknown quality {}", member.name))?;
            let is_cutoff = match &group {
                Some(_) => item.id == Some(p.cutoff),
                None => member.id == p.cutoff,
            };
            if is_cutoff {
                cutoff = Some(group.clone().unwrap_or_else(|| known.name.to_owned()));
            }
            qualities.push(RankedQuality {
                quality: known.name.to_owned(),
                rank,
                group: group.clone(),
                allowed: item.allowed,
            });
        }
    }
    let cutoff =
        cutoff.ok_or_else(|| anyhow!("the cutoff ({}) isn't one of its qualities", p.cutoff))?;
    let scores = p
        .format_items
        .iter()
        .map(|item| {
            let name = format_names
                .get(&item.format)
                .ok_or_else(|| anyhow!("unknown custom format {}", item.format))?;
            Ok(((*name).to_owned(), item.score))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let language = p
        .language
        .as_ref()
        .map(|l| language(languages, l.id))
        .transpose()?;
    Ok(NewProfile {
        name: p.name.clone(),
        upgrade_allowed: p.upgrade_allowed,
        cutoff,
        min_format_score: p.min_format_score,
        cutoff_format_score: p.cutoff_format_score,
        min_upgrade_format_score: p.min_upgrade_format_score,
        language,
        qualities,
        scores,
    })
}

#[cfg(test)]
pub mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::profiles::formats::{Compiled, Facts};
    use crate::release;

    /// What Radarr 6.4 and Sonarr 4.0 sent, and how they matched their formats against names.
    #[derive(Deserialize)]
    pub struct Fixture {
        pub languages: Vec<arr::IdName>,
        pub customformats: Vec<arr::CustomFormat>,
        pub qualityprofiles: Vec<arr::FullQualityProfile>,
        pub parse: Vec<Parsed>,
    }

    #[derive(Deserialize)]
    pub struct Parsed {
        pub title: String,
        pub quality: String,
        pub formats: Vec<String>,
    }

    pub fn fixture(app: App) -> Fixture {
        let text = match app {
            App::Radarr => include_str!("../../tests/fixtures/radarr.json"),
            App::Sonarr => include_str!("../../tests/fixtures/sonarr.json"),
        };
        serde_json::from_str(text).unwrap()
    }

    pub fn converted(app: App) -> Converted {
        let f = fixture(app);
        convert(app, &f.languages, &f.customformats, &f.qualityprofiles).unwrap()
    }

    #[test]
    fn custom_formats_match_what_radarr_and_sonarr_matched() {
        // Where Melete reads the name differently on purpose: it reads JPN as Japanese audio
        // (Sonarr only finds ENG).
        let known = ["[Group] Show - 05 [1080p][JPN+ENG]"];
        for app in [App::Radarr, App::Sonarr] {
            let formats: Vec<Compiled> = converted(app)
                .formats
                .into_iter()
                .map(|(name, conditions)| Compiled::new(app, name, conditions).unwrap())
                .collect();
            let mut differences = Vec::new();
            for row in fixture(app).parse {
                let parsed = release::parse(&row.title);
                let facts = Facts::new(&row.title, &parsed, None, &[], None);
                let mut matched: Vec<String> = formats
                    .iter()
                    .filter(|f| f.matches(&facts).unwrap())
                    .map(|f| f.name.clone())
                    .collect();
                matched.sort();
                let quality = quality::from_app(&row.quality).unwrap().name;
                let differs = matched != row.formats || facts.quality.name != quality;
                if differs != known.contains(&row.title.as_str()) {
                    differences.push(format!(
                        "{}: Melete {} {matched:?}, {} {quality} {:?}",
                        row.title,
                        facts.quality.name,
                        app.name(),
                        row.formats
                    ));
                }
            }
            assert!(differences.is_empty(), "{differences:#?}");
        }
    }

    #[test]
    fn profiles_keep_their_order_groups_cutoff_scores_and_language() {
        let expected_allowed = [
            "HDTV-1080p",
            "WEBDL-1080p",
            "WEBRip-1080p",
            "Bluray-1080p",
            "Remux-1080p",
            "HDTV-2160p",
            "WEBDL-2160p",
            "WEBRip-2160p",
            "Bluray-2160p",
            "Remux-2160p",
        ];
        for (app, language) in [(App::Radarr, Some("en")), (App::Sonarr, None)] {
            let converted = converted(app);
            let p = converted
                .profiles
                .iter()
                .find(|p| p.name == "4K Preferred")
                .unwrap();
            assert_eq!(p.cutoff, "Remux-2160p");
            assert!(p.upgrade_allowed);
            assert_eq!(p.language.as_deref(), language);
            assert_eq!(
                (
                    p.min_format_score,
                    p.cutoff_format_score,
                    p.min_upgrade_format_score
                ),
                (0, 0, 1)
            );
            let mut allowed: Vec<&RankedQuality> =
                p.qualities.iter().filter(|q| q.allowed).collect();
            allowed.sort_by_key(|q| q.rank);
            let mut names: Vec<&str> = allowed.iter().map(|q| q.quality.as_str()).collect();
            names.sort_by_key(|n| expected_allowed.iter().position(|e| e == n));
            assert_eq!(names, expected_allowed, "{}", app.name());
            // Both web qualities of a resolution share their group's rank.
            let web: Vec<&RankedQuality> = p
                .qualities
                .iter()
                .filter(|q| q.group.as_deref() == Some("WEB 1080p"))
                .collect();
            assert_eq!(web.len(), 2);
            assert_eq!(web[0].rank, web[1].rank);
            let remux_1080 = p
                .qualities
                .iter()
                .find(|q| q.quality == "Remux-1080p")
                .unwrap();
            assert!(web[0].rank < remux_1080.rank);
            let mut scores: Vec<(&str, i64)> = p
                .scores
                .iter()
                .filter(|(_, s)| *s != 0)
                .map(|(n, s)| (n.as_str(), *s))
                .collect();
            scores.sort();
            assert_eq!(
                scores,
                [
                    ("5.1 Surround", 25),
                    ("7.1 Surround", 75),
                    ("DV", 150),
                    ("HDR10", 100),
                    ("VOSTFR", -10000)
                ]
            );
        }
    }

    #[test]
    fn languages_go_through_each_apps_numbering() {
        // French is 2 in both apps, but Arabic is 31 in Radarr and 26 in Sonarr.
        let radarr = fixture(App::Radarr);
        let ids: HashMap<i64, &str> = radarr
            .languages
            .iter()
            .map(|l| (l.id, l.name.as_str()))
            .collect();
        assert_eq!(language(&ids, 31).unwrap(), "ar");
        let sonarr = fixture(App::Sonarr);
        let ids: HashMap<i64, &str> = sonarr
            .languages
            .iter()
            .map(|l| (l.id, l.name.as_str()))
            .collect();
        assert_eq!(language(&ids, 26).unwrap(), "ar");
        assert_eq!(language(&ids, -2).unwrap(), "original");
        for l in radarr
            .languages
            .iter()
            .chain(&sonarr.languages)
            .filter(|l| l.id > 0)
        {
            assert!(
                LANGUAGE_CODES.iter().any(|(name, _)| *name == l.name),
                "{}",
                l.name
            );
        }
    }

    #[test]
    fn the_parser_reads_every_language_conditions_can_name() {
        // A condition on a language the parser can't read would never match, and a release in
        // it would pass as the original language.
        for (name, code) in LANGUAGE_CODES {
            let tag = match *code {
                "pt-br" => "PT-BR".to_owned(),
                "es-419" => "LATINO".to_owned(),
                _ => name.to_uppercase(),
            };
            let title = format!("Movie.2023.{tag}.1080p.WEB-DL.H264-GRP");
            assert_eq!(release::parse(&title).languages, [*code], "{title}");
        }
    }

    fn with_format(app: App, specification: serde_json::Value) -> anyhow::Result<Converted> {
        let f = fixture(app);
        let format: arr::CustomFormat = serde_json::from_value(serde_json::json!({
            "id": 999, "name": "Odd", "specifications": [specification]
        }))
        .unwrap();
        convert(app, &f.languages, &[format], &[])
    }

    #[test]
    fn refuses_what_it_cant_test_faithfully() {
        let spec = |implementation: &str, value: serde_json::Value| {
            serde_json::json!({ "name": "c", "implementation": implementation, "negate": false,
                                "required": false, "fields": [{ "name": "value", "value": value }] })
        };
        let err = with_format(App::Radarr, spec("SomeFutureSpecification", 1.into())).unwrap_err();
        assert!(
            format!("{err:#}").contains("Odd")
                && format!("{err:#}").contains("SomeFutureSpecification"),
            "{err:#}"
        );
        let err = with_format(
            App::Radarr,
            spec("ReleaseTitleSpecification", "(unclosed".into()),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("can't compile"), "{err:#}");
        // Sonarr has no quality modifiers; Radarr has no release types.
        assert!(with_format(App::Sonarr, spec("QualityModifierSpecification", 5.into())).is_err());
        assert!(with_format(App::Radarr, spec("ReleaseTypeSpecification", 3.into())).is_err());
        assert!(with_format(App::Radarr, spec("SourceSpecification", 42.into())).is_err());
        // Each app's source numbers: 6 is TV in Radarr and Blu-ray in Sonarr.
        let source = |app| match &with_format(app, spec("SourceSpecification", 6.into()))
            .unwrap()
            .formats[0]
            .1[0]
            .test
        {
            Test::Source { sources } => sources.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(source(App::Radarr), [Source::Tv, Source::TvRaw]);
        assert_eq!(source(App::Sonarr), [Source::Bluray]);

        let f = fixture(App::Radarr);
        let mut profile: arr::FullQualityProfile = serde_json::from_value(serde_json::json!({
            "name": "P", "upgradeAllowed": true, "cutoff": 1,
            "items": [{ "quality": { "id": 1, "name": "Bluray-4320p" }, "items": [], "allowed": true }]
        }))
        .unwrap();
        let err = convert(
            App::Radarr,
            &f.languages,
            &[],
            std::slice::from_ref(&profile),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("Bluray-4320p"), "{err:#}");
        profile.items[0].quality = Some(arr::IdName {
            id: 1,
            name: "SDTV".into(),
        });
        profile.cutoff = 7;
        let err = convert(App::Radarr, &f.languages, &[], &[profile]).unwrap_err();
        assert!(format!("{err:#}").contains("cutoff"), "{err:#}");
    }
}
