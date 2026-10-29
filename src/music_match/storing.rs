//! Caching a matched release with its tracks, and giving the album's tracks their MusicBrainz ids
//! by disc and position.

use super::*;

pub(super) async fn store_release(
    tx: &mut sqlx::SqliteConnection,
    release: &Release,
    now: i64,
) -> anyhow::Result<()> {
    let credits: Vec<serde_json::Value> = release
        .artist_credit
        .iter()
        .map(|c| {
            json!({ "name": c.name, "join": c.joinphrase, "id": c.artist.id,
                    "sort_name": c.artist.sort_name })
        })
        .collect();
    sqlx::query(
        "INSERT INTO mb_releases (id, release_group_id, title, date, status, country, primary_type,
             secondary_types, first_release_date, artist_credit, cover_front, fetched_at, with_works,
             with_track_credits)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, 1)
         ON CONFLICT (id) DO UPDATE SET
             release_group_id = excluded.release_group_id, title = excluded.title,
             date = excluded.date, status = excluded.status, country = excluded.country,
             primary_type = excluded.primary_type, secondary_types = excluded.secondary_types,
             first_release_date = excluded.first_release_date,
             artist_credit = excluded.artist_credit, cover_front = excluded.cover_front,
             fetched_at = excluded.fetched_at, with_works = 1, with_track_credits = 1",
    )
    .bind(&release.id)
    .bind(&release.release_group.id)
    .bind(&release.title)
    .bind(&release.date)
    .bind(&release.status)
    .bind(&release.country)
    .bind(&release.release_group.primary_type)
    .bind(serde_json::to_string(
        release
            .release_group
            .secondary_types
            .as_deref()
            .unwrap_or_default(),
    )?)
    .bind(&release.release_group.first_release_date)
    .bind(serde_json::to_string(&credits)?)
    .bind(release.cover_art.as_ref().is_some_and(|c| c.front))
    .bind(now)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM mb_release_tracks WHERE release_id = ?")
        .bind(&release.id)
        .execute(&mut *tx)
        .await?;
    for medium in &release.media {
        for track in &medium.tracks {
            let performed = track.recording.performed().unwrap_or_default();
            let composers: Vec<serde_json::Value> = performed
                .composers
                .iter()
                .map(|c| json!({ "id": c.id, "name": c.name, "sort_name": c.sort_name }))
                .collect();
            sqlx::query(
                "INSERT INTO mb_release_tracks (release_id, disc, position, number, title, length_ms,
                     track_id, recording_id, work_id, work_title, movement, movement_number,
                     composers, artist_credit)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, nullif(?, ''), nullif(?, ''), ?, ?, ?, ?)",
            )
            .bind(&release.id)
            .bind(medium.position)
            .bind(track.position)
            .bind(&track.number)
            .bind(&track.title)
            .bind(track.length)
            .bind(&track.id)
            .bind(&track.recording.id)
            .bind(&performed.work_id)
            .bind(&performed.work_title)
            .bind(&performed.movement)
            .bind(performed.movement_number)
            .bind(serde_json::to_string(&composers)?)
            .bind(track_credit(&track.artist_credit))
            .execute(&mut *tx)
            .await?;
        }
    }
    Ok(())
}

/// A release track's credit as kept: each artist's name as credited, the join to the next, their
/// id and their own name. None when the answer had none.
fn track_credit(credit: &[musicbrainz::Credit]) -> Option<String> {
    let credits: Vec<serde_json::Value> = credit
        .iter()
        .map(|c| {
            json!({ "name": c.name, "join": c.joinphrase, "id": c.artist.id,
                    "artist": c.artist.name })
        })
        .collect();
    (!credits.is_empty()).then(|| serde_json::Value::from(credits).to_string())
}

/// Links the tracks of matched albums that lost their release ids (their files changed) to the
/// cached release again.
pub(super) async fn relink(pool: &SqlitePool) -> anyhow::Result<()> {
    let albums: Vec<(i64, String)> = sqlx::query_as(
        "SELECT DISTINCT a.id, a.match_release_id FROM music_albums a
         JOIN music_tracks t ON t.album_id = a.id AND t.missing_since IS NULL
         WHERE a.missing_since IS NULL AND a.match_status = 'matched'
           AND a.match_release_id IS NOT NULL AND t.match_recording_id IS NULL",
    )
    .fetch_all(pool)
    .await?;
    for (album_id, release_id) in albums {
        let mut tx = pool.begin().await?;
        link_tracks(&mut tx, album_id, &release_id).await?;
        tx.commit().await?;
    }
    Ok(())
}

/// Gives the album's tracks the cached release's recording and track ids by disc and position,
/// when the counts agree. A track without a disc number is on disc 1 of a one-disc release.
pub(super) async fn link_tracks(
    tx: &mut sqlx::SqliteConnection,
    album_id: i64,
    release_id: &str,
) -> anyhow::Result<()> {
    let theirs: Vec<(i64, i64, String, String)> = sqlx::query_as(
        "SELECT disc, position, recording_id, track_id FROM mb_release_tracks WHERE release_id = ?",
    )
    .bind(release_id)
    .fetch_all(&mut *tx)
    .await?;
    let ours: Vec<(i64, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT id, disc_number, track_number FROM music_tracks
         WHERE album_id = ? AND missing_since IS NULL",
    )
    .bind(album_id)
    .fetch_all(&mut *tx)
    .await?;
    if ours.len() != theirs.len() {
        return Ok(());
    }
    let mut discs: Vec<i64> = theirs.iter().map(|t| t.0).collect();
    discs.sort_unstable();
    discs.dedup();
    let only_disc = match discs[..] {
        [disc] => Some(disc),
        _ => None,
    };
    for (id, disc, number) in ours {
        let Some(number) = number else { continue };
        let Some(disc) = disc.or(only_disc) else {
            continue;
        };
        if let Some((_, _, recording, track)) = theirs.iter().find(|t| t.0 == disc && t.1 == number)
        {
            sqlx::query(
                "UPDATE music_tracks SET match_recording_id = ?, match_track_id = ? WHERE id = ?",
            )
            .bind(recording)
            .bind(track)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
    }
    Ok(())
}
