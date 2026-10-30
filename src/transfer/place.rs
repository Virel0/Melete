//! Putting a file in place without ever replacing one: a hard link, which fails rather than
//! overwrite.

use super::*;

/// Puts a finished copy in place without ever replacing what's there: a hard link fails if the
/// name is taken, and so does a rename with RENAME_NOREPLACE. Where neither works, nothing is
/// put in place.
pub(crate) fn place(partial: &Path, target: &Path) -> Result<(), Stop> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::hard_link(partial, target) {
        Ok(()) => {
            // A real link is the same file. (mergerfs can be set to fake one with a symlink
            // to the partial, which removing the partial would leave dangling.)
            let made = std::fs::symlink_metadata(target)?;
            let ours = std::fs::metadata(partial)?;
            if made.file_type().is_file() && made.ino() == ours.ino() && made.dev() == ours.dev() {
                std::fs::remove_file(partial)?;
                return Ok(());
            }
            if made.file_type().is_symlink()
                && std::fs::read_link(target).is_ok_and(|to| to == partial)
            {
                std::fs::remove_file(target)?;
                return rename_without_replacing(partial, target);
            }
            Err(Stop::Fail(format!(
                "{} isn't what was just put there",
                target.display()
            )))
        }
        Err(err) if err.kind() == ErrorKind::AlreadyExists => Err(appeared(target)),
        Err(_) => rename_without_replacing(partial, target),
    }
}

/// [`place`] for others: moves `from` to `to`, never replacing what's at `to`.
pub(crate) fn move_without_replacing(from: &Path, to: &Path) -> Result<(), String> {
    place(from, to).map_err(|stop| match stop {
        Stop::Retry(why) | Stop::Fail(why) | Stop::NotTheTitle(why) => why,
    })
}

pub(super) fn appeared(target: &Path) -> Stop {
    Stop::Fail(format!(
        "{} appeared while it was being copied; Melete doesn't replace files",
        target.display()
    ))
}

pub(super) fn rename_without_replacing(from: &Path, to: &Path) -> Result<(), Stop> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let path = |p: &Path| {
        CString::new(p.as_os_str().as_bytes())
            .map_err(|_| Stop::Fail(format!("{} has a NUL in its name", p.display())))
    };
    let (from_c, to_c) = (path(from)?, path(to)?);
    // SAFETY: both are valid NUL-terminated paths, and AT_FDCWD resolves them as given.
    let done = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from_c.as_ptr(),
            libc::AT_FDCWD,
            to_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if done == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::EEXIST) => Err(appeared(to)),
        Some(libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP) => Err(Stop::Fail(format!(
            "the library's file system can't put {} in place without risking replacing a file (no hard links, no rename without replace); Melete won't import there",
            to.display()
        ))),
        _ => Err(err.into()),
    }
}
