//! Starting a sign-in at the provider, from the web UI or an app.

use super::*;

/// Begins a sign-in at the provider, mapping why it couldn't to an answer. What the provider
/// said goes to the log, not the caller.
pub(super) async fn begin(
    client: &Client,
    purpose: Purpose,
    device: &str,
    address: &str,
    home: bool,
) -> Result<String, ApiError> {
    match client.begin_from(purpose, device, address, home).await {
        Ok(Ok(url)) => Ok(url),
        Ok(Err(BeginError::Busy)) => Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too many sign-ins are under way; try again in a few minutes",
        )),
        Err(err) => {
            tracing::warn!(error = format!("{err:#}"), "the identity provider failed");
            Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "the identity provider didn't answer as it should; an admin can test it under Settings",
            ))
        }
    }
}

#[derive(Deserialize)]
pub(super) struct Start {
    pub(super) device_name: String,
    /// An app's own URL scheme, or a port on the device itself; null (or left out) for the web UI.
    #[serde(default)]
    pub(super) return_to: Option<String>,
}

pub(super) async fn start(
    State(state): State<OidcState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    client_addr: Option<Extension<ClientAddr>>,
    opens: Option<Extension<Opens>>,
    Json(body): Json<Start>,
) -> Result<Response, ApiError> {
    let client = client(&state)?;
    let device = auth::validate_label(&body.device_name).map_err(bad_request)?;
    let address = address(client_addr, peer);
    let home = from_home(opens);
    match body.return_to {
        Some(return_to) => {
            if !client.may_return_to(&return_to) {
                return Err(bad_request(
                    "return_to must be one an admin listed for apps (MELETE_OIDC_APP_RETURNS), or a port on the device itself",
                ));
            }
            let secret = oidc::new_secret()?;
            let purpose = Purpose::App {
                secret: token_hash(&secret),
                return_to,
            };
            let url = begin(client, purpose, device, &address, home).await?;
            Ok(Json(json!({ "authorize_url": url, "secret": secret })).into_response())
        }
        None => {
            let bound = oidc::random(32)?;
            let purpose = Purpose::Web {
                binding: token_hash(&bound),
            };
            let url = begin(client, purpose, device, &address, home).await?;
            let cookie = binding_cookie(Some(&bound), client.https());
            Ok((
                [(header::SET_COOKIE, cookie)],
                Json(json!({ "authorize_url": url, "secret": null })),
            )
                .into_response())
        }
    }
}
