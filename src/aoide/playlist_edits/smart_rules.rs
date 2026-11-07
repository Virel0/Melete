//! Smart playlists' rules, written as a device writes them: a new smart playlist with its rules,
//! and new rules for one of the user's own. The rules are the apps' format (`aoide::smart`), held
//! in the row's `smartRules` as JSON text with its keys sorted, as the apps write it.

use super::*;
use crate::aoide::smart::Rules;

/// The rules as a row holds them.
fn rules_text(rules: &Rules) -> String {
    writing::canonical_json(&rules.to_json())
}

/// A new smart playlist named `name` with `rules`, after every other the user can see; its id.
///
/// # Errors
///
/// A write error.
pub async fn create_smart(
    pool: &SqlitePool,
    editor: &Editor<'_>,
    name: &str,
    rules: &Rules,
) -> Result<String, EditError> {
    let last = playlists::visible(pool, editor.user_id)
        .await?
        .iter()
        .filter_map(|p| p.row.get("sortIndex")?.as_str().map(str::to_owned))
        .max();
    let id = writing::new_id()?;
    let mut row = Row::new();
    row.insert("id".into(), id.clone().into());
    row.insert("name".into(), name.into());
    row.insert("isSmart".into(), true.into());
    row.insert("smartRules".into(), rules_text(rules).into());
    row.insert(
        "sortIndex".into(),
        fractional_index::between(last.as_deref(), None).into(),
    );
    // A new row's every field is new.
    let stamps: BTreeMap<String, i64> = row
        .keys()
        .chain(
            ["updatedAt", "deleted", "originDevice"]
                .map(String::from)
                .iter(),
        )
        .map(|k| (k.clone(), editor.now))
        .collect();
    write_playlist(pool, editor, row, &stamps, false).await?;
    Ok(id)
}

/// Gives the user's own smart playlist `id` new rules.
///
/// # Errors
///
/// [`EditError::HandMade`] for a hand-made playlist, [`EditError::NotOwner`] for someone
/// else's, [`EditError::NoSuchPlaylist`], or a write error.
pub async fn set_rules(
    pool: &SqlitePool,
    editor: &Editor<'_>,
    id: &str,
    rules: &Rules,
) -> Result<(), EditError> {
    let playlist = playlists::visible(pool, editor.user_id)
        .await?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or(EditError::NoSuchPlaylist)?;
    if playlist.owner_id != editor.user_id {
        return Err(EditError::NotOwner);
    }
    if !is_smart(&playlist.row) {
        return Err(EditError::HandMade);
    }
    // After every change it holds, as `own` times an edit.
    let latest = playlist
        .row
        .get("updatedAt")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let editor = Editor {
        now: editor.now.max(latest.saturating_add(1)),
        ..*editor
    };
    let mut row = playlist.row.clone();
    row.insert("smartRules".into(), rules_text(rules).into());
    if !row.contains_key("sortIndex") {
        let key = fractional_index::between(None, None);
        row.insert("sortIndex".into(), key.into());
    }
    save_row(pool, &editor, &playlist, row).await
}
