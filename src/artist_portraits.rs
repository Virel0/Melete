//! Artists' portraits for the artist results of a search for a request and for the library's
//! artists, from Wikimedia Commons: on only with `MELETE_ARTIST_PORTRAITS` (and MusicBrainz).
//! An artist's MusicBrainz url relations give its Wikidata item (asked of MusicBrainz through
//! its client, one request a second), the item's image (P18) is the portrait, and Commons gives
//! it at one width with its author and licence, which are kept with it and given with it. A
//! search looks up at most [`LOOKUPS`] artists it doesn't know yet; what it found, or that
//! there's none, is kept, so the next search for them asks nothing (a none is believed for
//! [`NONE_FOR`]). The picture itself is fetched once by the portrait route, when first asked
//! for, and cached.
//!
//! Kept to discovery posters' terms ([`crate::discover_posters`]): only artists a search listed
//! are served by MusicBrainz id, for 30 days after they were last listed, at most [`MOST`]
//! remembered; housekeeping forgets the rest and deletes their files, and every one when the
//! setting is off. When Commons fails to give a picture, it's neither offered nor asked for
//! again for [`FAIL_FOR`]. An artist only in libraries hidden from a user is, to them, like any
//! other artist, and gets a portrait the same way, so nothing tells them it's there.
//!
//! Artists in the library are looked up too, in the background, a few a run ([`library`]), the
//! same way and with the same three things leaving the server: the artist's MusicBrainz id (to
//! MusicBrainz), its Wikidata item's id (to www.wikidata.org) and the image's file name (to
//! Commons). Their portraits are served by their library id to those who may see them, and
//! aren't forgotten while they're in the library, however long ago a search listed them. An
//! artist whose albums share a folder holding a picture of them has that one instead, and
//! Commons isn't asked about them.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sqlx::SqlitePool;

use crate::picture_listings::{self, Kind};
use crate::{musicbrainz, wikimedia};

pub mod library;

pub use library::*;

/// How long a listed portrait is served.
pub const SERVED_FOR: i64 = 30 * 24 * 3600;
/// Artists remembered at most; older ones are forgotten first.
pub const MOST: i64 = 2000;
/// How long a portrait found is kept before it's looked up again.
pub const FOUND_FOR: i64 = 30 * 24 * 3600;
/// How long an artist having no portrait is believed without asking again.
pub const NONE_FOR: i64 = 7 * 24 * 3600;
/// How long Commons failing to give a picture is believed without asking again.
pub const FAIL_FOR: i64 = 24 * 3600;
/// Artists one search looks up at most: the closest few, each a MusicBrainz request a second.
pub const LOOKUPS: usize = 3;

/// What a portrait's licence asks to be shown with it, as Commons gives it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, sqlx::FromRow)]
pub struct Credit {
    /// Who made it, as plain text.
    pub author: Option<String>,
    /// Its licence's short name, such as "CC BY-SA 4.0".
    pub licence: Option<String>,
    /// Where the licence is written.
    pub licence_url: Option<String>,
    /// The picture's page on Commons.
    pub source_url: Option<String>,
}

/// The outside services a lookup asks, both there only with MusicBrainz on.
pub struct Services<'a> {
    pub musicbrainz: &'a musicbrainz::Client,
    pub wikimedia: &'a wikimedia::Client,
}

/// The address a search result gives for `artist`'s portrait.
pub fn url(artist: &str) -> String {
    format!("/api/v1/music/artist-portraits/{artist}")
}

/// Where `artist`'s portraits are kept, under the music covers' folder. `artist` must be a
/// MusicBrainz id, which can't climb out of it.
pub fn folder(covers: &Path, artist: &str) -> PathBuf {
    covers.join("portraits").join(artist)
}

/// The file the picture at `picture` is kept in: named for its address, so a portrait that
/// changed is fetched afresh.
pub fn cached(covers: &Path, artist: &str, picture: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let key = Sha256::digest(picture.as_bytes());
    let hex: String = key[..12].iter().map(|b| format!("{b:02x}")).collect();
    folder(covers, artist).join(format!("{hex}.img"))
}

#[derive(sqlx::FromRow)]
struct Known {
    looked_up_at: i64,
    file: Option<String>,
}

/// What a lookup found: the Commons file and its picture, or none.
type Found = Option<(String, wikimedia::Picture)>;

