//! Music quality profiles (admins only): importing them from Lidarr, the
//! built-in preset that prefers FLAC, choosing one for a wanted artist, and trying a release
//! name against one to see how it's read and decided.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use super::import::import_error;
use crate::arr::ArrClient;
use crate::music_profiles::{self, Changed, Deleted, ImportReport, MusicProfile, ProfileChange};
use crate::music_release;

/// The music quality profile routes, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/import/lidarr/profiles", post(import_lidarr))
        .route("/api/v1/music/profiles", get(list))
        .route(
            "/api/v1/music/profiles/presets/lossless",
            post(lossless_preset),
        )
        .route(
            "/api/v1/music/profiles/{id}",
            put(change_profile).delete(delete_profile),
        )
        .route("/api/v1/music/profiles/{id}/evaluate", post(evaluate))
        .route("/api/v1/wanted/artists/{id}/profile", put(choose_profile))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(pool)
}

#[derive(Deserialize)]
struct Connection {
    url: String,
    api_key: String,
}

async fn import_lidarr(
    State(pool): State<SqlitePool>,
    Json(connection): Json<Connection>,
) -> Result<Json<ImportReport>, ApiError> {
    let client = ArrClient::new(&connection.url, connection.api_key)
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))?;
    let report = music_profiles::import_lidarr(&pool, &client)
        .await
        .map_err(|err| import_error("Lidarr", err))?;
    Ok(Json(report))
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<MusicProfile>>, ApiError> {
    Ok(Json(music_profiles::list(&pool).await?))
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "no such music profile")
}

/// Creates the preset, or puts it back as it ships.
async fn lossless_preset(
    State(pool): State<SqlitePool>,
) -> Result<(StatusCode, Json<MusicProfile>), ApiError> {
    let id = music_profiles::lossless(&pool).await?;
    let profile = music_profiles::get(&pool, id)
        .await?
        .ok_or_else(not_found)?;
    Ok((StatusCode::CREATED, Json(profile)))
}

/// Changes a profile's name, upgrades, cutoff and allowed qualities.
async fn change_profile(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(change): Json<ProfileChange>,
) -> Result<Json<MusicProfile>, ApiError> {
    match music_profiles::change(&pool, id, &change).await? {
        Changed::Done => Ok(Json(
            music_profiles::get(&pool, id)
                .await?
                .ok_or_else(not_found)?,
        )),
        Changed::NotFound => Err(not_found()),
        Changed::Taken => Err(ApiError::new(
            StatusCode::CONFLICT,
            "another profile has that name",
        )),
        Changed::Invalid(why) => Err(ApiError::new(StatusCode::BAD_REQUEST, why)),
    }
}

/// Deletes a profile nothing uses.
async fn delete_profile(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    match music_profiles::delete(&pool, id).await? {
        Deleted::Done => Ok(StatusCode::NO_CONTENT),
        Deleted::NotFound => Err(not_found()),
        Deleted::InUse(why) => Err(ApiError::new(StatusCode::CONFLICT, why)),
    }
}

#[derive(Deserialize)]
struct EvaluateRequest {
    title: String,
}

/// How a release name is read, and whether the profile would take it.
async fn evaluate(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(request): Json<EvaluateRequest>,
) -> Result<Json<Value>, ApiError> {
    let profile = music_profiles::get(&pool, id)
        .await?
        .ok_or_else(not_found)?;
    let release = music_release::parse(&request.title);
    let decision = music_profiles::decide(&profile, &release);
    Ok(Json(json!({ "release": release, "decision": decision })))
}

#[derive(Deserialize)]
struct ChooseRequest {
    /// None goes back to the profile Lidarr names.
    profile_id: Option<i64>,
}

