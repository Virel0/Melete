//! The picture size a conversion held to a bitrate gets: fewer bits fit a smaller picture, and
//! encoding (and tone mapping) fewer pixels is quicker too.

use super::*;

/// The rungs: the most video bits a second each picture height is chosen for. Above the last,
/// H.264 stays at [`H264_TALLEST`] and HEVC keeps the source's.
const RUNGS: [(i64, i64); 3] = [(4_000_000, 720), (10_000_000, 1080), (20_000_000, 1440)];

/// The tallest picture H.264 is made at when held to a bitrate: 4K H.264 at a bitrate a
/// connection limits takes more than its pixels are worth, and many players can't decode it.
const H264_TALLEST: i64 = 1440;

/// The rung's height for a conversion to `codec` held to `bitrate` video bits a second; None
/// for the source's own.
pub fn rung(codec: VideoCodec, bitrate: i64) -> Option<i64> {
    RUNGS
        .iter()
        .find(|(most, _)| bitrate <= *most)
        .map(|(_, height)| *height)
        .or(match codec {
            VideoCodec::Hevc | VideoCodec::Copy => None,
            VideoCodec::H264 => Some(H264_TALLEST),
        })
}

/// The output height for a source `width` × `height` held to `bitrate`: the rung's, fitted in
/// its 16:9 box, so a scope film at the 1080 rung is 1920 wide and shorter than 1080. None
/// when the source fits as it is (it's never made bigger) or the codec copies the video.
pub fn height_for_bitrate(codec: VideoCodec, bitrate: i64, width: i64, height: i64) -> Option<i64> {
    if codec == VideoCodec::Copy || width <= 0 || height <= 0 {
        return None;
    }
    let rung = rung(codec, bitrate)?;
    let box_width = rung * 16 / 9;
    let fitted = (box_width as f64 * height as f64 / width as f64 / 2.0).floor() as i64 * 2;
    Some(rung.min(fitted)).filter(|h| *h < height)
}

#[cfg(test)]
mod tests;
