//! The certificate Melete got, kept where HTTPS reads it: the chain and its key as PEM files in
//! the data folder's `acme` folder (only Melete's user may open the folder or read the key),
//! each written whole beside it and then moved into place, so HTTPS never reads half of one.
//! HTTPS takes up a new pair at once. Backups copy the database only, never these.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use instant_acme::CertificateIdentifier;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

use crate::config::{CERT_FILE, KEY_FILE};

/// The certificate held, as far as renewing it goes.
#[derive(Debug, Clone)]
pub struct Held {
    /// The names it's for, as it lists them.
    pub names: Vec<String>,
    /// When it became valid, and when it runs out (Unix seconds).
    pub not_before: i64,
    pub not_after: i64,
    /// How the CA's renewal information names it; None when it can't be told.
    pub id: Option<CertificateIdentifier<'static>>,
}

impl Held {
    /// Whether it's for exactly `names`.
    pub fn is_for(&self, names: &[String]) -> bool {
        let mut held = self.names.clone();
        let mut wanted = names.to_vec();
        held.sort();
        wanted.sort();
        held == wanted
    }
}

/// Writes `contents` to `path` with `mode`, by way of a file beside it.
fn put(path: &Path, contents: &str, mode: u32) -> std::io::Result<()> {
    let beside = path.with_extension("new");
    let _ = std::fs::remove_file(&beside);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&beside)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&beside, path)
}

/// Keeps `chain` and its `key` in `folder` and has HTTPS take them up.
pub fn write(folder: &Path, chain: &str, key: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(folder)?;
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))?;
    put(&folder.join(KEY_FILE), key, 0o600)?;
    put(&folder.join(CERT_FILE), chain, 0o644)?;
    crate::tls::written();
    Ok(())
}

/// The leaf of the chain kept in `folder`, read; None when there's none or it doesn't read.
pub fn read(folder: &Path) -> Option<Held> {
    let path: PathBuf = folder.join(CERT_FILE);
    let leaf = CertificateDer::pem_file_iter(&path).ok()?.next()?.ok()?;
    held(&leaf)
}

/// What renewing needs to know of a certificate.
pub fn held(leaf: &CertificateDer<'_>) -> Option<Held> {
    let (_, parsed) = x509_parser::parse_x509_certificate(leaf.as_ref()).ok()?;
    let names = parsed
        .subject_alternative_name()
        .ok()
        .flatten()
        .map(|san| {
            san.value
                .general_names
                .iter()
                .filter_map(|n| match n {
                    x509_parser::extensions::GeneralName::DNSName(name) => Some((*name).to_owned()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Held {
        names,
        not_before: parsed.validity().not_before.timestamp(),
        not_after: parsed.validity().not_after.timestamp(),
        id: CertificateIdentifier::try_from(leaf).ok(),
    })
}
