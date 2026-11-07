//! Spans back from now, as rules write them (`-30d`, `-6m`, `1y`), and the calendar they're
//! counted in: months and years are calendar arithmetic, as the apps do it, so a month before
//! 31 March is the end of February. Counted in the listener's time, as a fixed offset from UTC
//! the client gives (the apps count in the device's own zone).

use super::SmartError;

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

/// A span's unit, by the letter that ends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
    /// `h`: exact hours.
    Hours,
    /// `d`: calendar days.
    Days,
    /// `w`: seven calendar days.
    Weeks,
    /// `m`: calendar months.
    Months,
    /// `y`: twelve calendar months.
    Years,
}

/// A span as written; kept relative until a rule is worked out, so "not played in a month"
/// keeps moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Interval {
    /// How many units.
    pub amount: u32,
    /// Which unit.
    pub unit: Unit,
}

impl Interval {
    /// `-30d` or `30d`: the minus is decoration, as every operator already looks back.
    ///
    /// # Errors
    ///
    /// [`SmartError::MalformedInterval`] when it isn't digits and one of `h d w m y`.
    pub fn parse(written: &str) -> Result<Self, SmartError> {
        let refuse = || SmartError::MalformedInterval(written.to_owned());
        let lowered = written.trim_matches([' ', '\t']).to_lowercase();
        let body = lowered.strip_prefix('-').unwrap_or(&lowered);
        let unit = match body.chars().last() {
            Some('h') => Unit::Hours,
            Some('d') => Unit::Days,
            Some('w') => Unit::Weeks,
            Some('m') => Unit::Months,
            Some('y') => Unit::Years,
            _ => return Err(refuse()),
        };
        let digits = &body[..body.len() - 1];
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(refuse());
        }
        let amount = digits.parse().map_err(|_| refuse())?;
        Ok(Self { amount, unit })
    }

    /// The instant this far before `now` (epoch milliseconds), counted on a calendar
    /// `offset_minutes` from UTC: hours exactly, the rest keeping the wall-clock time.
    pub fn ago(&self, now: i64, offset_minutes: i64) -> i64 {
        let amount = i64::from(self.amount);
        let months = match self.unit {
            Unit::Hours => return now.saturating_sub(amount.saturating_mul(HOUR_MS)),
            Unit::Days => return now.saturating_sub(amount.saturating_mul(DAY_MS)),
            Unit::Weeks => return now.saturating_sub(amount.saturating_mul(7 * DAY_MS)),
            Unit::Months => amount,
            Unit::Years => amount.saturating_mul(12),
        };
        let local = now.saturating_add(offset_minutes.saturating_mul(60_000));
        let (days, time) = (local.div_euclid(DAY_MS), local.rem_euclid(DAY_MS));
        let (year, month, day) = civil(days);
        let index = (year * 12 + i64::from(month) - 1).saturating_sub(months);
        let (year, month) = (index.div_euclid(12), (index.rem_euclid(12) + 1) as u32);
        let day = day.min(month_length(year, month));
        (days_from_civil(year, month, day) * DAY_MS + time)
            .saturating_sub(offset_minutes.saturating_mul(60_000))
    }
}

fn leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn month_length(year: i64, month: u32) -> u32 {
    match month {
        2 if leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 as a (year, month, day), by the proleptic Gregorian calendar.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// [`civil`]'s inverse.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let month = i64::from(month);
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Noon UTC on a day, in epoch milliseconds.
    fn noon(year: i64, month: u32, day: u32) -> i64 {
        days_from_civil(year, month, day) * DAY_MS + 12 * HOUR_MS
    }

    #[test]
    fn the_calendar_round_trips() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(noon(2023, 11, 14), 1_699_963_200_000);
        for days in [-800_000, -1, 0, 59, 60, 11_016, 19_782, 2_000_000] {
            let (y, m, d) = civil(days);
            assert_eq!(days_from_civil(y, m, d), days, "{days}");
        }
    }

    /// The apps' `months_and_years_are_calendar_arithmetic_not_multiples_of_a_day`.
    #[test]
    fn months_and_years_are_calendar_arithmetic_not_multiples_of_a_day() {
        let end_of_march = noon(2024, 3, 31);
        let ago = |written: &str| Interval::parse(written).unwrap().ago(end_of_march, 0);
        assert_eq!(ago("-1m"), noon(2024, 2, 29));
        assert_eq!(ago("-1y"), noon(2023, 3, 31));
        assert_eq!(ago("-13m"), noon(2023, 2, 28));
        assert_eq!(ago("-30d"), end_of_march - 30 * DAY_MS);
        assert_eq!(ago("-2w"), end_of_march - 14 * DAY_MS);
        assert_eq!(ago("-5h"), end_of_march - 5 * HOUR_MS);
    }

    /// A month counted in the listener's time: at 01:00 on 1 March where they are, which is
    /// still February in UTC, a month back is 1 February where they are.
    #[test]
    fn months_count_in_the_listeners_own_time() {
        let offset = 120;
        let now = days_from_civil(2024, 3, 1) * DAY_MS + HOUR_MS - 2 * HOUR_MS;
        let back = Interval::parse("1m").unwrap().ago(now, offset);
        assert_eq!(back, days_from_civil(2024, 2, 1) * DAY_MS - HOUR_MS);
        // In UTC it's 29 February at 23:00, and a month back is 29 January.
        assert_eq!(
            Interval::parse("1m").unwrap().ago(now, 0),
            days_from_civil(2024, 1, 29) * DAY_MS + 23 * HOUR_MS
        );
    }

    /// The apps' `intervals_parse_with_or_without_the_leading_minus_and_refuse_anything_else`.
    #[test]
    fn intervals_parse_with_or_without_the_minus_and_refuse_anything_else() {
        let six_months = Interval {
            amount: 6,
            unit: Unit::Months,
        };
        assert_eq!(Interval::parse("-6m"), Ok(six_months));
        assert_eq!(Interval::parse("6m"), Ok(six_months));
        assert_eq!(
            Interval::parse(" -1Y "),
            Ok(Interval {
                amount: 1,
                unit: Unit::Years
            })
        );
        for bad in ["banana", "30", "d", "3.5d", "--3d", "30 days", ""] {
            assert_eq!(
                Interval::parse(bad),
                Err(SmartError::MalformedInterval(bad.into())),
                "{bad}"
            );
        }
    }
}
