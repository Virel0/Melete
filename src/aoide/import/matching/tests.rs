use super::*;

/// A library with the tracks the tests look for, as `(title, artist tag, album, seconds)`, the
/// albums by their title, and `credits` as (track id, credited artist) for Melete's own credits.
async fn library(tracks: &[(&str, &str, &str, f64)], credits: &[(i64, &str)]) -> Library {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m')",
    )
    .execute(&pool)
    .await
    .unwrap();
    for (n, (title, artist, album, seconds)) in tracks.iter().enumerate() {
        let id = n as i64 + 1;
        sqlx::query(
            "INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                 seen_generation) VALUES (?1, 1, ?2, ?3, ?4, ?2, 1)",
        )
        .bind(id)
        .bind(format!("k{id}"))
        .bind(album)
        .bind(artist)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, artist, album,
                 duration_seconds) VALUES (?1, 1, ?2, 1, 1, 1, 1, ?1, 1, ?3, ?4, ?5, ?6)",
        )
        .bind(id)
        .bind(format!("t/{id}"))
        .bind(title)
        .bind(artist)
        .bind(album)
        .bind(seconds)
        .execute(&pool)
        .await
        .unwrap();
    }
    for (position, (track, name)) in credits.iter().enumerate() {
        sqlx::query("INSERT INTO music_artists (id, key, name) VALUES (?1, ?2, ?3)")
            .bind(100 + position as i64)
            .bind(format!("name:{name}"))
            .bind(name)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO music_track_artists (track_id, position, artist_id, name)
             VALUES (?, ?, ?, ?)",
        )
        .bind(track)
        .bind(position as i64)
        .bind(100 + position as i64)
        .bind(name)
        .execute(&pool)
        .await
        .unwrap();
    }
    Library::load(&pool).await.unwrap()
}

#[test]
fn a_name_credits_each_artist_it_lists() {
    assert_eq!(credited("Björk"), ["Björk"]);
    assert_eq!(
        credited("Massive Attack; Tracey Thorn"),
        [
            "Massive Attack; Tracey Thorn",
            "Massive Attack",
            "Tracey Thorn"
        ]
    );
    assert_eq!(
        credited("A, B & C feat. D"),
        ["A, B & C feat. D", "A", "B", "C", "D"]
    );
}

#[tokio::test]
async fn a_track_credited_to_several_artists_is_found_by_any_of_them() {
    let library = library(
        &[
            (
                "Protection",
                "Massive Attack; Tracey Thorn",
                "Protection",
                471.0,
            ),
            ("Teardrop", "Massive Attack", "Mezzanine", 330.0),
        ],
        &[(2, "Massive Attack"), (2, "Elizabeth Fraser")],
    )
    .await;
    for key in [
        "massive attack|protection|protection|471",
        "tracey thorn|protection|protection|471",
        "massive attack, tracey thorn|protection|protection|471",
        "massive attack & tracey thorn|protection|protection|471",
    ] {
        assert_eq!(library.find(key), Found::Exact(1), "{key}");
    }
    // Credited by Melete, not in the tag.
    assert_eq!(
        library.find("elizabeth fraser|mezzanine|teardrop|330"),
        Found::Exact(2)
    );
    assert_eq!(
        library.find("massive attack feat. elizabeth fraser|mezzanine|teardrop|330"),
        Found::Exact(2)
    );
    assert_eq!(
        library.find("someone else|mezzanine|teardrop|330"),
        Found::Missing
    );
}

#[tokio::test]
async fn a_key_whose_album_fits_none_is_found_on_another_and_ties_are_broken() {
    let library = library(
        &[
            ("Army of Me", "Björk", "Post", 234.0),
            ("Army of Me", "Björk", "Greatest Hits", 235.0),
            ("Hunter", "Björk", "Homogenic", 255.0),
            ("Joga", "Björk", "Homogenic", 305.0),
            ("Joga", "Björk", "Live", 305.0),
        ],
        &[],
    )
    .await;
    // One of that title within a second, on another album.
    assert_eq!(
        library.find("bjork|homogenic (deluxe)|hunter|256"),
        Found::Near(3)
    );
    // Two: the album whose name holds the key's, or is held in it.
    assert_eq!(
        library.find("bjork|post (remastered)|army of me|235"),
        Found::Near(1)
    );
    // No album fits: the one of exactly the length.
    assert_eq!(
        library.find("bjork|telegram|army of me|234"),
        Found::Near(1)
    );
    // Same length, neither album fits: still ambiguous.
    assert_eq!(library.find("bjork|vespertine|joga|305"), Found::Ambiguous);
    // Too far off in length, or another title: nothing.
    assert_eq!(
        library.find("bjork|telegram|army of me|240"),
        Found::Missing
    );
    assert_eq!(library.find("bjork|post|isobel|340"), Found::Missing);
}
