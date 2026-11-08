use super::*;
use crate::db;

const GROUP: &str = "b1392450-e666-3926-a536-22c65f834433";
const OTHER: &str = "2a0981fb-9593-3019-864b-ce934d97a16e";

async fn set(pool: &SqlitePool, name: &str, on: bool) {
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES (?, ?, 0)
         ON CONFLICT (name) DO UPDATE SET value = excluded.value",
    )
    .bind(name)
    .bind(if on { "on" } else { "off" })
    .execute(pool)
    .await
    .unwrap();
}

async fn turn(pool: &SqlitePool, on: bool) {
    set(pool, "MELETE_MUSICBRAINZ", true).await;
    set(pool, "MELETE_SEARCH_COVERS", on).await;
}

async fn noted(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM search_covers")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn offered_only_while_on_with_musicbrainz_and_for_real_ids() {
    let pool = db::open_in_memory().await;
    let all = Access::everything();
    let now = 100 * 24 * 3600;
    assert_eq!(offer(&pool, &all, Some(GROUP), now).await.unwrap(), None);
    assert_eq!(noted(&pool).await, 0, "off by default: nothing noted");
    set(&pool, "MELETE_SEARCH_COVERS", true).await;
    assert_eq!(
        offer(&pool, &all, Some(GROUP), now).await.unwrap(),
        None,
        "needs MusicBrainz on"
    );
    turn(&pool, true).await;
    assert_eq!(
        offer(&pool, &all, Some(GROUP), now)
            .await
            .unwrap()
            .as_deref(),
        Some("/api/v1/music/release-groups/b1392450-e666-3926-a536-22c65f834433/cover")
    );
    assert!(servable_to(&pool, &all, GROUP, now).await.unwrap());
    assert!(
        !servable_to(&pool, &all, OTHER, now).await.unwrap(),
        "never listed"
    );
    assert_eq!(
        offer(&pool, &all, Some("../../x"), now).await.unwrap(),
        None
    );
    assert_eq!(offer(&pool, &all, None, now).await.unwrap(), None);
    assert!(!servable_to(&pool, &all, "../../x", now).await.unwrap());
    turn(&pool, false).await;
    assert!(
        !servable_to(&pool, &all, GROUP, now).await.unwrap(),
        "off: not served"
    );
}

#[tokio::test]
async fn one_the_archive_lacks_isnt_offered_for_a_day() {
    let pool = db::open_in_memory().await;
    turn(&pool, true).await;
    let all = Access::everything();
    let now = 100 * 24 * 3600;
    offer(&pool, &all, Some(GROUP), now).await.unwrap();
    lacking(&pool, GROUP, now).await.unwrap();
    assert_eq!(
        offer(&pool, &all, Some(GROUP), now + 60).await.unwrap(),
        None
    );
    assert!(!servable_to(&pool, &all, GROUP, now + 60).await.unwrap());
    let later = now + MISS_FOR + 1;
    assert!(
        offer(&pool, &all, Some(GROUP), later)
            .await
            .unwrap()
            .is_some()
    );
    assert!(servable_to(&pool, &all, GROUP, later).await.unwrap());
}

#[tokio::test]
async fn an_album_in_a_hidden_library_isnt_given_to_who_may_not_see_it() {
    let pool = db::open_in_memory().await;
    turn(&pool, true).await;
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (4, 'music', 'music', '/m'),
             (5, 'private', 'music', '/p');
         INSERT INTO music_albums (library_id, key, title, artist_name, folder, seen_generation,
             match_release_group_id) VALUES (5, 'k', 'OK Computer', 'Radiohead', 'x', 1, ?)",
    )
    .bind(GROUP)
    .execute(&pool)
    .await
    .unwrap();
    let now = 100 * 24 * 3600;
    let kid = Access::limited(Some(vec![4]), None);
    assert_eq!(offer(&pool, &kid, Some(GROUP), now).await.unwrap(), None);
    let all = Access::everything();
    assert!(
        offer(&pool, &all, Some(GROUP), now)
            .await
            .unwrap()
            .is_some()
    );
    assert!(servable_to(&pool, &all, GROUP, now).await.unwrap());
    assert!(!servable_to(&pool, &kid, GROUP, now).await.unwrap());
    let sees_it = Access::limited(Some(vec![4, 5]), None);
    assert!(servable_to(&pool, &sees_it, GROUP, now).await.unwrap());
    assert!(
        offer(&pool, &kid, Some(OTHER), now)
            .await
            .unwrap()
            .is_some(),
        "music nowhere in the library is open to everyone"
    );
}

#[tokio::test]
async fn served_for_thirty_days_while_on_then_forgotten_with_their_files() {
    let pool = db::open_in_memory().await;
    turn(&pool, true).await;
    let dir = tempfile::tempdir().unwrap();
    let all = Access::everything();
    let now = 100 * 24 * 3600;
    for group in [GROUP, OTHER] {
        std::fs::create_dir_all(folder(dir.path(), group)).unwrap();
        std::fs::write(folder(dir.path(), group).join("cover.img"), b"jpeg").unwrap();
    }
    offer(&pool, &all, Some(GROUP), now - SERVED_FOR - 1)
        .await
        .unwrap();
    offer(&pool, &all, Some(OTHER), now - 10).await.unwrap();
    assert!(
        !servable_to(&pool, &all, GROUP, now).await.unwrap(),
        "past its time"
    );
    assert!(servable_to(&pool, &all, OTHER, now).await.unwrap());

    assert_eq!(prune(&pool, dir.path(), now).await.unwrap(), 1);
    assert!(!folder(dir.path(), GROUP).exists());
    assert!(folder(dir.path(), OTHER).join("cover.img").exists());

    turn(&pool, false).await;
    assert_eq!(
        prune(&pool, dir.path(), now).await.unwrap(),
        1,
        "off: all go"
    );
    assert!(!folder(dir.path(), OTHER).exists());
    assert_eq!(noted(&pool).await, 0);
}

#[tokio::test]
async fn at_most_two_thousand_are_remembered() {
    let pool = db::open_in_memory().await;
    turn(&pool, true).await;
    let dir = tempfile::tempdir().unwrap();
    let all = Access::everything();
    let now = 100 * 24 * 3600;
    let id = |n: i64| format!("00000000-0000-4000-8000-{n:012x}");
    for n in 0..MOST + 3 {
        offer(&pool, &all, Some(&id(n)), now - 10_000 + n)
            .await
            .unwrap();
    }
    assert_eq!(prune(&pool, dir.path(), now).await.unwrap(), 3);
    assert!(
        !servable_to(&pool, &all, &id(0), now).await.unwrap(),
        "the oldest went"
    );
    assert!(servable_to(&pool, &all, &id(MOST + 2), now).await.unwrap());
}
