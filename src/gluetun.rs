//! Gluetun's control server, for the VPN Melete's chosen services can go through (Gluetun's HTTP
//! proxy, set under Proxy). Melete reads how the VPN is (running or not, the address the outside
//! world sees, the forwarded port), and starts, stops and restarts it; nothing else. Which
//! servers it uses (provider, countries, keys) is Gluetun's own settings', which Melete neither
//! reads nor changes, so its key can't send the tunnel anywhere else. Its key goes in the
//! `X-API-Key` header and nowhere else. A control server that answers without a key is refused:
//! anyone on its network could stop the VPN.
//! What's asked and how it answers is Gluetun v3.41.3's (`internal/server`). Nothing here goes
//! through the proxy: the control server sits beside Melete.

use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use crate::config::Secret;

/// Gluetun's control server as Melete reaches it, with Melete's key.
#[derive(Clone)]
pub struct Client {
    http: crate::proxy::Http,
    base: String,
    key: Secret,
}

/// How the VPN is, for an admin to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct State {
    /// Whether the tunnel is up.
    pub running: bool,
    /// The address the outside world sees, while it's up.
    pub public_ip: Option<String>,
    /// That address's country, when Gluetun knows it.
    pub country: Option<String>,
    /// The port the VPN provider forwards, when there's one.
    pub forwarded_port: Option<u16>,
}

/// Why the control server couldn't be used, in words safe to show (never the key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trouble {
    /// It answered a request that carried no key.
    Open,
    /// It turned the key down.
    KeyRefused,
    /// It didn't answer, or not as Gluetun does.
    Unreachable(String),
    /// It turned a change down, with its reason.
    Refused(String),
}

impl std::fmt::Display for Trouble {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Trouble::Open => f.write_str(
                "Gluetun's control server answers without a key, so anyone on its network could stop the VPN; Melete won't use it until it asks for one. Mount the roles file compose.vpn.yaml names, with a key for Melete.",
            ),
            Trouble::KeyRefused => f.write_str("Gluetun's control server turned the key down."),
            Trouble::Unreachable(why) => write!(f, "Gluetun's control server didn't answer: {why}"),
            Trouble::Refused(why) => write!(f, "Gluetun turned it down: {why}"),
        }
    }
}

impl std::error::Error for Trouble {}

impl Client {
    /// A client for the control server at `base` (e.g. http://gluetun:8000), with Melete's
    /// `key`. Nothing is sent yet.
    ///
    /// # Errors
    ///
    /// When the HTTP client can't be built.
    pub fn new(base: &str, key: Secret) -> anyhow::Result<Self> {
        let http = crate::proxy::Http::through(false, |b| {
            b.timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("melete/", env!("CARGO_PKG_VERSION")))
        })?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            key,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Refused unless the control server wants a key on every route Melete uses: asked without
    /// one, it must turn Melete away (401). Starting and stopping is asked with a status Gluetun
    /// doesn't know, so even a server open to anyone changes nothing; any answer but 401 means
    /// anyone on its network could stop the VPN.
    async fn closed(&self) -> Result<(), Trouble> {
        let probes = [
            self.http.get(self.url("/v1/vpn/status")),
            self.http
                .request(reqwest::Method::PUT, self.url("/v1/vpn/status"))
                .json(&json!({ "status": "melete-checks-a-key-is-needed" })),
        ];
        for probe in probes {
            let answer = probe
                .send()
                .await
                .map_err(|e| Trouble::Unreachable(crate::proxy::said(e)))?;
            match answer.status().as_u16() {
                401 => {}
                404 => return Err(Trouble::Unreachable("it answered 404".into())),
                _ => return Err(Trouble::Open),
            }
        }
        Ok(())
    }

    /// A route's answer as JSON, asked with the key.
    async fn ask(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, Trouble> {
        let mut request = self
            .http
            .request(method, self.url(path))
            .header("X-API-Key", self.key.expose());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let answer = request
            .send()
            .await
            .map_err(|e| Trouble::Unreachable(crate::proxy::said(e)))?;
        let status = answer.status().as_u16();
        let text = answer.text().await.unwrap_or_default();
        match status {
            200..=299 => Ok(serde_json::from_str(&text).unwrap_or(Value::String(text))),
            401 | 403 => Err(Trouble::KeyRefused),
            400 => Err(Trouble::Refused(text.trim().chars().take(200).collect())),
            other => Err(Trouble::Unreachable(format!("it answered {other}"))),
        }
    }

    /// How the VPN is now.
    pub async fn state(&self) -> Result<State, Trouble> {
        self.closed().await?;
        let status = self
            .ask(reqwest::Method::GET, "/v1/vpn/status", None)
            .await?;
        let running = status["status"] == "running";
        let (mut public_ip, mut country, mut forwarded_port) = (None, None, None);
        if running {
            let ip = self
                .ask(reqwest::Method::GET, "/v1/publicip/ip", None)
                .await?;
            public_ip = ip["public_ip"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            country = ip["country"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let ports = self
                .ask(reqwest::Method::GET, "/v1/portforward", None)
                .await?;
            forwarded_port = ports["port"]
                .as_u64()
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0);
        }
        Ok(State {
            running,
            public_ip,
            country,
            forwarded_port,
        })
    }

    /// Starts the tunnel, or stops it. Stopped, Gluetun's firewall lets nothing out, so the
    /// services that go through it wait.
    pub async fn set_running(&self, running: bool) -> Result<(), Trouble> {
        self.closed().await?;
        let status = if running { "running" } else { "stopped" };
        self.ask(
            reqwest::Method::PUT,
            "/v1/vpn/status",
            Some(json!({ "status": status })),
        )
        .await
        .map(|_| ())
    }

    /// Stops the tunnel, then starts it again.
    pub async fn restart(&self) -> Result<(), Trouble> {
        self.set_running(false).await?;
        self.set_running(true).await
    }
}

/// Gluetun's control server as set up (`MELETE_VPN_URL` and `MELETE_VPN_KEY`, both or neither).
#[derive(Debug, Clone)]
pub struct Settings {
    /// Its address as Melete reaches it, e.g. http://gluetun:8000.
    pub url: String,
    /// Melete's key, from Gluetun's roles file.
    pub key: Secret,
}

impl Settings {
    /// The control server `get`'s settings set up, if any; refused when they don't hold
    /// together.
    pub fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<Settings>> {
        let set = |name: &str| {
            get(name)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let (url, key) = match (set("MELETE_VPN_URL"), set("MELETE_VPN_KEY")) {
            (None, None) => return Ok(None),
            (Some(url), Some(key)) => (url, key),
            _ => {
                anyhow::bail!("MELETE_VPN_URL and MELETE_VPN_KEY go together: set both, or neither")
            }
        };
        let parsed = reqwest::Url::parse(&url)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
            .filter(|u| u.username().is_empty() && u.password().is_none() && u.query().is_none())
            .ok_or_else(|| anyhow::anyhow!("MELETE_VPN_URL should be an http address such as http://gluetun:8000, with no login or query"))?;
        Ok(Some(Settings {
            url: parsed.as_str().trim_end_matches('/').to_owned(),
            key: Secret::new(&key),
        }))
    }
}

#[cfg(test)]
pub mod testing;

#[cfg(test)]
mod tests;
