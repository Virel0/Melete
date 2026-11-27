//! The tunnel's TCP/IP, in userspace (smoltcp): Melete's own address and one TCP port, where
//! the HTTP server is. A packet for anything else (another address, port or protocol, a
//! fragment) is dropped before the stack sees it, and the stack has no routes on, so nothing
//! that comes through the tunnel reaches the household's network, the host or the internet.
//! What it sends goes back into the tunnel, to the peer it's for, or nowhere.
//!
//! Its sockets are few and fixed in size: a handful listening, at most [`MAX_CONNECTIONS`]
//! open, [`PER_PEER`] of them for one peer, each with buffers of a set size. A socket counts
//! toward its peer from its first packet, while it's connecting, served or closing: one that
//! hasn't connected within [`HANDSHAKE`], or closed within [`CLOSING`] of the server being
//! done with it, is dropped, as is one that stops answering for a minute, and a removed
//! peer's are dropped at once.

use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpProtocol, Ipv4Packet, TcpPacket};
use tokio::sync::mpsc::error::{TryRecvError, TrySendError};
use tokio::sync::{Notify, mpsc};

use super::link::{self, CHUNK, Ends, Stream};

/// The port the HTTP server answers on, at Melete's tunnel address.
pub const PORT: u16 = 80;
/// The packets' largest size: WireGuard's usual, which fits under most links.
pub const MTU: u16 = 1420;
/// Connections open at once, for every peer together.
pub const MAX_CONNECTIONS: usize = 32;
/// Connections open at once from one peer.
pub const PER_PEER: usize = 8;
/// Sockets kept listening for a new connection.
const LISTENING: usize = 4;
/// Packets waiting either way.
pub const QUEUE: usize = 256;
const RX_BUFFER: usize = 64 * 1024;
const TX_BUFFER: usize = 256 * 1024;
/// How long a connection may go without its peer answering.
const TIMEOUT: Duration = Duration::from_secs(60);
const KEEP_ALIVE: Duration = Duration::from_secs(20);
/// How long the HTTP server keeps a connection on which no byte has been read or written.
pub const IDLE_LIMIT: Duration = Duration::from_secs(60);
/// How long a connection may take to be made, from its first packet.
pub const HANDSHAKE: Duration = Duration::from_secs(10);
/// How long a connection may take to close once the server is done with it: room for what's
/// still unsent, at a slow peer's pace.
pub const CLOSING: Duration = IDLE_LIMIT;
/// The longest the stack waits with nothing to do.
const IDLE: Duration = Duration::from_millis(500);

/// Peers removed, whose connections the stack drops as it next wakes.
#[derive(Default)]
pub struct Forget {
    addresses: Mutex<Vec<Ipv4Addr>>,
    pub wake: Arc<Notify>,
}

impl Forget {
    /// Drops every connection from `address`.
    pub fn address(&self, address: Ipv4Addr) {
        self.addresses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(address);
        self.wake.notify_one();
    }
}

/// Whether a decrypted packet may reach the stack: IPv4, whole, TCP, to Melete's own address
/// and port.
pub fn admits(own: Ipv4Addr, packet: &[u8]) -> bool {
    let Ok(ip) = Ipv4Packet::new_checked(packet) else {
        return false;
    };
    if ip.dst_addr() != own
        || ip.next_header() != IpProtocol::Tcp
        || ip.more_frags()
        || ip.frag_offset() != 0
    {
        return false;
    }
    TcpPacket::new_checked(ip.payload()).is_ok_and(|tcp| tcp.dst_port() == PORT)
}

/// Packets in and out of the stack, as smoltcp's device.
#[derive(Default)]
pub(super) struct Queues {
    pub(super) incoming: VecDeque<Vec<u8>>,
    pub(super) outgoing: VecDeque<Vec<u8>>,
}

pub(super) struct Incoming(Vec<u8>);
pub(super) struct Outgoing<'a>(&'a mut VecDeque<Vec<u8>>);

impl RxToken for Incoming {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl TxToken for Outgoing<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut packet = vec![0; len];
        let result = f(&mut packet);
        self.0.push_back(packet);
        result
    }
}

impl Device for Queues {
    type RxToken<'a> = Incoming;
    type TxToken<'a> = Outgoing<'a>;

    fn receive(&mut self, _now: Instant) -> Option<(Incoming, Outgoing<'_>)> {
        let packet = self.incoming.pop_front()?;
        Some((Incoming(packet), Outgoing(&mut self.outgoing)))
    }

    fn transmit(&mut self, _now: Instant) -> Option<Outgoing<'_>> {
        (self.outgoing.len() < QUEUE).then_some(Outgoing(&mut self.outgoing))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = usize::from(MTU);
        caps
    }
}

/// A socket, and once it's connected, its peer and its connection to the HTTP server.
struct Conn {
    handle: SocketHandle,
    peer: Option<(Ipv4Addr, Ends)>,
    /// Since when it's been connecting (out of listening, not yet handed to the server), then
    /// closing (the server done with it); None while it listens or is served.
    since: Option<Instant>,
}

