//! What needs an admin's attention, area by area: problems to fix, decisions waiting,
//! and notes. Worked out from the database each time it's asked; nothing is stored, and
//! nothing outside the server is contacted.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

mod matching;
mod profiles;

use matching::*;
use profiles::*;

/// The admin areas. Anime comes last, after the areas apps knew before it had one of its own,
/// so theirs keep their places in the answer.
pub const AREAS: [&str; 7] = [
    "movies", "series", "music", "indexers", "requests", "system", "anime",
];

/// How much a finding matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Something is wrong and needs fixing.
    Problem,
    /// Something waits for an admin's decision.
    Waiting,
    /// Worth knowing; nothing is wrong.
    Note,
}

/// One thing in an area an admin should know about.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Finding {
    /// How much it matters.
    pub level: Level,
    /// Stable, for apps to act on: no_indexers, indexers_all_off, indexers_off,
    /// indexer_failed, requests_waiting, downloads_failed, follow_failed, files_missing,
    /// files_unparsed, backup_failed, backup_old, gpu_unavailable, opensubtitles_used_up,
    /// disk_nearly_full, tmdb_off, tmdb_refused, tmdb_failing, titles_unmatched,
    /// requests_no_profile.
    pub code: &'static str,
    /// In plain words, ready to show.
    pub message: String,
    /// How many things it's about (requests waiting, downloads failed, files missing).
    pub count: i64,
    /// The indexer or library it's about, when it's about one.
    pub id: Option<i64>,
}

/// One admin area and what needs attention in it.
#[derive(Debug, Serialize)]
pub struct Area {
    /// The area's name, one of [`AREAS`].
    pub area: &'static str,
    /// Empty when nothing needs attention.
    pub findings: Vec<Finding>,
}

