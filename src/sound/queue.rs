//! Which tracks still need measuring, the ones apps asked about first, and measuring them one file
//! at a time.

use super::*;

/// What one round of measuring did, in tracks.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// Tracks measured and saved.
    pub measured: usize,
    /// Tracks ffmpeg rejected; not tried again until the file changes.
    pub failed: usize,
    /// Tracks that couldn't be read or were cut short; tried again on a later round.
    pub retry_later: usize,
}

#[derive(sqlx::FromRow)]
pub(super) struct Pending {
    pub(super) id: i64,
    pub(super) root: String,
    pub(super) rel_path: String,
    pub(super) modified_at: i64,
}

/// Tracks an app asked about before they were measured, measured ahead of the rest.
pub struct Requests(Mutex<BTreeSet<i64>>);

/// The one the API fills and the background measuring empties.
pub static REQUESTS: Requests = Requests::new();

/// More asked-for tracks than this wait their turn like any other.
pub(super) const MAX_REQUESTED: usize = 10_000;

impl Requests {
    /// An empty queue.
    pub const fn new() -> Self {
        Self(Mutex::new(BTreeSet::new()))
    }

    /// Puts tracks at the front of the queue and wakes the measuring.
    pub fn request(&self, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }
        let mut requested = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for &id in ids {
            if requested.len() >= MAX_REQUESTED {
                break;
            }
            requested.insert(id);
        }
        drop(requested);
        crate::jobs::ANALYSIS.notify_one();
    }

    pub(super) fn next(&self) -> Option<i64> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_first()
    }
}

/// Present tracks after `after` whose measurements are missing, older than the file, or from
/// an older analysis; only `only` when given.
pub(super) async fn pending(
    pool: &SqlitePool,
    after: i64,
    only: Option<i64>,
) -> sqlx::Result<Vec<Pending>> {
    sqlx::query_as(
        "SELECT t.id, l.path AS root, t.rel_path, t.modified_at
         FROM music_tracks t JOIN libraries l ON l.id = t.library_id
         WHERE t.missing_since IS NULL AND t.id > ?1 AND (?4 IS NULL OR t.id = ?4)
           AND (NOT EXISTS (SELECT 1 FROM music_sound_bounds b WHERE b.track_id = t.id
                              AND b.modified_at = t.modified_at AND b.analyzer >= ?2)
                OR NOT EXISTS (SELECT 1 FROM music_audio_analysis a WHERE a.track_id = t.id
                              AND a.modified_at = t.modified_at AND a.analyzer >= ?2)
                OR NOT EXISTS (SELECT 1 FROM music_beat_grids g WHERE g.track_id = t.id
                              AND g.modified_at = t.modified_at AND g.analyzer >= ?2)
                OR NOT EXISTS (SELECT 1 FROM music_arrangements r WHERE r.track_id = t.id
                              AND r.modified_at = t.modified_at AND r.analyzer >= ?2))
         ORDER BY t.id LIMIT ?3",
    )
    .bind(after)
    .bind(ANALYZER)
    .bind(BATCH)
    .bind(only)
    .fetch_all(pool)
    .await
}

/// Measures every pending track, one file at a time, oldest first except that tracks an app
/// asked about go before the next file.
pub async fn measure_pending(
    pool: &SqlitePool,
    tools: &Tools,
    requests: &Requests,
) -> anyhow::Result<Report> {
    let mut report = Report::default();
    // Each track is visited once a run, so one that must wait doesn't hold up the rest.
    let mut visited = HashSet::new();
    let mut after = 0;
    loop {
        let batch = pending(pool, after, None).await?;
        let Some(last) = batch.last() else {
            break;
        };
        after = last.id;
        for track in batch {
            while let Some(id) = requests.next() {
                if visited.insert(id)
                    && let Some(asked) = pending(pool, 0, Some(id)).await?.pop()
                {
                    measure_track(pool, tools, &asked, &mut report).await?;
                }
            }
            if visited.insert(track.id) {
                measure_track(pool, tools, &track, &mut report).await?;
            }
        }
    }
    Ok(report)
}

pub(super) async fn measure_track(
    pool: &SqlitePool,
    tools: &Tools,
    track: &Pending,
    report: &mut Report,
) -> anyhow::Result<()> {
    let file = Path::new(&track.root).join(&track.rel_path);
    let result = measure(tools, &file).await;
    match &result {
        Ok(_) => report.measured += 1,
        Err(MeasureError::Failed(reason)) => {
            tracing::warn!(
                rel_path = track.rel_path,
                reason,
                "couldn't measure a track"
            );
            report.failed += 1;
        }
        Err(err @ MeasureError::Unreadable(_)) => {
            tracing::debug!(rel_path = track.rel_path, error = %err, "measuring will be retried");
            report.retry_later += 1;
            return Ok(());
        }
        Err(err @ MeasureError::Unavailable(_)) => anyhow::bail!("{err}"),
    }
    store(pool, track.id, track.modified_at, &result).await?;
    Ok(())
}
