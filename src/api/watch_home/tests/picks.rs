//! AI picks through the API, with the fake OpenRouter.

use std::time::Duration;

use super::*;
use crate::ai::fake;

/// The main fixture with Film C (an Action film like Film A), AI picks on, and the routes
/// talking to the fake OpenRouter.
async fn with_ai() -> (Fixture, fake::Fake) {
    let f = fixture().await;
    sqlx::raw_sql(
        r#"INSERT INTO tmdb_movies (tmdb_id, title, year, overview, genres, fetched_at)
             VALUES (700, 'Film C', 2005, 'Another hacker.', '["Action"]', 0);
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
             match_status, match_tmdb_id) VALUES (30, 3, 'Film C', 'Film C', 1, 'matched', 700);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, added_at,
             movie_id, first_seen_generation, seen_generation)
             VALUES (30, 3, 'Film C/c.mkv', 1, 1, 30, 30, 1, 1);
         INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_AI_PICKS', 'on', 0);"#,
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let (fake, api) = fake::start().await;
    let client = Arc::new(fake::client(&api));
    let app = routes_from(
        f.pool.clone(),
        ArtworkState::stored_only(f.pool.clone()),
        None,
        Some(client),
    );
    let f = Fixture {
        pool: f.pool.clone(),
        app,
    };
    (f, fake)
}

async fn get(f: &Fixture, token: &str, uri: &str) -> (StatusCode, Value) {
    let request = Request::get(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = f.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Asks until `done` says the answer is what's waited for.
async fn until(f: &Fixture, token: &str, uri: &str, done: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..100 {
        let (status, answer) = get(f, token, uri).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        if done(&answer) {
            return answer;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{uri} never came round");
}

/// The fields a schema documents.
fn documented(schema: &str) -> Vec<String> {
    let spec: Value = serde_json::from_str(include_str!("../../openapi.json")).unwrap();
    let mut fields: Vec<String> = spec["components"]["schemas"][schema]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    fields.sort();
    fields
}

fn fields(value: &Value) -> Vec<String> {
    let mut fields: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    fields.sort();
    fields
}

#[tokio::test]
async fn featured_for_you_is_its_own_answer_and_a_home_row() {
    let (f, fake) = with_ai().await;
    let (me, token) = user(&f, "viewer").await;
    progress(&f, me, 6, 0.0, 10).await;
    *fake.content.lock().unwrap() =
        r#"{"picks": [{"id": "c1", "reason": "Another hacker story"}]}"#.into();
    let (status, first) = get(&f, &token, "/api/v1/ai/picks").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["status"], "working");
    assert_eq!(fields(&first), documented("AiPicks"));
    let ready = until(&f, &token, "/api/v1/ai/picks", |a| a["status"] == "ready").await;
    assert_eq!(
        ready["picks"],
        json!([{ "kind": "movie", "id": 30, "title": "Film C", "year": 2005, "poster": null,
                 "backdrop": null, "reason": "Another hacker story" }])
    );
    assert_eq!(fields(&ready["picks"][0]), documented("AiPick"));
    let home = home_of(&f, &token).await;
    assert_eq!(titles(row(&home, "featured_for_you")), ["movie:30"]);
    // The picks, and (from the home's "because you watched" row) titles like Film A, asked for
    // in the background.
    for _ in 0..100 {
        if fake.seen.lock().unwrap().len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let seen = fake.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    for (body, _) in seen.iter() {
        assert!(!body.to_string().contains("viewer"), "nobody is named");
    }
}

#[tokio::test]
async fn titles_like_one_come_from_the_library_until_the_ais_are_made() {
    let (f, fake) = with_ai().await;
    let (_, token) = user(&f, "viewer").await;
    *fake.content.lock().unwrap() = r#"{"picks": [{"id": "c1", "reason": "Hackers"}]}"#.into();
    let (status, first) = get(&f, &token, "/api/v1/movies/1/similar").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fields(&first), documented("SimilarTitles"));
    assert_eq!(first["source"], "library");
    assert_eq!(titles(&first["items"]), ["movie:30"]);
    assert_eq!(first["items"][0]["reason"], Value::Null);
    let made = until(&f, &token, "/api/v1/movies/1/similar", |a| {
        a["source"] == "ai"
    })
    .await;
    assert_eq!(titles(&made["items"]), ["movie:30"]);
    assert_eq!(made["items"][0]["reason"], "Hackers");
    let (status, _) = get(&f, &token, "/api/v1/movies/3/similar").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a gone film");
    let (status, series) = get(&f, &token, "/api/v1/series/1/similar").await;
    assert_eq!(status, StatusCode::OK, "{series}");
}
