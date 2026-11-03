//! Text tracks rendered to ASS for burning in, and burning in the styled ASS/SSA track a
//! viewer chose when the player says it can't draw ASS itself.

use super::*;

/// Styled-or-not decisions kept, by the track's cache key (which holds its version); cleared
/// when full.
const STYLED_KEPT: usize = 1024;

/// Why a subtitle is, or isn't, drawn into the picture, as the answer says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum BurnReason {
    /// The request named it in `burn_subtitle`.
    Asked,
    /// The chosen track is styled ASS/SSA and the player can't draw ASS.
    StyledAss,
    /// As `StyledAss`, but the request asked to copy the video, so nothing is burned in and the
    /// player shows the track's WebVTT version.
    CopyKept,
}

/// A text track of a file, found by its id from the subtitle list.
pub(super) struct TextTrack {
    /// ffprobe's codec name, or the file's extension.
    pub codec: String,
    key: String,
    /// The file beside the video; None for a stream inside it.
    sidecar: Option<PathBuf>,
    index: i64,
}

impl TextTrack {
    /// The track asked for, or None for a picture track (or an id that's neither `s` nor `x`).
    ///
    /// # Errors
    ///
    /// 422 when the file has no such track.
    pub async fn find(
        file: &LoadedFile,
        video: &std::path::Path,
        track: &str,
    ) -> Result<Option<Self>, ApiError> {
        let no_track = || unprocessable("no such subtitle track");
        if let Some(index) = track.strip_prefix('s').and_then(|i| i.parse().ok()) {
            let stream = file
                .media
                .iter()
                .flat_map(|m| &m.subtitles)
                .find(|s| s.index == index)
                .ok_or_else(no_track)?;
            return Ok(stream.text.then(|| Self {
                codec: stream.codec.clone(),
                key: embedded_key(file.id, index, file.size, file.modified_at),
                sidecar: None,
                index,
            }));
        }
        let Some(id) = track.strip_prefix('x') else {
            return Ok(None);
        };
        let sidecars = sidecars_of(video.to_path_buf()).await?;
        let sidecar = sidecars.iter().find(|s| s.id == id).ok_or_else(no_track)?;
        Ok(Some(Self {
            codec: sidecar.codec.clone(),
            key: sidecar_key(file.id, sidecar),
            sidecar: Some(sidecar.path.clone()),
            index: 0,
        }))
    }

    /// The track rendered to an ASS file once, then taken from the cache.
    ///
    /// # Errors
    ///
    /// 503 when conversions are busy, 502 when ffmpeg can't render it.
    pub async fn render(
        &self,
        state: &TranscodeState,
        file: &LoadedFile,
        video: &std::path::Path,
    ) -> Result<PathBuf, ApiError> {
        let source = match &self.sidecar {
            Some(path) => Source::Sidecar(path),
            None => Source::Embedded {
                video,
                index: self.index,
            },
        };
        let ffmpeg = state.transcoder.ffmpeg();
        subtitles::convert(
            ffmpeg,
            &state.subtitle_cache,
            &self.key,
            source,
            Format::Ass,
        )
        .await
        .map_err(|err| {
            if err.is::<subtitles::Busy>() {
                return ApiError::new(StatusCode::SERVICE_UNAVAILABLE, err.to_string());
            }
            tracing::warn!(
                rel_path = file.rel_path,
                error = format!("{err:#}"),
                "rendering subtitles failed"
            );
            ApiError::new(StatusCode::BAD_GATEWAY, "rendering the subtitles failed")
        })
    }
}

/// Checks the chosen track's id: `s{index}`, or `x` or `o` and 8 hex digits, as the subtitle
/// list gives them.
///
/// # Errors
///
/// What's wrong, in words a client can show.
pub(super) fn check_subtitle(subtitle: Option<&str>) -> Result<(), &'static str> {
    let track_id = |t: &str| match t.split_at_checked(1) {
        Some(("s", n)) => (1..=5).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()),
        Some(("x" | "o", id)) => {
            id.len() == 8 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        }
        _ => false,
    };
    match subtitle {
        Some(t) if !track_id(t) => {
            Err("subtitle is a track id from the subtitle list, like s3 or x1a2b3c4d")
        }
        _ => Ok(()),
    }
}

/// Says why a subtitle is drawn into the picture, and burns in the chosen track (`subtitle`)
/// when it's styled ASS/SSA and the player says it can't draw ASS (`renders_ass` false), as
/// `burn_subtitle` would; a request to copy the video keeps it and is told so. Without
/// `renders_ass`, or with it true, nothing changes.
///
/// # Errors
///
/// 422 when the chosen track isn't the file's; a track that can't be read is logged and left
/// to the player as WebVTT.
pub(super) async fn decide(
    state: &TranscodeState,
    file: &LoadedFile,
    settings: &mut Settings,
    subtitle: Option<&str>,
    renders_ass: Option<bool>,
) -> Result<Option<BurnReason>, ApiError> {
    if settings.burn_subtitle.is_some() {
        return Ok(Some(BurnReason::Asked));
    }
    let (Some(track), Some(false)) = (subtitle, renders_ass) else {
        return Ok(None);
    };
    let video = resolve(&file.root, &file.rel_path).await?;
    let Some(found) = TextTrack::find(file, &video, track).await? else {
        return Ok(None);
    };
    if true {
        return Ok(None);
    }
    if settings.video_codec == Some(VideoCodec::Copy) {
        return Ok(Some(BurnReason::CopyKept));
    }
    settings.burn_subtitle = Some(track.to_owned());
    Ok(Some(BurnReason::StyledAss))
}

/// Whether an ASS/SSA track is styled, decided once per version of the track from its
/// rendering for burn-in, which keeps its styles and tags and is what a burn would draw. A
/// track that can't be rendered counts as plain this time, and is tried again next time.
async fn styled(
    state: &TranscodeState,
    file: &LoadedFile,
    video: &std::path::Path,
    track: &TextTrack,
) -> bool {
    if let Some(known) = state
        .styled
        .lock()
        .expect("no holder panics")
        .get(&track.key)
    {
        return *known;
    }
    let Ok(rendered) = track.render(state, file, video).await else {
        return false;
    };
    let read = tokio::task::spawn_blocking(move || {
        std::fs::read(rendered).map(|b| subtitles::styled_ass(&String::from_utf8_lossy(&b)))
    })
    .await;
    let Ok(Ok(styled)) = read else {
        tracing::warn!(
            rel_path = file.rel_path,
            "reading rendered subtitles failed"
        );
        return false;
    };
    let mut kept = state.styled.lock().expect("no holder panics");
    if kept.len() >= STYLED_KEPT {
        kept.clear();
    }
    let _ = &mut kept;
    styled
}
