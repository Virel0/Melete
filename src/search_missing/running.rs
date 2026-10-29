//! A run: one search at a time at the pace, leaving out indexers that reach their limit.

use super::*;

/// Whether an indexer's answer says it has reached its limit: HTTP 429, or a Torznab error
/// about a limit (codes 500 and 501 are the request and download limits).
pub(crate) fn says_limit(error: &str) -> bool {
    let error = error.to_lowercase();
    error.contains("answered 429") || (error.contains("reported error") && error.contains("limit"))
}

pub(super) enum Ended {
    Done(String),
    Stopped(String),
}

#[derive(Default)]
pub(super) struct Counts {
    pub(super) done: i64,
    pub(super) searched: i64,
    pub(super) grabbed: i64,
    pub(super) notes: Vec<String>,
    /// Notes past those kept.
    pub(super) more_notes: usize,
}

impl Counts {
    pub(super) fn note(&mut self, note: String) {
        if self.notes.len() < NOTES_KEPT {
            self.notes.push(note);
        } else {
            self.more_notes += 1;
        }
    }

    pub(super) fn summary(&self, total: usize) -> String {
        let mut said = format!(
            "{} of {total} searched for, {} grabbed",
            self.searched, self.grabbed
        );
        if self.more_notes > 0 {
            said.push_str(&format!(" ({} more notes not kept)", self.more_notes));
        }
        said
    }
}

pub(super) async fn record(
    pool: &SqlitePool,
    id: i64,
    counts: &Counts,
    current: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE missing_searches SET done = ?, searched = ?, grabbed = ?, notes = ?, current = ?
         WHERE id = ?",
    )
    .bind(counts.done)
    .bind(counts.searched)
    .bind(counts.grabbed)
    .bind(serde_json::to_string(&counts.notes)?)
    .bind(current)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Waits until `until`, ending early when the run is asked to stop; true when it was.
pub(super) async fn wait_or_stop(pool: &SqlitePool, id: i64, until: Instant) -> sqlx::Result<bool> {
    loop {
        if stopping(pool, id).await? {
            return Ok(true);
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(false);
        }
        tokio::time::sleep(left.min(Duration::from_secs(1))).await;
    }
}

pub(super) async fn run(
    pool: &SqlitePool,
    sources: &Sources,
    id: i64,
    area: Area,
    goal: Goal,
) -> anyhow::Result<Ended> {
    let items = to_do(pool, area, goal).await?;
    let total = items.len();
    sqlx::query("UPDATE missing_searches SET total = ? WHERE id = ?")
        .bind(i64::try_from(total)?)
        .bind(id)
        .execute(pool)
        .await?;
    let client = torznab::Client::new()?;
    let music = artist_search::Sources {
        slskd: sources.slskd.clone(),
        pipeline: sources.pipeline.clone(),
    };
    let grabbing = match area {
        Area::Albums => music.pipeline.is_some() || music.slskd.is_some(),
        _ => sources.pipeline.is_some(),
    };
    let mut counts = Counts::default();
    if !grabbing {
        counts.note(
            "downloads are off (MELETE_DOWNLOADS): results are kept, nothing is grabbed".into(),
        );
    }
    let mut next_search = Instant::now();
    // Indexers found at their limit: left out of the rest of the run, by id and by name.
    let mut skipped: Vec<i64> = Vec::new();
    let mut limited: Vec<String> = Vec::new();
    for item in items {
        if wait_or_stop(pool, id, next_search).await? {
            return Ok(Ended::Stopped(format!(
                "asked to stop; {}",
                counts.summary(total)
            )));
        }
        let name = item.name();
        if !still_to_do(pool, goal, &item).await? {
            counts.done += 1;
            record(pool, id, &counts, None).await?;
            continue;
        }
        record(pool, id, &counts, Some(&name)).await?;
        let searched = search_item(pool, &client, sources, &music, &item, &skipped).await?;
        counts.done += 1;
        let search_id = match searched {
            Ok(search_id) => search_id,
            // Nothing else in the area can be searched for either.
            Err(Refused::NoIndexers) if !limited.is_empty() => {
                record(pool, id, &counts, None).await?;
                return Ok(Ended::Stopped(format!(
                    "every indexer has reached its limit ({}); start again later, or turn one \
                     off under Indexers; {}",
                    limited.join(", "),
                    counts.summary(total)
                )));
            }
            Err(Refused::NoIndexers) => {
                record(pool, id, &counts, None).await?;
                return Ok(Ended::Stopped(format!(
                    "{}; {}",
                    Refused::NoIndexers,
                    counts.summary(total)
                )));
            }
            Err(refused) => {
                counts.note(format!("{name}: {refused}"));
                record(pool, id, &counts, None).await?;
                continue;
            }
        };
        // It went out to the indexers: the next waits its turn. A refused one asked no one.
        next_search = Instant::now() + sources.pace;
        counts.searched += 1;
        let grabbed = grab_item(pool, sources, &music, grabbing, &item, search_id).await?;
        if grabbed.iter().any(|g| g.download_id.is_some()) {
            counts.grabbed += 1;
        } else if let Some(why) = grabbed.iter().rev().find_map(|g| g.refused.clone()) {
            counts.note(format!("{name}: not grabbed: {why}"));
        }
        record(pool, id, &counts, None).await?;
        leave_out_limited(pool, id, search_id, &mut counts, &mut skipped, &mut limited).await?;
    }
    Ok(Ended::Done(counts.summary(total)))
}

