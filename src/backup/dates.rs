//! The dates in copies' names, in UTC: `2026-10-28` for a day, `2026-10-28-101500` for a moment.

const DAY: i64 = 86_400;

/// The civil date (year, month, day) of a day since 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// Days since 1970-01-01 of a civil date, the other way round from [`civil`].
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The day of `unix` seconds, as `2026-10-28`.
pub(super) fn date(unix: i64) -> String {
    let (year, month, day) = civil(unix.div_euclid(DAY));
    format!("{year:04}-{month:02}-{day:02}")
}

/// The moment of `unix` seconds, as `2026-10-28-101500`.
pub(super) fn stamp(unix: i64) -> String {
    let seconds = unix.rem_euclid(DAY);
    format!(
        "{}-{:02}{:02}{:02}",
        date(unix),
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

/// The day since 1970-01-01 a `2026-10-28` names, when it's a real date.
pub(super) fn day_of(date: &str) -> Option<i64> {
    let mut parts = date.split('-');
    let mut next = |len: usize| {
        let part = parts.next().filter(|p| p.len() == len)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse::<i64>().ok())?
    };
    let (year, month, day) = (next(4)?, next(2)?, next(2)?);
    if parts.next().is_some() {
        return None;
    }
    let days = days_from_civil(year, month, day);
    (civil(days) == (year, month, day)).then_some(days)
}

/// The day since 1970-01-01 `unix` seconds fall on.
pub(super) fn day(unix: i64) -> i64 {
    unix.div_euclid(DAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_days_and_moments_in_utc() {
        // 2026-10-28 10:15:00 UTC.
        assert_eq!(date(1_793_182_500), "2026-10-28");
        assert_eq!(stamp(1_793_182_500), "2026-10-28-101500");
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(stamp(951_782_399), "2000-02-28-235959");
        assert_eq!(date(951_782_400), "2000-02-29");
    }

    #[test]
    fn reads_back_only_real_dates() {
        assert_eq!(day_of("2026-10-28"), Some(day(1_793_182_500)));
        assert_eq!(day_of("2000-02-29"), Some(day(951_782_400)));
        for wrong in [
            "2026-02-30",
            "2026-13-01",
            "26-10-28",
            "2026-10-28-1",
            "2026-1a-01",
            "",
        ] {
            assert_eq!(day_of(wrong), None, "{wrong}");
        }
    }
}