/// What needs attention, in every admin area.
#[derive(Debug, Serialize)]
pub struct Health {
    /// Every area, in [`AREAS`]' order, including those with nothing to say.
    pub areas: Vec<Area>,
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Findings as they're found, each with its area.
#[derive(Default)]
struct Found(Vec<(&'static str, Finding)>);

impl Found {
    /// Notes a finding in `area`.
    fn add(
        &mut self,
        area: &'static str,
        level: Level,
        code: &'static str,
        message: String,
        count: i64,
        id: Option<i64>,
    ) {
        self.0.push((
            area,
            Finding {
                level,
                code,
                message,
                count,
                id,
            },
        ));
    }
}

/// Every area, each with its findings: problems first, then what's waiting, then notes.
pub async fn check(pool: &SqlitePool) -> anyhow::Result<Health> {
    let mut found = Found::default();
    // Indexers: none, none searched, some turned off, or failing in the latest RSS sync.
    let on = indexers(pool, &mut found).await?;
    failed_in_latest_sync(pool, &on, &mut found).await?;
    requests_waiting(pool, &mut found).await?;
    downloads_failed(pool, &mut found).await?;
    follows_failed(pool, &mut found).await?;
    library_files(pool, &mut found).await?;
    backups(pool, crate::db::unix_now(), &mut found).await?;
    disks(pool, &mut found).await?;
    opensubtitles_used_up(pool, &mut found).await?;
    tmdb_off(pool, &mut found).await?;
    titles_to_match(pool, &mut found).await?;
    requests_without_profile(pool, &mut found).await?;
    Ok(Health {
        areas: by_area(found),
    })
}

/// Adds a problem to System that's known outside the database (the transcoder's, TMDB
/// matching's), first.
pub fn system_problem(health: &mut Health, finding: Finding) {
    if let Some(system) = health.areas.iter_mut().find(|a| a.area == "system") {
        system.findings.insert(0, finding);
    }
}

/// Adds to System that video conversions use the CPU because the GPU failed its test encode,
/// in ffmpeg's words. It's known to the transcoder, not the database, so it's added here.
pub fn gpu_unavailable(health: &mut Health, reason: &str) {
    let finding = Finding {
        level: Level::Problem,
        code: "gpu_unavailable",
        message: format!("Video conversions are using the CPU: {reason}"),
        count: 0,
        id: None,
    };
    system_problem(health, finding);
}

/// No indexers, every one turned off, or some turned off; the ones searched, by id.
async fn indexers(pool: &SqlitePool, found: &mut Found) -> anyhow::Result<Vec<i64>> {
    let indexers: Vec<(i64, bool)> =
        sqlx::query_as("SELECT id, enabled AND NOT turned_off FROM indexers")
            .fetch_all(pool)
            .await?;
    let on: Vec<i64> = indexers.iter().filter(|i| i.1).map(|i| i.0).collect();
    let off = i64::try_from(indexers.len() - on.len())?;
    if indexers.is_empty() {
        found.add(
            "indexers",
            Level::Problem,
            "no_indexers",
            "No indexers yet, so nothing can be searched for.".into(),
            0,
            None,
        );
    } else if on.is_empty() {
        found.add(
            "indexers",
            Level::Problem,
            "indexers_all_off",
            "Every indexer is turned off, so nothing can be searched for.".into(),
            off,
            None,
        );
    } else if off > 0 {
        found.add(
            "indexers",
            Level::Note,
            "indexers_off",
            format!("{} turned off.", plural(off, "indexer is", "indexers are")),
            off,
            None,
        );
    }
    Ok(on)
}

/// Indexers still searched that failed in the latest RSS sync.
async fn failed_in_latest_sync(
    pool: &SqlitePool,
    on: &[i64],
    found: &mut Found,
) -> anyhow::Result<()> {
    let latest: Option<String> = sqlx::query_scalar(
        "SELECT indexers FROM rss_syncs ORDER BY synced_at DESC, id DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    #[derive(Deserialize)]
    struct Asked {
        indexer_id: i64,
        name: String,
        error: Option<String>,
    }
    let asked: Vec<Asked> = latest
        .map(|json| serde_json::from_str(&json))
        .transpose()?
        .unwrap_or_default();
    // Only ones still searched: one turned off or removed since needs nothing more.
    for a in asked.into_iter().filter(|a| on.contains(&a.indexer_id)) {
        if let Some(error) = a.error {
            found.add(
                "indexers",
                Level::Problem,
                "indexer_failed",
                format!("{} failed in the latest RSS sync: {error}", a.name),
                1,
                Some(a.indexer_id),
            );
        }
    }
    Ok(())
}

/// Requests waiting for a decision: all of them under Requests, and each under the service it
/// belongs to (movies, series, anime or music), so each can say how many of its own wait.
async fn requests_waiting(pool: &SqlitePool, found: &mut Found) -> anyhow::Result<()> {
    let by_service: Vec<(String, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {}, count(*) FROM requests r WHERE r.state = 'requested' GROUP BY 1",
        crate::requests::SERVICE
    )))
    .fetch_all(pool)
    .await?;
    let waiting = |n: i64| {
        format!(
            "{} waiting for a decision.",
            plural(n, "request is", "requests are")
        )
    };
    let all: i64 = by_service.iter().map(|(_, n)| n).sum();
    if all > 0 {
        found.add(
            "requests",
            Level::Waiting,
            "requests_waiting",
            waiting(all),
            all,
            None,
        );
    }
    for area in ["movies", "series", "anime", "music"] {
        if let Some(&(_, n)) = by_service.iter().find(|(s, _)| s == area) {
            found.add(
                area,
                Level::Waiting,
                "requests_waiting",
                waiting(n),
                n,
                None,
            );
        }
    }
    Ok(())
}

/// Downloads that failed, by the service of what they were for: a series' under Anime when the
/// managed lists and history place it there (wanted as anime, or its folder's library has the
/// anime category).
async fn downloads_failed(pool: &SqlitePool, found: &mut Found) -> anyhow::Result<()> {
    let (movies, series, anime, music): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT count(d.wanted_movie_id),
                count(CASE WHEN t.service = 'series' THEN 1 END),
                count(CASE WHEN t.service = 'anime' THEN 1 END),
                count(d.wanted_album_id) + count(d.wanted_song_id)
         FROM downloads d
         LEFT JOIN title_event_titles t ON t.kind = 'series' AND t.wanted_id = d.wanted_series_id
         WHERE d.state = 'failed'",
    )
    .fetch_one(pool)
    .await?;
    let areas = [
        ("movies", movies),
        ("series", series),
        ("anime", anime),
        ("music", music),
    ];
    for (area, failed) in areas {
        if failed > 0 {
            found.add(
                area,
                Level::Problem,
                "downloads_failed",
                format!("{} failed.", plural(failed, "download", "downloads")),
                failed,
                None,
            );
        }
    }
    Ok(())
}

/// Followed artists whose last look for new albums failed; it's tried again the next day.
async fn follows_failed(pool: &SqlitePool, found: &mut Found) -> anyhow::Result<()> {
    let follows_failed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM followed_artist_checks c
         JOIN wanted_artists a ON a.id = c.wanted_artist_id
         WHERE c.error IS NOT NULL AND a.monitored = 1 AND a.monitor_new IN ('all', 'new')",
    )
    .fetch_one(pool)
    .await?;
    if follows_failed > 0 {
        found.add(
            "music",
            Level::Note,
            "follow_failed",
            format!(
                "Looking for new albums failed for {}; it's tried again the next day.",
                plural(follows_failed, "followed artist", "followed artists")
            ),
            follows_failed,
            None,
        );
    }
    Ok(())
}

