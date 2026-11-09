use super::testing::{self, KEY};
use super::*;

fn client(base: &str, key: &str) -> Client {
    Client::new(base, Secret::new(key)).unwrap()
}

#[tokio::test]
async fn reads_how_the_vpn_is_with_the_key_only() {
    let (base, fake) = testing::start().await;
    let state = client(&base, KEY).state().await.unwrap();
    assert_eq!(
        state,
        State {
            running: true,
            public_ip: Some("203.0.113.7".into()),
            country: Some("Netherlands".into()),
            forwarded_port: Some(51413),
        }
    );
    let asked = fake.lock().unwrap().asked.clone();
    // First without the key, on both routes, to be sure it's needed; then all with it.
    assert_eq!(
        asked[..2],
        [
            ("GET /v1/vpn/status".to_owned(), false),
            ("PUT /v1/vpn/status".to_owned(), false)
        ]
    );
    assert!(asked[2..].iter().all(|(_, keyed)| *keyed), "{asked:?}");
    assert!(
        fake.lock().unwrap().running,
        "the keyless probe changed nothing"
    );
    assert_eq!(
        client(&base, "wrong").state().await.unwrap_err(),
        Trouble::KeyRefused
    );
}

/// A control server open to anyone on any route Melete uses is refused, and nothing is changed:
/// open on every route, on reading the status only, or on starting and stopping only.
#[tokio::test]
async fn a_control_server_that_answers_without_a_key_is_refused() {
    for open in [
        vec!["*"],
        vec!["GET /v1/vpn/status"],
        vec!["PUT /v1/vpn/status"],
    ] {
        let (base, fake) = testing::start().await;
        fake.lock().unwrap().open = open.clone();
        let c = client(&base, KEY);
        assert_eq!(c.state().await.unwrap_err(), Trouble::Open, "{open:?}");
        assert_eq!(c.set_running(false).await.unwrap_err(), Trouble::Open);
        let fake = fake.lock().unwrap();
        assert!(fake.running, "nothing was changed: {open:?}");
        assert!(
            fake.asked.iter().all(|(_, keyed)| !keyed),
            "the key was never sent to it: {:?}",
            fake.asked
        );
    }
}

#[tokio::test]
async fn starts_stops_and_restarts() {
    let (base, fake) = testing::start().await;
    let c = client(&base, KEY);
    c.set_running(false).await.unwrap();
    assert!(!fake.lock().unwrap().running);
    let stopped = c.state().await.unwrap();
    assert_eq!((stopped.running, stopped.public_ip), (false, None));
    c.restart().await.unwrap();
    assert!(fake.lock().unwrap().running);
    let asked = fake.lock().unwrap().asked.clone();
    assert!(
        asked.iter().all(|(route, _)| !route.contains("settings")),
        "its settings are never read or changed: {asked:?}"
    );
}

#[tokio::test]
async fn the_key_is_never_in_what_is_said() {
    let (base, fake) = testing::start().await;
    fake.lock().unwrap().open = vec!["*"];
    let said = client(&base, KEY).state().await.unwrap_err().to_string();
    assert!(!said.contains(KEY), "{said}");
    let gone = client("http://127.0.0.1:9", KEY)
        .state()
        .await
        .unwrap_err()
        .to_string();
    assert!(!gone.contains(KEY), "{gone}");
}

/// The settings these make, or why they're refused.
fn read(pairs: &[(&str, &str)]) -> Result<Option<Settings>, String> {
    Settings::from_lookup(&|name: &str| {
        pairs
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
    })
    .map_err(|e| e.to_string())
}

#[test]
fn the_control_server_is_set_whole_or_not_at_all() {
    assert!(read(&[]).unwrap().is_none());
    let set = read(&[
        ("MELETE_VPN_URL", "http://gluetun:8000/"),
        ("MELETE_VPN_KEY", "k"),
    ])
    .unwrap()
    .unwrap();
    assert_eq!(set.url, "http://gluetun:8000");
    for refused in [
        &[("MELETE_VPN_URL", "http://gluetun:8000")][..],
        &[("MELETE_VPN_KEY", "k")],
        &[("MELETE_VPN_URL", "gluetun:8000"), ("MELETE_VPN_KEY", "k")],
        &[
            ("MELETE_VPN_URL", "http://user:pw@gluetun:8000"),
            ("MELETE_VPN_KEY", "k"),
        ],
    ] {
        assert!(read(refused).is_err(), "{refused:?}");
    }
    let debug = format!(
        "{:?}",
        read(&[
            ("MELETE_VPN_URL", "http://g:1"),
            ("MELETE_VPN_KEY", "the-key")
        ])
    );
    assert!(!debug.contains("the-key"), "{debug}");
}

#[tokio::test]
async fn the_test_says_how_it_is_and_refuses_an_open_one() {
    let (base, fake) = testing::start().await;
    let config = crate::config::Config::from_env_and(|name| match name {
        "MELETE_VPN_URL" => Some(base.clone()),
        "MELETE_VPN_KEY" => Some(KEY.to_owned()),
        _ => None,
    })
    .unwrap();
    let pool = crate::db::open_in_memory().await;
    let said = crate::service_check::check(&pool, "vpn", &config)
        .await
        .unwrap();
    assert_eq!(
        said.message,
        "Gluetun answered and took the key; the VPN is running, and the outside world sees 203.0.113.7 (Netherlands). Forwarded port: 51413."
    );
    assert!(said.ok);
    fake.lock().unwrap().open = vec!["PUT /v1/vpn/status"];
    let open = crate::service_check::check(&pool, "vpn", &config)
        .await
        .unwrap();
    assert!(
        !open.ok && open.message.contains("answers without a key"),
        "{open:?}"
    );
}
