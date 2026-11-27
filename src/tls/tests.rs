use std::sync::Arc;

use axum::extract::Extension;
use axum::routing::get;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::ways_in::{ClientAddr, WaysIn, parse_nets};

/// A throwaway certificate for localhost, written as PEM files in `dir`; its DER too.
fn certificate(dir: &Path, name: &str) -> (PathBuf, PathBuf, CertificateDer<'static>) {
    let made = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let (cert, key) = (
        dir.join(format!("{name}.crt")),
        dir.join(format!("{name}.key")),
    );
    std::fs::write(&cert, made.cert.pem()).unwrap();
    std::fs::write(&key, made.signing_key.serialize_pem()).unwrap();
    (cert, key, made.cert.der().clone())
}

/// HTTPS on a port of its own, serving who each request's client is as the ways in see it.
async fn https(
    dir: &Path,
    proxy_protocol: bool,
    trusted: &str,
) -> (SocketAddr, CertificateDer<'static>) {
    let (cert, key, der) = certificate(dir, "melete");
    let tls = Tls {
        bind: "127.0.0.1:0".parse().unwrap(),
        cert,
        key,
        proxy_protocol,
    };
    let ready = prepare(&tls).await.unwrap();
    let at = ready.local_addr().unwrap();
    let ways = Arc::new(WaysIn {
        ways: None,
        proxies: parse_nets(trusted).unwrap(),
    });
    let router =
        Router::new()
            .route(
                "/who",
                get(
                    |Extension(ClientAddr(client)): Extension<ClientAddr>| async move {
                        client.to_string()
                    },
                ),
            )
            .layer(axum::middleware::from_fn_with_state(
                ways.clone(),
                crate::ways_in::guard,
            ));
    tokio::spawn(serve(ready, router, ways.proxies.clone()));
    (at, der)
}

/// Asks for `/who` over TLS, sending `before` first (a PROXY header); the answer's body, or
/// why there was none.
async fn who(
    at: SocketAddr,
    root: &CertificateDer<'static>,
    before: &[u8],
) -> Result<String, String> {
    who_with(at, root, before, "").await
}

/// The same, with extra request headers (each line ending CRLF).
async fn who_with(
    at: SocketAddr,
    root: &CertificateDer<'static>,
    before: &[u8],
    headers: &str,
) -> Result<String, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(root.clone()).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut tcp = TcpStream::connect(at).await.map_err(|e| e.to_string())?;
    tcp.write_all(before).await.map_err(|e| e.to_string())?;
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let name = ServerName::try_from("localhost").unwrap();
    let mut tls = connector
        .connect(name, tcp)
        .await
        .map_err(|e| e.to_string())?;
    let request =
        format!("GET /who HTTP/1.1\r\nHost: localhost\r\n{headers}Connection: close\r\n\r\n");
    tls.write_all(request.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let mut answer = String::new();
    tls.read_to_string(&mut answer)
        .await
        .map_err(|e| e.to_string())?;
    answer
        .split_once("\r\n\r\n")
        .filter(|(head, _)| head.starts_with("HTTP/1.1 200"))
        .map(|(_, body)| body.to_owned())
        .ok_or(answer)
}

const PROXIED: &[u8] = b"PROXY TCP4 203.0.113.9 127.0.0.1 56324 8443\r\n";

#[tokio::test]
async fn https_serves_melete_with_the_client_its_trusted_proxy_names() {
    let dir = tempfile::tempdir().unwrap();
    // Straight in: the connection's own address.
    let (at, root) = https(dir.path(), false, "").await;
    assert_eq!(who(at, &root, b"").await.unwrap(), "127.0.0.1");
    // Through a trusted proxy passing it through: the client its PROXY header names.
    let dir = tempfile::tempdir().unwrap();
    let (at, root) = https(dir.path(), true, "127.0.0.1").await;
    assert_eq!(who(at, &root, PROXIED).await.unwrap(), "203.0.113.9");
    // A trusted proxy that sends no header isn't served.
    assert!(who(at, &root, b"").await.is_err());
}