async fn choose_profile(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(request): Json<ChooseRequest>,
) -> Result<StatusCode, ApiError> {
    if let Some(profile) = request.profile_id
        && music_profiles::get(&pool, profile).await?.is_none()
    {
        return Err(not_found());
    }
    let updated = sqlx::query("UPDATE wanted_artists SET profile_id = ? WHERE id = ?")
        .bind(request.profile_id)
        .bind(id)
        .execute(&pool)
        .await?
        .rows_affected();
    if updated == 0 {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such wanted artist",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::header;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::import::testing::FakeArr;

    async fn send(
        app: &Router,
        token: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn keys(spec: &Value, schema: &str, value: &Value) -> (Vec<String>, Vec<String>) {
        let mut documented: Vec<String> = spec["components"]["schemas"][schema]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let mut actual: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
        documented.sort();
        actual.sort();
        (actual, documented)
    }

    #[tokio::test]
    async fn imports_presets_chooses_and_evaluates_music_profiles() {
        let pool = crate::db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
             VALUES (1, 'a', 'Alder', 1, 'all', 'lidarr', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let admin = auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let viewer = auth::create_user(&pool, "viewer", "viewer-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, admin) = auth::issue_token(&pool, admin.id, TokenKind::Session, "t")
            .await
            .unwrap();
        let (_, viewer) = auth::issue_token(&pool, viewer.id, TokenKind::Session, "t")
            .await
            .unwrap();
        let app = routes(pool.clone());
        let spec: Value = serde_json::from_str(super::super::OPENAPI).unwrap();

        let fake = FakeArr::default();
        fake.reply("/api/v1/system/status", json!({ "appName": "Lidarr" }))
            .reply(
                "/api/v1/qualityprofile",
                json!([{ "id": 1, "name": "Any", "upgradeAllowed": false, "cutoff": 4, "items": [
                { "quality": { "id": 4, "name": "MP3-320" }, "items": [], "allowed": true } ] }]),
            );
        let connection = json!({ "url": fake.start().await, "api_key": "key" });
        assert_eq!(
            send(
                &app,
                &viewer,
                "POST",
                "/api/v1/import/lidarr/profiles",
                Some(connection.clone())
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        let (status, report) = send(
            &app,
            &admin,
            "POST",
            "/api/v1/import/lidarr/profiles",
            Some(connection),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{report}");
        assert_eq!(report, json!({ "profiles": 1, "skipped_qualities": [] }));
        let (a, d) = keys(&spec, "MusicProfileImportReport", &report);
        assert_eq!(a, d);

        let (status, preset) = send(
            &app,
            &admin,
            "POST",
            "/api/v1/music/profiles/presets/lossless",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(
            (&preset["name"], &preset["cutoff"]),
            (&json!("FLAC preferred"), &json!("Lossless"))
        );
        let (a, d) = keys(&spec, "MusicProfile", &preset);
        assert_eq!(a, d);
        let (a, d) = keys(&spec, "MusicRankedQuality", &preset["qualities"][0]);
        assert_eq!(a, d);
        let (_, profiles) = send(&app, &admin, "GET", "/api/v1/music/profiles", None).await;
        assert_eq!(profiles.as_array().unwrap().len(), 2);

        let id = preset["id"].as_i64().unwrap();
        let (status, evaluated) = send(
            &app,
            &admin,
            "POST",
            &format!("/api/v1/music/profiles/{id}/evaluate"),
            Some(json!({ "title": "Alder - One (2016) [FLAC 24-96]" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            evaluated,
            json!({ "release": { "quality": "FLAC 24bit", "year": 2016, "source": null },
                    "decision": { "accepted": true, "quality": "FLAC 24bit", "rank": 1, "reasons": [] } })
        );
        let (a, d) = keys(&spec, "MusicEvaluation", &evaluated);
        assert_eq!(a, d);
        assert_eq!(
            send(
                &app,
                &admin,
                "POST",
                "/api/v1/music/profiles/99/evaluate",
                Some(json!({ "title": "x" }))
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );

        let choose = |profile_id: Value| json!({ "profile_id": profile_id });
        assert_eq!(
            send(
                &app,
                &admin,
                "PUT",
                "/api/v1/wanted/artists/1/profile",
                Some(choose(json!(id)))
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
        let chosen: Option<i64> =
            sqlx::query_scalar("SELECT profile_id FROM wanted_artists WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(chosen, Some(id));
        assert_eq!(
            send(
                &app,
                &admin,
                "PUT",
                "/api/v1/wanted/artists/1/profile",
                Some(choose(json!(null)))
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            send(
                &app,
                &admin,
                "PUT",
                "/api/v1/wanted/artists/1/profile",
                Some(choose(json!(99)))
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(
                &app,
                &admin,
                "PUT",
                "/api/v1/wanted/artists/9/profile",
                Some(choose(json!(id)))
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn admins_change_music_profiles_and_delete_unused_ones() {
        let pool = crate::db::open_in_memory().await;
        let admin = auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, admin) = auth::issue_token(&pool, admin.id, TokenKind::Session, "t")
            .await
            .unwrap();
        let app = routes(pool.clone());
        let preset = music_profiles::lossless(&pool).await.unwrap();
        // A Lidarr profile, used by an artist by its name.
        let fake = FakeArr::default();
        let lidarr_profiles = json!([{ "id": 1, "name": "Any", "upgradeAllowed": false, "cutoff": 4, "items": [
            { "quality": { "id": 2, "name": "MP3-192" }, "items": [], "allowed": true },
            { "quality": { "id": 4, "name": "MP3-320" }, "items": [], "allowed": true } ] }]);
        fake.reply("/api/v1/system/status", json!({ "appName": "Lidarr" }))
            .reply("/api/v1/qualityprofile", lidarr_profiles);
        let connection = json!({ "url": fake.start().await, "api_key": "key" });
        let (status, _) = send(
            &app,
            &admin,
            "POST",
            "/api/v1/import/lidarr/profiles",
            Some(connection.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let any: i64 = sqlx::query_scalar("SELECT id FROM music_profiles WHERE name = 'Any'")
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at, quality_profile)
             VALUES (1, 'a', 'Alder', 1, 'all', 'lidarr', 0, 'Any')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let put = |id: i64, body: Value| {
            let (app, admin) = (app.clone(), admin.clone());
            async move {
                send(
                    &app,
                    &admin,
                    "PUT",
                    &format!("/api/v1/music/profiles/{id}"),
                    Some(body),
                )
                .await
            }
        };
        let lossless = |name: &str, cutoff: &str, allowed: Value| json!({ "name": name, "upgrade_allowed": true, "cutoff": cutoff, "allowed": allowed });
        let all_lossless = json!(["FLAC", "FLAC 24bit", "ALAC", "ALAC 24bit", "APE", "WavPack"]);

        // The preset: renamed, lossy no longer allowed.
        let (status, body) = put(
            preset,
            lossless("Lossless only", "Lossless", all_lossless.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["name"], "Lossless only");
        let allowed: Vec<&str> = body["qualities"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|q| q["allowed"] == true)
            .map(|q| q["quality"].as_str().unwrap())
            .collect();
        assert_eq!(allowed.len(), 6);
        assert!(!allowed.contains(&"MP3-320"));

        // What's refused, and why.
        for (body, why) in [
            (
                lossless(" ", "Lossless", all_lossless.clone()),
                "a name is 1 to 100 characters",
            ),
            (
                lossless("X", "Lossless", json!([])),
                "allow at least one quality",
            ),
            (
                lossless("X", "Lossless", json!(["FLAC", "WAV"])),
                "WAV isn't one of its qualities",
            ),
            (
                lossless("X", "Lossless", json!(["FLAC"])),
                "Lossless's qualities are allowed or not together",
            ),
            (
                lossless("X", "Lossy", all_lossless.clone()),
                "the cutoff is one of the qualities or groups it allows",
            ),
        ] {
            let (status, answer) = put(preset, body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(answer["error"], why);
        }
        let (status, answer) = put(any, json!({ "name": "Renamed", "upgrade_allowed": true, "cutoff": "MP3-320", "allowed": ["MP3-320"] })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            answer["error"],
            "a profile from Lidarr keeps Lidarr's name: artists from Lidarr find their profile by it"
        );
        let (status, _) = put(99, lossless("X", "Lossless", all_lossless.clone())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // A Lidarr profile changed here stays as set when Lidarr is imported again.
        let (status, body) = put(any, json!({ "name": "Any", "upgrade_allowed": true, "cutoff": "MP3-320", "allowed": ["MP3-320"] })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, _) = send(
            &app,
            &admin,
            "POST",
            "/api/v1/import/lidarr/profiles",
            Some(connection),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let kept = music_profiles::get(&pool, any).await.unwrap().unwrap();
        assert!(kept.upgrade_allowed);
        assert!(
            !kept
                .qualities
                .iter()
                .find(|q| q.quality == "MP3-192")
                .unwrap()
                .allowed
        );

        // Deleting: refused while used, saying by what.
        sqlx::query(
            "INSERT INTO wanted_songs (title, artist, source, added_at, music_profile_id)
                 VALUES ('Airbag', 'Radiohead', 'manual', 0, ?1);
             INSERT INTO request_defaults (kind, music_profile_id) VALUES ('music', ?1);",
        )
        .bind(preset)
        .execute(&pool)
        .await
        .unwrap();
        let delete = |id: i64| {
            let (app, admin) = (app.clone(), admin.clone());
            async move {
                send(
                    &app,
                    &admin,
                    "DELETE",
                    &format!("/api/v1/music/profiles/{id}"),
                    None,
                )
                .await
            }
        };
        let (status, body) = delete(any).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body["error"],
            "Any is used by 1 wanted artist; choose another profile there first"
        );
        let (status, body) = delete(preset).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body["error"],
            "Lossless only is used by 1 wanted song and the one approved music requests get; choose another profile there first"
        );
        sqlx::raw_sql(
            "DELETE FROM wanted_songs; DELETE FROM request_defaults; DELETE FROM wanted_artists;",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(delete(any).await.0, StatusCode::NO_CONTENT);
        assert_eq!(delete(any).await.0, StatusCode::NOT_FOUND);
    }
}
