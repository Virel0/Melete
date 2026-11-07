//! Replay: a listener's own listening over a period, as the Aoide apps' Replay shows it: how
//! many plays and minutes, how many songs, artists and albums, the busiest day, and the top
//! songs, artists and albums. Worked out from the play history (every device's listens and the
//! web's), counting a listen as a play as the apps do: it reached the end, or it wasn't skipped
//! and ran at least half the track or four minutes, whichever is less. Minutes count every
//! listen, skips too. The tops hold only what the listener may see. It can be narrowed to an
//! album or an artist, and says how often listens with an outcome finished, as the apps' finish
//! rate counts them.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::music::{Shown, albums_by_id, artists_by_id, bad_request, tracks_by_id};

#[derive(Clone)]
struct ReplayState {
    pool: SqlitePool,
    /// Whether covers come from the Cover Art Archive.
    archive: bool,
}

/// The Replay route, for everyone signed in. `archive`: whether covers come from the Cover Art
/// Archive.
pub fn routes(pool: SqlitePool, archive: bool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/replay", get(replay))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(ReplayState { pool, archive })
}

/// The listens of `?1`'s history started from `?2` (milliseconds, none for no start) until
/// before `?3`, of album `?5` and of artist `?6` (credited anywhere on the track, or an artist
/// that joined it) when given, each with its track (none when the track isn't in the library)
/// and the track's first credited artist. `?4` is the query's own.
const LISTENS: &str = "FROM music_plays p
     LEFT JOIN music_tracks t ON t.id = p.track_id
     LEFT JOIN music_track_artists ta ON ta.track_id = p.track_id AND ta.position = 0
     WHERE p.user_id = ?1 AND (?2 IS NULL OR p.started_at >= ?2)
       AND (?3 IS NULL OR p.started_at < ?3)
       AND (?5 IS NULL OR t.album_id = ?5)
       AND (?6 IS NULL OR EXISTS (SELECT 1 FROM music_track_artists x
            WHERE x.track_id = p.track_id AND x.artist_id IN
                (SELECT id FROM music_artists WHERE id = ?6 OR merged_into = ?6)))";

/// Whether listen `p` of track `t` counts as a play, as the Aoide apps decide it: it reached
/// the end, or it wasn't skipped, ended, and ran at least half the track or four minutes,
/// whichever is less (four minutes when the length isn't known).
const IS_PLAY: &str = "(p.completed = 1 OR (p.skipped = 0 AND p.ended_at IS NOT NULL
     AND coalesce(p.position_ms, 0) >= CASE
         WHEN t.duration_seconds IS NULL OR t.duration_seconds <= 0 THEN 240000
         ELSE min(CAST(t.duration_seconds * 1000 AS INTEGER) / 2, 240000) END))";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayQuery {
    /// Unix seconds: listens started at or after this.
    from: Option<i64>,
    /// Unix seconds: listens started before this.
    to: Option<i64>,
    /// The listener's offset from UTC in minutes, for which day was busiest.
    offset_minutes: Option<i64>,
    /// How many of each top list, 1 to 50; 10 when left out.
    limit: Option<i64>,
    /// Only listens of this album.
    album_id: Option<i64>,
    /// Only listens of this artist's songs.
    artist_id: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct Totals {
    plays: i64,
    listened_ms: i64,
    songs: i64,
    artists: i64,
    albums: i64,
    /// Listens with an outcome: finished or skipped.
    decided: i64,
    finished: i64,
}

/// The window and sizes a query asks for: from and to in milliseconds, the offset from UTC in
/// minutes, and how many of each top list.
struct Asked {
    from: Option<i64>,
    to: Option<i64>,
    offset: i64,
    limit: i64,
    album: Option<i64>,
    artist: Option<i64>,
}

fn asked(query: &ReplayQuery) -> Result<Asked, ApiError> {
    let offset = query.offset_minutes.unwrap_or(0);
    if !(-840..=840).contains(&offset) {
        return Err(bad_request(
            "offset_minutes must be from -840 to 840 (minutes from UTC)",
        ));
    }
    let limit = query.limit.unwrap_or(10);
    if !(1..=50).contains(&limit) {
        return Err(bad_request("limit must be from 1 to 50"));
    }
    if let (Some(from), Some(to)) = (query.from, query.to)
        && to <= from
    {
        return Err(bad_request("to must be after from"));
    }
    if query.album_id.is_some() && query.artist_id.is_some() {
        return Err(bad_request("ask for an album or an artist, not both"));
    }
    let ms = |t: Option<i64>| t.map(|t| t.saturating_mul(1000));
    Ok(Asked {
        from: ms(query.from),
        to: ms(query.to),
        offset,
        limit,
        album: query.album_id,
        artist: query.artist_id,
    })
}

