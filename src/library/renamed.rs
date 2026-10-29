//! A title folder renamed (`Dune` to `Dune (2021)`, `Show` to `Show (2019) [tvdb-12345]`)
//! stays the same movie or series row, so its watch history, matches, wanted entry, requests,
//! favourites and everything else that points at the row follow it. Only Melete's own rows
//! change; nothing on disk is touched.
//!
//! Told by the files alone, never by provider ids: in a whole scan, a title that was there
//! before and isn't now, and a folder no title had before, are the same title when every file
//! the old one still had is in the new one with the same size and the same change time (the
//! same bytes, moved). Only when that pairs exactly one old title with exactly one new folder;
//! anything less clear is left to be a new title, as before. Files whose path in the folder
//! didn't change keep their rows (and their ids); others are new files of the same title, so
//! the successors that follow pick them up as a rename within the folder.

use std::collections::{HashMap, HashSet};

use sqlx::SqliteConnection;

use super::scan::FoundFile;
use super::{Library, LibraryKind, walk};

/// A file's size and change time: the same bytes, wherever they are.
type Bytes = (i64, i64);

/// A title row that was there and a folder that is new, to become its folder.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Renamed {
    /// The title row.
    pub id: i64,
    /// Its folder (or loose file) as it was.
    pub from: String,
    /// Its folder (or loose file) now.
    pub to: String,
}

/// The title a found file belongs to: its top-level folder, or itself when loose (a movie).
fn title_path(library: &Library, rel_path: &str) -> Option<String> {
    match rel_path.split_once('/') {
        Some((folder, _)) => Some(folder.to_owned()),
        None if library.kind == LibraryKind::Movies => Some(rel_path.to_owned()),
        None => None,
    }
}

fn tables(library: &Library) -> (&'static str, &'static str) {
    match library.kind {
        LibraryKind::Shows => ("series", "series_id"),
        _ => ("movies", "movie_id"),
    }
}

/// Finds the titles of `library` a whole scan (`found`, with `ignored` folders skipped) shows
/// were renamed, before anything it found is stored.
pub(super) async fn find(
    tx: &mut SqliteConnection,
    library: &Library,
    found: &[FoundFile],
    ignored: &[String],
) -> anyhow::Result<Vec<Renamed>> {
    let (table, owner) = tables(library);
    let mut now: HashMap<String, HashMap<Bytes, usize>> = HashMap::new();
    for file in found {
        if let Some(title) = title_path(library, &file.rel_path) {
            *now.entry(title)
                .or_default()
                .entry((file.size, file.modified))
                .or_default() += 1;
        }
    }
    let present: HashSet<String> = now.keys().cloned().collect();
    let known: HashSet<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT rel_path FROM {table} WHERE library_id = ?"
    )))
    .bind(library.id)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .collect();
    now.retain(|title, _| !known.contains(title));
    if now.is_empty() {
        return Ok(Vec::new());
    }
    let gone: Vec<(i64, String, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT t.id, t.rel_path, f.size, f.modified_at
         FROM {table} t JOIN media_files f ON f.{owner} = t.id AND f.missing_since IS NULL
         WHERE t.library_id = ?1 AND t.missing_since IS NULL AND NOT {}
         ORDER BY t.id",
        walk::ignored("t.rel_path", 2)
    )))
    .bind(library.id)
    .bind(serde_json::to_string(ignored)?)
    .fetch_all(&mut *tx)
    .await?;
    let mut was: HashMap<(i64, String), HashMap<Bytes, usize>> = HashMap::new();
    for (id, rel_path, size, modified) in gone {
        if present.contains(&rel_path) {
            continue;
        }
        *was.entry((id, rel_path))
            .or_default()
            .entry((size, modified))
            .or_default() += 1;
    }
    Ok(pair(&was, &now))
}

/// Each old title whose files are all among exactly one new folder's, and that folder's only
/// such title.
fn pair(
    was: &HashMap<(i64, String), HashMap<Bytes, usize>>,
    now: &HashMap<String, HashMap<Bytes, usize>>,
) -> Vec<Renamed> {
    let holds = |old: &HashMap<Bytes, usize>, new: &HashMap<Bytes, usize>| {
        old.iter()
            .all(|(bytes, n)| new.get(bytes).is_some_and(|m| m >= n))
    };
    let mut pairs = Vec::new();
    for ((id, from), old) in was {
        let fits: Vec<&String> = now
            .iter()
            .filter(|(_, new)| holds(old, new))
            .map(|(to, _)| to)
            .collect();
        if let [to] = fits.as_slice() {
            pairs.push(Renamed {
                id: *id,
                from: from.clone(),
                to: (*to).clone(),
            });
        }
    }
    let mut taken: HashMap<String, usize> = HashMap::new();
    for p in &pairs {
        *taken.entry(p.to.clone()).or_default() += 1;
    }
    pairs.retain(|p| taken[&p.to] == 1);
    pairs.sort_by_key(|p| p.id);
    pairs
}

/// Gives each renamed title its new folder, and its files and extras whose path in the folder
/// is unchanged their new paths, so the scan then records them as the same rows.
pub(super) async fn follow(
    tx: &mut SqliteConnection,
    library: &Library,
    renamed: &[Renamed],
    (found, extras): (&[FoundFile], &[FoundFile]),
    now: i64,
) -> anyhow::Result<()> {
    let (table, owner) = tables(library);
    let there: HashMap<&str, Bytes> = found
        .iter()
        .chain(extras)
        .map(|f| (f.rel_path.as_str(), (f.size, f.modified)))
        .collect();
    for r in renamed {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET rel_path = ? WHERE id = ?"
        )))
        .bind(&r.to)
        .bind(r.id)
        .execute(&mut *tx)
        .await?;
        for files in ["media_files", "media_extras"] {
            move_rows(tx, (files, owner), r, &there).await?;
        }
        crate::history::folder_renamed(tx, (table, r.id), (&r.from, &r.to), now).await?;
    }
    Ok(())
}

/// Gives the rows of `files` (videos or extras) of the renamed title `r` their path in its new
/// folder, each one whose file is there with the same size and change time.
async fn move_rows(
    tx: &mut SqliteConnection,
    (files, owner): (&str, &str),
    r: &Renamed,
    there: &HashMap<&str, Bytes>,
) -> anyhow::Result<()> {
    let rows: Vec<(i64, String, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT id, rel_path, size, modified_at FROM {files}
         WHERE {owner} = ? AND missing_since IS NULL"
    )))
    .bind(r.id)
    .fetch_all(&mut *tx)
    .await?;
    let inside = format!("{}/", r.from);
    for (id, rel_path, size, modified) in rows {
        let moved = if rel_path == r.from {
            r.to.clone()
        } else if let Some(rest) = rel_path.strip_prefix(&inside) {
            format!("{}/{rest}", r.to)
        } else {
            continue;
        };
        if there.get(moved.as_str()) == Some(&(size, modified)) {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE {files} SET rel_path = ? WHERE id = ?"
            )))
            .bind(&moved)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