/// The stack and what it's joined to.
pub struct Stack {
    own: Ipv4Addr,
    iface: Interface,
    sockets: SocketSet<'static>,
    conns: Vec<Conn>,
    queues: Queues,
    from_tunnel: mpsc::Receiver<Vec<u8>>,
    to_tunnel: mpsc::Sender<Vec<u8>>,
    accepted: mpsc::Sender<(Stream, SocketAddr)>,
    forget: Arc<Forget>,
    started: std::time::Instant,
}

/// The ends of the stack's queues, for WireGuard and the HTTP server.
pub struct Joins {
    pub from_tunnel: mpsc::Receiver<Vec<u8>>,
    pub to_tunnel: mpsc::Sender<Vec<u8>>,
    pub accepted: mpsc::Sender<(Stream, SocketAddr)>,
}

impl Stack {
    /// The stack at `own` in a range of `prefix` bits.
    pub fn new(own: Ipv4Addr, prefix: u8, joins: Joins, forget: Arc<Forget>) -> Stack {
        let mut queues = Queues::default();
        let mut config = Config::new(HardwareAddress::Ip);
        let mut seed = [0u8; 8];
        // Only for TCP's starting numbers; a fixed one would still work.
        let _ = getrandom::fill(&mut seed);
        config.random_seed = u64::from_le_bytes(seed);
        let mut iface = Interface::new(config, &mut queues, Instant::from_millis(0));
        iface.update_ip_addrs(|addrs| {
            addrs
                .push(IpCidr::new(IpAddress::Ipv4(own), prefix))
                .expect("an interface takes at least one address");
        });
        Stack {
            own,
            iface,
            sockets: SocketSet::new(Vec::new()),
            conns: Vec::new(),
            queues,
            from_tunnel: joins.from_tunnel,
            to_tunnel: joins.to_tunnel,
            accepted: joins.accepted,
            forget,
            started: std::time::Instant::now(),
        }
    }

    fn now(&self) -> Instant {
        Instant::from_micros(i64::try_from(self.started.elapsed().as_micros()).unwrap_or(i64::MAX))
    }

    /// Runs until WireGuard stops.
    pub async fn run(mut self) {
        self.listen();
        let wake = self.forget.wake.clone();
        loop {
            let now = self.now();
            let delay = self
                .iface
                .poll_delay(now, &self.sockets)
                .map_or(IDLE, |d| Duration::from_micros(d.total_micros()))
                .min(IDLE);
            tokio::select! {
                got = self.from_tunnel.recv() => match got {
                    Some(packet) => self.take(packet),
                    None => return,
                },
                () = wake.notified() => {}
                () = tokio::time::sleep(delay) => {}
            }
            while self.queues.incoming.len() < QUEUE
                && let Ok(packet) = self.from_tunnel.try_recv()
            {
                self.take(packet);
            }
            self.drop_forgotten();
            self.iface
                .poll(self.now(), &mut self.queues, &mut self.sockets);
            self.serve();
            self.listen();
            // What serving queued goes out now, not at the next wake.
            self.iface
                .poll(self.now(), &mut self.queues, &mut self.sockets);
            for packet in self.queues.outgoing.drain(..) {
                let _ = self.to_tunnel.try_send(packet);
            }
        }
    }

    fn take(&mut self, packet: Vec<u8>) {
        if admits(self.own, &packet) && self.queues.incoming.len() < QUEUE {
            self.queues.incoming.push_back(packet);
        }
    }

    /// Keeps [`LISTENING`] sockets listening, within [`MAX_CONNECTIONS`].
    fn listen(&mut self) {
        let listening = self
            .conns
            .iter()
            .filter(|c| self.sockets.get::<tcp::Socket>(c.handle).is_listening())
            .count();
        for _ in listening..LISTENING {
            if self.conns.len() >= MAX_CONNECTIONS + LISTENING {
                return;
            }
            let mut socket = tcp::Socket::new(
                tcp::SocketBuffer::new(vec![0; RX_BUFFER]),
                tcp::SocketBuffer::new(vec![0; TX_BUFFER]),
            );
            socket.set_timeout(Some(TIMEOUT.into()));
            socket.set_keep_alive(Some(KEEP_ALIVE.into()));
            if socket.listen((IpAddress::Ipv4(self.own), PORT)).is_err() {
                return;
            }
            let handle = self.sockets.add(socket);
            self.conns.push(Conn {
                handle,
                peer: None,
                since: None,
            });
        }
    }

