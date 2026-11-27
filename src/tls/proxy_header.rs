//! The PROXY protocol's header, v1 (text) and v2 (binary), as HAProxy and nginx send it ahead of
//! a connection they pass through: who the client is. Read only on connections from a trusted
//! proxy, exactly as long as the header is, so the TLS that follows is left as it is; anything
//! that isn't a well-formed header within its size is refused.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use tokio::io::{AsyncRead, AsyncReadExt};

/// v2's opening, and the longest v1 line the protocol allows (CRLF included).
const V2_SIGNATURE: [u8; 12] = *b"\r\n\r\n\0\r\nQUIT\n";
const V1_MOST: usize = 107;
/// The longest v2 address block taken (its TLVs included); the protocol allows more, which
/// nothing here needs.
const V2_MOST: usize = 512;

/// Why a header was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Bad {
    NotAHeader,
    TooLong,
    Malformed(&'static str),
}

/// The client a v1 line names; None for `PROXY UNKNOWN` (a health check, say).
pub fn v1(line: &str) -> Result<Option<IpAddr>, Bad> {
    let line = line.strip_suffix("\r\n").ok_or(Bad::Malformed("no CRLF"))?;
    let mut parts = line.split(' ');
    if parts.next() != Some("PROXY") {
        return Err(Bad::NotAHeader);
    }
    let family = parts.next().ok_or(Bad::Malformed("no family"))?;
    if family == "UNKNOWN" {
        return Ok(None);
    }
    let rest: Vec<&str> = parts.collect();
    let [source, _, source_port, dest_port] = rest[..] else {
        return Err(Bad::Malformed("not four fields"));
    };
    let client: IpAddr = match family {
        "TCP4" => IpAddr::V4(
            source
                .parse::<Ipv4Addr>()
                .map_err(|_| Bad::Malformed("TCP4 address"))?,
        ),
        "TCP6" => IpAddr::V6(
            source
                .parse::<Ipv6Addr>()
                .map_err(|_| Bad::Malformed("TCP6 address"))?,
        ),
        _ => return Err(Bad::Malformed("family")),
    };
    for port in [source_port, dest_port] {
        port.parse::<u16>().map_err(|_| Bad::Malformed("port"))?;
    }
    Ok(Some(client))
}

/// The client a v2 header names, from the 4 bytes after the signature and the address block;
/// None for a LOCAL connection or a family other than TCP over IPv4 or IPv6.
pub fn v2(head: [u8; 4], block: &[u8]) -> Result<Option<IpAddr>, Bad> {
    let [version_command, family, ..] = head;
    if version_command >> 4 != 2 {
        return Err(Bad::Malformed("version"));
    }
    match version_command & 0x0f {
        0 => return Ok(None),
        1 => {}
        _ => return Err(Bad::Malformed("command")),
    }
    Ok(match family {
        0x11 if block.len() >= 12 => Some(IpAddr::V4(Ipv4Addr::new(
            block[0], block[1], block[2], block[3],
        ))),
        0x21 if block.len() >= 36 => {
            let mut source = [0u8; 16];
            source.copy_from_slice(&block[..16]);
            Some(IpAddr::V6(Ipv6Addr::from(source)))
        }
        0x11 | 0x21 => return Err(Bad::Malformed("address block too short")),
        _ => None,
    })
}