async fn replay(
    State(state): State<ReplayState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ReplayQuery>,
) -> Result<Json<Value>, ApiError> {
    let asked = asked(&query)?;
    let (pool, me) = (&state.pool, current.user.id);
    let totals = totals(pool, me, &asked).await?;
    let busiest = busiest_day(pool, me, &asked).await?;
    let (songs, artists, albums) = (
        top(pool, me, &asked, "p.track_id").await?,
        top(pool, me, &asked, "ta.artist_id").await?,
        top(pool, me, &asked, "t.album_id").await?,
    );
    let ids = |rows: &[(i64, i64)]| rows.iter().map(|(id, _)| *id).collect::<Vec<i64>>();
    let tracks = tracks_by_id(pool, &current.access, me, &ids(&songs)).await?;
    let found_artists = artists_by_id(pool, &current.access, &ids(&artists)).await?;
    let shown = Shown {
        archive: state.archive,
    };
    let found_albums = albums_by_id(pool, &current.access, shown, &ids(&albums)).await?;
    let ranked = |rows: &[(i64, i64)], found: &HashMap<i64, Value>, key: &str| -> Vec<Value> {
        rows.iter()
            .filter_map(|(id, plays)| Some(json!({ "plays": plays, key: found.get(id)? })))
            .collect()
    };
    Ok(Json(json!({
        "plays": totals.plays,
        "minutes": totals.listened_ms / 60_000,
        "songs": totals.songs,
        "artists": totals.artists,
        "albums": totals.albums,
        "busiest_day": busiest.map(|(date, plays)| json!({ "date": date, "plays": plays })),
        "finish": { "decided": totals.decided, "finished": totals.finished },
        "top_songs": ranked(&songs, &tracks, "track"),
        "top_artists": ranked(&artists, &found_artists, "artist"),
        "top_albums": ranked(&albums, &found_albums, "album"),
    })))
}

async fn totals(pool: &SqlitePool, me: i64, asked: &Asked) -> sqlx::Result<Totals> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT coalesce(sum({IS_PLAY}), 0) AS plays,
             coalesce(sum(coalesce(p.position_ms, 0)), 0) AS listened_ms,
             count(DISTINCT CASE WHEN {IS_PLAY} THEN p.track_key END) AS songs,
             count(DISTINCT CASE WHEN {IS_PLAY} THEN ta.artist_id END) AS artists,
             count(DISTINCT CASE WHEN {IS_PLAY} THEN t.album_id END) AS albums,
             coalesce(sum(p.ended_at IS NOT NULL AND (p.completed = 1 OR p.skipped = 1)), 0)
                 AS decided,
             coalesce(sum(p.ended_at IS NOT NULL AND p.completed = 1), 0) AS finished
         {LISTENS}"
    )))
    .bind(me)
    .bind(asked.from)
    .bind(asked.to)
    .bind(None::<i64>)
    .bind(asked.album)
    .bind(asked.artist)
    .fetch_one(pool)
    .await
}

/// The day, on the listener's clock, with the most plays (the latest of a tie), and how many.
async fn busiest_day(
    pool: &SqlitePool,
    me: i64,
    asked: &Asked,
) -> sqlx::Result<Option<(String, i64)>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT strftime('%Y-%m-%d', p.started_at / 1000 + ?4 * 60, 'unixepoch') AS day,
             count(*) AS plays
         {LISTENS} AND {IS_PLAY}
         GROUP BY day ORDER BY plays DESC, day DESC LIMIT 1"
    )))
    .bind(me)
    .bind(asked.from)
    .bind(asked.to)
    .bind(asked.offset)
    .bind(asked.album)
    .bind(asked.artist)
    .fetch_optional(pool)
    .await
}

