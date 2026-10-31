//! What TMDB sends back, as Melete reads it: search results, details, seasons, lists and images.

use super::*;

#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub(super) struct Page<T> {
    #[serde(default, deserialize_with = "null_as_default")]
    pub(super) results: Vec<T>,
    #[serde(default)]
    pub(super) total_pages: Option<i64>,
}

/// A movie in TMDB's search or find results.
#[derive(Debug, Deserialize)]
pub struct MovieResult {
    /// TMDB's movie id.
    pub id: i64,
    /// The title in English, or as TMDB knows it.
    pub title: String,
    /// The title in its original language, when TMDB has one.
    pub original_title: Option<String>,
    /// The first release as YYYY-MM-DD; None or empty when TMDB doesn't know it.
    pub release_date: Option<String>,
    /// Its poster's path on TMDB's image server, when it has one.
    #[serde(default)]
    pub poster_path: Option<String>,
}

/// A series in TMDB's search or find results.
#[derive(Debug, Deserialize)]
pub struct SeriesResult {
    /// TMDB's series id.
    pub id: i64,
    /// The name in English, or as TMDB knows it.
    pub name: String,
    /// The name in its original language, when TMDB has one.
    pub original_name: Option<String>,
    /// The first episode's air date as YYYY-MM-DD; None or empty when TMDB doesn't know it.
    pub first_air_date: Option<String>,
    /// Its poster's path on TMDB's image server, when it has one.
    #[serde(default)]
    pub poster_path: Option<String>,
}

/// A page of a chart or list.
#[derive(Debug, Deserialize)]
pub struct ListPage {
    /// A chart's titles; empty for a list.
    #[serde(default, deserialize_with = "null_as_default")]
    pub results: Vec<ListEntry>,
    /// A list's titles; empty for a chart.
    #[serde(default, deserialize_with = "null_as_default")]
    pub items: Vec<ListEntry>,
    /// How many pages there are; None when TMDB doesn't say.
    pub total_pages: Option<u32>,
}

/// A movie (`title`) or series (`name`) on a list; lists say which with `media_type`.
#[derive(Debug, Deserialize)]
pub struct ListEntry {
    /// TMDB's movie or series id.
    pub id: i64,
    /// "movie" or "tv"; None on a chart, which holds one kind only.
    pub media_type: Option<String>,
    /// A movie's title; None for a series.
    pub title: Option<String>,
    /// A series' name; None for a movie.
    pub name: Option<String>,
}

/// What TMDB found for an outside id: usually one movie or one series, or nothing.
#[derive(Debug, Default, Deserialize)]
pub struct FindResults {
    /// Movies with that id; empty when there are none.
    #[serde(default, deserialize_with = "null_as_default")]
    pub movie_results: Vec<MovieResult>,
    /// Series with that id; empty when there are none.
    #[serde(default, deserialize_with = "null_as_default")]
    pub tv_results: Vec<SeriesResult>,
}

/// A genre as TMDB names it, in English.
#[derive(Debug, Deserialize)]
pub struct Genre {
    /// The genre's name, such as Drama.
    pub name: String,
}

/// A movie's details, for matching and for showing it.
#[derive(Debug, Deserialize)]
pub struct MovieDetails {
    /// TMDB's movie id.
    pub id: i64,
    /// The IMDb id (`tt…`); None or empty when TMDB has none.
    pub imdb_id: Option<String>,
    /// ISO 639-1.
    #[serde(default)]
    pub original_language: Option<String>,
    /// The title in English, or as TMDB knows it.
    pub title: String,
    /// The title in its original language.
    pub original_title: Option<String>,
    /// The first release as YYYY-MM-DD; None or empty when TMDB doesn't know it.
    pub release_date: Option<String>,
    /// A short summary of the plot.
    pub overview: Option<String>,
    /// The running time in minutes; None or 0 when TMDB doesn't know it.
    pub runtime: Option<i64>,
    /// The poster's file on TMDB's image server, such as `/abc.jpg`.
    pub poster_path: Option<String>,
    /// The backdrop's file on TMDB's image server, such as `/abc.jpg`.
    pub backdrop_path: Option<String>,
    /// Its genres; empty when TMDB lists none.
    #[serde(default, deserialize_with = "null_as_default")]
    pub genres: Vec<Genre>,
    /// The collection it belongs to, if any.
    #[serde(default)]
    pub belongs_to_collection: Option<CollectionRef>,
    /// TMDB's translations, asked for when the metadata language isn't English, to fill what
    /// isn't translated.
    #[serde(default)]
    pub translations: Option<Translations>,
    /// The language it was asked for in (`en-US` unless set otherwise).
    #[serde(skip)]
    pub language: String,
    /// The fields filled from English, as the language had none.
    #[serde(skip)]
    pub from_english: Vec<&'static str>,
}

