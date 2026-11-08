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