/// Looks `artist` up: MusicBrainz's url relations for its Wikidata item, the item's image, and
/// Commons for that file.
async fn look_up(services: &Services<'_>, artist: &str) -> anyhow::Result<Found> {
    let item = match services.musicbrainz.artist_wikidata(artist).await {
        Ok(item) => item,
        Err(musicbrainz::Error::NotFound) => None,
        Err(err) => anyhow::bail!("{err}"),
    };
    let Some(item) = item else {
        return Ok(None);
    };
    let Some(file) = services.wikimedia.image_of(&item).await? else {
        return Ok(None);
    };
    Ok(services
        .wikimedia
        .picture(&file)
        .await?
        .map(|picture| (file, picture)))
}

/// Keeps what a lookup found, or that there's none. An artist new here isn't listed yet
/// (`listed_at` 0): [`offer`] notes a search listing them, and a lookup for the library doesn't.
async fn keep(pool: &SqlitePool, artist: &str, found: &Found, now: i64) -> sqlx::Result<()> {
    let file = found.as_ref().map(|(file, _)| file.clone());
    let picture = found.as_ref().map(|(_, p)| p);
    sqlx::query(
        "INSERT INTO artist_portraits (mb_artist_id, listed_at, looked_up_at, file, picture_url,
             author, licence, licence_url, source_url)
         VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (mb_artist_id) DO UPDATE SET looked_up_at = excluded.looked_up_at,
             file = excluded.file, picture_url = excluded.picture_url, failed_at = NULL,
             author = excluded.author, licence = excluded.licence,
             licence_url = excluded.licence_url, source_url = excluded.source_url",
    )
    .bind(artist)
    .bind(now)
    .bind(file)
    .bind(picture.map(|p| &p.url))
    .bind(picture.and_then(|p| p.author.as_ref()))
    .bind(picture.and_then(|p| p.licence.as_ref()))
    .bind(picture.and_then(|p| p.licence_url.as_ref()))
    .bind(picture.and_then(|p| p.page.as_ref()))
    .execute(pool)
    .await?;
    Ok(())
}

/// Whether what `artist` has is known: kept from before, or looked up now when it isn't known
/// or is out of date and `lookups` has some left (an out-of-date one is used when none are).
/// A failed lookup is logged and kept nowhere, so a later search asks again.
async fn known(
    pool: &SqlitePool,
    services: &Services<'_>,
    artist: &str,
    lookups: &mut usize,
    now: i64,
) -> anyhow::Result<bool> {
    let known: Option<Known> =
        sqlx::query_as("SELECT looked_up_at, file FROM artist_portraits WHERE mb_artist_id = ?")
            .bind(artist)
            .fetch_optional(pool)
            .await?;
    let fresh = known.as_ref().is_some_and(|k| {
        let kept_for = if k.file.is_some() {
            FOUND_FOR
        } else {
            NONE_FOR
        };
        k.looked_up_at >= now - kept_for
    });
    if fresh || *lookups == 0 {
        return Ok(known.is_some());
    }
    *lookups -= 1;
    match look_up(services, artist).await {
        Ok(found) => {
            keep(pool, artist, &found, now).await?;
            Ok(true)
        }
        Err(err) => {
            tracing::warn!(
                error = format!("{err:#}"),
                "couldn't look up an artist's portrait"
            );
            Ok(false)
        }
    }
}

/// An artist result's portrait: with the setting on, what is [`known`] of it, which notes it as
/// listed for `user` (past [`MOST`], the forgetting is woken). Its address and credit, or none
/// (also for a day after Commons failed to give it).
pub async fn offer(
    pool: &SqlitePool,
    services: &Services<'_>,
    artist: Option<&str>,
    lookups: &mut usize,
    user: i64,
    now: i64,
) -> anyhow::Result<Option<(String, Credit)>> {
    let Some(artist) = artist else {
        return Ok(None);
    };
    if !crate::library::tags::is_mbid(artist) || !crate::settings::artist_portraits(pool).await? {
        return Ok(None);
    }
    if !known(pool, services, artist, lookups, now).await? {
        return Ok(None);
    }
    sqlx::query("UPDATE artist_portraits SET listed_at = ? WHERE mb_artist_id = ?")
        .bind(now)
        .bind(artist)
        .execute(pool)
        .await?;
    picture_listings::note(pool, Kind::Portrait, artist, user, now).await?;
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM artist_portraits WHERE listed_at > 0")
        .fetch_one(pool)
        .await?;
    picture_listings::over(kept, MOST);
    let credit: Option<Credit> = sqlx::query_as(
        "SELECT author, licence, licence_url, source_url FROM artist_portraits
         WHERE mb_artist_id = ? AND file IS NOT NULL AND coalesce(failed_at, 0) < ?",
    )
    .bind(artist)
    .bind(now - FAIL_FOR)
    .fetch_optional(pool)
    .await?;
    Ok(credit.map(|credit| (url(artist), credit)))
}

