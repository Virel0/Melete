use std::sync::atomic::Ordering;

use super::testing::{self, KEY};
use super::*;

#[test]
fn the_hash_is_the_size_and_both_ends() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.bin");
    // 128 KB of words 1, 2, 3…: the first 64 KB sum to 1 + … + 8192, the last to the rest.
    let words: Vec<u8> = (1u64..=16_384).flat_map(u64::to_le_bytes).collect();
    std::fs::write(&path, &words).unwrap();
    let expected = 131_072u64 + (1..=16_384u64).sum::<u64>();
    assert_eq!(hash(&path).unwrap(), Some(format!("{expected:016x}")));
    std::fs::write(&path, b"short").unwrap();
    assert_eq!(hash(&path).unwrap(), None, "under 64 KB");
}

#[test]
fn reset_times_are_read() {
    assert_eq!(talking::parsed_time("1970-01-01T00:00:00.000Z"), Some(0));
    assert_eq!(
        talking::parsed_time("2030-01-02T03:04:05.000Z"),
        Some(1_893_553_445)
    );
    assert_eq!(talking::parsed_time("soon"), None);
}

fn header<'a>(seen: &'a testing::Seen, name: &str) -> Option<&'a str> {
    seen.headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

#[tokio::test]
async fn searching_sends_only_whats_needed() {
    let (base, fake) = testing::start().await;
    let client = Client::with_api(&testing::settings(false), &base).unwrap();
    let wanted = Wanted {
        hash: Some("8e245d9679d31e12".into()),
        tmdb_id: Some(1399),
        imdb_id: Some("tt0944947".into()),
        episode: Some((1, 2)),
        language: "fr".into(),
    };
    let found = client.search(&wanted).await.unwrap();
    let ids: Vec<i64> = found.iter().map(|f| f.id).collect();
    assert_eq!(
        ids,
        [102, 101],
        "made for this file first, one without files left out"
    );
    assert!(found[0].matches_file);

    let seen = fake.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].query,
        "episode_number=2&languages=fr&moviehash=8e245d9679d31e12&parent_tmdb_id=1399&season_number=1",
        "sorted, and only these"
    );
    assert_eq!(header(&seen[0], "api-key"), Some(KEY));
    assert_eq!(header(&seen[0], "authorization"), None);
    let agent = header(&seen[0], "user-agent").unwrap();
    assert_eq!(agent, concat!("Melete v", env!("CARGO_PKG_VERSION")));
    let names: Vec<&str> = seen[0].headers.iter().map(|(k, _)| k.as_str()).collect();
    for name in &names {
        assert!(
            ["api-key", "accept", "user-agent", "host"].contains(name),
            "nothing else is sent: {names:?}"
        );
    }
}

#[tokio::test]
async fn a_movie_is_searched_by_its_own_ids() {
    let (base, fake) = testing::start().await;
    let client = Client::with_api(&testing::settings(false), &base).unwrap();
    let wanted = Wanted {
        hash: None,
        tmdb_id: None,
        imdb_id: Some("tt0133093".into()),
        episode: None,
        language: "pt-br".into(),
    };
    client.search(&wanted).await.unwrap();
    assert_eq!(
        fake.seen.lock().unwrap()[0].query,
        "imdb_id=0133093&languages=pt-br"
    );
}

#[tokio::test]
async fn downloads_go_under_the_login_when_theres_one() {
    let (base, fake) = testing::start().await;
    let client = Client::with_api(&testing::settings(true), &base).unwrap();
    let srt = client.download(102).await.unwrap();
    assert_eq!(
        String::from_utf8(srt).unwrap(),
        "1\n00:00:01,000 --> 00:00:04,000\nFetched 102.srt\n"
    );
    let seen = fake.seen.lock().unwrap().clone();
    // A login kept from another test may stand in for signing in here.
    let download = seen.last().unwrap();
    assert_eq!(download.path, "/download");
    assert_eq!(
        header(download, "authorization"),
        Some("Bearer login-token")
    );
    assert_eq!(download.body, r#"{"file_id":102}"#);
    for s in &seen {
        assert!(!s.query.contains(testing::PASSWORD), "never in a URL");
    }
}

#[tokio::test]
async fn used_up_downloads_say_when_they_come_back() {
    let (base, fake) = testing::start().await;
    fake.quota_reached.store(true, Ordering::Relaxed);
    let client = Client::with_api(&testing::settings(false), &base).unwrap();
    assert_eq!(
        client.download(102).await.unwrap_err(),
        Failure::Quota(1_893_553_445)
    );
}

#[tokio::test]
async fn checking_sends_the_key_and_the_login_and_nothing_else() {
    let (base, fake) = testing::start().await;
    let client = Client::with_api(&testing::settings(true), &base).unwrap();
    assert_eq!(client.check().await, Ok(()));
    let seen = fake.seen.lock().unwrap().clone();
    let paths: Vec<&str> = seen.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(paths, ["/infos/formats", "/login"]);
    for s in &seen {
        assert_eq!(header(s, "api-key"), Some(KEY));
        assert_eq!(
            header(s, "user-agent"),
            Some(concat!("Melete v", env!("CARGO_PKG_VERSION")))
        );
        assert!(s.query.is_empty(), "nothing in the address");
        for (name, _) in &s.headers {
            assert!(
                [
                    "api-key",
                    "accept",
                    "user-agent",
                    "host",
                    "content-type",
                    "content-length"
                ]
                .contains(&name.as_str()),
                "nothing else is sent: {name}"
            );
        }
    }
    assert_eq!(
        seen[1].body,
        format!(
            r#"{{"password":"{}","username":"{}"}}"#,
            testing::PASSWORD,
            testing::USER
        ),
        "the login in the body"
    );
}

#[tokio::test]
async fn a_wrong_key_or_login_is_refused() {
    let (base, _fake) = testing::start().await;
    let mut settings = testing::settings(false);
    assert_eq!(
        Client::with_api(&settings, &base).unwrap().check().await,
        Ok(())
    );
    settings.key = Secret::for_tests("wrong");
    let wrong = Client::with_api(&settings, &base).unwrap();
    assert_eq!(wrong.check().await, Err(Failure::Refused));
    let mut settings = testing::settings(true);
    settings.login = Some((testing::USER.into(), Secret::for_tests("nope")));
    let client = Client::with_api(&settings, &base).unwrap();
    assert_eq!(client.check().await, Err(Failure::Refused));
    let message = Failure::Refused.to_string();
    assert!(!message.contains("nope") && !message.contains(KEY));
}
