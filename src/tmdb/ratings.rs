//! Movies' and series' content ratings, by country.

use std::collections::BTreeMap;

use super::*;

impl Client {
    /// A movie's (or with `tv`, a series') certification in each country that gives one, by
    /// ISO 3166 country code; `None` when TMDB has no such title. Only the title's id leaves
    /// the server.
    pub async fn certifications(
        &self,
        tv: bool,
        id: i64,
    ) -> anyhow::Result<Option<BTreeMap<String, String>>> {
        #[derive(Deserialize)]
        struct Reply {
            #[serde(default, deserialize_with = "null_as_default")]
            results: Vec<Country>,
        }
        #[derive(Deserialize)]
        struct Country {
            iso_3166_1: Option<String>,
            // Series: one rating per country.
            rating: Option<String>,
            // Movies: a certification per release; the theatrical one counts first.
            #[serde(default, deserialize_with = "null_as_default")]
            release_dates: Vec<Release>,
        }
        #[derive(Deserialize)]
        struct Release {
            certification: Option<String>,
            #[serde(rename = "type")]
            kind: Option<i64>,
        }
        let path = if tv {
            format!("/tv/{id}/content_ratings")
        } else {
            format!("/movie/{id}/release_dates")
        };
        let Some(reply) = self.get::<Reply>(&path, &[]).await? else {
            return Ok(None);
        };
        let mut found = BTreeMap::new();
        for country in reply.results {
            let Some(code) = country.iso_3166_1.filter(|c| !c.trim().is_empty()) else {
                continue;
            };
            let mut releases: Vec<&Release> = country.release_dates.iter().collect();
            // TMDB's release type 3 is theatrical.
            releases.sort_by_key(|r| r.kind != Some(3));
            let certification = country.rating.or_else(|| {
                releases
                    .iter()
                    .find_map(|r| r.certification.clone().filter(|c| !c.trim().is_empty()))
            });
            if let Some(certification) = certification.filter(|c| !c.trim().is_empty()) {
                found.insert(code.trim().to_uppercase(), certification.trim().to_owned());
            }
        }
        Ok(Some(found))
    }
}
