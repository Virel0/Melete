//! A Home view for each user in one request: what they played lately, what they've had on
//! most, what's new in the library, and mixes made for them, from their own play history and
//! Discover. Nothing leaves the server.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::music::{ALBUM_COLUMNS, AlbumRow, Shown, tracks_by_id};
use crate::access::Access;
use crate::aoide::{discover, listening};
use crate::db::now_ms;

/// Albums or tracks in each row.
const ROW: i64 = 12;
/// Tracks in each mix.
const MIX: usize = 25;
/// "Lately", for what's been played most.
const LATELY_MS: i64 = 30 * 86_400_000;
/// A record finished this often, and not heard for this long, is one to rediscover.
const REDISCOVER_PLAYS: i64 = 3;
const REDISCOVER_AFTER_MS: i64 = 60 * 86_400_000;
/// At most this many from one album in a mix.
const PER_ALBUM: usize = 2;

#[derive(Clone)]
struct HomeState {
    pool: SqlitePool,
    /// Whether covers come from the Cover Art Archive (with MusicBrainz matching on).
    archive: bool,
}

/// The Home route, for everyone signed in. `archive`: whether covers come from the Cover Art Archive.
pub fn routes(pool: SqlitePool, archive: bool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/home", get(home))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(HomeState { pool, archive })
}

/// Present albums by id, in the order given.
async fn albums_in_order(
    state: &HomeState,
    shown: Shown,
    access: &Access,
    ids: &[i64],
) -> Result<Vec<Value>, ApiError> {
    let rows: Vec<AlbumRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ALBUM_COLUMNS} FROM music_albums a
         LEFT JOIN mb_releases r ON r.id = a.match_release_id
         WHERE a.missing_since IS NULL AND a.id IN (SELECT value FROM json_each(?)) AND {}",
        access.album_sql("a")
    )))
    .bind(serde_json::to_string(ids).unwrap_or_else(|_| "[]".into()))
    .fetch_all(&state.pool)
    .await?;
    let by_id: HashMap<i64, Value> = rows.iter().map(|r| (r.id, r.to_json(shown))).collect();
    Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
}

/// Present tracks by id, in the order given, at most [`PER_ALBUM`] from one album and
/// [`MIX`] in all.
async fn mix_tracks(
    pool: &SqlitePool,
    access: &Access,
    me: i64,
    ids: &[i64],
) -> Result<Vec<Value>, ApiError> {
    let tracks = tracks_by_id(pool, access, me, ids).await?;
    let mut per_album: HashMap<i64, usize> = HashMap::new();
    Ok(ids
        .iter()
        .filter_map(|id| tracks.get(id))
        .filter(|t| {
            let count = per_album
                .entry(t["album_id"].as_i64().unwrap_or(0))
                .or_default();
            *count += 1;
            *count <= PER_ALBUM
        })
        .take(MIX)
        .cloned()
        .collect())
}

async fn home(
    State(state): State<HomeState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let pool = &state.pool;
    let me = current.user.id;
    let now = now_ms();
    let shown = Shown::to(pool, state.archive, me).await?;
    // The play history, with what Aoide's devices synced read in (and Discover's input).
    let history = listening::play_events(pool, me).await?;

    let access = &current.access;
    let recently_played = recently_played(&state, shown, access, me).await?;
    let most_played = most_played(pool, access, me, now).await?;
    let recently_added = recently_added(&state, shown, access).await?;
    let mixes = mixes(pool, access, me, now, &history).await?;

    Ok(Json(json!({
        "recently_played": recently_played,
        "most_played": most_played,
        "recently_added": recently_added,
        "mixes": mixes,
    })))
}

/// The albums the user played last, latest first.
async fn recently_played(
    state: &HomeState,
    shown: Shown,
    access: &Access,
    me: i64,
) -> Result<Vec<Value>, ApiError> {
    let pool = &state.pool;
    let played: Vec<i64> = sqlx::query_scalar(
        "SELECT t.album_id FROM music_plays p
         JOIN music_tracks t ON t.id = p.track_id AND t.missing_since IS NULL
         WHERE p.user_id = ? AND t.album_id IS NOT NULL
         GROUP BY t.album_id ORDER BY max(p.started_at) DESC, t.album_id LIMIT ?",
    )
    .bind(me)
    .bind(ROW)
    .fetch_all(pool)
    .await?;
    albums_in_order(state, shown, access, &played).await
}

