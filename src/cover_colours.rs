//! Colours from album covers, worked out once on the server so every app's
//! backgrounds and players match the artwork without each device doing it: a background, and
//! four text colours to set on it, as Apple Music's artwork colours are given. The background is
//! the colour the cover's edges are most of (what surrounds the picture when it's shown on it);
//! the first two text colours are the cover's most common colours that read on that background
//! (4.5:1 and 3:1, WCAG's contrast for text and for large text), else white or black; the other
//! two are those softened toward the background. Covers are read with ffmpeg, which Melete
//! already runs, scaled to 48 by 48. Only covers already on the server are read: a folder's, a
//! track's tags', or a Cover Art Archive one fetched already. Nothing is fetched for this.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt;

/// The size a cover is read at: enough for its colours, small enough to be quick.
const SIDE: usize = 48;
/// How deep the edge counts as edge, in those pixels.
const EDGE: usize = 6;

/// An album cover's colours, each as `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Colours {
    /// The colour the cover's edges are most of, for behind it.
    pub background: String,
    /// The cover's most common colour that reads on the background at 4.5:1, else white or black.
    pub text1: String,
    /// Another of its colours that reads at 3:1, else the first text colour softened.
    pub text2: String,
    /// The first text colour, softened toward the background.
    pub text3: String,
    /// The second text colour, softened toward the background.
    pub text4: String,
}

type Rgb = [f64; 3];

fn hex(c: Rgb) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        c[0].round().clamp(0.0, 255.0) as u8,
        c[1].round().clamp(0.0, 255.0) as u8,
        c[2].round().clamp(0.0, 255.0) as u8
    )
}

/// WCAG's relative luminance.
fn luminance(c: Rgb) -> f64 {
    let channel = |v: f64| {
        let v = v / 255.0;
        if v <= 0.039_28 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(c[0]) + 0.7152 * channel(c[1]) + 0.0722 * channel(c[2])
}

/// WCAG's contrast ratio, 1 to 21.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn distance(a: Rgb, b: Rgb) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Colours are kept as whole channels, as they're sent, so a contrast checked is the one shown.
fn mix(a: Rgb, b: Rgb, toward_b: f64) -> Rgb {
    [0, 1, 2].map(|i| (a[i] + (b[i] - a[i]) * toward_b).round())
}

/// The colours a set of pixels falls into, most first: each bucket (3 bits a channel) with its
/// pixels' average.
fn buckets(pixels: impl Iterator<Item = [u8; 3]>) -> Vec<(usize, Rgb)> {
    let mut sums: HashMap<u16, (usize, [f64; 3])> = HashMap::new();
    for p in pixels {
        let key = (u16::from(p[0] >> 5) << 6) | (u16::from(p[1] >> 5) << 3) | u16::from(p[2] >> 5);
        let entry = sums.entry(key).or_default();
        entry.0 += 1;
        for (sum, v) in entry.1.iter_mut().zip(p) {
            *sum += f64::from(v);
        }
    }
    let mut out: Vec<(u16, usize, Rgb)> = sums
        .into_iter()
        .map(|(key, (n, sum))| (key, n, sum.map(|s| (s / n as f64).round())))
        .collect();
    // Most pixels first; a tie on the bucket, so the answer never depends on hashing.
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out.into_iter().map(|(_, n, c)| (n, c)).collect()
}

/// A cover's colours from its pixels, row by row, `side` by `side`.
pub fn from_pixels(pixels: &[[u8; 3]], side: usize) -> Option<Colours> {
    if side == 0 || pixels.len() != side * side {
        return None;
    }
    let edge = EDGE.min(side / 2);
    let on_edge = |i: usize| {
        let (x, y) = (i % side, i / side);
        x < edge || y < edge || x >= side - edge || y >= side - edge
    };
    let background = buckets(
        pixels
            .iter()
            .enumerate()
            .filter(|(i, _)| on_edge(*i))
            .map(|(_, p)| *p),
    )
    .first()?
    .1;
    let all = buckets(pixels.iter().copied());
    let readable =
        |c: &Rgb, ratio: f64| contrast(*c, background) >= ratio && distance(*c, background) > 48.0;
    let plain = |ratio: f64| {
        let (white, black) = ([255.0; 3], [0.0; 3]);
        let best = if contrast(white, background) >= contrast(black, background) {
            white
        } else {
            black
        };
        // Toward the cover's own colour, while it still reads.
        let tinted = mix(best, background, 0.12);
        if contrast(tinted, background) >= ratio {
            tinted
        } else {
            best
        }
    };
    let text1 = all
        .iter()
        .map(|(_, c)| *c)
        .find(|c| readable(c, 4.5))
        .unwrap_or_else(|| plain(4.5));
    let text2 = all
        .iter()
        .map(|(_, c)| *c)
        .find(|c| readable(c, 3.0) && distance(*c, text1) > 48.0)
        .unwrap_or_else(|| {
            let softer = mix(text1, background, 0.25);
            if contrast(softer, background) >= 3.0 {
                softer
            } else {
                text1
            }
        });
    Some(Colours {
        background: hex(background),
        text1: hex(text1),
        text2: hex(text2),
        text3: hex(mix(text1, background, 0.4)),
        text4: hex(mix(text2, background, 0.4)),
    })
}

/// A cover's pixels at `SIDE` by `SIDE`, through ffmpeg.
async fn pixels(ffmpeg: &Path, image: &[u8]) -> anyhow::Result<Vec<[u8; 3]>> {
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
            &format!("scale={SIDE}:{SIDE}:flags=area"),
        ])
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"])
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
            "ffmpeg couldn't read the cover: {}",
            String::from_utf8_lossy(&out.stderr)
                .trim()
                .chars()
                .take(200)
                .collect::<String>()
        );
    }
    anyhow::ensure!(
        out.stdout.len() == SIDE * SIDE * 3,
        "ffmpeg gave {} bytes",
        out.stdout.len()
    );
    Ok(out.stdout.as_chunks::<3>().0.to_vec())
}

