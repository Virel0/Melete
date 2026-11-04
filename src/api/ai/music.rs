//! Music by description, for the apps' AI search and mood mixes. The model turns the words
//! into filters or smart-playlist rules and never picks tracks; the apps run those against the
//! library as they run their own. What goes to OpenRouter: the words, and the names of the
//! genres the asker may see (so the model uses the library's own); never a title, an artist,
//! listening history or who asked. Each call counts as one of the user's AI searches.

use super::*;

/// The most genres told: the ones with the most tracks, which keeps a library with thousands
/// of stray tags to a prompt of a few thousand words.
const MAX_GENRES: i64 = 300;

#[derive(Deserialize)]
pub(super) struct MusicSearch {
    /// What someone wants to hear, in their own words.
    query: String,
}

#[derive(Deserialize)]
pub(super) struct MusicMix {
    /// The mood, in their own words.
    description: String,
}

/// Filters for music that fits a description, for the app to run against the library.
pub(super) async fn search(
    State(state): State<AiState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<MusicSearch>,
) -> Result<Json<Value>, ApiError> {
    let (client, query) = ready(&state, &current, &body.query)?;
    let genres = genres(&state.pool, &current.access).await?;
    let filters = client.music_filters(query, &genres).await.map_err(failed)?;
    Ok(Json(json!(filters)))
}

/// Smart-playlist rules for a mood, the names it mentioned, and the rules left out with why.
pub(super) async fn mix(
    State(state): State<AiState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<MusicMix>,
) -> Result<Json<Value>, ApiError> {
    let (client, description) = ready(&state, &current, &body.description)?;
    let genres = genres(&state.pool, &current.access).await?;
    let mix = client
        .music_mix(description, &genres)
        .await
        .map_err(failed)?;
    Ok(Json(json!({
        "names": mix.names,
        "rules": mix.rules,
        "rejected": mix.rejected,
    })))
}

