use super::*;

#[test]
fn keeps_to_wikimedias_hosts_over_https() {
    let ok = |u: &str| allowed(&reqwest::Url::parse(u).unwrap());
    assert!(ok("https://www.wikidata.org/w/api.php"));
    assert!(ok("https://commons.wikimedia.org/w/api.php"));
    assert!(ok(
        "https://upload.wikimedia.org/wikipedia/commons/thumb/a/ab/X.jpg/500px-X.jpg"
    ));
    assert!(!ok("http://upload.wikimedia.org/x.jpg"));
    assert!(!ok("https://upload.wikimedia.org.evil.example/x.jpg"));
    assert!(!ok("https://en.wikipedia.org/x.jpg"));
}

#[test]
fn pictures_come_from_commons_picture_hosts_only() {
    let picture = |u: &str| is_picture(&reqwest::Url::parse(u).unwrap());
    assert!(picture(
        "https://upload.wikimedia.org/wikipedia/commons/thumb/a/ab/X.jpg/500px-X.jpg"
    ));
    assert!(picture(testing::THUMB_URL));
    assert!(!picture("http://thumb.wikimedia.org/x.jpg"));
    assert!(!picture("https://thumb.wikimedia.org.evil.example/x.jpg"));
    assert!(!picture("https://commons.wikimedia.org/x.jpg"));
    assert!(!picture("https://www.wikidata.org/x.jpg"));
}

#[test]
fn only_items_and_plain_text_pass() {
    assert!(is_item("Q44190"));
    for bad in ["Q", "Q0", "Q01", "P18", "Q1 2", "q5", "Q12345678901234567"] {
        assert!(!is_item(bad), "{bad}");
    }
    assert_eq!(
        plain("<a href=\"x\">Some &amp; One</a>,\n <i>own work</i>").as_deref(),
        Some("Some & One, own work")
    );
    assert_eq!(
        plain("One<br/>Two<br>Three").as_deref(),
        Some("One Two Three")
    );
    assert_eq!(plain("<span></span>"), None);
    assert_eq!(plain(&"x".repeat(500)).map(|t| t.len()), Some(MAX_TEXT));
}

#[tokio::test]
async fn finds_an_items_image_and_its_picture_with_its_credit() {
    let (client, log) = testing::start().await;
    let file = client.image_of(testing::ITEM).await.unwrap();
    assert_eq!(
        file.as_deref(),
        Some(testing::FILE),
        "not the deprecated one"
    );
    assert_eq!(client.image_of(testing::BARE_ITEM).await.unwrap(), None);
    assert!(client.image_of("../x").await.is_err());

    let found = client.picture(testing::FILE).await.unwrap().unwrap();
    assert_eq!(
        (
            found.author.as_deref(),
            found.licence.as_deref(),
            found.licence_url.as_deref()
        ),
        (
            Some("Some & One"),
            Some("CC BY-SA 4.0"),
            Some("https://creativecommons.org/licenses/by-sa/4.0")
        )
    );
    assert!(found.url.ends_with("/thumb/500px.jpg"));
    assert_eq!(client.picture("Nowhere.jpg").await.unwrap(), None);
    let (bytes, kind) = client.download(&found.url).await.unwrap();
    assert_eq!(
        (bytes.as_slice(), kind.as_str()),
        (testing::JPEG, "image/jpeg")
    );
    assert_eq!(
        *log.lock().unwrap(),
        [
            "wikidata Q44190",
            "wikidata Q2",
            "commons File:Radiohead in concert.jpg",
            "commons File:Nowhere.jpg",
            "picture 500px.jpg"
        ],
        "only the item's id, the file's name, and the picture"
    );
}

