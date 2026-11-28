//! The TLS-ALPN challenge, through HTTPS as a CA reaches it behind a passthrough: after a
//! trusted proxy's PROXY header.

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::acme::testing::alpn::{acme_identifier, shown};
use crate::config::Challenge;
use crate::tls::challenge;

const PROXIED: &[u8] = b"PROXY TCP4 203.0.113.9 127.0.0.1 56324 443\r\n";

/// HTTPS on a port of its own, taking PROXY headers from this machine, serving `/who`.
async fn https(dir: &Path) -> SocketAddr {
    let tls = Tls {
        bind: "127.0.0.1:0".parse().unwrap(),
        cert: dir.join(crate::config::CERT_FILE),
        key: dir.join(crate::config::KEY_FILE),
        proxy_protocol: true,
    };
    let ready = crate::tls::prepare_awaiting(&tls).await.unwrap();
    let at = ready.local_addr().unwrap();
    let router = axum::Router::new().route("/who", axum::routing::get(|| async { "melete" }));
    let trusted = crate::ways_in::parse_nets("127.0.0.1").unwrap();
    tokio::spawn(crate::tls::serve(ready, router, trusted));
    at
}

/// What HTTPS answers a request for `/who` sent over a handshake offering `alpn` alone.
async fn asked_over(at: SocketAddr, name: &str, alpn: &[u8]) -> String {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let roots = rustls::RootCertStore::empty();
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![alpn.to_vec()];
    // The challenge certificate is self-signed: take it, to see what's served after.
    config
        .dangerous()
        .set_certificate_verifier(crate::acme::testing::alpn::any());
    let mut tcp = tokio::net::TcpStream::connect(at).await.unwrap();
    tcp.write_all(PROXIED).await.unwrap();
    let server = ServerName::try_from(name.to_owned()).unwrap();
    let Ok(mut tls) = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(server, tcp)
        .await
    else {
        return "no handshake".to_owned();
    };
    let _ = tls
        .write_all(b"GET /who HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await;
    let mut answer = String::new();
    let _ = tls.read_to_string(&mut answer).await;
    answer
}

#[tokio::test]
async fn the_challenge_certificate_goes_to_acme_tls_alone_while_its_order_waits() {
    let dir = tempfile::tempdir().unwrap();
    let at = https(dir.path()).await;
    let name = "alpn-one.example.org";
    let acme_tls: &[&[u8]] = &[b"acme-tls/1"];
    // Nothing waiting: a CA's handshake fails, rather than getting HTTPS's own certificate.
    assert!(shown(at, PROXIED, name, acme_tls).await.is_err());
    let digest = [7u8; 32];
    let offered = challenge::offer(name, challenge::made(name, &digest).unwrap());
    // Behind the PROXY header, as through a passthrough: the challenge certificate.
    let (leaf, agreed) = shown(at, PROXIED, name, acme_tls).await.unwrap();
    assert_eq!(acme_identifier(&leaf), Some(digest.to_vec()));
    assert_eq!(agreed.as_deref(), Some(&b"acme-tls/1"[..]));
    // Nothing is served on that connection.
    let answer = asked_over(at, name, b"acme-tls/1").await;
    assert!(!answer.contains("melete"), "{answer}");
    // Every other client gets HTTPS's own certificate, even one offering acme-tls/1 too.
    for alpn in [&[&b"http/1.1"[..]][..], &[b"http/1.1", b"acme-tls/1"]] {
        let (leaf, agreed) = shown(at, PROXIED, name, alpn).await.unwrap();
        assert_eq!(acme_identifier(&leaf), None, "{alpn:?}");
        assert_eq!(agreed.as_deref(), Some(&b"http/1.1"[..]));
    }
    assert!(asked_over(at, name, b"http/1.1").await.contains("melete"));
    // Another name gets none, nor does a trusted proxy's connection without its header.
    assert!(
        shown(at, PROXIED, "alpn-other.example.org", acme_tls)
            .await
            .is_err()
    );
    assert!(shown(at, b"", name, acme_tls).await.is_err());
    // Taken back once the order is done.
    drop(offered);
    assert!(shown(at, PROXIED, name, acme_tls).await.is_err());
}

#[tokio::test]
async fn tls_alpn_gets_the_exact_name_through_the_passthrough() {
    let mut world = World::new().await;
    let at = https(&world.folder()).await;
    world.acme.challenge = Challenge::TlsAlpn;
    world.acme.domain = "alpn-watch.example.org".into();
    world.acme.desec_token = None;
    {
        let mut ca = world.ca.lock().unwrap();
        ca.tls_at = Some(at);
        ca.tls_before = PROXIED.to_vec();
    }
    world.round(false).await;
    let held = world.held();
    assert_eq!(held.names, ["alpn-watch.example.org"]);
    assert!(
        world.desec.lock().unwrap().writes.is_empty(),
        "nothing sent to deSEC"
    );
    assert_eq!(world.state().await.failures, 0);
    // The challenge certificate is gone once the order is done.
    let acme_tls: &[&[u8]] = &[b"acme-tls/1"];
    assert!(
        shown(at, PROXIED, "alpn-watch.example.org", acme_tls)
            .await
            .is_err()
    );
    // And HTTPS serves what it got, to an ordinary client, without a restart.
    let kept = CertificateDer::pem_file_iter(world.folder().join(crate::config::CERT_FILE))
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    let mut served = None;
    for _ in 0..50 {
        let (leaf, _) = shown(at, PROXIED, "alpn-watch.example.org", &[b"http/1.1"])
            .await
            .unwrap();
        if leaf == kept {
            served = Some(leaf);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(served.is_some(), "the certificate got is served");
    // The names shown are the one asked for, and the CAA record is for it alone: no
    // issuewild, as no wildcard is asked for.
    let mut config = crate::config::Config::from_env_and(|_: &str| None).unwrap();
    config.acme = Some(world.acme.clone());
    let shown = crate::acme::status(&world.pool, &config).await.unwrap();
    assert_eq!(shown.names, ["alpn-watch.example.org"]);
    assert_eq!(shown.caa.len(), 1, "{:?}", shown.caa);
    assert!(
        shown.caa[0].starts_with("alpn-watch.example.org. CAA 0 issue \""),
        "{:?}",
        shown.caa
    );
    // Its Test asks nothing of deSEC.
    let (fine, said) = crate::acme::test::zone_takes(&world.acme, &world.reach).await;
    assert!(fine && said.contains("needs no token"), "{said}");
    assert!(world.desec.lock().unwrap().paths.is_empty());
}

#[tokio::test]
async fn the_renew_guards_hold_for_tls_alpn_too() {
    let mut world = World::new().await;
    world.acme.domain = "alpn-guard.example.org".into();
    world.ca.lock().unwrap().refuse_orders = Some("no".into());
    assert!(world.round(false).await > unix_now());
    assert_eq!(world.state().await.failures, 1);
    // Switched to TLS-ALPN for the same domain: the DNS challenge's wait isn't carried over.
    world.ca.lock().unwrap().refuse_orders = None;
    let at = https(&world.folder()).await;
    world.acme.challenge = Challenge::TlsAlpn;
    world.acme.desec_token = None;
    {
        let mut ca = world.ca.lock().unwrap();
        ca.tls_at = Some(at);
        ca.tls_before = PROXIED.to_vec();
    }
    world.round(false).await;
    let held = world.held();
    assert_eq!(held.names, ["alpn-guard.example.org"]);
    assert_eq!(world.state().await.failures, 0);
    // A certificate it just got, for the same names from the same CA, isn't asked for again.
    let state = world.state().await;
    let fits = held.is_for(&world.acme.names());
    let why = renewal::ask_refused(&state, Some((&held, fits)), false, unix_now()).unwrap();
    assert!(why.contains("less than a day old"), "{why}");
    // A failed renewal by TLS-ALPN waits before it's tried again, like any other.
    world.ca.lock().unwrap().refuse_orders = Some("no".into());
    let retry_at = world.round(true).await;
    assert!(retry_at > unix_now());
    assert_eq!(world.state().await.failures, 1);
    assert_eq!(world.round(false).await, retry_at, "not tried again yet");
    assert_eq!(world.state().await.failures, 1);
}

#[test]
fn tls_alpn_needs_https_and_no_token() {
    let read = |pairs: &'static [(&'static str, &'static str)]| {
        crate::config::Config::from_env_and(move |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        })
    };
    let without_https = read(&[
        ("MELETE_ACME_DOMAIN", "watch.example.org"),
        ("MELETE_ACME_CHALLENGE", "tls-alpn"),
    ])
    .unwrap_err()
    .to_string();
    assert!(without_https.contains("MELETE_TLS_BIND"), "{without_https}");
    let set = read(&[
        ("MELETE_ACME_DOMAIN", "watch.example.org"),
        ("MELETE_ACME_CHALLENGE", "tls-alpn"),
        ("MELETE_TLS_BIND", "0.0.0.0:8443"),
    ])
    .unwrap();
    let acme = set.acme.unwrap();
    assert_eq!(
        acme.names(),
        ["watch.example.org"],
        "the exact name, no wildcard"
    );
    assert!(acme.desec_token.is_none());
    assert!(
        read(&[
            ("MELETE_ACME_DOMAIN", "x.example.org"),
            ("MELETE_ACME_CHALLENGE", "http")
        ])
        .is_err()
    );
}
