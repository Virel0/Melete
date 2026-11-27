//! The tunnel end to end, in-process: a client made with the same WireGuard library and a
//! TCP/IP stack of its own does a real handshake with Melete's tunnel over UDP on this machine
//! and asks HTTP through it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::routing::get;
use base64::Engine;
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::socket::tcp;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr};

use super::*;

const OWN: Ipv4Addr = Ipv4Addr::new(10, 73, 0, 1);
/// The household user in the party, the only one there is.
const ALICE: i64 = 1;

/// What the tunnel serves in these tests: the party's route, saying who asked, and one route
/// of the household's.
fn app() -> Router {
    Router::new()
        .route(
            "/api/v1/party",
            get(|ConnectInfo(from): ConnectInfo<SocketAddr>| async move {
                format!("the party, for {}", from.ip())
            }),
        )
        .route("/api/v1/movies", get(|| async { "the whole library" }))
        .route(
            "/",
            get(|| async { "the page" }).post(|| async { "a change" }),
        )
        .route("/web/app.js", get(|| async { "the page's script" }))
}

/// A tunnel on a port of this machine's choosing, and a party with an open invite.
async fn started() -> (SqlitePool, Arc<Tunnel>, i64, String) {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'films', 'movies', '/f');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'Film', 'Film', 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation)
             VALUES (1, 1, 'Film/a.mkv', 1, 0, 1, 1, 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let user = crate::auth::testing::user(&pool, "alice", false).await;
    assert_eq!(user.id, ALICE);
    let (token, _) =
        crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "tv")
            .await
            .unwrap();
    let film = crate::watch_party::State {
        file_id: 1,
        position_ms: 0,
        playing: true,
    };
    let party = crate::watch_party::start(&pool, (user.id, token), &film, None)
        .await
        .unwrap()
        .unwrap();
    let (invite, _) = crate::watch_party::guests::invite(&pool, (user.id, token), party)
        .await
        .unwrap()
        .unwrap();
    let settings = Settings {
        port: 0,
        endpoint: "melete.example:51820".into(),
        net: (Ipv4Addr::new(10, 73, 0, 0), 24),
    };
    let tunnel = Tunnel::start(&pool, settings, app()).await.unwrap();
    (pool, tunnel, party, invite)
}

fn field<'a>(config: &'a str, key: &str) -> &'a str {
    config
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix(" = "))
        .unwrap_or_else(|| panic!("the config has no {key}"))
}

fn key(text: &str) -> [u8; 32] {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .unwrap();
    bytes.try_into().unwrap()
}

/// A WireGuard client as the config sets it up, sending what's for `through` into the tunnel,
/// with a TCP/IP stack of its own.
struct Client {
    _device: Device<Transports>,
    iface: Interface,
    sockets: SocketSet<'static>,
    queues: netstack::Queues,
    from_tunnel: mpsc::Receiver<Vec<u8>>,
    to_tunnel: mpsc::Sender<Vec<u8>>,
    started: std::time::Instant,
    next_port: u16,
}

impl Client {
    async fn new(made: &Made, port: u16, through: Ipv4Network) -> Client {
        let config = &made.config;
        assert_eq!(field(config, "Endpoint"), "melete.example:51820");
        assert_eq!(field(config, "AllowedIPs"), "10.73.0.1/32");
        assert_eq!(field(config, "Address"), format!("{}/32", made.address));
        let server = Peer::new(PublicKey::from(key(field(config, "PublicKey"))))
            .with_endpoint(SocketAddr::from(([127, 0, 0, 1], port)))
            .with_allowed_ip(IpNetwork::V4(through))
            .with_preshared_key(key(field(config, "PresharedKey")));
        let (to_stack, from_tunnel) = mpsc::channel(netstack::QUEUE);
        let (to_tunnel, packets) = mpsc::channel(netstack::QUEUE);
        let device = gotatun::device::build()
            .with_default_udp()
            .with_ip_pair(
                link::ToStack(to_stack),
                link::FromStack {
                    packets,
                    mtu: netstack::MTU,
                },
            )
            .with_private_key(StaticSecret::from(key(field(config, "PrivateKey"))))
            .with_peer(server)
            .build()
            .await
            .unwrap();
        let mut queues = netstack::Queues::default();
        let mut iface = Interface::new(
            Config::new(HardwareAddress::Ip),
            &mut queues,
            smoltcp::time::Instant::from_millis(0),
        );
        iface.update_ip_addrs(|addrs| {
            addrs
                .push(IpCidr::new(IpAddress::Ipv4(made.address), 24))
                .unwrap();
        });
        // Everything else goes to the tunnel too, as a client that tries its luck would send it.
        iface.routes_mut().add_default_ipv4_route(OWN).unwrap();
        Client {
            _device: device,
            iface,
            sockets: SocketSet::new(Vec::new()),
            queues,
            from_tunnel,
            to_tunnel,
            started: std::time::Instant::now(),
            next_port: 40000,
        }
    }

