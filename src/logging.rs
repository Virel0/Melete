//! The log: what it shows by default, and the tag reader's warnings, kept out of it but noted
//! for the scan that caused them.
//!
//! lofty, which reads music tags, warns once per file for common, harmless cases (an ID3 tag
//! inside a FLAC file, a frame of an older ID3 version). On a large music library that's
//! thousands of lines a scan, so the default filter leaves them out, and a scan sums them up in
//! one line instead (`library::music`). `RUST_LOG` still sets the filter whole.

use std::cell::RefCell;

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::filter::{EnvFilter, Targets};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

/// The filter when `RUST_LOG` isn't set: Melete's own messages from info up, and only errors
/// from crates known to warn about every other file.
pub const DEFAULT_FILTER: &str = "info,lofty=error";

/// Starts the log: `RUST_LOG`'s filter, else [`DEFAULT_FILTER`]. Messages the `log` crate
/// carries (lofty's among them) come through as well.
pub fn init() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    subscriber(filter, std::io::stdout).init();
}

/// The log written to `writer` through `filter`, with the tag reader's warnings noted for
/// [`noting`] whatever the filter lets through.
pub fn subscriber<W>(filter: EnvFilter, writer: W) -> impl Subscriber + Send + Sync
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_filter(filter),
        )
        .with(TagWarnings.with_filter(Targets::new().with_target("lofty", Level::WARN)))
}

thread_local! {
    /// The tag reader's warnings on this thread while [`noting`] runs.
    static NOTED: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// Runs `read`, returning what it gave and the tag reader's warnings on this thread meanwhile,
/// in order. lofty reads on the caller's thread, so these are the file's own.
pub fn noting<T>(read: impl FnOnce() -> T) -> (T, Vec<String>) {
    NOTED.with(|noted| *noted.borrow_mut() = Some(Vec::new()));
    let result = read();
    let warnings = NOTED.with(|noted| noted.borrow_mut().take().unwrap_or_default());
    (result, warnings)
}

/// Notes each of the tag reader's warnings for [`noting`].
struct TagWarnings;

impl<S: Subscriber> Layer<S> for TagWarnings {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        NOTED.with(|noted| {
            if let Some(warnings) = noted.borrow_mut().as_mut() {
                let mut message = Message(String::new());
                event.record(&mut message);
                warnings.push(message.0);
            }
        });
    }
}

/// An event's message.
struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

#[cfg(test)]
pub mod testing {
    use super::*;

    /// Sends the `log` crate's messages through a subscriber that notes the tag reader's
    /// warnings, once for the test binary, as [`init`] does for the server.
    pub fn note_tag_warnings() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            // Another test may have set a subscriber first; then this one can't be set.
            let _ = tracing_subscriber::registry()
                .with(TagWarnings.with_filter(Targets::new().with_target("lofty", Level::WARN)))
                .try_init();
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// A writer the test can read back.
    #[derive(Clone, Default)]
    struct Written(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Written {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("not poisoned")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'w> MakeWriter<'w> for Written {
        type Writer = Written;
        fn make_writer(&'w self) -> Written {
            self.clone()
        }
    }

    #[test]
    fn the_default_log_leaves_out_the_tag_readers_warnings_but_notes_them() {
        let written = Written::default();
        let log = subscriber(EnvFilter::new(DEFAULT_FILTER), written.clone());
        let ((), noted) = tracing::subscriber::with_default(log, || {
            noting(|| {
                tracing::warn!(target: "lofty::flac::read", "an ID3v2 tag in a FLAC file");
                tracing::error!(target: "lofty::probe", "a file lofty can't read");
                tracing::info!(target: "melete::background", "scan finished");
                tracing::debug!(target: "melete::background", "a detail");
            })
        });
        let written = String::from_utf8(written.0.lock().expect("not poisoned").clone())
            .expect("the log is text");
        assert!(written.contains("scan finished"), "{written}");
        assert!(written.contains("a file lofty can't read"), "{written}");
        assert!(!written.contains("an ID3v2 tag"), "{written}");
        assert!(!written.contains("a detail"), "{written}");
        assert_eq!(
            noted,
            ["an ID3v2 tag in a FLAC file", "a file lofty can't read"],
            "noted whether or not the log shows them"
        );
    }

    #[test]
    fn rust_log_still_sets_the_filter_whole() {
        let written = Written::default();
        let log = subscriber(EnvFilter::new("info"), written.clone());
        tracing::subscriber::with_default(log, || {
            tracing::warn!(target: "lofty::flac::read", "an ID3v2 tag in a FLAC file");
        });
        let written = written.0.lock().expect("not poisoned").clone();
        assert!(String::from_utf8_lossy(&written).contains("an ID3v2 tag"));
    }

    #[test]
    fn nothing_is_noted_outside_noting() {
        let log = subscriber(EnvFilter::new("off"), Written::default());
        let ((), noted) = tracing::subscriber::with_default(log, || {
            tracing::warn!(target: "lofty::flac::read", "before");
            noting(|| ())
        });
        assert!(noted.is_empty());
    }
}