/// Dates as YYYY-MM-DD.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReleaseDates {
    /// The earliest digital release; None when TMDB has none.
    pub digital: Option<String>,
    /// The earliest physical release (DVD, Blu-ray); None when TMDB has none.
    pub physical: Option<String>,
    /// The earliest showing on TV (a TV film's premiere); None when TMDB has none.
    pub tv: Option<String>,
}

/// A series' details, for matching and for showing it.
#[derive(Debug, Deserialize)]
pub struct SeriesDetails {
    /// TMDB's series id.
    pub id: i64,
    /// The name in English, or as TMDB knows it.
    pub name: String,
    /// ISO 639-1.
    #[serde(default)]
    pub original_language: Option<String>,
    /// The name in its original language.
    pub original_name: Option<String>,
    /// The first episode's air date as YYYY-MM-DD; None or empty when TMDB doesn't know it.
    pub first_air_date: Option<String>,
    /// A short summary.
    pub overview: Option<String>,
    /// Whether it's still running, as TMDB says it: Returning Series, Ended, Canceled...
    pub status: Option<String>,
    /// The poster's file on TMDB's image server, such as `/abc.jpg`.
    pub poster_path: Option<String>,
    /// The backdrop's file on TMDB's image server, such as `/abc.jpg`.
    pub backdrop_path: Option<String>,
    /// Its genres; empty when TMDB lists none.
    #[serde(default, deserialize_with = "null_as_default")]
    pub genres: Vec<Genre>,
    /// Its seasons, specials (season 0) included; empty when TMDB lists none.
    #[serde(default, deserialize_with = "null_as_default")]
    pub seasons: Vec<SeasonSummary>,
    /// Its IMDb and TVDB ids, which [`Client::series`] always asks for; None if TMDB left them out.
    pub external_ids: Option<ExternalIds>,
    /// TMDB's translations, asked for when the metadata language isn't English, to fill what
    /// isn't translated.
    #[serde(default)]
    pub translations: Option<Translations>,
    /// The language it was asked for in (`en-US` unless set otherwise).
    #[serde(skip)]
    pub language: String,
    /// The fields filled from English, as the language had none.
    #[serde(skip)]
    pub from_english: Vec<&'static str>,
}

/// A season as a series' details list it.
#[derive(Debug, Deserialize)]
pub struct SeasonSummary {
    /// The season's number; 0 is specials.
    pub season_number: i64,
}

/// A series' ids elsewhere.
#[derive(Debug, Deserialize)]
pub struct ExternalIds {
    /// The IMDb id (`tt…`); None or empty when TMDB has none.
    pub imdb_id: Option<String>,
    /// The TVDB id; None when TMDB has none.
    pub tvdb_id: Option<i64>,
}

/// One season of a series, with its episodes.
#[derive(Debug, Deserialize)]
pub struct Season {
    /// Its episodes in order; empty when TMDB lists none.
    #[serde(default, deserialize_with = "null_as_default")]
    pub episodes: Vec<Episode>,
}

/// An episode as TMDB lists it.
#[derive(Debug, Deserialize)]
pub struct Episode {
    /// TMDB's episode id.
    pub id: i64,
    /// The season's number; 0 is specials.
    pub season_number: i64,
    /// The episode's number within its season.
    pub episode_number: i64,
    /// The episode's title.
    pub name: Option<String>,
    /// A short summary.
    pub overview: Option<String>,
    /// When it first aired, as YYYY-MM-DD; None or empty when it hasn't been announced.
    pub air_date: Option<String>,
    /// The running time in minutes; None when TMDB doesn't know it.
    pub runtime: Option<i64>,
    /// A still from it on TMDB's image server, such as `/abc.jpg`.
    pub still_path: Option<String>,
    /// The fields filled from English, as the language had none.
    #[serde(skip)]
    pub from_english: Vec<&'static str>,
}

/// A title's translations, as `append_to_response=translations` gives them.
#[derive(Debug, Default, Deserialize)]
pub struct Translations {
    /// One per language and region TMDB has.
    #[serde(default, deserialize_with = "null_as_default")]
    pub translations: Vec<Translation>,
}

