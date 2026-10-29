//! Matching one movie or series to TMDB automatically, and what's recorded when that fails.

use super::*;

/// Logs a failed title and counts it, unless TMDB refused the token or asked for a long
/// pause, which end the run.
pub(super) fn skip(
    err: anyhow::Error,
    what: &str,
    title: &str,
    report: &mut RefreshReport,
) -> anyhow::Result<()> {
    if err.is::<Unauthorized>() || err.is::<RateLimited>() {
        return Err(err);
    }
    tracing::warn!(
        what,
        title,
        error = format!("{err:#}"),
        "TMDB request failed; will retry next run"
    );
    report.errors += 1;
    Ok(())
}

pub(super) async fn match_movie(
    tmdb: &tmdb::Client,
    row: &TitleRow,
) -> anyhow::Result<Outcome<MovieDetails>> {
    if let Some(id) = row.tmdb_id
        && let Some(details) = tmdb.movie(id).await?
    {
        return Ok(Outcome::Matched {
            details,
            method: "tmdb_tag",
        });
    }
    if let Some(imdb) = &row.imdb_id
        && let Some(found) = tmdb
            .find(ExternalId::Imdb(imdb))
            .await?
            .movie_results
            .first()
        && let Some(details) = tmdb.movie(found.id).await?
    {
        return Ok(Outcome::Matched {
            details,
            method: "imdb_tag",
        });
    }
    if let Some(ids) = nfo_ids(row)
        && let Some(outcome) = movie_by_nfo(tmdb, &ids).await?
    {
        return Ok(outcome);
    }

    let to_candidates = |results: Vec<tmdb::MovieResult>| -> Vec<Candidate> {
        results
            .into_iter()
            .map(|r| Candidate {
                tmdb_id: r.id,
                year: year_of(r.release_date.as_deref()),
                title: r.title,
                original_title: r.original_title,
            })
            .collect()
    };
    let mut choice = choose(
        &row.parsed_title,
        row.parsed_year,
        &to_candidates(
            tmdb.search_movies(&row.parsed_title, row.parsed_year)
                .await?,
        ),
    );
    if choice == Choice::Nothing && row.parsed_year.is_some() {
        // TMDB's year filter is strict; the folder year may be off by one.
        choice = choose(
            &row.parsed_title,
            row.parsed_year,
            &to_candidates(tmdb.search_movies(&row.parsed_title, None).await?),
        );
    }
    Ok(match choice {
        Choice::One(id) => match tmdb.movie(id).await? {
            Some(details) => Outcome::Matched {
                details,
                method: "search",
            },
            None => bail!("TMDB search returned movie {id}, which has no details"),
        },
        Choice::Ambiguous(candidates) => Outcome::Ambiguous(candidates),
        Choice::Nothing => Outcome::Unmatched,
    })
}

pub(super) async fn match_series(
    tmdb: &tmdb::Client,
    row: &TitleRow,
) -> anyhow::Result<Outcome<(SeriesDetails, Vec<Season>)>> {
    if let Some(id) = row.tmdb_id
        && let Some(details) = fetch_series(tmdb, id).await?
    {
        return Ok(Outcome::Matched {
            details,
            method: "tmdb_tag",
        });
    }
    let tagged = [
        (row.tvdb_id.map(ExternalId::Tvdb), "tvdb_tag"),
        (row.imdb_id.as_deref().map(ExternalId::Imdb), "imdb_tag"),
    ];
    for (id, method) in tagged {
        if let Some(id) = id
            && let Some(found) = tmdb.find(id).await?.tv_results.first()
            && let Some(details) = fetch_series(tmdb, found.id).await?
        {
            return Ok(Outcome::Matched { details, method });
        }
    }
    if let Some(ids) = nfo_ids(row)
        && let Some(outcome) = series_by_nfo(tmdb, &ids).await?
    {
        return Ok(outcome);
    }

    let to_candidates = |results: Vec<tmdb::SeriesResult>| -> Vec<Candidate> {
        results
            .into_iter()
            .map(|r| Candidate {
                tmdb_id: r.id,
                year: year_of(r.first_air_date.as_deref()),
                title: r.name,
                original_title: r.original_name,
            })
            .collect()
    };
    let mut choice = choose(
        &row.parsed_title,
        row.parsed_year,
        &to_candidates(
            tmdb.search_series(&row.parsed_title, row.parsed_year)
                .await?,
        ),
    );
    if choice == Choice::Nothing && row.parsed_year.is_some() {
        choice = choose(
            &row.parsed_title,
            row.parsed_year,
            &to_candidates(tmdb.search_series(&row.parsed_title, None).await?),
        );
    }
    Ok(match choice {
        Choice::One(id) => match fetch_series(tmdb, id).await? {
            Some(details) => Outcome::Matched {
                details,
                method: "search",
            },
            None => bail!("TMDB search returned series {id}, which has no details"),
        },
        Choice::Ambiguous(candidates) => Outcome::Ambiguous(candidates),
        Choice::Nothing => Outcome::Unmatched,
    })
}

/// A series with all its seasons, or `None` if TMDB has no such series.
pub(super) async fn fetch_series(
    tmdb: &tmdb::Client,
    id: i64,
) -> anyhow::Result<Option<(SeriesDetails, Vec<Season>)>> {
    let Some(details) = tmdb.series(id).await? else {
        return Ok(None);
    };
    let numbers: Vec<i64> = details.seasons.iter().map(|s| s.season_number).collect();
    let seasons = tmdb.seasons(id, &numbers).await?;
    Ok(Some((details, seasons)))
}

pub(super) async fn record_miss<D>(
    pool: &SqlitePool,
    kind: Kind,
    row: &TitleRow,
    outcome: Outcome<D>,
    now: i64,
    report: &mut RefreshReport,
) -> anyhow::Result<()> {
    match outcome {
        Outcome::Ambiguous(candidates) => {
            let candidates = serde_json::to_string(&candidates)?;
            if set_auto_match(pool, kind, row, None, Some(&candidates), now).await? {
                report.ambiguous += 1;
            }
        }
        Outcome::Unmatched => {
            if set_auto_match(pool, kind, row, None, None, now).await? {
                report.unmatched += 1;
            }
        }
        Outcome::Matched { .. } => unreachable!("matches are stored by the caller"),
    }
    Ok(())
}
