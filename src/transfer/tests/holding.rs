//! What a download held back or failed leaves behind: never a part file or a folder it made, and
//! never a partly placed pack without saying so; and what passes the running-time check.

use super::verifying::{clip, fixes};
use super::*;

/// A wanted series "New Show" in the shows library, unmonitored, whose season 1 episodes run 45
/// minutes on TMDB.
async fn new_show(r: &Rig) -> i64 {
    let show: i64 = sqlx::query_scalar(
        "INSERT INTO wanted_series (tvdb_id, tmdb_id, title, monitored, series_type, source,
             added_at, library_id)
         VALUES (9, 90, 'New Show', 0, 'standard', 'sonarr', 0, 2) RETURNING id",
    )
    .fetch_one(&r.pool)
    .await
    .unwrap();
    sqlx::raw_sql(
        "INSERT INTO tmdb_series (tmdb_id, name, fetched_at) VALUES (90, 'New Show', 0);
         INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, runtime_minutes)
             VALUES (90, 1, 1, 901, 45), (90, 1, 2, 902, 45);",
    )
    .execute(&r.pool)
    .await
    .unwrap();
    show
}

fn shows(r: &Rig) -> Vec<String> {
    tree(&r.local.path().join("shows"))
}

#[tokio::test]
async fn a_wrong_film_with_extras_doesnt_pass_as_all_videos_together() {
    let r = rig().await;
    let wanted = r.want_movie(None).await;
    r.runtime(wanted, 170).await;
    // 92 and 80 minutes add up to the film's 170, but neither is it.
    let files = r.seed(&[
        ("Some.Movie.2024/Some.Movie.2024.mkv", clip(92 * 60)),
        ("Some.Movie.2024/Some.Movie.2024.Extras.mkv", clip(80 * 60)),
    ]);
    let id = r
        .movie_download(wanted, "Some.Movie.2024.1080p.WEB-DL", &files)
        .await;

    transfer(&r.pool, &r.seedbox, Some(&fixes()), id)
        .await
        .unwrap();

    let (state, message, ..) = r.state(id).await;
    let message = message.unwrap();
    assert_eq!(state, "failed", "{message}");
    assert!(
        message.contains("runs 1 h 32 m; the film runs 2 h 50 m"),
        "{message}"
    );
    assert!(r.movies().is_empty(), "{:?}", r.movies());
}

#[tokio::test]
async fn pack_files_that_dont_say_their_episode_are_held_back() {
    let r = rig().await;
    let show = new_show(&r).await;
    let files = r.seed(&[
        ("New.Show.S01/one.mkv", clip(45 * 60)),
        ("New.Show.S01/two.mkv", clip(45 * 60)),
    ]);
    let id = r
        .download(None, Some((show, 1)), "/home/user/Completed", &files)
        .await;

    transfer(&r.pool, &r.seedbox, Some(&fixes()), id)
        .await
        .unwrap();

    let (state, message, ..) = r.state(id).await;
    let message = message.unwrap();
    assert_eq!(state, "failed", "{message}");
    assert!(
        message.contains("one.mkv's name doesn't say which episode of season 1 it is"),
        "{message}"
    );
    assert_eq!(shows(&r), [".keep"]);
}

#[tokio::test]
async fn a_partly_placed_pack_that_proves_wrong_is_held_for_an_admin() {
    let r = rig().await;
    let show = new_show(&r).await;
    let episode_one = clip(45 * 60);
    let files = r.seed(&[
        ("New.Show.S01/New.Show.S01E01.mkv", episode_one.clone()),
        ("New.Show.S01/New.Show.S01E02.mkv", clip(120)),
    ]);
    let id = r
        .download(None, Some((show, 1)), "/home/user/Completed", &files)
        .await;
    // An earlier try put episode 1 in place before it stopped.
    let season = r.local.path().join("shows/New Show/Season 01");
    std::fs::create_dir_all(&season).unwrap();
    let placed = season.join("New.Show.S01E01.mkv");
    std::fs::write(&placed, &episode_one).unwrap();
    sqlx::query("UPDATE downloads SET placed = json_array(?) WHERE id = ?")
        .bind(placed.to_string_lossy())
        .bind(id)
        .execute(&r.pool)
        .await
        .unwrap();

    let again = transfer(&r.pool, &r.seedbox, Some(&fixes()), id)
        .await
        .unwrap();

    let (state, message, ..) = r.state(id).await;
    let message = message.unwrap();
    assert_eq!(state, "failed", "{message}");
    assert!(
        message.contains("New.Show.S01E02.mkv runs 2 m; episode 2 runs 45 m")
            && message.contains(&format!(
                "an earlier try already put {} in the library, so it's held for an admin to check it",
                placed.display()
            )),
        "{message}"
    );
    assert_eq!(again, None, "not searched again while its files are there");
    assert_eq!(
        shows(&r),
        [".keep", "New Show/Season 01/New.Show.S01E01.mkv"],
        "episode 2 isn't placed, and no part file is left"
    );
    let blocked: Vec<String> = sqlx::query_scalar("SELECT title FROM blocklist")
        .fetch_all(&r.pool)
        .await
        .unwrap();
    assert_eq!(blocked, ["Some.Release"]);
}

