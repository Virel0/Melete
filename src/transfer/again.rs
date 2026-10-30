//! Searching again for a title whose download proved not to be it, while it's still monitored:
//! the search its own page would make, and the best accepted result grabbed, as an RSS sync
//! would (the blocklisted release is passed over).

use super::*;

/// Brings one finished download home ([`transfer`]), and searches again for its title when
/// the download proved not to be it.
pub async fn bring_home(
    pool: &SqlitePool,
    seedbox: &Seedbox,
    fixes: Option<&Fixes>,
    sources: &crate::fulfil::Sources,
    id: i64,
) -> anyhow::Result<()> {
    if let Some(target) = transfer(pool, seedbox, fixes, id).await? {
        let note = search_again(pool, sources, target).await?;
        sqlx::query("UPDATE downloads SET message = message || '; ' || ? WHERE id = ?")
            .bind(note)
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Searches again for `target` when it's still monitored and downloads are on, grabbing the
/// best accepted result; says what came of it.
async fn search_again(
    pool: &SqlitePool,
    sources: &crate::fulfil::Sources,
    target: Target,
) -> anyhow::Result<String> {
    if !monitored(pool, target).await? {
        return Ok("it isn't monitored, so it wasn't searched for again".into());
    }
    if crate::runtime_checks::stopped(pool, crate::runtime_checks::title_of(target)).await? {
        return Ok(format!(
            "{} downloads of it didn't pass the running-time check, so Melete has stopped searching for it by itself; if its runtime is right after all, an admin can say so on its page",
            crate::runtime_checks::STOP_AFTER
        ));
    }
    let Some(pipeline) = sources.pipeline.as_deref() else {
        return Ok("downloads are off, so it wasn't searched for again".into());
    };
    let client = crate::torznab::Client::new()?;
    let search_id =
        match crate::search::search(pool, &client, sources.tmdb.as_deref(), target).await? {
            Ok(search_id) => search_id,
            Err(refused) => return Ok(format!("couldn't search again: {refused}")),
        };
    let mut grabbed = Vec::new();
    crate::rss::grab_for(pool, pipeline, target, search_id, &mut grabbed).await?;
    Ok(match grabbed.iter().find(|g| g.download_id.is_some()) {
        Some(g) => format!("searched again and grabbed {}", g.title),
        None => "searched again; nothing else to grab yet".into(),
    })
}

/// Whether the movie, or the series and its season, are still monitored.
async fn monitored(pool: &SqlitePool, target: Target) -> sqlx::Result<bool> {
    match target {
        Target::Movie(id) => {
            sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM wanted_movies WHERE id = ? AND monitored = 1)",
            )
            .bind(id)
            .fetch_one(pool)
            .await
        }
        Target::Series { id, season, .. } => {
            sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM wanted_series w WHERE w.id = ?1 AND w.monitored = 1
                     AND NOT EXISTS (SELECT 1 FROM wanted_seasons s
                                     WHERE s.wanted_series_id = w.id AND s.season = ?2
                                       AND s.monitored = 0))",
            )
            .bind(id)
            .bind(season)
            .fetch_one(pool)
            .await
        }
    }
}

/// Counts a download held back by the running-time check against its title, or starts the count
/// again when one is imported ([`crate::runtime_checks`]).
pub(super) async fn count_check(
    pool: &SqlitePool,
    row: &Row,
    id: i64,
    outcome: &Result<String, Stop>,
    check: Option<&verify::Check>,
) -> anyhow::Result<()> {
    let Some(title) = row.target().map(crate::runtime_checks::title_of) else {
        return Ok(());
    };
    match outcome {
        Ok(_) => crate::runtime_checks::reset(pool, title).await?,
        Err(Stop::NotTheTitle(why) | Stop::Held(why)) => {
            let seconds = check.and_then(|c| c.measured.lock().ok().and_then(|m| *m));
            let failure = crate::runtime_checks::Failure {
                at: unix_now(),
                download_id: id,
                release: row.title.clone(),
                reason: why.clone(),
            };
            crate::runtime_checks::record_failure(pool, title, failure, seconds).await?;
        }
        Err(_) => {}
    }
    Ok(())
}
