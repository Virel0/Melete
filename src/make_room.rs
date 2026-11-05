//! Make room: suggestions for freeing space in the movies and series libraries, each a title with
//! one action (delete it, swap it for a smaller release, or compress it), the space that would
//! free, and the reasons. Worked out from the database when asked; nothing is changed here.
//!
//! A title is never suggested while anyone is partway through it, has it on their list, or asked
//! for it in the last [`RECENT_REQUEST_DAYS`] days. The rest are scored on how little they're
//! watched, how long they've sat untouched, TMDB's score for them, and whether a major studio
//! made them (kept longer: easy to find again, but also what the household chose to keep). The
//! action is the one that keeps the most worth keeping: a copy far larger than it needs to be is
//! swapped or compressed rather than deleted when the title is still wanted.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::unix_now;

pub mod compressing;
mod studios;
pub mod swapping;

const DAY: i64 = 86_400;
/// A request this recent keeps its title off the list.
const RECENT_REQUEST_DAYS: i64 = 90;
/// Below this score a title isn't suggested at all.
const SUGGEST_FROM: i64 = 25;
/// Gigabytes an hour past which a copy is large: a 1080p remux runs 20 to 30, a good 1080p
/// encode 4 to 8.
const LARGE_GB_PER_HOUR: f64 = 10.0;
/// What a swapped copy is expected to take, in gigabytes an hour: a 1080p HEVC web release.
const SWAPPED_GB_PER_HOUR: f64 = 3.0;
/// The share of an H.264 or older copy that compressing it to HEVC is expected to free. Only such
/// copies of at least half [`LARGE_GB_PER_HOUR`] are offered it: on HEVC or AV1, or a small
/// copy, it would save little and cost quality.
const COMPRESS_SAVES: f64 = 0.5;

/// What a suggestion does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Its files go to the library's trash, deleted 30 days later.
    Delete,
    /// Wanted again at a smaller quality; the release found replaces the copy, which goes to the
    /// trash.
    Swap,
    /// Converted to HEVC on the GPU; the copy goes to the trash once the new file is checked.
    Compress,
}

/// One title suggested for making room.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    /// `movie` or `series`.
    pub kind: &'static str,
    pub id: i64,
    pub title: String,
    pub year: Option<i64>,
    /// In a library with the anime category, or wanted as anime.
    pub anime: bool,
    /// Its files' size in bytes.
    pub size: i64,
    pub action: Action,
    /// The space the action is expected to free, in bytes.
    pub saves: i64,
    /// How strongly it's suggested, 0 to 100: the list is ordered by it.
    pub score: i64,
    /// Why, as short codes an app turns into words: `never_watched`, `not_watched_lately`,
    /// `untouched`, `low_rating`, `large_copy`, `compressible`.
    pub reasons: Vec<&'static str>,
}

/// What's known about one title, as the scoring reads it.
#[derive(Debug, Clone, Default, sqlx::FromRow)]
pub(crate) struct Facts {
    pub series: bool,
    pub id: i64,
    pub title: String,
    pub year: Option<i64>,
    pub anime: bool,
    pub size: i64,
    /// Running time in seconds: the film's, else its files' probed lengths together.
    pub seconds: Option<f64>,
    /// The largest file's video codec, as ffprobe names it.
    pub codec: Option<String>,
    /// Users who finished any of it.
    pub watchers: i64,
    /// When anyone last watched any of it.
    pub last_watched: Option<i64>,
    /// When its first file came in.
    pub added: Option<i64>,
    pub tmdb_score: Option<f64>,
    pub tmdb_votes: Option<i64>,
    /// Its companies, as stored (a JSON array of names).
    pub companies: Option<String>,
    /// Someone's partway through it, has it on their list, asked for it lately, or it's being
    /// swapped or compressed now.
    pub protected: bool,
    /// On the wanted list, so a smaller release can be searched for with its profile.
    pub wanted: bool,
}

