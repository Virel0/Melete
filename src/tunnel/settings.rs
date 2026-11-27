//! The tunnel's settings: the UDP port it listens on (`MELETE_TUNNEL_PORT`), the address its
//! peers dial (`MELETE_TUNNEL_ENDPOINT`), and the addresses inside it (`MELETE_TUNNEL_NET`).
//! Off unless the port is set.

use std::net::Ipv4Addr;

use anyhow::bail;

/// The addresses inside the tunnel unless `MELETE_TUNNEL_NET` says otherwise.
const DEFAULT_NET: (Ipv4Addr, u8) = (Ipv4Addr::new(10, 73, 0, 0), 24);

/// How the tunnel is set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The UDP port it listens on, on every address of the container.
    pub port: u16,
    /// Where peers reach it, `host:port`, as their configs say.
    pub endpoint: String,
    /// The range inside the tunnel: Melete has its first address, peers the rest.
    pub net: (Ipv4Addr, u8),
}

impl Settings {
    /// The settings `get` gives; None when the port isn't set.
    ///
    /// # Errors
    /// What's wrong with a value, worded for the admin who set it.
    pub fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<Self>> {
        let set = |key: &str| get(key).filter(|v| !v.trim().is_empty());
        let Some(port) = set("MELETE_TUNNEL_PORT") else {
            return Ok(None);
        };
        let Some(port) = port.trim().parse::<u16>().ok().filter(|p| *p > 0) else {
            bail!("MELETE_TUNNEL_PORT={port:?} should be a UDP port, 1 to 65535");
        };
        let Some(endpoint) = set("MELETE_TUNNEL_ENDPOINT") else {
            bail!(
                "MELETE_TUNNEL_PORT needs MELETE_TUNNEL_ENDPOINT: the address peers reach it at, \
                 like melete.example:{port}"
            );
        };
        let endpoint = endpoint.trim();
        if endpoint
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        {
            bail!("MELETE_TUNNEL_ENDPOINT={endpoint:?} should be a host, or host:port");
        }
        let has_port = endpoint.rsplit_once(':').is_some_and(|(host, p)| {
            !host.is_empty() && !host.ends_with(':') && p.parse::<u16>().is_ok()
        });
        let endpoint = if has_port {
            endpoint.to_owned()
        } else {
            format!("{endpoint}:{port}")
        };
        let net = match set("MELETE_TUNNEL_NET") {
            None => DEFAULT_NET,
            Some(text) => net(text.trim())?,
        };
        Ok(Some(Settings {
            port,
            endpoint,
            net,
        }))
    }

    /// Melete's own address inside the tunnel: the range's first.
    pub fn own_address(&self) -> Ipv4Addr {
        Ipv4Addr::from(u32::from(self.net.0) + 1)
    }

    /// The addresses peers may be given, lowest first: all but the range's own, Melete's and
    /// the last (broadcast).
    pub fn peer_addresses(&self) -> impl Iterator<Item = Ipv4Addr> {
        let first = u32::from(self.net.0) + 2;
        let last = u32::from(self.net.0) + (1u32 << (32 - self.net.1)) - 2;
        (first..=last).map(Ipv4Addr::from)
    }

    /// Whether `address` is one peers may be given.
    pub fn is_peer_address(&self, address: Ipv4Addr) -> bool {
        let first = u32::from(self.net.0) + 2;
        let last = u32::from(self.net.0) + (1u32 << (32 - self.net.1)) - 2;
        (first..=last).contains(&u32::from(address))
    }
}

/// A range as `MELETE_TUNNEL_NET` gives it: private IPv4, from /16 to /29, written from its
/// start ("10.73.0.0/24").
fn net(text: &str) -> anyhow::Result<(Ipv4Addr, u8)> {
    let wrong = || {
        anyhow::anyhow!(
            "MELETE_TUNNEL_NET={text:?} should be a private IPv4 range from /16 to /29, like 10.73.0.0/24"
        )
    };
    let (addr, prefix) = text.split_once('/').ok_or_else(wrong)?;
    let addr: Ipv4Addr = addr.parse().map_err(|_| wrong())?;
    let prefix: u8 = prefix.parse().map_err(|_| wrong())?;
    if !(16..=29).contains(&prefix) || !addr.is_private() {
        return Err(wrong());
    }
    let mask = u32::MAX << (32 - prefix);
    if u32::from(addr) & !mask != 0 {
        return Err(wrong());
    }
    Ok((addr, prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(pairs: &[(&str, &str)]) -> anyhow::Result<Option<Settings>> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Settings::from_lookup(&|key: &str| {
            pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        })
    }

    #[test]
    fn off_unless_the_port_is_set() {
        assert_eq!(read(&[]).unwrap(), None);
        assert_eq!(read(&[("MELETE_TUNNEL_PORT", " ")]).unwrap(), None);
        assert_eq!(
            read(&[("MELETE_TUNNEL_ENDPOINT", "melete.example")]).unwrap(),
            None
        );
    }

    #[test]
    fn reads_the_port_the_endpoint_and_the_range() {
        let on = read(&[
            ("MELETE_TUNNEL_PORT", "51820"),
            ("MELETE_TUNNEL_ENDPOINT", "melete.example"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(on.endpoint, "melete.example:51820");
        assert_eq!(on.net, DEFAULT_NET);
        assert_eq!(on.own_address(), Ipv4Addr::new(10, 73, 0, 1));
        let peers: Vec<_> = on.peer_addresses().collect();
        assert_eq!(peers.first(), Some(&Ipv4Addr::new(10, 73, 0, 2)));
        assert_eq!(peers.last(), Some(&Ipv4Addr::new(10, 73, 0, 254)));
        let mapped = read(&[
            ("MELETE_TUNNEL_PORT", "51820"),
            ("MELETE_TUNNEL_ENDPOINT", "192.168.1.10:443"),
            ("MELETE_TUNNEL_NET", "172.16.8.0/29"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(mapped.endpoint, "192.168.1.10:443");
        assert_eq!(mapped.peer_addresses().count(), 5);
        // A bracketed IPv6 endpoint keeps its own port.
        let v6 = read(&[
            ("MELETE_TUNNEL_PORT", "51820"),
            ("MELETE_TUNNEL_ENDPOINT", "[2001:db8::1]:51821"),
        ]);
        assert_eq!(v6.unwrap().unwrap().endpoint, "[2001:db8::1]:51821");
    }

    #[test]
    fn refuses_what_doesnt_fit() {
        let with = |key, value| {
            let mut pairs = vec![
                ("MELETE_TUNNEL_PORT", "51820"),
                ("MELETE_TUNNEL_ENDPOINT", "melete.example"),
            ];
            pairs.retain(|(k, _)| *k != key);
            pairs.push((key, value));
            read(&pairs)
        };
        assert!(with("MELETE_TUNNEL_PORT", "0").is_err());
        assert!(with("MELETE_TUNNEL_PORT", "udp").is_err());
        assert!(with("MELETE_TUNNEL_ENDPOINT", "").is_err());
        assert!(with("MELETE_TUNNEL_ENDPOINT", "melete .example").is_err());
        for range in [
            "8.8.8.0/24",
            "10.73.0.0/8",
            "10.73.0.0/30",
            "10.73.0.1/24",
            "10.73.0.0",
        ] {
            assert!(with("MELETE_TUNNEL_NET", range).is_err(), "{range}");
        }
    }
}
