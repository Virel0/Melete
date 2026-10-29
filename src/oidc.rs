//! Signing in through an outside identity provider with OpenID Connect, off unless
//! `MELETE_OIDC_*` is set. The only code that talks to the provider.
//!
//! The authorization code flow, with PKCE and a nonce: the browser is sent to the provider,
//! comes back to Melete's callback with a code, and Melete trades the code for an ID token at
//! the provider's token endpoint, with its client secret. That answer comes straight from the
//! provider over TLS, which is what vouches for the token (OpenID Connect Core 3.1.3.7), so
//! endpoints must be https (or on this machine) and its claims are checked: issuer, audience,
//! expiry and nonce. A provider identity signs in only as the Melete user it was linked to,
//! by that user while signed in; nobody is made or matched by name.
//!
//! What leaves the server: to the provider, the client id and secret, the code, the PKCE
//! verifier and Melete's callback address, and only the `openid` scope is asked for, so the
//! provider hands back an opaque id and nothing else. Who sees it: the provider, which the
//! household chose. Turned off by clearing the settings.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::config::{Oidc, oidc_url_allowed};
use crate::db::unix_now;

mod attempts;
mod id_token;
mod secrets;

pub use attempts::*;
use id_token::*;
pub use secrets::*;

const USER_AGENT: &str = concat!("Melete/", env!("CARGO_PKG_VERSION"));
/// How long a sign-in at the provider may take.
const ATTEMPT_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// How long an app has to redeem the code it was sent back with.
const RESULT_LIFETIME: Duration = Duration::from_secs(2 * 60);
const MAX_ATTEMPTS: usize = 200;
/// The last of those kept for requests through a way in that opens everything (the household's
/// own), so sign-ins started from outside can't use them all up.
const HOME_RESERVE: usize = 40;
const MAX_ATTEMPTS_PER_ADDRESS: usize = 10;
/// The provider's discovery document is asked for again after this.
const DISCOVERY_TTL: Duration = Duration::from_secs(60 * 60);
/// Answers bigger than this aren't read.
const MAX_ANSWER: usize = 1 << 20;
/// Clocks may differ this much between Melete and the provider.
const CLOCK_SKEW: i64 = 120;
/// What the secret an app holds while it signs in at the provider starts with.
pub const SECRET_PREFIX: &str = "mos_";

/// The provider's endpoints, from its discovery document.
#[derive(Debug, Clone)]
struct Provider {
    authorization_endpoint: reqwest::Url,
    token_endpoint: reqwest::Url,
    /// Whether the client secret goes in the Authorization header (the default) or the body.
    basic_auth: bool,
}

/// Why a sign-in at the provider wasn't started.
#[derive(Debug, PartialEq, Eq)]
pub enum BeginError {
    /// Too many sign-ins are under way, in all or from this address.
    Busy,
}

/// The provider as Melete talks to it: its settings, its discovery document (fetched again
/// after an hour) and the sign-ins under way.
pub struct Client {
    http: reqwest::Client,
    settings: Oidc,
    provider: Mutex<Option<(Provider, Instant)>>,
    pending: Arc<Pending>,
    /// What this client's sign-ins are kept under in `pending`.
    scope: Vec<u8>,
}

impl Client {
    /// A client with sign-ins of its own, for a check or a test.
    pub fn new(settings: Oidc) -> anyhow::Result<Self> {
        Self::with_pending(settings, Arc::default())
    }

