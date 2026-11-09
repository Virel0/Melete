//! Reading the proxy's settings, refused unless they hold together.

use super::*;

impl Setup {
    /// The setup `get`'s settings make. A service switched on with no proxy to go through is
    /// refused rather than left to go direct.
    pub fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<Setup> {
        let set = |name: &str| {
            get(name)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let at = match (set("MELETE_PROXY_HOST"), set("MELETE_PROXY_PORT")) {
            (None, None) => None,
            (Some(host), Some(port)) => Some(Proxy {
                kind: kind(set("MELETE_PROXY_TYPE").as_deref())?,
                host: host_allowed(&host).then_some(host).ok_or_else(|| {
                    anyhow::anyhow!("MELETE_PROXY_HOST should be a name or an IP address alone, such as 192.168.1.10 or [::1], without a scheme, port or path")
                })?,
                port: port.parse().ok().filter(|p| *p > 0).ok_or_else(|| {
                    anyhow::anyhow!("MELETE_PROXY_PORT={port:?} should be a port, 1 to 65535")
                })?,
                login: login(&set)?,
            }),
            _ => anyhow::bail!(
                "MELETE_PROXY_HOST and MELETE_PROXY_PORT go together: set both, or neither"
            ),
        };
        let mut through = Vec::new();
        for service in Service::ALL {
            let name = service.setting();
            match set(name).as_deref() {
                None | Some("off") => {}
                Some("on") if at.is_some() => through.push(service),
                Some("on") => anyhow::bail!(
                    "{name} is on, but no proxy is set: set the proxy's host and port first, as nothing switched on goes direct"
                ),
                Some(other) => anyhow::bail!("{name}={other:?} should be on or off"),
            }
        }
        Ok(Setup { at, through })
    }
}

/// The kind of proxy `MELETE_PROXY_TYPE` names; SOCKS5 when it's unset.
fn kind(set: Option<&str>) -> anyhow::Result<Kind> {
    Ok(match set {
        None | Some("socks5") => Kind::Socks5,
        Some("http") => Kind::Http,
        Some("https") => Kind::Https,
        Some(other) => {
            anyhow::bail!("MELETE_PROXY_TYPE={other:?} should be socks5, http or https")
        }
    })
}

/// Whether `host` is a name or an address alone: no scheme, port, path or login.
fn host_allowed(host: &str) -> bool {
    let bracketed = host.starts_with('[') && host.ends_with(']') && host.len() > 2;
    match bracketed {
        true => host[1..host.len() - 1]
            .parse::<std::net::Ipv6Addr>()
            .is_ok(),
        false => {
            host.len() <= 253
                && host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        }
    }
}

/// The proxy's login, when a username is set; SOCKS5 takes at most 255 bytes of each.
fn login(set: &impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<(String, Secret)>> {
    match (set("MELETE_PROXY_USERNAME"), set("MELETE_PROXY_PASSWORD")) {
        (None, None) => Ok(None),
        (None, Some(_)) => anyhow::bail!("MELETE_PROXY_PASSWORD needs MELETE_PROXY_USERNAME too"),
        (Some(user), password) => {
            let password = password.unwrap_or_default();
            if user.len() > 255 || password.len() > 255 {
                anyhow::bail!("the proxy's username and password take at most 255 bytes each");
            }
            Ok(Some((user, Secret::new(&password))))
        }
    }
}
