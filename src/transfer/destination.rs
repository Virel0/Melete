//! Where a download goes in the library: a movie's or series' folder, an album's "Album (Year)" in
//! its artist's folder, a song's album.

use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct Target {
    pub(super) title: String,
    pub(super) year: Option<i64>,
    /// The adopted series, for finding its season folders.
    pub(super) series_id: Option<i64>,
    /// The adopted folder, when there is one, and its library.
    pub(super) adopted: Option<String>,
    pub(super) adopted_library: Option<String>,
    /// The library a new folder goes in.
    pub(super) library: Option<String>,
}

/// The folder a download's files go in: the adopted folder (and season folder) when the title
/// has one, else a new "Title (Year)" folder in its library.
pub(super) async fn destination(
    pool: &SqlitePool,
    row: &Row,
) -> anyhow::Result<Option<(PathBuf, PathBuf)>> {
    if let Some(album) = row.wanted_album_id {
        return album_destination(pool, album).await;
    }
    let (query, wanted) = match (row.wanted_movie_id, row.wanted_series_id) {
        (Some(movie), _) => (
            "SELECT w.title, w.year, NULL AS series_id, adopted_lib.path || '/' || m.rel_path AS adopted,
                    adopted_lib.path AS adopted_library, wanted_lib.path AS library
             FROM wanted_movies w
             LEFT JOIN movies m ON m.id = w.movie_id AND m.missing_since IS NULL
             LEFT JOIN libraries adopted_lib ON adopted_lib.id = m.library_id
             LEFT JOIN libraries wanted_lib ON wanted_lib.id = w.library_id
             WHERE w.id = ?",
            movie,
        ),
        (None, Some(series)) => (
            "SELECT w.title, w.year, s.id AS series_id, adopted_lib.path || '/' || s.rel_path AS adopted,
                    adopted_lib.path AS adopted_library, wanted_lib.path AS library
             FROM wanted_series w
             LEFT JOIN series s ON s.id = w.series_id AND s.missing_since IS NULL
             LEFT JOIN libraries adopted_lib ON adopted_lib.id = s.library_id
             LEFT JOIN libraries wanted_lib ON wanted_lib.id = w.library_id
             WHERE w.id = ?",
            series,
        ),
        (None, None) => return Ok(None),
    };
    let target: Option<Target> = sqlx::query_as(query)
        .bind(wanted)
        .fetch_optional(pool)
        .await?;
    let Some(target) = target else {
        return Ok(None);
    };
    let (root, title_dir) = match (target.adopted, target.adopted_library, target.library) {
        (Some(adopted), Some(root), _) => {
            let adopted = PathBuf::from(adopted);
            // A movie adopted as a loose file in the library: new files go beside it.
            let is_file = tokio::fs::metadata(&adopted)
                .await
                .is_ok_and(|m| m.is_file());
            let folder = match (is_file, adopted.parent()) {
                (true, Some(parent)) => parent.to_path_buf(),
                _ => adopted,
            };
            (PathBuf::from(root), folder)
        }
        (_, _, Some(library)) => {
            let root = PathBuf::from(&library);
            let folder = root.join(folder_name(&target.title, target.year));
            (root, folder)
        }
        _ => return Ok(None),
    };
    let (Some(season), Some(_)) = (row.season, row.wanted_series_id) else {
        return Ok(Some((root, title_dir)));
    };
    // The folder this season's files are already in, whatever it's called; else Season NN.
    let existing: Option<String> = match target.series_id {
        Some(series_id) => {
            sqlx::query_scalar(
                "SELECT rel_path FROM media_files
                 WHERE series_id = ? AND season = ? AND missing_since IS NULL
                 ORDER BY id LIMIT 1",
            )
            .bind(series_id)
            .bind(season)
            .fetch_optional(pool)
            .await?
        }
        None => None,
    };
    // rel_path is relative to the library: "<series folder>/<season folder>/<file>", or
    // "<series folder>/<file>" where a show keeps its episodes loose.
    let parts: Vec<String> = existing
        .map(|rel| rel.split('/').map(str::to_owned).collect())
        .unwrap_or_default();
    let season_dir = match parts.as_slice() {
        [_, _] => return Ok(Some((root, title_dir))),
        [_, folder, _] => folder.clone(),
        _ => format!("Season {season:02}"),
    };
    Ok(Some((root, title_dir.join(season_dir))))
}

#[derive(sqlx::FromRow)]
pub(super) struct AlbumTarget {
    pub(super) title: String,
    pub(super) year: Option<i64>,
    pub(super) artist: String,
    pub(super) library: Option<String>,
    /// The folder of one of the artist's albums already in the library, below its artist's.
    pub(super) sibling: Option<String>,
}

