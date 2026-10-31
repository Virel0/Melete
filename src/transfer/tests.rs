use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::config::{self, Secret};
use crate::db;
use crate::seedbox::testing;

struct Rig {
    pool: SqlitePool,
    seedbox: Seedbox,
    /// The seedbox's files, under Completed/.
    remote: tempfile::TempDir,
    /// The libraries: movies/ and shows/.
    local: tempfile::TempDir,
    _server: testing::Running,
}

async fn rig() -> Rig {
    let remote = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(remote.path().join("Completed")).unwrap();
    let local = tempfile::tempdir().unwrap();
    // Libraries are never empty; an empty one is an unmounted share.
    for library in ["movies", "shows"] {
        std::fs::create_dir_all(local.path().join(library)).unwrap();
        std::fs::write(local.path().join(library).join(".keep"), b"").unwrap();
    }
    let server = testing::start(remote.path()).await;
    let seedbox = Seedbox::new(config::Seedbox {
        host: "127.0.0.1".into(),
        port: server.port,
        user: "user".into(),
        password: Secret::for_tests("pw"),
        host_key: server.fingerprint.clone(),
        deluge_dir: "/home/user/Completed".into(),
        sftp_dir: "/Completed".into(),
    });
    let pool = db::open_in_memory().await;
    for (id, name, kind) in [(1, "movies", "movies"), (2, "shows", "shows")] {
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (?, ?, ?, ?)")
            .bind(id)
            .bind(name)
            .bind(kind)
            .bind(local.path().join(name).to_string_lossy().into_owned())
            .execute(&pool)
            .await
            .unwrap();
    }
    Rig {
        pool,
        seedbox,
        remote,
        local,
        _server: server,
    }
}

impl Rig {
    /// Puts files on the seedbox; returns Deluge's list of them.
    fn seed(&self, files: &[(&str, Vec<u8>)]) -> String {
        let listed: Vec<serde_json::Value> = files
            .iter()
            .map(|(path, data)| {
                let full = self.remote.path().join("Completed").join(path);
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                std::fs::write(full, data).unwrap();
                serde_json::json!({ "path": path, "size": data.len() })
            })
            .collect();
        serde_json::to_string(&listed).unwrap()
    }

