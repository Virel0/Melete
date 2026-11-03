//! Joins the changelog entries and decisions pull requests leave as files of their own into
//! `CHANGELOG.md` and the decisions table of `docs/DESIGN.md`, at release time.
//!
//! Each pull request adds `changes/<topic>.md` (its changelog entries under Keep a Changelog
//! headings) and, for a new rule or behaviour, `docs/decisions/<topic>.md` (`Decision:`, `Why:`
//! and `Date:`), instead of editing the shared files, so two pull requests never change the
//! same lines. A release runs `cargo run --bin release_docs -- <version> <date>` from the
//! repository's root: the entries go under the new version, each decision gets the next number
//! and its row, and the files are removed. `cargo run --bin release_docs -- --check` only checks
//! the files, as the tests do.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Keep a Changelog's sections, in the order a version lists them.
const SECTIONS: [&str; 6] = [
    "Added",
    "Changed",
    "Deprecated",
    "Removed",
    "Fixed",
    "Security",
];
const CHANGES_DIR: &str = "changes";
const DECISIONS_DIR: &str = "docs/decisions";
const CHANGELOG: &str = "CHANGELOG.md";
const DESIGN: &str = "docs/DESIGN.md";
const UNRELEASED: &str = "## [Unreleased]";

/// A version's entries, by section in [`SECTIONS`]' order; each entry keeps its own lines.
type Entries = [Vec<String>; SECTIONS.len()];

/// One decision waiting for its number.
#[derive(Debug, PartialEq)]
struct Decision {
    what: String,
    why: String,
    date: String,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = Path::new(".");
    let outcome = match args.as_slice() {
        [flag] if flag == "--check" => check(root).map(|n| format!("{n} pending files are fine")),
        [version, date] => release(root, version, date),
        _ => Err("usage: release_docs <version> <YYYY-MM-DD> | --check".into()),
    };
    match outcome {
        Ok(said) => println!("{said}"),
        Err(why) => {
            eprintln!("release_docs: {why}");
            std::process::exit(1);
        }
    }
}

