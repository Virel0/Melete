//! Quality profiles and custom formats (admins only): importing them from Radarr and Sonarr,
//! making and changing them in Melete, reading them, and trying a release name against a
//! profile to see the decision and why.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router, middleware};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use super::import::import_error;
use crate::arr::ArrClient;
use crate::profiles::decide::{Decision, Existing, MatchedFormat};
use crate::profiles::formats::Facts;
use crate::profiles::{self, App, CustomFormat, ImportReport, Profile, edit};
use crate::release;

/// The quality profile and custom format routes, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/import/radarr/profiles", post(import_radarr))
        .route("/api/v1/import/sonarr/profiles", post(import_sonarr))
        .route("/api/v1/profiles", get(list).post(create_profile))
        .route("/api/v1/profiles/presets/anime", post(anime_preset))
        .route(
            "/api/v1/profiles/{id}",
            get(one).put(update_profile).delete(delete_profile),
        )
        .route("/api/v1/profiles/{id}/evaluate", post(evaluate))
        .route(
            "/api/v1/custom-formats",
            get(custom_formats).post(create_format),
        )
        .route(
            "/api/v1/custom-formats/{id}",
            put(update_format).delete(delete_format),
        )
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(pool)
}

#[derive(Deserialize)]
struct Connection {
    url: String,
    api_key: String,
}

async fn import(
    pool: &SqlitePool,
    app: App,
    connection: Connection,
) -> Result<Json<ImportReport>, ApiError> {
    let client = ArrClient::new(&connection.url, connection.api_key)
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))?;
    let report = profiles::import(pool, &client, app)
        .await
        .map_err(|err| import_error(app.name(), err))?;
    Ok(Json(report))
}

async fn import_radarr(
    State(pool): State<SqlitePool>,
    Json(connection): Json<Connection>,
) -> Result<Json<ImportReport>, ApiError> {
    import(&pool, App::Radarr, connection).await
}

async fn import_sonarr(
    State(pool): State<SqlitePool>,
    Json(connection): Json<Connection>,
) -> Result<Json<ImportReport>, ApiError> {
    import(&pool, App::Sonarr, connection).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    /// Only this Manage service's profiles: movies, series or anime.
    service: Option<String>,
}

async fn list(
    State(pool): State<SqlitePool>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<Profile>>, ApiError> {
    if let Some(service) = &query.service
        && !profiles::service::SERVICES.contains(&service.as_str())
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "service is movies, series or anime",
        ));
    }
    let all = profiles::list(&pool).await?;
    Ok(Json(
        all.into_iter()
            .filter(|p| query.service.as_ref().is_none_or(|s| *s == p.service))
            .collect(),
    ))
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "no such quality profile")
}

async fn one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<Profile>, ApiError> {
    profiles::get(&pool, id)
        .await?
        .map(Json)
        .ok_or_else(not_found)
}

async fn custom_formats(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<CustomFormat>>, ApiError> {
    Ok(Json(profiles::custom_formats(&pool).await?))
}

fn refused(what: &str, refused: edit::Refused) -> ApiError {
    match refused {
        edit::Refused::Invalid(why) => ApiError::new(StatusCode::BAD_REQUEST, why),
        edit::Refused::NotFound => ApiError::new(StatusCode::NOT_FOUND, format!("no such {what}")),
        edit::Refused::Taken => ApiError::new(
            StatusCode::CONFLICT,
            format!("another {what} for the same kind of titles has that name"),
        ),
    }
}

async fn format_view(pool: &SqlitePool, id: i64) -> Result<CustomFormat, ApiError> {
    profiles::custom_formats(pool)
        .await?
        .into_iter()
        .find(|f| f.id == id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such custom format"))
}

async fn create_format(
    State(pool): State<SqlitePool>,
    Json(input): Json<edit::FormatInput>,
) -> Result<(StatusCode, Json<CustomFormat>), ApiError> {
    let id = edit::create_format(&pool, &input)
        .await?
        .map_err(|r| refused("custom format", r))?;
    Ok((StatusCode::CREATED, Json(format_view(&pool, id).await?)))
}

async fn update_format(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(input): Json<edit::FormatInput>,
) -> Result<Json<CustomFormat>, ApiError> {
    edit::update_format(&pool, id, &input)
        .await?
        .map_err(|r| refused("custom format", r))?;
    Ok(Json(format_view(&pool, id).await?))
}

async fn delete_format(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    match edit::delete_format(&pool, id).await? {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such custom format",
        )),
    }
}

