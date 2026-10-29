//! Monitoring below the title: a series' seasons and episodes, and an artist's rule for new
//! albums.

use super::*;

/// Monitors a season of a wanted series or stops.
pub(super) async fn monitor_season(
    State(state): State<ImportState>,
    Path((id, season)): Path<(i64, i64)>,
    Json(body): Json<Monitored>,
) -> Result<StatusCode, ApiError> {
    if season < 0 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "season is 0 or more",
        ));
    }
    if wanted_edit::set_season_monitored(&state.pool, id, season, body.monitored).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such wanted series",
        ))
    }
}

/// Monitors an episode of a wanted series, or leaves it out on its own.
pub(super) async fn monitor_episode(
    State(state): State<ImportState>,
    Path((id, season, episode)): Path<(i64, i64, i64)>,
    Json(body): Json<Monitored>,
) -> Result<StatusCode, ApiError> {
    if season < 0 || episode < 1 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "season is 0 or more and episode 1 or more",
        ));
    }
    match wanted_edit::set_episode_monitored(&state.pool, id, season, episode, body.monitored)
        .await?
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(EpisodeNotSet::NotFound) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such wanted series",
        )),
        Err(EpisodeNotSet::SeasonOff) => Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("season {season} isn't monitored; monitor the season first"),
        )),
    }
}

#[derive(Deserialize)]
pub(super) struct ArtistChange {
    monitor_new: String,
}

/// Sets what happens to albums a wanted artist releases later.
pub(super) async fn change_artist(
    State(state): State<ImportState>,
    Path(id): Path<i64>,
    Json(body): Json<ArtistChange>,
) -> Result<StatusCode, ApiError> {
    if !matches!(body.monitor_new.as_str(), "all" | "new" | "none") {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "monitor_new is all, new or none",
        ));
    }
    if wanted_edit::set_monitor_new(&state.pool, id, &body.monitor_new).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such wanted artist",
        ))
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::header;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;

    async fn put(app: &Router, uri: &str, token: &str, body: Value) -> StatusCode {
        let request = Request::put(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        app.clone().oneshot(request).await.unwrap().status()
    }

    #[tokio::test]
    async fn admins_switch_seasons_episodes_and_the_rule_for_new_albums() {
        let pool = db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO wanted_series (id, tvdb_id, title, monitored, series_type, source, added_at)
                 VALUES (1, 10, 'A Show', 1, 'standard', 'sonarr', 0);
             INSERT INTO wanted_seasons (wanted_series_id, season, monitored)
                 VALUES (1, 1, 1), (1, 2, 0);
             INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source,
                     added_at)
                 VALUES (1, 'mb-1', 'Radiohead', 1, 'all', 'lidarr', 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut tokens = Vec::new();
        for (name, admin) in [("alice", true), ("bob", false)] {
            let user = auth::create_user(&pool, name, format!("{name}-password"), admin)
                .await
                .unwrap()
                .unwrap();
            let (_, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
                .await
                .unwrap();
            tokens.push(token);
        }
        let (admin, viewer) = (&tokens[0], &tokens[1]);
        let app = routes(pool.clone(), None);
        let on = json!({ "monitored": true });
        let off = json!({ "monitored": false });
        let series = "/api/v1/wanted/series";
        let cases = [
            (
                format!("{series}/1/seasons/2"),
                on.clone(),
                StatusCode::NO_CONTENT,
            ),
            (
                format!("{series}/1/seasons/1/episodes/3"),
                off.clone(),
                StatusCode::NO_CONTENT,
            ),
            (
                format!("{series}/1/seasons/4/episodes/1"),
                on.clone(),
                StatusCode::CONFLICT,
            ),
            (
                format!("{series}/9/seasons/1"),
                on.clone(),
                StatusCode::NOT_FOUND,
            ),
            (
                format!("{series}/9/seasons/1/episodes/1"),
                off.clone(),
                StatusCode::NOT_FOUND,
            ),
            (
                format!("{series}/1/seasons/-1"),
                on.clone(),
                StatusCode::BAD_REQUEST,
            ),
            (
                format!("{series}/1/seasons/1/episodes/0"),
                on.clone(),
                StatusCode::BAD_REQUEST,
            ),
            (
                "/api/v1/wanted/artists/1".into(),
                json!({ "monitor_new": "new" }),
                StatusCode::NO_CONTENT,
            ),
            (
                "/api/v1/wanted/artists/1".into(),
                json!({ "monitor_new": "studio" }),
                StatusCode::BAD_REQUEST,
            ),
            (
                "/api/v1/wanted/artists/9".into(),
                json!({ "monitor_new": "none" }),
                StatusCode::NOT_FOUND,
            ),
        ];
        for (uri, body, expected) in cases {
            assert_eq!(
                put(&app, &uri, admin, body.clone()).await,
                expected,
                "{uri} {body}"
            );
            if expected == StatusCode::NO_CONTENT {
                let refused = put(&app, &uri, viewer, body).await;
                assert_eq!(refused, StatusCode::FORBIDDEN, "{uri}");
            }
        }
        let seasons: Vec<(i64, bool, bool)> = sqlx::query_as(
            "SELECT season, monitored, monitored_here FROM wanted_seasons ORDER BY season",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(seasons, [(1, true, false), (2, true, true)]);
        let left_out: Vec<(i64, i64, bool)> =
            sqlx::query_as("SELECT season, episode, monitored FROM wanted_episode_overrides")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(left_out, [(1, 3, false)]);
        let rule: String = sqlx::query_scalar("SELECT monitor_new FROM wanted_artists")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rule, "new");
    }
}
