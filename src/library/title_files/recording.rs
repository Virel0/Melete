//! Keeping a scan's findings in title folders: the `.nfo` ids (read again only when a file
//! changed) and the poster and backdrop, on the title's row.

use sqlx::{SqliteConnection, SqlitePool};

use super::super::{Library, LibraryKind};
use super::*;

/// The titles whose `.nfo` files changed since they were last read, by folder: what the files
/// are now (`None` when there are none any more) and the ids they give.
pub(in super::super) type ReadIds = HashMap<String, (Option<String>, NfoIds)>;

/// A title to read the `.nfo` files of: its folder, what the files are now, and each file's
/// path and size.
type ToRead = (String, Option<String>, Vec<(String, i64)>);

fn table(library: &Library) -> &'static str {
    match library.kind {
        LibraryKind::Shows => "series",
        _ => "movies",
    }
}

/// Reads the `.nfo` files of the titles whose files changed since the last scan (new, edited,
/// added or gone), before the scan's transaction starts.
pub(in super::super) async fn read_changed_nfo(
    pool: &SqlitePool,
    library: &Library,
    beside: &ByTitle<'_>,
    (scope, ignored): (Option<&str>, &[String]),
) -> anyhow::Result<ReadIds> {
    // Only the titles this scan looked at: in its scope, and not in an ignored folder.
    let stored: HashMap<String, Option<String>> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT rel_path, nfo_read FROM {} WHERE library_id = ?1 AND {} AND NOT {}",
        table(library),
        super::super::walk::in_scope("rel_path", 2),
        super::super::walk::ignored("rel_path", 3),
    )))
    .bind(library.id)
    .bind(scope)
    .bind(serde_json::to_string(ignored)?)
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    let mut to_read: Vec<ToRead> = Vec::new();
    for (title, files) in beside {
        let now = files.nfo_signature();
        if stored.get(*title).cloned().flatten() != now {
            let nfo = files
                .nfo
                .iter()
                .map(|f| (f.rel_path.clone(), f.size))
                .collect();
            to_read.push(((*title).to_owned(), now, nfo));
        }
    }
    for (title, was) in &stored {
        if was.is_some() && !beside.contains_key(title.as_str()) {
            to_read.push((title.clone(), None, Vec::new()));
        }
    }
    let root = library.path.clone();
    Ok(tokio::task::spawn_blocking(move || {
        to_read
            .into_iter()
            .map(|(title, now, nfo)| {
                let ids = read_nfo_ids(&root, &nfo);
                (title, (now, ids))
            })
            .collect()
    })
    .await?)
}

/// Records what a title folder holds beside its videos on the title's row. `read` is given
/// when its `.nfo` files changed: then the new ids are kept, and a title matched on its own
/// (no *arr id, no id in the folder name, not by hand) is matched again, since its `.nfo` now
/// says something else.
pub(in super::super) async fn record(
    tx: &mut SqliteConnection,
    library: &Library,
    id: i64,
    files: Option<&TitleFiles<'_>>,
    read: Option<&(Option<String>, NfoIds)>,
) -> anyhow::Result<()> {
    let table = table(library);
    if let Some((signature, ids)) = read {
        let ids = (!ids.is_empty())
            .then(|| serde_json::to_string(ids))
            .transpose()?;
        let own_ids = match library.kind {
            LibraryKind::Shows => {
                "arr_tmdb_id IS NULL AND arr_tvdb_id IS NULL AND tmdb_id IS NULL
                 AND tvdb_id IS NULL AND imdb_id IS NULL"
            }
            _ => "arr_tmdb_id IS NULL AND tmdb_id IS NULL AND imdb_id IS NULL",
        };
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET match_status = NULL, match_tmdb_id = NULL, match_method = NULL,
                 match_candidates = NULL
             WHERE id = ?1 AND nfo_ids IS NOT ?2 AND match_method IS NOT 'manual' AND {own_ids}"
        )))
        .bind(id)
        .bind(&ids)
        .execute(&mut *tx)
        .await?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET nfo_ids = ?, nfo_read = ? WHERE id = ?"
        )))
        .bind(&ids)
        .bind(signature)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    let image = |file: Option<&FoundFile>| {
        file.map(|f| (f.rel_path.clone(), artwork_name(library.id, f)))
            .unzip()
    };
    let (poster, poster_key) = image(files.and_then(|f| f.poster));
    let (backdrop, backdrop_key) = image(files.and_then(|f| f.backdrop));
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET local_poster = ?1, local_poster_key = ?2, local_backdrop = ?3,
             local_backdrop_key = ?4
         WHERE id = ?5 AND (local_poster_key IS NOT ?2 OR local_backdrop_key IS NOT ?4)"
    )))
    .bind(poster)
    .bind(poster_key)
    .bind(backdrop)
    .bind(backdrop_key)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    Ok(())
}
