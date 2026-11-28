//! Live playback first: while a conversion runs for someone watching, background media work
//! (seek-bar previews, intros and credits, trailers' sound, Make room's compressing) starts no
//! new item, so the viewer's conversion has the GPU and the CPU to itself. What already runs
//! finishes.

use std::sync::{OnceLock, Weak};

use super::*;

/// The server's transcoder, which background work gives way to; tests make transcoders of
/// their own and never set it, so their background work never waits.
static SERVED: OnceLock<Weak<Transcoder>> = OnceLock::new();

/// How often waiting background work looks again.
const LOOK_AGAIN: Duration = Duration::from_secs(5);

impl Transcoder {
    /// Makes this the transcoder background work gives way to; the server's, set once.
    pub fn put_viewers_first(self: &Arc<Self>) {
        let _ = SERVED.set(Arc::downgrade(self));
    }
}

/// Whether a conversion runs for someone watching now.
fn viewers_converting() -> bool {
    SERVED
        .get()
        .and_then(Weak::upgrade)
        .is_some_and(|t| t.busy())
}

/// Waits until no conversion runs for someone watching: background media work calls it before
/// starting each item.
pub async fn after_viewers() {
    while viewers_converting() {
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}
