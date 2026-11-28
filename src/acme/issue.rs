//! Asking the CA for a certificate: an order for the names, each name's DNS challenge answered
//! with a TXT record through deSEC (removed again after, whether the CA took it or not), then a
//! new key, a signing request for it, and the chain the CA gives back. Only one runs at a time,
//! and the Test never touches the record while one does.

use std::time::Duration;

use anyhow::Context;
use instant_acme::{
    Account, AuthorizationStatus, CertificateIdentifier, ChallengeType, Identifier, NewOrder,
    Order, OrderStatus, RetryPolicy,
};

use sqlx::SqlitePool;

use super::stage::Stage;
use crate::config::{Acme, Challenge};
use crate::desec;

/// Where the DNS challenge is answered: deSEC's API, and the nameservers a record is looked
/// for on before the CA is told; deSEC's own but for tests, which also wait less.
#[derive(Debug, Clone)]
pub struct Reach {
    pub desec_api: String,
    pub nameservers: Vec<String>,
    pub served_within: Option<Duration>,
}

impl Reach {
    /// deSEC itself.
    pub fn desec() -> Reach {
        Reach {
            desec_api: desec::API.to_owned(),
            nameservers: desec::NAMESERVERS.iter().map(|s| (*s).to_owned()).collect(),
            served_within: None,
        }
    }
}

/// The challenge records in use, by deSEC's address, zone and name: set by an order, or by the
/// Test.
static IN_USE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// A challenge record held while it's set; let go when dropped.
pub struct Claim(String);

impl Claim {
    /// The record at `name` in `zone` on deSEC at `api`, unless it's in use.
    pub fn take(api: &str, zone: &str, name: &str) -> Option<Claim> {
        let key = format!("{api} {zone} {name}");
        let mut in_use = IN_USE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if in_use.contains(&key) {
            return None;
        }
        in_use.push(key.clone());
        Some(Claim(key))
    }

    /// The same, waiting while it's in use.
    async fn wait(api: &str, zone: &str, name: &str) -> Claim {
        loop {
            if let Some(claim) = Self::take(api, zone, name) {
                return claim;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        IN_USE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|k| *k != self.0);
    }
}

/// A certificate the CA issued: its chain, leaf first, and its key, both PEM.
pub struct Issued {
    pub chain: String,
    pub key: String,
}

/// How long the CA may take to check the challenges, and to issue once asked.
const CHECKED_WITHIN: Duration = Duration::from_secs(180);
const ISSUED_WITHIN: Duration = Duration::from_secs(120);

/// A new order for `acme`'s names, replacing `replaces` when given and the CA takes that.
async fn order(
    account: &Account,
    acme: &Acme,
    replaces: Option<CertificateIdentifier<'static>>,
) -> anyhow::Result<Order> {
    let identifiers: Vec<Identifier> = acme.names().into_iter().map(Identifier::Dns).collect();
    if let Some(replaces) = replaces {
        match account
            .new_order(&NewOrder::new(&identifiers).replaces(replaces))
            .await
        {
            Ok(order) => return Ok(order),
            Err(err) => tracing::info!(
                error = %err,
                "the CA didn't take the order as a renewal; asking for a new certificate"
            ),
        }
    }
    Ok(account.new_order(&NewOrder::new(&identifiers)).await?)
}

/// A name the order still needs proved, and its key authorization's digest.
struct Needed {
    name: String,
    digest: Vec<u8>,
}

impl Needed {
    /// The digest as the DNS challenge's TXT value.
    fn dns_value(&self) -> String {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&self.digest)
    }
}

/// The challenge type the CA is answered with.
fn kind(acme: &Acme) -> ChallengeType {
    match acme.challenge {
        Challenge::Dns => ChallengeType::Dns01,
        Challenge::TlsAlpn => ChallengeType::TlsAlpn01,
    }
}

/// What the order still needs, one for each name not yet authorized, by `kind` of challenge.
async fn needed(order: &mut Order, kind: ChallengeType) -> anyhow::Result<Vec<Needed>> {
    let mut found = Vec::new();
    let mut authorizations = order.authorizations();
    while let Some(authorization) = authorizations.next().await {
        let mut authorization = authorization?;
        match authorization.status {
            AuthorizationStatus::Valid => continue,
            AuthorizationStatus::Pending => {}
            other => anyhow::bail!("the CA says a name's authorization is {other:?}"),
        }
        let challenge = authorization
            .challenge(kind.clone())
            .with_context(|| format!("the CA offers no {kind:?} challenge"))?;
        found.push(Needed {
            name: challenge.identifier().to_string(),
            digest: challenge.key_authorization().digest().as_ref().to_vec(),
        });
    }
    Ok(found)
}

/// Tells the CA each pending challenge (of `kind`) is ready, then waits for its verdict.
async fn answer(order: &mut Order, kind: ChallengeType) -> anyhow::Result<()> {
    let mut authorizations = order.authorizations();
    while let Some(authorization) = authorizations.next().await {
        let mut authorization = authorization?;
        if authorization.status != AuthorizationStatus::Pending {
            continue;
        }
        let mut challenge = authorization
            .challenge(kind.clone())
            .with_context(|| format!("the CA offers no {kind:?} challenge"))?;
        challenge.set_ready().await?;
    }
    let polling = RetryPolicy::new()
        .initial_delay(Duration::from_secs(2))
        .backoff(1.5)
        .timeout(CHECKED_WITHIN);
    match order.poll_ready(&polling).await? {
        OrderStatus::Ready => Ok(()),
        _ => Err(anyhow::anyhow!(
            "the CA didn't take the challenge: {}",
            why_invalid(order).await
        )),
    }
}