    async fn want_movie(&self, movie_id: Option<i64>) -> i64 {
        sqlx::query_scalar(
                "INSERT INTO wanted_movies (tmdb_id, title, year, monitored, source, added_at, library_id, movie_id)
                 VALUES (1, 'Some Movie', 2024, 1, 'radarr', 0, 1, ?) RETURNING id",
            )
            .bind(movie_id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn download(
        &self,
        movie: Option<i64>,
        series: Option<(i64, i64)>,
        save_path: &str,
        files: &str,
    ) -> i64 {
        sqlx::query_scalar(
                "INSERT INTO downloads (infohash, title, wanted_movie_id, wanted_series_id, season, decision,
                     state, save_path, files, added_at, updated_at, completed_at)
                 VALUES (lower(hex(randomblob(20))), 'Some.Release', ?, ?, ?, '{}', 'completed', ?, ?, 0, 0, 0)
                 RETURNING id",
            )
            .bind(movie)
            .bind(series.map(|(id, _)| id))
            .bind(series.map(|(_, season)| season))
            .bind(save_path)
            .bind(files)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn state(&self, id: i64) -> (String, Option<String>, Option<String>, i64) {
        sqlx::query_as("SELECT state, message, destination, attempts FROM downloads WHERE id = ?")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

fn data(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[tokio::test]
async fn brings_videos_and_subtitles_into_a_new_title_folder() {
    let r = rig().await;
    let movie = data(300_000, 1);
    let files = r.seed(&[
        ("Some.Movie.2024/Some.Movie.2024.mkv", movie.clone()),
        (
            "Some.Movie.2024/Some.Movie.2024.en.srt",
            b"1\n00:00:01,000 --> 00:00:02,000\nHi\n".to_vec(),
        ),
        ("Some.Movie.2024/sample.mkv", data(100, 2)),
        ("Some.Movie.2024/info.nfo", b"nfo".to_vec()),
        ("Some.Movie.2024/Screens/one.mkv", data(10, 3)),
    ]);
    let wanted = r.want_movie(None).await;
    let id = r
        .download(Some(wanted), None, "/home/user/Completed", &files)
        .await;
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();

    let dir = r.local.path().join("movies/Some Movie (2024)");
    let (state, message, destination, _) = r.state(id).await;
    assert_eq!(state, "imported", "{message:?}");
    assert_eq!(destination.as_deref(), Some(dir.to_str().unwrap()));
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["Some.Movie.2024.en.srt", "Some.Movie.2024.mkv"],
        "no samples, screens or partial files"
    );
    assert_eq!(
        std::fs::read(dir.join("Some.Movie.2024.mkv")).unwrap(),
        movie
    );
    assert_eq!(mode(&dir.join("Some.Movie.2024.mkv")), 0o664);
    assert_eq!(mode(&dir), 0o775);
}

#[tokio::test]
async fn resumes_into_the_adopted_folder_and_replaces_nothing() {
    let r = rig().await;
    let adopted = r.local.path().join("movies/Some_Movie_2024");
    std::fs::create_dir_all(&adopted).unwrap();
    std::fs::write(adopted.join("Some.Movie.720p.mkv"), b"the old file").unwrap();
    std::fs::set_permissions(&adopted, std::fs::Permissions::from_mode(0o750)).unwrap();
    sqlx::query(
            "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation) VALUES (5, 1, 'Some_Movie_2024', 'Some Movie', 1)",
        )
        .execute(&r.pool)
        .await
        .unwrap();
    let movie = data(500_000, 9);
    let files = r.seed(&[("Some.Movie.2024.2160p.mkv", movie.clone())]);
    let wanted = r.want_movie(Some(5)).await;
    let id = r
        .download(Some(wanted), None, "/home/user/Completed", &files)
        .await;
    // Half copied before a restart.
    std::fs::write(
        adopted.join(format!(".Some.Movie.2024.2160p.mkv.{id}.melete-part")),
        &movie[..200_000],
    )
    .unwrap();
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();

    assert_eq!(r.state(id).await.0, "imported");
    assert_eq!(
        std::fs::read(adopted.join("Some.Movie.2024.2160p.mkv")).unwrap(),
        movie
    );
    assert_eq!(
        std::fs::read(adopted.join("Some.Movie.720p.mkv")).unwrap(),
        b"the old file"
    );
    assert_eq!(mode(&adopted), 0o750, "an existing folder keeps its mode");
    assert!(
        !adopted
            .join(".Some.Movie.2024.2160p.mkv.melete-part")
            .exists()
    );

    // The same name already there with other contents: refused, untouched.
    let other = r.seed(&[("Again/Some.Movie.720p.mkv", data(64, 4))]);
    let id = r
        .download(Some(wanted), None, "/home/user/Completed", &other)
        .await;
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    let (state, message, ..) = r.state(id).await;
    assert_eq!(state, "failed");
    assert!(message.unwrap().contains("Melete doesn't replace files"));
    assert_eq!(
        std::fs::read(adopted.join("Some.Movie.720p.mkv")).unwrap(),
        b"the old file"
    );
}

/// Every file under `dir`, by its path below it, links as links.
fn every_file(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let meta = std::fs::symlink_metadata(&path).unwrap();
        let name = path
            .strip_prefix(dir)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if meta.is_dir() {
            found.extend(every_file(&path).into_iter().map(|n| format!("{name}/{n}")));
        } else {
            found.push(name);
        }
    }
    found.sort();
    found
}

#[tokio::test]
async fn removing_a_download_deletes_its_part_file_and_nothing_else() {
    let r = rig().await;
    let adopted = r.local.path().join("movies/Some_Movie_2024");
    std::fs::create_dir_all(&adopted).unwrap();
    std::fs::write(adopted.join("Some.Movie.720p.mkv"), b"the old file").unwrap();
    sqlx::query(
            "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation) VALUES (5, 1, 'Some_Movie_2024', 'Some Movie', 1)",
        )
        .execute(&r.pool)
        .await
        .unwrap();
    let movie = data(500_000, 9);
    r.seed(&[("Some.Movie.2024.2160p.mkv", movie.clone())]);
    // Deluge says it's bigger than the seedbox has it: tried again later, the copy kept.
    let files = format!(
        r#"[{{"path": "Some.Movie.2024.2160p.mkv", "size": {}}}]"#,
        movie.len() + 1
    );
    let wanted = r.want_movie(Some(5)).await;
    let id = r
        .download(Some(wanted), None, "/home/user/Completed", &files)
        .await;
    let partial = adopted.join(format!(".Some.Movie.2024.2160p.mkv.{id}.melete-part"));
    std::fs::write(&partial, &movie[..200_000]).unwrap();
    // Look-alikes: another download's part file, ones not quite named so, and the name
    // without the dot.
    for decoy in [
        format!(".Some.Movie.2024.2160p.mkv.{id}1.melete-part"),
        format!(".Some.Movie.2024.2160p.mkv.1{id}.melete-part"),
        format!(".Some.Movie.2024.2160p.mkv.{id}.melete-part.bak"),
        format!("Some.Movie.2024.2160p.mkv.{id}.melete-part"),
        format!(".{id}.melete-part"),
        format!(".Some.Movie.2024.2160p.mkv.{id}.melete-parts"),
    ] {
        std::fs::write(adopted.join(decoy), b"not this download's").unwrap();
    }
    let before = every_file(r.local.path());
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    assert_eq!(r.state(id).await.0, "completed", "to be tried again");
    let recorded: String = sqlx::query_scalar("SELECT partials FROM downloads WHERE id = ?")
        .bind(id)
        .fetch_one(&r.pool)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<String>>(&recorded).unwrap(),
        [partial.to_string_lossy()]
    );

    assert!(crate::downloads::remove(&r.pool, id).await.unwrap());
    let after = every_file(r.local.path());
    let gone: Vec<&String> = before.iter().filter(|f| !after.contains(f)).collect();
    assert_eq!(
        gone,
        [&format!(
            "movies/Some_Movie_2024/.Some.Movie.2024.2160p.mkv.{id}.melete-part"
        )],
        "only its own part file goes"
    );
    assert_eq!(after.len(), before.len() - 1);
    assert_eq!(
        std::fs::read(adopted.join("Some.Movie.720p.mkv")).unwrap(),
        b"the old file"
    );
}

#[tokio::test]
async fn deletes_only_what_is_surely_a_downloads_own_part_file() {
    let r = rig().await;
    let movies = r.local.path().join("movies");
    let dir = movies.join("Film (2020)");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Film.mkv"), b"the film").unwrap();
    // Named as download 7's part file, but a link (to the film), and a folder.
    std::os::unix::fs::symlink(dir.join("Film.mkv"), dir.join(".Link.mkv.7.melete-part")).unwrap();
    std::fs::create_dir(dir.join(".Folder.7.melete-part")).unwrap();
    std::fs::write(dir.join(".Folder.7.melete-part/inside.mkv"), b"inside").unwrap();
    // Outside every library.
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::write(elsewhere.path().join(".Away.mkv.7.melete-part"), b"away").unwrap();
    let mine = dir.join(".Film.mkv.7.melete-part");
    std::fs::write(&mine, b"half").unwrap();
    let before_library = every_file(r.local.path());
    let path = |p: &Path| p.to_string_lossy().into_owned();
    // As if the record had been tampered with: none of these may go.
    let recorded = vec![
        path(&dir.join("Film.mkv")),
        path(&dir.join(".Link.mkv.7.melete-part")),
        path(&dir.join(".Folder.7.melete-part")),
        path(&elsewhere.path().join(".Away.mkv.7.melete-part")),
        ".Film.mkv.7.melete-part".to_owned(),
        format!("{}/../Film (2020)/.Film.mkv.8.melete-part", path(&dir)),
        path(&dir.join(".Film.mkv.8.melete-part")),
        path(&dir.join(".Gone.mkv.7.melete-part")),
        path(&mine),
    ];
    let (deleted, left) = delete_partials(&r.pool, 7, &recorded).await.unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(left.len(), 7, "{left:?}");
    assert!(!mine.exists());
    let mut expected = before_library.clone();
    expected.retain(|f| !f.ends_with(".Film.mkv.7.melete-part"));
    assert_eq!(every_file(r.local.path()), expected);
    assert_eq!(std::fs::read(dir.join("Film.mkv")).unwrap(), b"the film");
    assert!(elsewhere.path().join(".Away.mkv.7.melete-part").exists());
}

#[tokio::test]
async fn refuses_links_on_the_seedbox_and_files_outside_its_folder() {
    let r = rig().await;
    std::fs::write(r.remote.path().join("secret.mkv"), b"not for you").unwrap();
    std::fs::create_dir_all(r.remote.path().join("Completed/Linked")).unwrap();
    std::os::unix::fs::symlink(
        r.remote.path().join("secret.mkv"),
        r.remote.path().join("Completed/Linked/movie.mkv"),
    )
    .unwrap();
    let files = r#"[{"path":"Linked/movie.mkv","size":11}]"#;
    let wanted = r.want_movie(None).await;
    let id = r
        .download(Some(wanted), None, "/home/user/Completed", files)
        .await;
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    let (state, message, ..) = r.state(id).await;
    assert_eq!(state, "failed");
    assert!(message.unwrap().contains("isn't a regular file"));
    assert!(
        !r.local
            .path()
            .join("movies/Some Movie (2024)/movie.mkv")
            .exists()
    );

    let outside = r
        .download(Some(wanted), None, "/home/user/Downloads", files)
        .await;
    transfer(&r.pool, &r.seedbox, None, outside).await.unwrap();
    assert!(
        r.state(outside)
            .await
            .1
            .unwrap()
            .contains("outside the folder MELETE_SEEDBOX_PATHS maps")
    );
    let odd = r
        .download(
            Some(wanted),
            None,
            "/home/user/Completed",
            r#"[{"path":"../x.mkv","size":1}]"#,
        )
        .await;
    transfer(&r.pool, &r.seedbox, None, odd).await.unwrap();
    assert_eq!(r.state(odd).await.0, "failed");
    let orphan = r.download(None, None, "/home/user/Completed", files).await;
    transfer(&r.pool, &r.seedbox, None, orphan).await.unwrap();
    assert!(
        r.state(orphan)
            .await
            .1
            .unwrap()
            .contains("nowhere to put it")
    );
}

#[tokio::test]
async fn retries_later_when_the_library_is_read_only_or_the_seedbox_is_away() {
    let r = rig().await;
    let files = r.seed(&[("Some.Movie.mkv", data(1000, 5))]);
    let wanted = r.want_movie(None).await;
    let id = r
        .download(Some(wanted), None, "/home/user/Completed", &files)
        .await;
    let movies = r.local.path().join("movies");
    std::fs::set_permissions(&movies, std::fs::Permissions::from_mode(0o555)).unwrap();
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    std::fs::set_permissions(&movies, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (state, message, _, attempts) = r.state(id).await;
    assert_eq!((state.as_str(), attempts), ("completed", 1));
    assert!(message.unwrap().contains("isn't writable"));
    // Not due again yet.
    assert_eq!(next_due(&r.pool).await.unwrap(), None);
    sqlx::query("UPDATE downloads SET next_attempt_at = 0")
        .execute(&r.pool)
        .await
        .unwrap();
    assert_eq!(next_due(&r.pool).await.unwrap(), Some(id));

    let mut away = config::Seedbox {
        host: "127.0.0.1".into(),
        port: 9,
        user: "user".into(),
        password: Secret::for_tests("pw"),
        host_key: "SHA256:x".into(),
        deluge_dir: "/home/user/Completed".into(),
        sftp_dir: "/Completed".into(),
    };
    away.port = 9;
    transfer(&r.pool, &Seedbox::new(away), None, id)
        .await
        .unwrap();
    let (state, message, _, attempts) = r.state(id).await;
    assert_eq!((state.as_str(), attempts), ("completed", 2));
    assert!(message.unwrap().contains("trying again in 2 minutes"));
    // Once both are back, it goes through.
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    assert_eq!(r.state(id).await.0, "imported");
}

#[tokio::test]
async fn episodes_go_in_the_seasons_existing_folder_or_a_new_one() {
    let r = rig().await;
    std::fs::create_dir_all(r.local.path().join("shows/Show/S1")).unwrap();
    sqlx::query("INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation) VALUES (3, 2, 'Show', 'Show', 1)")
            .execute(&r.pool)
            .await
            .unwrap();
    sqlx::query(
            "INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, season, episode,
                 first_seen_generation, seen_generation)
             VALUES (2, 'Show/S1/Show.S01E01.mkv', 1, 0, 3, 1, 1, 1, 1)",
        )
        .execute(&r.pool)
        .await
        .unwrap();
    let wanted: i64 = sqlx::query_scalar(
            "INSERT INTO wanted_series (tvdb_id, title, year, monitored, series_type, source, added_at, library_id, series_id)
             VALUES (1, 'Show', 2020, 1, 'standard', 'sonarr', 0, 2, 3) RETURNING id",
        )
        .fetch_one(&r.pool)
        .await
        .unwrap();
    let first = r.seed(&[("Show.S01E02.1080p.WEB.H264-GRP.mkv", data(100, 6))]);
    let id = r
        .download(None, Some((wanted, 1)), "/home/user/Completed", &first)
        .await;
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    assert!(
        r.local
            .path()
            .join("shows/Show/S1/Show.S01E02.1080p.WEB.H264-GRP.mkv")
            .exists()
    );
    let pack = r.seed(&[
        (
            "Show.S02.1080p.BluRay.x264-GRP/Show.S02E01.mkv",
            data(100, 7),
        ),
        (
            "Show.S02.1080p.BluRay.x264-GRP/Show.S02E02.mkv",
            data(100, 8),
        ),
    ]);
    let id = r
        .download(None, Some((wanted, 2)), "/home/user/Completed", &pack)
        .await;
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    assert!(
        r.local
            .path()
            .join("shows/Show/Season 02/Show.S02E02.mkv")
            .exists()
    );

    // A show that keeps its episodes loose gets them loose.
    std::fs::create_dir_all(r.local.path().join("shows/Loose")).unwrap();
    sqlx::query("INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation) VALUES (4, 2, 'Loose', 'Loose', 1)")
            .execute(&r.pool)
            .await
            .unwrap();
    sqlx::query(
            "INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, season, episode,
                 first_seen_generation, seen_generation)
             VALUES (2, 'Loose/Loose.S01E01.mkv', 1, 0, 4, 1, 1, 1, 1)",
        )
        .execute(&r.pool)
        .await
        .unwrap();
    let loose: i64 = sqlx::query_scalar(
            "INSERT INTO wanted_series (tvdb_id, title, monitored, series_type, source, added_at, library_id, series_id)
             VALUES (2, 'Loose', 1, 'standard', 'sonarr', 0, 2, 4) RETURNING id",
        )
        .fetch_one(&r.pool)
        .await
        .unwrap();
    let episode = r.seed(&[("Loose.S01E02.mkv", data(100, 9))]);
    let id = r
        .download(None, Some((loose, 1)), "/home/user/Completed", &episode)
        .await;
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    assert!(r.local.path().join("shows/Loose/Loose.S01E02.mkv").exists());
}

#[tokio::test]
async fn picks_up_transfers_cut_off_by_a_restart() {
    let r = rig().await;
    let files = r.seed(&[("Some.Movie.mkv", data(1000, 5))]);
    let wanted = r.want_movie(None).await;
    let id = r
        .download(Some(wanted), None, "/home/user/Completed", &files)
        .await;
    sqlx::query("UPDATE downloads SET state = 'transferring'")
        .execute(&r.pool)
        .await
        .unwrap();
    let pipeline = Arc::new(Pipeline {
        deluge: crate::deluge::Client::new("http://127.0.0.1:9", Secret::for_tests("x")).unwrap(),
        fetcher: crate::downloads::Fetcher::new().unwrap(),
        seedbox: Some(r.seedbox),
    });
    let fixes = Arc::new(Fixes {
        tools: crate::sound::Tools {
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
        },
        coverart: None,
        lyrics: None,
    });
    let worker = tokio::spawn(work(r.pool.clone(), pipeline, fixes));
    for _ in 0..100 {
        let state: String = sqlx::query_scalar("SELECT state FROM downloads WHERE id = ?")
            .bind(id)
            .fetch_one(&r.pool)
            .await
            .unwrap();
        if state == "imported" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    worker.abort();
    let state: String = sqlx::query_scalar("SELECT state FROM downloads WHERE id = ?")
        .bind(id)
        .fetch_one(&r.pool)
        .await
        .unwrap();
    assert_eq!(state, "imported");
}

#[test]
fn new_folders_are_named_title_and_year() {
    assert_eq!(
        folder_name("Dune: Part Two", Some(2024)),
        "Dune Part Two (2024)"
    );
    assert_eq!(
        folder_name("Doctor Who (2005)", Some(2005)),
        "Doctor Who (2005)"
    );
    assert_eq!(folder_name("../etc", None), "etc");
    assert_eq!(folder_name("AC/DC: Live", Some(1991)), "AC DC Live (1991)");
}

#[test]
fn placing_never_replaces_anything() {
    let dir = tempfile::tempdir().unwrap();
    let (partial, target) = (
        dir.path().join(".x.1.melete-part"),
        dir.path().join("x.mkv"),
    );
    std::fs::write(&partial, b"new").unwrap();
    std::fs::write(&target, b"old").unwrap();
    assert!(matches!(place(&partial, &target), Err(Stop::Fail(why)) if why.contains("appeared")));
    assert!(
        matches!(rename_without_replacing(&partial, &target), Err(Stop::Fail(why)) if why.contains("appeared"))
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"old");
    assert_eq!(std::fs::read(&partial).unwrap(), b"new");
    // A dangling link at the name counts as taken too.
    let link = dir.path().join("dangling.mkv");
    std::os::unix::fs::symlink(dir.path().join("nowhere"), &link).unwrap();
    assert!(matches!(place(&partial, &link), Err(Stop::Fail(_))));
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    // A free name: placed, the partial gone.
    let free = dir.path().join("free.mkv");
    place(&partial, &free).unwrap();
    assert_eq!(std::fs::read(&free).unwrap(), b"new");
    assert!(!partial.exists());
    // Renaming without replacing works where links don't.
    std::fs::write(&partial, b"again").unwrap();
    rename_without_replacing(&partial, &dir.path().join("renamed.mkv")).unwrap();
    assert!(!partial.exists());
}

#[tokio::test]
async fn trusts_only_its_own_files_after_a_restart() {
    let r = rig().await;
    let movie = data(1000, 3);
    let files = r.seed(&[("Some.Movie.mkv", movie.clone())]);
    let wanted = r.want_movie(None).await;
    let dir = r.local.path().join("movies/Some Movie (2024)");
    std::fs::create_dir_all(&dir).unwrap();
    // Same name and size, but someone else's file: not taken for this download's.
    std::fs::write(dir.join("Some.Movie.mkv"), data(1000, 200)).unwrap();
    let id = r
        .download(Some(wanted), None, "/home/user/Completed", &files)
        .await;
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    assert_eq!(r.state(id).await.0, "failed");
    assert_eq!(
        std::fs::read(dir.join("Some.Movie.mkv")).unwrap(),
        data(1000, 200)
    );

    // This download placed it, then a restart came before the database heard: done.
    std::fs::remove_file(dir.join("Some.Movie.mkv")).unwrap();
    let again = r
        .download(Some(wanted), None, "/home/user/Completed", &files)
        .await;
    std::fs::write(dir.join("Some.Movie.mkv"), &movie).unwrap();
    sqlx::query("UPDATE downloads SET placed = json_array(?) WHERE id = ?")
        .bind(dir.join("Some.Movie.mkv").to_string_lossy().into_owned())
        .bind(again)
        .execute(&r.pool)
        .await
        .unwrap();
    transfer(&r.pool, &r.seedbox, None, again).await.unwrap();
    assert_eq!(r.state(again).await.0, "imported");
}

#[tokio::test]
async fn a_part_file_that_is_a_link_is_refused_and_never_written_through() {
    let r = rig().await;
    let files = r.seed(&[("Some.Movie.2024.mkv", data(300_000, 5))]);
    let wanted = r.want_movie(None).await;
    let id = r
        .download(Some(wanted), None, "/home/user/Completed", &files)
        .await;
    let dir = r.local.path().join("movies/Some Movie (2024)");
    std::fs::create_dir_all(&dir).unwrap();
    // Something outside the library, linked at the part file's name.
    let elsewhere = r.local.path().join("elsewhere.bin");
    std::fs::write(&elsewhere, b"not Melete's to write").unwrap();
    let partial = dir.join(format!(".Some.Movie.2024.mkv.{id}.melete-part"));
    std::os::unix::fs::symlink(&elsewhere, &partial).unwrap();

    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    let (state, message, ..) = r.state(id).await;
    assert_eq!(state, "failed");
    assert_eq!(
        message.unwrap(),
        format!(
            "{} is a link; Melete won't write through it",
            partial.display()
        )
    );
    assert_eq!(std::fs::read(&elsewhere).unwrap(), b"not Melete's to write");
    assert!(std::fs::symlink_metadata(&partial).unwrap().is_symlink());
    assert!(!dir.join("Some.Movie.2024.mkv").exists());
}

#[tokio::test]
async fn loose_movies_unmounted_libraries_and_giving_up() {
    let r = rig().await;
    // A movie adopted as a loose file in the library root: the new file goes beside it.
    std::fs::write(r.local.path().join("movies/Some.Movie.2010.mkv"), b"old").unwrap();
    sqlx::query("INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation) VALUES (6, 1, 'Some.Movie.2010.mkv', 'Some Movie', 1)")
            .execute(&r.pool)
            .await
            .unwrap();
    let files = r.seed(&[("Some.Movie.2010.1080p.mkv", data(500, 1))]);
    let wanted = r.want_movie(Some(6)).await;
    let id = r
        .download(Some(wanted), None, "/home/user/Completed", &files)
        .await;
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    assert_eq!(r.state(id).await.0, "imported");
    assert!(
        r.local
            .path()
            .join("movies/Some.Movie.2010.1080p.mkv")
            .exists()
    );

    // An empty library folder is an unmounted share: nothing is written there.
    let shows = r.local.path().join("shows");
    std::fs::remove_file(shows.join(".keep")).unwrap();
    let wanted_show: i64 = sqlx::query_scalar(
            "INSERT INTO wanted_series (tvdb_id, title, monitored, series_type, source, added_at, library_id)
             VALUES (9, 'New Show', 1, 'standard', 'sonarr', 0, 2) RETURNING id",
        )
        .fetch_one(&r.pool)
        .await
        .unwrap();
    let episode = r.seed(&[("New.Show.S01E01.mkv", data(10, 2))]);
    let id = r
        .download(
            None,
            Some((wanted_show, 1)),
            "/home/user/Completed",
            &episode,
        )
        .await;
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    let (state, message, ..) = r.state(id).await;
    assert_eq!(state, "completed");
    assert!(message.unwrap().contains("is the share mounted"));
    assert_eq!(std::fs::read_dir(&shows).unwrap().count(), 0);

    // After the last try, it fails rather than waiting forever.
    sqlx::query("UPDATE downloads SET attempts = 11, next_attempt_at = 0 WHERE id = ?")
        .bind(id)
        .execute(&r.pool)
        .await
        .unwrap();
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    let (state, message, ..) = r.state(id).await;
    assert_eq!(state, "failed");
    assert!(message.unwrap().starts_with("gave up after 12 tries"));
}

impl Rig {
    /// A music library, an artist wanting OK Computer in it, and a finished download of it.
    async fn album_download(&self, release_date: Option<&str>, files: &str) -> i64 {
        let music = self.local.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        std::fs::write(music.join(".keep"), b"").unwrap();
        sqlx::query(
                "INSERT INTO libraries (id, name, kind, path) VALUES (3, 'music', 'music', ?);
                 INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, library_id, source, added_at)
                     VALUES (1, 'mb-radiohead', 'Radiohead', 1, 'all', 3, 'lidarr', 0);
                 INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, release_date, monitored, source, added_at)
                     VALUES (7, 1, 'rg', 'OK Computer', ?, 1, 'lidarr', 0);",
            )
            .bind(music.to_string_lossy().into_owned())
            .bind(release_date)
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query_scalar(
                "INSERT INTO downloads (infohash, title, wanted_album_id, decision, state, save_path,
                     files, added_at, updated_at, completed_at)
                 VALUES (lower(hex(randomblob(20))), 'Radiohead - OK Computer', 7, '{}', 'completed',
                     '/home/user/Completed', ?, 0, 0, 0)
                 RETURNING id",
            )
            .bind(files)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

/// Every file under `dir`, relative to it, in order.
fn tree(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            found.extend(tree(&path).into_iter().map(|f| format!("{name}/{f}")));
        } else {
            found.push(name);
        }
    }
    found.sort();
    found
}

