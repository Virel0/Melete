//! The Certificates service's Test: whether the deSEC token sees the domain's zone and may
//! write its `_acme-challenge` TXT record, by adding a value there and taking it out again. No
//! certificate is asked for and the CA isn't contacted. Records others keep there stay.

use super::issue::{Claim, Reach};
use crate::config::Acme;
use crate::desec;

/// How the Test went, and what to say.
pub async fn zone_takes(acme: &Acme, reach: &Reach) -> (bool, String) {
    match tried(acme, reach).await {
        Ok(said) => (true, said),
        Err(err) => (false, format!("{err:#}.")),
    }
}

async fn tried(acme: &Acme, reach: &Reach) -> anyhow::Result<String> {
    let desec = desec::Client::new(&reach.desec_api, acme.desec_token.clone())?;
    let Some(zone) = desec.zone(&acme.domain).await? else {
        anyhow::bail!(
            "deSEC took the token, but it sees no zone holding {}; check the domain, and that the token belongs to the account holding it",
            acme.domain
        );
    };
    let subname = desec::challenge_subname(&acme.domain, &zone)
        .ok_or_else(|| anyhow::anyhow!("{} isn't in the zone {zone}", acme.domain))?;
    let Some(_held) = Claim::take(&reach.desec_api, &zone, &subname) else {
        anyhow::bail!("Melete is getting a certificate right now; test again once it's done");
    };
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).map_err(|e| anyhow::anyhow!("no randomness available: {e}"))?;
    let value = format!(
        "melete-test-{}",
        random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let values = [value];
    desec.add_txt(&zone, &subname, &values).await?;
    desec.remove_txt(&zone, &subname, &values).await?;
    Ok(format!(
        "deSEC took the token: it wrote a test value to {subname}.{zone}'s TXT record and took it out again. No certificate was asked for."
    ))
}
