//! Secrets kept in the database, sealed: outside services' keys and passwords set from the web
//! UI, indexers' API keys, two-factor secrets, and search results' links (which can carry a
//! key or a passkey). Each is encrypted with AES-256-GCM (aws-lc-rs, the TLS library already in
//! the build) under a random 256-bit key in the data folder's `secrets.key`, apart from
//! `melete.db`, with a fresh random nonce per value. Each is bound, as associated data, to where
//! it's kept: a setting to its name and a two-factor secret to its user, so neither opens moved
//! to another; indexers' keys, download links and guids only to their column. Sealing keeps the
//! secrets from someone who can read the database or a backup without the key file; it doesn't
//! stop someone who can write the database. Backups copy only the database.
//!
//! A database is switched to sealed storage once, at the server's first start after upgrading
//! (see [`startup`]): its plain values are sealed then, and from that point the database
//! refuses plain ones (triggers) and reading one fails; the backup copies made before are
//! sealed in place after it. A database not switched yet, as a test opens one, keeps them plain
//! as before.
//!
//! Nothing here logs a value or puts one in an error; errors name what can't be read.

use std::sync::OnceLock;

use aws_lc_rs::aead::{AES_256_GCM, Aad, NONCE_LEN, Nonce, RandomizedNonceKey};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

mod copies;
mod key_file;
pub mod recovery;
mod sealing;
pub mod startup;

pub use key_file::KEY_FILE;

/// How a sealed value stored as text starts: the version of this format.
const TEXT_PREFIX: &str = "sealed:v1:";
/// The first byte of a sealed value stored as bytes: the version of this format.
const BYTES_VERSION: u8 = 1;
/// The bytes the key is made of.
pub const KEY_LEN: usize = 32;

/// The key secrets are sealed with. Never in `Debug` output.
pub struct Key(RandomizedNonceKey);

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(<redacted>)")
    }
}

/// Why a stored secret can't be read. Says nothing of the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unreadable {
    /// It's sealed, and the key doesn't open it: another key's, or altered.
    WrongKey,
    /// It's kept plain in a database that keeps secrets sealed.
    Unsealed,
    /// No key is loaded to open it with.
    NoKey,
}

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::WrongKey => "it can't be opened with the key in secrets.key",
            Self::Unsealed => "it's kept unsealed in a database that keeps secrets sealed",
            Self::NoKey => "no secrets key is loaded to open it with",
        })
    }
}

impl std::error::Error for Unreadable {}

impl Key {
    /// The key made of these bytes.
    pub fn from_bytes(bytes: &[u8; KEY_LEN]) -> Self {
        Self(
            RandomizedNonceKey::new(&AES_256_GCM, bytes)
                .expect("AES-256-GCM takes a key of exactly 32 bytes, which this is"),
        )
    }

    /// Seals `plain`, bound to `what`: the nonce, then the ciphertext and its tag.
    fn seal(&self, what: &str, plain: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut sealed = plain.to_vec();
        let nonce = self
            .0
            .seal_in_place_append_tag(Aad::from(what.as_bytes()), &mut sealed)
            .map_err(|_| anyhow::anyhow!("sealing a secret failed (no randomness?)"))?;
        let mut out = nonce.as_ref().to_vec();
        out.append(&mut sealed);
        Ok(out)
    }

    /// Opens what [`Key::seal`] made, bound to `what`.
    fn open(&self, what: &str, sealed: &[u8]) -> Result<Vec<u8>, Unreadable> {
        let (nonce, body) = sealed
            .split_at_checked(NONCE_LEN)
            .ok_or(Unreadable::WrongKey)?;
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| Unreadable::WrongKey)?;
        let mut body = body.to_vec();
        let plain = self
            .0
            .open_in_place(nonce, Aad::from(what.as_bytes()), &mut body)
            .map_err(|_| Unreadable::WrongKey)?;
        Ok(plain.to_vec())
    }

    /// `plain` sealed as text, for a TEXT column.
    pub fn seal_text(&self, what: &str, plain: &str) -> anyhow::Result<String> {
        let sealed = self.seal(what, plain.as_bytes())?;
        Ok(format!("{TEXT_PREFIX}{}", STANDARD.encode(sealed)))
    }

    /// The text [`Key::seal_text`] sealed.
    pub fn open_text(&self, what: &str, stored: &str) -> Result<String, Unreadable> {
        let encoded = stored
            .strip_prefix(TEXT_PREFIX)
            .ok_or(Unreadable::Unsealed)?;
        let sealed = STANDARD.decode(encoded).map_err(|_| Unreadable::WrongKey)?;
        String::from_utf8(self.open(what, &sealed)?).map_err(|_| Unreadable::WrongKey)
    }

    /// `plain` sealed as bytes, for a BLOB column: the format's version, then the sealed value.
    pub fn seal_bytes(&self, what: &str, plain: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut out = vec![BYTES_VERSION];
        out.append(&mut self.seal(what, plain)?);
        Ok(out)
    }

    /// The bytes [`Key::seal_bytes`] sealed.
    pub fn open_bytes(&self, what: &str, stored: &[u8]) -> Result<Vec<u8>, Unreadable> {
        match stored.split_first() {
            Some((&BYTES_VERSION, sealed)) => self.open(what, sealed),
            _ => Err(Unreadable::Unsealed),
        }
    }
}

