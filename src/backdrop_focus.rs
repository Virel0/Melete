//! Where a backdrop's subject is, so apps crop around it rather than by guesswork.
//!
//! A rule, not a model: the subject of a film still is where its detail is, and the rest
//! (sky, a blurred background, a dark room) is smooth. The image is read through ffmpeg in grey
//! at 64 by 36, so no image library is added; each pixel's edge strength (Sobel) above the
//! image's average weighs it, and the focus is the weighted centre. A flat image has its
//! centre as its focus.

use std::path::Path;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;

const WIDTH: usize = 64;
const HEIGHT: usize = 36;

/// The focus of a `width` by `height` grey image, as x and y from 0 to 1 (left, top), to three
/// places.
pub fn focus(gray: &[u8], width: usize, height: usize) -> (f64, f64) {
    assert_eq!(gray.len(), width * height, "a whole image");
    let at = |x: usize, y: usize| f64::from(gray[y * width + x]);
    let mut strengths = Vec::new();
    for y in 1..height.saturating_sub(1) {
        for x in 1..width.saturating_sub(1) {
            let gx = at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x - 1, y)
                - at(x - 1, y + 1);
            let gy = at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x, y - 1)
                - at(x + 1, y - 1);
            strengths.push((x, y, gx.hypot(gy)));
        }
    }
    let mean = strengths.iter().map(|(_, _, s)| s).sum::<f64>() / strengths.len().max(1) as f64;
    let (mut total, mut sx, mut sy) = (0.0, 0.0, 0.0);
    for (x, y, strength) in strengths {
        let weight = (strength - mean).max(0.0);
        total += weight;
        // Pixel centres, so a weight in the last column isn't read as the very edge.
        sx += weight * (x as f64 + 0.5);
        sy += weight * (y as f64 + 0.5);
    }
    if total <= f64::EPSILON {
        return (0.5, 0.5);
    }
    let round = |v: f64| (v.clamp(0.0, 1.0) * 1000.0).round() / 1000.0;
    (
        round(sx / total / width as f64),
        round(sy / total / height as f64),
    )
}

/// The focus of an image file's bytes (JPEG or PNG), through ffmpeg.
pub async fn of_image(ffmpeg: &Path, image: &[u8]) -> anyhow::Result<(f64, f64)> {
    let mut child = tokio::process::Command::new(ffmpeg)
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            "pipe:0",
        ])
        .args([
            "-frames:v",
            "1",
            "-vf",
            &format!("scale={WIDTH}:{HEIGHT}:flags=area"),
        ])
        .args(["-f", "rawvideo", "-pix_fmt", "gray", "pipe:1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdin"))?;
    let bytes = image.to_vec();
    let writer = tokio::spawn(async move {
        // ffmpeg may stop reading once it has a frame; that's no error.
        let _ = stdin.write_all(&bytes).await;
    });
    let out = tokio::time::timeout(std::time::Duration::from_secs(30), child.wait_with_output())
        .await??;
    let _ = writer.await;
    if !out.status.success() {
        anyhow::bail!(
            "ffmpeg couldn't read the backdrop: {}",
            String::from_utf8_lossy(&out.stderr)
                .trim()
                .chars()
                .take(200)
                .collect::<String>()
        );
    }
    anyhow::ensure!(
        out.stdout.len() == WIDTH * HEIGHT,
        "ffmpeg gave {} bytes",
        out.stdout.len()
    );
    Ok(focus(&out.stdout, WIDTH, HEIGHT))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn the_focus_is_where_the_detail_is() {
        let (w, h) = (WIDTH, HEIGHT);
        let mut gray = vec![100u8; w * h];
        // A checkered patch in the top right on a flat ground.
        for y in 4..12 {
            for x in 44..56 {
                gray[y * w + x] = if (x + y) % 2 == 0 { 0 } else { 255 };
            }
        }
        let (x, y) = focus(&gray, w, h);
        assert!((0.68..0.88).contains(&x), "{x}");
        assert!((0.1..0.35).contains(&y), "{y}");
        assert_eq!(focus(&vec![7u8; w * h], w, h), (0.5, 0.5), "flat");
    }

    /// A 1280 by 720 JPEG: a flat grey ground with ffmpeg's busy test pattern at (`x`, `y`).
    pub(crate) fn backdrop(path: &Path, x: u32, y: u32) {
        let status = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
            .arg("color=c=0x808080:s=1280x720")
            .args(["-f", "lavfi", "-i", "testsrc=s=200x200"])
            .args(["-filter_complex", &format!("[0][1]overlay={x}:{y}")])
            .args(["-frames:v", "1"])
            .arg(path)
            .status()
            .expect("ffmpeg runs");
        assert!(status.success());
    }

    #[tokio::test]
    async fn reads_a_backdrop_through_ffmpeg() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.jpg");
        backdrop(&path, 50, 450);
        let bytes = std::fs::read(&path).unwrap();
        let (x, y) = of_image(Path::new("ffmpeg"), &bytes).await.unwrap();
        // The pattern's centre is at (150, 550) of 1280 by 720.
        assert!((0.05..0.2).contains(&x), "{x}");
        assert!((0.65..0.85).contains(&y), "{y}");
        assert!(
            of_image(Path::new("ffmpeg"), b"not an image")
                .await
                .is_err()
        );
    }
}