    fn drop_forgotten(&mut self) {
        let gone: Vec<Ipv4Addr> = std::mem::take(
            &mut *self
                .forget
                .addresses
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        for conn in &mut self.conns {
            let socket = self.sockets.get_mut::<tcp::Socket>(conn.handle);
            let from = conn
                .peer
                .as_ref()
                .map(|(ip, _)| *ip)
                .or_else(|| remote(socket));
            if from.is_some_and(|ip| gone.contains(&ip)) {
                socket.abort();
                conn.peer = None;
            }
        }
    }

    /// Hands new connections to the HTTP server, moves data both ways, and lets closed
    /// sockets go: one connecting or closing too long, or past its peer's [`PER_PEER`], goes
    /// too.
    fn serve(&mut self) {
        let now = self.now();
        // Each peer's sockets out of listening: those served, then the rest as they come.
        let mut open: Vec<Ipv4Addr> = self
            .conns
            .iter()
            .filter_map(|c| c.peer.as_ref().map(|(ip, _)| *ip))
            .collect();
        let mut kept = Vec::with_capacity(self.conns.len());
        for mut conn in std::mem::take(&mut self.conns) {
            let socket = self.sockets.get_mut::<tcp::Socket>(conn.handle);
            // One closed since the last poll stays for the next, which sends the peer its
            // reset when it was aborted.
            let closed = socket.state() == tcp::State::Closed;
            if conn.peer.is_none() && !socket.is_listening() {
                let since = *conn.since.get_or_insert(now);
                let room = remote(socket)
                    .is_some_and(|ip| open.iter().filter(|o| **o == ip).count() < PER_PEER);
                match socket.state() {
                    // Made, or made and already finished sending: the server answers it.
                    tcp::State::Established | tcp::State::CloseWait => {
                        conn.peer = accept(socket, &open, &self.accepted, &self.forget.wake);
                        conn.since = None;
                    }
                    tcp::State::SynReceived if room && now - since < HANDSHAKE.into() => {}
                    _ => socket.abort(),
                }
                if socket.state() != tcp::State::Closed
                    && let Some(ip) = remote(socket)
                {
                    open.push(ip);
                }
            }
            if let Some((_, ends)) = &mut conn.peer {
                pump(socket, ends);
                if ends.closed && now - *conn.since.get_or_insert(now) >= CLOSING.into() {
                    socket.abort();
                }
            }
            if closed {
                self.sockets.remove(conn.handle);
            } else {
                kept.push(conn);
            }
        }
        self.conns = kept;
    }
}

/// A connection's ends, once it's handed to the HTTP server through `accepted`; None when its
/// peer has as many `open` as it may, or the server's behind, and it's refused.
fn accept(
    socket: &mut tcp::Socket,
    open: &[Ipv4Addr],
    accepted: &mpsc::Sender<(Stream, SocketAddr)>,
    wake: &Arc<Notify>,
) -> Option<(Ipv4Addr, Ends)> {
    let (Some(ip), Some(endpoint)) = (remote(socket), socket.remote_endpoint()) else {
        socket.abort();
        return None;
    };
    if open.iter().filter(|o| **o == ip).count() >= PER_PEER {
        socket.abort();
        return None;
    }
    let (stream, ends) = link::pair(wake.clone(), IDLE_LIMIT);
    let from = SocketAddr::new(ip.into(), endpoint.port);
    if accepted.try_send((stream, from)).is_err() {
        socket.abort();
        return None;
    }
    Some((ip, ends))
}

/// The peer's tunnel address, once a socket has one.
fn remote(socket: &tcp::Socket) -> Option<Ipv4Addr> {
    let IpAddress::Ipv4(ip) = socket.remote_endpoint()?.addr;
    Some(ip)
}

/// Moves what's arrived to the HTTP server, and what it sent to the socket, as far as each
/// side has room.
fn pump(socket: &mut tcp::Socket, ends: &mut Ends) {
    to_server(socket, ends);
    from_server(socket, ends);
}

fn to_server(socket: &mut tcp::Socket, ends: &mut Ends) {
    let Some(sender) = &ends.incoming else {
        return;
    };
    let mut server_gone = false;
    while socket.can_recv() {
        let permit = match sender.try_reserve() {
            Ok(permit) => permit,
            Err(TrySendError::Full(())) => return,
            Err(TrySendError::Closed(())) => {
                server_gone = true;
                break;
            }
        };
        let chunk = socket.recv(|buf| {
            let n = buf.len().min(CHUNK);
            (n, bytes::Bytes::copy_from_slice(&buf[..n]))
        });
        match chunk {
            Ok(chunk) if !chunk.is_empty() => permit.send(chunk),
            _ => break,
        }
    }
    // The peer has sent all it will: the server reads to the end.
    if server_gone || (!socket.may_recv() && !socket.can_recv()) {
        ends.incoming = None;
    }
}

/// Sends what the server wrote, and closes the socket for sending once it's done.
fn from_server(socket: &mut tcp::Socket, ends: &mut Ends) {
    while !ends.closed {
        let chunk = match ends.pending.take() {
            Some(chunk) => chunk,
            None => match ends.outgoing.try_recv() {
                Ok(chunk) => chunk,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => return finish(socket, ends),
            },
        };
        let sent = match socket.can_send() {
            true => socket.send_slice(&chunk).unwrap_or(0),
            false => 0,
        };
        if sent < chunk.len() {
            ends.pending = Some(chunk.slice(sent..));
            return;
        }
    }
}

/// The server's done with the connection: closed for sending, or dropped when it gave up on it
/// as idle, as what's unsent would hold it open.
fn finish(socket: &mut tcp::Socket, ends: &mut Ends) {
    if ends.timed_out.load(Ordering::SeqCst) {
        socket.abort();
    } else {
        socket.close();
    }
    ends.closed = true;
}

#[cfg(test)]
mod tests;
