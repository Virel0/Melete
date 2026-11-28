use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};

use super::*;
use crate::config::{Secret, Tls};
use crate::desec;

/// Melete's database, a data folder, deSEC's and a CA's stand-ins holding example.org, and
/// Certificates set up for them.
struct World {
    pool: SqlitePool,
    dir: tempfile::TempDir,
    desec: desec::testing::Shared,
    ca: testing::Shared,
    acme: Acme,
    reach: Reach,
}

impl World {
    async fn new() -> World {
        let pool = crate::db::open_in_memory().await;
        let (api, dns, desec) = desec::testing::start(&["example.org"]).await;
        let (directory, ca) = testing::start(dns).await;
        World {
            pool,
            dir: tempfile::tempdir().unwrap(),
            desec,
            ca,
            acme: Acme {
                domain: "example.org".into(),
                desec_token: Secret::for_tests(desec::testing::TOKEN),
                email: None,
                staging: true,
                directory,
                settle: Duration::ZERO,
            },
            reach: Reach {
                desec_api: api,
                nameservers: vec![dns.to_string()],
                served_within: Some(Duration::from_secs(2)),
            },
        }
    }

    fn folder(&self) -> PathBuf {
        self.dir.path().join("acme")
    }

    async fn round(&self, asked: bool) -> i64 {
        round(&self.pool, &self.folder(), &self.acme, &self.reach, asked)
            .await
            .unwrap()
    }

    fn held(&self) -> files::Held {
        files::read(&self.folder()).expect("a certificate is kept")
    }

    async fn state(&self) -> renewal::State {
        renewal::load(&self.pool).await.unwrap()
    }
}

