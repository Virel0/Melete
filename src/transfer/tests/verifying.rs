//! A movie or episode is checked against TMDB's runtime before it's put in place.

use super::*;
use crate::deluge;
use crate::downloads::Fetcher;
use crate::profiles::{self, App};
use crate::rss::tests::{indexer, releases, want_dune};

/// A `seconds`-long video, tiny and quick to make: 32x32 black at one frame a second.
pub(super) fn clip(seconds: u32) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mkv");
    let status = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!(
            "color=c=black:size=32x32:rate=1:duration={seconds}"
        ))
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&path)
        .status()
        .expect("ffmpeg is installed");
    assert!(status.success());
    std::fs::read(path).unwrap()
}

pub(super) fn fixes() -> Fixes {
    Fixes {
        tools: crate::sound::Tools {
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
        },
        coverart: None,
        lyrics: None,
    }
}

impl Rig {
    /// A finished download of `title` for wanted movie `movie`, holding `files`.
    pub(super) async fn movie_download(&self, movie: i64, title: &str, files: &str) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO downloads (infohash, title, wanted_movie_id, decision, state, save_path,
                 files, added_at, updated_at, completed_at)
             VALUES (lower(hex(randomblob(20))), ?, ?, '{}', 'completed', '/home/user/Completed',
                 ?, 0, 0, 0)
             RETURNING id",
        )
        .bind(title)
        .bind(movie)
        .bind(files)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    /// TMDB's runtime for the wanted movie, in minutes.
    pub(super) async fn runtime(&self, movie: i64, minutes: i64) {
        sqlx::query(
            "INSERT INTO tmdb_movies (tmdb_id, title, runtime_minutes, fetched_at)
             SELECT tmdb_id, title, ?, 0 FROM wanted_movies WHERE id = ?",
        )
        .bind(minutes)
        .bind(movie)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    pub(super) async fn blocklisted(&self) -> Vec<String> {
        sqlx::query_scalar("SELECT title FROM blocklist ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .unwrap()
    }

    /// What's in the movies library besides the marker that it's mounted.
    pub(super) fn movies(&self) -> Vec<String> {
        tree(&self.local.path().join("movies"))
            .into_iter()
            .filter(|f| f != ".keep")
            .collect()
    }
}

#[tokio::test]
async fn a_release_running_another_length_is_held_back_blocklisted_and_searched_again() {
    let r = rig().await;
    for app in [App::Radarr, App::Sonarr] {
        let converted = crate::profiles::import::tests::converted(app);
        profiles::save(&r.pool, app, &converted).await.unwrap();
    }
    let dune = want_dune(&r.pool).await;
    sqlx::query("UPDATE wanted_movies SET library_id = 1 WHERE id = ?")
        .bind(dune)
        .execute(&r.pool)
        .await
        .unwrap();
    r.runtime(dune, 170).await;
    let fake = "Dune.Part.Two.2024.1080p.WEBRip.x265-FAKE";
    let good = "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR";
    // The right name, and a 92-minute film inside.
    let files = r.seed(&[(&format!("{fake}/{fake}.mkv"), clip(92 * 60))]);
    let id = r.movie_download(dune, fake, &files).await;
    let feed = indexer(&r.pool, "[2000]").await;
    feed.lock().unwrap().0 = releases(&[(fake, ""), (good, "")]);
    let (url, deluge) = deluge::testing::start().await;
    let sources = crate::fulfil::Sources {
        pipeline: Some(Arc::new(Pipeline {
            deluge: deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
            fetcher: Fetcher::new().unwrap(),
            seedbox: None,
        })),
        ..Default::default()
    };

    bring_home(&r.pool, &r.seedbox, Some(&fixes()), &sources, id)
        .await
        .unwrap();

    let (state, message, destination, _) = r.state(id).await;
    let message = message.unwrap();
    assert_eq!(state, "failed", "{message}");
    assert!(
        message.contains(&format!("{fake}.mkv runs 1 h 32 m; the film runs 2 h 50 m")),
        "{message}"
    );
    assert!(message.contains("blocklisted"), "{message}");
    assert!(
        message.ends_with(&format!("searched again and grabbed {good}")),
        "{message}"
    );
    assert_eq!(destination, None);
    assert!(
        r.movies().is_empty(),
        "no file, part file or new folder in the library: {:?}",
        r.movies()
    );
    assert_eq!(r.blocklisted().await, [fake]);
    let grabbed: Vec<String> =
        sqlx::query_scalar("SELECT title FROM downloads WHERE id <> ? ORDER BY id")
            .bind(id)
            .fetch_all(&r.pool)
            .await
            .unwrap();
    assert_eq!(grabbed, [good], "the fake is passed over");
    assert_eq!(deluge.lock().unwrap().torrents.len(), 1);
}

#[tokio::test]
async fn a_release_running_about_as_long_as_the_film_is_placed() {
    let r = rig().await;
    let wanted = r.want_movie(None).await;
    // 8% short of TMDB's runtime: a cut without the long credits, or PAL speed-up.
    r.runtime(wanted, 100).await;
    let movie = clip(92 * 60);
    let files = r.seed(&[("Some.Movie.2024/Some.Movie.2024.mkv", movie.clone())]);
    let id = r
        .movie_download(wanted, "Some.Movie.2024.1080p.WEB-DL", &files)
        .await;

    transfer(&r.pool, &r.seedbox, Some(&fixes()), id)
        .await
        .unwrap();

    let (state, message, ..) = r.state(id).await;
    assert_eq!(state, "imported", "{message:?}");
    assert_eq!(r.movies(), ["Some Movie (2024)/Some.Movie.2024.mkv"]);
    assert_eq!(
        std::fs::read(
            r.local
                .path()
                .join("movies/Some Movie (2024)/Some.Movie.2024.mkv")
        )
        .unwrap(),
        movie
    );
    assert!(r.blocklisted().await.is_empty());
}

#[tokio::test]
async fn a_release_whose_running_time_cant_be_read_is_held_back() {
    let r = rig().await;
    let wanted = r.want_movie(None).await;
    sqlx::query("UPDATE wanted_movies SET monitored = 0")
        .execute(&r.pool)
        .await
        .unwrap();
    r.runtime(wanted, 100).await;
    let files = r.seed(&[
        ("Some.Movie.2024/Some.Movie.2024.mkv", data(300_000, 4)),
        (
            "Some.Movie.2024/Some.Movie.2024.en.srt",
            b"1\n00:00:01,000 --> 00:00:02,000\nHi\n".to_vec(),
        ),
    ]);
    let id = r
        .movie_download(wanted, "Some.Movie.2024.1080p.WEB-DL", &files)
        .await;

    bring_home(
        &r.pool,
        &r.seedbox,
        Some(&fixes()),
        &crate::fulfil::Sources::default(),
        id,
    )
    .await
    .unwrap();

    let (state, message, ..) = r.state(id).await;
    let message = message.unwrap();
    assert_eq!(state, "failed", "{message}");
    assert!(
        message.contains("couldn't read how long Some.Movie.2024.mkv runs"),
        "{message}"
    );
    assert!(
        message.ends_with("it isn't monitored, so it wasn't searched for again"),
        "{message}"
    );
    assert!(
        r.movies().is_empty(),
        "neither the video nor its subtitles are placed: {:?}",
        r.movies()
    );
    assert_eq!(r.blocklisted().await, ["Some.Movie.2024.1080p.WEB-DL"]);
}

#[tokio::test]
async fn an_episode_running_another_length_is_held_back() {
    let r = rig().await;
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
    // Episode 1 runs as long as TMDB says; episode 2 is a two-minute clip.
    let files = r.seed(&[
        ("New.Show.S01/New.Show.S01E01.mkv", clip(43 * 60)),
        ("New.Show.S01/New.Show.S01E02.mkv", clip(120)),
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
        message.contains("New.Show.S01E02.mkv runs 2 m; episode 2 runs 45 m"),
        "{message}"
    );
    assert_eq!(
        tree(&r.local.path().join("shows")),
        [".keep"],
        "neither episode is placed"
    );
}
