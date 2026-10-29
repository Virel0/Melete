//! The titles a list names, from TMDB, IMDb or Rotten Tomatoes, and the one TMDB title each is.

use super::*;

/// The titles a list names, in its order, up to `max`.
pub(super) async fn names(
    sources: &Sources,
    list: &List,
    series: bool,
    max: usize,
) -> anyhow::Result<Vec<Named>> {
    let first = max as u32;
    Ok(match list.provider.as_str() {
        "tmdb" => {
            let path = tmdb_path(&list.list, series)
                .ok_or_else(|| anyhow::anyhow!("{:?} isn't a TMDB list", list.list))?;
            let wanted_type = if series { "tv" } else { "movie" };
            let mut named = Vec::new();
            for page in 1..=MAX_PAGES {
                let Some(found) = sources.tmdb.list_page(&path, page).await? else {
                    anyhow::bail!("TMDB has no such list");
                };
                let pages = found.total_pages.unwrap_or(1);
                for entry in found.results.into_iter().chain(found.items) {
                    // Lists mix movies and series; charts are one kind.
                    if entry
                        .media_type
                        .as_deref()
                        .is_some_and(|t| t != wanted_type)
                    {
                        continue;
                    }
                    let title = entry.title.or(entry.name).unwrap_or_default();
                    named.push(Named::Tmdb(entry.id, title));
                }
                if named.len() >= max || page >= pages {
                    break;
                }
            }
            named.truncate(max);
            named
        }
        "imdb" => {
            let ids = match imdb_chart(&list.list, series) {
                Some(chart) => sources.imdb.chart(chart, first).await?,
                None => sources.imdb.list(&list.list, first).await?,
            };
            ids.into_iter().take(max).map(Named::Imdb).collect()
        }
        "rottentomatoes" => sources
            .rottentomatoes
            .browse(&list.list)
            .await?
            .into_iter()
            .filter(|t| t.series == series)
            .take(max)
            .map(Named::Titled)
            .collect(),
        other => anyhow::bail!("unknown provider {other}"),
    })
}

/// The one TMDB title a named title is; why it was skipped when it can't be pinned down.
pub(super) async fn pin(
    tmdb: &tmdb::Client,
    named: Named,
    series: bool,
) -> anyhow::Result<Result<(i64, String), Skipped>> {
    let skip = |title: String, why: String| Ok(Err(Skipped { title, why }));
    match named {
        Named::Tmdb(id, title) => Ok(Ok((id, title))),
        Named::Imdb(imdb_id) => {
            let found = tmdb.find(ExternalId::Imdb(&imdb_id)).await?;
            let hit = if series {
                found.tv_results.first().map(|s| (s.id, s.name.clone()))
            } else {
                found.movie_results.first().map(|m| (m.id, m.title.clone()))
            };
            match hit {
                Some(hit) => Ok(Ok(hit)),
                None => skip(
                    imdb_id,
                    format!(
                        "TMDB knows no {} by this IMDb id",
                        if series { "series" } else { "movie" }
                    ),
                ),
            }
        }
        Named::Titled(listed) => {
            let label = match listed.year {
                Some(year) => format!("{} ({year})", listed.title),
                None => listed.title.clone(),
            };
            let candidates: Vec<Candidate> = if series {
                tmdb.search_series(&listed.title, listed.year)
                    .await?
                    .into_iter()
                    .map(|s| Candidate {
                        tmdb_id: s.id,
                        year: s.first_air_date.as_deref().and_then(want::year_of),
                        title: s.name,
                        original_title: s.original_name,
                    })
                    .collect()
            } else {
                tmdb.search_movies(&listed.title, listed.year)
                    .await?
                    .into_iter()
                    .map(|m| Candidate {
                        tmdb_id: m.id,
                        year: m.release_date.as_deref().and_then(want::year_of),
                        title: m.title,
                        original_title: m.original_title,
                    })
                    .collect()
            };
            match metadata::choose(&listed.title, listed.year, &candidates) {
                Choice::One(id) => Ok(Ok((id, label))),
                Choice::Ambiguous(_) => skip(label, "more than one TMDB title fits".into()),
                Choice::Nothing => skip(label, "no TMDB title fits".into()),
            }
        }
    }
}
