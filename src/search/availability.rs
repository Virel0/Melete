//! Whether a film can be out in a release's form yet, judged from TMDB's dates: its cinema
//! release, and its earliest digital and disc releases anywhere. A WEB release before the film is
//! out digitally, a Blu-ray before it's out on disc, or a cinema recording before it's in
//! cinemas is most likely a fake under the right name. A movie profile's minimum availability
//! also holds every release back until the film is in cinemas or released.
//!
//! A day's slack allows for time zones: a film out tomorrow somewhere may be out already. A
//! missing digital or disc date counts as 90 days after the cinema release, as Radarr counts it;
//! a film TMDB has no dates for isn't judged.

use super::*;
use crate::release::Source;

/// How far out a film must be before a movie profile takes its releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Minimum {
    /// Any time, once it's announced.
    Announced,
    /// From its cinema release.
    InCinemas,
    /// From its digital or disc release.
    Released,
}

impl Minimum {
    /// The setting as stored: "announced", "in_cinemas" or "released"; None for anything else.
    pub fn parse(text: &str) -> Option<Minimum> {
        match text {
            "announced" => Some(Minimum::Announced),
            "in_cinemas" => Some(Minimum::InCinemas),
            "released" => Some(Minimum::Released),
            _ => None,
        }
    }
}

/// TMDB's dates for a film, as YYYY-MM-DD, with the last day a date may fall on for the film
/// to count as out.
#[derive(Debug, Default, Clone, sqlx::FromRow)]
pub(crate) struct Dates {
    pub(crate) cinema: Option<String>,
    pub(crate) digital: Option<String>,
    pub(crate) physical: Option<String>,
    /// 90 days after the cinema release, for a digital or disc date TMDB doesn't have.
    pub(crate) cinema_plus_90: Option<String>,
    /// Tomorrow, by the server's clock in UTC.
    pub(crate) out_by: String,
}

/// The stored dates of the film with this TMDB id; None when TMDB's entry isn't cached.
pub(crate) async fn dates(pool: &SqlitePool, tmdb_id: Option<i64>) -> sqlx::Result<Option<Dates>> {
    let Some(tmdb_id) = tmdb_id else {
        return Ok(None);
    };
    sqlx::query_as(
        "SELECT nullif(release_date, '') AS cinema, nullif(digital_release, '') AS digital,
                nullif(physical_release, '') AS physical,
                date(nullif(release_date, ''), '+90 days') AS cinema_plus_90,
                date('now', '+1 day') AS out_by
         FROM tmdb_movies WHERE tmdb_id = ?",
    )
    .bind(tmdb_id)
    .fetch_optional(pool)
    .await
}

/// Why a release of the film isn't taken yet, by the profile's minimum and the release's form;
/// None when it may be.
pub(crate) fn refusal(minimum: Minimum, dates: &Dates, source: Option<Source>) -> Option<String> {
    let future =
        |date: Option<&String>| date.filter(|d| d.as_str() > dates.out_by.as_str()).cloned();
    let earliest = |a: &Option<String>, b: &Option<String>| match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b).clone()),
        (a, b) => a.clone().or_else(|| b.clone()),
    };
    let released = earliest(&dates.digital, &dates.physical).or(dates.cinema_plus_90.clone());
    let in_cinemas = dates.cinema.clone().or(released.clone());
    let waiting = match minimum {
        Minimum::Released => future(released.as_ref())
            .map(|d| format!("the film isn't out digitally or on disc until {d}")),
        Minimum::InCinemas => {
            future(in_cinemas.as_ref()).map(|d| format!("the film isn't in cinemas until {d}"))
        }
        Minimum::Announced => None,
    };
    if waiting.is_some() {
        return waiting;
    }
    let (form, out) = match source? {
        Source::WebDl | Source::WebRip | Source::Hdtv => (
            "a WEB or TV release before the film's digital release",
            dates
                .digital
                .clone()
                .or(dates.physical.clone())
                .or(dates.cinema_plus_90.clone()),
        ),
        Source::Bluray | Source::Dvd => (
            "a disc release before the film's disc release",
            dates
                .physical
                .clone()
                .or(dates.digital.clone())
                .or(dates.cinema_plus_90.clone()),
        ),
        Source::Cam | Source::Telesync | Source::Telecine => (
            "a cinema recording before the film's cinema release",
            in_cinemas,
        ),
        Source::Screener => return None,
    };
    future(out.as_ref()).map(|d| format!("{form} on {d} is most likely a fake"))
}

#[cfg(test)]
mod tests;
