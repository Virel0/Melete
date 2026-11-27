//! Asking a nameserver itself for a name's TXT record, over UDP: enough to see that deSEC's
//! servers answer the challenge's value before the CA is told to look. Only the question goes
//! out (the name and "TXT"), to the nameserver asked; nothing is cached or recursed.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::UdpSocket;

/// How long one answer may take.
const WAIT: Duration = Duration::from_secs(3);
const TXT: u16 = 16;

/// A query for `name`'s TXT record, with id `id`, not asking for recursion.
fn query(id: u16, name: &str) -> Vec<u8> {
    let mut out = id.to_be_bytes().to_vec();
    out.extend([0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.trim_end_matches('.').split('.') {
        out.push(u8::try_from(label.len()).unwrap_or(63).min(63));
        out.extend(label.bytes().take(63));
    }
    out.extend([0, 0, 16, 0, 1]);
    out
}

/// Skips a name at `at` in `packet` (labels, ending in a zero or a pointer); where it ends.
fn skip_name(packet: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let len = *packet.get(at)?;
        match len {
            0 => return Some(at + 1),
            l if l & 0xc0 == 0xc0 => return Some(at + 2),
            l => at += 1 + usize::from(l),
        }
    }
}

fn u16_at(packet: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*packet.get(at)?, *packet.get(at + 1)?]))
}

/// The TXT values in an answer to the query with id `id`: each record's strings joined.
pub fn answers(id: u16, packet: &[u8]) -> Option<Vec<String>> {
    if u16_at(packet, 0)? != id || packet.get(2)? & 0x80 == 0 || packet.get(3)? & 0x0f != 0 {
        return None;
    }
    let questions = u16_at(packet, 4)?;
    let records = u16_at(packet, 6)?;
    let mut at = 12;
    for _ in 0..questions {
        at = skip_name(packet, at)? + 4;
    }
    let mut found = Vec::new();
    for _ in 0..records {
        at = skip_name(packet, at)?;
        let kind = u16_at(packet, at)?;
        let length = usize::from(u16_at(packet, at + 8)?);
        let data = packet.get(at + 10..at + 10 + length)?;
        at += 10 + length;
        if kind != TXT {
            continue;
        }
        let mut value = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let n = usize::from(data[i]);
            value.extend(data.get(i + 1..i + 1 + n)?);
            i += 1 + n;
        }
        found.push(String::from_utf8_lossy(&value).into_owned());
    }
    Some(found)
}

/// `name`'s TXT values as the nameserver at `server` answers; empty when it has none, an error
/// when it doesn't answer in time.
pub async fn txt(server: SocketAddr, name: &str) -> anyhow::Result<Vec<String>> {
    let local: SocketAddr = if server.is_ipv4() {
        "0.0.0.0:0".parse()?
    } else {
        "[::]:0".parse()?
    };
    let socket = UdpSocket::bind(local).await?;
    socket.connect(server).await?;
    let mut id = [0u8; 2];
    getrandom::fill(&mut id).map_err(|e| anyhow::anyhow!("no randomness available: {e}"))?;
    let id = u16::from_be_bytes(id);
    socket.send(&query(id, name)).await?;
    let mut buffer = vec![0u8; 4096];
    let waited = tokio::time::timeout(WAIT, async {
        loop {
            let n = socket.recv(&mut buffer).await?;
            if let Some(found) = answers(id, &buffer[..n]) {
                return anyhow::Ok(found);
            }
        }
    })
    .await;
    waited.map_err(|_| anyhow::anyhow!("{server} didn't answer in time"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desec::testing::dns_answer as answer;

    #[test]
    fn an_answer_gives_its_txt_values_and_a_wrong_one_none() {
        let asked = query(0x1234, "_acme-challenge.example.org");
        assert_eq!(&asked[12..13], &[15]);
        let said = answer(&asked, &["one", "two"]);
        assert_eq!(
            answers(0x1234, &said),
            Some(vec!["one".to_owned(), "two".to_owned()])
        );
        assert_eq!(answers(0x1235, &said), None, "another query's answer");
        assert_eq!(answers(0x1234, &asked), None, "a question isn't an answer");
        assert_eq!(answers(0x1234, &said[..said.len() - 2]), None, "cut short");
        let mut refused = answer(&asked, &[]);
        refused[3] = 5;
        assert_eq!(answers(0x1234, &refused), None);
    }
}