/// An album's cover as it's on the server now: what it is (to know when it changes) and its bytes.
async fn cover(
    pool: &SqlitePool,
    covers: &Path,
    album: i64,
) -> anyhow::Result<Option<(String, Vec<u8>)>> {
    let row: Option<(String, Option<String>, Option<String>, bool)> = sqlx::query_as(
        "SELECT l.path, a.cover_file, a.match_release_group_id, coalesce(r.cover_front, 0)
         FROM music_albums a JOIN libraries l ON l.id = a.library_id
         LEFT JOIN mb_releases r ON r.id = a.match_release_id
         WHERE a.id = ? AND a.missing_since IS NULL",
    )
    .bind(album)
    .fetch_optional(pool)
    .await?;
    let Some((root, cover_file, group, archive)) = row else {
        return Ok(None);
    };
    if let Some(file) = cover_file {
        let path = within(&root, &file).await?;
        let meta = tokio::fs::metadata(&path).await?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        return Ok(Some((
            format!("file:{file}:{}:{modified}", meta.len()),
            tokio::fs::read(&path).await?,
        )));
    }
    let tagged: Option<(String, i64, i64)> = sqlx::query_as(
        "SELECT rel_path, size, modified_at FROM music_tracks
         WHERE album_id = ? AND missing_since IS NULL AND has_picture = 1
         ORDER BY coalesce(disc_number, 1), track_number, rel_path LIMIT 1",
    )
    .bind(album)
    .fetch_optional(pool)
    .await?;
    if let Some((rel_path, size, modified)) = tagged {
        let path = within(&root, &rel_path).await?;
        let picture = tokio::task::spawn_blocking(move || crate::library::tags::picture(&path))
            .await?
            .map_err(|err| anyhow::anyhow!(err))?;
        return Ok(picture.map(|(bytes, _)| (format!("tag:{rel_path}:{size}:{modified}"), bytes)));
    }
    // The Cover Art Archive's, only when the music API has fetched it already.
    if let (Some(group), true) = (group, archive)
        && let Ok(bytes) = tokio::fs::read(covers.join(format!("rg-{group}.img"))).await
    {
        return Ok(Some((format!("rg:{group}"), bytes)));
    }
    Ok(None)
}