async fn create_profile(
    State(pool): State<SqlitePool>,
    Json(input): Json<edit::ProfileInput>,
) -> Result<(StatusCode, Json<Profile>), ApiError> {
    let id = edit::create_profile(&pool, &input)
        .await?
        .map_err(|r| refused("quality profile", r))?;
    let profile = profiles::get(&pool, id).await?.ok_or_else(not_found)?;
    Ok((StatusCode::CREATED, Json(profile)))
}

#[derive(Deserialize)]
struct PresetRequest {
    /// "sonarr" for series, "radarr" for movies.
    source: String,
    /// For series: also choose it for every wanted series Sonarr calls anime.
    #[serde(default)]
    apply_to_anime: bool,
}

/// Makes the anime profile and its formats.
async fn anime_preset(
    State(pool): State<SqlitePool>,
    Json(request): Json<PresetRequest>,
) -> Result<(StatusCode, Json<Profile>), ApiError> {
    let id = edit::anime_preset(&pool, &request.source)
        .await?
        .map_err(|r| refused("quality profile", r))?;
    if request.apply_to_anime && request.source == "sonarr" {
        // Every series under Anime: wanted as anime, or in a library with the anime category.
        sqlx::query(
            "UPDATE wanted_series SET profile_id = ? WHERE id IN
                 (SELECT wanted_id FROM title_event_titles WHERE kind = 'series' AND service = 'anime')",
        )
        .bind(id)
        .execute(&pool)
        .await?;
    }
    let profile = profiles::get(&pool, id).await?.ok_or_else(not_found)?;
    Ok((StatusCode::CREATED, Json(profile)))
}

async fn update_profile(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(input): Json<edit::ProfileInput>,
) -> Result<Json<Profile>, ApiError> {
    edit::update_profile(&pool, id, &input)
        .await?
        .map_err(|r| refused("quality profile", r))?;
    profiles::get(&pool, id)
        .await?
        .map(Json)
        .ok_or_else(not_found)
}

async fn delete_profile(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    match edit::delete_profile(&pool, id).await? {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(not_found()),
    }
}

#[derive(Deserialize)]
struct EvaluateRequest {
    name: String,
    /// Bytes, as an indexer would give it.
    size: Option<u64>,
    #[serde(default)]
    indexer_flags: Vec<String>,
    /// The title's original language (ISO 639-1), which a name without languages is assumed
    /// to be in.
    original_language: Option<String>,
    /// The name of the file already on disk, to decide whether this would be an upgrade.
    existing: Option<String>,
}

#[derive(Serialize)]
struct Evaluation {
    decision: Decision,
    existing: Option<ExistingFile>,
}

#[derive(Serialize)]
struct ExistingFile {
    quality: String,
    formats: Vec<MatchedFormat>,
    score: i64,
}

fn check_name(field: &str, name: &str) -> Result<(), ApiError> {
    if name.trim().is_empty() || name.chars().count() > 500 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("{field} is 1 to 500 characters"),
        ));
    }
    Ok(())
}

