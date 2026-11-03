//! Music by description: a phrase like "upbeat rock from the nineties" as filters, which the
//! apps run against the library as they run their own.
//!
//! **The model translates; it never chooses tracks.** Asked to pick songs for a named genre, a
//! model was right two to five times in eight, worse than useless when the library can answer
//! the same question exactly; mapping a phrase onto the library's genres and a year range is
//! what a model does well. The library's genre names go with the phrase for that reason: left
//! to itself a model invents "Chill" and "Workout", which match nothing.

use serde::Serialize;

use super::*;

/// The order a listener asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Ordering {
    /// A to Z by name, unless they asked for something else.
    #[default]
    ByName,
    MostPlayed,
    NewestFirst,
    Random,
    RecentlyPlayed,
}

impl Ordering {
    const ALL: [Ordering; 5] = [
        Self::ByName,
        Self::MostPlayed,
        Self::NewestFirst,
        Self::Random,
        Self::RecentlyPlayed,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Self::ByName => "by_name",
            Self::MostPlayed => "most_played",
            Self::NewestFirst => "newest_first",
            Self::Random => "random",
            Self::RecentlyPlayed => "recently_played",
        }
    }
}

/// What a phrase becomes: filters the music API understands.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct MusicFilters {
    /// Words to match names against, when the phrase named a song, album or artist.
    pub search_term: Option<String>,
    /// Genres the library has, as it spells them, each once; never the model's invention.
    pub genres: Vec<String>,
    /// The earliest release year, inclusive; never after `to_year`.
    pub from_year: Option<i64>,
    /// The latest release year, inclusive.
    pub to_year: Option<i64>,
    /// Only what the listener likes.
    pub liked_only: bool,
    pub ordering: Ordering,
}

impl Client {
    /// `phrase` as filters over a library with `genres`.
    pub async fn music_filters(
        &self,
        phrase: &str,
        genres: &[String],
    ) -> Result<MusicFilters, Failure> {
        let this_year = crate::aoide::discover::year_of(crate::db::unix_now().div_euclid(86_400));
        let answer = self.complete(&search_prompt(genres), phrase).await?;
        parse_filters(&answer, genres, this_year).ok_or_else(|| {
            Failure::Other("the model's answer wasn't the filters asked for".to_owned())
        })
    }
}

/// The instructions, carrying the library's genres; the phrase goes as the user's message.
fn search_prompt(genres: &[String]) -> String {
    let orderings: Vec<String> = Ordering::ALL
        .iter()
        .map(|o| format!("\"{}\"", o.as_str()))
        .collect();
    format!(
        "You translate a listener's description of music into search filters. You never \
         choose songs; you only produce filters, and the music server selects.\n\n\
         Answer with JSON only, no other text, in exactly this shape:\n\
         {{\"search_term\": string|null, \"genres\": string[], \"from_year\": number|null, \
         \"to_year\": number|null, \"liked_only\": boolean, \"ordering\": {}}}\n\n\
         Rules:\n\
         - genres MUST be chosen from this library's genres, spelled exactly as given. If none \
         apply, answer an empty array. Never invent a genre.\n\
         - from_year and to_year express a period: \"the nineties\" is 1990 to 1999.\n\
         - search_term is a song, album or artist name if the description names one.\n\
         - liked_only is true only for favourites, liked, loved or starred music.\n\
         - ordering is \"by_name\" unless the listener asked for something else.\n\n\
         This library's genres: {}",
        orderings.join("|"),
        vocabulary(genres, "(none: leave genres empty)"),
    )
}

/// The library's genres for a prompt, or `none` when it has none.
pub(super) fn vocabulary(genres: &[String], none: &str) -> String {
    if genres.is_empty() {
        none.to_owned()
    } else {
        genres.join(", ")
    }
}

