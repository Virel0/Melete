//! What AI picks may spend: each call costs money on the household's OpenRouter account, so
//! each user may start at most [`STARTS_PER_HOUR`] of them in an hour (as many as AI searches),
//! retries after a failure included, and the server runs at most [`AT_ONCE`] at a time. Kept
//! by the API across its being set up afresh, so saving a setting forgives nothing.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::*;

/// The most AI picks calls one user may start in an hour.
pub const STARTS_PER_HOUR: usize = 30;
/// The most AI picks calls running at once, server-wide.
pub const AT_ONCE: usize = 2;

/// Why an AI picks call wasn't started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// [`AT_ONCE`] calls are running already.
    Busy,
    /// The user started [`STARTS_PER_HOUR`] in the last hour.
    Limited,
}

/// The calls each user started in the last hour, and the calls running now.
pub struct Budget {
    started: Mutex<HashMap<i64, VecDeque<Instant>>>,
    running: Arc<Semaphore>,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            started: Mutex::default(),
            running: Arc::new(Semaphore::new(AT_ONCE)),
        }
    }
}

impl Budget {
    /// A place for one more call for `user`, held until it's dropped; refused when the server
    /// is running as many as it may or the user started as many as they may this hour. Nothing
    /// is counted until [`Budget::started`] says the call was made.
    pub fn place(&self, user: i64) -> Result<OwnedSemaphorePermit, Refusal> {
        let permit = self
            .running
            .clone()
            .try_acquire_owned()
            .map_err(|_| Refusal::Busy)?;
        let mut started = self.started.lock().expect("no holder panics");
        let hour = Duration::from_secs(3600);
        let now = Instant::now();
        started.retain(|_, times| times.back().is_some_and(|t| now.duration_since(*t) < hour));
        let times = started.entry(user).or_default();
        while times
            .front()
            .is_some_and(|t| now.duration_since(*t) >= hour)
        {
            times.pop_front();
        }
        if times.len() >= STARTS_PER_HOUR {
            return Err(Refusal::Limited);
        }
        Ok(permit)
    }

    /// Counts a call started for `user`.
    pub fn started(&self, user: i64) {
        let mut started = self.started.lock().expect("no holder panics");
        started.entry(user).or_default().push_back(Instant::now());
    }
}
