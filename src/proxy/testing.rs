//! Stand-ins for tests: an HTTP proxy and a SOCKS5 one that note what they're asked and answer
//! every request with the same body, proxies that relay plain connections, a plain site that
//! counts its requests, and a way to build clients with a setup of the test's own.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;

/// What a stand-in was asked.
#[derive(Debug, Default)]
pub struct Seen {
    /// Each HTTP request's first line, e.g. "GET http://tmdb.example/3/movie/1 HTTP/1.1".
    pub requests: Vec<String>,
    /// Each HTTP request's Proxy-Authorization header, if any.
    pub authorizations: Vec<Option<String>>,
    /// Each SOCKS5 connect: the address type (3 a name, 1 IPv4, 4 IPv6), the address, the port.
    pub socks: Vec<(u8, String, u16)>,
    /// Each SOCKS5 login, user and password.
    pub logins: Vec<(String, String)>,
}

pub type Log = Arc<Mutex<Seen>>;

/// Runs `test` with `setup` in use for the clients it builds, and how the proxy answered.
pub async fn with<F: Future>(setup: Setup, test: F) -> (F::Output, Arc<Applied>) {
    let in_use = Arc::new(Mutex::new(Arc::new(Applied::new(setup))));
    let output = TESTING.scope(in_use.clone(), test).await;
    let applied = in_use.lock().unwrap().clone();
    (output, applied)
}

/// Puts `setup` in use in place of the test's, as saving the settings does for the server.
pub fn switch(setup: Setup) {
    TESTING.with(|in_use| *in_use.lock().unwrap() = Arc::new(Applied::new(setup)));
}

/// [`switch`] for another task, such as a stand-in's handler, to call while the test runs.
pub fn switcher() -> impl Fn(Setup) + Send + Sync + 'static {
    let in_use = TESTING.with(Arc::clone);
    move |setup| *in_use.lock().unwrap() = Arc::new(Applied::new(setup))
}

/// A proxy of `kind` at `at`, without a login.
pub fn proxy(kind: Kind, at: SocketAddr) -> Proxy {
    Proxy {
        kind,
        host: at.ip().to_string(),
        port: at.port(),
        login: None,
    }
}

/// An address nothing listens on.
pub async fn nothing_at() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap()
}

async fn listen() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = listener.local_addr().unwrap();
    (listener, at)
}

/// Reads one HTTP request's head, returning its lines.
async fn read_head(stream: &mut TcpStream) -> Vec<String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && stream.read_exact(&mut byte).await.is_ok() {
        head.push(byte[0]);
    }
    String::from_utf8_lossy(&head)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Reads one HTTP request's head, and answers it with `body`. Returns the head's lines.
async fn answer(stream: &mut TcpStream, body: &str) -> Vec<String> {
    let lines = read_head(stream).await;
    let reply = format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes()).await;
    lines
}

fn note_http(log: &Log, lines: &[String]) {
    let mut seen = log.lock().unwrap();
    seen.requests
        .push(lines.first().cloned().unwrap_or_default());
    seen.authorizations.push(
        lines
            .iter()
            .find_map(|l| {
                l.strip_prefix("proxy-authorization: ")
                    .or(l.strip_prefix("Proxy-Authorization: "))
            })
            .map(str::to_owned),
    );
}

/// An HTTP proxy answering every request with `body` itself, and where it listens.
pub async fn http_proxy(body: &'static str) -> (SocketAddr, Log) {
    let (listener, at) = listen().await;
    let log = Log::default();
    let noted = log.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let lines = answer(&mut stream, body).await;
            note_http(&noted, &lines);
        }
    });
    (at, log)
}

/// A plain site answering every request with `body`, counting them.
pub async fn site(body: &'static str) -> (SocketAddr, Log) {
    http_proxy(body).await
}

/// A SOCKS5 proxy that wants `login` when given, and answers every request sent through it
/// with `body` itself.
pub async fn socks5(
    login: Option<(&'static str, &'static str)>,
    body: &'static str,
) -> (SocketAddr, Log) {
    let (listener, at) = listen().await;
    let log = Log::default();
    let noted = log.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            if let Some(()) = socks_handshake(&mut stream, login, &noted).await {
                let lines = answer(&mut stream, body).await;
                note_http(&noted, &lines);
            }
        }
    });
    (at, log)
}

