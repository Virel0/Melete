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
    async fn offer(&self, artist: &str, lookups: &mut usize, now: i64) -> Option<(String, Credit)> {
        let services = Services {
            musicbrainz: &self.mb,
            wikimedia: &self.wiki,
        };
        offer(&self.pool, &services, Some(artist), lookups, now)
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
    let mut lookups = LOOKUPS;
    assert_eq!(r.offer(RADIOHEAD, &mut lookups, NOW).await, None);
    set(&r.pool, "MELETE_ARTIST_PORTRAITS", true).await;
    assert_eq!(
        r.offer(RADIOHEAD, &mut lookups, NOW).await,
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
    let mut lookups = LOOKUPS;
    let (url, found) = r.offer(RADIOHEAD, &mut lookups, NOW).await.unwrap();
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
    let picture = servable(&r.pool, RADIOHEAD, NOW).await.unwrap();
    assert!(picture.unwrap().ends_with("/thumb/500px.jpg"));

    // The next search asks nothing, and needs no lookup.
    let again = r.offer(RADIOHEAD, &mut lookups, NOW + 60).await;
    assert_eq!(again.map(|(u, _)| u), Some(url));
    assert_eq!(r.asked(), (1, 2));
    assert_eq!(lookups, LOOKUPS - 1);
    // Out of date after 30 days: looked up again.
    r.offer(RADIOHEAD, &mut lookups, NOW + FOUND_FOR + 1)
        .await
        .unwrap();
    assert_eq!(r.asked(), (2, 4));
}

#[tokio::test]
async fn an_artist_without_one_is_believed_a_while_and_a_failed_lookup_is_kept_nowhere() {
    let r = rig().await;
    turn(&r.pool, true).await;
    let mut lookups = 10;
    let unknown = "00000000-0000-4000-8000-000000000001";
    for _ in 0..2 {
        assert_eq!(r.offer(TRIBUTE_ACT, &mut lookups, NOW).await, None);
        assert_eq!(r.offer(unknown, &mut lookups, NOW).await, None);
    }
    assert_eq!(
        r.asked(),
        (2, 0),
        "asked once each; no Wikidata link, no item asked for"
    );
    assert!(servable(&r.pool, TRIBUTE_ACT, NOW).await.unwrap().is_none());
    r.offer(TRIBUTE_ACT, &mut lookups, NOW + NONE_FOR + 1).await;
    assert_eq!(r.asked(), (3, 0), "asked again a week later");

    // Commons not answering: nothing kept, so the next search asks again.
    let nowhere = "http://127.0.0.1:9".to_owned();
    let dead = wikimedia::Client::with_bases(nowhere.clone(), nowhere, true).unwrap();
    let services = Services {
        musicbrainz: &r.mb,
        wikimedia: &dead,
    };
    for _ in 0..2 {
        let offered = offer(&r.pool, &services, Some(RADIOHEAD), &mut lookups, NOW)
            .await
            .unwrap();
        assert_eq!(offered, None);
    }
    assert_eq!(r.asked().0, 5, "looked up both times");
}

#[tokio::test]
async fn a_picture_commons_failed_to_give_is_neither_offered_nor_served_for_a_day() {
    let r = rig().await;
    turn(&r.pool, true).await;
    let mut lookups = LOOKUPS;
    r.offer(RADIOHEAD, &mut lookups, NOW).await.unwrap();
    failed(&r.pool, RADIOHEAD, NOW).await.unwrap();
    assert_eq!(r.offer(RADIOHEAD, &mut lookups, NOW + 60).await, None);
    assert!(
        servable(&r.pool, RADIOHEAD, NOW + 60)
            .await
            .unwrap()
            .is_none()
    );
    let later = NOW + FAIL_FOR + 1;
    assert!(r.offer(RADIOHEAD, &mut lookups, later).await.is_some());
    assert!(servable(&r.pool, RADIOHEAD, later).await.unwrap().is_some());
    assert_eq!(r.asked().0, 1, "the failure needed no lookup again");
}

#[tokio::test]
async fn a_search_looks_up_only_a_few() {
    let r = rig().await;
    turn(&r.pool, true).await;
    let mut lookups = 1;
    r.offer(TRIBUTE_ACT, &mut lookups, NOW).await;
    assert_eq!(r.offer(RADIOHEAD, &mut lookups, NOW).await, None);
    assert_eq!(r.asked().0, 1, "none left for the second");
    let mut next_search = LOOKUPS;
    assert!(r.offer(RADIOHEAD, &mut next_search, NOW).await.is_some());
}

#[tokio::test]
async fn served_for_thirty_days_while_on_then_forgotten_with_their_files() {
    let r = rig().await;
    turn(&r.pool, true).await;
    let dir = tempfile::tempdir().unwrap();
    let mut lookups = LOOKUPS;
    r.offer(TRIBUTE_ACT, &mut lookups, NOW - SERVED_FOR - 1)
        .await;
    r.offer(RADIOHEAD, &mut lookups, NOW - 10).await.unwrap();
    for artist in [TRIBUTE_ACT, RADIOHEAD] {
        std::fs::create_dir_all(folder(dir.path(), artist)).unwrap();
        std::fs::write(folder(dir.path(), artist).join("x.img"), b"jpeg").unwrap();
    }
    assert_eq!(prune(&r.pool, dir.path(), NOW).await.unwrap(), 1);
    assert!(!folder(dir.path(), TRIBUTE_ACT).exists());
    assert!(folder(dir.path(), RADIOHEAD).exists());
    assert!(servable(&r.pool, RADIOHEAD, NOW).await.unwrap().is_some());
    assert!(
        servable(&r.pool, RADIOHEAD, NOW + SERVED_FOR)
            .await
            .unwrap()
            .is_none(),
        "past its time"
    );

    turn(&r.pool, false).await;
    assert!(servable(&r.pool, RADIOHEAD, NOW).await.unwrap().is_none());
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

/// A music library holding an album by each of `artists` (MusicBrainz ids, or none), present
/// or not; the album's id is the artist's.
async fn in_library(pool: &SqlitePool, artists: &[(Option<&str>, bool)]) {
    sqlx::query(
        "INSERT OR IGNORE INTO libraries (id, name, kind, path) VALUES (1, 'm', 'music', '/m')",
    )
    .execute(pool)
    .await
    .unwrap();
    for (n, (artist, present)) in artists.iter().enumerate() {
        let id = i64::try_from(n).unwrap() + 1;
        sqlx::query("INSERT INTO music_artists (id, key, name, mb_artist_id) VALUES (?, ?, ?, ?)")
            .bind(id)
            .bind(format!("k{id}"))
            .bind(format!("Artist {id}"))
            .bind(artist)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO music_albums (id, library_id, key, title, artist_id, artist_name, folder,
                 seen_generation, missing_since) VALUES (?1, 1, ?2, 'A', ?1, 'A', ?2, 1, ?3)",
        )
        .bind(id)
        .bind(format!("a/{id}"))
        .bind((!present).then_some(1))
        .execute(pool)
        .await
        .unwrap();
    }
}

fn unknown(n: usize) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}

