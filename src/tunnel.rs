//! Melete's own WireGuard tunnel: someone outside the household reaches Melete with no
//! WireGuard on the router and nothing more of the container than one published UDP port.
//! Melete answers WireGuard itself, in userspace (GotaTun), on `MELETE_TUNNEL_PORT`: no TUN
//! device, no extra privileges. What a peer sends is decrypted into a userspace TCP/IP stack
//! (`netstack`) that knows one address, Melete's own in the tunnel, and one port, where
//! Melete's HTTP API answers; nothing is forwarded or routed anywhere else.
//!
//! Each peer has a fixed address in the tunnel's range, the only one WireGuard accepts its
//! packets from, so a request's address says which peer sent it. Requests reach the API with
//! it as their address, through the ways in (`MELETE_WAYS_IN`) like any other, so a way can be
//! set for the tunnel's range. Before that, the tunnel's own gate refuses a peer it no longer
//! has, and holds a watch party guest's peer to the party's routes, whatever the ways say.
//!
//! Off unless the port is set. Melete's key is made from the OS's randomness and kept sealed;
//! a peer's keys are made the same way, its private key handed once to whoever made it, in the
//! peer's config, and never kept. Peers live in memory: a restart ends them. A removed peer is
//! gone from WireGuard and its connections are dropped at once.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use anyhow::Context;
use axum::Router;
use axum::serve::ListenerExt;
use gotatun::device::{Device, Peer};
use gotatun::udp::socket::UdpSocketFactory;
use gotatun::x25519::{PublicKey, StaticSecret};
use ipnetwork::{IpNetwork, Ipv4Network};
use sqlx::SqlitePool;
use tokio::sync::mpsc;

mod guests;
mod keys;
mod link;
mod netstack;
mod settings;

pub use guests::Arrival;
pub use netstack::PORT;
pub use settings::Settings;

/// The most peers the tunnel has at once.
pub const MAX_PEERS: usize = 250;

type Transports = (UdpSocketFactory, link::ToStack, link::FromStack);

/// What a peer is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A watch party guest's: it reaches their party's routes alone.
    Guest(GuestPeer),
}

/// A guest's peer: made with an invite, and kept while it's unused, then while the guest who
/// joined through it is in their party.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestPeer {
    pub party_id: i64,
    /// The invite's secret, hashed.
    pub invite_hash: Vec<u8>,
    /// The guest who joined through it.
    pub guest_id: Option<i64>,
}

struct Entry {
    public_key: PublicKey,
    kind: Kind,
}

/// The tunnel's peers, by their address in it.
#[derive(Default)]
pub struct Peers(Mutex<BTreeMap<Ipv4Addr, Entry>>);

impl Peers {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<Ipv4Addr, Entry>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What the peer at `address` is for; None when there's none.
    pub fn kind_of(&self, address: Ipv4Addr) -> Option<Kind> {
        self.lock().get(&address).map(|e| e.kind.clone())
    }
}

/// A peer just made: its address, and the config for whoever made it, shown once. `Debug`
/// never shows the config, which holds the peer's private key.
pub struct Made {
    pub address: Ipv4Addr,
    pub config: String,
}

impl std::fmt::Debug for Made {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Made {{ address: {}, config: <redacted> }}",
            self.address
        )
    }
}

/// The tunnel, running.
pub struct Tunnel {
    settings: Settings,
    own_public: PublicKey,
    device: Device<Transports>,
    peers: Arc<Peers>,
    forget: Arc<netstack::Forget>,
}

/// The tunnel once it's started, for the API; empty while it's off.
#[derive(Default)]
pub struct Slot(OnceLock<Arc<Tunnel>>);

impl Slot {
    pub fn get(&self) -> Option<&Arc<Tunnel>> {
        self.0.get()
    }

    pub fn set(&self, tunnel: Arc<Tunnel>) {
        let _ = self.0.set(tunnel);
    }
}

impl Tunnel {
    /// Starts the tunnel as `settings` say, serving `app` to its peers.
    ///
    /// # Errors
    /// When the UDP port can't be opened, or the key can't be read or kept.
    pub async fn start(
        pool: &SqlitePool,
        settings: Settings,
        app: Router,
    ) -> anyhow::Result<Arc<Tunnel>> {
        let own = settings.own_address();
        let (to_stack, from_tunnel) = mpsc::channel(netstack::QUEUE);
        let (to_tunnel, packets) = mpsc::channel(netstack::QUEUE);
        let (accepted, connections) = mpsc::channel(netstack::MAX_CONNECTIONS);
        let secret = keys::own(pool).await?;
        let own_public = PublicKey::from(&secret);
        let device = open(settings.port, secret, (link::ToStack(to_stack), packets)).await?;
        let forget = Arc::new(netstack::Forget::default());
        let joins = netstack::Joins {
            from_tunnel,
            to_tunnel,
            accepted,
        };
        tokio::spawn(netstack::Stack::new(own, settings.net.1, joins, forget.clone()).run());
        let peers = Arc::new(Peers::default());
        let tunnel = Arc::new(Tunnel {
            settings,
            own_public,
            device,
            peers: peers.clone(),
            forget,
        });
        let listener = link::Listener {
            accepted: connections,
            local: SocketAddr::new(own.into(), PORT),
        };
        tokio::spawn(serve(listener, app, peers));
        tokio::spawn(guests::sweep(Arc::downgrade(&tunnel), pool.clone()));
        Ok(tunnel)
    }

