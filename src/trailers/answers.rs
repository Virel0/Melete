//! The downloader's answers: each recorded against its title, its video moved under the config
//! folder once it's checked to be an MP4 of a sensible size.

use super::*;

/// The downloader's answer to one job.
#[derive(Debug, Deserialize)]
struct Answer {
    id: String,
    ok: bool,
    /// The video's file name in `done/`, when it got one.
    file: Option<String>,
    height: Option<i64>,
    duration: Option<f64>,
    /// Why YouTube hasn't it (removed, private, blocked), when it hasn't.
    unavailable: Option<String>,
    error: Option<String>,
}

/// A plain file name from the downloader: nothing that leaves the folder it's in.
pub(super) fn plain(name: &str) -> Option<&str> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && !name.starts_with('.');
    ok.then_some(name)
}

/// Moves a finished video into the videos folder as `<job>.mp4`, checking it looks like an MP4
/// of a sensible size first. Returns its name there.
fn take_video(done: &Path, name: &str, videos: &Path, job: &str) -> anyhow::Result<String> {
    let name =
        plain(name).ok_or_else(|| anyhow::anyhow!("the downloader named no usable video"))?;
    let from = done.join(name);
    let size = std::fs::metadata(&from)?.len();
    anyhow::ensure!(
        (12..=MAX_VIDEO).contains(&size),
        "the video is {size} bytes"
    );
    let mut head = [0u8; 12];
    std::io::Read::read_exact(&mut std::fs::File::open(&from)?, &mut head)?;
    anyhow::ensure!(&head[4..8] == b"ftyp", "the video isn't an MP4");
    std::fs::create_dir_all(videos)?;
    let file = format!("{job}.mp4");
    let partial = videos.join(format!(".{file}.partial"));
    // The jobs folder may be on another volume, so copy rather than rename across.
    std::fs::copy(&from, &partial)?;
    std::fs::rename(&partial, videos.join(&file))?;
    let _ = std::fs::remove_file(&from);
    Ok(file)
}

pub(super) async fn read_answers(pool: &SqlitePool, folders: &Folders) -> anyhow::Result<()> {
    let done = folders.jobs.join("done");
    let Ok(entries) = std::fs::read_dir(&done) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let answer: Option<Answer> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        if let Some(answer) = answer
            && let Some(title) = Title::from_job(&answer.id)
        {
            record(pool, folders, title, answer).await?;
        }
        // Read, or unreadable: either way it's been dealt with.
        let _ = std::fs::remove_file(&path);
    }
    Ok(())
}

async fn record(
    pool: &SqlitePool,
    folders: &Folders,
    title: Title,
    answer: Answer,
) -> anyhow::Result<()> {
    let done = folders.jobs.join("done");
    let (movie_id, series_id) = title.ids();
    let queued: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM downloaded_trailers
             WHERE movie_id IS ?1 AND series_id IS ?2 AND state = 'queued')",
    )
    .bind(movie_id)
    .bind(series_id)
    .fetch_one(pool)
    .await?;
    let leave = |name: Option<&str>| {
        if let Some(name) = name.and_then(plain) {
            let _ = std::fs::remove_file(done.join(name));
        }
    };
    if !queued {
        // Nothing asked for it (a job from before a restart that was already answered).
        leave(answer.file.as_deref());
        return Ok(());
    }
    let now = unix_now();
    let job = title.job();
    let file = answer.file.clone();
    let outcome = match (answer.ok, answer.unavailable, answer.file) {
        (true, Some(why), _) => Ok((None, Some(why))),
        (true, None, Some(name)) => take_video(&done, &name, &folders.videos, &job)
            .map(|file| (Some(file), None))
            .map_err(|err| format!("{err:#}")),
        (true, None, None) => {
            Err("the downloader answered with neither a video nor a reason".to_owned())
        }
        (false, ..) => Err(answer
            .error
            .unwrap_or_else(|| "the downloader gave no reason".into())),
    };
    // Whatever wasn't taken isn't left behind. A trailer downloaded before stays until a new
    // one replaces it.
    leave(file.as_deref());
    match outcome {
        Ok((Some(file), _)) => {
            sqlx::query(
                "UPDATE downloaded_trailers SET state = 'done', file = ?, height = ?,
                     duration_seconds = ?, reason = NULL, attempts = 0, finished_at = ?
                 WHERE movie_id IS ? AND series_id IS ? AND state = 'queued'",
            )
            .bind(&file)
            .bind(answer.height)
            .bind(answer.duration)
            .bind(now)
            .bind(movie_id)
            .bind(series_id)
            .execute(pool)
            .await?;
        }
        Ok((None, unavailable)) => {
            sqlx::query(
                "UPDATE downloaded_trailers SET state = 'unavailable', reason = ?, attempts = 0,
                     finished_at = ?
                 WHERE movie_id IS ? AND series_id IS ? AND state = 'queued'",
            )
            .bind(
                unavailable
                    .unwrap_or_default()
                    .chars()
                    .take(300)
                    .collect::<String>(),
            )
            .bind(now)
            .bind(movie_id)
            .bind(series_id)
            .execute(pool)
            .await?;
        }
        Err(why) => {
            tracing::warn!(job, error = %why, "a title's trailer couldn't be downloaded");
            sqlx::query(
                "UPDATE downloaded_trailers SET state = 'failed', reason = ?,
                     attempts = attempts + 1, finished_at = ?
                 WHERE movie_id IS ? AND series_id IS ? AND state = 'queued'",
            )
            .bind(why.chars().take(500).collect::<String>())
            .bind(now)
            .bind(movie_id)
            .bind(series_id)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}
