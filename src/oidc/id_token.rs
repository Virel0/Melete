//! Checking the claims of an ID token the token endpoint handed over: issuer, audience, expiry and
//! nonce.

use super::*;

/// The subject of an ID token the token endpoint handed over, once its claims check out:
/// Melete's issuer, Melete among its audience (and the party it was issued to, when it names
/// one), not expired, and this sign-in's nonce.
pub(super) fn check_id_token(
    token: &str,
    issuer: &str,
    client_id: &str,
    nonce: &str,
    now: i64,
) -> anyhow::Result<String> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Audience {
        One(String),
        Many(Vec<String>),
    }
    #[derive(Deserialize)]
    struct Claims {
        iss: String,
        sub: String,
        aud: Audience,
        exp: i64,
        #[serde(default)]
        iat: Option<i64>,
        #[serde(default)]
        nonce: Option<String>,
        #[serde(default)]
        azp: Option<String>,
    }
    let mut parts = token.split('.');
    let (Some(_header), Some(payload), Some(_signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        bail!("the provider's ID token isn't a JWT");
    };
    let payload = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| anyhow!("the provider's ID token can't be read"))?;
    let claims: Claims = serde_json::from_slice(&payload)
        .map_err(|_| anyhow!("the provider's ID token lacks what OpenID Connect requires"))?;
    if claims.iss != issuer {
        bail!("the ID token is from another issuer");
    }
    let audience = match claims.aud {
        Audience::One(a) => vec![a],
        Audience::Many(a) => a,
    };
    if !audience.iter().any(|a| a == client_id) {
        bail!("the ID token isn't for Melete");
    }
    if claims.azp.as_deref().is_some_and(|azp| azp != client_id)
        || (audience.len() > 1 && claims.azp.is_none())
    {
        bail!("the ID token was issued to another party");
    }
    if claims.exp + CLOCK_SKEW < now {
        bail!("the ID token has expired");
    }
    if claims.iat.is_some_and(|iat| iat > now + CLOCK_SKEW) {
        bail!("the ID token was issued in the future; check the clocks");
    }
    if claims.nonce.as_deref() != Some(nonce) {
        bail!("the ID token isn't for this sign-in");
    }
    if claims.sub.is_empty() || claims.sub.len() > 255 {
        bail!("the ID token names nobody");
    }
    Ok(claims.sub)
}