    /// Melete's own address in the tunnel.
    pub fn own_address(&self) -> Ipv4Addr {
        self.settings.own_address()
    }

    /// The UDP port it listens on.
    pub async fn port(&self) -> u16 {
        self.device.read(async |d| d.listen_port()).await
    }

    /// Makes a peer for a guest of party `party_id`, coming with the invite hashed as
    /// `invite_hash`; None when the tunnel has all the peers it takes.
    pub async fn add_guest(
        &self,
        party_id: i64,
        invite_hash: Vec<u8>,
    ) -> anyhow::Result<Option<Made>> {
        let kind = Kind::Guest(GuestPeer {
            party_id,
            invite_hash,
            guest_id: None,
        });
        self.add(kind).await
    }

    async fn add(&self, kind: Kind) -> anyhow::Result<Option<Made>> {
        let (secret, public_key) = keys::peer()?;
        let preshared = keys::random()?;
        let address = {
            let mut peers = self.peers.lock();
            let free = self
                .settings
                .peer_addresses()
                .find(|a| !peers.contains_key(a));
            let Some(address) = free.filter(|_| peers.len() < MAX_PEERS) else {
                return Ok(None);
            };
            peers.insert(address, Entry { public_key, kind });
            address
        };
        let peer = Peer::new(public_key)
            .with_allowed_ip(IpNetwork::V4(Ipv4Network::new(address, 32)?))
            .with_preshared_key(preshared);
        let added = self.device.add_peer(peer).await;
        if !matches!(added, Ok(true)) {
            self.peers.lock().remove(&address);
            anyhow::bail!("the tunnel didn't take a new peer");
        }
        let config = self.config(address, &secret, &preshared);
        Ok(Some(Made { address, config }))
    }

    /// Removes the peer at `address`: its requests are refused, WireGuard forgets it, and its
    /// connections are dropped, at once.
    pub async fn remove(&self, address: Ipv4Addr) {
        let entry = self.peers.lock().remove(&address);
        if let Some(entry) = entry {
            let _ = self.device.remove_peer(&entry.public_key).await;
        }
        self.forget.address(address);
    }

    /// The config a peer's owner imports into WireGuard: its key and address, and Melete's
    /// key, address and endpoint. Only Melete's address goes through the tunnel.
    fn config(&self, address: Ipv4Addr, secret: &StaticSecret, preshared: &[u8; 32]) -> String {
        format!(
            "[Interface]\nPrivateKey = {}\nAddress = {address}/32\n\n[Peer]\nPublicKey = {}\n\
             PresharedKey = {}\nAllowedIPs = {}/32\nEndpoint = {}\nPersistentKeepalive = 25\n",
            keys::encode(&secret.to_bytes()),
            keys::encode(self.own_public.as_bytes()),
            keys::encode(preshared),
            self.own_address(),
            self.settings.endpoint,
        )
    }
}

/// WireGuard on UDP `port` with Melete's key, its packets to and from the network stack.
async fn open(
    port: u16,
    secret: StaticSecret,
    (to_stack, packets): (link::ToStack, mpsc::Receiver<Vec<u8>>),
) -> anyhow::Result<Device<Transports>> {
    let from_stack = link::FromStack {
        packets,
        mtu: netstack::MTU,
    };
    let device = gotatun::device::build()
        .with_default_udp()
        .with_listen_port(port)
        .with_ip_pair(to_stack, from_stack)
        .build()
        .await
        .context("starting the tunnel")?;
    // With no peers yet, setting the key is what opens the UDP port.
    device
        .set_private_key(secret)
        .await
        .with_context(|| format!("opening UDP port {port} for the tunnel"))?;
    Ok(device)
}

/// Serves `app` on the tunnel's connections, behind its gate.
async fn serve(listener: link::Listener, app: Router, peers: Arc<Peers>) {
    let app = app.layer(axum::middleware::from_fn_with_state(peers, guests::guard));
    // Through TapIo, each connection's address is its ConnectInfo.
    let served = axum::serve(
        listener.tap_io(|_| {}),
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await;
    if let Err(e) = served {
        tracing::error!("the tunnel's HTTP server stopped: {e}");
    }
}

#[cfg(test)]
mod tests;