#[tokio::test]
async fn brings_an_albums_tracks_and_cover_into_its_artists_folder() {
    let r = rig().await;
    let track = data(200_000, 4);
    let release = "Radiohead - OK Computer (1997) [FLAC]";
    let files = r.seed(&[
        (&format!("{release}/CD1/01 - Airbag.flac"), track.clone()),
        (&format!("{release}/CD2/01 - Lucky.flac"), data(1_000, 5)),
        (&format!("{release}/cover.jpg"), b"jpg".to_vec()),
        (&format!("{release}/Scans/front.jpg"), b"scan".to_vec()),
        (&format!("{release}/rip.log"), b"log".to_vec()),
        (&format!("{release}/Sample/01.flac"), data(10, 6)),
        (&format!("{release}/CD1/._01 - Airbag.flac"), data(10, 9)),
        (
            &format!("__MACOSX/{release}/CD1/._01 - Airbag.flac"),
            data(10, 9),
        ),
    ]);
    let id = r.album_download(Some("1997-05-21"), &files).await;
    assert_eq!(next_due(&r.pool).await.unwrap(), Some(id));
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();

    let dir = r.local.path().join("music/Radiohead/OK Computer (1997)");
    let (state, message, destination, _) = r.state(id).await;
    assert_eq!(state, "imported", "{message:?}");
    assert_eq!(destination.as_deref(), Some(dir.to_str().unwrap()));
    assert_eq!(
        tree(&dir),
        ["CD1/01 - Airbag.flac", "CD2/01 - Lucky.flac", "cover.jpg"],
        "disc folders kept; no scans, logs, samples, Mac copies or partial files"
    );
    assert_eq!(
        std::fs::read(dir.join("CD1/01 - Airbag.flac")).unwrap(),
        track
    );
    assert_eq!(mode(&dir.join("CD1")), 0o775);
    assert_eq!(mode(&dir.join("CD1/01 - Airbag.flac")), 0o664);
}

