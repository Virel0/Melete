use std::time::{Duration, Instant};

use super::testing::{self, Answers};
use super::*;

const SEED: &str = "9e2ad5bc-c6f9-40d2-a36f-3122ee2072a3";
const ALIKE: &str = "d201070b-ff15-49ce-877b-f91032617faf";
const ARTIST: &str = "a74b1b7f-71a5-4011-9441-d0b5e4122711";
const OTHER_ARTIST: &str = "b6b2bb8d-54a9-491f-9607-7b546023b433";
const GROUP: &str = "74e36cbc-a747-3ebf-a60e-51e656c87741";

#[test]
fn keeps_to_listenbrainz_hosts_over_https() {
    let ok = |u: &str| allowed(&reqwest::Url::parse(u).unwrap());
    assert!(ok("https://labs.api.listenbrainz.org/similar-artists/json"));
    assert!(ok("https://api.listenbrainz.org/1/metadata/recording/"));
    assert!(!ok("http://api.listenbrainz.org/1/metadata/recording/"));
    assert!(!ok("https://listenbrainz.org/user/someone"));
    assert!(!ok("https://api.listenbrainz.org.evil.example/1/"));
    assert!(!ok("https://musicbrainz.org/ws/2/"));
}

fn answers() -> Answers {
    let mut answers = Answers::default();
    answers.recordings.insert(
        SEED.into(),
        vec![
            testing::recording(ALIKE, "Where Is My Mind?", "Pixies", "Surfer Rosa", 900),
            testing::recording("not an id", "Broken", "Nobody", "None", 2000),
            testing::recording(GROUP, "Closer", "Pixies", "Doolittle", 1200),
        ],
    );
    answers.artists.insert(
        ARTIST.into(),
        vec![
            testing::artist(ARTIST, "Itself", 5000),
            testing::artist(OTHER_ARTIST, "Pixies", 11),
        ],
    );
    answers.metadata.insert(
        ALIKE.into(),
        testing::metadata(&[(OTHER_ARTIST, "Pixies")], "Surfer Rosa", GROUP),
    );
    answers
}

#[tokio::test]
async fn asks_by_musicbrainz_ids_alone_and_reads_the_answers() {
    let (client, log) = testing::start(answers()).await;
    let alike = client.similar_recordings(SEED).await.unwrap();
    let ids: Vec<&str> = alike.iter().map(|r| r.recording_mbid.as_str()).collect();
    assert_eq!(ids, [GROUP, ALIKE], "closest first, and only real ids");
    assert_eq!(alike[1].recording_name, "Where Is My Mind?");
    assert_eq!(alike[1].artist_credit_name, "Pixies");
    assert_eq!(alike[1].release_name.as_deref(), Some("Surfer Rosa"));

    let artists = client.similar_artists(ARTIST).await.unwrap();
    assert_eq!(artists.len(), 1, "never the artist asked about");
    assert_eq!(
        (artists[0].artist_mbid.as_str(), artists[0].score),
        (OTHER_ARTIST, 11.0)
    );

    let asked = [ALIKE.to_owned(), SEED.to_owned()];
    let known = client.recordings(&asked).await.unwrap();
    assert_eq!(known.len(), 1, "only those it knows");
    assert_eq!(
        known[ALIKE],
        Recording {
            artist_mbids: vec![OTHER_ARTIST.into()],
            artist: Some("Pixies".into()),
            release: Some("Surfer Rosa".into()),
            release_group_mbid: Some(GROUP.into()),
        }
    );
    assert!(client.recordings(&[]).await.unwrap().is_empty());
    assert_eq!(
        *log.lock().unwrap(),
        [
            format!("similar-recordings recording_mbids={SEED} algorithm={RECORDINGS_ALGORITHM}"),
            format!("similar-artists artist_mbids={ARTIST} algorithm={ARTISTS_ALGORITHM}"),
            format!("metadata recording_mbids={ALIKE},{SEED} inc=artist release"),
        ],
        "MusicBrainz ids and the fixed parameters, nothing else, and nothing to say who asked"
    );
}

#[tokio::test]
async fn sends_nothing_that_isnt_a_musicbrainz_id() {
    let (client, log) = testing::start(answers()).await;
    assert!(client.similar_recordings("someone's name").await.is_err());
    assert!(client.similar_artists("../1/user").await.is_err());
    assert!(client.recordings(&["x".into()]).await.is_err());
    let too_many = vec![SEED.to_owned(); METADATA_BATCH + 1];
    assert!(client.recordings(&too_many).await.is_err());
    assert!(log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn waits_out_a_short_pause_and_tries_again() {
    let (client, log) = testing::start(answers()).await;
    let started = Instant::now();
    let found = client.similar_recordings(testing::PAUSED).await.unwrap();
    assert!(found.is_empty());
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "the second waited"
    );
    assert_eq!(log.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_long_pause_holds_every_request_back() {
    let (client, log) = testing::start(answers()).await;
    let held = client.similar_recordings(testing::HELD).await.unwrap_err();
    assert!(held.downcast_ref::<Busy>().is_some(), "{held:#}");
    assert!(client.held());
    let after = client.similar_artists(ARTIST).await.unwrap_err();
    assert!(after.downcast_ref::<Busy>().is_some(), "{after:#}");
    assert_eq!(log.lock().unwrap().len(), 1, "nothing sent while held");
}

#[tokio::test]
async fn no_requests_left_holds_the_next_until_the_limit_resets() {
    let (client, log) = testing::start(answers()).await;
    client.similar_recordings(testing::SPENT).await.unwrap();
    let started = Instant::now();
    client.similar_artists(ARTIST).await.unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "held for the second X-RateLimit-Reset-In said: {:?}",
        started.elapsed()
    );
    assert_eq!(log.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn keeps_to_one_request_a_turn() {
    let (client, log) = testing::start(answers()).await;
    let client = client.spaced(Duration::from_millis(300));
    let started = Instant::now();
    for _ in 0..3 {
        client.similar_artists(ARTIST).await.unwrap();
    }
    assert!(started.elapsed() >= Duration::from_millis(600));
    assert_eq!(log.lock().unwrap().len(), 3);
}