/// Every title worth suggesting, the strongest first, `major` naming extra studios to count as
/// major beside the built-in list.
pub async fn suggestions(pool: &SqlitePool, major: &[String]) -> sqlx::Result<Vec<Suggestion>> {
    let now = unix_now();
    let mut out: Vec<Suggestion> = facts(pool, now)
        .await?
        .into_iter()
        .filter_map(|f| suggest(&f, now, major))
        .collect();
    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(b.saves.cmp(&a.saves))
            .then(a.title.cmp(&b.title))
    });
    Ok(out)
}

/// What one title is suggested for, if anything.
pub(crate) fn suggest(f: &Facts, now: i64, major: &[String]) -> Option<Suggestion> {
    if f.protected || f.size <= 0 {
        return None;
    }
    let studio = studios::of(f.companies.as_deref(), major);
    let gb_per_hour = f
        .seconds
        .filter(|s| *s > 60.0)
        .map(|s| f.size as f64 / 1e9 / (s / 3600.0));
    let large = gb_per_hour.is_some_and(|g| g >= LARGE_GB_PER_HOUR);
    let compressible = f
        .codec
        .as_deref()
        .is_some_and(|c| compressing::COMPRESSIBLE.contains(&c))
        && gb_per_hour.is_some_and(|g| g >= LARGE_GB_PER_HOUR / 2.0);
    let (score, mut reasons) = score(f, now, studio, large);
    if compressible {
        reasons.push("compressible");
    }
    if score < SUGGEST_FROM {
        return None;
    }
    let (action, saves) = action(f, studio, large, compressible)?;
    Some(Suggestion {
        kind: if f.series { "series" } else { "movie" },
        id: f.id,
        title: f.title.clone(),
        year: f.year,
        anime: f.anime,
        size: f.size,
        action,
        saves,
        score,
        reasons,
    })
}

/// How strongly a title is suggested, 0 to 100, and why: how little it's watched, how long it's
/// sat untouched, a low score on TMDB and a large copy add; a high score and a major studio take
/// away.
fn score(f: &Facts, now: i64, studio: bool, large: bool) -> (i64, Vec<&'static str>) {
    let mut score = 0;
    let mut reasons = Vec::new();
    match f.last_watched {
        None if f.watchers == 0 => {
            score += 35;
            reasons.push("never_watched");
        }
        Some(at) if now - at > 365 * DAY => {
            score += 25;
            reasons.push("not_watched_lately");
        }
        Some(at) if now - at > 180 * DAY => {
            score += 10;
            reasons.push("not_watched_lately");
        }
        _ => {}
    }
    if f.added.is_some_and(|at| now - at > 365 * DAY) && f.last_watched.is_none() {
        score += 15;
        reasons.push("untouched");
    }
    match f.tmdb_score {
        Some(s) if rated(f) && s < 6.0 => {
            score += 20;
            reasons.push("low_rating");
        }
        Some(s) if rated(f) && s >= 7.5 => score -= 15,
        _ => {}
    }
    if studio {
        score -= 15;
    }
    if large {
        score += 15;
        reasons.push("large_copy");
    }
    (score.clamp(0, 100), reasons)
}

/// Whether TMDB's score rests on enough votes to go by.
fn rated(f: &Facts) -> bool {
    f.tmdb_votes.unwrap_or(0) >= 50
}

/// The action for a title and the bytes it frees. One worth keeping (watched by someone, well
/// rated, or a major studio's) is compressed when its copy compresses well, else swapped when
/// it's large and on the wanted list (a swap searches with its profile), else left alone; the
/// rest are deleted.
fn action(f: &Facts, studio: bool, large: bool, compressible: bool) -> Option<(Action, i64)> {
    let worth_keeping =
        f.watchers > 0 || studio || (rated(f) && f.tmdb_score.unwrap_or(0.0) >= 7.0);
    if !worth_keeping {
        return Some((Action::Delete, f.size));
    }
    if compressible {
        return Some((Action::Compress, (f.size as f64 * COMPRESS_SAVES) as i64));
    }
    let swapped = f
        .seconds
        .map(|s| (s / 3600.0 * SWAPPED_GB_PER_HOUR * 1e9) as i64)?;
    (large && f.wanted).then(|| (Action::Swap, (f.size - swapped).max(0)))
}