    /// A client keeping its sign-ins under way in `pending`, where a client built before it
    /// with the same settings left them.
    pub fn with_pending(settings: Oidc, pending: Arc<Pending>) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(15))
            // The client secret goes to the token endpoint, and nowhere it might send us.
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            http,
            scope: scope(&settings),
            settings,
            provider: Mutex::new(None),
            pending,
        })
    }

    /// The provider, as identities linked through it are kept.
    pub fn issuer(&self) -> &str {
        &self.settings.issuer
    }

    /// What the sign-in button calls the provider.
    pub fn name(&self) -> &str {
        &self.settings.name
    }

    /// Whether Melete's own pages are on https, so its cookies should say Secure.
    pub fn https(&self) -> bool {
        self.settings.redirect_url.starts_with("https://")
    }

    /// Whether the provider answers as it should: its discovery document, naming itself as
    /// set, with endpoints Melete may use. Nobody signs in.
    pub async fn check(&self) -> anyhow::Result<()> {
        self.discover().await.map(|_| ())
    }

    async fn provider(&self) -> anyhow::Result<Provider> {
        if let Some((provider, at)) = &*self.provider.lock().expect("no holder panics")
            && at.elapsed() < DISCOVERY_TTL
        {
            return Ok(provider.clone());
        }
        let provider = self.discover().await?;
        *self.provider.lock().expect("no holder panics") = Some((provider.clone(), Instant::now()));
        Ok(provider)
    }

    async fn discover(&self) -> anyhow::Result<Provider> {
        #[derive(Deserialize)]
        struct Discovery {
            issuer: String,
            authorization_endpoint: String,
            token_endpoint: String,
            #[serde(default)]
            token_endpoint_auth_methods_supported: Option<Vec<String>>,
        }
        let issuer = &self.settings.issuer;
        let url = format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        );
        let answer = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow!("the provider didn't answer: {}", e.without_url()))?;
        let found: Discovery = read_json(answer)
            .await
            .context("the provider's discovery document")?;
        if found.issuer != *issuer {
            bail!(
                "the provider calls itself {:?}, not {issuer:?}; set MELETE_OIDC_ISSUER to exactly what it says",
                found.issuer
            );
        }
        let endpoint = |name: &str, text: &str| -> anyhow::Result<reqwest::Url> {
            let url = reqwest::Url::parse(text)
                .with_context(|| format!("the provider's {name} isn't a URL"))?;
            if !oidc_url_allowed(&url) {
                bail!("the provider's {name} isn't https");
            }
            Ok(url)
        };
        let methods = found.token_endpoint_auth_methods_supported;
        let basic_auth = match &methods {
            None => true,
            Some(m) if m.iter().any(|m| m == "client_secret_basic") => true,
            Some(m) if m.iter().any(|m| m == "client_secret_post") => false,
            Some(_) => bail!(
                "the provider takes no client secret (client_secret_basic or client_secret_post)"
            ),
        };
        Ok(Provider {
            authorization_endpoint: endpoint(
                "authorization endpoint",
                &found.authorization_endpoint,
            )?,
            token_endpoint: endpoint("token endpoint", &found.token_endpoint)?,
            basic_auth,
        })
    }

    /// Starts a sign-in at the provider, as from home: the address to send the browser to.
    #[cfg(test)]
    pub async fn begin(
        &self,
        purpose: Purpose,
        device_name: &str,
        address: &str,
    ) -> anyhow::Result<Result<String, BeginError>> {
        self.begin_from(purpose, device_name, address, true).await
    }

    /// Starts a sign-in at the provider: the address to send the browser to. `home` when the
    /// request came through a way in that opens everything, which may use the places kept
    /// for home.
    pub async fn begin_from(
        &self,
        purpose: Purpose,
        device_name: &str,
        address: &str,
        home: bool,
    ) -> anyhow::Result<Result<String, BeginError>> {
        // Room is checked before the provider is asked, and again when the attempt is kept.
        if self.busy(address, home, Instant::now()) {
            return Ok(Err(BeginError::Busy));
        }
        let provider = self.provider().await?;
        let state = random(32)?;
        let nonce = random(32)?;
        let verifier = random(32)?;
        let mut url = provider.authorization_endpoint.clone();
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.settings.client_id)
            .append_pair("redirect_uri", &self.settings.redirect_url)
            .append_pair("scope", "openid")
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &challenge(&verifier))
            .append_pair("code_challenge_method", "S256");
        let now = Instant::now();
        let mut attempts = self.pending.attempts.lock().expect("no holder panics");
        if busy_in(&mut attempts, address, home, now) {
            return Ok(Err(BeginError::Busy));
        }
        attempts.insert(
            state,
            Attempt {
                purpose,
                device_name: device_name.to_owned(),
                address: address.to_owned(),
                nonce,
                verifier,
                expires: now + ATTEMPT_LIFETIME,
                scope: self.scope.clone(),
            },
        );
        Ok(Ok(url.into()))
    }

    fn busy(&self, address: &str, home: bool, now: Instant) -> bool {
        busy_in(
            &mut self.pending.attempts.lock().expect("no holder panics"),
            address,
            home,
            now,
        )
    }

    /// The attempt a callback's state names, taken out so it's used once; None when there's
    /// none, it ran out or it began under other settings.
    pub fn take(&self, state: &str) -> Option<Attempt> {
        let mut attempts = self.pending.attempts.lock().expect("no holder panics");
        let attempt = attempts.remove(state)?;
        (attempt.expires > Instant::now() && attempt.scope == self.scope).then_some(attempt)
    }

    /// Trades the provider's code for an ID token and checks it: who signed in.
    pub async fn finish(&self, attempt: &Attempt, code: &str) -> anyhow::Result<Identity> {
        #[derive(Deserialize)]
        struct Tokens {
            id_token: String,
        }
        let provider = self.provider().await?;
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", self.settings.redirect_url.as_str()),
            ("code_verifier", attempt.verifier.as_str()),
        ];
        let secret = self.settings.client_secret.expose();
        let request = self.http.post(provider.token_endpoint.clone());
        let request = if provider.basic_auth {
            // RFC 6749 2.3.1: each is form-encoded before they're joined.
            request.basic_auth(
                form_encode(&self.settings.client_id),
                Some(form_encode(secret)),
            )
        } else {
            form.push(("client_id", self.settings.client_id.as_str()));
            form.push(("client_secret", secret));
            request
        };
        let answer = request
            .form(&form)
            .send()
            .await
            .map_err(|e| anyhow!("the provider didn't answer: {}", e.without_url()))?;
        if !answer.status().is_success() {
            // Its error body may echo what was sent; only the status is kept.
            bail!("the provider refused the sign-in ({})", answer.status());
        }
        let tokens: Tokens = read_json(answer).await.context("the provider's tokens")?;
        let subject = check_id_token(
            &tokens.id_token,
            &self.settings.issuer,
            &self.settings.client_id,
            &attempt.nonce,
            unix_now(),
        )?;
        Ok(Identity {
            issuer: self.settings.issuer.clone(),
            subject,
        })
    }

    /// Keeps an app's finished sign-in for it to redeem: the one-time code to send it back
    /// with.
    pub fn finished(
        &self,
        secret: Vec<u8>,
        user_id: i64,
        device_name: String,
    ) -> anyhow::Result<String> {
        let code = random(32)?;
        let now = Instant::now();
        let mut finished = self.pending.finished.lock().expect("no holder panics");
        finished.retain(|_, f| f.expires > now);
        finished.insert(
            crate::auth::token_hash(&code),
            Finished {
                secret,
                user_id,
                device_name,
                expires: now + RESULT_LIFETIME,
                scope: self.scope.clone(),
            },
        );
        Ok(code)
    }

    /// Whether an app may be sent back to `return_to`.
    pub fn may_return_to(&self, return_to: &str) -> bool {
        return_to_allowed(return_to, &self.settings.app_returns)
    }

    /// Keeps an app's sign-in that's done at the provider until the person in the browser
    /// confirms it: the token for the browser's cookie and the one for the page's form.
    pub fn to_confirm(&self, confirm: Confirm) -> anyhow::Result<(String, String)> {
        let token = random(32)?;
        let form = random(32)?;
        let now = Instant::now();
        let mut confirms = self.pending.confirms.lock().expect("no holder panics");
        confirms.retain(|_, (_, c, _)| c.expires > now);
        confirms.insert(
            crate::auth::token_hash(&token),
            (
                crate::auth::token_hash(&form),
                Confirm {
                    expires: now + CONFIRM_LIFETIME,
                    ..confirm
                },
                self.scope.clone(),
            ),
        );
        Ok((token, form))
    }

    /// The sign-in waiting for this browser's cookie and its page's form token, once; None
    /// when either is wrong, it ran out or it began under other settings.
    pub fn confirmed(&self, token: &str, form: &str) -> Option<Confirm> {
        let mut confirms = self.pending.confirms.lock().expect("no holder panics");
        let key = crate::auth::token_hash(token);
        let (form_hash, _, _) = confirms.get(&key)?;
        if *form_hash != crate::auth::token_hash(form) {
            return None;
        }
        let (_, confirm, scope) = confirms.remove(&key)?;
        (confirm.expires > Instant::now() && scope == self.scope).then_some(confirm)
    }

    /// The user and device name an app's code and secret redeem, once; none for a code given
    /// under other settings.
    pub fn redeem(&self, code: &str, secret: &str) -> Option<(i64, String)> {
        let mut finished = self.pending.finished.lock().expect("no holder panics");
        let key = crate::auth::token_hash(code);
        let found = finished.get(&key)?;
        // A wrong secret leaves the code for its own app; a code alone is no use to anyone.
        if found.secret != crate::auth::token_hash(secret) {
            return None;
        }
        let found = finished.remove(&key)?;
        (found.expires > Instant::now() && found.scope == self.scope)
            .then_some((found.user_id, found.device_name))
    }
}