/// Reads a PROXY header off the start of `stream`, and no more: the client it names.
pub async fn read(stream: &mut (impl AsyncRead + Unpin)) -> Result<Option<IpAddr>, Bad> {
    let io = |_| Bad::Malformed("the connection ended");
    let mut first = [0u8; 12];
    stream.read_exact(&mut first[..5]).await.map_err(io)?;
    if &first[..5] == b"PROXY" {
        let mut line = b"PROXY".to_vec();
        while !line.ends_with(b"\r\n") {
            if line.len() >= V1_MOST {
                return Err(Bad::TooLong);
            }
            line.push(stream.read_u8().await.map_err(io)?);
        }
        let line = std::str::from_utf8(&line).map_err(|_| Bad::Malformed("not text"))?;
        return v1(line);
    }
    stream.read_exact(&mut first[5..]).await.map_err(io)?;
    if first != V2_SIGNATURE {
        return Err(Bad::NotAHeader);
    }
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await.map_err(io)?;
    let length = usize::from(u16::from_be_bytes([head[2], head[3]]));
    if length > V2_MOST {
        return Err(Bad::TooLong);
    }
    let mut block = vec![0u8; length];
    stream.read_exact(&mut block).await.map_err(io)?;
    v2(head, &block)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_v1_line_names_its_client() {
        assert_eq!(
            v1("PROXY TCP4 203.0.113.9 192.168.1.10 56324 8443\r\n"),
            Ok(Some("203.0.113.9".parse().unwrap()))
        );
        assert_eq!(
            v1("PROXY TCP6 2001:db8::1 2001:db8::2 4000 443\r\n"),
            Ok(Some("2001:db8::1".parse().unwrap()))
        );
        assert_eq!(v1("PROXY UNKNOWN\r\n"), Ok(None));
        for bad in [
            "PROXY TCP4 203.0.113.9 192.168.1.10 56324 8443",
            "PROXY TCP4 nope 192.168.1.10 1 2\r\n",
            "PROXY TCP4 203.0.113.9 192.168.1.10 99999 2\r\n",
            "PROXY UDP4 203.0.113.9 192.168.1.10 1 2\r\n",
            "GET / HTTP/1.1\r\n",
        ] {
            assert!(v1(bad).is_err(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn a_header_is_read_and_nothing_after_it() {
        // v2, TCP over IPv4, then the TLS that follows.
        let mut bytes = V2_SIGNATURE.to_vec();
        bytes.extend([
            0x21, 0x11, 0, 12, 203, 0, 113, 9, 192, 168, 1, 10, 0xdc, 0x04, 0x20, 0xfb,
        ]);
        bytes.extend(b"\x16\x03\x01");
        let mut stream = &bytes[..];
        assert_eq!(
            read(&mut stream).await,
            Ok(Some("203.0.113.9".parse().unwrap()))
        );
        assert_eq!(stream, b"\x16\x03\x01", "the TLS that follows is left");

        let line = b"PROXY TCP4 203.0.113.9 192.168.1.10 56324 8443\r\n\x16\x03".to_vec();
        let mut stream = &line[..];
        assert_eq!(
            read(&mut stream).await,
            Ok(Some("203.0.113.9".parse().unwrap()))
        );
        assert_eq!(stream, b"\x16\x03");

        // LOCAL (a health check) names nobody.
        let mut local = V2_SIGNATURE.to_vec();
        local.extend([0x20, 0x00, 0, 0]);
        assert_eq!(read(&mut &local[..]).await, Ok(None));
    }

    #[tokio::test]
    async fn anything_else_is_refused() {
        assert_eq!(
            read(&mut &b"\x16\x03\x01\x02\x00\x01\x00\x01\xfc\x03\x03\x00"[..]).await,
            Err(Bad::NotAHeader)
        );
        let long = format!("PROXY TCP4 {}\r\n", "1".repeat(120));
        assert_eq!(read(&mut long.as_bytes()).await, Err(Bad::TooLong));
        let mut huge = V2_SIGNATURE.to_vec();
        huge.extend([0x21, 0x11, 0xff, 0xff]);
        assert_eq!(read(&mut &huge[..]).await, Err(Bad::TooLong));
        let mut short = V2_SIGNATURE.to_vec();
        short.extend([0x21, 0x11, 0, 4, 1, 2, 3, 4]);
        assert!(read(&mut &short[..]).await.is_err());
        assert!(read(&mut &b"PROX"[..]).await.is_err(), "cut off");
    }
}