#[tokio::test]
async fn the_dns_challenge_gets_a_wildcard_and_its_record_goes_after() {
    let world = World::new().await;
    // Someone else's value at the same name stays as it is.
    world.desec.lock().unwrap().records.insert(
        ("example.org".into(), "_acme-challenge".into()),
        vec!["someone-else".into()],
    );
    world.round(false).await;
    let held = world.held();
    assert!(held.is_for(&["*.example.org".into(), "example.org".into()]));
    assert!(held.not_after > unix_now() + 80 * 86_400);
    assert_eq!(
        world.state().await.directory.as_deref(),
        Some(world.acme.directory.as_str())
    );
    let desec = world.desec.lock().unwrap();
    // Set with both names' values beside the other one, then back to the other one alone.
    let (first, last) = (&desec.writes[0], desec.writes.last().unwrap());
    assert_eq!(first.0, "_acme-challenge");
    assert_eq!(first.1.len(), 3, "{first:?}");
    assert_eq!(last.1, ["someone-else"]);
    assert_eq!(
        desec.records[&("example.org".to_owned(), "_acme-challenge".to_owned())],
        ["someone-else"]
    );
    // The token went in the header and nowhere else.
    assert!(
        desec
            .paths
            .iter()
            .all(|p| !p.contains(desec::testing::TOKEN))
    );
    let key = std::fs::metadata(world.folder().join(crate::config::KEY_FILE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(key.permissions().mode() & 0o777, 0o600);
}

#[tokio::test]
async fn the_record_goes_when_the_ca_turns_the_challenge_down_too() {
    let world = World::new().await;
    // The CA asks a nameserver that holds nothing.
    let (_, empty, _) = desec::testing::start(&["example.org"]).await;
    world.ca.lock().unwrap().dns = empty;
    world.round(false).await;
    assert!(files::read(&world.folder()).is_none());
    let state = world.state().await;
    let desec = world.desec.lock().unwrap();
    assert_eq!(desec.writes.len(), 2, "set, then removed");
    assert!(desec.writes[1].1.is_empty());
    assert_eq!(state.failures, 1);
    let why = state.last_error.unwrap();
    assert!(why.contains("No TXT record found"), "{why}");
}

#[tokio::test]
async fn the_record_goes_when_desecs_servers_never_answer_it() {
    let world = World::new().await;
    world.desec.lock().unwrap().serving = false;
    world.round(false).await;
    let why = world.state().await.last_error.unwrap();
    assert!(why.contains("still didn't answer"), "{why}");
    let desec = world.desec.lock().unwrap();
    assert_eq!(desec.writes.len(), 2, "set, then removed");
    assert!(desec.records.values().all(Vec::is_empty));
    assert!(
        world.ca.lock().unwrap().orders[0]
            .authz
            .iter()
            .all(|a| a.status == "pending"),
        "the CA was never told to look"
    );
}

#[tokio::test]
async fn the_account_key_is_kept_sealed_and_used_again() {
    let world = World::new().await;
    sqlx::query("INSERT INTO secrets_sealed (id, sealed_at) VALUES (1, 0)")
        .execute(&world.pool)
        .await
        .unwrap();
    let email = Some("admin@example.org");
    let first = account::get(&world.pool, &world.acme.directory, email)
        .await
        .unwrap();
    let again = account::get(&world.pool, &world.acme.directory, email)
        .await
        .unwrap();
    assert_eq!(first.id(), again.id());
    assert_eq!(world.ca.lock().unwrap().accounts.len(), 1, "made once");
    assert_eq!(
        world.ca.lock().unwrap().accounts[0].1,
        ["mailto:admin@example.org"]
    );
    let stored: String = sqlx::query_scalar("SELECT credentials FROM acme_accounts")
        .fetch_one(&world.pool)
        .await
        .unwrap();
    assert!(stored.starts_with("sealed:v1:"));
    assert!(!stored.contains("key_pkcs8"));
    let plain = sqlx::query("UPDATE acme_accounts SET credentials = '{\"id\":\"x\"}'")
        .execute(&world.pool)
        .await;
    assert!(plain.is_err(), "refused plain");
    // A new address goes to the CA; none takes it off.
    account::get(&world.pool, &world.acme.directory, None)
        .await
        .unwrap();
    assert!(world.ca.lock().unwrap().accounts[0].1.is_empty());
    assert_eq!(
        account::uri(&world.pool, &world.acme.directory)
            .await
            .unwrap()
            .as_deref(),
        Some(first.id())
    );
    // Another directory has its own account.
    let (other, _) = testing::start(world.reach.nameservers[0].parse().unwrap()).await;
    let elsewhere = account::get(&world.pool, &other, None).await.unwrap();
    assert_ne!(elsewhere.id(), first.id());
}

#[tokio::test]
async fn renewing_falls_in_the_window_the_ca_suggests() {
    let world = World::new().await;
    let now = unix_now();
    let window = (now + 10 * 86_400, now + 12 * 86_400);
    world.ca.lock().unwrap().window = Some(window);
    // A certificate already ten days old, so a window moved into the past is followed at once.
    world.ca.lock().unwrap().backdate = 10 * 86_400;
    world.round(false).await;
    // Not due: when it is falls in the window, and nothing more was asked for.
    let next = world.round(false).await;
    let state = world.state().await;
    let renew_at = state.renew_at.unwrap();
    assert!((window.0..=window.1).contains(&renew_at), "{renew_at}");
    assert!(next <= renew_at && next > now);
    assert_eq!(world.ca.lock().unwrap().orders.len(), 1);
    let asked = world.ca.lock().unwrap().ari_asked.clone();
    assert_eq!(asked.len(), 1);
    // Asked again later, the time picked stays while it's in the window.
    let mut state = world.state().await;
    state.ask_again_at = None;
    renewal::save(&world.pool, &state).await.unwrap();
    world.round(false).await;
    assert_eq!(world.ca.lock().unwrap().ari_asked.len(), 2);
    assert_eq!(world.state().await.renew_at, Some(renew_at));
    // The CA moves the window into the past: renewed now, as a replacement for the one held.
    world.ca.lock().unwrap().window = Some((now - 2 * 86_400, now - 86_400));
    let mut state = world.state().await;
    state.ask_again_at = None;
    renewal::save(&world.pool, &state).await.unwrap();
    world.round(false).await;
    let ca = world.ca.lock().unwrap();
    assert_eq!(ca.orders.len(), 2);
    assert_eq!(ca.orders[1].replaces.as_deref(), Some(asked[0].as_str()));
}

#[tokio::test]
async fn without_renewal_information_it_renews_two_thirds_through() {
    let world = World::new().await;
    world.round(false).await;
    world.round(false).await;
    let held = world.held();
    assert_eq!(
        world.state().await.renew_at,
        Some(renewal::two_thirds(held.not_before, held.not_after))
    );
    assert_eq!(world.ca.lock().unwrap().orders.len(), 1);
}

type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// A webhook set up as the alerts' way, noting each alert's title and message.
async fn alerts_to(pool: &SqlitePool) -> Seen {
    let seen = Seen::default();
    let kept = seen.clone();
    let app = axum::Router::new().route(
        "/hook",
        post(move |headers: HeaderMap, body: String| async move {
            let title = headers["title"].to_str().unwrap().to_owned();
            kept.lock().unwrap().push((title, body));
            StatusCode::OK
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_ALERT_WEBHOOK_URL', ?, 0)",
    )
    .bind(at)
    .execute(pool)
    .await
    .unwrap();
    seen
}

#[tokio::test]
async fn a_failed_renewal_alerts_admins_with_the_expiry_and_a_later_success_says_so() {
    let world = World::new().await;
    let seen = alerts_to(&world.pool).await;
    world.round(false).await;
    assert!(seen.lock().unwrap().is_empty(), "nothing to say yet");
    let until = crate::follow::date(world.held().not_after);
    world.ca.lock().unwrap().refuse_orders = Some("too many certificates lately".into());
    let next = world.round(true).await;
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let (title, message) = &seen[0];
        assert_eq!(title, "HTTPS's certificate for example.org isn't renewing");
        // Words of Melete's own, not the CA's.
        assert!(
            message.contains("the CA turned the order down"),
            "{message}"
        );
        assert!(
            !message.contains("too many certificates lately"),
            "{message}"
        );
        assert!(
            message.contains(&format!("runs out on {until}")),
            "{message}"
        );
    }
    // Kept across a restart: the next look waits for the retry, and asks the CA nothing.
    let state = world.state().await;
    assert_eq!((state.failures, state.retry_at), (1, Some(next)));
    assert!(next >= unix_now() + 9 * 60);
    let orders = world.ca.lock().unwrap().orders.len();
    assert_eq!(world.round(false).await, next);
    assert_eq!(world.ca.lock().unwrap().orders.len(), orders);
    // Failing again soon after says nothing new.
    world.round(true).await;
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(world.state().await.failures, 2);
    // After two in a row, asking now waits for the retry too.
    world.ca.lock().unwrap().refuse_orders = None;
    let orders = world.ca.lock().unwrap().orders.len();
    world.round(true).await;
    assert_eq!(world.ca.lock().unwrap().orders.len(), orders, "not asked");
    assert!(
        world
            .state()
            .await
            .last_error
            .unwrap()
            .contains("too many certificates lately"),
        "the CA's words stay for the page"
    );
    // Then the retry comes, it works, and says so.
    let mut state = world.state().await;
    state.retry_at = Some(unix_now() - 1);
    renewal::save(&world.pool, &state).await.unwrap();
    world.round(true).await;
    assert_eq!(
        world.state().await,
        renewal::State {
            directory: Some(world.acme.directory.clone()),
            ..renewal::State::default()
        }
    );
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].0, "HTTPS's certificate for example.org is renewed");
}

#[tokio::test]
async fn a_first_certificate_that_fails_alerts_too_and_secrets_never_show() {
    let world = World::new().await;
    let seen = alerts_to(&world.pool).await;
    let mut acme = world.acme.clone();
    acme.desec_token = Secret::for_tests("a-wrong-token-never-shown");
    let log = crate::logging::testing::capture("info");
    round(&world.pool, &world.folder(), &acme, &world.reach, false)
        .await
        .unwrap();
    let state = world.state().await;
    let why = state.last_error.unwrap();
    assert!(why.contains("deSEC turned the token down"), "{why}");
    let (title, message) = seen.lock().unwrap()[0].clone();
    assert_eq!(
        title,
        "Melete couldn't get HTTPS's certificate for example.org"
    );
    for said in [why, message, log.written.text()] {
        assert!(!said.contains("a-wrong-token-never-shown"), "{said}");
        assert!(!said.contains(desec::testing::TOKEN), "{said}");
    }
}

#[tokio::test]
async fn the_status_shows_the_account_for_caa_and_never_a_key() {
    let world = World::new().await;
    world.round(false).await;
    let env = |name: &str| match name {
        "MELETE_DATA_DIR" => Some(world.dir.path().to_string_lossy().into_owned()),
        "MELETE_ACME_DOMAIN" => Some("example.org".to_owned()),
        "MELETE_ACME_DESEC_TOKEN" => Some(desec::testing::TOKEN.to_owned()),
        _ => None,
    };
    let mut config = crate::config::Config::from_env_and(env).unwrap();
    config.acme.as_mut().unwrap().directory = world.acme.directory.clone();
    let status = status(&world.pool, &config).await.unwrap();
    let uri = status.account_uri.clone().unwrap();
    assert_eq!(
        status.caa[0],
        format!("example.org. CAA 0 issue \"letsencrypt.org; accounturi={uri}\"")
    );
    assert!(status.caa[1].contains("issuewild"));
    assert!(status.certificate.as_ref().unwrap().from_this_ca);
    let said = serde_json::to_string(&status).unwrap();
    let key = std::fs::read_to_string(world.folder().join(crate::config::KEY_FILE)).unwrap();
    assert!(!said.contains(desec::testing::TOKEN));
    assert!(!said.contains(&key[40..80]) && !said.contains("PRIVATE"));
}

#[test]
fn certificates_are_off_until_a_domain_and_token_are_set() {
    let read = |pairs: &'static [(&'static str, &'static str)]| {
        crate::config::Config::from_env_and(move |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        })
    };
    let none = read(&[]).unwrap();
    assert!(none.acme.is_none());
    let views = crate::settings::views(&none, &std::collections::HashMap::new(), &|_| None);
    assert!(!views.iter().find(|s| s.key == "certificates").unwrap().on);
    let set = read(&[
        ("MELETE_ACME_DOMAIN", "Example.org."),
        ("MELETE_ACME_DESEC_TOKEN", "t"),
    ])
    .unwrap();
    let acme = set.acme.unwrap();
    assert_eq!(acme.names(), ["*.example.org", "example.org"]);
    assert!(acme.staging, "staging until switched");
    assert_eq!(acme.directory, crate::config::STAGING);
    assert_eq!(
        acme.settle,
        Duration::from_secs(120),
        "two minutes unless set"
    );
    let waits = read(&[
        ("MELETE_ACME_DOMAIN", "example.org"),
        ("MELETE_ACME_DESEC_TOKEN", "t"),
        ("MELETE_ACME_DNS_WAIT", "30"),
    ])
    .unwrap();
    assert_eq!(waits.acme.unwrap().settle, Duration::from_secs(30));
    for wrong in [
        &[("MELETE_ACME_DOMAIN", "example.org")][..],
        &[("MELETE_ACME_DESEC_TOKEN", "t")],
        &[
            ("MELETE_ACME_DOMAIN", "*.example.org"),
            ("MELETE_ACME_DESEC_TOKEN", "t"),
        ],
        &[
            ("MELETE_ACME_DOMAIN", "localhost"),
            ("MELETE_ACME_DESEC_TOKEN", "t"),
        ],
        &[
            ("MELETE_ACME_DOMAIN", "example.org"),
            ("MELETE_ACME_DESEC_TOKEN", "t"),
            ("MELETE_ACME_CA", "other"),
        ],
        &[
            ("MELETE_ACME_DOMAIN", "example.org"),
            ("MELETE_ACME_DESEC_TOKEN", "t"),
            ("MELETE_ACME_DNS_WAIT", "3600"),
        ],
        &[
            ("MELETE_ACME_DOMAIN", "example.org"),
            ("MELETE_ACME_DESEC_TOKEN", "t"),
            ("MELETE_ACME_EMAIL", "alice"),
        ],
    ] {
        assert!(read(wrong).is_err(), "{wrong:?}");
    }
}

