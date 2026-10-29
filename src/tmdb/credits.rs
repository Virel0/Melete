//! Who's in a title and who made it, and the collections films belong to, as TMDB lists them.

use super::*;

/// A title's cast and crew, as `/credits` lists them.
#[derive(Debug, Default, Deserialize)]
pub struct Credits {
    /// Its cast, with each one's billing order.
    #[serde(default, deserialize_with = "null_as_default")]
    pub cast: Vec<CastMember>,
    /// Its crew, with each one's job.
    #[serde(default, deserialize_with = "null_as_default")]
    pub crew: Vec<CrewMember>,
}

/// One of a title's cast.
#[derive(Debug, Deserialize)]
pub struct CastMember {
    /// TMDB's person id.
    pub id: i64,
    /// Their name.
    pub name: String,
    /// Who they play; empty when TMDB doesn't say.
    pub character: Option<String>,
    /// Their place in the billing, top first.
    pub order: Option<i64>,
    /// Their picture on TMDB's image server.
    pub profile_path: Option<String>,
}

/// One of a title's crew.
#[derive(Debug, Deserialize)]
pub struct CrewMember {
    /// TMDB's person id.
    pub id: i64,
    /// Their name.
    pub name: String,
    /// What they did: Director, Screenplay...
    pub job: Option<String>,
    /// Their picture on TMDB's image server.
    pub profile_path: Option<String>,
}

/// A series' creator, as its details list them.
#[derive(Debug, Deserialize)]
pub struct Creator {
    /// TMDB's person id.
    pub id: i64,
    /// Their name.
    pub name: String,
    /// Their picture on TMDB's image server.
    pub profile_path: Option<String>,
}

/// The collection a movie belongs to, as its details name it.
#[derive(Debug, Clone, Deserialize)]
pub struct CollectionRef {
    /// TMDB's collection id.
    pub id: i64,
    /// Its name ("The Matrix Collection").
    pub name: String,
    /// Its poster on TMDB's image server.
    pub poster_path: Option<String>,
    /// Its backdrop on TMDB's image server.
    pub backdrop_path: Option<String>,
}

/// A collection with all its films, in or out of the library.
#[derive(Debug, Deserialize)]
pub struct Collection {
    /// Its films.
    #[serde(default, deserialize_with = "null_as_default")]
    pub parts: Vec<Listed>,
}

#[derive(Deserialize)]
struct SeriesCredits {
    #[serde(default, deserialize_with = "null_as_default")]
    created_by: Vec<Creator>,
    #[serde(default)]
    credits: Credits,
}

impl Client {
    /// A movie's cast and crew, in the metadata language; `None` when TMDB has no such movie.
    pub async fn movie_credits(&self, id: i64) -> anyhow::Result<Option<Credits>> {
        self.get_in(&self.language, &format!("/movie/{id}/credits"), &[])
            .await
    }

    /// A series' creators, cast and crew, in one request; `None` when TMDB has no such series.
    pub async fn series_credits(&self, id: i64) -> anyhow::Result<Option<(Vec<Creator>, Credits)>> {
        let found: Option<SeriesCredits> = self
            .get_in(
                &self.language,
                &format!("/tv/{id}"),
                &[("append_to_response", "credits")],
            )
            .await?;
        Ok(found.map(|s| (s.created_by, s.credits)))
    }

    /// A collection's films; `None` when TMDB has no such collection. Only its id leaves the
    /// server.
    pub async fn collection(&self, id: i64) -> anyhow::Result<Option<Collection>> {
        self.get_in(&self.language, &format!("/collection/{id}"), &[])
            .await
    }
}