#[tokio::test]
async fn a_proxy_header_from_anyone_not_trusted_is_never_believed() {
    let dir = tempfile::tempdir().unwrap();
    let (at, root) = https(dir.path(), true, "192.168.1.10").await;
    // Not from the trusted proxy: the header isn't read, and isn't TLS, so nothing's served.
    assert!(who(at, &root, PROXIED).await.is_err());
    assert_eq!(who(at, &root, b"").await.unwrap(), "127.0.0.1");
}

#[test]
fn a_renewed_certificate_is_taken_up_and_a_broken_one_isnt() {
    let dir = tempfile::tempdir().unwrap();
    let (cert, key, _) = certificate(dir.path(), "melete");
    let tls = Tls {
        bind: "127.0.0.1:0".parse().unwrap(),
        cert: cert.clone(),
        key: key.clone(),
        proxy_protocol: false,
    };
    let certificate_in_use = Certificate::open(&tls).unwrap();
    let first = certificate_in_use.current.read().unwrap().0.clone();
    // Unchanged files: nothing read.
    certificate_in_use.reload();
    assert!(Arc::ptr_eq(
        &first,
        &certificate_in_use.current.read().unwrap().0
    ));
    // A renewal: new files, later times.
    std::thread::sleep(Duration::from_millis(20));
    let (new_cert, new_key, _) = certificate(dir.path(), "renewed");
    std::fs::rename(new_cert, &cert).unwrap();
    std::fs::rename(new_key, &key).unwrap();
    certificate_in_use.reload();
    let second = certificate_in_use.current.read().unwrap().0.clone();
    assert!(!Arc::ptr_eq(&first, &second), "the renewal is taken up");
    // A broken one keeps the certificate in use.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&cert, "not a certificate").unwrap();
    certificate_in_use.reload();
    assert!(Arc::ptr_eq(
        &second,
        &certificate_in_use.current.read().unwrap().0
    ));
}

#[test]
fn a_wrong_certificate_says_which_file_and_never_what_it_holds() {
    let dir = tempfile::tempdir().unwrap();
    let (cert, _, _) = certificate(dir.path(), "one");
    let (_, other_key, _) = certificate(dir.path(), "two");
    let mismatched = load(&cert, &other_key).unwrap_err().to_string();
    assert!(mismatched.contains("isn't the key of"), "{mismatched}");
    let missing = load(&dir.path().join("none.crt"), &other_key).unwrap_err();
    assert!(format!("{missing:#}").contains("none.crt"));
    let key_text = std::fs::read_to_string(&other_key).unwrap();
    std::fs::write(dir.path().join("bad.crt"), "garbage").unwrap();
    let bad = format!(
        "{:#}",
        load(&dir.path().join("bad.crt"), &other_key).unwrap_err()
    );
    assert!(
        !bad.contains("PRIVATE KEY") && !bad.contains(&key_text[30..60]),
        "{bad}"
    );
}

#[tokio::test]
async fn on_https_a_client_never_names_itself() {
    let dir = tempfile::tempdir().unwrap();
    let (at, root) = https(dir.path(), true, "127.0.0.1").await;
    // A trusted proxy's header naming nobody (a health check): the connection's own address,
    // never what X-Forwarded-For says, which only the client could have written.
    let forged = "X-Forwarded-For: 198.51.100.77\r\n";
    assert_eq!(
        who_with(at, &root, b"PROXY UNKNOWN\r\n", forged)
            .await
            .unwrap(),
        "127.0.0.1"
    );
    assert_eq!(
        who_with(at, &root, PROXIED, forged).await.unwrap(),
        "203.0.113.9"
    );
    // Without the PROXY protocol too.
    let dir = tempfile::tempdir().unwrap();
    let (at, root) = https(dir.path(), false, "127.0.0.1").await;
    assert_eq!(who_with(at, &root, b"", forged).await.unwrap(), "127.0.0.1");
}

#[test]
fn one_client_has_at_most_its_share_of_connections() {
    let client: IpAddr = "198.51.100.200".parse().unwrap();
    let held: Vec<PerClient> = (0..PER_CLIENT)
        .map(|_| PerClient::take(client).unwrap())
        .collect();
    assert!(PerClient::take(client).is_none(), "one more is refused");
    assert!(
        PerClient::take("198.51.100.201".parse().unwrap()).is_some(),
        "others aren't held up"
    );
    drop(held);
    assert!(
        PerClient::take(client).is_some(),
        "places come back as connections close"
    );
}