/// An album's cover as it's on the server now, named by where it is and what its bytes are (a
/// moved file with the same picture isn't new), with its bytes. Motion covers are kept
/// against the same name.
pub async fn current(
    pool: &SqlitePool,
    covers: &Path,
    album: i64,
) -> anyhow::Result<Option<(String, Vec<u8>)>> {
    Ok(cover(pool, covers, album)
        .await?
        .map(|(source, bytes)| (format!("{source}#{}", &hex_digest(&bytes)[..16]), bytes)))
}

/// A library file, refused if it resolves outside its library.
async fn within(root: &str, rel_path: &str) -> anyhow::Result<PathBuf> {
    let root = tokio::fs::canonicalize(root).await?;
    let path = tokio::fs::canonicalize(root.join(rel_path)).await?;
    anyhow::ensure!(
        path.starts_with(&root),
        "the cover resolves outside its library"
    );
    Ok(path)
}

/// What one round of working out cover colours did, in albums.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// Albums whose colours were worked out and saved.
    pub measured: usize,
    /// Albums whose cover gave no colours or couldn't be read by ffmpeg; tried again when the cover
    /// changes.
    pub failed: usize,
}

/// Works out the colours of every album whose cover is new or changed since; quick enough to
/// run after each scan.
pub async fn measure_pending(
    pool: &SqlitePool,
    ffmpeg: &Path,
    covers: &Path,
) -> anyhow::Result<Report> {
    let albums: Vec<(i64, Option<String>)> = sqlx::query_as(
        "SELECT a.id, c.source FROM music_albums a LEFT JOIN music_cover_colours c ON c.album_id = a.id
         WHERE a.missing_since IS NULL ORDER BY a.id",
    )
    .fetch_all(pool)
    .await?;
    let mut report = Report::default();
    for (album, measured) in albums {
        let found = match current(pool, covers, album).await {
            Ok(found) => found,
            Err(err) => {
                tracing::debug!(
                    album,
                    error = format!("{err:#}"),
                    "couldn't read an album cover"
                );
                continue;
            }
        };
        let Some((source, bytes)) = found else {
            if measured.is_some() {
                sqlx::query("DELETE FROM music_cover_colours WHERE album_id = ?")
                    .bind(album)
                    .execute(pool)
                    .await?;
            }
            continue;
        };
        if measured.as_deref() == Some(source.as_str()) {
            continue;
        }
        let (colours, error) = match pixels(ffmpeg, &bytes).await.map(|p| from_pixels(&p, SIDE)) {
            Ok(Some(colours)) => (Some(serde_json::to_string(&colours)?), None),
            Ok(None) => (None, Some("no colours in it".to_owned())),
            Err(err) => (None, Some(format!("{err:#}"))),
        };
        if colours.is_some() {
            report.measured += 1;
        } else {
            report.failed += 1;
        }
        sqlx::query(
            "INSERT INTO music_cover_colours (album_id, source, colours, error, measured_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (album_id) DO UPDATE SET source = excluded.source, colours = excluded.colours,
                 error = excluded.error, measured_at = excluded.measured_at",
        )
        .bind(album)
        .bind(&source)
        .bind(colours)
        .bind(error)
        .bind(crate::db::unix_now())
        .execute(pool)
        .await?;
    }
    Ok(report)
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(hex: &str) -> Rgb {
        let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap();
        [
            f64::from(v >> 16),
            f64::from((v >> 8) & 0xff),
            f64::from(v & 0xff),
        ]
    }

    /// A cover `side` wide: `ground` all over, `inner` as a square in the middle (`inset` from the
    /// edges) and `spot` as a smaller one inside it.
    fn cover(
        side: usize,
        ground: [u8; 3],
        inner: [u8; 3],
        inset: usize,
        spot: [u8; 3],
    ) -> Vec<[u8; 3]> {
        (0..side * side)
            .map(|i| {
                let (x, y) = (i % side, i / side);
                let within = |m: usize| x >= m && y >= m && x < side - m && y < side - m;
                if within(inset + 8) {
                    spot
                } else if within(inset) {
                    inner
                } else {
                    ground
                }
            })
            .collect()
    }

    #[test]
    fn the_edge_is_the_background_and_the_covers_own_colours_are_the_text() {
        let pixels = cover(
            SIDE,
            [0x10, 0x20, 0x40],
            [0xf0, 0xc0, 0x40],
            8,
            [0xf0, 0x80, 0x80],
        );
        let c = from_pixels(&pixels, SIDE).unwrap();
        assert_eq!(c.background, "#102040");
        assert_eq!(c.text1, "#f0c040", "the most of the cover that reads");
        assert_eq!(c.text2, "#f08080");
        // Softened toward the background, 40% of the way.
        assert_eq!(c.text3, hex(mix(rgb("#f0c040"), rgb("#102040"), 0.4)));
        assert_eq!(c.text4, hex(mix(rgb("#f08080"), rgb("#102040"), 0.4)));
    }

    #[test]
    fn a_cover_with_nothing_that_reads_gets_white_or_black() {
        // Greys too close to each other to read on one another.
        let pixels = cover(
            SIDE,
            [0x30, 0x30, 0x30],
            [0x48, 0x48, 0x48],
            4,
            [0x40, 0x40, 0x40],
        );
        let c = from_pixels(&pixels, SIDE).unwrap();
        assert_eq!(c.background, "#303030");
        assert!(
            rgb(&c.text1).iter().all(|v| *v > 200.0),
            "near white on dark: {}",
            c.text1
        );
        let light = cover(
            SIDE,
            [0xe8, 0xe8, 0xe0],
            [0xd8, 0xd8, 0xd0],
            4,
            [0xe0, 0xe0, 0xd8],
        );
        let c = from_pixels(&light, SIDE).unwrap();
        assert!(
            rgb(&c.text1).iter().all(|v| *v < 60.0),
            "near black on light: {}",
            c.text1
        );
    }

    #[test]
    fn the_text_colours_always_read_on_the_background() {
        // Covers of every sort, from a fixed sequence so a failure can be found again.
        let mut seed = 0x2545_f491_u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            [(seed >> 56) as u8, (seed >> 48) as u8, (seed >> 40) as u8]
        };
        for _ in 0..2000 {
            let (ground, inner, spot) = (next(), next(), next());
            let inset = usize::from(next()[0] % 20);
            let c = from_pixels(&cover(SIDE, ground, inner, inset, spot), SIDE).unwrap();
            let bg = rgb(&c.background);
            assert!(contrast(rgb(&c.text1), bg) >= 4.5, "{c:?}");
            assert!(contrast(rgb(&c.text2), bg) >= 3.0, "{c:?}");
        }
    }

    #[test]
    fn only_a_whole_square_of_pixels_is_read() {
        assert_eq!(from_pixels(&[[0, 0, 0]; 10], 4), None);
        assert_eq!(from_pixels(&[], 0), None);
    }

    /// An image made by ffmpeg: `ground` with a `box` square in the middle.
    fn image(path: &Path, ground: &str, inner: &str) {
        let status = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!("color=c={ground}:s=120x120,format=rgb24"))
            .args([
                "-vf",
                &format!("drawbox=x=30:y=30:w=60:h=60:color={inner}:t=fill"),
            ])
            .args(["-frames:v", "1"])
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success());
    }

    async fn colours_of(
        pool: &SqlitePool,
        album: i64,
    ) -> Option<(Option<Colours>, Option<String>)> {
        let row: Option<(Option<String>, Option<String>)> =
            sqlx::query_as("SELECT colours, error FROM music_cover_colours WHERE album_id = ?")
                .bind(album)
                .fetch_optional(pool)
                .await
                .unwrap();
        row.map(|(c, e)| (c.map(|c| serde_json::from_str(&c).unwrap()), e))
    }

    #[tokio::test]
    async fn measures_covers_on_the_server_once_each_and_again_when_they_change() {
        let pool = crate::db::open_in_memory().await;
        let library = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let covers = tempfile::tempdir().unwrap();
        let root = library.path();
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        image(&root.join("a/cover.png"), "0x102040", "0xf0c040");
        image(&outside.path().join("secret.png"), "0xff0000", "0x00ff00");
        std::os::unix::fs::symlink(outside.path().join("secret.png"), root.join("b/cover.png"))
            .unwrap();
        // Kept as the music API keeps the archive's: by its release group, in no format's name.
        image(&covers.path().join("archive.png"), "0xf0f0e8", "0x202020");
        std::fs::rename(
            covers.path().join("archive.png"),
            covers.path().join("rg-group.img"),
        )
        .unwrap();
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', ?)")
            .bind(root.to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(
            r#"INSERT INTO mb_releases (id, release_group_id, title, artist_credit, cover_front, fetched_at)
                   VALUES ('rel', 'group', 'Three', '[]', 1, 0);
               INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation,
                   cover_file, match_release_id, match_release_group_id) VALUES
                   (1, 1, 'k1', 'One', 'Alder', 'a', 1, 'a/cover.png', NULL, NULL),
                   (2, 1, 'k2', 'Two', 'Alder', 'b', 1, 'b/cover.png', NULL, NULL),
                   (3, 1, 'k3', 'Three', 'Alder', 'c', 1, NULL, 'rel', 'group'),
                   (4, 1, 'k4', 'Four', 'Alder', 'd', 1, NULL, NULL, NULL);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let ffmpeg = Path::new("ffmpeg");

        let report = measure_pending(&pool, ffmpeg, covers.path()).await.unwrap();
        assert_eq!(
            report,
            Report {
                measured: 2,
                failed: 0
            }
        );
        let (one, _) = colours_of(&pool, 1).await.unwrap();
        let one = one.unwrap();
        assert_eq!(
            (one.background.as_str(), one.text1.as_str()),
            ("#102040", "#f0c040")
        );
        let (three, _) = colours_of(&pool, 3).await.unwrap();
        assert_eq!(
            three.unwrap().background,
            "#f0f0e8",
            "the archive's, as fetched already"
        );
        assert_eq!(
            colours_of(&pool, 2).await,
            None,
            "a cover outside its library isn't read"
        );
        assert_eq!(colours_of(&pool, 4).await, None, "no cover, no colours");

        // Nothing changed: nothing measured again.
        assert_eq!(
            measure_pending(&pool, ffmpeg, covers.path()).await.unwrap(),
            Report::default()
        );

        // A new cover is measured again; one that can't be read keeps why.
        image(&root.join("a/cover.png"), "0x400010", "0xffffff");
        std::fs::write(covers.path().join("rg-group.img"), b"not a picture").unwrap();
        let report = measure_pending(&pool, ffmpeg, covers.path()).await.unwrap();
        assert_eq!(
            report,
            Report {
                measured: 1,
                failed: 1
            }
        );
        assert_eq!(
            colours_of(&pool, 1).await.unwrap().0.unwrap().background,
            "#400010"
        );
        let (none, error) = colours_of(&pool, 3).await.unwrap();
        assert!(none.is_none() && error.unwrap().contains("ffmpeg"));
        // And tried again only when it changes.
        assert_eq!(
            measure_pending(&pool, ffmpeg, covers.path()).await.unwrap(),
            Report::default()
        );

        // A cover gone takes its colours with it.
        std::fs::remove_file(covers.path().join("rg-group.img")).unwrap();
        measure_pending(&pool, ffmpeg, covers.path()).await.unwrap();
        assert_eq!(colours_of(&pool, 3).await, None);
    }
}
