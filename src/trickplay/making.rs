//! Making a file's previews with ffmpeg, and the background run that makes them for every file
//! that needs them.

use super::*;

/// What a run of making previews did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Files whose previews were made.
    pub made: usize,
    /// Files ffmpeg couldn't read; looked at again only once they change.
    pub failed: usize,
}

/// The ffmpeg command line for a file's previews: keyframes only, one at least `INTERVAL`
/// after the last, scaled to `width` by `height`, its time logged (`showinfo`), and tiled into
/// numbered JPEG sheets in `dir`.
pub fn args(input: &Path, width: i64, height: i64, dir: &Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = ["-hide_banner", "-nostdin", "-v", "info"]
        .iter()
        .map(Into::into)
        .collect();
    args.extend(["-skip_frame", "nokey", "-i"].map(Into::into));
    let mut file = std::ffi::OsString::from("file:");
    file.push(input);
    args.push(file);
    let filters = format!(
        "select='isnan(prev_selected_t)+gte(t-prev_selected_t,{INTERVAL})',scale={width}:{height},showinfo,tile={COLUMNS}x{ROWS}"
    );
    args.extend(
        [
            "-map",
            "0:v:0",
            "-an",
            "-sn",
            "-dn",
            "-vf",
            &filters,
            "-fps_mode",
            "vfr",
            "-q:v",
            "5",
            "-start_number",
            "0",
            "-f",
            "image2",
        ]
        .map(Into::into),
    );
    args.push(dir.join("%d.jpg").into());
    args
}

/// The times of the pictures `showinfo` logged, in order.
pub fn logged_times(log: &str) -> Vec<f64> {
    log.lines()
        .filter(|l| l.contains("Parsed_showinfo"))
        .filter_map(|l| {
            let at = l.split("pts_time:").nth(1)?;
            at.split_whitespace().next()?.parse().ok()
        })
        .collect()
}

/// The height of a picture `wide` pixels wide in the shape of a video `width` by `height`,
/// even, as encoders want.
pub fn picture_height(wide: i64, width: i64, height: i64) -> i64 {
    let h = (wide as f64 * height as f64 / width as f64 / 2.0).round() as i64 * 2;
    h.clamp(2, wide * 4)
}

/// Makes a file's previews `wide` pixels wide into `dir` (made afresh), with its index.
///
/// # Errors
///
/// ffmpeg's reason when it couldn't read the file, or why the sheets couldn't be written.
pub async fn make(
    ffmpeg: &Path,
    input: &Path,
    video: (i64, i64),
    wide: i64,
    dir: &Path,
) -> anyhow::Result<Index> {
    let height = picture_height(wide, video.0, video.1);
    let _ = tokio::fs::remove_dir_all(dir).await;
    tokio::fs::create_dir_all(dir).await?;
    let run = Command::new(ffmpeg)
        .args(args(input, wide, height, dir))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(MAKE_TIMEOUT, run)
        .await
        .context("reading the file's keyframes took over 30 minutes")?
        .with_context(|| format!("starting {}", ffmpeg.display()))?;
    let log = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let reason: String = log
            .lines()
            .rev()
            .find(|l| !l.contains("Parsed_showinfo"))
            .unwrap_or("ffmpeg failed")
            .chars()
            .take(300)
            .collect();
        anyhow::bail!("ffmpeg couldn't read it: {reason}");
    }
    let times = logged_times(&log);
    anyhow::ensure!(!times.is_empty(), "ffmpeg found no keyframes");
    let per_sheet = (COLUMNS * ROWS) as usize;
    let index = Index {
        width: wide,
        height,
        columns: COLUMNS,
        rows: ROWS,
        sheets: times.len().div_ceil(per_sheet) as i64,
        times,
    };
    for sheet in 0..index.sheets {
        let path = dir.join(format!("{sheet}.jpg"));
        anyhow::ensure!(path.exists(), "ffmpeg didn't write sheet {sheet}");
    }
    tokio::fs::write(dir.join("index.json"), serde_json::to_vec(&index)?).await?;
    Ok(index)
}