fn busy_in(
    attempts: &mut HashMap<String, Attempt>,
    address: &str,
    home: bool,
    now: Instant,
) -> bool {
    attempts.retain(|_, a| a.expires > now);
    // An IPv6 /64 counts as one address.
    let group = crate::quick_connect::address_group(address);
    let from_here = attempts
        .values()
        .filter(|a| crate::quick_connect::address_group(&a.address) == group)
        .count();
    let room = if home {
        MAX_ATTEMPTS
    } else {
        MAX_ATTEMPTS - HOME_RESERVE
    };
    attempts.len() >= room || from_here >= MAX_ATTEMPTS_PER_ADDRESS
}

/// Where an app may be sent back to: exactly one of the addresses an admin listed
/// (`MELETE_OIDC_APP_RETURNS`), or plain http to a port on the device itself (RFC 8252). Any
/// other scheme could be a browser's or another app's that hands the code to someone else.
pub fn return_to_allowed(text: &str, listed: &[String]) -> bool {
    let Ok(url) = reqwest::Url::parse(text) else {
        return false;
    };
    if text.len() > 500 || url.fragment().is_some() {
        return false;
    }
    let loopback = url.scheme() == "http"
        && url.host_str().is_some_and(|host| {
            host.trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        });
    loopback
        || listed
            .iter()
            .any(|l| l == text || *l == String::from(url.clone()))
}

