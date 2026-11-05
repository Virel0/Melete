//! A title's own poster and backdrop, from its folder in the library: served under artwork
//! names that say nothing about the path (`local-<hash>.jpg`), cropped to the shape of its kind
//! (2:3 for a poster, 16:9 for a backdrop) around the picture's subject and resized once by
//! ffmpeg to the size asked for, and cached beside TMDB's images. Only images a present title's row names
//! are read, and only from inside its library.

use std::path::Path;
use std::process::Stdio;

use sqlx::SqlitePool;

use super::ApiError;
use super::media::{BACKDROP_SIZES, POSTER_SIZES, save_atomically};

/// An image file larger than this isn't read.
const MAX_SOURCE_BYTES: u64 = 50 << 20;

/// Whether an artwork file name is a title's own image rather than TMDB's.
pub(super) fn is_local(file: &str) -> bool {
    file.starts_with("local-")
}

/// The image as a JPEG at `size` (`w342` and the like: that many pixels wide at most;
/// `original` up to [`ORIGINAL_WIDTH`]), from the cache or made now; `None` when no present title has an image by that name at that kind of
/// size, or it can't be read.
pub(super) async fn image(
    pool: &SqlitePool,
    artwork_dir: &Path,
    ffmpeg: &Path,
    size: &str,
    file: &str,
) -> Result<Option<Vec<u8>>, ApiError> {
    let cached = artwork_dir.join(size).join(file);
    match tokio::fs::read(&cached).await {
        Ok(bytes) => return Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    let name = format!("/{file}");
    let source: Option<(String, String, bool)> = sqlx::query_as(
        "SELECT l.path, x.local_poster, 1 FROM movies x JOIN libraries l ON l.id = x.library_id
             WHERE x.local_poster_key = ?1 AND x.missing_since IS NULL
         UNION ALL SELECT l.path, x.local_backdrop, 0 FROM movies x
             JOIN libraries l ON l.id = x.library_id
             WHERE x.local_backdrop_key = ?1 AND x.missing_since IS NULL
         UNION ALL SELECT l.path, x.local_poster, 1 FROM series x
             JOIN libraries l ON l.id = x.library_id
             WHERE x.local_poster_key = ?1 AND x.missing_since IS NULL
         UNION ALL SELECT l.path, x.local_backdrop, 0 FROM series x
             JOIN libraries l ON l.id = x.library_id
             WHERE x.local_backdrop_key = ?1 AND x.missing_since IS NULL
         LIMIT 1",
    )
    .bind(&name)
    .fetch_optional(pool)
    .await?;
    let Some((root, rel_path, poster)) = source else {
        return Ok(None);
    };
    let sizes = if poster { POSTER_SIZES } else { BACKDROP_SIZES };
    let Some(width) = sizes.contains(&size).then(|| width_of(size)).flatten() else {
        return Ok(None);
    };
    let Some(bytes) = read_inside(Path::new(&root), &rel_path).await else {
        return Ok(None);
    };
    let shape = if poster { 2.0 / 3.0 } else { 16.0 / 9.0 };
    match fit(ffmpeg, &bytes, shape, width).await {
        Ok(jpeg) => {
            if let Err(err) = save_atomically(&cached, &jpeg).await {
                tracing::warn!(error = format!("{err:#}"), "could not cache artwork");
            }
            Ok(Some(jpeg))
        }
        Err(err) => {
            tracing::warn!(
                error = format!("{err:#}"),
                "could not resize a title's own image"
            );
            Ok(None)
        }
    }
}

/// The widest a title's own image is served at, asked for `original`: a 4K screen's width.
const ORIGINAL_WIDTH: u32 = 3840;

/// How wide a size is at most: `w342` 342 pixels, `original` [`ORIGINAL_WIDTH`].
fn width_of(size: &str) -> Option<u32> {
    if size == "original" {
        return Some(ORIGINAL_WIDTH);
    }
    size.strip_prefix('w')?.parse().ok()
}

/// The file's bytes, if it's inside the library (a link out of it isn't followed) and not too
/// large.
async fn read_inside(root: &Path, rel_path: &str) -> Option<Vec<u8>> {
    let root = tokio::fs::canonicalize(root).await.ok()?;
    let path = tokio::fs::canonicalize(root.join(rel_path)).await.ok()?;
    if !path.starts_with(&root) {
        return None;
    }
    let metadata = tokio::fs::metadata(&path).await.ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES {
        return None;
    }
    tokio::fs::read(&path).await.ok()
}

/// The image cropped to `shape` (width over height) around its subject, as the backdrop focus
/// finds it (the middle when it can't), then as a JPEG at most `width` pixels wide. An image
/// already of that shape loses nothing.
async fn fit(ffmpeg: &Path, image: &[u8], shape: f64, width: u32) -> anyhow::Result<Vec<u8>> {
    let (x, y) = crate::backdrop_focus::of_image(ffmpeg, image)
        .await
        .unwrap_or((0.5, 0.5));
    let crop = crop_filter(shape, x, y);
    to_jpeg(
        ffmpeg,
        image,
        &format!("{crop},scale='min({width},iw)':-2:flags=lanczos"),
    )
    .await
}

/// ffmpeg's crop to `shape`, as large as fits, centred on `x`, `y` (0 to 1) as far as the edges
/// allow.
fn crop_filter(shape: f64, x: f64, y: f64) -> String {
    format!(
        "crop=w='min(iw,ih*{shape:.6})':h='min(ih,iw/{shape:.6})':\
         x='max(0,min(iw-ow,{x:.4}*iw-ow/2))':y='max(0,min(ih-oh,{y:.4}*ih-oh/2))'"
    )
}

/// The image as a JPEG at most `width` pixels wide, its proportions kept.
pub(super) async fn resize(ffmpeg: &Path, image: &[u8], width: u32) -> anyhow::Result<Vec<u8>> {
    to_jpeg(
        ffmpeg,
        image,
        &format!("scale='min({width},iw)':-2:flags=lanczos"),
    )
    .await
}

/// The image through ffmpeg's `filter`, as a JPEG.
async fn to_jpeg(ffmpeg: &Path, image: &[u8], filter: &str) -> anyhow::Result<Vec<u8>> {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new(ffmpeg)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
        .args(["-i", "pipe:0", "-frames:v", "1"])
        .args(["-vf", filter])
        .args(["-pix_fmt", "yuvj420p", "-q:v", "3"])
        .args(["-f", "image2pipe", "-c:v", "mjpeg", "pipe:1"])
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
    anyhow::ensure!(
        out.status.success() && out.stdout.starts_with(b"\xff\xd8"),
        "ffmpeg couldn't read the image: {}",
        String::from_utf8_lossy(&out.stderr)
            .trim()
            .chars()
            .take(200)
            .collect::<String>()
    );
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::api::title_artwork::ArtworkState;

    /// The width a baseline or progressive JPEG says it is, from its frame header.
    fn jpeg_width(bytes: &[u8]) -> Option<u16> {
        let mut i = 2;
        while i + 9 < bytes.len() {
            let marker = bytes[i + 1];
            let length = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            if (0xc0..=0xc2).contains(&marker) {
                return Some(u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]));
            }
            i += 2 + length;
        }
        None
    }

    async fn fixture(library: &Path, outside: &Path) -> SqlitePool {
        let folder = library.join("Home Video");
        std::fs::create_dir_all(&folder).unwrap();
        let made = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
            .args(["color=c=blue:s=800x1200", "-frames:v", "1"])
            .arg(folder.join("poster.png"))
            .status()
            .unwrap();
        assert!(made.success());
        std::fs::write(
            outside.join("elsewhere.jpg"),
            b"\xff\xd8\xff not the library's",
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.join("elsewhere.jpg"), folder.join("fanart.jpg"))
            .unwrap();
        let pool = crate::db::open_in_memory().await;
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'm', 'movies', ?)")
            .bind(library.to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(
            "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
                 local_poster, local_poster_key, local_backdrop, local_backdrop_key) VALUES
                 (1, 1, 'Home Video', 'Home Video', 1, 'Home Video/poster.png',
                  '/local-0123456789abcdef0123456789abcdef.jpg', 'Home Video/fanart.jpg',
                  '/local-fedcba9876543210fedcba9876543210.jpg')",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn a_titles_own_images_are_shown_resized_and_cached() {
        let library = tempfile::tempdir().unwrap();
        let artwork = tempfile::tempdir().unwrap();
        let pool = fixture(library.path(), artwork.path()).await;
        let user = crate::auth::testing::user(&pool, "u", false).await;
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        // Nothing is fetched from TMDB here; an address that answers nothing makes sure.
        let images = crate::tmdb::Images::with_base("http://127.0.0.1:9".into()).unwrap();
        let state = ArtworkState::new(
            pool.clone(),
            None,
            images.clone(),
            artwork.path().into(),
            "ffmpeg".into(),
        );
        let app =
            super::super::media::routes_with(pool.clone(), artwork.path().into(), images, state);
        let get = |uri: &str| {
            let request = Request::get(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                let status = response.status();
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                (status, bytes.to_vec())
            }
        };

        let (_, list) = get("/api/v1/movies").await;
        let list: serde_json::Value = serde_json::from_slice(&list).unwrap();
        assert_eq!(
            list["items"][0]["poster"],
            "/api/v1/artwork/w342/local-0123456789abcdef0123456789abcdef.jpg"
        );
        let (_, detail) = get("/api/v1/movies/1").await;
        let detail: serde_json::Value = serde_json::from_slice(&detail).unwrap();
        assert_eq!(
            detail["poster"],
            "/api/v1/artwork/w500/local-0123456789abcdef0123456789abcdef.jpg"
        );

        let (status, jpeg) =
            get("/api/v1/artwork/w342/local-0123456789abcdef0123456789abcdef.jpg").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            jpeg_width(&jpeg),
            Some(342),
            "resized to the size asked for"
        );
        let kept = artwork
            .path()
            .join("w342/local-0123456789abcdef0123456789abcdef.jpg");
        assert!(kept.exists(), "and kept");
        for refused in [
            // A poster at a backdrop's size.
            "/api/v1/artwork/w1280/local-0123456789abcdef0123456789abcdef.jpg",
            // A name no title has.
            "/api/v1/artwork/w342/local-00000000000000000000000000000000.jpg",
            // A backdrop that's a link out of the library.
            "/api/v1/artwork/w1280/local-fedcba9876543210fedcba9876543210.jpg",
        ] {
            assert_eq!(get(refused).await.0, StatusCode::NOT_FOUND, "{refused}");
        }
        // At a larger size for a large screen, never wider than the image (800 pixels).
        let (status, jpeg) =
            get("/api/v1/artwork/w780/local-0123456789abcdef0123456789abcdef.jpg").await;
        assert_eq!((status, jpeg_width(&jpeg)), (StatusCode::OK, Some(780)));
    }

    /// The width and height a baseline or progressive JPEG says it is.
    fn jpeg_size(bytes: &[u8]) -> Option<(u16, u16)> {
        let mut i = 2;
        while i + 9 < bytes.len() {
            let marker = bytes[i + 1];
            let length = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            if (0xc0..=0xc2).contains(&marker) {
                let height = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]);
                let width = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]);
                return Some((width, height));
            }
            i += 2 + length;
        }
        None
    }

    #[tokio::test]
    async fn a_titles_own_images_come_in_their_kinds_shape() {
        let library = tempfile::tempdir().unwrap();
        let artwork = tempfile::tempdir().unwrap();
        let pool = fixture(library.path(), artwork.path()).await;
        // A square poster and a backdrop four times as wide as it's high.
        let folder = library.path().join("Odd Shapes");
        std::fs::create_dir_all(&folder).unwrap();
        for (size, name) in [("1000x1000", "poster.png"), ("4000x1000", "fanart.png")] {
            let made = std::process::Command::new("ffmpeg")
                .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
                .arg(format!("color=c=green:s={size}"))
                .args(["-frames:v", "1"])
                .arg(folder.join(name))
                .status()
                .unwrap();
            assert!(made.success());
        }
        sqlx::query(
            "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
                 local_poster, local_poster_key, local_backdrop, local_backdrop_key) VALUES
                 (2, 1, 'Odd Shapes', 'Odd Shapes', 1, 'Odd Shapes/poster.png',
                  '/local-11111111111111111111111111111111.jpg', 'Odd Shapes/fanart.png',
                  '/local-22222222222222222222222222222222.jpg')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let made = |size: &str, file: &str| {
            let (pool, artwork, size, file) = (
                pool.clone(),
                artwork.path().to_path_buf(),
                size.to_owned(),
                file.to_owned(),
            );
            async move {
                let made = image(&pool, &artwork, Path::new("ffmpeg"), &size, &file).await;
                made.ok().flatten().expect("made")
            }
        };
        let poster = made("w500", "local-11111111111111111111111111111111.jpg").await;
        assert_eq!(jpeg_size(&poster), Some((500, 750)), "2:3");
        let backdrop = made("w1280", "local-22222222222222222222222222222222.jpg").await;
        assert_eq!(jpeg_size(&backdrop), Some((1280, 720)), "16:9");
    }

    #[test]
    fn the_crop_centres_on_the_subject_as_far_as_the_edges_allow() {
        assert_eq!(
            crop_filter(16.0 / 9.0, 0.25, 0.5),
            "crop=w='min(iw,ih*1.777778)':h='min(ih,iw/1.777778)':\
             x='max(0,min(iw-ow,0.2500*iw-ow/2))':y='max(0,min(ih-oh,0.5000*ih-oh/2))'"
        );
    }

    #[test]
    fn original_is_a_4k_screens_width_at_most() {
        assert_eq!(width_of("w342"), Some(342));
        assert_eq!(width_of("original"), Some(ORIGINAL_WIDTH));
        assert_eq!(width_of("h632"), None);
    }
}
