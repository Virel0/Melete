//! What the *arrs send back, as Melete reads it: status, profiles, custom formats, movies,
//! series, artists, albums and Prowlarr's indexers.

use super::*;

/// What an *arr says about itself; only its name is read.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatus {
    /// "Radarr", "Sonarr", "Lidarr" or "Prowlarr" (compared ignoring case).
    pub app_name: String,
}

/// A quality profile by id and name, as listed for choosing one.
#[derive(Debug, Deserialize)]
pub struct QualityProfile {
    /// The *arr's own id for the profile.
    pub id: i64,
    /// The name shown in the *arr.
    pub name: String,
}

/// A movie in Radarr's list.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadarrMovie {
    /// Radarr's own id for the movie.
    pub id: i64,
    /// Radarr's title for it.
    pub title: String,
    /// The release year; None when Radarr doesn't know it.
    pub year: Option<i64>,
    /// The TMDB movie id, which the wanted list is keyed on.
    pub tmdb_id: i64,
    /// The IMDb id (`tt…`), when Radarr has one.
    pub imdb_id: Option<String>,
    /// Whether Radarr is looking for it.
    pub monitored: bool,
    /// When Radarr starts searching: announced, inCinemas or released.
    pub minimum_availability: Option<String>,
    /// The id of its quality profile in Radarr.
    pub quality_profile_id: Option<i64>,
    /// The movie's folder as Radarr sees it, whether or not it exists yet.
    pub path: Option<String>,
    /// The root folder it sits in as Radarr sees it, which picks the library.
    pub root_folder_path: Option<String>,
    /// The movie's original language, as Radarr's language id and name.
    #[serde(default)]
    pub original_language: Option<IdName>,
}

/// An artist in Lidarr's list.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LidarrArtist {
    /// Lidarr's own id for the artist, which its albums point to.
    pub id: i64,
    /// The artist's name in Lidarr.
    pub artist_name: String,
    /// The MusicBrainz artist id.
    pub foreign_artist_id: String,
    /// Whether Lidarr is looking for the artist's albums.
    pub monitored: bool,
    /// The id of its quality profile in Lidarr.
    pub quality_profile_id: Option<i64>,
    /// The root folder it sits in as Lidarr sees it, which picks the library.
    pub root_folder_path: Option<String>,
    /// all, new or none.
    pub monitor_new_items: Option<String>,
}

/// An album in Lidarr's list.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LidarrAlbum {
    /// Lidarr's own id for the album.
    pub id: i64,
    /// The album's title in Lidarr.
    pub title: String,
    /// The MusicBrainz release group id.
    pub foreign_album_id: String,
    /// Lidarr's id for the album's artist ([`LidarrArtist::id`]).
    pub artist_id: i64,
    /// Whether Lidarr is looking for it.
    pub monitored: bool,
    /// An ISO timestamp, such as 2016-04-29T00:00:00Z.
    pub release_date: Option<String>,
    /// The primary type: Album, EP, Single, Broadcast or Other.
    pub album_type: Option<String>,
    /// Names, or objects with a name, depending on the version.
    #[serde(default)]
    pub secondary_types: Vec<serde_json::Value>,
}

/// A season of a series in Sonarr's list.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SonarrSeason {
    /// The season's number; 0 is specials.
    pub season_number: i64,
    /// Whether Sonarr is looking for the season.
    pub monitored: bool,
}

/// A series in Sonarr's list.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SonarrSeries {
    /// Sonarr's own id for the series, for fetching its episodes.
    pub id: i64,
    /// Sonarr's title for it.
    pub title: String,
    /// The year it started; None when Sonarr doesn't know it.
    pub year: Option<i64>,
    /// The TVDB series id, which the wanted list is keyed on.
    pub tvdb_id: i64,
    /// Sonarr v4 knows it; v3 doesn't.
    pub tmdb_id: Option<i64>,
    /// The IMDb id (`tt…`), when Sonarr has one.
    pub imdb_id: Option<String>,
    /// Whether Sonarr is looking for it.
    pub monitored: bool,
    /// Sonarr's series type: standard, daily or anime.
    pub series_type: Option<String>,
    /// The id of its quality profile in Sonarr.
    pub quality_profile_id: Option<i64>,
    /// The series' folder as Sonarr sees it, whether or not it exists yet.
    pub path: Option<String>,
    /// The root folder it sits in as Sonarr sees it, which picks the library.
    pub root_folder_path: Option<String>,
    /// Its seasons and whether each is monitored; empty when Sonarr sent none.
    #[serde(default)]
    pub seasons: Vec<SonarrSeason>,
    /// Sonarr 4 has it.
    #[serde(default)]
    pub original_language: Option<IdName>,
}

/// An episode of one series in Sonarr; only its numbers and monitoring are read.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SonarrEpisode {
    /// The season's number; 0 is specials.
    pub season_number: i64,
    /// The episode's number within its season.
    pub episode_number: i64,
    /// Whether Sonarr is looking for it.
    pub monitored: bool,
}