#[test]
fn https_takes_the_certificate_melete_gets_and_refuses_files_beside_it() {
    let on: &[(&str, &str)] = &[
        ("MELETE_DATA_DIR", "/path/to/config"),
        ("MELETE_ACME_DOMAIN", "example.org"),
        ("MELETE_ACME_DESEC_TOKEN", "t"),
    ];
    let read = |extra: &[(&'static str, &'static str)]| {
        let all: Vec<(&str, &str)> = on.iter().chain(extra).copied().collect();
        crate::config::Config::from_env_and(move |name: &str| {
            all.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        })
    };
    let tls = read(&[("MELETE_TLS_BIND", "0.0.0.0:8443")])
        .unwrap()
        .tls
        .unwrap();
    assert_eq!(tls.cert, Path::new("/path/to/config/acme/certificate.pem"));
    assert_eq!(tls.key, Path::new("/path/to/config/acme/key.pem"));
    assert!(read(&[]).unwrap().tls.is_none());
    let both = read(&[
        ("MELETE_TLS_BIND", "0.0.0.0:8443"),
        ("MELETE_TLS_CERT", "/certs/melete.crt"),
        ("MELETE_TLS_KEY", "/certs/melete.key"),
    ])
    .unwrap_err()
    .to_string();
    assert!(both.contains("Certificates is on"), "{both}");
}

