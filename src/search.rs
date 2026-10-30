//! Searching indexers for a wanted movie, season or episode. Every result is checked to be that
//! title, decided on with the title's quality profile, and kept with its reasons, so it can be
//! shown why one was grabbed or not. Nothing is grabbed here; that's `downloads`.
//!
//! The title check fails closed: a release is only for a wanted title when its parsed title is
//! the wanted title (or TMDB's original title) once case, accents, punctuation and a leading
//! article are set aside, its year is the title's, and it's the kind of release searched for.
//! Against a file on disk that can't be judged, nothing is called better.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::profiles::decide::{Decision, Existing};
use crate::profiles::formats::Facts;
use crate::profiles::{self, App, Loaded};
use crate::release::{self, Episodes, Release};
use crate::torznab;

mod indexer_params;
mod on_disk;
mod titles;
mod view;
mod wanted;

pub(crate) use indexer_params::*;
use on_disk::*;
pub(crate) use on_disk::{OnDisk, Read, best_read, cutoff_unmet, file_name, judge_file};
pub(crate) use titles::*;
pub use view::*;
pub(crate) use wanted::*;

/// Searches are kept this long, then removed with their decisions.
const KEEP_SECONDS: i64 = 30 * 24 * 3600;

/// What a search is for: a wanted movie, or a wanted series' season or episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A wanted movie, by its row id.
    Movie(i64),
    /// A season, or one episode of it.
    Series {
        /// The wanted series' row id.
        id: i64,
        /// The season number.
        season: i64,
        /// One episode; `None` for the whole season.
        episode: Option<i64>,
    },
}

/// Why a search couldn't start.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// There's no such wanted title.
    NotWanted,
    /// The title's quality profile isn't in Melete.
    NoProfile(String),
    /// No indexer is turned on.
    NoIndexers,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NotWanted => f.write_str("no such wanted title"),
            Refused::NoProfile(why) => f.write_str(why),
            Refused::NoIndexers => {
                f.write_str("no indexers are turned on; import them from Prowlarr")
            }
        }
    }
}

/// What a search looks for.
#[derive(Debug, Default, Clone)]
pub(crate) struct Wanted {
    pub(crate) titles: Vec<String>,
    year: Option<i64>,
    pub(crate) imdb_id: Option<String>,
    pub(crate) tmdb_id: Option<i64>,
    pub(crate) tvdb_id: Option<i64>,
    original_language: Option<String>,
    profile: Option<String>,
    /// A profile chosen in Melete, used over `profile`.
    profile_id: Option<i64>,
    /// The adopted folder, for the file already on disk.
    pub(crate) movie_id: Option<i64>,
    pub(crate) series_id: Option<i64>,
}

pub(crate) struct IndexerRow {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) api_key: String,
    pub(crate) categories: Vec<i64>,
    pub(crate) kind: torznab::Kind,
    movie_params: Vec<String>,
    tv_params: Vec<String>,
}

/// How one indexer answered a search.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexerOutcome {
    /// The indexer's row id.
    pub indexer_id: i64,
    /// Its name when it was asked, kept if it's later removed.
    pub name: String,
    /// How many results it gave; 0 when it failed or wasn't asked.
    pub results: usize,
    /// Why it gave none: it failed, or wasn't asked (no categories for what was searched).
    /// Never holds its address or key. `None` when it answered.
    pub error: Option<String>,
}

/// Runs a search and stores it; returns its id.
pub async fn search(
    pool: &SqlitePool,
    client: &torznab::Client,
    tmdb: Option<&crate::tmdb::Client>,
    target: Target,
) -> anyhow::Result<Result<i64, Refused>> {
    search_skipping(pool, client, tmdb, target, &[]).await
}