#[tokio::test]
async fn refuses_failures_large_pictures_and_other_hosts_without_saying_where() {
    let (client, _) = testing::start().await;
    let failed = client.picture("Failing.jpg").await.unwrap_err();
    assert!(format!("{failed:#}").contains("500"), "{failed:#}");
    let large = client.picture("Large.jpg").await.unwrap().unwrap();
    let too_large = client.download(&large.url).await.unwrap_err();
    assert!(
        format!("{too_large:#}").contains("too large"),
        "{too_large:#}"
    );

    // The redirect leads somewhere that answers: followed, it finds the image.
    let followed = client.image_of(testing::MOVED_ITEM).await.unwrap();
    assert_eq!(followed.as_deref(), Some(testing::FILE));

    // As the real client: a redirect off Wikimedia isn't followed, a picture not on Commons'
    // picture hosts isn't given or fetched, and no error says where it pointed.
    let (strict, log) = testing::start_with(false).await;
    let moved = strict.image_of(testing::MOVED_ITEM).await.unwrap_err();
    assert!(!format!("{moved:#}").contains("127.0.0.1"), "{moved:#}");
    assert_eq!(
        log.lock().unwrap().as_slice(),
        ["wikidata Q5"],
        "the redirect wasn't followed"
    );
    assert_eq!(strict.picture(testing::FILE).await.unwrap(), None);
    // Commons gives resized pictures on thumb.wikimedia.org now: one there is given.
    let moved = strict.picture(testing::THUMB_FILE).await.unwrap().unwrap();
    assert_eq!(moved.url, testing::THUMB_URL);
    let elsewhere = strict.download(&large.url).await.unwrap_err();
    assert!(
        !format!("{elsewhere:#}").contains("127.0.0.1"),
        "{elsewhere:#}"
    );
    assert!(
        !log.lock().unwrap().iter().any(|l| l.starts_with("picture")),
        "nothing fetched from elsewhere"
    );
}

#[tokio::test]
async fn keeps_to_one_request_a_turn() {
    let (client, log) = testing::start().await;
    let client = client.spaced(std::time::Duration::from_millis(300));
    let started = std::time::Instant::now();
    for _ in 0..3 {
        client.image_of(testing::BARE_ITEM).await.unwrap();
    }
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(600),
        "three turns, 300 ms apart: {:?}",
        started.elapsed()
    );
    assert_eq!(log.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn waits_out_a_short_pause_and_tries_again() {
    let (client, log) = testing::start().await;
    let started = std::time::Instant::now();
    let file = client.image_of(testing::PAUSED_ITEM).await.unwrap();
    assert_eq!(file.as_deref(), Some(testing::FILE));
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(1),
        "the second ask waited the second asked for: {:?}",
        started.elapsed()
    );
    assert_eq!(log.lock().unwrap().len(), 2);
    assert!(!client.held(), "a short pause isn't a hold");
}

#[tokio::test]
async fn a_long_pause_holds_every_request_back_without_sending_them() {
    let (client, log) = testing::start().await;
    let held = client.image_of(testing::HELD_ITEM).await.unwrap_err();
    assert_eq!(held.downcast_ref::<Busy>(), Some(&Busy), "{held:#}");
    assert!(client.held());
    // Neither another item, Commons nor a picture is asked while the hour lasts.
    let other = client.image_of(testing::ITEM).await.unwrap_err();
    assert!(other.downcast_ref::<Busy>().is_some(), "{other:#}");
    let picture = testing::picture_url(&client, "500px.jpg");
    assert!(client.download(&picture).await.unwrap_err().is::<Busy>());
    assert!(
        client
            .picture(testing::FILE)
            .await
            .unwrap_err()
            .is::<Busy>()
    );
    assert_eq!(
        *log.lock().unwrap(),
        ["wikidata Q7"],
        "asked once, then held"
    );
}

#[tokio::test]
async fn a_picture_asked_too_often_holds_the_rest_back() {
    let (client, log) = testing::start().await;
    let held = testing::picture_url(&client, testing::HELD_PICTURE);
    assert!(client.download(&held).await.unwrap_err().is::<Busy>());
    assert!(client.held());
    assert!(
        client
            .image_of(testing::ITEM)
            .await
            .unwrap_err()
            .is::<Busy>()
    );
    assert_eq!(*log.lock().unwrap(), ["picture held.jpg"]);
}

#[tokio::test]
async fn too_many_waiting_their_turn_are_refused_rather_than_kept_waiting() {
    let (client, log) = testing::start().await;
    // At eight seconds a turn, only the turns within the longest wait (15 s) are taken.
    let client = std::sync::Arc::new(client.spaced(std::time::Duration::from_secs(8)));
    let asks: Vec<_> = (0..8)
        .map(|_| {
            let client = client.clone();
            tokio::spawn(async move { client.image_of(testing::BARE_ITEM).await })
        })
        .collect();
    let mut busy = 0;
    for ask in asks {
        match ask.await.unwrap() {
            Ok(_) => {}
            Err(err) if err.is::<Busy>() => busy += 1,
            Err(err) => panic!("{err:#}"),
        }
    }
    let sent = log.lock().unwrap().len();
    assert_eq!(sent + busy, 8);
    assert_eq!(
        sent, 2,
        "turns at 0 and 8 seconds; the next is past the wait"
    );
    assert!(!client.held(), "nobody asked for a pause");
}