async fn read_json<T: serde::de::DeserializeOwned>(answer: reqwest::Response) -> anyhow::Result<T> {
    if !answer.status().is_success() {
        bail!("it answered {}", answer.status());
    }
    if answer
        .content_length()
        .is_some_and(|n| n > MAX_ANSWER as u64)
    {
        bail!("its answer is too big");
    }
    let bytes = answer
        .bytes()
        .await
        .map_err(|e| anyhow!("its answer was cut off: {}", e.without_url()))?;
    if bytes.len() > MAX_ANSWER {
        bail!("its answer is too big");
    }
    serde_json::from_slice(&bytes).map_err(|e| anyhow!("its answer isn't what was expected: {e}"))
}

/// A provider on this machine for tests: discovery and a token endpoint that hands out an ID
/// token for `subject`, with the nonce of the last authorization it was shown.
#[cfg(test)]
pub mod fake {
    use std::sync::{Arc, Mutex};

    use axum::extract::State;
    use axum::routing::{get, post};
    use axum::{Form, Json, Router};
    use serde_json::{Value, json};

    use super::*;

    #[derive(Default)]
    pub struct Seen {
        /// What the token endpoint was sent last.
        pub form: HashMap<String, String>,
        pub authorization: Option<String>,
    }

    #[derive(Clone)]
    pub struct Fake {
        pub issuer: String,
        /// The nonce to put in the next ID token, and who it says signed in.
        pub nonce: Arc<Mutex<String>>,
        pub subject: Arc<Mutex<String>>,
        pub seen: Arc<Mutex<Seen>>,
    }

