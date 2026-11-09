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
    // First without the key, to be sure it's needed; then everything with it.
    assert_eq!(asked[0], ("GET /v1/vpn/status".to_owned(), false));
    assert!(asked[1..].iter().all(|(_, keyed)| *keyed), "{asked:?}");
    assert_eq!(
        client(&base, "wrong").state().await.unwrap_err(),
        Trouble::KeyRefused
    );
}

#[tokio::test]
async fn a_control_server_that_answers_without_a_key_is_refused() {
    let (base, fake) = testing::start().await;
    fake.lock().unwrap().open = true;
    let c = client(&base, KEY);
    assert_eq!(c.state().await.unwrap_err(), Trouble::Open);
    assert_eq!(c.set_running(false).await.unwrap_err(), Trouble::Open);
    assert_eq!(
        c.keep_countries(&["Sweden".into()]).await.unwrap_err(),
        Trouble::Open
    );
    let fake = fake.lock().unwrap();
    assert!(fake.running, "nothing was changed");
    assert!(fake.settings.is_empty());
    assert!(
        fake.asked
            .iter()
            .all(|(route, _)| route == "GET /v1/vpn/status")
    );
}

#[tokio::test]
async fn starts_stops_restarts_and_keeps_countries() {
    let (base, fake) = testing::start().await;
    let c = client(&base, KEY);
    c.set_running(false).await.unwrap();
    assert!(!fake.lock().unwrap().running);
    let stopped = c.state().await.unwrap();
    assert_eq!((stopped.running, stopped.public_ip), (false, None));
    c.restart().await.unwrap();
    assert!(fake.lock().unwrap().running);

    c.keep_countries(&["Sweden".into()]).await.unwrap();
    assert_eq!(
        fake.lock().unwrap().settings,
        [json!({ "provider": { "server_selection": { "countries": ["Sweden"] } } })]
    );
    let refused = c.keep_countries(&["Atlantis".into()]).await.unwrap_err();
    assert_eq!(refused, Trouble::Refused("country is not valid".into()));
    c.keep_countries(&[]).await.unwrap();
    assert_eq!(
        fake.lock().unwrap().settings.len(),
        2,
        "nothing to keep, nothing sent"
    );
}

#[test]
fn countries_are_read_from_a_list() {
    assert_eq!(
        countries(Some(" Netherlands, Sweden ,,")),
        ["Netherlands", "Sweden"]
    );
    assert!(countries(None).is_empty());
}

#[tokio::test]
async fn the_key_is_never_in_what_is_said() {
    let (base, fake) = testing::start().await;
    fake.lock().unwrap().open = true;
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
        (COUNTRIES_SETTING, "Sweden"),
    ])
    .unwrap()
    .unwrap();
    assert_eq!(
        (set.url.as_str(), set.countries),
        ("http://gluetun:8000", vec!["Sweden".to_owned()])
    );
    for refused in [
        &[("MELETE_VPN_URL", "http://gluetun:8000")][..],
        &[("MELETE_VPN_KEY", "k")],
        &[(COUNTRIES_SETTING, "Sweden")],
        &[("MELETE_VPN_URL", "gluetun:8000"), ("MELETE_VPN_KEY", "k")],
        &[
            ("MELETE_VPN_URL", "http://user:pw@gluetun:8000"),
            ("MELETE_VPN_KEY", "k"),
        ],
    ] {
        assert!(read(refused).is_err(), "{refused:?}");
    }
    let debug = format!(
        "{set:?}",
        set = read(&[
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
    fake.lock().unwrap().open = true;
    let open = crate::service_check::check(&pool, "vpn", &config)
        .await
        .unwrap();
    assert!(
        !open.ok && open.message.contains("answers without a key"),
        "{open:?}"
    );
}
