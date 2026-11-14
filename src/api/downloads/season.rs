//! Taking a season's search the way a sync would: its best accepted pack, else the single
//! episodes it found for the season's missing episodes (`rss::grab_for`). Search on a title's
//! page grabs the best accepted result itself; when no pack is accepted, this takes the rest.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Serialize;

use super::{ApiError, DownloadState};
use crate::rss::Grabbed;
use crate::search::Target;

/// What was grabbed, and a word on what was taken one by one.
#[derive(Serialize)]
pub(super) struct Taken {
    grabbed: Vec<Grabbed>,
    said: Option<String>,
}

/// Grabs from a season's search what a sync would: its best accepted pack, else its single
/// episodes for the season's missing episodes.
pub(super) async fn take_season(
    State(state): State<DownloadState>,
    Path(id): Path<i64>,
) -> Result<Json<Taken>, ApiError> {
    let found: Option<(Option<i64>, Option<i64>, Option<i64>)> =
        sqlx::query_as("SELECT wanted_series_id, season, episode FROM searches WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let Some(found) = found else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such search"));
    };
    let (Some(series), Some(season), None) = found else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "only a whole season's search is taken this way",
        ));
    };
    let Some(pipeline) = state.pipeline.as_deref() else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "downloads are off; MELETE_DOWNLOADS=on with Deluge's URL and password turns them on",
        ));
    };
    let target = Target::Series {
        id: series,
        season,
        episode: None,
    };
    let mut grabbed = Vec::new();
    let said = crate::rss::grab_for(&state.pool, pipeline, target, id, &mut grabbed).await?;
    Ok(Json(Taken {
        grabbed,
        said: said.took,
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::header;
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::config::Secret;
    use crate::deluge;
    use crate::downloads::{Fetcher, Pipeline};
    use crate::rss::tests::{indexer, releases, setup, want_frieren};

    async fn take(app: &axum::Router, token: &str, search: i64) -> (StatusCode, Value) {
        let request = Request::post(format!("/api/v1/searches/{search}/take"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn the_title_pages_search_takes_a_season_found_only_as_single_episodes() {
        let pool = setup().await;
        let frieren = want_frieren(&pool, None).await;
        sqlx::raw_sql(
            "INSERT INTO tmdb_series (tmdb_id, tvdb_id, name, fetched_at) VALUES (209867, 424536, 'Frieren', 0);
             INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, air_date) VALUES
                 (209867, 1, 1, 1, '2023-09-29'), (209867, 1, 2, 2, '2023-09-29');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let feed = indexer(&pool, "[5000]").await;
        feed.lock().unwrap().0 = releases(&[
            (
                "Frieren.Beyond.Journeys.End.S01E01.1080p.WEB-DL.x264-GRP",
                "",
            ),
            (
                "Frieren.Beyond.Journeys.End.S01E02.1080p.WEB-DL.x264-GRP",
                "",
            ),
        ]);
        // The title page's Search: the season searched for, nothing accepted.
        let client = crate::torznab::Client::new().unwrap();
        let season = Target::Series {
            id: frieren,
            season: 1,
            episode: None,
        };
        let search = crate::search::search(&pool, &client, None, season)
            .await
            .unwrap()
            .unwrap();
        let accepted: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM release_decisions WHERE search_id = ? AND accepted = 1",
        )
        .bind(search)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(accepted, 0);

        let user = crate::auth::testing::user(&pool, "admin", true).await;
        let token = crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
            .await
            .unwrap()
            .1;
        // Downloads off: refused.
        let off = super::super::routes(pool.clone(), None, None);
        assert_eq!(take(&off, &token, search).await.0, StatusCode::CONFLICT);

        let (url, deluge_seen) = deluge::testing::start().await;
        let pipeline = Arc::new(Pipeline {
            deluge: deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
            fetcher: Fetcher::new().unwrap(),
            seedbox: None,
        });
        let app = super::super::routes(pool.clone(), Some(pipeline), None);
        let (status, taken) = take(&app, &token, search).await;
        assert_eq!(status, StatusCode::OK, "{taken}");
        let started = taken["grabbed"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|g| g["download_id"].is_i64())
            .count();
        assert_eq!(started, 2, "{taken}");
        assert!(
            taken["said"].as_str().unwrap().contains("one by one"),
            "{taken}"
        );
        assert_eq!(deluge_seen.lock().unwrap().torrents.len(), 2);
        // Taken again: both on their way, nothing more.
        let (_, again) = take(&app, &token, search).await;
        assert_eq!(again["grabbed"], serde_json::json!([]), "{again}");
        assert_eq!(deluge_seen.lock().unwrap().torrents.len(), 2);

        assert_eq!(take(&app, &token, 999).await.0, StatusCode::NOT_FOUND);
    }
}