/// Whether a value kept as text is sealed (rather than plain, from before the switch).
pub fn is_sealed_text(stored: &str) -> bool {
    stored.starts_with(TEXT_PREFIX)
}

/// What a setting's secret is bound to.
pub fn setting(name: &str) -> String {
    format!("settings.{name}")
}

/// What every indexer's API key is bound to.
pub const INDEXER_KEY: &str = "indexers.api_key";

/// What a search result's download link is bound to.
pub const DOWNLOAD_URL: &str = "release_decisions.download_url";

/// What a search result's guid is bound to.
pub const GUID: &str = "release_decisions.guid";

/// What to store for a search result's link or guid (`what`), which can carry an indexer's key
/// or a tracker's passkey: sealed as a secret is, in a database that keeps them sealed.
pub fn link_to_store(
    sealing: bool,
    what: &str,
    link: Option<&str>,
) -> sqlx::Result<Option<String>> {
    link.map(|link| to_store(sealing, what, link)).transpose()
}

/// What a user's two-factor secret is bound to.
pub fn two_factor(user_id: i64) -> String {
    format!("two_factor.{user_id}")
}

/// The key the server loaded at its start.
static LOADED: OnceLock<Key> = OnceLock::new();

/// Makes `key` the one secrets are sealed and opened with, for as long as the server runs.
pub fn install(key: Key) -> anyhow::Result<()> {
    LOADED
        .set(key)
        .map_err(|_| anyhow::anyhow!("a secrets key is loaded already"))
}

/// The key loaded at the server's start. Tests that load none use a fixed one of their own.
pub fn key() -> Result<&'static Key, Unreadable> {
    if let Some(key) = LOADED.get() {
        return Ok(key);
    }
    #[cfg(test)]
    {
        static FOR_TESTS: OnceLock<Key> = OnceLock::new();
        Ok(FOR_TESTS.get_or_init(|| Key::from_bytes(&[7; KEY_LEN])))
    }
    #[cfg(not(test))]
    Err(Unreadable::NoKey)
}

/// Whether this database keeps secrets sealed: it was switched at a server's start.
pub async fn sealing<'c>(db: impl sqlx::SqliteExecutor<'c>) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM secrets_sealed)")
        .fetch_one(db)
        .await
}

/// What to store for a secret kept as text, bound to `what`: sealed in a database that keeps
/// them sealed, else as it is. An empty value (nothing needed) is kept empty.
pub fn to_store(sealing: bool, what: &str, plain: &str) -> sqlx::Result<String> {
    if !sealing || plain.is_empty() {
        return Ok(plain.to_owned());
    }
    key()
        .map_err(|e| sqlx::Error::Encode(e.into()))?
        .seal_text(what, plain)
        .map_err(|e| sqlx::Error::Encode(e.into()))
}

/// A secret kept as text as it was entered, from what [`to_store`] stored. A plain value is
/// refused in a database that keeps secrets sealed. The error, a decoding one, names no value.
pub fn from_store(sealing: bool, what: &str, stored: &str) -> sqlx::Result<String> {
    opened_text(key().ok(), sealing, what, stored).map_err(|why| {
        sqlx::Error::Decode(format!("the stored secret {what} can't be read: {why}").into())
    })
}

/// [`from_store`]'s answer with `key` (None when there's none), and why it can't be read.
pub(crate) fn opened_text(
    key: Option<&Key>,
    sealing: bool,
    what: &str,
    stored: &str,
) -> Result<String, Unreadable> {
    if is_sealed_text(stored) {
        return key.ok_or(Unreadable::NoKey)?.open_text(what, stored);
    }
    if sealing && !stored.is_empty() {
        return Err(Unreadable::Unsealed);
    }
    Ok(stored.to_owned())
}

/// What to store for a secret kept as bytes, bound to `what`, as [`to_store`] does for text.
pub fn bytes_to_store(sealing: bool, what: &str, plain: &[u8]) -> sqlx::Result<Vec<u8>> {
    if !sealing {
        return Ok(plain.to_vec());
    }
    key()
        .map_err(|e| sqlx::Error::Encode(e.into()))?
        .seal_bytes(what, plain)
        .map_err(|e| sqlx::Error::Encode(e.into()))
}

/// A secret kept as bytes, from what [`bytes_to_store`] stored; `plain_len` is how long the
/// secret is, which tells a plain one from a sealed one.
pub fn bytes_from_store(
    sealing: bool,
    what: &str,
    stored: &[u8],
    plain_len: usize,
) -> sqlx::Result<Vec<u8>> {
    opened_bytes(key().ok(), sealing, what, stored, plain_len).map_err(|why| {
        sqlx::Error::Decode(format!("the stored secret {what} can't be read: {why}").into())
    })
}

/// [`bytes_from_store`]'s answer with `key` (None when there's none), and why it can't be read.
pub(crate) fn opened_bytes(
    key: Option<&Key>,
    sealing: bool,
    what: &str,
    stored: &[u8],
    plain_len: usize,
) -> Result<Vec<u8>, Unreadable> {
    if stored.len() != plain_len {
        return key.ok_or(Unreadable::NoKey)?.open_bytes(what, stored);
    }
    if sealing {
        return Err(Unreadable::Unsealed);
    }
    Ok(stored.to_vec())
}

#[cfg(test)]
mod tests;
