//! How far a swap for a smaller copy may go, and why a release goes too far.

use super::*;

/// How far a swap for a smaller copy may go: at most this many lines high and this many bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shrink {
    pub max_height: i64,
    pub max_bytes: i64,
}

impl Shrink {
    /// Why a release is too large for the swap, if it is. One whose size isn't given, or whose
    /// name doesn't say its height, can't be known smaller, so it's turned down too.
    pub fn rejection(&self, title: &str, size: Option<u64>) -> Option<String> {
        let height = release::parse(title).resolution;
        match height {
            None => {
                return Some(
                    "its name doesn't say its height, so it can't be known smaller".into(),
                );
            }
            Some(h) if h > self.max_height => {
                return Some(format!(
                    "{h}p is above the {}p a smaller copy may be",
                    self.max_height
                ));
            }
            _ => {}
        }
        let Some(size) = size.and_then(|s| i64::try_from(s).ok()) else {
            return Some("its size isn't given, so it can't be known smaller".into());
        };
        (size > self.max_bytes).then(|| {
            format!(
                "{:.1} GB is more than the {:.1} GB a smaller copy may take",
                size as f64 / 1e9,
                self.max_bytes as f64 / 1e9
            )
        })
    }
}
