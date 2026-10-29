//! A track's lyrics: beside it, in its tags, else from LRCLIB, and kept with their search index.

use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct TrackRow {
    pub(super) root: String,
    pub(super) rel_path: String,
    pub(super) size: i64,
    pub(super) modified_at: i64,
    pub(super) title: String,
    pub(super) artist: String,
    pub(super) album: String,
    pub(super) duration_seconds: Option<f64>,
}

#[derive(sqlx::FromRow)]
pub(super) struct Kept {
    pub(super) source: String,
    pub(super) text: Option<String>,
    pub(super) synced: bool,
    pub(super) track_size: i64,
    pub(super) track_modified_at: i64,
    pub(super) checked_at: i64,
}

/// The track's file and the `.lrc` beside it, both inside the library; none when the library
/// or the file can't be reached.
pub(super) async fn paths(root: &str, rel_path: &str) -> Option<(PathBuf, Option<PathBuf>)> {
    let root = tokio::fs::canonicalize(root).await.ok()?;
    let track = tokio::fs::canonicalize(root.join(rel_path)).await.ok()?;
    if !track.starts_with(&root) {
        return None;
    }
    let sidecar = tokio::fs::canonicalize(track.with_extension("lrc"))
        .await
        .ok()
        .filter(|p| p.starts_with(&root));
    Some((track, sidecar))
}

pub(super) async fn read_sidecar(path: &Path) -> Option<String> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = tokio::fs::read(path).await.ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Lyrics found beside the track or in its tags. `Err` when its file can't be reached.
pub(super) async fn local(row: &TrackRow) -> Result<Option<Lyrics>, ()> {
    let (track, sidecar) = paths(&row.root, &row.rel_path).await.ok_or(())?;
    if let Some(sidecar) = sidecar
        && let Some(text) = read_sidecar(&sidecar).await
        && let Some(lyrics) = Lyrics::from_text(Source::File, &text)
    {
        return Ok(Some(lyrics));
    }
    let tagged = tokio::task::spawn_blocking(move || crate::library::tags::lyrics(&track))
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    Ok(tagged.and_then(|text| Lyrics::from_text(Source::Tags, &text)))
}

/// The lyrics' lines in the search index, in place of any the track had: the
/// non-empty ones, with their place and start.
pub(super) async fn index_lines(
    conn: &mut sqlx::SqliteConnection,
    track_id: i64,
    lyrics: Option<&Lyrics>,
) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM music_lyric_lines WHERE track_id = ?")
        .bind(track_id)
        .execute(&mut *conn)
        .await?;
    let lines = lyrics.map(Lyrics::lines).unwrap_or_default();
    for (position, line) in lines.iter().enumerate() {
        if line.text.trim().is_empty() {
            continue;
        }
        sqlx::query(
            "INSERT INTO music_lyric_lines (track_id, position, start_seconds, text)
             VALUES (?, ?, ?, ?)",
        )
        .bind(track_id)
        .bind(i64::try_from(position).unwrap_or(i64::MAX))
        .bind(line.start)
        .bind(&line.text)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Keeps what was found for a track (or that LRCLIB had nothing, as `None`), and indexes it.
pub(super) async fn keep(
    pool: &SqlitePool,
    track_id: i64,
    file: (i64, i64),
    lyrics: Option<&Lyrics>,
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO music_lyrics (track_id, source, text, synced, track_size,
             track_modified_at, checked_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (track_id) DO UPDATE SET source = excluded.source, text = excluded.text,
             synced = excluded.synced, track_size = excluded.track_size,
             track_modified_at = excluded.track_modified_at, checked_at = excluded.checked_at",
    )
    .bind(track_id)
    .bind(lyrics.map_or("none", |l| l.source.as_str()))
    .bind(lyrics.map(|l| &l.text))
    .bind(lyrics.is_some_and(|l| l.synced))
    .bind(file.0)
    .bind(file.1)
    .bind(unix_now())
    .execute(&mut *tx)
    .await?;
    index_lines(&mut tx, track_id, lyrics).await?;
    tx.commit().await
}

/// Forgets a track's lyrics, found in a file that no longer has them.
pub(super) async fn forget(pool: &SqlitePool, track_id: i64) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM music_lyrics WHERE track_id = ?")
        .bind(track_id)
        .execute(&mut *tx)
        .await?;
    index_lines(&mut tx, track_id, None).await?;
    tx.commit().await
}

