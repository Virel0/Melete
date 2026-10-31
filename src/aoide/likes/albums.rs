//! Finding the album a like names as a device finds it. A rescan can give an album a new id
//! while it stays in the library (it's keyed anew by a new MusicBrainz release, album tag or
//! folder), and a device then relinks the like by its content key: so does Melete, reading and
//! writing alike, so the web and every device agree on what's liked.

use std::collections::HashSet;

use super::*;
use crate::access::Access;

/// The albums `user_id` likes, newest like first, as [`liked`] gives them but each found as a
/// device finds it: by its id when that's a present album `access` lets them see, else by its
/// content key when exactly one such album has it. An album found through two rows is listed
/// once, by the newer.
///
/// # Errors
///
/// Fails when the database does.
pub async fn liked_albums(
    pool: &SqlitePool,
    user_id: i64,
    access: &Access,
) -> sqlx::Result<Vec<(Option<i64>, Like)>> {
    let liked = liked(pool, Kind::Album, user_id).await?;
    let visible = access.album_sql("a");
    let ids: Vec<i64> = liked.iter().filter_map(|(id, _)| *id).collect();
    let present: HashSet<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT a.id FROM music_albums a
         WHERE a.missing_since IS NULL AND a.id IN (SELECT value FROM json_each(?)) AND {visible}"
    )))
    .bind(serde_json::to_string(&ids).unwrap_or_else(|_| "[]".into()))
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    let mut by_key = None;
    let mut seen = HashSet::new();
    let mut found = Vec::with_capacity(liked.len());
    for (id, row) in liked {
        let mut id = id.filter(|id| present.contains(id));
        if id.is_none() {
            if by_key.is_none() {
                by_key = Some(present_by_key(pool, &visible).await?);
            }
            // Only one album with the key is found; several are none, never a guess.
            id = match by_key.as_ref().and_then(|k| k.get(&row.content_key)) {
                Some(ids) if ids.len() == 1 => Some(ids[0]),
                _ => None,
            };
        }
        if id.is_none_or(|id| seen.insert(id)) {
            found.push((id, row));
        }
    }
    Ok(found)
}

/// Present albums that `visible` (SQL over `music_albums a`) lets through, by content key.
async fn present_by_key(
    pool: &SqlitePool,
    visible: &str,
) -> sqlx::Result<HashMap<String, Vec<i64>>> {
    let albums: Vec<(i64, String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT a.id, a.title, a.artist_name FROM music_albums a
         WHERE a.missing_since IS NULL AND {visible}"
    )))
    .fetch_all(pool)
    .await?;
    let mut by_key: HashMap<String, Vec<i64>> = HashMap::new();
    for (id, title, artist) in albums {
        by_key
            .entry(writing::album_content_key(&artist, &title))
            .or_default()
            .push(id);
    }
    Ok(by_key)
}

/// For the present album `id`, which no row of `rows` names: the latest row with its content
/// key whose id names no present album, renamed to it. A device relinks that row to this
/// album, so a change goes into it rather than into a second row for the same album.
pub(super) async fn left_behind(
    pool: &SqlitePool,
    rows: Vec<Like>,
    id: i64,
) -> sqlx::Result<Option<Like>> {
    let Some(link) = writing::album_link(pool, id).await? else {
        return Ok(None);
    };
    let present: HashSet<i64> =
        sqlx::query_scalar("SELECT id FROM music_albums WHERE missing_since IS NULL")
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();
    let names_present = |r: &Like| {
        normalise(&r.item_id)
            .parse()
            .is_ok_and(|id: i64| present.contains(&id))
    };
    let left = rows
        .into_iter()
        .filter(|r| r.content_key == link.content_key && !names_present(r))
        .reduce(|a, b| if b.beats(&a) { b } else { a });
    Ok(left.map(|r| Like {
        item_id: link.album_id,
        ..r
    }))
}
