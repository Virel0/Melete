//! The downloader's answers to songs' jobs: a search's results chosen from and the audio asked
//! for, or the audio checked and brought into the library; each recorded against its song.

use super::*;

/// Reads the downloader's answers to songs' jobs in `done/`, each recorded against its song;
/// one nothing waits for any more is dropped with its file.
pub(super) async fn read_answers(
    pool: &SqlitePool,
    jobs: &Path,
    fixes: &Fixes,
    now: i64,
) -> anyhow::Result<()> {
    let done = jobs.join("done");
    let Ok(entries) = std::fs::read_dir(&done) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
        else {
            continue;
        };
        let Some(job) = Job::from_name(stem) else {
            continue;
        };
        let answer: Option<Answer> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let file = answer
            .as_ref()
            .and_then(|a| a.file.as_deref())
            .and_then(crate::trailers::plain_name)
            .map(|name| done.join(name));
        match answer {
            Some(answer) => record(pool, jobs, fixes, job, answer, now).await?,
            None => fail(pool, job, "the downloader's answer couldn't be read", now).await?,
        }
        // Whatever the record didn't take isn't left behind.
        if let Some(file) = file {
            let _ = std::fs::remove_file(file);
        }
        let _ = std::fs::remove_file(&path);
    }
    Ok(())
}

/// The song a job is for, when it's waiting for that job's answer.
async fn waiting(pool: &SqlitePool, job: Job) -> anyhow::Result<Option<Wanted>> {
    let (song, state) = match job {
        Job::Search(song) => (song, "searching"),
        Job::Audio(song) => (song, "downloading"),
    };
    Ok(sqlx::query_as(
        "SELECT s.id, s.title, s.artist, s.length_seconds FROM song_youtube y
         JOIN wanted_songs s ON s.id = y.wanted_song_id
         WHERE y.wanted_song_id = ? AND y.state = ?",
    )
    .bind(song)
    .bind(state)
    .fetch_optional(pool)
    .await?)
}

async fn record(
    pool: &SqlitePool,
    jobs: &Path,
    fixes: &Fixes,
    job: Job,
    answer: Answer,
    now: i64,
) -> anyhow::Result<()> {
    let Some(song) = waiting(pool, job).await? else {
        return Ok(());
    };
    if !answer.ok {
        let why = answer
            .error
            .unwrap_or_else(|| "the downloader gave no reason".into());
        return fail(pool, job, &why, now).await;
    }
    if let Some(why) = answer.unavailable {
        // Removed, or blocked here: the next video that fits, if there's one.
        let why = format!("YouTube won't give it here: {why}");
        return next_video(pool, jobs, song.id, &why, now).await;
    }
    match job {
        Job::Search(_) => chosen(pool, jobs, &song, &answer.results, now).await,
        Job::Audio(_) => {
            let Some(name) = answer.file.as_deref().and_then(crate::trailers::plain_name) else {
                return fail(pool, job, "the downloader answered without a file", now).await;
            };
            // One chosen by hand is the song, whatever its length.
            if !picks::by_hand(pool, song.id).await?
                && !choosing::length_fits(song.length_seconds, answer.duration)
            {
                let said = answer
                    .duration
                    .map_or("unknown".into(), |d| format!("{d:.0} s"));
                let why = format!("the audio YouTube gave is {said} long, not the song's length");
                return next_video(pool, jobs, song.id, &why, now).await;
            }
            let file = jobs.join("done").join(name);
            match import(pool, fixes, &song, &file).await {
                Ok((placed, note)) => imported(pool, song.id, &placed, &note, now).await,
                Err(why) => fail(pool, job, &why, now).await,
            }
        }
    }
}