/// Searches for one item as its own search would, leaving out the indexers in `skipped`.
async fn search_item(
    pool: &SqlitePool,
    client: &torznab::Client,
    sources: &Sources,
    music: &artist_search::Sources,
    item: &Item,
    skipped: &[i64],
) -> anyhow::Result<Result<i64, Refused>> {
    Ok(match (item, item.target()) {
        (Item::Album { id, .. }, _) => {
            album_search::search_skipping(pool, client, music.slskd.as_deref(), *id, skipped)
                .await?
        }
        (_, Some(target)) => {
            search::search_skipping(pool, client, sources.tmdb.as_deref(), target, skipped).await?
        }
        (_, None) => Err(Refused::NotWanted),
    })
}

/// Grabs the best accepted result of an item's search, when downloads are on for it.
async fn grab_item(
    pool: &SqlitePool,
    sources: &Sources,
    music: &artist_search::Sources,
    grabbing: bool,
    item: &Item,
    search_id: i64,
) -> anyhow::Result<Vec<Grabbed>> {
    let mut grabbed: Vec<Grabbed> = Vec::new();
    match (item, item.target(), &sources.pipeline) {
        (Item::Album { .. }, ..) if grabbing => {
            artist_search::grab_best(pool, music, search_id, &mut grabbed).await?;
        }
        (_, Some(target), Some(pipeline)) => {
            crate::rss::grab_for(pool, pipeline, target, search_id, &mut grabbed).await?;
        }
        _ => {}
    }
    Ok(grabbed)
}

/// Leaves the indexers that said in this search they reached their limit out of the rest of
/// the run, with a note for each.
async fn leave_out_limited(
    pool: &SqlitePool,
    id: i64,
    search_id: i64,
    counts: &mut Counts,
    skipped: &mut Vec<i64>,
    limited: &mut Vec<String>,
) -> anyhow::Result<()> {
    // An indexer at its limit isn't asked again by this run; the rest carry on.
    let indexers: String = sqlx::query_scalar("SELECT indexers FROM searches WHERE id = ?")
        .bind(search_id)
        .fetch_one(pool)
        .await?;
    let outcomes: Vec<IndexerOutcome> = serde_json::from_str(&indexers)?;
    for outcome in outcomes {
        if outcome.error.as_deref().is_some_and(says_limit)
            && !skipped.contains(&outcome.indexer_id)
        {
            skipped.push(outcome.indexer_id);
            counts.note(format!(
                "{} says it has reached its limit; not asked again by this run",
                outcome.name
            ));
            limited.push(outcome.name);
            record(pool, id, counts, None).await?;
        }
    }
    Ok(())
}
