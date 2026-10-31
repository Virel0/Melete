//! The managed lists: one per service (movies, series, anime and artists), each joining what's
//! in the library with what's wanted, one row per title, with the title's state worked out
//! from what's on disk, what's monitored, what's out and what's on its way. Nothing here is
//! stored: every list is worked out when asked.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::progress::{self, Step};

mod artists;
mod cutoff;
mod downloads;
mod movies;
mod series;

pub use artists::*;
use downloads::*;
pub use movies::*;
pub use series::*;

/// Where a title stands, one word for the list. Worked out in this order: something it lacks
/// on its way is `downloading`; a monitored part that's out and not on disk is `missing`;
/// anything on disk is `in_library` (an upgrade on its way and not monitoring both leave it
/// there); a monitored title with nothing out yet is `unreleased`; the rest is `unmonitored`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// On disk, and nothing monitored that's out is missing.
    InLibrary,
    /// Monitored, out, and not (all) on disk, with nothing on its way.
    Missing,
    /// What it lacks is on its way: downloading, being copied home or imported.
    Downloading,
    /// Monitored, and nothing of it is out yet.
    Unreleased,
    /// Nothing on disk and nothing monitored: kept on the list, not looked for.
    Unmonitored,
}

/// What a title's state is worked out from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Facts {
    /// Something of it is on disk (or was imported and waits for the next scan).
    pub on_disk: bool,
    /// Monitored parts that are out and not on disk: a movie, episodes, albums.
    pub missing: i64,
    /// A download for it is under way.
    pub under_way: bool,
    /// Something of it is monitored.
    pub monitored: bool,
}

impl Facts {
    /// The title's state, by the order [`State`] gives.
    pub fn state(self) -> State {
        if self.under_way && (self.missing > 0 || !self.on_disk) {
            State::Downloading
        } else if self.missing > 0 {
            State::Missing
        } else if self.on_disk {
            State::InLibrary
        } else if self.monitored {
            State::Unreleased
        } else {
            State::Unmonitored
        }
    }
}

/// A download under way for a title.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DownloadNow {
    /// The download's id.
    pub id: i64,
    /// Where it has got to: downloading, transferring or importing.
    pub step: Step,
    /// How much of it is done, while downloading.
    pub percent: Option<f64>,
}

impl DownloadNow {
    /// The download from its row's state, progress and message.
    pub fn new(id: i64, state: &str, progress: f64, message: Option<&str>) -> Self {
        let step = progress::download_step(state, message);
        DownloadNow {
            id,
            step,
            percent: (step == Step::Downloading).then_some(progress),
        }
    }
}

/// The download states of one under way.
pub const UNDER_WAY: &str = "('downloading', 'completed', 'transferring')";

/// One row of a managed list: what every service's rows have, and the kind's own fields.
#[derive(Debug, Clone, Serialize)]
pub struct Title<D> {
    /// The wanted entry's id; null for a title in the library that isn't wanted.
    pub wanted_id: Option<i64>,
    /// Its title, as TMDB or MusicBrainz gives it, else as its folder is named.
    pub title: String,
    /// Its year, when known.
    pub year: Option<i64>,
    /// Its poster's address, when there is one: a movie's or series' folder's own poster, else
    /// TMDB's; for an artist, the cover of the first of their albums on disk with its own.
    pub poster: Option<String>,
    /// Where it stands.
    pub state: State,
    /// Whether it's looked for: false when it isn't wanted.
    pub monitored: bool,
    /// Whether it's on the wanted list.
    pub wanted: bool,
    /// The profile it's decided with: Melete's id, when it has one.
    pub profile_id: Option<i64>,
    /// The profile's name: Melete's, else the one the arr gave.
    pub profile: Option<String>,
    /// The library it's in, or the one new files go to.
    pub library: Option<String>,
    /// That library's category: anime or kids.
    pub category: Option<String>,
    /// Bytes on disk.
    pub size: i64,
    /// When it was put on the wanted list, else when its first file was found; Unix seconds.
    pub added_at: Option<i64>,
    /// A download under way for it.
    pub download: Option<DownloadNow>,
    /// Whether its file is below its profile's cutoff (for a series, any monitored episode's),
    /// so an upgrade is looked for; null when that isn't judged: not monitored, no file in the
    /// folder linked to it, no profile, a file not judged yet, or music, which isn't upgraded.
    pub cutoff_unmet: Option<bool>,
    /// The kind's own fields.
    #[serde(flatten)]
    pub detail: D,
}

/// How a list is sorted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    /// By title, case aside (the default).
    #[default]
    Title,
    /// Newest added first.
    Added,
    /// Newest first; titles without a year last.
    Year,
    /// Largest first.
    Size,
}

/// What a list is narrowed to and sorted by.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Only titles in this state.
    pub state: Option<State>,
    /// Only titles monitored (true) or not (false).
    pub monitored: Option<bool>,
    /// Only titles on the wanted list (true) or not (false).
    pub wanted: Option<bool>,
    /// Only titles in (or going to) the library of this name.
    pub library: Option<String>,
    /// `anime`, `kids`, or `none` for libraries without one.
    pub category: Option<String>,
    /// Part of the title, case aside.
    pub q: Option<String>,
    /// Only titles whose file is below the cutoff (true) or judged and not (false).
    pub cutoff_unmet: Option<bool>,
    /// Only the title with this wanted entry.
    pub wanted_id: Option<i64>,
    /// Only the title whose folder in the library this is (a movie's, a series' or an
    /// artist's), as [`Folder`] says.
    pub folder: Option<i64>,
    /// How the list is sorted.
    pub sort: Sort,
}

