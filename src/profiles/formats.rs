//! Custom formats: named sets of conditions on a release, which profiles score. They behave as
//! in Radarr and Sonarr (checked against both: tests/fixtures). Conditions of one kind match
//! together when every required one holds and at least one holds; a format matches when every
//! kind does. A negated condition holds when its test fails, so a required negated title
//! pattern is enough on its own to match its kind.

use std::mem::{Discriminant, discriminant};

use serde::{Deserialize, Serialize};

use super::App;
use super::quality::{Modifier, Quality, Source};
use crate::release::{Episodes, Release};

/// What one condition of a custom format tests. Source, resolution and modifier are the
/// release's quality's; a year test's bounds are both included.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Test {
    /// A case-insensitive regex (with lookarounds, as .NET has) on the release name.
    ReleaseTitle {
        pattern: String,
    },
    /// Radarr's edition test ("Director's Cut"). Melete doesn't parse editions, so it tests
    /// the whole name.
    Edition {
        pattern: String,
    },
    /// A case-insensitive regex on the release group; no group never matches.
    ReleaseGroup {
        pattern: String,
    },
    /// An ISO 639-1 code, or "original", "unknown" or "any". With `except`, the release has a
    /// language other than this one.
    Language {
        language: String,
        except: bool,
    },
    Source {
        sources: Vec<Source>,
    },
    Resolution {
        resolution: i64,
    },
    Modifier {
        modifier: Modifier,
    },
    /// GiB: more than `min`, at most `max`.
    Size {
        min: f64,
        max: f64,
    },
    Year {
        min: i64,
        max: i64,
    },
    ReleaseType {
        release_type: ReleaseType,
    },
    /// Melete's own (Radarr and Sonarr have none): the name lists subtitles in this language
    /// (ISO 639-1), or with "any", some subtitles at all.
    Subtitles {
        language: String,
    },
    /// "freeleech", "internal", "scene", "nuked"…, as the indexer flags the release.
    IndexerFlag {
        flag: String,
    },
}

/// How many episodes a release holds, as Sonarr's release type test sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseType {
    /// No episode numbers: a daily episode, or none found.
    Unknown,
    SingleEpisode,
    MultiEpisode,
    SeasonPack,
}

/// One test in a custom format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    /// The condition's name, for messages.
    pub name: String,
    /// Holds when the test fails instead.
    pub negate: bool,
    /// Must hold for its kind to match; otherwise one of its kind holding is enough.
    pub required: bool,
    /// What it tests.
    #[serde(flatten)]
    pub test: Test,
}

/// What custom formats test about a release.
pub struct Facts<'a> {
    /// The release's name.
    pub name: &'a str,
    /// The name, parsed.
    pub release: &'a Release,
    /// Its quality; Unknown when the name gives none Melete knows.
    pub quality: &'static Quality,
    /// Bytes, when the indexer says.
    pub size: Option<u64>,
    /// What the indexer flags it as.
    pub indexer_flags: &'a [String],
    /// Audio languages: those the name gives, else the title's original language, else
    /// "unknown". Dual audio adds the original language, as Radarr does.
    pub languages: Vec<String>,
    /// The title's original language, when known.
    pub original_language: Option<&'a str>,
}

impl<'a> Facts<'a> {
    /// The facts about a release, worked out from its name and what the indexer says.
    pub fn new(
        name: &'a str,
        release: &'a Release,
        size: Option<u64>,
        indexer_flags: &'a [String],
        original_language: Option<&'a str>,
    ) -> Self {
        let quality =
            super::quality::find(&release.quality()).unwrap_or(&super::quality::QUALITIES[0]);
        let mut languages: Vec<String> = release.languages.iter().map(|l| l.to_string()).collect();
        match original_language {
            Some(original) if languages.is_empty() || release.dual_audio => {
                if !languages.iter().any(|l| l == original) {
                    languages.push(original.to_owned());
                }
            }
            None if languages.is_empty() => languages.push("unknown".into()),
            _ => {}
        }
        Facts {
            name,
            release,
            quality,
            size,
            indexer_flags,
            languages,
            original_language,
        }
    }
}

/// A custom format ready to test: its patterns compiled.
pub struct Compiled {
    /// The app the format came from, whose quirks it keeps.
    app: App,
    /// The format's name.
    pub name: String,
    conditions: Vec<(Condition, Option<fancy_regex::Regex>)>,
}