/// A track's lyrics: beside it or in its tags as they are now; else as LRCLIB gave them (asked
/// once, and again only after a while when it had none, or when the file changed); else none.
/// While the track's file can't be reached, the lyrics last found stand.
pub async fn for_track(
    pool: &SqlitePool,
    lrclib: Option<&lrclib::Client>,
    track_id: i64,
) -> Result<Answer, Error> {
    let row: Option<TrackRow> = sqlx::query_as(
        "SELECT l.path AS root, t.rel_path, t.size, t.modified_at, t.title,
             coalesce(t.artist, t.album_artist, a.artist_name, '') AS artist,
             coalesce(a.title, t.album, '') AS album, t.duration_seconds
         FROM music_tracks t JOIN libraries l ON l.id = t.library_id
         LEFT JOIN music_albums a ON a.id = t.album_id
         WHERE t.id = ? AND t.missing_since IS NULL",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(Answer::NoTrack);
    };
    let kept: Option<Kept> = sqlx::query_as(
        "SELECT source, text, synced, track_size, track_modified_at, checked_at
         FROM music_lyrics WHERE track_id = ?",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?;
    let as_kept = |k: &Kept| {
        Source::parse(&k.source)
            .zip(k.text.clone())
            .map(|(source, text)| Lyrics {
                source,
                synced: k.synced,
                text,
            })
    };
    let found = match local(&row).await {
        Ok(found) => found,
        // Unreachable for now: what was found last stands.
        Err(()) => {
            return Ok(kept
                .as_ref()
                .and_then(as_kept)
                .map_or(Answer::Nothing, Answer::Found));
        }
    };
    if let Some(lyrics) = found {
        keep(pool, track_id, (row.size, row.modified_at), Some(&lyrics)).await?;
        return Ok(Answer::Found(lyrics));
    }
    // Nothing local: LRCLIB's answer, kept for this version of the file.
    let same_file = |k: &Kept| k.track_size == row.size && k.track_modified_at == row.modified_at;
    if let Some(k) = kept.as_ref().filter(|k| same_file(k)) {
        match k.source.as_str() {
            "lrclib" => {
                if let Some(lyrics) = as_kept(k) {
                    return Ok(Answer::Found(lyrics));
                }
            }
            "none" if unix_now() - k.checked_at < RETRY_AFTER_SECONDS => {
                return Ok(Answer::Nothing);
            }
            _ => {}
        }
    }
    let Some(client) = lrclib else {
        // A file's lyrics that have since gone aren't kept as if they were still there.
        if kept
            .as_ref()
            .is_some_and(|k| k.source == "file" || k.source == "tags")
        {
            forget(pool, track_id).await?;
        }
        return Ok(Answer::Nothing);
    };
    let asked = lrclib::Track {
        title: &row.title,
        artist: &row.artist,
        album: &row.album,
        seconds: row
            .duration_seconds
            .map_or(0, |s| s.max(0.0).round() as u64),
    };
    let known = client.lyrics(&asked).await.map_err(Error::Lrclib)?;
    let lyrics = known
        .synced
        .as_deref()
        .or(known.plain.as_deref())
        .and_then(|text| Lyrics::from_text(Source::Lrclib, text));
    keep(pool, track_id, (row.size, row.modified_at), lyrics.as_ref()).await?;
    Ok(lyrics.map_or(Answer::Nothing, Answer::Found))
}
