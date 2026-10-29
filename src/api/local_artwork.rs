//! A title's own poster and backdrop, from its folder in the library: served under artwork
//! names that say nothing about the path (`local-<hash>.jpg`), resized once by ffmpeg to the
//! size asked for and cached beside TMDB's images. Only images a present title's row names
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

/// The image as a JPEG at `size` (`w342` and the like: that many pixels wide at most), from the
/// cache or made now; `None` when no present title has an image by that name at that kind of
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
    let Some(width) = sizes
        .contains(&size)
        .then(|| size.strip_prefix('w')?.parse::<u32>().ok())
        .flatten()
    else {
        return Ok(None);
    };
    let Some(bytes) = read_inside(Path::new(&root), &rel_path).await else {
        return Ok(None);
    };
    match resize(ffmpeg, &bytes, width).await {
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

/// The image as a JPEG at most `width` pixels wide, its proportions kept.
async fn resize(ffmpeg: &Path, image: &[u8], width: u32) -> anyhow::Result<Vec<u8>> {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new(ffmpeg)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
        .args(["-i", "pipe:0", "-frames:v", "1"])
        .args(["-vf", &format!("scale='min({width},iw)':-2:flags=lanczos")])
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
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
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
    }
}
