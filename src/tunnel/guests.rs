//! The tunnel's gate, and its watch party guests' peers. Every request through the tunnel
//! comes from a peer's address; the gate refuses one whose peer is gone, holds a guest's peer
//! to the party's routes (`Part::Party`) and the web page's static files, and a household
//! device's to its parts, notes what each request is for (for the admins' tunnel page), and
//! notes the peer on the request ([`Arrival`]) so joining a party can tie the peer to the guest
//! who joined, and signing in can hold a device to its user. A guest's peer is kept while its
//! invite is unused and good, then while that guest is in their party, and removed as soon as
//! the sweep finds otherwise.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sqlx::SqlitePool;

use super::{DevicePeer, GuestPeer, Kind, Peers, Tunnel, devices};
use crate::watch_party::guests;
use crate::ways_in::{Part, part};

/// How often guests' peers, and devices', are checked.
const SWEEP_EVERY: Duration = Duration::from_secs(2);
/// How long a peer whose invite was just used waits for its guest to be tied to it: joining
/// spends the invite a moment before.
const JOINING: Duration = Duration::from_secs(10);

/// The tunnel peer a request came through.
#[derive(Clone)]
pub struct Arrival {
    pub(super) peers: Arc<Peers>,
    pub(super) address: Ipv4Addr,
}

impl Arrival {
    /// Whether the invite hashed `invite_hash` may be used through this peer: a guest's peer
    /// takes the invite it was made with, before anyone has joined through it, and no other.
    pub fn may_join(&self, invite_hash: &[u8]) -> bool {
        matches!(
            self.peers.kind_of(self.address),
            Some(Kind::Guest(guest)) if guest.guest_id.is_none() && guest.invite_hash == invite_hash
        )
    }

    /// Whether guest `guest_id`'s pass may be used through this peer: a guest's peer carries
    /// the guest who joined through it alone.
    pub fn carries(&self, guest_id: i64) -> bool {
        matches!(
            self.peers.kind_of(self.address),
            Some(Kind::Guest(guest)) if guest.guest_id == Some(guest_id)
        )
    }

    /// Whether user `user_id` may be signed in through this peer: a household device's peer
    /// carries its own user alone, and a guest's none.
    pub fn signs_in(&self, user_id: i64) -> bool {
        matches!(
            self.peers.kind_of(self.address),
            Some(Kind::Device(device)) if device.user_id == user_id
        )
    }

    /// The household device this peer is, when it's one.
    pub fn device(&self) -> Option<DevicePeer> {
        match self.peers.kind_of(self.address)? {
            Kind::Device(device) => Some(device),
            Kind::Guest(_) => None,
        }
    }

    /// Ties the peer, when it's a guest's of party `party_id` that no guest has joined
    /// through yet, to `guest_id`, who just did: it lasts as long as they're in the party.
    pub fn joined(&self, party_id: i64, guest_id: i64) {
        let mut peers = self.peers.lock();
        if let Some(entry) = peers.get_mut(&self.address)
            && let Some(Kind::Guest(guest)) = &mut entry.kind
            && guest.party_id == party_id
            && guest.guest_id.is_none()
        {
            guest.guest_id = Some(guest_id);
        }
    }
}

#[cfg(test)]
impl Arrival {
    /// A request through a guest's peer for party `party_id`, made with the invite hashed
    /// `invite_hash`, that no guest has joined through yet.
    pub fn guest_for_testing(party_id: i64, invite_hash: Vec<u8>) -> Arrival {
        let peers = Arc::new(Peers::default());
        let address = Ipv4Addr::new(10, 73, 0, 2);
        let secret = gotatun::x25519::StaticSecret::from([7; 32]);
        let kind = Some(Kind::Guest(GuestPeer {
            party_id,
            invite_hash,
            guest_id: None,
            made_by: 1,
        }));
        let public_key = gotatun::x25519::PublicKey::from(&secret);
        let mut entry = super::Entry::held(public_key);
        entry.kind = kind;
        peers.lock().insert(address, entry);
        Arrival { peers, address }
    }

