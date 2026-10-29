//! Matching albums to MusicBrainz by hand, for admins: the albums matching
//! couldn't settle with their candidates, a search of MusicBrainz, and matching one (or
//! recording that it has no release). Decisions by hand are final.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::music_match::{self, ByHand};
use crate::musicbrainz;

#[derive(Clone)]
struct MatchState {
    pool: SqlitePool,
    musicbrainz: Option<Arc<musicbrainz::Client>>,
}

/// The MusicBrainz matching routes, for admins only.
pub fn routes(pool: SqlitePool, musicbrainz: Option<Arc<musicbrainz::Client>>) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/unmatched", get(unmatched))
        .route("/api/v1/music/search", get(search))
        .route(
            "/api/v1/music/albums/{id}/match",
            put(match_album).delete(no_match),
        )
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(MatchState { pool, musicbrainz })
}

fn off() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "MusicBrainz matching is off; set MELETE_MUSICBRAINZ=on",
    )
}

#[derive(sqlx::FromRow)]
struct UnmatchedRow {
    id: i64,
    title: String,
    artist_name: String,
    library: String,
    folder: String,
    match_status: String,
    tracks: i64,
    match_candidates: Option<String>,
}

/// Albums matching left unmatched or ambiguous, not decided by hand.
async fn unmatched(State(state): State<MatchState>) -> Result<Json<Value>, ApiError> {
    let rows: Vec<UnmatchedRow> = sqlx::query_as(
        "SELECT a.id, a.title, a.artist_name, l.name AS library, a.folder, a.match_status,
             (SELECT count(*) FROM music_tracks t
                 WHERE t.album_id = a.id AND t.missing_since IS NULL) AS tracks,
             a.match_candidates
         FROM music_albums a JOIN libraries l ON l.id = a.library_id
         WHERE a.missing_since IS NULL AND a.match_status IN ('unmatched', 'ambiguous')
           AND a.match_method IS NOT 'manual'
         ORDER BY l.name, a.artist_name COLLATE NOCASE, a.title COLLATE NOCASE",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut albums = Vec::with_capacity(rows.len());
    for row in rows {
        let candidates: Value = match &row.match_candidates {
            Some(json) => serde_json::from_str(json)?,
            None => json!([]),
        };
        albums.push(json!({
            "id": row.id,
            "title": row.title,
            "artist": row.artist_name,
            "library": row.library,
            "folder": row.folder,
            "status": row.match_status,
            "tracks": row.tracks,
            "candidates": candidates,
        }));
    }
    Ok(Json(json!(albums)))
}

#[derive(Deserialize)]
struct SearchQuery {
    /// The album's title.
    q: Option<String>,
    artist: Option<String>,
}

async fn search(
    State(state): State<MatchState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Value>, ApiError> {
    let Some(mb) = &state.musicbrainz else {
        return Err(off());
    };
    let title = query.q.as_deref().map(str::trim).unwrap_or_default();
    if title.is_empty() || title.chars().count() > 200 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "q is the album's title, up to 200 characters",
        ));
    }
    let artist = query
        .artist
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty());
    let found = music_match::search_by_hand(mb, title, artist)
        .await
        .map_err(|err| ApiError::new(StatusCode::BAD_GATEWAY, err.to_string()))?;
    Ok(Json(json!(found)))
}

#[derive(Deserialize)]
struct MatchRequest {
    release_id: String,
}

async fn match_album(
    State(state): State<MatchState>,
    Path(id): Path<i64>,
    Json(body): Json<MatchRequest>,
) -> Result<Json<Value>, ApiError> {
    let Some(mb) = &state.musicbrainz else {
        return Err(off());
    };
    if !crate::library::tags::is_mbid(&body.release_id) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "release_id is a MusicBrainz release id",
        ));
    }
    let outcome = music_match::match_by_hand(&state.pool, mb, id, &body.release_id)
        .await
        .map_err(|err| ApiError::new(StatusCode::BAD_GATEWAY, format!("{err:#}")))?;
    match outcome {
        ByHand::Matched { release_group_id } => Ok(Json(json!({
            "id": id,
            "match_status": "matched",
            "match_method": "manual",
            "mb_release_id": body.release_id,
            "mb_release_group_id": release_group_id,
        }))),
        ByHand::NoSuchAlbum => Err(ApiError::new(StatusCode::NOT_FOUND, "no such album")),
        ByHand::NoSuchRelease => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "MusicBrainz has no such release",
        )),
    }
}

