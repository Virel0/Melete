//! How one file is encoded: the plan worked out from its probe and the settings, and subtitles
//! burned in.

use super::*;

/// How one file is encoded, worked out from its probe and the settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The file to read.
    pub input: PathBuf,
    /// Its length in seconds, from the probe; always more than 0.
    pub duration: f64,
    /// The video stream's index in the file.
    pub video: i64,
    /// Decode on the GPU too, keeping frames there.
    pub gpu_decode: bool,
    /// Scale to this height; `None` keeps the source's.
    pub height: Option<i64>,
    /// HDR source, tone-mapped to SDR: always for H.264; for HEVC only when burning subtitles in
    /// or for Dolby Vision without an HDR10 or HLG picture to keep (profile 5).
    pub tonemap: bool,
    /// 10- or 12-bit source, converted to 8-bit unless `ten_bit`.
    pub high_bit_depth: bool,
    /// What the video becomes.
    pub codec: VideoCodec,
    /// HEVC output in 10 bits (Main 10): HDR kept, or a 10-bit source.
    pub ten_bit: bool,
    /// Fragmented MP4 segments with an init section (HEVC), rather than MPEG-TS.
    pub fmp4: bool,
    /// Dolby Vision with no picture of its own for players without it (profile 5): copied
    /// video is tagged dvh1, which such players refuse rather than show with wrong colours.
    pub dolby_vision_only: bool,
    /// For copied video, where segments start; set by the caller from [`keyframes`].
    pub keyframes: Option<Arc<Keyframes>>,
    /// The video bit rate to encode at, in bits per second.
    pub bitrate: i64,
    /// The audio stream's index in the file; None when it has no audio.
    pub audio: Option<i64>,
    /// Audio channels in the output: what was asked (2 by default), but no more than the
    /// source has.
    pub channels: i64,
    /// A subtitle track to draw into the picture; None for none.
    pub burn: Option<Burn>,
    /// ffmpeg's audio filters: the downmix to stereo and the audio mode's; None for none.
    pub audio_filter: Option<String>,
}

/// A subtitle track drawn into the picture.
#[derive(Debug, Clone, PartialEq)]
pub enum Burn {
    /// A picture track (PGS, VobSub) from the same file, scaled to the output size.
    Picture { index: i64, width: i64, height: i64 },
    /// A text track rendered to an ASS file first, with the fonts attached to the video when
    /// it has some; see [`Burn::text`].
    Text {
        ass: PathBuf,
        fonts: Option<PathBuf>,
    },
}

/// Whether a path is safe in a filter expression: plain characters only.
pub(super) fn plain_path(path: &std::path::Path) -> bool {
    path.to_str().is_some_and(|p| {
        p.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'))
    })
}

impl Burn {
    /// The path goes into a filter expression, so only plain characters are allowed. It is
    /// always a subtitle cache file named from numbers, never a library file name.
    pub fn text(path: PathBuf) -> Result<Self, &'static str> {
        if !plain_path(&path) {
            return Err("the subtitle cache path has characters ffmpeg's filters can't take");
        }
        Ok(Self::Text {
            ass: path,
            fonts: None,
        })
    }

    /// Renders with the fonts in `dir` (attached to the video). Also a cache path, so the
    /// same characters are allowed.
    pub fn with_fonts(self, dir: PathBuf) -> Result<Self, &'static str> {
        if !plain_path(&dir) {
            return Err("the fonts path has characters ffmpeg's filters can't take");
        }
        match self {
            Self::Text { ass, .. } => Ok(Self::Text {
                ass,
                fonts: Some(dir),
            }),
            picture => Ok(picture),
        }
    }
}

