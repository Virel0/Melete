use std::sync::{Arc, Mutex};

use super::*;
use crate::db;
use crate::musicbrainz::testing::{RADIOHEAD, TRIBUTE_ACT};
use crate::wikimedia::testing as commons;

const NOW: i64 = 100 * 24 * 3600;

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
    set(pool, "MELETE_ARTIST_PORTRAITS", on).await;
}

struct Rig {
    pool: SqlitePool,
    mb: musicbrainz::Client,
    mb_log: Arc<Mutex<Vec<String>>>,
    wiki: wikimedia::Client,
    wiki_log: commons::Log,
}

async fn rig() -> Rig {
    let pool = db::open_in_memory().await;
    let (mb, mb_log) = crate::musicbrainz::testing::start(0).await;
    let (wiki, wiki_log) = commons::start().await;
    Rig {
        pool,
        mb,
        mb_log,
        wiki,
        wiki_log,
    }
}

impl Rig {
    async fn offer(
        &self,
        access: &Access,
        artist: &str,
        lookups: &mut usize,
        now: i64,
    ) -> Option<(String, Credit)> {
        let services = Services {
            musicbrainz: &self.mb,
            wikimedia: &self.wiki,
        };
        offer(&self.pool, &services, access, Some(artist), lookups, now)
            .await
            .unwrap()
    }

    fn asked(&self) -> (usize, usize) {
        (
            self.mb_log.lock().unwrap().len(),
            self.wiki_log.lock().unwrap().len(),
        )
    }
}

fn credit() -> Credit {
    Credit {
        author: Some("Some & One".into()),
        licence: Some("CC BY-SA 4.0".into()),
        licence_url: Some("https://creativecommons.org/licenses/by-sa/4.0".into()),
        source_url: None,
    }
}

#[tokio::test]
async fn nothing_is_asked_while_off_or_without_musicbrainz() {
    let r = rig().await;
    let all = Access::everything();
    let mut lookups = LOOKUPS;
    assert_eq!(r.offer(&all, RADIOHEAD, &mut lookups, NOW).await, None);
    set(&r.pool, "MELETE_ARTIST_PORTRAITS", true).await;
    assert_eq!(
        r.offer(&all, RADIOHEAD, &mut lookups, NOW).await,
        None,
        "needs MusicBrainz on"
    );
    assert_eq!(r.asked(), (0, 0), "nothing asked");
    assert_eq!(lookups, LOOKUPS);
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM artist_portraits")
        .fetch_one(&r.pool)
        .await
        .unwrap();
    assert_eq!(kept, 0);
}

#[tokio::test]
async fn a_portrait_is_found_once_with_its_credit_then_kept() {
    let r = rig().await;
    turn(&r.pool, true).await;
    let all = Access::everything();
    let mut lookups = LOOKUPS;
    let (url, found) = r.offer(&all, RADIOHEAD, &mut lookups, NOW).await.unwrap();
    assert_eq!(
        url,
        "/api/v1/music/artist-portraits/a74b1b7f-71a5-4011-9441-d0b5e4122711"
    );
    assert_eq!(found.author, credit().author);
    assert_eq!(found.licence, credit().licence);
    assert_eq!(found.licence_url, credit().licence_url);
    assert!(
        found
            .source_url
            .unwrap()
            .ends_with("/wiki/File:Radiohead_in_concert.jpg")
    );
    assert_eq!(lookups, LOOKUPS - 1);
    assert_eq!(
        *r.wiki_log.lock().unwrap(),
        ["wikidata Q44190", "commons File:Radiohead in concert.jpg"],
        "the item's id and the file's name; the picture waits to be asked for"
    );
    let picture = servable_to(&r.pool, &all, RADIOHEAD, NOW).await.unwrap();
    assert!(picture.unwrap().ends_with("/thumb/500px.jpg"));

    // The next search asks nothing, and needs no lookup.
    let again = r.offer(&all, RADIOHEAD, &mut lookups, NOW + 60).await;
    assert_eq!(again.map(|(u, _)| u), Some(url));
    assert_eq!(r.asked(), (1, 2));
    assert_eq!(lookups, LOOKUPS - 1);
    // Out of date after 30 days: looked up again.
    r.offer(&all, RADIOHEAD, &mut lookups, NOW + FOUND_FOR + 1)
        .await
        .unwrap();
    assert_eq!(r.asked(), (2, 4));
}

#[tokio::test]
async fn an_artist_without_one_is_believed_a_while_and_a_failure_is_kept_nowhere() {
    let r = rig().await;
    turn(&r.pool, true).await;
    let all = Access::everything();
    let mut lookups = 10;
    let unknown = "00000000-0000-4000-8000-000000000001";
    for _ in 0..2 {
        assert_eq!(r.offer(&all, TRIBUTE_ACT, &mut lookups, NOW).await, None);
        assert_eq!(r.offer(&all, unknown, &mut lookups, NOW).await, None);
    }
    assert_eq!(
        r.asked(),
        (2, 0),
        "asked once each; no Wikidata link, no item asked for"
    );
    assert!(
        servable_to(&r.pool, &all, TRIBUTE_ACT, NOW)
            .await
            .unwrap()
            .is_none()
    );
    r.offer(&all, TRIBUTE_ACT, &mut lookups, NOW + NONE_FOR + 1)
        .await;
    assert_eq!(r.asked(), (3, 0), "asked again a week later");

    // Commons not answering: nothing kept, so the next search asks again.
    let nowhere = "http://127.0.0.1:9".to_owned();
    let dead = wikimedia::Client::with_bases(nowhere.clone(), nowhere, true).unwrap();
    let services = Services {
        musicbrainz: &r.mb,
        wikimedia: &dead,
    };
    for _ in 0..2 {
        let offered = offer(&r.pool, &services, &all, Some(RADIOHEAD), &mut lookups, NOW)
            .await
            .unwrap();
        assert_eq!(offered, None);
    }
    assert_eq!(r.asked().0, 5, "looked up both times");
}

