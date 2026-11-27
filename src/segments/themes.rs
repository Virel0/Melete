//! Openings and endings told apart, and who has seen which.
//!
//! When an episode's intro or credits are found by matching, the start of its fingerprint (60
//! seconds at most) is compared with those of the openings, or endings, already known for the
//! series, in any season. One sharing at least half of the shorter of the two (10 seconds at
//! least) is the same song, and the segment takes its id; otherwise the segment is a new one
//! (a new season's opening, or a change partway through a season) and gets a new id, kept with
//! that fingerprint. Each user's player says when they've played one through, so automatic
//! skipping can leave alone what's new to them. A segment with no id (found before ids were
//! given, by the ending alone, or from a chapter only) is never new.

use sqlx::{SqliteConnection, SqlitePool};

use super::Segment;
use super::fingerprint::{frames, shared};
use crate::db::unix_now;

/// How much of a segment's start is kept and compared, in seconds.
pub const KEPT_SECONDS: f64 = 60.0;
/// The least two must share to be the same song, in seconds.
const SAME_SECONDS: f64 = 10.0;
/// The least share of the shorter of the two that must agree.
const SAME_SHARE: f64 = 0.5;

/// The start of `segment`'s fingerprint, out of `print` (an episode's, its audio starting at
/// `from` seconds): [`KEPT_SECONDS`] at most.
pub fn excerpt(print: &[u32], from: f64, segment: Segment) -> Vec<u32> {
    let at = |seconds: f64| frames((seconds - from).max(0.0)).min(print.len());
    let start = at(segment.start);
    let end = at(segment.end).min(start + frames(KEPT_SECONDS));
    print[start..end.max(start)].to_vec()
}

/// Whether two excerpts are the same song.
fn same(a: &[u32], b: &[u32]) -> Option<usize> {
    let least = frames(SAME_SECONDS).max((a.len().min(b.len()) as f64 * SAME_SHARE) as usize);
    shared(a, b, least).map(|found| found.len())
}

fn to_bytes(print: &[u32]) -> Vec<u8> {
    print.iter().flat_map(|hash| hash.to_le_bytes()).collect()
}

fn from_bytes(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|four| u32::from_le_bytes(*four))
        .collect()
}

/// The id of the opening (`kind` "intro") or ending ("credits") of the series that `print`, a
/// segment's [`excerpt`], is: the known one it shares the most with, or else a new one kept
/// with it. None when it's too short to tell.
pub async fn recognise(
    conn: &mut SqliteConnection,
    series_id: i64,
    kind: &str,
    print: &[u32],
) -> sqlx::Result<Option<i64>> {
    if print.len() < frames(SAME_SECONDS) {
        return Ok(None);
    }
    let print = &print[..print.len().min(frames(KEPT_SECONDS))];
    let known: Vec<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT id, fingerprint FROM segment_themes WHERE series_id = ? AND kind = ? ORDER BY id",
    )
    .bind(series_id)
    .bind(kind)
    .fetch_all(&mut *conn)
    .await?;
    let mut best: Option<(usize, i64)> = None;
    for (id, bytes) in known {
        if let Some(length) = same(print, &from_bytes(&bytes))
            && best.is_none_or(|(most, _)| length > most)
        {
            best = Some((length, id));
        }
    }
    if let Some((_, id)) = best {
        return Ok(Some(id));
    }
    let id = sqlx::query_scalar(
        "INSERT INTO segment_themes (series_id, kind, fingerprint, created_at)
         VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(series_id)
    .bind(kind)
    .bind(to_bytes(print))
    .bind(unix_now())
    .fetch_one(&mut *conn)
    .await?;
    Ok(Some(id))
}

/// Notes as having seen `theme` everyone who had already played a file past the part of it
/// (ending at `end` seconds) just told apart as that theme, or finished the file: they've
/// reached past it, so nothing changes for them.
pub async fn reached_past(
    conn: &mut SqliteConnection,
    file_id: i64,
    theme: i64,
    end: f64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO segment_themes_seen (user_id, theme_id, seen_at)
         SELECT user_id, ?, ? FROM playback_progress
         WHERE file_id = ? AND (watched = 1 OR play_count > 0 OR position_seconds >= ?)
         ON CONFLICT (user_id, theme_id) DO NOTHING",
    )
    .bind(theme)
    .bind(unix_now())
    .bind(file_id)
    .bind(end)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Whether the intro and the credits found in the current version of a file are new to a
/// user: told apart, and not yet seen by them.
pub async fn new_to(pool: &SqlitePool, file_id: i64, user_id: i64) -> sqlx::Result<(bool, bool)> {
    let kinds: Vec<String> = sqlx::query_scalar(
        "SELECT seg.kind FROM media_segments seg
         JOIN media_segment_scans s ON s.file_id = seg.file_id
         JOIN media_files f ON f.id = seg.file_id
             AND f.size = s.size AND f.modified_at = s.modified_at
         WHERE seg.file_id = ? AND seg.theme_id IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM segment_themes_seen w
               WHERE w.user_id = ? AND w.theme_id = seg.theme_id)",
    )
    .bind(file_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    let has = |kind: &str| kinds.iter().any(|k| k == kind);
    Ok((has("intro"), has("credits")))
}

