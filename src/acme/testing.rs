//! A stand-in ACME CA on localhost: a directory, nonces, accounts (each known by its key's
//! thumbprint), orders with one authorization per name and a DNS challenge each, checked by
//! asking a nameserver for the TXT record as a CA would, then a certificate for the signing
//! request, signed by its own CA certificate, and renewal information with a window set by the
//! test. Signatures aren't checked: it's what Melete sends that's under test.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rustls::pki_types::CertificateDer;
use serde_json::{Value, json};

/// One name's authorization.
#[derive(Debug, Clone)]
pub struct Authz {
    /// The name, without `*.`, and whether it's the wildcard.
    pub value: String,
    pub wildcard: bool,
    pub token: String,
    pub status: &'static str,
}

/// One order.
#[derive(Debug, Clone)]
pub struct Order {
    pub account: usize,
    pub names: Vec<String>,
    pub authz: Vec<Authz>,
    pub finalized: bool,
    pub chain: Option<String>,
    /// The certificate it said it replaces, as ARI names it.
    pub replaces: Option<String>,
}

/// What the stand-in holds, and was asked.
pub struct Fake {
    pub base: String,
    /// The nameserver challenges are checked on.
    pub dns: SocketAddr,
    nonce: u64,
    /// Each account's key thumbprint and contacts.
    pub accounts: Vec<(String, Vec<String>)>,
    pub orders: Vec<Order>,
    /// How long certificates it issues run, in seconds.
    pub lifetime: i64,
    /// The renewal window it suggests, Unix seconds; None offers no renewal information.
    pub window: Option<(i64, i64)>,
    /// When each challenge was checked.
    pub checked_at: Vec<std::time::Instant>,
    /// The certificates its renewal information was asked about.
    pub ari_asked: Vec<String>,
    /// Orders refused from now on, with this problem's detail.
    pub refuse_orders: Option<String>,
    ca_params: rcgen::CertificateParams,
    ca_key: rcgen::KeyPair,
    /// Its CA certificate, which tests trust, and as PEM.
    pub root: CertificateDer<'static>,
    root_pem: String,
}

pub type Shared = Arc<Mutex<Fake>>;

/// Starts the stand-in, checking challenges on the nameserver at `dns`; its directory's URL and
/// its state.
pub async fn start(dns: SocketAddr) -> (String, Shared) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Stand-in CA");
    let made = ca_params.self_signed(&ca_key).unwrap();
    let (root, root_pem) = (made.der().clone(), made.pem());
    let fake = Shared::new(Mutex::new(Fake {
        base: base.clone(),
        dns,
        nonce: 0,
        accounts: Vec::new(),
        orders: Vec::new(),
        lifetime: 90 * 86_400,
        window: None,
        checked_at: Vec::new(),
        ari_asked: Vec::new(),
        refuse_orders: None,
        ca_params,
        ca_key,
        root,
        root_pem,
    }));
    let router = axum::Router::new()
        .fallback(answer)
        .with_state(fake.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (format!("{base}/directory"), fake)
}