#[tokio::test]
async fn an_albums_tracks_are_fixed_before_it_counts_as_imported() {
    let r = rig().await;
    let made = tempfile::tempdir().unwrap();
    let clip = made.path().join("01.flac");
    let status = std::process::Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=d=4"])
        .arg(&clip)
        .status()
        .expect("ffmpeg is installed");
    assert!(status.success());
    let audio = std::fs::read(&clip).unwrap();
    let files = r.seed(&[("OK Computer/01 - Airbag.flac", audio.clone())]);
    let id = r.album_download(Some("1997"), &files).await;
    let fixes = Fixes {
        tools: crate::sound::Tools {
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
        },
        coverart: None,
        lyrics: None,
    };
    transfer(&r.pool, &r.seedbox, Some(&fixes), id)
        .await
        .unwrap();

    let (state, message, _, _) = r.state(id).await;
    assert_eq!(state, "imported");
    assert_eq!(message.as_deref(), Some("in the library; ReplayGain added"));
    let dir = r.local.path().join("music/Radiohead/OK Computer (1997)");
    assert_eq!(tree(&dir), ["01 - Airbag.flac"], "no copies left behind");
    assert_ne!(
        std::fs::read(dir.join("01 - Airbag.flac")).unwrap(),
        audio,
        "tagged"
    );
}

