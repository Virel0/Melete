use super::*;

const H264: VideoCodec = VideoCodec::H264;
const HEVC: VideoCodec = VideoCodec::Hevc;

#[test]
fn up_to_4_mbits_is_720p() {
    assert_eq!(rung(H264, 2_000_000), Some(720));
    assert_eq!(rung(H264, 4_000_000), Some(720));
    assert_eq!(rung(HEVC, 4_000_000), Some(720));
}

#[test]
fn up_to_10_mbits_is_1080p() {
    assert_eq!(rung(H264, 4_000_001), Some(1080));
    assert_eq!(rung(H264, 8_000_000 - 384_000), Some(1080), "an 8M way");
    assert_eq!(rung(H264, 10_000_000), Some(1080));
    assert_eq!(rung(HEVC, 10_000_000), Some(1080));
}

#[test]
fn up_to_20_mbits_is_1440p() {
    assert_eq!(rung(H264, 10_000_001), Some(1440));
    assert_eq!(rung(H264, 20_000_000), Some(1440));
    assert_eq!(rung(HEVC, 20_000_000), Some(1440));
}

#[test]
fn above_20_mbits_h264_stays_at_1440p_and_hevc_keeps_the_source() {
    assert_eq!(rung(H264, 20_000_001), Some(1440));
    assert_eq!(rung(H264, 200_000_000), Some(1440));
    assert_eq!(rung(HEVC, 20_000_001), None);
    assert_eq!(
        height_for_bitrate(HEVC, 40_000_000, 3840, 2160),
        None,
        "4K HEVC"
    );
    assert_eq!(height_for_bitrate(H264, 40_000_000, 3840, 2160), Some(1440));
}

#[test]
fn a_4k_source_held_to_an_8m_way_is_1080p() {
    assert_eq!(
        height_for_bitrate(H264, 8_000_000 - 384_000, 3840, 2160),
        Some(1080)
    );
}

#[test]
fn never_upscales() {
    assert_eq!(height_for_bitrate(H264, 7_616_000, 1920, 1080), None);
    assert_eq!(height_for_bitrate(H264, 2_000_000, 1280, 720), None);
    assert_eq!(height_for_bitrate(H264, 2_000_000, 640, 480), None);
    assert_eq!(height_for_bitrate(H264, 2_000_000, 1920, 1080), Some(720));
}

#[test]
fn wide_and_narrow_pictures_fit_the_rungs_box() {
    // A scope film: 1920 wide at the 1080 rung, shorter than 1080.
    assert_eq!(height_for_bitrate(H264, 7_616_000, 3840, 1600), Some(800));
    // 4:3 stays the rung's height, narrower than its box.
    assert_eq!(height_for_bitrate(H264, 3_000_000, 1440, 1080), Some(720));
}

#[test]
fn copied_video_has_no_rung() {
    assert_eq!(
        height_for_bitrate(VideoCodec::Copy, 1_000_000, 3840, 2160),
        None
    );
}
