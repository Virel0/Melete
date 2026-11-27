use std::net::Ipv4Addr;

use axum::body::Body;
use axum::extract::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::tunnel::{Settings, Tunnel};

/// The household: alice and bob, and carol who's an admin, each with a token; and the
/// devices' routes with the tunnel on (`on`) and off.
struct Household {
    pool: SqlitePool,
    tunnel: Arc<Tunnel>,
    on: Router,
    off: Router,
    alice: (i64, String),
    bob: (i64, String),
    carol: String,
}

async fn household() -> Household {
    let pool = crate::db::open_in_memory().await;
    let token = async |name: &str, admin: bool| {
        let user = crate::auth::testing::user(&pool, name, admin).await;
        let kind = crate::auth::TokenKind::Session;
        let (_, token) = crate::auth::issue_token(&pool, user.id, kind, "t")
            .await
            .unwrap();
        (user.id, token)
    };
    let alice = token("alice", false).await;
    let bob = token("bob", false).await;
    let (_, carol) = token("carol", true).await;
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
        pool,
        tunnel,
        alice,
        bob,
        carol,
    }
}

async fn send(
    app: &Router,
    (token, arrival): (&str, Option<Arrival>),
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    if let Some(arrival) = arrival {
        request.extensions_mut().insert(arrival);
    }
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

const DEVICES: &str = "/api/v1/tunnel/devices";

fn key_text(bytes: [u8; 32]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn key(value: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(value.as_str().unwrap())
        .unwrap()
}

#[tokio::test]
async fn adding_takes_a_name_known_parts_and_a_real_public_key() {
    let h = household().await;
    let alice = (h.alice.1.as_str(), None);
    let wrong = [
        json!({ "name": "", "parts": ["music"] }),
        json!({ "name": "x".repeat(61), "parts": ["music"] }),
        json!({ "name": "Two\nlines", "parts": ["music"] }),
        json!({ "name": "Phone", "parts": [] }),
        json!({ "name": "Phone", "parts": ["party"] }),
        json!({ "name": "Phone", "parts": ["music"], "public_key": "not base64!" }),
        json!({ "name": "Phone", "parts": ["music"], "public_key": key_text([5; 32])[..40] }),
        json!({ "name": "Phone", "parts": ["music"], "public_key": key_text([0; 32]) }),
    ];
    for body in wrong {
        let (status, _) = send(&h.on, alice.clone(), "POST", DEVICES, body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
    let body = json!({ "name": " Phone ", "parts": ["requests", "music", "music"],
                       "public_key": key_text([5; 32]) });
    let (status, own) = send(&h.on, alice.clone(), "POST", DEVICES, body).await;
    assert_eq!(status, StatusCode::CREATED, "{own}");
    assert_eq!(own["name"], "Phone");
    assert_eq!(own["parts"], json!(["music", "requests"]));
    assert_eq!(
        own["wireguard_config"],
        Value::Null,
        "the device made its own key"
    );
    assert_eq!(own["server_public_key"], json!(h.tunnel.own_public_key()));
    assert_eq!(own["endpoint"], "melete.example:51820");
    assert_eq!(own["server"], "http://10.73.0.1");
    assert_eq!(key(&own["preshared_key"]).len(), 32);
    let body = json!({ "name": "TV", "parts": ["video"] });
    let (status, made) = send(&h.on, alice.clone(), "POST", DEVICES, body).await;
    assert_eq!(status, StatusCode::CREATED);
    let config = made["wireguard_config"].as_str().unwrap();
    let preshared = format!("PresharedKey = {}", made["preshared_key"].as_str().unwrap());
    assert!(config.contains("PrivateKey = ") && config.contains(&preshared));
    // With the tunnel off, there's nowhere to add one.
    let body = json!({ "name": "TV", "parts": ["video"] });
    let (status, _) = send(&h.off, alice, "POST", DEVICES, body).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn the_list_is_ones_own_devices_with_no_keys() {
    let h = household().await;
    let alice = (h.alice.1.as_str(), None);
    let bob = (h.bob.1.as_str(), None);
    let body = json!({ "name": "Phone", "parts": ["music"], "public_key": key_text([5; 32]) });
    let (_, added) = send(&h.on, alice.clone(), "POST", DEVICES, body).await;
    let body = json!({ "name": "TV", "parts": ["video"] });
    let (_, made) = send(&h.on, alice.clone(), "POST", DEVICES, body).await;
    let body = json!({ "name": "Bob's", "parts": ["video"] });
    send(&h.on, bob.clone(), "POST", DEVICES, body).await;
    let (status, listed) = send(&h.on, alice, "GET", DEVICES, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["enabled"], true);
    let devices = listed["devices"].as_array().unwrap();
    let names: Vec<&str> = devices
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Phone", "TV"]);
    let fields: Vec<&str> = devices[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(fields, ["address", "created_at", "id", "name", "parts"]);
    assert_eq!(devices[0]["address"], added["address"]);
    let text = listed.to_string();
    for secret in [&added["preshared_key"], &made["preshared_key"]] {
        assert!(!text.contains(secret.as_str().unwrap()));
    }
    assert!(!text.contains(&key_text([5; 32])) && !text.contains("PrivateKey"));
    let (_, off) = send(&h.off, bob, "GET", DEVICES, Value::Null).await;
    assert_eq!(off["enabled"], false);
    assert_eq!(off["devices"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_device_is_removed_by_its_owner_or_an_admin_and_its_peer_at_once() {
    let h = household().await;
    let alice = (h.alice.1.as_str(), None);
    let mut added = Vec::new();
    for name in ["Phone", "TV"] {
        let body = json!({ "name": name, "parts": ["video"] });
        let (_, device) = send(&h.on, alice.clone(), "POST", DEVICES, body).await;
        let address: Ipv4Addr = device["address"].as_str().unwrap().parse().unwrap();
        added.push((device["id"].as_i64().unwrap(), address));
    }
    let [(phone, phone_at), (tv, tv_at)] = added[..] else {
        unreachable!()
    };
    let uri = |id: i64| format!("{DEVICES}/{id}");
    let bob = (h.bob.1.as_str(), None);
    let (status, _) = send(&h.on, bob, "DELETE", &uri(phone), Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "not bob's");
    assert!(h.tunnel.kind_of(phone_at).is_some());
    let (status, _) = send(&h.on, alice, "DELETE", &uri(phone), Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(h.tunnel.kind_of(phone_at), None);
    let carol = (h.carol.as_str(), None);
    let (status, _) = send(&h.on, carol, "DELETE", &uri(tv), Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "an admin removes anyone's");
    assert_eq!(h.tunnel.kind_of(tv_at), None);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM tunnel_devices")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn through_a_device_only_its_own_user_is_signed_in() {
    let h = household().await;
    let device = || Some(Arrival::device_for_testing(h.alice.0, vec![Part::Music]));
    let (status, _) = send(&h.on, (&h.alice.1, device()), "GET", DEVICES, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(&h.on, (&h.bob.1, device()), "GET", DEVICES, Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("another user's device")
    );
    // An admin is no exception.
    let (status, _) = send(&h.on, (&h.carol, device()), "GET", DEVICES, Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Nor does a device's peer carry a watch party guest, or take an invite.
    let arrival = device().unwrap();
    assert!(!arrival.carries(1) && !arrival.may_join(&[0; 32]));
    // A guest's peer signs in no household user.
    let guest = Some(Arrival::guest_for_testing(1, vec![0; 32]));
    let (status, _) = send(&h.on, (&h.alice.1, guest), "GET", DEVICES, Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn through_a_device_a_new_device_reaches_no_more_than_it_does() {
    let h = household().await;
    let device = || Some(Arrival::device_for_testing(h.alice.0, vec![Part::Music]));
    let wider = json!({ "name": "Tablet", "parts": ["music", "video"] });
    let (status, _) = send(&h.on, (&h.alice.1, device()), "POST", DEVICES, wider).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let same = json!({ "name": "Tablet", "parts": ["music"] });
    let (status, _) = send(&h.on, (&h.alice.1, device()), "POST", DEVICES, same).await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn the_eleventh_device_is_refused() {
    let h = household().await;
    let alice = (h.alice.1.as_str(), None);
    for n in 0..MAX_PER_USER {
        let body = json!({ "name": format!("Device {n}"), "parts": ["video"] });
        let (status, _) = send(&h.on, alice.clone(), "POST", DEVICES, body).await;
        assert_eq!(status, StatusCode::CREATED);
    }
    let body = json!({ "name": "One more", "parts": ["video"] });
    let (status, _) = send(&h.on, alice, "POST", DEVICES, body).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn a_device_reaches_no_more_than_the_way_it_was_added_through_opens() {
    let h = household().await;
    let through = |opens: Vec<Part>, parts: Value| {
        let mut request = Request::post(DEVICES)
            .header("authorization", format!("Bearer {}", h.alice.1))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "name": "Laptop", "parts": parts }).to_string(),
            ))
            .unwrap();
        request.extensions_mut().insert(Opens(Some(opens)));
        request
    };
    let status = async |request: Request| h.on.clone().oneshot(request).await.unwrap().status();
    // A way that opens watch parties alone, a VPS's say: signing in works there, but a device
    // made through it can't reach the library.
    let party_way = through(vec![Part::Party], json!(["video"]));
    assert_eq!(status(party_way).await, StatusCode::FORBIDDEN);
    let (_, listed) = send(&h.on, (&h.alice.1, None), "GET", DEVICES, Value::Null).await;
    assert_eq!(listed["devices"], json!([]), "nothing was added");
    let music_way = || vec![Part::Music, Part::Requests];
    let wider = through(music_way(), json!(["music", "video"]));
    assert_eq!(status(wider).await, StatusCode::FORBIDDEN);
    let same = through(music_way(), json!(["music", "requests"]));
    assert_eq!(status(same).await, StatusCode::CREATED);
}
