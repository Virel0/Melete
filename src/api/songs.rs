//! Wanted songs (admins only): a single track to have, fetched through Soulseek.
//! Adding one says where it goes and how its results are decided: by default the wanted
//! artist's library and profile when the artist is on the wanted list, else the only music
//! library and the preset that prefers FLAC.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router, middleware};
use serde::Deserialize;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::db::unix_now;
use crate::library::tags::is_mbid;
use crate::music_profiles;
use crate::song_search::{WANTED_SONG, WantedSong};

/// The wanted song routes, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/wanted/songs", get(list).post(add))
        .route("/api/v1/wanted/songs/{id}", delete(remove))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(pool)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<WantedSong>>, ApiError> {
    let songs: Vec<WantedSong> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{WANTED_SONG} ORDER BY artist COLLATE NOCASE, title COLLATE NOCASE, id"
    )))
    .fetch_all(&pool)
    .await?;
    Ok(Json(songs))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewSong {
    title: String,
    artist: String,
    album: Option<String>,
    length_seconds: Option<i64>,
    mb_recording_id: Option<String>,
    mb_artist_id: Option<String>,
    library_id: Option<i64>,
    music_profile_id: Option<i64>,
}

fn bad(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, message)
}

/// Trimmed text, or why it won't do.
fn text(value: &str, field: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 300 {
        return Err(bad(format!("{field} must be 1 to 300 characters")));
    }
    Ok(value.to_owned())
}

async fn add(
    State(pool): State<SqlitePool>,
    Json(new): Json<NewSong>,
) -> Result<(StatusCode, Json<WantedSong>), ApiError> {
    let (title, artist, album) = checked(&new)?;
    // The wanted artist of that name, when there is one.
    let wanted_artist: Option<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, library_id FROM wanted_artists WHERE name = ? COLLATE NOCASE ORDER BY id LIMIT 1",
    )
    .bind(&artist)
    .fetch_optional(&pool)
    .await?;

    let library_id = library_for(&pool, new.library_id, wanted_artist).await?;
    let music_profile_id = profile_for(&pool, new.music_profile_id, wanted_artist).await?;

    refuse_if_wanted(&pool, &new.mb_recording_id, &title, &artist, &album).await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO wanted_songs (title, artist, album, length_seconds, mb_recording_id,
             mb_artist_id, library_id, music_profile_id, source, added_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'manual', ?) RETURNING id",
    )
    .bind(&title)
    .bind(&artist)
    .bind(&album)
    .bind(new.length_seconds)
    .bind(&new.mb_recording_id)
    .bind(&new.mb_artist_id)
    .bind(library_id)
    .bind(music_profile_id)
    .bind(unix_now())
    .fetch_one(&pool)
    .await?;
    let song: WantedSong =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{WANTED_SONG} WHERE id = ?")))
            .bind(id)
            .fetch_one(&pool)
            .await?;
    Ok((StatusCode::CREATED, Json(song)))
}

/// The song's title, artist and album trimmed, once every field is checked.
fn checked(new: &NewSong) -> Result<(String, String, Option<String>), ApiError> {
    let title = text(&new.title, "title")?;
    let artist = text(&new.artist, "artist")?;
    let album = new.album.as_deref().map(|a| text(a, "album")).transpose()?;
    if new
        .length_seconds
        .is_some_and(|l| !(1..=36_000).contains(&l))
    {
        return Err(bad("length_seconds must be 1 to 36000"));
    }
    for (id, field) in [
        (&new.mb_recording_id, "mb_recording_id"),
        (&new.mb_artist_id, "mb_artist_id"),
    ] {
        if id.as_deref().is_some_and(|id| !is_mbid(id)) {
            return Err(bad(format!("{field} isn't a MusicBrainz id")));
        }
    }
    Ok((title, artist, album))
}

/// The music library the song goes to: the one asked for, else the wanted artist's, else the
/// only one there is.
async fn library_for(
    pool: &SqlitePool,
    asked: Option<i64>,
    wanted_artist: Option<(i64, Option<i64>)>,
) -> Result<i64, ApiError> {
    Ok(match asked {
        Some(id) => {
            let music: Option<String> =
                sqlx::query_scalar("SELECT kind FROM libraries WHERE id = ?")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
            match music.as_deref() {
                Some("music") => id,
                Some(_) => return Err(bad("library_id isn't a music library")),
                None => return Err(bad("there's no such library")),
            }
        }
        None => match wanted_artist.and_then(|(_, library)| library) {
            Some(id) => id,
            None => {
                let music: Vec<i64> =
                    sqlx::query_scalar("SELECT id FROM libraries WHERE kind = 'music' ORDER BY id")
                        .fetch_all(pool)
                        .await?;
                match music.as_slice() {
                    [only] => *only,
                    [] => return Err(bad("there's no music library to put it in")),
                    _ => {
                        return Err(bad(
                            "there's more than one music library; say which (library_id)",
                        ));
                    }
                }
            }
        },
    })
}

/// The music profile that decides the song's results: the one asked for, else the wanted
/// artist's, else the preset that prefers lossless.
async fn profile_for(
    pool: &SqlitePool,
    asked: Option<i64>,
    wanted_artist: Option<(i64, Option<i64>)>,
) -> Result<i64, ApiError> {
    Ok(match asked {
        Some(id) => {
            if music_profiles::get(pool, id).await?.is_none() {
                return Err(bad("there's no such music profile"));
            }
            id
        }
        None => {
            let artists = match wanted_artist {
                Some((id, _)) => music_profiles::for_artist(pool, id).await?,
                None => None,
            };
            match artists {
                Some(profile) => profile.id,
                None => music_profiles::lossless(pool).await?,
            }
        }
    })
}

