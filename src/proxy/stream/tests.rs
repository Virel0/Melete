use super::*;
use crate::config::Secret;
use crate::proxy::testing::{self, Log};
use crate::seedbox::{self, Seedbox};

/// A name nothing here can look up: reaching it at all means the proxy looked it up.
const ONLY_THE_PROXY_KNOWS: &str = "seedbox.invalid";

fn settings(host: &str, port: u16, host_key: &str) -> crate::config::Seedbox {
    crate::config::Seedbox {
        host: host.into(),
        port,
        user: "user".into(),
        password: Secret::for_tests("pw"),
        host_key: host_key.into(),
        deluge_dir: "/home/user/Completed".into(),
        sftp_dir: "/Completed".into(),
    }
}

fn setup(proxy: Option<Proxy>, through: &[Service]) -> Setup {
    Setup {
        at: proxy,
        through: through.to_vec(),
    }
}

fn with_login(mut proxy: Proxy) -> Proxy {
    proxy.login = Some(("proxyuser".into(), Secret::for_tests("proxypass")));
    proxy
}

/// A seedbox's SFTP server with one finished download.
async fn seedbox_server() -> (tempfile::TempDir, seedbox::testing::Running) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("Completed/Some.Movie")).unwrap();
    let server = seedbox::testing::start(root.path()).await;
    (root, server)
}

fn socks(log: &Log) -> Vec<(u8, String, u16)> {
    log.lock().unwrap().socks.clone()
}

#[tokio::test]
async fn the_seedbox_goes_through_socks5_with_the_name_looked_up_by_the_proxy() {
    let (_root, server) = seedbox_server().await;
    let (at, log) = testing::relay(Kind::Socks5, Some(("proxyuser", "proxypass"))).await;
    let proxy = with_login(testing::proxy(Kind::Socks5, at));
    let pinned = Seedbox::new(settings(
        ONLY_THE_PROXY_KNOWS,
        server.port,
        &server.fingerprint,
    ));
    let ((key, report), applied) = testing::with(setup(Some(proxy), &[Service::Seedbox]), async {
        let key = seedbox::host_key(ONLY_THE_PROXY_KNOWS, server.port).await;
        (key, pinned.test(&[]).await)
    })
    .await;
    assert_eq!(key.unwrap(), server.fingerprint);
    assert_eq!(report.unwrap().entries, 1);
    let asked = (3, ONLY_THE_PROXY_KNOWS.to_owned(), server.port);
    assert_eq!(
        socks(&log),
        [asked.clone(), asked],
        "the name, not an address"
    );
    let login = ("proxyuser".to_owned(), "proxypass".to_owned());
    assert_eq!(log.lock().unwrap().logins, [login.clone(), login]);
    assert_eq!(*server.passwords.lock().unwrap(), ["pw"]);
    assert!(applied.status.down().is_none());
}

#[tokio::test]
async fn the_seedbox_goes_through_an_http_proxy_with_connect() {
    let (_root, server) = seedbox_server().await;
    let (at, log) = testing::relay(Kind::Http, Some(("proxyuser", "proxypass"))).await;
    let proxy = with_login(testing::proxy(Kind::Http, at));
    let pinned = Seedbox::new(settings(
        ONLY_THE_PROXY_KNOWS,
        server.port,
        &server.fingerprint,
    ));
    let (report, _) =
        testing::with(setup(Some(proxy), &[Service::Seedbox]), pinned.test(&[])).await;
    assert_eq!(report.unwrap().entries, 1);
    let seen = log.lock().unwrap();
    assert_eq!(
        seen.requests,
        [format!(
            "CONNECT {ONLY_THE_PROXY_KNOWS}:{} HTTP/1.1",
            server.port
        )]
    );
    assert_eq!(
        seen.authorizations,
        [Some("Basic cHJveHl1c2VyOnByb3h5cGFzcw==".to_owned())]
    );
}

