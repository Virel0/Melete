use std::collections::BTreeSet;

use axum::body::Body;
use axum::extract::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::tunnel::{NewDevice, Settings};
use crate::ways_in::Part;

/// The household: alice, who isn't an admin, and carol, who is, each with a token; a party of
/// alice's with a guest, Sam, and an invite no one has used; and the routes with the tunnel on
/// (`on`) and off.
struct Household {
    pool: SqlitePool,
    tunnel: Arc<Tunnel>,
    on: Router,
    off: Router,
    alice: String,
    carol: String,
    party: i64,
    sam: i64,
    unused_invite: Vec<u8>,
}

async fn household() -> Household {
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
    let token = async |name: &str, admin: bool| {
        let user = crate::auth::testing::user(&pool, name, admin).await;
        let kind = crate::auth::TokenKind::Session;
        let (id, token) = crate::auth::issue_token(&pool, user.id, kind, "t")
            .await
            .unwrap();
        (user.id, id, token)
    };
    let (alice_id, alice_token_id, alice) = token("alice", false).await;
    let (_, _, carol) = token("carol", true).await;
    let film = crate::watch_party::State {
        file_id: 1,
        position_ms: 0,
        playing: true,
    };
    let who = (alice_id, alice_token_id);
    let party = crate::watch_party::start(&pool, who, &film, None)
        .await
        .unwrap()
        .unwrap();
    let invite = async || guests::invite(&pool, who, party).await.unwrap().unwrap().0;
    let (used, unused) = (invite().await, invite().await);
    let (_, sam) = guests::accept(&pool, &used, "Sam").await.unwrap().unwrap();
    let settings = Settings {
        port: 0,
        endpoint: "melete.example:51820".into(),
        net: (Ipv4Addr::new(10, 73, 0, 0), 24),
    };
    let tunnel = Tunnel::start(&pool, settings, Router::new()).await.unwrap();
    let slot = Arc::new(Slot::default());
    slot.set(tunnel.clone());
    Household {
        on: routes(pool.clone(), slot),
        off: routes(pool.clone(), Arc::default()),
        tunnel,
        alice,
        carol,
        party,
        sam: sam.id,
        unused_invite: crate::auth::token_hash(&unused),
        pool,
    }
}

impl Household {
    /// Alice's "Living room iPad", reaching music, on the tunnel: its address and id.
    async fn ipad(&self) -> (Ipv4Addr, i64) {
        let new = NewDevice {
            user_id: 1,
            name: "Living room iPad".into(),
            parts: vec![Part::Music],
            public_key: None,
        };
        let added = self.tunnel.add_device(&self.pool, new).await.unwrap();
        let added = added.unwrap();
        (added.address, added.id)
    }

    /// Sam's peer, tied to Sam as joining through it would, and the unused invite's.
    async fn guests(&self) -> (Ipv4Addr, Ipv4Addr) {
        let used = vec![1; 32];
        let sams = self.tunnel.add_guest(self.party, used, 1).await.unwrap();
        let sams = sams.unwrap().address;
        let arrival = self.tunnel.arrival_for_testing(sams);
        arrival.joined(self.party, self.sam);
        let hash = self.unused_invite.clone();
        let unused = self.tunnel.add_guest(self.party, hash, 1).await.unwrap();
        (sams, unused.unwrap().address)
    }
}

async fn send(app: &Router, token: Option<&str>, method: &str, uri: &str) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = request.body(Body::empty()).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

const PEERS: &str = "/api/v1/tunnel/peers";

/// `value`'s fields are exactly `schema`'s in openapi.json.
fn assert_shape(schema: &str, value: &Value) {
    let spec: Value = serde_json::from_str(crate::api::OPENAPI).unwrap();
    let documented: BTreeSet<String> = spec["components"]["schemas"][schema]["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("no schema {schema}"))
        .keys()
        .cloned()
        .collect();
    let actual: BTreeSet<String> = value
        .as_object()
        .unwrap_or_else(|| panic!("{schema}: not an object: {value}"))
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        actual, documented,
        "{schema} fields differ from openapi.json"
    );
}