/// A title's text in one language and region.
#[derive(Debug, Default, Deserialize)]
pub struct Translation {
    /// The language, ISO 639-1.
    #[serde(default)]
    pub iso_639_1: String,
    /// The region, ISO 3166-1.
    #[serde(default)]
    pub iso_3166_1: String,
    /// The text; empty where it's not translated.
    #[serde(default)]
    pub data: TranslationData,
}

/// A translation's text: `title` for a movie, `name` for a series.
#[derive(Debug, Default, Deserialize)]
pub struct TranslationData {
    /// A movie's title.
    pub title: Option<String>,
    /// A series' name.
    pub name: Option<String>,
    /// A short summary.
    pub overview: Option<String>,
}

/// A movie or series as TMDB's charts, recommendations and searches list it: `title`
/// and `release_date` for a movie, `name` and `first_air_date` for a series.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Listed {
    /// TMDB's movie or series id.
    pub id: i64,
    /// A movie's title; None for a series.
    pub title: Option<String>,
    /// A series' name; None for a movie.
    pub name: Option<String>,
    /// A movie's first release as YYYY-MM-DD; None for a series or when unknown.
    pub release_date: Option<String>,
    /// A series' first air date as YYYY-MM-DD; None for a movie or when unknown.
    pub first_air_date: Option<String>,
    /// A short summary.
    pub overview: Option<String>,
    /// The poster's file on TMDB's image server, such as `/abc.jpg`.
    pub poster_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ListedPage {
    #[serde(default, deserialize_with = "null_as_default")]
    pub(super) results: Vec<Listed>,
}

/// A title's images, as `/movie/{id}/images` lists them.
#[derive(Debug, Default, Deserialize)]
pub struct ImageList {
    /// Its backdrops; empty when there are none.
    #[serde(default, deserialize_with = "null_as_default")]
    pub backdrops: Vec<Image>,
    /// Its logos (the title as artwork); empty when there are none.
    #[serde(default, deserialize_with = "null_as_default")]
    pub logos: Vec<Image>,
}

/// One image of a title on TMDB's image server, with its votes.
#[derive(Debug, Clone, Deserialize)]
pub struct Image {
    /// The image's file on TMDB's image server, such as `/abc.jpg`.
    pub file_path: String,
    /// The language of any text on it; none for a picture without text.
    pub iso_639_1: Option<String>,
    /// How TMDB's users rate it, out of 10; 0 when nobody voted.
    #[serde(default, deserialize_with = "null_as_default")]
    pub vote_average: f64,
    /// How many users voted on it.
    #[serde(default, deserialize_with = "null_as_default")]
    pub vote_count: i64,
}

impl ImageList {
    /// The best-voted backdrop without text (no language), a JPEG or PNG.
    pub fn textless_backdrop(&self) -> Option<&str> {
        best(
            self.backdrops
                .iter()
                .filter(|i| i.iso_639_1.is_none())
                .filter(|i| i.file_path.ends_with(".jpg") || i.file_path.ends_with(".png")),
        )
    }

    /// The best-voted English logo, else the best without a language; PNGs only, as an SVG
    /// can carry script.
    #[cfg(test)]
    pub fn logo(&self) -> Option<&str> {
        self.logo_in("en")
    }

    /// The best-voted logo in `language` (ISO 639-1), else in English, else without a
    /// language; PNGs only, as an SVG can carry script.
    pub fn logo_in(&self, language: &str) -> Option<&str> {
        let pngs = || self.logos.iter().filter(|i| i.file_path.ends_with(".png"));
        let in_language = |l: &str| best(pngs().filter(move |i| i.iso_639_1.as_deref() == Some(l)));
        in_language(language)
            .or_else(|| in_language("en"))
            .or_else(|| best(pngs().filter(|i| i.iso_639_1.is_none())))
    }
}

/// The image with the best vote, then the most votes; ties go to the first file name.
pub(super) fn best<'a>(images: impl Iterator<Item = &'a Image>) -> Option<&'a str> {
    images
        .max_by(|a, b| {
            a.vote_average
                .total_cmp(&b.vote_average)
                .then(a.vote_count.cmp(&b.vote_count))
                .then(b.file_path.cmp(&a.file_path))
        })
        .map(|i| i.file_path.as_str())
}

/// An outside id to look up with [`Client::find`].
#[derive(Debug, Clone, Copy)]
pub enum ExternalId<'a> {
    /// `tt` followed by digits.
    Imdb(&'a str),
    Tvdb(i64),
}
