//! A result as the search route answers it: the candidate with what only that route gives.

use super::*;

/// A result as `GET /requests/search` answers it: the candidate, with its summary and its
/// portrait's credit.
#[derive(Debug, Serialize)]
pub struct Found {
    #[serde(flatten)]
    pub candidate: Candidate,
    /// A movie's or series' summary; None for music, or when TMDB has none.
    pub overview: Option<String>,
    /// An artist's portrait's author and licence, from Wikimedia Commons; None without one.
    pub poster_credit: Option<crate::artist_portraits::Credit>,
}

impl From<Candidate> for Found {
    fn from(candidate: Candidate) -> Self {
        let overview = candidate.overview.clone();
        let poster_credit = candidate.poster_credit.clone();
        Found {
            candidate,
            overview,
            poster_credit,
        }
    }
}
