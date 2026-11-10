use super::*;
use crate::db;

const NOW: i64 = 1_000_000;

fn found(id: &str, title: &str, channel: &str, duration: f64) -> Found {
    Found {
        id: id.into(),
        title: title.into(),
        channel: channel.into(),
        duration: Some(duration),
    }
}

/// The result chosen: the best fitting.
fn choose<'a>(song: &Song<'_>, found: &'a [Found]) -> Option<&'a Found> {
    choosing::ranked(song, found).into_iter().next()
}

fn song<'a>(title: &'a str, artist: &'a str, length: Option<i64>) -> Song<'a> {
    Song {
        title,
        artist,
        length_seconds: length,
    }
}

#[test]
fn a_video_is_the_song_only_with_its_title_artist_and_length() {
    let wanted = song("El Corrido del Río Bravo", "Dueto Sol & Luna", Some(180));
    let results = [
        // Another artist's song of the name.
        found(
            "aaaaaaaaaaa",
            "El Corrido del Río Bravo",
            "Los Otros",
            181.0,
        ),
        // A cover by someone, naming them.
        found(
            "bbbbbbbbbbb",
            "El Corrido del Río Bravo (cover) Dueto Sol y Luna",
            "Fan",
            180.0,
        ),
        // Theirs, but sped up.
        found(
            "ccccccccccc",
            "Dueto Sol & Luna - El Corrido del Río Bravo (sped up)",
            "Fan",
            150.0,
        ),
        // Theirs, but a minute longer: another take.
        found(
            "ddddddddddd",
            "Dueto Sol & Luna - El Corrido del Río Bravo",
            "Fan",
            240.0,
        ),
        // An upload naming them, the right length.
        found(
            "eeeeeeeeeee",
            "Dueto Sol y Luna - El Corrido del Río Bravo",
            "Someone",
            183.0,
        ),
        // Their own channel, the right length.
        found(
            "fffffffffff",
            "El Corrido del Río Bravo",
            "Dueto Sol & Luna",
            179.0,
        ),
    ];
    assert_eq!(choose(&wanted, &results).unwrap().id, "fffffffffff");
    // Without their channel's, the upload naming them ("y" for "&") is it; without that, none.
    assert_eq!(choose(&wanted, &results[..5]).unwrap().id, "eeeeeeeeeee");
    assert_eq!(choose(&wanted, &results[..4]), None);
    // As an old record's transfer is often titled: the duo first of several credited, their
    // name unaccented with "y", the title without its article or its bracketed translation,
    // the label's catalogue number after it, on a collector's channel, eight seconds longer.
    let credited = song(
        "El Corrido del Río Bravo (The Ballad of Rio Bravo)",
        "Dueto Sol & Luna & Mariachi Azul",
        Some(197),
    );
    let transfer = [
        found(
            "lllllllllll",
            "Dueto Mexico - Aqui Esta Rio Bravo",
            "Collection",
            197.0,
        ),
        found(
            "mmmmmmmmmmm",
            "Dueto Sol Y Luna - Corrido Del Rio Bravo - Label 1234-A",
            "Collection",
            205.0,
        ),
    ];
    assert_eq!(choose(&credited, &transfer).unwrap().id, "mmmmmmmmmmm");
    // The artist's "- Topic" channel (YouTube's copy of the release) beats their own.
    let topic = found(
        "ggggggggggg",
        "El Corrido del Río Bravo",
        "Dueto Sol & Luna - Topic",
        184.0,
    );
    let both = [results[5].clone(), topic];
    assert_eq!(choose(&wanted, &both).unwrap().id, "ggggggggggg");
    // A version the song's own title asks for is fine.
    let live = song("Creep (Live)", "Radiohead", None);
    let live_video = [found(
        "hhhhhhhhhhh",
        "Radiohead - Creep (Live)",
        "Radiohead",
        260.0,
    )];
    assert!(choose(&live, &live_video).is_some());
    // A song of unknown length: nothing longer than a quarter of an hour, nor of no length.
    let unknown = song("Creep", "Radiohead", None);
    let long = [found("iiiiiiiiiii", "Radiohead - Creep", "x", 3600.0)];
    assert_eq!(choose(&unknown, &long), None);
    let mut no_length = found("jjjjjjjjjjj", "Radiohead - Creep", "x", 0.0);
    no_length.duration = None;
    assert_eq!(choose(&unknown, &[no_length]), None);
    // A featured artist in the song's title isn't needed in the video's.
    let feat = song("Lovely (feat. Someone)", "Radiohead", Some(200));
    let plain = [found("kkkkkkkkkkk", "Radiohead - Lovely", "x", 201.0)];
    assert!(choose(&feat, &plain).is_some());
}