/// The video chosen from a search's results, and its audio asked for; none when none fits.
async fn chosen(
    pool: &SqlitePool,
    jobs: &Path,
    song: &Wanted,
    results: &[Found],
    now: i64,
) -> anyhow::Result<()> {
    // Turned off meanwhile: nothing more goes to YouTube, and it's as if never tried.
    if !crate::settings::song_youtube(pool).await? {
        sqlx::query("DELETE FROM song_youtube WHERE wanted_song_id = ?")
            .bind(song.id)
            .execute(pool)
            .await?;
        return Ok(());
    }
    let fitting = choosing::ranked(&song.song(), results);
    let Some((video, others)) = fitting.split_first() else {
        let why = format!(
            "none of YouTube's {} results has its title, artist and length",
            results.len()
        );
        return none(pool, song.id, &why, now).await;
    };
    download(pool, jobs, song.id, video, others, "searching", now).await
}

/// Asks for `video`'s audio, keeping `others` (the fitting results not tried yet, best first)
/// for when it can't be used; only while the song's try is in `state`.
async fn download(
    pool: &SqlitePool,
    jobs: &Path,
    song: i64,
    video: &Found,
    others: &[&Found],
    state: &str,
    now: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE song_youtube SET state = 'downloading', video = ?, video_title = ?, channel = ?,
             duration_seconds = ?, others = ?, message = NULL, updated_at = ?
         WHERE wanted_song_id = ? AND state = ?",
    )
    .bind(&video.id)
    .bind(&video.title)
    .bind(&video.channel)
    .bind(video.duration)
    .bind(serde_json::to_string(others)?)
    .bind(now)
    .bind(song)
    .bind(state)
    .execute(pool)
    .await?;
    let job = Job::Audio(song);
    write_job(
        jobs,
        job,
        &json!({ "id": job.name(), "kind": "song", "video": video.id }),
    )
}

/// The chosen video can't be used (`why`): the next that fits is asked for, or, when none is
/// left, no video is had.
async fn next_video(
    pool: &SqlitePool,
    jobs: &Path,
    song: i64,
    why: &str,
    now: i64,
) -> anyhow::Result<()> {
    let others: String =
        sqlx::query_scalar("SELECT others FROM song_youtube WHERE wanted_song_id = ?")
            .bind(song)
            .fetch_one(pool)
            .await?;
    let others: Vec<Found> = serde_json::from_str(&others).unwrap_or_default();
    let Some((video, rest)) = others.split_first() else {
        let why = format!("no other video that fits is left; the last: {why}");
        return none(pool, song, &why, now).await;
    };
    let rest: Vec<&Found> = rest.iter().collect();
    tracing::info!(song, why, "trying the next YouTube video that fits");
    download(pool, jobs, song, video, &rest, "downloading", now).await
}

async fn none(pool: &SqlitePool, song: i64, why: &str, now: i64) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE song_youtube SET state = 'none', message = ?, updated_at = ? WHERE wanted_song_id = ?",
    )
    .bind(why)
    .bind(now)
    .bind(song)
    .execute(pool)
    .await?;
    Ok(())
}

async fn fail(pool: &SqlitePool, job: Job, why: &str, now: i64) -> anyhow::Result<()> {
    let song = match job {
        Job::Search(song) | Job::Audio(song) => song,
    };
    tracing::warn!(song, why, "fetching a song from YouTube failed");
    sqlx::query(
        "UPDATE song_youtube SET state = 'failed', attempts = attempts + 1, message = ?,
             updated_at = ?
         WHERE wanted_song_id = ? AND state IN ('searching', 'downloading')",
    )
    .bind(why)
    .bind(now)
    .bind(song)
    .execute(pool)
    .await?;
    Ok(())
}

async fn imported(
    pool: &SqlitePool,
    song: i64,
    placed: &str,
    note: &str,
    now: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE song_youtube SET state = 'imported', placed = ?, message = ?, updated_at = ?
         WHERE wanted_song_id = ?",
    )
    .bind(placed)
    .bind(note)
    .bind(now)
    .bind(song)
    .execute(pool)
    .await?;
    tracing::info!(song, placed, "a song came from YouTube");
    Ok(())
}