/// The certificate HTTPS at `at` presents for melete.example.org, checked against `root`.
async fn presented(
    at: SocketAddr,
    root: &CertificateDer<'static>,
) -> Result<CertificateDer<'static>, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(root.clone()).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let tcp = tokio::net::TcpStream::connect(at)
        .await
        .map_err(|e| e.to_string())?;
    let name = ServerName::try_from("melete.example.org").unwrap();
    let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .map_err(|e| e.to_string())?;
    Ok(tls.get_ref().1.peer_certificates().unwrap()[0]
        .clone()
        .into_owned())
}

#[tokio::test]
async fn https_serves_the_certificate_once_its_got() {
    let world = World::new().await;
    let tls = Tls {
        bind: "127.0.0.1:0".parse().unwrap(),
        cert: world.folder().join(crate::config::CERT_FILE),
        key: world.folder().join(crate::config::KEY_FILE),
        proxy_protocol: false,
    };
    let ready = crate::tls::prepare_awaiting(&tls).await.unwrap();
    let at = ready.local_addr().unwrap();
    tokio::spawn(crate::tls::serve(ready, axum::Router::new(), Vec::new()));
    let root = world.ca.lock().unwrap().root.clone();
    // Until there's one, a stand-in no client trusts.
    assert!(presented(at, &root).await.is_err());
    world.round(false).await;
    let kept = CertificateDer::pem_file_iter(&tls.cert)
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    let mut served = presented(at, &root).await;
    for _ in 0..50 {
        if served.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        served = presented(at, &root).await;
    }
    assert_eq!(served.unwrap(), kept, "taken up at once, without a restart");
}