#[tokio::test]
async fn held_back_it_removes_every_folder_it_made_across_tries_and_none_from_before() {
    let r = rig().await;
    let show = new_show(&r).await;
    // The show's folder was there, empty, before the download.
    std::fs::create_dir_all(r.local.path().join("shows/New Show")).unwrap();
    let files = r.seed(&[("New.Show.S01E02.mkv", clip(120))]);
    let id = r
        .download(None, Some((show, 1)), "/home/user/Completed", &files)
        .await;
    // The first try makes the season folder and copies, but can't check: no ffprobe.
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    let (state, message, ..) = r.state(id).await;
    assert_eq!(state, "completed", "{message:?}");
    assert!(r.local.path().join("shows/New Show/Season 01").is_dir());

    // The next try checks it, and it isn't the episode.
    transfer(&r.pool, &r.seedbox, Some(&fixes()), id)
        .await
        .unwrap();

    assert_eq!(r.state(id).await.0, "failed");
    assert!(
        r.local.path().join("shows/New Show").is_dir(),
        "the folder that was there before stays"
    );
    assert!(
        !r.local.path().join("shows/New Show/Season 01").exists(),
        "the season folder it made is gone"
    );
    assert_eq!(shows(&r), [".keep"], "and no part file is left");
}

#[tokio::test]
async fn a_failure_after_copying_leaves_no_part_files() {
    let r = rig().await;
    let wanted = r.want_movie(None).await;
    let dir = r.local.path().join("movies/Some Movie (2024)");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Some.Movie.2024.en.srt"), b"someone else's").unwrap();
    // The video copies; the subtitles' name is taken, which fails the download.
    let files = r.seed(&[
        ("Some.Movie.2024/Some.Movie.2024.mkv", data(300_000, 5)),
        ("Some.Movie.2024/Some.Movie.2024.en.srt", b"1\n".to_vec()),
    ]);
    let id = r
        .movie_download(wanted, "Some.Movie.2024.1080p.WEB-DL", &files)
        .await;

    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();

    let (state, message, ..) = r.state(id).await;
    assert_eq!(state, "failed", "{message:?}");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["Some.Movie.2024.en.srt"],
        "the video's part file is gone"
    );
}

#[tokio::test]
async fn after_three_failed_checks_it_stops_searching_until_the_runtime_is_said_right() {
    let r = rig().await;
    let wanted = r.want_movie(None).await;
    r.runtime(wanted, 170).await;
    let sources = crate::fulfil::Sources::default();
    let film = clip(92 * 60);
    let mut said = Vec::new();
    for n in 1..=3 {
        let release = format!("Some.Movie.2024.1080p.WEB-DL-GRP{n}");
        let files = r.seed(&[(&format!("{release}/{release}.mkv"), film.clone())]);
        let id = r.movie_download(wanted, &release, &files).await;
        bring_home(&r.pool, &r.seedbox, Some(&fixes()), &sources, id)
            .await
            .unwrap();
        let (state, message, ..) = r.state(id).await;
        assert_eq!(state, "failed");
        said.push(message.unwrap());
    }
    assert!(said[1].ends_with("downloads are off, so it wasn't searched for again"));
    assert!(
        said[2].ends_with(
            "3 downloads of it didn't pass the running-time check, so Melete has stopped searching for it by itself; if its runtime is right after all, an admin can say so on its page"
        ),
        "{}",
        said[2]
    );

    // An admin says the runtime is right: the next one, as long as the last, is placed.
    let title = crate::search::Title::Movie(wanted);
    crate::runtime_checks::say_right(&r.pool, title)
        .await
        .unwrap();
    let release = "Some.Movie.2024.1080p.WEB-DL-GRP4";
    let files = r.seed(&[(&format!("{release}/{release}.mkv"), film)]);
    let id = r.movie_download(wanted, release, &files).await;
    bring_home(&r.pool, &r.seedbox, Some(&fixes()), &sources, id)
        .await
        .unwrap();
    let (state, message, ..) = r.state(id).await;
    assert_eq!(state, "imported", "{message:?}");
    let now = crate::runtime_checks::status(&r.pool, title)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((now.failures, now.overridden), (0, true));
}
