//! deSEC's DNS API, for the ACME DNS challenge: which of the account's zones a name is in, and
//! the `_acme-challenge` TXT record there, set while the CA checks it and removed after. The
//! token goes in the `Authorization` header to deSEC and nowhere else (never a URL, a log line or
//! an error). Only the domain's names and the challenge's values leave the server, to deSEC.
//! deSEC throttles writes to a zone: a request it turns away for that is tried again after the
//! `Retry-After` it gives, a few times at most. Records others put beside Melete's are left as
//! they are, though deSEC changes a record set whole: another client changing the same one at
//! the same moment can lose its change, or Melete's (which a read after the write catches).
//! Every answer is capped at 64 KiB. Nothing here goes through the proxy.

use std::time::Duration;

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;

use crate::config::Secret;

pub mod dns;
#[cfg(test)]
pub mod testing;

/// deSEC's API.
pub const API: &str = "https://desec.io/api/v1";
/// deSEC's nameservers, which a record is looked for on before the CA is told to check it.
pub const NAMESERVERS: [&str; 2] = ["ns1.desec.io:53", "ns2.desec.org:53"];
/// The lowest TTL deSEC takes.
const TTL: u32 = 3600;
/// How many times a throttled request is tried again, and the longest wait taken between.
const THROTTLED_TRIES: u32 = 5;
const LONGEST_WAIT: Duration = Duration::from_secs(120);

/// deSEC's API with one token.
#[derive(Clone)]
pub struct Client {
    http: crate::proxy::Http,
    base: String,
    token: Secret,
}

/// A record set's TXT values, as deSEC keeps them (each in quotes).
#[derive(Debug, Deserialize)]
struct RrSet {
    records: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Domain {
    name: String,
}

/// What a TXT value looks like in deSEC's records: in quotes.
fn quoted(value: &str) -> String {
    format!("\"{value}\"")
}

impl Client {
    /// deSEC's API at `base` (its own, [`API`], but for tests), with `token`.
    pub fn new(base: &str, token: Secret) -> anyhow::Result<Client> {
        let http = crate::proxy::Http::through(false, |b| {
            b.timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("melete/", env!("CARGO_PKG_VERSION")))
        })?;
        Ok(Client {
            http,
            base: base.trim_end_matches('/').to_owned(),
            token,
        })
    }

    /// Sends what `make` builds with the token, again after a throttled answer's wait; the
    /// answer, or why there was none, in words without the token.
    async fn send(
        &self,
        make: impl Fn(&crate::proxy::Http) -> reqwest::RequestBuilder,
    ) -> anyhow::Result<reqwest::Response> {
        for _ in 0..THROTTLED_TRIES {
            let answer = make(&self.http)
                .header("Authorization", format!("Token {}", self.token.expose()))
                .send()
                .await
                .map_err(|err| {
                    anyhow::anyhow!("deSEC didn't answer: {}", crate::proxy::said(err))
                })?;
            if answer.status() != StatusCode::TOO_MANY_REQUESTS {
                return Ok(answer);
            }
            let wait = answer
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()?.trim().parse::<u64>().ok())
                .map_or(Duration::from_secs(5), Duration::from_secs)
                .min(LONGEST_WAIT);
            tracing::info!(
                seconds = wait.as_secs(),
                "deSEC asks to wait before the next change"
            );
            tokio::time::sleep(wait).await;
        }
        anyhow::bail!("deSEC kept turning requests away as too many; try again later")
    }

    /// The account's zone that `name` is in; None when the token sees none holding it.
    pub async fn zone(&self, name: &str) -> anyhow::Result<Option<String>> {
        let url = format!("{}/domains/", self.base);
        let answer = self
            .send(|http| http.get(&url).query(&[("owns_qname", name)]))
            .await?;
        let found: Vec<Domain> = serde_json::from_slice(&capped(ok(answer).await?).await?)?;
        Ok(found.into_iter().next().map(|d| d.name))
    }

    /// The TXT values at `subname` in `zone`, without their quotes.
    pub async fn txt(&self, zone: &str, subname: &str) -> anyhow::Result<Vec<String>> {
        let url = format!("{}/domains/{zone}/rrsets/{subname}/TXT/", self.base);
        let answer = self.send(|http| http.get(&url)).await?;
        if answer.status() == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        let set: RrSet = serde_json::from_slice(&capped(ok(answer).await?).await?)?;
        Ok(set.records.iter().map(|r| unquoted(r).to_owned()).collect())
    }

    /// Makes the TXT values at `subname` in `zone` exactly `values`; none removes the record.
    async fn put_txt(&self, zone: &str, subname: &str, values: &[String]) -> anyhow::Result<()> {
        let url = format!("{}/domains/{zone}/rrsets/", self.base);
        let body = json!([{
            "subname": subname,
            "type": "TXT",
            "ttl": TTL,
            "records": values.iter().map(|v| quoted(v)).collect::<Vec<_>>(),
        }]);
        let answer = self
            .send(|http| http.request(reqwest::Method::PATCH, &url).json(&body))
            .await?;
        ok(answer).await?;
        Ok(())
    }

    /// Adds `values` to the TXT record at `subname` in `zone`, beside what's there.
    pub async fn add_txt(
        &self,
        zone: &str,
        subname: &str,
        values: &[String],
    ) -> anyhow::Result<()> {
        let mut all = self.txt(zone, subname).await?;
        all.extend(
            values
                .iter()
                .filter(|v| !all.contains(v))
                .cloned()
                .collect::<Vec<_>>(),
        );
        self.put_txt(zone, subname, &all).await?;
        // Read back: a change another client made at the same moment may have replaced ours.
        let now = self.txt(zone, subname).await?;
        anyhow::ensure!(
            values.iter().all(|v| now.contains(v)),
            "the challenge's TXT values didn't stay in deSEC's record: something else changed it at the same time"
        );
        Ok(())
    }

    /// Takes `values` out of the TXT record at `subname` in `zone`, leaving the rest.
    pub async fn remove_txt(
        &self,
        zone: &str,
        subname: &str,
        values: &[String],
    ) -> anyhow::Result<()> {
        let before = self.txt(zone, subname).await?;
        let left: Vec<String> = before
            .iter()
            .filter(|v| !values.contains(v))
            .cloned()
            .collect();
        if left.len() == before.len() {
            return Ok(());
        }
        self.put_txt(zone, subname, &left).await
    }
}

