//! Household devices on the tunnel: each a user's, named, with its own peer that lasts. A
//! device's row keeps its public key, the preshared key Melete made for it (sealed), its fixed
//! address and the parts it reaches (`tunnel_devices`); its private key is the device's own
//! when it sent its public key, and otherwise is made here and handed once in its config, never
//! kept. Devices come back on the tunnel when it starts, with the addresses they had; one whose
//! row is gone (removed, or its user deleted) is removed by the sweep.

use std::collections::HashSet;
use std::net::Ipv4Addr;

use gotatun::x25519::PublicKey;
use sqlx::SqlitePool;

use super::{Entry, Kind, MAX_PEERS, Refusal, Tunnel, keys};
use crate::secrets;
use crate::ways_in::Part;

/// The most devices one user has, as the database holds it.
pub const MAX_PER_USER: i64 = 10;

/// The parts a device can be given, by the names the API and `MELETE_WAYS_IN` use.
const PARTS: [(Part, &str); 4] = [
    (Part::Video, "video"),
    (Part::Music, "music"),
    (Part::Requests, "requests"),
    (Part::Admin, "admin"),
];

/// The parts `names` name, in a fixed order, once each.
///
/// # Errors
/// A name that isn't a device's part, or none at all.
pub fn device_parts(names: &[String]) -> Result<Vec<Part>, String> {
    for name in names {
        if !PARTS.iter().any(|(_, n)| n == name) {
            return Err(format!(
                "{name:?} isn't a part a device can reach: video, music, requests or admin"
            ));
        }
    }
    let parts: Vec<Part> = PARTS
        .iter()
        .filter(|(_, n)| names.iter().any(|name| name == n))
        .map(|(p, _)| *p)
        .collect();
    if parts.is_empty() {
        return Err("give the device at least one part: video, music, requests or admin".into());
    }
    Ok(parts)
}

/// A device's part by its name.
pub fn part_name(part: Part) -> &'static str {
    PARTS
        .iter()
        .find(|(p, _)| *p == part)
        .map_or("always", |(_, n)| n)
}

/// A household device's peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePeer {
    pub id: i64,
    /// Whose it is: every signed-in request through it is theirs.
    pub user_id: i64,
    /// What it reaches besides what every way does.
    pub parts: Vec<Part>,
}

impl DevicePeer {
    /// Whether a request for `part` may go through it: sign-in, one's own account and the
    /// web page always (a device has a part of the household's, so signing in is open to it),
    /// its own parts too, and a watch party guest's routes never.
    pub fn reaches(&self, part: Part) -> bool {
        matches!(part, Part::Always | Part::Account)
            || (part != Part::Party && self.parts.contains(&part))
    }
}

/// A device to add.
pub struct NewDevice {
    pub user_id: i64,
    pub name: String,
    pub parts: Vec<Part>,
    /// The device's own public key; None for Melete to make the pair.
    pub public_key: Option<[u8; 32]>,
}

/// A device just added: its preshared key, and its config when Melete made its key pair, are
/// shown this once. `Debug` shows neither.
pub struct Added {
    pub id: i64,
    pub address: Ipv4Addr,
    pub created_at: i64,
    pub preshared_key: String,
    pub config: Option<String>,
}

impl std::fmt::Debug for Added {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Added {{ id: {}, address: {}, keys: <redacted> }}",
            self.id, self.address
        )
    }
}

/// What a device's preshared key is bound to when sealed: its own public key, so a sealed key
/// moved to another device's row doesn't open.
fn sealed_as(public_key: &[u8; 32]) -> String {
    format!("tunnel_devices.preshared_key.{}", keys::encode(public_key))
}

/// An address as the database keeps it, a number.
pub fn stored_address(stored: i64) -> Option<Ipv4Addr> {
    u32::try_from(stored).ok().map(Ipv4Addr::from)
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    user_id: i64,
    public_key: Vec<u8>,
    preshared_key: Vec<u8>,
    address: i64,
    video: bool,
    music: bool,
    requests: bool,
    admin: bool,
}

