//! Asking every turned-on indexer at once for its newest releases.

use super::*;

/// What each indexer answered, in the indexers' order, with the releases it sent.
pub(super) type Outcomes = Vec<(IndexerOutcome, Vec<torznab::Item>)>;

/// Asks every indexer at once for its newest movie and TV releases, and in a request of its
/// own its newest music when albums are wanted, so video doesn't crowd music out.
pub(super) async fn ask_indexers(
    client: &torznab::Client,
    indexers: &[search::IndexerRow],
    albums_wanted: bool,
) -> anyhow::Result<Outcomes> {
    let mut tasks = tokio::task::JoinSet::new();
    for (position, indexer) in indexers.iter().enumerate() {
        let (video, music) = categories(indexer, albums_wanted);
        let none = if albums_wanted {
            "it has no movie, TV or music categories"
        } else {
            "it has no movie or TV categories"
        };
        let client = client.clone();
        let (url, api_key, kind) = (indexer.url.clone(), indexer.api_key.clone(), indexer.kind);
        tasks.spawn(async move {
            if video.is_empty() && music.is_empty() {
                return (position, Err(none.to_owned()));
            }
            let endpoint = torznab::Endpoint {
                url: &url,
                api_key: &api_key,
            };
            (
                position,
                newest(&client, kind, &endpoint, [video, music]).await,
            )
        });
    }
    let mut outcomes: Vec<Option<(IndexerOutcome, Vec<torznab::Item>)>> =
        indexers.iter().map(|_| None).collect();
    while let Some(joined) = tasks.join_next().await {
        let (position, result) = joined?;
        let indexer = &indexers[position];
        let (error, items) = match result {
            Ok((items, partly)) => (partly, items),
            Err(error) => (Some(error), Vec::new()),
        };
        let outcome = IndexerOutcome {
            indexer_id: indexer.id,
            name: indexer.name.clone(),
            results: items.len(),
            error,
        };
        outcomes[position] = Some((outcome, items));
    }
    Ok(outcomes.into_iter().flatten().collect())
}

/// The categories an indexer is asked for: its movie and TV ones, and its music ones when
/// albums are wanted.
fn categories(indexer: &search::IndexerRow, albums_wanted: bool) -> (Vec<String>, Vec<String>) {
    let in_range = |ranges: &[(i64, i64)]| -> Vec<String> {
        indexer
            .categories
            .iter()
            .filter(|c| ranges.iter().any(|&(from, to)| (from..to).contains(*c)))
            .map(i64::to_string)
            .collect()
    };
    let video = in_range(&[(2000, 3000), (5000, 6000)]);
    let music = if albums_wanted {
        in_range(&[(3000, 4000)])
    } else {
        Vec::new()
    };
    (video, music)
}

/// An indexer's newest releases in each list of categories that isn't empty, a request each:
/// what failed alongside what came back, or only what failed when nothing came back.
async fn newest(
    client: &torznab::Client,
    kind: torznab::Kind,
    endpoint: &torznab::Endpoint<'_>,
    lists: [Vec<String>; 2],
) -> Result<(Vec<torznab::Item>, Option<String>), String> {
    let (mut items, mut errors) = (Vec::new(), Vec::new());
    for categories in lists.into_iter().filter(|c| !c.is_empty()) {
        let params = [("t", "search"), ("cat", &categories.join(","))];
        match client.search_as(kind, endpoint, &params).await {
            Ok(found) => items.extend(found),
            Err(err) => errors.push(err.to_string()),
        }
    }
    let error = (!errors.is_empty()).then(|| errors.join("; "));
    match error {
        Some(error) if items.is_empty() => Err(error),
        error => Ok((items, error)),
    }
}
