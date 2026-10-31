//! How far a download's copy home has got, as the downloads list and the progress stream show it.

use serde::Serialize;

/// How far a download being copied home has got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Copying {
    /// Bytes copied home so far, counting what an earlier copy that was cut off left.
    pub bytes: i64,
    /// Bytes to copy in all.
    pub total: i64,
    /// The speed of this copy so far, once it has run a second; None before.
    pub bytes_per_second: Option<i64>,
    /// Seconds left at that speed; None without one.
    pub seconds_left: Option<i64>,
}

impl Copying {
    /// What the transfer wrote of a download (`copied`, `copy_total`, `copy_rate`), while it's
    /// being copied home (`transferring`, not yet tagging an album, and not yet checked
    /// against its title); None otherwise.
    pub fn of(
        state: &str,
        message: Option<&str>,
        written: (Option<i64>, Option<i64>, Option<i64>),
    ) -> Option<Copying> {
        use crate::progress::{Step, download_step};
        if state != "transferring" || download_step(state, message) != Step::Transferring {
            return None;
        }
        // Copied already, and being checked before anything is placed.
        if message == Some(crate::transfer::CHECKING) {
            return None;
        }
        let (Some(bytes), Some(total), rate) = written else {
            return None;
        };
        let rate = rate.filter(|r| *r > 0);
        Some(Copying {
            bytes,
            total,
            bytes_per_second: rate,
            seconds_left: rate.map(|r| ((total - bytes).max(0) + r - 1) / r),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_shown_while_the_copied_files_are_checked() {
        let written = (Some(4_000), Some(4_000), Some(1_000));
        assert_eq!(
            Copying::of("transferring", Some(crate::transfer::CHECKING), written),
            None
        );
        assert!(Copying::of("transferring", Some("copying 1 of 1: a.mkv"), written).is_some());
    }

    #[test]
    fn shown_only_while_copying_home_with_the_time_left_rounded_up() {
        let written = (Some(1_000), Some(4_000), Some(1_000));
        assert_eq!(
            Copying::of("transferring", Some("copying 1 of 1: a.mkv"), written),
            Some(Copying {
                bytes: 1_000,
                total: 4_000,
                bytes_per_second: Some(1_000),
                seconds_left: Some(3)
            })
        );
        assert_eq!(
            Copying::of("transferring", None, (Some(1_000), Some(4_000), Some(999)))
                .and_then(|c| c.seconds_left),
            Some(4)
        );
        // No speed yet: no time left.
        assert_eq!(
            Copying::of("transferring", None, (Some(0), Some(4_000), None))
                .and_then(|c| c.seconds_left),
            None
        );
        // Tagging an album, waiting, imported, or nothing written: none.
        assert_eq!(
            Copying::of("transferring", Some("measuring and tagging"), written),
            None
        );
        assert_eq!(Copying::of("completed", None, written), None);
        assert_eq!(Copying::of("imported", None, written), None);
        assert_eq!(Copying::of("transferring", None, (None, None, None)), None);
    }
}
