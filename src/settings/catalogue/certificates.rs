//! HTTPS's certificate got by Melete itself from Let's Encrypt, by the DNS challenge through
//! deSEC.

use super::*;

/// What it says leaves the server.
#[cfg(test)]
pub(super) const WHAT: [&str; 5] = [
    "The domain's names go to Let's Encrypt and to deSEC",
    "the challenge's TXT values go to deSEC",
    "The deSEC token goes to deSEC alone",
    "No email goes to Let's Encrypt unless one is set here",
    "only that name goes to Let's Encrypt",
];

pub(super) const CERTIFICATES: Service = Service {
    key: "certificates",
    name: "Certificates (Let's Encrypt)",
    group: Group::SignIn,
    purpose: "HTTPS's certificate, got and renewed by Melete",
    about: "Gets the certificate for Melete's own HTTPS (MELETE_TLS_BIND) from Let's Encrypt, and renews it before it runs out, so nothing else at home needs to. The domain is proved by the DNS challenge: Melete sets a TXT record at _acme-challenge under it through deSEC's API, waits until deSEC's nameservers answer it and a little longer, and removes it once Let's Encrypt has looked. The certificate is a wildcard (*.example.org) with the bare name, so the public Certificate Transparency logs list no other name. The domain's names go to Let's Encrypt and to deSEC, and the challenge's TXT values go to deSEC (anyone may look the record up while it's there). The deSEC token goes to deSEC alone, and stays on the server. The account's key is made on the server and kept sealed; Let's Encrypt sees only its public half and signed requests. No email goes to Let's Encrypt unless one is set here. Turning it on agrees to Let's Encrypt's Subscriber Agreement. A renewal that fails is tried again, and admins are told through Alerts, with the domain and when the certificate in use runs out. Test checks the token can write the zone, adding a TXT value at _acme-challenge and removing it, without asking for a certificate. Or, with the TLS-ALPN challenge, Let's Encrypt connects to the name on port 443, through the VPS passing HTTPS on, and Melete's HTTPS answers it with a certificate made for the check alone, only while an order waits: no DNS token on the server and nothing sent to deSEC, but the certificate is for that exact name, which the Certificate Transparency logs then list; only that name goes to Let's Encrypt.",
    fields: &[
        field(
            "MELETE_ACME_DOMAIN",
            "Domain",
            "The domain alone, as deSEC holds it or a name under it, e.g. example.org: the certificate is for *.example.org and example.org.",
            Input::Text,
        ),
        field(
            "MELETE_ACME_CHALLENGE",
            "Challenge",
            "DNS sets a TXT record through deSEC's API and gets a wildcard, with nothing reachable. TLS-ALPN has Melete's HTTPS answer Let's Encrypt on port 443 through the passthrough (MELETE_TLS_BIND needed): no deSEC token, but the domain is then the exact name Melete is reached at, which the CT logs list.",
            Input::Choice {
                options: &[
                    Choice::new("dns", "DNS, through deSEC (a wildcard)"),
                    Choice::new(
                        "tls-alpn",
                        "TLS-ALPN, through the HTTPS passthrough (the exact name)",
                    ),
                ],
            },
        ),
        field(
            "MELETE_ACME_DESEC_TOKEN",
            "deSEC token",
            "From deSEC's Token management. Recommended: a token scoped by a policy to the _acme-challenge TXT record under the domain alone, so it can change nothing else.",
            Input::Secret,
        ),
        field(
            "MELETE_ACME_DNS_WAIT",
            "Wait after deSEC answers",
            "Seconds to wait once deSEC's own nameservers answer the record, before Let's Encrypt is told to look: its checks from elsewhere in the world can see it a couple of minutes later. 120 when empty; 0 to 900.",
            Input::Number,
        ),
        field(
            "MELETE_ACME_EMAIL",
            "Contact email",
            "Optional: on the Let's Encrypt account, for its notices about it. Empty sends none.",
            Input::Text,
        ),
        field(
            "MELETE_ACME_CA",
            "Server",
            "Staging gives certificates no browser trusts, under rate limits a first try can't hit: switch to Let's Encrypt once a staging certificate comes through.",
            Input::Choice {
                options: &[
                    Choice::new(
                        "staging",
                        "Let's Encrypt's staging server, for trying it out",
                    ),
                    Choice::new("production", "Let's Encrypt"),
                ],
            },
        ),
    ],
};

#[cfg(test)]
#[test]
fn it_says_what_leaves_the_server() {
    for what in WHAT {
        assert!(CERTIFICATES.about.contains(what), "{what}");
    }
}