/// How many titles of a service stand in each state, whatever the filter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    /// In the library.
    pub in_library: usize,
    /// Wanted and missing.
    pub missing: usize,
    /// On their way.
    pub downloading: usize,
    /// Not out yet.
    pub unreleased: usize,
    /// Not looked for.
    pub unmonitored: usize,
    /// Below their profile's cutoff, whatever their state.
    pub cutoff_unmet: usize,
}

/// One page of a managed list.
#[derive(Debug, Serialize)]
pub struct Page<D> {
    /// Titles matching the filter, on every page.
    pub total: usize,
    /// Every title of the service by state, whatever the filter.
    pub counts: Counts,
    /// This page's titles.
    pub items: Vec<Title<D>>,
}

impl Counts {
    fn of<D>(titles: &[Title<D>]) -> Counts {
        let mut counts = Counts::default();
        for title in titles {
            *match title.state {
                State::InLibrary => &mut counts.in_library,
                State::Missing => &mut counts.missing,
                State::Downloading => &mut counts.downloading,
                State::Unreleased => &mut counts.unreleased,
                State::Unmonitored => &mut counts.unmonitored,
            } += 1;
            counts.cutoff_unmet += usize::from(title.cutoff_unmet == Some(true));
        }
        counts
    }
}

/// A title's folder in the library: a movie's, a series' or an artist's.
pub trait Folder {
    /// Its id, when something of it is in the library.
    fn folder(&self) -> Option<i64>;
}

impl Folder for () {
    fn folder(&self) -> Option<i64> {
        None
    }
}

impl Filter {
    fn keeps<D: Folder>(&self, title: &Title<D>, q: Option<&Search>) -> bool {
        self.state.is_none_or(|s| title.state == s)
            && self.wanted_id.is_none_or(|id| title.wanted_id == Some(id))
            && self
                .folder
                .is_none_or(|id| title.detail.folder() == Some(id))
            && self.monitored.is_none_or(|m| title.monitored == m)
            && self.wanted.is_none_or(|w| title.wanted == w)
            && self
                .cutoff_unmet
                .is_none_or(|c| title.cutoff_unmet == Some(c))
            && self
                .library
                .as_deref()
                .is_none_or(|l| title.library.as_deref() == Some(l))
            && match self.category.as_deref() {
                None => true,
                Some("none") => title.category.is_none(),
                Some(c) => title.category.as_deref() == Some(c),
            }
            && q.is_none_or(|q| q.finds(&title.title))
    }
}

/// What a list is searched for: the text as typed, case aside, and folded as other lists fold
/// it, so "amelie" finds "Amélie" and "spiderman" "Spider-Man".
struct Search {
    lower: String,
    folded: String,
}

impl Search {
    fn new(q: &str) -> Search {
        Search {
            lower: q.to_lowercase(),
            folded: crate::search_keys::fold(q),
        }
    }

    fn finds(&self, title: &str) -> bool {
        title.to_lowercase().contains(&self.lower)
            || (!self.folded.is_empty() && crate::search_keys::fold(title).contains(&self.folded))
    }
}

fn by_title<D>(a: &Title<D>, b: &Title<D>) -> Ordering {
    a.title
        .to_lowercase()
        .cmp(&b.title.to_lowercase())
        .then_with(|| a.year.cmp(&b.year))
        .then_with(|| a.wanted_id.cmp(&b.wanted_id))
}

/// The page of `titles` the filter asks for, `limit` from `offset`, with the counts.
pub fn page<D: Folder>(
    titles: Vec<Title<D>>,
    filter: &Filter,
    limit: usize,
    offset: usize,
) -> Page<D> {
    let counts = Counts::of(&titles);
    let q = filter.q.as_deref().map(Search::new);
    // Each with its sort key: words folded, accents aside, a leading article left out.
    let mut kept: Vec<(String, Title<D>)> = titles
        .into_iter()
        .filter(|t| filter.keeps(t, q.as_ref()))
        .map(|t| (crate::search_keys::sort_key(&t.title), t))
        .collect();
    kept.sort_by(|(key_a, a), (key_b, b)| {
        match filter.sort {
            Sort::Title => Ordering::Equal,
            Sort::Added => b.added_at.cmp(&a.added_at),
            Sort::Year => a
                .year
                .is_none()
                .cmp(&b.year.is_none())
                .then(b.year.cmp(&a.year)),
            Sort::Size => b.size.cmp(&a.size),
        }
        .then_with(|| key_a.cmp(key_b))
        .then_with(|| by_title(a, b))
    });
    let total = kept.len();
    let items = kept
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(_, t)| t)
        .collect();
    Page {
        total,
        counts,
        items,
    }
}

/// The poster's address for a TMDB poster path or a title folder's own poster's artwork name.
pub(crate) fn poster(path: Option<String>) -> Option<String> {
    path.map(|p| format!("/api/v1/artwork/w342{p}"))
}

#[cfg(test)]
pub(crate) mod scale;
#[cfg(test)]
mod tests;