impl Rig {
    async fn look_up_library(&self, now: i64) -> Looked {
        let services = Services {
            musicbrainz: &self.mb,
            wikimedia: &self.wiki,
        };
        look_up_library(&self.pool, &services, now).await.unwrap()
    }
}

#[tokio::test]
async fn the_librarys_artists_are_looked_up_in_the_background_a_few_a_run() {
    let r = rig().await;
    let others: Vec<String> = (1..=LIBRARY_LOOKUPS + 1).map(unknown).collect();
    let mut artists: Vec<(Option<&str>, bool)> = vec![(Some(RADIOHEAD), true)];
    artists.extend(others.iter().map(|a| (Some(a.as_str()), true)));
    let gone = unknown(999);
    artists.extend([
        (None, true),
        (Some(gone.as_str()), false),
        (Some("not-an-id"), true),
    ]);
    in_library(&r.pool, &artists).await;

    assert_eq!(r.look_up_library(NOW).await, Looked::default(), "off");
    turn(&r.pool, true).await;
    assert_eq!(r.look_up_library(NOW).await.looked_up, LIBRARY_LOOKUPS);
    assert_eq!(r.look_up_library(NOW).await.looked_up, 2, "the rest");
    assert_eq!(r.look_up_library(NOW).await, Looked::default(), "all known");
    let asked = r.mb_log.lock().unwrap().clone();
    assert_eq!(asked.len(), LIBRARY_LOOKUPS + 2, "each once");
    assert!(
        !asked
            .iter()
            .any(|a| a.contains(&gone) || a.contains("not-an-id")),
        "only present artists with a MusicBrainz id: {asked:?}"
    );
    let credits = credits(&r.pool, &[RADIOHEAD, &others[0]], NOW)
        .await
        .unwrap();
    assert_eq!(credits.len(), 1);
    assert_eq!(credits[RADIOHEAD].author, credit().author);
    assert!(
        servable_in_library(&r.pool, RADIOHEAD, NOW)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        servable(&r.pool, RADIOHEAD, NOW).await.unwrap().is_none(),
        "a lookup for the library isn't a search listing it"
    );

    // The nones are asked again after a week, the portrait found after 30 days.
    let looked = r.look_up_library(NOW + NONE_FOR + 1).await;
    assert_eq!(looked.looked_up, LIBRARY_LOOKUPS);
    assert_eq!(r.look_up_library(NOW + NONE_FOR + 1).await.looked_up, 1);
    let later = r.look_up_library(NOW + FOUND_FOR + 1).await;
    assert_eq!(
        later.looked_up, LIBRARY_LOOKUPS,
        "the oldest first, Radiohead among them"
    );
    assert!(
        r.mb_log.lock().unwrap()[LIBRARY_LOOKUPS * 2 + 3..]
            .iter()
            .any(|a| a.contains(RADIOHEAD))
    );
}