#[tokio::test]
async fn an_album_joins_its_artists_folder_in_the_library() {
    let r = rig().await;
    let files = r.seed(&[
        ("01 - Airbag.flac", data(1_000, 7)),
        ("folder.png", b"png".to_vec()),
    ]);
    let id = r.album_download(None, &files).await;
    sqlx::raw_sql(
            "INSERT INTO music_artists (id, key, name, mb_artist_id) VALUES (1, 'mb:mb-radiohead', 'Radiohead', 'mb-radiohead');
             INSERT INTO music_albums (library_id, key, title, artist_id, artist_name, folder, seen_generation)
                 VALUES (3, 'dir:Kid A', 'Kid A', 1, 'Radiohead', 'R/Radiohead (band)/Kid A', 1);",
        )
        .execute(&r.pool)
        .await
        .unwrap();
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();

    let dir = r.local.path().join("music/R/Radiohead (band)/OK Computer");
    let (state, message, _, _) = r.state(id).await;
    assert_eq!(state, "imported", "{message:?}");
    assert_eq!(tree(&dir), ["01 - Airbag.flac", "folder.png"]);
}

#[tokio::test]
async fn an_album_without_tracks_or_a_music_library_fails() {
    let r = rig().await;
    let files = r.seed(&[
        ("OK Computer/cover.jpg", b"jpg".to_vec()),
        ("OK Computer/info.txt", b"txt".to_vec()),
    ]);
    let id = r.album_download(Some("1997"), &files).await;
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    let (state, message, _, _) = r.state(id).await;
    assert_eq!(state, "failed");
    assert_eq!(
        message.as_deref(),
        Some("Radiohead - OK Computer has no audio files to import")
    );

    let files = r.seed(&[("OK Computer/01.flac", data(10, 8))]);
    sqlx::query("UPDATE wanted_artists SET library_id = NULL")
        .execute(&r.pool)
        .await
        .unwrap();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO downloads (infohash, title, wanted_album_id, decision, state, save_path,
                 files, added_at, updated_at, completed_at)
             VALUES (lower(hex(randomblob(20))), 'Radiohead - OK Computer', 7, '{}', 'completed',
                 '/home/user/Completed', ?, 0, 0, 0)
             RETURNING id",
    )
    .bind(files)
    .fetch_one(&r.pool)
    .await
    .unwrap();
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    let (state, message, _, _) = r.state(id).await;
    assert_eq!(state, "failed");
    assert!(message.unwrap().contains("nowhere to put it"));
    assert!(!r.local.path().join("music/Radiohead").exists());
}

