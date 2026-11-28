//! One conversion for a watch party: its members play the same file in step, so those whose
//! asks come out the same (the file's version, the audio, the picture's size and bitrate) share
//! one ffmpeg run and the playlist it makes, each through their own link.

use sha2::{Digest, Sha256};

use super::*;

/// The key a link's conversion runs under: shared by the party's viewers when the link's
/// device or guest is in a watch party playing this file, else the link's own (`own`, its
/// token's hash). Two keys are the same only for the same party, file version and plan, so a
/// guest held to a lower cap never shares a better conversion: theirs is planned apart.
pub(super) async fn conversion_key(
    pool: &SqlitePool,
    link: (i64, Option<i64>),
    file: &LoadedFile,
    plan: &transcode::Plan,
    own: &[u8],
) -> Result<Vec<u8>, ApiError> {
    let Some(party) = party_of(pool, link, file.id).await? else {
        return Ok(own.to_vec());
    };
    let mut same = plan.clone();
    // Only what's said in the log, not what's made.
    same.about = transcode::About::default();
    let mut key = Sha256::new();
    key.update(b"watch party conversion\0");
    for n in [party, file.id, file.size, file.modified_at] {
        key.update(n.to_le_bytes());
    }
    key.update(format!("{same:?}").as_bytes());
    Ok(key.finalize().to_vec())
}

/// The watch party a link's device or guest is in that plays `file_id`, if any.
async fn party_of(
    pool: &SqlitePool,
    (token_id, guest_id): (i64, Option<i64>),
    file_id: i64,
) -> sqlx::Result<Option<i64>> {
    match guest_id {
        Some(guest) => {
            sqlx::query_scalar(
                "SELECT p.id FROM watch_party_guests g JOIN watch_parties p ON p.id = g.party_id
                 WHERE g.id = ? AND p.file_id = ?",
            )
            .bind(guest)
            .bind(file_id)
            .fetch_optional(pool)
            .await
        }
        None => {
            sqlx::query_scalar(
                "SELECT p.id FROM watch_party_members m JOIN watch_parties p ON p.id = m.party_id
                 WHERE m.token_id = ? AND p.file_id = ? ORDER BY p.id LIMIT 1",
            )
            .bind(token_id)
            .bind(file_id)
            .fetch_optional(pool)
            .await
        }
    }
}
