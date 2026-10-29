//! Importing an *arr's lists: checking the request and its root folders, the import, and the
//! metadata refresh after.

use super::*;

#[derive(Deserialize)]
pub(super) struct ImportRequest {
    /// The *arr as Melete can reach it, e.g. http://192.168.1.10:7878.
    pub(super) url: String,
    pub(super) api_key: String,
    /// The *arr's root folder → a Melete library's name, where the names don't line up.
    #[serde(default)]
    pub(super) root_folders: HashMap<String, String>,
}

/// Mistakes in the request are 400; an *arr that can't be reached or misbehaves is 502.
pub(in super::super) fn import_error(app: &str, err: anyhow::Error) -> ApiError {
    if err.is::<Rejected>() {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("{app} rejected the API key"),
        )
    } else if err.is::<WrongApp>() {
        ApiError::new(StatusCode::BAD_REQUEST, err.to_string())
    } else if err.is::<crate::profiles::import::Unsupported>() {
        ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, err.to_string())
    } else if err.is::<import::RefusedToEmpty>() {
        ApiError::new(StatusCode::CONFLICT, err.to_string())
    } else if err.is::<sqlx::Error>() {
        ApiError::Internal(err)
    } else {
        ApiError::new(StatusCode::BAD_GATEWAY, format!("{err:#}"))
    }
}

pub(super) async fn check_root_folders(
    pool: &SqlitePool,
    kind: &str,
    root_folders: &HashMap<String, String>,
) -> Result<(), ApiError> {
    let known: Vec<String> = import::libraries(pool, kind)
        .await?
        .into_iter()
        .map(|l| l.name)
        .collect();
    let unknown: Vec<&str> = root_folders
        .values()
        .filter(|name| !known.contains(name))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("no {kind} library named {}", unknown.join(", ")),
        ));
    }
    Ok(())
}

/// Runs a metadata refresh in the background, so folders given an *arr's ids get matched and
/// newly wanted titles show on the calendar.
pub(super) fn refresh_soon(state: &ImportState) {
    if let Some(tmdb) = state.tmdb.clone() {
        let pool = state.pool.clone();
        tokio::spawn(async move {
            if let Err(err) = metadata::refresh(&pool, &tmdb).await {
                tracing::warn!(
                    error = format!("{err:#}"),
                    "metadata refresh after import failed"
                );
            }
            if let Err(err) = metadata::refresh_wanted(&pool, &tmdb).await {
                tracing::warn!(
                    error = format!("{err:#}"),
                    "wanted titles' metadata refresh after import failed"
                );
            }
        });
    }
}

pub(super) async fn import_radarr(
    State(state): State<ImportState>,
    Json(request): Json<ImportRequest>,
) -> Result<Json<import::RadarrReport>, ApiError> {
    let client = ArrClient::new(&request.url, request.api_key)
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))?;
    check_root_folders(&state.pool, "movies", &request.root_folders).await?;
    let report = import::import_radarr(&state.pool, &client, &request.root_folders)
        .await
        .map_err(|err| import_error("Radarr", err))?;
    if report.rematching > 0 {
        refresh_soon(&state);
    }
    Ok(Json(report))
}

pub(super) async fn import_sonarr(
    State(state): State<ImportState>,
    Json(request): Json<ImportRequest>,
) -> Result<Json<import::SonarrReport>, ApiError> {
    let client = ArrClient::new(&request.url, request.api_key)
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))?;
    check_root_folders(&state.pool, "shows", &request.root_folders).await?;
    let report = import::import_sonarr(&state.pool, &client, &request.root_folders)
        .await
        .map_err(|err| import_error("Sonarr", err))?;
    if report.rematching > 0 {
        refresh_soon(&state);
    }
    Ok(Json(report))
}

pub(super) async fn import_lidarr(
    State(state): State<ImportState>,
    Json(request): Json<ImportRequest>,
) -> Result<Json<import::LidarrReport>, ApiError> {
    let client = ArrClient::new(&request.url, request.api_key)
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))?;
    check_root_folders(&state.pool, "music", &request.root_folders).await?;
    let report = import::import_lidarr(&state.pool, &client, &request.root_folders)
        .await
        .map_err(|err| import_error("Lidarr", err))?;
    Ok(Json(report))
}
