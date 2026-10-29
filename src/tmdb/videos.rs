//! A title's videos from TMDB (trailers, teasers and the rest, with where each is hosted), for
//! trailers online. Asked for by the TMDB id the title is already matched to.

use super::*;

/// One of a title's videos, as TMDB lists it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Video {
    /// The video's id on its site: a YouTube video id.
    pub key: String,
    /// Its name, as TMDB has it.
    #[serde(default)]
    pub name: String,
    /// Where it's hosted, such as "YouTube" or "Vimeo".
    pub site: String,
    /// What it is: "Trailer", "Teaser", "Featurette", "Clip"…
    #[serde(rename = "type")]
    pub kind: String,
    /// Whether the studio or network published it.
    #[serde(default)]
    pub official: bool,
    /// Its language, ISO 639-1 ("en"); None when TMDB has none.
    #[serde(rename = "iso_639_1", default)]
    pub language: Option<String>,
    /// Its height in pixels (360 to 2160).
    #[serde(default)]
    pub size: i64,
    /// When it was published.
    #[serde(default)]
    pub published_at: Option<String>,
}

#[derive(Deserialize)]
struct VideoList {
    #[serde(default, deserialize_with = "null_as_default")]
    results: Vec<Video>,
}

impl Client {
    /// A movie's (or with `tv`, a series') videos in the metadata language and English;
    /// `None` when TMDB has no such title.
    pub async fn videos(&self, tv: bool, id: i64) -> anyhow::Result<Option<Vec<Video>>> {
        let kind = if tv { "tv" } else { "movie" };
        let code = language::code(&self.language);
        let languages = if code == "en" {
            "en".to_owned()
        } else {
            format!("{code},en")
        };
        let list: Option<VideoList> = self
            .get_in(
                &self.language,
                &format!("/{kind}/{id}/videos"),
                &[("include_video_language", &languages)],
            )
            .await?;
        Ok(list.map(|l| l.results))
    }
}