/// The ids `key` (a track, artist or album column) with the most plays, most first (the latest
/// played of a tie), each with how many.
async fn top(
    pool: &SqlitePool,
    me: i64,
    asked: &Asked,
    key: &'static str,
) -> sqlx::Result<Vec<(i64, i64)>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {key} AS id, count(*) AS plays
         {LISTENS} AND {IS_PLAY} AND {key} IS NOT NULL
         GROUP BY {key} ORDER BY plays DESC, max(p.started_at) DESC, {key} LIMIT ?4"
    )))
    .bind(me)
    .bind(asked.from)
    .bind(asked.to)
    .bind(asked.limit)
    .bind(asked.album)
    .bind(asked.artist)
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};

    /// 2026-11-05 23:30 UTC, in milliseconds.
    const LATE: i64 = 1_793_921_400_000;
    /// 2026-11-06 10:00 UTC.
    const MORNING: i64 = 1_793_959_200_000;

    /// Three tracks by Alice (200 s, 600 s and no known length) on two albums, and Bob's track.
    async fn library(pool: &SqlitePool) {
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_artists (id, key, name) VALUES (1, 'name:alice', 'Alice'),
                 (2, 'name:bob', 'Bob');
             INSERT INTO music_albums (id, library_id, key, title, artist_id, artist_name, folder,
                 seen_generation)
                 VALUES (1, 1, 'k1', 'First Album', 1, 'Alice', 'a', 1),
                        (2, 1, 'k2', 'Second Album', 1, 'Alice', 'b', 1),
                        (3, 1, 'k3', 'Bobs Album', 2, 'Bob', 'c', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, artist, genres,
                 duration_seconds)
                 VALUES (7, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Short', 'Alice', '[]', 200),
                        (8, 1, 'b/1', 1, 1, 1, 1, 2, 1, 'Long', 'Alice', '[]', 600),
                        (9, 1, 'b/2', 1, 1, 1, 1, 2, 1, 'Unknown length', 'Alice', '[]', NULL),
                        (10, 1, 'c/1', 1, 1, 1, 1, 3, 1, 'Bobs Song', 'Bob', '[]', 200);
             INSERT INTO music_track_artists (track_id, position, artist_id, name)
                 VALUES (7, 0, 1, 'Alice'), (8, 0, 1, 'Alice'), (9, 0, 1, 'Alice'),
                        (10, 0, 2, 'Bob');"#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    async fn listen(
        pool: &SqlitePool,
        user: i64,
        (track, started, position_s): (i64, i64, i64),
        (ended, completed, skipped): (bool, bool, bool),
    ) {
        sqlx::query(
            "INSERT INTO music_plays (user_id, source, event_id, track_key, track_id, device,
                 started_at, ended_at, position_ms, completed, skipped)
             VALUES (?, 'api', ?, ?, ?, 'Laptop', ?, ?, ?, ?, ?)",
        )
        .bind(user)
        .bind(format!("e{started}-{track}-{position_s}"))
        .bind(track.to_string())
        .bind(track)
        .bind(started)
        .bind(ended.then_some(started + position_s * 1000))
        .bind(position_s * 1000)
        .bind(completed)
        .bind(skipped)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn get(app: &Router, token: &str, uri: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .uri(uri)
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
    async fn counts_plays_as_the_apps_do_and_ranks_what_was_played() {
        let pool = crate::db::open_in_memory().await;
        library(&pool).await;
        let alice = crate::auth::testing::user(&pool, "alice", false).await.id;
        let bob = crate::auth::testing::user(&pool, "bob", false).await.id;
        let token = auth::issue_token(&pool, alice, TokenKind::Session, "Laptop")
            .await
            .unwrap()
            .1;
        let ended = (true, false, false);
        // Plays: to the end; past half of 200 s; past four minutes of 600 s; past four minutes
        // of an unknown length.
        listen(&pool, alice, (7, LATE, 200), (true, true, false)).await;
        listen(&pool, alice, (7, LATE + 1, 120), ended).await;
        listen(&pool, alice, (8, MORNING, 250), ended).await;
        listen(&pool, alice, (9, MORNING + 1, 241), ended).await;
        // Not plays: under half; skipped, however long; never ended.
        listen(&pool, alice, (7, MORNING + 2, 50), ended).await;
        listen(&pool, alice, (8, MORNING + 3, 300), (true, false, true)).await;
        listen(&pool, alice, (7, MORNING + 4, 150), (false, false, false)).await;
        // Someone else's, and one before the window.
        listen(&pool, bob, (10, MORNING, 200), (true, true, false)).await;
        listen(
            &pool,
            alice,
            (10, LATE - 86_400_000 * 30, 200),
            (true, true, false),
        )
        .await;
        let app = routes(pool.clone(), false);

        let from = (LATE - 86_400_000) / 1000;
        let (status, replay) =
            get(&app, &token, &format!("/api/v1/music/replay?from={from}")).await;
        assert_eq!(status, StatusCode::OK, "{replay}");
        assert_eq!(
            (
                &replay["plays"],
                &replay["minutes"],
                &replay["songs"],
                &replay["artists"],
                &replay["albums"]
            ),
            (&json!(4), &json!(21), &json!(3), &json!(1), &json!(2)),
            "minutes count every listen in the window: 1311 s"
        );
        let titles = |list: &Value, key: &str, field: &str| -> Vec<(String, i64)> {
            list.as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    (
                        e[key][field].as_str().unwrap().to_owned(),
                        e["plays"].as_i64().unwrap(),
                    )
                })
                .collect()
        };
        assert_eq!(
            titles(&replay["top_songs"], "track", "title"),
            [
                ("Short".to_owned(), 2),
                ("Unknown length".to_owned(), 1),
                ("Long".to_owned(), 1)
            ]
        );
        assert_eq!(
            titles(&replay["top_albums"], "album", "title"),
            [
                ("Second Album".to_owned(), 2),
                ("First Album".to_owned(), 2)
            ]
        );
        assert_eq!(
            titles(&replay["top_artists"], "artist", "name"),
            [("Alice".to_owned(), 4)]
        );
        // Two plays late on the 5th and two on the morning of the 6th: a tie in UTC, the 6th
        // as the latest; an hour east of UTC, all four fall on the 6th.
        assert_eq!(
            replay["busiest_day"],
            json!({ "date": "2026-11-06", "plays": 2 })
        );
        let (_, east) = get(
            &app,
            &token,
            &format!("/api/v1/music/replay?from={from}&offset_minutes=60"),
        )
        .await;
        assert_eq!(
            east["busiest_day"],
            json!({ "date": "2026-11-06", "plays": 4 })
        );

        // All time takes in the older play too.
        let (_, all) = get(&app, &token, "/api/v1/music/replay").await;
        assert_eq!(all["plays"], json!(5));
    }

    #[tokio::test]
    async fn narrows_to_an_album_or_an_artist_and_counts_what_finished() {
        let pool = crate::db::open_in_memory().await;
        library(&pool).await;
        let alice = crate::auth::testing::user(&pool, "alice", false).await.id;
        let token = auth::issue_token(&pool, alice, TokenKind::Session, "Laptop")
            .await
            .unwrap()
            .1;
        let ended = (true, false, false);
        // The first album (track 7): finished twice, skipped once, stopped part way once, and
        // once never ended. The second album (track 8): one play.
        listen(&pool, alice, (7, LATE, 200), (true, true, false)).await;
        listen(&pool, alice, (7, LATE + 1, 200), (true, true, false)).await;
        listen(&pool, alice, (7, LATE + 2, 10), (true, false, true)).await;
        listen(&pool, alice, (7, LATE + 3, 60), ended).await;
        listen(&pool, alice, (7, LATE + 4, 150), (false, false, false)).await;
        listen(&pool, alice, (8, MORNING, 300), ended).await;
        listen(&pool, alice, (10, MORNING + 1, 200), (true, true, false)).await;
        let app = routes(pool.clone(), false);

        let (_, album) = get(&app, &token, "/api/v1/music/replay?album_id=1").await;
        assert_eq!((&album["plays"], &album["songs"]), (&json!(2), &json!(1)));
        assert_eq!(
            album["finish"],
            json!({ "decided": 3, "finished": 2 }),
            "finished or skipped count; stopped part way or never ended don't"
        );
        let (_, alice_songs) = get(&app, &token, "/api/v1/music/replay?artist_id=1&limit=5").await;
        let titles: Vec<&str> = alice_songs["top_songs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["track"]["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, ["Short", "Long"], "Bob's song isn't hers");
        let (_, bob) = get(&app, &token, "/api/v1/music/replay?artist_id=2").await;
        assert_eq!(bob["plays"], json!(1));
        let (status, _) = get(&app, &token, "/api/v1/music/replay?artist_id=1&album_id=1").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn refuses_a_window_or_size_that_cant_be() {
        let pool = crate::db::open_in_memory().await;
        let alice = crate::auth::testing::user(&pool, "alice", false).await.id;
        let token = auth::issue_token(&pool, alice, TokenKind::Session, "Laptop")
            .await
            .unwrap()
            .1;
        let app = routes(pool.clone(), false);
        for uri in [
            "/api/v1/music/replay?limit=0",
            "/api/v1/music/replay?limit=51",
            "/api/v1/music/replay?offset_minutes=900",
            "/api/v1/music/replay?from=100&to=100",
        ] {
            assert_eq!(
                get(&app, &token, uri).await.0,
                StatusCode::BAD_REQUEST,
                "{uri}"
            );
        }
        let (status, empty) = get(&app, &token, "/api/v1/music/replay").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (&empty["plays"], &empty["busiest_day"], &empty["top_songs"]),
            (&json!(0), &Value::Null, &json!([]))
        );
    }
}
