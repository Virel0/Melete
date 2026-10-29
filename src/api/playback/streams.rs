//! Stream links for a file and the streams they open, with byte ranges so players can seek.

use super::*;

/// Creates a stream URL for a file, with where to resume and the tracks to play it with.
pub(super) async fn play(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(file_id): Path<i64>,
    limit: Option<Extension<crate::ways_in::VideoLimit>>,
) -> Result<Json<Value>, ApiError> {
    require_playable(&pool, &current.access, file_id).await?;
    let tracks = super::super::languages::picked(&pool, current.user.id, file_id).await?;
    let (token, expires_at) = create_play_session(&pool, current.token_id, file_id, None).await?;
    let progress = load_progress(&pool, current.user.id, file_id)
        .await?
        .unwrap_or_default();
    Ok(Json(json!({
        "stream_url": format!("/api/v1/stream/{token}"),
        "expires_at": expires_at,
        "resume_seconds": progress.position_seconds,
        "duration_seconds": progress.duration_seconds,
        "watched": progress.watched,
        "segments": super::super::segments::of_file(&pool, file_id).await?,
        "tracks": tracks,
        "max_bitrate": way_limit(limit),
    })))
}

/// Creates a stream link for a file and returns its token and expiry. With `transcode`
/// settings the token also opens an HLS playlist of the file re-encoded with them.
pub(in super::super) async fn create_play_session(
    pool: &SqlitePool,
    token_id: i64,
    file_id: i64,
    transcode: Option<&str>,
) -> Result<(String, i64), ApiError> {
    let now = unix_now();
    // An extra's links are kept apart, since a file's link must name a library file.
    let table = if extras::is_extra_id(file_id) {
        ("extra_play_sessions", "extra_id")
    } else {
        ("play_sessions", "file_id")
    };
    let (sessions, column) = table;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM {sessions} WHERE expires_at <= ?"
    )))
    .bind(now)
    .execute(pool)
    .await?;
    let token = auth::random_token(STREAM_PREFIX)?;
    let expires_at = now + STREAM_LIFETIME;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO {sessions} (token_hash, token_id, {column}, created_at, expires_at, transcode)
         VALUES (?, ?, ?, ?, ?, ?)"
    )))
    .bind(auth::token_hash(&token))
    .bind(token_id)
    .bind(file_id)
    .bind(now)
    .bind(expires_at)
    .bind(transcode)
    .execute(pool)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM {sessions} WHERE token_id = ?1 AND id NOT IN
             (SELECT id FROM {sessions} WHERE token_id = ?1 ORDER BY id DESC LIMIT ?2)"
    )))
    .bind(token_id)
    .bind(MAX_STREAMS_PER_TOKEN)
    .execute(pool)
    .await?;
    Ok((token, expires_at))
}

/// Streams a file for a stream URL, with byte ranges so players can seek. Through a way in with
/// a video limit, a file over it is refused, downloads included.
pub(super) async fn stream(
    State(pool): State<SqlitePool>,
    Path(token): Path<String>,
    limit: Option<Extension<crate::ways_in::VideoLimit>>,
    request: Request,
) -> Result<Response, ApiError> {
    let gone = || {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "this stream link is unknown or has expired; ask to play again",
        )
    };
    if !token.starts_with(STREAM_PREFIX) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "not a stream link"));
    }
    let row: Option<(String, String, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT root, rel_path, token_id, file_id FROM {LINKS} WHERE token_hash = ? AND expires_at > ?"
    )))
    .bind(auth::token_hash(&token))
    .bind(unix_now())
    .fetch_optional(&pool)
    .await?;
    let Some((root, rel_path, token_id, file_id)) = row else {
        return Err(gone());
    };
    if !holder_sees(&pool, token_id, file_id).await? {
        return Err(gone());
    }
    if let Some(limit) = way_limit(limit) {
        within_limit(limit, stored_bitrate(&pool, file_id).await?)?;
    }
    let path = resolve(&root, &rel_path).await?;
    let response = ServeFile::new(path).try_call(request).await?;
    Ok(response.map(Body::new))
}

/// The file on disk. Refuses anything, such as a symlink, that resolves outside its library.
pub(in super::super) async fn resolve(root: &str, rel_path: &str) -> Result<PathBuf, ApiError> {
    let root = tokio::fs::canonicalize(root).await.map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "the library folder can't be reached right now",
        )
    })?;
    let not_on_disk = || ApiError::new(StatusCode::NOT_FOUND, "the file is not on disk right now");
    let path = tokio::fs::canonicalize(root.join(rel_path))
        .await
        .map_err(|_| not_on_disk())?;
    if !path.starts_with(&root) {
        tracing::warn!(
            rel_path,
            "refusing to stream a file that resolves outside its library"
        );
        return Err(not_on_disk());
    }
    Ok(path)
}
