//! Metadata in a chosen language (`MELETE_METADATA_LANGUAGE`, a TMDB language with its region
//! such as `fr-FR`), with English where TMDB has no translation. Only details, seasons and
//! images are asked for in that language; searches and lookups stay in English, as matching
//! compares folder names against them.

use super::*;

/// The language everything was asked for in before the setting existed, and the fallback.
pub const ENGLISH: &str = "en-US";

/// Details, seasons and images in the metadata language.
impl Client {
    /// The client, asking for details, seasons and images in `language` (a TMDB language,
    /// such as `fr-FR`) with English where there's no translation. Searches stay in English.
    pub fn with_language(mut self, language: String) -> Self {
        self.language = language;
        self
    }

    /// The language details are asked for in.
    pub fn language(&self) -> &str {
        &self.language
    }

    /// `None` when TMDB has no movie with this id. In the metadata language, with English
    /// where it has no translation.
    pub async fn movie(&self, id: i64) -> anyhow::Result<Option<MovieDetails>> {
        let translations: &[(&str, &str)] = if self.language == ENGLISH {
            &[]
        } else {
            &[("append_to_response", "translations")]
        };
        let found: Option<MovieDetails> = self
            .get_in(&self.language, &format!("/movie/{id}"), translations)
            .await?;
        Ok(found.map(|mut d| {
            language::fill_movie(&mut d, &self.language);
            d
        }))
    }

    /// A movie's (or with `tv`, a series') backdrops and logos in the metadata language,
    /// English or no language; `None` when TMDB has no such title.
    pub async fn images(&self, tv: bool, id: i64) -> anyhow::Result<Option<ImageList>> {
        let kind = if tv { "tv" } else { "movie" };
        let code = language::code(&self.language);
        let languages = if code == "en" {
            "en,null".to_owned()
        } else {
            format!("{code},en,null")
        };
        self.get(
            &format!("/{kind}/{id}/images"),
            &[("include_image_language", &languages)],
        )
        .await
    }

    /// `None` when TMDB has no series with this id. In the metadata language, with English
    /// where it has no translation.
    pub async fn series(&self, id: i64) -> anyhow::Result<Option<SeriesDetails>> {
        let append = if self.language == ENGLISH {
            "external_ids"
        } else {
            "external_ids,translations"
        };
        let found: Option<SeriesDetails> = self
            .get_in(
                &self.language,
                &format!("/tv/{id}"),
                &[("append_to_response", append)],
            )
            .await?;
        Ok(found.map(|mut d| {
            language::fill_series(&mut d, &self.language);
            d
        }))
    }

    /// Fetches the given seasons of a series, up to 20 per request, in the metadata language;
    /// episodes it has no name or summary for get English ones, asked for once more.
    pub async fn seasons(&self, series_id: i64, numbers: &[i64]) -> anyhow::Result<Vec<Season>> {
        let mut seasons = self.seasons_in(&self.language, series_id, numbers).await?;
        if self.language != ENGLISH && language::lacking(&seasons) {
            let english = self.seasons_in(ENGLISH, series_id, numbers).await?;
            language::fill_episodes(&mut seasons, &english);
        }
        Ok(seasons)
    }

    async fn seasons_in(
        &self,
        language: &str,
        series_id: i64,
        numbers: &[i64],
    ) -> anyhow::Result<Vec<Season>> {
        let mut seasons = Vec::with_capacity(numbers.len());
        for chunk in numbers.chunks(MAX_APPENDS) {
            let append = chunk
                .iter()
                .map(|n| format!("season/{n}"))
                .collect::<Vec<_>>()
                .join(",");
            let Some(body) = self
                .get_in::<Value>(
                    language,
                    &format!("/tv/{series_id}"),
                    &[("append_to_response", &append)],
                )
                .await?
            else {
                bail!("TMDB series {series_id} disappeared while fetching its seasons");
            };
            for n in chunk {
                // A missing season would otherwise silently drop its episodes from the cache.
                let Some(season) = body.get(format!("season/{n}")) else {
                    bail!("TMDB left season {n} out of series {series_id}");
                };
                seasons.push(
                    Season::deserialize(season)
                        .with_context(|| format!("decoding season {n} of series {series_id}"))?,
                );
            }
        }
        Ok(seasons)
    }
}

/// Whether `value` is a language as TMDB takes it: ISO 639-1, with an ISO 3166-1 region or
/// not (`fr`, `fr-FR`, `pt-BR`).
pub fn is_language(value: &str) -> bool {
    let b = value.as_bytes();
    let code = |s: &[u8]| s.iter().all(u8::is_ascii_lowercase);
    match b.len() {
        2 => code(b),
        5 => code(&b[..2]) && b[2] == b'-' && b[3..].iter().all(u8::is_ascii_uppercase),
        _ => false,
    }
}

/// The language part of a TMDB language (`fr` of `fr-FR`).
pub fn code(language: &str) -> &str {
    language.split('-').next().unwrap_or(language)
}

fn empty(text: &Option<String>) -> bool {
    text.as_deref().is_none_or(|t| t.trim().is_empty())
}

/// The translation in `language`, and the English one (the US one first).
fn pick<'a>(
    translations: &'a Option<Translations>,
    language: &str,
) -> (Option<&'a Translation>, Option<&'a Translation>) {
    let all = translations
        .as_ref()
        .map_or(&[][..], |t| t.translations.as_slice());
    let (code, region) = language.split_once('-').unwrap_or((language, ""));
    let mine = all
        .iter()
        .find(|t| t.iso_639_1 == code && (region.is_empty() || t.iso_3166_1 == region));
    let english = all
        .iter()
        .find(|t| t.iso_639_1 == "en" && t.iso_3166_1 == "US")
        .or_else(|| all.iter().find(|t| t.iso_639_1 == "en"));
    (mine, english)
}

