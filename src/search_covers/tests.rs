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

/// A user searches are listed for.
async fn someone(pool: &SqlitePool) -> i64 {
    crate::auth::testing::user(pool, "alice", false).await.id
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
    let me = someone(&pool).await;
    let now = 100 * 24 * 3600;
    assert_eq!(offer(&pool, Some(GROUP), me, now).await.unwrap(), None);
    assert_eq!(noted(&pool).await, 0, "off by default: nothing noted");
    set(&pool, "MELETE_SEARCH_COVERS", true).await;
    assert_eq!(
        offer(&pool, Some(GROUP), me, now).await.unwrap(),
        None,
        "needs MusicBrainz on"
    );
    turn(&pool, true).await;
    assert_eq!(
        offer(&pool, Some(GROUP), me, now).await.unwrap().as_deref(),
        Some("/api/v1/music/release-groups/b1392450-e666-3926-a536-22c65f834433/cover")
    );
    assert!(servable(&pool, GROUP, me, now).await.unwrap());
    assert!(
        !servable(&pool, OTHER, me, now).await.unwrap(),
        "never listed"
    );
    assert_eq!(offer(&pool, Some("../../x"), me, now).await.unwrap(), None);
    assert_eq!(offer(&pool, None, me, now).await.unwrap(), None);
    assert!(!servable(&pool, "../../x", me, now).await.unwrap());
    turn(&pool, false).await;
    assert!(
        !servable(&pool, GROUP, me, now).await.unwrap(),
        "off: not served"
    );
}

#[tokio::test]
async fn one_missed_isnt_offered_or_served_for_a_day() {
    let pool = db::open_in_memory().await;
    let me = someone(&pool).await;
    turn(&pool, true).await;
    let now = 100 * 24 * 3600;
    offer(&pool, Some(GROUP), me, now).await.unwrap();
    missed(&pool, GROUP, me, now).await.unwrap();
    assert_eq!(offer(&pool, Some(GROUP), me, now + 60).await.unwrap(), None);
    assert!(!servable(&pool, GROUP, me, now + 60).await.unwrap());
    let later = now + MISS_FOR + 1;
    assert!(
        offer(&pool, Some(GROUP), me, later)
            .await
            .unwrap()
            .is_some()
    );
    assert!(servable(&pool, GROUP, me, later).await.unwrap());
}

#[tokio::test]
async fn served_for_thirty_days_while_on_then_forgotten_with_their_files() {
    let pool = db::open_in_memory().await;
    let me = someone(&pool).await;
    turn(&pool, true).await;
    let dir = tempfile::tempdir().unwrap();
    let now = 100 * 24 * 3600;
    for group in [GROUP, OTHER] {
        std::fs::create_dir_all(folder(dir.path(), group)).unwrap();
        std::fs::write(folder(dir.path(), group).join("cover.img"), b"jpeg").unwrap();
    }
    offer(&pool, Some(GROUP), me, now - SERVED_FOR - 1)
        .await
        .unwrap();
    offer(&pool, Some(OTHER), me, now - 10).await.unwrap();
    assert!(
        !servable(&pool, GROUP, me, now).await.unwrap(),
        "past its time"
    );
    assert!(servable(&pool, OTHER, me, now).await.unwrap());

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
    let me = someone(&pool).await;
    turn(&pool, true).await;
    let dir = tempfile::tempdir().unwrap();
    let now = 100 * 24 * 3600;
    let id = |n: i64| format!("00000000-0000-4000-8000-{n:012x}");
    for n in 0..MOST + 3 {
        offer(&pool, Some(&id(n)), me, now - 10_000 + n)
            .await
            .unwrap();
    }
    assert_eq!(prune(&pool, dir.path(), now).await.unwrap(), 3);
    assert!(
        !servable(&pool, &id(0), me, now).await.unwrap(),
        "the oldest went"
    );
    assert!(servable(&pool, &id(MOST + 2), me, now).await.unwrap());
}

#[tokio::test]
async fn served_only_to_whom_a_search_listed_it_for() {
    let pool = db::open_in_memory().await;
    let me = someone(&pool).await;
    let other = crate::auth::testing::user(&pool, "bob", false).await.id;
    turn(&pool, true).await;
    let now = 100 * 24 * 3600;
    offer(&pool, Some(GROUP), me, now).await.unwrap();
    assert!(servable(&pool, GROUP, me, now).await.unwrap());
    assert!(
        !servable(&pool, GROUP, other, now).await.unwrap(),
        "someone else's search isn't told"
    );
    offer(&pool, Some(GROUP), other, now).await.unwrap();
    assert!(servable(&pool, GROUP, other, now).await.unwrap());
}

#[tokio::test]
async fn a_listing_past_the_most_kept_wakes_the_forgetting() {
    let pool = db::open_in_memory().await;
    let me = someone(&pool).await;
    turn(&pool, true).await;
    let now = 100 * 24 * 3600;
    let id = |n: i64| format!("00000000-0000-4000-8000-{n:012x}");
    for n in 0..MOST {
        offer(&pool, Some(&id(n)), me, now).await.unwrap();
    }
    // Any wake left by another test first.
    while tokio::time::timeout(
        std::time::Duration::ZERO,
        crate::picture_listings::PRUNE.notified(),
    )
    .await
    .is_ok()
    {}
    let woken = crate::picture_listings::PRUNE.notified();
    tokio::pin!(woken);
    woken.as_mut().enable();
    offer(&pool, Some(&id(MOST)), me, now).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), woken)
        .await
        .expect("one past the most kept wakes the forgetting");
}