#[tokio::test]
async fn only_admins_list_and_remove_peers_and_only_with_the_tunnel_on() {
    let h = household().await;
    let (ipad, _) = h.ipad().await;
    let one = format!("{PEERS}/{ipad}");
    for (method, uri) in [("GET", PEERS), ("DELETE", one.as_str())] {
        let (status, _) = send(&h.on, None, method, uri).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method}");
        let (status, _) = send(&h.on, Some(&h.alice), method, uri).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method}");
        let (status, _) = send(&h.off, Some(&h.carol), method, uri).await;
        assert_eq!(status, StatusCode::CONFLICT, "{method}");
    }
    assert!(
        h.tunnel.kind_of(ipad).is_some(),
        "alice's refusal removed nothing"
    );
    let (status, _) = send(&h.on, Some(&h.carol), "GET", PEERS).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn the_list_has_devices_and_guests_with_exactly_the_documented_fields() {
    let h = household().await;
    let (ipad, ipad_id) = h.ipad().await;
    let (sams, unused) = h.guests().await;
    let (status, body) = send(&h.on, Some(&h.carol), "GET", PEERS).await;
    assert_eq!(status, StatusCode::OK);
    assert_shape("TunnelPeers", &body);
    assert_eq!(body["max_peers"], 250);
    let peers = body["peers"].as_array().unwrap();
    let at = |a: Ipv4Addr| {
        peers
            .iter()
            .find(|p| p["address"] == a.to_string())
            .unwrap_or_else(|| panic!("no peer at {a}: {body}"))
    };
    assert_eq!(peers.len(), 3);
    for peer in peers {
        assert_shape("TunnelPeer", peer);
        assert_shape("TunnelPeerUser", &peer["user"]);
        // Nothing has connected: no handshake, nowhere it connects from, nothing asked.
        assert_eq!(peer["connected"], false);
        for empty in ["last_handshake", "endpoint", "activity", "last_request_at"] {
            assert_eq!(peer[empty], Value::Null, "{empty}");
        }
        assert_eq!(
            (peer["rx_bytes"].clone(), peer["connections"].clone()),
            (json!(0), json!(0))
        );
    }
    let device = at(ipad);
    assert_eq!(device["kind"], "device");
    assert_shape("TunnelPeerDevice", &device["device"]);
    assert_eq!(
        device["device"],
        json!({ "id": ipad_id, "name": "Living room iPad", "parts": ["music"] })
    );
    assert_eq!(device["user"], json!({ "id": 1, "username": "alice" }));
    assert_eq!(device["guest"], Value::Null);
    let sam = at(sams);
    assert_eq!(sam["kind"], "guest");
    assert_shape("TunnelPeerGuest", &sam["guest"]);
    assert_eq!(
        sam["guest"],
        json!({ "party_id": h.party, "guest_id": h.sam, "name": "Sam" })
    );
    assert_eq!(sam["user"]["username"], "alice", "who made the invite");
    assert_eq!(sam["device"], Value::Null);
    let waiting = at(unused);
    assert_eq!(
        waiting["guest"]["name"],
        Value::Null,
        "no one has joined through it yet"
    );
    // No key is in the answer.
    let text = body.to_string();
    assert!(!text.contains(&h.tunnel.own_public_key()));
    assert!(!text.to_lowercase().contains("key"), "{text}");
}

#[tokio::test]
async fn removing_a_device_takes_its_peer_and_its_row_and_unknown_addresses_are_refused() {
    let h = household().await;
    let (ipad, _) = h.ipad().await;
    let one = format!("{PEERS}/{ipad}");
    let (status, _) = send(&h.on, Some(&h.carol), "DELETE", &one).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(h.tunnel.kind_of(ipad), None);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM tunnel_devices")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(
        rows, 0,
        "so it doesn't come back when the tunnel starts again"
    );
    for (uri, refused) in [
        (one.clone(), StatusCode::NOT_FOUND),
        (format!("{PEERS}/10.73.0.99"), StatusCode::NOT_FOUND),
        (format!("{PEERS}/10.73.0"), StatusCode::BAD_REQUEST),
    ] {
        let (status, body) = send(&h.on, Some(&h.carol), "DELETE", &uri).await;
        assert_eq!(status, refused, "{uri}");
        assert!(body["error"].is_string());
    }
}

#[tokio::test]
async fn removing_a_guests_peer_ends_their_place_in_the_party_or_its_unused_invite() {
    let h = household().await;
    let (sams, unused) = h.guests().await;
    assert!(guests::still_in(&h.pool, h.sam).await.unwrap());
    assert!(
        guests::invite_open(&h.pool, &h.unused_invite)
            .await
            .unwrap()
    );
    for address in [sams, unused] {
        let uri = format!("{PEERS}/{address}");
        let (status, _) = send(&h.on, Some(&h.carol), "DELETE", &uri).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(h.tunnel.kind_of(address), None);
    }
    assert!(!guests::still_in(&h.pool, h.sam).await.unwrap());
    assert!(
        !guests::invite_open(&h.pool, &h.unused_invite)
            .await
            .unwrap()
    );
}
