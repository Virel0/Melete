//! A library, listeners and a fake ListenBrainz to make For You lists from, for tests.
//!
//! Library 1 is open; library 2 is what a listener limited to library 1 can't see. Alice
//! finished track 1 ("Seed Song" by Alder, recording [`id`]`(1)`) twice and track 5 (which she
//! flagged not interested) once, and started track 4 five days ago; Bob finished track 8. What
//! ListenBrainz says goes with Alice's song, from best to worst: recordings 2 (track 2, in the
//! library by its id), 4 (track 4, heard lately), 5 (track 5, flagged), 3 (track 3 has no
//! recording id, but its artist's id and title), 9 (track 9 by Fir, in library 2), 20 and 21
//! (from one album, not in the library), 22 and 1 (her own song). Alongside Alder are Birch,
//! with tracks 6, 7 and 8 on three albums, then Gale, with none in the library but recordings
//! 40, 41 and 42 most listened-to, then an artist whose top recordings ListenBrainz refuses;
//! alongside Birch is Alder.

use serde_json::Value;
use sqlx::SqlitePool;

use crate::listenbrainz::testing::{self, Answers, Log};
use crate::plays::{self, Listen, Source};

/// A MusicBrainz id made from a number, for the world's recordings, artists and albums.
pub fn id(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

/// Now, in seconds since 1970: some fixed time.
pub const NOW: i64 = 1_800_000_000;

/// The world: its database, Alice's and Bob's ids, and a fake ListenBrainz with its log.
pub struct World {
    /// The database.
    pub pool: SqlitePool,
    /// Alice's user id.
    pub alice: i64,
    /// Bob's user id.
    pub bob: i64,
    /// A client for the fake ListenBrainz.
    pub client: crate::listenbrainz::Client,
    /// What the fake was asked.
    pub log: Log,
}

/// Turns `name` on.
pub async fn on(pool: &SqlitePool, name: &str) {
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES (?, 'on', 0)
         ON CONFLICT (name) DO UPDATE SET value = 'on'",
    )
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
}

/// A play of `track` by `user`, `days` ago, finished or not.
pub async fn play(pool: &SqlitePool, user: i64, track: i64, days: i64, completed: bool, n: u32) {
    let listen = Listen {
        event_id: format!("e{user}-{track}-{n}"),
        track_key: track.to_string(),
        device: "Phone".into(),
        started_at: (NOW - days * 86_400) * 1000,
        ended_at: None,
        position_ms: None,
        completed,
        skipped: false,
        activity: None,
    };
    plays::record(pool, user, Source::Api, &listen)
        .await
        .unwrap();
}

fn library_sql() -> String {
    let (alder, birch, cedar, fir) = (id(101), id(102), id(103), id(104));
    let track = |n: i64, lib: i64, album: i64, title: &str, artist: &str, rec: Option<u32>| {
        let rec = rec.map_or("NULL".into(), |r| format!("'{}'", id(r)));
        format!("({n}, {lib}, 'p/{n}', 1, 1, 1, 1, {album}, 1, '{title}', '{artist}', '[]', {rec})")
    };
    let tracks = [
        track(1, 1, 1, "Seed Song", "Alder", Some(1)),
        track(2, 1, 2, "Owned By Id", "Cedar", Some(2)),
        track(3, 1, 3, "Owned by Title!", "Cedar", None),
        track(4, 1, 4, "Heard Lately", "Cedar", Some(4)),
        track(5, 1, 5, "Flagged", "Cedar", Some(5)),
        track(6, 1, 6, "Birch One", "Birch", Some(6)),
        track(7, 1, 7, "Birch Two", "Birch", Some(7)),
        track(8, 1, 8, "Birch Three", "Birch", Some(8)),
        track(9, 2, 9, "Hidden", "Fir", Some(9)),
    ];
    let albums: Vec<String> = (1..=9)
        .map(|n| {
            let lib = if n == 9 { 2 } else { 1 };
            format!("({n}, {lib}, 'k{n}', 'Album {n}', 'x', 'a{n}', 1)")
        })
        .collect();
    let names = ["", "Alder", "Birch", "Cedar", "Fir"];
    let credits: Vec<String> = [
        (1, 1),
        (2, 3),
        (3, 3),
        (4, 3),
        (5, 3),
        (6, 2),
        (7, 2),
        (8, 2),
        (9, 4),
    ]
    .iter()
    .map(|&(t, a)| format!("({t}, 0, {a}, '{}')", names[a]))
    .collect();
    format!(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m'),
             (2, 'more', 'music', '/n');
         INSERT INTO music_artists (id, key, name, mb_artist_id) VALUES
             (1, 'alder', 'Alder', '{alder}'), (2, 'birch', 'Birch', '{birch}'),
             (3, 'cedar', 'Cedar', '{cedar}'), (4, 'fir', 'Fir', '{fir}');
         INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
             seen_generation) VALUES {};
         INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
             first_seen_generation, seen_generation, album_id, tagged, title, artist, genres,
             mb_recording_id) VALUES {};
         INSERT INTO music_track_artists (track_id, position, artist_id, name) VALUES {};",
        albums.join(", "),
        tracks.join(", "),
        credits.join(", ")
    )
}