impl Row {
    fn parts(&self) -> Vec<Part> {
        let on = [self.video, self.music, self.requests, self.admin];
        PARTS
            .iter()
            .zip(on)
            .filter(|(_, on)| *on)
            .map(|((p, _), _)| *p)
            .collect()
    }
}

impl Tunnel {
    /// Adds `new`'s device: kept in the database and on the tunnel at once. Refused when the
    /// tunnel is full, the user has [`MAX_PER_USER`], or its public key is taken.
    pub async fn add_device(
        &self,
        pool: &SqlitePool,
        new: NewDevice,
    ) -> anyhow::Result<Result<Added, Refusal>> {
        let (secret, public_key) = match new.public_key {
            Some(bytes) => (None, PublicKey::from(bytes)),
            None => {
                let (secret, public_key) = keys::peer()?;
                (Some(secret), public_key)
            }
        };
        let preshared = keys::random()?;
        let taken: Vec<Ipv4Addr> = sqlx::query_scalar("SELECT address FROM tunnel_devices")
            .fetch_all(pool)
            .await?
            .into_iter()
            .filter_map(stored_address)
            .collect();
        let own = self.own_public;
        let held = self.hold(public_key, &taken, |peers| {
            let in_use = public_key == own || peers.values().any(|e| e.public_key == public_key);
            in_use.then_some(Refusal::KeyInUse)
        });
        let address = match held {
            Ok(address) => address,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let stored = store(pool, &new, public_key.as_bytes(), address, &preshared).await;
        let (id, created_at) = match stored {
            Ok(Ok(row)) => row,
            Ok(Err(refusal)) => {
                self.peers.lock().remove(&address);
                return Ok(Err(refusal));
            }
            Err(e) => {
                self.peers.lock().remove(&address);
                return Err(e);
            }
        };
        let kind = Kind::Device(DevicePeer {
            id,
            user_id: new.user_id,
            parts: new.parts,
        });
        if let Err(e) = self.admit(address, kind, preshared).await {
            sqlx::query("DELETE FROM tunnel_devices WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await?;
            return Err(e);
        }
        let config = secret.map(|secret| self.config(address, &secret, &preshared));
        Ok(Ok(Added {
            id,
            address,
            created_at,
            preshared_key: keys::encode(&preshared),
            config,
        }))
    }

    /// Removes device `id`'s peer at `address`, when it's still that device's.
    pub async fn remove_device(&self, address: Ipv4Addr, id: i64) {
        let entry = {
            let mut peers = self.peers.lock();
            let ours = matches!(
                peers.get(&address),
                Some(Entry { kind: Some(Kind::Device(d)), .. }) if d.id == id
            );
            if !ours {
                return;
            }
            peers.remove(&address)
        };
        self.forget_peer(address, entry).await;
    }

    /// What the peer at `address` is; None when there's none.
    pub fn kind_of(&self, address: Ipv4Addr) -> Option<Kind> {
        self.peers.kind_of(address)
    }

    /// Melete's public key, as WireGuard's configs write it.
    pub fn own_public_key(&self) -> String {
        keys::encode(self.own_public.as_bytes())
    }

    /// Where peers reach the tunnel, `host:port`.
    pub fn endpoint(&self) -> &str {
        &self.settings.endpoint
    }

    /// Puts a stored device back on the tunnel; why not, when it can't be.
    async fn restore(&self, sealing: bool, row: Row) -> anyhow::Result<Result<(), &'static str>> {
        let Some(address) =
            stored_address(row.address).filter(|a| self.settings.is_peer_address(*a))
        else {
            return Ok(Err("its address isn't in MELETE_TUNNEL_NET"));
        };
        let Ok(public_key) = <[u8; 32]>::try_from(row.public_key.as_slice()) else {
            return Ok(Err("its public key isn't 32 bytes"));
        };
        let what = sealed_as(&public_key);
        let opened = secrets::bytes_from_store(sealing, &what, &row.preshared_key, 32);
        let Some(preshared) = opened.ok().and_then(|k| <[u8; 32]>::try_from(k).ok()) else {
            return Ok(Err(
                "its preshared key can't be opened with this secrets key",
            ));
        };
        let public_key = PublicKey::from(public_key);
        {
            let mut peers = self.peers.lock();
            let clash =
                peers.contains_key(&address) || peers.values().any(|e| e.public_key == public_key);
            if clash || peers.len() >= MAX_PEERS {
                return Ok(Err("the tunnel has no room for it"));
            }
            peers.insert(address, Entry::held(public_key));
        }
        let parts = row.parts();
        let kind = Kind::Device(DevicePeer {
            id: row.id,
            user_id: row.user_id,
            parts,
        });
        Ok(match self.admit(address, kind, preshared).await {
            Ok(()) => Ok(()),
            Err(_) => Err("WireGuard didn't take it"),
        })
    }
}

/// Keeps `new`'s device with its key, address and sealed preshared key: its id and when.
async fn store(
    pool: &SqlitePool,
    new: &NewDevice,
    public_key: &[u8; 32],
    address: Ipv4Addr,
    preshared: &[u8; 32],
) -> anyhow::Result<Result<(i64, i64), Refusal>> {
    let sealing = secrets::sealing(pool).await?;
    let sealed = secrets::bytes_to_store(sealing, &sealed_as(public_key), preshared)?;
    let has = |part| new.parts.contains(&part);
    let now = crate::db::unix_now();
    let inserted: sqlx::Result<i64> = sqlx::query_scalar(
        "INSERT INTO tunnel_devices (user_id, name, public_key, preshared_key, address, video,
             music, requests, admin, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(new.user_id)
    .bind(&new.name)
    .bind(public_key.as_slice())
    .bind(sealed)
    .bind(i64::from(u32::from(address)))
    .bind(has(Part::Video))
    .bind(has(Part::Music))
    .bind(has(Part::Requests))
    .bind(has(Part::Admin))
    .bind(now)
    .fetch_one(pool)
    .await;
    let refused = |e: &sqlx::Error, what: &str| {
        e.as_database_error()
            .is_some_and(|d| d.message().contains(what))
    };
    match inserted {
        Ok(id) => Ok(Ok((id, now))),
        Err(e) if refused(&e, "at most 10 tunnel devices") => Ok(Err(Refusal::TooManyDevices)),
        Err(e) if refused(&e, "tunnel_devices.public_key") => Ok(Err(Refusal::KeyInUse)),
        Err(e) => Err(e.into()),
    }
}

/// Puts every stored device back on the tunnel as it starts; one that can't be is left off,
/// with a warning that says why.
pub(super) async fn load(tunnel: &Tunnel, pool: &SqlitePool) -> anyhow::Result<()> {
    let sealing = secrets::sealing(pool).await?;
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, user_id, public_key, preshared_key, address, video, music, requests, admin
         FROM tunnel_devices ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    let mut loaded = 0;
    for row in rows {
        let id = row.id;
        match tunnel.restore(sealing, row).await? {
            Ok(()) => loaded += 1,
            Err(why) => tracing::warn!(
                device = id,
                "a household device isn't on the tunnel: {why}; remove it and add it again"
            ),
        }
    }
    if loaded > 0 {
        tracing::info!(devices = loaded, "household devices are on the tunnel");
    }
    Ok(())
}

/// Removes the devices' peers whose rows are gone: removed, or their user deleted.
pub(super) async fn sweep_once(tunnel: &Tunnel, pool: &SqlitePool) -> sqlx::Result<()> {
    let devices: Vec<(Ipv4Addr, i64)> = tunnel
        .peers
        .lock()
        .iter()
        .filter_map(|(address, entry)| match &entry.kind {
            Some(Kind::Device(device)) => Some((*address, device.id)),
            _ => None,
        })
        .collect();
    if devices.is_empty() {
        return Ok(());
    }
    let kept: HashSet<i64> = sqlx::query_scalar("SELECT id FROM tunnel_devices")
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    for (address, id) in devices {
        if !kept.contains(&id) {
            tunnel.remove_device(address, id).await;
        }
    }
    Ok(())
}
