//! Ways in: which parts of Melete each way of reaching it opens,
//! told apart by the address a connection comes from. A household might reach Melete at home,
//! over its own WireGuard, and through a reverse proxy on a VPS (Caddy or nginx, tunnelled
//! home over WireGuard), and open only music and requests through the VPS. The ways and what
//! each opens are the admin's to set (`MELETE_WAYS_IN`); without them every address reaches
//! everything, as before. With them, an address in none of them reaches nothing.
//!
//! Signing in, one's own account and the web page itself go through every way there is; the
//! rest is video, music, requests or admin. A route nothing here names counts as admin, so a
//! new one is shut rather than left open.
//!
//! Behind a reverse proxy every client arrives from the proxy's address. A proxy that's
//! trusted (`MELETE_TRUSTED_PROXIES`) may say who the client is (`X-Forwarded-For`), for
//! sign-in limits and the log; no one else may. The way in is always the proxy's own.

use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// What a request reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// Signing in and out, one's own account, the web page: through every way.
    Always,
    /// Movies and series: browsing, playing, streaming.
    Video,
    /// The music library, and Aoide's sync and the rest.
    Music,
    /// Asking for things and following them, playlist imports.
    Requests,
    /// Settings, Wanted, users, approvals: what admins do.
    Admin,
}

impl Part {
    fn named(name: &str) -> Option<Part> {
        Some(match name {
            "video" => Part::Video,
            "music" => Part::Music,
            "requests" => Part::Requests,
            "admin" => Part::Admin,
            _ => return None,
        })
    }
}

/// What a request for `path` reaches.
pub fn part(path: &str) -> Part {
    if path == "/" || path.starts_with("/web/") {
        return Part::Always;
    }
    if path.starts_with("/aoide/") {
        return Part::Music;
    }
    let Some(api) = path.strip_prefix("/api/v1/") else {
        return Part::Admin;
    };
    let first = api.split('/').next().unwrap_or_default();
    match first {
        // The event stream carries only what the way opens (`Opens`).
        "health" | "openapi.json" | "auth" | "events" => Part::Always,
        // It measures the way in itself, whichever app asks.
        "speedtest" => Part::Always,
        "me" if api.starts_with("me/continue")
            || api.starts_with("me/progress")
            || api.starts_with("me/next-up")
            || api.starts_with("me/favourites") =>
        {
            Part::Video
        }
        "me" => Part::Always,
        "users" if api == "users/directory" => Part::Always,
        "movies" | "series" if api.ends_with("/match") || api.ends_with("/rescan") => Part::Admin,
        "files" if api.ends_with("/wrong") => Part::Admin,
        "movies" | "series" | "artwork" | "calendar" | "files" | "stream" | "hls" | "browse"
        | "discover" | "items" | "people" | "collections" | "offline" | "home" => Part::Video,
        "music"
            if api.starts_with("music/profiles")
                || api.starts_with("music/transitions/summary")
                || api.starts_with("music/motion-covers/status") =>
        {
            Part::Admin
        }
        "music" | "playlists" => Part::Music,
        // Finding movies and series by describing them; music's AI would get its own arm.
        "ai" => Part::Video,
        "requests"
            if api.starts_with("requests/settings")
                || api.ends_with("/approve")
                || api.ends_with("/decline")
                || api.ends_with("/reopen") =>
        {
            Part::Admin
        }
        "requests" => Part::Requests,
        _ => Part::Admin,
    }
}

/// An address, or a range of them: "192.168.0.0/24", "10.20.0.1", "fd00::/8".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Net {
    addr: IpAddr,
    prefix: u8,
}

impl Net {
    /// An address or a range, as an admin writes it.
    ///
    /// # Errors
    /// What's wrong with `text`, worded for the admin who wrote it.
    pub fn parse(text: &str) -> Result<Net, String> {
        let (addr, prefix) = match text.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (text, None),
        };
        let addr: IpAddr = addr
            .parse()
            .map_err(|_| format!("{text:?} isn't an address or a range like 192.168.0.0/24"))?;
        let most = if addr.is_ipv4() { 32 } else { 128 };
        let prefix =
            match prefix {
                None => most,
                Some(p) => p.parse::<u8>().ok().filter(|p| *p <= most).ok_or_else(|| {
                    format!("{text:?} has a range size that doesn't fit its address")
                })?,
            };
        Ok(Net { addr, prefix })
    }

    /// Whether `ip` is in the range. An IPv4 address written as IPv6 counts as IPv4.
    pub fn contains(&self, ip: IpAddr) -> bool {
        let bits = |a: IpAddr| -> u128 {
            match a {
                IpAddr::V4(v4) => u128::from(u32::from(v4)),
                IpAddr::V6(v6) => u128::from(v6),
            }
        };
        let ip = ip.to_canonical();
        if ip.is_ipv4() != self.addr.is_ipv4() {
            return false;
        }
        let width = if ip.is_ipv4() { 32 } else { 128 };
        if self.prefix == 0 {
            return true;
        }
        let shift = width - u32::from(self.prefix);
        (bits(ip) >> shift) == (bits(self.addr) >> shift)
    }
}

