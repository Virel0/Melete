//! A plain connection for what isn't HTTP (the seedbox's SFTP), going the way its service's
//! switch says: direct, or only ever through the proxy, with the same kill switch as the HTTP
//! clients. SOCKS5 has the proxy look the name up; HTTP and HTTPS proxies are asked to CONNECT,
//! an HTTPS one over TLS checked as reqwest checks it.

use std::io;
use std::time::Duration;

use hyper_util::client::legacy::connect::proxy::{SocksV5, Tunnel};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tower::{Service as _, ServiceExt as _};

use super::*;

/// A connection, direct or through the proxy.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

pub type Connection = Box<dyn Stream>;

/// How long reaching the proxy, and its answer to the handshake, may take.
const HANDSHAKE: Duration = Duration::from_secs(20);

/// A connection to `host` and `port` for `service`: through the proxy when its switch is on (and
/// none at all when there's no proxy, or it fails), else direct. A failure at the proxy is an
/// [`Unreachable`] inside the error, named in its words, and noted for admin health.
/// `host` may be a name or an IP address, an IPv6 one with or without brackets.
pub async fn connect(service: Service, host: &str, port: u16) -> io::Result<Connection> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let applied = current();
    if !applied.setup.uses(service) {
        return Ok(Box::new(TcpStream::connect((bare, port)).await?));
    }
    let Some(proxy) = &applied.setup.at else {
        return Err(io::Error::other(Unreachable::NoProxy));
    };
    // A host that's no address isn't the proxy's fault: said as it is, the proxy not asked.
    let target = target(bare, port)?;
    match tokio::time::timeout(HANDSHAKE, through(proxy, target)).await {
        Ok(Ok(connection)) => {
            applied.status.connected();
            Ok(connection)
        }
        // The failure's own words aren't passed on: they can carry the login.
        _ => {
            let bare = proxy.host.trim_start_matches('[').trim_end_matches(']');
            let why = match watch::answers(bare, proxy.port).await {
                true => Unreachable::Through(proxy.address()),
                false => Unreachable::Down(proxy.address()),
            };
            applied.status.failed(&why);
            tracing::warn!("{why}");
            Err(io::Error::other(why))
        }
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Where the proxy is asked to connect: `bare` (an IPv6 address in brackets) at `port`.
fn target(bare: &str, port: u16) -> io::Result<::http::Uri> {
    let host = match bare.parse::<std::net::Ipv6Addr>() {
        Ok(_) => format!("[{bare}]"),
        Err(_) => bare.to_owned(),
    };
    format!("tcp://{host}:{port}").parse().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{bare:?} isn't a host name or an IP address"),
        )
    })
}

/// A connection to `target` through `proxy`, its handshake done.
async fn through(proxy: &Proxy, target: ::http::Uri) -> Result<Connection, BoxError> {
    let to = proxy_uri(proxy)?;
    let reach = reaching(proxy.clone());
    let done = match proxy.kind {
        Kind::Socks5 => {
            let socks = SocksV5::new(to, reach).local_dns(false);
            let mut socks = match &proxy.login {
                Some((user, password)) => socks.with_auth(user.clone(), password.expose().into()),
                None => socks,
            };
            socks.ready().await?.call(target).await?
        }
        Kind::Http | Kind::Https => {
            let tunnel = Tunnel::new(to, reach);
            let mut tunnel = match &proxy.login {
                Some((user, password)) => tunnel.with_auth(basic(user, password.expose())?),
                None => tunnel,
            };
            tunnel.ready().await?.call(target).await?
        }
    };
    Ok(done.into_inner())
}

/// The proxy's own address, as the handshakes take it.
fn proxy_uri(proxy: &Proxy) -> Result<::http::Uri, BoxError> {
    let scheme = match proxy.kind {
        Kind::Socks5 => "socks5h",
        Kind::Http => "http",
        Kind::Https => "https",
    };
    Ok(format!("{scheme}://{}:{}", proxy.host, proxy.port).parse()?)
}

/// `Proxy-Authorization` for a login, marked sensitive.
fn basic(user: &str, password: &str) -> Result<::http::HeaderValue, BoxError> {
    use base64::Engine as _;
    let login = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
    let mut value = ::http::HeaderValue::from_str(&format!("Basic {login}"))?;
    value.set_sensitive(true);
    Ok(value)
}

/// What opens the connection to the proxy itself: plain TCP, and TLS on top for an HTTPS proxy.
fn reaching(
    proxy: Proxy,
) -> impl tower::Service<
    ::http::Uri,
    Response = TokioIo<Connection>,
    Error = BoxError,
    Future = impl Future<Output = Result<TokioIo<Connection>, BoxError>> + Send + 'static,
> + Clone {
    tower::service_fn(move |_: ::http::Uri| {
        let proxy = proxy.clone();
        async move {
            let host = proxy
                .host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_owned();
            let tcp = TcpStream::connect((host.as_str(), proxy.port)).await?;
            let connection: Connection = match proxy.kind {
                Kind::Https => Box::new(tls(host, tcp).await?),
                Kind::Socks5 | Kind::Http => Box::new(tcp),
            };
            Ok(TokioIo::new(connection))
        }
    })
}

/// `tcp` to the proxy at `host`, over TLS checked against the system's roots.
async fn tls(
    host: String,
    tcp: TcpStream,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, BoxError> {
    use rustls_platform_verifier::BuilderVerifierExt as _;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_platform_verifier()?
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host)?;
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    Ok(connector.connect(name, tcp).await?)
}

#[cfg(test)]
mod tests;