/// RFC 3339, from Unix seconds.
pub fn rfc3339(unix: i64) -> String {
    let day = crate::follow::date(unix);
    let s = unix.rem_euclid(86_400);
    format!("{day}T{:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60)
}

fn b64(value: &str) -> Value {
    URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

/// RFC 7638's thumbprint of an EC key.
fn thumbprint(jwk: &Value) -> String {
    let canonical = format!(
        r#"{{"crv":"{}","kty":"EC","x":"{}","y":"{}"}}"#,
        jwk["crv"].as_str().unwrap(),
        jwk["x"].as_str().unwrap(),
        jwk["y"].as_str().unwrap()
    );
    URL_SAFE_NO_PAD.encode(aws_lc_rs::digest::digest(
        &aws_lc_rs::digest::SHA256,
        canonical.as_bytes(),
    ))
}

impl Fake {
    fn nonce(&mut self) -> String {
        self.nonce += 1;
        format!("nonce-{}", self.nonce)
    }

    fn reply(&mut self, status: StatusCode, location: Option<String>, body: Value) -> Response {
        let mut headers = vec![("replay-nonce".to_owned(), self.nonce())];
        if let Some(location) = location {
            headers.push(("location".to_owned(), location));
        }
        let mut response = (status, axum::Json(body)).into_response();
        for (name, value) in headers {
            response.headers_mut().insert(
                axum::http::HeaderName::try_from(name).unwrap(),
                value.parse().unwrap(),
            );
        }
        response
    }

    fn order_json(&self, n: usize) -> Value {
        let order = &self.orders[n];
        let status = if order.chain.is_some() {
            "valid"
        } else if order.authz.iter().any(|a| a.status == "invalid") {
            "invalid"
        } else if order.authz.iter().all(|a| a.status == "valid") {
            "ready"
        } else {
            "pending"
        };
        json!({
            "status": status,
            "identifiers": order.names.iter().map(|n| json!({"type": "dns", "value": n})).collect::<Vec<_>>(),
            "authorizations": (0..order.authz.len()).map(|i| format!("{}/authz/{n}/{i}", self.base)).collect::<Vec<_>>(),
            "finalize": format!("{}/finalize/{n}", self.base),
            "certificate": order.chain.as_ref().map(|_| format!("{}/cert/{n}", self.base)),
            "replaces": order.replaces,
        })
    }

    fn authz_json(&self, n: usize, i: usize) -> Value {
        let a = &self.orders[n].authz[i];
        json!({
            "identifier": {"type": "dns", "value": a.value},
            "wildcard": a.wildcard,
            "status": a.status,
            "challenges": [{
                "type": "dns-01",
                "url": format!("{}/chall/{n}/{i}", self.base),
                "token": a.token,
                "status": match a.status { "valid" => "valid", "invalid" => "invalid", _ => "pending" },
                "error": (a.status == "invalid").then(|| json!({
                    "type": "urn:ietf:params:acme:error:unauthorized",
                    "detail": format!("No TXT record found at _acme-challenge.{}", a.value),
                })),
            }],
        })
    }

    /// A certificate for the signing request `csr`, and the stand-in's CA certificate after it.
    fn sign(&self, csr: &str) -> String {
        let der = URL_SAFE_NO_PAD.decode(csr).unwrap();
        let mut request = rcgen::CertificateSigningRequestParams::from_der(&der.into()).unwrap();
        let now = crate::db::unix_now();
        let epoch = rcgen::date_time_ymd(1970, 1, 1);
        request.params.not_before = epoch + Duration::from_secs((now - 60).unsigned_abs());
        request.params.not_after =
            epoch + Duration::from_secs((now + self.lifetime).unsigned_abs());
        request.params.use_authority_key_identifier_extension = true;
        let issuer = rcgen::Issuer::from_params(&self.ca_params, &self.ca_key);
        let leaf = request.signed_by(&issuer).unwrap();
        format!("{}{}", leaf.pem(), self.root_pem)
    }
}

/// The account a request's `kid` names.
fn account_of(fake: &Fake, protected: &Value) -> Option<usize> {
    let kid = protected["kid"].as_str()?;
    kid.strip_prefix(&format!("{}/acct/", fake.base))?
        .parse()
        .ok()
}

async fn answer(
    axum::extract::State(fake): axum::extract::State<Shared>,
    method: Method,
    uri: Uri,
    body: String,
) -> Response {
    let parts: Vec<String> = uri
        .path()
        .trim_matches('/')
        .split('/')
        .map(str::to_owned)
        .collect();
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    if method == Method::GET || method == Method::HEAD {
        return get(&fake, &parts);
    }
    let jws: Value = serde_json::from_str(&body).unwrap();
    let protected = b64(jws["protected"].as_str().unwrap());
    let payload = b64(jws["payload"].as_str().unwrap_or(""));
    if let ["chall", n, i] = parts[..] {
        return challenge(&fake, n.parse().unwrap(), i.parse().unwrap()).await;
    }
    let mut fake = fake.lock().unwrap();
    match parts[..] {
        ["account"] => {
            let thumb = thumbprint(&protected["jwk"]);
            let contact = contacts(&payload);
            let n = match fake.accounts.iter().position(|(t, _)| *t == thumb) {
                Some(n) => n,
                None => {
                    fake.accounts.push((thumb, contact.clone()));
                    fake.accounts.len() - 1
                }
            };
            let at = format!("{}/acct/{n}", fake.base);
            fake.reply(
                StatusCode::CREATED,
                Some(at),
                json!({"status": "valid", "contact": contact}),
            )
        }
        ["acct", n] => {
            let n: usize = n.parse().unwrap();
            if payload.get("contact").is_some() {
                fake.accounts[n].1 = contacts(&payload);
            }
            let contact = fake.accounts[n].1.clone();
            fake.reply(
                StatusCode::OK,
                None,
                json!({"status": "valid", "contact": contact}),
            )
        }
        ["order"] => new_order(&mut fake, &protected, &payload),
        ["order", n] => {
            let body = fake.order_json(n.parse().unwrap());
            fake.reply(StatusCode::OK, None, body)
        }
        ["authz", n, i] => {
            let body = fake.authz_json(n.parse().unwrap(), i.parse().unwrap());
            fake.reply(StatusCode::OK, None, body)
        }
        ["finalize", n] => {
            let n: usize = n.parse().unwrap();
            let chain = fake.sign(payload["csr"].as_str().unwrap());
            fake.orders[n].finalized = true;
            fake.orders[n].chain = Some(chain);
            let (body, at) = (fake.order_json(n), format!("{}/order/{n}", fake.base));
            fake.reply(StatusCode::OK, Some(at), body)
        }
        ["cert", n] => {
            let chain = fake.orders[n.parse::<usize>().unwrap()]
                .chain
                .clone()
                .unwrap();
            let nonce = fake.nonce();
            (
                StatusCode::OK,
                [
                    (
                        "content-type",
                        "application/pem-certificate-chain".to_owned(),
                    ),
                    ("replay-nonce", nonce),
                ],
                chain,
            )
                .into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn contacts(payload: &Value) -> Vec<String> {
    payload["contact"]
        .as_array()
        .map(|c| {
            c.iter()
                .filter_map(|v| Some(v.as_str()?.to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

fn get(fake: &Shared, parts: &[&str]) -> Response {
    let mut fake = fake.lock().unwrap();
    match parts {
        ["directory"] => {
            let base = fake.base.clone();
            let mut directory = json!({
                "newNonce": format!("{base}/nonce"),
                "newAccount": format!("{base}/account"),
                "newOrder": format!("{base}/order"),
                "revokeCert": format!("{base}/revoke"),
            });
            if fake.window.is_some() {
                directory["renewalInfo"] = json!(format!("{base}/renewal-info"));
            }
            axum::Json(directory).into_response()
        }
        ["nonce"] => {
            let nonce = fake.nonce();
            (StatusCode::OK, [("replay-nonce", nonce)]).into_response()
        }
        ["renewal-info", id] => {
            fake.ari_asked.push((*id).to_owned());
            let Some((start, end)) = fake.window else {
                return StatusCode::NOT_FOUND.into_response();
            };
            let body = json!({"suggestedWindow": {"start": rfc3339(start), "end": rfc3339(end)}});
            (StatusCode::OK, [("retry-after", "21600")], axum::Json(body)).into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn new_order(fake: &mut Fake, protected: &Value, payload: &Value) -> Response {
    if let Some(detail) = fake.refuse_orders.clone() {
        return fake.reply(
            StatusCode::FORBIDDEN,
            None,
            json!({"type": "urn:ietf:params:acme:error:rejectedIdentifier", "detail": detail, "status": 403}),
        );
    }
    let account = account_of(fake, protected).unwrap();
    let names: Vec<String> = payload["identifiers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["value"].as_str().unwrap().to_owned())
        .collect();
    let n = fake.orders.len();
    let authz = names
        .iter()
        .enumerate()
        .map(|(i, name)| Authz {
            value: name.trim_start_matches("*.").to_owned(),
            wildcard: name.starts_with("*."),
            token: format!("token-{n}-{i}"),
            status: "pending",
        })
        .collect();
    fake.orders.push(Order {
        account,
        names,
        authz,
        finalized: false,
        chain: None,
        replaces: payload["replaces"].as_str().map(str::to_owned),
    });
    let (body, at) = (fake.order_json(n), format!("{}/order/{n}", fake.base));
    fake.reply(StatusCode::CREATED, Some(at), body)
}

/// Checks a DNS challenge as a CA does: the TXT record at `_acme-challenge.<name>` holds the
/// key authorization's digest.
async fn challenge(fake: &Shared, n: usize, i: usize) -> Response {
    let (dns, name, expected) = {
        let fake = fake.lock().unwrap();
        let order = &fake.orders[n];
        let a = &order.authz[i];
        let thumb = &fake.accounts[order.account].0;
        let digest = aws_lc_rs::digest::digest(
            &aws_lc_rs::digest::SHA256,
            format!("{}.{thumb}", a.token).as_bytes(),
        );
        (
            fake.dns,
            format!("_acme-challenge.{}", a.value),
            URL_SAFE_NO_PAD.encode(digest),
        )
    };
    let found = crate::desec::dns::txt(dns, &name).await.unwrap_or_default();
    let mut fake = fake.lock().unwrap();
    fake.checked_at.push(std::time::Instant::now());
    fake.orders[n].authz[i].status = if found.contains(&expected) {
        "valid"
    } else {
        "invalid"
    };
    let body = fake.authz_json(n, i)["challenges"][0].clone();
    fake.reply(StatusCode::OK, None, body)
}
