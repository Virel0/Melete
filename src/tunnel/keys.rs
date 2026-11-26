//! The tunnel's keys: Melete's own, made once from the OS's randomness and kept sealed in the
//! database, and each peer's, made the same way and never kept: only its public half stays,
//! and the private half goes once into the config handed to whoever made the peer. No key is
//! logged or put in an error.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use gotatun::x25519::{PublicKey, StaticSecret};
use sqlx::SqlitePool;

use crate::secrets;

/// What Melete's tunnel key is bound to when sealed.
const SEALED_AS: &str = "tunnel_key.private_key";
const KEY_LEN: usize = 32;

/// 32 bytes from the OS's randomness.
pub fn random() -> anyhow::Result<[u8; KEY_LEN]> {
    let mut bytes = [0u8; KEY_LEN];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness available: {e}"))?;
    Ok(bytes)
}

/// A key as WireGuard's configs write it.
pub fn encode(key: &[u8; KEY_LEN]) -> String {
    STANDARD.encode(key)
}

/// Melete's own key: the one kept, or a new one, kept now. One kept that can't be opened (the
/// secrets key changed) is replaced, and configs made with the old one stop working.
pub async fn own(pool: &SqlitePool) -> anyhow::Result<StaticSecret> {
    let sealing = secrets::sealing(pool).await?;
    let stored: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT private_key FROM tunnel_key WHERE id = 1")
            .fetch_optional(pool)
            .await?;
    if let Some(stored) = stored {
        match secrets::bytes_from_store(sealing, SEALED_AS, &stored, KEY_LEN) {
            Ok(plain) => {
                let bytes: [u8; KEY_LEN] = plain
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("the stored tunnel key isn't 32 bytes"))?;
                return Ok(StaticSecret::from(bytes));
            }
            Err(_) => tracing::warn!(
                "the tunnel's key can't be opened with this secrets key; making a new one, so \
                 tunnel configs made before stop working"
            ),
        }
    }
    let bytes = random()?;
    let stored = secrets::bytes_to_store(sealing, SEALED_AS, &bytes)?;
    sqlx::query(
        "INSERT INTO tunnel_key (id, private_key, created_at) VALUES (1, ?, ?)
         ON CONFLICT (id) DO UPDATE SET private_key = excluded.private_key,
             created_at = excluded.created_at",
    )
    .bind(stored)
    .bind(crate::db::now_ms())
    .execute(pool)
    .await?;
    Ok(StaticSecret::from(bytes))
}

/// A new peer's keys: its private key, and its public key.
pub fn peer() -> anyhow::Result<(StaticSecret, PublicKey)> {
    let secret = StaticSecret::from(random()?);
    let public = PublicKey::from(&secret);
    Ok((secret, public))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_own_key_is_made_once_and_kept() {
        let pool = crate::db::open_in_memory().await;
        let first = PublicKey::from(&own(&pool).await.unwrap());
        let again = PublicKey::from(&own(&pool).await.unwrap());
        assert_eq!(first, again);
        let (a, _) = peer().unwrap();
        let (b, _) = peer().unwrap();
        assert_ne!(a.to_bytes(), b.to_bytes());
        assert_eq!(encode(&[0; KEY_LEN]).len(), 44);
    }

    #[tokio::test]
    async fn a_sealed_database_keeps_the_key_sealed() {
        let pool = crate::db::open_in_memory().await;
        sqlx::query("INSERT INTO secrets_sealed (id, sealed_at) VALUES (1, 0)")
            .execute(&pool)
            .await
            .unwrap();
        let key = own(&pool).await.unwrap();
        let stored: Vec<u8> = sqlx::query_scalar("SELECT private_key FROM tunnel_key")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_ne!(stored.len(), KEY_LEN);
        assert!(!stored.windows(KEY_LEN).any(|w| w == key.to_bytes()));
        assert_eq!(
            PublicKey::from(&own(&pool).await.unwrap()),
            PublicKey::from(&key)
        );
        // Plain, it's refused.
        let plain = sqlx::query("UPDATE tunnel_key SET private_key = ?")
            .bind(vec![1u8; KEY_LEN])
            .execute(&pool)
            .await;
        assert!(plain.is_err());
    }
}
