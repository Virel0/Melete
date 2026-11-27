//! Household devices on Melete's own WireGuard tunnel over HTTP (`crate::tunnel`): each user
//! adds, lists and removes their own, and an admin removes anyone's. A device's preshared key,
//! and its config when Melete made its key pair, come back once when it's added and never
//! again.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Extension, Json, Router, middleware};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::auth::{AppState, CurrentUser, require_user};
use super::{ApiError, bad_request};
use crate::tunnel::{self, Arrival, MAX_PER_USER, NewDevice, Refusal, Slot};
use crate::ways_in::Part;

/// The longest a device's name is.
const NAME_CHARS: usize = 60;

#[derive(Clone)]
struct DeviceState {
    pool: SqlitePool,
    tunnel: Arc<Slot>,
}

/// The routes, adding devices' peers to `tunnel` once it's started.
pub fn routes(pool: SqlitePool, tunnel: Arc<Slot>) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/tunnel/devices", get(list).post(add))
        .route("/api/v1/tunnel/devices/{id}", delete(remove))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(DeviceState { pool, tunnel })
}

#[derive(sqlx::FromRow)]
struct Listed {
    id: i64,
    name: String,
    address: i64,
    video: bool,
    music: bool,
    requests: bool,
    admin: bool,
    created_at: i64,
}

impl Listed {
    fn json(&self) -> Value {
        let on = [
            (Part::Video, self.video),
            (Part::Music, self.music),
            (Part::Requests, self.requests),
            (Part::Admin, self.admin),
        ];
        let parts: Vec<&str> = on
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(p, _)| tunnel::part_name(p))
            .collect();
        json!({
            "id": self.id,
            "name": self.name,
            "parts": parts,
            "address": tunnel::stored_address(self.address).map(|a| a.to_string()),
            "created_at": self.created_at,
        })
    }
}

/// The caller's own devices, with no keys, and whether the tunnel is on to add more.
async fn list(
    State(state): State<DeviceState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let rows: Vec<Listed> = sqlx::query_as(
        "SELECT id, name, address, video, music, requests, admin, created_at
         FROM tunnel_devices WHERE user_id = ? ORDER BY id",
    )
    .bind(current.user.id)
    .fetch_all(&state.pool)
    .await?;
    let devices: Vec<Value> = rows.iter().map(Listed::json).collect();
    Ok(Json(json!({
        "enabled": state.tunnel.get().is_some(),
        "limit": MAX_PER_USER,
        "devices": devices,
    })))
}

#[derive(Deserialize)]
struct NewBody {
    name: String,
    parts: Vec<String>,
    /// The device's own WireGuard public key, base64; without it Melete makes the pair.
    #[serde(default)]
    public_key: Option<String>,
}

/// A device's name, trimmed: 1 to 60 characters, none of them control characters.
fn name_of(name: &str) -> Result<&str, ApiError> {
    let name = name.trim();
    let count = name.chars().count();
    if count == 0 || count > NAME_CHARS || name.chars().any(char::is_control) {
        return Err(bad_request(
            "a device's name is 1 to 60 characters, on one line",
        ));
    }
    Ok(name)
}

/// A WireGuard public key as its configs write it: base64 of 32 bytes, not all zeros.
pub(super) fn public_key_of(text: &str) -> Result<[u8; 32], ApiError> {
    let wrong = || bad_request("public_key is a WireGuard public key: 32 bytes in base64");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|_| wrong())?;
    let key: [u8; 32] = bytes.try_into().map_err(|_| wrong())?;
    if key == [0; 32] {
        return Err(wrong());
    }
    Ok(key)
}

/// Adds a device for the caller, as its body says; through a device's own tunnel, one that
/// reaches no more than that device does.
async fn add(
    State(state): State<DeviceState>,
    Extension(current): Extension<CurrentUser>,
    arrival: Option<Extension<Arrival>>,
    Json(body): Json<NewBody>,
) -> Result<Response, ApiError> {
    let tunnel = state.tunnel.get().cloned().ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "Melete's own tunnel isn't set up on this server (MELETE_TUNNEL_PORT)",
        )
    })?;
    let name = name_of(&body.name)?.to_owned();
    let parts = tunnel::device_parts(&body.parts).map_err(|e| bad_request(&e))?;
    let public_key = body.public_key.as_deref().map(public_key_of).transpose()?;
    if let Some(Extension(arrival)) = &arrival
        && let Some(through) = arrival.device()
        && !parts.iter().all(|p| through.parts.contains(p))
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "through a device's tunnel, a new device reaches no more than that one does",
        ));
    }
    let new = NewDevice {
        user_id: current.user.id,
        name,
        parts,
        public_key,
    };
    let names: Vec<&str> = new.parts.iter().map(|p| tunnel::part_name(*p)).collect();
    let (name, names) = (new.name.clone(), json!(names));
    let added = tunnel
        .add_device(&state.pool, new)
        .await?
        .map_err(refused)?;
    let body = json!({
        "id": added.id,
        "name": name,
        "parts": names,
        "address": added.address.to_string(),
        "created_at": added.created_at,
        "server_public_key": tunnel.own_public_key(),
        "server_address": tunnel.own_address().to_string(),
        "server": format!("http://{}", tunnel.own_address()),
        "endpoint": tunnel.endpoint(),
        "preshared_key": added.preshared_key,
        "wireguard_config": added.config,
    });
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

fn refused(refusal: Refusal) -> ApiError {
    match refusal {
        Refusal::TooManyDevices => ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!("you have {MAX_PER_USER} devices on the tunnel; remove one first"),
        ),
        Refusal::KeyInUse => ApiError::new(
            StatusCode::CONFLICT,
            "that public key is already on the tunnel; make a new key pair for this device",
        ),
        Refusal::Full | Refusal::TooManyUnused => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "the tunnel has as many peers as it takes; remove a device or try again later",
        ),
    }
}

/// Removes one of the caller's devices, or anyone's for an admin, and its peer at once.
async fn remove(
    State(state): State<DeviceState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let found: Option<(i64, i64)> =
        sqlx::query_as("SELECT user_id, address FROM tunnel_devices WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let Some((owner, address)) =
        found.filter(|(owner, _)| *owner == current.user.id || current.user.is_admin)
    else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such device of yours",
        ));
    };
    sqlx::query("DELETE FROM tunnel_devices WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(owner)
        .execute(&state.pool)
        .await?;
    if let (Some(tunnel), Some(address)) = (state.tunnel.get(), tunnel::stored_address(address)) {
        tunnel.remove_device(address, id).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;
