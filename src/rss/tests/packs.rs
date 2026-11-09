//! Season packs taken in part by an RSS sync.

use super::*;

#[tokio::test]
async fn a_pack_leaves_out_what_is_on_disk_or_on_its_way() {
    let pool = setup().await;
    let pack = "Some.Anime.S02.1080p.BluRay.x264-GRP";
    let (pipeline, _fake) = some_anime(
        &pool,
        &[(2, 6, "Some.Anime.S02E06.2160p.BluRay.REMUX.HEVC-GRP.mkv")],
        &[pack],
    )
    .await;
    // S02E07 is on its way.
    sqlx::query(
        "UPDATE tmdb_episodes SET air_date = '2020-01-01';
         INSERT INTO downloads (infohash, title, wanted_series_id, season, episode, decision,
             state, added_at, updated_at)
         VALUES ('a11bae70b2fdd9eb95b63e746f8594dce2650b0c', '[SubsPlease] Some Anime - 31', 1,
             2, 7, '{}', 'downloading', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let client = torznab::Client::new().unwrap();
    let view = sync(&pool, &client, None, Some(&pipeline))
        .await
        .unwrap()
        .unwrap();
    let grabbed: Vec<(&str, bool, bool)> = view
        .grabbed
        .iter()
        .map(|g| (g.title.as_str(), g.upgrade, g.download_id.is_some()))
        .collect();
    assert_eq!(grabbed, [(pack, false, true)]);
    let (episode, episodes, replaces): (Option<i64>, String, String) =
        sqlx::query_as("SELECT episode, episodes, replaces FROM downloads WHERE title = ?")
            .bind(pack)
            .fetch_one(&pool)
            .await
            .unwrap();
    let episodes: Vec<i64> = serde_json::from_str(&episodes).unwrap();
    let expected: Vec<i64> = (1..=23).filter(|e| ![6, 7].contains(e)).collect();
    assert_eq!((episode, episodes, replaces), (None, expected, "[]".into()));
}

#[tokio::test]
async fn a_pack_taken_in_part_holds_up_only_its_own_episodes() {
    let pool = setup().await;
    let (pipeline, _fake) = some_anime(
        &pool,
        &[],
        &[
            "[SubsPlease] Some Anime - 25 (1080p) [ABCD1234].mkv",
            "[SubsPlease] Some Anime - 31 (1080p) [ABCD1234].mkv",
        ],
    )
    .await;
    // A pack of season 2 is on its way for its first episodes only.
    sqlx::query(
        "INSERT INTO downloads (infohash, title, wanted_series_id, season, decision, state,
             added_at, updated_at, episodes)
         VALUES ('a11bae70b2fdd9eb95b63e746f8594dce2650b0c', 'Some.Anime.S02.1080p.BluRay', 1, 2,
             '{}', 'downloading', 0, 0, '[1, 2, 3]')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let client = torznab::Client::new().unwrap();
    let view = sync(&pool, &client, None, Some(&pipeline))
        .await
        .unwrap()
        .unwrap();
    // The 25th is S02E01, on its way; the 31st, S02E07, isn't.
    let titles: Vec<&str> = view.grabbed.iter().map(|g| g.title.as_str()).collect();
    assert_eq!(
        titles,
        ["[SubsPlease] Some Anime - 31 (1080p) [ABCD1234].mkv"]
    );
}

#[tokio::test]
async fn a_pack_isnt_taken_in_part_without_the_seasons_episode_list() {
    let pool = setup().await;
    let pack = "Some.Anime.S02.1080p.BluRay.x264-GRP";
    let (pipeline, _fake) = some_anime(
        &pool,
        &[(2, 6, "Some.Anime.S02E06.2160p.BluRay.REMUX.HEVC-GRP.mkv")],
        &[pack],
    )
    .await;
    // TMDB lists the episodes but no air dates: which have aired isn't known.
    let client = torznab::Client::new().unwrap();
    let view = sync(&pool, &client, None, Some(&pipeline))
        .await
        .unwrap()
        .unwrap();
    assert!(view.grabbed.is_empty(), "{:?}", view.grabbed);
    let wanted = search::load_wanted(
        &pool,
        Target::Series {
            id: 1,
            season: 2,
            episode: None,
        },
    )
    .await
    .unwrap()
    .unwrap();
    let target = Target::Series {
        id: 1,
        season: 2,
        episode: None,
    };
    let why = grab_kind(&pool, target, &wanted, pack, &serde_json::json!({}))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(
        why,
        "season 2's episode list isn't known, so which of its episodes are missing can't be told"
    );
}