/// A quality profile in full, for importing it. Items are listed lowest first.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FullQualityProfile {
    /// The profile's name in the *arr.
    pub name: String,
    /// Whether a file that meets the profile is still replaced by a better one.
    pub upgrade_allowed: bool,
    /// A quality's id, or a group's (1000 and up).
    pub cutoff: i64,
    /// The qualities and groups, lowest first, each allowed or not.
    pub items: Vec<ProfileItem>,
    /// The custom-format score a release needs to be taken at all; 0 when missing.
    #[serde(default)]
    pub min_format_score: i64,
    /// The custom-format score past which upgrades stop; 0 when missing.
    #[serde(default)]
    pub cutoff_format_score: i64,
    /// Radarr 5 and Sonarr 4 have it; 1 is their default.
    #[serde(default = "one")]
    pub min_upgrade_format_score: i64,
    /// The score each custom format adds; empty when missing.
    #[serde(default)]
    pub format_items: Vec<FormatItem>,
    /// Radarr only.
    pub language: Option<IdName>,
}

fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::deserialize(deserializer)?.unwrap_or_default())
}

fn one() -> i64 {
    1
}

/// A quality, or a group of qualities ranked together.
#[derive(Debug, Deserialize)]
pub struct ProfileItem {
    /// A group's id (1000 and up); None for a single quality.
    pub id: Option<i64>,
    /// A group's name; None for a single quality.
    pub name: Option<String>,
    /// The quality; None for a group.
    pub quality: Option<IdName>,
    /// A group's qualities; empty for a single quality.
    #[serde(default)]
    pub items: Vec<ProfileItem>,
    /// Whether the profile accepts it.
    pub allowed: bool,
}

/// Anything an *arr refers to by id and also names: a quality or a language.
#[derive(Debug, Deserialize)]
pub struct IdName {
    /// The *arr's own id; Radarr and Sonarr number languages differently.
    pub id: i64,
    /// Its name in the *arr.
    pub name: String,
}

/// A custom format's score in a quality profile.
#[derive(Debug, Deserialize)]
pub struct FormatItem {
    /// The custom format's id ([`CustomFormat::id`]).
    pub format: i64,
    /// What a release matching the format adds to its score; negative to hold it back.
    pub score: i64,
}

/// A custom format: a named set of conditions a release can match.
#[derive(Debug, Deserialize)]
pub struct CustomFormat {
    /// The *arr's own id, which profiles' format items point to.
    pub id: i64,
    /// The format's name in the *arr.
    pub name: String,
    /// Its conditions. For each kind of test, every required one and at least one of them must
    /// hold.
    pub specifications: Vec<Specification>,
}

/// One condition of a custom format.
#[derive(Debug, Deserialize)]
pub struct Specification {
    /// The condition's name, as given in the *arr.
    pub name: String,
    /// What kind of test it is, such as ReleaseTitleSpecification.
    pub implementation: String,
    /// Whether the condition holds when the test fails rather than passes.
    pub negate: bool,
    /// Whether this condition must hold, rather than any one of the conditions of its kind.
    pub required: bool,
    /// The test's settings, such as a pattern or a resolution.
    pub fields: Vec<Field>,
}

/// One setting of a custom format's condition.
#[derive(Debug, Deserialize)]
pub struct Field {
    /// The setting's name, such as value or min.
    pub name: String,
    /// Its value, whose shape depends on the test; null when missing.
    #[serde(default)]
    pub value: serde_json::Value,
}

/// One of Prowlarr's indexers. Its `fields` (which hold the tracker's own credentials) are
/// never read.
#[derive(Debug, Deserialize)]
pub struct ProwlarrIndexer {
    /// Prowlarr's own id, which its per-indexer feed URL holds.
    pub id: i64,
    /// The indexer's name in Prowlarr.
    pub name: String,
    /// Whether it's enabled in Prowlarr.
    pub enable: bool,
    /// "torrent" or "usenet".
    pub protocol: String,
    /// What it can search; empty when Prowlarr sent null.
    #[serde(default, deserialize_with = "null_as_default")]
    pub capabilities: ProwlarrCapabilities,
}

/// What one of Prowlarr's indexers can search. Anything missing is empty.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProwlarrCapabilities {
    /// The Newznab categories it has, each with its subcategories.
    pub categories: Vec<ProwlarrCategory>,
    /// What a movie search can name, such as q, imdbId or tmdbId.
    pub movie_search_params: Vec<String>,
    /// What a TV search can name, such as q, tvdbId, season or ep.
    pub tv_search_params: Vec<String>,
}

/// A Newznab category, such as 2000 Movies, with its subcategories.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProwlarrCategory {
    /// The category's number.
    pub id: i64,
    /// Its subcategories, such as 2040 Movies/HD; empty when there are none.
    #[serde(default)]
    pub sub_categories: Vec<ProwlarrCategory>,
}
