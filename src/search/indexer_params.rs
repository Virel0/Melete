//! The turned-on indexers, and the parameters each is asked a search with.

use super::*;

pub(crate) async fn enabled_indexers(pool: &SqlitePool) -> anyhow::Result<Vec<IndexerRow>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: i64,
        name: String,
        url: String,
        api_key: String,
        categories: String,
        movie: String,
        tv: String,
        kind: String,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, url, api_key, categories, movie_search_params AS movie,
                tv_search_params AS tv, kind
         FROM indexers WHERE enabled = 1 AND turned_off = 0 ORDER BY name, id",
    )
    .fetch_all(pool)
    .await?;
    let sealing = crate::secrets::sealing(pool).await?;
    rows.into_iter()
        .map(
            |Row {
                 id,
                 name,
                 url,
                 api_key,
                 categories,
                 movie,
                 tv,
                 kind,
             }| {
                Ok(IndexerRow {
                    id,
                    name,
                    url,
                    api_key: crate::secrets::from_store(
                        sealing,
                        crate::secrets::INDEXER_KEY,
                        &api_key,
                    )?,
                    kind: torznab::Kind::parse(&kind).ok_or_else(|| {
                        anyhow::anyhow!("indexer {id} is of an unknown kind {kind:?}")
                    })?,
                    categories: serde_json::from_str(&categories)?,
                    movie_params: serde_json::from_str(&movie)?,
                    tv_params: serde_json::from_str(&tv)?,
                })
            },
        )
        .collect()
}

/// The Torznab query for one indexer: by id where it takes one, else by title.
pub(super) fn params(
    indexer: &IndexerRow,
    wanted: &Wanted,
    target: Target,
) -> Result<Vec<(&'static str, String)>, String> {
    let title = wanted.titles.first().cloned().unwrap_or_default();
    let (range, kind, supported) = match target {
        Target::Movie(_) => (2000..3000, "movie", &indexer.movie_params),
        Target::Series { .. } => (5000..6000, "TV", &indexer.tv_params),
    };
    let categories: Vec<String> = indexer
        .categories
        .iter()
        .filter(|c| range.contains(*c))
        .map(i64::to_string)
        .collect();
    if categories.is_empty() {
        return Err(format!("it has no {kind} categories"));
    }
    let takes = |param: &str| supported.iter().any(|p| p == param);
    let mut params = vec![("cat", categories.join(","))];
    match target {
        Target::Movie(_) => {
            let imdb = wanted
                .imdb_id
                .as_deref()
                .map(|i| i.trim_start_matches("tt").to_owned());
            if let (true, Some(imdb)) = (takes("imdbid"), imdb) {
                params.extend([("t", "movie".into()), ("imdbid", imdb)]);
            } else if let (true, Some(tmdb)) = (takes("tmdbid"), wanted.tmdb_id) {
                params.extend([("t", "movie".into()), ("tmdbid", tmdb.to_string())]);
            } else {
                let q = match wanted.year {
                    Some(year) => format!("{title} {year}"),
                    None => title,
                };
                let t = if takes("q") { "movie" } else { "search" };
                params.extend([("t", t.into()), ("q", q)]);
            }
        }
        Target::Series {
            season, episode, ..
        } => {
            let numbered = takes("season") && (episode.is_none() || takes("ep"));
            match (takes("tvdbid"), wanted.tvdb_id, numbered) {
                (true, Some(tvdb), true) => {
                    params.extend([("t", "tvsearch".into()), ("tvdbid", tvdb.to_string())])
                }
                _ if takes("q") && numbered => {
                    params.extend([("t", "tvsearch".into()), ("q", title.clone())])
                }
                _ => {
                    // No numbered search: put the numbers in the text, as release names have them.
                    let numbers = match episode {
                        Some(e) => format!("S{season:02}E{e:02}"),
                        None => format!("S{season:02}"),
                    };
                    params.extend([("t", "search".into()), ("q", format!("{title} {numbers}"))]);
                    return Ok(params);
                }
            }
            params.push(("season", season.to_string()));
            if let Some(e) = episode {
                params.push(("ep", e.to_string()));
            }
        }
    }
    Ok(params)
}
