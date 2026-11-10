//! A season taken episode by episode: when a search for a season grabbed no pack of it, as when
//! an indexer has the season only as single episodes, the single episodes it found of the
//! season are taken for its missing episodes instead (those aired, monitored, not on disk and
//! nothing on its way, as a pack would find them), the best for each, each judged as its
//! episode's search would judge it and recorded as that episode's download. No other search is
//! made for them.

use super::*;

/// Takes, for each of the season's missing episodes, the best single episode of it the season's
/// search `search_id` found: refused only for being single episodes, holding none but missing
/// episodes, and grabbable as its episode ([`grab_kind`]). What it took, in `said`.
pub(super) async fn one_by_one(
    pool: &SqlitePool,
    pipeline: &Pipeline,
    (id, season): (i64, i64),
    wanted: &Wanted,
    search_id: i64,
    grabbed: &mut Vec<Grabbed>,
    said: &mut Said,
) -> anyhow::Result<()> {
    let mut missing: Vec<i64> = missing_episodes(pool, id, season).await?;
    if missing.is_empty() {
        return Ok(());
    }
    let results: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id, title, decision FROM release_decisions WHERE search_id = ? AND accepted = 0
           AND lower(title) NOT IN (SELECT lower(title) FROM blocklist)
         ORDER BY json_extract(decision, '$.rank') DESC NULLS LAST,
                  json_extract(decision, '$.score') DESC, seeders DESC NULLS LAST, id",
    )
    .bind(search_id)
    .fetch_all(pool)
    .await?;
    let mut took = Vec::new();
    for (release_id, title, decision) in results {
        let decision: serde_json::Value = serde_json::from_str(&decision)?;
        if !downloads::single_episodes_only(&decision) {
            continue;
        }
        let Episodes::Standard {
            season: s,
            episodes,
        } = release::parse(&title).episodes
        else {
            continue;
        };
        // All of it still missing: one already taken, or on disk, isn't fetched twice.
        if s != season || episodes.is_empty() || !episodes.iter().all(|e| missing.contains(e)) {
            continue;
        }
        let first = episodes.iter().copied().min().unwrap_or_default();
        let target = Target::Series {
            id,
            season,
            episode: Some(first),
        };
        if !matches!(
            grab_kind(pool, target, wanted, &title, &decision).await?,
            Ok(Kind::New)
        ) {
            continue;
        }
        let outcome =
            downloads::grab_episode(pool, &pipeline.deluge, &pipeline.fetcher, release_id, first)
                .await?;
        let (download_id, refused) = match outcome {
            Ok(download) => {
                missing.retain(|e| !episodes.contains(e));
                took.extend(episodes);
                (Some(download.id), None)
            }
            Err(refused) => (None, Some(refused.to_string())),
        };
        grabbed.push(Grabbed {
            release_id,
            title,
            upgrade: false,
            download_id,
            refused,
        });
        if missing.is_empty() {
            break;
        }
    }
    if !took.is_empty() {
        said.took = Some(format!(
            "no pack of it to take; took episodes {} one by one",
            numbers(&took)
        ));
    }
    Ok(())
}
