//! Melete's own WireGuard tunnel for admins (`crate::tunnel`): every peer it has now, whose it
//! is and what it's doing, and removing any of them at once. Where a peer connects from is in
//! the answer only while it's connected, read from WireGuard when asked and never kept or
//! logged. No key is ever in an answer.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::auth::{AppState, require_admin};
use super::{ApiError, bad_request};
use crate::tunnel::{self, Kind, MAX_PEERS, PeerNow, Slot, Tunnel};
use crate::watch_party::guests;

#[derive(Clone)]
struct PeersState {
    pool: SqlitePool,
    tunnel: Arc<Slot>,
}

/// The routes, for admins alone, on `tunnel` once it's started.
pub fn routes(pool: SqlitePool, tunnel: Arc<Slot>) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/tunnel/peers", get(list))
        .route("/api/v1/tunnel/peers/{address}", delete(remove))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(PeersState { pool, tunnel })
}

fn running(state: &PeersState) -> Result<Arc<Tunnel>, ApiError> {
    state.tunnel.get().cloned().ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "Melete's own tunnel isn't set up on this server (MELETE_TUNNEL_PORT)",
        )
    })
}

/// Household users' names, and devices' and guests', by id, for the peers listed.
#[derive(Default)]
struct Names {
    users: HashMap<i64, String>,
    devices: HashMap<i64, String>,
    guests: HashMap<i64, String>,
}

impl Names {
    async fn of(pool: &SqlitePool) -> sqlx::Result<Names> {
        let pairs = async |sql: &'static str| -> sqlx::Result<HashMap<i64, String>> {
            let rows: Vec<(i64, String)> = sqlx::query_as(sql).fetch_all(pool).await?;
            Ok(rows.into_iter().collect())
        };
        Ok(Names {
            users: pairs("SELECT id, username FROM users").await?,
            devices: pairs("SELECT id, name FROM tunnel_devices").await?,
            guests: pairs("SELECT id, name FROM watch_party_guests").await?,
        })
    }

    fn user(&self, id: i64) -> Value {
        json!({ "id": id, "username": self.users.get(&id) })
    }
}

/// One peer as the API gives it.
fn peer_json(peer: &PeerNow, names: &Names) -> Value {
    let (kind, user, device, guest) = match &peer.kind {
        Kind::Device(d) => {
            let parts: Vec<&str> = d.parts.iter().map(|p| tunnel::part_name(*p)).collect();
            let device = json!({ "id": d.id, "name": names.devices.get(&d.id), "parts": parts });
            ("device", names.user(d.user_id), device, Value::Null)
        }
        Kind::Guest(g) => {
            let name = g.guest_id.and_then(|id| names.guests.get(&id));
            let guest = json!({ "party_id": g.party_id, "guest_id": g.guest_id, "name": name });
            ("guest", names.user(g.made_by), Value::Null, guest)
        }
    };
    json!({
        "address": peer.address.to_string(),
        "kind": kind,
        "user": user,
        "device": device,
        "guest": guest,
        "connected": peer.connected(),
        "last_handshake": peer.last_handshake.map(|d| d.as_secs()),
        "rx_bytes": peer.rx_bytes,
        "tx_bytes": peer.tx_bytes,
        "connections": peer.connections,
        "activity": peer.activity.map(tunnel::Activity::name),
        "last_request_at": peer.last_request_at,
        "endpoint": peer.endpoint.map(|e| e.to_string()),
    })
}

/// Every peer the tunnel has now.
async fn list(State(state): State<PeersState>) -> Result<Json<Value>, ApiError> {
    let tunnel = running(&state)?;
    let peers = tunnel.peers_now().await;
    let names = Names::of(&state.pool).await?;
    let peers: Vec<Value> = peers.iter().map(|p| peer_json(p, &names)).collect();
    Ok(Json(json!({ "max_peers": MAX_PEERS, "peers": peers })))
}

/// Removes the peer at `address` at once. A device's row goes too, so it doesn't come back
/// when the tunnel starts again; a guest leaves their party, as when sent away, or, before
/// anyone joined through it, its invite is withdrawn.
async fn remove(
    State(state): State<PeersState>,
    Path(address): Path<String>,
) -> Result<StatusCode, ApiError> {
    let tunnel = running(&state)?;
    let address: Ipv4Addr = address
        .parse()
        .map_err(|_| bad_request("give the peer's address in the tunnel, such as 10.73.0.2"))?;
    let Some(kind) = tunnel.kind_of(address) else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "the tunnel has no peer at that address",
        ));
    };
    match kind {
        Kind::Device(device) => {
            sqlx::query("DELETE FROM tunnel_devices WHERE id = ?")
                .bind(device.id)
                .execute(&state.pool)
                .await?;
        }
        Kind::Guest(guest) => match guest.guest_id {
            Some(id) => guests::leave(&state.pool, id).await?,
            None => guests::withdraw(&state.pool, &guest.invite_hash).await?,
        },
    }
    tunnel.remove(address).await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;