#[tokio::test]
async fn a_failed_library_lookup_is_counted_and_kept_nowhere() {
    let r = rig().await;
    turn(&r.pool, true).await;
    in_library(&r.pool, &[(Some(RADIOHEAD), true)]).await;
    let nowhere = "http://127.0.0.1:9".to_owned();
    let dead = wikimedia::Client::with_bases(nowhere.clone(), nowhere, true).unwrap();
    let services = Services {
        musicbrainz: &r.mb,
        wikimedia: &dead,
    };
    for _ in 0..2 {
        let looked = look_up_library(&r.pool, &services, NOW).await.unwrap();
        assert_eq!(
            looked,
            Looked {
                looked_up: 1,
                failed: 1
            },
            "asked again"
        );
    }
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM artist_portraits")
        .fetch_one(&r.pool)
        .await
        .unwrap();
    assert_eq!(kept, 0);
}

#[tokio::test]
async fn an_artist_with_a_picture_of_their_own_isnt_looked_up() {
    let r = rig().await;
    turn(&r.pool, true).await;
    in_library(&r.pool, &[(Some(RADIOHEAD), true)]).await;
    // A second album in the same folder, both with the picture above them.
    sqlx::raw_sql(
        "INSERT INTO music_albums (id, library_id, key, title, artist_id, artist_name, folder,
             seen_generation) VALUES (2, 1, 'b', 'B', 1, 'A', 'a/2', 1);
         UPDATE music_albums SET artist_file = 'a/artist.jpg';",
    )
    .execute(&r.pool)
    .await
    .unwrap();
    assert_eq!(r.look_up_library(NOW).await, Looked::default());
    assert_eq!(r.asked(), (0, 0));
    // One album elsewhere: the picture isn't theirs alone.
    sqlx::query("UPDATE music_albums SET artist_file = NULL WHERE id = 2")
        .execute(&r.pool)
        .await
        .unwrap();
    assert_eq!(r.look_up_library(NOW).await.looked_up, 1);
}

#[tokio::test]
async fn the_librarys_artists_are_kept_past_their_time_and_the_most_until_they_go_or_its_off() {
    let r = rig().await;
    turn(&r.pool, true).await;
    let dir = tempfile::tempdir().unwrap();
    in_library(
        &r.pool,
        &[(Some(RADIOHEAD), true), (Some(TRIBUTE_ACT), true)],
    )
    .await;
    r.look_up_library(NOW).await;
    let later = NOW + 2 * SERVED_FOR;
    for n in 0..MOST + 3 {
        sqlx::query(
            "INSERT INTO artist_portraits (mb_artist_id, listed_at, looked_up_at) VALUES (?, ?, ?)",
        )
        .bind(unknown(usize::try_from(n).unwrap()))
        .bind(later - 10_000 + n)
        .bind(later)
        .execute(&r.pool)
        .await
        .unwrap();
    }
    assert_eq!(
        prune(&r.pool, dir.path(), later).await.unwrap(),
        3,
        "only searched ones"
    );
    assert!(
        servable_in_library(&r.pool, RADIOHEAD, later)
            .await
            .unwrap()
            .is_some()
    );

    // Gone from the library: past its time like any other.
    sqlx::query("UPDATE music_albums SET missing_since = 1 WHERE artist_id = 2")
        .execute(&r.pool)
        .await
        .unwrap();
    assert_eq!(prune(&r.pool, dir.path(), later).await.unwrap(), 1);
    let kept: i64 =
        sqlx::query_scalar("SELECT count(*) FROM artist_portraits WHERE mb_artist_id IN (?, ?)")
            .bind(RADIOHEAD)
            .bind(TRIBUTE_ACT)
            .fetch_one(&r.pool)
            .await
            .unwrap();
    assert_eq!(kept, 1);

    turn(&r.pool, false).await;
    assert_eq!(
        prune(&r.pool, dir.path(), later).await.unwrap(),
        usize::try_from(MOST).unwrap() + 1,
        "off: all go, the library's too"
    );
}