/// The genres of the tracks present that the user may see, as `/music/genres` names them,
/// most tracks first.
async fn genres(pool: &SqlitePool, access: &crate::access::Access) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT min(g.value) FROM music_tracks t, json_each(t.genres) g
         WHERE t.missing_since IS NULL AND {}
         GROUP BY lower(g.value) ORDER BY count(DISTINCT t.id) DESC, lower(g.value) LIMIT ?",
        access.track_sql("t")
    )))
    .bind(MAX_GENRES)
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::ai::fake;

    /// Two music libraries, the second hidden from bob; alice's and bob's tokens.
    async fn setup(client: Option<Arc<Client>>) -> (Router, String, String) {
        let pool = crate::db::open_in_memory().await;
        let mut tokens = Vec::new();
        for name in ["alice", "bob"] {
            let user = crate::auth::testing::user(&pool, name, false).await;
            let kind = crate::auth::TokenKind::Session;
            tokens.push(
                crate::auth::issue_token(&pool, user.id, kind, "t")
                    .await
                    .unwrap()
                    .1,
            );
        }
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES
                   (1, 'music', 'music', '/m'), (2, 'private', 'music', '/p');
               INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                   first_seen_generation, seen_generation, tagged, title, genres,
                   missing_since) VALUES
                   (1, 1, 'a', 1, 1, 1, 1, 1, 'Paranoid Android', '["Rock", "Art Rock"]', NULL),
                   (2, 1, 'b', 1, 1, 1, 1, 1, 'So What', '["Jazz", "rock"]', NULL),
                   (3, 1, 'c', 1, 1, 1, 1, 1, 'Gone', '["Gone Genre"]', 5),
                   (4, 2, 'd', 1, 1, 1, 1, 1, 'Secret', '["Hidden Genre"]', NULL);
               INSERT INTO user_access (user_id, all_libraries, max_age, updated_at)
                   SELECT id, 0, NULL, 0 FROM users WHERE username = 'bob';
               INSERT INTO user_libraries (user_id, library_id)
                   SELECT id, 1 FROM users WHERE username = 'bob';"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let bob = tokens.pop().unwrap();
        (routes(pool, client), tokens.pop().unwrap(), bob)
    }

    async fn post(app: &Router, token: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::post(uri)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// The system prompt and the user's message of the last request OpenRouter saw.
    fn sent(fake: &fake::Fake) -> (String, String) {
        let seen = fake.seen.lock().unwrap();
        let messages = &seen.last().unwrap().0["messages"];
        let text = |i: usize| messages[i]["content"].as_str().unwrap().to_owned();
        (text(0), text(1))
    }

    #[tokio::test]
    async fn a_search_is_the_models_filters_over_the_genres_the_asker_may_see() {
        let (fake, api) = fake::start().await;
        *fake.content.lock().unwrap() =
            r#"{"genres": ["rock", "Hidden Genre", "Chill"], "from_year": 1990, "to_year": 1999,
                "search_term": null, "liked_only": false, "ordering": "most_played"}"#
                .into();
        let (app, _, bob) = setup(Some(Arc::new(fake::client(&api)))).await;
        let query = json!({ "query": "upbeat rock from the nineties" });
        let (status, body) = post(&app, &bob, "/api/v1/ai/music/search", query).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!({ "search_term": null, "genres": ["Rock"], "from_year": 1990,
                    "to_year": 1999, "liked_only": false, "ordering": "most_played" })
        );
        let (system, user) = sent(&fake);
        assert_eq!(user, "upbeat rock from the nineties");
        // Most tracks first, each genre once whatever its case, and only what bob may see.
        assert!(
            system.ends_with("This library's genres: Rock, Art Rock, Jazz"),
            "{system}"
        );
        let all = fake.seen.lock().unwrap()[0].0.to_string();
        for private in ["Paranoid", "Hidden", "Gone", "bob"] {
            assert!(!all.contains(private), "{private} left the server: {all}");
        }
    }

    #[tokio::test]
    async fn a_mix_is_the_models_rules_names_and_refusals() {
        let (fake, api) = fake::start().await;
        *fake.content.lock().unwrap() = r#"{"match": "all", "limit": 40, "names": ["Helldivers 2"],
            "rules": [{"field": "genre", "op": "is", "value": "Rock"},
                      {"field": "year", "op": "contains", "value": 1990}]}"#
            .into();
        let (app, alice, _) = setup(Some(Arc::new(fake::client(&api)))).await;
        let mood = json!({ "description": "Helldivers 2 session music" });
        let (status, body) = post(&app, &alice, "/api/v1/ai/music/mix", mood).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!({ "names": ["Helldivers 2"],
                    "rules": { "match": "all", "limit": 40,
                               "rules": [{ "field": "genre", "op": "is", "value": "Rock" }] },
                    "rejected": ["contains does not apply to year"] })
        );
        let (system, user) = sent(&fake);
        assert_eq!(user, "Helldivers 2 session music");
        assert!(
            system.contains("Rock, Art Rock, Hidden Genre, Jazz"),
            "{system}"
        );
    }

    #[tokio::test]
    async fn off_without_a_key_and_counted_with_movie_searches() {
        let (app, alice, _) = setup(None).await;
        for (uri, body) in [
            ("/api/v1/ai/music/search", json!({ "query": "x" })),
            ("/api/v1/ai/music/mix", json!({ "description": "x" })),
        ] {
            let (status, _) = post(&app, &alice, uri, body).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        }

        let (fake, api) = fake::start().await;
        *fake.content.lock().unwrap() = r#"{"titles": []}"#.into();
        let (app, alice, bob) = setup(Some(Arc::new(fake::client(&api)))).await;
        for words in ["", "  ", &"x".repeat(301)] {
            let (status, _) = post(
                &app,
                &alice,
                "/api/v1/ai/music/mix",
                json!({ "description": words }),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        for i in 0..SEARCHES_PER_HOUR {
            let (uri, body) = match i % 3 {
                0 => (
                    "/api/v1/ai/search",
                    json!({ "kind": "movies", "query": "x" }),
                ),
                1 => ("/api/v1/ai/music/search", json!({ "query": "x" })),
                _ => ("/api/v1/ai/music/mix", json!({ "description": "x" })),
            };
            let (status, body) = post(&app, &alice, uri, body).await;
            assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        }
        let (status, _) = post(
            &app,
            &alice,
            "/api/v1/ai/music/mix",
            json!({ "description": "x" }),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

        *fake.content.lock().unwrap() = "Sorry, I can't help with that.".into();
        let (status, body) = post(
            &app,
            &bob,
            "/api/v1/ai/music/search",
            json!({ "query": "x" }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            body["error"],
            "OpenRouter: the model's answer wasn't the filters asked for"
        );
        *fake.status.lock().unwrap() = 429;
        let (status, _) = post(
            &app,
            &bob,
            "/api/v1/ai/music/mix",
            json!({ "description": "x" }),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        *fake.status.lock().unwrap() = 402;
        let (status, body) = post(
            &app,
            &bob,
            "/api/v1/ai/music/mix",
            json!({ "description": "x" }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["error"], "the OpenRouter account is out of credit");
    }
}