/// A music library in `music`, the setting on, and wanted song 2 (three seconds long), which
/// Soulseek was searched for long enough ago.
async fn rig(music: &Path) -> SqlitePool {
    let pool = db::open_in_memory().await;
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (3, 'music', 'music', ?1);
         INSERT INTO wanted_songs (id, title, artist, length_seconds, library_id, source, added_at)
             VALUES (2, 'Río Bravo', 'Dueto Sol & Luna', 3, 3, 'manual', 0);
         INSERT INTO searches (wanted_song_id, searched_at, indexers) VALUES (2, ?2, '[]');
         INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_SONG_YOUTUBE', 'on', 0);",
    )
    .bind(music.to_string_lossy().into_owned())
    .bind(NOW - SETTLED - 1)
    .execute(&pool)
    .await
    .unwrap();
    pool
}

fn fixes() -> Fixes {
    Fixes {
        tools: crate::sound::Tools {
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
        },
        coverart: None,
        lyrics: None,
    }
}

fn job(jobs: &Path, name: &str) -> Option<serde_json::Value> {
    let bytes = std::fs::read(jobs.join("queue").join(format!("{name}.json"))).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn answer(jobs: &Path, name: &str, body: serde_json::Value) {
    std::fs::create_dir_all(jobs.join("done")).unwrap();
    std::fs::write(
        jobs.join("done").join(format!("{name}.json")),
        body.to_string(),
    )
    .unwrap();
}

async fn state(pool: &SqlitePool) -> Option<(String, Option<String>)> {
    sqlx::query_as("SELECT state, message FROM song_youtube WHERE wanted_song_id = 2")
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn progress(pool: &SqlitePool) -> crate::progress::Progress {
    crate::progress::of(pool, "song", 2).await.unwrap().unwrap()
}

#[tokio::test]
async fn a_song_soulseek_hadnt_is_searched_chosen_downloaded_and_put_in_the_library() {
    let (music, jobs) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let pool = rig(music.path()).await;
    std::fs::create_dir_all(jobs.path().join("queue")).unwrap();
    // Before: Soulseek had nothing, and the song says so.
    let before = progress(&pool).await;
    assert_eq!(
        before.message.as_deref(),
        Some("Soulseek was searched and had nothing that fits")
    );

    step(&pool, jobs.path(), &fixes(), NOW).await.unwrap();
    let search = job(jobs.path(), "song-2-search").expect("a search queued");
    assert_eq!(search["kind"], "song_search");
    assert_eq!(search["query"], "Dueto Sol & Luna Río Bravo");
    assert_eq!(
        progress(&pool).await.message.as_deref(),
        Some("looking for it on YouTube")
    );
    // One at a time: nothing more is queued while it's under way.
    step(&pool, jobs.path(), &fixes(), NOW + 30).await.unwrap();

    std::fs::remove_file(jobs.path().join("queue/song-2-search.json")).unwrap();
    answer(
        jobs.path(),
        "song-2-search",
        json!({"id": "song-2-search", "ok": true, "results": [
            {"id": "aaaaaaaaaaa", "title": "Río Bravo (cover)", "channel": "Dueto Sol & Luna", "duration": 3},
            {"id": "bbbbbbbbbbb", "title": "Río Bravo", "channel": "Dueto Sol & Luna", "duration": 3},
        ]}),
    );
    step(&pool, jobs.path(), &fixes(), NOW + 60).await.unwrap();
    let audio = job(jobs.path(), "song-2-audio").expect("its audio asked for");
    assert_eq!(
        (&audio["kind"], &audio["video"]),
        (&json!("song"), &json!("bbbbbbbbbbb"))
    );
    assert_eq!(progress(&pool).await.step, Step::Downloading);

    // The downloader's audio: a real three-second Opus file.
    let clip = jobs.path().join("done/song-2-audio.opus");
    let made = std::process::Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=d=3",
            "-c:a",
            "libopus",
        ])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(made.success());
    // A file of the name there already stays as it is.
    let dir = music.path().join("Dueto Sol & Luna/Singles");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Río Bravo.opus"), b"someone else's").unwrap();
    answer(
        jobs.path(),
        "song-2-audio",
        json!({"id": "song-2-audio", "ok": true, "file": "song-2-audio.opus", "duration": 3.0,
               "codec": "opus", "bitrate": 128000}),
    );
    step(&pool, jobs.path(), &fixes(), NOW + 90).await.unwrap();
    let (state, _) = state(&pool).await.unwrap();
    assert_eq!(state, "imported");
    assert_eq!(
        std::fs::read(dir.join("Río Bravo.opus")).unwrap(),
        b"someone else's"
    );
    let placed = dir.join("Río Bravo (2).opus");
    assert!(placed.exists(), "put beside it under another name");
    assert!(!clip.exists(), "taken from the downloader's folder");
    let got = progress(&pool).await;
    assert_eq!(
        (got.step, got.source.as_deref()),
        (Step::InLibrary, Some("youtube"))
    );
    // Done: nothing more is queued for it.
    step(&pool, jobs.path(), &fixes(), NOW + 120).await.unwrap();
    assert!(job(jobs.path(), "song-2-search").is_none());
}