#[tokio::test]
async fn the_test_writes_and_removes_a_value_and_asks_for_no_certificate() {
    let world = World::new().await;
    world.desec.lock().unwrap().throttle = 1;
    let (fine, said) = test::zone_takes(&world.acme, &world.reach).await;
    assert!(fine, "{said}");
    assert!(said.contains("_acme-challenge.example.org"), "{said}");
    {
        let desec = world.desec.lock().unwrap();
        assert_eq!(desec.writes.len(), 2, "added, then taken out");
        assert!(desec.writes[0].1[0].starts_with("melete-test-"));
        assert!(desec.writes[1].1.is_empty());
    }
    {
        let ca = world.ca.lock().unwrap();
        assert!(
            ca.accounts.is_empty() && ca.orders.is_empty(),
            "the CA isn't asked"
        );
    }
    // A token whose policy doesn't allow the record, a wrong one, a domain it doesn't hold.
    world.desec.lock().unwrap().read_only = true;
    let (fine, said) = test::zone_takes(&world.acme, &world.reach).await;
    assert!(
        !fine && said.contains("_acme-challenge TXT record"),
        "{said}"
    );
    let mut wrong = world.acme.clone();
    wrong.desec_token = Secret::for_tests("a-wrong-token-never-shown");
    let (fine, said) = test::zone_takes(&wrong, &world.reach).await;
    assert!(!fine && said.contains("turned the token down"), "{said}");
    assert!(!said.contains("a-wrong-token-never-shown"));
    let mut other = world.acme.clone();
    other.domain = "example.net".into();
    let (fine, said) = test::zone_takes(&other, &world.reach).await;
    assert!(
        !fine && said.contains("sees no zone holding example.net"),
        "{said}"
    );
}

