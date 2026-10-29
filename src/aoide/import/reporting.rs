//! What was found and what would be, or was, written, for the person running the import.

use super::*;

/// What was found and what would be (or was) written, for the person running it.
pub fn report(plan: &Plan) -> String {
    let mut out = String::from("Users in the sidecar's data:\n");
    for user in &plan.users {
        let mapping = match &user.mapping {
            Mapping::To(_, name) => format!("-> {name}"),
            Mapping::Skipped => "skipped".to_owned(),
            Mapping::Unmapped => "NOT MAPPED".to_owned(),
        };
        let devices = if user.device_names.is_empty() {
            String::new()
        } else {
            format!(" (devices: {})", user.device_names.join(", "))
        };
        out += &format!(
            "  {}  {} ops{devices}  {mapping}\n",
            user.jellyfin_id, user.ops
        );
    }
    out += "Ops to import:\n";
    for (entity, count) in &plan.by_entity {
        out += &format!("  {entity}: {count}\n");
    }
    out += &format!(
        "  left behind: {} queue states (only a device's latest matters)\n",
        plan.queue_states
    );
    out += &format!(
        "Playlist owners: {}, shares: {}, artwork: {}\n",
        plan.owners.len(),
        plan.shares.len(),
        plan.images.len()
    );
    if plan.cleared > 0 {
        out += &format!(
            "Playlists' Jellyfin artwork and source ids cleared (they name nothing here): {}\n",
            plan.cleared
        );
    }
    if !plan.bad_images.is_empty() {
        out += &format!(
            "Artwork left behind (bytes don't match their hash, or not JPEG, PNG or WebP): {}\n",
            plan.bad_images.join(", ")
        );
    }
    let t = &plan.tracks;
    out += &format!(
        "Track references: {} found, {} of them a second off; {} ambiguous, {} not found, {} without a content key\n",
        t.exact + t.near,
        t.near,
        t.ambiguous.values().sum::<usize>(),
        t.missing.values().sum::<usize>(),
        t.keyless
    );
    for (label, keys) in [("Ambiguous", &t.ambiguous), ("Not found", &t.missing)] {
        if keys.is_empty() {
            continue;
        }
        out += &format!("{label} (kept with their Jellyfin id; records carrying each):\n");
        for (key, count) in keys {
            out += &format!("  {count:>5}  {key}\n");
        }
    }
    out
}