impl Compiled {
    /// Fails on a pattern that doesn't compile, naming it.
    pub fn new(app: App, name: String, conditions: Vec<Condition>) -> anyhow::Result<Self> {
        let conditions = conditions
            .into_iter()
            .map(|c| {
                let pattern = match &c.test {
                    Test::ReleaseTitle { pattern }
                    | Test::Edition { pattern }
                    | Test::ReleaseGroup { pattern } => Some(compile(pattern).map_err(|err| {
                        anyhow::anyhow!("custom format {name}: condition {}: {err}", c.name)
                    })?),
                    _ => None,
                };
                Ok((c, pattern))
            })
            .collect::<anyhow::Result<_>>()?;
        Ok(Self {
            app,
            name,
            conditions,
        })
    }

    /// Whether the format matches. Fails when a pattern gives up (too much backtracking), so
    /// the caller can refuse to decide rather than guess.
    pub fn matches(&self, facts: &Facts) -> Result<bool, String> {
        // A format without conditions matches nothing (Radarr won't save one).
        if self.conditions.is_empty() {
            return Ok(false);
        }
        let mut kinds: Vec<(Discriminant<Test>, bool, bool)> = Vec::new();
        for (condition, pattern) in &self.conditions {
            let held = holds(self.app, &condition.test, pattern.as_ref(), facts)
                .map_err(|err| format!("custom format {}: {err}", self.name))?
                != condition.negate;
            let kind = discriminant(&condition.test);
            let index = match kinds.iter().position(|(k, _, _)| *k == kind) {
                Some(i) => i,
                None => {
                    kinds.push((kind, false, true));
                    kinds.len() - 1
                }
            };
            let (_, any_held, required_held) = &mut kinds[index];
            *any_held |= held;
            *required_held &= held || !condition.required;
        }
        Ok(kinds.iter().all(|(_, any, required)| *any && *required))
    }
}

/// A custom format pattern as a case-insensitive regex, with lookarounds as .NET has them.
///
/// # Errors
/// When the pattern isn't a valid regex.
pub fn compile(pattern: &str) -> Result<fancy_regex::Regex, fancy_regex::Error> {
    fancy_regex::Regex::new(&format!("(?i){pattern}"))
}

fn holds(
    app: App,
    test: &Test,
    pattern: Option<&fancy_regex::Regex>,
    facts: &Facts,
) -> Result<bool, String> {
    let find = |text: &str| {
        // Only `Compiled::new` builds the list this comes from, and it compiles every one.
        let pattern = pattern.expect("pattern tests always carry their compiled pattern");
        pattern.is_match(text).map_err(|err| err.to_string())
    };
    Ok(match test {
        Test::ReleaseTitle { .. } | Test::Edition { .. } => find(facts.name)?,
        Test::ReleaseGroup { .. } => match &facts.release.group {
            Some(group) => find(group)?,
            None => false,
        },
        Test::Language { language, except } => {
            if language == "any" {
                return Ok(true);
            }
            let wanted = match (language.as_str(), facts.original_language) {
                ("original", Some(original)) => original,
                _ => language.as_str(),
            };
            match except {
                true => facts.languages.iter().any(|l| l != wanted),
                false => facts.languages.iter().any(|l| l == wanted),
            }
        }
        Test::Source { sources } => sources.contains(&facts.quality.source),
        Test::Resolution { resolution } => facts.quality.resolution_in(app) == *resolution,
        Test::Modifier { modifier } => facts.quality.modifier == *modifier,
        Test::Size { min, max } => match facts.size {
            Some(bytes) => {
                let gib = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                gib > *min && gib <= *max
            }
            None => false,
        },
        Test::Year { min, max } => facts
            .release
            .year
            .is_some_and(|y| (*min..=*max).contains(&y)),
        Test::ReleaseType { release_type } => {
            *release_type == self::release_type(&facts.release.episodes)
        }
        Test::IndexerFlag { flag } => facts.indexer_flags.contains(flag),
        Test::Subtitles { language } => match language.as_str() {
            "any" => !facts.release.subtitles.is_empty() || facts.release.multi_subs,
            code => facts.release.subtitles.contains(&code),
        },
    })
}

