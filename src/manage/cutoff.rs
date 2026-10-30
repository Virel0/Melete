//! Cutoff unmet in the managed lists: whether a monitored wanted title's file is below its
//! profile's cutoff. Judged on the files an upgrade would be compared with (those of the folder
//! linked to the wanted entry), with the same choice of best file and the same profile rule as
//! searches and RSS, so the list and the upgrades never disagree.

use std::collections::HashMap;

use super::*;
use crate::profiles::{self, App, Loaded};
use crate::search::{OnDisk, Read};

/// The quality profiles titles are decided with, each loaded once per list.
pub(crate) struct Profiles {
    by_name: HashMap<(String, String), i64>,
    services: HashMap<i64, String>,
    loaded: HashMap<i64, Option<Loaded>>,
}

impl Profiles {
    /// The profiles of `app`'s kind of title, by their service and name.
    pub(crate) async fn new(pool: &SqlitePool, app: App) -> anyhow::Result<Self> {
        let rows: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT id, service, name FROM quality_profiles WHERE source = ?")
                .bind(app.as_str())
                .fetch_all(pool)
                .await?;
        Ok(Profiles {
            by_name: rows
                .iter()
                .map(|(id, service, name)| ((service.clone(), name.clone()), *id))
                .collect(),
            services: rows
                .into_iter()
                .map(|(id, service, _)| (id, service))
                .collect(),
            loaded: HashMap::new(),
        })
    }

    /// The profile an arr's name means for `service`, as searches read it: the service's own,
    /// else for anime Series' (Sonarr shares its profiles).
    fn named(&self, service: &str, name: &str) -> Option<i64> {
        let of = |service: &str| self.by_name.get(&(service.into(), name.into())).copied();
        of(service).or_else(|| (service == "anime").then(|| of("series")).flatten())
    }

    /// The profile a wanted title of `service` is decided with, chosen as searches choose it:
    /// one chosen in Melete for its service, else the one its arr named. None when neither is
    /// here, or its custom formats don't load (searches would refuse it too).
    pub(crate) async fn get(
        &mut self,
        pool: &SqlitePool,
        service: &str,
        (id, name): (Option<i64>, Option<&str>),
    ) -> anyhow::Result<Option<&Loaded>> {
        let chosen = id
            .filter(|id| self.services.get(id).is_some_and(|s| s == service))
            .or_else(|| name.and_then(|n| self.named(service, n)));
        let Some(id) = chosen else {
            return Ok(None);
        };
        if let std::collections::hash_map::Entry::Vacant(entry) = self.loaded.entry(id) {
            entry.insert(profiles::load(pool, id).await.unwrap_or_else(|err| {
                tracing::warn!(
                    profile = id,
                    error = format!("{err:#}"),
                    "a profile won't load"
                );
                None
            }));
        }
        Ok(self.loaded.get(&id).and_then(Option::as_ref))
    }
}

/// A file's quality read from its name, or from its probe when the name doesn't say (asked
/// for only then); None when neither does.
pub(crate) async fn read(pool: &SqlitePool, id: i64, path: &str) -> anyhow::Result<Option<Read>> {
    if let Ok(file) = Read::file(path, None) {
        return Ok(Some(file));
    }
    let probe: Option<String> = sqlx::query_scalar(
        "SELECT p.info FROM media_probes p JOIN media_files f ON f.id = p.file_id
         WHERE p.file_id = ? AND p.size = f.size AND p.modified_at = f.modified_at",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .flatten();
    Ok(Read::file(path, probe.as_deref()).ok())
}

/// A present episode file: its id, path, season, and the episodes it holds.
pub(crate) struct EpisodeFile {
    pub(crate) id: i64,
    pub(crate) path: String,
    pub(crate) season: i64,
    pub(crate) first: i64,
    pub(crate) last: i64,
}

impl EpisodeFile {
    /// The episodes it holds (a file claiming more than a thousand is cut short).
    pub(crate) fn episodes(&self) -> std::ops::RangeInclusive<i64> {
        self.first..=self.last.min(self.first + 1000)
    }
}

/// The monitored episodes whose files are below the profile's cutoff, each judged on every file
/// holding it, as that episode's upgrade search judges it; and whether any was judged at all.
pub(crate) async fn episodes_unmet(
    pool: &SqlitePool,
    profile: &Loaded,
    files: &[&EpisodeFile],
    monitored: impl Fn(i64, i64) -> bool,
    original: Option<&str>,
) -> anyhow::Result<(bool, Vec<(i64, i64)>)> {
    let mut holding: std::collections::BTreeMap<(i64, i64), Vec<usize>> = Default::default();
    let mut read = Vec::with_capacity(files.len());
    for file in files {
        let mut wanted = false;
        for episode in file.episodes().filter(|e| monitored(file.season, *e)) {
            holding
                .entry((file.season, episode))
                .or_default()
                .push(read.len());
            wanted = true;
        }
        // Only files holding a monitored episode are read.
        read.push(match wanted {
            true => self::read(pool, file.id, &file.path).await?,
            false => None,
        });
    }
    let mut judged = false;
    let mut below = Vec::new();
    for (episode, held_by) in holding {
        let verdict = unmet(profile, held_by.iter().map(|i| read[*i].as_ref()), original);
        judged |= verdict.is_some();
        if verdict == Some(true) {
            below.push(episode);
        }
    }
    Ok((judged, below))
}

/// Whether the best of these files is below the profile's cutoff; None when there are none or
/// one can't be judged.
pub(crate) fn unmet<'a>(
    profile: &Loaded,
    files: impl IntoIterator<Item = Option<&'a Read>>,
    original: Option<&str>,
) -> Option<bool> {
    let files: Option<Vec<&Read>> = files.into_iter().collect();
    match crate::search::best_read(profile, files?, original) {
        OnDisk::File(existing) => Some(profile.cutoff_unmet(&existing)),
        OnDisk::Nothing | OnDisk::Unclear(_) => None,
    }
}
