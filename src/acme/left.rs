//! The challenge values Melete sets in a TXT record, noted before they're set and forgotten once
//! they're removed. Values a crash left there are then taken out at the next look, and only
//! those: Melete can't tell any other value at that name is its own, so it leaves the rest.

use sqlx::SqlitePool;

use super::issue::Reach;
use crate::config::Acme;

/// Notes `values` as about to be set at `subname` in `zone`, on deSEC at `api`.
pub async fn note(
    pool: &SqlitePool,
    api: &str,
    zone: &str,
    subname: &str,
    values: &[String],
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO acme_challenge_left (id, api, zone, subname, ours, set_at)
         VALUES (1, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET api = excluded.api, zone = excluded.zone,
             subname = excluded.subname, ours = excluded.ours, set_at = excluded.set_at",
    )
    .bind(api)
    .bind(zone)
    .bind(subname)
    .bind(serde_json::to_string(values)?)
    .bind(crate::db::unix_now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Forgets the values noted: they're removed.
pub async fn forget(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM acme_challenge_left")
        .execute(pool)
        .await?;
    Ok(())
}

/// Takes out values left set, when they were set on the deSEC `reach` names; forgotten once
/// they're gone. Kept to try again when deSEC can't be reached.
pub async fn take_out(pool: &SqlitePool, acme: &Acme, reach: &Reach) -> anyhow::Result<()> {
    let left: Option<(String, String, String, String)> =
        sqlx::query_as("SELECT api, zone, subname, ours FROM acme_challenge_left")
            .fetch_optional(pool)
            .await?;
    let Some((api, zone, subname, ours)) = left else {
        return Ok(());
    };
    if api != reach.desec_api {
        return Ok(());
    }
    let ours: Vec<String> = serde_json::from_str(&ours)?;
    let desec = crate::desec::Client::new(&api, acme.desec_token.clone())?;
    let Some(_held) = super::issue::Claim::take(&api, &zone, &subname) else {
        return Ok(());
    };
    desec.remove_txt(&zone, &subname, &ours).await?;
    forget(pool).await?;
    tracing::info!("took out challenge values an earlier attempt left in the TXT record");
    Ok(())
}