#[tokio::test]
async fn the_ca_looks_only_a_while_after_desec_answers() {
    let mut world = World::new().await;
    world.acme.settle = Duration::from_millis(1500);
    world.round(false).await;
    assert!(files::read(&world.folder()).is_some());
    let set = world.desec.lock().unwrap().written_at[0];
    let looked = world.ca.lock().unwrap().checked_at[0];
    assert!(
        looked.duration_since(set) >= Duration::from_millis(1500),
        "{:?}",
        looked.duration_since(set)
    );
}

#[tokio::test]
async fn a_certificate_whose_key_doesnt_match_is_got_again() {
    let world = World::new().await;
    world.round(false).await;
    // As if a crash came between writing the key and the certificate.
    let other = rcgen::KeyPair::generate().unwrap();
    std::fs::write(
        world.folder().join(crate::config::KEY_FILE),
        other.serialize_pem(),
    )
    .unwrap();
    assert!(
        files::read(&world.folder()).is_none(),
        "not a pair: nothing held"
    );
    world.round(false).await;
    assert_eq!(world.ca.lock().unwrap().orders.len(), 2);
    assert!(files::read(&world.folder()).is_some(), "a pair again");
}

#[tokio::test]
async fn a_window_past_the_certificates_end_is_kept_before_it() {
    let world = World::new().await;
    let now = unix_now();
    world.ca.lock().unwrap().window = Some((now + 100 * 86_400, now + 101 * 86_400));
    world.round(false).await;
    world.round(false).await;
    let held = world.held();
    let renew_at = world.state().await.renew_at.unwrap();
    assert!(renew_at <= held.not_after - 3 * 86_400, "{renew_at}");
    // And one in the past doesn't replace a new certificate at once.
    world.ca.lock().unwrap().window = Some((now - 2 * 86_400, now - 86_400));
    let mut state = world.state().await;
    state.ask_again_at = None;
    renewal::save(&world.pool, &state).await.unwrap();
    world.round(false).await;
    assert_eq!(world.ca.lock().unwrap().orders.len(), 1);
    assert!(world.state().await.renew_at.unwrap() >= held.not_before + 86_400);
}

