//! What the tunnel's peers are doing now, for the admins' tunnel page: each peer's last
//! handshake and bytes from WireGuard, its connections open now from the stack, and what its
//! requests are for, which the gate notes. Where a peer connects from, its outside address, is
//! read from WireGuard when asked, given only while the peer is connected, and never kept or
//! logged.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use super::{Kind, Peers, Tunnel};
use crate::ways_in::{Part, part};

/// How recent a peer's last handshake is while it's connected: WireGuard makes a new one every
/// two minutes while packets flow, and a session is refused three minutes after its own.
pub const CONNECTED: Duration = Duration::from_secs(180);
/// How long, in seconds, a request to watch, listen, follow a party or download says what a
/// peer is doing, over the browsing (progress reports, lists) that comes between.
const BUSY_FOR: i64 = 90;

/// What a peer's requests are for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Browsing,
    /// A video stream or a transcode's playlist and segments.
    Watching,
    /// A track's stream.
    Listening,
    /// A watch party guest's routes and stream links.
    InParty,
    /// An offline copy's download.
    Downloading,
}

impl Activity {
    /// What a request for `path` is for.
    pub fn of(path: &str) -> Activity {
        if part(path) == Part::Party {
            return Activity::InParty;
        }
        let api = path.strip_prefix("/api/v1/").unwrap_or_default();
        let under = |prefix: &str| api.starts_with(prefix);
        if under("offline/download/") {
            Activity::Downloading
        } else if under("music/stream/") {
            Activity::Listening
        } else if under("stream/") || under("hls/") {
            Activity::Watching
        } else {
            Activity::Browsing
        }
    }

    /// Its name in the API.
    pub fn name(self) -> &'static str {
        match self {
            Activity::Browsing => "browsing",
            Activity::Watching => "watching",
            Activity::Listening => "listening",
            Activity::InParty => "in_party",
            Activity::Downloading => "downloading",
        }
    }
}

/// A peer's requests, as the gate notes them: when the last came, and the last that wasn't
/// browsing, with when, in Unix seconds.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Seen {
    last: Option<i64>,
    busy: Option<(Activity, i64)>,
    /// Its last handshake as last read from WireGuard, Unix seconds: WireGuard forgets it once
    /// a quiet peer's session lapses, and the page shouldn't then say it never connected.
    handshake: Option<i64>,
}

impl Seen {
    fn note(&mut self, activity: Activity, now: i64) {
        self.last = Some(now);
        if activity != Activity::Browsing {
            self.busy = Some((activity, now));
        }
    }

    /// What the peer is doing at `now`: what its last request that wasn't browsing was for,
    /// within [`BUSY_FOR`], else browsing; None before its first request.
    fn doing(&self, now: i64) -> Option<Activity> {
        self.last?;
        Some(match self.busy {
            Some((activity, at)) if now - at < BUSY_FOR => activity,
            _ => Activity::Browsing,
        })
    }
}

impl Peers {
    /// Notes a request for `path` through the peer at `address`.
    pub(super) fn note(&self, address: Ipv4Addr, path: &str) {
        let now = crate::db::unix_now();
        if let Some(entry) = self.lock().get_mut(&address) {
            entry.seen.note(Activity::of(path), now);
        }
    }
}

/// A peer, now. `Debug` leaves out where it connects from.
pub struct PeerNow {
    pub address: Ipv4Addr,
    pub kind: Kind,
    /// How long since its last handshake, as WireGuard has it or as last seen (its last request
    /// came over one too); None before its first.
    pub last_handshake: Option<Duration>,
    /// Whether WireGuard has a session with it, made within [`CONNECTED`].
    live: bool,
    /// Bytes received from it and sent to it, through WireGuard, since it was added.
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    /// Its connections open now.
    pub connections: usize,
    pub activity: Option<Activity>,
    /// When its last request came, Unix seconds.
    pub last_request_at: Option<i64>,
    /// Where it connects from, while it's connected alone: never to be kept or logged.
    pub endpoint: Option<SocketAddr>,
}

impl PeerNow {
    /// Whether it's connected: WireGuard has a session with it, made within [`CONNECTED`]. A
    /// last handshake remembered from before doesn't count.
    pub fn connected(&self) -> bool {
        self.live
    }
}

