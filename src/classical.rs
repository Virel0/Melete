//! Composers, works and their parts, worked out from every present track
//! after each scan and match: what its tags say first (composer, work, movement), MusicBrainz's
//! work and recording relations filling the gaps for tracks of matched albums. A composer or work
//! MusicBrainz knows is one row however its tags spell it ("mb:<id>"); a name only in tags joins
//! the MusicBrainz one spelled the same (accents, case and punctuation aside), else is one of its own
//! ("name:<folded>"). Rebuilt whole each time, so nothing here is ever edited by hand.

use std::collections::HashMap;

use serde::Deserialize;
use sqlx::SqlitePool;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// A name as keys compare it: accents, case, punctuation and a leading "The" aside, so
/// "Dvořák" in one album's tags and "Dvorak" in another's are one composer.
fn fold(text: &str) -> String {
    let plain: String = text.nfd().filter(|c| !is_combining_mark(*c)).collect();
    crate::music_match::fold(&plain)
}

#[derive(sqlx::FromRow)]
struct Track {
    id: i64,
    composer: Option<String>,
    work: Option<String>,
    movement: Option<String>,
    movement_number: Option<i64>,
    mb_work_id: Option<String>,
    mb_work_title: Option<String>,
    mb_movement: Option<String>,
    mb_movement_number: Option<i64>,
    mb_composers: Option<String>,
    /// The track's genres, as JSON.
    genres: String,
}

/// A genre that makes a work classical: the classical family ("Classical", "Modern Classical",
/// "Classical Crossover") and its forms, but not "Neoclassical" (a rock and electronic style).
fn classical_genre(genre: &str) -> bool {
    let genre = genre.trim().to_lowercase();
    genre.starts_with("classical")
        || genre.ends_with(" classical")
        || [
            "opera",
            "baroque",
            "chamber music",
            "choral",
            "orchestral",
            "art song",
            "lied",
        ]
        .contains(&genre.as_str())
}

