use super::*;
use crate::db;

/// Alice (1) and Bob (2).
async fn users(pool: &SqlitePool) {
    let alice = crate::auth::testing::user(pool, "alice", true).await;
    let bob = crate::auth::testing::user(pool, "bob", false).await;
    assert_eq!((alice.id, bob.id), (1, 2));
}

#[tokio::test]
async fn a_listing_is_its_users_and_only_for_its_time() {
    let pool = db::open_in_memory().await;
    users(&pool).await;
    note(&pool, Kind::Cover, "key", 1, 100).await.unwrap();
    assert!(listed_for(&pool, Kind::Cover, "key", 1, 100).await.unwrap());
    // Not someone else's, another kind's, or before it was listed.
    assert!(!listed_for(&pool, Kind::Cover, "key", 2, 0).await.unwrap());
    assert!(
        !listed_for(&pool, Kind::Portrait, "key", 1, 0)
            .await
            .unwrap()
    );
    assert!(!listed_for(&pool, Kind::Cover, "key", 1, 101).await.unwrap());
    // Listed again, it's from then.
    note(&pool, Kind::Cover, "key", 1, 200).await.unwrap();
    assert!(listed_for(&pool, Kind::Cover, "key", 1, 150).await.unwrap());
    assert_eq!(forget_older(&pool, 201).await.unwrap(), 1);
    assert!(!listed_for(&pool, Kind::Cover, "key", 1, 0).await.unwrap());
}

#[tokio::test]
async fn a_forgotten_picture_or_user_takes_its_listings() {
    let pool = db::open_in_memory().await;
    users(&pool).await;
    for user in [1, 2] {
        note(&pool, Kind::Poster, "/a.jpg", user, 100)
            .await
            .unwrap();
    }
    note(&pool, Kind::Poster, "/b.jpg", 2, 100).await.unwrap();
    forget(&pool, Kind::Poster, "/a.jpg").await.unwrap();
    assert!(
        !listed_for(&pool, Kind::Poster, "/a.jpg", 1, 0)
            .await
            .unwrap()
    );
    assert!(
        listed_for(&pool, Kind::Poster, "/b.jpg", 2, 0)
            .await
            .unwrap()
    );
    sqlx::query("DELETE FROM users WHERE id = 2")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        !listed_for(&pool, Kind::Poster, "/b.jpg", 2, 0)
            .await
            .unwrap()
    );
}