/// Refuses a song that's already wanted, by its recording or by its title, artist and album.
async fn refuse_if_wanted(
    pool: &SqlitePool,
    mb_recording_id: &Option<String>,
    title: &str,
    artist: &str,
    album: &Option<String>,
) -> Result<(), ApiError> {
    let already: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM wanted_songs
         WHERE (mb_recording_id IS NOT NULL AND mb_recording_id = ?)
            OR (title = ? COLLATE NOCASE AND artist = ? COLLATE NOCASE
                AND coalesce(album, '') = coalesce(?, '') COLLATE NOCASE)",
    )
    .bind(mb_recording_id)
    .bind(title)
    .bind(artist)
    .bind(album)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = already {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("it's already wanted (song {id})"),
        ));
    }
    Ok(())
}

/// No longer wanted: its searches go with it.
async fn remove(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let removed = sqlx::query("DELETE FROM wanted_songs WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await?
        .rows_affected();
    if removed == 0 {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such wanted song"));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        token: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn setup(music_libraries: &[i64]) -> (SqlitePool, Router, String) {
        let pool = db::open_in_memory().await;
        for id in music_libraries {
            sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (?, ?, 'music', ?)")
                .bind(id)
                .bind(format!("music{id}"))
                .bind(format!("/media/music{id}"))
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (9, 'movies', 'movies', '/media/movies')")
            .execute(&pool)
            .await
            .unwrap();
        let user = auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let token = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap()
            .1;
        (pool.clone(), routes(pool), token)
    }

    #[tokio::test]
    async fn a_song_goes_to_the_only_music_library_with_the_flac_preset() {
        let (pool, app, token) = setup(&[3]).await;
        let (status, song) = send(
            &app,
            "POST",
            "/api/v1/wanted/songs",
            &token,
            json!({"title": " Paranoid Android ", "artist": "Radiohead", "length_seconds": 383}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{song}");
        let preset = music_profiles::lossless(&pool).await.unwrap();
        assert_eq!(
            (
                &song["title"],
                &song["library_id"],
                &song["music_profile_id"],
                &song["album"]
            ),
            (
                &json!("Paranoid Android"),
                &json!(3),
                &json!(preset),
                &Value::Null
            )
        );
        let (status, listed) = send(&app, "GET", "/api/v1/wanted/songs", &token, Value::Null).await;
        assert_eq!(
            (status, listed.as_array().unwrap().len()),
            (StatusCode::OK, 1)
        );

        // The same song again, whatever its case.
        let (status, body) = send(
            &app,
            "POST",
            "/api/v1/wanted/songs",
            &token,
            json!({"title": "paranoid android", "artist": "RADIOHEAD"}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");

        let id = song["id"].as_i64().unwrap();
        let uri = format!("/api/v1/wanted/songs/{id}");
        assert_eq!(
            send(&app, "DELETE", &uri, &token, Value::Null).await.0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            send(&app, "DELETE", &uri, &token, Value::Null).await.0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn a_wanted_artists_song_takes_its_library_and_profile() {
        let (pool, app, token) = setup(&[3, 4]).await;
        let profile: i64 = sqlx::query_scalar(
            "INSERT INTO music_profiles (source, name, upgrade_allowed, cutoff, updated_at)
             VALUES ('lidarr', 'Lossless', 0, 'FLAC', 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO wanted_artists (mb_artist_id, name, monitored, monitor_new, quality_profile, library_id, source, added_at)
             VALUES ('mb-a', 'Radiohead', 1, 'all', 'Lossless', 4, 'lidarr', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (status, song) = send(
            &app,
            "POST",
            "/api/v1/wanted/songs",
            &token,
            json!({"title": "Creep", "artist": "radiohead", "album": "Pablo Honey"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{song}");
        assert_eq!(
            (&song["library_id"], &song["music_profile_id"]),
            (&json!(4), &json!(profile))
        );

        // Someone else, with two music libraries to choose from: it asks which.
        let (status, body) = send(
            &app,
            "POST",
            "/api/v1/wanted/songs",
            &token,
            json!({"title": "Song", "artist": "Nobody"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("say which (library_id)")
        );
    }

    #[tokio::test]
    async fn turns_down_what_wont_do() {
        let (_, app, token) = setup(&[3]).await;
        for (body, why) in [
            (json!({"title": "  ", "artist": "A"}), "title must be"),
            (
                json!({"title": "T", "artist": "A", "length_seconds": 0}),
                "length_seconds",
            ),
            (
                json!({"title": "T", "artist": "A", "mb_recording_id": "nope"}),
                "isn't a MusicBrainz id",
            ),
            (
                json!({"title": "T", "artist": "A", "library_id": 9}),
                "isn't a music library",
            ),
            (
                json!({"title": "T", "artist": "A", "library_id": 77}),
                "no such library",
            ),
            (
                json!({"title": "T", "artist": "A", "music_profile_id": 77}),
                "no such music profile",
            ),
        ] {
            let (status, answer) =
                send(&app, "POST", "/api/v1/wanted/songs", &token, body.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert!(
                answer["error"].as_str().unwrap().contains(why),
                "{body}: {answer}"
            );
        }
        let (status, _) = send(
            &app,
            "POST",
            "/api/v1/wanted/songs",
            &token,
            json!({"title": "T", "artist": "A", "colour": "blue"}),
        )
        .await;
        assert!(status.is_client_error(), "unknown fields are refused");
    }
}
