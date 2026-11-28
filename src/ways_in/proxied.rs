//! Ways in that take in a trusted proxy's address. The ways go by the connection's own address,
//! the proxy's, whatever its PROXY header or `X-Forwarded-For` says of the client (that's only
//! for sign-in limits and the log), so everyone the proxy passes on, from anywhere, comes in by
//! such a way and reaches what it opens. That's what a relay for the household is for; for a
//! relay meant for guests alone, a way that opens more than watch parties is worth a word.

use super::{Net, Part, Way, WaysIn};
use crate::admin_health::{Finding, Level};

impl Net {
    /// Whether the two share an address: with ranges, one holds the other.
    fn overlaps(&self, other: &Net) -> bool {
        self.contains(other.addr) || other.contains(self.addr)
    }

    /// As an admin would write it: "10.20.0.1", "10.20.0.0/24".
    fn written(&self) -> String {
        let most = if self.addr.is_ipv4() { 32 } else { 128 };
        if self.prefix == most {
            self.addr.to_string()
        } else {
            format!("{}/{}", self.addr, self.prefix)
        }
    }
}

/// What a way opens, in `MELETE_WAYS_IN`'s words.
fn opened_words(way: &Way) -> String {
    let Some(parts) = &way.parts else {
        return "all".to_owned();
    };
    let words: Vec<&str> = parts
        .iter()
        .filter_map(|p| match p {
            Part::Video => Some("video"),
            Part::Music => Some("music"),
            Part::Requests => Some("requests"),
            Part::Admin => Some("admin"),
            Part::Party => Some("party"),
            Part::Always | Part::Account => None,
        })
        .collect();
    words.join(",")
}

impl WaysIn {
    /// Ways in that take in a trusted proxy's address and open some of the household's parts
    /// (video, music, requests or admin, and with them signing in): each way's name, the proxy
    /// as written, and what the way opens.
    pub fn proxied_household_ways(&self) -> Vec<(String, String, String)> {
        let Some(ways) = &self.ways else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for way in ways.iter().filter(|w| w.opens(Part::Account)) {
            let proxy = self
                .proxies
                .iter()
                .find(|p| way.nets.iter().any(|n| n.overlaps(p)));
            if let Some(proxy) = proxy {
                found.push((way.name.clone(), proxy.written(), opened_words(way)));
            }
        }
        found
    }

    /// Admin health's notes under System: one for each way a trusted proxy comes in by that
    /// opens the household's parts.
    pub fn findings(&self) -> Vec<Finding> {
        self.proxied_household_ways()
            .into_iter()
            .map(|(way, proxy, opens)| Finding {
                level: Level::Note,
                code: "proxy_way_opens_household",
                message: format!(
                    "Everyone the trusted proxy at {proxy} passes on, from anywhere, comes in by the way in \"{way}\" and reaches what it opens ({opens}), signing in included. That's right for a relay the household uses; for one meant for watch party guests alone, open only party through it."
                ),
                count: 1,
                id: None,
            })
            .collect()
    }

    /// The warnings worth giving at startup about the ways in: one address standing for many
    /// clients, and ways a trusted proxy opens the household's parts by.
    pub fn warn_at_start(&self) {
        // A proxy not trusted to say who its clients are makes them all one address: one
        // client's failed sign-ins or Quick Connect codes then hold up all the others.
        for (way, address) in self.untrusted_single_hosts() {
            tracing::warn!(
                way,
                %address,
                "this way in comes from one address, likely a reverse proxy; add it to MELETE_TRUSTED_PROXIES, or every client through it counts as that one address for sign-in limits and Quick Connect"
            );
        }
        for (way, proxy, opens) in self.proxied_household_ways() {
            tracing::warn!(
                way,
                proxy,
                opens,
                "this way in takes in a trusted proxy's address, so everyone the proxy passes on, from anywhere, reaches what it opens, signing in included; for a relay meant for watch party guests alone, open only party through it"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ways_in::{parse_nets, parse_ways};

    fn ways(ways: &str, proxies: &str) -> WaysIn {
        WaysIn {
            ways: Some(parse_ways(ways).unwrap()),
            proxies: parse_nets(proxies).unwrap(),
        }
    }

    #[test]
    fn a_way_for_a_trusted_proxy_that_opens_the_household_is_noted() {
        let found = ways(
            "home 192.168.1.0/24 all; vps 10.20.0.1 music,requests",
            "10.20.0.1",
        )
        .proxied_household_ways();
        assert_eq!(
            found,
            vec![(
                "vps".to_owned(),
                "10.20.0.1".to_owned(),
                "music,requests".to_owned()
            )]
        );
        // A proxy written as a range, inside a way's range.
        let found = ways("relay 10.20.0.0/24 all", "10.20.0.1/32").proxied_household_ways();
        assert_eq!(
            found,
            vec![("relay".to_owned(), "10.20.0.1".to_owned(), "all".to_owned())]
        );
        let found = ways("relay 10.20.0.1 video", "10.20.0.0/24").proxied_household_ways();
        assert_eq!(found[0].1, "10.20.0.0/24");
    }

    #[test]
    fn a_way_for_guests_alone_or_one_no_proxy_comes_by_is_not() {
        // Watch parties alone: what a relay for guests should open.
        assert!(
            ways("vps 10.20.0.1 party", "10.20.0.1")
                .proxied_household_ways()
                .is_empty()
        );
        // The household's ways, which no trusted proxy comes in by.
        assert!(
            ways("home 192.168.1.0/24 all; vps 10.20.0.1 party", "10.20.0.1")
                .proxied_household_ways()
                .is_empty()
        );
        // No ways set: everything reaches everything, and there's nothing to say of a way.
        let none = WaysIn {
            ways: None,
            proxies: parse_nets("10.20.0.1").unwrap(),
        };
        assert!(none.proxied_household_ways().is_empty());
        assert!(none.findings().is_empty());
    }

    #[test]
    fn the_note_names_the_way_the_proxy_and_what_it_opens() {
        let set = ways("vps 10.20.0.1 music,party", "10.20.0.1");
        // Anyone through the proxy is let in by that way: the reason for the note.
        assert!(
            set.allows("10.20.0.1".parse().unwrap(), Part::Music)
                .is_ok()
        );
        let findings = set.findings();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, "proxy_way_opens_household");
        assert_eq!(findings[0].level, Level::Note);
        let message = &findings[0].message;
        for word in ["\"vps\"", "10.20.0.1", "(music,party)", "only party"] {
            assert!(message.contains(word), "{word} in {message}");
        }
    }
}
