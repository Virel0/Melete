//! The aoide-sidecar, rewritten in Rust as part of Melete: what Aoide's apps need from a
//! server beyond the library, under its `/aoide/*` contract.

pub mod automix;
pub mod discover;
pub mod fractional_index;
pub mod import;
pub mod library;
pub mod likes;
pub mod listening;
pub mod matcher;
pub mod next;
pub mod pending;
pub mod planner;
pub mod playlist_edits;
pub mod playlists;
pub mod smart;
pub mod sync;
pub mod taste;
pub mod track_flags;
pub mod writing;
