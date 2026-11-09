//! Watching every connection made through the proxy: one that fails comes back as an error in
//! plain words naming the proxy (never its login, nor the failure's own text, which can carry
//! it), and whether the proxy answers is kept for admin health. A client with no proxy to go
//! through refuses every connection the same way, without making one.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use super::*;
use crate::admin_health::{Finding, Level};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Why a connection through the proxy wasn't made. Nothing was sent direct instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unreachable {
    /// The proxy itself didn't answer, at its address.
    Down(String),
    /// The proxy answered, but the connection through it failed: it turned the login down, or
    /// couldn't reach the site.
    Through(String),
    /// Set to go through the proxy, but none is set.
    NoProxy,
}

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unreachable::Down(at) => write!(
                f,
                "The proxy at {at} doesn't answer, so nothing was sent: it never goes direct"
            ),
            Unreachable::Through(at) => write!(
                f,
                "The proxy at {at} answered, but the login was turned down or the site couldn't be reached through it; nothing was sent direct"
            ),
            Unreachable::NoProxy => f.write_str(
                "This is set to go through the proxy, but no proxy is set, so nothing was sent",
            ),
        }
    }
}

impl std::error::Error for Unreachable {}

/// How the proxy has answered since its setup was put in use.
#[derive(Default)]
pub struct Status {
    /// Since when it hasn't answered (Unix seconds), and at which address; None while it does.
    down: Mutex<Option<(i64, String)>>,
    /// Failed connections so far, and the latest one's words.
    failures: AtomicU64,
    latest: Mutex<Option<Unreachable>>,
}

impl Status {
    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn connected(&self) {
        Self::lock(&self.down).take();
    }

    pub(super) fn failed(&self, why: &Unreachable) {
        if let Unreachable::Down(at) = why {
            Self::lock(&self.down).get_or_insert_with(|| (crate::db::unix_now(), at.clone()));
        } else {
            self.connected();
        }
        *Self::lock(&self.latest) = Some(why.clone());
        self.failures.fetch_add(1, Ordering::SeqCst);
    }

    /// Since when the proxy hasn't answered, and its address; None while it answers.
    pub fn down(&self) -> Option<(i64, String)> {
        Self::lock(&self.down).clone()
    }

    /// Failed connections so far, to compare with [`Status::since`] later.
    pub fn mark(&self) -> u64 {
        self.failures.load(Ordering::SeqCst)
    }

    /// The latest failure's words, when there's been one since `mark`.
    pub fn since(&self, mark: u64) -> Option<Unreachable> {
        (self.mark() > mark)
            .then(|| Self::lock(&self.latest).clone())
            .flatten()
    }
}

/// Why a request failed at the proxy, when that's what it failed at.
pub fn unreachable_in(err: &(dyn std::error::Error + 'static)) -> Option<Unreachable> {
    std::iter::successors(Some(err), |e| e.source())
        .find_map(|e| e.downcast_ref::<Unreachable>())
        .cloned()
}

/// A failed request in words safe to show: the proxy's, when it failed there, else reqwest's
/// own without the URL (which can carry a key).
pub fn said(err: reqwest::Error) -> String {
    match unreachable_in(&err) {
        Some(why) => why.to_string(),
        None => err.without_url().to_string(),
    }
}

/// Failed connections through the proxy in use so far; with [`trouble_since`], whether one
/// failed in between.
pub fn mark() -> u64 {
    current().status.mark()
}

/// The words of the latest failed connection through the proxy, when there's been one since
/// `mark`.
pub fn trouble_since(mark: u64) -> Option<String> {
    current().status.since(mark).map(|why| why.to_string())
}

/// What admin health says while the proxy in use doesn't answer: a problem, naming it.
pub fn finding() -> Option<Finding> {
    let applied = current();
    let (since, at) = applied.status.down()?;
    let minutes = (crate::db::unix_now() - since).max(0) / 60;
    Some(Finding {
        level: Level::Problem,
        code: "proxy_down",
        message: format!(
            "The proxy at {at} hasn't answered for {minutes} min: the services set to go through it fail until it's back, and none goes direct. Check it under Outside services, Proxy."
        ),
        count: 0,
        id: None,
    })
}

/// The connector layer every proxied client gets.
#[derive(Clone)]
pub(super) struct Watch {
    /// The proxy's address and where to knock to see whether it answers; None refuses all.
    proxy: Option<(String, String, u16)>,
    status: Option<Arc<Status>>,
}

impl Watch {
    pub(super) fn through(proxy: &Proxy, status: Option<Arc<Status>>) -> Watch {
        let host = proxy.host.trim_start_matches('[').trim_end_matches(']');
        Watch {
            proxy: Some((proxy.address(), host.to_owned(), proxy.port)),
            status,
        }
    }

    pub(super) fn refusing() -> Watch {
        Watch {
            proxy: None,
            status: None,
        }
    }
}

impl<S> tower::Layer<S> for Watch {
    type Service = Watched<S>;

    fn layer(&self, inner: S) -> Watched<S> {
        Watched {
            inner,
            watch: self.clone(),
        }
    }
}

#[derive(Clone)]
pub(super) struct Watched<S> {
    inner: S,
    watch: Watch,
}

impl<S, R> tower::Service<R> for Watched<S>
where
    S: tower::Service<R, Error = BoxError>,
    S::Future: Send + 'static,
    S::Response: Send + 'static,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, BoxError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        match self.watch.proxy {
            Some(_) => self.inner.poll_ready(cx),
            None => Poll::Ready(Ok(())),
        }
    }

    fn call(&mut self, request: R) -> Self::Future {
        let Some((address, host, port)) = self.watch.proxy.clone() else {
            return Box::pin(async { Err(Box::new(Unreachable::NoProxy) as BoxError) });
        };
        let status = self.watch.status.clone();
        let connecting = self.inner.call(request);
        Box::pin(async move {
            match connecting.await {
                Ok(connection) => {
                    if let Some(status) = &status {
                        status.connected();
                    }
                    Ok(connection)
                }
                Err(_) => {
                    // The failure's own words aren't passed on: they can carry the login.
                    let why = match answers(&host, port).await {
                        true => Unreachable::Through(address),
                        false => Unreachable::Down(address),
                    };
                    if let Some(status) = &status {
                        status.failed(&why);
                    }
                    tracing::warn!("{why}");
                    Err(Box::new(why) as BoxError)
                }
            }
        })
    }
}

/// Whether anything takes a connection at the proxy's address, within 5 seconds.
pub(super) async fn answers(host: &str, port: u16) -> bool {
    let knock = tokio::net::TcpStream::connect((host, port));
    matches!(
        tokio::time::timeout(Duration::from_secs(5), knock).await,
        Ok(Ok(_))
    )
}
