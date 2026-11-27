//! The tunnel's gate, and its watch party guests' peers. Every request through the tunnel
//! comes from a peer's address; the gate refuses one whose peer is gone, holds a guest's peer
//! to the party's routes (`Part::Party`) and the web page's static files, and notes the peer on the request ([`Arrival`]) so
//! joining a party can tie the peer to the guest who joined. A guest's peer is kept while its
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

use super::{GuestPeer, Kind, Peers, Tunnel};
use crate::watch_party::guests;
use crate::ways_in::{Part, part};

/// How often guests' peers are checked.
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
    /// Ties the peer, when it's a guest's of party `party_id` that no guest has joined
    /// through yet, to `guest_id`, who just did: it lasts as long as they're in the party.
    pub fn joined(&self, party_id: i64, guest_id: i64) {
        let mut peers = self.peers.lock();
        if let Some(entry) = peers.get_mut(&self.address)
            && let Kind::Guest(guest) = &mut entry.kind
            && guest.party_id == party_id
            && guest.guest_id.is_none()
        {
            guest.guest_id = Some(guest_id);
        }
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
    }
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

/// Removes guests' peers that are no longer wanted, every [`SWEEP_EVERY`], until the tunnel
/// is gone.
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
        .map(|(address, entry)| match &entry.kind {
            Kind::Guest(guest) => (*address, guest.clone()),
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