#[tokio::test]
async fn only_songs_soulseek_is_done_with_are_tried_and_only_while_on() {
    let (music, jobs) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let pool = rig(music.path()).await;
    std::fs::create_dir_all(jobs.path().join("queue")).unwrap();
    let queued = |jobs: &Path| job(jobs, "song-2-search").is_some();
    // Searched just now: the next source, or its album, may be on its way soon.
    step(&pool, jobs.path(), &fixes(), NOW - SETTLED)
        .await
        .unwrap();
    assert!(!queued(jobs.path()));
    // A Soulseek download on its way.
    sqlx::query(
        "INSERT INTO downloads (id, source, soulseek, title, wanted_song_id, decision, state,
             added_at, updated_at) VALUES (4, 'soulseek', '{}', 'x', 2, '{}', 'downloading', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    step(&pool, jobs.path(), &fixes(), NOW).await.unwrap();
    assert!(!queued(jobs.path()));
    // It failed (refused, say), and no next source followed: settled, it's tried.
    sqlx::query("UPDATE downloads SET state = 'failed', updated_at = ?")
        .bind(NOW - SETTLED - 1)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE settings SET value = 'off'")
        .execute(&pool)
        .await
        .unwrap();
    step(&pool, jobs.path(), &fixes(), NOW).await.unwrap();
    assert!(!queued(jobs.path()), "off: nothing goes to YouTube");
    sqlx::query("UPDATE settings SET value = 'on'")
        .execute(&pool)
        .await
        .unwrap();
    step(&pool, jobs.path(), &fixes(), NOW).await.unwrap();
    assert!(queued(jobs.path()));

    // No video fits: believed for a month.
    std::fs::remove_file(jobs.path().join("queue/song-2-search.json")).unwrap();
    answer(
        jobs.path(),
        "song-2-search",
        json!({"id": "song-2-search", "ok": true, "results": []}),
    );
    step(&pool, jobs.path(), &fixes(), NOW + 30).await.unwrap();
    assert_eq!(state(&pool).await.unwrap().0, "none");
    let got = progress(&pool).await;
    assert_eq!(got.step, Step::Failed);
    assert!(
        got.message
            .unwrap()
            .starts_with("not on Soulseek, and not on YouTube either"),
    );
    step(&pool, jobs.path(), &fixes(), NOW + 60).await.unwrap();
    assert!(!queued(jobs.path()));
    step(&pool, jobs.path(), &fixes(), NOW + 30 + NONE_FOR)
        .await
        .unwrap();
    assert!(queued(jobs.path()), "asked again a month later");
}

#[tokio::test]
async fn a_failure_or_a_lost_job_is_tried_again_a_day_later_three_times() {
    let (music, jobs) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let pool = rig(music.path()).await;
    std::fs::create_dir_all(jobs.path().join("queue")).unwrap();
    step(&pool, jobs.path(), &fixes(), NOW).await.unwrap();
    // The downloader never answers: a day on, it's given up on.
    step(&pool, jobs.path(), &fixes(), NOW + 86_400)
        .await
        .unwrap();
    let (state, message) = state(&pool).await.unwrap();
    assert_eq!(state, "failed");
    assert!(message.unwrap().contains("didn't answer"));
    // An answer to a job nothing waits for is dropped.
    answer(
        jobs.path(),
        "song-2-audio",
        json!({"id": "song-2-audio", "ok": true, "file": "song-2-audio.opus", "duration": 3.0}),
    );
    std::fs::write(jobs.path().join("done/song-2-audio.opus"), b"x").unwrap();
    step(&pool, jobs.path(), &fixes(), NOW + 86_401)
        .await
        .unwrap();
    assert!(!jobs.path().join("done/song-2-audio.opus").exists());
    assert!(!jobs.path().join("done/song-2-audio.json").exists());
    // Tried again a day after the failure; after three failures, no more.
    for attempt in 2..=3 {
        let at = NOW + attempt * 2 * 86_400;
        std::fs::remove_dir_all(jobs.path().join("queue")).unwrap();
        std::fs::create_dir_all(jobs.path().join("queue")).unwrap();
        step(&pool, jobs.path(), &fixes(), at).await.unwrap();
        assert!(job(jobs.path(), "song-2-search").is_some(), "try {attempt}");
        answer(
            jobs.path(),
            "song-2-search",
            json!({"id": "song-2-search", "ok": false, "error": "HTTP Error 429"}),
        );
        step(&pool, jobs.path(), &fixes(), at + 30).await.unwrap();
    }
    std::fs::remove_dir_all(jobs.path().join("queue")).unwrap();
    std::fs::create_dir_all(jobs.path().join("queue")).unwrap();
    step(&pool, jobs.path(), &fixes(), NOW + 30 * 86_400)
        .await
        .unwrap();
    assert!(
        job(jobs.path(), "song-2-search").is_none(),
        "three tries at most"
    );
}

#[tokio::test]
async fn audio_not_as_long_as_the_song_is_left_out() {
    let (music, jobs) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let pool = rig(music.path()).await;
    std::fs::create_dir_all(jobs.path().join("queue")).unwrap();
    step(&pool, jobs.path(), &fixes(), NOW).await.unwrap();
    answer(
        jobs.path(),
        "song-2-search",
        json!({"id": "song-2-search", "ok": true, "results": [
            {"id": "bbbbbbbbbbb", "title": "Río Bravo", "channel": "Dueto Sol & Luna", "duration": 3},
        ]}),
    );
    step(&pool, jobs.path(), &fixes(), NOW + 30).await.unwrap();
    std::fs::write(jobs.path().join("done/song-2-audio.opus"), b"x").unwrap();
    answer(
        jobs.path(),
        "song-2-audio",
        json!({"id": "song-2-audio", "ok": true, "file": "song-2-audio.opus", "duration": 95.0}),
    );
    step(&pool, jobs.path(), &fixes(), NOW + 60).await.unwrap();
    let (state, message) = state(&pool).await.unwrap();
    assert_eq!(state, "none");
    assert!(message.unwrap().contains("95 s long"));
    assert!(
        !music.path().join("Dueto Sol & Luna").exists(),
        "nothing put in the library"
    );
}

#[tokio::test]
async fn turned_off_while_searching_nothing_more_goes_to_youtube() {
    let (music, jobs) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let pool = rig(music.path()).await;
    std::fs::create_dir_all(jobs.path().join("queue")).unwrap();
    step(&pool, jobs.path(), &fixes(), NOW).await.unwrap();
    sqlx::query("UPDATE settings SET value = 'off'")
        .execute(&pool)
        .await
        .unwrap();
    answer(
        jobs.path(),
        "song-2-search",
        json!({"id": "song-2-search", "ok": true, "results": [
            {"id": "bbbbbbbbbbb", "title": "Río Bravo", "channel": "Dueto Sol & Luna", "duration": 3},
        ]}),
    );
    step(&pool, jobs.path(), &fixes(), NOW + 30).await.unwrap();
    assert!(
        job(jobs.path(), "song-2-audio").is_none(),
        "no download asked for"
    );
    assert_eq!(state(&pool).await, None, "as if never tried");
}

#[tokio::test]
async fn a_video_blocked_here_gives_way_to_the_next_that_fits() {
    let (music, jobs) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let pool = rig(music.path()).await;
    std::fs::create_dir_all(jobs.path().join("queue")).unwrap();
    step(&pool, jobs.path(), &fixes(), NOW).await.unwrap();
    answer(
        jobs.path(),
        "song-2-search",
        json!({"id": "song-2-search", "ok": true, "results": [
            {"id": "aaaaaaaaaaa", "title": "Río Bravo", "channel": "Dueto Sol & Luna", "duration": 3},
            {"id": "ccccccccccc", "title": "Dueto Sol y Luna - Río Bravo", "channel": "Fan", "duration": 3},
            {"id": "ddddddddddd", "title": "Something else", "channel": "Fan", "duration": 3},
        ]}),
    );
    step(&pool, jobs.path(), &fixes(), NOW + 30).await.unwrap();
    assert_eq!(
        job(jobs.path(), "song-2-audio").unwrap()["video"],
        "aaaaaaaaaaa"
    );
    std::fs::remove_file(jobs.path().join("queue/song-2-audio.json")).unwrap();
    // Blocked here: the next that fits is asked for, and the one that doesn't never is.
    answer(
        jobs.path(),
        "song-2-audio",
        json!({"id": "song-2-audio", "ok": true,
               "unavailable": "ERROR: [youtube] aaaaaaaaaaa: not made this video available in your country"}),
    );
    step(&pool, jobs.path(), &fixes(), NOW + 60).await.unwrap();
    assert_eq!(
        job(jobs.path(), "song-2-audio").unwrap()["video"],
        "ccccccccccc"
    );
    assert_eq!(progress(&pool).await.step, Step::Downloading);
    std::fs::remove_file(jobs.path().join("queue/song-2-audio.json")).unwrap();
    // That one blocked too: nothing that fits is left.
    answer(
        jobs.path(),
        "song-2-audio",
        json!({"id": "song-2-audio", "ok": true, "unavailable": "ERROR: Video unavailable"}),
    );
    step(&pool, jobs.path(), &fixes(), NOW + 90).await.unwrap();
    assert!(job(jobs.path(), "song-2-audio").is_none());
    let (state, message) = state(&pool).await.unwrap();
    assert_eq!(state, "none");
    assert!(
        message
            .unwrap()
            .starts_with("no other video that fits is left")
    );
}

#[test]
fn a_video_is_named_by_its_id_or_any_of_youtubes_links() {
    let id = "dQw4w9WgXcQ";
    for text in [
        id,
        " https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42 ",
        "https://youtu.be/dQw4w9WgXcQ?si=abc",
        "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
        "https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=x",
        "https://www.youtube.com/shorts/dQw4w9WgXcQ",
        "https://www.youtube.com/embed/dQw4w9WgXcQ",
    ] {
        assert_eq!(picks::video_id(text).as_deref(), Some(id), "{text}");
    }
    for text in [
        "",
        "dQw4w9WgXc",
        "https://example.com/watch?v=dQw4w9WgXcQ",
        "https://www.youtube.com/watch?v=short",
        "ftp://youtu.be/dQw4w9WgXcQ",
        "https://www.youtube.com/channel/dQw4w9WgXcQ",
    ] {
        assert_eq!(picks::video_id(text), None, "{text}");
    }
}

#[tokio::test]
async fn a_video_chosen_by_hand_is_fetched_as_the_song_whatever_its_length() {
    let (music, jobs) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let pool = rig(music.path()).await;
    std::fs::create_dir_all(jobs.path().join("queue")).unwrap();
    // The song came to nothing on YouTube.
    sqlx::query(
        "INSERT INTO song_youtube (wanted_song_id, state, message, started_at, updated_at)
         VALUES (2, 'none', 'none of YouTube''s 10 results has its title, artist and length', 0, ?)",
    )
    .bind(NOW)
    .execute(&pool)
    .await
    .unwrap();
    let picked = picks::pick(&pool, 2, "https://youtu.be/ddddddddddd", NOW)
        .await
        .unwrap();
    assert_eq!(picked, Ok("ddddddddddd".to_owned()));
    step(&pool, jobs.path(), &fixes(), NOW + 30).await.unwrap();
    let audio = job(jobs.path(), "song-2-audio").expect("its audio asked for");
    assert_eq!(audio["video"], "ddddddddddd");
    let clip = jobs.path().join("done/song-2-audio.opus");
    std::fs::create_dir_all(jobs.path().join("done")).unwrap();
    let made = std::process::Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=d=3",
            "-c:a",
            "libopus",
        ])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(made.success());
    // Far longer than the song: chosen by hand, it's the song anyway.
    answer(
        jobs.path(),
        "song-2-audio",
        json!({"id": "song-2-audio", "ok": true, "file": "song-2-audio.opus", "duration": 95.0}),
    );
    step(&pool, jobs.path(), &fixes(), NOW + 60).await.unwrap();
    assert_eq!(state(&pool).await.unwrap().0, "imported");
    let got = progress(&pool).await;
    assert_eq!(
        (got.step, got.source.as_deref()),
        (Step::InLibrary, Some("youtube"))
    );
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM song_youtube_picks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "taken");
}

