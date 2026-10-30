//! Choosing the quality profile a wanted movie or series is searched and grabbed with.

use super::*;

#[derive(Deserialize)]
pub(super) struct ProfileChoice {
    /// A quality profile for this kind of title, or null to use the one the *arr names.
    pub(super) profile_id: Option<i64>,
}

/// Sets the profile a wanted title is searched and grabbed with; re-imports keep it. The profile
/// must be for the title's service: an anime series takes one of Anime's, never one of Series'.
pub(super) async fn choose_profile(
    pool: &SqlitePool,
    table: &str,
    source: &str,
    id: i64,
    choice: ProfileChoice,
) -> Result<StatusCode, ApiError> {
    if let Some(profile_id) = choice.profile_id {
        let kind: Option<(String, String)> =
            sqlx::query_as("SELECT source, service FROM quality_profiles WHERE id = ?")
                .bind(profile_id)
                .fetch_optional(pool)
                .await?;
        let wanted_service = match source {
            "sonarr" => crate::profiles::service::of_wanted_series(pool, id).await?,
            _ => "movies".into(),
        };
        match kind {
            None => {
                return Err(ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "no such quality profile",
                ));
            }
            Some((kind, _)) if kind != source => {
                return Err(ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("that profile is for {kind} titles; this one needs a {source} profile"),
                ));
            }
            Some((_, service)) if service != wanted_service => {
                return Err(ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!(
                        "that profile is one of {service}'s; this series needs one of {wanted_service}'s"
                    ),
                ));
            }
            Some(_) => {}
        }
    }
    let changed = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET profile_id = ? WHERE id = ?"
    )))
    .bind(choice.profile_id)
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such wanted title"));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn choose_movie_profile(
    State(state): State<ImportState>,
    Path(id): Path<i64>,
    Json(choice): Json<ProfileChoice>,
) -> Result<StatusCode, ApiError> {
    choose_profile(&state.pool, "wanted_movies", "radarr", id, choice).await
}

pub(super) async fn choose_series_profile(
    State(state): State<ImportState>,
    Path(id): Path<i64>,
    Json(choice): Json<ProfileChoice>,
) -> Result<StatusCode, ApiError> {
    choose_profile(&state.pool, "wanted_series", "sonarr", id, choice).await
}
