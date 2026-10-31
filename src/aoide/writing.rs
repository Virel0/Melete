//! Ops the server writes into a user's sync log on their behalf, for apps that don't sync
//! (the web UI): each is exactly what an Aoide device would push for the same change, so a
//! device applies it without knowing where it came from. The writer is the session the request
//! came with, as a device that doesn't name itself is; its ops go through the same checks and
//! the same push as a device's.

use serde_json::{Map, Value};
use sqlx::SqlitePool;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use super::sync;

/// Why a change couldn't be written.
#[derive(Debug)]
pub enum WriteError {
    /// The playlist isn't the user's to edit.
    Refused(String),
    /// The database failed.
    Database(sqlx::Error),
    /// No randomness to mint an id with.
    NoRandomness,
}

impl From<sqlx::Error> for WriteError {
    fn from(err: sqlx::Error) -> Self {
        Self::Database(err)
    }
}

/// The device name ops written for a session carry: what the sync routes call a device that
/// doesn't name itself.
pub fn session_device(token_id: i64) -> String {
    format!("token-{token_id}")
}

/// A fresh row or op id, as the apps mint them: an upper-case hyphenated version 4 UUID.
///
/// # Errors
///
/// [`WriteError::NoRandomness`] when the system has none to give.
pub fn new_id() -> Result<String, WriteError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| WriteError::NoRandomness)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// JSON with every object's keys sorted, at every depth, and nothing between tokens: the
/// encoding the apps write payloads in, so the same row is always the same bytes.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write(value, &mut out);
    out
}

fn write(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            out.push('{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write(&map[key.as_str()], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// One change to write: a row of an entity, upserted whole or soft-deleted.
pub struct Change {
    /// Which of the sync entities it changes.
    pub entity: &'static str,
    /// Whether it deletes the row (its payload then says `deleted`).
    pub delete: bool,
    /// The full row after the change, its `id` the row's id.
    pub row: Map<String, Value>,
}

/// Appends `changes`, in order, to `user_id`'s log as one push from `device` at `now`: each
/// an op with a fresh id, the row as its payload. All of `changes` must be to one row, or one
/// playlist and its items, so that a refusal refuses them all.
///
/// # Errors
///
/// [`WriteError::Refused`] when a change is to a playlist the user may not edit (nothing is
/// then written), else a database error or no randomness.
pub async fn append(
    pool: &SqlitePool,
    user_id: i64,
    device: &str,
    changes: Vec<Change>,
    now: i64,
) -> Result<(), WriteError> {
    let mut ops = Vec::with_capacity(changes.len());
    for change in changes {
        let entity_id = change
            .row
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let op = format!(
            r#"{{"opId":{},"entity":{},"entityId":{},"operation":{},"payload":{},"createdAt":{now}}}"#,
            Value::from(new_id()?),
            Value::from(change.entity),
            Value::from(entity_id),
            Value::from(if change.delete { "delete" } else { "upsert" }),
            canonical_json(&Value::Object(change.row)),
        );
        let raw = serde_json::value::RawValue::from_string(op)
            .map_err(|err| WriteError::Refused(err.to_string()))?;
        ops.push(sync::validate(&raw).map_err(|(_, why)| WriteError::Refused(why))?);
    }
    // A push leaves out ops on a playlist the user may not edit. Every change written here at
    // once is to one row, or one playlist and its items, so all of them are left out or none.
    let pushed = sync::push(pool, user_id, device, ops, now).await?;
    match pushed.denied.into_iter().next() {
        Some((_, why)) => Err(WriteError::Refused(why)),
        None => Ok(()),
    }
}

/// How a row names a track: its id as the apps write it (`jellyfinId`, which holds Melete's
/// track id) and the content key they relink it by when an id stops resolving.
pub struct TrackLink {
    /// Melete's track id, as text.
    pub jellyfin_id: String,
    /// `artist|album|title|seconds`, as the apps fold it.
    pub content_key: String,
}

/// A present track's link, made from what the music API shows of it, as an app would make it
/// from its track cache: the album artist (else the first credited artist, else "Unknown
/// Artist"), the album, the title and the length to the nearest second. None when the track
/// isn't in the library.
///
/// # Errors
///
/// Fails when the database does.
pub async fn track_link(pool: &SqlitePool, track_id: i64) -> sqlx::Result<Option<TrackLink>> {
    /// Title, artist tag, album, album artist, length in seconds.
    type Named = (String, Option<String>, String, String, Option<f64>);
    let row: Option<Named> = sqlx::query_as(
        "SELECT t.title, t.artist, a.title, a.artist_name,
             coalesce(t.duration_seconds, (SELECT p.duration_seconds FROM music_track_probes p
                 WHERE p.track_id = t.id AND p.size = t.size AND p.modified_at = t.modified_at))
         FROM music_tracks t JOIN music_albums a ON a.id = t.album_id
         WHERE t.id = ? AND t.missing_since IS NULL",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(title, artist, album, album_artist, seconds)| {
        let artist = Some(album_artist)
            .filter(|a| !a.is_empty())
            .or_else(|| artist.and_then(|a| a.split(", ").next().map(str::to_owned)))
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| "Unknown Artist".to_owned());
        TrackLink {
            jellyfin_id: track_id.to_string(),
            content_key: content_key(&artist, &album, &title, seconds),
        }
    }))
}