/// The filters in a model's answer, keeping only what can be believed: genres the library has
/// (matched ignoring case, in the library's spelling, each once), years from 1860 to next year
/// (a reversed range put right), a known ordering (else by name). None without a JSON object.
fn parse_filters(answer: &str, genres: &[String], this_year: i64) -> Option<MusicFilters> {
    let start = answer.find('{')?;
    let end = answer.rfind('}')?;
    let value: Value = serde_json::from_str(answer.get(start..=end)?).ok()?;
    let mut found: Vec<String> = Vec::new();
    for wanted in value["genres"].as_array().into_iter().flatten() {
        let Some(wanted) = wanted.as_str().map(str::to_lowercase) else {
            continue;
        };
        if let Some(genre) = genres.iter().find(|g| g.to_lowercase() == wanted)
            && !found.contains(genre)
        {
            found.push(genre.clone());
        }
    }
    let year = |v: &Value| {
        let year = v
            .as_f64()
            .map(|y| y.trunc() as i64)
            .or_else(|| leading_number(v.as_str()?))?;
        (1860..=this_year + 1).contains(&year).then_some(year)
    };
    let (from_year, to_year) = match (year(&value["from_year"]), year(&value["to_year"])) {
        (Some(a), Some(b)) => (Some(a.min(b)), Some(a.max(b))),
        other => other,
    };
    let ordering = value["ordering"].as_str();
    Some(MusicFilters {
        search_term: value["search_term"]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned),
        genres: found,
        from_year,
        to_year,
        liked_only: value["liked_only"] == Value::Bool(true),
        ordering: Ordering::ALL
            .into_iter()
            .find(|o| Some(o.as_str()) == ordering)
            .unwrap_or_default(),
    })
}

/// The number a text starts with, as models write years: "1990s" is 1990.
fn leading_number(text: &str) -> Option<i64> {
    let text = text.trim_start();
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    text[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn genres() -> Vec<String> {
        ["Rock", "Jazz", "Hip-Hop"].map(String::from).to_vec()
    }

    fn parse(answer: Value) -> MusicFilters {
        parse_filters(&answer.to_string(), &genres(), 2026).unwrap()
    }

    #[test]
    fn the_prompt_carries_the_librarys_genres_and_every_ordering() {
        let prompt = search_prompt(&genres());
        assert!(prompt.contains("This library's genres: Rock, Jazz, Hip-Hop"));
        assert!(prompt.contains(
            r#""ordering": "by_name"|"most_played"|"newest_first"|"random"|"recently_played"}"#
        ));
        assert!(search_prompt(&[]).contains("leave genres empty"));
    }

    #[test]
    fn genres_are_the_librarys_own_each_once_in_its_spelling() {
        let found = parse(json!({ "genres": ["jazz", "HIP-HOP", "Chill", "Jazz", 3] }));
        assert_eq!(found.genres, ["Jazz", "Hip-Hop"]);
    }

    #[test]
    fn years_are_plausible_and_in_order() {
        let found = parse(json!({ "from_year": "1999", "to_year": 1990.0 }));
        assert_eq!((found.from_year, found.to_year), (Some(1990), Some(1999)));
        assert_eq!(parse(json!({ "from_year": "1990s" })).from_year, Some(1990));
        assert_eq!(parse(json!({ "from_year": 2027 })).from_year, Some(2027));
        assert_eq!(parse(json!({ "from_year": 2028 })).from_year, None);
        assert_eq!(parse(json!({ "from_year": 1200 })).from_year, None);
    }

    #[test]
    fn the_rest_is_believed_only_when_its_what_was_asked_for() {
        let found = parse(json!({ "search_term": " Muse ", "liked_only": "yes",
                                  "ordering": "newest_first" }));
        assert_eq!(found.search_term.as_deref(), Some("Muse"));
        assert!(!found.liked_only);
        assert_eq!(found.ordering, Ordering::NewestFirst);
        let found = parse(json!({ "search_term": "  ", "liked_only": true, "ordering": "vibes" }));
        assert_eq!(found.search_term, None);
        assert!(found.liked_only);
        assert_eq!(found.ordering, Ordering::ByName);
    }

    #[test]
    fn filters_are_read_from_whatever_the_model_wraps_them_in() {
        let fenced = "Sure:\n```json\n{\"genres\": [\"rock\"]}\n```";
        assert_eq!(
            parse_filters(fenced, &genres(), 2026).unwrap().genres,
            ["Rock"]
        );
        assert_eq!(
            parse_filters("{}", &genres(), 2026).unwrap(),
            MusicFilters::default()
        );
        assert!(parse_filters("I can't help with that.", &genres(), 2026).is_none());
        assert!(parse_filters(r#"["Rock"]"#, &genres(), 2026).is_none());
    }
}
