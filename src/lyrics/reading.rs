//! Reading the lyrics beside every track and in their tags, looking again only at what changed.

use super::*;

/// What a pass over the library's lyrics did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Tracks whose lyrics were read and kept.
    pub found: u64,
    /// Tracks whose lyrics had gone from their file.
    pub gone: u64,
}

/// A file's modification time in nanoseconds, to tell an edited `.lrc` from the one seen.
pub(super) async fn modified_ns(path: &Path) -> Option<i64> {
    let modified = tokio::fs::metadata(path).await.ok()?.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(since.as_nanos()).ok()
}

#[derive(sqlx::FromRow)]
pub(super) struct Seen {
    pub(super) id: i64,
    pub(super) root: String,
    pub(super) rel_path: String,
    pub(super) size: i64,
    pub(super) modified_at: i64,
    pub(super) seen_size: Option<i64>,
    pub(super) seen_modified_at: Option<i64>,
    pub(super) sidecar_modified_at: Option<i64>,
    pub(super) kept_source: Option<String>,
}

/// Reads lyrics beside every track and in their tags into the kept lyrics and the search
/// index, looking again only at tracks or `.lrc` files that changed since the last
/// pass. Local only: nothing is asked of LRCLIB. A library that can't be reached is left as
/// it was.
pub async fn read_library(pool: &SqlitePool) -> sqlx::Result<Report> {
    let tracks: Vec<Seen> = sqlx::query_as(
        "SELECT t.id, l.path AS root, t.rel_path, t.size, t.modified_at,
             s.track_size AS seen_size, s.track_modified_at AS seen_modified_at,
             s.sidecar_modified_at, k.source AS kept_source
         FROM music_tracks t JOIN libraries l ON l.id = t.library_id
         LEFT JOIN music_lyrics_seen s ON s.track_id = t.id
         LEFT JOIN music_lyrics k ON k.track_id = t.id
         WHERE t.missing_since IS NULL ORDER BY t.id",
    )
    .fetch_all(pool)
    .await?;
    let mut report = Report::default();
    let mut reachable: HashMap<String, bool> = HashMap::new();
    for track in tracks {
        if !*reachable.entry(track.root.clone()).or_insert(
            tokio::fs::metadata(&track.root)
                .await
                .is_ok_and(|m| m.is_dir()),
        ) {
            continue;
        }
        let sidecar_at = match paths(&track.root, &track.rel_path).await {
            Some((_, Some(sidecar))) => modified_ns(&sidecar).await,
            Some((_, None)) => None,
            // The file's gone since the scan; the next scan will say so.
            None => continue,
        };
        let unchanged = track.seen_size == Some(track.size)
            && track.seen_modified_at == Some(track.modified_at)
            && track.sidecar_modified_at == sidecar_at;
        if unchanged {
            continue;
        }
        let row = TrackRow {
            root: track.root,
            rel_path: track.rel_path,
            size: track.size,
            modified_at: track.modified_at,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            duration_seconds: None,
        };
        let Ok(found) = local(&row).await else {
            continue;
        };
        match found {
            Some(lyrics) => {
                keep(pool, track.id, (row.size, row.modified_at), Some(&lyrics)).await?;
                report.found += 1;
            }
            None if matches!(track.kept_source.as_deref(), Some("file" | "tags")) => {
                forget(pool, track.id).await?;
                report.gone += 1;
            }
            None => {}
        }
        sqlx::query(
            "INSERT INTO music_lyrics_seen (track_id, track_size, track_modified_at,
                 sidecar_modified_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (track_id) DO UPDATE SET track_size = excluded.track_size,
                 track_modified_at = excluded.track_modified_at,
                 sidecar_modified_at = excluded.sidecar_modified_at",
        )
        .bind(track.id)
        .bind(row.size)
        .bind(row.modified_at)
        .bind(sidecar_at)
        .execute(pool)
        .await?;
    }
    // Lyrics kept before the search index was, indexed now.
    let unindexed: Vec<(i64, String, String, bool)> = sqlx::query_as(
        "SELECT k.track_id, k.source, k.text, k.synced FROM music_lyrics k
         WHERE k.text IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM music_lyric_lines l WHERE l.track_id = k.track_id)",
    )
    .fetch_all(pool)
    .await?;
    for (track_id, source, text, synced) in unindexed {
        let Some(source) = Source::parse(&source) else {
            continue;
        };
        let lyrics = Lyrics {
            source,
            synced,
            text,
        };
        let mut conn = pool.acquire().await?;
        index_lines(&mut conn, track_id, Some(&lyrics)).await?;
    }
    Ok(report)
}
