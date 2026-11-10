use super::picking::{Candidate, rank};
use super::testing::{NOW, World, id, play, world};
use super::*;
use crate::listenbrainz::{ARTISTS_ALGORITHM, RECORDINGS_ALGORITHM};

fn everything() -> Access {
    Access::everything()
}

/// Only library 1.
fn limited() -> Access {
    Access::limited(Some(vec![1]), None)
}

/// What a list is, in short: `t<id>` for a track to play, `s<n>` for a song to request (its
/// recording's number), with what it came from.
fn short(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .map(|i| match (&i.track_id, &i.song) {
            (Some(t), _) => format!("t{t} {}", i.because.artist),
            (None, Some(s)) => {
                let n: u32 = s.mb_recording_id[24..].parse().unwrap();
                format!("s{n} {}", i.because.artist)
            }
            (None, None) => "nothing".into(),
        })
        .collect()
}

async fn made(w: &World, user: i64, access: &Access) -> Vec<Item> {
    make_for(&w.pool, &w.client, user, access, NOW)
        .await
        .unwrap();
    for_listener(&w.pool, user, access, NOW)
        .await
        .unwrap()
        .items
}

#[tokio::test]
async fn seeds_are_the_listeners_own_finished_plays_less_what_they_flagged() {
    let w = world().await;
    let flagged = track_ids(
        crate::aoide::listening::not_interested(&w.pool, w.alice)
            .await
            .unwrap(),
    );
    assert_eq!(flagged, HashSet::from([5]));
    let seeds = seeds::read(&w.pool, w.alice, &everything(), &flagged, NOW)
        .await
        .unwrap();
    let mbids = |s: &[seeds::Seed]| s.iter().map(|s| s.mbid.clone()).collect::<Vec<_>>();
    // Not track 4 (started, never finished), track 5 (flagged) or Bob's track 8.
    assert_eq!(mbids(&seeds.recordings), [id(1)]);
    assert_eq!(mbids(&seeds.artists), [id(101)]);
    assert_eq!(seeds.recordings[0].weight, 1.0);
    assert_eq!(
        seeds.recordings[0].because,
        Because {
            kind: "song".into(),
            title: Some("Seed Song".into()),
            artist: "Alder".into()
        }
    );
    let bob = seeds::read(&w.pool, w.bob, &everything(), &HashSet::new(), NOW)
        .await
        .unwrap();
    assert_eq!(
        (mbids(&bob.recordings), mbids(&bob.artists)),
        (vec![id(8)], vec![id(102)])
    );
    // Plays older than 90 days count for nothing.
    let later = NOW + 100 * 86_400;
    let gone = seeds::read(&w.pool, w.alice, &everything(), &flagged, later)
        .await
        .unwrap();
    assert!(gone.recordings.is_empty() && gone.artists.is_empty());
}

#[tokio::test]
async fn offers_what_the_library_has_to_play_and_the_rest_to_request() {
    let w = world().await;
    let items = made(&w, w.alice, &everything()).await;
    // Track 2 by its recording, 3 by its artist and title, 6 and 7 by Birch (two at most) and 9;
    // songs 20 (21 is from the same album) and 22, then Gale's most listened-to, 40 and 41 (42
    // would be Gale's third). Never track 4 (heard lately), 5 (flagged) or 1 (her own), nor any
    // of them to request; the artist ListenBrainz refused is left out, and the list made all
    // the same.
    assert_eq!(
        short(&items),
        [
            "t2 Alder",
            "t6 Alder",
            "t7 Alder",
            "t3 Alder",
            "t9 Alder",
            "s20 Alder",
            "s22 Alder",
            "s40 Alder",
            "s41 Alder"
        ]
    );
    assert_eq!(items[7].because.kind, "artist", "Gale came from her artist");
    assert_eq!(items[1].because.kind, "artist");
    assert_eq!(items[0].because.title.as_deref(), Some("Seed Song"));
    let song = items[5].song.clone().unwrap();
    assert_eq!(
        song,
        Song {
            mb_recording_id: id(20),
            title: "Far Away".into(),
            artist: "Dune".into(),
            mb_artist_ids: vec![id(120)],
            release: Some("Some Album".into()),
            release_group: Some(id(220)),
        }
    );
}

#[tokio::test]
async fn sends_musicbrainz_ids_alone_and_only_the_listeners_own() {
    let w = world().await;
    made(&w, w.alice, &everything()).await;
    assert_eq!(
        *w.log.lock().unwrap(),
        [
            format!(
                "similar-recordings recording_mbids={} algorithm={RECORDINGS_ALGORITHM}",
                id(1)
            ),
            format!(
                "similar-artists artist_mbids={} algorithm={ARTISTS_ALGORITHM}",
                id(101)
            ),
            format!("top-recordings {}", id(102)),
            format!("top-recordings {}", id(105)),
            format!("top-recordings {}", crate::listenbrainz::testing::REFUSED),
            format!(
                "metadata recording_mbids={} inc=artist release",
                [3, 20, 21, 22, 40, 41, 42].map(id).join(",")
            ),
        ],
        "Alice's seeds, the artists alongside hers, then the suggestions the library hasn't by \
         id; nothing of Bob's"
    );
    // A list made again asks only what was refused: the answers are kept.
    made(&w, w.alice, &everything()).await;
    let again = w.log.lock().unwrap()[6..].to_vec();
    assert_eq!(
        again,
        [format!(
            "top-recordings {}",
            crate::listenbrainz::testing::REFUSED
        )]
    );
    // Bob's is made from his own plays: Birch, and Alder alongside.
    let bob = made(&w, w.bob, &everything()).await;
    assert_eq!(short(&bob), ["t1 Birch"]);
    let log = w.log.lock().unwrap();
    assert_eq!(log.len(), 10);
    assert!(log[7].contains(&id(8)) && log[8].contains(&id(102)) && log[9].contains(&id(101)));
}

