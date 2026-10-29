//! Downloading Soulseek results through slskd. Grabbing a result
//! asks slskd for the folder's files in batches, one per folder below the album's (a CD1 or
//! Disc 2 folder), each into a numbered folder of the download's own in slskd's downloads
//! folder: `melete/<download id>/<n>`. The import then takes whatever is in those folders,
//! whatever slskd named the files, and nothing else.
//!
//! slskd is asked first whether it shares anything; Melete won't use it if it does. A download
//! that fails stays recorded as failed, so its id (and its folder) is never reused.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::downloads::{self, Download, Refused};
use crate::slskd::{self, Transfer, Wanted};
use crate::soulseek::Source;

/// How often slskd is asked about downloads in progress.
const FOLLOW_EVERY: Duration = Duration::from_secs(30);

/// What a Soulseek download keeps: whom it's from, and its parts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stored {
    /// The Soulseek user it's fetched from.
    pub username: String,
    /// The album folder on the peer's machine.
    pub folder: String,
    /// Its batches, one per folder below the album's.
    pub parts: Vec<Part>,
}

/// One batch: the files of one folder below the album's, fetched into `melete/<id>/<n>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Part {
    /// The batch's id, made by Melete when asking slskd for it; its transfers are listed under it.
    pub batch: String,
    /// The folder below the album's it came from ("" or "CD1"); the import recreates it.
    pub folder: String,
    /// The files in the batch.
    pub files: Vec<Wanted>,
}

/// slskd's folder for part `n` (from 1) of download `id`, relative to its downloads folder.
pub fn destination(id: i64, n: usize) -> String {
    format!("melete/{id}/{n}")
}

#[derive(sqlx::FromRow)]
struct ReleaseRow {
    title: String,
    accepted: bool,
    decision: String,
    soulseek: Option<String>,
    wanted_album_id: Option<i64>,
    wanted_song_id: Option<i64>,
}

/// The folder each file sits in below the album's, or None for a path that climbs out of it
/// or has an empty or "." part.
fn below(album_folder: &str, filename: &str) -> Option<String> {
    let separator = if filename.contains('\\') { '\\' } else { '/' };
    let rest = filename
        .strip_prefix(album_folder)?
        .strip_prefix(separator)?;
    let parts: Vec<&str> = rest.split(separator).collect();
    let (_, folders) = parts.split_last()?;
    if parts
        .iter()
        .any(|p| p.is_empty() || *p == "." || *p == "..")
    {
        return None;
    }
    Some(folders.join("/"))
}

/// The result's files, grouped by the folder below the album's, in the order first seen.
fn parts(source: &Source) -> Option<Vec<(String, Vec<Wanted>)>> {
    let mut grouped: Vec<(String, Vec<Wanted>)> = Vec::new();
    for file in &source.files {
        let folder = below(&source.folder, &file.filename)?;
        let wanted = Wanted {
            filename: file.filename.clone(),
            size: file.size,
        };
        match grouped.iter_mut().find(|(f, _)| *f == folder) {
            Some((_, files)) => files.push(wanted),
            None => grouped.push((folder, vec![wanted])),
        }
    }
    Some(grouped)
}

/// Asks slskd to fetch an accepted Soulseek result and starts following it.
pub async fn grab(
    pool: &SqlitePool,
    slskd: &slskd::Client,
    release_id: i64,
) -> anyhow::Result<Result<Download, Refused>> {
    let (row, source) = match grabbable(pool, release_id).await? {
        Ok(found) => found,
        Err(refused) => return Ok(Err(refused)),
    };
    let Some(grouped) = parts(&source) else {
        return Ok(Err(Refused::Slskd(
            "the peer lists a file outside its album folder".into(),
        )));
    };
    if let Err(err) = slskd.ensure_not_sharing().await {
        return Ok(Err(Refused::Slskd(format!("{err:#}"))));
    }
    let stored = batches(&source, grouped)?;
    let size: i64 = source.files.iter().map(|f| f.size).sum();
    let id = insert_download(pool, &row, &stored, size).await?;
    let Some(id) = id else {
        return Ok(Err(Refused::Already(already(pool, &source).await?)));
    };
    if let Err(refused) = enqueue(pool, slskd, &stored, id).await? {
        return Ok(Err(refused));
    }
    sqlx::query("UPDATE downloads SET message = 'queued with slskd' WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(Ok(downloads::get(pool, id).await?.ok_or_else(|| {
        anyhow::anyhow!("download {id} vanished")
    })?))
}