/// The tracks the user played most lately, each with how often.
async fn most_played(
    pool: &SqlitePool,
    access: &Access,
    me: i64,
    now: i64,
) -> Result<Vec<Value>, ApiError> {
    let most: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT p.track_id, count(*) AS plays FROM music_plays p
         JOIN music_tracks t ON t.id = p.track_id AND t.missing_since IS NULL
         WHERE p.user_id = ? AND p.started_at >= ? AND p.skipped = 0
         GROUP BY p.track_id ORDER BY plays DESC, max(p.started_at) DESC, p.track_id LIMIT ?",
    )
    .bind(me)
    .bind(now - LATELY_MS)
    .bind(ROW)
    .fetch_all(pool)
    .await?;
    let ids: Vec<i64> = most.iter().map(|(id, _)| *id).collect();
    let tracks = tracks_by_id(pool, access, me, &ids).await?;
    let most_played: Vec<Value> = most
        .iter()
        .filter_map(|(id, plays)| Some(json!({ "plays": plays, "track": tracks.get(id)? })))
        .collect();
    Ok(most_played)
}

/// The albums added to the library last, newest first.
async fn recently_added(
    state: &HomeState,
    shown: Shown,
    access: &Access,
) -> Result<Vec<Value>, ApiError> {
    let pool = &state.pool;
    let added: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM music_albums WHERE missing_since IS NULL ORDER BY id DESC LIMIT ?",
    )
    .bind(ROW)
    .fetch_all(pool)
    .await?;
    albums_in_order(state, shown, access, &added).await
}