impl std::fmt::Debug for PeerNow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerNow")
            .field("address", &self.address)
            .field("last_handshake", &self.last_handshake)
            .field("connections", &self.connections)
            .field("endpoint", &self.endpoint.map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

/// When a peer's last handshake was, Unix seconds: WireGuard's, while it has a session (`live`,
/// how long ago), else the latest of the last one read and its last request, which came over one.
fn latest_handshake(live: Option<Duration>, seen: Seen, now: i64) -> Option<i64> {
    let live = live.map(|ago| now - i64::try_from(ago.as_secs()).unwrap_or(i64::MAX).min(now));
    [live, seen.handshake, seen.last]
        .into_iter()
        .flatten()
        .max()
}

fn secs_ago(now: i64, at: i64) -> Duration {
    Duration::from_secs(u64::try_from(now - at).unwrap_or(0))
}

fn connected(last_handshake: Option<Duration>) -> bool {
    last_handshake.is_some_and(|ago| ago < CONNECTED)
}

/// `endpoint`, while a peer whose last handshake was `last_handshake` ago is connected.
fn while_connected(
    last_handshake: Option<Duration>,
    endpoint: Option<SocketAddr>,
) -> Option<SocketAddr> {
    endpoint.filter(|_| connected(last_handshake))
}

impl Tunnel {
    /// Every peer the tunnel has now, by address, with what WireGuard and the stack know of it.
    pub async fn peers_now(&self) -> Vec<PeerNow> {
        let listed: Vec<_> = self
            .peers
            .lock()
            .iter()
            .filter_map(|(a, e)| Some((*a, e.public_key, e.kind.clone()?, e.seen)))
            .collect();
        let stats = self.device.peers().await;
        let now = crate::db::unix_now();
        let bytes = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
        listed
            .into_iter()
            .map(|(address, public_key, kind, seen)| {
                let found = stats.iter().find(|s| s.peer.public_key == public_key);
                let live = found.and_then(|s| s.stats.last_handshake);
                let endpoint = found.and_then(|s| s.peer.endpoint);
                let latest = latest_handshake(live, seen, now);
                if let Some(at) = latest
                    && let Some(entry) = self.peers.lock().get_mut(&address)
                {
                    entry.seen.handshake = Some(at);
                }
                PeerNow {
                    address,
                    kind,
                    last_handshake: latest.map(|at| secs_ago(now, at)),
                    live: connected(live),
                    rx_bytes: found.map_or(0, |s| bytes(s.stats.rx_bytes)),
                    tx_bytes: found.map_or(0, |s| bytes(s.stats.tx_bytes)),
                    connections: self.open.of(address),
                    activity: seen.doing(now),
                    last_request_at: seen.last,
                    endpoint: while_connected(live, endpoint),
                }
            })
            .collect()
    }

    /// A request through the peer at `address`, for tests outside the tunnel.
    #[cfg(test)]
    pub fn arrival_for_testing(&self, address: Ipv4Addr) -> super::Arrival {
        super::Arrival {
            peers: self.peers.clone(),
            address,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_that_connected_keeps_its_last_handshake_once_its_session_lapses() {
        let now = 1_000_000;
        let quiet = Seen::default();
        // WireGuard's own, while it has a session.
        let live = Some(Duration::from_secs(10));
        assert_eq!(latest_handshake(live, quiet, now), Some(now - 10));
        // Its session lapsed: what was read before, or its last request, whichever is later.
        let read = Seen {
            handshake: Some(now - 400),
            ..Seen::default()
        };
        assert_eq!(latest_handshake(None, read, now), Some(now - 400));
        let asked = Seen {
            last: Some(now - 200),
            handshake: Some(now - 400),
            ..Seen::default()
        };
        assert_eq!(latest_handshake(None, asked, now), Some(now - 200));
        assert_eq!(latest_handshake(None, quiet, now), None, "never connected");
        assert_eq!(secs_ago(now, now - 200), Duration::from_secs(200));
        // Connected only while WireGuard has a session.
        assert!(!connected(None));
    }

    #[test]
    fn a_request_is_for_what_its_path_says() {
        for (path, activity) in [
            ("/api/v1/stream/mls_abc", Activity::Watching),
            ("/api/v1/hls/mlt_x/main.m3u8", Activity::Watching),
            ("/api/v1/music/stream/mls_abc", Activity::Listening),
            ("/api/v1/offline/download/abc", Activity::Downloading),
            ("/api/v1/party/state", Activity::InParty),
            ("/api/v1/stream/mls_gabc", Activity::InParty),
            ("/api/v1/offline", Activity::Browsing),
            ("/api/v1/movies", Activity::Browsing),
            ("/web/app.js", Activity::Browsing),
        ] {
            assert_eq!(Activity::of(path), activity, "{path}");
        }
    }

    #[test]
    fn watching_outlasts_the_browsing_between_for_a_while() {
        let mut seen = Seen::default();
        assert_eq!(seen.doing(100), None);
        seen.note(Activity::Browsing, 100);
        assert_eq!(seen.doing(100), Some(Activity::Browsing));
        seen.note(Activity::Watching, 110);
        // A progress report while it plays.
        seen.note(Activity::Browsing, 120);
        assert_eq!(seen.last, Some(120));
        assert_eq!(seen.doing(120 + BUSY_FOR - 11), Some(Activity::Watching));
        assert_eq!(seen.doing(110 + BUSY_FOR), Some(Activity::Browsing));
    }

    #[test]
    fn where_a_peer_connects_from_is_given_while_it_is_connected_alone() {
        let from = Some(SocketAddr::from(([192, 168, 1, 10], 51000)));
        let ago = |s| Some(Duration::from_secs(s));
        assert_eq!(while_connected(ago(5), from), from);
        assert_eq!(while_connected(ago(179), from), from);
        assert_eq!(while_connected(ago(180), from), None);
        assert_eq!(while_connected(ago(3600), from), None);
        assert_eq!(while_connected(None, from), None);
    }
}
