//! Artist results get a portrait from Commons, with its credit, only with portraits on.

use super::*;
use crate::musicbrainz::testing::{RADIOHEAD, TRIBUTE_ACT};
use crate::wikimedia::testing as commons;

const ARTISTS: &str = "/api/v1/requests/search?kind=artist&q=Radiohead";

fn by_id<'a>(found: &'a Value, id: &str) -> &'a Value {
    found
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["mb_artist_id"] == id)
        .unwrap()
}

#[tokio::test]
async fn artists_get_a_portrait_with_its_credit_only_when_on() {
    let pool = db::open_in_memory().await;
    let alice = token(&pool, "alice", false).await;
    let (mb, mb_log) = crate::musicbrainz::testing::start(0).await;
    let (wiki, wiki_log) = commons::start().await;
    let app = routes_with(
        pool.clone(),
        Some(Arc::new(mb)),
        Sources::default(),
        Some(Arc::new(wiki)),
    );
    let (status, found) = send(&app, "GET", ARTISTS, &alice, None).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(by_id(&found, RADIOHEAD)["poster"], Value::Null);
    assert_eq!(by_id(&found, RADIOHEAD)["poster_credit"], Value::Null);
    assert!(wiki_log.lock().unwrap().is_empty(), "off: nothing asked");
    let searched = mb_log.lock().unwrap().len();

    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_MUSICBRAINZ', 'on', 0),
             ('MELETE_ARTIST_PORTRAITS', 'on', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    for _ in 0..2 {
        let (_, found) = send(&app, "GET", ARTISTS, &alice, None).await;
        let radiohead = by_id(&found, RADIOHEAD);
        assert_eq!(
            radiohead["poster"],
            json!(format!("/api/v1/music/artist-portraits/{RADIOHEAD}"))
        );
        let credit = &radiohead["poster_credit"];
        assert_eq!(
            (
                &credit["author"],
                &credit["licence"],
                &credit["licence_url"]
            ),
            (
                &json!("Some & One"),
                &json!("CC BY-SA 4.0"),
                &json!("https://creativecommons.org/licenses/by-sa/4.0")
            )
        );
        assert!(
            credit["source_url"]
                .as_str()
                .unwrap()
                .contains("/wiki/File:")
        );
        let tribute = by_id(&found, TRIBUTE_ACT);
        assert_eq!(
            (&tribute["poster"], &tribute["poster_credit"]),
            (&Value::Null, &Value::Null)
        );
    }
    // Each artist looked up once (beside the two searches), then kept.
    let lookups = mb_log.lock().unwrap().len() - searched;
    assert_eq!(lookups, 2 + 2);
    assert_eq!(
        wiki_log.lock().unwrap().len(),
        2,
        "Radiohead's item, then its file"
    );
    // Albums and songs carry no credit.
    let (_, albums) = send(
        &app,
        "GET",
        "/api/v1/requests/search?kind=album&q=OK",
        &alice,
        None,
    )
    .await;
    assert_eq!(albums[0]["poster_credit"], Value::Null);
}