    /// A request through user `user_id`'s household device, which reaches `parts`.
    pub fn device_for_testing(user_id: i64, parts: Vec<Part>) -> Arrival {
        let peers = Arc::new(Peers::default());
        let address = Ipv4Addr::new(10, 73, 0, 2);
        let secret = gotatun::x25519::StaticSecret::from([8; 32]);
        let kind = Some(Kind::Device(DevicePeer {
            id: 1,
            user_id,
            parts,
        }));
        let public_key = gotatun::x25519::PublicKey::from(&secret);
        let mut entry = super::Entry::held(public_key);
        entry.kind = kind;
        peers.lock().insert(address, entry);
        Arrival { peers, address }
    }
}

/// Lets a request through when its peer is still there and may reach what it asks for.
pub(super) async fn guard(
    State(peers): State<Arc<Peers>>,
    mut request: Request,
    next: Next,
) -> Response {
    let address = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .and_then(|ConnectInfo(from)| match from.ip() {
            IpAddr::V4(v4) => Some(v4),
            IpAddr::V6(_) => None,
        });
    let Some((address, kind)) = address.and_then(|a| Some((a, peers.kind_of(a)?))) else {
        return refused("this tunnel peer has been removed");
    };
    match kind {
        Kind::Guest(_) if !guest_reaches(&request) => {
            return refused("a watch party guest's tunnel reaches their party alone");
        }
        Kind::Guest(_) => {}
        Kind::Device(device) if !device.reaches(part(request.uri().path())) => {
            return refused("this device's tunnel doesn't reach this part of Melete");
        }
        Kind::Device(_) => {}
    }
    peers.note(address, request.uri().path());
    request.extensions_mut().insert(Arrival { peers, address });
    next.run(request).await
}

/// What a guest's peer reaches: the party's routes, and the web page itself (its static files,
/// read only) to follow the party in a browser.
fn guest_reaches(request: &Request) -> bool {
    let path = request.uri().path();
    let page = path == "/" || path.starts_with("/web/");
    let reading = matches!(*request.method(), Method::GET | Method::HEAD);
    part(path) == Part::Party || (page && reading)
}

fn refused(why: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({ "error": why })),
    )
        .into_response()
}

/// Removes guests' peers that are no longer wanted, and devices' whose rows are gone, every
/// [`SWEEP_EVERY`], until the tunnel is gone.
pub(super) async fn sweep(tunnel: Weak<Tunnel>, pool: SqlitePool) {
    let mut spent = HashMap::new();
    loop {
        tokio::time::sleep(SWEEP_EVERY).await;
        let Some(tunnel) = tunnel.upgrade() else {
            return;
        };
        if let Err(e) = sweep_once(&tunnel, &pool, &mut spent, JOINING).await {
            tracing::warn!("checking the tunnel's guests failed: {e}");
        }
        if let Err(e) = devices::sweep_once(&tunnel, &pool).await {
            tracing::warn!("checking the tunnel's household devices failed: {e}");
        }
    }
}

/// One sweep: `spent` notes since when each peer's invite has been used with no guest tied
/// to it, and one waiting longer than `joining` goes.
pub(super) async fn sweep_once(
    tunnel: &Tunnel,
    pool: &SqlitePool,
    spent: &mut HashMap<Ipv4Addr, Instant>,
    joining: Duration,
) -> sqlx::Result<()> {
    let guests: Vec<(Ipv4Addr, GuestPeer)> = tunnel
        .peers
        .lock()
        .iter()
        .filter_map(|(address, entry)| match &entry.kind {
            Some(Kind::Guest(guest)) => Some((*address, guest.clone())),
            _ => None,
        })
        .collect();
    spent.retain(|address, _| guests.iter().any(|(a, _)| a == address));
    for (address, guest) in guests {
        let wanted = match guest.guest_id {
            Some(id) => guests::still_in(pool, id).await?,
            None if guests::invite_open(pool, &guest.invite_hash).await? => true,
            None => spent.entry(address).or_insert_with(Instant::now).elapsed() < joining,
        };
        if !wanted {
            spent.remove(&address);
            tunnel.remove(address).await;
        }
    }
    Ok(())
}
