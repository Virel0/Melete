use std::collections::HashMap;

use serde_json::value::RawValue;
use serde_json::{Value as Json, json};
use sqlx::SqlitePool;

use super::rule::Value;
use super::*;
use crate::aoide::sync;

const DAY: i64 = 86_400_000;
const FIXTURE: &str = include_str!("../../../tests/fixtures/aoide/smart-playlists.json");

/// Pushes one op as a device of `user` would.
async fn push(pool: &SqlitePool, user: i64, entity: &str, payload: Json) {
    let id = payload["id"].as_str().unwrap().to_owned();
    let op = json!({"opId": format!("{entity}-{id}"), "entity": entity, "entityId": id,
        "operation": "upsert", "payload": payload, "createdAt": 1});
    let raw = RawValue::from_string(op.to_string()).unwrap();
    sync::push(pool, user, "phone", vec![sync::validate(&raw).unwrap()], 1)
        .await
        .unwrap();
}

/// A library from the fixture, its listens and likes `user`'s: each track's id by its key.
async fn library(pool: &SqlitePool, user: i64, spec: &Json, now: i64) -> HashMap<String, i64> {
    sqlx::query(
        "INSERT OR IGNORE INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m')",
    )
    .execute(pool)
    .await
    .unwrap();
    let mut ids = HashMap::new();
    for (i, t) in spec["tracks"].as_array().unwrap().iter().enumerate() {
        let id = i as i64 + 1;
        let key = t["key"].as_str().unwrap();
        ids.insert(key.to_owned(), id);
        sqlx::query(
            "INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
             VALUES (?, 1, ?, ?, ?, ?, 1)",
        )
        .bind(id)
        .bind(key)
        .bind(t["album"].as_str())
        .bind(t["album_artist"].as_str().unwrap_or(""))
        .bind(key)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, genres, year,
                 duration_seconds)
             VALUES (?1, 1, ?2, 1, 1, 1, 1, ?1, 1, ?3, ?4, ?5, ?6)",
        )
        .bind(id)
        .bind(key)
        .bind(t["title"].as_str())
        .bind(t["genres"].to_string())
        .bind(t["year"].as_i64())
        .bind(t["duration_ms"].as_i64().map(|ms| ms as f64 / 1000.0))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO music_artists (id, key, name) VALUES (?1, ?2, ?3)")
            .bind(id)
            .bind(key)
            .bind(t["artist"].as_str())
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO music_track_artists (track_id, position, artist_id, name) VALUES (?1, 0, ?1, ?2)",
        )
        .bind(id)
        .bind(t["artist"].as_str())
        .execute(pool)
        .await
        .unwrap();
    }
    for (n, p) in spec["plays"].as_array().unwrap().iter().enumerate() {
        let track = ids[p["track"].as_str().unwrap()];
        let started = now - p["days_ago"].as_i64().unwrap() * DAY;
        let ms = p["ms"].as_i64().unwrap();
        sqlx::query(
            "INSERT INTO music_plays (user_id, source, event_id, track_key, track_id, device,
                 started_at, ended_at, position_ms, completed, skipped)
             VALUES (?, 'sync', ?, ?, ?, 'phone', ?, ?, ?, ?, 0)",
        )
        .bind(user)
        .bind(format!("e{n}"))
        .bind(track.to_string())
        .bind(track)
        .bind(started)
        .bind(started + ms)
        .bind(ms)
        .bind(p["completed"].as_bool().unwrap())
        .execute(pool)
        .await
        .unwrap();
    }
    for like in spec["likes"].as_array().unwrap() {
        let track = ids[like["track"].as_str().unwrap()];
        push(
            pool,
            user,
            "likes",
            json!({"id": format!("like-{track}"), "jellyfinId": track.to_string(),
                "contentKey": "", "liked": like["liked"], "deleted": like["deleted"],
                "updatedAt": 10, "originDevice": "phone"}),
        )
        .await;
    }
    ids
}

async fn listener(pool: &SqlitePool, name: &str) -> Listener {
    Listener {
        user_id: crate::auth::testing::user(pool, name, false).await.id,
        offset_minutes: 0,
    }
}

/// Every case in the apps' fixture gives the songs it gives on the apps.
#[tokio::test]
async fn the_apps_cases_give_the_same_songs_here() {
    let fixture: Json = serde_json::from_str(FIXTURE).unwrap();
    let now = fixture["now"].as_i64().unwrap();
    let mut built = HashMap::new();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let which = case["library"].as_str().unwrap();
        if !built.contains_key(which) {
            let pool = crate::db::open_in_memory().await;
            let me = listener(&pool, "alice").await;
            let ids = library(&pool, me.user_id, &fixture["libraries"][which], now).await;
            built.insert(which.to_owned(), (pool, me, ids));
        }
        let (pool, me, ids) = &built[which];
        let keys: HashMap<i64, &str> = ids.iter().map(|(k, v)| (*v, k.as_str())).collect();
        let rules = Rules::from_json(&case["rules"]).unwrap_or_else(|e| panic!("{name}: {e}"));
        let at = now + case["at_days"].as_i64().unwrap_or(0) * DAY;
        let found = evaluate(pool, &rules, at, *me).await.unwrap();
        let found: Vec<&str> = found.iter().map(|id| keys[id]).collect();
        let expect: Vec<&str> = case["expect"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k.as_str().unwrap())
            .collect();
        assert_eq!(found, expect, "{name}");
    }
    assert_eq!(built.len(), 4, "every library was used");
}

