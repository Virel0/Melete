//! A torrent as Deluge reports it, and the fields Melete asks for.

use serde::Deserialize;

/// One torrent as Deluge sees it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Status {
    /// The torrent's name.
    pub name: String,
    /// "Downloading", "Seeding", "Paused", "Checking", "Queued", "Error"...
    pub state: String,
    /// How much is downloaded, in percent (0 to 100).
    pub progress: f64,
    /// The size of the files it downloads, in bytes.
    pub total_size: i64,
    /// The folder on the seedbox it downloads into.
    pub save_path: String,
    /// Whether every file it downloads is complete.
    pub is_finished: bool,
    /// Its files; empty for a magnet link until Deluge has fetched the metadata.
    pub files: Vec<TorrentFile>,
    /// Deluge's tracker or error message; often empty.
    pub message: String,
    /// How much it has uploaded for each byte downloaded; 0 when Deluge doesn't say.
    #[serde(default)]
    pub ratio: f64,
    /// How long it has seeded, in seconds; 0 when Deluge doesn't say.
    #[serde(default)]
    pub seeding_time: i64,
    /// Seeders it's connected to; None when Deluge doesn't say.
    #[serde(default)]
    pub num_seeds: Option<i64>,
    /// Seeders in the swarm, as its trackers count them (-1 or 0 when none or not known); None
    /// when Deluge doesn't say.
    #[serde(default)]
    pub total_seeds: Option<i64>,
    /// Whole copies among the peers it's connected to; None when Deluge doesn't say.
    #[serde(default)]
    pub distributed_copies: Option<f64>,
}

impl Status {
    /// Whether anyone has the whole of it: a seeder connected or counted by its trackers, or a
    /// whole copy among its peers. None when Deluge says none of that.
    pub fn seeded(&self) -> Option<bool> {
        if self.num_seeds.is_none()
            && self.total_seeds.is_none()
            && self.distributed_copies.is_none()
        {
            return None;
        }
        Some(
            self.num_seeds.is_some_and(|n| n > 0)
                || self.total_seeds.is_some_and(|n| n > 0)
                || self.distributed_copies.is_some_and(|c| c >= 1.0),
        )
    }
}

/// A file in a torrent.
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
pub struct TorrentFile {
    /// Relative to the save path, starting with the torrent's folder when it has one.
    pub path: String,
    /// Its size in bytes.
    pub size: i64,
}

pub(super) const STATUS_FIELDS: [&str; 13] = [
    "name",
    "state",
    "progress",
    "total_size",
    "save_path",
    "is_finished",
    "files",
    "message",
    "ratio",
    "seeding_time",
    "num_seeds",
    "total_seeds",
    "distributed_copies",
];
