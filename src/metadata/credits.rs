//! Cast and crew for the titles in the library: fetched after their details, and again
//! whenever the details are, one request a title. Only titles in the library: their TMDB ids
//! are already what TMDB is asked about.

use super::*;
use crate::tmdb::{Creator, Credits};

/// Top-billed cast kept per title.
const MAX_CAST: usize = 20;
/// The crew jobs kept: who directed and who wrote.
const KEY_JOBS: &[&str] = &[
    "Director",
    "Screenplay",
    "Writer",
    "Story",
    "Novel",
    "Teleplay",
];

/// Fetches the cast and crew of present titles whose details were fetched since their credits
/// were (or that have none yet). Stops when TMDB refuses the token or asks for a long pause;
/// other failures skip a title until the next run. Returns how many titles got credits.
pub async fn refresh_credits(pool: &SqlitePool, tmdb: &tmdb::Client) -> anyhow::Result<usize> {
    let now = unix_now();
    let mut done = 0;
    let movies: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT t.tmdb_id FROM tmdb_movies t JOIN movies m ON m.match_tmdb_id = t.tmdb_id
         WHERE m.missing_since IS NULL
           AND (t.credits_fetched_at IS NULL OR t.credits_fetched_at < t.fetched_at)",
    )
    .fetch_all(pool)
    .await?;
    for id in movies {
        match tmdb.movie_credits(id).await {
            Ok(credits) => {
                let credits = credits.unwrap_or_default();
                store_credits(pool, Owner::Movie(id), &[], &credits, now).await?;
                done += 1;
            }
            Err(err) => skip(
                err,
                "movie credits",
                &id.to_string(),
                &mut RefreshReport::default(),
            )?,
        }
    }
    let series: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT t.tmdb_id FROM tmdb_series t JOIN series s ON s.match_tmdb_id = t.tmdb_id
         WHERE s.missing_since IS NULL
           AND (t.credits_fetched_at IS NULL OR t.credits_fetched_at < t.fetched_at)",
    )
    .fetch_all(pool)
    .await?;
    for id in series {
        match tmdb.series_credits(id).await {
            Ok(found) => {
                let (creators, credits) = found.unwrap_or_default();
                store_credits(pool, Owner::Series(id), &creators, &credits, now).await?;
                done += 1;
            }
            Err(err) => skip(
                err,
                "series credits",
                &id.to_string(),
                &mut RefreshReport::default(),
            )?,
        }
    }
    Ok(done)
}

/// Whose credits these are, by TMDB id.
#[derive(Clone, Copy)]
enum Owner {
    Movie(i64),
    Series(i64),
}

/// A credit to keep: the person's id, name and picture, their role, and the character or job.
type Credit<'a> = (i64, &'a str, Option<&'a str>, &'a str, Option<&'a str>);

/// Replaces a title's credits: its creators first, then its key crew, then its top-billed cast.
async fn store_credits(
    pool: &SqlitePool,
    owner: Owner,
    creators: &[Creator],
    credits: &Credits,
    now: i64,
) -> anyhow::Result<()> {
    let (movie, series, table) = match owner {
        Owner::Movie(id) => (Some(id), None, "tmdb_movies"),
        Owner::Series(id) => (None, Some(id), "tmdb_series"),
    };
    let mut cast: Vec<_> = credits.cast.iter().collect();
    cast.sort_by_key(|c| c.order.unwrap_or(i64::MAX));
    let mut rows: Vec<Credit> = Vec::new();
    for c in creators {
        rows.push((
            c.id,
            &c.name,
            c.profile_path.as_deref(),
            "crew",
            Some("Creator"),
        ));
    }
    for c in &credits.crew {
        let Some(job) = c.job.as_deref().filter(|j| KEY_JOBS.contains(j)) else {
            continue;
        };
        if !rows.iter().any(|r| r.0 == c.id && r.4 == Some(job)) {
            rows.push((c.id, &c.name, c.profile_path.as_deref(), "crew", Some(job)));
        }
    }
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM tmdb_credits WHERE movie_tmdb_id IS ? AND series_tmdb_id IS ?")
        .bind(movie)
        .bind(series)
        .execute(&mut *tx)
        .await?;
    let cast_rows = cast.iter().take(MAX_CAST).map(|c| {
        let character = c.character.as_deref().filter(|s| !s.trim().is_empty());
        (
            c.id,
            c.name.as_str(),
            c.profile_path.as_deref(),
            "cast",
            character,
        )
    });
    for (position, (person, name, profile, role, detail)) in
        rows.into_iter().chain(cast_rows).enumerate()
    {
        sqlx::query(
            "INSERT INTO tmdb_people (id, name, profile_path) VALUES (?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET name = excluded.name,
                 profile_path = excluded.profile_path",
        )
        .bind(person)
        .bind(name)
        .bind(profile.filter(|p| !p.is_empty()))
        .execute(&mut *tx)
        .await?;
        let (character, job) = if role == "cast" {
            (detail, None)
        } else {
            (None, detail)
        };
        sqlx::query(
            "INSERT INTO tmdb_credits (movie_tmdb_id, series_tmdb_id, person_id, role, character,
                 job, position)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(movie)
        .bind(series)
        .bind(person)
        .bind(role)
        .bind(character)
        .bind(job)
        .bind(position as i64)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET credits_fetched_at = ? WHERE tmdb_id = ?"
    )))
    .bind(now)
    .bind(movie.or(series))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
