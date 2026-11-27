//! HTTPS served by Melete itself: on its own address, with a certificate and key from files.
//! Off unless all three are set. A proxy in front may pass the encrypted connection through as
//! it is and say who the client is with the PROXY protocol, believed only from the proxies
//! trusted for `X-Forwarded-For` (`MELETE_TRUSTED_PROXIES`).

use super::*;

/// HTTPS, as set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tls {
    /// `MELETE_TLS_BIND`: where HTTPS listens, e.g. 0.0.0.0:8443. Plain HTTP stays on
    /// `MELETE_BIND`.
    pub bind: SocketAddr,
    /// `MELETE_TLS_CERT`: the certificate chain, PEM, leaf first, as Melete's container sees it.
    pub cert: PathBuf,
    /// `MELETE_TLS_KEY`: its private key, PEM.
    pub key: PathBuf,
    /// `MELETE_TLS_PROXY_PROTOCOL=on`: connections from a trusted proxy begin with a PROXY
    /// protocol header (v1 or v2) saying who the client is.
    pub proxy_protocol: bool,
}

const NAMES: [&str; 3] = ["MELETE_TLS_BIND", "MELETE_TLS_CERT", "MELETE_TLS_KEY"];

pub(super) fn tls(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<Tls>> {
    let value = |key: &str| {
        get(key)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let proxy_protocol = switch(get, "MELETE_TLS_PROXY_PROTOCOL")?;
    let set: Vec<Option<String>> = NAMES.iter().map(|k| value(k)).collect();
    let (Some(bind), Some(cert), Some(key)) = (&set[0], &set[1], &set[2]) else {
        if let Some((name, _)) = NAMES.iter().zip(&set).find(|(_, v)| v.is_some()) {
            bail!(
                "{name} is set, so MELETE_TLS_BIND, MELETE_TLS_CERT and MELETE_TLS_KEY are all needed"
            );
        }
        anyhow::ensure!(
            !proxy_protocol,
            "MELETE_TLS_PROXY_PROTOCOL is on, but HTTPS isn't set up (MELETE_TLS_BIND, MELETE_TLS_CERT, MELETE_TLS_KEY)"
        );
        return Ok(None);
    };
    let bind: SocketAddr = bind
        .parse()
        .with_context(|| format!("MELETE_TLS_BIND={bind:?} is not an address like 0.0.0.0:8443"))?;
    let (cert, key) = (PathBuf::from(cert), PathBuf::from(key));
    anyhow::ensure!(
        cert.is_absolute() && key.is_absolute(),
        "MELETE_TLS_CERT and MELETE_TLS_KEY are paths from the root, as Melete's container sees them"
    );
    Ok(Some(Tls {
        bind,
        cert,
        key,
        proxy_protocol,
    }))
}

/// HTTPS as set, where Melete gets its certificate itself when `acme` (the folder that
/// certificate is kept in) is given: then only `MELETE_TLS_BIND` is set, and a certificate and
/// key given too are refused, as two would disagree.
pub(super) fn tls_with(
    get: &impl Fn(&str) -> Option<String>,
    acme: Option<&std::path::Path>,
) -> anyhow::Result<Option<Tls>> {
    let Some(folder) = acme else {
        return tls(get);
    };
    let value = |key: &str| {
        get(key)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    anyhow::ensure!(
        value("MELETE_TLS_CERT").is_none() && value("MELETE_TLS_KEY").is_none(),
        "Certificates is on, so Melete gets HTTPS's certificate itself, but MELETE_TLS_CERT or MELETE_TLS_KEY is set too: remove them from .env, or turn Certificates off"
    );
    let proxy_protocol = switch(get, "MELETE_TLS_PROXY_PROTOCOL")?;
    let Some(bind) = value("MELETE_TLS_BIND") else {
        anyhow::ensure!(
            !proxy_protocol,
            "MELETE_TLS_PROXY_PROTOCOL is on, but HTTPS isn't set up (MELETE_TLS_BIND)"
        );
        return Ok(None);
    };
    let bind: SocketAddr = bind
        .parse()
        .with_context(|| format!("MELETE_TLS_BIND={bind:?} is not an address like 0.0.0.0:8443"))?;
    Ok(Some(Tls {
        bind,
        cert: folder.join(super::acme::CERT_FILE),
        key: folder.join(super::acme::KEY_FILE),
        proxy_protocol,
    }))
}