async fn evaluate(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(request): Json<EvaluateRequest>,
) -> Result<Json<Evaluation>, ApiError> {
    check_name("name", &request.name)?;
    if let Some(existing) = &request.existing {
        check_name("existing", existing)?;
    }
    if request
        .original_language
        .as_ref()
        .is_some_and(|l| l.is_empty() || l.len() > 16)
        || request.indexer_flags.len() > 20
        || request.indexer_flags.iter().any(|f| f.len() > 40)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "original_language or indexer_flags is too long",
        ));
    }
    let loaded = profiles::load(&pool, id).await?.ok_or_else(not_found)?;
    let original = request.original_language.as_deref();
    let existing = match &request.existing {
        Some(name) => {
            let parsed = release::parse(name.trim());
            let facts = Facts::new(name.trim(), &parsed, None, &[], original);
            let (formats, score) = loaded.score(&facts).map_err(|err| {
                ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("the existing file: {err}"),
                )
            })?;
            let file = ExistingFile {
                quality: facts.quality.name.to_owned(),
                formats,
                score,
            };
            Some((file, parsed.proper))
        }
        None => None,
    };
    let name = request.name.trim();
    let parsed = release::parse(name);
    let facts = Facts::new(
        name,
        &parsed,
        request.size,
        &request.indexer_flags,
        original,
    );
    let on_disk = existing.as_ref().map(|(file, proper)| Existing {
        quality: file.quality.clone(),
        proper: *proper,
        score: file.score,
    });
    let decision = loaded.decide(&facts, on_disk.as_ref());
    Ok(Json(Evaluation {
        decision,
        existing: existing.map(|(file, _)| file),
    }))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::header;
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;
    use crate::import::testing::FakeArr;

    struct Fixture {
        app: Router,
        admin: String,
        viewer: String,
    }

    async fn fixture() -> Fixture {
        let pool = db::open_in_memory().await;
        let mut tokens = Vec::new();
        for (name, admin) in [("admin", true), ("viewer", false)] {
            let user = auth::create_user(&pool, name, format!("{name}-password"), admin)
                .await
                .unwrap()
                .unwrap();
            let (_, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
                .await
                .unwrap();
            tokens.push(token);
        }
        let viewer = tokens.pop().unwrap();
        Fixture {
            app: routes(pool),
            admin: tokens.pop().unwrap(),
            viewer,
        }
    }

    async fn send(
        f: &Fixture,
        method: &str,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap();
        let response = f.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// A Radarr serving the captured fixture, changed by `edit`.
    async fn radarr(edit: impl FnOnce(&mut Value)) -> String {
        let mut captured: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/radarr.json")).unwrap();
        edit(&mut captured);
        let fake = FakeArr::default();
        fake.reply("/api/v3/system/status", json!({ "appName": "Radarr" }))
            .reply("/api/v3/language", captured["languages"].clone())
            .reply("/api/v3/customformat", captured["customformats"].clone())
            .reply(
                "/api/v3/qualityprofile",
                captured["qualityprofiles"].clone(),
            );
        fake.start().await
    }

    async fn import(f: &Fixture, url: &str) -> (StatusCode, Value) {
        send(
            f,
            "POST",
            "/api/v1/import/radarr/profiles",
            &f.admin,
            Some(json!({ "url": url, "api_key": "key" })),
        )
        .await
    }

    #[tokio::test]
    async fn imports_lists_and_evaluates() {
        let f = fixture().await;
        let (status, report) = import(&f, &radarr(|_| {}).await).await;
        assert_eq!(status, StatusCode::OK, "{report}");
        assert_eq!(
            report,
            json!({ "formats": 18, "profiles": 2, "removed_formats": 0, "removed_profiles": 0 })
        );

        let (status, profiles) = send(&f, "GET", "/api/v1/profiles", &f.admin, None).await;
        assert_eq!(status, StatusCode::OK);
        let names: Vec<&str> = profiles
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["4K Preferred", "HD-1080p"]);
        let id = profiles[0]["id"].as_i64().unwrap();
        let (_, profile) = send(&f, "GET", &format!("/api/v1/profiles/{id}"), &f.admin, None).await;
        assert_eq!(profile, profiles[0]);
        assert_eq!(profile["cutoff"], "Remux-2160p");
        assert_eq!(profile["language"], "en");
        let web = profile["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == "WEB 2160p")
            .unwrap();
        assert_eq!(web["qualities"], json!(["WEBDL-2160p", "WEBRip-2160p"]));
        assert_eq!(web["allowed"], true);

        let (_, formats) = send(&f, "GET", "/api/v1/custom-formats", &f.admin, None).await;
        let hdr10 = formats
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "HDR10")
            .unwrap();
        assert_eq!(
            hdr10["conditions"][1],
            json!({ "name": "Not HDR10+", "negate": true, "required": true, "kind": "release_title",
                    "pattern": "\\bhdr10(\\+|p(lus)?)\\b" })
        );

        let (status, evaluation) = send(
            &f,
            "POST",
            &format!("/api/v1/profiles/{id}/evaluate"),
            &f.admin,
            Some(json!({ "name": "Movie.2023.2160p.WEB-DL.DDP5.1.DV.HDR.H.265-GRP", "original_language": "en",
                         "existing": "Movie.2023.1080p.WEB-DL.DDP5.1.H.264-GRP" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{evaluation}");
        assert_eq!(evaluation["decision"]["accepted"], true, "{evaluation}");
        assert_eq!(evaluation["decision"]["quality"], "WEBDL-2160p");
        // DV and HDR10; "DDP5.1" has no word boundary before the 5, so the test format's
        // \b5[ .]1\b doesn't match it (nor would it in Radarr).
        assert_eq!(evaluation["decision"]["score"], 150 + 100);
        assert_eq!(evaluation["existing"]["quality"], "WEBDL-1080p");
        assert_eq!(evaluation["existing"]["score"], 100);
    }

    #[tokio::test]
    async fn a_reimport_drops_what_the_app_dropped() {
        let f = fixture().await;
        import(&f, &radarr(|_| {}).await).await;
        let url = radarr(|captured| {
            captured["qualityprofiles"]
                .as_array_mut()
                .unwrap()
                .retain(|p| p["name"] == "4K Preferred");
            let profile = &mut captured["qualityprofiles"][0];
            profile["formatItems"]
                .as_array_mut()
                .unwrap()
                .retain(|i| i["name"] == "DV");
            captured["customformats"]
                .as_array_mut()
                .unwrap()
                .retain(|c| c["name"] == "DV");
        })
        .await;
        let (status, report) = import(&f, &url).await;
        assert_eq!(status, StatusCode::OK, "{report}");
        assert_eq!(
            report,
            json!({ "formats": 1, "profiles": 1, "removed_formats": 17, "removed_profiles": 1 })
        );
        let (_, profiles) = send(&f, "GET", "/api/v1/profiles", &f.admin, None).await;
        assert_eq!(
            profiles[0]["formats"],
            json!([{ "format_id": profiles[0]["formats"][0]["format_id"], "name": "DV", "score": 150 }])
        );
    }

    #[tokio::test]
    async fn explains_what_went_wrong() {
        let f = fixture().await;
        let (status, _) = send(&f, "GET", "/api/v1/profiles", &f.viewer, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = send(&f, "GET", "/api/v1/profiles/9", &f.admin, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(
            &f,
            "POST",
            "/api/v1/profiles/9/evaluate",
            &f.admin,
            Some(json!({ "name": "Movie.2020.1080p" })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let unsupported = radarr(|captured| {
            captured["customformats"][0]["specifications"][0]["implementation"] =
                "SomeFutureSpecification".into();
        })
        .await;
        let (status, body) = import(&f, &unsupported).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("SomeFutureSpecification"),
            "{body}"
        );
        let empty = radarr(|captured| captured["qualityprofiles"] = json!([])).await;
        let (status, _) = import(&f, &empty).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) = send(&f, "GET", "/api/v1/profiles", &f.admin, None).await;
        assert_eq!(status, StatusCode::OK);

        import(&f, &radarr(|_| {}).await).await;
        for body in [
            json!({ "name": "" }),
            json!({ "name": "x".repeat(501) }),
            json!({ "name": "ok", "existing": " " }),
        ] {
            let (status, _) = send(
                &f,
                "POST",
                "/api/v1/profiles/1/evaluate",
                &f.admin,
                Some(body),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
    }

    fn english_subs_format() -> Value {
        json!({ "source": "sonarr", "name": "English Subtitles",
                "conditions": [{ "name": "en subs", "negate": false, "required": false, "kind": "subtitles", "language": "en" }] })
    }

    #[tokio::test]
    async fn formats_are_made_changed_and_deleted_in_melete() {
        let f = fixture().await;
        let (status, format) = send(
            &f,
            "POST",
            "/api/v1/custom-formats",
            &f.admin,
            Some(english_subs_format()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{format}");
        assert_eq!(
            (format["origin"].as_str(), format["source"].as_str()),
            (Some("melete"), Some("sonarr"))
        );
        let id = format["id"].as_i64().unwrap();
        let (status, _) = send(
            &f,
            "POST",
            "/api/v1/custom-formats",
            &f.admin,
            Some(english_subs_format()),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "names are unique per kind of title"
        );
        let bad = json!({ "source": "sonarr", "name": "Bad",
                          "conditions": [{ "name": "c", "negate": false, "required": false, "kind": "release_title", "pattern": "(unclosed" }] });
        let (status, body) = send(&f, "POST", "/api/v1/custom-formats", &f.admin, Some(bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("Bad"), "{body}");
        for odd in [
            json!({ "source": "lidarr", "name": "X", "conditions": [] }),
            json!({ "source": "sonarr", "name": " ", "conditions": [] }),
        ] {
            let (status, _) = send(&f, "POST", "/api/v1/custom-formats", &f.admin, Some(odd)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        let mut renamed = english_subs_format();
        renamed["name"] = "Eng Subs".into();
        let (status, changed) = send(
            &f,
            "PUT",
            &format!("/api/v1/custom-formats/{id}"),
            &f.admin,
            Some(renamed.clone()),
        )
        .await;
        assert_eq!(
            (status, changed["name"].as_str()),
            (StatusCode::OK, Some("Eng Subs"))
        );
        renamed["source"] = "radarr".into();
        let (status, _) = send(
            &f,
            "PUT",
            &format!("/api/v1/custom-formats/{id}"),
            &f.admin,
            Some(renamed),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "a format can't change sides"
        );
        let (status, _) = send(
            &f,
            "DELETE",
            &format!("/api/v1/custom-formats/{id}"),
            &f.admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = send(
            &f,
            "DELETE",
            &format!("/api/v1/custom-formats/{id}"),
            &f.admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(
            &f,
            "POST",
            "/api/v1/custom-formats",
            &f.viewer,
            Some(english_subs_format()),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    fn anime_profile(format_id: i64) -> Value {
        json!({
            "source": "sonarr", "name": "Anime", "upgrade_allowed": true, "cutoff": "Bluray-2160p",
            "min_format_score": 0, "cutoff_format_score": 0, "min_upgrade_format_score": 1, "language": null,
            "items": [
                { "name": "WEB 1080p", "qualities": ["WEBDL-1080p", "WEBRip-1080p"], "allowed": true },
                { "name": "Bluray-1080p", "qualities": ["Bluray-1080p"], "allowed": true },
                { "name": "Bluray-2160p", "qualities": ["Bluray-2160p"], "allowed": true }
            ],
            "formats": [{ "format_id": format_id, "score": 100 }]
        })
    }

    #[tokio::test]
    async fn profiles_are_made_checked_and_decide_with_subtitles() {
        let f = fixture().await;
        let (_, format) = send(
            &f,
            "POST",
            "/api/v1/custom-formats",
            &f.admin,
            Some(english_subs_format()),
        )
        .await;
        let format_id = format["id"].as_i64().unwrap();
        let (status, profile) = send(
            &f,
            "POST",
            "/api/v1/profiles",
            &f.admin,
            Some(anime_profile(format_id)),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{profile}");
        assert_eq!(profile["origin"], "melete");
        assert_eq!(
            profile["items"][0],
            json!({ "name": "WEB 1080p", "qualities": ["WEBDL-1080p", "WEBRip-1080p"], "allowed": true })
        );
        assert_eq!(
            profile["formats"],
            json!([{ "format_id": format_id, "name": "English Subtitles", "score": 100 }])
        );
        let id = profile["id"].as_i64().unwrap();

        // The subtitles condition, through a decision.
        let (_, evaluation) = send(
            &f,
            "POST",
            &format!("/api/v1/profiles/{id}/evaluate"),
            &f.admin,
            Some(json!({ "name": "Frieren.S01E05.JAPANESE.1080p.WEB.H264.ENG.SUBS-GRP" })),
        )
        .await;
        assert_eq!(
            evaluation["decision"]["formats"],
            json!([{ "name": "English Subtitles", "score": 100 }])
        );

        // What's refused, and why.
        type Change = Box<dyn Fn(&mut Value)>;
        let cases: Vec<(&str, Change)> = vec![
            (
                "isn't a quality",
                Box::new(|p| p["items"][0]["qualities"][0] = "WEBDL-4320p".into()),
            ),
            (
                "more than one item",
                Box::new(|p| p["items"][1]["qualities"][0] = "WEBDL-1080p".into()),
            ),
            (
                "isn't one of the items",
                Box::new(|p| p["cutoff"] = "Remux-2160p".into()),
            ),
            (
                "isn't allowed",
                Box::new(|p| p["items"][2]["allowed"] = false.into()),
            ),
            (
                "at least 1",
                Box::new(|p| p["min_upgrade_format_score"] = 0.into()),
            ),
            (
                "no format",
                Box::new(|p| p["formats"][0]["format_id"] = 999.into()),
            ),
        ];
        for (expected, change) in cases {
            let mut body = anime_profile(format_id);
            body["name"] = "Other".into();
            change(&mut body);
            let (status, error) = send(&f, "POST", "/api/v1/profiles", &f.admin, Some(body)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{expected}");
            assert!(
                error["error"].as_str().unwrap().contains(expected),
                "{error}"
            );
        }
        let mut movies = anime_profile(format_id);
        movies["source"] = "radarr".into();
        let (_, error) = send(&f, "POST", "/api/v1/profiles", &f.admin, Some(movies)).await;
        assert!(
            error["error"].as_str().unwrap().contains("is for sonarr"),
            "{error}"
        );

        let mut changed = anime_profile(format_id);
        changed["min_format_score"] = 100.into();
        let (status, profile) = send(
            &f,
            "PUT",
            &format!("/api/v1/profiles/{id}"),
            &f.admin,
            Some(changed),
        )
        .await;
        assert_eq!(
            (status, profile["min_format_score"].as_i64()),
            (StatusCode::OK, Some(100))
        );
        let (status, _) = send(
            &f,
            "POST",
            "/api/v1/profiles",
            &f.admin,
            Some(anime_profile(format_id)),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) = send(
            &f,
            "DELETE",
            &format!("/api/v1/profiles/{id}"),
            &f.admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = send(
            &f,
            "PUT",
            &format!("/api/v1/profiles/{id}"),
            &f.admin,
            Some(anime_profile(format_id)),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn reimports_leave_melete_s_own_alone() {
        let f = fixture().await;
        let url = radarr(|_| {}).await;
        import(&f, &url).await;
        let (_, profiles) = send(&f, "GET", "/api/v1/profiles", &f.admin, None).await;
        let imported = profiles
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "4K Preferred")
            .unwrap()
            .clone();
        let id = imported["id"].as_i64().unwrap();
        // Edit the imported profile in Melete: it becomes Melete's own.
        let edited = json!({
            "source": "radarr", "name": "4K Preferred", "upgrade_allowed": false, "cutoff": "Bluray-1080p",
            "min_format_score": 50, "cutoff_format_score": 0, "min_upgrade_format_score": 1, "language": "en",
            "items": [{ "name": "Bluray-1080p", "qualities": ["Bluray-1080p"], "allowed": true }], "formats": []
        });
        let (status, _) = send(
            &f,
            "PUT",
            &format!("/api/v1/profiles/{id}"),
            &f.admin,
            Some(edited),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // And a format of Melete's own the app doesn't have.
        let mine = json!({ "source": "radarr", "name": "Mine",
                           "conditions": [{ "name": "c", "negate": false, "required": false, "kind": "release_title", "pattern": "mine" }] });
        let (status, _) = send(&f, "POST", "/api/v1/custom-formats", &f.admin, Some(mine)).await;
        assert_eq!(status, StatusCode::CREATED);

        let (status, _) = import(&f, &url).await;
        assert_eq!(status, StatusCode::OK);
        let (_, after) = send(&f, "GET", &format!("/api/v1/profiles/{id}"), &f.admin, None).await;
        assert_eq!(
            (after["min_format_score"].as_i64(), after["origin"].as_str()),
            (Some(50), Some("melete"))
        );
        assert_eq!(
            after["items"].as_array().unwrap().len(),
            1,
            "Melete's edit kept"
        );
        let (_, formats) = send(&f, "GET", "/api/v1/custom-formats", &f.admin, None).await;
        assert!(
            formats
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["name"] == "Mine"),
            "not removed as stale"
        );
    }

    #[tokio::test]
    async fn series_and_anime_each_have_their_own_profiles() {
        let f = fixture().await;
        let profile = |service: Option<&str>| {
            let mut body = json!({
                "source": "sonarr", "name": "HD-1080p", "upgrade_allowed": true,
                "cutoff": "WEBDL-1080p", "min_format_score": 0, "cutoff_format_score": 0,
                "min_upgrade_format_score": 1, "language": null, "formats": [],
                "items": [{ "name": "WEBDL-1080p", "qualities": ["WEBDL-1080p"], "allowed": true }]
            });
            if let Some(service) = service {
                body["service"] = json!(service);
            }
            body
        };
        // One for Anime, then one of the same name for Series: names are kept apart by service.
        let (status, anime) = send(
            &f,
            "POST",
            "/api/v1/profiles",
            &f.admin,
            Some(profile(Some("anime"))),
        )
        .await;
        assert_eq!(
            (status, anime["service"].as_str()),
            (StatusCode::CREATED, Some("anime"))
        );
        let (status, series) = send(
            &f,
            "POST",
            "/api/v1/profiles",
            &f.admin,
            Some(profile(None)),
        )
        .await;
        assert_eq!(
            (status, series["service"].as_str()),
            (StatusCode::CREATED, Some("series"))
        );
        let (status, _) = send(
            &f,
            "POST",
            "/api/v1/profiles",
            &f.admin,
            Some(profile(Some("anime"))),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "one HD-1080p for Anime");

        let ids = |list: Value| -> Vec<Value> {
            list.as_array()
                .unwrap()
                .iter()
                .map(|p| p["id"].clone())
                .collect()
        };
        let (_, under_series) =
            send(&f, "GET", "/api/v1/profiles?service=series", &f.admin, None).await;
        assert_eq!(
            ids(under_series),
            [series["id"].clone()],
            "Anime's isn't Series'"
        );
        let (_, under_anime) =
            send(&f, "GET", "/api/v1/profiles?service=anime", &f.admin, None).await;
        assert_eq!(ids(under_anime), [anime["id"].clone()]);
        let (status, _) = send(&f, "GET", "/api/v1/profiles?service=music", &f.admin, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // A profile keeps its service: a change that names another is refused, one that leaves
        // it out keeps it.
        let uri = format!("/api/v1/profiles/{}", anime["id"]);
        let (status, _) = send(&f, "PUT", &uri, &f.admin, Some(profile(Some("series")))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, kept) = send(&f, "PUT", &uri, &f.admin, Some(profile(None))).await;
        assert_eq!(
            (status, kept["service"].as_str()),
            (StatusCode::OK, Some("anime"))
        );
        // Sonarr's profiles are Series' or Anime's, Radarr's Movies'.
        let mut wrong = profile(Some("movies"));
        wrong["name"] = json!("Other");
        let (status, _) = send(&f, "POST", "/api/v1/profiles", &f.admin, Some(wrong)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        // The anime preset for series is Anime's.
        let (_, preset) = send(
            &f,
            "POST",
            "/api/v1/profiles/presets/anime",
            &f.admin,
            Some(json!({ "source": "sonarr" })),
        )
        .await;
        assert_eq!(preset["service"], "anime");
    }
}