/// Works out how to encode a file from its probe and a client's settings, which
/// [`Settings::validate`] has already checked.
///
/// # Errors
///
/// Why the file can't be played this way, in words a client can show: no known duration, no
/// video stream, video that can't be copied, or an audio or subtitle track that isn't there.
pub fn plan(
    input: PathBuf,
    info: &MediaInfo,
    settings: &Settings,
    hw: Hardware,
) -> Result<Plan, &'static str> {
    let duration = info
        .duration_seconds
        .filter(|d| d.is_finite() && *d > 0.0)
        .ok_or("the file's duration is unknown, so it can't be cut into segments")?;
    let video = info.video.first().ok_or("the file has no video stream")?;
    let codec = settings.video_codec.unwrap_or_default();
    if codec == VideoCodec::Copy {
        check_copyable(info, video)?;
    }
    let source_height = video.height.unwrap_or(1080);
    let height = settings.max_height.filter(|max| *max < source_height);
    let out_height = height.unwrap_or(source_height);
    let default_bitrate = default_bitrate(out_height);
    let bitrate = settings
        .max_bitrate
        .map_or(default_bitrate, |max| max.min(default_bitrate));
    let high_bit_depth = video
        .pixel_format
        .as_deref()
        .is_some_and(|p| p.contains("10") || p.contains("12"));
    // Burning subtitles happens on the CPU, so frames are decoded there.
    let gpu_decode = hw == Hardware::Nvenc
        && codec != VideoCodec::Copy
        && NVDEC_CODECS.contains(&video.codec.as_str())
        && !(video.codec == "h264" && high_bit_depth)
        && settings.burn_subtitle.is_none();
    let source_width = video.width.unwrap_or(source_height * 16 / 9);
    let out_width =
        ((source_width * out_height) as f64 / source_height as f64 / 2.0).round() as i64 * 2;
    let burn = picture_burn(info, settings, out_width, out_height)?;

    let audio = chosen_audio(info, settings)?;
    let channels = settings
        .audio_channels
        .unwrap_or(2)
        .min(audio.and_then(|a| a.channels).unwrap_or(2));
    // What players without Dolby Vision see: a profile 7 or 8.1 file is HDR10 to them.
    let picture = video.picture();
    let keep_hdr = codec == VideoCodec::Hevc
        && settings.burn_subtitle.is_none()
        && matches!(picture, Picture::Hdr10 | Picture::Hlg);
    // An SDR base layer (Dolby Vision 8.2) needs none.
    let tonemap = codec != VideoCodec::Copy && picture != Picture::Sdr && !keep_hdr;
    Ok(Plan {
        input,
        duration,
        video: video.index,
        gpu_decode,
        height,
        tonemap,
        high_bit_depth,
        codec,
        // Burned-in subtitles are drawn on 8-bit frames.
        ten_bit: codec == VideoCodec::Hevc
            && high_bit_depth
            && !tonemap
            && settings.burn_subtitle.is_none(),
        fmp4: codec == VideoCodec::Hevc || (codec == VideoCodec::Copy && video.codec == "hevc"),
        dolby_vision_only: picture == Picture::DolbyVisionOnly,
        keyframes: None,
        bitrate,
        audio: audio.map(|a| a.index),
        channels,
        burn,
        audio_filter: audio.and_then(|a| {
            let source = a.channels.unwrap_or(2);
            audio_filter(settings.audio_mode.unwrap_or_default(), source, channels)
        }),
    })
}

/// Refuses copying video that isn't H.264 or HEVC, or isn't in a file with a keyframe index.
fn check_copyable(info: &MediaInfo, video: &crate::probe::VideoStream) -> Result<(), &'static str> {
    if !matches!(video.codec.as_str(), "h264" | "hevc") {
        return Err("only H.264 and HEVC video can be copied; ask for h264 or hevc");
    }
    let indexed = info.container.as_deref().is_some_and(|c| {
        c.split(',')
            .any(|f| matches!(f, "matroska" | "mov" | "mp4"))
    });
    if !indexed {
        return Err(
            "copying the video needs an MKV or MP4 file, whose index says where the keyframes are; ask for h264 or hevc",
        );
    }
    Ok(())
}

/// The bitrate for a picture this many lines high, before any limit the settings set.
fn default_bitrate(out_height: i64) -> i64 {
    match out_height {
        h if h >= 2160 => 25_000_000,
        h if h >= 1440 => 16_000_000,
        h if h >= 1080 => 10_000_000,
        h if h >= 720 => 5_000_000,
        _ => 2_500_000,
    }
}

/// The picture subtitle track to burn in, scaled to the output size, when one is asked for.
fn picture_burn(
    info: &MediaInfo,
    settings: &Settings,
    out_width: i64,
    out_height: i64,
) -> Result<Option<Burn>, &'static str> {
    // Picture tracks are planned here; text tracks are rendered to a file by the caller, which
    // then sets `burn` to `Burn::Text`.
    Ok(
        match settings
            .burn_subtitle
            .as_deref()
            .and_then(|t| t.strip_prefix('s'))
        {
            Some(index) => {
                let index: i64 = index.parse().map_err(|_| "no such subtitle track")?;
                let track = info
                    .subtitles
                    .iter()
                    .find(|s| s.index == index)
                    .ok_or("no such subtitle track")?;
                (!track.text).then_some(Burn::Picture {
                    index,
                    width: out_width,
                    height: out_height,
                })
            }
            None => None,
        },
    )
}

/// The audio stream asked for, else the default one, else the first.
fn chosen_audio<'a>(
    info: &'a MediaInfo,
    settings: &Settings,
) -> Result<Option<&'a crate::probe::AudioStream>, &'static str> {
    Ok(match settings.audio_stream {
        Some(index) => Some(
            info.audio
                .iter()
                .find(|a| a.index == index)
                .ok_or("no such audio stream")?,
        ),
        None => info.audio.iter().find(|a| a.default).or(info.audio.first()),
    })
}

impl Plan {
    /// Where each segment starts, in seconds from the start of the file.
    pub fn segment_starts(&self) -> Vec<f64> {
        match &self.keyframes {
            Some(k) => k.starts.clone(),
            None => (0..segment_count(self.duration))
                .map(|n| n as f64 * SEGMENT_SECONDS)
                .collect(),
        }
    }

    /// How many segments the playlist lists: one per keyframe start for copied video, else
    /// fixed segments.
    pub fn segment_count(&self) -> i64 {
        match &self.keyframes {
            Some(k) => k.starts.len() as i64,
            None => segment_count(self.duration),
        }
    }

    /// The segments' file extension, in the playlist and in the transcode folder.
    pub fn extension(&self) -> &'static str {
        if self.fmp4 { "m4s" } else { "ts" }
    }

    /// The whole file as a VOD playlist, segments named `{n}.ts` or `{n}.m4s`.
    pub fn playlist(&self) -> String {
        playlist_of(
            &self.segment_starts(),
            self.duration,
            self.extension(),
            self.fmp4,
        )
    }
}
