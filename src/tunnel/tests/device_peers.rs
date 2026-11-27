//! Household devices on the tunnel, end to end where it matters: a device's peer reaches its
//! own parts, keeps its address across a restart, and goes with its row.

use super::*;
use crate::tunnel::devices::{self, Added};
use crate::ways_in::Part;

const BOB: i64 = 2;

/// What the tunnel serves here: one route of each part, and the page.
fn parts_app() -> Router {
    Router::new()
        .route("/api/v1/music/artists", get(|| async { "the artists" }))
        .route("/api/v1/movies", get(|| async { "the films" }))
        .route("/api/v1/me", get(|| async { "you" }))
        .route("/api/v1/party", get(|| async { "a party" }))
        .route("/", get(|| async { "the page" }))
}

fn settings() -> Settings {
    Settings {
        port: 0,
        endpoint: "melete.example:51820".into(),
        net: (Ipv4Addr::new(10, 73, 0, 0), 24),
    }
}

/// A database with alice and bob, and the tunnel on.
async fn household() -> (SqlitePool, Arc<Tunnel>) {
    let pool = crate::db::open_in_memory().await;
    let alice = crate::auth::testing::user(&pool, "alice", false).await;
    let bob = crate::auth::testing::user(&pool, "bob", false).await;
    assert_eq!((alice.id, bob.id), (ALICE, BOB));
    let tunnel = Tunnel::start(&pool, settings(), parts_app()).await.unwrap();
    (pool, tunnel)
}

fn new(user_id: i64, parts: &[Part], public_key: Option<[u8; 32]>) -> NewDevice {
    NewDevice {
        user_id,
        name: "Living room iPad".into(),
        parts: parts.to_vec(),
        public_key,
    }
}

async fn add(tunnel: &Tunnel, pool: &SqlitePool, device: NewDevice) -> Added {
    tunnel.add_device(pool, device).await.unwrap().unwrap()
}

/// The config a device that made its own key, `secret`, writes from what adding it answered.
fn own_config(tunnel: &Tunnel, added: &Added, secret: &StaticSecret) -> Made {
    let config = format!(
        "[Interface]\nPrivateKey = {}\nAddress = {}/32\n\n[Peer]\nPublicKey = {}\n\
         PresharedKey = {}\nAllowedIPs = {}/32\nEndpoint = {}\n",
        keys::encode(&secret.to_bytes()),
        added.address,
        tunnel.own_public_key(),
        added.preshared_key,
        tunnel.own_address(),
        tunnel.endpoint(),
    );
    Made {
        address: added.address,
        config,
    }
}

