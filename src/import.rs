//! Importing Radarr's and Sonarr's lists into the wanted list, and handing their ids to the
//! matcher for folders already on disk.

use std::collections::HashMap;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::arr::ArrClient;
use crate::db::unix_now;
use crate::wanted_edit::{self, Kind};

mod lidarr;
mod radarr;
mod sonarr;

pub use lidarr::*;
pub use radarr::*;
pub use sonarr::*;

/// A library an *arr's root folder can stand for.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LibraryRef {
    /// The library's row id.
    pub id: i64,
    /// Its name, as shown and as an explicit root folder mapping names it.
    pub name: String,
    /// Its folder on this server.
    pub path: String,
}

/// Libraries of one kind (`movies` or `shows`).
pub async fn libraries(pool: &SqlitePool, kind: &str) -> anyhow::Result<Vec<LibraryRef>> {
    Ok(
        sqlx::query_as("SELECT id, name, path FROM libraries WHERE kind = ? ORDER BY name")
            .bind(kind)
            .fetch_all(pool)
            .await?,
    )
}

fn last_component(path: &str) -> Option<&str> {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|c| !c.is_empty())
}

/// The library an *arr root folder stands for: an explicit mapping (root folder → library
/// name) first, then a library named like the folder or whose path ends like it, then the only
/// library of the kind if there is just one.
pub fn library_for(
    root: Option<&str>,
    libraries: &[LibraryRef],
    explicit: &HashMap<String, String>,
) -> Option<i64> {
    let root = root?.trim_end_matches('/');
    if let Some(name) = explicit
        .get(root)
        .or_else(|| explicit.get(&format!("{root}/")))
    {
        return libraries.iter().find(|l| &l.name == name).map(|l| l.id);
    }
    let folder = last_component(root)?;
    libraries
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(folder) || last_component(&l.path) == Some(folder))
        .or(if libraries.len() == 1 {
            libraries.first()
        } else {
            None
        })
        .map(|l| l.id)
}

/// The *arr listed nothing while entries from it exist; nothing was changed.
#[derive(Debug)]
pub struct RefusedToEmpty(pub String);

impl std::fmt::Display for RefusedToEmpty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RefusedToEmpty {}

/// Removes wanted entries that came from `source` and are no longer in its list. Refuses to
/// remove everything when the *arr listed nothing: that's a wrong URL or a fresh install far
/// more often than a real "I want nothing".
async fn remove_unlisted(
    tx: &mut sqlx::SqliteConnection,
    source: &str,
    listed: &[i64],
) -> anyhow::Result<u64> {
    let (count, delete) = match source {
        "radarr" => (
            "SELECT count(*) FROM wanted_movies WHERE source = 'radarr'",
            "DELETE FROM wanted_movies WHERE source = 'radarr'
             AND tmdb_id NOT IN (SELECT value FROM json_each(?))",
        ),
        _ => (
            "SELECT count(*) FROM wanted_series WHERE source = 'sonarr'",
            "DELETE FROM wanted_series WHERE source = 'sonarr'
             AND tvdb_id NOT IN (SELECT value FROM json_each(?))",
        ),
    };
    if listed.is_empty() {
        let existing: i64 = sqlx::query_scalar(count).fetch_one(&mut *tx).await?;
        if existing > 0 {
            return Err(RefusedToEmpty(format!(
                "{source} listed nothing; refusing to remove all {existing} entries imported from it (check the URL)"
            ))
            .into());
        }
    }
    let removed = sqlx::query(delete)
        .bind(serde_json::to_string(listed)?)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    Ok(removed)
}

#[cfg(test)]
pub mod testing {
    //! A stand-in for Radarr and Sonarr that serves canned lists to the right API key.

    use std::sync::{Arc, Mutex};

    use axum::Json;
    use axum::extract::{Request, State};
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use serde_json::Value;

    #[derive(Clone, Default)]
    pub struct FakeArr {
        /// (path and query, body)
        pub replies: Arc<Mutex<Vec<(String, Value)>>>,
    }

    impl FakeArr {
        pub fn reply(&self, path: &str, body: Value) -> &Self {
            self.replies.lock().unwrap().push((path.into(), body));
            self
        }

        /// Starts the server and returns its URL.
        pub async fn start(&self) -> String {
            let app = axum::Router::new()
                .fallback(handle)
                .with_state(self.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            format!("http://{addr}")
        }
    }

    async fn handle(State(fake): State<FakeArr>, request: Request) -> Response {
        if request
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            != Some("key")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let wanted = request
            .uri()
            .path_and_query()
            .map(|p| p.to_string())
            .unwrap_or_default();
        let replies = fake.replies.lock().unwrap();
        match replies.iter().find(|(path, _)| *path == wanted) {
            Some((_, body)) => Json(body.clone()).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }
}

#[cfg(test)]
mod tests;
