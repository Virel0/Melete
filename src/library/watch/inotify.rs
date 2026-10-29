//! Linux's inotify, through libc: watching a folder and reading what changed in it.

use std::fs::File;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// What a folder is watched for: a write finished, a file or folder moved in or out, made or
/// deleted. Folders only, and links aren't followed.
const MASK: u32 = libc::IN_CLOSE_WRITE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_ONLYDIR
    | libc::IN_DONT_FOLLOW;

/// An inotify instance, read without blocking.
pub(super) struct Inotify {
    /// Its file descriptor, to read events from; closed when dropped.
    pub(super) file: File,
    /// Whether the system's limit on watches has been reached.
    pub(super) full: bool,
}

impl Inotify {
    /// A new instance.
    pub(super) fn new() -> io::Result<Self> {
        // SAFETY: inotify_init1 takes only flags and returns a new descriptor or -1.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the descriptor was just made and nothing else owns it.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        Ok(Self {
            file: File::from(owned),
            full: false,
        })
    }

    /// Watches the folder at `path`; its watch descriptor.
    pub(super) fn add(&mut self, path: &Path) -> io::Result<i32> {
        let path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let fd = std::os::fd::AsRawFd::as_raw_fd(&self.file);
        // SAFETY: the descriptor is this instance's, open while it lives; the path is a valid
        // C string for the length of the call.
        let wd = unsafe { libc::inotify_add_watch(fd, path.as_ptr(), MASK) };
        if wd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(wd)
    }
}

/// One event: the watch it's for, what happened, and the name in the folder it concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Event {
    /// The watch descriptor of the folder.
    pub(super) wd: i32,
    /// What happened (`IN_*` bits).
    pub(super) mask: u32,
    /// The name in the folder; empty for an event about the folder itself.
    pub(super) name: String,
}

/// The events in `bytes`, as read from the descriptor: each a header (watch, mask, cookie and
/// the name's length) then the name, padded with NULs. Names that aren't UTF-8 are left out.
pub(super) fn events(bytes: &[u8]) -> Vec<Event> {
    const HEADER: usize = 16;
    let word = |at: usize| -> [u8; 4] {
        let mut word = [0u8; 4];
        word.copy_from_slice(&bytes[at..at + 4]);
        word
    };
    let mut found = Vec::new();
    let mut at = 0;
    while at + HEADER <= bytes.len() {
        let wd = i32::from_ne_bytes(word(at));
        let mask = u32::from_ne_bytes(word(at + 4));
        let len = u32::from_ne_bytes(word(at + 12)) as usize;
        let end = (at + HEADER + len).min(bytes.len());
        let raw = &bytes[at + HEADER..end];
        let raw = &raw[..raw.iter().position(|b| *b == 0).unwrap_or(raw.len())];
        if let Ok(name) = std::str::from_utf8(raw) {
            found.push(Event {
                wd,
                mask,
                name: name.to_owned(),
            });
        }
        at = end;
    }
    found
}

/// A folder made or moved in, to watch too.
pub(super) fn is_new_folder(event: &Event) -> bool {
    event.mask & libc::IN_ISDIR != 0 && event.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0
}

/// Something moved in or out.
pub(super) fn is_move(event: &Event) -> bool {
    event.mask & (libc::IN_MOVED_FROM | libc::IN_MOVED_TO) != 0
}

/// Whether the event changes what a scan finds: a write finished, a move, a deletion, or a
/// new folder. A file just made is still being written; its finished write counts instead.
pub(super) fn counts(event: &Event) -> bool {
    let folder = event.mask & libc::IN_ISDIR != 0;
    event.mask & (libc::IN_CLOSE_WRITE | libc::IN_MOVED_FROM | libc::IN_MOVED_TO | libc::IN_DELETE)
        != 0
        || (folder && event.mask & libc::IN_CREATE != 0)
}
