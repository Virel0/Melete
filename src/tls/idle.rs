//! A connection closed once nothing has moved either way for a while: a request body that
//! never comes, or a response its client never reads, would otherwise hold its place for good.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Instant, Sleep};

/// `inner`, ended with a timed-out error once no byte has been read or written for `limit`.
pub struct Idle<T> {
    inner: T,
    limit: Duration,
    deadline: Pin<Box<Sleep>>,
}

impl<T> Idle<T> {
    pub fn new(inner: T, limit: Duration) -> Self {
        Idle {
            inner,
            limit,
            deadline: Box::pin(tokio::time::sleep(limit)),
        }
    }

    fn moved(&mut self) {
        self.deadline.as_mut().reset(Instant::now() + self.limit);
    }

    /// Waiting on `inner`: an error once the deadline has passed, else still waiting.
    fn waited<R>(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<R>> {
        ready!(self.deadline.as_mut().poll(cx));
        Poll::Ready(Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "nothing moved on the connection for too long",
        )))
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Idle<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(done) => {
                this.moved();
                Poll::Ready(done)
            }
            Poll::Pending => this.waited(cx),
        }
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Idle<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_write(cx, buf) {
            Poll::Ready(done) => {
                this.moved();
                Poll::Ready(done)
            }
            Poll::Pending => this.waited(cx),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_flush(cx) {
            Poll::Ready(done) => Poll::Ready(done),
            Poll::Pending => this.waited(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[tokio::test]
    async fn a_connection_where_nothing_moves_is_ended_and_one_moving_isnt() {
        let (near, mut far) = tokio::io::duplex(64);
        let mut idle = Idle::new(near, Duration::from_millis(100));
        // Bytes keep coming: each read is in time, however long it all takes.
        for _ in 0..4 {
            far.write_all(b"x").await.unwrap();
            let mut one = [0u8; 1];
            idle.read_exact(&mut one).await.unwrap();
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
        // Then nothing: the read gives up.
        let mut one = [0u8; 1];
        let err = idle.read_exact(&mut one).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        // A write nobody reads (the pipe full) gives up the same way.
        let (near, _far) = tokio::io::duplex(8);
        let mut idle = Idle::new(near, Duration::from_millis(100));
        let err = idle.write_all(&[0u8; 64]).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }
}