#[tokio::test]
async fn a_track_the_listener_cant_see_is_never_offered_to_play() {
    let w = world().await;
    let items = made(&w, w.alice, &limited()).await;
    assert_eq!(
        short(&items),
        [
            "t2 Alder",
            "t6 Alder",
            "t7 Alder",
            "t3 Alder",
            "s9 Alder",
            "s20 Alder",
            "s22 Alder",
            "s40 Alder",
            "s41 Alder"
        ],
        "track 9 is in a library hidden from her: to her it's a song to request"
    );
    // Kept for one who sees everything, it's left out when served to one who doesn't.
    let w = world().await;
    make_for(&w.pool, &w.client, w.alice, &everything(), NOW)
        .await
        .unwrap();
    let served = for_listener(&w.pool, w.alice, &limited(), NOW)
        .await
        .unwrap();
    assert!(!short(&served.items).contains(&"t9 Alder".to_owned()));
}

#[tokio::test]
async fn what_was_heard_or_flagged_since_leaves_the_list() {
    let w = world().await;
    made(&w, w.alice, &everything()).await;
    play(&w.pool, w.alice, 2, 0, false, 9).await;
    crate::aoide::track_flags::set(
        &w.pool,
        w.alice,
        "Phone",
        6,
        crate::aoide::track_flags::Flag::NotInterested,
        true,
        NOW * 1000,
    )
    .await
    .unwrap();
    let served = for_listener(&w.pool, w.alice, &everything(), NOW)
        .await
        .unwrap();
    assert_eq!(served.state, State::Ready);
    assert_eq!(
        short(&served.items),
        [
            "t7 Alder",
            "t3 Alder",
            "t9 Alder",
            "s20 Alder",
            "s22 Alder",
            "s40 Alder",
            "s41 Alder"
        ]
    );
}

#[tokio::test]
async fn says_where_a_list_stands() {
    let w = world().await;
    let carol = crate::auth::testing::user(&w.pool, "carol", false).await.id;
    let state = |user| {
        let pool = w.pool.clone();
        async move {
            for_listener(&pool, user, &everything(), NOW)
                .await
                .unwrap()
                .state
        }
    };
    assert_eq!(state(carol).await, State::NoHistory);
    assert_eq!(state(w.alice).await, State::Waiting);
    make_for(&w.pool, &w.client, w.alice, &everything(), NOW)
        .await
        .unwrap();
    let served = for_listener(&w.pool, w.alice, &everything(), NOW)
        .await
        .unwrap();
    assert_eq!((served.state, served.made_at), (State::Ready, Some(NOW)));
    assert_eq!(state(w.bob).await, State::Waiting, "Bob's is his own");
}

#[tokio::test]
async fn background_runs_make_the_lists_due_and_no_others() {
    let w = world().await;
    let made = make_due(&w.pool, &w.client, NOW).await.unwrap();
    assert_eq!(made, Made { made: 2, failed: 0 });
    // Nothing is due again until a day has passed and their plays changed.
    assert_eq!(
        make_due(&w.pool, &w.client, NOW + 60).await.unwrap(),
        Made::default()
    );
    let tomorrow = NOW + REMAKE_AFTER + 1;
    assert_eq!(
        make_due(&w.pool, &w.client, tomorrow).await.unwrap(),
        Made::default()
    );
    play(&w.pool, w.bob, 7, 0, true, 2).await;
    let made = make_due(&w.pool, &w.client, tomorrow).await.unwrap();
    assert_eq!(
        made,
        Made { made: 1, failed: 0 },
        "Bob's, as his plays changed"
    );
}

