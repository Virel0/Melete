use std::net::SocketAddr;

use super::testing::{self, Log, with};
use super::*;

/// A setup sending `through` to `proxy`.
fn setup(proxy: Proxy, through: &[Service]) -> Setup {
    Setup {
        at: Some(proxy),
        through: through.to_vec(),
    }
}

/// A client for `service` that takes outside.example to mean `site` when it goes direct.
fn service(service: Service, site: SocketAddr) -> Http {
    Http::new(service, move |b| b.resolve("outside.example", site)).unwrap()
}

/// The same for a client with a choice of its own, `through` the proxy or not.
fn its_own(through: bool, site: SocketAddr) -> Http {
    Http::through(through, move |b| b.resolve("outside.example", site)).unwrap()
}

/// Asks `http://outside.example:<port>/hello` with `http`; what came back, or the whole error.
async fn ask(http: Http, site: SocketAddr) -> Result<String, String> {
    let url = format!("http://outside.example:{}/hello", site.port());
    match http.get(url).send().await {
        Ok(answer) => Ok(answer.text().await.unwrap()),
        Err(err) => Err(chain(&err)),
    }
}

/// An error with every cause behind it.
fn chain(err: &(dyn std::error::Error + 'static)) -> String {
    std::iter::successors(Some(err), |e| e.source())
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(": ")
}

fn asked(log: &Log) -> Vec<String> {
    log.lock().unwrap().requests.clone()
}

#[tokio::test]
async fn chosen_services_go_through_the_proxy_and_others_go_direct() {
    let (proxy_at, proxy_log) = testing::http_proxy("from the proxy").await;
    let (site, site_log) = testing::site("from the site").await;
    let chosen = setup(
        testing::proxy(Kind::Http, proxy_at),
        &[Service::Tmdb, Service::Slskd],
    );
    let ((tmdb, slskd, musicbrainz, indexer), _) = with(chosen, async {
        (
            ask(service(Service::Tmdb, site), site).await,
            ask(service(Service::Slskd, site), site).await,
            ask(service(Service::MusicBrainz, site), site).await,
            ask(its_own(false, site), site).await,
        )
    })
    .await;
    assert_eq!(tmdb.unwrap(), "from the proxy");
    assert_eq!(
        slskd.unwrap(),
        "from the proxy",
        "a local service can be chosen too"
    );
    assert_eq!(musicbrainz.unwrap(), "from the site");
    assert_eq!(indexer.unwrap(), "from the site");
    let through = format!("GET http://outside.example:{}/hello HTTP/1.1", site.port());
    assert_eq!(asked(&proxy_log), [through.clone(), through]);
    assert_eq!(asked(&site_log).len(), 2, "the rest went direct");
}

#[tokio::test]
async fn the_services_own_clients_follow_the_choice() {
    let (proxy_at, proxy_log) = testing::http_proxy("[]").await;
    let (site, site_log) = testing::site("{}").await;
    let chosen = setup(testing::proxy(Kind::Http, proxy_at), &[Service::Lyrics]);
    with(chosen, async {
        // LRCLIB is sent through the proxy, at a name only the proxy is asked about.
        let lrclib = crate::lrclib::Client::with_base("http://lrclib.example".into()).unwrap();
        let track = crate::lrclib::Track {
            title: "Song",
            artist: "Band",
            album: "Album",
            seconds: 200,
        };
        let _ = lrclib.lyrics(&track).await;
        // MusicBrainz isn't.
        let base = format!("http://{site}");
        let mb =
            crate::musicbrainz::Client::with_base(base, Default::default(), Default::default())
                .unwrap();
        let _ = mb.artist("89ad4ac3-39f7-470e-963a-56509c546377").await;
    })
    .await;
    let through = asked(&proxy_log);
    assert_eq!(through.len(), 1, "{through:?}");
    assert!(
        through[0].starts_with("GET http://lrclib.example/api/get?"),
        "{through:?}"
    );
    let direct = asked(&site_log);
    assert_eq!(direct.len(), 1, "{direct:?}");
    assert!(direct[0].starts_with("GET /"), "{direct:?}");
}

#[tokio::test]
async fn a_down_proxy_fails_closed_naming_it() {
    let gone = testing::nothing_at().await;
    let (site, site_log) = testing::site("from the site").await;
    let mut proxy = testing::proxy(Kind::Socks5, gone);
    proxy.login = Some(("alice".into(), Secret::new("the-proxy-password")));
    let ((failed, finding), applied) = with(setup(proxy, &[Service::Tmdb]), async {
        (ask(service(Service::Tmdb, site), site).await, finding())
    })
    .await;
    let failed = failed.unwrap_err();
    assert!(
        failed.contains(&format!("The proxy at socks5h://{gone} doesn't answer")),
        "{failed}"
    );
    assert!(
        !failed.contains("alice") && !failed.contains("the-proxy-password"),
        "{failed}"
    );
    assert!(asked(&site_log).is_empty(), "nothing went direct");
    assert_eq!(
        applied.status.down().unwrap().1,
        format!("socks5h://{gone}")
    );
    let finding = finding.unwrap();
    assert_eq!(finding.code, "proxy_down");
    assert!(
        finding.message.contains(&format!("socks5h://{gone}")),
        "{}",
        finding.message
    );
}

#[tokio::test]
async fn set_to_use_a_proxy_with_none_set_sends_nothing() {
    let (site, site_log) = testing::site("from the site").await;
    let (failed, _) = with(Setup::default(), async {
        ask(its_own(true, site), site).await
    })
    .await;
    assert!(
        failed
            .unwrap_err()
            .contains("no proxy is set, so nothing was sent")
    );
    assert!(asked(&site_log).is_empty());
}

#[tokio::test]
async fn a_proxy_that_answers_again_is_up() {
    let (proxy_at, _) = testing::http_proxy("ok").await;
    let (site, _) = testing::site("from the site").await;
    let (_, applied) = with(
        setup(testing::proxy(Kind::Http, proxy_at), &[Service::Tmdb]),
        async {
            current()
                .status
                .failed(&Unreachable::Down("http://somewhere:1".into()));
            assert!(finding().is_some());
            ask(service(Service::Tmdb, site), site).await.unwrap();
            assert!(finding().is_none());
        },
    )
    .await;
    assert!(applied.status.down().is_none());
}

#[tokio::test]
async fn socks5_sends_the_name_not_an_address() {
    let (proxy_at, log) = testing::socks5(None, "through socks").await;
    let chosen = setup(testing::proxy(Kind::Socks5, proxy_at), &[Service::Tmdb]);
    let (answer, _) = with(chosen, async {
        // A name that doesn't resolve here: only the proxy can look it up.
        let http = Http::new(Service::Tmdb, |b| b).unwrap();
        http.get("http://tmdb.example:8080/3/configuration")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    })
    .await;
    assert_eq!(answer, "through socks");
    let seen = log.lock().unwrap();
    assert_eq!(seen.socks, [(3, "tmdb.example".to_owned(), 8080)]);
    assert_eq!(seen.requests, ["GET /3/configuration HTTP/1.1"]);
}

#[tokio::test]
async fn no_secret_in_answers_logs_or_errors() {
    let log = crate::logging::testing::capture("info");
    // The right login goes to the proxy, and only there.
    let (proxy_at, seen) = testing::socks5(Some(("alice", "the-proxy-password")), "ok").await;
    let mut proxy = testing::proxy(Kind::Socks5, proxy_at);
    proxy.login = Some(("alice".into(), Secret::new("the-proxy-password")));
    let (site, _) = testing::site("from the site").await;
    let (answer, _) = with(setup(proxy.clone(), &[Service::Tmdb]), async {
        ask(service(Service::Tmdb, site), site).await
    })
    .await;
    assert_eq!(answer.unwrap(), "ok");
    assert_eq!(
        seen.lock().unwrap().logins,
        [("alice".to_owned(), "the-proxy-password".to_owned())]
    );

    // One the proxy turns down fails, and no word of it says what the login was.
    let (strict_at, _) = testing::socks5(Some(("alice", "another")), "ok").await;
    let mut strict = testing::proxy(Kind::Socks5, strict_at);
    strict.login = proxy.login.clone();
    let (refused, _) = with(setup(strict.clone(), &[Service::Tmdb]), async {
        ask(service(Service::Tmdb, site), site).await
    })
    .await;
    let refused = refused.unwrap_err();
    assert!(
        refused.contains(&format!("The proxy at socks5h://{strict_at} answered")),
        "{refused}"
    );
    let tested = test(&strict, "http://echo.example/").await.unwrap_err();

    let config = Config::from_env_and(|name| match name {
        "MELETE_PROXY_HOST" => Some("192.168.1.10".into()),
        "MELETE_PROXY_PORT" => Some("1080".into()),
        "MELETE_PROXY_USERNAME" => Some("alice".into()),
        "MELETE_PROXY_PASSWORD" => Some("the-proxy-password".into()),
        _ => None,
    })
    .unwrap();
    let debug = format!("{config:?} {strict:?}");
    for said in [&refused, &tested, &debug, &log.written.text()] {
        assert!(!said.contains("the-proxy-password"), "{said}");
    }
    assert!(
        !refused.contains("alice") && !tested.contains("alice"),
        "{refused} {tested}"
    );
    assert!(
        log.written.text().contains("The proxy at"),
        "the failure is logged"
    );
}

#[tokio::test]
async fn the_test_says_what_address_the_outside_world_sees() {
    let (proxy_at, log) = testing::http_proxy("203.0.113.7\n").await;
    let said = test(
        &testing::proxy(Kind::Http, proxy_at),
        "http://echo.example/",
    )
    .await
    .unwrap();
    assert_eq!(
        said,
        "The proxy answered. The outside world sees 203.0.113.7, as echo.example saw it."
    );
    assert_eq!(asked(&log), ["GET http://echo.example/ HTTP/1.1"]);

    let gone = testing::nothing_at().await;
    let down = test(&testing::proxy(Kind::Http, gone), "http://echo.example/")
        .await
        .unwrap_err();
    assert_eq!(
        down,
        format!(
            "The proxy at http://{gone} doesn't answer. Check its host, port and type, and that it's running."
        )
    );
    let (odd_at, _) = testing::http_proxy("<html>").await;
    let odd = test(&testing::proxy(Kind::Http, odd_at), "http://echo.example/").await;
    assert!(odd.unwrap_err().contains("isn't an address"));
}

/// The setup these settings make, or why they're refused.
fn read(pairs: &[(&str, &str)]) -> Result<Setup, String> {
    Setup::from_lookup(&|name: &str| {
        pairs
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
    })
    .map_err(|e| e.to_string())
}

#[test]
fn reads_the_settings_and_refuses_what_would_go_direct() {
    let none = read(&[]).unwrap();
    assert!(
        none.at.is_none() && none.through.is_empty(),
        "nothing by default"
    );
    let set = read(&[
        ("MELETE_PROXY_HOST", "192.168.1.10"),
        ("MELETE_PROXY_PORT", "1080"),
        ("MELETE_PROXY_TMDB", "on"),
        ("MELETE_PROXY_SLSKD", "off"),
    ])
    .unwrap();
    assert_eq!(set.at.unwrap().address(), "socks5h://192.168.1.10:1080");
    assert_eq!(set.through, [Service::Tmdb]);
    let refused = read(&[("MELETE_PROXY_TMDB", "on")]).unwrap_err();
    assert!(refused.contains("no proxy is set"), "{refused}");
    for host in ["socks5://x", "a:1", "a/b", "user@a", "a b", "[nope]"] {
        let refused = read(&[("MELETE_PROXY_HOST", host), ("MELETE_PROXY_PORT", "1")]);
        assert!(refused.is_err(), "{host}");
    }
    assert!(read(&[("MELETE_PROXY_HOST", "[::1]"), ("MELETE_PROXY_PORT", "1")]).is_ok());
    assert!(
        read(&[("MELETE_PROXY_HOST", "a")]).is_err(),
        "a host needs a port"
    );
    let no_user = read(&[
        ("MELETE_PROXY_HOST", "a"),
        ("MELETE_PROXY_PORT", "1"),
        ("MELETE_PROXY_PASSWORD", "pw"),
    ]);
    assert!(no_user.unwrap_err().contains("needs MELETE_PROXY_USERNAME"));
    let kind = read(&[
        ("MELETE_PROXY_HOST", "a"),
        ("MELETE_PROXY_PORT", "1"),
        ("MELETE_PROXY_TYPE", "https"),
    ]);
    assert_eq!(kind.unwrap().at.unwrap().address(), "https://a:1");
}

#[test]
fn every_service_has_its_switch_in_the_settings() {
    let proxy = crate::settings::SERVICES
        .iter()
        .find(|s| s.key == "proxy")
        .unwrap();
    for service in Service::ALL {
        assert!(
            proxy.fields.iter().any(|f| f.name == service.setting()
                && f.input == crate::settings::Input::Toggle { on: "on" }),
            "{service:?}"
        );
    }
}

use crate::config::Config;

/// A client kept for a long job (an import, a search for everything missing) follows a switch
/// made while it runs, from its next request: through the proxy once switched on, and nowhere
/// once its way can't be made.
#[tokio::test]
async fn a_client_kept_for_a_job_follows_a_switch_made_while_it_runs() {
    let (proxy_at, proxy_log) = testing::http_proxy("{}").await;
    let (site, site_log) = testing::site("{}").await;
    let proxy = testing::proxy(Kind::Http, proxy_at);
    let ((), _) = with(setup(proxy.clone(), &[]), async {
        let base = format!("http://{site}");
        let mb =
            crate::musicbrainz::Client::with_base(base, Default::default(), Default::default())
                .unwrap();
        let artist = "89ad4ac3-39f7-470e-963a-56509c546377";
        let _ = mb.artist(artist).await;
        assert_eq!(asked(&site_log).len(), 1, "direct before the switch");

        testing::switch(setup(proxy.clone(), &[Service::MusicBrainz]));
        let _ = mb.artist(artist).await;
        assert_eq!(asked(&site_log).len(), 1, "nothing direct after it");
        let through = asked(&proxy_log);
        assert_eq!(through.len(), 1, "{through:?}");
        assert!(
            through[0].starts_with(&format!("GET http://{site}/")),
            "{through:?}"
        );

        // Switched on with no proxy to use (refused when saved; here as if it got through).
        testing::switch(Setup {
            at: None,
            through: vec![Service::MusicBrainz],
        });
        assert!(mb.artist(artist).await.is_err(), "refused");
        let sent = (asked(&site_log).len(), asked(&proxy_log).len());
        assert_eq!(sent, (1, 1), "nothing sent, direct or through");
    })
    .await;
}

#[tokio::test]
async fn each_indexer_goes_its_own_way() {
    use crate::torznab::Endpoint;
    let feed = "<rss><channel></channel></rss>";
    let (proxy_at, proxy_log) = testing::http_proxy(feed).await;
    let (site, site_log) = testing::site(feed).await;
    let direct = format!("http://{site}/api");
    let chosen = setup(testing::proxy(Kind::Http, proxy_at), &[]);
    with(chosen, async {
        let client = crate::torznab::Client::new().unwrap();
        let tracker = Endpoint {
            url: "http://tracker.example/api",
            api_key: "",
        };
        let search = [("t", "search")];
        client
            .through(true)
            .search(&tracker, &search)
            .await
            .unwrap();
        let local = Endpoint {
            url: &direct,
            api_key: "",
        };
        client.through(false).search(&local, &search).await.unwrap();
    })
    .await;
    assert_eq!(
        asked(&proxy_log),
        ["GET http://tracker.example/api?t=search HTTP/1.1"]
    );
    assert_eq!(asked(&site_log), ["GET /api?t=search HTTP/1.1"]);

    // Set to go through a proxy when none is set: refused, nothing sent.
    let (refused, _) = with(Setup::default(), async {
        let client = crate::torznab::Client::new().unwrap().through(true);
        let local = Endpoint {
            url: &direct,
            api_key: "",
        };
        client.search(&local, &[("t", "search")]).await.unwrap_err()
    })
    .await;
    assert!(refused.to_string().contains("no proxy is set"), "{refused}");
    assert_eq!(asked(&site_log).len(), 1);
}

#[tokio::test]
async fn flaresolverr_is_given_the_proxy_or_asked_nothing() {
    use crate::flaresolverr::Client;
    use crate::flaresolverr::testing::{Plays, start_as};
    let (url, log) = start_as(Plays::FlareSolverr(Some("3.4.0")), "<html></html>").await;
    let pages = || {
        log.lock()
            .unwrap()
            .iter()
            .filter(|l| !l.is_empty())
            .cloned()
            .collect::<Vec<_>>()
    };
    let at: SocketAddr = "192.168.1.10:1080".parse().unwrap();
    let page = |setup: Setup, through: bool, solver: String| async move {
        with(setup, async {
            Client::new(&solver)
                .unwrap()
                .through(through)
                .get("https://uindex.org/")
                .await
        })
        .await
        .0
    };
    page(
        setup(testing::proxy(Kind::Socks5, at), &[]),
        true,
        url.clone(),
    )
    .await
    .unwrap();
    page(
        setup(testing::proxy(Kind::Socks5, at), &[]),
        false,
        url.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        pages(),
        [
            r#"proxy {"url":"socks5://192.168.1.10:1080"}"#,
            "https://uindex.org/",
            "https://uindex.org/",
        ],
        "the browser goes through the proxy, and only for an indexer set to"
    );

    // What can't be handed over is refused, and no page is asked for: a proxy with a login (its
    // password never goes to FlareSolverr), an HTTPS one, none at all, a solver that isn't
    // FlareSolverr, and a FlareSolverr too old to read the field, or that doesn't say.
    let with_login = |kind| {
        let mut proxy = testing::proxy(kind, at);
        proxy.login = Some(("alice".into(), Secret::new("the-proxy-password")));
        setup(proxy, &[])
    };
    let (byparr, _) = start_as(Plays::Byparr, "<html></html>").await;
    let (old, old_log) = start_as(Plays::FlareSolverr(Some("v3.1.2")), "<html></html>").await;
    let (unsaid, unsaid_log) = start_as(Plays::FlareSolverr(None), "<html></html>").await;
    let socks = || setup(testing::proxy(Kind::Socks5, at), &[]);
    for (setup, solver, why) in [
        (
            with_login(Kind::Socks5),
            url.clone(),
            "never given a proxy's password",
        ),
        (
            with_login(Kind::Http),
            url.clone(),
            "never given a proxy's password",
        ),
        (
            setup(testing::proxy(Kind::Https, at), &[]),
            url.clone(),
            "can't use an HTTPS proxy",
        ),
        (Setup::default(), url.clone(), "no proxy is set"),
        (socks(), byparr, "isn't FlareSolverr itself"),
        (socks(), old, "FlareSolverr v3.1.2 may not read the proxy"),
        (socks(), unsaid, "of an unknown version"),
    ] {
        let refused = page(setup, true, solver).await.unwrap_err();
        assert!(refused.contains(why), "{refused}");
        assert!(!refused.contains("the-proxy-password"), "{refused}");
    }
    assert_eq!(pages().len(), 3, "nothing more was fetched");
    for log in [old_log, unsaid_log] {
        let asked = log.lock().unwrap().clone();
        assert!(
            asked.iter().all(|l| l.is_empty()),
            "only asked what it is: {asked:?}"
        );
    }
    assert!(!log.lock().unwrap().iter().any(|l| l.contains("password")));
}

#[test]
fn flaresolverr_reads_the_proxy_from_3_2_0() {
    for (version, takes) in [
        ("3.2.0", true),
        ("v3.3.21", true),
        ("3.4.0", true),
        ("4.0", true),
        ("3.1.2", false),
        ("v3.0.0", false),
        ("2.2.10", false),
        ("", false),
        ("unknown", false),
    ] {
        assert_eq!(
            crate::flaresolverr::takes_a_proxy(version),
            takes,
            "{version}"
        );
    }
}

/// A feed added by hand to go through the proxy is asked what it can do through it, and is
/// stored going that way from the start: never a moment direct.
#[tokio::test]
async fn an_indexer_added_through_the_proxy_never_starts_direct() {
    let caps = include_str!("../../tests/fixtures/prowlarr/caps.xml");
    let (proxy_at, log) = testing::http_proxy(caps).await;
    let pool = crate::db::open_in_memory().await;
    let (added, _) = with(setup(testing::proxy(Kind::Http, proxy_at), &[]), async {
        let client = crate::torznab::Client::new().unwrap();
        let url = "http://tracker.example/api";
        crate::indexers::add_torznab(&pool, &client, "Mine", url, "", true)
            .await
            .unwrap()
            .unwrap()
    })
    .await;
    let proxy: bool = sqlx::query_scalar("SELECT proxy FROM indexers WHERE id = ?")
        .bind(added)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(proxy, "stored through the proxy by the insert itself");
    assert_eq!(
        asked(&log),
        ["GET http://tracker.example/api?t=caps HTTP/1.1"]
    );
}
