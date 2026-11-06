//! A download's extras: creditless openings and endings, PVs, CMs, menus, trailers and the like
//! that a pack carries beside a movie or its episodes. They go in the title's `Extras` folder,
//! which a library scan keeps apart from the title's own videos, and the running-time check
//! passes them over; a video whose name gives its episode is never one.

use crate::release::{self, Episodes};

/// The folder in a movie's or season's folder that extras go in.
pub(crate) const EXTRAS: &str = "Extras";

/// Folders in a download that hold extras, besides those a library scan knows
/// ([`crate::library::extras::is_extras_folder`]).
const FOLDERS: &[&str] = &[
    "nc",
    "ncop",
    "nced",
    "creditless",
    "menu",
    "menus",
    "pv",
    "pvs",
    "cm",
    "cms",
    "bonus",
    "extra",
];

/// Words in a file's name that mark it as an extra, alone or followed by a number ("NCOP2",
/// "PV 01").
const WORDS: &[&str] = &[
    "ncop",
    "nced",
    "creditless",
    "pv",
    "cm",
    "menu",
    "trailer",
    "teaser",
    "preview",
];

/// Whether a download's file, by its path in the download, is an extra: in an extras folder, or
/// named as one, unless its name gives an episode.
pub(crate) fn is_extra(parts: &[&str]) -> bool {
    let Some((name, folders)) = parts.split_last() else {
        return false;
    };
    if gives_episode(name) {
        return false;
    }
    folders.iter().any(|folder| {
        let lower = folder.to_lowercase();
        FOLDERS.contains(&lower.as_str()) || crate::library::extras::is_extras_folder(folder)
    }) || crate::library::extras::has_extra_name(name)
        || named_extra(name)
}

/// Whether the name gives an episode by season and number, or a date: then it's an episode
/// whatever else it says. A bare number isn't enough ("NCOP 01").
fn gives_episode(name: &str) -> bool {
    match release::parse(name).episodes {
        Episodes::Standard { episodes, .. } => !episodes.is_empty(),
        Episodes::Daily(_) => true,
        _ => false,
    }
}

/// Whether a word of the name, without its extension, marks an extra: one of [`WORDS`], or
/// "NC" before "OP" or "ED" ("NC OP1").
fn named_extra(name: &str) -> bool {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let words: Vec<String> = stem
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            w.to_lowercase()
                .trim_end_matches(|c: char| c.is_ascii_digit())
                .to_owned()
        })
        .collect();
    words.iter().any(|w| WORDS.contains(&w.as_str()))
        || words
            .windows(2)
            .any(|pair| pair[0] == "nc" && ["op", "ed"].contains(&pair[1].as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extra(path: &str) -> bool {
        is_extra(&path.split('/').collect::<Vec<_>>())
    }

    #[test]
    fn creditless_songs_pvs_and_menus_are_extras() {
        for path in [
            "Pack/Show - S01 NCED (BD 1080p HEVC) [Group].mkv",
            "Pack/[Group] Show - NCOP1 [1080p].mkv",
            "Pack/[Group] Show - NC OP 02 [1080p].mkv",
            "Pack/Show - Creditless Ending.mkv",
            "Pack/Show - PV 01.mkv",
            "Pack/Show CM2.mkv",
            "Pack/Show - Menu 01.mkv",
            "Pack/Show - Trailer.mkv",
            "Pack/Show - S01 NCED.ass",
            "Pack/Movie-featurette.mkv",
        ] {
            assert!(extra(path), "{path}");
        }
    }

    #[test]
    fn what_extras_folders_hold_is_extra() {
        for path in [
            "Pack/Extras/Show - Opening.mkv",
            "Pack/NC/Show 01.mkv",
            "Pack/Featurettes/Making of.mkv",
            "Pack/Bonus/Interview.mkv",
            "Pack/S01/Menus/01.mkv",
        ] {
            assert!(extra(path), "{path}");
        }
    }

    #[test]
    fn episodes_and_titles_that_only_look_like_extras_arent() {
        for path in [
            "Pack/Show - S01E03 (BD 1080p).mkv",
            "Pack/Show.S01E01.Preview.Night.1080p.mkv",
            "Pack/Show.2024.05.01.1080p.mkv",
            "Pack/Some.Movie.2024.Extras.mkv",
            "Pack/[Group] Show - 05 [1080p].mkv",
            "Pack/Pace.2020.1080p.mkv",
            "Pack/Ed.and.Edd.S01E02.mkv",
            "Pack/Uncommon.Title.2021.mkv",
        ] {
            assert!(!extra(path), "{path}");
        }
    }
}
