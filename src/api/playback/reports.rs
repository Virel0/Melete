//! Where the user is in a file, and marking it watched, by report or by hand.

use super::*;

#[derive(Deserialize)]
pub(super) struct ProgressReport {
    pub(super) position_seconds: f64,
    pub(super) duration_seconds: Option<f64>,
    /// The viewer chose to start over, so a position near the start replaces a later one.
    #[serde(default)]
    pub(super) restart: bool,
}

pub(super) fn valid_seconds(value: f64) -> bool {
    value.is_finite() && (0.0..=MAX_SECONDS).contains(&value)
}

/// Records where the user is. Reaching 90% of the duration marks the file watched and resets
/// the position, so the next play starts over. A report near the start when the saved position
/// is well in is ignored unless it's a restart, and the saved progress is the answer.
pub(super) async fn report_progress(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(file_id): Path<i64>,
    Json(report): Json<ProgressReport>,
) -> Result<Json<Value>, ApiError> {
    let duration_ok = report
        .duration_seconds
        .is_none_or(|d| valid_seconds(d) && d > 0.0);
    if !valid_seconds(report.position_seconds) || !duration_ok {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "positions and durations are 0 to 604800 seconds, durations above 0",
        ));
    }
    require_file(&pool, &current.access, file_id).await?;
    let now = unix_now();
    let stored = load_progress(&pool, current.user.id, file_id)
        .await?
        .unwrap_or_default();
    // Only a probe of this exact version of the file.
    let probed: Option<f64> = sqlx::query_scalar(
        "SELECT json_extract(p.info, '$.duration_seconds')
         FROM media_probes p JOIN media_files f ON f.id = p.file_id
         WHERE p.file_id = ? AND p.size = f.size AND p.modified_at = f.modified_at",
    )
    .bind(file_id)
    .fetch_optional(&pool)
    .await?
    .flatten();
    let duration = report
        .duration_seconds
        .or(stored.duration_seconds)
        .or(probed.filter(|d| *d > 0.0));
    let finished = duration.is_some_and(|d| report.position_seconds >= d * WATCHED_FRACTION);
    if !finished
        && !report.restart
        && report.position_seconds < NEAR_START
        && stored.position_seconds >= FAR_IN
    {
        return Ok(Json(progress_json(&stored)));
    }

    // Single statements that touch only what this report changes, so a "mark unwatched" from
    // another device in between isn't written back over.
    let progress: Progress = if finished {
        let (first_count, increment, without_link) =
            match count_viewing(&pool, current.token_id, file_id, now).await? {
                Viewing::Counted => (1, 1, false),
                Viewing::AlreadyCounted => (0, 0, false),
                // Played without asking for a stream link (a downloaded copy, say): count it
                // if it wasn't watched yet.
                Viewing::NoLink => (1, 0, true),
            };
        sqlx::query_as(
            "INSERT INTO playback_progress (user_id, file_id, position_seconds, duration_seconds,
                 watched, play_count, updated_at)
             VALUES (?1, ?2, 0, ?3, 1, ?4, ?5)
             ON CONFLICT (user_id, file_id) DO UPDATE SET
                 position_seconds = 0,
                 duration_seconds = COALESCE(excluded.duration_seconds, duration_seconds),
                 play_count = play_count
                     + CASE WHEN ?7 THEN 1 - watched ELSE ?6 END,
                 watched = 1,
                 updated_at = excluded.updated_at
             RETURNING position_seconds, duration_seconds, watched, play_count",
        )
        .bind(current.user.id)
        .bind(file_id)
        .bind(report.duration_seconds)
        .bind(first_count)
        .bind(now)
        .bind(increment)
        .bind(without_link)
        .fetch_one(&pool)
        .await?
    } else {
        sqlx::query_as(
            "INSERT INTO playback_progress (user_id, file_id, position_seconds, duration_seconds,
                 updated_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (user_id, file_id) DO UPDATE SET
                 position_seconds = excluded.position_seconds,
                 duration_seconds = COALESCE(excluded.duration_seconds, duration_seconds),
                 updated_at = excluded.updated_at
             RETURNING position_seconds, duration_seconds, watched, play_count",
        )
        .bind(current.user.id)
        .bind(file_id)
        .bind(report.position_seconds)
        .bind(report.duration_seconds)
        .bind(now)
        .fetch_one(&pool)
        .await?
    };
    Ok(Json(progress_json(&progress)))
}

pub(super) enum Viewing {
    /// This play is the first to reach the end through its stream link.
    Counted,
    /// The link's play was already counted (the player rewound past 90% and came back).
    AlreadyCounted,
    /// No live stream link for this file from this session.
    NoLink,
}

