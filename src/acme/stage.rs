//! Where an attempt failed, said in fixed words: what an alert carries, rather than the CA's or
//! deSEC's own text, which can run long and quote what was sent (the contact address, say). The
//! details stay on the Certificates page.

/// The step of an attempt that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Account,
    Order,
    Dns,
    Served,
    Challenge,
    Issue,
    Keep,
}

impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Stage::Account => "the CA couldn't be reached, or its account opened",
            Stage::Order => {
                "the CA turned the order down (a rate limit, or a policy such as a CAA record)"
            }
            Stage::Dns => {
                "deSEC turned the change to the challenge's TXT record down, or didn't answer"
            }
            Stage::Served => "deSEC's nameservers didn't answer the challenge's TXT record in time",
            Stage::Challenge => "the CA didn't accept the DNS challenge",
            Stage::Issue => "the CA didn't issue the certificate",
            Stage::Keep => "the certificate couldn't be kept in the config folder",
        })
    }
}

/// What an alert says went wrong with `err`: its stage, or nothing more particular.
pub fn said(err: &anyhow::Error) -> String {
    err.downcast_ref::<Stage>().map_or_else(
        || "something went wrong on the server".to_owned(),
        ToString::to_string,
    )
}
