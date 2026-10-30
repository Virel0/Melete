//! Keeping probe results, chapters included, and probing every file that needs it.

use super::*;

/// Stores a probe result with its chapters, replacing whatever was stored for the file before.
/// Timeouts aren't stored, so the file is tried again next time.
pub async fn store_probed(
    pool: &SqlitePool,
    file_id: i64,
    size: i64,
    modified_at: i64,
    result: &Result<Probed, ProbeError>,
) -> anyhow::Result<()> {
    // An extra keeps its probe on its own row, without chapters.
    if crate::library::extras::is_extra_id(file_id) {
        let info = match result {
            Ok(probed) => Ok(probed.info.clone()),
            Err(ProbeError::Failed(reason)) => Err(ProbeError::Failed(reason.clone())),
            Err(_) => return Ok(()),
        };
        return store(pool, file_id, size, modified_at, &info).await;
    }
    let found = match result {
        Ok(probed) => Found::Info(
            serde_json::to_string(&probed.info)?,
            probed
                .chapters
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        ),
        Err(err) => Found::from_error(err),
    };
    write(pool, (file_id, size, modified_at), found).await
}

/// What a probe found, as stored.
enum Found {
    /// The streams' JSON, and the chapters' when the file was read for them.
    Info(String, Option<String>),
    /// Why ffprobe rejected the file.
    Rejected(String),
    /// Nothing worth storing: the file is tried again next time.
    Nothing,
}

impl Found {
    fn from_error(err: &ProbeError) -> Self {
        match err {
            ProbeError::Failed(reason) => Self::Rejected(reason.clone()),
            ProbeError::TimedOut | ProbeError::Unreadable(_) | ProbeError::Unavailable(_) => {
                Self::Nothing
            }
        }
    }
}

/// Writes what was found for one version (id, size, date) of a file.
async fn write(pool: &SqlitePool, version: (i64, i64, i64), found: Found) -> anyhow::Result<()> {
    let (info, chapters, error) = match found {
        Found::Info(info, chapters) => (Some(info), chapters, None),
        Found::Rejected(reason) => (None, None, Some(reason)),
        Found::Nothing => return Ok(()),
    };
    let (file_id, size, modified_at) = version;
    sqlx::query(
        "INSERT INTO media_probes (file_id, size, modified_at, probed_at, info, error, chapters)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (file_id) DO UPDATE SET
             size = excluded.size, modified_at = excluded.modified_at,
             probed_at = excluded.probed_at, info = excluded.info, error = excluded.error,
             chapters = excluded.chapters",
    )
    .bind(file_id)
    .bind(size)
    .bind(modified_at)
    .bind(unix_now())
    .bind(info)
    .bind(error)
    .bind(chapters)
    .execute(pool)
    .await?;
    Ok(())
}

/// What a probing run did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ProbeReport {
    /// Files probed.
    pub probed: usize,
    /// Files ffprobe rejected; stored, and retried only when the file changes.
    pub failed: usize,
    /// Timeouts and unreadable files; tried again next run.
    pub retry_later: usize,
}

/// Probes every present file and extra that has no probe yet, whose size or date changed
/// since, or (a file) that was probed before its chapters were read.
///
/// Fails before touching anything if ffprobe can't run, so a missing binary can't be recorded
/// as thousands of broken files.
pub async fn probe_pending(pool: &SqlitePool, ffprobe: &Path) -> anyhow::Result<ProbeReport> {
    let version = Command::new(ffprobe)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .with_context(|| format!("starting {}; set MELETE_FFPROBE", ffprobe.display()))?;
    if !version.success() {
        bail!("{} -version failed", ffprobe.display());
    }

    let pending: Vec<(i64, String, String, i64, i64)> = sqlx::query_as(
        "SELECT f.id, l.path, f.rel_path, f.size, f.modified_at
         FROM media_files f
         JOIN libraries l ON l.id = f.library_id
         LEFT JOIN media_probes p ON p.file_id = f.id
         WHERE f.missing_since IS NULL
           AND (p.file_id IS NULL OR p.size <> f.size OR p.modified_at <> f.modified_at
                -- Read before chapters were.
                OR (p.info IS NOT NULL AND p.chapters IS NULL)
                -- Dolby Vision probed before its profile was recorded.
                OR EXISTS (SELECT 1 FROM json_each(p.info, '$.video') v
                           WHERE json_extract(v.value, '$.hdr') = 'Dolby Vision'
                             AND json_extract(v.value, '$.dolby_vision') IS NULL))
         UNION ALL
         -- Extras, kept apart from the files but probed alike.
         SELECT e.id, l.path, e.rel_path, e.size, e.modified_at
         FROM media_extras e
         JOIN libraries l ON l.id = e.library_id
         WHERE e.missing_since IS NULL
           AND (e.probed_size IS NOT e.size OR e.probed_modified_at IS NOT e.modified_at)
         ORDER BY 1",
    )
    .fetch_all(pool)
    .await?;
    if !pending.is_empty() {
        tracing::info!(files = pending.len(), "probing files");
    }
    let pace = crate::jobs::pace::Pace::start("library", "probing files", pending.len());

    let slots = Arc::new(Semaphore::new(PARALLEL_PROBES));
    let ffprobe: Arc<PathBuf> = Arc::new(ffprobe.to_path_buf());
    let mut tasks = JoinSet::new();
    for (id, root, rel_path, size, modified_at) in pending {
        let (slots, ffprobe) = (slots.clone(), ffprobe.clone());
        tasks.spawn(async move {
            let _slot = slots
                .acquire_owned()
                .await
                .expect("semaphore is never closed");
            let result = probe_with_chapters(&ffprobe, &Path::new(&root).join(&rel_path)).await;
            (id, size, modified_at, rel_path, result)
        });
    }
    let mut report = ProbeReport::default();
    while let Some(joined) = tasks.join_next().await {
        let item = pace.item();
        let (id, size, modified_at, rel_path, result) = joined?;
        match &result {
            Ok(_) => report.probed += 1,
            Err(ProbeError::Failed(reason)) => {
                tracing::warn!(rel_path, reason, "ffprobe rejected a file");
                report.failed += 1;
                item.failed();
            }
            Err(err @ (ProbeError::TimedOut | ProbeError::Unreadable(_))) => {
                tracing::debug!(rel_path, error = %err, "probe will be retried");
                report.retry_later += 1;
            }
            Err(err @ ProbeError::Unavailable(_)) => bail!("{err}"),
        }
        store_probed(pool, id, size, modified_at, &result).await?;
    }
    pace.finish();
    Ok(report)
}
