//! HTTPS served by Melete itself (`MELETE_TLS_*`), beside plain HTTP: the same service, over
//! TLS, with a certificate and key read from files and read again when they change (a renewal
//! needs no restart). A proxy in front can then pass connections through as they are, never
//! able to read them, and say who each client is with the PROXY protocol: believed only from a
//! trusted proxy (`MELETE_TRUSTED_PROXIES`), and required of one once it's on.
//!
//! Each connection has its PROXY header and its handshake in a few seconds or is dropped, and
//! no more than [`MOST_CONNECTIONS`] are open at once, so a flood can't wear Melete down.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, SystemTime};

use anyhow::Context;
use axum::Router;
use axum::extract::ConnectInfo;
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

use crate::config::Tls;
use crate::ways_in::{Net, ProxiedClient};

pub mod proxy_header;

/// Connections open at once at most.
pub const MOST_CONNECTIONS: usize = 1024;
/// How long a PROXY header, a handshake and a request's headers may take.
const HEADER_LIMIT: Duration = Duration::from_secs(5);
const HANDSHAKE_LIMIT: Duration = Duration::from_secs(10);
const REQUEST_HEADERS_LIMIT: Duration = Duration::from_secs(30);
/// How often the certificate files are looked at for a change.
const RELOAD_EVERY: Duration = Duration::from_secs(10 * 60);

/// When the certificate's and key's files last changed, as read.
type Versions = Option<(SystemTime, SystemTime)>;

/// The certificate in use, read again when its files change.
#[derive(Debug)]
struct Certificate {
    cert: PathBuf,
    key: PathBuf,
    current: RwLock<(Arc<CertifiedKey>, Versions)>,
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

fn changed_at(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Reads a certificate chain and its key; says which file is wrong, never what's in it.
fn load(cert: &Path, key: &Path) -> anyhow::Result<CertifiedKey> {
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
        .with_context(|| format!("{} can't be read as PEM", cert.display()))?
        .collect::<Result<_, _>>()
        .with_context(|| {
            format!(
                "{} holds something that isn't a certificate",
                cert.display()
            )
        })?;
    anyhow::ensure!(!chain.is_empty(), "{} holds no certificate", cert.display());
    let private = PrivateKeyDer::from_pem_file(key)
        .with_context(|| format!("{} holds no private key Melete can read", key.display()))?;
    let signing = provider()
        .key_provider
        .load_private_key(private)
        .with_context(|| format!("{}'s key isn't one Melete can sign with", key.display()))?;
    let certified = CertifiedKey::new(chain, signing);
    certified
        .keys_match()
        .with_context(|| format!("{} isn't the key of {}", key.display(), cert.display()))?;
    Ok(certified)
}

impl Certificate {
    fn open(tls: &Tls) -> anyhow::Result<Self> {
        let first = load(&tls.cert, &tls.key)?;
        Ok(Certificate {
            cert: tls.cert.clone(),
            key: tls.key.clone(),
            current: RwLock::new((Arc::new(first), Self::versions(&tls.cert, &tls.key))),
        })
    }

    fn versions(cert: &Path, key: &Path) -> Versions {
        Some((changed_at(cert)?, changed_at(key)?))
    }

    /// Reads the files again if either changed; a pair that doesn't load keeps the one in use.
    fn reload(&self) {
        let now = Self::versions(&self.cert, &self.key);
        let seen = self
            .current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .1;
        if now.is_none() || now == seen {
            return;
        }
        match load(&self.cert, &self.key) {
            Ok(fresh) => {
                *self.current.write().unwrap_or_else(PoisonError::into_inner) =
                    (Arc::new(fresh), now);
                tracing::info!("HTTPS took up its renewed certificate");
            }
            Err(err) => tracing::warn!(
                error = format!("{err:#}"),
                "HTTPS keeps its certificate: the changed files don't load"
            ),
        }
    }
}

impl ResolvesServerCert for Certificate {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(
            self.current
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .0
                .clone(),
        )
    }
}

/// HTTPS, ready to serve: its certificate read (a wrong one stops Melete starting, so it's
/// seen at once) and its address taken.
pub struct Ready {
    listener: TcpListener,
    acceptor: TlsAcceptor,
    certificate: Arc<Certificate>,
    proxy_protocol: bool,
}

impl Ready {
    /// Where it listens (the port, when 0 was asked for).
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

/// Reads the certificate and takes HTTPS's address.
pub async fn prepare(tls: &Tls) -> anyhow::Result<Ready> {
    let certificate = Arc::new(Certificate::open(tls)?);
    let mut config = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_cert_resolver(certificate.clone());
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let listener = TcpListener::bind(tls.bind)
        .await
        .with_context(|| format!("HTTPS can't listen on {}", tls.bind))?;
    Ok(Ready {
        listener,
        acceptor: TlsAcceptor::from(Arc::new(config)),
        certificate,
        proxy_protocol: tls.proxy_protocol,
    })
}

/// Serves `router` over HTTPS for as long as the server runs.
pub async fn serve(ready: Ready, router: Router, trusted: Vec<Net>) {
    let Ready {
        listener,
        acceptor,
        certificate,
        proxy_protocol,
    } = ready;
    let watched = certificate.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(RELOAD_EVERY).await;
            let certificate = watched.clone();
            let _ = tokio::task::spawn_blocking(move || certificate.reload()).await;
        }
    });
    let trusted = Arc::new(trusted);
    let slots = Arc::new(Semaphore::new(MOST_CONNECTIONS));
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(err) => {
                tracing::warn!(error = %err, "HTTPS couldn't take a connection");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        // Full: the newest waits on nobody, and is let go.
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let (acceptor, router, trusted) = (acceptor.clone(), router.clone(), trusted.clone());
        tokio::spawn(async move {
            let _slot = slot;
            let proxied = proxy_protocol && trusted.iter().any(|n| n.contains(peer.ip()));
            if let Err(why) = connection(stream, peer, proxied, acceptor, router).await {
                tracing::debug!(%peer, why, "an HTTPS connection ended early");
            }
        });
    }
}

/// One connection: its PROXY header when it comes from a trusted proxy, its handshake, then
/// its requests, each with the connection's address and the client the header named.
async fn connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    proxied: bool,
    acceptor: TlsAcceptor,
    router: Router,
) -> Result<(), String> {
    let client: Option<IpAddr> = if proxied {
        tokio::time::timeout(HEADER_LIMIT, proxy_header::read(&mut stream))
            .await
            .map_err(|_| "no PROXY header in time".to_owned())?
            .map_err(|bad| format!("a PROXY header that isn't one: {bad:?}"))?
    } else {
        None
    };
    let tls = tokio::time::timeout(HANDSHAKE_LIMIT, acceptor.accept(stream))
        .await
        .map_err(|_| "no handshake in time".to_owned())?
        .map_err(|err| format!("handshake: {err}"))?;
    let service = router.map_request(move |mut request: axum::http::Request<Incoming>| {
        request.extensions_mut().insert(ConnectInfo(peer));
        if let Some(client) = client {
            request.extensions_mut().insert(ProxiedClient(client));
        }
        request
    });
    hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(REQUEST_HEADERS_LIMIT)
        .serve_connection(TokioIo::new(tls), TowerToHyperService::new(service))
        .with_upgrades()
        .await
        .map_err(|err| format!("serving: {err}"))
}

#[cfg(test)]
mod tests;
