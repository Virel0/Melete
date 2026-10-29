//! Picking the TMDB search result that is a title, by its normalised name and year.

use super::*;

/// A search result, reduced to what matching needs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    /// TMDB's movie or series id.
    pub tmdb_id: i64,
    /// The title or name in English, or as TMDB knows it.
    pub title: String,
    /// The title in its original language, which may match the folder too; never sent to clients.
    #[serde(skip)]
    pub original_title: Option<String>,
    /// The year of its release or first air date; None when TMDB doesn't know it.
    pub year: Option<i64>,
}

/// Which search result is the title.
#[derive(Debug, PartialEq)]
pub enum Choice {
    /// Exactly one result fits: its TMDB id.
    One(i64),
    /// More than one fits: those, for someone to choose from.
    Ambiguous(Vec<Candidate>),
    /// None fits.
    Nothing,
}

/// Picks the search result that is this title, if exactly one fits.
///
/// A result fits when its title or original title matches after normalising, and its year is
/// within one of the folder's (release dates differ between countries). Without a folder year,
/// every same-titled result fits, so remakes come out ambiguous.
pub fn choose(title: &str, year: Option<i64>, candidates: &[Candidate]) -> Choice {
    let wanted = normalize(title);
    let fits: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| {
            let title_fits = normalize(&c.title) == wanted
                || c.original_title
                    .as_deref()
                    .is_some_and(|t| normalize(t) == wanted);
            let year_fits = match (year, c.year) {
                (Some(want), Some(have)) => (want - have).abs() <= 1,
                (Some(_), None) => false,
                (None, _) => true,
            };
            title_fits && year_fits
        })
        .collect();
    match fits.as_slice() {
        [] => Choice::Nothing,
        [one] => Choice::One(one.tmdb_id),
        _ => {
            let exact: Vec<&&Candidate> = fits
                .iter()
                .filter(|c| year.is_some() && c.year == year)
                .collect();
            match exact.as_slice() {
                [one] => Choice::One(one.tmdb_id),
                _ => Choice::Ambiguous(fits.into_iter().take(MAX_CANDIDATES).cloned().collect()),
            }
        }
    }
}

/// Lowercase, `&` as "and", apostrophes dropped, other punctuation as spaces.
pub(super) fn normalize(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for c in title.chars() {
        match c {
            '\'' | '\u{2019}' => {}
            '&' => out.push_str(" and "),
            c if c.is_alphanumeric() => out.extend(c.to_lowercase()),
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