/// The Soulseek result and what it offers, when it's accepted and not blocklisted by its title.
async fn grabbable(
    pool: &SqlitePool,
    release_id: i64,
) -> anyhow::Result<Result<(ReleaseRow, Source), Refused>> {
    let row: Option<ReleaseRow> = sqlx::query_as(
        "SELECT d.title, d.accepted, d.decision, d.soulseek, s.wanted_album_id, s.wanted_song_id
         FROM release_decisions d JOIN searches s ON s.id = d.search_id
         WHERE d.id = ?",
    )
    .bind(release_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(Err(Refused::NoRelease));
    };
    let Some(source) = row.soulseek.as_deref() else {
        return Ok(Err(Refused::NoRelease));
    };
    if !row.accepted {
        let decision: serde_json::Value = serde_json::from_str(&row.decision)?;
        let why = decision["reasons"][0]
            .as_str()
            .unwrap_or("no reason recorded")
            .to_owned();
        return Ok(Err(Refused::NotAccepted(why)));
    }
    if crate::blocklist::blocks(pool, &row.title, None).await? {
        return Ok(Err(Refused::Blocklisted));
    }
    let source: Source = serde_json::from_str(source)?;
    Ok(Ok((row, source)))
}

/// What the download keeps: one batch, with a new id, per folder below the album's.
fn batches(source: &Source, grouped: Vec<(String, Vec<Wanted>)>) -> anyhow::Result<Stored> {
    let mut stored = Stored {
        username: source.username.clone(),
        folder: source.folder.clone(),
        parts: Vec::new(),
    };
    for (folder, files) in grouped {
        stored.parts.push(Part {
            batch: slskd::new_batch_id()?,
            folder,
            files,
        });
    }
    Ok(stored)
}

/// Records the download with its batches; None when the same thing is downloading already.
async fn insert_download(
    pool: &SqlitePool,
    row: &ReleaseRow,
    stored: &Stored,
    size: i64,
) -> anyhow::Result<Option<i64>> {
    let now = unix_now();
    // The batches are recorded before slskd hears of them, so a restart can follow them.
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO downloads (source, soulseek, title, wanted_album_id, wanted_song_id, decision,
             state, size, message, added_at, updated_at)
         VALUES ('soulseek', ?, ?, ?, ?, ?, 'downloading', ?, 'asking slskd for it', ?, ?)
         ON CONFLICT DO NOTHING
         RETURNING id",
    )
    .bind(serde_json::to_string(stored)?)
    .bind(&row.title)
    .bind(row.wanted_album_id)
    .bind(row.wanted_song_id)
    .bind(&row.decision)
    .bind(size)
    .bind(now)
    .bind(now)
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

/// The download already fetching this result.
async fn already(pool: &SqlitePool, source: &Source) -> anyhow::Result<i64> {
    // The same folder for an album; the same file of it for a song.
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM downloads
             WHERE source = 'soulseek' AND state != 'failed'
               AND json_extract(soulseek, '$.username') = ? AND json_extract(soulseek, '$.folder') = ?
               AND (wanted_song_id IS NULL
                    OR json_extract(soulseek, '$.parts[0].files[0].filename') = ?)",
    )
    .bind(&source.username)
    .bind(&source.folder)
    .bind(source.files.first().map(|f| f.filename.as_str()))
    .fetch_optional(pool)
    .await?;
    Ok(existing.unwrap_or_default())
}

/// Hands each batch to slskd; when one is refused, stops the rest and records the download as
/// failed.
async fn enqueue(
    pool: &SqlitePool,
    slskd: &slskd::Client,
    stored: &Stored,
    id: i64,
) -> anyhow::Result<Result<(), Refused>> {
    for (n, part) in stored.parts.iter().enumerate() {
        let queued = slskd
            .enqueue(
                &part.batch,
                &stored.username,
                &destination(id, n + 1),
                &part.files,
            )
            .await;
        if let Err(err) = queued {
            let why = format!("{err:#}");
            cancel(slskd, stored).await;
            sqlx::query(
                "UPDATE downloads SET state = 'failed', message = ?, updated_at = ? WHERE id = ?",
            )
            .bind(&why)
            .bind(unix_now())
            .bind(id)
            .execute(pool)
            .await?;
            return Ok(Err(Refused::Slskd(why)));
        }
    }
    Ok(Ok(()))
}