/// A proxy that relays each connection it's asked for (SOCKS5's connect, or HTTP's CONNECT) to
/// the IP address asked, or to 127.0.0.1 for any name, so a name only the proxy knows works. A
/// `login` it wants is checked; an HTTP one turned down gets a 407.
pub async fn relay(kind: Kind, login: Option<(&'static str, &'static str)>) -> (SocketAddr, Log) {
    let (listener, at) = listen().await;
    let log = Log::default();
    let noted = log.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let noted = noted.clone();
            tokio::spawn(async move {
                let Some(to) = relay_handshake(kind, &mut stream, login, &noted).await else {
                    return;
                };
                if let Ok(mut there) = TcpStream::connect(to).await {
                    let _ = tokio::io::copy_bidirectional(&mut stream, &mut there).await;
                }
            });
        }
    });
    (at, log)
}

/// Where a relaying proxy was asked to connect, its handshake answered; None if refused.
async fn relay_handshake(
    kind: Kind,
    stream: &mut TcpStream,
    login: Option<(&str, &str)>,
    log: &Log,
) -> Option<SocketAddr> {
    let at = |host: &str, port: u16| {
        let ip = host.trim_start_matches('[').trim_end_matches(']').parse();
        SocketAddr::new(ip.unwrap_or(std::net::Ipv4Addr::LOCALHOST.into()), port)
    };
    if kind == Kind::Socks5 {
        socks_handshake(stream, login, log).await?;
        let seen = log.lock().unwrap();
        return seen.socks.last().map(|(_, host, port)| at(host, *port));
    }
    let lines = read_head(stream).await;
    note_http(log, &lines);
    let wanted = login.map(|(user, password)| {
        use base64::Engine as _;
        let pair = format!("{user}:{password}");
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(pair)
        )
    });
    let given = log.lock().unwrap().authorizations.last().cloned().flatten();
    if wanted.is_some() && given != wanted {
        let _ = stream
            .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
            .await;
        return None;
    }
    let target = lines.first()?.strip_prefix("CONNECT ")?.split(' ').next()?;
    let (host, port) = target.rsplit_once(':')?;
    let to = at(host, port.parse().ok()?);
    stream
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await
        .ok()?;
    Some(to)
}

async fn read_vec(stream: &mut TcpStream, n: usize) -> Option<Vec<u8>> {
    let mut bytes = vec![0u8; n];
    stream.read_exact(&mut bytes).await.ok()?;
    Some(bytes)
}

async fn socks_handshake(
    stream: &mut TcpStream,
    login: Option<(&str, &str)>,
    log: &Log,
) -> Option<()> {
    let greeting = read_vec(stream, 2).await?;
    let methods = read_vec(stream, usize::from(greeting[1])).await?;
    let method = if login.is_some() { 2 } else { 0 };
    if greeting[0] != 5 || !methods.contains(&method) {
        stream.write_all(&[5, 0xff]).await.ok()?;
        return None;
    }
    stream.write_all(&[5, method]).await.ok()?;
    if let Some((user, password)) = login {
        let version = read_vec(stream, 2).await?;
        let got_user = read_vec(stream, usize::from(version[1])).await?;
        let length = read_vec(stream, 1).await?;
        let got_password = read_vec(stream, usize::from(length[0])).await?;
        let (got_user, got_password) = (
            String::from_utf8_lossy(&got_user).into_owned(),
            String::from_utf8_lossy(&got_password).into_owned(),
        );
        let right = got_user == user && got_password == password;
        log.lock().unwrap().logins.push((got_user, got_password));
        stream
            .write_all(&[1, if right { 0 } else { 1 }])
            .await
            .ok()?;
        if !right {
            return None;
        }
    }
    let request = read_vec(stream, 4).await?;
    let address = match request[3] {
        3 => {
            let length = read_vec(stream, 1).await?;
            String::from_utf8_lossy(&read_vec(stream, usize::from(length[0])).await?).into_owned()
        }
        1 => std::net::Ipv4Addr::from(<[u8; 4]>::try_from(read_vec(stream, 4).await?).ok()?)
            .to_string(),
        _ => std::net::Ipv6Addr::from(<[u8; 16]>::try_from(read_vec(stream, 16).await?).ok()?)
            .to_string(),
    };
    let port = read_vec(stream, 2).await?;
    log.lock()
        .unwrap()
        .socks
        .push((request[3], address, u16::from_be_bytes([port[0], port[1]])));
    stream
        .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])
        .await
        .ok()?;
    Some(())
}
