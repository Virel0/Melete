//! The key file, `secrets.key` in the data folder: 64 hexadecimal characters, readable and
//! writable by Melete's user alone. Made once, never replaced: a key that's there is read, and
//! one that can't be read stops the start rather than being made again.

use std::io::{ErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::{Context, bail};

use super::{KEY_LEN, Key};

/// The key file's name in the data folder.
pub const KEY_FILE: &str = "secrets.key";

/// The key in `path`, or None when there's no file there. Anything else that keeps it from
/// being read is an error: a new key is never made over one that's there.
pub(super) fn read(path: &Path) -> anyhow::Result<Option<Key>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("reading the secrets key {}", path.display()));
        }
    };
    let Some(bytes) = parse(text.trim()) else {
        bail!(
            "{} isn't a secrets key Melete made: it should be {} hexadecimal characters",
            path.display(),
            KEY_LEN * 2
        );
    };
    let mode = std::fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        tracing::warn!(
            file = %path.display(),
            mode = format!("{:o}", mode & 0o777),
            "the secrets key can be read by others than Melete's user; make it 600 (chmod 600)"
        );
    }
    Ok(Some(Key::from_bytes(&bytes)))
}

/// Makes a new random key in `path`, mode 600, failing if a file is there already.
pub(super) fn create(path: &Path) -> anyhow::Result<Key> {
    let mut bytes = [0u8; KEY_LEN];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness available: {e}"))?;
    let text: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("creating the secrets key {}", path.display()))?;
    file.write_all(format!("{text}\n").as_bytes())
        .and_then(|()| file.sync_all())
        .with_context(|| format!("writing the secrets key {}", path.display()))?;
    Ok(Key::from_bytes(&bytes))
}

/// The bytes 64 hexadecimal characters stand for.
fn parse(text: &str) -> Option<[u8; KEY_LEN]> {
    if text.len() != KEY_LEN * 2 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0u8; KEY_LEN];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}
