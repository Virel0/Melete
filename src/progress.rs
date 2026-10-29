//! How each wanted title is getting on: one step out of Wanted, Looking for
//! a download, Downloading, Transferring, Importing, In your library and Failed, worked out
//! from the wanted item, whether it's on disk, and its newest download, for torrents and
//! Soulseek alike. Requests become wanted items once accepted, so this is their status
//! too.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::unix_now;

/// How long a finished download stays in the activity list.
const RECENT_SECONDS: i64 = 24 * 3600;
/// Entries in the activity list at most.
const LIST_LIMIT: i64 = 200;

/// Where a wanted title has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Wanted, and nothing is looking for it on its own (a song waits for a search).
    Wanted,
    /// RSS syncs look out for it, or a search is under way.
    Searching,
    Downloading,
    /// Fetched, and being copied home (or waiting to be).
    Transferring,
    /// In place, and being tagged and measured.
    Importing,
    InLibrary,
    Failed,
}

/// How one wanted title is getting on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Progress {
    /// movie, series, album, song, or artist for a whole-artist search.
    pub kind: &'static str,
    /// The wanted item's id (for an artist search, the wanted artist's).
    pub wanted_id: Option<i64>,
    /// The title, as the wanted item or the download names it.
    pub title: String,
    /// Where it has got to.
    pub step: Step,
    /// How much of the download is done, while downloading.
    pub percent: Option<f64>,
    /// The latest word on it, in plain words.
    pub message: Option<String>,
    /// Its newest download, when there is one.
    pub download_id: Option<i64>,
    /// torrent or soulseek, when there's a download.
    pub source: Option<String>,
    /// When the download last changed; null for a state worked out from the library.
    pub updated_at: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct DownloadRow {
    id: i64,
    source: String,
    title: String,
    state: String,
    progress: f64,
    message: Option<String>,
    updated_at: i64,
    wanted_movie_id: Option<i64>,
    wanted_series_id: Option<i64>,
    wanted_album_id: Option<i64>,
    wanted_song_id: Option<i64>,
    season: Option<i64>,
    episode: Option<i64>,
    /// The wanted title's name, when it's still wanted.
    wanted_title: Option<String>,
}

/// The step a download stands at.
pub fn download_step(state: &str, message: Option<&str>) -> Step {
    match state {
        "downloading" => Step::Downloading,
        "completed" => Step::Transferring,
        // Copying, then fixing an album or song in place (album_fix's messages).
        "transferring" if message.is_some_and(|m| m.starts_with("measuring")) => Step::Importing,
        "transferring" => Step::Transferring,
        "imported" => Step::InLibrary,
        _ => Step::Failed,
    }
}

/// The plain words for a download's state when it has no message of its own.
fn download_message(row: &DownloadRow) -> Option<String> {
    match (row.state.as_str(), &row.message) {
        (_, Some(message)) if !message.is_empty() => Some(message.clone()),
        ("completed", _) if row.source == "soulseek" => {
            Some("waiting to be copied from slskd's folder".into())
        }
        ("completed", _) => Some("waiting to be copied from the seedbox".into()),
        _ => None,
    }
}

fn kind_of(row: &DownloadRow) -> (&'static str, Option<i64>) {
    match (
        row.wanted_movie_id,
        row.wanted_series_id,
        row.wanted_album_id,
        row.wanted_song_id,
    ) {
        (Some(id), ..) => ("movie", Some(id)),
        (_, Some(id), ..) => ("series", Some(id)),
        (_, _, Some(id), _) => ("album", Some(id)),
        (.., Some(id)) => ("song", Some(id)),
        _ => ("download", None),
    }
}