/// One way in: its name, the addresses it comes from, and what it opens.
#[derive(Debug, Clone, PartialEq)]
pub struct Way {
    /// The way's name, as the admin gave it.
    pub name: String,
    /// The addresses connections through it come from.
    pub nets: Vec<Net>,
    /// None for everything.
    pub parts: Option<Vec<Part>>,
    /// The most bits per second a video may take through it; None for no limit.
    pub max_bitrate: Option<i64>,
}

impl Way {
    fn opens(&self, part: Part) -> bool {
        part == Part::Always
            || self
                .parts
                .as_ref()
                .is_none_or(|parts| parts.contains(&part))
    }
}

/// The ways in, as `MELETE_WAYS_IN` lists them: separated by ";", each a name, its addresses
/// (comma-separated), what it opens (comma-separated: video, music, requests, admin, or all),
/// and optionally the most a video may take through it, in bits per second with k, M or G:
/// "home 192.168.0.0/24 all; vps 10.20.0.1 video,music max=8M".
pub fn parse_ways(text: &str) -> Result<Vec<Way>, String> {
    let mut ways: Vec<Way> = Vec::new();
    for entry in text.split(';').map(str::trim).filter(|e| !e.is_empty()) {
        let fields: Vec<&str> = entry.split_whitespace().collect();
        let (name, nets, parts, limit) = match fields[..] {
            [name, nets, parts] => (name, nets, parts, None),
            [name, nets, parts, limit] => (name, nets, parts, Some(limit)),
            _ => {
                return Err(format!(
                    "{entry:?} should be a name, its addresses and what it opens, like \"vps 10.20.0.1 music,requests\", and optionally a video limit like max=8M"
                ));
            }
        };
        let max_bitrate = limit.map(parse_limit).transpose()?;
        if ways.iter().any(|w| w.name == name) {
            return Err(format!("the way in {name:?} is listed twice"));
        }
        let nets = nets
            .split(',')
            .map(Net::parse)
            .collect::<Result<Vec<_>, _>>()?;
        let parts = if parts == "all" {
            None
        } else {
            Some(
                parts
                    .split(',')
                    .map(|p| {
                        Part::named(p).ok_or_else(|| {
                            format!("{p:?} isn't something a way in opens: video, music, requests, admin or all")
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            )
        };
        ways.push(Way {
            name: name.to_owned(),
            nets,
            parts,
            max_bitrate,
        });
    }
    Ok(ways)
}

/// A way's video limit as an admin writes it, "max=8M": bits per second, with k (thousand), M
/// (million) or G (billion), from 100k to 200M, the range a conversion can be asked for.
fn parse_limit(text: &str) -> Result<i64, String> {
    let wrong = || {
        format!("{text:?} should be a video limit like max=8M (bits per second, with k, M or G)")
    };
    let value = text.strip_prefix("max=").ok_or_else(wrong)?;
    let (number, scale) = match value.char_indices().last() {
        Some((i, 'k' | 'K')) => (&value[..i], 1e3),
        Some((i, 'M')) => (&value[..i], 1e6),
        Some((i, 'G')) => (&value[..i], 1e9),
        _ => (value, 1.0),
    };
    let number: f64 = number.parse().map_err(|_| wrong())?;
    let bits = (number * scale).round();
    if !(100_000.0..=200_000_000.0).contains(&bits) {
        return Err(format!(
            "{text:?} is outside what a video can be held to: 100k to 200M bits per second"
        ));
    }
    Ok(bits as i64)
}

/// Addresses or ranges separated by commas, as `MELETE_TRUSTED_PROXIES` lists them.
pub fn parse_nets(text: &str) -> Result<Vec<Net>, String> {
    text.split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(Net::parse)
        .collect()
}

/// The ways in and the proxies trusted to say who their clients are.
#[derive(Debug, Clone, Default)]
pub struct WaysIn {
    /// None when none are set: every address reaches everything.
    pub ways: Option<Vec<Way>>,
    /// Proxies trusted to say who their clients are (`X-Forwarded-For`); nobody else is.
    pub proxies: Vec<Net>,
}

/// What the way a request came in by opens, for a handler that serves several parts at once
/// (the event stream): None for everything.
#[derive(Debug, Clone, PartialEq)]
pub struct Opens(pub Option<Vec<Part>>);

impl Opens {
    /// Whether it opens `part`. [`Part::Always`] is open through every way.
    pub fn has(&self, part: Part) -> bool {
        part == Part::Always || self.0.as_ref().is_none_or(|parts| parts.contains(&part))
    }
}

/// The most bits per second a video may take through the way a request came in by: None for
/// no limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoLimit(pub Option<i64>);

/// The client's own address: the connection's, or what a trusted proxy says it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAddr(pub IpAddr);

impl WaysIn {
    /// Ways in that come from one address that isn't a trusted proxy: most likely a reverse
    /// proxy or tunnel whose clients then all share that address, for sign-in limits and caps
    /// alike. Each with the address.
    pub fn untrusted_single_hosts(&self) -> Vec<(String, IpAddr)> {
        let Some(ways) = &self.ways else {
            return Vec::new();
        };
        ways.iter()
            .flat_map(|w| w.nets.iter().map(move |n| (w, n)))
            .filter(|(_, n)| n.prefix == if n.addr.is_ipv4() { 32 } else { 128 })
            .filter(|(_, n)| !n.addr.is_loopback())
            .filter(|(_, n)| !self.proxies.iter().any(|p| p.contains(n.addr)))
            .map(|(w, n)| (w.name.clone(), n.addr))
            .collect()
    }

    /// Whether a connection from `peer` may reach `part`; why not, when it mayn't.
    pub fn allows(&self, peer: IpAddr, part: Part) -> Result<(), String> {
        let Some(ways) = &self.ways else {
            return Ok(());
        };
        // Inside the container itself: a check run with docker exec.
        if peer.to_canonical().is_loopback() {
            return Ok(());
        }
        match ways
            .iter()
            .find(|w| w.nets.iter().any(|n| n.contains(peer)))
        {
            Some(way) if way.opens(part) => Ok(()),
            Some(way) => Err(format!(
                "this isn't open through the way you came in ({})",
                way.name
            )),
            None => Err("this address isn't one of Melete's ways in".to_owned()),
        }
    }

    /// What a connection from `peer` may reach: everything when no ways are set or it's from
    /// inside the container, else what its way opens.
    pub fn opens(&self, peer: IpAddr) -> Opens {
        let Some(ways) = &self.ways else {
            return Opens(None);
        };
        if peer.to_canonical().is_loopback() {
            return Opens(None);
        }
        match ways
            .iter()
            .find(|w| w.nets.iter().any(|n| n.contains(peer)))
        {
            Some(way) => Opens(way.parts.clone()),
            None => Opens(Some(Vec::new())),
        }
    }

    /// The video limit of the way a connection from `peer` comes in by; none from inside the
    /// container, or when no ways are set.
    pub fn video_limit(&self, peer: IpAddr) -> VideoLimit {
        let way = self
            .ways
            .as_ref()
            .filter(|_| !peer.to_canonical().is_loopback())
            .and_then(|ways| {
                ways.iter()
                    .find(|w| w.nets.iter().any(|n| n.contains(peer)))
            });
        VideoLimit(way.and_then(|w| w.max_bitrate))
    }

    /// Who the client is: a trusted proxy's word, the nearest address in `X-Forwarded-For`
    /// that isn't a trusted proxy too; else the connection's own address.
    pub fn client(&self, peer: IpAddr, headers: &HeaderMap) -> IpAddr {
        let trusted = |ip: IpAddr| self.proxies.iter().any(|n| n.contains(ip));
        if !trusted(peer) {
            return peer;
        }
        let forwarded: Vec<&str> = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(str::trim)
            .collect();
        for hop in forwarded.iter().rev() {
            match hop.parse::<IpAddr>() {
                Ok(ip) if trusted(ip) => continue,
                Ok(ip) => return ip,
                // Garbled: nothing further along can be believed.
                Err(_) => return peer,
            }
        }
        peer
    }
}

/// Lets a request through when its way in opens what it asks for, noting who the client is.
pub async fn guard(
    State(ways): State<std::sync::Arc<WaysIn>>,
    mut request: Request,
    next: Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());
    let Some(peer) = peer else {
        // Only tests serve without addresses; with ways set, an unknown one is shut out.
        if ways.ways.is_some() {
            return refused("this address isn't one of Melete's ways in".into());
        }
        return next.run(request).await;
    };
    let part = part(request.uri().path());
    if let Err(why) = ways.allows(peer, part) {
        // Says which address Melete saw, so a setup can be checked.
        tracing::info!(%peer, path = request.uri().path(), why, "refused by the ways in");
        return refused(why);
    }
    let client = ways.client(peer, request.headers());
    request.extensions_mut().insert(ClientAddr(client));
    request.extensions_mut().insert(ways.opens(peer));
    request.extensions_mut().insert(ways.video_limit(peer));
    next.run(request).await
}

fn refused(why: String) -> Response {
    (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({ "error": why })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use axum::{Extension, Router};
    use tower::ServiceExt;

    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn reads_addresses_and_ranges() {
        let home = Net::parse("192.168.0.0/24").unwrap();
        assert!(home.contains(ip("192.168.0.77")));
        assert!(!home.contains(ip("192.168.1.1")));
        // An IPv4 client as a dual-stack listener sees it.
        assert!(home.contains(ip("::ffff:192.168.0.77")));
        let one = Net::parse("10.20.0.1").unwrap();
        assert!(one.contains(ip("10.20.0.1")) && !one.contains(ip("10.20.0.2")));
        assert!(Net::parse("fd00::/8").unwrap().contains(ip("fd12::1")));
        assert!(!Net::parse("fd00::/8").unwrap().contains(ip("10.0.0.1")));
        assert!(Net::parse("0.0.0.0/0").unwrap().contains(ip("203.0.113.9")));
        assert!(Net::parse("10.0.0.0/33").is_err());
        assert!(Net::parse("melete.example").is_err());
    }

    #[test]
    fn reads_the_ways_in_as_the_admin_writes_them() {
        let ways =
            parse_ways("home 192.168.0.0/24,fd00::/8 all; vps 10.20.0.1 music,requests ;").unwrap();
        assert_eq!(ways.len(), 2);
        assert_eq!((ways[0].nets.len(), ways[0].parts.clone()), (2, None));
        assert_eq!(ways[1].parts, Some(vec![Part::Music, Part::Requests]));
        assert!(
            parse_ways("vps 10.20.0.1")
                .unwrap_err()
                .contains("a name, its addresses")
        );
        assert!(
            parse_ways("vps 10.20.0.1 games")
                .unwrap_err()
                .contains("\"games\" isn't")
        );
        assert!(
            parse_ways("a 10.0.0.1 all; a 10.0.0.2 all")
                .unwrap_err()
                .contains("listed twice")
        );
        assert!(
            parse_ways("a nowhere all")
                .unwrap_err()
                .contains("isn't an address")
        );
        assert_eq!(parse_ways(" ").unwrap(), []);
    }

    /// Every documented route falls in the part its tag says: a route added without one here
    /// would count as admin, and this would say so.
    #[test]
    fn every_documented_route_is_in_the_part_its_tag_says() {
        let spec: serde_json::Value =
            serde_json::from_str(include_str!("api/openapi.json")).unwrap();
        let mut checked = 0;
        for (path, operations) in spec["paths"].as_object().unwrap() {
            for (method, op) in operations.as_object().unwrap() {
                let tag = op["tags"][0].as_str().unwrap();
                let expected = match tag {
                    "System" | "Accounts" => Part::Always,
                    "Browse" | "Playback" => Part::Video,
                    "Music" | "Aoide" => Part::Music,
                    "Requests" => Part::Requests,
                    "Admin" => Part::Admin,
                    other => panic!("{method} {path}: no part for the tag {other:?}"),
                };
                let concrete = path.replace("{", "x").replace("}", "");
                assert_eq!(part(&concrete), expected, "{method} {path} ({tag})");
                checked += 1;
            }
        }
        assert!(checked > 100);
        assert_eq!(part("/"), Part::Always);
        assert_eq!(part("/web/app.js"), Part::Always);
        assert_eq!(
            part("/somewhere/else"),
            Part::Admin,
            "an unknown route is shut"
        );
    }

    fn household() -> WaysIn {
        WaysIn {
            ways: Some(
                parse_ways("home 192.168.0.0/24 all; vps 10.20.0.1 music,requests").unwrap(),
            ),
            proxies: parse_nets("10.20.0.1").unwrap(),
        }
    }

    #[test]
    fn a_way_from_one_untrusted_address_is_pointed_out() {
        let mut ways = household();
        assert!(
            ways.untrusted_single_hosts().is_empty(),
            "the VPS is trusted"
        );
        ways.proxies.clear();
        assert_eq!(
            ways.untrusted_single_hosts(),
            [("vps".to_owned(), ip("10.20.0.1"))],
            "a range like home's isn't a proxy"
        );
        assert!(WaysIn::default().untrusted_single_hosts().is_empty());
    }

    #[test]
    fn each_way_opens_what_its_listed_with() {
        let ways = household();
        for part in [Part::Video, Part::Music, Part::Requests, Part::Admin] {
            assert_eq!(ways.allows(ip("192.168.0.20"), part), Ok(()));
        }
        let vps = ip("10.20.0.1");
        assert_eq!(ways.allows(vps, Part::Music), Ok(()));
        assert_eq!(ways.allows(vps, Part::Requests), Ok(()));
        assert_eq!(
            ways.allows(vps, Part::Always),
            Ok(()),
            "signing in goes through every way"
        );
        assert_eq!(
            ways.allows(vps, Part::Video),
            Err("this isn't open through the way you came in (vps)".into())
        );
        assert!(ways.allows(vps, Part::Admin).is_err());
        assert_eq!(
            ways.allows(ip("203.0.113.9"), Part::Always),
            Err("this address isn't one of Melete's ways in".into())
        );
        assert_eq!(
            ways.allows(ip("127.0.0.1"), Part::Admin),
            Ok(()),
            "inside the container"
        );
        assert_eq!(
            WaysIn::default().allows(ip("203.0.113.9"), Part::Admin),
            Ok(()),
            "none set: as before"
        );
    }

    #[test]
    fn only_a_trusted_proxy_says_who_the_client_is() {
        let ways = household();
        let said = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert("x-forwarded-for", value.parse().unwrap());
            headers
        };
        assert_eq!(
            ways.client(ip("10.20.0.1"), &said("203.0.113.9")),
            ip("203.0.113.9")
        );
        // What the client claimed itself is left of what the proxy saw.
        assert_eq!(
            ways.client(ip("10.20.0.1"), &said("1.2.3.4, 203.0.113.9")),
            ip("203.0.113.9")
        );
        assert_eq!(
            ways.client(ip("192.168.0.20"), &said("203.0.113.9")),
            ip("192.168.0.20"),
            "not a proxy"
        );
        assert_eq!(
            ways.client(ip("10.20.0.1"), &said("garbage")),
            ip("10.20.0.1")
        );
        assert_eq!(
            ways.client(ip("10.20.0.1"), &HeaderMap::new()),
            ip("10.20.0.1")
        );
    }

    #[tokio::test]
    async fn the_guard_lets_through_what_the_way_opens() {
        let app = Router::new()
            .route("/api/v1/music/artists", get(|| async { "music" }))
            .route("/api/v1/movies", get(|| async { "movies" }))
            .route(
                "/api/v1/me",
                get(
                    |Extension(ClientAddr(client)): Extension<ClientAddr>| async move {
                        client.to_string()
                    },
                ),
            )
            .layer(axum::middleware::from_fn_with_state(
                std::sync::Arc::new(household()),
                guard,
            ));
        let send = |path: &'static str, from: &'static str, forwarded: Option<&'static str>| {
            let app = app.clone();
            async move {
                let mut request = HttpRequest::get(path).body(Body::empty()).unwrap();
                request
                    .extensions_mut()
                    .insert(ConnectInfo(SocketAddr::new(from.parse().unwrap(), 50_000)));
                if let Some(f) = forwarded {
                    request
                        .headers_mut()
                        .insert("x-forwarded-for", f.parse().unwrap());
                }
                let response = app.oneshot(request).await.unwrap();
                let status = response.status();
                let bytes = axum::body::to_bytes(response.into_body(), 1 << 16)
                    .await
                    .unwrap();
                (status, String::from_utf8(bytes.to_vec()).unwrap())
            }
        };
        assert_eq!(
            send("/api/v1/music/artists", "10.20.0.1", None).await.0,
            StatusCode::OK
        );
        let (status, body) = send("/api/v1/movies", "10.20.0.1", None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("the way you came in (vps)"), "{body}");
        assert_eq!(
            send("/api/v1/movies", "192.168.0.20", None).await.0,
            StatusCode::OK
        );
        assert_eq!(
            send("/api/v1/movies", "203.0.113.9", None).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            send("/api/v1/me", "10.20.0.1", Some("203.0.113.9")).await,
            (StatusCode::OK, "203.0.113.9".into()),
            "the client, as the trusted proxy says"
        );
        assert_eq!(
            send("/api/v1/me", "192.168.0.20", Some("203.0.113.9"))
                .await
                .1,
            "192.168.0.20",
            "anyone else's word isn't taken"
        );
    }
}

#[cfg(test)]
mod limit_tests;
