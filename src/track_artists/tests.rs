use super::*;

const A: &str = "a74b1b7f-71a5-4011-9441-d0b5e4122711";
const B: &str = "8f6bd1e4-fbe1-4f50-aa9b-94c450ec0f11";
const C: &str = "0383dadf-2a4e-4d10-a46a-e9e041da8eb3";

fn whole(name: &str, ids: &[&str]) -> Said {
    Said::Whole(
        name.to_owned(),
        ids.iter().map(|i| (*i).to_owned()).collect(),
    )
}

fn names(credits: &[Credit]) -> Vec<(&str, Option<&str>)> {
    credits
        .iter()
        .map(|c| (c.name.as_str(), c.mb_artist_id.as_deref()))
        .collect()
}

fn alone(said: &Said) -> Vec<Credit> {
    credits(said, &Known::default(), None)
}

#[test]
fn names_with_slashes_commas_and_ampersands_stay_whole() {
    for name in [
        "AC/DC",
        "Earth, Wind & Fire",
        "Simon & Garfunkel",
        "Crosby, Stills, Nash and Young",
        "Sunn O)))",
        "Mötley Crüe; Live",
    ] {
        assert_eq!(names(&alone(&whole(name, &[]))), [(name, None)], "{name}");
    }
    assert_eq!(
        featuring_parts("Earth, Wind & Fire"),
        ["Earth, Wind & Fire"]
    );
}

#[test]
fn featuring_splits_only_on_evidence() {
    // Parts that match the ids one for one.
    let paired = whole("Alder feat. Birch", &[A, B]);
    assert_eq!(
        names(&alone(&paired)),
        [("Alder", Some(A)), ("Birch", Some(B))]
    );
    // No ids and parts nobody knows: left whole.
    let unknown = whole("Alder feat. Birch", &[]);
    assert_eq!(names(&alone(&unknown)), [("Alder feat. Birch", None)]);
    // Parts every one of which is known apart.
    let mut known = Known::default();
    known.learn("alder", None);
    known.learn("BIRCH", None);
    assert_eq!(
        names(&credits(&whole("Alder Ft. Birch", &[]), &known, None)),
        [("Alder", None), ("Birch", None)]
    );
    // One of them unknown: whole.
    assert_eq!(
        names(&credits(&whole("Alder featuring Cedar", &[]), &known, None)),
        [("Alder featuring Cedar", None)]
    );
    // One id is the main artist's.
    assert_eq!(
        names(&alone(&whole("Alder feat. Birch", &[A]))),
        [("Alder", Some(A))]
    );
    assert_eq!(
        featuring_parts("A featuring B ft. C"),
        ["A", "B", "C"],
        "every separator"
    );
    assert_eq!(featuring_parts("Feat. A"), ["Feat. A"], "no part before it");
}

#[test]
fn ids_that_cant_be_paired_name_artists_only_when_all_are_known() {
    let duet = whole("Alder & Birch", &[A, B]);
    assert_eq!(names(&alone(&duet)), [("Alder & Birch", None)]);
    let mut known = Known::default();
    known.learn("Alder", Some(A));
    assert_eq!(
        names(&credits(&duet, &known, None)),
        [("Alder & Birch", None)]
    );
    known.learn("Birch", Some(B));
    assert_eq!(
        names(&credits(&duet, &known, None)),
        [("Alder", Some(A)), ("Birch", Some(B))]
    );
}

fn track(artist: Option<&str>, listed: &[&str], ids: &[&str]) -> Track {
    Track {
        artist: artist.map(str::to_owned),
        artist_names: serde_json::to_string(listed).unwrap(),
        mb_artist_ids: serde_json::to_string(ids).unwrap(),
        ..Track::default()
    }
}

#[test]
fn a_multi_value_tag_lists_artists_apart() {
    let two = said(&track(Some("Alder & Birch"), &["Alder", "Birch"], &[A, B]));
    assert_eq!(
        names(&alone(&two)),
        [("Alder", Some(A)), ("Birch", Some(B))]
    );
    // Ids that don't pair one for one are left out rather than given to the wrong name.
    let one_id = said(&track(Some("Alder & Birch"), &["Alder", "Birch"], &[A]));
    assert_eq!(names(&alone(&one_id)), [("Alder", None), ("Birch", None)]);
    // One value is the same as the artist tag.
    assert_eq!(
        said(&track(Some("Alder"), &["Alder"], &[A])),
        whole("Alder", &[A])
    );
    assert_eq!(said(&track(None, &[], &[])), Said::Untagged);
    assert_eq!(
        names(&credits(&Said::Untagged, &Known::default(), Some("Alder"))),
        [("Alder", None)],
        "an untagged track is its album artist's"
    );
}

#[test]
fn musicbrainz_credits_come_first() {
    let mut t = track(Some("Alder feat. Birch"), &[], &[]);
    t.mb_credit = Some(
        json_credit(&[("Alder", A, "Alder"), ("Birch Trio", B, "The Birch Trio")]).to_string(),
    );
    let credits = alone(&said(&t));
    assert_eq!(
        names(&credits),
        [("Alder", Some(A)), ("Birch Trio", Some(B))]
    );
    assert_eq!(credits[1].own_name.as_deref(), Some("The Birch Trio"));
}

fn json_credit(credits: &[(&str, &str, &str)]) -> serde_json::Value {
    serde_json::Value::from(
        credits
            .iter()
            .map(|(name, id, artist)| {
                serde_json::json!({ "name": name, "join": "", "id": id, "artist": artist })
            })
            .collect::<Vec<_>>(),
    )
}