/// An album's folder: "Album (Year)" in the folder the artist's albums already in the library
/// share, else in a new one named after the artist.
pub(crate) async fn album_destination(
    pool: &SqlitePool,
    album: i64,
) -> anyhow::Result<Option<(PathBuf, PathBuf)>> {
    let target: Option<AlbumTarget> = sqlx::query_as(
        "SELECT al.title, ar.name AS artist, lib.path AS library,
                CASE WHEN al.release_date GLOB '[0-9][0-9][0-9][0-9]*'
                     THEN CAST(substr(al.release_date, 1, 4) AS INTEGER) END AS year,
                (SELECT a.folder FROM music_albums a JOIN music_artists m ON m.id = a.artist_id
                 WHERE a.library_id = lib.id AND m.mb_artist_id = ar.mb_artist_id
                   AND a.missing_since IS NULL AND a.folder LIKE '%/%'
                 ORDER BY a.id LIMIT 1) AS sibling
         FROM wanted_albums al
         JOIN wanted_artists ar ON ar.id = al.wanted_artist_id
         LEFT JOIN libraries lib ON lib.id = ar.library_id AND lib.kind = 'music'
         WHERE al.id = ?",
    )
    .bind(album)
    .fetch_optional(pool)
    .await?;
    let Some(AlbumTarget {
        title,
        year,
        artist,
        library: Some(library),
        sibling,
    }) = target
    else {
        return Ok(None);
    };
    let artist_dir = match sibling.as_deref().and_then(|s| Path::new(s).parent()) {
        Some(parent) => parent.to_path_buf(),
        None => PathBuf::from(folder_name(&artist, None)),
    };
    let album_dir = folder_name(&title, year);
    // A name of nothing but characters folders can't hold would put the album in the library's
    // own folder.
    if artist_dir.as_os_str().is_empty() || album_dir.is_empty() {
        return Ok(None);
    }
    let root = PathBuf::from(library);
    let dir = root.join(artist_dir).join(album_dir);
    Ok(Some((root, dir)))
}

#[derive(sqlx::FromRow)]
pub(super) struct SongTarget {
    pub(super) artist: String,
    pub(super) album: Option<String>,
    pub(super) library: Option<String>,
    /// The folder of the song's album when the library has it already.
    pub(super) album_folder: Option<String>,
    /// The folder of one of the artist's albums in the library, below its artist's.
    pub(super) sibling: Option<String>,
}

/// A song's folder: its album's folder when the library has that album by that
/// artist already; else, in the folder the artist's albums share (or a new one named after the
/// artist), a new folder named after its album, or "Singles" when it's wanted on its own.
pub(crate) async fn song_destination(
    pool: &SqlitePool,
    song: i64,
) -> anyhow::Result<Option<(PathBuf, PathBuf)>> {
    let target: Option<SongTarget> = sqlx::query_as(
        "SELECT s.artist, s.album, lib.path AS library,
                (SELECT a.folder FROM music_albums a LEFT JOIN music_artists m ON m.id = a.artist_id
                 WHERE a.library_id = lib.id AND a.missing_since IS NULL AND s.album IS NOT NULL
                   AND a.title = s.album COLLATE NOCASE
                   AND (m.mb_artist_id = s.mb_artist_id OR a.artist_name = s.artist COLLATE NOCASE)
                 ORDER BY a.id LIMIT 1) AS album_folder,
                (SELECT a.folder FROM music_albums a LEFT JOIN music_artists m ON m.id = a.artist_id
                 WHERE a.library_id = lib.id AND a.missing_since IS NULL AND a.folder LIKE '%/%'
                   AND (m.mb_artist_id = s.mb_artist_id OR a.artist_name = s.artist COLLATE NOCASE)
                 ORDER BY a.id LIMIT 1) AS sibling
         FROM wanted_songs s
         LEFT JOIN libraries lib ON lib.id = s.library_id AND lib.kind = 'music'
         WHERE s.id = ?",
    )
    .bind(song)
    .fetch_optional(pool)
    .await?;
    let Some(SongTarget {
        artist,
        album,
        library: Some(library),
        album_folder,
        sibling,
    }) = target
    else {
        return Ok(None);
    };
    let root = PathBuf::from(library);
    if let Some(folder) = album_folder {
        return Ok(Some((root.clone(), root.join(folder))));
    }
    let artist_dir = match sibling.as_deref().and_then(|s| Path::new(s).parent()) {
        Some(parent) => parent.to_path_buf(),
        None => PathBuf::from(folder_name(&artist, None)),
    };
    let song_dir = match &album {
        Some(album) => folder_name(album, None),
        None => "Singles".to_owned(),
    };
    // A name of nothing but characters folders can't hold would put it in the library's own
    // folder.
    if artist_dir.as_os_str().is_empty() || song_dir.is_empty() {
        return Ok(None);
    }
    let dir = root.join(artist_dir).join(song_dir);
    Ok(Some((root, dir)))
}

/// A new title folder's name: "Title (Year)", without characters folders can't hold.
pub(super) fn folder_name(title: &str, year: Option<i64>) -> String {
    let clean: String = title
        .chars()
        .map(|c| {
            if matches!(
                c,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'
            ) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    // No hidden folder, no "." or "..": leading dots (and the spaces they leave) go.
    let clean = clean
        .trim_start_matches(|c: char| c == '.' || c.is_whitespace())
        .to_owned();
    match year {
        // Sonarr's titles may already carry it: "Doctor Who (2005)".
        Some(year) if !clean.ends_with(&format!("({year})")) => format!("{clean} ({year})"),
        _ => clean,
    }
}
