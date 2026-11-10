//! A wanted song's progress beyond its downloads: whether Soulseek was searched for it, and its
//! try on YouTube when Soulseek couldn't give it (`song_youtube`).

use super::*;

/// A wanted song's latest try on YouTube (`song_youtube`).
#[derive(sqlx::FromRow)]
pub(super) struct YoutubeRow {
    pub(super) state: String,
    video_title: Option<String>,
    message: Option<String>,
    attempts: i64,
    pub(super) updated_at: i64,
}

impl YoutubeRow {
    /// Searching or downloading, or failed with tries left.
    pub(super) fn under_way(&self) -> bool {
        matches!(self.state.as_str(), "searching" | "downloading")
    }

    /// Its step and word.
    pub(super) fn standing(&self) -> (Step, String) {
        let why = self.message.clone().unwrap_or_default();
        match self.state.as_str() {
            "imported" => (Step::InLibrary, "from YouTube".into()),
            "searching" => (Step::Searching, "looking for it on YouTube".into()),
            "downloading" => (
                Step::Downloading,
                match &self.video_title {
                    Some(video) => format!("downloading from YouTube: {video}"),
                    None => "downloading from YouTube".into(),
                },
            ),
            "none" => (
                Step::Failed,
                format!("not on Soulseek, and not on YouTube either: {why}"),
            ),
            _ if self.attempts < 3 => (
                Step::Failed,
                format!(
                    "not on Soulseek; fetching it from YouTube failed, and is tried again tomorrow: {why}"
                ),
            ),
            _ => (
                Step::Failed,
                format!("not on Soulseek; fetching it from YouTube failed: {why}"),
            ),
        }
    }
}

/// A song as its try on YouTube has it: under way, or in the library from there.
pub(super) fn from_youtube(kind: &'static str, id: i64, title: String, y: YoutubeRow) -> Progress {
    let (step, message) = y.standing();
    Progress {
        kind,
        wanted_id: Some(id),
        title,
        step,
        percent: None,
        message: Some(message),
        download_id: None,
        source: Some("youtube".into()),
        updated_at: Some(y.updated_at),
        copied: None,
    }
}

pub(super) async fn youtube(pool: &SqlitePool, song: i64) -> sqlx::Result<Option<YoutubeRow>> {
    sqlx::query_as(
        "SELECT state, video_title, message, attempts, updated_at FROM song_youtube
         WHERE wanted_song_id = ?",
    )
    .bind(song)
    .fetch_optional(pool)
    .await
}

/// Whether Soulseek has been searched for the wanted song (in the searches kept, 30 days).
pub(super) async fn searched(pool: &SqlitePool, song: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM searches WHERE wanted_song_id = ?)")
        .bind(song)
        .fetch_one(pool)
        .await
}