fn release_type(episodes: &Episodes) -> ReleaseType {
    match episodes {
        Episodes::Standard { episodes, .. } | Episodes::Absolute(episodes)
            if episodes.len() > 1 =>
        {
            ReleaseType::MultiEpisode
        }
        Episodes::Standard { .. } | Episodes::Absolute(_) => ReleaseType::SingleEpisode,
        Episodes::Seasons(_) | Episodes::CompleteSeries => ReleaseType::SeasonPack,
        // Sonarr counts episode numbers, and a daily episode has none.
        Episodes::Daily(_) | Episodes::None => ReleaseType::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::parse;

    fn condition(test: Test, negate: bool, required: bool) -> Condition {
        Condition {
            name: "c".into(),
            negate,
            required,
            test,
        }
    }

    fn title(pattern: &str) -> Test {
        Test::ReleaseTitle {
            pattern: pattern.into(),
        }
    }

    fn matches(conditions: Vec<Condition>, name: &str) -> bool {
        let release = parse(name);
        let facts = Facts::new(name, &release, None, &[], None);
        Compiled::new(App::Radarr, "f".into(), conditions)
            .unwrap()
            .matches(&facts)
            .unwrap()
    }

    #[test]
    fn kinds_combine_with_and_conditions_of_a_kind_with_or() {
        let either = || {
            vec![
                condition(title(r"\bproper\b"), false, false),
                condition(title(r"\brepack\b"), false, false),
            ]
        };
        assert!(matches(either(), "Movie.2020.REPACK.1080p.WEB-DL-GRP"));
        assert!(!matches(either(), "Movie.2020.1080p.WEB-DL-GRP"));
        let mut both = either();
        both.push(condition(
            Test::Resolution { resolution: 2160 },
            false,
            false,
        ));
        assert!(!matches(both.clone(), "Movie.2020.REPACK.1080p.WEB-DL-GRP"));
        assert!(matches(both, "Movie.2020.REPACK.2160p.WEB-DL-GRP"));
    }

    #[test]
    fn a_required_negated_condition_matches_its_kind_alone() {
        // Radarr's behaviour, confirmed against it: "HDR10" below matches names without HDR.
        let hdr10 = || {
            vec![
                condition(title(r"\bhdr(10)?\b"), false, false),
                condition(title(r"\bhdr10(\+|p(lus)?)\b"), true, true),
            ]
        };
        assert!(matches(hdr10(), "Movie.2020.2160p.WEB-DL.HDR.H.265-GRP"));
        assert!(matches(hdr10(), "Movie.2020.1080p.WEB-DL.H.264-GRP"));
        assert!(!matches(
            hdr10(),
            "Movie.2020.2160p.WEB-DL.HDR10Plus.H.265-GRP"
        ));
    }

    #[test]
    fn patterns_are_case_insensitive_and_may_look_around() {
        let lookahead = vec![condition(
            title(r"^(?!.*\bx264\b).*\b1080P\b"),
            false,
            false,
        )];
        assert!(matches(
            lookahead.clone(),
            "Movie.2020.1080p.WEB-DL.H.264-GRP"
        ));
        assert!(!matches(lookahead, "Movie.2020.1080p.BluRay.x264-GRP"));
        assert!(
            Compiled::new(
                App::Radarr,
                "bad".into(),
                vec![condition(title("(unclosed"), false, false)]
            )
            .is_err()
        );
    }

    #[test]
    fn languages_fall_back_to_the_original() {
        let english = || {
            vec![condition(
                Test::Language {
                    language: "en".into(),
                    except: false,
                },
                false,
                false,
            )]
        };
        let name = "Movie.2020.1080p.WEB-DL-GRP";
        let release = parse(name);
        let compiled = Compiled::new(App::Radarr, "en".into(), english()).unwrap();
        let facts = Facts::new(name, &release, None, &[], Some("en"));
        assert!(compiled.matches(&facts).unwrap());
        let facts = Facts::new(name, &release, None, &[], None);
        assert!(!compiled.matches(&facts).unwrap(), "unknown isn't English");
        let except = vec![condition(
            Test::Language {
                language: "en".into(),
                except: true,
            },
            false,
            false,
        )];
        assert!(matches(
            except.clone(),
            "Movie.2020.FRENCH.ENGLISH.1080p.BluRay.x264-GRP"
        ));
        assert!(!matches(except, "Movie.2020.ENGLISH.1080p.BluRay.x264-GRP"));
    }

    #[test]
    fn sizes_and_release_types() {
        let name = "Show.S01.1080p.BluRay.x264-GRP";
        let release = parse(name);
        let gib = 1024 * 1024 * 1024;
        let size = |min, max| vec![condition(Test::Size { min, max }, false, false)];
        let compiled = Compiled::new(App::Radarr, "s".into(), size(1.0, 10.0)).unwrap();
        for (bytes, expected) in [
            (Some(5 * gib), true),
            (Some(gib), false),
            (Some(10 * gib), true),
            (None, false),
        ] {
            let facts = Facts::new(name, &release, bytes, &[], None);
            assert_eq!(compiled.matches(&facts).unwrap(), expected, "{bytes:?}");
        }
        let pack = vec![condition(
            Test::ReleaseType {
                release_type: ReleaseType::SeasonPack,
            },
            false,
            false,
        )];
        assert!(matches(pack.clone(), name));
        assert!(!matches(pack, "Show.S01E01.1080p.BluRay.x264-GRP"));
    }
}
