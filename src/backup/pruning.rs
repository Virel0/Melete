//! Removing copies past what's kept. Only files named as Melete names its copies are ever
//! removed, and only regular files: anything else in the folder is left alone.

use super::*;

/// What one file in the backups folder is, by its name.
#[derive(Debug, PartialEq, Eq)]
enum Named {
    /// `daily-2026-10-28.db`, with its day.
    Daily(i64),
    /// `before-migrating-from-0075-2026-10-28-101500.db`.
    BeforeMigrating,
    /// `.daily-2026-10-28.db.part`, a copy being written or cut off.
    Part,
    /// Anything else, copies made by hand among them: never removed here.
    Other,
}

fn named(name: &str) -> Named {
    if let Some(day) = name
        .strip_prefix("daily-")
        .and_then(|rest| rest.strip_suffix(".db"))
        .and_then(dates::day_of)
    {
        return Named::Daily(day);
    }
    if name.starts_with("before-migrating-from-") && name.ends_with(".db") {
        return Named::BeforeMigrating;
    }
    let ours = ["daily-", "before-migrating-from-", "by-hand-"];
    if let Some(inner) = name
        .strip_prefix('.')
        .and_then(|n| n.strip_suffix(".db.part"))
        && ours.iter().any(|prefix| inner.starts_with(prefix))
    {
        return Named::Part;
    }
    Named::Other
}

/// The names to remove from those in the folder, as of day `today`: daily copies past the last
/// [`DAILY_KEPT`] days, keeping the newest of each of the [`WEEKLY_KEPT`] weeks before them;
/// copies made before migrating past the newest [`BEFORE_MIGRATING_KEPT`]. Part files are
/// judged by the caller, who knows their age.
fn past_keeping(names: &[String], today: i64) -> Vec<String> {
    let mut daily: Vec<(i64, &String)> = Vec::new();
    let mut migrating: Vec<&String> = Vec::new();
    for name in names {
        match named(name) {
            // A day ahead of today (a clock set back) is kept until it's past.
            Named::Daily(day) if day <= today => daily.push((day, name)),
            Named::BeforeMigrating => migrating.push(name),
            _ => {}
        }
    }
    daily.sort_by_key(|d| std::cmp::Reverse(d.0));
    // Newest first; the names sort by the version they hold, then when they were made.
    migrating.sort_by(|a, b| b.cmp(a));
    let mut remove = Vec::new();
    let mut weeks = Vec::new();
    for (i, (day, name)) in daily.into_iter().enumerate() {
        if i < DAILY_KEPT {
            continue;
        }
        // Weeks from Monday; 1970-01-01 was a Thursday.
        let week = (day + 3).div_euclid(7);
        if !weeks.contains(&week) && weeks.len() < WEEKLY_KEPT {
            weeks.push(week);
        } else {
            remove.push(name.clone());
        }
    }
    remove.extend(migrating.into_iter().skip(BEFORE_MIGRATING_KEPT).cloned());
    remove
}

/// Removes from the backups folder `dir` the copies past keeping as of `now` (Unix seconds),
/// and part files a day old (copies cut off). Answers the names removed.
pub fn prune(dir: &Path, now: i64) -> anyhow::Result<Vec<String>> {
    let mut names = Vec::new();
    let mut stale_parts = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        // Never through a link, never a folder.
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if named(&name) == Named::Part {
            let modified = entry.metadata()?.modified()?;
            let age = std::time::SystemTime::now()
                .duration_since(modified)
                .unwrap_or_default();
            if age > Duration::from_secs(86_400) {
                stale_parts.push(name);
            }
            continue;
        }
        names.push(name);
    }
    let mut removed = past_keeping(&names, dates::day(now));
    removed.extend(stale_parts);
    for name in &removed {
        remove_if_there(&dir.join(name))?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_its_own_names_only() {
        assert!(matches!(named("daily-2026-10-28.db"), Named::Daily(_)));
        assert_eq!(named("daily-2026-02-30.db"), Named::Other);
        assert_eq!(named("daily-2026-10-28.db.bak"), Named::Other);
        assert_eq!(
            named("before-migrating-from-0075-2026-10-28-101500.db"),
            Named::BeforeMigrating
        );
        assert_eq!(named(".daily-2026-10-28.db.part"), Named::Part);
        assert_eq!(named(".by-hand-2026-10-28-101500.db.part"), Named::Part);
        assert_eq!(named(".something.db.part"), Named::Other);
        assert_eq!(named("by-hand-2026-10-28-101500.db"), Named::Other);
        assert_eq!(named("melete.db"), Named::Other);
    }

    #[test]
    fn keeps_a_week_of_days_then_a_copy_a_week_for_four_weeks() {
        let today = dates::day_of("2026-10-28").unwrap();
        // Sixty days of daily copies, and others that are never pruned here.
        let mut names: Vec<String> = (0..60)
            .map(|ago| format!("daily-{}.db", dates::date((today - ago) * 86_400)))
            .collect();
        names.push("by-hand-2026-01-01-000000.db".into());
        names.push("melete.db".into());
        let removed = past_keeping(&names, today);
        let mut kept: Vec<&String> = names.iter().filter(|n| !removed.contains(n)).collect();
        kept.sort();
        assert_eq!(
            kept,
            [
                "by-hand-2026-01-01-000000.db",
                // Before the last seven days, the newest of each of four weeks (from Monday):
                // Sundays, and the Wednesday ending the week the seven days begin in.
                "daily-2026-10-04.db",
                "daily-2026-10-11.db",
                "daily-2026-10-18.db",
                "daily-2026-10-21.db",
                // The last seven days.
                "daily-2026-10-22.db",
                "daily-2026-10-23.db",
                "daily-2026-10-24.db",
                "daily-2026-10-25.db",
                "daily-2026-10-26.db",
                "daily-2026-10-27.db",
                "daily-2026-10-28.db",
                "melete.db",
            ]
        );
    }

    #[test]
    fn keeps_the_newest_copies_made_before_migrating() {
        let names: Vec<String> = [70, 71, 72, 73, 74, 75, 76]
            .iter()
            .map(|v| format!("before-migrating-from-{v:04}-2026-10-{v}-000000.db"))
            .collect();
        let removed = past_keeping(&names, 0);
        assert_eq!(removed, [names[1].clone(), names[0].clone()]);
    }
}