/// The user's mixes that have tracks: Discover's week, then records loved and not heard for a
/// while.
async fn mixes(
    pool: &SqlitePool,
    access: &Access,
    me: i64,
    now: i64,
    history: &[crate::aoide::taste::PlayEvent],
) -> Result<Vec<Value>, ApiError> {
    // Mixes: Discover's week, then records loved and not heard for a while.
    let flags = listening::not_interested(pool, me).await?;
    let library = super::aoide_next::library(pool).await?;
    let discovered = discover::rank(&me.to_string(), &library, history, &flags, MIX * 2, now);
    let discover_ids: Vec<i64> = discovered
        .candidates
        .iter()
        .filter_map(|c| c.id.parse().ok())
        .collect();
    let loved: Vec<i64> = sqlx::query_scalar(
        "SELECT p.track_id FROM music_plays p
         JOIN music_tracks t ON t.id = p.track_id AND t.missing_since IS NULL
         WHERE p.user_id = ?
         GROUP BY p.track_id
         HAVING sum(p.completed) >= ? AND max(p.started_at) < ?
         ORDER BY sum(p.completed) DESC, max(p.started_at), p.track_id LIMIT ?",
    )
    .bind(me)
    .bind(REDISCOVER_PLAYS)
    .bind(now - REDISCOVER_AFTER_MS)
    .bind(i64::try_from(MIX * 4).unwrap_or(100))
    .fetch_all(pool)
    .await?;
    let flagged: HashSet<i64> = flags.iter().filter_map(|f| f.parse().ok()).collect();
    let loved: Vec<i64> = loved
        .into_iter()
        .filter(|id| !flagged.contains(id))
        .collect();
    let mut mixes = Vec::new();
    for (key, name, about, ids) in [
        (
            "discover",
            "Discover",
            "Records in the library you haven't heard through, picked by your taste; new each week.",
            discover_ids,
        ),
        (
            "rediscover",
            "Rediscover",
            "Records you played often and haven't heard for a while.",
            loved,
        ),
    ] {
        let tracks = mix_tracks(pool, access, me, &ids).await?;
        if !tracks.is_empty() {
            mixes.push(json!({ "key": key, "name": name, "about": about, "tracks": tracks }));
        }
    }
    Ok(mixes)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::plays::{self, Listen, Source};

    async fn user(pool: &SqlitePool, name: &str) -> (i64, String) {
        let user = auth::create_user(pool, name, "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let token = auth::issue_token(pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap()
            .1;
        (user.id, token)
    }

    async fn get(app: &Router, token: &str) -> Value {
        let request = Request::get("/api/v1/music/home")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn listen(pool: &SqlitePool, user: i64, id: &str, track: i64, at: i64, how: &str) {
        let listen = Listen {
            event_id: id.into(),
            track_key: track.to_string(),
            device: "Phone".into(),
            started_at: at,
            ended_at: None,
            position_ms: None,
            completed: how == "completed",
            skipped: how == "skipped",
            activity: None,
        };
        plays::record(pool, user, Source::Api, &listen)
            .await
            .unwrap();
    }

    fn ids(list: &Value, path: &str) -> Vec<i64> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|v| v.pointer(path).unwrap().as_i64().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn home_is_built_from_each_users_own_listening() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k1', 'One', 'Alder', 'a', 1), (2, 1, 'k2', 'Two', 'Birch', 'b', 1),
                        (3, 1, 'k3', 'Three', 'Cedar', 'c', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, artist, genres) VALUES
                 (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'A1', 'Alder', '["Rock"]'),
                 (2, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'A2', 'Alder', '["Rock"]'),
                 (3, 1, 'a/3', 1, 1, 1, 1, 1, 1, 'A3', 'Alder', '["Rock"]'),
                 (4, 1, 'b/1', 1, 1, 1, 1, 2, 1, 'B1', 'Birch', '["Rock"]'),
                 (5, 1, 'c/1', 1, 1, 1, 1, 3, 1, 'C1', 'Cedar', '["Rock"]');"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let (alice, alice_token) = user(&pool, "alice").await;
        let (_, bob_token) = user(&pool, "bob").await;
        let now = now_ms();
        let day = 86_400_000;
        // Lately: B1 twice, A1 once and once skipped; long ago, A2 finished three times.
        // Over a week ago, so Discover (which reads the week as it began) has seen them too.
        listen(&pool, alice, "1", 4, now - 10 * day, "completed").await;
        listen(&pool, alice, "2", 4, now - 9 * day, "completed").await;
        listen(&pool, alice, "3", 1, now - 8 * day, "completed").await;
        listen(&pool, alice, "4", 1, now - 8 * day + 1, "skipped").await;
        for (i, ago) in [100, 99, 98].into_iter().enumerate() {
            listen(
                &pool,
                alice,
                &format!("old{i}"),
                2,
                now - ago * day,
                "completed",
            )
            .await;
        }
        let app = routes(pool.clone(), false);

        let home = get(&app, &alice_token).await;
        assert_eq!(
            ids(&home["recently_played"], "/id"),
            [1, 2],
            "latest first, once each"
        );
        assert_eq!(
            ids(&home["most_played"], "/track/id"),
            [4, 1],
            "lately, skips not counted"
        );
        assert_eq!(ids(&home["most_played"], "/plays"), [2, 1]);
        assert_eq!(ids(&home["recently_added"], "/id"), [3, 2, 1]);
        let mixes: Vec<&str> = home["mixes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["key"].as_str().unwrap())
            .collect();
        assert_eq!(mixes, ["discover", "rediscover"]);
        let discover = ids(&home["mixes"][0]["tracks"], "/id");
        assert!(
            !discover.is_empty() && !discover.contains(&1) && !discover.contains(&4),
            "no record heard through: {discover:?}"
        );
        assert_eq!(ids(&home["mixes"][1]["tracks"], "/id"), [2]);

        // Bob has heard nothing: no history rows, the library's newest, and Discover alone.
        let theirs = get(&app, &bob_token).await;
        assert_eq!(theirs["recently_played"], json!([]));
        assert_eq!(theirs["most_played"], json!([]));
        assert_eq!(ids(&theirs["recently_added"], "/id"), [3, 2, 1]);
        assert_eq!(theirs["mixes"].as_array().unwrap().len(), 1);
    }
}