#[tokio::test]
async fn a_login_the_proxy_turns_down_reaches_nothing() {
    let (_root, server) = seedbox_server().await;
    let (at, _) = testing::relay(Kind::Http, Some(("proxyuser", "other"))).await;
    let proxy = with_login(testing::proxy(Kind::Http, at));
    let address = proxy.address();
    let pinned = Seedbox::new(settings("127.0.0.1", server.port, &server.fingerprint));
    let (err, _) = testing::with(setup(Some(proxy), &[Service::Seedbox]), pinned.test(&[])).await;
    let said = err.unwrap_err().to_string();
    assert!(
        said.contains(&format!("The proxy at {address} answered")),
        "{said}"
    );
    assert!(
        !said.contains("proxypass") && !said.contains("proxyuser"),
        "{said}"
    );
    assert!(server.passwords.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_seedbox_goes_direct_unless_switched_on() {
    let (_root, server) = seedbox_server().await;
    let (at, log) = testing::relay(Kind::Socks5, None).await;
    let pinned = Seedbox::new(settings("127.0.0.1", server.port, &server.fingerprint));
    let others = setup(Some(testing::proxy(Kind::Socks5, at)), &[Service::Deluge]);
    let (report, _) = testing::with(others, pinned.test(&[])).await;
    assert_eq!(report.unwrap().entries, 1);
    assert!(socks(&log).is_empty());
}

#[tokio::test]
async fn a_down_proxy_fails_closed_and_nothing_goes_direct() {
    let (_root, server) = seedbox_server().await;
    let gone = testing::proxy(Kind::Socks5, testing::nothing_at().await);
    let address = gone.address();
    // The seedbox answers direct; set to go through the proxy, it isn't reached.
    let pinned = Seedbox::new(settings("127.0.0.1", server.port, &server.fingerprint));
    let ((key, err), applied) = testing::with(setup(Some(gone), &[Service::Seedbox]), async {
        let key = seedbox::host_key("127.0.0.1", server.port).await;
        (key, pinned.test(&[]).await)
    })
    .await;
    for said in [key.unwrap_err().to_string(), err.unwrap_err().to_string()] {
        assert!(
            said.contains(&format!("The proxy at {address} doesn't answer")),
            "{said}"
        );
    }
    assert!(server.passwords.lock().unwrap().is_empty());
    assert_eq!(applied.status.down().map(|(_, at)| at), Some(address));
}

#[tokio::test]
async fn switched_on_with_no_proxy_set_connects_to_nothing() {
    let (_root, server) = seedbox_server().await;
    let pinned = Seedbox::new(settings("127.0.0.1", server.port, &server.fingerprint));
    let (err, _) = testing::with(setup(None, &[Service::Seedbox]), pinned.test(&[])).await;
    assert_eq!(
        err.unwrap_err(),
        seedbox::Error::Unreachable(Unreachable::NoProxy.to_string())
    );
}

#[tokio::test]
async fn an_https_proxy_is_spoken_to_over_tls_only() {
    let (_root, server) = seedbox_server().await;
    // A proxy speaking plain HTTP, set up as HTTPS: the TLS handshake fails, nothing is sent.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = listener.local_addr().unwrap();
    let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
    let noted = heard.clone();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        while let Ok((mut stream, _)) = listener.accept().await {
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
            let mut bytes = vec![0; 4096];
            let n = stream.read(&mut bytes).await.unwrap_or(0);
            noted.lock().unwrap().extend_from_slice(&bytes[..n]);
        }
    });
    let proxy = testing::proxy(Kind::Https, at);
    let address = proxy.address();
    let pinned = Seedbox::new(settings("127.0.0.1", server.port, &server.fingerprint));
    let (err, _) = testing::with(setup(Some(proxy), &[Service::Seedbox]), pinned.test(&[])).await;
    let said = err.unwrap_err().to_string();
    assert!(
        said.contains(&format!("The proxy at {address} answered")),
        "{said}"
    );
    // What reached it is a TLS handshake record (0x16), never a CONNECT in the clear.
    let mut first = None;
    for _ in 0..200 {
        first = heard.lock().unwrap().first().copied();
        if first.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(first, Some(0x16), "a TLS hello first");
    assert!(server.passwords.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_ipv6_seedbox_is_reached_with_or_without_brackets() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("Completed")).unwrap();
    let server = seedbox::testing::start_at(root.path(), "[::1]:0").await;
    let (at, log) = testing::relay(Kind::Http, None).await;
    let through = setup(Some(testing::proxy(Kind::Http, at)), &[Service::Seedbox]);
    for host in ["::1", "[::1]"] {
        let pinned = Seedbox::new(settings(host, server.port, &server.fingerprint));
        let (direct, _) = testing::with(setup(None, &[]), pinned.test(&[])).await;
        assert!(direct.is_ok(), "{host} direct: {direct:?}");
        let (proxied, _) = testing::with(through.clone(), pinned.test(&[])).await;
        assert!(proxied.is_ok(), "{host} through the proxy: {proxied:?}");
    }
    let connect = format!("CONNECT [::1]:{} HTTP/1.1", server.port);
    assert_eq!(log.lock().unwrap().requests, [connect.clone(), connect]);
}

#[tokio::test]
async fn a_host_that_is_no_address_isnt_put_on_the_proxy() {
    let (at, log) = testing::relay(Kind::Http, None).await;
    let pinned = Seedbox::new(settings("seed box", 22, "SHA256:x"));
    let through = setup(Some(testing::proxy(Kind::Http, at)), &[Service::Seedbox]);
    let (err, applied) = testing::with(through, pinned.test(&[])).await;
    let said = err.unwrap_err().to_string();
    assert!(
        said.contains("isn't a host name or an IP address"),
        "{said}"
    );
    assert!(log.lock().unwrap().requests.is_empty());
    assert_eq!(applied.status.mark(), 0, "no proxy failure noted");
}