/// Stops whatever slskd is still fetching for a download; what can't be stopped is left.
async fn cancel(slskd: &slskd::Client, stored: &Stored) {
    for part in &stored.parts {
        let Ok(Some(transfers)) = slskd.batch(&part.batch).await else {
            continue;
        };
        for transfer in transfers.iter().filter(|t| !t.state.contains("Completed")) {
            if let Err(err) = slskd.cancel(&stored.username, &transfer.id).await {
                tracing::warn!(
                    error = format!("{err:#}"),
                    "couldn't cancel a Soulseek file"
                );
            }
        }
    }
}

/// Follows Soulseek downloads for as long as the server runs.
pub async fn follow(pool: SqlitePool, slskd: std::sync::Arc<slskd::Client>) {
    loop {
        if let Err(err) = refresh(&pool, &slskd).await {
            tracing::warn!(
                error = format!("{err:#}"),
                "couldn't ask slskd about downloads"
            );
        }
        tokio::time::sleep(FOLLOW_EVERY).await;
    }
}

/// How a download stands, from its files' transfers.
#[derive(Debug, PartialEq)]
enum Standing {
    Done,
    Failed(String),
    Going { progress: f64, message: String },
}

fn standing(expected: usize, transfers: &[Transfer]) -> Standing {
    if let Some(failed) = transfers.iter().find(|t| t.failed()) {
        let name = failed
            .filename
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or_default();
        let state = failed.state.trim_start_matches("Completed, ");
        let why = match &failed.exception {
            Some(exception) if !exception.is_empty() => format!(" ({exception})"),
            _ => String::new(),
        };
        return Standing::Failed(format!("slskd: {name}: {state}{why}"));
    }
    let done = transfers.iter().filter(|t| t.succeeded()).count();
    if done == expected && transfers.len() == expected {
        return Standing::Done;
    }
    let size: i64 = transfers.iter().map(|t| t.size).sum();
    let got: i64 = transfers
        .iter()
        .map(|t| t.bytes_transferred.min(t.size))
        .sum();
    let progress = if size > 0 {
        (got as f64 * 100.0 / size as f64).clamp(0.0, 99.9)
    } else {
        0.0
    };
    let message = if done == 0 && transfers.iter().all(|t| t.state.contains("Queued")) {
        match transfers.iter().filter_map(|t| t.place_in_queue).min() {
            Some(place) => format!("queued by the peer (place {place})"),
            None => "queued".to_owned(),
        }
    } else {
        format!("downloading: {done} of {expected} files done")
    };
    Standing::Going { progress, message }
}

