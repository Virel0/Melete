//! An invite that comes with a way in: a peer of Melete's own WireGuard tunnel for the guest,
//! so they reach their party from outside the household with no other way set up.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::json;

use super::{PartyState, refused};
use crate::api::ApiError;
use crate::api::auth::CurrentUser;
use crate::tunnel::{Refusal, UNUSED_PER_USER};
use crate::watch_party::guests;

/// Makes an invite as `invites` does, and a tunnel peer for the guest who'll use it: its
/// config holds the peer's private key and is shown this once. The peer reaches the party's
/// routes alone, and goes when the invite does unused, or once the guest who joined through it
/// leaves or the party ends.
pub(super) async fn make(
    State(state): State<PartyState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Response, ApiError> {
    let tunnel = state.tunnel.get().cloned().ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "Melete's own tunnel isn't set up on this server (MELETE_TUNNEL_PORT)",
        )
    })?;
    let (secret, expires_at) = guests::invite(&state.pool, (current.user.id, current.token_id), id)
        .await?
        .map_err(refused)?;
    let hash = crate::auth::token_hash(&secret);
    let added = tunnel.add_guest(id, hash.clone(), current.user.id).await;
    if !matches!(added, Ok(Ok(_))) {
        // No peer, so no invite either.
        guests::withdraw(&state.pool, &hash).await?;
    }
    let made = match added? {
        Ok(made) => made,
        Err(refusal) => {
            return Err(match refusal {
                Refusal::Full => ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "the tunnel has as many peers as it takes; try again once a party has ended",
                ),
                Refusal::TooManyUnused => ApiError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    format!(
                        "you have {UNUSED_PER_USER} tunnel invites to this party that no guest \
                         has used yet; wait for one to be used or run out"
                    ),
                ),
                // A guest's peer has keys of its own, and isn't a device.
                Refusal::TooManyDevices | Refusal::KeyInUse => ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "the tunnel didn't take a new peer; try again",
                ),
            });
        }
    };
    let body = json!({
        "invite": secret,
        "path": format!("/#/guest/{secret}"),
        "expires_at": expires_at,
        "address": made.address.to_string(),
        "server": format!("http://{}", tunnel.own_address()),
        "wireguard_config": made.config,
    });
    Ok((StatusCode::CREATED, Json(body)).into_response())
}