#[tokio::test]
async fn a_video_chosen_by_hand_waits_for_what_is_under_way_and_only_while_on() {
    let (music, jobs) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let pool = rig(music.path()).await;
    std::fs::create_dir_all(jobs.path().join("queue")).unwrap();
    // Its own search is with the downloader: the pick waits for its answer.
    step(&pool, jobs.path(), &fixes(), NOW).await.unwrap();
    picks::pick(&pool, 2, "ddddddddddd", NOW)
        .await
        .unwrap()
        .unwrap();
    step(&pool, jobs.path(), &fixes(), NOW + 30).await.unwrap();
    assert!(job(jobs.path(), "song-2-audio").is_none());
    answer(
        jobs.path(),
        "song-2-search",
        json!({"id": "song-2-search", "ok": true, "results": []}),
    );
    step(&pool, jobs.path(), &fixes(), NOW + 60).await.unwrap();
    step(&pool, jobs.path(), &fixes(), NOW + 90).await.unwrap();
    assert_eq!(
        job(jobs.path(), "song-2-audio").unwrap()["video"],
        "ddddddddddd"
    );
    assert!(picks::by_hand(&pool, 2).await.unwrap());
    // A search made afterwards (the next automatic try) checks length again.
    sqlx::query("UPDATE song_youtube SET state = 'failed', updated_at = 0")
        .execute(&pool)
        .await
        .unwrap();
    std::fs::remove_dir_all(jobs.path().join("queue")).unwrap();
    std::fs::create_dir_all(jobs.path().join("queue")).unwrap();
    step(&pool, jobs.path(), &fixes(), NOW + 2 * 86_400)
        .await
        .unwrap();
    assert!(job(jobs.path(), "song-2-search").is_some());
    assert!(!picks::by_hand(&pool, 2).await.unwrap());
    // Off: nothing is chosen, and nothing waits.
    sqlx::query("UPDATE settings SET value = 'off'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        picks::pick(&pool, 2, "eeeeeeeeeee", NOW).await.unwrap(),
        Err(picks::NotPicked::Off)
    );
    assert_eq!(
        picks::pick(&pool, 99, "eeeeeeeeeee", NOW).await.unwrap(),
        Err(picks::NotPicked::NoSong)
    );
    assert_eq!(
        picks::pick(&pool, 2, "not a link", NOW).await.unwrap(),
        Err(picks::NotPicked::NotAVideo)
    );
}
