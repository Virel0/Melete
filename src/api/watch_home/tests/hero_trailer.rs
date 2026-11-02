//! Where each hero pick's trailer comes from.

use super::*;

/// The hero pick `kind:id`'s trailer.
fn trailer_of<'a>(home: &'a Value, title: &str) -> &'a Value {
    let pick = home["hero"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| format!("{}:{}", p["kind"].as_str().unwrap(), p["id"]) == title)
        .unwrap_or_else(|| panic!("{title} isn't a pick in {home}"));
    &pick["trailer"]
}

#[tokio::test]
async fn hero_picks_say_which_trailer_they_have() {
    let f = fixture().await;
    // Film A has one in its folder; no other title has one.
    sqlx::query(
        "INSERT INTO media_extras (id, library_id, rel_path, size, modified_at, movie_id, kind,
             title, first_seen_generation, seen_generation)
             VALUES (1099511627776, 3, 'Film A (1999)/a-trailer.mkv', 1, 1, 1, 'trailer',
                 'Film A', 1, 1);",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let (_, token) = user(&f, "viewer").await;
    let home = home_of(&f, &token).await;
    assert_eq!(
        trailer_of(&home, "movie:1"),
        &json!({ "source": "folder", "file_id": 1099511627776_i64 })
    );
    assert_eq!(trailer_of(&home, "series:1"), &Value::Null, "none");
    assert_eq!(trailer_of(&home, "movie:2"), &Value::Null, "none");
    // Rows' items carry no trailer.
    assert!(row(&home, "latest_movies")[1].get("trailer").is_none());
}

/// Film A's trailer at `trailer` lines, the film itself at `film`.
async fn heights(f: &Fixture, trailer: i64, film: i64) {
    let probe = |h: i64| json!({ "video": [{ "height": h }] }).to_string();
    sqlx::query(
        "INSERT INTO media_extras (id, library_id, rel_path, size, modified_at, movie_id, kind,
             title, first_seen_generation, seen_generation, probed_size, probed_modified_at,
             probe_info)
             VALUES (1099511627776, 3, 'Film A (1999)/a-trailer.mkv', 1, 1, 1, 'trailer',
                 'Film A', 1, 1, 1, 1, ?1)
         ON CONFLICT (id) DO UPDATE SET probe_info = excluded.probe_info;
         INSERT INTO media_probes (file_id, size, modified_at, probed_at, info)
             VALUES (6, 1, 1, 1, ?2)
         ON CONFLICT (file_id) DO UPDATE SET info = excluded.info;",
    )
    .bind(probe(trailer))
    .bind(probe(film))
    .execute(&f.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_soft_trailer_gives_way_to_a_clip_of_the_title() {
    let f = fixture().await;
    let (_, token) = user(&f, "viewer").await;
    let folder = json!({ "source": "folder", "file_id": 1099511627776_i64 });
    let trailer = |trailer, film| {
        let (f, token) = (&f, &token);
        async move {
            heights(f, trailer, film).await;
            trailer_of(&home_of(f, token).await, "movie:1").clone()
        }
    };
    assert_eq!(
        trailer(720, 2160).await,
        Value::Null,
        "720 behind a 4K film"
    );
    assert_eq!(trailer(1080, 2160).await, folder, "1080 is sharp enough");
    assert_eq!(trailer(2160, 2160).await, folder);
    assert_eq!(
        trailer(720, 720).await,
        folder,
        "as sharp as the film itself"
    );
    assert_eq!(trailer(480, 720).await, Value::Null);
}

/// Film A's trailer at `trailer` lines, the film itself at `film`.
async fn heights(f: &Fixture, trailer: i64, film: i64) {
    let probe = |h: i64| json!({ "video": [{ "height": h }] }).to_string();
    sqlx::query(
        "INSERT INTO media_extras (id, library_id, rel_path, size, modified_at, movie_id, kind,
             title, first_seen_generation, seen_generation, probed_size, probed_modified_at,
             probe_info)
             VALUES (1099511627776, 3, 'Film A (1999)/a-trailer.mkv', 1, 1, 1, 'trailer',
                 'Film A', 1, 1, 1, 1, ?1)
         ON CONFLICT (id) DO UPDATE SET probe_info = excluded.probe_info;
         INSERT INTO media_probes (file_id, size, modified_at, probed_at, info)
             VALUES (6, 1, 1, 1, ?2)
         ON CONFLICT (file_id) DO UPDATE SET info = excluded.info;",
    )
    .bind(probe(trailer))
    .bind(probe(film))
    .execute(&f.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_soft_trailer_gives_way_to_a_clip_of_the_title() {
    let f = fixture().await;
    let (_, token) = user(&f, "viewer").await;
    let folder = json!({ "source": "folder", "file_id": 1099511627776_i64, "embed_url": null });
    let trailer = |trailer, film| {
        let (f, token) = (&f, &token);
        async move {
            heights(f, trailer, film).await;
            trailer_of(&home_of(f, token).await, "movie:1").clone()
        }
    };
    assert_eq!(
        trailer(720, 2160).await,
        Value::Null,
        "720 behind a 4K film"
    );
    assert_eq!(trailer(1080, 2160).await, folder, "1080 is sharp enough");
    assert_eq!(trailer(2160, 2160).await, folder);
    assert_eq!(
        trailer(720, 720).await,
        folder,
        "as sharp as the film itself"
    );
    assert_eq!(trailer(480, 720).await, Value::Null);
}