    pub async fn start() -> Fake {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}/realm", listener.local_addr().unwrap());
        let fake = Fake {
            issuer: issuer.clone(),
            nonce: Arc::default(),
            subject: Arc::new(Mutex::new("subject-1".into())),
            seen: Arc::default(),
        };
        let discovery = json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
            "token_endpoint_auth_methods_supported": ["client_secret_basic"],
        });
        let app = Router::new()
            .route(
                "/realm/.well-known/openid-configuration",
                get(move || {
                    let discovery = discovery.clone();
                    async move { Json(discovery) }
                }),
            )
            .route("/realm/token", post(token))
            .with_state(fake.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        fake
    }

    async fn token(
        State(fake): State<Fake>,
        headers: axum::http::HeaderMap,
        Form(form): Form<HashMap<String, String>>,
    ) -> Json<Value> {
        {
            let mut seen = fake.seen.lock().unwrap();
            seen.form = form;
            seen.authorization = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
        }
        let claims = json!({
            "iss": fake.issuer,
            "sub": *fake.subject.lock().unwrap(),
            "aud": "melete",
            "exp": unix_now() + 300,
            "iat": unix_now(),
            "nonce": *fake.nonce.lock().unwrap(),
        });
        Json(json!({ "id_token": jwt(&claims), "token_type": "Bearer", "access_token": "x" }))
    }

    pub fn jwt(claims: &Value) -> String {
        format!(
            "{}.{}.c2ln",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256"}"#),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        )
    }

    pub fn settings(issuer: &str) -> Oidc {
        Oidc {
            issuer: issuer.to_owned(),
            client_id: "melete".into(),
            client_secret: crate::config::Secret::for_tests("client secret&"),
            redirect_url: "https://melete.example/api/v1/auth/oidc/callback".into(),
            name: "Example ID".into(),
            app_returns: vec![
                "com.example.app:/signed-in".into(),
                "aoide://signed-in".into(),
            ],
        }
    }

    /// The state and nonce a begun sign-in sent the browser off with.
    pub fn query(url: &str) -> HashMap<String, String> {
        reqwest::Url::parse(url)
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn a_sign_in_goes_out_with_pkce_and_comes_back_checked() {
        let fake = fake::start().await;
        let client = Client::new(fake::settings(&fake.issuer)).unwrap();
        client.check().await.unwrap();
        let purpose = Purpose::Web {
            binding: vec![1, 2],
        };
        let url = client
            .begin(purpose.clone(), "Firefox on Linux", "192.168.1.10")
            .await
            .unwrap()
            .unwrap();
        assert!(url.starts_with(&format!("{}/authorize?", fake.issuer)));
        let query = fake::query(&url);
        assert_eq!(query["scope"], "openid", "nothing more is asked for");
        assert_eq!(query["client_id"], "melete");
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(
            query["redirect_uri"],
            "https://melete.example/api/v1/auth/oidc/callback"
        );
        *fake.nonce.lock().unwrap() = query["nonce"].clone();

        let attempt = client.take(&query["state"]).unwrap();
        assert_eq!(attempt.purpose, purpose);
        assert!(client.take(&query["state"]).is_none(), "used once");
        let identity = client.finish(&attempt, "the-code").await.unwrap();
        assert_eq!(
            identity,
            Identity {
                issuer: fake.issuer.clone(),
                subject: "subject-1".into()
            }
        );
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen.form["code"], "the-code");
        assert_eq!(seen.form["grant_type"], "authorization_code");
        assert_eq!(
            challenge(&seen.form["code_verifier"]),
            query["code_challenge"]
        );
        assert!(
            !seen.form.contains_key("client_secret"),
            "the secret goes in the header"
        );
        let basic = seen.authorization.as_deref().unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(basic.strip_prefix("Basic ").unwrap())
            .unwrap();
        assert_eq!(decoded, b"melete:client+secret%26");
    }

    #[tokio::test]
    async fn a_token_for_another_sign_in_is_refused() {
        let fake = fake::start().await;
        let client = Client::new(fake::settings(&fake.issuer)).unwrap();
        let url = client
            .begin(
                Purpose::Link {
                    user_id: 1,
                    binding: vec![],
                },
                "",
                "a",
            )
            .await
            .unwrap()
            .unwrap();
        *fake.nonce.lock().unwrap() = "someone else's".into();
        let attempt = client.take(&fake::query(&url)["state"]).unwrap();
        let err = client.finish(&attempt, "code").await.unwrap_err();
        assert!(err.to_string().contains("isn't for this sign-in"), "{err}");
    }

    #[tokio::test]
    async fn sign_ins_outlive_a_new_client_only_under_the_same_settings() {
        let fake = fake::start().await;
        let pending: Arc<Pending> = Arc::default();
        let link = Purpose::Link {
            user_id: 1,
            binding: vec![],
        };
        let begin = async |client: &Client| {
            let url = client.begin(link.clone(), "", "a").await.unwrap().unwrap();
            fake::query(&url)["state"].clone()
        };
        let confirm = Confirm {
            secret: vec![1],
            user_id: 1,
            username: "alice".into(),
            device_name: "Living room iPad".into(),
            return_to: "aoide://signed-in".into(),
            expires: Instant::now(),
        };
        let first = Client::with_pending(fake::settings(&fake.issuer), pending.clone()).unwrap();
        let state = begin(&first).await;
        let code = first
            .finished(crate::auth::token_hash("s"), 1, "phone".into())
            .unwrap();
        let (token, form) = first.to_confirm(confirm.clone()).unwrap();
        drop(first);

        // The services set up afresh with the same settings: all of it is still there.
        let same = Client::with_pending(fake::settings(&fake.issuer), pending.clone()).unwrap();
        assert!(same.take(&state).is_some());
        assert_eq!(same.redeem(&code, "s"), Some((1, "phone".into())));
        assert!(same.confirmed(&token, &form).is_some());

        // Under another client id, none of it counts.
        let state = begin(&same).await;
        let code = same
            .finished(crate::auth::token_hash("s"), 1, "phone".into())
            .unwrap();
        let (token, form) = same.to_confirm(confirm).unwrap();
        let other = Client::with_pending(
            Oidc {
                client_id: "another".into(),
                ..fake::settings(&fake.issuer)
            },
            pending,
        )
        .unwrap();
        assert!(other.take(&state).is_none());
        assert_eq!(other.redeem(&code, "s"), None);
        assert!(other.confirmed(&token, &form).is_none());
    }

    #[tokio::test]
    async fn a_provider_naming_itself_otherwise_is_refused() {
        let fake = fake::start().await;
        let client = Client::new(fake::settings(&format!("{}/", fake.issuer))).unwrap();
        let err = client.check().await.unwrap_err();
        assert!(err.to_string().contains("calls itself"), "{err}");
    }

    #[test]
    fn id_tokens_are_checked_claim_by_claim() {
        let good = json!({ "iss": "https://id.example", "sub": "abc", "aud": "melete",
                           "exp": 1000, "iat": 900, "nonce": "n" });
        let check = |claims: &serde_json::Value| {
            check_id_token(&fake::jwt(claims), "https://id.example", "melete", "n", 950)
        };
        assert_eq!(check(&good).unwrap(), "abc");
        let with = |key: &str, value: serde_json::Value| {
            let mut c = good.clone();
            c[key] = value;
            c
        };
        let refused = [
            (with("iss", json!("https://evil.example")), "another issuer"),
            (with("aud", json!("someone-else")), "isn't for Melete"),
            (with("aud", json!(["melete", "other"])), "another party"),
            (with("azp", json!("other")), "another party"),
            (with("exp", json!(700)), "expired"),
            (with("iat", json!(5000)), "future"),
            (with("nonce", json!("m")), "this sign-in"),
            (with("sub", json!("")), "names nobody"),
        ];
        for (claims, why) in refused {
            let err = check(&claims).unwrap_err().to_string();
            assert!(err.contains(why), "{claims}: {err}");
        }
        let mut no_nonce = good.clone();
        no_nonce.as_object_mut().unwrap().remove("nonce");
        assert!(check(&no_nonce).is_err());
        let mut shared = with("aud", json!(["melete", "other"]));
        shared["azp"] = json!("melete");
        assert_eq!(
            check(&shared).unwrap(),
            "abc",
            "several audiences, issued to Melete"
        );
        assert!(check_id_token("not-a-jwt", "i", "c", "n", 0).is_err());
        assert!(check_id_token("a.b.c.d", "i", "c", "n", 0).is_err());
    }

    #[test]
    fn apps_are_sent_back_only_where_the_admin_listed() {
        let listed = ["com.example.app:/signed-in".to_owned()];
        for ok in [
            "com.example.app:/signed-in",
            "http://127.0.0.1:53111/callback",
            "http://[::1]:8000/",
        ] {
            assert!(return_to_allowed(ok, &listed), "{ok}");
        }
        for bad in [
            "com.example.app:/other",
            "x-safari-https://attacker.example/cb",
            "googlechromes://attacker.example/cb",
            "aoide://signed-in",
            "https://evil.example/",
            "http://evil.example/",
            "http://192.168.1.10/",
            "javascript:alert(1)",
            "com.example.app:/signed-in#frag",
            "not a url",
        ] {
            assert!(!return_to_allowed(bad, &listed), "{bad}");
        }
        assert!(
            !return_to_allowed("com.example.app:/signed-in", &[]),
            "none listed"
        );
    }

    #[test]
    fn a_confirmation_needs_the_browsers_cookie_and_the_pages_token_once() {
        let client = Client::new(fake::settings("https://id.example")).unwrap();
        let confirm = Confirm {
            secret: vec![1],
            user_id: 7,
            username: "alice".into(),
            device_name: "TV".into(),
            return_to: "aoide://signed-in".into(),
            expires: Instant::now(),
        };
        let (cookie, form) = client.to_confirm(confirm).unwrap();
        assert!(client.confirmed(&cookie, "wrong").is_none());
        assert!(client.confirmed(&form, &cookie).is_none());
        assert_eq!(client.confirmed(&cookie, &form).unwrap().user_id, 7);
        assert!(client.confirmed(&cookie, &form).is_none(), "once");
    }

    #[test]
    fn a_finished_sign_in_is_redeemed_once_and_only_with_its_secret() {
        let client = Client::new(fake::settings("https://id.example")).unwrap();
        let secret = new_secret().unwrap();
        let code = client
            .finished(crate::auth::token_hash(&secret), 7, "TV".into())
            .unwrap();
        assert_eq!(client.redeem(&code, "mos_wrong"), None);
        assert_eq!(client.redeem(&secret, &code), None);
        assert_eq!(client.redeem(&code, &secret), Some((7, "TV".into())));
        assert_eq!(client.redeem(&code, &secret), None);
    }

    #[tokio::test]
    async fn only_so_many_sign_ins_are_under_way() {
        let fake = fake::start().await;
        let client = Client::new(fake::settings(&fake.issuer)).unwrap();
        for _ in 0..MAX_ATTEMPTS_PER_ADDRESS {
            let purpose = Purpose::Web { binding: vec![] };
            client
                .begin(purpose, "x", "192.168.1.10")
                .await
                .unwrap()
                .unwrap();
        }
        let purpose = Purpose::Web { binding: vec![] };
        assert_eq!(
            client
                .begin(purpose.clone(), "x", "192.168.1.10")
                .await
                .unwrap(),
            Err(BeginError::Busy)
        );
        assert!(
            client
                .begin(purpose, "x", "192.168.1.11")
                .await
                .unwrap()
                .is_ok()
        );
    }

    #[tokio::test]
    async fn places_are_kept_for_home_when_outside_fills_the_rest() {
        let fake = fake::start().await;
        let client = Client::new(fake::settings(&fake.issuer)).unwrap();
        let purpose = || Purpose::Web { binding: vec![] };
        for n in 0..MAX_ATTEMPTS - HOME_RESERVE {
            let address = format!("203.0.{}.{}", n / 200, n % 200);
            let started = client.begin_from(purpose(), "x", &address, false).await;
            started.unwrap().unwrap();
        }
        let outside = client.begin_from(purpose(), "x", "203.0.9.9", false).await;
        assert_eq!(outside.unwrap(), Err(BeginError::Busy));
        let home = client
            .begin_from(purpose(), "x", "192.168.1.10", true)
            .await;
        assert!(home.unwrap().is_ok(), "home still gets in");
    }

    #[tokio::test]
    async fn an_ipv6_network_counts_as_one_address() {
        let fake = fake::start().await;
        let client = Client::new(fake::settings(&fake.issuer)).unwrap();
        for n in 0..MAX_ATTEMPTS_PER_ADDRESS {
            let purpose = Purpose::Web { binding: vec![] };
            let address = format!("2001:db8:7:7::{n:x}");
            client.begin(purpose, "x", &address).await.unwrap().unwrap();
        }
        let purpose = Purpose::Web { binding: vec![] };
        assert_eq!(
            client
                .begin(purpose.clone(), "x", "2001:db8:7:7:abcd::1")
                .await
                .unwrap(),
            Err(BeginError::Busy)
        );
        assert!(
            client
                .begin(purpose, "x", "2001:db8:7:8::1")
                .await
                .unwrap()
                .is_ok()
        );
    }

    #[test]
    fn client_secrets_are_form_encoded_for_basic() {
        assert_eq!(form_encode("a b&c:d"), "a+b%26c%3Ad");
        assert_eq!(form_encode("Az09-._*"), "Az09-._*");
    }
}