pub(super) async fn count_viewing(
    pool: &SqlitePool,
    token_id: i64,
    file_id: i64,
    now: i64,
) -> Result<Viewing, ApiError> {
    let marked = sqlx::query(
        "UPDATE play_sessions SET counted = 1 WHERE id = (
             SELECT id FROM play_sessions
             WHERE token_id = ? AND file_id = ? AND expires_at > ? AND counted = 0
             ORDER BY id DESC LIMIT 1)",
    )
    .bind(token_id)
    .bind(file_id)
    .bind(now)
    .execute(pool)
    .await?
    .rows_affected();
    if marked == 1 {
        return Ok(Viewing::Counted);
    }
    let has_link: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM play_sessions
             WHERE token_id = ? AND file_id = ? AND expires_at > ?)",
    )
    .bind(token_id)
    .bind(file_id)
    .bind(now)
    .fetch_one(pool)
    .await?;
    Ok(if has_link {
        Viewing::AlreadyCounted
    } else {
        Viewing::NoLink
    })
}

#[derive(Deserialize)]
pub(super) struct WatchedRequest {
    pub(super) watched: bool,
}

/// Marks a file watched or unwatched by hand. Either way the next play starts over.
pub(super) async fn set_watched(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(file_id): Path<i64>,
    Json(body): Json<WatchedRequest>,
) -> Result<Json<Value>, ApiError> {
    require_file(&pool, &current.access, file_id).await?;
    let progress: Progress = sqlx::query_as(
        "INSERT INTO playback_progress (user_id, file_id, position_seconds, watched, play_count,
             updated_at)
         VALUES (?1, ?2, 0, ?3, ?3, ?4)
         ON CONFLICT (user_id, file_id) DO UPDATE SET
             position_seconds = 0,
             play_count = play_count + (excluded.watched AND NOT watched),
             watched = excluded.watched,
             updated_at = excluded.updated_at
         RETURNING position_seconds, duration_seconds, watched, play_count",
    )
    .bind(current.user.id)
    .bind(file_id)
    .bind(body.watched)
    .bind(unix_now())
    .fetch_one(&pool)
    .await?;
    Ok(Json(progress_json(&progress)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SeriesWatchedRequest {
    pub(super) watched: bool,
    /// Only this season (0 for specials); the whole series, specials too, without it. Unknown
    /// fields are refused, so a misspelt `season` can't mark the whole series.
    pub(super) season: Option<i64>,
}

/// Marks every episode file of a series, or of one of its seasons, watched or unwatched by
/// hand, as marking each file would, in one statement: files the user may see that are present
/// and named as episodes. Files already as asked (and at the start) are left alone, so `changed`
/// says how many it changed.
pub(super) async fn set_series_watched(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(series_id): Path<i64>,
    Json(body): Json<SeriesWatchedRequest>,
) -> Result<Json<Value>, ApiError> {
    if body.season.is_some_and(|s| s < 0) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "season is 0 (specials) or more",
        ));
    }
    let found: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT x.id FROM series x WHERE x.id = ? AND x.missing_since IS NULL AND {}",
        current.access.series_sql("x")
    )))
    .bind(series_id)
    .fetch_optional(&pool)
    .await?;
    if found.is_none() {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such series"));
    }
    // The SELECT's WHERE keeps SQLite from reading ON CONFLICT as part of the join.
    let changed = sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO playback_progress (user_id, file_id, position_seconds, watched, play_count,
             updated_at)
         SELECT ?1, f.id, 0, ?2, ?2, ?3 FROM media_files f
         LEFT JOIN playback_progress p ON p.user_id = ?1 AND p.file_id = f.id
         WHERE f.series_id = ?4 AND f.season IS NOT NULL AND (?5 IS NULL OR f.season = ?5)
             AND f.missing_since IS NULL AND {}
             AND NOT (coalesce(p.watched, 0) = ?2 AND coalesce(p.position_seconds, 0) = 0)
         ON CONFLICT (user_id, file_id) DO UPDATE SET
             position_seconds = 0,
             play_count = play_count + (excluded.watched AND NOT watched),
             watched = excluded.watched,
             updated_at = excluded.updated_at",
        current.access.file_sql("f")
    )))
    .bind(current.user.id)
    .bind(body.watched)
    .bind(unix_now())
    .bind(series_id)
    .bind(body.season)
    .execute(&pool)
    .await?
    .rows_affected();
    Ok(Json(json!({ "changed": changed })))
}
