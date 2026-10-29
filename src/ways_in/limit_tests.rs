use super::*;

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

#[test]
fn a_way_can_hold_video_to_a_limit() {
    let ways = parse_ways(
        "home 192.168.0.0/24 all; vps 10.20.0.1 video,music max=8M; phone 10.10.0.0/24 all max=2.5M; slow 10.7.0.1 all max=800k",
    )
    .unwrap();
    let limits: Vec<Option<i64>> = ways.iter().map(|w| w.max_bitrate).collect();
    assert_eq!(
        limits,
        [None, Some(8_000_000), Some(2_500_000), Some(800_000)]
    );
    assert_eq!(ways[1].parts, Some(vec![Part::Video, Part::Music]));
    assert_eq!(
        parse_ways("a 10.0.0.1 all max=1G").unwrap_err(),
        "\"max=1G\" is outside what a video can be held to: 100k to 200M bits per second"
    );
    for wrong in ["max=", "max=fast", "limit=8M", "max=8Mbit"] {
        let err = parse_ways(&format!("a 10.0.0.1 all {wrong}")).unwrap_err();
        assert!(err.contains("like max=8M"), "{wrong}: {err}");
    }
    assert!(
        parse_ways("a 10.0.0.1 all max=8M extra")
            .unwrap_err()
            .contains("optionally a video limit")
    );
}

#[test]
fn the_limit_is_the_way_the_connection_comes_in_by() {
    let mut ways = WaysIn {
        ways: Some(parse_ways("home 192.168.0.0/24 all; vps 10.20.0.1 all max=8M").unwrap()),
        proxies: parse_nets("10.20.0.1").unwrap(),
    };
    assert_eq!(
        ways.video_limit(ip("10.20.0.1")),
        VideoLimit(Some(8_000_000))
    );
    assert_eq!(ways.video_limit(ip("192.168.0.5")), VideoLimit(None));
    assert_eq!(
        ways.video_limit(ip("127.0.0.1")),
        VideoLimit(None),
        "inside the container"
    );
    assert_eq!(
        ways.video_limit(ip("203.0.113.9")),
        VideoLimit(None),
        "refused anyway"
    );
    ways.ways = None;
    assert_eq!(ways.video_limit(ip("10.20.0.1")), VideoLimit(None));
}

#[tokio::test]
async fn the_guard_says_the_limit_and_a_forwarded_client_doesnt_change_it() {
    use axum::Router;
    use axum::body::Body;
    use axum::routing::get;
    use tower::ServiceExt;
    let ways = WaysIn {
        ways: Some(parse_ways("home 192.168.0.0/24 all; vps 10.20.0.1 all max=8M").unwrap()),
        proxies: parse_nets("10.20.0.1").unwrap(),
    };
    let app = Router::new()
        .route(
            "/api/v1/movies",
            get(
                |axum::Extension(VideoLimit(limit)): axum::Extension<VideoLimit>| async move {
                    format!("{limit:?}")
                },
            ),
        )
        .layer(axum::middleware::from_fn_with_state(
            std::sync::Arc::new(ways),
            guard,
        ));
    for (from, forwarded, expected) in [
        ("10.20.0.1", Some("192.168.0.5"), "Some(8000000)"),
        ("192.168.0.5", None, "None"),
    ] {
        let mut request = axum::http::Request::get("/api/v1/movies")
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(ip(from), 50_000)));
        if let Some(f) = forwarded {
            request
                .headers_mut()
                .insert("x-forwarded-for", f.parse().unwrap());
        }
        let response = app.clone().oneshot(request).await.unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 10)
            .await
            .unwrap();
        assert_eq!(bytes, expected, "from {from}");
    }
}
