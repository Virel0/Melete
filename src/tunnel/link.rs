//! What joins the tunnel's parts: WireGuard hands decrypted packets to the network stack and
//! takes its replies ([`ToStack`], [`FromStack`]), and the stack hands each connection to the
//! HTTP server as a [`Stream`] through a [`Listener`]. Every queue is bounded; a full one drops
//! packets, as a busy network card would, and TCP sends them again.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
use gotatun::packet::{Ip, Packet, PacketBufPool};
use gotatun::tun::{IpRecv, IpSend, MtuWatcher};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{Notify, mpsc};
use tokio::time::{Instant, Sleep};
use tokio_util::sync::PollSender;

/// The most a connection's chunk holds, either way.
pub const CHUNK: usize = 16 * 1024;

/// Packets WireGuard decrypted, on their way to the network stack.
pub struct ToStack(pub mpsc::Sender<Vec<u8>>);

impl IpSend for ToStack {
    async fn send(&mut self, packet: Packet<Ip>) -> io::Result<()> {
        // Dropped when the stack is behind.
        let _ = self.0.try_send(packet.into_bytes().as_ref().to_vec());
        Ok(())
    }
}

/// The network stack's packets, on their way to WireGuard to be sent to their peer.
pub struct FromStack {
    pub packets: mpsc::Receiver<Vec<u8>>,
    pub mtu: u16,
}

impl IpRecv for FromStack {
    async fn recv<'a>(
        &'a mut self,
        _pool: &mut PacketBufPool,
    ) -> io::Result<impl Iterator<Item = Packet<Ip>> + Send + 'a> {
        loop {
            let Some(bytes) = self.packets.recv().await else {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the tunnel's network stack stopped",
                ));
            };
            if let Ok(packet) = Packet::from_bytes(BytesMut::from(&bytes[..])).try_into_ip() {
                return Ok(std::iter::once(packet));
            }
        }
    }

    fn mtu(&self) -> MtuWatcher {
        MtuWatcher::new(self.mtu)
    }
}

/// One connection through the tunnel, as the HTTP server reads and writes it. Once no byte has
/// been read or written for its idle limit, reading or writing fails as timed out, so a peer
/// that sends a request body slowly, or never reads its answer, doesn't hold the connection.
pub struct Stream {
    incoming: mpsc::Receiver<Bytes>,
    unread: Bytes,
    outgoing: PollSender<Bytes>,
    /// Wakes the network stack when there's room or something to send.
    wake: Arc<Notify>,
    idle: Duration,
    deadline: Pin<Box<Sleep>>,
    timed_out: Arc<AtomicBool>,
}

/// The network stack's side of a [`Stream`].
pub struct Ends {
    /// What came from the peer; None once they've sent all they will.
    pub incoming: Option<mpsc::Sender<Bytes>>,
    /// What the server sends back; closed once it's done.
    pub outgoing: mpsc::Receiver<Bytes>,
    /// Part of a chunk the socket hadn't room for.
    pub pending: Option<Bytes>,
    /// Whether the socket's been closed for sending.
    pub closed: bool,
    /// Whether the server gave up on the connection as idle: it's dropped, not closed.
    pub timed_out: Arc<AtomicBool>,
}

/// A connection's two sides, waking the network stack through `wake`, ended once nothing has
/// moved for `idle`.
pub fn pair(wake: Arc<Notify>, idle: Duration) -> (Stream, Ends) {
    let (to_server, incoming) = mpsc::channel(4);
    let (outgoing, from_server) = mpsc::channel(4);
    let timed_out = Arc::new(AtomicBool::new(false));
    let stream = Stream {
        incoming,
        unread: Bytes::new(),
        outgoing: PollSender::new(outgoing),
        wake,
        idle,
        deadline: Box::pin(tokio::time::sleep(idle)),
        timed_out: timed_out.clone(),
    };
    let ends = Ends {
        incoming: Some(to_server),
        outgoing: from_server,
        pending: None,
        closed: false,
        timed_out,
    };
    (stream, ends)
}

impl Stream {
    fn moved(&mut self) {
        self.deadline.as_mut().reset(Instant::now() + self.idle);
    }

    /// Waiting on the peer or the stack: an error once the idle limit has passed.
    fn waited<R>(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<R>> {
        ready!(self.deadline.as_mut().poll(cx));
        self.timed_out.store(true, Ordering::SeqCst);
        self.wake.notify_one();
        Poll::Ready(Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "nothing moved on the tunnel connection for too long",
        )))
    }

    fn read_now(&mut self, cx: &mut Context<'_>, out: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if self.unread.is_empty() {
            match ready!(self.incoming.poll_recv(cx)) {
                Some(chunk) => {
                    self.unread = chunk;
                    // There's room again for what the socket holds.
                    self.wake.notify_one();
                }
                None => return Poll::Ready(Ok(())),
            }
        }
        let n = out.remaining().min(self.unread.len());
        out.put_slice(&self.unread[..n]);
        self.unread.advance(n);
        Poll::Ready(Ok(()))
    }

    fn write_now(&mut self, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        let gone = || io::Error::from(io::ErrorKind::BrokenPipe);
        ready!(self.outgoing.poll_reserve(cx)).map_err(|_| gone())?;
        let n = data.len().min(CHUNK);
        self.outgoing
            .send_item(Bytes::copy_from_slice(&data[..n]))
            .map_err(|_| gone())?;
        self.wake.notify_one();
        Poll::Ready(Ok(n))
    }
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match this.read_now(cx, out) {
            Poll::Ready(done) => {
                this.moved();
                Poll::Ready(done)
            }
            Poll::Pending => this.waited(cx),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match this.write_now(cx, data) {
            Poll::Ready(done) => {
                this.moved();
                Poll::Ready(done)
            }
            Poll::Pending => this.waited(cx),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.outgoing.close();
        self.wake.notify_one();
        Poll::Ready(Ok(()))
    }
}

/// The connections the network stack accepted, for `axum::serve`, each with the peer's tunnel
/// address.
pub struct Listener {
    pub accepted: mpsc::Receiver<(Stream, SocketAddr)>,
    pub local: SocketAddr,
}

impl axum::serve::Listener for Listener {
    type Io = Stream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Stream, SocketAddr) {
        match self.accepted.recv().await {
            Some(accepted) => accepted,
            // The stack stopped: nothing more comes.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local)
    }
}