/// Like [`search`], leaving out the indexers in `skip`: ones a search-all-missing run found at
/// their limit, so the run goes on with the rest.
pub async fn search_skipping(
    pool: &SqlitePool,
    client: &torznab::Client,
    tmdb: Option<&crate::tmdb::Client>,
    target: Target,
    skip: &[i64],
) -> anyhow::Result<Result<i64, Refused>> {
    let Some(wanted) = load_wanted(pool, target).await? else {
        return Ok(Err(Refused::NotWanted));
    };
    let profile = match profile_for(pool, target, &wanted).await? {
        Ok(profile) => profile,
        Err(refused) => return Ok(Err(refused)),
    };
    let indexers: Vec<IndexerRow> = enabled_indexers(pool)
        .await?
        .into_iter()
        .filter(|i| !skip.contains(&i.id))
        .collect();
    if indexers.is_empty() {
        return Ok(Err(Refused::NoIndexers));
    }

    // Ask every indexer at once.
    let mut tasks = tokio::task::JoinSet::new();
    for (position, indexer) in indexers.iter().enumerate() {
        let params = params(indexer, &wanted, target);
        let client = client.clone();
        let (url, api_key, kind) = (indexer.url.clone(), indexer.api_key.clone(), indexer.kind);
        tasks.spawn(async move {
            let result = match params {
                Ok(params) => {
                    let pairs: Vec<(&str, &str)> =
                        params.iter().map(|(k, v)| (*k, v.as_str())).collect();
                    let endpoint = torznab::Endpoint {
                        url: &url,
                        api_key: &api_key,
                    };
                    client
                        .search_as(kind, &endpoint, &pairs)
                        .await
                        .map_err(|err| err.to_string())
                }
                Err(why) => Err(why),
            };
            (position, result)
        });
    }
    let mut outcomes: Vec<Option<(IndexerOutcome, Vec<torznab::Item>)>> =
        indexers.iter().map(|_| None).collect();
    while let Some(joined) = tasks.join_next().await {
        let (position, result) = joined?;
        let indexer = &indexers[position];
        let (error, items) = match result {
            Ok(items) => (None, items),
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

    let items: Vec<(i64, &torznab::Item)> = outcomes
        .iter()
        .flatten()
        .flat_map(|(outcome, items)| items.iter().map(|item| (outcome.indexer_id, item)))
        .collect();
    let decided = decide_all(pool, tmdb, &wanted, &profile, target, &items).await?;
    let summary: Vec<&IndexerOutcome> = outcomes.iter().flatten().map(|(o, _)| o).collect();
    let search_id = store(pool, target, profile.profile.id, None, &summary, &decided).await?;
    Ok(Ok(search_id))
}

/// The wanted title's quality profile.
pub(crate) async fn profile_for(
    pool: &SqlitePool,
    target: Target,
    wanted: &Wanted,
) -> anyhow::Result<Result<Loaded, Refused>> {
    let (app, service) = match target {
        Target::Movie(_) => (App::Radarr, "movies".to_owned()),
        Target::Series { id, .. } => (
            App::Sonarr,
            profiles::service::of_wanted_series(pool, id).await?,
        ),
    };
    // A profile chosen in Melete; it's for this title's service (checked when chosen; one
    // chosen before the title moved between Series and Anime is passed over).
    if let Some(id) = wanted.profile_id
        && let Some(profile) = profiles::load(pool, id).await?
        && profile.profile.service == service
    {
        return Ok(Ok(profile));
    }
    let Some(profile_name) = wanted.profile.as_deref() else {
        return Ok(Err(Refused::NoProfile(
            "the title has no quality profile".into(),
        )));
    };
    let profile_id = profiles::service::named(pool, &service, profile_name).await?;
    Ok(match profile_id {
        Some(id) => profiles::load(pool, id).await?,
        None => None,
    }
    .ok_or_else(|| {
        Refused::NoProfile(format!(
            "the quality profile {profile_name:?} isn't imported; import {}'s profiles first",
            app.name()
        ))
    }))
}

/// Decides on every result for a target, before anything is written: each is checked to be
/// the wanted title and compared with what's on disk where it would go (all the episodes a
/// multi-episode release covers).
pub(crate) async fn decide_all<'a>(
    pool: &SqlitePool,
    tmdb: Option<&crate::tmdb::Client>,
    wanted: &Wanted,
    profile: &Loaded,
    target: Target,
    items: &[(i64, &'a torznab::Item)],
) -> anyhow::Result<Vec<(i64, &'a torznab::Item, Decision)>> {
    let original = wanted.original_language.as_deref();
    let mut disk: HashMap<Option<(i64, i64, i64)>, OnDisk> = HashMap::new();
    let mut looked = Looked::default();
    let mut decided = Vec::new();
    for &(indexer_id, item) in items {
        let parsed = release::parse(&item.title);
        // Where it goes; renumbered when it's numbered from the first episode.
        let mut episodes = parsed.episodes.clone();
        let facts = Facts::new(&item.title, &parsed, item.size, &item.flags, original);
        let mut mismatch = mismatch(&parsed, wanted, target);
        if let Some(reason) = mismatch.take() {
            mismatch = match second_look(pool, tmdb, wanted, target, &parsed, &mut looked).await? {
                Look::Lifted => None,
                // Numbered from the first episode: judged, and compared with what's on disk,
                // as the season and episodes it maps to.
                Look::Renumbered(renumbered) => {
                    episodes = renumbered;
                    None
                }
                Look::Kept(None) => Some(reason),
                Look::Kept(Some(note)) => Some(format!("{reason} ({note})")),
            };
        }
        let place = match (target, &episodes) {
            (Target::Movie(_), _) => Some(None),
            (
                Target::Series {
                    episode: Some(_), ..
                },
                Episodes::Standard { season, episodes },
            ) => {
                let (first, last) = (episodes.iter().min(), episodes.iter().max());
                first.zip(last).map(|(f, l)| Some((*season, *f, *l)))
            }
            _ => None,
        };
        let mut decision = match place.filter(|_| mismatch.is_none()) {
            Some(key) => {
                let state = match disk.entry(key) {
                    Entry::Occupied(known) => known.into_mut(),
                    Entry::Vacant(new) => new.insert(on_disk(pool, profile, wanted, key).await?),
                };
                match state {
                    OnDisk::Nothing => profile.decide(&facts, None),
                    OnDisk::File(existing) => profile.decide(&facts, Some(existing)),
                    OnDisk::Unclear(why) => {
                        let mut decision = profile.decide(&facts, None);
                        decision.rejections.push(why.clone());
                        decision.accepted = false;
                        decision
                    }
                }
            }
            None => profile.decide(&facts, None),
        };
        if let Some(reason) = mismatch {
            decision.rejections.insert(0, reason);
            decision.accepted = false;
        }
        decided.push((indexer_id, item, decision));
    }
    Ok(decided)
}

/// Stores a search (from an RSS sync, with its id) with its decisions; returns its id.
pub(crate) async fn store(
    pool: &SqlitePool,
    target: Target,
    profile_id: i64,
    rss_sync_id: Option<i64>,
    summary: &[&IndexerOutcome],
    decided: &[(i64, &torznab::Item, Decision)],
) -> anyhow::Result<i64> {
    let now = unix_now();
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM searches WHERE searched_at < ?")
        .bind(now - KEEP_SECONDS)
        .execute(&mut *tx)
        .await?;
    let (movie_id, series_id, season, episode) = match target {
        Target::Movie(id) => (Some(id), None, None, None),
        Target::Series {
            id,
            season,
            episode,
        } => (None, Some(id), Some(season), episode),
    };
    let search_id: i64 = sqlx::query_scalar(
        "INSERT INTO searches (wanted_movie_id, wanted_series_id, season, episode, profile_id,
             searched_at, indexers, rss_sync_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(movie_id)
    .bind(series_id)
    .bind(season)
    .bind(episode)
    .bind(profile_id)
    .bind(now)
    .bind(serde_json::to_string(&summary)?)
    .bind(rss_sync_id)
    .fetch_one(&mut *tx)
    .await?;
    let sealing = crate::secrets::sealing(&mut *tx).await?;
    let link =
        |what, link: &Option<String>| crate::secrets::link_to_store(sealing, what, link.as_deref());
    for (indexer_id, item, decision) in decided {
        sqlx::query(
            "INSERT INTO release_decisions (search_id, indexer_id, title, size, seeders, peers,
                     published, infohash, download_url, guid, accepted, decision)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(search_id)
        .bind(indexer_id)
        .bind(&item.title)
        .bind(item.size.and_then(|s| i64::try_from(s).ok()))
        .bind(item.seeders)
        .bind(item.peers)
        .bind(item.published)
        .bind(&item.infohash)
        .bind(link(crate::secrets::DOWNLOAD_URL, &item.download_url)?)
        .bind(link(crate::secrets::GUID, &item.guid)?)
        .bind(decision.accepted)
        .bind(serde_json::to_string(&decision)?)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(search_id)
}

#[cfg(test)]
mod tests;
