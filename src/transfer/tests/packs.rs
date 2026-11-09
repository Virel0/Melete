//! Season packs taken in part: only the episodes they were taken for come home.

use super::*;

/// "Show" with season 2 wanted, in the shows library, and a completed download of its season 2
/// pack holding `files`, taken for `episodes` (None for the whole pack).
async fn pack(r: &Rig, files: &[(&str, Vec<u8>)], episodes: Option<&str>) -> i64 {
    sqlx::raw_sql(
        "INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (3, 2, 'Show', 'Show', 1);
         INSERT INTO wanted_series (id, tvdb_id, title, year, monitored, series_type, source,
                 added_at, library_id, series_id)
             VALUES (1, 1, 'Show', 2020, 1, 'standard', 'sonarr', 0, 2, 3)",
    )
    .execute(&r.pool)
    .await
    .unwrap();
    let listed = r.seed(files);
    let id = r
        .download(None, Some((1, 2)), "/home/user/Completed", &listed)
        .await;
    sqlx::query("UPDATE downloads SET episodes = ? WHERE id = ?")
        .bind(episodes)
        .bind(id)
        .execute(&r.pool)
        .await
        .unwrap();
    id
}

/// What's in the season's folder.
fn season_folder(r: &Rig) -> Vec<String> {
    let dir = r.local.path().join("shows/Show/Season 02");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn only_the_episodes_a_pack_was_taken_for_come_home() {
    let r = rig().await;
    let dir = "Show.S02.1080p.BluRay.x264-GRP";
    let id = pack(
        &r,
        &[
            (&format!("{dir}/Show.S02E01.mkv"), data(100, 1)),
            (&format!("{dir}/Show.S02E02.mkv"), data(100, 2)),
            (&format!("{dir}/Show.S02E03.mkv"), data(100, 3)),
            (&format!("{dir}/Show.S02E03.en.srt"), b"1\n".to_vec()),
            (&format!("{dir}/Show.S02E03E04.mkv"), data(100, 4)),
            // Nothing in its name says which episode it is.
            (&format!("{dir}/Bonus.mkv"), data(100, 5)),
            (
                &format!("{dir}/Extras/Show.S02E01.Making.Of.mkv"),
                data(100, 6),
            ),
        ],
        Some("[1, 3]"),
    )
    .await;
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    let (state, ..) = r.state(id).await;
    assert_eq!(state, "imported");
    assert_eq!(
        season_folder(&r),
        ["Show.S02E01.mkv", "Show.S02E03.en.srt", "Show.S02E03.mkv"]
    );
}

#[tokio::test]
async fn a_pack_none_of_whose_files_can_be_placed_brings_nothing_home() {
    let r = rig().await;
    let id = pack(
        &r,
        &[
            ("Show.S02.1080p.BluRay.x264-GRP/01.mkv", data(100, 1)),
            (
                "Show.S02.1080p.BluRay.x264-GRP/Show.S02E02.mkv",
                data(100, 2),
            ),
        ],
        Some("[1]"),
    )
    .await;
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    let (state, message, destination, _) = r.state(id).await;
    assert_eq!((state.as_str(), destination), ("failed", None));
    assert_eq!(
        message.as_deref(),
        Some(
            "none of its files can be told to be one of the episodes it was taken for (1 of \
             season 2)"
        )
    );
    assert!(!r.local.path().join("shows/Show/Season 02").exists());
}

/// Names `id`'s release `title`.
async fn titled(r: &Rig, id: i64, title: &str) {
    sqlx::query("UPDATE downloads SET title = ? WHERE id = ?")
        .bind(title)
        .bind(id)
        .execute(&r.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_pack_of_several_seasons_brings_home_only_the_season_it_was_taken_for() {
    let r = rig().await;
    let dir = "Show.S01-S03.1080p.BluRay.x264-GRP";
    let id = pack(
        &r,
        &[
            (&format!("{dir}/Season 1/Show.S01E01.mkv"), data(100, 1)),
            (&format!("{dir}/Season 2/Show.S02E01.mkv"), data(100, 2)),
            (&format!("{dir}/Season 2/Show.S02E02.mkv"), data(100, 3)),
            (
                &format!("{dir}/Season 2/Show.S02E02.en.srt"),
                b"1\n".to_vec(),
            ),
            (&format!("{dir}/Season 3/Show.S03E01.mkv"), data(100, 4)),
            // Which season it's of can't be told.
            (&format!("{dir}/Bonus.mkv"), data(100, 5)),
        ],
        None,
    )
    .await;
    titled(&r, id, dir).await;
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    let (state, ..) = r.state(id).await;
    assert_eq!(state, "imported");
    assert_eq!(
        season_folder(&r),
        ["Show.S02E01.mkv", "Show.S02E02.en.srt", "Show.S02E02.mkv"]
    );
    assert!(!r.local.path().join("shows/Show/Season 01").exists());
    assert!(!r.local.path().join("shows/Show/Season 03").exists());
}

#[tokio::test]
async fn a_pack_of_several_seasons_without_the_season_brings_nothing_home() {
    let r = rig().await;
    let dir = "Show.Complete.Series.1080p.BluRay.x264-GRP";
    let id = pack(
        &r,
        &[
            (&format!("{dir}/Show.S01E01.mkv"), data(100, 1)),
            (&format!("{dir}/Show.S03E01.mkv"), data(100, 2)),
        ],
        None,
    )
    .await;
    titled(&r, id, dir).await;
    transfer(&r.pool, &r.seedbox, None, id).await.unwrap();
    let (state, message, destination, _) = r.state(id).await;
    assert_eq!((state.as_str(), destination), ("failed", None));
    assert_eq!(
        message.as_deref(),
        Some(
            "it holds several seasons, and none of its files can be told to be an episode of \
             season 2, the one it was taken for"
        )
    );
}

#[tokio::test]
async fn a_pack_of_several_seasons_is_checked_against_its_own_season_only() {
    let r = rig().await;
    let show = super::holding::new_show(&r).await;
    let dir = "New.Show.S01-S02.1080p.WEB-DL-GRP";
    let files = r.seed(&[
        (
            &format!("{dir}/New.Show.S01E01.mkv"),
            super::verifying::clip(45 * 60),
        ),
        (
            &format!("{dir}/New.Show.S01E02.mkv"),
            super::verifying::clip(45 * 60),
        ),
        // Season 2's runs otherwise; it stays on the seedbox and isn't judged.
        (
            &format!("{dir}/New.Show.S02E01.mkv"),
            super::verifying::clip(20 * 60),
        ),
    ]);
    let id = r
        .download(None, Some((show, 1)), "/home/user/Completed", &files)
        .await;
    titled(&r, id, dir).await;

    transfer(&r.pool, &r.seedbox, Some(&super::verifying::fixes()), id)
        .await
        .unwrap();

    let (state, message, ..) = r.state(id).await;
    assert_eq!(state, "imported", "{message:?}");
    assert_eq!(
        super::holding::shows(&r),
        [
            ".keep",
            "New Show/Season 01/New.Show.S01E01.mkv",
            "New Show/Season 01/New.Show.S01E02.mkv"
        ]
    );
}