    fn poll(&mut self) {
        let now = smoltcp::time::Instant::from_millis(
            i64::try_from(self.started.elapsed().as_millis()).unwrap(),
        );
        self.iface.poll(now, &mut self.queues, &mut self.sockets);
        for packet in self.queues.outgoing.drain(..) {
            let _ = self.to_tunnel.try_send(packet);
        }
    }

    /// Sends `request` to `to` over TCP and reads until the other side closes: the answer,
    /// or None when no connection is made within `wait`.
    async fn ask(&mut self, to: (Ipv4Addr, u16), request: &str, wait: Duration) -> Option<String> {
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; 64 * 1024]),
            tcp::SocketBuffer::new(vec![0; 64 * 1024]),
        );
        self.next_port += 1;
        let remote = (IpAddress::Ipv4(to.0), to.1);
        socket
            .connect(self.iface.context(), remote, self.next_port)
            .unwrap();
        let handle = self.sockets.add(socket);
        let until = tokio::time::Instant::now() + wait;
        let (mut connected, mut answer) = (false, Vec::new());
        while tokio::time::Instant::now() < until {
            tokio::select! {
                Some(packet) = self.from_tunnel.recv() => self.queues.incoming.push_back(packet),
                () = tokio::time::sleep(Duration::from_millis(5)) => {}
            }
            while let Ok(packet) = self.from_tunnel.try_recv() {
                self.queues.incoming.push_back(packet);
            }
            self.poll();
            let socket = self.sockets.get_mut::<tcp::Socket>(handle);
            if !connected && socket.may_send() {
                connected = true;
                socket.send_slice(request.as_bytes()).unwrap();
            }
            while socket.can_recv() {
                socket
                    .recv(|buf| {
                        answer.extend_from_slice(buf);
                        (buf.len(), ())
                    })
                    .unwrap();
            }
            let done = connected && !socket.may_recv();
            if done || socket.state() == tcp::State::Closed {
                break;
            }
            self.poll();
        }
        self.sockets.remove(handle);
        connected.then(|| String::from_utf8_lossy(&answer).into_owned())
    }
}

fn get_request(path: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: 10.73.0.1\r\nConnection: close\r\n\r\n")
}

const WAIT: Duration = Duration::from_secs(5);

/// The client sends only what's for Melete's address into the tunnel, as the config says.
fn own_only() -> Ipv4Network {
    Ipv4Network::new(OWN, 32).unwrap()
}

