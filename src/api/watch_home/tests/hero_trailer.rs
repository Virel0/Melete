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
async fn hero_picks_say_where_their_trailer_comes_from() {
    let f = fixture().await;
    // Film A has one in its folder (and online); Show has one online only.
    let videos = json!([{ "key": "vKQi3bBA1y8", "name": "Official Trailer", "language": "en",
        "kind": "Trailer", "official": true, "size": 1080, "published_at": null }])
    .to_string();
    sqlx::query(
        "INSERT INTO media_extras (id, library_id, rel_path, size, modified_at, movie_id, kind,
             title, first_seen_generation, seen_generation)
             VALUES (1099511627776, 3, 'Film A (1999)/a-trailer.mkv', 1, 1, 1, 'trailer',
                 'Film A', 1, 1);
         UPDATE tmdb_movies SET videos = ?1 WHERE tmdb_id = 603;
         UPDATE tmdb_series SET videos = ?1 WHERE tmdb_id = 10;",
    )
    .bind(&videos)
    .execute(&f.pool)
    .await
    .unwrap();
    let (_, token) = user(&f, "viewer").await;
    let folder = json!({ "source": "folder", "file_id": 1099511627776_i64, "embed_url": null });

    let home = home_of(&f, &token).await;
    assert_eq!(trailer_of(&home, "movie:1"), &folder);
    assert_eq!(
        trailer_of(&home, "series:1"),
        &Value::Null,
        "online only while trailers online are on"
    );
    assert_eq!(trailer_of(&home, "movie:2"), &Value::Null, "none");
    // Rows' items carry no trailer.
    assert!(row(&home, "latest_movies")[1].get("trailer").is_none());

    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_TRAILERS_ONLINE', 'on', 0)",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let home = home_of(&f, &token).await;
    assert_eq!(trailer_of(&home, "movie:1"), &folder, "the folder's first");
    assert_eq!(
        trailer_of(&home, "series:1"),
        &json!({ "source": "online", "file_id": null,
                 "embed_url": "https://www.youtube-nocookie.com/embed/vKQi3bBA1y8" })
    );
}
