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
//! has, holds a watch party guest's peer to the party's routes and the web page, and a
//! household device's to the parts it was given, whatever the ways say.
//!
//! Off unless the port is set. Melete's key is made from the OS's randomness and kept sealed;
//! a guest's peer's keys are made the same way, its private key handed once to whoever made it,
//! in the peer's config, and never kept. Guests' peers live in memory: a restart ends them.
//! Household devices' are kept in the database (`devices`) and come back with the tunnel. A
//! removed peer is gone from WireGuard and its connections are dropped at once.

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

mod devices;
mod guests;
mod keys;
mod link;
mod netstack;
mod settings;

pub use devices::{DevicePeer, MAX_PER_USER, NewDevice, device_parts, part_name, stored_address};
pub use guests::Arrival;
pub use netstack::PORT;
pub use settings::Settings;

/// The most peers the tunnel has at once.
pub const MAX_PEERS: usize = 250;
/// The most guests' peers one household user has for one party that no guest has joined
/// through yet.
pub const UNUSED_PER_USER: usize = 5;

type Transports = (UdpSocketFactory, link::ToStack, link::FromStack);

/// What a peer is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A watch party guest's: it reaches their party's routes and the web page alone.
    Guest(GuestPeer),
    /// A household device's: it reaches the parts it was given, as its user alone.
    Device(DevicePeer),
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
    /// The household user who made it.
    pub made_by: i64,
}

impl GuestPeer {
    /// Whether it's one of `made_by`'s for party `party_id` that no guest has joined through.
    fn unused(&self, party_id: i64, made_by: i64) -> bool {
        self.party_id == party_id && self.made_by == made_by && self.guest_id.is_none()
    }
}

/// Why no peer was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The tunnel has [`MAX_PEERS`].
    Full,
    /// Its maker has [`UNUSED_PER_USER`] for the party that no guest has joined through.
    TooManyUnused,
    /// The user has [`MAX_PER_USER`] devices.
    TooManyDevices,
    /// Another peer, or Melete itself, has the public key.
    KeyInUse,
}

struct Entry {
    public_key: PublicKey,
    /// None while the address is held for a peer being made.
    kind: Option<Kind>,
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
        self.lock().get(&address).and_then(|e| e.kind.clone())
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
        devices::load(&tunnel, pool).await?;
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

    /// Makes a peer, for user `made_by`, for a guest of party `party_id` coming with the
    /// invite hashed as `invite_hash`; refused when the tunnel is full, or the user has as
    /// many unused for the party as they may.
    pub async fn add_guest(
        &self,
        party_id: i64,
        invite_hash: Vec<u8>,
        made_by: i64,
    ) -> anyhow::Result<Result<Made, Refusal>> {
        let (secret, public_key) = keys::peer()?;
        let preshared = keys::random()?;
        let held = self.hold(public_key, &[], |peers| {
            let unused = peers
                .values()
                .filter(|e| matches!(&e.kind, Some(Kind::Guest(g)) if g.unused(party_id, made_by)))
                .count();
            (unused >= UNUSED_PER_USER).then_some(Refusal::TooManyUnused)
        });
        let address = match held {
            Ok(address) => address,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let kind = Kind::Guest(GuestPeer {
            party_id,
            invite_hash,
            guest_id: None,
            made_by,
        });
        self.admit(address, kind, preshared).await?;
        let config = self.config(address, &secret, &preshared);
        Ok(Ok(Made { address, config }))
    }

    /// Holds a free address for a new peer with `public_key`, one not in `taken` either, unless
    /// `refuse` finds a reason not to, or the tunnel is full.
    fn hold(
        &self,
        public_key: PublicKey,
        taken: &[Ipv4Addr],
        refuse: impl FnOnce(&BTreeMap<Ipv4Addr, Entry>) -> Option<Refusal>,
    ) -> Result<Ipv4Addr, Refusal> {
        let mut peers = self.peers.lock();
        if let Some(refusal) = refuse(&peers) {
            return Err(refusal);
        }
        let free = self
            .settings
            .peer_addresses()
            .find(|a| !peers.contains_key(a) && !taken.contains(a));
        let Some(address) = free.filter(|_| peers.len() < MAX_PEERS) else {
            return Err(Refusal::Full);
        };
        let kind = None;
        peers.insert(address, Entry { public_key, kind });
        Ok(address)
    }

    /// Gives the address held for a peer its `kind`, and adds the peer to WireGuard; the
    /// address is let go when WireGuard doesn't take it.
    async fn admit(
        &self,
        address: Ipv4Addr,
        kind: Kind,
        preshared: [u8; 32],
    ) -> anyhow::Result<()> {
        let public_key = {
            let mut peers = self.peers.lock();
            let entry = peers
                .get_mut(&address)
                .context("the tunnel let go of an address held for a new peer")?;
            entry.kind = Some(kind);
            entry.public_key
        };
        let peer = Peer::new(public_key)
            .with_allowed_ip(IpNetwork::V4(Ipv4Network::new(address, 32)?))
            .with_preshared_key(preshared);
        let added = self.device.add_peer(peer).await;
        if !matches!(added, Ok(true)) {
            self.peers.lock().remove(&address);
            anyhow::bail!("the tunnel didn't take a new peer");
        }
        Ok(())
    }

    /// Removes the peer at `address`: its requests are refused, WireGuard forgets it, and its
    /// connections are dropped, at once.
    pub async fn remove(&self, address: Ipv4Addr) {
        let entry = self.peers.lock().remove(&address);
        self.forget_peer(address, entry).await;
    }

    /// WireGuard forgets a peer taken out of [`Peers`], and its connections are dropped.
    async fn forget_peer(&self, address: Ipv4Addr, entry: Option<Entry>) {
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
