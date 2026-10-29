//! A playlist's items: adding tracks, removing one and moving one, each writing only the rows
//! it changes, as a device does.

use super::*;

fn position(item: &Row) -> Option<&str> {
    item.get("position").and_then(Value::as_str)
}

/// Keys for `count` new items between `lower` and `upper`; refused when those leave no room.
fn keys_between(
    count: usize,
    lower: Option<&str>,
    upper: Option<&str>,
) -> Result<Vec<String>, EditError> {
    if let (Some(lower), Some(upper)) = (lower, upper)
        && lower >= upper
    {
        return Err(EditError::NoGap);
    }
    Ok(fractional_index::positions(count, lower, upper))
}

/// Adds `tracks`, in order, so the first lands at `index` (counted in the playlist as it
/// is), or at the end. A track already there is added again, as on the devices. Every track
/// must be in the library.
///
/// # Errors
///
/// When it isn't the user's own hand-made playlist, a track isn't in the library, or a write
/// error.
pub async fn add(
    pool: &SqlitePool,
    editor: &Editor<'_>,
    id: &str,
    tracks: &[i64],
    index: Option<usize>,
) -> Result<(), EditError> {
    let (playlist, editor) = own(pool, editor, id).await?;
    let editor = &editor;
    let mut links = Vec::with_capacity(tracks.len());
    for &track in tracks {
        let link = writing::track_link(pool, track)
            .await?
            .ok_or(EditError::NoSuchTrack(track))?;
        links.push(link);
    }
    let items = &playlist.items;
    let at = index.unwrap_or(items.len()).min(items.len());
    let lower = at.checked_sub(1).and_then(|i| position(&items[i]));
    let upper = items.get(at).and_then(position);
    let keys = keys_between(links.len(), lower, upper)?;
    let mut changes = Vec::with_capacity(links.len());
    for (link, key) in links.into_iter().zip(keys) {
        let mut row = Row::new();
        row.insert("id".into(), writing::new_id()?.into());
        row.insert("playlistId".into(), id.into());
        row.insert("jellyfinId".into(), link.jellyfin_id.into());
        row.insert("contentKey".into(), link.content_key.into());
        row.insert("position".into(), key.into());
        changes.push(item_change(&row, editor, false));
    }
    writing::append(pool, editor.user_id, editor.device, changes, editor.now).await?;
    Ok(())
}

/// Removes item `item` from the playlist.
///
/// # Errors
///
/// When it isn't the user's own hand-made playlist or has no such item, or a write error.
pub async fn remove(
    pool: &SqlitePool,
    editor: &Editor<'_>,
    id: &str,
    item: &str,
) -> Result<(), EditError> {
    let (playlist, editor) = own(pool, editor, id).await?;
    let editor = &editor;
    let held = find(&playlist, item)?;
    let change = item_change(held, editor, true);
    writing::append(
        pool,
        editor.user_id,
        editor.device,
        vec![change],
        editor.now,
    )
    .await?;
    Ok(())
}

/// Moves item `item` so it lands at `index`, counted in the playlist with it taken out. One
/// row is written; none when it's there already.
///
/// # Errors
///
/// When it isn't the user's own hand-made playlist or has no such item, when its neighbours
/// there leave no room, or a write error.
pub async fn reorder(
    pool: &SqlitePool,
    editor: &Editor<'_>,
    id: &str,
    item: &str,
    index: usize,
) -> Result<(), EditError> {
    let (playlist, editor) = own(pool, editor, id).await?;
    let editor = &editor;
    let held = find(&playlist, item)?;
    let others: Vec<&Row> = playlist
        .items
        .iter()
        .filter(|i| i.get("id").and_then(Value::as_str) != Some(item))
        .collect();
    let at = index.min(others.len());
    let lower = at.checked_sub(1).and_then(|i| position(others[i]));
    let upper = others.get(at).and_then(|i| position(i));
    let current = position(held).unwrap_or_default();
    let above = lower.is_none_or(|lower| lower < current);
    let below = upper.is_none_or(|upper| current < upper);
    if above && below {
        return Ok(());
    }
    let key = keys_between(1, lower, upper)?.remove(0);
    let mut row = held.clone();
    row.insert("position".into(), key.into());
    let change = item_change(&row, editor, false);
    writing::append(
        pool,
        editor.user_id,
        editor.device,
        vec![change],
        editor.now,
    )
    .await?;
    Ok(())
}

fn find<'a>(playlist: &'a Playlist, item: &str) -> Result<&'a Row, EditError> {
    playlist
        .items
        .iter()
        .find(|i| i.get("id").and_then(Value::as_str) == Some(item))
        .ok_or(EditError::NoSuchItem)
}