/// The picture's address on Commons' picture hosts, when the portrait route may fetch and serve
/// `artist`'s portrait to `user`: the setting on, listed for them in the last 30 days (so nobody
/// learns what someone else searched for), with a portrait, and not failed in the last day.
pub async fn servable(
    pool: &SqlitePool,
    artist: &str,
    user: i64,
    now: i64,
) -> anyhow::Result<Option<String>> {
    if !picture_listings::listed_for(pool, Kind::Portrait, artist, user, now - SERVED_FOR).await? {
        return Ok(None);
    }
    picture_url(pool, artist, now - SERVED_FOR, now).await
}

/// [`servable`] for an artist in the library, whom no search need have listed.
pub async fn servable_in_library(
    pool: &SqlitePool,
    artist: &str,
    now: i64,
) -> anyhow::Result<Option<String>> {
    picture_url(pool, artist, i64::MIN, now).await
}

/// The picture's address, with the setting on, for `artist` listed since `listed_since`, with a
/// portrait, and not failed in the last day.
async fn picture_url(
    pool: &SqlitePool,
    artist: &str,
    listed_since: i64,
    now: i64,
) -> anyhow::Result<Option<String>> {
    if !crate::library::tags::is_mbid(artist) || !crate::settings::artist_portraits(pool).await? {
        return Ok(None);
    }
    Ok(sqlx::query_scalar(
        "SELECT picture_url FROM artist_portraits WHERE mb_artist_id = ? AND listed_at >= ?
             AND file IS NOT NULL AND coalesce(failed_at, 0) < ?",
    )
    .bind(artist)
    .bind(listed_since)
    .bind(now - FAIL_FOR)
    .fetch_optional(pool)
    .await?
    .flatten())
}

/// Notes that Commons failed to give `artist`'s picture (an error, or something not an image):
/// neither offered nor asked for again for [`FAIL_FOR`].
pub async fn failed(pool: &SqlitePool, artist: &str, now: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE artist_portraits SET failed_at = ? WHERE mb_artist_id = ?")
        .bind(now)
        .bind(artist)
        .execute(pool)
        .await?;
    Ok(())
}

async fn forget_files(covers: &Path, artist: &str) -> std::io::Result<()> {
    if !crate::library::tags::is_mbid(artist) {
        return Ok(());
    }
    match tokio::fs::remove_dir_all(folder(covers, artist)).await {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// Forgets artists past their time, beyond [`MOST`], or all of them when the setting is off,
/// and deletes their cached files (under `covers`, the music covers' folder). Artists in the
/// library are kept while the setting is on, and don't count towards [`MOST`]. How many were
/// forgotten.
pub async fn prune(pool: &SqlitePool, covers: &Path, now: i64) -> anyhow::Result<usize> {
    let on = crate::settings::artist_portraits(pool).await?;
    let gone: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "WITH searched AS (SELECT mb_artist_id, listed_at FROM artist_portraits
             WHERE mb_artist_id NOT IN (SELECT mb_artist_id FROM ({})))
         SELECT mb_artist_id FROM artist_portraits
         WHERE NOT ?1 OR mb_artist_id IN (SELECT mb_artist_id FROM searched
             WHERE listed_at < ?2 OR mb_artist_id NOT IN (
                 SELECT mb_artist_id FROM searched
                 ORDER BY listed_at DESC, mb_artist_id LIMIT ?3))",
        library::in_library()
    )))
    .bind(on)
    .bind(now - SERVED_FOR)
    .bind(MOST)
    .fetch_all(pool)
    .await?;
    for artist in &gone {
        sqlx::query("DELETE FROM artist_portraits WHERE mb_artist_id = ?")
            .bind(artist)
            .execute(pool)
            .await?;
        picture_listings::forget(pool, Kind::Portrait, artist).await?;
        forget_files(covers, artist).await?;
    }
    Ok(gone.len())
}

#[cfg(test)]
mod tests;