/// Why the CA turned a challenge down, as it says.
async fn why_invalid(order: &mut Order) -> String {
    let mut authorizations = order.authorizations();
    while let Some(Ok(mut authorization)) = authorizations.next().await {
        if let Ok(state) = authorization.refresh().await
            && let Some(problem) = state.challenges.iter().find_map(|c| c.error.as_ref())
        {
            return problem.to_string();
        }
    }
    "it gave no reason".to_owned()
}

/// Sets the challenge's TXT record (noting its values first), has the CA check it, and removes
/// the record again.
async fn challenge(
    pool: &SqlitePool,
    order: &mut Order,
    acme: &Acme,
    reach: &Reach,
) -> anyhow::Result<()> {
    let needed = needed(order, kind(acme)).await.context(Stage::Challenge)?;
    if needed.is_empty() {
        return answer(order, kind(acme)).await.context(Stage::Challenge);
    }
    if acme.challenge == Challenge::TlsAlpn {
        return tls_alpn(order, &needed).await;
    }
    let values: Vec<String> = needed.iter().map(Needed::dns_value).collect();
    let token = acme
        .desec_token
        .clone()
        .context("the DNS challenge needs a deSEC token")?;
    let desec = desec::Client::new(&reach.desec_api, token)?;
    let zone = desec.zone(&acme.domain).await.context(Stage::Dns)?;
    let zone = zone
        .with_context(|| {
            format!(
                "the deSEC token sees no zone holding {}; check the domain and the token",
                acme.domain
            )
        })
        .context(Stage::Dns)?;
    let subname = desec::challenge_subname(&acme.domain, &zone)
        .context("the domain isn't in the zone deSEC named")
        .context(Stage::Dns)?;
    let name = format!("_acme-challenge.{}", acme.domain);
    let _held = Claim::wait(&reach.desec_api, &zone, &subname).await;
    super::left::note(pool, &reach.desec_api, &zone, &subname, &values).await?;
    let answered = set_and_answer(
        order,
        acme,
        reach,
        &desec,
        (&zone, &subname, &name),
        &values,
    )
    .await;
    match desec.remove_txt(&zone, &subname, &values).await {
        Ok(()) => super::left::forget(pool).await?,
        Err(err) => tracing::warn!(
            error = format!("{err:#}"),
            "couldn't remove the challenge's TXT record; it's taken out at the next look"
        ),
    }
    answered
}

/// Sets `values` at `place` (zone, subname, full name), waits until deSEC's nameservers answer
/// them and a while more, and has the CA check them.
async fn set_and_answer(
    order: &mut Order,
    acme: &Acme,
    reach: &Reach,
    desec: &desec::Client,
    (zone, subname, name): (&str, &str, &str),
    values: &[String],
) -> anyhow::Result<()> {
    desec
        .add_txt(zone, subname, values)
        .await
        .context(Stage::Dns)?;
    desec::wait_served(&reach.nameservers, name, values, reach.served_within)
        .await
        .context(Stage::Served)?;
    // Answered here isn't answered everywhere the CA checks from yet.
    tokio::time::sleep(acme.settle).await;
    answer(order, ChallengeType::Dns01)
        .await
        .context(Stage::Challenge)
}

/// Offers each name's challenge certificate on HTTPS while the CA checks it (TLS-ALPN-01), and
/// takes them all back after, whatever it says.
async fn tls_alpn(order: &mut Order, needed: &[Needed]) -> anyhow::Result<()> {
    let mut offered = Vec::new();
    for need in needed {
        let made = crate::tls::challenge::made(&need.name, &need.digest)?;
        offered.push(crate::tls::challenge::offer(&need.name, made));
    }
    let answered = answer(order, ChallengeType::TlsAlpn01)
        .await
        .context(Stage::Challenge);
    drop(offered);
    answered
}

/// Asks the CA on `account` for a certificate for `acme`'s names, by the DNS challenge.
pub async fn issue(
    pool: &SqlitePool,
    account: &Account,
    acme: &Acme,
    reach: &Reach,
    replaces: Option<CertificateIdentifier<'static>>,
) -> anyhow::Result<Issued> {
    let mut order = order(account, acme, replaces).await.context(Stage::Order)?;
    challenge(pool, &mut order, acme, reach).await?;
    finish(&mut order, acme).await.context(Stage::Issue)
}

/// A new key, a signing request for it, and the chain the CA gives back.
async fn finish(order: &mut Order, acme: &Acme) -> anyhow::Result<Issued> {
    let key = rcgen::KeyPair::generate()?;
    let mut params = rcgen::CertificateParams::new(acme.names())?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    let request = params.serialize_request(&key)?;
    order.finalize_csr(request.der()).await?;
    let polling = RetryPolicy::new()
        .initial_delay(Duration::from_secs(1))
        .backoff(1.5)
        .timeout(ISSUED_WITHIN);
    let chain = order.poll_certificate(&polling).await?;
    Ok(Issued {
        chain,
        key: key.serialize_pem(),
    })
}