/// Libraries: files gone from disk, and files whose names or tags couldn't be read. A file
/// removed on purpose stays listed as missing, so these are notes, not problems.
async fn library_files(pool: &SqlitePool, found: &mut Found) -> anyhow::Result<()> {
    let libraries: Vec<(i64, String, i64, i64)> = sqlx::query_as(
        "SELECT l.id, l.name,
             (SELECT count(*) FROM media_files f
                 WHERE f.library_id = l.id AND f.missing_since IS NOT NULL)
             + (SELECT count(*) FROM music_tracks t
                 WHERE t.library_id = l.id AND t.missing_since IS NOT NULL),
             (SELECT count(*) FROM media_files f
                 WHERE f.library_id = l.id AND f.missing_since IS NULL
                   AND f.series_id IS NOT NULL AND f.season IS NULL)
             + (SELECT count(*) FROM music_tracks t
                 WHERE t.library_id = l.id AND t.missing_since IS NULL AND t.tagged = 0)
         FROM libraries l ORDER BY l.name",
    )
    .fetch_all(pool)
    .await?;
    for (id, name, missing, unparsed) in libraries {
        if missing > 0 {
            found.add(
                "system",
                Level::Note,
                "files_missing",
                format!(
                    "{name}: {} missing from disk.",
                    plural(missing, "file is", "files are")
                ),
                missing,
                Some(id),
            );
        }
        if unparsed > 0 {
            found.add(
                "system",
                Level::Note,
                "files_unparsed",
                format!(
                    "{name}: {} couldn't be read for a season and episode, or tags.",
                    plural(unparsed, "file", "files")
                ),
                unparsed,
                Some(id),
            );
        }
    }
    Ok(())
}

/// Libraries whose disk is nearly full: less than [`crate::disk::NEARLY_FULL_PERCENT`] free. A
/// library that can't be measured is left to the scan, which says so.
async fn disks(pool: &SqlitePool, found: &mut Found) -> anyhow::Result<()> {
    let libraries: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id, name, path FROM libraries ORDER BY name")
            .fetch_all(pool)
            .await?;
    for (id, name, path) in libraries {
        let Ok(space) = crate::disk::space(std::path::Path::new(&path)) else {
            continue;
        };
        if let Some(message) = nearly_full(&name, space) {
            found.add(
                "system",
                Level::Problem,
                "disk_nearly_full",
                message,
                1,
                Some(id),
            );
        }
    }
    Ok(())
}

/// OpenSubtitles' downloads for the day used up: fetching resumes when they come back.
async fn opensubtitles_used_up(pool: &SqlitePool, found: &mut Found) -> anyhow::Result<()> {
    if let Some(reset_at) = crate::opensubtitles::used_up_until(pool).await? {
        let at = reset_at.rem_euclid(86_400);
        found.add(
            "system",
            Level::Note,
            "opensubtitles_used_up",
            format!(
                "OpenSubtitles' downloads for today are used up; fetching subtitles resumes at {:02}:{:02} UTC.",
                at / 3600,
                at % 3600 / 60
            ),
            0,
            None,
        );
    }
    Ok(())
}

/// What's said of a library whose disk is nearly full; None when it isn't.
fn nearly_full(name: &str, space: crate::disk::Space) -> Option<String> {
    space.nearly_full().then(|| {
        format!(
            "The disk the {name} library is on is nearly full: {} GB free ({}%).",
            space.free / 1_000_000_000,
            space.free * 100 / space.total
        )
    })
}

