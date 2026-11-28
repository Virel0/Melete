//! The ACME TLS-ALPN-01 challenge on HTTPS (RFC 8737): while an order waits, a connection whose
//! client offers `acme-tls/1` alone, for the name being proved, gets that name's challenge
//! certificate, and nothing else: no HTTP is served on it, and no other client ever gets one. With
//! nothing waiting for that name, such a handshake fails. It comes after a trusted proxy's PROXY
//! header as any connection does, so the CA reaches it through a passthrough.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use rustls::server::ClientHello;
use rustls::sign::CertifiedKey;

/// The protocol a CA's checking connection offers, alone.
pub const ACME_TLS: &[u8] = b"acme-tls/1";

/// The challenge certificates waiting, by the name they prove.
static WAITING: std::sync::LazyLock<RwLock<HashMap<String, Arc<CertifiedKey>>>> =
    std::sync::LazyLock::new(Default::default);

/// A challenge certificate offered while it's held; taken back when it's dropped.
pub struct Offered(String);

impl Drop for Offered {
    fn drop(&mut self) {
        WAITING
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

/// Offers `certificate` to the CA's connections for `name` until what's returned is dropped.
pub fn offer(name: &str, certificate: CertifiedKey) -> Offered {
    let name = name.to_ascii_lowercase();
    WAITING
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(name.clone(), Arc::new(certificate));
    Offered(name)
}

/// Whether `hello` is a CA's checking connection: `acme-tls/1` and nothing else offered.
pub fn is_challenge(hello: &ClientHello<'_>) -> bool {
    hello
        .alpn()
        .is_some_and(|mut offered| offered.next() == Some(ACME_TLS) && offered.next().is_none())
}

/// The challenge certificate for the name `hello` asks for, while one waits.
pub fn certificate(hello: &ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
    let name = hello.server_name()?.to_ascii_lowercase();
    WAITING
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&name)
        .cloned()
}

/// The challenge certificate for `name`: self-signed, holding the key authorization's SHA-256
/// `digest` in its acmeIdentifier extension, as RFC 8737 asks.
pub fn made(name: &str, digest: &[u8]) -> anyhow::Result<CertifiedKey> {
    let key = rcgen::KeyPair::generate()?;
    let mut params = rcgen::CertificateParams::new(vec![name.to_owned()])?;
    params
        .custom_extensions
        .push(rcgen::CustomExtension::new_acme_identifier(digest));
    let cert = params.self_signed(&key)?;
    let private = rustls::pki_types::PrivateKeyDer::from(
        rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()),
    );
    let signing = super::provider().key_provider.load_private_key(private)?;
    Ok(CertifiedKey::new(vec![cert.der().clone()], signing))
}
