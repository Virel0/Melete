//! The admins' view of the tunnel's peers, end to end: what WireGuard, the stack and the gate
//! know of a peer once a real client has connected through it, and that the peer list is an
//! admin's part of Melete for a device.

use tokio::sync::Notify;

use super::*;
use crate::tunnel::devices::Added;
use crate::ways_in::Part;

/// What the tunnel serves here: a list, a film that comes once `film` is told to, and the
/// admins' peer list.
fn app(film: Arc<Notify>) -> Router {
    let stream = move || {
        let film = film.clone();
        async move {
            film.notified().await;
            "the film"
        }
    };
    Router::new()
        .route("/api/v1/music/artists", get(|| async { "the artists" }))
        .route("/api/v1/stream/{token}", get(stream))
        .route("/api/v1/tunnel/peers", get(|| async { "the peers" }))
}

async fn household(film: Arc<Notify>) -> (SqlitePool, Arc<Tunnel>) {
    let pool = crate::db::open_in_memory().await;
    let alice = crate::auth::testing::user(&pool, "alice", true).await;
    assert_eq!(alice.id, ALICE);
    let settings = Settings {
        port: 0,
        endpoint: "melete.example:51820".into(),
        net: (Ipv4Addr::new(10, 73, 0, 0), 24),
    };
    let tunnel = Tunnel::start(&pool, settings, app(film)).await.unwrap();
    (pool, tunnel)
}

/// Alice's device reaching `parts`, and a client connected through it.
async fn device(tunnel: &Tunnel, pool: &SqlitePool, parts: &[Part]) -> (Added, Client) {
    let new = NewDevice {
        user_id: ALICE,
        name: "Living room iPad".into(),
        parts: parts.to_vec(),
        public_key: None,
    };
    let added = tunnel.add_device(pool, new).await.unwrap().unwrap();
    let made = Made {
        address: added.address,
        config: added.config.clone().unwrap(),
    };
    let client = Client::new(&made, tunnel.port().await, own_only()).await;
    (added, client)
}

/// The peer at `address` now.
async fn now(tunnel: &Tunnel, address: Ipv4Addr) -> PeerNow {
    let peers = tunnel.peers_now().await;
    let found = peers.into_iter().find(|p| p.address == address);
    found.unwrap_or_else(|| panic!("no peer at {address}"))
}

/// The peer at `address` once `wanted` holds of it, within [`WAIT`].
async fn once(tunnel: &Tunnel, address: Ipv4Addr, wanted: impl Fn(&PeerNow) -> bool) -> PeerNow {
    let until = tokio::time::Instant::now() + WAIT;
    loop {
        let peer = now(tunnel, address).await;
        if wanted(&peer) {
            return peer;
        }
        assert!(tokio::time::Instant::now() < until, "never came: {peer:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn status(client: &mut Client, path: &str) -> String {
    let answer = client.ask((OWN, PORT), &get_request(path), WAIT).await;
    let answer = answer.unwrap_or_else(|| panic!("no connection for {path}"));
    answer.lines().next().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn a_connected_peers_handshake_bytes_connections_activity_and_endpoint_show() {
    let film = Arc::new(Notify::new());
    let (pool, tunnel) = household(film.clone()).await;
    let (added, mut client) = device(&tunnel, &pool, &[Part::Video, Part::Music]).await;
    let address = added.address;
    let before = now(&tunnel, address).await;
    assert!(matches!(before.kind, Kind::Device(DevicePeer { id, .. }) if id == added.id));
    assert!(!before.connected());
    assert_eq!((before.last_handshake, before.endpoint), (None, None));
    assert_eq!((before.rx_bytes, before.connections), (0, 0));
    assert_eq!((before.activity, before.last_request_at), (None, None));
    // While the film is being asked for, its connection is open and the peer is watching.
    let watching = async {
        let open = once(&tunnel, address, |p| p.connections == 1).await;
        film.notify_one();
        open
    };
    let request = get_request("/api/v1/stream/mls_x");
    let (answer, open) = tokio::join!(client.ask((OWN, PORT), &request, WAIT), watching);
    assert!(answer.unwrap().ends_with("the film"));
    assert_eq!(open.activity, Some(Activity::Watching));
    // A list asked for while it plays doesn't hide that it's watching.
    assert_eq!(
        status(&mut client, "/api/v1/music/artists").await,
        "HTTP/1.1 200 OK"
    );
    let after = now(&tunnel, address).await;
    assert_eq!(after.activity, Some(Activity::Watching));
    let asked = after.last_request_at.unwrap();
    assert!((crate::db::unix_now() - asked).abs() <= 5);
    assert!(after.connected());
    assert!(after.last_handshake.unwrap() < Duration::from_secs(60));
    assert!(after.rx_bytes > 0 && after.tx_bytes > 0);
    // Where it connects from: this machine, as WireGuard saw it, kept out of `Debug`.
    let endpoint = after.endpoint.expect("it's connected");
    assert_eq!(endpoint.ip(), std::net::IpAddr::from(Ipv4Addr::LOCALHOST));
    assert!(!format!("{after:?}").contains("127.0.0.1"));
    // Removed, it's no longer listed and its connections are dropped.
    tunnel.remove(address).await;
    assert!(tunnel.peers_now().await.is_empty());
    let until = tokio::time::Instant::now() + WAIT;
    while tunnel.open.of(address) > 0 {
        assert!(
            tokio::time::Instant::now() < until,
            "its connections stayed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_guest_following_their_party_is_in_party() {
    let (_pool, tunnel, party, invite) = started().await;
    let made = tunnel
        .add_guest(party, crate::auth::token_hash(&invite), ALICE)
        .await
        .unwrap()
        .unwrap();
    let mut client = Client::new(&made, tunnel.port().await, own_only()).await;
    let answer = client
        .ask((OWN, PORT), &get_request("/api/v1/party"), WAIT)
        .await;
    assert!(answer.unwrap().starts_with("HTTP/1.1 200"));
    let guest = now(&tunnel, made.address).await;
    assert!(matches!(guest.kind, Kind::Guest(GuestPeer { party_id, .. }) if party_id == party));
    assert_eq!(guest.activity, Some(Activity::InParty));
    // A refused request is noted as nothing.
    let refused = client
        .ask((OWN, PORT), &get_request("/api/v1/stream/mls_x"), WAIT)
        .await;
    assert!(refused.unwrap().starts_with("HTTP/1.1 403"));
    let guest = now(&tunnel, made.address).await;
    assert_eq!(guest.activity, Some(Activity::InParty));
}

#[tokio::test]
async fn a_device_reaches_the_peer_list_only_with_the_admin_part() {
    let (pool, tunnel) = household(Arc::default()).await;
    let (_, mut music) = device(&tunnel, &pool, &[Part::Music]).await;
    let peers = status(&mut music, "/api/v1/tunnel/peers").await;
    assert_eq!(peers, "HTTP/1.1 403 Forbidden");
    let (_, mut admin) = device(&tunnel, &pool, &[Part::Admin]).await;
    let peers = status(&mut admin, "/api/v1/tunnel/peers").await;
    assert_eq!(peers, "HTTP/1.1 200 OK");
}
