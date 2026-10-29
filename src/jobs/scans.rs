//! When the libraries are scanned whole, and which folders are scanned in between. A whole scan
//! lists every folder of every library, which wakes sleeping disks, so it runs on a schedule
//! (`MELETE_SCAN_SCHEDULE`: every so many hours, or once a day at a time); what Melete imports,
//! and what an admin asks for, is scanned on its own at once.

use std::sync::Mutex;

/// The whole-library scan's schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    /// Every so many seconds after the last whole scan.
    Every(i64),
    /// Once a day at this many minutes after the server's midnight.
    Daily(i64),
}

impl Schedule {
    /// Every 6 hours, as before the setting existed.
    pub const DEFAULT: Schedule = Schedule::Every(6 * 3600);

    /// `6h` (1 to 168 hours), `04:00` (a time of day, the server's), or empty for the default.
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        if value.is_empty() {
            return Ok(Self::DEFAULT);
        }
        if let Some(hours) = value.strip_suffix('h')
            && let Ok(hours) = hours.parse::<i64>()
            && (1..=168).contains(&hours)
        {
            return Ok(Self::Every(hours * 3600));
        }
        if let Some((h, m)) = value.split_once(':')
            && h.len() == 2
            && m.len() == 2
            && let (Ok(h), Ok(m)) = (h.parse::<i64>(), m.parse::<i64>())
            && (0..24).contains(&h)
            && (0..60).contains(&m)
        {
            return Ok(Self::Daily(h * 60 + m));
        }
        Err(format!(
            "MELETE_SCAN_SCHEDULE={value:?} should be a number of hours such as 6h, or a time such as 04:00"
        ))
    }

    /// Whether a whole scan is due at `now` (Unix seconds, `offset` seconds east of UTC on the
    /// server) after the last one at `last`; always, when there's been none.
    pub fn due(self, last: Option<i64>, now: i64, offset: i64) -> bool {
        let Some(last) = last else { return true };
        match self {
            Self::Every(seconds) => now - last >= seconds,
            Self::Daily(minute) => {
                // The latest time the clock showed that minute, at or before now.
                let local = now + offset;
                let day_start = local - local.rem_euclid(86_400);
                let mut at = day_start + minute * 60 - offset;
                if at > now {
                    at -= 86_400;
                }
                last < at
            }
        }
    }
}

/// The server's offset from UTC now, in seconds (0 when it can't be told).
pub fn utc_offset(now: i64) -> i64 {
    let time = now as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // localtime_r reads the time and writes only into `tm`, which outlives the call.
    let found = unsafe { libc::localtime_r(&time, &mut tm) };
    if found.is_null() { 0 } else { tm.tm_gmtoff }
}

/// Scans asked for since the last run.
struct Asked {
    whole: bool,
    /// Libraries by id, each whole (`None`) or one title folder.
    folders: Vec<(i64, Option<String>)>,
    /// When the last whole scan started, in Unix seconds.
    last_whole: Option<i64>,
}

static ASKED: Mutex<Asked> = Mutex::new(Asked {
    whole: false,
    folders: Vec::new(),
    last_whole: None,
});

/// What a housekeeping run scans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every library, whole.
    All,
    /// These libraries (by id), each whole or one title folder.
    Some(Vec<(i64, Option<String>)>),
}

/// Asks for every library to be scanned whole at the next run.
pub fn ask_whole() {
    ASKED.lock().expect("no holder panics").whole = true;
}

/// Asks for one library to be scanned at the next run, whole or only `folder`.
pub fn ask_folder(library_id: i64, folder: Option<String>) {
    let mut asked = ASKED.lock().expect("no holder panics");
    let request = (library_id, folder);
    if !asked.folders.contains(&request) {
        asked.folders.push(request);
    }
}

/// How long from `now` until the next whole scan is due, for the tasks list; at once when
/// there's been none.
pub fn next_whole(schedule: Schedule, now: i64) -> Option<std::time::Duration> {
    let last = ASKED.lock().expect("no holder panics").last_whole?;
    let at = next_after(schedule, last, utc_offset(now));
    Some(std::time::Duration::from_secs((at - now).max(0) as u64))
}

/// When the whole scan after one at `last` is due (`offset` seconds east of UTC).
fn next_after(schedule: Schedule, last: i64, offset: i64) -> i64 {
    match schedule {
        Schedule::Every(seconds) => last + seconds,
        Schedule::Daily(minute) => {
            // The first time after the last scan that the clock shows that minute.
            let local = last + offset;
            let day_start = local - local.rem_euclid(86_400);
            let mut at = day_start + minute * 60 - offset;
            if at <= last {
                at += 86_400;
            }
            at
        }
    }
}

/// What the next run scans: everything when a whole scan was asked for or is due, else the
/// libraries and folders asked for; `None` when there's nothing to do.
pub fn take(schedule: Schedule, now: i64) -> Option<Scope> {
    let mut asked = ASKED.lock().expect("no holder panics");
    let folders = std::mem::take(&mut asked.folders);
    if asked.whole || schedule.due(asked.last_whole, now, utc_offset(now)) {
        asked.whole = false;
        asked.last_whole = Some(now);
        return Some(Scope::All);
    }
    (!folders.is_empty()).then_some(Scope::Some(folders))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_next_whole_scan_follows_the_schedule() {
        let day = 86_400;
        assert_eq!(next_after(Schedule::Every(3600), 1000, 0), 4600);
        // 04:00 daily: a scan at 05:00 is next at 04:00 the day after; one at 03:00, that day.
        let four = Schedule::Daily(240);
        assert_eq!(
            next_after(four, 10 * day + 5 * 3600, 0),
            11 * day + 4 * 3600
        );
        assert_eq!(
            next_after(four, 10 * day + 3 * 3600, 0),
            10 * day + 4 * 3600
        );
        // Two hours east of UTC: 04:00 there is 02:00 UTC.
        assert_eq!(
            next_after(four, 10 * day + 3 * 3600, 7200),
            11 * day + 2 * 3600
        );
    }

    #[test]
    fn schedules_are_hours_or_a_time_of_day() {
        assert_eq!(Schedule::parse(""), Ok(Schedule::DEFAULT));
        assert_eq!(Schedule::parse("24h"), Ok(Schedule::Every(86_400)));
        assert_eq!(Schedule::parse("04:30"), Ok(Schedule::Daily(270)));
        for bad in ["0h", "169h", "6", "4:00", "24:00", "04:60", "daily"] {
            assert!(Schedule::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_whole_scan_is_due_once_its_time_has_come() {
        let hour = 3600;
        let six = Schedule::Every(6 * hour);
        assert!(six.due(None, 0, 0), "at startup");
        assert!(!six.due(Some(0), 5 * hour, 0));
        assert!(six.due(Some(0), 6 * hour, 0));

        // At 04:00, on a server two hours east of UTC: 02:00 UTC.
        let four = Schedule::Daily(4 * 60);
        let day = 86_400;
        let at = 10 * day + 2 * hour;
        assert!(
            !four.due(Some(at - hour), at - 60, 2 * hour),
            "not 04:00 yet"
        );
        assert!(four.due(Some(at - hour), at, 2 * hour), "04:00 came");
        assert!(!four.due(Some(at), at + 20 * hour, 2 * hour), "done today");
        assert!(four.due(Some(at), at + day, 2 * hour), "the next day");
    }
}