/// Rules about likes and listens read the listener's own; a song they marked Not interested
/// is left out whatever the rules say.
#[tokio::test]
async fn the_listeners_own_likes_listens_and_not_interested() {
    let fixture: Json = serde_json::from_str(FIXTURE).unwrap();
    let now = fixture["now"].as_i64().unwrap();
    let pool = crate::db::open_in_memory().await;
    let alice = listener(&pool, "alice").await;
    let bob = listener(&pool, "bob").await;
    let ids = library(&pool, alice.user_id, &fixture["libraries"]["four"], now).await;
    let rules = |text: &str| Rules::from_json(&serde_json::from_str(text).unwrap()).unwrap();
    let liked = rules(r#"{"match":"all","rules":[{"field":"liked","op":"is","value":true}]}"#);
    let played = rules(r#"{"match":"all","rules":[{"field":"play_count","op":">","value":0}]}"#);
    assert_eq!(
        evaluate(&pool, &liked, now, alice).await.unwrap(),
        [ids["so-what"]]
    );
    assert!(evaluate(&pool, &liked, now, bob).await.unwrap().is_empty());
    assert!(evaluate(&pool, &played, now, bob).await.unwrap().is_empty());

    let everything = rules(r#"{"match":"all","rules":[]}"#);
    push(
        &pool,
        alice.user_id,
        "track_flags",
        json!({"id": "f", "jellyfinId": ids["xtal"].to_string(), "contentKey": "",
            "notInterested": true, "dontCount": false, "updatedAt": 10, "deleted": false,
            "originDevice": "phone"}),
    )
    .await;
    let mine = evaluate(&pool, &everything, now, alice).await.unwrap();
    assert!(!mine.contains(&ids["xtal"]) && mine.len() == 3, "{mine:?}");
    assert_eq!(
        evaluate(&pool, &everything, now, bob).await.unwrap().len(),
        4
    );
    // A track gone from the library is no song.
    sqlx::query("UPDATE music_tracks SET missing_since = 1 WHERE id = ?")
        .bind(ids["autumn"])
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        evaluate(&pool, &everything, now, bob).await.unwrap().len(),
        3
    );
}

/// What each field allows, spelled out rather than taken from the code under test, as the
/// apps' matrix is.
fn allowed(field: Field) -> &'static [Operator] {
    use Operator::*;
    match field {
        Field::Title | Field::Artist | Field::Album | Field::AlbumArtist | Field::Genre => {
            &[Is, IsNot, Contains, DoesNotContain, BeginsWith]
        }
        Field::Year | Field::DurationMs | Field::PlayCount | Field::SkipCount => {
            &[Is, IsNot, GreaterThan, LessThan]
        }
        Field::LastPlayed | Field::FirstPlayed => &[Before, After, InTheLast, NotInTheLast],
        Field::Liked => &[Is, IsNot],
    }
}

fn suitable(field: Field) -> Value {
    match field {
        Field::Title | Field::Artist | Field::Album | Field::AlbumArtist | Field::Genre => {
            Value::Text("a".into())
        }
        Field::Year | Field::DurationMs | Field::PlayCount | Field::SkipCount => Value::Number(1),
        Field::LastPlayed | Field::FirstPlayed => Value::Text("-30d".into()),
        Field::Liked => Value::Flag(true),
    }
}

/// The apps' `every_field_and_operator_pair_is_evaluated_or_refused_by_name`.
#[tokio::test]
async fn every_field_and_operator_pair_is_worked_out_or_refused_by_name() {
    let fixture: Json = serde_json::from_str(FIXTURE).unwrap();
    let now = fixture["now"].as_i64().unwrap();
    let pool = crate::db::open_in_memory().await;
    let me = listener(&pool, "alice").await;
    library(&pool, me.user_id, &fixture["libraries"]["four"], now).await;
    let mut accepted = 0;
    for field in Field::ALL {
        for op in Operator::ALL {
            let value = suitable(field);
            let written = json!({"match": "all",
                "rules": [{"field": field.wire_name(), "op": op.wire_name(), "value": value.to_json()}]});
            let built = Rule::new(field, op, value.clone());
            let decoded = Rules::from_json(&written);
            if allowed(field).contains(&op) {
                accepted += 1;
                let rule = built.unwrap_or_else(|e| panic!("{field:?} {op:?}: {e}"));
                let rules = Rules::new(Match::All, vec![rule], None, None).unwrap();
                assert_eq!(decoded.as_ref(), Ok(&rules), "{field:?} {op:?}");
                evaluate(&pool, &rules, now, me).await.unwrap();
            } else {
                let refusal = SmartError::OperatorNotApplicable { field, op };
                assert_eq!(built, Err(refusal.clone()), "{field:?} {op:?}");
                assert_eq!(decoded, Err(refusal), "{field:?} {op:?}");
            }
        }
    }
    assert_eq!(accepted, 5 * 5 + 4 * 4 + 2 * 4 + 2);
    for field in Field::ALL {
        for dir in [Direction::Asc, Direction::Desc] {
            let sort = Some(Sort { field, dir });
            let rules = Rules::new(Match::All, vec![], sort, None).unwrap();
            let sorted = evaluate(&pool, &rules, now, me).await;
            if field == Field::Genre {
                assert!(matches!(
                    sorted,
                    Err(EvaluateError::Rules(SmartError::FieldNotSortable(
                        Field::Genre
                    )))
                ));
            } else {
                assert_eq!(sorted.unwrap().len(), 4, "{field:?} {dir:?}");
            }
        }
    }
}

/// The apps' `illegal_rules_are_refused_rather_than_quietly_matching_nothing` and
/// `unknown_fields_and_operators_are_named_not_ignored`.
#[test]
fn the_impossible_is_refused_by_name() {
    let rule = |field, op, value| Rule::new(field, op, value);
    let text = |t: &str| Value::Text(t.into());
    assert_eq!(
        rule(Field::Title, Operator::Is, Value::Number(3)),
        Err(SmartError::ValueTypeMismatch {
            field: Field::Title,
            op: Operator::Is
        })
    );
    assert_eq!(
        rule(Field::Liked, Operator::Is, text("yes")),
        Err(SmartError::ValueTypeMismatch {
            field: Field::Liked,
            op: Operator::Is
        })
    );
    // A span in no unit isn't a span.
    assert_eq!(
        rule(Field::LastPlayed, Operator::InTheLast, Value::Number(30)),
        Err(SmartError::ValueTypeMismatch {
            field: Field::LastPlayed,
            op: Operator::InTheLast
        })
    );
    assert_eq!(
        rule(Field::LastPlayed, Operator::InTheLast, text("a fortnight")),
        Err(SmartError::MalformedInterval("a fortnight".into()))
    );
    assert_eq!(
        Rules::new(Match::Any, vec![], None, None),
        Err(SmartError::NoRules)
    );
    assert_eq!(
        Rules::new(Match::All, vec![], None, Some(0)),
        Err(SmartError::NonPositiveLimit(0))
    );
    let read = |text: &str| Rules::from_json(&serde_json::from_str(text).unwrap());
    assert_eq!(
        read(r#"{"match":"all","rules":[{"field":"titel","op":"is","value":"x"}]}"#),
        Err(SmartError::UnknownField("titel".into()))
    );
    assert_eq!(
        read(r#"{"match":"all","rules":[{"field":"title","op":"soundsLike","value":"x"}]}"#),
        Err(SmartError::UnknownOperator("soundsLike".into()))
    );
    assert_eq!(
        read(r#"{"match":"all","rules":[{"field":"year","op":"is","value":1959.5}]}"#),
        Err(SmartError::MalformedValue)
    );
    assert!(matches!(
        read(r#"{"match":"some","rules":[]}"#),
        Err(SmartError::MalformedJson(_))
    ));
    assert!(matches!(
        Rules::from_json(&json!("not json")),
        Err(SmartError::MalformedJson(_))
    ));
}

/// The apps' `rules_round_trip_through_the_documents_field_spelling`, and `>=` read as
/// `greaterThan` one lower.
#[test]
fn rules_round_trip_as_the_apps_write_them() {
    let original = Rules::new(
        Match::Any,
        vec![
            Rule::new(Field::PlayCount, Operator::GreaterThan, Value::Number(5)).unwrap(),
            Rule::new(
                Field::LastPlayed,
                Operator::NotInTheLast,
                Value::Text("-6m".into()),
            )
            .unwrap(),
        ],
        Some(Sort {
            field: Field::LastPlayed,
            dir: Direction::Desc,
        }),
        Some(25),
    )
    .unwrap();
    let written = crate::aoide::writing::canonical_json(&original.to_json());
    assert_eq!(
        written,
        r#"{"limit":25,"match":"any","rules":[{"field":"play_count","op":"greaterThan","value":5},{"field":"last_played","op":"notInTheLast","value":"-6m"}],"sort":{"dir":"desc","field":"last_played"}}"#
    );
    assert_eq!(Rules::from_json(&Json::String(written)), Ok(original));
    let symbol = Rules::from_json(&json!({"match": "all",
        "rules": [{"field": "play_count", "op": ">=", "value": 3}]}))
    .unwrap();
    assert_eq!(
        symbol.rules[0],
        Rule::new(Field::PlayCount, Operator::GreaterThan, Value::Number(2)).unwrap()
    );
}
