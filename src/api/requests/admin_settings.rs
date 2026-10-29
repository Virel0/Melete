//! The requests settings: each kind's defaults, and whose requests are let through.

use super::*;

pub(super) const KINDS: &[&str] = &["movies", "series", "anime", "music"];

pub(super) async fn settings(State(state): State<RequestState>) -> Result<Json<Value>, ApiError> {
    // Kind, library, quality profile and music profile, for the kinds that are set.
    type Set = (String, Option<i64>, Option<i64>, Option<i64>);
    let set: Vec<Set> = sqlx::query_as(
        "SELECT kind, library_id, profile_id, music_profile_id FROM request_defaults",
    )
    .fetch_all(&state.pool)
    .await?;
    let defaults: Vec<Value> = KINDS
        .iter()
        .map(|kind| {
            let row = set.iter().find(|(k, ..)| k == kind);
            json!({
                "kind": kind,
                "library_id": row.and_then(|r| r.1),
                "profile_id": row.and_then(|r| r.2),
                "music_profile_id": row.and_then(|r| r.3),
            })
        })
        .collect();
    let users: Vec<(i64, String, bool, bool)> = sqlx::query_as(
        "SELECT id, username, is_admin, requests_approved FROM users ORDER BY username",
    )
    .fetch_all(&state.pool)
    .await?;
    let users: Vec<Value> = users
        .into_iter()
        .map(|(id, username, is_admin, approved)| {
            json!({ "id": id, "username": username, "is_admin": is_admin, "requests_approved": approved || is_admin })
        })
        .collect();
    Ok(Json(json!({ "defaults": defaults, "users": users })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Defaults {
    pub(super) library_id: Option<i64>,
    pub(super) profile_id: Option<i64>,
    pub(super) music_profile_id: Option<i64>,
}

pub(super) async fn set_defaults(
    State(state): State<RequestState>,
    Path(kind): Path<String>,
    Json(body): Json<Defaults>,
) -> Result<Json<Value>, ApiError> {
    let bad = |message: &str| ApiError::new(StatusCode::BAD_REQUEST, message.to_owned());
    if !KINDS.contains(&kind.as_str()) {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "the kind is movies, series, anime or music",
        ));
    }
    let music = kind == "music";
    if music && body.profile_id.is_some() {
        return Err(bad("music takes music_profile_id, not profile_id"));
    }
    if !music && body.music_profile_id.is_some() {
        return Err(bad("only music takes music_profile_id"));
    }
    if let Some(id) = body.library_id {
        let library_kind: Option<String> =
            sqlx::query_scalar("SELECT kind FROM libraries WHERE id = ?")
                .bind(id)
                .fetch_optional(&state.pool)
                .await?;
        let wanted = match kind.as_str() {
            "movies" => "movies",
            "music" => "music",
            _ => "shows",
        };
        if library_kind.as_deref() != Some(wanted) {
            return Err(bad(&format!("library {id} isn't a {wanted} library")));
        }
    }
    if let Some(id) = body.profile_id {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM quality_profiles WHERE id = ?)")
                .bind(id)
                .fetch_one(&state.pool)
                .await?;
        if !exists {
            return Err(bad("there's no such quality profile"));
        }
    }
    if let Some(id) = body.music_profile_id {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM music_profiles WHERE id = ?)")
                .bind(id)
                .fetch_one(&state.pool)
                .await?;
        if !exists {
            return Err(bad("there's no such music profile"));
        }
    }
    sqlx::query(
        "INSERT INTO request_defaults (kind, library_id, profile_id, music_profile_id) VALUES (?, ?, ?, ?)
         ON CONFLICT (kind) DO UPDATE SET library_id = excluded.library_id,
             profile_id = excluded.profile_id, music_profile_id = excluded.music_profile_id",
    )
    .bind(&kind)
    .bind(body.library_id)
    .bind(body.profile_id)
    .bind(body.music_profile_id)
    .execute(&state.pool)
    .await?;
    Ok(Json(json!({
        "kind": kind, "library_id": body.library_id, "profile_id": body.profile_id,
        "music_profile_id": body.music_profile_id,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UserSetting {
    pub(super) requests_approved: bool,
}

pub(super) async fn set_user(
    State(state): State<RequestState>,
    Path(id): Path<i64>,
    Json(body): Json<UserSetting>,
) -> Result<StatusCode, ApiError> {
    let changed = sqlx::query("UPDATE users SET requests_approved = ? WHERE id = ?")
        .bind(body.requests_approved)
        .bind(id)
        .execute(&state.pool)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such user"));
    }
    Ok(StatusCode::NO_CONTENT)
}
