//! The stack on its own, fed hand-made TCP segments as a peer's tunnel would send them.

use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{Ipv4Repr, TcpControl, TcpRepr, TcpSeqNumber};

use super::*;

const OWN: Ipv4Addr = Ipv4Addr::new(10, 73, 0, 1);
const GREEDY: Ipv4Addr = Ipv4Addr::new(10, 73, 0, 2);
const OTHER: Ipv4Addr = Ipv4Addr::new(10, 73, 0, 3);

/// A segment from `from`'s `port` to the HTTP server.
fn segment(
    from: Ipv4Addr,
    port: u16,
    control: TcpControl,
    (seq, ack): (u32, Option<u32>),
    payload: &[u8],
) -> Vec<u8> {
    let tcp = TcpRepr {
        src_port: port,
        dst_port: PORT,
        control,
        seq_number: TcpSeqNumber(seq.cast_signed()),
        ack_number: ack.map(|a| TcpSeqNumber(a.cast_signed())),
        window_len: 65535,
        window_scale: None,
        max_seg_size: None,
        sack_permitted: false,
        sack_ranges: [None; 3],
        timestamp: None,
        payload,
    };
    let ip = Ipv4Repr {
        src_addr: from,
        dst_addr: OWN,
        next_header: IpProtocol::Tcp,
        payload_len: tcp.buffer_len(),
        hop_limit: 64,
    };
    let mut packet = vec![0; ip.buffer_len() + tcp.buffer_len()];
    let caps = ChecksumCapabilities::default();
    let mut ip_packet = Ipv4Packet::new_unchecked(&mut packet);
    ip.emit(&mut ip_packet, &caps);
    tcp.emit(
        &mut TcpPacket::new_unchecked(ip_packet.payload_mut()),
        &IpAddress::Ipv4(from),
        &IpAddress::Ipv4(OWN),
        &caps,
    );
    packet
}

fn syn(from: Ipv4Addr, port: u16) -> Vec<u8> {
    segment(from, port, TcpControl::Syn, (100, None), &[])
}

/// What the stack sent back: to whom, at which port, its flags and sequence number.
#[derive(Debug)]
struct Reply {
    to: Ipv4Addr,
    port: u16,
    syn_ack: bool,
    rst: bool,
    seq: u32,
}

struct Harness {
    to_stack: mpsc::Sender<Vec<u8>>,
    from_stack: mpsc::Receiver<Vec<u8>>,
    accepted: mpsc::Receiver<(Stream, SocketAddr)>,
}

impl Harness {
    fn start() -> Harness {
        let (to_stack, from_tunnel) = mpsc::channel(QUEUE);
        let (to_tunnel, from_stack) = mpsc::channel(QUEUE);
        let (accepted_tx, accepted) = mpsc::channel(QUEUE);
        let joins = Joins {
            from_tunnel,
            to_tunnel,
            accepted: accepted_tx,
        };
        let stack = Stack::new(OWN, 24, joins, Arc::new(Forget::default()));
        tokio::spawn(stack.run());
        Harness {
            to_stack,
            from_stack,
            accepted,
        }
    }

    /// Sends `packet`, and gives the stack a moment with it.
    async fn send(&self, packet: Vec<u8>) {
        self.to_stack.send(packet).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    /// What the stack has sent back since last asked.
    async fn replies(&mut self) -> Vec<Reply> {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut replies = Vec::new();
        while let Ok(packet) = self.from_stack.try_recv() {
            let ip = Ipv4Packet::new_checked(&packet).unwrap();
            let tcp = TcpPacket::new_checked(ip.payload()).unwrap();
            replies.push(Reply {
                to: ip.dst_addr(),
                port: tcp.dst_port(),
                syn_ack: tcp.syn() && tcp.ack(),
                rst: tcp.rst(),
                seq: tcp.seq_number().0.cast_unsigned(),
            });
        }
        replies
    }
}

#[tokio::test]
async fn a_peer_starting_connections_it_never_finishes_leaves_room_for_others() {
    let mut stack = Harness::start();
    // More half-made connections than the stack has sockets, from one peer.
    for port in 0..u16::try_from(MAX_CONNECTIONS + LISTENING + 4).unwrap() {
        stack.send(syn(GREEDY, 1000 + port)).await;
    }
    stack.replies().await;
    stack.send(syn(OTHER, 2000)).await;
    let replies = stack.replies().await;
    assert!(
        replies.iter().any(|r| r.to == OTHER && r.syn_ack),
        "{replies:?}"
    );
}

#[tokio::test]
async fn a_connection_that_has_finished_sending_by_the_time_it_is_made_is_still_answered() {
    let mut stack = Harness::start();
    stack.send(syn(OTHER, 3000)).await;
    let replies = stack.replies().await;
    let syn_ack = replies
        .iter()
        .find(|r| r.syn_ack)
        .expect("the stack answers");
    // The handshake's last step, a request and the end of sending, in one segment.
    let request = b"GET / HTTP/1.1\r\nHost: melete\r\n\r\n";
    let ack = Some(syn_ack.seq.wrapping_add(1));
    stack
        .send(segment(OTHER, 3000, TcpControl::Fin, (101, ack), request))
        .await;
    let (_, from) = tokio::time::timeout(Duration::from_secs(2), stack.accepted.recv())
        .await
        .expect("handed to the server")
        .unwrap();
    assert_eq!(from, SocketAddr::new(OTHER.into(), 3000));
}

#[tokio::test]
async fn a_connection_left_half_made_is_dropped() {
    let mut stack = Harness::start();
    stack.send(syn(GREEDY, 4000)).await;
    assert!(stack.replies().await.iter().any(|r| r.syn_ack));
    tokio::time::sleep(HANDSHAKE + Duration::from_secs(1)).await;
    let replies = stack.replies().await;
    assert!(
        replies.iter().any(|r| r.port == 4000 && r.rst),
        "{replies:?}"
    );
}
