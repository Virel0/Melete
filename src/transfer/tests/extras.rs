//! A pack's extras (creditless songs, trailers, featurettes) go in the title's Extras folder,
//! passed over by the running-time check; the title's own videos are still checked.

use super::holding::{new_show, shows};
use super::verifying::{clip, fixes};
use super::*;

#[tokio::test]
async fn a_season_pack_with_creditless_songs_imports_them_as_extras() {
    let r = rig().await;
    let show = new_show(&r).await;
    let files = r.seed(&[
        (
            "New.Show.S01/New Show - S01E01 (BD 1080p).mkv",
            clip(45 * 60),
        ),
        (
            "New.Show.S01/New Show - S01E02 (BD 1080p).mkv",
            clip(45 * 60),
        ),
        ("New.Show.S01/New Show - S01 NCED (BD 1080p).mkv", clip(90)),
        (
            "New.Show.S01/New Show - S01 NCED (BD 1080p).ass",
            b"[Script Info]".to_vec(),
        ),
        ("New.Show.S01/NC/New Show - Opening.mkv", clip(90)),
    ]);
    let id = r
        .download(None, Some((show, 1)), "/home/user/Completed", &files)
        .await;

    transfer(&r.pool, &r.seedbox, Some(&fixes()), id)
        .await
        .unwrap();

    let (state, message, ..) = r.state(id).await;
    assert_eq!(state, "imported", "{message:?}");
    assert_eq!(
        shows(&r),
        [
            ".keep",
            "New Show/Season 01/Extras/New Show - Opening.mkv",
            "New Show/Season 01/Extras/New Show - S01 NCED (BD 1080p).ass",
            "New Show/Season 01/Extras/New Show - S01 NCED (BD 1080p).mkv",
            "New Show/Season 01/New Show - S01E01 (BD 1080p).mkv",
            "New Show/Season 01/New Show - S01E02 (BD 1080p).mkv",
        ]
    );
}

#[tokio::test]
async fn the_episodes_beside_extras_are_still_checked() {
    let r = rig().await;
    let show = new_show(&r).await;
    let files = r.seed(&[
        ("New.Show.S01/New Show - S01E01.mkv", clip(45 * 60)),
        ("New.Show.S01/New Show - S01E02.mkv", clip(120)),
        ("New.Show.S01/New Show - NCOP1.mkv", clip(90)),
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
        message.contains("New Show - S01E02.mkv runs 2 m; episode 2 runs 45 m"),
        "{message}"
    );
    assert_eq!(shows(&r), [".keep"]);
}

#[tokio::test]
async fn a_download_of_only_extras_is_refused() {
    let r = rig().await;
    let show = new_show(&r).await;
    let files = r.seed(&[
        ("New.Show.S01.Extras/New Show - NCOP.mkv", clip(90)),
        ("New.Show.S01.Extras/New Show - NCED.mkv", clip(90)),
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
    assert!(message.contains("holds only extras"), "{message}");
    assert_eq!(shows(&r), [".keep"]);
}

#[tokio::test]
async fn a_movies_largest_video_is_the_film_whatever_its_name() {
    let r = rig().await;
    let wanted = r.want_movie(None).await;
    r.runtime(wanted, 100).await;
    let files = r.seed(&[
        (
            "Trailer.Park.Movie.2024/Trailer.Park.Movie.2024.mkv",
            clip(100 * 60),
        ),
        (
            "Trailer.Park.Movie.2024/Featurettes/Making of.mkv",
            clip(300),
        ),
    ]);
    let id = r
        .movie_download(wanted, "Trailer.Park.Movie.2024.1080p.WEB-DL", &files)
        .await;

    transfer(&r.pool, &r.seedbox, Some(&fixes()), id)
        .await
        .unwrap();

    let (state, message, ..) = r.state(id).await;
    assert_eq!(state, "imported", "{message:?}");
    assert_eq!(
        r.movies(),
        [
            "Some Movie (2024)/Extras/Making of.mkv",
            "Some Movie (2024)/Trailer.Park.Movie.2024.mkv",
        ]
    );
}