/// How long to wait for deSEC's nameservers to answer a record, and how often to ask.
const SERVED_WITHIN: Duration = Duration::from_secs(300);
const ASK_EVERY: Duration = Duration::from_secs(5);

/// Waits until every nameserver in `servers` (host:port) answers each of `values` at `name`;
/// an error when one still doesn't within `within`.
pub async fn wait_served(
    servers: &[String],
    name: &str,
    values: &[String],
    within: Option<Duration>,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + within.unwrap_or(SERVED_WITHIN);
    for server in servers {
        let address = tokio::net::lookup_host(server.as_str())
            .await?
            .next()
            .ok_or_else(|| anyhow::anyhow!("{server} has no address"))?;
        loop {
            let found = dns::txt(address, name).await.unwrap_or_default();
            if values.iter().all(|v| found.contains(v)) {
                break;
            }
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "deSEC's nameserver {server} still didn't answer the challenge's TXT record at {name}"
            );
            tokio::time::sleep(within.map_or(ASK_EVERY, |w| (w / 10).min(ASK_EVERY))).await;
        }
    }
    Ok(())
}

/// The longest answer taken from deSEC: a zone or a record set is a few hundred bytes.
const MOST: usize = 64 * 1024;

/// `answer`'s body, read in pieces, refused once it's longer than [`MOST`].
async fn capped(mut answer: reqwest::Response) -> anyhow::Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = answer
        .chunk()
        .await
        .map_err(|err| anyhow::anyhow!("deSEC's answer broke off: {}", crate::proxy::said(err)))?
    {
        body.extend_from_slice(&chunk);
        anyhow::ensure!(body.len() <= MOST, "deSEC's answer is far too long");
    }
    Ok(body)
}

/// A TXT value as deSEC writes it, without the one pair of quotes around it.
fn unquoted(record: &str) -> &str {
    record
        .strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .unwrap_or(record)
}

/// `answer`, unless deSEC turned the request down: then why, in plain words.
async fn ok(answer: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    let status = answer.status();
    if status.is_success() {
        return Ok(answer);
    }
    let said: String = capped(answer)
        .await
        .ok()
        .and_then(|body| serde_json::from_slice::<serde_json::Value>(&body).ok())
        .and_then(|v| Some(v.get("detail")?.as_str()?.to_owned()))
        .unwrap_or_default();
    let said = said.chars().take(200).collect::<String>();
    anyhow::bail!(match status {
        StatusCode::UNAUTHORIZED => "deSEC turned the token down".to_owned(),
        StatusCode::FORBIDDEN => format!(
            "deSEC won't let this token make that change ({said}); its policy must allow the _acme-challenge TXT record"
        ),
        _ if said.is_empty() => format!("deSEC answered {status}"),
        _ => format!("deSEC answered {status}: {said}"),
    })
}

/// Where in `zone` the challenge record for `domain` is: `_acme-challenge`, then the labels of
/// `domain` below the zone's.
pub fn challenge_subname(domain: &str, zone: &str) -> Option<String> {
    if domain == zone {
        return Some("_acme-challenge".to_owned());
    }
    let below = domain.strip_suffix(zone)?.strip_suffix('.')?;
    Some(format!("_acme-challenge.{below}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_record_sits_under_the_zone() {
        assert_eq!(
            challenge_subname("example.org", "example.org").as_deref(),
            Some("_acme-challenge")
        );
        assert_eq!(
            challenge_subname("home.example.org", "example.org").as_deref(),
            Some("_acme-challenge.home")
        );
        assert_eq!(challenge_subname("example.org", "other.org"), None);
        assert_eq!(challenge_subname("badexample.org", "example.org"), None);
    }
}