#[tokio::test]
async fn a_failed_list_keeps_the_one_from_before_and_waits_to_be_tried_again() {
    let w = world().await;
    for user in [w.alice, w.bob] {
        make_for(&w.pool, &w.client, user, &everything(), NOW)
            .await
            .unwrap();
    }
    let before = for_listener(&w.pool, w.alice, &everything(), NOW)
        .await
        .unwrap();
    // Her next seed is one ListenBrainz holds back for an hour.
    sqlx::query("UPDATE music_tracks SET mb_recording_id = ? WHERE id = 1")
        .bind(crate::listenbrainz::testing::HELD)
        .execute(&w.pool)
        .await
        .unwrap();
    let later = NOW + REMAKE_AFTER + 1;
    play(&w.pool, w.alice, 1, -1, true, 3).await;
    let made = make_due(&w.pool, &w.client, later).await.unwrap();
    assert_eq!(made, Made { made: 0, failed: 1 });
    let error: Option<String> = sqlx::query_scalar("SELECT error FROM for_you WHERE user_id = ?")
        .bind(w.alice)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert!(error.unwrap().contains("pause"));
    let after = for_listener(&w.pool, w.alice, &everything(), later)
        .await
        .unwrap();
    assert_eq!(after.items, before.items);
    assert_eq!(after.made_at, Some(NOW));
    // Not tried again within the hour, nor while ListenBrainz holds requests back.
    assert_eq!(
        make_due(&w.pool, &w.client, later + 60).await.unwrap(),
        Made::default()
    );
}

#[tokio::test]
async fn turned_off_there_is_no_list_and_nothing_is_kept() {
    let w = world().await;
    make_for(&w.pool, &w.client, w.alice, &everything(), NOW)
        .await
        .unwrap();
    sqlx::query("UPDATE settings SET value = 'off' WHERE name = 'MELETE_LISTENBRAINZ'")
        .execute(&w.pool)
        .await
        .unwrap();
    let served = for_listener(&w.pool, w.alice, &everything(), NOW)
        .await
        .unwrap();
    assert_eq!(
        served,
        Served {
            state: State::Off,
            made_at: None,
            items: Vec::new()
        }
    );
    assert_eq!(
        make_due(&w.pool, &w.client, NOW + 90_000).await.unwrap(),
        Made::default()
    );
    // What the services' setup runs while it's off.
    forget_while_off(w.pool.clone()).await;
    let kept: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM for_you) + (SELECT count(*) FROM listenbrainz_answers)",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(kept, 0);
    assert_eq!(w.log.lock().unwrap().len(), 6, "nothing asked while off");
}

fn candidate(n: u32, playable: bool, score: f64, artist: &str, release: Option<&str>) -> Candidate {
    let because = Because {
        kind: "song".into(),
        title: Some("Seed".into()),
        artist: "Seeder".into(),
    };
    let song = (!playable).then(|| Song {
        mb_recording_id: id(n),
        title: format!("Song {n}"),
        artist: artist.into(),
        mb_artist_ids: Vec::new(),
        release: None,
        release_group: release.map(str::to_owned),
    });
    Candidate {
        item: Item {
            track_id: playable.then_some(i64::from(n)),
            song,
            because,
        },
        score,
        order: if playable {
            format!("track:{n:012}")
        } else {
            format!("song:{}", id(n))
        },
        artist: artist.into(),
        release: release.map(str::to_owned),
    }
}

fn numbers(items: &[Item]) -> Vec<u32> {
    items
        .iter()
        .map(|i| match (&i.track_id, &i.song) {
            (Some(t), _) => u32::try_from(*t).unwrap(),
            (None, Some(s)) => s.mb_recording_id[24..].parse().unwrap(),
            (None, None) => 0,
        })
        .collect()
}

#[test]
fn two_by_one_artist_one_from_one_album_ties_broken_the_same_way_every_time() {
    let all = vec![
        candidate(1, true, 0.9, "a", Some("x")),
        candidate(2, true, 0.9, "a", Some("y")),
        candidate(3, true, 0.95, "a", Some("z")),
        candidate(4, false, 0.8, "b", Some("x")),
        candidate(5, false, 0.7, "b", Some("w")),
        candidate(6, false, 0.7, "c", Some("w")),
        candidate(7, false, 0.6, "c", None),
        candidate(8, false, 0.6, "c", None),
        candidate(9, false, 0.5, "c", None),
    ];
    let ranked = numbers(&rank(all.clone()));
    // 3 and 1 by a (2 would be a's third), 4's album is 1's, 6's is 5's, 9 would be c's third;
    // 1 before 2 and 5 before 6 on their ids.
    assert_eq!(ranked, [3, 1, 5, 7, 8]);
    let mut reversed = all;
    reversed.reverse();
    assert_eq!(numbers(&rank(reversed)), ranked);
}

#[test]
fn each_kind_gets_half_first_and_the_rest_goes_to_the_best() {
    let mut all = Vec::new();
    for n in 1..=30 {
        all.push(candidate(n, true, 0.9, &format!("p{n}"), None));
        all.push(candidate(100 + n, false, 0.1, &format!("r{n}"), None));
    }
    let ranked = rank(all.clone());
    assert_eq!(ranked.len(), ITEMS);
    let to_play = ranked.iter().filter(|i| i.track_id.is_some()).count();
    assert_eq!(to_play, ITEMS / 2, "half and half, however the scores fall");
    // With only five to request, the rest are to play: all 30, as 40 aren't there.
    let few: Vec<Candidate> = all
        .into_iter()
        .filter(|c| numbers(std::slice::from_ref(&c.item))[0] <= 105)
        .collect();
    let ranked = rank(few);
    assert_eq!(ranked.iter().filter(|i| i.song.is_some()).count(), 5);
    assert_eq!(ranked.len(), 35);
}