/// What the fake ListenBrainz says, as the module's doc tells it.
pub fn answers() -> Answers {
    let mut answers = Answers::default();
    let rec = |n: u32, title: &str, artist: &str, score: i64| {
        testing::recording(&id(n), title, artist, "Some Album", score)
    };
    let found: Vec<Value> = vec![
        rec(2, "Owned By Id", "Cedar", 1000),
        rec(4, "Heard Lately", "Cedar", 950),
        rec(5, "Flagged", "Cedar", 900),
        rec(3, "Owned By Title", "Cedar", 850),
        rec(9, "Hidden", "Fir", 800),
        rec(20, "Far Away", "Dune", 700),
        rec(21, "Far Again", "Dune", 650),
        rec(22, "Wanted Song", "Echo", 600),
        rec(1, "Seed Song", "Alder", 500),
    ];
    answers.recordings.insert(id(1), found);
    answers.artists.insert(
        id(101),
        vec![
            testing::artist(&id(102), "Birch", 40),
            testing::artist(&id(105), "Gale", 20),
            testing::artist(testing::REFUSED, "Refused", 10),
        ],
    );
    answers.top.insert(
        id(105),
        vec![
            testing::top(&id(40), "Gale Hit", "Gale", 1000),
            testing::top(&id(41), "Gale Two", "Gale", 500),
            testing::top(&id(42), "Gale Three", "Gale", 250),
        ],
    );
    answers
        .artists
        .insert(id(102), vec![testing::artist(&id(101), "Alder", 40)]);
    let cedar = [(id(103), "Cedar".to_owned())];
    let meta = |artists: &[(String, String)], group: u32| {
        let artists: Vec<(&str, &str)> = artists
            .iter()
            .map(|(a, n)| (a.as_str(), n.as_str()))
            .collect();
        testing::metadata(&artists, "Some Album", &id(group))
    };
    answers.metadata.insert(id(3), meta(&cedar, 203));
    answers
        .metadata
        .insert(id(9), meta(&[(id(104), "Fir".to_owned())], 209));
    let dune = [(id(120), "Dune".to_owned())];
    answers.metadata.insert(id(20), meta(&dune, 220));
    answers.metadata.insert(id(21), meta(&dune, 220));
    answers
        .metadata
        .insert(id(22), meta(&[(id(122), "Echo".to_owned())], 222));
    let gale = [(id(105), "Gale".to_owned())];
    for n in [40, 41, 42] {
        answers.metadata.insert(id(n), meta(&gale, 200 + n));
    }
    answers
}

/// The world, with For You on (and MusicBrainz).
pub async fn world() -> World {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(library_sql()))
        .execute(&pool)
        .await
        .unwrap();
    on(&pool, "MELETE_MUSICBRAINZ").await;
    on(&pool, "MELETE_LISTENBRAINZ").await;
    let alice = crate::auth::testing::user(&pool, "alice", false).await.id;
    let bob = crate::auth::testing::user(&pool, "bob", false).await.id;
    play(&pool, alice, 1, 10, true, 1).await;
    play(&pool, alice, 1, 20, true, 2).await;
    play(&pool, alice, 5, 15, true, 1).await;
    play(&pool, alice, 4, 5, false, 1).await;
    play(&pool, bob, 8, 3, true, 1).await;
    crate::aoide::track_flags::set(
        &pool,
        alice,
        "Phone",
        5,
        crate::aoide::track_flags::Flag::NotInterested,
        true,
        NOW * 1000,
    )
    .await
    .unwrap();
    let (client, log) = testing::start(answers()).await;
    World {
        pool,
        alice,
        bob,
        client,
        log,
    }
}
