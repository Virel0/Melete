//! What a client asks a transcode for, and the checks made before anything runs.

use super::*;

/// What a client asks for. Everything is optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// The tallest picture wanted, in lines (144 to 4320); a smaller source keeps its own.
    pub max_height: Option<i64>,
    /// Video bits per second.
    pub max_bitrate: Option<i64>,
    /// Stream index from the file's audio streams; default is the file's default track.
    pub audio_stream: Option<i64>,
    /// Default 2 (stereo).
    pub audio_channels: Option<i64>,
    /// A subtitle track to burn into the picture, by its id from the subtitle list: `s{index}`
    /// (embedded) or `x{n}` (a file beside the video).
    pub burn_subtitle: Option<String>,
    /// Default h264.
    pub video_codec: Option<VideoCodec>,
    /// What the converted audio sounds like; default normal.
    pub audio_mode: Option<AudioMode>,
}

/// What the video becomes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    /// H.264 with HDR tone-mapped to SDR, in MPEG-TS segments: plays everywhere.
    #[default]
    H264,
    /// HEVC in fragmented MP4 segments, keeping HDR10 and HLG as they are.
    Hevc,
    /// The file's own H.264 or HEVC video, unchanged; only the audio is converted.
    Copy,
}

impl Settings {
    /// Checks the settings before anything runs.
    ///
    /// # Errors
    ///
    /// What's wrong, in words a client can show: a value out of range, a subtitle track id that
    /// isn't one, or `copy` with a setting that would change the picture.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.max_height.is_some_and(|h| !(144..=4320).contains(&h)) {
            return Err("max_height is 144 to 4320");
        }
        if self
            .max_bitrate
            .is_some_and(|b| !(100_000..=200_000_000).contains(&b))
        {
            return Err("max_bitrate is 100000 to 200000000 bits per second");
        }
        if self.audio_channels.is_some_and(|c| !(1..=8).contains(&c)) {
            return Err("audio_channels is 1 to 8");
        }
        // s{stream index}, or x and the 8 hex digits identifying a file beside the video.
        let track_id = |t: &str| match t.split_at_checked(1) {
            Some(("s", n)) => (1..=5).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()),
            Some(("x", id)) => {
                id.len() == 8 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            }
            _ => false,
        };
        if self.burn_subtitle.as_deref().is_some_and(|t| !track_id(t)) {
            return Err("burn_subtitle is a track id from the subtitle list, like s3 or x1a2b3c4d");
        }
        let changes_picture =
            self.max_height.is_some() || self.max_bitrate.is_some() || self.burn_subtitle.is_some();
        if self.video_codec == Some(VideoCodec::Copy) && changes_picture {
            return Err(
                "video_codec copy keeps the video as it is, so max_height, max_bitrate and burn_subtitle can't be used with it",
            );
        }
        Ok(())
    }
}
