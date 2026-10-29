//! Matching by the ids in a title folder's `.nfo` files: used after an *arr id and an id in
//! the folder name, before searching, and never over a match made by hand. Every id is looked
//! up; one title for all of them is a match, more than one is left for someone to choose.

use super::*;
use crate::library::NfoIds;

/// The `.nfo` ids a scan stored for the title, if any.
pub(super) fn nfo_ids(row: &TitleRow) -> Option<NfoIds> {
    let ids: NfoIds = serde_json::from_str(row.nfo_ids.as_deref()?).ok()?;
    (!ids.is_empty()).then_some(ids)
}

/// What the titles the ids lead to make: none is no answer (searching goes on), one a match,
/// more an ambiguous title with them as candidates.
fn decide<D>(found: Vec<D>, candidate: impl Fn(&D) -> Candidate) -> Option<Outcome<D>> {
    match found.len() {
        0 => None,
        1 => found.into_iter().next().map(|details| Outcome::Matched {
            details,
            method: "nfo",
        }),
        _ => Some(Outcome::Ambiguous(found.iter().map(candidate).collect())),
    }
}

/// The movie the ids lead to: TMDB ids looked up directly, IMDb ids through TMDB's find unless
/// a movie already found has that IMDb id.
pub(super) async fn movie_by_nfo(
    tmdb: &tmdb::Client,
    ids: &NfoIds,
) -> anyhow::Result<Option<Outcome<MovieDetails>>> {
    let mut found: Vec<MovieDetails> = Vec::new();
    for &id in &ids.tmdb {
        if let Some(details) = tmdb.movie(id).await?
            && !found.iter().any(|f| f.id == details.id)
        {
            found.push(details);
        }
    }
    for imdb in &ids.imdb {
        if found.iter().any(|f| f.imdb_id.as_deref() == Some(imdb)) {
            continue;
        }
        if let Some(result) = tmdb
            .find(ExternalId::Imdb(imdb))
            .await?
            .movie_results
            .first()
            && !found.iter().any(|f| f.id == result.id)
            && let Some(details) = tmdb.movie(result.id).await?
        {
            found.push(details);
        }
    }
    Ok(decide(found, |d| Candidate {
        tmdb_id: d.id,
        title: d.title.clone(),
        original_title: d.original_title.clone(),
        year: year_of(d.release_date.as_deref()),
    }))
}

/// The series the ids lead to, with its seasons when it's the only one: TMDB ids looked up
/// directly, TVDB and IMDb ids through TMDB's find unless a series already found has them.
pub(super) async fn series_by_nfo(
    tmdb: &tmdb::Client,
    ids: &NfoIds,
) -> anyhow::Result<Option<Outcome<(SeriesDetails, Vec<Season>)>>> {
    let mut found: Vec<SeriesDetails> = Vec::new();
    for &id in &ids.tmdb {
        if let Some(details) = tmdb.series(id).await?
            && !found.iter().any(|f| f.id == details.id)
        {
            found.push(details);
        }
    }
    let external = |f: &SeriesDetails| {
        let ids = f.external_ids.as_ref();
        (
            ids.and_then(|i| i.tvdb_id),
            ids.and_then(|i| i.imdb_id.clone()),
        )
    };
    let elsewhere = ids
        .tvdb
        .iter()
        .map(|&id| (ExternalId::Tvdb(id), Some(id), None))
        .chain(
            ids.imdb
                .iter()
                .map(|id| (ExternalId::Imdb(id), None, Some(id.clone()))),
        );
    for (id, tvdb, imdb) in elsewhere {
        let known = found.iter().any(|f| {
            let (has_tvdb, has_imdb) = external(f);
            (tvdb.is_some() && has_tvdb == tvdb) || (imdb.is_some() && has_imdb == imdb)
        });
        if known {
            continue;
        }
        if let Some(result) = tmdb.find(id).await?.tv_results.first()
            && !found.iter().any(|f| f.id == result.id)
            && let Some(details) = tmdb.series(result.id).await?
        {
            found.push(details);
        }
    }
    let candidate = |d: &SeriesDetails| Candidate {
        tmdb_id: d.id,
        title: d.name.clone(),
        original_title: d.original_name.clone(),
        year: year_of(d.first_air_date.as_deref()),
    };
    Ok(match decide(found, candidate) {
        Some(Outcome::Matched { details, method }) => {
            let numbers: Vec<i64> = details.seasons.iter().map(|s| s.season_number).collect();
            let seasons = tmdb.seasons(details.id, &numbers).await?;
            Some(Outcome::Matched {
                details: (details, seasons),
                method,
            })
        }
        Some(Outcome::Ambiguous(candidates)) => Some(Outcome::Ambiguous(candidates)),
        Some(Outcome::Unmatched) | None => None,
    })
}
