//! AI picks, while an admin has them on (`MELETE_AI_PICKS`, off by default) and OpenRouter is
//! set up: "Featured for you", chosen for each viewer from what they watched, and titles like
//! one title.
//!
//! What's sent to OpenRouter (through `crate::ai`, the only code that talks to it): for a
//! viewer's picks, the titles they finished or are watching (about 15, an episode counting as
//! its series) with their year and up to three genres; for titles like one, that title with
//! its premise. With either, candidates from the library that the asking user may see, each by
//! an opaque id with its title, year, up to three genres and a premise cut to about 12 words.
//! Never a username, an account, a path or anything else about the household.
//!
//! Picks are kept, served at once, and worked out again in the background: a viewer's when what
//! they watched changes or after a day, titles like one after a week. Making them starts at most
//! every 10 minutes for each, which also bounds what it costs. New picks take the old ones'
//! place only once they're made, never while someone reads them. Off, not set up or failing,
//! there are none, and the answer says why.

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::access::Access;
use crate::ai::Client;
use crate::db::unix_now;

mod gather;
mod stored;

use gather::*;
use stored::*;

/// The most picks a viewer gets, and the most titles like one.
pub const PICKS: usize = 12;
/// How long a viewer's picks stand while what they watched stays the same.
const PICKS_FOR: i64 = 24 * 3600;
/// How long titles like one stand.
const SIMILAR_FOR: i64 = 7 * 24 * 3600;
/// Making picks starts at most this often for each viewer, or each title.
const TRY_EVERY: i64 = 10 * 60;

/// A movie or a series, by its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Title {
    /// Whether it's a series; else a movie.
    pub series: bool,
    /// The movie's or series' id.
    pub id: i64,
}

/// A title picked, and why in a few words.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pick {
    /// The title.
    #[serde(flatten)]
    pub title: Title,
    /// Why, as the model put it.
    pub reason: String,
}

/// Where a viewer's picks stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// An admin hasn't turned them on.
    Off,
    /// They're on, but OpenRouter isn't set up.
    Unavailable,
    /// Nothing watched yet to pick from.
    NoHistory,
    /// Being made for the first time.
    Working,
    /// Made; these are them.
    Ready,
    /// Making them failed, and there are none from before; why.
    Failed(String),
}

/// A viewer's picks as served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    /// Where they stand.
    pub status: Status,
    /// When they were made.
    pub made_at: Option<i64>,
    /// The picks still there that the viewer may see and hasn't watched, best first.
    pub picks: Vec<Pick>,
}

impl Served {
    fn none(status: Status) -> Served {
        Served {
            status,
            made_at: None,
            picks: Vec::new(),
        }
    }
}

/// Whether an admin turned AI picks on (`MELETE_AI_PICKS`); off by default.
pub async fn on(pool: &SqlitePool) -> sqlx::Result<bool> {
    Ok(crate::settings::value(pool, "MELETE_AI_PICKS")
        .await?
        .as_deref()
        == Some("on"))
}

/// A viewer's picks as they stand, starting to make them afresh in the background when what
/// they watched changed or they're a day old. Nothing is sent while they're off.
pub async fn for_viewer(
    pool: &SqlitePool,
    client: Option<&Arc<Client>>,
    user: i64,
    access: &Access,
) -> anyhow::Result<Served> {
    if !on(pool).await? {
        return Ok(Served::none(Status::Off));
    }
    let Some(client) = client else {
        return Ok(Served::none(Status::Unavailable));
    };
    let watched = history(pool, user, access).await?;
    if watched.is_empty() {
        return Ok(Served::none(Status::NoHistory));
    }
    let key = history_key(&watched);
    let kept = viewer_row(pool, user).await?;
    let now = unix_now();
    let due = kept
        .as_ref()
        .is_none_or(|k| k.history != key || k.made_at.is_none_or(|at| now - at >= PICKS_FOR));
    if due && claim_viewer(pool, user, &key, now - TRY_EVERY).await? {
        let (pool, client, access) = (pool.clone(), client.clone(), access.clone());
        tokio::spawn(async move {
            if let Err(err) = make_for_viewer(&pool, &client, user, &access).await {
                tracing::warn!(error = format!("{err:#}"), "AI picks couldn't be made");
            }
        });
    }
    let kept = viewer_row(pool, user).await?.unwrap_or_default();
    let picks = still_there(pool, Some(user), access, &kept.picks).await?;
    let status = match (&kept.made_at, &kept.error) {
        (Some(_), _) => Status::Ready,
        (None, Some(error)) => Status::Failed(error.clone()),
        (None, None) => Status::Working,
    };
    Ok(Served {
        status,
        made_at: kept.made_at,
        picks,
    })
}