/// Asks slskd how each Soulseek download is doing; marks finished ones completed and ones
/// with a file that failed failed. Returns how many it asked about.
pub async fn refresh(pool: &SqlitePool, slskd: &slskd::Client) -> anyhow::Result<usize> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, soulseek FROM downloads WHERE source = 'soulseek' AND state = 'downloading'",
    )
    .fetch_all(pool)
    .await?;
    for (id, stored) in &rows {
        let stored: Stored = serde_json::from_str(stored)?;
        let expected: usize = stored.parts.iter().map(|p| p.files.len()).sum();
        let mut transfers = Vec::new();
        let mut forgotten = false;
        for part in &stored.parts {
            match slskd.batch(&part.batch).await? {
                Some(found) => transfers.extend(found),
                None => forgotten = true,
            }
        }
        let standing = if forgotten {
            Standing::Failed("slskd no longer knows part of it".into())
        } else {
            standing(expected, &transfers)
        };
        let now = unix_now();
        match standing {
            Standing::Done => {
                sqlx::query(
                    "UPDATE downloads SET state = 'completed', progress = 100, completed_at = ?,
                         message = 'all of it is in slskd''s folder', updated_at = ?
                     WHERE id = ? AND state = 'downloading'",
                )
                .bind(now)
                .bind(now)
                .bind(id)
                .execute(pool)
                .await?;
            }
            Standing::Failed(why) => {
                cancel(slskd, &stored).await;
                sqlx::query(
                    "UPDATE downloads SET state = 'failed', message = ?, updated_at = ?
                     WHERE id = ? AND state = 'downloading'",
                )
                .bind(why)
                .bind(now)
                .bind(id)
                .execute(pool)
                .await?;
            }
            Standing::Going { progress, message } => {
                sqlx::query(
                    "UPDATE downloads SET progress = ?, message = ?, updated_at = ?
                     WHERE id = ? AND state = 'downloading'",
                )
                .bind(progress)
                .bind(message)
                .bind(now)
                .bind(id)
                .execute(pool)
                .await?;
            }
        }
    }
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::slskd::testing;

    const FOLDER: &str = "@@m\\Radiohead\\OK Computer";

    async fn fixture(accepted: bool) -> SqlitePool {
        let pool = db::open_in_memory().await;
        let source = serde_json::json!({
            "username": "alice",
            "folder": FOLDER,
            "files": [
                {"filename": format!("{FOLDER}\\CD1\\01 Airbag.flac"), "size": 30},
                {"filename": format!("{FOLDER}\\CD1\\02 Paranoid Android.flac"), "size": 50},
                {"filename": format!("{FOLDER}\\CD2\\01 Lucky.flac"), "size": 40},
                {"filename": format!("{FOLDER}\\cover.jpg"), "size": 5},
            ],
        });
        sqlx::query(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
                 VALUES (1, 'mb-a', 'Radiohead', 1, 'all', 'lidarr', 0);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at)
                 VALUES (7, 1, 'rg', 'OK Computer', 1, 'lidarr', 0);
             INSERT INTO searches (id, wanted_album_id, searched_at, indexers) VALUES (1, 7, 0, '[]');
             INSERT INTO release_decisions (id, search_id, title, size, accepted, decision, soulseek)
                 VALUES (5, 1, 'Radiohead - OK Computer', 125, ?, ?, ?);",
        )
        .bind(accepted)
        .bind(if accepted {
            r#"{"accepted":true,"quality":"FLAC","rank":1,"reasons":[]}"#
        } else {
            r#"{"accepted":false,"quality":"MP3-320","rank":null,"reasons":["the profile doesn't allow MP3-320"]}"#
        })
        .bind(source.to_string())
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn state(pool: &SqlitePool, id: i64) -> (String, f64, Option<String>) {
        sqlx::query_as("SELECT state, progress, message FROM downloads WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[test]
    fn files_keep_the_folder_below_the_albums_and_nothing_climbs_out() {
        assert_eq!(below("a\\b", "a\\b\\CD1\\01.flac").as_deref(), Some("CD1"));
        assert_eq!(below("a\\b", "a\\b\\01.flac").as_deref(), Some(""));
        assert_eq!(below("a/b", "a/b/x/y/01.flac").as_deref(), Some("x/y"));
        assert_eq!(below("a\\b", "a\\bc\\01.flac"), None, "another folder");
        assert_eq!(below("a\\b", "a\\b\\..\\01.flac"), None);
        assert_eq!(below("a\\b", "a\\b\\\\01.flac"), None);
        assert_eq!(below("a\\b", "x\\01.flac"), None);
    }

    #[tokio::test]
    async fn grabs_a_folder_in_one_batch_per_disc_into_its_own_folders() {
        let pool = fixture(true).await;
        let (slskd, seen) = testing::start(serde_json::json!([]), 1).await;
        let download = grab(&pool, &slskd, 5).await.unwrap().unwrap();
        assert_eq!(
            (
                download.source.as_str(),
                download.infohash.as_deref(),
                download.wanted_album_id,
                download.state.as_str(),
                download.size,
                download.message.as_deref()
            ),
            (
                "soulseek",
                None,
                Some(7),
                "downloading",
                Some(125),
                Some("queued with slskd")
            )
        );
        let id = download.id;
        {
            let seen = seen.lock().unwrap();
            assert_eq!(
                seen.shares_asked, 1,
                "asked first whether it shares anything"
            );
            let sent: Vec<(String, String, usize)> = seen
                .enqueued
                .iter()
                .map(|b| {
                    (
                        b["username"].as_str().unwrap().to_owned(),
                        b["options"]["destination"].as_str().unwrap().to_owned(),
                        b["files"].as_array().unwrap().len(),
                    )
                })
                .collect();
            assert_eq!(
                sent,
                [
                    ("alice".into(), format!("melete/{id}/1"), 2),
                    ("alice".into(), format!("melete/{id}/2"), 1),
                    ("alice".into(), format!("melete/{id}/3"), 1),
                ]
            );
            assert_eq!(
                seen.enqueued[0]["files"][1],
                serde_json::json!({"filename": format!("{FOLDER}\\CD1\\02 Paranoid Android.flac"), "size": 50})
            );
        }
        let stored: String = sqlx::query_scalar("SELECT soulseek FROM downloads WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let stored: Stored = serde_json::from_str(&stored).unwrap();
        let folders: Vec<&str> = stored.parts.iter().map(|p| p.folder.as_str()).collect();
        assert_eq!(folders, ["CD1", "CD2", ""]);
        assert_eq!(
            stored.parts[0].batch,
            seen.lock().unwrap().enqueued[0]["id"].as_str().unwrap()
        );

        // The same folder again, while the first is under way.
        assert_eq!(
            grab(&pool, &slskd, 5).await.unwrap(),
            Err(Refused::Already(id))
        );
    }

    #[tokio::test]
    async fn refuses_when_slskd_shares_anything_or_the_result_was_turned_down() {
        let pool = fixture(true).await;
        let (slskd, seen) = testing::start(serde_json::json!([]), 1).await;
        seen.lock().unwrap().shared = 1;
        assert_eq!(
            grab(&pool, &slskd, 5).await.unwrap(),
            Err(Refused::Slskd(
                "slskd shares 1 folder(s); Melete won't use an slskd that shares anything".into()
            ))
        );
        assert!(seen.lock().unwrap().enqueued.is_empty());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM downloads")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        // Searching is refused too.
        let err = slskd.search("Radiohead").await.unwrap_err().to_string();
        assert!(
            err.contains("won't use an slskd that shares anything"),
            "{err}"
        );

        let turned_down = fixture(false).await;
        assert_eq!(
            grab(&turned_down, &slskd, 5).await.unwrap(),
            Err(Refused::NotAccepted(
                "the profile doesn't allow MP3-320".into()
            ))
        );
    }

    #[tokio::test]
    async fn a_song_is_one_file_and_another_from_its_folder_is_another_download() {
        let pool = db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO wanted_songs (id, title, artist, source, added_at)
                 VALUES (2, 'Paranoid Android', 'Radiohead', 'manual', 0),
                        (3, 'Airbag', 'Radiohead', 'manual', 0);
             INSERT INTO searches (id, wanted_song_id, searched_at, indexers)
                 VALUES (1, 2, 0, '[]'), (2, 3, 0, '[]');
             INSERT INTO release_decisions (id, search_id, title, accepted, decision, soulseek) VALUES
                 (5, 1, 'Radiohead - Paranoid Android', 1, '{\"accepted\":true,\"reasons\":[]}',
                  '{\"username\":\"alice\",\"folder\":\"a\",\"files\":[{\"filename\":\"a\\\\02 Paranoid Android.flac\",\"size\":9}]}'),
                 (6, 2, 'Radiohead - Airbag', 1, '{\"accepted\":true,\"reasons\":[]}',
                  '{\"username\":\"alice\",\"folder\":\"a\",\"files\":[{\"filename\":\"a\\\\01 Airbag.flac\",\"size\":7}]}');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (slskd, seen) = testing::start(serde_json::json!([]), 1).await;
        let song = grab(&pool, &slskd, 5).await.unwrap().unwrap();
        assert_eq!(
            (
                song.wanted_song_id,
                song.wanted_album_id,
                song.source.as_str()
            ),
            (Some(2), None, "soulseek")
        );
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.enqueued.len(), 1);
            assert_eq!(
                seen.enqueued[0]["options"]["destination"],
                format!("melete/{}/1", song.id)
            );
            assert_eq!(seen.enqueued[0]["files"].as_array().unwrap().len(), 1);
        }
        let other = grab(&pool, &slskd, 6).await.unwrap().unwrap();
        assert_ne!(other.id, song.id, "another song from the same folder");
        assert_eq!(
            grab(&pool, &slskd, 5).await.unwrap(),
            Err(Refused::Already(song.id)),
            "but the same song again isn't"
        );

        // The database holds that a song comes through Soulseek, and is one title.
        let torrent = sqlx::query(
            "INSERT INTO downloads (infohash, title, wanted_song_id, decision, state, added_at, updated_at)
             VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'x', 2, '{}', 'downloading', 0, 0)",
        )
        .execute(&pool)
        .await;
        assert!(
            torrent
                .unwrap_err()
                .to_string()
                .contains("through Soulseek")
        );
        let both = sqlx::query("UPDATE downloads SET wanted_album_id = 7 WHERE id = ?")
            .bind(song.id)
            .execute(&pool)
            .await;
        assert!(both.unwrap_err().to_string().contains("one wanted title"));
    }

    #[tokio::test]
    async fn a_refused_enqueue_stays_failed_and_the_folder_can_be_tried_again() {
        let pool = fixture(true).await;
        let (slskd, seen) = testing::start(serde_json::json!([]), 1).await;
        seen.lock().unwrap().refuse_enqueue = Some("User alice appears to be offline".into());
        assert_eq!(
            grab(&pool, &slskd, 5).await.unwrap(),
            Err(Refused::Slskd(
                "slskd couldn't queue it: User alice appears to be offline".into()
            ))
        );
        let (state_now, _, message) = state(&pool, 1).await;
        assert_eq!(state_now, "failed");
        assert_eq!(
            message.as_deref(),
            Some("slskd couldn't queue it: User alice appears to be offline")
        );

        seen.lock().unwrap().refuse_enqueue = None;
        let again = grab(&pool, &slskd, 5).await.unwrap().unwrap();
        assert_ne!(again.id, 1, "a new download; the failed one keeps its id");
    }

    #[tokio::test]
    async fn follows_slskd_until_every_file_is_there() {
        let pool = fixture(true).await;
        let (slskd, seen) = testing::start(serde_json::json!([]), 1).await;
        let id = grab(&pool, &slskd, 5).await.unwrap().unwrap().id;

        assert_eq!(refresh(&pool, &slskd).await.unwrap(), 1);
        assert_eq!(
            state(&pool, id).await,
            (
                "downloading".into(),
                0.0,
                Some("queued by the peer (place 3)".into())
            )
        );
        seen.lock().unwrap().transfer_state = "InProgress".into();
        refresh(&pool, &slskd).await.unwrap();
        let (now, progress, message) = state(&pool, id).await;
        assert_eq!(now, "downloading");
        // Half of each file, rounded down: 62 of 125 bytes.
        assert!((progress - 49.6).abs() < 0.01, "{progress}");
        assert_eq!(message.as_deref(), Some("downloading: 0 of 4 files done"));

        seen.lock().unwrap().transfer_state = "Completed, Succeeded".into();
        refresh(&pool, &slskd).await.unwrap();
        assert_eq!(
            state(&pool, id).await,
            (
                "completed".into(),
                100.0,
                Some("all of it is in slskd's folder".into())
            )
        );
        assert_eq!(
            refresh(&pool, &slskd).await.unwrap(),
            0,
            "nothing left to follow"
        );
    }

    #[tokio::test]
    async fn a_file_the_peer_refuses_fails_the_download() {
        let pool = fixture(true).await;
        let (slskd, seen) = testing::start(serde_json::json!([]), 1).await;
        let id = grab(&pool, &slskd, 5).await.unwrap().unwrap().id;
        seen.lock().unwrap().transfer_state = "Completed, Rejected".into();
        refresh(&pool, &slskd).await.unwrap();
        assert_eq!(
            state(&pool, id).await,
            (
                "failed".into(),
                0.0,
                Some("slskd: 01 Airbag.flac: Rejected".into())
            )
        );

        // slskd forgetting a batch (its database cleared, say) fails it too.
        let pool = fixture(true).await;
        let id = grab(&pool, &slskd, 5).await.unwrap().unwrap().id;
        {
            let mut seen = seen.lock().unwrap();
            seen.transfer_state = String::new();
            seen.forgotten = true;
        }
        refresh(&pool, &slskd).await.unwrap();
        assert_eq!(
            state(&pool, id).await.2.as_deref(),
            Some("slskd no longer knows part of it")
        );
    }

    #[tokio::test]
    async fn a_failed_download_stops_what_slskd_is_still_fetching() {
        let pool = fixture(true).await;
        let (slskd, seen) = testing::start(serde_json::json!([]), 1).await;
        let id = grab(&pool, &slskd, 5).await.unwrap().unwrap().id;
        let stored: Stored = serde_json::from_str(
            &sqlx::query_scalar::<_, String>("SELECT soulseek FROM downloads WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap(),
        )
        .unwrap();
        cancel(&slskd, &stored).await;
        let cancelled = seen.lock().unwrap().cancelled.clone();
        assert_eq!(cancelled.len(), 4, "every file still queued");
        assert!(cancelled[0].starts_with("alice/"), "{cancelled:?}");
    }
}