/// Notes that a user has seen the file's intro (or credits), and so every episode's that is the
/// same song. Nothing to note when the file's current version has none told apart.
pub async fn seen(pool: &SqlitePool, file_id: i64, user_id: i64, kind: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO segment_themes_seen (user_id, theme_id, seen_at)
         SELECT ?, seg.theme_id, ? FROM media_segments seg
         JOIN media_segment_scans s ON s.file_id = seg.file_id
         JOIN media_files f ON f.id = seg.file_id
             AND f.size = s.size AND f.modified_at = s.modified_at
         WHERE seg.file_id = ? AND seg.kind = ? AND seg.theme_id IS NOT NULL
         ON CONFLICT (user_id, theme_id) DO NOTHING",
    )
    .bind(user_id)
    .bind(unix_now())
    .bind(file_id)
    .bind(kind)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::fingerprint::fingerprint;
    use super::super::fingerprint::tests::{Noise, episode};
    use super::*;
    use crate::db;

    async fn show() -> SqlitePool {
        let pool = db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'shows', 'shows', '/s');
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation) VALUES
                 (1, 1, 'Show', 'Show', 1), (2, 1, 'Other', 'Other', 1);",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    /// The excerpt of a song played `before` seconds into an episode of its own.
    fn heard(seed: u64, before: f64, song: &[f32]) -> Vec<u32> {
        let print = fingerprint(&episode(seed, before, song, 20.0));
        let length = song.len() as f64 / 8000.0;
        let segment = Segment {
            start: before,
            end: before + length,
        };
        excerpt(&print, 0.0, segment)
    }

    #[tokio::test]
    async fn the_same_song_keeps_its_id_and_a_new_one_gets_its_own() {
        let pool = show().await;
        let mut conn = pool.acquire().await.unwrap();
        let first = Noise::new(11).tune(90.0);
        let second = Noise::new(12).tune(85.0);
        let a = recognise(&mut conn, 1, "intro", &heard(1, 30.0, &first))
            .await
            .unwrap()
            .unwrap();
        let again = recognise(&mut conn, 1, "intro", &heard(2, 4.7, &first))
            .await
            .unwrap();
        assert_eq!(
            again,
            Some(a),
            "the same opening, elsewhere in another episode"
        );
        let b = recognise(&mut conn, 1, "intro", &heard(3, 30.0, &second))
            .await
            .unwrap()
            .unwrap();
        assert_ne!(a, b, "a changed opening is a new one");
        assert_eq!(
            recognise(&mut conn, 1, "intro", &heard(4, 12.0, &second))
                .await
                .unwrap(),
            Some(b)
        );
        assert_eq!(
            recognise(&mut conn, 1, "intro", &heard(5, 12.0, &first))
                .await
                .unwrap(),
            Some(a),
            "the first one, back again"
        );
        let ending = recognise(&mut conn, 1, "credits", &heard(6, 12.0, &first))
            .await
            .unwrap();
        assert!(
            ending.is_some_and(|e| e != a),
            "endings are told apart on their own"
        );
        let elsewhere = recognise(&mut conn, 2, "intro", &heard(7, 12.0, &first))
            .await
            .unwrap();
        assert!(elsewhere.is_some_and(|e| e != a), "each series its own");
        drop(conn);
        let kept: Vec<i64> =
            sqlx::query_scalar("SELECT length(fingerprint) FROM segment_themes ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(kept.len(), 4);
        assert!(
            kept.iter()
                .all(|&bytes| bytes == 4 * frames(KEPT_SECONDS) as i64),
            "a minute at most: {kept:?}"
        );
    }

    #[tokio::test]
    async fn too_short_a_stretch_is_not_told_apart() {
        let pool = show().await;
        let mut conn = pool.acquire().await.unwrap();
        let short = heard(1, 10.0, &Noise::new(13).tune(8.0));
        assert_eq!(
            recognise(&mut conn, 1, "intro", &short).await.unwrap(),
            None
        );
    }

    #[test]
    fn an_excerpt_is_the_segments_start() {
        let print: Vec<u32> = (0..10_000).collect();
        let segment = Segment {
            start: 310.0,
            end: 320.0,
        };
        let part = excerpt(&print, 300.0, segment);
        assert_eq!(part.first(), Some(&(frames(10.0) as u32)));
        assert_eq!(part.len(), frames(20.0) - frames(10.0));
        let long = Segment {
            start: 0.0,
            end: 200.0,
        };
        assert_eq!(excerpt(&print, 0.0, long).len(), frames(KEPT_SECONDS));
        assert_eq!(from_bytes(&to_bytes(&part)), part);
    }
}