fn from_download(row: DownloadRow) -> Progress {
    let (kind, wanted_id) = kind_of(&row);
    let base = row
        .wanted_title
        .clone()
        .unwrap_or_else(|| row.title.clone());
    let title = match (row.season, row.episode) {
        (Some(s), Some(e)) => format!("{base}, season {s} episode {e}"),
        (Some(s), None) => format!("{base}, season {s}"),
        _ => base,
    };
    let step = download_step(&row.state, row.message.as_deref());
    Progress {
        kind,
        wanted_id,
        title,
        step,
        percent: (step == Step::Downloading).then_some(row.progress),
        message: download_message(&row),
        download_id: Some(row.id),
        source: Some(row.source.clone()),
        updated_at: Some(row.updated_at),
    }
}

const DOWNLOAD_COLUMNS: &str = "SELECT d.id, d.source, d.title, d.state, d.progress, d.message,
        d.updated_at, d.wanted_movie_id, d.wanted_series_id, d.wanted_album_id, d.wanted_song_id,
        d.season, d.episode,
        coalesce(m.title, s.title, al.title || ' by ' || ar.name, so.title || ' by ' || so.artist)
            AS wanted_title
     FROM downloads d
     LEFT JOIN wanted_movies m ON m.id = d.wanted_movie_id
     LEFT JOIN wanted_series s ON s.id = d.wanted_series_id
     LEFT JOIN wanted_albums al ON al.id = d.wanted_album_id
     LEFT JOIN wanted_artists ar ON ar.id = al.wanted_artist_id
     LEFT JOIN wanted_songs so ON so.id = d.wanted_song_id";

/// What's on the move: every download under way, and those that finished or failed in the
/// last day, newest first; and whole-artist searches queued or running.
pub async fn list(pool: &SqlitePool) -> anyhow::Result<Vec<Progress>> {
    let rows: Vec<DownloadRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{DOWNLOAD_COLUMNS}
         WHERE d.state IN ('downloading', 'completed', 'transferring') OR d.updated_at >= ?
         ORDER BY d.updated_at DESC, d.id DESC LIMIT ?"
    )))
    .bind(unix_now() - RECENT_SECONDS)
    .bind(LIST_LIMIT)
    .fetch_all(pool)
    .await?;
    let runs: Vec<(i64, String, String, i64, i64)> = sqlx::query_as(
        "SELECT r.wanted_artist_id, a.name, r.state, json_array_length(r.albums), r.started_at
         FROM artist_searches r JOIN wanted_artists a ON a.id = r.wanted_artist_id
         WHERE r.state IN ('queued', 'running') ORDER BY r.id",
    )
    .fetch_all(pool)
    .await?;
    let mut all: Vec<Progress> = runs
        .into_iter()
        .map(|(artist, name, state, done, started)| Progress {
            kind: "artist",
            wanted_id: Some(artist),
            title: format!("Every album by {name}"),
            step: Step::Searching,
            percent: None,
            message: Some(if state == "queued" {
                "waiting for another search to finish".into()
            } else {
                format!("{done} albums done so far")
            }),
            download_id: None,
            source: None,
            updated_at: Some(started),
        })
        .collect();
    all.extend(rows.into_iter().map(from_download));
    Ok(all)
}

/// The kinds of wanted item, as their API paths name them.
pub fn kind(path: &str) -> Option<&'static str> {
    match path {
        "movies" => Some("movie"),
        "series" => Some("series"),
        "albums" => Some("album"),
        "songs" => Some("song"),
        _ => None,
    }
}

/// What one wanted item has in place of a download: its title, whether it's on disk, and
/// whether something looks out for it on its own.
struct Wanted {
    title: String,
    on_disk: bool,
    looked_for: bool,
}

