//! HTTPS's certificate got from an ACME CA (Let's Encrypt) by Melete itself, rather than from
//! files: for a domain, by the DNS challenge through deSEC, as a wildcard (`*.example.org`) and
//! the bare name. Off unless the domain is set; the CA's staging server until switched to the
//! real one.

use super::*;

/// The ACME directories Melete knows: Let's Encrypt's staging server, then its real one.
pub const STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";
pub const PRODUCTION: &str = "https://acme-v02.api.letsencrypt.org/directory";

/// The files the certificate and its key are kept in, in the folder [`folder`] names: where
/// HTTPS reads them.
pub const CERT_FILE: &str = "certificate.pem";
pub const KEY_FILE: &str = "key.pem";

/// The folder in the data folder that holds the certificate Melete got.
pub fn folder(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("acme")
}

/// Getting HTTPS's certificate from an ACME CA, as set.
#[derive(Debug, Clone)]
pub struct Acme {
    /// `MELETE_ACME_DOMAIN`: the domain, e.g. example.org; the certificate holds
    /// `*.example.org` and `example.org`.
    pub domain: String,
    /// `MELETE_ACME_DESEC_TOKEN`: a deSEC token that may write the domain's
    /// `_acme-challenge` TXT record.
    pub desec_token: Secret,
    /// `MELETE_ACME_EMAIL`: an address on the ACME account, for the CA's notices; none unless
    /// set.
    pub email: Option<String>,
    /// `MELETE_ACME_CA=production`: the CA's real server; its staging server otherwise, whose
    /// certificates no browser trusts but whose rate limits a first try can't run into.
    pub staging: bool,
    /// The CA's directory: [`STAGING`] or [`PRODUCTION`] as `staging` says (a stand-in's in
    /// tests).
    pub directory: String,
    /// `MELETE_ACME_DNS_WAIT`: how long to wait once deSEC's nameservers answer the challenge's
    /// record before the CA is told to look, as its checks from elsewhere may see it later; 120
    /// seconds unless set.
    pub settle: std::time::Duration,
}

impl Acme {
    /// The names asked for: the wildcard, then the bare name.
    pub fn names(&self) -> Vec<String> {
        vec![format!("*.{}", self.domain), self.domain.clone()]
    }
}

/// Seconds to wait by default once deSEC's nameservers answer the challenge's record.
const DEFAULT_SETTLE: u64 = 120;

/// Whether `name` is a domain name the DNS challenge can be run for: two labels or more, each
/// letters, digits and hyphens (not at either end), lower case, 253 characters at most.
pub fn domain_name(name: &str) -> bool {
    let labels: Vec<&str> = name.split('.').collect();
    name.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

pub(super) fn acme(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<Acme>> {
    let value = |key: &str| {
        get(key)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let (domain, token) = match (
        value("MELETE_ACME_DOMAIN"),
        value("MELETE_ACME_DESEC_TOKEN"),
    ) {
        (None, None) => return Ok(None),
        (Some(domain), Some(token)) => (domain, token),
        (Some(_), None) => bail!(
            "MELETE_ACME_DOMAIN is set, so the DNS challenge needs a deSEC token (MELETE_ACME_DESEC_TOKEN)"
        ),
        (None, Some(_)) => bail!(
            "MELETE_ACME_DESEC_TOKEN is set, so the domain the certificate is for is needed too (MELETE_ACME_DOMAIN)"
        ),
    };
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    anyhow::ensure!(
        !domain.starts_with("*."),
        "MELETE_ACME_DOMAIN is the domain alone, e.g. example.org: the wildcard is added to it"
    );
    anyhow::ensure!(
        domain_name(&domain),
        "MELETE_ACME_DOMAIN={domain:?} is not a domain name like example.org"
    );
    match value("MELETE_ACME_CHALLENGE").as_deref() {
        None | Some("dns") => {}
        Some(other) => bail!("MELETE_ACME_CHALLENGE is dns, not {other:?}"),
    }
    let staging = match value("MELETE_ACME_CA").as_deref() {
        None | Some("staging") => true,
        Some("production") => false,
        Some(other) => bail!("MELETE_ACME_CA is staging or production, not {other:?}"),
    };
    let settle = match value("MELETE_ACME_DNS_WAIT") {
        None => DEFAULT_SETTLE,
        Some(seconds) => seconds
            .parse::<u64>()
            .ok()
            .filter(|s| *s <= 900)
            .with_context(|| {
                format!("MELETE_ACME_DNS_WAIT={seconds:?} is a number of seconds from 0 to 900")
            })?,
    };
    let email = value("MELETE_ACME_EMAIL");
    if let Some(email) = &email {
        anyhow::ensure!(
            super::alerts::address(email),
            "MELETE_ACME_EMAIL is one email address, like admin@example.org"
        );
    }
    Ok(Some(Acme {
        domain,
        desec_token: Secret(token),
        email,
        staging,
        directory: if staging { STAGING } else { PRODUCTION }.to_owned(),
        settle: std::time::Duration::from_secs(settle),
    }))
}

/// Certificates as set, and HTTPS, which serves the certificate Melete gets when it's on.
pub(super) fn with_tls(
    get: &impl Fn(&str) -> Option<String>,
    data_dir: &std::path::Path,
) -> anyhow::Result<(Option<Acme>, Option<Tls>)> {
    let acme = acme(get)?;
    let folder = acme.as_ref().map(|_| folder(data_dir));
    let tls = super::tls::tls_with(get, folder.as_deref())?;
    Ok((acme, tls))
}
