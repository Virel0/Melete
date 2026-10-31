//! The second pool a viewer's picks come from, while "Trending and similar titles" is on: this
//! week's trending movies and series on TMDB that aren't in the library, which the viewer may
//! ask for. Only which chart leaves for TMDB (and, for a viewer with a highest rating, each
//! title's TMDB id to learn its rating, as discovery does); the model is told of each just what
//! it's told of a library title: an opaque id, title, year, up to three genres and a short
//! premise.

use crate::ai::Told;
use crate::tmdb::{self, Listed};

use super::*;

/// The most trending titles offered to the model.
const TRENDING: usize = 20;

/// A trending title not in the library, as a pick keeps it: by kind and TMDB id, with what an
/// app shows of it, and why it was picked.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Requestable {
    /// Whether it's a series; else a movie.
    pub series: bool,
    /// Its TMDB id.
    pub tmdb_id: i64,
    /// Its title as TMDB lists it.
    pub title: String,
    /// The year it came out, when known.
    pub year: Option<i64>,
    /// Its poster's file on TMDB's image server, such as `/abc.jpg`.
    pub poster_path: Option<String>,
    /// Why, as the model put it; empty until it's picked.
    pub reason: String,
}

/// TMDB's genre ids as names, for the genres a trending title is told with. These ids are
/// TMDB's fixed list, the same for every title.
const GENRES: [(i64, &str); 27] = [
    (28, "Action"),
    (12, "Adventure"),
    (16, "Animation"),
    (35, "Comedy"),
    (80, "Crime"),
    (99, "Documentary"),
    (18, "Drama"),
    (10751, "Family"),
    (14, "Fantasy"),
    (36, "History"),
    (27, "Horror"),
    (10402, "Music"),
    (9648, "Mystery"),
    (10749, "Romance"),
    (878, "Science Fiction"),
    (10770, "TV Movie"),
    (53, "Thriller"),
    (10752, "War"),
    (37, "Western"),
    (10759, "Action & Adventure"),
    (10762, "Kids"),
    (10763, "News"),
    (10764, "Reality"),
    (10765, "Sci-Fi & Fantasy"),
    (10766, "Soap"),
    (10767, "Talk"),
    (10768, "War & Politics"),
];

fn genre_names(ids: &[i64]) -> Vec<String> {
    ids.iter()
        .filter_map(|id| GENRES.iter().find(|(g, _)| g == id))
        .map(|(_, name)| (*name).to_owned())
        .collect()
}

/// Whether a title with this TMDB id is in the library (present, whoever may see it) or wanted:
/// it isn't offered to be asked for.
async fn known(pool: &SqlitePool, series: bool, tmdb_id: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar(if series {
        "SELECT EXISTS (SELECT 1 FROM series WHERE match_tmdb_id = ?1 AND missing_since IS NULL)
             OR EXISTS (SELECT 1 FROM wanted_series WHERE tmdb_id = ?1)"
    } else {
        "SELECT EXISTS (SELECT 1 FROM movies WHERE match_tmdb_id = ?1 AND missing_since IS NULL)
             OR EXISTS (SELECT 1 FROM wanted_movies WHERE tmdb_id = ?1)"
    })
    .bind(tmdb_id)
    .fetch_one(pool)
    .await
}

/// This week's trending movies and series that aren't in the library or wanted, and that
/// `access` may be shown (for a viewer with a highest rating, only those rated within it, their
/// ratings fetched first), each told by an id of its own from `c<first>` on. None while
/// trending and similar titles are off, without TMDB, or when TMDB doesn't answer.
pub(super) async fn candidates(
    pool: &SqlitePool,
    tmdb: Option<&tmdb::Client>,
    access: &Access,
    first: usize,
) -> anyhow::Result<Vec<Candidate>> {
    let Some(tmdb) = tmdb else {
        return Ok(Vec::new());
    };
    if !crate::settings::discovery(pool).await? {
        return Ok(Vec::new());
    }
    let mut listed: Vec<(bool, Listed)> = Vec::new();
    for series in [false, true] {
        match tmdb.trending(series, true).await {
            Ok(titles) => listed.extend(titles.into_iter().map(|t| (series, t))),
            Err(err) => tracing::warn!(
                error = format!("{err:#}"),
                "trending titles for AI picks couldn't be read"
            ),
        }
    }
    let mut fresh = Vec::new();
    for (series, title) in listed {
        if !known(pool, series, title.id).await? {
            fresh.push((series, title));
        }
    }
    if access.limits_rating() {
        let ids: Vec<(bool, i64)> = fresh.iter().map(|(s, t)| (*s, t.id)).collect();
        crate::ratings::fetch_unknown(pool, tmdb, &ids).await?;
    }
    let mut out = Vec::new();
    for (series, title) in fresh {
        if out.len() == TRENDING {
            break;
        }
        if !access.rates_tmdb(pool, series, title.id).await? {
            continue;
        }
        let (name, date) = if series {
            (title.name.or(title.title), title.first_air_date)
        } else {
            (title.title, title.release_date)
        };
        let Some(name) = name.filter(|n| !n.is_empty()) else {
            continue;
        };
        let year = date.as_deref().and_then(crate::want::year_of);
        let id = format!("c{}", first + out.len());
        let told = Told::new(
            Some(id),
            &name,
            year,
            &genre_names(&title.genre_ids),
            title.overview.as_deref(),
        );
        out.push(Candidate {
            title: Title {
                series,
                id: title.id,
            },
            told,
            requestable: Some(Requestable {
                series,
                tmdb_id: title.id,
                title: name,
                year,
                poster_path: title.poster_path,
                reason: String::new(),
            }),
        });
    }
    Ok(out)
}

/// A requestable pick as served: still not in the library or wanted, and still one `access`
/// may be shown.
pub(super) async fn still_requestable(
    pool: &SqlitePool,
    access: &Access,
    pick: &Requestable,
) -> sqlx::Result<bool> {
    Ok(!known(pool, pick.series, pick.tmdb_id).await?
        && access.rates_tmdb(pool, pick.series, pick.tmdb_id).await?)
}