#[tokio::test]
async fn a_search_looks_up_only_a_few() {
    let r = rig().await;
    turn(&r.pool, true).await;
    let all = Access::everything();
    let mut lookups = 1;
    r.offer(&all, TRIBUTE_ACT, &mut lookups, NOW).await;
    assert_eq!(r.offer(&all, RADIOHEAD, &mut lookups, NOW).await, None);
    assert_eq!(r.asked().0, 1, "none left for the second");
    let mut next_search = LOOKUPS;
    assert!(
        r.offer(&all, RADIOHEAD, &mut next_search, NOW)
            .await
            .is_some()
    );
}

#[tokio::test]
async fn an_artist_only_in_a_hidden_library_isnt_given_to_who_may_not_see_it() {
    let r = rig().await;
    turn(&r.pool, true).await;
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (4, 'music', 'music', '/m'),
             (5, 'private', 'music', '/p');
         INSERT INTO music_artists (id, key, name, mb_artist_id) VALUES (1, 'r', 'Radiohead', ?);
         INSERT INTO music_albums (library_id, key, title, artist_name, artist_id, folder,
             seen_generation) VALUES (5, 'k', 'OK Computer', 'Radiohead', 1, 'x', 1)",
    )
    .bind(RADIOHEAD)
    .execute(&r.pool)
    .await
    .unwrap();
    let kid = Access::limited(Some(vec![4]), None);
    let mut lookups = LOOKUPS;
    assert_eq!(r.offer(&kid, RADIOHEAD, &mut lookups, NOW).await, None);
    assert_eq!(r.asked(), (0, 0));
    let all = Access::everything();
    assert!(r.offer(&all, RADIOHEAD, &mut lookups, NOW).await.is_some());
    assert!(
        servable_to(&r.pool, &kid, RADIOHEAD, NOW)
            .await
            .unwrap()
            .is_none()
    );
    let sees_it = Access::limited(Some(vec![4, 5]), None);
    assert!(
        servable_to(&r.pool, &sees_it, RADIOHEAD, NOW)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn served_for_thirty_days_while_on_then_forgotten_with_their_files() {
    let r = rig().await;
    turn(&r.pool, true).await;
    let dir = tempfile::tempdir().unwrap();
    let all = Access::everything();
    let mut lookups = LOOKUPS;
    r.offer(&all, TRIBUTE_ACT, &mut lookups, NOW - SERVED_FOR - 1)
        .await;
    r.offer(&all, RADIOHEAD, &mut lookups, NOW - 10)
        .await
        .unwrap();
    for artist in [TRIBUTE_ACT, RADIOHEAD] {
        std::fs::create_dir_all(folder(dir.path(), artist)).unwrap();
        std::fs::write(folder(dir.path(), artist).join("x.img"), b"jpeg").unwrap();
    }
    assert_eq!(prune(&r.pool, dir.path(), NOW).await.unwrap(), 1);
    assert!(!folder(dir.path(), TRIBUTE_ACT).exists());
    assert!(folder(dir.path(), RADIOHEAD).exists());
    assert!(
        servable_to(&r.pool, &all, RADIOHEAD, NOW)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        servable_to(&r.pool, &all, RADIOHEAD, NOW + SERVED_FOR)
            .await
            .unwrap()
            .is_none(),
        "past its time"
    );

    turn(&r.pool, false).await;
    assert!(
        servable_to(&r.pool, &all, RADIOHEAD, NOW)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        prune(&r.pool, dir.path(), NOW).await.unwrap(),
        1,
        "off: all go"
    );
    assert!(!folder(dir.path(), RADIOHEAD).exists());
}

#[tokio::test]
async fn at_most_two_thousand_are_remembered() {
    let r = rig().await;
    turn(&r.pool, true).await;
    let dir = tempfile::tempdir().unwrap();
    for n in 0..MOST + 3 {
        sqlx::query(
            "INSERT INTO artist_portraits (mb_artist_id, listed_at, looked_up_at) VALUES (?, ?, ?)",
        )
        .bind(format!("00000000-0000-4000-8000-{n:012x}"))
        .bind(NOW - 10_000 + n)
        .bind(NOW)
        .execute(&r.pool)
        .await
        .unwrap();
    }
    assert_eq!(prune(&r.pool, dir.path(), NOW).await.unwrap(), 3);
    let oldest: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM artist_portraits
             WHERE mb_artist_id = '00000000-0000-4000-8000-000000000000')",
    )
    .fetch_one(&r.pool)
    .await
    .unwrap();
    assert!(!oldest, "the oldest went");
}
