//! How much room the file systems Melete keeps things on have: each library's, the data
//! folder's and the transcode folder's.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use serde::Serialize;

/// A file system's size and what's free on it, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Space {
    /// Its whole size.
    pub total: u64,
    /// What an unprivileged user may still write.
    pub free: u64,
}

/// Below this share free, a disk is nearly full.
pub const NEARLY_FULL_PERCENT: u64 = 5;

impl Space {
    /// Whether less than [`NEARLY_FULL_PERCENT`] of it is free.
    pub fn nearly_full(self) -> bool {
        self.total > 0 && self.free.saturating_mul(100) < self.total * NEARLY_FULL_PERCENT
    }
}

/// The size of the file system `path` is on, and what's free there.
pub fn space(path: &Path) -> std::io::Result<Space> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "a NUL in the path"))?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: the path is a valid C string, and statvfs only writes into the struct it's given.
    let answered = unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) };
    if answered != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: statvfs succeeded, so it filled the struct in.
    let stats = unsafe { stats.assume_init() };
    // The fields are u64 here but narrower on some platforms, so the casts stay.
    #[allow(clippy::unnecessary_cast)]
    let (block, blocks, available) = (
        stats.f_frsize as u64,
        stats.f_blocks as u64,
        stats.f_bavail as u64,
    );
    Ok(Space {
        total: blocks * block,
        free: available * block,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_the_file_system_a_folder_is_on() {
        let folder = tempfile::tempdir().unwrap();
        let measured = space(folder.path()).unwrap();
        assert!(
            measured.total > 0 && measured.free <= measured.total,
            "{measured:?}"
        );
        assert!(space(&folder.path().join("not-there")).is_err());
    }

    #[test]
    fn nearly_full_is_under_five_percent_free() {
        let of = |free| Space { total: 1000, free };
        assert!(of(49).nearly_full());
        assert!(!of(50).nearly_full());
        assert!(!Space { total: 0, free: 0 }.nearly_full());
    }
}