async fn status(client: &mut Client, path: &str) -> String {
    let answer = client.ask((OWN, PORT), &get_request(path), WAIT).await;
    let answer = answer.unwrap_or_else(|| panic!("no connection for {path}"));
    answer.lines().next().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn a_device_reaches_its_own_parts_and_what_every_way_does_and_nothing_else() {
    let (pool, tunnel) = household().await;
    let added = add(&tunnel, &pool, new(ALICE, &[Part::Music], None)).await;
    let config = added
        .config
        .clone()
        .expect("Melete made the key, so the config");
    assert!(!format!("{added:?}").contains(&added.preshared_key));
    let made = Made {
        address: added.address,
        config,
    };
    let mut client = Client::new(&made, tunnel.port().await, own_only()).await;
    for (path, answer) in [
        ("/api/v1/music/artists", "HTTP/1.1 200 OK"),
        ("/api/v1/me", "HTTP/1.1 200 OK"),
        ("/", "HTTP/1.1 200 OK"),
        ("/api/v1/movies", "HTTP/1.1 403 Forbidden"),
        ("/api/v1/party", "HTTP/1.1 403 Forbidden"),
    ] {
        assert_eq!(status(&mut client, path).await, answer, "{path}");
    }
}

#[tokio::test]
async fn a_device_with_its_own_key_gets_no_config_and_still_connects() {
    let (pool, tunnel) = household().await;
    let secret = StaticSecret::from([9; 32]);
    let public = PublicKey::from(&secret).to_bytes();
    let added = add(&tunnel, &pool, new(ALICE, &[Part::Video], Some(public))).await;
    assert_eq!(added.config, None, "its private key is the device's own");
    let made = own_config(&tunnel, &added, &secret);
    let mut client = Client::new(&made, tunnel.port().await, own_only()).await;
    assert_eq!(
        status(&mut client, "/api/v1/movies").await,
        "HTTP/1.1 200 OK"
    );
    // The same key again, or Melete's own, is refused.
    for taken in [public, key(&tunnel.own_public_key())] {
        let again = tunnel.add_device(&pool, new(BOB, &[Part::Video], Some(taken)));
        assert_eq!(again.await.unwrap().unwrap_err(), Refusal::KeyInUse);
    }
}

#[tokio::test]
async fn a_device_comes_back_after_a_restart_at_the_same_address_with_its_key_sealed() {
    let pool = crate::db::open_in_memory().await;
    sqlx::query("INSERT INTO secrets_sealed (id, sealed_at) VALUES (1, 0)")
        .execute(&pool)
        .await
        .unwrap();
    crate::auth::testing::user(&pool, "alice", false).await;
    let first = Tunnel::start(&pool, settings(), parts_app()).await.unwrap();
    // A guest's peer first: the device's address comes after it.
    first
        .add_guest(1, vec![1; 32], ALICE)
        .await
        .unwrap()
        .unwrap();
    let added = add(&first, &pool, new(ALICE, &[Part::Music], None)).await;
    assert_eq!(added.address, Ipv4Addr::new(10, 73, 0, 3));
    let made = Made {
        address: added.address,
        config: added.config.clone().unwrap(),
    };
    let stored: Vec<u8> = sqlx::query_scalar("SELECT preshared_key FROM tunnel_devices")
        .fetch_one(&pool)
        .await
        .unwrap();
    let plain = key(&added.preshared_key);
    assert_ne!(stored.len(), 32);
    assert!(!stored.windows(32).any(|w| w == plain));
    let unsealed = sqlx::query("UPDATE tunnel_devices SET preshared_key = ?")
        .bind(plain.to_vec())
        .execute(&pool)
        .await;
    assert!(unsealed.is_err(), "a plain key is refused");
    drop(first);
    let second = Tunnel::start(&pool, settings(), parts_app()).await.unwrap();
    assert!(matches!(
        second.kind_of(added.address),
        Some(Kind::Device(DevicePeer { id, user_id: ALICE, .. })) if id == added.id
    ));
    let mut client = Client::new(&made, second.port().await, own_only()).await;
    let answer = status(&mut client, "/api/v1/music/artists").await;
    assert_eq!(answer, "HTTP/1.1 200 OK");
}

#[tokio::test]
async fn a_user_has_at_most_ten_devices_and_addresses_kept_for_others_are_skipped() {
    let (pool, tunnel) = household().await;
    // A device kept in the database but not on the tunnel still has its address.
    sqlx::query(
        "INSERT INTO tunnel_devices (user_id, name, public_key, preshared_key, address, video,
             music, requests, admin, created_at)
         VALUES (?, 'Kept', ?, ?, ?, 1, 0, 0, 0, 0)",
    )
    .bind(BOB)
    .bind(vec![3u8; 32])
    .bind(vec![4u8; 32])
    .bind(i64::from(u32::from(Ipv4Addr::new(10, 73, 0, 2))))
    .execute(&pool)
    .await
    .unwrap();
    let mut addresses = Vec::new();
    for _ in 0..MAX_PER_USER {
        addresses.push(
            add(&tunnel, &pool, new(ALICE, &[Part::Video], None))
                .await
                .address,
        );
    }
    assert_eq!(addresses[0], Ipv4Addr::new(10, 73, 0, 3));
    let eleventh = tunnel.add_device(&pool, new(ALICE, &[Part::Video], None));
    assert_eq!(
        eleventh.await.unwrap().unwrap_err(),
        Refusal::TooManyDevices
    );
    // Nothing was left held for it, and bob's count apart.
    let bobs = add(&tunnel, &pool, new(BOB, &[Part::Video], None)).await;
    assert_eq!(bobs.address, Ipv4Addr::new(10, 73, 0, 13));
}

#[tokio::test]
async fn a_deleted_users_devices_go_with_the_sweep_and_others_stay() {
    let (pool, tunnel) = household().await;
    let alices = add(&tunnel, &pool, new(ALICE, &[Part::Music], None)).await;
    let bobs = add(&tunnel, &pool, new(BOB, &[Part::Music], None)).await;
    devices::sweep_once(&tunnel, &pool).await.unwrap();
    assert!(tunnel.kind_of(alices.address).is_some());
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(ALICE)
        .execute(&pool)
        .await
        .unwrap();
    devices::sweep_once(&tunnel, &pool).await.unwrap();
    assert_eq!(tunnel.kind_of(alices.address), None);
    assert!(tunnel.kind_of(bobs.address).is_some());
    // Removing a device takes its own peer alone.
    tunnel.remove_device(bobs.address, bobs.id + 1).await;
    assert!(tunnel.kind_of(bobs.address).is_some());
    tunnel.remove_device(bobs.address, bobs.id).await;
    assert_eq!(tunnel.kind_of(bobs.address), None);
}