async fn no_match(
    State(state): State<MatchState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if music_match::no_match(&state.pool, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(StatusCode::NOT_FOUND, "no such album"))
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    const OK_COMPUTER: &str = "fba5f8fe-c6c8-4511-8562-c9febf482674";

    /// Album 1 is ambiguous (two Queen candidates), album 2 unmatched with 12 tracks, album 3
    /// matched; an admin and a viewer.
    async fn fixture(musicbrainz: bool) -> (Router, SqlitePool, String, String) {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
               INSERT INTO music_artists (id, key, name) VALUES (1, 'name:radiohead', 'Radiohead');
               INSERT INTO music_albums (id, library_id, key, title, artist_id, artist_name, folder,
                   seen_generation, match_status, match_candidates) VALUES
                   (1, 1, 'k1', 'Greatest Hits', NULL, 'Queen', 'q', 1, 'ambiguous',
                    '[{"release_id":"r1","release_group_id":"g1","title":"Greatest Hits"}]'),
                   (2, 1, 'k2', 'OK Computer', 1, 'Radiohead', 'r', 1, 'unmatched', NULL),
                   (3, 1, 'k3', 'Done', 1, 'Radiohead', 'd', 1, 'matched', NULL);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        for n in 1..=12 {
            sqlx::query(
                "INSERT INTO music_tracks (library_id, rel_path, size, modified_at,
                     first_seen_generation, seen_generation, album_id, tagged, title, track_number)
                 VALUES (1, ?, 1, 1, 1, 1, 2, 1, 't', ?)",
            )
            .bind(format!("r/{n}.flac"))
            .bind(n)
            .execute(&pool)
            .await
            .unwrap();
        }
        let mut tokens = Vec::new();
        for (name, admin) in [("admin", true), ("viewer", false)] {
            let user = crate::auth::create_user(&pool, name, "a-password".into(), admin)
                .await
                .unwrap()
                .unwrap();
            let (_, token) =
                crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                    .await
                    .unwrap();
            tokens.push(token);
        }
        let mb = if musicbrainz {
            Some(Arc::new(crate::musicbrainz::testing::start(0).await.0))
        } else {
            None
        };
        let viewer = tokens.pop().unwrap();
        (
            routes(pool.clone(), mb),
            pool,
            tokens.pop().unwrap(),
            viewer,
        )
    }

    async fn send(
        app: &Router,
        token: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        let body = match body {
            Some(body) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(body.to_string())
            }
            None => Body::empty(),
        };
        let response = app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn lists_searches_and_matches_by_hand() {
        let (app, pool, admin, viewer) = fixture(true).await;
        let (status, list) = send(&app, &admin, "GET", "/api/v1/music/unmatched", None).await;
        assert_eq!(status, StatusCode::OK);
        let spec: Value = serde_json::from_str(super::super::OPENAPI).unwrap();
        let fields = |schema: &str| -> Vec<String> {
            let mut keys: Vec<String> = spec["components"]["schemas"][schema]["properties"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect();
            keys.sort();
            keys
        };
        let keys = |v: &Value| -> Vec<String> {
            let mut keys: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };
        assert_eq!(keys(&list[0]), fields("MusicUnmatched"));
        let titles: Vec<&str> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["title"].as_str().unwrap())
            .collect();
        assert_eq!(
            titles,
            ["Greatest Hits", "OK Computer"],
            "by artist: Queen, then Radiohead"
        );
        assert_eq!(list[0]["candidates"][0]["release_id"], "r1");

        let (status, found) = send(
            &app,
            &admin,
            "GET",
            "/api/v1/music/search?q=OK%20Computer&artist=Radiohead",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(found.as_array().unwrap().len(), 1, "one per release group");
        assert_eq!(found[0]["tracks"], 12);
        assert_eq!(keys(&found[0]), fields("MusicCandidate"));

        let (status, matched) = send(
            &app,
            &admin,
            "PUT",
            "/api/v1/music/albums/2/match",
            Some(json!({ "release_id": OK_COMPUTER })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{matched}");
        assert_eq!(matched["match_method"], "manual");
        assert_eq!(keys(&matched), fields("MusicMatchResult"));
        assert_eq!(
            matched["mb_release_group_id"],
            "b1392450-e666-3926-a536-22c65f834433"
        );
        let linked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM music_tracks WHERE album_id = 2 AND match_recording_id IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(linked, 12);
        let artist: Option<String> =
            sqlx::query_scalar("SELECT mb_artist_id FROM music_artists WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(artist.is_some());

        // No release: recorded, and the album leaves the list.
        assert_eq!(
            send(&app, &admin, "DELETE", "/api/v1/music/albums/1/match", None)
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        let (_, list) = send(&app, &admin, "GET", "/api/v1/music/unmatched", None).await;
        assert_eq!(list, json!([]));
        // Matching never takes either up again.
        let untried: i64 =
            sqlx::query_scalar("SELECT count(*) FROM music_albums WHERE match_status IS NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(untried, 0);

        assert_eq!(
            send(&app, &viewer, "GET", "/api/v1/music/unmatched", None)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn refuses_bad_requests_and_says_when_musicbrainz_is_off() {
        let (app, _, admin, _) = fixture(true).await;
        for (method, uri, body) in [
            ("GET", "/api/v1/music/search", None),
            (
                "PUT",
                "/api/v1/music/albums/2/match",
                Some(json!({ "release_id": "nope" })),
            ),
        ] {
            assert_eq!(
                send(&app, &admin, method, uri, body).await.0,
                StatusCode::BAD_REQUEST,
                "{uri}"
            );
        }
        let other = "00000000-0000-0000-0000-000000000000";
        let (status, _) = send(
            &app,
            &admin,
            "PUT",
            "/api/v1/music/albums/2/match",
            Some(json!({ "release_id": other })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "MusicBrainz has no such release"
        );
        let (status, _) = send(
            &app,
            &admin,
            "PUT",
            "/api/v1/music/albums/99/match",
            Some(json!({ "release_id": OK_COMPUTER })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            send(
                &app,
                &admin,
                "DELETE",
                "/api/v1/music/albums/99/match",
                None
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );

        let (off, _, admin, _) = fixture(false).await;
        assert_eq!(
            send(&off, &admin, "GET", "/api/v1/music/search?q=x", None)
                .await
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        // Recording "no release" needs no MusicBrainz.
        assert_eq!(
            send(&off, &admin, "DELETE", "/api/v1/music/albums/2/match", None)
                .await
                .0,
            StatusCode::NO_CONTENT
        );
    }
}
