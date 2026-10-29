//! The anime profile preset: its formats and the "Anime" profile, made all or nothing.

use super::*;

/// The anime profile preset: 1080p, 4K when available, and by audio: dual Japanese and
/// English, then English, then Japanese with English subtitles. Raws (no subtitles) are refused.
/// Releases that don't name their subtitles still count (most fansubs don't), scoring nothing
/// for audio. Makes its formats and the profile "Anime" for `source`; returns the profile's id.
pub async fn anime_preset(pool: &SqlitePool, source: &str) -> anyhow::Result<Result<i64, Refused>> {
    let formats = anime_formats();
    let mut made = Vec::new();
    let mut scores = Vec::new();
    let mut refused = None;
    for (name, score, conditions) in formats {
        let input = FormatInput {
            source: source.into(),
            name: name.into(),
            conditions,
        };
        match create_format(pool, &input).await? {
            Ok(id) => {
                made.push(id);
                scores.push(ScoreInput {
                    format_id: id,
                    score,
                });
            }
            Err(why) => {
                refused = Some(why);
                break;
            }
        }
    }
    let created = match refused {
        Some(why) => Err(why),
        None => {
            let profile = anime_profile(source, scores);
            create_profile(pool, &profile).await?
        }
    };
    if created.is_err() {
        // All or nothing: a failed preset leaves no formats behind.
        for id in made {
            delete_format(pool, id).await?;
        }
    }
    Ok(created)
}

/// The preset's formats, by name, with their scores and conditions.
fn anime_formats() -> [(&'static str, i64, Vec<Condition>); 4] {
    use super::super::formats::Test;
    let condition = |name: &str, required: bool, test: Test| Condition {
        name: name.into(),
        negate: false,
        required,
        test,
    };
    let language = |code: &str| Test::Language {
        language: code.into(),
        except: false,
    };
    [
        (
            "Anime: dual audio",
            300,
            vec![condition(
                "Dual audio",
                false,
                Test::ReleaseTitle {
                    pattern: r"\b(dual[ ._-]?audio|jpn?[ ._+&-]+eng?|eng?[ ._+&-]+jpn?)\b".into(),
                },
            )],
        ),
        (
            "Anime: English audio",
            200,
            vec![condition("English", true, language("en"))],
        ),
        (
            "Anime: Japanese audio, English subtitles",
            100,
            vec![
                condition("Japanese", true, language("ja")),
                condition(
                    "English subtitles",
                    true,
                    Test::Subtitles {
                        language: "en".into(),
                    },
                ),
            ],
        ),
        (
            "Anime: raw",
            -10000,
            vec![condition(
                "Raw",
                false,
                Test::ReleaseTitle {
                    pattern: r"\braws?\b".into(),
                },
            )],
        ),
    ]
}

/// The "Anime" profile for `source`: 1080p, and 2160p when available, scored by `scores`.
fn anime_profile(source: &str, scores: Vec<ScoreInput>) -> ProfileInput {
    let tier = |name: &str, resolution: &str| ItemInput {
        name: name.into(),
        qualities: ["HDTV", "WEBRip", "WEBDL", "Bluray", "Remux"]
            .iter()
            .map(|kind| format!("{kind}-{resolution}"))
            .collect(),
        allowed: true,
    };
    ProfileInput {
        source: source.into(),
        name: "Anime".into(),
        upgrade_allowed: true,
        cutoff: "2160p".into(),
        min_format_score: 0,
        cutoff_format_score: 300,
        min_upgrade_format_score: 1,
        language: Some("any".into()),
        items: vec![tier("1080p", "1080p"), tier("2160p", "2160p")],
        formats: scores,
    }
}