async fn wanted(pool: &SqlitePool, kind: &str, id: i64) -> anyhow::Result<Option<Wanted>> {
    let row: Option<(String, bool, bool)> = match kind {
        "movie" => {
            sqlx::query_as(
                "SELECT w.title,
                        EXISTS (SELECT 1 FROM media_files f WHERE f.movie_id = w.movie_id
                                AND f.missing_since IS NULL),
                        w.monitored = 1
                 FROM wanted_movies w WHERE w.id = ?",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?
        }
        "series" => {
            sqlx::query_as(
                "SELECT w.title,
                        EXISTS (SELECT 1 FROM media_files f WHERE f.series_id = w.series_id
                                AND f.missing_since IS NULL),
                        w.monitored = 1
                 FROM wanted_series w WHERE w.id = ?",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?
        }
        "album" => {
            sqlx::query_as(
                "SELECT w.title || ' by ' || a.name,
                        EXISTS (SELECT 1 FROM music_albums m WHERE m.missing_since IS NULL
                                AND w.mb_release_group_id IN (m.mb_release_group_id, m.match_release_group_id)),
                        w.monitored = 1 AND a.monitored = 1
                 FROM wanted_albums w JOIN wanted_artists a ON a.id = w.wanted_artist_id
                 WHERE w.id = ?",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?
        }
        // A song is in the library once a download of it is; nothing looks for songs on its
        // own, so one waits for a search.
        "song" => {
            sqlx::query_as(
                "SELECT title || ' by ' || artist, 0, 0 FROM wanted_songs WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?
        }
        _ => None,
    };
    Ok(row.map(|(title, on_disk, looked_for)| Wanted {
        title,
        on_disk,
        looked_for,
    }))
}

/// How one wanted item is getting on; None when it isn't wanted.
pub async fn of(
    pool: &SqlitePool,
    kind: &'static str,
    id: i64,
) -> anyhow::Result<Option<Progress>> {
    let Some(wanted) = wanted(pool, kind, id).await? else {
        return Ok(None);
    };
    let column = match kind {
        "movie" => "wanted_movie_id",
        "series" => "wanted_series_id",
        "album" => "wanted_album_id",
        _ => "wanted_song_id",
    };
    // A song fetched through its album, with no download of its own, goes as the
    // album's download goes.
    let whose = if kind == "song" {
        "(d.wanted_song_id = ?1
          OR (d.wanted_album_id = (SELECT wanted_album_id FROM wanted_songs WHERE id = ?1)
              AND NOT EXISTS (SELECT 1 FROM downloads x
                              WHERE x.wanted_song_id = ?1 AND x.state <> 'failed')))"
            .to_owned()
    } else {
        format!("d.{column} = ?1")
    };
    // One under way comes first, whenever it was grabbed; else the newest.
    let latest: Option<DownloadRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{DOWNLOAD_COLUMNS} WHERE {whose}
         ORDER BY d.state IN ('downloading', 'completed', 'transferring') DESC, d.id DESC LIMIT 1"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let under_way =
        |d: &DownloadRow| ["downloading", "completed", "transferring"].contains(&d.state.as_str());
    let progress = match latest {
        // On disk is where a request ends, even while an upgrade of it downloads.
        Some(d) if under_way(&d) && !wanted.on_disk => from_download(d),
        // On disk, or imported and waiting for the next scan to see it.
        latest if wanted.on_disk || latest.as_ref().is_some_and(|d| d.state == "imported") => {
            Progress {
                kind,
                wanted_id: Some(id),
                title: wanted.title,
                step: Step::InLibrary,
                percent: None,
                message: None,
                download_id: latest.as_ref().map(|d| d.id),
                source: latest.map(|d| d.source),
                updated_at: None,
            }
        }
        latest => {
            let failed = latest.filter(|d| d.state == "failed");
            let why = failed.as_ref().and_then(|d| d.message.clone());
            let (step, message) = match (wanted.looked_for, &why) {
                // Still looked out for: a failed try is behind it, not the end.
                (true, Some(why)) => (Step::Searching, Some(format!("the last try failed: {why}"))),
                (true, None) => (Step::Searching, Some("looking out for a release".into())),
                (false, Some(why)) => (Step::Failed, Some(why.clone())),
                (false, None) => (
                    Step::Wanted,
                    Some(if kind == "song" {
                        "waiting for a search".into()
                    } else {
                        "not monitored".into()
                    }),
                ),
            };
            Progress {
                kind,
                wanted_id: Some(id),
                title: wanted.title,
                step,
                percent: None,
                message,
                download_id: failed.as_ref().map(|d| d.id),
                source: failed.map(|d| d.source),
                updated_at: None,
            }
        }
    };
    Ok(Some(progress))
}

/// One season of a wanted series, as it stands: whether it's looked for, how much of it
/// is in the library, and a download of it under way.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeasonProgress {
    /// The season's number.
    pub season: i64,
    /// Whether it's looked for (monitored), whoever asked for it.
    pub monitored: bool,
    /// Episodes TMDB lists for it that have aired; null when TMDB's list isn't known here.
    pub episodes: Option<i64>,
    /// How many of its episodes are in the library.
    pub in_library: i64,
    /// Where the season has got to.
    pub step: Step,
    /// How much of the download is done, while downloading.
    pub percent: Option<f64>,
}

/// Each season of a wanted series, in order; empty when it isn't wanted.
pub async fn seasons(
    pool: &SqlitePool,
    wanted_series_id: i64,
) -> anyhow::Result<Vec<SeasonProgress>> {
    let listed: Vec<(i64, bool)> = sqlx::query_as(
        "SELECT season, monitored = 1 FROM wanted_seasons WHERE wanted_series_id = ?
         ORDER BY season",
    )
    .bind(wanted_series_id)
    .fetch_all(pool)
    .await?;
    if listed.is_empty() {
        return Ok(Vec::new());
    }
    // Episodes on disk, a file holding several counted for each.
    let files: Vec<(i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT f.season, f.episode, f.episode_end FROM media_files f
           JOIN wanted_series w ON w.series_id = f.series_id
         WHERE w.id = ? AND f.missing_since IS NULL AND f.season IS NOT NULL",
    )
    .bind(wanted_series_id)
    .fetch_all(pool)
    .await?;
    let mut on_disk = std::collections::BTreeSet::new();
    for (season, first, last) in files {
        for episode in first..=last.unwrap_or(first).min(first + 1000) {
            on_disk.insert((season, episode));
        }
    }
    // Aired episodes, and whether each is left out on its own.
    let aired: Vec<(i64, i64, bool)> = sqlx::query_as(
        "SELECT e.season, e.episode,
                EXISTS (SELECT 1 FROM wanted_episode_overrides o
                        WHERE o.wanted_series_id = w.id AND o.season = e.season
                          AND o.episode = e.episode AND o.monitored = 0)
         FROM tmdb_episodes e
           JOIN wanted_series w ON w.tmdb_id = e.series_tmdb_id
         WHERE w.id = ? AND e.air_date IS NOT NULL AND e.air_date <= date('now')",
    )
    .bind(wanted_series_id)
    .fetch_all(pool)
    .await?;
    let known = !aired.is_empty();
    // Downloads under way, newest first; one without a season (a whole-series pack) is every
    // season's.
    let moving: Vec<(Option<i64>, String, f64, Option<String>)> = sqlx::query_as(
        "SELECT season, state, progress, message FROM downloads
         WHERE wanted_series_id = ? AND state IN ('downloading', 'completed', 'transferring')
         ORDER BY id DESC",
    )
    .bind(wanted_series_id)
    .fetch_all(pool)
    .await?;
    Ok(listed
        .into_iter()
        .map(|(season, monitored)| {
            let in_library = on_disk.iter().filter(|(s, _)| *s == season).count() as i64;
            let of_season = || aired.iter().filter(|(s, ..)| *s == season);
            let episodes = known.then(|| of_season().count() as i64);
            // Every aired episode that isn't left out on its own is here.
            let complete = episodes.is_some_and(|n| n > 0)
                && of_season().all(|(s, e, left_out)| *left_out || on_disk.contains(&(*s, *e)));
            let download = moving.iter().find(|(s, ..)| s.is_none_or(|s| s == season));
            let (step, percent) = match download {
                Some((_, state, done, message)) if !complete => {
                    let step = download_step(state, message.as_deref());
                    (step, (step == Step::Downloading).then_some(*done))
                }
                _ if complete => (Step::InLibrary, None),
                _ if monitored => (Step::Searching, None),
                // Not looked for: what's here is all there'll be.
                _ if in_library > 0 => (Step::InLibrary, None),
                _ => (Step::Wanted, None),
            };
            SeasonProgress {
                season,
                monitored,
                episodes,
                in_library,
                step,
                percent,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn fixture() -> SqlitePool {
        let pool = db::open_in_memory().await;
        let now = unix_now();
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m'), (3, 'music', 'music', '/a');
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation) VALUES (5, 1, 'Here (2020)', 'Here', 1);
             INSERT INTO media_files (library_id, rel_path, size, modified_at, first_seen_generation, seen_generation, movie_id)
                 VALUES (1, 'Here (2020)/Here.mkv', 1, 0, 1, 1, 5);
             INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at, movie_id) VALUES
                 (1, 1, 'Here', 1, 'radarr', 0, 5),
                 (2, 2, 'Missing', 1, 'radarr', 0, NULL),
                 (3, 3, 'Forgotten', 0, 'radarr', 0, NULL);
             INSERT INTO wanted_series (id, tvdb_id, title, monitored, series_type, source, added_at) VALUES (1, 1, 'A Show', 1, 'standard', 'sonarr', 0);
             INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
                 VALUES (1, 'mb-a', 'Radiohead', 1, 'all', 'lidarr', 0);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at) VALUES
                 (7, 1, 'rg-ok', 'OK Computer', 1, 'lidarr', 0),
                 (8, 1, 'rg-kid', 'Kid A', 1, 'lidarr', 0);
             INSERT INTO wanted_songs (id, title, artist, source, added_at) VALUES
                 (2, 'Airbag', 'Radiohead', 'manual', 0),
                 (3, 'Lucky', 'Radiohead', 'manual', 0);
             INSERT INTO downloads (id, infohash, title, wanted_movie_id, decision, state, progress, added_at, updated_at)
                 VALUES (10, 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'Missing.2024.1080p', 2, '{}', 'downloading', 42.5, 0, ?1);
             INSERT INTO downloads (id, infohash, title, wanted_series_id, season, episode, decision, state, message, added_at, updated_at)
                 VALUES (11, 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'A.Show.S02E03', 1, 2, 3, '{}', 'transferring', 'copying 1 of 2: x.mkv', 0, ?1);
             INSERT INTO downloads (id, source, soulseek, title, wanted_album_id, decision, state, message, added_at, updated_at) VALUES
                 (12, 'soulseek', '{\"username\":\"a\",\"folder\":\"f\",\"parts\":[]}', 'Radiohead - OK Computer', 7, '{}', 'completed', NULL, 0, ?1),
                 (13, 'soulseek', '{\"username\":\"a\",\"folder\":\"g\",\"parts\":[]}', 'Radiohead - Kid A', 8, '{}', 'failed', 'slskd: 01.flac: Rejected', 0, ?1),
                 (14, 'soulseek', '{\"username\":\"a\",\"folder\":\"h\",\"parts\":[]}', 'Radiohead - Airbag', NULL, '{}', 'imported', 'in the library', 0, ?1),
                 (15, 'soulseek', '{\"username\":\"a\",\"folder\":\"i\",\"parts\":[]}', 'Old one', NULL, '{}', 'imported', NULL, 0, 0);
             UPDATE downloads SET wanted_song_id = 2 WHERE id = 14;
             INSERT INTO artist_searches (wanted_artist_id, grab, state, albums, started_at)
                 VALUES (1, 1, 'running', '[{}, {}]', ?1);",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[test]
    fn a_downloads_state_is_a_step() {
        assert_eq!(download_step("downloading", None), Step::Downloading);
        assert_eq!(download_step("completed", None), Step::Transferring);
        assert_eq!(
            download_step("transferring", Some("copying 3 of 12: a.flac")),
            Step::Transferring
        );
        assert_eq!(
            download_step("transferring", Some("measuring and tagging")),
            Step::Importing
        );
        assert_eq!(download_step("imported", None), Step::InLibrary);
        assert_eq!(download_step("failed", Some("x")), Step::Failed);
    }

    #[tokio::test]
    async fn lists_whats_on_the_move_and_what_just_finished() {
        let pool = fixture().await;
        let all = list(&pool).await.unwrap();
        // Kind, wanted id, title, step, percent and message.
        type Row<'a> = (
            &'a str,
            Option<i64>,
            &'a str,
            Step,
            Option<f64>,
            Option<&'a str>,
        );
        let summary: Vec<Row> = all
            .iter()
            .map(|p| {
                (
                    p.kind,
                    p.wanted_id,
                    p.title.as_str(),
                    p.step,
                    p.percent,
                    p.message.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "artist",
                    Some(1),
                    "Every album by Radiohead",
                    Step::Searching,
                    None,
                    Some("2 albums done so far")
                ),
                (
                    "song",
                    Some(2),
                    "Airbag by Radiohead",
                    Step::InLibrary,
                    None,
                    Some("in the library")
                ),
                (
                    "album",
                    Some(8),
                    "Kid A by Radiohead",
                    Step::Failed,
                    None,
                    Some("slskd: 01.flac: Rejected")
                ),
                (
                    "album",
                    Some(7),
                    "OK Computer by Radiohead",
                    Step::Transferring,
                    None,
                    Some("waiting to be copied from slskd's folder")
                ),
                (
                    "series",
                    Some(1),
                    "A Show, season 2 episode 3",
                    Step::Transferring,
                    None,
                    Some("copying 1 of 2: x.mkv")
                ),
                (
                    "movie",
                    Some(2),
                    "Missing",
                    Step::Downloading,
                    Some(42.5),
                    None
                ),
            ],
            "newest first; the download finished two days ago isn't listed"
        );
    }

    #[tokio::test]
    async fn one_wanted_items_step() {
        let pool = fixture().await;
        let step = |kind, id| {
            let pool = pool.clone();
            async move {
                of(&pool, kind, id)
                    .await
                    .unwrap()
                    .map(|p| (p.step, p.message, p.percent))
            }
        };
        assert_eq!(
            step("movie", 1).await,
            Some((Step::InLibrary, None, None)),
            "on disk"
        );
        assert_eq!(
            step("movie", 2).await,
            Some((Step::Downloading, None, Some(42.5)))
        );
        assert_eq!(
            step("movie", 3).await,
            Some((Step::Wanted, Some("not monitored".into()), None))
        );
        assert_eq!(
            step("album", 8).await,
            Some((
                Step::Searching,
                Some("the last try failed: slskd: 01.flac: Rejected".into()),
                None
            )),
            "still looked out for after a failed try"
        );
        assert_eq!(
            step("song", 2).await,
            Some((Step::InLibrary, None, None)),
            "imported"
        );
        assert_eq!(
            step("song", 3).await,
            Some((Step::Wanted, Some("waiting for a search".into()), None))
        );
        assert_eq!(step("movie", 99).await, None);

        // A download under way wins over a newer one that's done, unless it's on disk.
        sqlx::query(
            "INSERT INTO downloads (id, source, soulseek, title, wanted_song_id, decision, state, added_at, updated_at)
             VALUES (20, 'soulseek', '{\"username\":\"b\",\"folder\":\"j\",\"parts\":[]}', 'x', 3, '{}', 'downloading', 0, 0),
                    (21, 'soulseek', '{\"username\":\"b\",\"folder\":\"k\",\"parts\":[]}', 'y', 3, '{}', 'imported', 0, 0);
             INSERT INTO downloads (id, infohash, title, wanted_movie_id, decision, state, added_at, updated_at)
             VALUES (22, 'cccccccccccccccccccccccccccccccccccccccc', 'Here.2160p', 1, '{}', 'downloading', 0, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(step("song", 3).await.map(|s| s.0), Some(Step::Downloading));
        assert_eq!(
            step("movie", 1).await.map(|s| s.0),
            Some(Step::InLibrary),
            "an upgrade downloading doesn't take it out of the library"
        );
        assert_eq!(kind("albums"), Some("album"));
        assert_eq!(kind("artists"), None);
    }

    #[tokio::test]
    async fn each_season_of_a_series_has_its_own_step() {
        let pool = fixture().await;
        assert_eq!(
            seasons(&pool, 1).await.unwrap(),
            vec![],
            "no seasons listed"
        );
        // Season 1 is all here, season 2 is being copied home, season 3 is looked for with one
        // episode of two here (a file holding two counts twice), season 4 isn't asked for, and
        // season 5 hasn't aired.
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (2, 'shows', 'shows', '/s');
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation) VALUES (7, 2, 'A Show', 'A Show', 1);
             INSERT INTO tmdb_series (tmdb_id, name, fetched_at) VALUES (70, 'A Show', 0);
             UPDATE wanted_series SET series_id = 7, tmdb_id = 70 WHERE id = 1;
             INSERT INTO wanted_seasons (wanted_series_id, season, monitored)
                 VALUES (1, 1, 1), (1, 2, 1), (1, 3, 1), (1, 4, 0), (1, 5, 1);
             INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, air_date) VALUES
                 (70, 1, 1, 1, '2001-01-01'), (70, 1, 2, 2, '2001-01-08'),
                 (70, 2, 1, 3, '2002-01-01'),
                 (70, 3, 1, 4, '2003-01-01'), (70, 3, 2, 5, '2003-01-08'),
                 (70, 4, 1, 6, '2004-01-01'),
                 (70, 5, 1, 7, '2999-01-01');
             INSERT INTO media_files (library_id, rel_path, size, modified_at, first_seen_generation, seen_generation, series_id, season, episode, episode_end) VALUES
                 (2, 'A Show/S01E01-E02.mkv', 1, 0, 1, 1, 7, 1, 1, 2),
                 (2, 'A Show/S03E01.mkv', 1, 0, 1, 1, 7, 3, 1, NULL),
                 (2, 'A Show/S03E01.old.mkv', 1, 0, 1, 1, 7, 3, 1, NULL);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let got: Vec<_> = seasons(&pool, 1)
            .await
            .unwrap()
            .into_iter()
            .map(|s| (s.season, s.monitored, s.episodes, s.in_library, s.step))
            .collect();
        assert_eq!(
            got,
            vec![
                (1, true, Some(2), 2, Step::InLibrary),
                (2, true, Some(1), 0, Step::Transferring),
                (3, true, Some(2), 1, Step::Searching),
                (4, false, Some(1), 0, Step::Wanted),
                (5, true, Some(0), 0, Step::Searching),
            ]
        );
    }

    #[tokio::test]
    async fn an_episode_left_out_doesnt_keep_its_season_looked_for() {
        let pool = fixture().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (2, 'shows', 'shows', '/s');
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
                 VALUES (7, 2, 'A Show', 'A Show', 1);
             INSERT INTO tmdb_series (tmdb_id, name, fetched_at) VALUES (70, 'A Show', 0);
             UPDATE wanted_series SET series_id = 7, tmdb_id = 70 WHERE id = 1;
             INSERT INTO wanted_seasons (wanted_series_id, season, monitored) VALUES (1, 1, 1);
             INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, air_date)
                 VALUES (70, 1, 1, 1, '2001-01-01'), (70, 1, 2, 2, '2001-01-08');
             INSERT INTO media_files (library_id, rel_path, size, modified_at,
                     first_seen_generation, seen_generation, series_id, season, episode)
                 VALUES (2, 'A Show/S01E01.mkv', 1, 0, 1, 1, 7, 1, 1);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let step = || async { seasons(&pool, 1).await.unwrap()[0].step };
        assert_eq!(step().await, Step::Searching, "episode 2 is missing");
        crate::wanted_edit::set_episode_monitored(&pool, 1, 1, 2, false)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(step().await, Step::InLibrary, "episode 2 is left out");
    }
}