/// Each track's artists by title: credited name and artist id.
async fn credited(pool: &SqlitePool) -> Vec<(String, Vec<(String, i64)>)> {
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT t.title, ta.name, ta.artist_id FROM music_track_artists ta
         JOIN music_tracks t ON t.id = ta.track_id ORDER BY t.title, ta.position",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let mut out: Vec<(String, Vec<(String, i64)>)> = Vec::new();
    for (title, name, id) in rows {
        match out.last_mut() {
            Some((t, list)) if *t == title => list.push((name, id)),
            _ => out.push((title, vec![(name, id)])),
        }
    }
    out
}

#[tokio::test]
async fn links_credits_to_album_artists_known_names_and_ids_and_keeps_tracks() {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
           INSERT INTO music_artists (id, key, name, mb_artist_id) VALUES
               (1, 'name:alder', 'Alder', NULL), (2, 'mb:{A}', 'Rowan', '{A}');
           INSERT INTO mb_releases (id, release_group_id, title, artist_credit, fetched_at)
               VALUES ('rel', 'rg', 'Rowan Live', '[]', 0);
           INSERT INTO mb_release_tracks (release_id, disc, position, title, track_id,
               recording_id, artist_credit) VALUES ('rel', 1, 1, 'Duet', 'tr-1', 'rec-1',
               '{credit}');
           INSERT INTO music_albums (id, library_id, key, title, artist_id, artist_name, folder,
               seen_generation, compilation, match_status, match_release_id) VALUES
               (1, 1, 'k1', 'Alder Songs', 1, 'Alder', 'a', 1, 0, NULL, NULL),
               (2, 1, 'k2', 'Mixtape', NULL, 'Various Artists', 'm', 1, 1, NULL, NULL),
               (3, 1, 'k3', 'Rowan Live', 2, 'Rowan', 'r', 1, 0, 'matched', 'rel');
           INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
               first_seen_generation, seen_generation, album_id, tagged, title, artist,
               artist_names, mb_artist_ids, match_track_id) VALUES
               (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'One', 'Alder', '[]', '["{C}"]', NULL),
               (2, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Two', NULL, '[]', '[]', NULL),
               (3, 1, 'a/3', 1, 1, 1, 1, 1, 1, 'Three', 'Alder feat. Birch', '[]', '[]', NULL),
               (4, 1, 'm/1', 1, 1, 1, 1, 2, 1, 'Four', 'Birch', '[]', '[]', NULL),
               (5, 1, 'm/2', 1, 1, 1, 1, 2, 1, 'Five', 'Cedar feat. Elm', '[]', '[]', NULL),
               (6, 1, 'm/3', 1, 1, 1, 1, 2, 1, 'Six', 'alder; ROWAN', '["alder","ROWAN"]',
                '[]', NULL),
               (7, 1, 'r/1', 1, 1, 1, 1, 3, 1, 'Duet', 'Rowan & Birch', '[]', '[]', 'tr-1');"#,
        credit = json_credit(&[("Rowan", A, "Rowan"), ("Birch", B, "Birch")]),
    )))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(refresh(&pool).await.unwrap(), 7);
    let birch: i64 = sqlx::query_scalar("SELECT id FROM music_artists WHERE key = ?")
        .bind(format!("mb:{B}"))
        .fetch_one(&pool)
        .await
        .unwrap();
    let named = |name: &str| (name.to_owned(), 0);
    let mut found = credited(&pool).await;
    // Name-only artists made here have new ids; compare those by name.
    for (_, list) in &mut found {
        for (_, id) in list
            .iter_mut()
            .filter(|(_, id)| ![1, 2, birch].contains(id))
        {
            *id = 0;
        }
    }
    assert_eq!(
        found,
        [
            // MusicBrainz's credit over the tags' "Rowan & Birch": both by id.
            (
                "Duet".into(),
                vec![("Rowan".into(), 2), ("Birch".into(), birch)]
            ),
            // Parts nobody knows apart: whole.
            ("Five".into(), vec![named("Cedar feat. Elm")]),
            // A name alone: the one artist of that name, made by an id on a later track.
            ("Four".into(), vec![("Birch".into(), birch)]),
            // An id no row had, spelled as the album artist with no id: the album artist.
            ("One".into(), vec![("Alder".into(), 1)]),
            // Listed apart, on a compilation: each the one artist of that name.
            ("Six".into(), vec![("alder".into(), 1), ("ROWAN".into(), 2)]),
            // Both parts known apart: split, to the album artist and to Birch.
            (
                "Three".into(),
                vec![("Alder".into(), 1), ("Birch".into(), birch)]
            ),
            // No artist tag: the album's artist.
            ("Two".into(), vec![("Alder".into(), 1)]),
        ]
    );
    // Every artist, the ones made here too, has its folded search key.
    let unkeyed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM music_artists WHERE search_key IS NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unkeyed, 0);
    refreshing_again_changes_nothing(&pool).await;
}

/// Again: the same rows, no new artists, and the tracks themselves untouched; a track gone
/// missing has no artists until it's back.
async fn refreshing_again_changes_nothing(pool: &SqlitePool) {
    let pool = pool.clone();
    let artists: i64 = sqlx::query_scalar("SELECT count(*) FROM music_artists")
        .fetch_one(&pool)
        .await
        .unwrap();
    let before = credited(&pool).await;
    refresh(&pool).await.unwrap();
    assert_eq!(credited(&pool).await, before);
    let again: i64 = sqlx::query_scalar("SELECT count(*) FROM music_artists")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(again, artists);
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM music_tracks ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(ids, [1, 2, 3, 4, 5, 6, 7]);
    sqlx::query("UPDATE music_tracks SET missing_since = 1 WHERE id = 4")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(refresh(&pool).await.unwrap(), 6);
}