/// The database's copies, as of `now`: the latest daily one failed, or the newest one made is
/// older than [`crate::backup::STALE_AFTER`].
async fn backups(pool: &SqlitePool, now: i64, found: &mut Found) -> anyhow::Result<()> {
    let latest_daily: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT error FROM backups WHERE kind = 'daily' ORDER BY made_at DESC, id DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    if let Some((Some(error),)) = latest_daily {
        found.add(
            "system",
            Level::Problem,
            "backup_failed",
            format!("The latest daily backup of the database failed: {error}"),
            1,
            None,
        );
    }
    let newest: Option<i64> =
        sqlx::query_scalar("SELECT max(made_at) FROM backups WHERE file IS NOT NULL")
            .fetch_one(pool)
            .await?;
    if let Some(newest) = newest.filter(|at| now - at > crate::backup::STALE_AFTER) {
        let days = (now - newest) / 86_400;
        found.add(
            "system",
            Level::Problem,
            "backup_old",
            format!(
                "The newest backup of the database is {} old.",
                plural(days, "day", "days")
            ),
            days,
            None,
        );
    }
    Ok(())
}

/// Every area in [`AREAS`]' order with its findings, problems first, then what's waiting,
/// then notes.
fn by_area(found: Found) -> Vec<Area> {
    let rank = |level: Level| match level {
        Level::Problem => 0,
        Level::Waiting => 1,
        Level::Note => 2,
    };
    let mut areas: Vec<Area> = AREAS
        .iter()
        .map(|&area| Area {
            area,
            findings: Vec::new(),
        })
        .collect();
    for (area, finding) in found.0 {
        if let Some(a) = areas.iter_mut().find(|a| a.area == area) {
            a.findings.push(finding);
        }
    }
    for a in &mut areas {
        a.findings.sort_by_key(|f| rank(f.level));
    }
    areas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn of<'a>(health: &'a Health, area: &str) -> &'a [Finding] {
        &health
            .areas
            .iter()
            .find(|a| a.area == area)
            .unwrap()
            .findings
    }

    #[tokio::test]
    async fn an_empty_server_needs_indexers_and_nothing_else() {
        let pool = db::open_in_memory().await;
        let health = check(&pool).await.unwrap();
        assert_eq!(
            health.areas.iter().map(|a| a.area).collect::<Vec<_>>(),
            AREAS
        );
        let codes: Vec<(&str, &str)> = health
            .areas
            .iter()
            .flat_map(|a| a.findings.iter().map(move |f| (a.area, f.code)))
            .collect();
        assert_eq!(codes, [("indexers", "no_indexers")]);
    }

    #[tokio::test]
    async fn finds_failing_indexers_waiting_requests_and_failed_downloads() {
        let pool = db::open_in_memory().await;
        let tpb = crate::indexers::add_preset(&pool, "thepiratebay")
            .await
            .unwrap()
            .unwrap();
        let nyaa = crate::indexers::add_preset(&pool, "nyaa")
            .await
            .unwrap()
            .unwrap();
        let tosho = crate::indexers::add_preset(&pool, "animetosho")
            .await
            .unwrap()
            .unwrap();
        crate::indexers::update(&pool, tosho, "AnimeTosho", false)
            .await
            .unwrap()
            .unwrap();
        let user = crate::auth::create_user(&pool, "alice", "alice-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        // An older sync where every indexer failed, then the latest: Nyaa failed, and so did
        // AnimeTosho, which has been turned off since.
        let asked = |nyaa_error: &str| {
            serde_json::json!([
                { "indexer_id": tpb, "name": "The Pirate Bay", "results": 3, "error": null },
                { "indexer_id": nyaa, "name": "Nyaa", "results": 0, "error": nyaa_error },
                { "indexer_id": tosho, "name": "AnimeTosho", "results": 0, "error": "refused" },
            ])
            .to_string()
        };
        for (at, error) in [
            (10, "old"),
            (20, "the indexer answered 503 Service Unavailable"),
        ] {
            sqlx::query(
                "INSERT INTO rss_syncs (synced_at, indexers, new_releases, grabbed)
                 VALUES (?, ?, 0, '[]')",
            )
            .bind(at)
            .bind(asked(error))
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO requests (user_id, kind, tmdb_id, title, state, created_at)
             VALUES (?1, 'movie', 1, 'Dune', 'requested', 0),
                    (?1, 'movie', 2, 'Arrival', 'requested', 0);
             INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at)
             VALUES (1, 1, 'Dune', 1, 'manual', 0);",
        )
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO downloads (infohash, title, wanted_movie_id, decision, state, added_at,
                 updated_at)
             VALUES ('a11bae70b2fdd9eb95b63e746f8594dce2650b0c', 'Dune.2021', 1, '{}', 'failed',
                 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let health = check(&pool).await.unwrap();
        assert_eq!(
            of(&health, "indexers"),
            [
                Finding {
                    level: Level::Problem,
                    code: "indexer_failed",
                    message: "Nyaa failed in the latest RSS sync: the indexer answered 503 Service Unavailable".into(),
                    count: 1,
                    id: Some(nyaa),
                },
                Finding {
                    level: Level::Note,
                    code: "indexers_off",
                    message: "1 indexer is turned off.".into(),
                    count: 1,
                    id: None,
                },
            ]
        );
        assert_eq!(
            of(&health, "requests")[0].message,
            "2 requests are waiting for a decision."
        );
        assert_eq!(of(&health, "requests")[0].count, 2);
        assert_eq!(of(&health, "movies")[0].code, "downloads_failed");
        assert!(of(&health, "series").is_empty());
        assert!(of(&health, "music").is_empty());
        sqlx::query(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
             VALUES (1, 'a1', 'Arca', 1, 'all', 'manual', 0);
             INSERT INTO followed_artist_checks (wanted_artist_id, since, checked_at, looked, error)
             VALUES (1, '2026-10-01', 0, 0, 'MusicBrainz answered 503');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let health = check(&pool).await.unwrap();
        assert_eq!(
            of(&health, "music")[0].message,
            "Looking for new albums failed for 1 followed artist; it's tried again the next day."
        );

        // Every indexer off is a problem of its own.
        for id in [tpb, nyaa] {
            crate::indexers::update(&pool, id, "x", false)
                .await
                .unwrap()
                .unwrap();
        }
        let health = check(&pool).await.unwrap();
        assert_eq!(
            of(&health, "indexers")
                .iter()
                .map(|f| f.code)
                .collect::<Vec<_>>(),
            ["indexers_all_off"]
        );
    }

    #[tokio::test]
    async fn anime_and_each_service_have_their_own_failures_and_waiting_requests() {
        let pool = db::open_in_memory().await;
        let user = crate::auth::create_user(&pool, "alice", "alice-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        // Three wanted series: one standard, one wanted as anime, and one standard that goes
        // to a library with the anime category; each with a failed download.
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path, category)
             VALUES (1, 'Shows', 'shows', '/s', NULL), (2, 'Anime', 'shows', '/a', 'anime');
             INSERT INTO wanted_series (id, tvdb_id, tmdb_id, title, monitored, series_type,
                 library_id, source, added_at)
             VALUES (1, 11, 101, 'Severance', 1, 'standard', 1, 'manual', 0),
                    (2, 12, 102, 'Frieren', 1, 'anime', 1, 'manual', 0),
                    (3, 13, 103, 'Mushishi', 1, 'standard', 2, 'manual', 0);
             INSERT INTO downloads (infohash, title, wanted_series_id, decision, state,
                 added_at, updated_at)
             VALUES ('1111111111111111111111111111111111111111', 'a', 1, '{}', 'failed', 0, 0),
                    ('2222222222222222222222222222222222222222', 'b', 2, '{}', 'failed', 0, 0),
                    ('3333333333333333333333333333333333333333', 'c', 3, '{}', 'failed', 0, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Waiting: a movie, two series (one TMDB has as anime, one from before that was
        // kept), an anime series and an album; one approved series request doesn't wait.
        sqlx::query(
            "INSERT INTO requests (user_id, kind, tmdb_id, mb_artist_id, mb_release_group_id,
                 title, anime, state, created_at, decided_at)
             VALUES (?1, 'movie', 1, NULL, NULL, 'Dune', NULL, 'requested', 0, NULL),
                    (?1, 'series', 2, NULL, NULL, 'Andor', 0, 'requested', 0, NULL),
                    (?1, 'series', 3, NULL, NULL, 'Bluey', NULL, 'requested', 0, NULL),
                    (?1, 'series', 4, NULL, NULL, 'Dandadan', 1, 'requested', 0, NULL),
                    (?1, 'series', 5, NULL, NULL, 'Monster', 1, 'approved', 0, 0),
                    (?1, 'album', NULL, 'a1', 'rg1', 'Kid A', NULL, 'requested', 0, NULL)",
        )
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();

        let health = check(&pool).await.unwrap();
        let said = |area: &str| {
            of(&health, area)
                .iter()
                .map(|f| (f.code, f.count))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            said("series"),
            [("downloads_failed", 1), ("requests_waiting", 2)]
        );
        assert_eq!(
            said("anime"),
            [("downloads_failed", 2), ("requests_waiting", 1)]
        );
        assert_eq!(said("movies"), [("requests_waiting", 1)]);
        assert_eq!(said("music"), [("requests_waiting", 1)]);
        assert_eq!(
            of(&health, "anime")[1].message,
            "1 request is waiting for a decision."
        );
        // Requests still says how many wait in all.
        assert_eq!(said("requests"), [("requests_waiting", 5)]);
    }

    #[test]
    fn a_library_disk_under_five_percent_free_is_a_problem() {
        let space = |free| crate::disk::Space {
            total: 1_000_000_000_000,
            free,
        };
        assert_eq!(
            nearly_full("Films", space(40_000_000_000)).as_deref(),
            Some("The disk the Films library is on is nearly full: 40 GB free (4%).")
        );
        assert_eq!(nearly_full("Films", space(60_000_000_000)), None);
    }

    #[tokio::test]
    async fn a_library_with_room_raises_nothing() {
        let pool = db::open_in_memory().await;
        let folder = tempfile::tempdir().unwrap();
        sqlx::query("INSERT INTO libraries (name, kind, path) VALUES ('films', 'movies', ?)")
            .bind(folder.path().to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        let mut found = Found::default();
        disks(&pool, &mut found).await.unwrap();
        let roomy = crate::disk::space(folder.path()).unwrap();
        assert_eq!(found.0.is_empty(), !roomy.nearly_full());
    }

    #[tokio::test]
    async fn titles_to_match_are_noted_under_their_service_and_tmdb_off_under_system() {
        let pool = db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path, category)
             VALUES (1, 'Films', 'movies', '/no/such/m', NULL),
                    (2, 'Shows', 'shows', '/no/such/s', NULL),
                    (3, 'Anime', 'shows', '/no/such/a', 'anime');
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
                 match_status, match_method, missing_since)
             VALUES (1, 1, 'A', 'A', 0, 'unmatched', 'search', NULL),
                    (2, 1, 'B', 'B', 0, 'ambiguous', 'search', NULL),
                    (3, 1, 'C', 'C', 0, 'unmatched', 'manual', NULL),
                    (4, 1, 'D', 'D', 0, 'unmatched', 'search', 5),
                    (5, 1, 'E', 'E', 0, 'matched', 'search', NULL),
                    (6, 1, 'F', 'F', 0, NULL, NULL, NULL);
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation,
                 match_status, match_method)
             VALUES (1, 2, 'G', 'G', 0, 'matched', 'search'),
                    (2, 3, 'H', 'H', 0, 'ambiguous', 'search');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let health = check(&pool).await.unwrap();
        let said = |area: &str| {
            of(&health, area)
                .iter()
                .map(|f| (f.level, f.code, f.count))
                .collect::<Vec<_>>()
        };
        assert_eq!(said("movies"), [(Level::Note, "titles_unmatched", 2)]);
        assert_eq!(
            of(&health, "movies")[0].message,
            "2 titles need a match: TMDB couldn't settle them."
        );
        assert_eq!(said("anime"), [(Level::Note, "titles_unmatched", 1)]);
        assert!(said("series").is_empty());
        assert_eq!(said("system"), [(Level::Note, "tmdb_off", 0)]);
        assert!(
            of(&health, "system")[0]
                .message
                .contains("Outside services")
        );

        // With a token, TMDB isn't said to be off.
        sqlx::query(
            "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_TMDB_TOKEN', 't', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let health = check(&pool).await.unwrap();
        assert!(of(&health, "system").is_empty());
    }

    #[tokio::test]
    async fn tmdb_off_says_nothing_without_a_video_library() {
        let pool = db::open_in_memory().await;
        sqlx::query("INSERT INTO libraries (name, kind, path) VALUES ('Music', 'music', '/no/m')")
            .execute(&pool)
            .await
            .unwrap();
        let mut found = Found::default();
        tmdb_off(&pool, &mut found).await.unwrap();
        assert!(found.0.is_empty());
    }
}