#[tokio::test]
async fn failures_for_another_ca_dont_hold_this_one_up() {
    let mut world = World::new().await;
    world.ca.lock().unwrap().refuse_orders = Some("no".into());
    let retry_at = world.round(false).await;
    assert!(retry_at > unix_now());
    // Switched to another CA: tried at once, with no wait carried over.
    let (other, ca) = testing::start(world.reach.nameservers[0].parse().unwrap()).await;
    world.acme.directory = other;
    world.round(false).await;
    assert_eq!(ca.lock().unwrap().orders.len(), 1);
    assert!(world.held().not_after > unix_now());
    assert_eq!(world.state().await.failures, 0);
}

#[tokio::test]
async fn challenge_values_a_crash_left_are_taken_out_and_no_others() {
    let world = World::new().await;
    world.desec.lock().unwrap().records.insert(
        ("example.org".into(), "_acme-challenge".into()),
        vec!["left-by-melete".into(), "someone-else".into()],
    );
    left::note(
        &world.pool,
        &world.reach.desec_api,
        "example.org",
        "_acme-challenge",
        &["left-by-melete".to_owned()],
    )
    .await
    .unwrap();
    world.round(false).await;
    assert_eq!(
        world.desec.lock().unwrap().records
            [&("example.org".to_owned(), "_acme-challenge".to_owned())],
        ["someone-else"]
    );
    let noted: i64 = sqlx::query_scalar("SELECT count(*) FROM acme_challenge_left")
        .fetch_one(&world.pool)
        .await
        .unwrap();
    assert_eq!(noted, 0, "forgotten once removed");
}

#[tokio::test]
async fn desec_answers_are_capped_quotes_kept_and_lost_writes_caught() {
    let world = World::new().await;
    let desec = desec::Client::new(&world.reach.desec_api, world.acme.desec_token.clone()).unwrap();
    // A value ending in a quote of its own keeps it.
    world.desec.lock().unwrap().records.insert(
        ("example.org".into(), "_acme-challenge".into()),
        vec!["ends in a quote\"".into()],
    );
    let ours = ["ours".to_owned()];
    desec
        .add_txt("example.org", "_acme-challenge", &ours)
        .await
        .unwrap();
    desec
        .remove_txt("example.org", "_acme-challenge", &ours)
        .await
        .unwrap();
    assert_eq!(
        world.desec.lock().unwrap().records
            [&("example.org".to_owned(), "_acme-challenge".to_owned())],
        ["ends in a quote\""]
    );
    // A write deSEC took but didn't keep is caught by reading it back.
    world.desec.lock().unwrap().lose_writes = true;
    let lost = desec
        .add_txt("example.org", "_acme-challenge", &ours)
        .await
        .unwrap_err();
    assert!(lost.to_string().contains("didn't stay"), "{lost}");
    // An answer far too long is refused.
    world.desec.lock().unwrap().padding = 100 * 1024;
    let long = desec.zone("example.org").await.unwrap_err();
    assert!(long.to_string().contains("far too long"), "{long}");
}

#[test]
fn asking_now_is_refused_while_one_runs_lately_asked_or_new() {
    let now = 10_000_000;
    let state = renewal::State::default();
    let new = files::Held {
        names: Vec::new(),
        not_before: now - 3600,
        not_after: now + 89 * 86_400,
        id: None,
    };
    assert!(
        renewal::ask_refused(&state, None, true, now).is_some(),
        "one runs"
    );
    assert!(renewal::ask_refused(&state, Some((&new, true)), false, now).is_some());
    assert_eq!(
        renewal::ask_refused(&state, Some((&new, false)), false, now),
        None,
        "from another CA"
    );
    assert_eq!(renewal::ask_refused(&state, None, false, now), None);
    let asked = renewal::State {
        asked_at: Some(now - 60),
        ..renewal::State::default()
    };
    let why = renewal::ask_refused(&asked, None, false, now).unwrap();
    assert!(why.contains("ask again in 14 minutes"), "{why}");
    assert_eq!(
        renewal::ask_refused(&asked, None, false, now + 15 * 60),
        None
    );
}