/// The pending files of `dir`, by name, leaving out its README.
fn pending(root: &Path, dir: &str) -> Result<Vec<PathBuf>, String> {
    let Ok(read) = std::fs::read_dir(root.join(dir)) else {
        return Ok(Vec::new());
    };
    let mut files: Vec<PathBuf> = read
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .filter(|p| p.file_name().is_some_and(|n| n != "README.md"))
        .collect();
    files.sort();
    Ok(files)
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Checks every pending file; returns how many there are.
fn check(root: &Path) -> Result<usize, String> {
    let changes = pending(root, CHANGES_DIR)?;
    let decisions = pending(root, DECISIONS_DIR)?;
    for path in &changes {
        parse_entries(&read(path)?).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    for path in &decisions {
        parse_decision(&read(path)?).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(changes.len() + decisions.len())
}

/// A changelog file's entries: `### Section` headings, each with `- ` entries whose further
/// lines are indented two spaces.
fn parse_entries(text: &str) -> Result<Entries, String> {
    let mut entries: Entries = Default::default();
    let mut section: Option<usize> = None;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("### ") {
            let at = SECTIONS
                .iter()
                .position(|s| *s == name.trim())
                .ok_or_else(|| format!("unknown section {:?}", name.trim()))?;
            section = Some(at);
        } else if line.trim().is_empty() {
            continue;
        } else if let Some(at) = section {
            if line.starts_with("- ") {
                entries[at].push(line.to_owned());
            } else if line.starts_with("  ") && !entries[at].is_empty() {
                let last = entries[at].last_mut().expect("checked not empty");
                last.push('\n');
                last.push_str(line);
            } else {
                return Err(format!("not an entry or its continuation: {line:?}"));
            }
        } else {
            return Err(format!("text before any section heading: {line:?}"));
        }
    }
    if entries.iter().all(Vec::is_empty) {
        return Err("no entries".into());
    }
    Ok(entries)
}

/// A decision file: `Decision:`, `Why:` and `Date:` (YYYY-MM-DD), each field running on until
/// the next; lines are joined with spaces, as a table row needs.
fn parse_decision(text: &str) -> Result<Decision, String> {
    let mut fields: [(&str, String); 3] = [
        ("Decision:", String::new()),
        ("Why:", String::new()),
        ("Date:", String::new()),
    ];
    let mut current: Option<usize> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(at) = fields.iter().position(|(key, _)| line.starts_with(key)) {
            if !fields[at].1.is_empty() {
                return Err(format!("{} twice", fields[at].0));
            }
            current = Some(at);
            fields[at].1 = line[fields[at].0.len()..].trim().to_owned();
        } else if let Some(at) = current {
            fields[at].1.push(' ');
            fields[at].1.push_str(line);
        } else {
            return Err(format!("text before `Decision:`: {line:?}"));
        }
    }
    for (key, value) in &fields {
        if value.is_empty() {
            return Err(format!("no {key}"));
        }
        if value.contains('|') {
            return Err(format!(
                "{key} holds a `|`, which would break the table's row"
            ));
        }
    }
    let [(_, what), (_, why), (_, date)] = fields;
    if !is_date(&date) {
        return Err(format!("Date: {date:?} isn't YYYY-MM-DD"));
    }
    Ok(Decision { what, why, date })
}

fn is_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

fn is_version(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
}

/// Joins every pending file in as `version` of `date`, then removes the files.
fn release(root: &Path, version: &str, date: &str) -> Result<String, String> {
    if !is_version(version) {
        return Err(format!("{version:?} isn't a version like 1.2.3"));
    }
    if !is_date(date) {
        return Err(format!("{date:?} isn't YYYY-MM-DD"));
    }
    let changes = pending(root, CHANGES_DIR)?;
    let decisions = pending(root, DECISIONS_DIR)?;
    let mut parts = Vec::new();
    for path in &changes {
        parts.push(parse_entries(&read(path)?).map_err(|e| format!("{}: {e}", path.display()))?);
    }
    let mut new = Vec::new();
    for path in &decisions {
        new.push(parse_decision(&read(path)?).map_err(|e| format!("{}: {e}", path.display()))?);
    }
    // Numbered in the order they were made; files of the same day by name.
    new.sort_by(|a, b| a.date.cmp(&b.date));
    let changelog = join_changelog(&read(&root.join(CHANGELOG))?, &parts, version, date)?;
    let design = join_decisions(&read(&root.join(DESIGN))?, &new)?;
    std::fs::write(root.join(CHANGELOG), changelog).map_err(|e| e.to_string())?;
    std::fs::write(root.join(DESIGN), design).map_err(|e| e.to_string())?;
    for path in changes.iter().chain(&decisions) {
        std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(format!(
        "{version}: {} changelog files and {} decisions joined",
        changes.len(),
        decisions.len()
    ))
}

/// The changelog with what's under Unreleased and `parts` moved under `version`, and its
/// comparison links moved on.
fn join_changelog(
    text: &str,
    parts: &[Entries],
    version: &str,
    date: &str,
) -> Result<String, String> {
    if text.contains(&format!("## [{version}]")) {
        return Err(format!("the changelog already has {version}"));
    }
    let start = text
        .find(&format!("{UNRELEASED}\n"))
        .ok_or("no `## [Unreleased]` heading")?;
    let body_at = start + UNRELEASED.len() + 1;
    let end = text[body_at..]
        .find("\n## [")
        .map_or(text.len(), |i| body_at + i + 1);
    let written = &text[body_at..end];
    let mut all: Entries = if written.trim().is_empty() {
        Entries::default()
    } else {
        parse_entries(written).map_err(|e| format!("under Unreleased in the changelog: {e}"))?
    };
    for part in parts {
        for (into, from) in all.iter_mut().zip(part) {
            into.extend(from.iter().cloned());
        }
    }
    if all.iter().all(Vec::is_empty) {
        return Err("nothing to release: no entries under Unreleased or in changes/".into());
    }
    let mut section = format!("{UNRELEASED}\n\n## [{version}] - {date}\n");
    for (name, entries) in SECTIONS.iter().zip(&all) {
        if entries.is_empty() {
            continue;
        }
        let _ = write!(section, "\n### {name}\n\n");
        for entry in entries {
            section.push_str(entry);
            section.push('\n');
        }
    }
    section.push('\n');
    let mut out = format!("{}{}{}", &text[..start], section, &text[end..]);
    // The real links, not the template's (`vX.Y.Z`) inside the comment.
    let link = "[Unreleased]: https://github.com/Virel0/Melete/compare/v";
    let at = out
        .match_indices(link)
        .map(|(i, _)| i)
        .find(|i| out[i + link.len()..].starts_with(|c: char| c.is_ascii_digit()))
        .ok_or("no `[Unreleased]:` comparison link")?;
    let line_end = out[at..].find('\n').map_or(out.len(), |i| at + i);
    let previous = out[at + link.len()..line_end]
        .strip_suffix("...HEAD")
        .ok_or("the `[Unreleased]:` link doesn't end in ...HEAD")?
        .to_owned();
    let links = format!(
        "{link}{version}...HEAD\n[{version}]: https://github.com/Virel0/Melete/compare/v{previous}...v{version}"
    );
    out.replace_range(at..line_end, &links);
    Ok(out)
}

/// The design doc with each decision appended to its table, numbered on from the last row.
fn join_decisions(text: &str, decisions: &[Decision]) -> Result<String, String> {
    if decisions.is_empty() {
        return Ok(text.to_owned());
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let (last_at, last) = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            let n = l
                .strip_prefix("| ")?
                .split(" |")
                .next()?
                .parse::<u32>()
                .ok()?;
            Some((i, n))
        })
        .next_back()
        .ok_or("no numbered rows in the decisions table")?;
    let mut rows = String::new();
    for (n, d) in (last + 1..).zip(decisions) {
        let _ = writeln!(rows, "| {n} | {} | {} | {} |", d.what, d.why, d.date);
    }
    let mut out: String = lines[..=last_at].concat();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&rows);
    out.push_str(&lines[last_at + 1..].concat());
    Ok(out)
}

#[cfg(test)]
mod tests;