/// Fills a title and overview TMDB has no translation of from English, and says which.
/// A title stays as TMDB gave it when the title is in its own language (the original's) or
/// the translation has one.
fn fill(
    title: &mut String,
    overview: &mut Option<String>,
    original_language: Option<&str>,
    translations: &Option<Translations>,
    language: &str,
) -> Vec<&'static str> {
    let (mine, english) = pick(translations, language);
    let Some(english) = english else {
        return Vec::new();
    };
    let english_title = english.data.title.clone().or(english.data.name.clone());
    let translated = mine.is_some_and(|t| !empty(&t.data.title) || !empty(&t.data.name));
    let mut filled = Vec::new();
    if !translated && original_language != Some(code(language)) && !empty(&english_title) {
        *title = english_title.unwrap_or_default();
        filled.push("title");
    }
    if empty(overview) && !empty(&english.data.overview) {
        *overview = english.data.overview.clone();
        filled.push("overview");
    }
    filled
}

/// Fills what a movie's details lack in their language from English.
pub(super) fn fill_movie(d: &mut MovieDetails, language: &str) {
    d.language = language.to_owned();
    if language == ENGLISH {
        return;
    }
    let original = d.original_language.clone();
    d.from_english = fill(
        &mut d.title,
        &mut d.overview,
        original.as_deref(),
        &d.translations,
        language,
    );
}

/// Fills what a series' details lack in their language from English.
pub(super) fn fill_series(d: &mut SeriesDetails, language: &str) {
    d.language = language.to_owned();
    if language == ENGLISH {
        return;
    }
    let original = d.original_language.clone();
    d.from_english = fill(
        &mut d.name,
        &mut d.overview,
        original.as_deref(),
        &d.translations,
        language,
    );
}

/// Whether any episode lacks a name or an overview in its language.
pub(super) fn lacking(seasons: &[Season]) -> bool {
    seasons
        .iter()
        .flat_map(|s| &s.episodes)
        .any(|e| empty(&e.name) || empty(&e.overview))
}

/// Fills episodes' missing names and overviews from the same seasons in English.
pub(super) fn fill_episodes(seasons: &mut [Season], english: &[Season]) {
    let english: std::collections::HashMap<(i64, i64), &Episode> = english
        .iter()
        .flat_map(|s| &s.episodes)
        .map(|e| ((e.season_number, e.episode_number), e))
        .collect();
    for episode in seasons.iter_mut().flat_map(|s| &mut s.episodes) {
        let Some(en) = english.get(&(episode.season_number, episode.episode_number)) else {
            continue;
        };
        if empty(&episode.name) && !empty(&en.name) {
            episode.name = en.name.clone();
            episode.from_english.push("name");
        }
        if empty(&episode.overview) && !empty(&en.overview) {
            episode.overview = en.overview.clone();
            episode.from_english.push("overview");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_are_a_code_and_perhaps_a_region() {
        for good in ["fr", "fr-FR", "es-ES", "pt-BR", "en-US"] {
            assert!(is_language(good), "{good}");
        }
        for bad in ["", "FR", "fr_FR", "fr-fr", "french", "es-419", "fr-FR&x=1"] {
            assert!(!is_language(bad), "{bad}");
        }
        assert_eq!(code("fr-FR"), "fr");
    }

    #[test]
    fn logos_in_the_language_come_first() {
        let list: ImageList = serde_json::from_value(serde_json::json!({ "logos": [
            { "file_path": "/en.png", "iso_639_1": "en", "vote_average": 9.0, "vote_count": 9 },
            { "file_path": "/fr.png", "iso_639_1": "fr", "vote_average": 5.0, "vote_count": 1 },
            { "file_path": "/none.png", "iso_639_1": null, "vote_average": 9.9, "vote_count": 9 }
        ]}))
        .unwrap();
        assert_eq!(list.logo_in("fr"), Some("/fr.png"));
        assert_eq!(list.logo_in("de"), Some("/en.png"));
    }

    fn movie(json: serde_json::Value) -> MovieDetails {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn untranslated_text_comes_from_english() {
        let translations = serde_json::json!({ "translations": [
            { "iso_639_1": "en", "iso_3166_1": "US",
              "data": { "title": "Spirited Away", "overview": "A girl in a spirit world." } },
            { "iso_639_1": "fr", "iso_3166_1": "FR", "data": { "title": "", "overview": "" } },
        ]});
        let mut d = movie(serde_json::json!({ "id": 129, "title": "千と千尋の神隠し",
            "original_language": "ja", "overview": "", "translations": translations }));
        fill_movie(&mut d, "fr-FR");
        assert_eq!(d.title, "Spirited Away");
        assert_eq!(d.overview.as_deref(), Some("A girl in a spirit world."));
        assert_eq!(d.from_english, ["title", "overview"]);
        assert_eq!(d.language, "fr-FR");

        // Translated, or in its own language: kept.
        let translations = serde_json::json!({ "translations": [
            { "iso_639_1": "en", "iso_3166_1": "US", "data": { "title": "Amelie", "overview": "En." } },
        ]});
        let mut d = movie(
            serde_json::json!({ "id": 194, "title": "Le Fabuleux Destin d'Amélie Poulain",
            "original_language": "fr", "overview": "Fr.", "translations": translations }),
        );
        fill_movie(&mut d, "fr-FR");
        assert_eq!(d.title, "Le Fabuleux Destin d'Amélie Poulain");
        assert!(d.from_english.is_empty());
    }
}