/// The apps' fingerprint of a recording: artist, album and title folded, and the length
/// rounded to the second, joined with `|`.
pub fn content_key(artist: &str, album: &str, title: &str, seconds: Option<f64>) -> String {
    let ms = seconds.map_or(0, |s| (s * 1000.0).round() as i64);
    let seconds = (ms + 500) / 1000;
    format!("{}|{}|{}|{seconds}", fold(artist), fold(album), fold(title))
}

/// How a row names an album: its id (`albumId`, Melete's album id) and the content key a
/// device relinks it by when the id stops resolving.
pub struct AlbumLink {
    /// Melete's album id, as text.
    pub album_id: String,
    /// `artist|album`, as the apps fold it.
    pub content_key: String,
}

/// A present album's link, made from what the music API shows of it: its artist (else
/// "Unknown Artist") and title. None when the album isn't in the library.
///
/// # Errors
///
/// Fails when the database does.
pub async fn album_link(pool: &SqlitePool, album_id: i64) -> sqlx::Result<Option<AlbumLink>> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT title, artist_name FROM music_albums WHERE id = ? AND missing_since IS NULL",
    )
    .bind(album_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(title, artist)| AlbumLink {
        album_id: album_id.to_string(),
        content_key: album_content_key(&artist, &title),
    }))
}

/// The apps' fingerprint of an album: its artist ("Unknown Artist" when it has none) and
/// title, folded as a track's are, joined with `|`.
pub fn album_content_key(artist: &str, title: &str) -> String {
    let artist = if artist.trim().is_empty() {
        "Unknown Artist"
    } else {
        artist
    };
    format!("{}|{}", fold(artist), fold(title))
}

/// Accents and case folded away, trimmed, then one pass collapsing double spaces: the apps'
/// order of operations.
fn fold(text: &str) -> String {
    let folded: String = text
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase();
    folded.trim().replace("  ", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_keys_fold_as_the_apps_do() {
        // The apps' own example, and their rounding of the length.
        assert_eq!(
            content_key(
                "The Quiet Engines",
                "Low Tide",
                "Harbour Lights",
                Some(243.5)
            ),
            "the quiet engines|low tide|harbour lights|244"
        );
        assert_eq!(
            content_key(" Björk ", "Homogenic", "Jóga    x", None),
            "bjork|homogenic|joga  x|0"
        );
    }

    #[test]
    fn album_keys_fold_as_track_keys_do() {
        assert_eq!(album_content_key(" Björk ", "Homogénic"), "bjork|homogenic");
        assert_eq!(album_content_key("  ", "Demos"), "unknown artist|demos");
    }

    #[test]
    fn ids_are_the_apps_spelling() {
        let id = new_id().unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(id, id.to_uppercase());
        assert_eq!(&id[14..15], "4", "version 4");
        assert!(matches!(&id[19..20], "8" | "9" | "A" | "B"), "{id}");
        assert_ne!(id, new_id().unwrap());
    }

    #[test]
    fn payloads_are_sorted_and_compact() {
        let value: Value =
            serde_json::from_str(r#"{"b":1,"a":{"d":[true,null],"c":"x"},"é":"ü"}"#).unwrap();
        assert_eq!(
            canonical_json(&value),
            r#"{"a":{"c":"x","d":[true,null]},"b":1,"é":"ü"}"#
        );
    }
}