/// Present videos with a picture, probed in their current version, without previews made from
/// it, or with previews made at another width than `?1` asks for. One text for counting them
/// and taking the next, so the two can't drift apart.
macro_rules! pending_files {
    () => {
        "FROM media_files f
         JOIN libraries l ON l.id = f.library_id
         JOIN media_probes p ON p.file_id = f.id
             AND p.size = f.size AND p.modified_at = f.modified_at
         LEFT JOIN media_trickplay t ON t.file_id = f.id
         WHERE f.missing_since IS NULL
           AND json_extract(p.info, '$.video[0].width') > 0
           AND json_extract(p.info, '$.video[0].height') > 0
           AND (t.file_id IS NULL OR t.size <> f.size OR t.modified_at <> f.modified_at
                OR (t.error IS NULL AND t.width <> ?1))"
    };
}

/// A file whose previews are to be made.
#[derive(sqlx::FromRow)]
struct Pending {
    id: i64,
    library: String,
    rel_path: String,
    size: i64,
    modified_at: i64,
    width: i64,
    height: i64,
    /// It has previews of this version already, at another width.
    remake: bool,
}

/// Makes previews for every present video not made yet in its current version, one at a
/// time, while it's turned on (`MELETE_TRICKPLAY`), at the width chosen
/// (`MELETE_TRICKPLAY_WIDTH`). Previews made at another width are made again, after the files
/// with none; the old ones are served until the new ones replace them. A file ffmpeg can't
/// read keeps why, and is looked at again only once it changes; one that fails at a new width
/// keeps its old previews and isn't tried at that width again.
pub async fn make_pending(pool: &SqlitePool, ffmpeg: &Path, root: &Path) -> anyhow::Result<Report> {
    let mut report = Report::default();
    let total: i64 = sqlx::query_scalar(concat!("SELECT count(*) ", pending_files!()))
        .bind(super::width(pool).await?)
        .fetch_one(pool)
        .await?;
    let total = usize::try_from(total).unwrap_or_default();
    let pace = crate::jobs::pace::Pace::start("trickplay", "making seek-bar previews", total);
    loop {
        // Turned off meanwhile: the run stops, and says it stopped before the end.
        if !crate::settings::trickplay(pool).await? {
            return Ok(report);
        }
        let wide = super::width(pool).await?;
        let next: Option<Pending> = sqlx::query_as(concat!(
            "SELECT f.id, l.path AS library, f.rel_path, f.size, f.modified_at,
                 json_extract(p.info, '$.video[0].width') AS width,
                 json_extract(p.info, '$.video[0].height') AS height,
                 coalesce(t.size = f.size AND t.modified_at = f.modified_at, 0) AS remake ",
            pending_files!(),
            " ORDER BY remake, f.id LIMIT 1"
        ))
        .bind(wide)
        .fetch_optional(pool)
        .await?;
        let Some(file) = next else {
            pace.finish();
            return Ok(report);
        };
        let item = pace.item();
        let making = root.join(format!(
            ".making-{}-{}-{}",
            file.id, file.size, file.modified_at
        ));
        let input = Path::new(&file.library).join(&file.rel_path);
        let made = make(ffmpeg, &input, (file.width, file.height), wide, &making).await;
        let error = match made {
            Ok(_) => {
                let done = version_dir(root, file.id, file.size, file.modified_at);
                let _ = tokio::fs::remove_dir_all(&done).await;
                tokio::fs::rename(&making, &done).await?;
                report.made += 1;
                None
            }
            Err(err) => {
                let _ = tokio::fs::remove_dir_all(&making).await;
                tracing::warn!(
                    rel_path = file.rel_path,
                    error = format!("{err:#}"),
                    "couldn't make seek-bar previews"
                );
                report.failed += 1;
                item.failed();
                // The previews it has stay; it isn't tried at this width again.
                (!file.remake).then(|| format!("{err:#}"))
            }
        };
        record(pool, &file, wide, error).await?;
    }
}

/// Keeps what making a file's previews came to: made (or kept) at `wide`, or why it failed.
async fn record(
    pool: &SqlitePool,
    file: &Pending,
    wide: i64,
    error: Option<String>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO media_trickplay (file_id, size, modified_at, made_at, error, width)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (file_id) DO UPDATE SET size = excluded.size,
             modified_at = excluded.modified_at, made_at = excluded.made_at,
             error = excluded.error, width = excluded.width",
    )
    .bind(file.id)
    .bind(file.size)
    .bind(file.modified_at)
    .bind(unix_now())
    .bind(error)
    .bind(wide)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
