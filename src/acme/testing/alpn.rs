//! The stand-in CA's TLS-ALPN-01 check: it connects to Melete's HTTPS as a CA does (after a PROXY
//! header, when it stands for a passthrough), offering `acme-tls/1` alone and the name being
//! proved, and reads the acmeIdentifier extension of the certificate it's shown.

use std::net::SocketAddr;
use std::sync::Arc;

use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio::io::AsyncWriteExt;

/// The acmeIdentifier extension's OID (RFC 8737).
const ACME_IDENTIFIER: &str = "1.3.6.1.5.5.7.1.31";

/// Takes any certificate: a TLS-ALPN-01 challenge certificate is self-signed.
#[derive(Debug)]
struct Any(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for Any {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.3 only".into()))
    }

    /// Checked with the certificate's key alone: webpki won't parse a certificate with the
    /// acmeIdentifier extension, which it doesn't know and which is critical.
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let (_, parsed) = x509_parser::parse_x509_certificate(cert.as_ref())
            .map_err(|_| rustls::Error::General("not a certificate".into()))?;
        let spki = rustls::pki_types::SubjectPublicKeyInfoDer::from(
            parsed.tbs_certificate.subject_pki.raw,
        );
        rustls::crypto::verify_tls13_signature_with_raw_key(
            message,
            &spki,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// A verifier that takes any certificate, for a client checking a challenge certificate.
pub fn any() -> Arc<dyn ServerCertVerifier> {
    Arc::new(Any(Arc::new(rustls::crypto::aws_lc_rs::default_provider())))
}

/// The certificate HTTPS at `at` shows a client offering `alpn` for `name` (after `before`),
/// and the protocol agreed; why there's none when the handshake fails.
pub async fn shown(
    at: SocketAddr,
    before: &[u8],
    name: &str,
    alpn: &[&[u8]],
) -> Result<(CertificateDer<'static>, Option<Vec<u8>>), String> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(any())
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    let mut tcp = tokio::net::TcpStream::connect(at)
        .await
        .map_err(|e| e.to_string())?;
    tcp.write_all(before).await.map_err(|e| e.to_string())?;
    let server = ServerName::try_from(name.to_owned()).unwrap();
    let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(server, tcp)
        .await
        .map_err(|e| e.to_string())?;
    let (_, connection) = tls.get_ref();
    let leaf = connection.peer_certificates().unwrap()[0]
        .clone()
        .into_owned();
    Ok((leaf, connection.alpn_protocol().map(<[u8]>::to_vec)))
}

/// The digest a certificate's acmeIdentifier extension holds, if it has one.
pub fn acme_identifier(leaf: &CertificateDer<'_>) -> Option<Vec<u8>> {
    let (_, parsed) = x509_parser::parse_x509_certificate(leaf.as_ref()).ok()?;
    let extension = parsed
        .extensions()
        .iter()
        .find(|e| e.oid.to_id_string() == ACME_IDENTIFIER)?;
    // An OCTET STRING of the 32-byte digest.
    let value = extension.value;
    (value.len() == 34 && value[0] == 0x04 && value[1] == 0x20).then(|| value[2..].to_vec())
}