#[tokio::test]
async fn a_soulseek_download_isnt_fetched_from_the_seedbox() {
    let pool = db::open_in_memory().await;
    sqlx::raw_sql(
            "INSERT INTO downloads (source, soulseek, title, decision, state, added_at, updated_at, completed_at)
                 VALUES ('soulseek', '{\"username\":\"a\",\"folder\":\"f\",\"parts\":[]}', 'A - One', '{}',
                         'completed', 0, 0, 1);",
        )
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(next_due(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn copying_home_counts_from_a_resumed_part_file_up_to_the_total() {
    let r = rig().await;
    // Every count written, in order.
    sqlx::raw_sql(
        "CREATE TABLE copied_log (n INTEGER PRIMARY KEY, copied INTEGER, total INTEGER);
         CREATE TRIGGER log_copied AFTER UPDATE OF copied ON downloads
         BEGIN INSERT INTO copied_log (copied, total) VALUES (NEW.copied, NEW.copy_total); END;",
    )
    .execute(&r.pool)
    .await
    .unwrap();
    let movie = data(3_000_000, 7);
    let files = r.seed(&[("Some.Movie.2024/Some.Movie.2024.mkv", movie.clone())]);
    let wanted = r.want_movie(None).await;
    let id = r
        .download(Some(wanted), None, "/home/user/Completed", &files)
        .await;
    // A third of it copied before a restart.
    let dir = r.local.path().join("movies/Some Movie (2024)");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!(".Some.Movie.2024.mkv.{id}.melete-part")),
        &movie[..1_000_000],
    )
    .unwrap();
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    assert_eq!(r.state(id).await.0, "imported");

    let log: Vec<(i64, i64)> = sqlx::query_as("SELECT copied, total FROM copied_log ORDER BY n")
        .fetch_all(&r.pool)
        .await
        .unwrap();
    let counts: Vec<i64> = log.iter().map(|(c, _)| *c).collect();
    assert_eq!(
        counts.first(),
        Some(&1_000_000),
        "from the part file's length: {counts:?}"
    );
    assert_eq!(
        counts.last(),
        Some(&3_000_000),
        "up to the total: {counts:?}"
    );
    assert!(
        counts.windows(2).all(|w| w[0] <= w[1]),
        "never back: {counts:?}"
    );
    assert!(log.iter().all(|(_, total)| *total == 3_000_000));
}