/// What's known about every present movie and series with files.
async fn facts(pool: &SqlitePool, now: i64) -> sqlx::Result<Vec<Facts>> {
    let recent = now - RECENT_REQUEST_DAYS * DAY;
    let mut all: Vec<Facts> = Vec::new();
    for (series, titles, tmdb, column, name, runtime, kind) in [
        (
            false,
            "movies",
            "tmdb_movies",
            "movie_id",
            "title",
            "t.runtime_minutes * 60.0",
            "movie",
        ),
        (
            true,
            "series",
            "tmdb_series",
            "series_id",
            "name",
            "NULL",
            "series",
        ),
    ] {
        let wanted = if series {
            "wanted_series"
        } else {
            "wanted_movies"
        };
        let anime = if series {
            "(l.category IS 'anime' OR EXISTS (SELECT 1 FROM wanted_series w
                 WHERE w.series_id = x.id AND w.series_type = 'anime'))"
        } else {
            "l.category IS 'anime'"
        };
        let rows: Vec<Facts> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "WITH files AS (
                 SELECT f.{column} AS title_id, f.id, f.size, f.added_at, f.modified_at,
                     p.info
                 FROM media_files f LEFT JOIN media_probes p ON p.file_id = f.id
                 WHERE f.{column} IS NOT NULL AND f.missing_since IS NULL
             )
             SELECT {series} AS series, x.id, COALESCE(t.{name}, x.parsed_title) AS title,
                 COALESCE(t.year, x.parsed_year) AS year, {anime} AS anime,
                 (SELECT sum(size) FROM files WHERE title_id = x.id) AS size,
                 COALESCE({runtime}, (SELECT sum(CAST(json_extract(info, '$.duration_seconds')
                     AS REAL)) FROM files WHERE title_id = x.id)) AS seconds,
                 (SELECT json_extract(info, '$.video[0].codec') FROM files WHERE title_id = x.id
                     ORDER BY size DESC LIMIT 1) AS codec,
                 (SELECT count(DISTINCT pp.user_id) FROM playback_progress pp
                     JOIN files fi ON fi.id = pp.file_id
                     WHERE fi.title_id = x.id AND pp.watched = 1) AS watchers,
                 (SELECT max(pp.updated_at) FROM playback_progress pp
                     JOIN files fi ON fi.id = pp.file_id
                     WHERE fi.title_id = x.id AND (pp.watched = 1 OR pp.position_seconds > 0))
                     AS last_watched,
                 (SELECT min(coalesce(added_at, modified_at)) FROM files WHERE title_id = x.id)
                     AS added,
                 t.score AS tmdb_score, t.score_votes AS tmdb_votes, t.companies,
                 (EXISTS (SELECT 1 FROM playback_progress pp JOIN files fi ON fi.id = pp.file_id
                     WHERE fi.title_id = x.id AND pp.watched = 0 AND pp.position_seconds > 0)
                  OR EXISTS (SELECT 1 FROM favourites v WHERE v.{column} = x.id)
                  OR EXISTS (SELECT 1 FROM requests r WHERE r.kind = '{kind}'
                      AND r.tmdb_id = x.match_tmdb_id AND r.created_at >= ?1)
                  OR EXISTS (SELECT 1 FROM swaps sw JOIN files fi ON fi.id = sw.file_id
                      WHERE fi.title_id = x.id AND sw.state IN ('waiting', 'grabbed'))
                  OR EXISTS (SELECT 1 FROM compressions c JOIN files fi ON fi.id = c.file_id
                      WHERE fi.title_id = x.id AND c.state IN ('waiting', 'running')))
                     AS protected,
                 EXISTS (SELECT 1 FROM {wanted} w WHERE w.{column} = x.id) AS wanted
             FROM {titles} x JOIN libraries l ON l.id = x.library_id
             LEFT JOIN {tmdb} t ON t.tmdb_id = x.match_tmdb_id
             WHERE x.missing_since IS NULL
               AND EXISTS (SELECT 1 FROM files WHERE title_id = x.id)"
        )))
        .bind(recent)
        .fetch_all(pool)
        .await?;
        all.extend(rows);
    }
    Ok(all)
}

#[cfg(test)]
mod tests;