#[tokio::test]
async fn a_guest_reaches_their_party_and_the_page_through_the_tunnel_and_nothing_else() {
    let (_pool, tunnel, party, invite) = started().await;
    let made = tunnel
        .add_guest(party, crate::auth::token_hash(&invite), ALICE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(made.address, Ipv4Addr::new(10, 73, 0, 2));
    assert!(!format!("{made:?}").contains("PrivateKey"));
    let mut client = Client::new(&made, tunnel.port().await, own_only()).await;
    let party = client
        .ask((OWN, PORT), &get_request("/api/v1/party"), WAIT)
        .await
        .unwrap();
    assert!(party.starts_with("HTTP/1.1 200"), "{party}");
    // The API saw the peer's own tunnel address.
    assert!(party.ends_with("the party, for 10.73.0.2"), "{party}");
    let library = client
        .ask((OWN, PORT), &get_request("/api/v1/movies"), WAIT)
        .await
        .unwrap();
    assert!(library.starts_with("HTTP/1.1 403"), "{library}");
    assert!(!library.contains("the whole library"));
    // The web page itself loads, to follow the party in a browser; only read.
    for (path, body) in [("/", "the page"), ("/web/app.js", "the page's script")] {
        let page = client.ask((OWN, PORT), &get_request(path), WAIT).await;
        let page = page.unwrap();
        assert!(
            page.starts_with("HTTP/1.1 200") && page.ends_with(body),
            "{page}"
        );
    }
    let post =
        "POST / HTTP/1.1\r\nHost: 10.73.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let change = client.ask((OWN, PORT), post, WAIT).await.unwrap();
    assert!(change.starts_with("HTTP/1.1 403"), "{change}");
}

#[tokio::test]
async fn a_user_has_at_most_five_unused_tunnel_invites_to_a_party() {
    let (_pool, tunnel, party, _invite) = started().await;
    let hash = |n: u8| vec![n; 32];
    for n in 0..5 {
        let made = tunnel.add_guest(party, hash(n), ALICE).await.unwrap();
        assert!(made.is_ok(), "{n}");
    }
    let sixth = tunnel.add_guest(party, hash(5), ALICE).await.unwrap();
    assert_eq!(sixth.unwrap_err(), Refusal::TooManyUnused);
    // Someone else's, and another party's, count apart.
    assert!(
        tunnel
            .add_guest(party, hash(6), ALICE + 1)
            .await
            .unwrap()
            .is_ok()
    );
    assert!(
        tunnel
            .add_guest(party + 1, hash(7), ALICE)
            .await
            .unwrap()
            .is_ok()
    );
    // Once a guest has joined through one, it's no longer unused.
    let first = Ipv4Addr::new(10, 73, 0, 2);
    let arrival = Arrival {
        peers: tunnel.peers.clone(),
        address: first,
    };
    arrival.joined(party, 41);
    assert!(
        tunnel
            .add_guest(party, hash(8), ALICE)
            .await
            .unwrap()
            .is_ok()
    );
}

#[tokio::test]
async fn a_tunnel_connection_where_nothing_moves_is_ended_and_one_moving_isnt() {
    use std::sync::atomic::Ordering;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let idle = Duration::from_millis(100);
    let (mut stream, ends) = link::pair(Arc::default(), idle);
    let to_server = ends.incoming.clone().unwrap();
    // Bytes keep coming: each read is in time, however long it all takes.
    for _ in 0..4 {
        to_server
            .send(bytes::Bytes::from_static(b"x"))
            .await
            .unwrap();
        let mut one = [0u8; 1];
        stream.read_exact(&mut one).await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    assert!(!ends.timed_out.load(Ordering::SeqCst));
    // Then nothing: the read gives up, and the stack is told to drop the connection.
    let mut one = [0u8; 1];
    let err = stream.read_exact(&mut one).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    assert!(ends.timed_out.load(Ordering::SeqCst));
    // An answer the peer never takes (the stack's queue full) gives up the same way.
    let (mut stream, ends) = link::pair(Arc::default(), idle);
    let err = stream
        .write_all(&vec![0u8; 8 * link::CHUNK])
        .await
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    assert!(ends.timed_out.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_removed_peer_is_refused_at_once() {
    let (_pool, tunnel, party, invite) = started().await;
    let made = tunnel
        .add_guest(party, crate::auth::token_hash(&invite), ALICE)
        .await
        .unwrap()
        .unwrap();
    let mut client = Client::new(&made, tunnel.port().await, own_only()).await;
    let before = client
        .ask((OWN, PORT), &get_request("/api/v1/party"), WAIT)
        .await;
    assert!(before.unwrap().starts_with("HTTP/1.1 200"));
    tunnel.remove(made.address).await;
    assert_eq!(tunnel.peers.kind_of(made.address), None);
    let after = client
        .ask(
            (OWN, PORT),
            &get_request("/api/v1/party"),
            Duration::from_secs(2),
        )
        .await;
    assert_eq!(after, None);
}

#[tokio::test]
async fn nothing_but_meletes_own_http_is_reachable() {
    // Something on this machine that a forwarding tunnel would let a peer reach.
    let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_port = local.local_addr().unwrap().port();
    let reached = Arc::new(AtomicBool::new(false));
    let noted = reached.clone();
    tokio::spawn(async move {
        while local.accept().await.is_ok() {
            noted.store(true, Ordering::SeqCst);
        }
    });
    let (_pool, tunnel, party, invite) = started().await;
    let made = tunnel
        .add_guest(party, crate::auth::token_hash(&invite), ALICE)
        .await
        .unwrap()
        .unwrap();
    // A client that sends everything into the tunnel, whatever its config says.
    let everything = Ipv4Network::new(Ipv4Addr::UNSPECIFIED, 0).unwrap();
    let mut client = Client::new(&made, tunnel.port().await, everything).await;
    let melete = client
        .ask((OWN, PORT), &get_request("/api/v1/party"), WAIT)
        .await;
    assert!(melete.unwrap().starts_with("HTTP/1.1 200"));
    let elsewhere = [
        (Ipv4Addr::LOCALHOST, local_port),
        (OWN, local_port),
        (OWN, 8100),
        (OWN, 22),
        (Ipv4Addr::new(192, 168, 1, 10), 80),
        (Ipv4Addr::new(10, 73, 0, 3), 80),
    ];
    for to in elsewhere {
        let answer = client
            .ask(to, &get_request("/"), Duration::from_millis(1500))
            .await;
        assert_eq!(answer, None, "{to:?}");
    }
    assert!(!reached.load(Ordering::SeqCst));
}

/// An IPv4 packet from a peer to `to`, carrying `protocol` with a TCP or UDP header to `port`.
fn packet(to: Ipv4Addr, protocol: u8, port: u16, fragment: bool) -> Vec<u8> {
    let mut p = vec![0x45, 0, 0, 40, 0, 0, 0, 0, 64, protocol, 0, 0];
    if fragment {
        p[6] = 0x20;
    }
    p.extend_from_slice(&[10, 73, 0, 2]);
    p.extend_from_slice(&to.octets());
    p.extend_from_slice(&40000u16.to_be_bytes());
    p.extend_from_slice(&port.to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0x50, 0x02, 0xff, 0xff, 0, 0, 0, 0]);
    p
}

#[test]
fn the_stack_takes_only_tcp_to_meletes_own_port() {
    assert!(netstack::admits(OWN, &packet(OWN, 6, PORT, false)));
    assert!(!netstack::admits(OWN, &packet(OWN, 6, 22, false)));
    assert!(!netstack::admits(OWN, &packet(OWN, 17, PORT, false)));
    assert!(!netstack::admits(OWN, &packet(OWN, 1, PORT, false)));
    assert!(!netstack::admits(OWN, &packet(OWN, 6, PORT, true)));
    let lan = Ipv4Addr::new(192, 168, 1, 10);
    assert!(!netstack::admits(OWN, &packet(lan, 6, PORT, false)));
    assert!(!netstack::admits(OWN, &[0x60; 40]));
    assert!(!netstack::admits(OWN, &[]));
}

#[tokio::test]
async fn a_guests_peer_lasts_while_its_invite_is_open_then_while_the_guest_is_in() {
    let (pool, tunnel, party, invite) = started().await;
    let peer = |made: &Made| made.address;
    let unused = tunnel
        .add_guest(party, crate::auth::token_hash(&invite), ALICE)
        .await
        .unwrap()
        .unwrap();
    let mut spent = HashMap::new();
    let sweep = async |spent: &mut HashMap<Ipv4Addr, std::time::Instant>| {
        guests::sweep_once(&tunnel, &pool, spent, Duration::ZERO)
            .await
            .unwrap();
    };
    sweep(&mut spent).await;
    assert!(tunnel.peers.kind_of(peer(&unused)).is_some());
    // The invite is used with no guest tied to the peer: it goes.
    let (_, someone) = crate::watch_party::guests::accept(&pool, &invite, "Sam")
        .await
        .unwrap()
        .unwrap();
    sweep(&mut spent).await;
    assert_eq!(tunnel.peers.kind_of(peer(&unused)), None);
    // A guest who joined through their peer keeps it until they leave.
    let user: i64 = sqlx::query_scalar("SELECT id FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    let token: i64 = sqlx::query_scalar("SELECT id FROM tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    let (second, _) = crate::watch_party::guests::invite(&pool, (user, token), party)
        .await
        .unwrap()
        .unwrap();
    let used = tunnel
        .add_guest(party, crate::auth::token_hash(&second), ALICE)
        .await
        .unwrap()
        .unwrap();
    let (_, guest) = crate::watch_party::guests::accept(&pool, &second, "Kim")
        .await
        .unwrap()
        .unwrap();
    let arrival = Arrival {
        peers: tunnel.peers.clone(),
        address: peer(&used),
    };
    // Tied only to a guest of its own party.
    arrival.joined(party + 1, someone.id);
    arrival.joined(party, guest.id);
    sweep(&mut spent).await;
    assert!(matches!(
        tunnel.peers.kind_of(peer(&used)),
        Some(Kind::Guest(GuestPeer { guest_id: Some(id), .. })) if id == guest.id
    ));
    crate::watch_party::guests::leave(&pool, guest.id)
        .await
        .unwrap();
    sweep(&mut spent).await;
    assert_eq!(tunnel.peers.kind_of(peer(&used)), None);
}
