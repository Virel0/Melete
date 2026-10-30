//! An artist known only by name joining the MusicBrainz artist of that name: when one album of
//! theirs is tagged with MusicBrainz ids (by a tagger, say) and another isn't, they'd be two
//! artists otherwise. Names compare as search keys compare them (case, accents, spaces and
//! punctuation aside). Two MusicBrainz artists of one name stay apart, which is what the ids are
//! for, and a name matching several of them stays on its own.

use std::collections::HashMap;

/// Points every name-keyed artist's albums at the one MusicBrainz artist of that name, if
/// there's exactly one, and notes which artist each joined, so its address and pins lead there
/// (reading an artist follows `merged_into`); a name-keyed artist with no such match stands on
/// its own again. Run after every music scan, over every library, so it doesn't matter which
/// album or library was read first.
pub(super) async fn merge_named_artists(tx: &mut sqlx::SqliteConnection) -> anyhow::Result<()> {
    let rows: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id, key, name FROM music_artists ORDER BY id")
            .fetch_all(&mut *tx)
            .await?;
    let mut by_name: HashMap<String, Vec<i64>> = HashMap::new();
    for (id, key, name) in &rows {
        if key.starts_with("mb:") {
            by_name
                .entry(crate::search_keys::fold(name))
                .or_default()
                .push(*id);
        }
    }
    for (id, key, name) in &rows {
        if !key.starts_with("name:") {
            continue;
        }
        let joined = match by_name
            .get(&crate::search_keys::fold(name))
            .map(Vec::as_slice)
        {
            Some([only]) => Some(*only),
            _ => None,
        };
        sqlx::query("UPDATE music_artists SET merged_into = ? WHERE id = ?")
            .bind(joined)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if let Some(joined) = joined {
            sqlx::query("UPDATE music_albums SET artist_id = ? WHERE artist_id = ?")
                .bind(joined)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
    }
    Ok(())
}