#[derive(Deserialize)]
struct MbComposer {
    id: String,
    name: String,
    sort_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct Composer {
    name: String,
    sort_name: Option<String>,
    mb_artist_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct Work {
    title: String,
    composer: Option<String>,
    mb_work_id: Option<String>,
    /// Has a part, or a track in a classical genre: browsed as classical music.
    classical: bool,
}

/// One track's place: its composer's and work's keys, and its part.
#[derive(Debug, PartialEq)]
struct Place {
    track: i64,
    composer: Option<String>,
    work: Option<String>,
    movement: Option<String>,
    movement_number: Option<i64>,
}

#[derive(Debug, Default, PartialEq)]
struct Worked {
    composers: HashMap<String, Composer>,
    works: HashMap<String, Work>,
    places: Vec<Place>,
}

/// Each track's composer, work and part, with the composers and works they name.
fn work_out(tracks: &[Track]) -> Worked {
    let mut out = Worked::default();
    let mb_composers: Vec<Option<MbComposer>> = first_mb_composers(tracks);
    // MusicBrainz's composers and works first, so names in tags can join them.
    let composer_names = add_mb_composers(&mut out, &mb_composers);
    let work_titles = add_mb_works(&mut out, tracks, &mb_composers, &composer_names);
    for (t, mb) in tracks.iter().zip(&mb_composers) {
        let composer = composer_of(&composer_names, t, mb);
        if let Some((key, name)) = &composer {
            out.composers
                .entry(key.clone())
                .or_insert_with(|| Composer {
                    name: name.clone(),
                    sort_name: None,
                    mb_artist_id: None,
                });
        }
        let composer = composer.map(|c| c.0);
        let work = work_key(&mut out, t, &composer, &work_titles);
        if composer.is_none() && work.is_none() {
            continue;
        }
        let place = Place {
            track: t.id,
            composer,
            work,
            movement: t.movement.clone().or_else(|| t.mb_movement.clone()),
            movement_number: t.movement_number.or(t.mb_movement_number),
        };
        let genres: Vec<String> = serde_json::from_str(&t.genres).unwrap_or_default();
        if (place.movement.is_some()
            || place.movement_number.is_some()
            || genres.iter().any(|g| classical_genre(g)))
            && let Some(work) = place.work.as_ref().and_then(|w| out.works.get_mut(w))
        {
            work.classical = true;
        }
        out.places.push(place);
    }
    out
}

/// Each track's first MusicBrainz composer, if it has any.
fn first_mb_composers(tracks: &[Track]) -> Vec<Option<MbComposer>> {
    tracks
        .iter()
        .map(|t| {
            let list: Vec<MbComposer> = t
                .mb_composers
                .as_deref()
                .and_then(|c| serde_json::from_str(c).ok())
                .unwrap_or_default();
            list.into_iter().next()
        })
        .collect()
}

/// Adds the MusicBrainz composers; returns their keys by folded name and sort name.
fn add_mb_composers(
    out: &mut Worked,
    mb_composers: &[Option<MbComposer>],
) -> HashMap<String, String> {
    let mut composer_names: HashMap<String, String> = HashMap::new();
    for c in mb_composers.iter().flatten() {
        let key = format!("mb:{}", c.id);
        for name in [Some(&c.name), c.sort_name.as_ref()].into_iter().flatten() {
            composer_names
                .entry(fold(name))
                .or_insert_with(|| key.clone());
        }
        out.composers.entry(key).or_insert_with(|| Composer {
            name: c.name.clone(),
            sort_name: c.sort_name.clone(),
            mb_artist_id: Some(c.id.clone()),
        });
    }
    composer_names
}

/// The track's composer, as its key and name: MusicBrainz's, else the one its tags name, under
/// the key of a MusicBrainz composer spelled the same when there is one.
fn composer_of(
    composer_names: &HashMap<String, String>,
    t: &Track,
    mb: &Option<MbComposer>,
) -> Option<(String, String)> {
    if let Some(c) = mb {
        return Some((format!("mb:{}", c.id), c.name.clone()));
    }
    let name = t.composer.clone()?;
    let folded = fold(&name);
    (!folded.is_empty()).then(|| {
        let key = composer_names
            .get(&folded)
            .cloned()
            .unwrap_or_else(|| format!("name:{folded}"));
        (key, name)
    })
}

/// Adds the MusicBrainz works; returns their keys by composer and folded title.
fn add_mb_works(
    out: &mut Worked,
    tracks: &[Track],
    mb_composers: &[Option<MbComposer>],
    composer_names: &HashMap<String, String>,
) -> HashMap<(Option<String>, String), String> {
    let mut work_titles: HashMap<(Option<String>, String), String> = HashMap::new();
    for (t, mb) in tracks.iter().zip(mb_composers) {
        if let (Some(id), Some(title)) = (&t.mb_work_id, &t.mb_work_title) {
            let composer = composer_of(composer_names, t, mb).map(|c| c.0);
            let key = format!("mb:{id}");
            work_titles
                .entry((composer.clone(), fold(title)))
                .or_insert_with(|| key.clone());
            out.works.entry(key).or_insert_with(|| Work {
                title: title.clone(),
                composer,
                mb_work_id: Some(id.clone()),
                classical: false,
            });
        }
    }
    work_titles
}

/// The key of the track's work, adding a work named only in its tags; None without one.
fn work_key(
    out: &mut Worked,
    t: &Track,
    composer: &Option<String>,
    work_titles: &HashMap<(Option<String>, String), String>,
) -> Option<String> {
    match (&t.mb_work_id, &t.work) {
        (Some(id), _) => Some(format!("mb:{id}")),
        (None, Some(title)) if !fold(title).is_empty() => {
            let folded = fold(title);
            let key = work_titles
                .get(&(composer.clone(), folded.clone()))
                .cloned()
                .unwrap_or_else(|| {
                    format!("name:{}:{folded}", composer.as_deref().unwrap_or_default())
                });
            out.works.entry(key.clone()).or_insert_with(|| Work {
                title: title.clone(),
                composer: composer.clone(),
                mb_work_id: None,
                classical: false,
            });
            Some(key)
        }
        _ => None,
    }
}

/// Works out every present track's composer, work and part again, and keeps only the composers
/// and works some track has.
pub async fn refresh(pool: &SqlitePool) -> anyhow::Result<(usize, usize)> {
    let tracks: Vec<Track> = sqlx::query_as(
        "SELECT t.id, t.composer, t.work, t.movement, t.movement_number, t.genres,
             rt.work_id AS mb_work_id, rt.work_title AS mb_work_title,
             rt.movement AS mb_movement, rt.movement_number AS mb_movement_number,
             rt.composers AS mb_composers
         FROM music_tracks t
         LEFT JOIN music_albums a ON a.id = t.album_id AND a.match_status = 'matched'
         LEFT JOIN mb_release_tracks rt
             ON rt.release_id = a.match_release_id AND rt.track_id = t.match_track_id
         WHERE t.missing_since IS NULL ORDER BY t.id",
    )
    .fetch_all(pool)
    .await?;
    let worked = work_out(&tracks);
    let mut tx = pool.begin().await?;
    let mut composer_ids: HashMap<&str, i64> = HashMap::new();
    for (key, c) in &worked.composers {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO music_composers (key, name, sort_name, mb_artist_id) VALUES (?, ?, ?, ?)
             ON CONFLICT (key) DO UPDATE SET name = excluded.name,
                 sort_name = excluded.sort_name, mb_artist_id = excluded.mb_artist_id
             RETURNING id",
        )
        .bind(key)
        .bind(&c.name)
        .bind(&c.sort_name)
        .bind(&c.mb_artist_id)
        .fetch_one(&mut *tx)
        .await?;
        composer_ids.insert(key, id);
    }
    let mut work_ids: HashMap<&str, i64> = HashMap::new();
    for (key, w) in &worked.works {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO music_works (key, title, composer_id, mb_work_id, classical)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (key) DO UPDATE SET title = excluded.title,
                 composer_id = excluded.composer_id, mb_work_id = excluded.mb_work_id,
                 classical = excluded.classical
             RETURNING id",
        )
        .bind(key)
        .bind(&w.title)
        .bind(w.composer.as_deref().and_then(|c| composer_ids.get(c)))
        .bind(&w.mb_work_id)
        .bind(w.classical)
        .fetch_one(&mut *tx)
        .await?;
        work_ids.insert(key, id);
    }
    sqlx::query("DELETE FROM music_track_works")
        .execute(&mut *tx)
        .await?;
    for place in &worked.places {
        sqlx::query(
            "INSERT INTO music_track_works (track_id, composer_id, work_id, movement, movement_number)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(place.track)
        .bind(place.composer.as_deref().and_then(|c| composer_ids.get(c)))
        .bind(place.work.as_deref().and_then(|w| work_ids.get(w)))
        .bind(&place.movement)
        .bind(place.movement_number.filter(|n| *n > 0))
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "DELETE FROM music_works WHERE id NOT IN
             (SELECT work_id FROM music_track_works WHERE work_id IS NOT NULL)",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM music_composers WHERE id NOT IN
             (SELECT composer_id FROM music_track_works WHERE composer_id IS NOT NULL
              UNION SELECT composer_id FROM music_works WHERE composer_id IS NOT NULL)",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((worked.composers.len(), worked.works.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::musicbrainz::testing;

    fn tagged(
        id: i64,
        composer: Option<&str>,
        work: Option<&str>,
        movement: Option<&str>,
    ) -> Track {
        Track {
            id,
            composer: composer.map(str::to_owned),
            work: work.map(str::to_owned),
            movement: movement.map(str::to_owned),
            movement_number: None,
            mb_work_id: None,
            mb_work_title: None,
            mb_movement: None,
            mb_movement_number: None,
            mb_composers: None,
            genres: "[]".into(),
        }
    }

    fn from_musicbrainz(id: i64, movement: &str, number: i64) -> Track {
        Track {
            mb_work_id: Some("w5".into()),
            mb_work_title: Some("Symphony no. 5 in C minor, op. 67".into()),
            mb_movement: Some(movement.into()),
            mb_movement_number: Some(number),
            mb_composers: Some(
                r#"[{"id": "lvb", "name": "Ludwig van Beethoven", "sort_name": "Beethoven, Ludwig van"}]"#
                    .into(),
            ),
            ..tagged(id, None, None, None)
        }
    }

    #[test]
    fn tags_come_first_and_names_join_what_musicbrainz_knows() {
        let worked = work_out(&[
            from_musicbrainz(1, "Allegro con brio", 1),
            // A tag's own movement name wins over MusicBrainz's.
            Track {
                movement: Some("Andante".into()),
                ..from_musicbrainz(2, "Andante con moto", 2)
            },
            // Only tags, spelled differently: the same composer and work.
            Track {
                movement_number: Some(3),
                ..tagged(
                    3,
                    Some("Ludwig van Beethoven"),
                    Some("Symphony No. 5 in C Minor, Op. 67"),
                    Some("Scherzo"),
                )
            },
            tagged(4, Some("BEETHOVEN, Ludwig van"), None, None),
            tagged(5, Some("Erik Satie"), Some("Gymnopédies"), None),
            tagged(6, None, Some("Gymnopédies"), None),
            tagged(7, None, None, None),
        ]);
        let place = |track: i64| worked.places.iter().find(|p| p.track == track);
        for track in [1, 2, 3, 4] {
            assert_eq!(
                place(track).unwrap().composer.as_deref(),
                Some("mb:lvb"),
                "{track}"
            );
        }
        for track in [1, 2, 3] {
            assert_eq!(
                place(track).unwrap().work.as_deref(),
                Some("mb:w5"),
                "{track}"
            );
        }
        assert_eq!(place(2).unwrap().movement.as_deref(), Some("Andante"));
        assert_eq!(place(2).unwrap().movement_number, Some(2));
        assert_eq!(
            (
                place(3).unwrap().movement.as_deref(),
                place(3).unwrap().movement_number
            ),
            (Some("Scherzo"), Some(3))
        );
        assert_eq!(place(4).unwrap().work, None, "a composer without a work");
        assert_eq!(
            worked.works["name:name:eriksatie:gymnopedies"].title,
            "Gymnopédies"
        );
        assert_eq!(
            place(6).unwrap().work.as_deref(),
            Some("name::gymnopedies"),
            "a work without a composer is another work"
        );
        assert_eq!(place(7), None, "neither: no place");
        assert_eq!(worked.composers.len(), 2);
        assert_eq!(
            worked.composers["mb:lvb"].sort_name.as_deref(),
            Some("Beethoven, Ludwig van")
        );
        assert_eq!(worked.works.len(), 3);
    }

    #[test]
    fn works_with_parts_or_a_classical_genre_are_classical() {
        for (genre, classical) in [
            ("Classical", true),
            ("classical: romantic", true),
            ("Modern Classical", true),
            ("Opera", true),
            ("Neoclassical", false),
            ("Neo-classical darkwave", false),
            ("Pop", false),
        ] {
            assert_eq!(classical_genre(genre), classical, "{genre}");
        }
        let worked = work_out(&[
            from_musicbrainz(1, "Allegro con brio", 1),
            tagged(2, Some("Erik Satie"), Some("Gymnopédies"), None),
            Track {
                genres: r#"["Piano", "Classical"]"#.into(),
                ..tagged(3, Some("Erik Satie"), Some("Vexations"), None)
            },
            tagged(4, Some("Lennon"), Some("Help!"), None),
        ]);
        let classical = |key: &str| worked.works[key].classical;
        assert!(classical("mb:w5"), "it has parts");
        assert!(
            classical("name:name:eriksatie:vexations"),
            "a classical genre"
        );
        assert!(!classical("name:name:eriksatie:gymnopedies"));
        assert!(!classical("name:name:lennon:help"));
    }

    async fn composers_and_works(
        pool: &SqlitePool,
    ) -> (Vec<(i64, String)>, Vec<(i64, String, Option<i64>)>) {
        let composers = sqlx::query_as("SELECT id, name FROM music_composers ORDER BY name")
            .fetch_all(pool)
            .await
            .unwrap();
        let works = sqlx::query_as("SELECT id, title, composer_id FROM music_works ORDER BY title")
            .fetch_all(pool)
            .await
            .unwrap();
        (composers, works)
    }

    #[tokio::test]
    async fn works_out_matched_and_tagged_tracks_and_keeps_only_what_tracks_have() {
        let pool = crate::db::open_in_memory().await;
        crate::music_match::tests::matched_before_works(&pool).await;
        let (mb, _) = testing::start(0).await;
        crate::music_match::match_pending(&pool, &mb).await.unwrap();
        sqlx::query(
            "INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, tagged, title, composer, work)
             VALUES (50, 1, 'satie.flac', 1, 1, 1, 1, 1, 'Gymnopédie No. 1', 'Erik Satie',
                 'Gymnopédies')",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(refresh(&pool).await.unwrap(), (2, 2));
        let (composers, works) = composers_and_works(&pool).await;
        assert_eq!(
            composers.iter().map(|c| c.1.as_str()).collect::<Vec<_>>(),
            ["Erik Satie", "Ludwig van Beethoven"]
        );
        assert_eq!(works[1].1, "Symphony no. 5 in C minor, op. 67");
        assert_eq!(works[1].2, Some(composers[1].0));
        let parts: Vec<(Option<String>, Option<i64>)> = sqlx::query_as(
            "SELECT movement, movement_number FROM music_track_works WHERE work_id = ?
             ORDER BY movement_number",
        )
        .bind(works[1].0)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[2], (Some("Scherzo. Allegro".into()), Some(3)));

        // Again: the same rows. Then a track gone takes its composer and work with it.
        refresh(&pool).await.unwrap();
        assert_eq!(
            composers_and_works(&pool).await,
            (composers.clone(), works.clone())
        );
        sqlx::query("UPDATE music_tracks SET missing_since = 1 WHERE id = 50")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(refresh(&pool).await.unwrap(), (1, 1));
        let (left, works_left) = composers_and_works(&pool).await;
        assert_eq!(left, [composers[1].clone()]);
        assert_eq!(works_left, [works[1].clone()]);
    }
}
