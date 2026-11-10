//! A YouTube video an admin chose for a wanted song by hand: for a song Melete couldn't match,
//! or matched wrongly. The worker takes it at its next round, when nothing for the song is under
//! way and nothing else is with the downloader, and fetches it as the song without matching it
//! or checking its length: the admin said it's the song. Only while songs from YouTube are on.

use super::*;

/// The id of a YouTube video, from the id itself or a link to it: youtube.com's `watch?v=`,
/// `youtu.be/`, `/shorts/`, `/embed/` and `/live/`, music.youtube.com's too. None for anything
/// else.
pub fn video_id(text: &str) -> Option<String> {
    let text = text.trim();
    let is_id = |s: &str| {
        s.len() == 11
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    if is_id(text) {
        return Some(text.to_owned());
    }
    let url = reqwest::Url::parse(text).ok()?;
    if !matches!(url.scheme(), "https" | "http") {
        return None;
    }
    let host = url
        .host_str()?
        .trim_start_matches("www.")
        .trim_start_matches("m.");
    let id = match host {
        "youtu.be" => url.path_segments()?.next().map(str::to_owned),
        "youtube.com" | "music.youtube.com" => {
            let mut path = url.path_segments()?;
            match path.next() {
                Some("watch") => url
                    .query_pairs()
                    .find(|(k, _)| k == "v")
                    .map(|(_, v)| v.into_owned()),
                Some("shorts" | "embed" | "live") => path.next().map(str::to_owned),
                _ => None,
            }
        }
        _ => None,
    }?;
    is_id(&id).then_some(id)
}

/// Why a video couldn't be chosen for a song.
#[derive(Debug, PartialEq, Eq)]
pub enum NotPicked {
    /// There's no such wanted song.
    NoSong,
    /// Songs from YouTube are off.
    Off,
    /// What was given isn't a YouTube video's id or link.
    NotAVideo,
}

/// Chooses the video `text` names for wanted song `song`, in place of any chosen before; the
/// video's id. Fetched at the worker's next round.
pub async fn pick(
    pool: &SqlitePool,
    song: i64,
    text: &str,
    now: i64,
) -> anyhow::Result<Result<String, NotPicked>> {
    let Some(video) = video_id(text) else {
        return Ok(Err(NotPicked::NotAVideo));
    };
    let wanted: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM wanted_songs WHERE id = ?)")
            .bind(song)
            .fetch_one(pool)
            .await?;
    if !wanted {
        return Ok(Err(NotPicked::NoSong));
    }
    if !crate::settings::song_youtube(pool).await? {
        return Ok(Err(NotPicked::Off));
    }
    sqlx::query(
        "INSERT INTO song_youtube_picks (wanted_song_id, video, asked_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (wanted_song_id) DO UPDATE SET video = excluded.video,
             asked_at = excluded.asked_at",
    )
    .bind(song)
    .bind(&video)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(Ok(video))
}

/// Takes the oldest video chosen by hand for a song with nothing under way: its download asked
/// for, as a try of the song's chosen by hand. Whether one was taken.
pub(super) async fn take(pool: &SqlitePool, jobs: &Path, now: i64) -> anyhow::Result<bool> {
    let picked: Option<(i64, String)> = sqlx::query_as(
        "SELECT p.wanted_song_id, p.video FROM song_youtube_picks p
         WHERE NOT EXISTS (SELECT 1 FROM song_youtube y WHERE y.wanted_song_id = p.wanted_song_id
                           AND y.state IN ('searching', 'downloading'))
         ORDER BY p.asked_at, p.wanted_song_id LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    let Some((song, video)) = picked else {
        return Ok(false);
    };
    sqlx::query(
        "INSERT INTO song_youtube (wanted_song_id, state, video, started_at, updated_at, by_hand,
             message)
         VALUES (?1, 'downloading', ?2, ?3, ?3, 1, 'chosen by hand')
         ON CONFLICT (wanted_song_id) DO UPDATE SET state = 'downloading', video = ?2,
             video_title = NULL, channel = NULL, duration_seconds = NULL, placed = NULL,
             others = '[]', attempts = 0, by_hand = 1, message = 'chosen by hand',
             started_at = ?3, updated_at = ?3",
    )
    .bind(song)
    .bind(&video)
    .bind(now)
    .execute(pool)
    .await?;
    sqlx::query("DELETE FROM song_youtube_picks WHERE wanted_song_id = ?")
        .bind(song)
        .execute(pool)
        .await?;
    let job = Job::Audio(song);
    write_job(
        jobs,
        job,
        &json!({ "id": job.name(), "kind": "song", "video": video }),
    )?;
    Ok(true)
}

/// Whether the song's try is one chosen by hand.
pub(super) async fn by_hand(pool: &SqlitePool, song: i64) -> sqlx::Result<bool> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT by_hand FROM song_youtube WHERE wanted_song_id = ?")
            .bind(song)
            .fetch_optional(pool)
            .await?
            == Some(1),
    )
}