/// Makes a viewer's picks now and keeps them; on failure keeps why, and the picks from before.
pub async fn make_for_viewer(
    pool: &SqlitePool,
    client: &Client,
    user: i64,
    access: &Access,
) -> anyhow::Result<()> {
    let watched = history(pool, user, access).await?;
    let key = history_key(&watched);
    let candidates = candidates_for_viewer(pool, user, access, &watched).await?;
    let told: Vec<_> = watched.iter().map(|w| w.told.clone()).collect();
    let picked = if candidates.is_empty() {
        Ok(Vec::new())
    } else {
        let offered: Vec<_> = candidates.iter().map(|c| c.told.clone()).collect();
        client.pick(&told, &offered, PICKS).await
    };
    match picked {
        Ok(picked) => {
            let picks = resolve(&candidates, picked);
            keep_viewer(pool, user, &key, &picks).await?;
            Ok(())
        }
        Err(failure) => {
            fail_viewer(pool, user, &failure.to_string()).await?;
            Err(failure.into())
        }
    }
}

/// Titles like `title` from the AI, best first, that `access` sees and `user` hasn't watched;
/// None while AI picks are off or not set up, or until they've been made (they're started in
/// the background), so the caller goes by Melete's own likeness. Made once for everyone who
/// sees the same titles.
pub async fn like(
    pool: &SqlitePool,
    client: Option<&Arc<Client>>,
    title: Title,
    user: i64,
    access: &Access,
) -> anyhow::Result<Option<Vec<Pick>>> {
    let (true, Some(client)) = (on(pool).await?, client) else {
        return Ok(None);
    };
    let seen = access.key();
    let kept = similar_row(pool, title, &seen).await?;
    let now = unix_now();
    let due = kept
        .as_ref()
        .is_none_or(|k| k.made_at.is_none_or(|at| now - at >= SIMILAR_FOR));
    if due && claim_similar(pool, title, &seen, now - TRY_EVERY).await? {
        let (pool, client, access) = (pool.clone(), client.clone(), access.clone());
        tokio::spawn(async move {
            if let Err(err) = make_like(&pool, &client, title, &access).await {
                tracing::warn!(
                    error = format!("{err:#}"),
                    "AI titles like one couldn't be made"
                );
            }
        });
    }
    match kept.filter(|k| k.made_at.is_some()) {
        Some(kept) => Ok(Some(
            still_there(pool, Some(user), access, &kept.picks).await?,
        )),
        None => Ok(None),
    }
}

/// Makes the titles like `title` for those who see what `access` sees, now, and keeps them.
pub async fn make_like(
    pool: &SqlitePool,
    client: &Client,
    title: Title,
    access: &Access,
) -> anyhow::Result<()> {
    let seen = access.key();
    let Some(told) = told_of(pool, title).await? else {
        return Ok(());
    };
    let candidates = candidates_like(pool, access, title).await?;
    let picked = if candidates.is_empty() {
        Ok(Vec::new())
    } else {
        let offered: Vec<_> = candidates.iter().map(|c| c.told.clone()).collect();
        client.similar(&told, &offered, PICKS).await
    };
    match picked {
        Ok(picked) => {
            keep_similar(pool, title, &seen, &resolve(&candidates, picked)).await?;
            Ok(())
        }
        Err(failure) => {
            fail_similar(pool, title, &seen, &failure.to_string()).await?;
            Err(failure.into())
        }
    }
}

/// The picks as titles: each candidate's id taken back to the title it stood for.
fn resolve(candidates: &[Candidate], picked: Vec<crate::ai::Picked>) -> Vec<Pick> {
    picked
        .into_iter()
        .filter_map(|p| {
            let c = candidates
                .iter()
                .find(|c| c.told.id.as_ref() == Some(&p.id))?;
            Some(Pick {
                title: c.title,
                reason: p.reason,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
