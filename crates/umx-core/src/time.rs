//! Epoch milliseconds to ISO 8601 UTC text (`2026-11-09T05:50:12Z`), and the `date` kind's values: a year, a
//! month, a day or a date-time, each a span of UTC time (R4), in years 0001 to 9999 (F211: before 1970 too). Pure
//! arithmetic, no clock.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

/// A clock reading in milliseconds since the epoch as UTC text, to the second ([`iso_from_secs`]).
pub fn iso_from_ms(ms: u64) -> String {
    // u64::MAX / 1000 is below i64::MAX, so the seconds always fit.
    iso_from_secs((ms / 1000) as i64)
}

/// Seconds since the epoch, before it too (F211), as UTC text with a `Z`: `1969-07-20T20:17:40Z`. The year is
/// written with four digits, so give an instant in years 0001 to 9999 (every date-time [`normalise_date`] reads is).
pub fn iso_from_secs(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// The first and last second of years 0001 to 9999, UTC: the date-times umx reads (F211).
const FIRST_SECOND: i64 = -62_135_596_800;
const LAST_SECOND: i64 = 253_402_300_799;

// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn num(s: &str) -> Option<u32> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// A `date` value, by its precision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Parsed {
    Year(i64),
    Month(i64, u32),
    Day(i64, u32, u32),
    /// Whole seconds since the epoch, UTC, and whether the text gave a `Z` or an offset (ws008 TM3: without one it
    /// was taken as UTC).
    Instant(i64, bool),
}

/// Normalises a date value (workshop 007 rule 5, ws008 TM2): `YYYY`, `YYYY-MM` and `YYYY-MM-DD` stay as written; a
/// date-time becomes UTC with a `Z` and whole seconds. Accepts `T` or a space, optional seconds and fraction, and
/// `Z` or `+HH:MM`/`-HH:MM` (none means UTC). None if it is not a real date.
pub fn normalise_date(s: &str) -> Option<String> {
    Some(match parse(s)? {
        Parsed::Year(y) => format!("{y:04}"),
        Parsed::Month(y, m) => format!("{y:04}-{m:02}"),
        Parsed::Day(y, m, d) => format!("{y:04}-{m:02}-{d:02}"),
        Parsed::Instant(t, _) => iso_from_secs(t),
    })
}

/// A date value's span of UTC time in whole seconds, half-open `[start, end)`: a year, a month or a day is that
/// whole UTC period, and a date-time (held to the second) is its one second. Calendar spans nest or are disjoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateSpan {
    pub start: i64,
    pub end: i64,
}

impl DateSpan {
    /// How `self` stands against `other` in a comparison (`<`, `=`, `>` and the rest): `Less` when it ends by the
    /// time `other` starts, `Greater` when it starts once `other` has ended, else `Equal` (they overlap: one holds
    /// the other). So `2026-10-29T22:40:00Z` is before `2026-10-30` and equal to `2026-10-29`, and `2025` is neither
    /// before nor after `2025-02`.
    pub fn relation(&self, other: &DateSpan) -> Ordering {
        if self.end <= other.start {
            Ordering::Less
        } else if self.start >= other.end {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }

    /// The one total order for sorting: by start, then the longer span first, so `2025` < `2025-01` <
    /// `2025-01-01` < `2025-01-01T00:00:00Z`. On normalised values it is the order of their text.
    pub fn order(&self, other: &DateSpan) -> Ordering {
        self.start.cmp(&other.start).then(other.end.cmp(&self.end))
    }
}

/// The span of a date value, normalised or not; None if `s` is not a date.
pub fn date_span(s: &str) -> Option<DateSpan> {
    let day = |y, m, d| days_from_civil(y, m, d) * 86_400;
    let (start, end) = match parse(s)? {
        Parsed::Year(y) => (day(y, 1, 1), day(y + 1, 1, 1)),
        Parsed::Month(y, 12) => (day(y, 12, 1), day(y + 1, 1, 1)),
        Parsed::Month(y, m) => (day(y, m, 1), day(y, m + 1, 1)),
        Parsed::Day(y, m, d) => (day(y, m, d), day(y, m, d) + 86_400),
        Parsed::Instant(t, _) => (t, t + 1),
    };
    Some(DateSpan { start, end })
}

const DAY: i64 = 86_400;

/// How fine a date value is (`2025-01` is the whole month, not its first day). Finest first, so `a <= b` reads
/// "`a` is at least as fine as `b`".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Precision {
    Second,
    Day,
    Month,
    Year,
}

impl DateSpan {
    /// The value's precision, read from its length: one second, one day, 28 to 31 days, or a year.
    pub fn precision(&self) -> Precision {
        match self.end - self.start {
            1 => Precision::Second,
            DAY => Precision::Day,
            len if len <= 31 * DAY => Precision::Month,
            _ => Precision::Year,
        }
    }
}

/// A unit of the Cypher date functions (`date_part`, `date_trunc`, `date_diff`), named as LadybugDB names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Year,
    Quarter,
    Month,
    /// The ISO week: weeks start on Monday, and week 1 holds the year's first Thursday.
    Week,
    Day,
    /// A part only (Sunday is 0, as LadybugDB), never a cut.
    DayOfWeek,
    Hour,
    Minute,
    Second,
}

impl Unit {
    /// Every unit, with its name.
    pub const ALL: [(&'static str, Unit); 9] = [
        ("year", Unit::Year),
        ("quarter", Unit::Quarter),
        ("month", Unit::Month),
        ("week", Unit::Week),
        ("day", Unit::Day),
        ("dayofweek", Unit::DayOfWeek),
        ("hour", Unit::Hour),
        ("minute", Unit::Minute),
        ("second", Unit::Second),
    ];

    /// The unit a name gives, in any case; None if no unit has that name.
    pub fn parse(name: &str) -> Option<Unit> {
        Unit::ALL
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|&(_, u)| u)
    }

    /// Whether the unit cuts time into buckets (`date_trunc`, `date_diff`): every unit but `dayofweek`.
    pub fn is_cut(self) -> bool {
        self != Unit::DayOfWeek
    }

    /// The coarsest precision a value may have for this unit to mean something on it.
    fn needs(self) -> Precision {
        match self {
            Unit::Year => Precision::Year,
            Unit::Quarter | Unit::Month => Precision::Month,
            Unit::Week | Unit::Day | Unit::DayOfWeek => Precision::Day,
            Unit::Hour | Unit::Minute | Unit::Second => Precision::Second,
        }
    }
}

/// The UTC calendar day, and second of that day, a span starts in.
fn civil_of(start: i64) -> (i64, u32, u32, i64) {
    let (y, m, d) = civil_from_days(start.div_euclid(DAY));
    (y, m, d, start.rem_euclid(DAY))
}

/// Monday is 0.
fn weekday_from_monday(days: i64) -> i64 {
    (days + 3).rem_euclid(7)
}

/// `YYYY-MM-DD` for a day number; None outside the years a date value can be written in (0001 to 9999, F211).
fn day_text(days: i64) -> Option<String> {
    let (y, m, d) = civil_from_days(days);
    (1..=9999)
        .contains(&y)
        .then(|| format!("{y:04}-{m:02}-{d:02}"))
}

/// `YYYY-MM-DDTHH:MM:SSZ` for an instant, before 1970 too (F211); None outside years 0001 to 9999.
fn instant_text(secs: i64) -> Option<String> {
    (FIRST_SECOND..=LAST_SECOND)
        .contains(&secs)
        .then(|| iso_from_secs(secs))
}

/// `date_part(unit, value)`: the unit's number in the value's UTC calendar; None when the unit is finer than the
/// value (`day` of `2025-01`, `hour` of a day: the part is not known).
pub fn date_part(unit: Unit, span: DateSpan) -> Option<i64> {
    if span.precision() > unit.needs() {
        return None;
    }
    let (y, m, d, second) = civil_of(span.start);
    let days = span.start.div_euclid(DAY);
    Some(match unit {
        Unit::Year => y,
        Unit::Quarter => i64::from((m - 1) / 3 + 1),
        Unit::Month => i64::from(m),
        Unit::Week => {
            let thursday = days - weekday_from_monday(days) + 3;
            let (iso_year, _, _) = civil_from_days(thursday);
            (thursday - days_from_civil(iso_year, 1, 1)) / 7 + 1
        }
        Unit::Day => i64::from(d),
        Unit::DayOfWeek => (days + 4).rem_euclid(7),
        Unit::Hour => second / 3600,
        Unit::Minute => second % 3600 / 60,
        Unit::Second => second % 60,
    })
}

/// Where the bucket of `unit` holding the value starts, in seconds; None when the unit is finer than the value, or
/// is not a cut.
fn bucket_start(unit: Unit, span: DateSpan) -> Option<i64> {
    if !unit.is_cut() || span.precision() > unit.needs() {
        return None;
    }
    let (y, m, _, _) = civil_of(span.start);
    let days = span.start.div_euclid(DAY);
    Some(match unit {
        Unit::Year => days_from_civil(y, 1, 1) * DAY,
        Unit::Quarter => days_from_civil(y, (m - 1) / 3 * 3 + 1, 1) * DAY,
        Unit::Month => days_from_civil(y, m, 1) * DAY,
        Unit::Week => (days - weekday_from_monday(days)) * DAY,
        Unit::Day => days * DAY,
        Unit::Hour => span.start - span.start.rem_euclid(3600),
        Unit::Minute => span.start - span.start.rem_euclid(60),
        Unit::Second | Unit::DayOfWeek => span.start,
    })
}

/// `date_trunc(unit, value)`: the start of the unit's bucket holding the value, as text: `YYYY-MM-DD` for `day` and
/// coarser, `YYYY-MM-DDTHH:MM:SSZ` for `hour`, `minute` and `second`. None when the unit is finer than the value
/// (`week` of `2025-01`), or is not a cut (`dayofweek`).
pub fn date_trunc(unit: Unit, span: DateSpan) -> Option<String> {
    let start = bucket_start(unit, span)?;
    match unit {
        Unit::Hour | Unit::Minute | Unit::Second => instant_text(start),
        _ => day_text(start.div_euclid(DAY)),
    }
}

/// `date_diff(unit, a, b)`, DuckDB's `date_diff`: the number of unit boundaries crossed going from `a` to `b`, which
/// is how many buckets `date_trunc(unit, b)` lies after `date_trunc(unit, a)`; except `week`, which is whole 7-day
/// spans between the two UTC days, truncated toward zero, as DuckDB counts weeks. Positive when `b` is later,
/// negative when earlier; so `date_diff('month', '2025-01-31', '2025-02-01')` is 1,
/// `date_diff('day', '2025-01-01T23:59:59Z', '2025-01-02T00:00:00Z')` is 1, and `date_diff('week', <a Sunday>, <the
/// Monday after>)` is 0. None when the unit is finer than either value, or is not a cut. (Complete units would be
/// DuckDB's `date_sub`, which umx does not have.)
pub fn date_diff(unit: Unit, a: DateSpan, b: DateSpan) -> Option<i64> {
    let (from, to) = (bucket_start(unit, a)?, bucket_start(unit, b)?);
    let months = |s: i64| {
        let (y, m, _, _) = civil_of(s);
        y * 12 + i64::from(m) - 1
    };
    Some(match unit {
        Unit::Year | Unit::Quarter | Unit::Month => {
            let per = match unit {
                Unit::Year => 12,
                Unit::Quarter => 3,
                _ => 1,
            };
            (months(to) - months(from)) / per
        }
        Unit::Week => (b.start.div_euclid(DAY) - a.start.div_euclid(DAY)) / 7,
        Unit::Day => (to - from) / DAY,
        Unit::Hour => (to - from) / 3600,
        Unit::Minute => (to - from) / 60,
        Unit::Second | Unit::DayOfWeek => to - from,
    })
}

/// Cypher's `date(value)`: the value as a date, at its own precision: a year or a month as written (`2025`,
/// `2025-01`), a day as itself, and a date-time cut to its UTC day (R4).
pub fn date_text(span: DateSpan) -> Option<String> {
    let (y, m, _, _) = civil_of(span.start);
    match span.precision() {
        Precision::Year => Some(format!("{y:04}")),
        Precision::Month => Some(format!("{y:04}-{m:02}")),
        Precision::Day | Precision::Second => day_text(span.start.div_euclid(DAY)),
    }
}

/// Cypher's `timestamp(value)`: the instant a date-time is, or a day's UTC midnight, as `YYYY-MM-DDTHH:MM:SSZ`,
/// before 1970 too (F211). None for a month or a year (not an instant).
pub fn timestamp_text(span: DateSpan) -> Option<String> {
    if span.precision() > Precision::Day {
        return None;
    }
    instant_text(span.start)
}

fn parse(s: &str) -> Option<Parsed> {
    let s = s.trim();
    // Cheap reject: comparisons ask this of every text, and a date starts with four digits.
    if s.len() < 4 || !s.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let (date, rest) = if s.len() > 10 {
        (s.get(..10)?, s.get(10..)?)
    } else {
        (s, "")
    };
    let mut parts = date.split('-');
    let y = parts.next()?;
    if y.len() != 4 {
        return None;
    }
    // Years 0001 to 9999 (F211): year 0000 is not a date umx reads.
    let y = num(y).filter(|&y| y >= 1)? as i64;
    let Some(m) = parts.next() else {
        return Some(Parsed::Year(y));
    };
    if m.len() != 2 {
        return None;
    }
    let m = num(m).filter(|m| (1..=12).contains(m))?;
    let Some(d) = parts.next() else {
        return Some(Parsed::Month(y, m));
    };
    if d.len() != 2 || parts.next().is_some() {
        return None;
    }
    let d = num(d)?;
    if d == 0 || civil_from_days(days_from_civil(y, m, d)) != (y, m, d) {
        return None;
    }
    if rest.is_empty() {
        return Some(Parsed::Day(y, m, d));
    }
    let rest = rest.strip_prefix(['T', ' '])?;
    let (clock, offset_min, offset): (&str, i64, bool) =
        if let Some(c) = rest.strip_suffix(['Z', 'z']) {
            (c, 0, true)
        } else if let Some(i) = rest.rfind(['+', '-']).filter(|&i| i >= 5) {
            let (c, off) = rest.split_at(i);
            let sign = if off.starts_with('-') { -1 } else { 1 };
            let (oh, om) = off[1..].split_once(':').unwrap_or((&off[1..], "00"));
            let (oh, om) = (num(oh)? as i64, num(om)? as i64);
            if oh > 23 || om > 59 {
                return None;
            }
            (c, sign * (oh * 60 + om), true)
        } else {
            (rest, 0, false)
        };
    let clock = match clock.split_once('.') {
        Some((clock, fraction)) => {
            if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            clock
        }
        None => clock,
    };
    let mut hms = clock.split(':');
    let (h, mi) = (num(hms.next()?)?, num(hms.next()?)?);
    let sec = match hms.next() {
        Some(x) => num(x)?,
        None => 0,
    };
    if h > 23 || mi > 59 || sec > 59 || hms.next().is_some() {
        return None;
    }
    let total =
        days_from_civil(y, m, d) * 86_400 + (h * 3600 + mi * 60 + sec) as i64 - offset_min * 60;
    // Before 1970 too (F211), within years 0001 to 9999 once the offset is applied.
    (FIRST_SECOND..=LAST_SECOND)
        .contains(&total)
        .then_some(Parsed::Instant(total, offset))
}

/// True if `s` is already a normalised date or date-time (`normalise_date(s) == Some(s)`), without building the
/// normalised text: a parsed value is in its normal form exactly when it has that form's shape.
pub fn looks_like_date(s: &str) -> bool {
    let b = s.as_bytes();
    match parse(s) {
        Some(Parsed::Year(_)) => b.len() == 4,
        Some(Parsed::Month(..)) => b.len() == 7,
        Some(Parsed::Day(..)) => b.len() == 10,
        Some(Parsed::Instant(..)) => {
            b.len() == 20 && b[10] == b'T' && b[13] == b':' && b[16] == b':' && b[19] == b'Z'
        }
        None => false,
    }
}

/// ws008 TM3 (`time_no_offset`, R4): `s` is a date-time with a clock and no `Z` or offset, which umx takes as universal
/// time, so a local time written bare is stored hours out. A plain date, a month or a year has no clock to offset.
/// The parse that stores the value decides (R332-1): padding is trimmed the same way, with no second reading.
pub fn written_without_offset(s: &str) -> bool {
    matches!(parse(s), Some(Parsed::Instant(_, false)))
}

/// ws008 TM3 (`time_as_text`): text that reads as a time in the world: a date of month precision or finer, a
/// date-time, or a clock time (`9:40`, `09:40:00`, `9:40 am`). A bare year is left alone: four digits are often not
/// a date.
pub fn reads_as_time(s: &str) -> bool {
    matches!(
        parse(s),
        Some(Parsed::Month(..) | Parsed::Day(..) | Parsed::Instant(..))
    ) || clock_time(s)
}

/// ws008 TM3 (`time_as_text`): a field name that says it holds a time (`met_at`, `signed_on`).
pub fn time_like_name(name: &str) -> bool {
    name.ends_with("_at") || name.ends_with("_on")
}

fn clock_time(s: &str) -> bool {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    let (clock, twelve) = match lower
        .strip_suffix("am")
        .or_else(|| lower.strip_suffix("pm"))
    {
        Some(c) => (c.trim_end(), true),
        None => (lower.as_str(), false),
    };
    let parts: Vec<&str> = clock.split(':').collect();
    let digits =
        |p: &str, lens: &[usize]| lens.contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit());
    if !(2..=3).contains(&parts.len())
        || !digits(parts[0], &[1, 2])
        || !parts[1..].iter().all(|p| digits(p, &[2]))
    {
        return false;
    }
    let num = |p: &str| p.parse::<u32>().unwrap_or(u32::MAX);
    let h = num(parts[0]);
    let in_range = if twelve {
        (1..=12).contains(&h)
    } else {
        h <= 23
    };
    in_range && parts[1..].iter().all(|p| num(p) <= 59)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_date_time_without_an_offset_is_found_and_dates_and_offsets_are_not() {
        use super::written_without_offset as bare;
        for s in [
            "2026-10-30T09:40",
            "2026-10-30T09:40:00",
            "2026-10-30 09:40",
            "2026-10-30T09:40:00.5",
        ] {
            assert!(bare(s), "{s}");
        }
        for s in [
            "2026-10-30T09:40:00Z",
            "2026-10-30T09:40:00z",
            "2026-10-30T09:40:00+11:00",
            "2026-10-30T09:40-05:00",
            "2026-10-30",
            "2026-10",
            "2025",
            "not a date",
            "2026-10-30T25:00",
        ] {
            assert!(!bare(s), "{s}");
        }
        // R332-1: padding is trimmed as the stored value is, so it never changes the answer.
        for (s, want) in [
            (" 2026-10-30T09:40:00", true),
            ("2026-10-30T09:40:00 ", true),
            ("\t2026-10-30 09:40\n", true),
            ("2026-10-30T09:40:00Z ", false),
            (" 2026-10-30T09:40:00z", false),
            (" 2026-10-30T09:40:00+11:00 ", false),
            (" 2026-10-30T09:40:00-05:00 ", false),
        ] {
            assert_eq!(bare(s), want, "{s:?}");
            assert!(super::normalise_date(s).is_some(), "{s:?} is a date");
        }
        // An offset needs its colon: +1100 is no date at all, so it is never stored and never warns.
        assert_eq!(super::normalise_date("2026-10-30T09:40:00+1100"), None);
        assert!(!bare("2026-10-30T09:40:00+1100"));
    }

    #[test]
    fn time_text_is_a_month_or_finer_or_a_clock_time() {
        use super::{reads_as_time, time_like_name};
        for s in [
            "2026-10",
            "2026-10-30",
            "2026-10-30T09:40:00Z",
            "9:40",
            "09:40:00",
            "9:40 am",
            "11:05PM",
        ] {
            assert!(reads_as_time(s), "{s}");
        }
        for s in [
            "2025",
            "9",
            "9:4",
            "24:00",
            "13:00 pm",
            "ratio 3:2",
            "v1.2",
            "10:60",
            "",
        ] {
            assert!(!reads_as_time(s), "{s}");
        }
        assert!(time_like_name("met_at") && time_like_name("signed_on"));
        assert!(
            !time_like_name("at") && !time_like_name("onboarding") && !time_like_name("status")
        );
    }

    use super::*;

    /// F211: dates and date-times before 1970 read, normalise, span and compare like later ones, within years 0001
    /// to 9999; outside that range (year 0000, or an offset that carries the instant past either end) a value is not
    /// a date.
    #[test]
    fn dates_before_1970_read_span_and_compare_within_years_0001_to_9999() {
        assert_eq!(iso_from_secs(-1), "1969-12-31T23:59:59Z");
        assert_eq!(iso_from_secs(FIRST_SECOND), "0001-01-01T00:00:00Z");
        assert_eq!(iso_from_secs(LAST_SECOND), "9999-12-31T23:59:59Z");
        for (given, stored) in [
            ("1969-07-20T16:17:40-04:00", "1969-07-20T20:17:40Z"),
            ("1969-07-20T20:17:40Z", "1969-07-20T20:17:40Z"),
            ("1066-10-14 09:00", "1066-10-14T09:00:00Z"),
            ("1066-10-14", "1066-10-14"),
            ("1969-02", "1969-02"),
            ("0001-01-01T00:00:00Z", "0001-01-01T00:00:00Z"),
            ("9999-12-31T23:59:59Z", "9999-12-31T23:59:59Z"),
            ("0001", "0001"),
        ] {
            assert_eq!(normalise_date(given).as_deref(), Some(stored), "{given}");
            assert!(looks_like_date(stored), "{stored}");
        }
        for s in [
            "0000",
            "0000-12-31",
            "0000-12-31T23:59:59Z",
            "0001-01-01T00:30:00+01:00",
            "9999-12-31T23:30:00-01:00",
        ] {
            assert_eq!(normalise_date(s), None, "{s}");
            assert_eq!(date_span(s), None, "{s}");
        }
        // A month before 1970 is that whole UTC month; an instant in it is inside it; across 1970 the order holds.
        let feb = date_span("1969-02").unwrap();
        assert_eq!(feb.end - feb.start, 28 * 86_400);
        let span = |s| date_span(s).unwrap();
        assert_eq!(span("1969-02-14T12:00:00Z").relation(&feb), Ordering::Equal);
        assert_eq!(
            span("1969-12-31T23:59:59Z").relation(&span("1970")),
            Ordering::Less
        );
        assert_eq!(
            span("1970-01-01T00:00:00Z").relation(&span("1969")),
            Ordering::Greater
        );
        assert_eq!(
            span("1066").relation(&span("1969-07-20T20:17:40Z")),
            Ordering::Less
        );
    }

    #[test]
    fn known_instants() {
        assert_eq!(iso_from_ms(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_ms(1_794_203_355_851), "2026-11-09T05:49:15Z");
        assert_eq!(iso_from_ms(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(normalise_date("2026-11-09").as_deref(), Some("2026-11-09"));
        assert_eq!(
            normalise_date("2026-11-09T15:50:12.345+10:00").as_deref(),
            Some("2026-11-09T05:50:12Z")
        );
        assert_eq!(
            normalise_date("2026-11-09 05:50").as_deref(),
            Some("2026-11-09T05:50:00Z")
        );
        assert_eq!(normalise_date("2026-02-30"), None);
        assert_eq!(normalise_date("soon"), None);
        assert_eq!(normalise_date("2026-11-09T05:50:12.junkZ"), None);
        assert_eq!(normalise_date("2026-11-09T05:50:12.Z"), None);
        assert_eq!(normalise_date("123456789é"), None);
        assert!(!looks_like_date("2026-11-09T05:50:12.5Z"));
        assert!(looks_like_date("2026-11-09"));
        assert!(looks_like_date("2026-11-09T05:49:15Z"));
        assert!(!looks_like_date("2026-1-09"));
    }

    #[test]
    fn month_and_year_round_trip_and_bad_ones_are_refused() {
        for s in [
            "2025",
            "2025-01",
            "2025-12",
            "2024-02-29",
            "2026-11-09T05:50:12Z",
            "1969",
        ] {
            assert_eq!(normalise_date(s).as_deref(), Some(s), "{s}");
            assert!(looks_like_date(s), "{s}");
        }
        assert_eq!(normalise_date(" 2025-01 ").as_deref(), Some("2025-01"));
        assert_eq!(
            normalise_date("2026-10-30T09:40:00+11:00").as_deref(),
            Some("2026-10-29T22:40:00Z")
        );
        for s in [
            "2025-13",
            "2025-00",
            "2025-1",
            "2025-",
            "2025-01-",
            "25",
            "20251",
            "202",
            "+2025",
            "2025-01T10:00",
            "2025-1-01",
            "2025-01-1",
            "2025-01-01-01",
            "abcd",
            "2025-ab",
        ] {
            assert_eq!(normalise_date(s), None, "{s}");
            assert_eq!(date_span(s), None, "{s}");
        }
    }

    /// `looks_like_date` reads the shape without building the text; it must agree with the round trip.
    #[test]
    fn looks_like_date_is_the_round_trip() {
        for s in [
            "2025",
            " 2025",
            "2025 ",
            "2025-01",
            "2025-01-15",
            "2026-11-09T05:50:12Z",
            "2026-11-09T05:50:12z",
            "2026-11-09 05:50:12Z",
            "2026-11-09T05:5:012Z",
            "2026-11-09T05:50.12Z",
            "2026-11-09T05:50:12",
            "2026-11-09T05:50Z",
            "2026-11-09T05:50:12+00:00",
            "2026-11-09T15:50:12+10:00",
            "2026-11-09T05:50:12.0Z",
            "1969-12-31T23:59:59Z",
            "soon",
            "",
        ] {
            assert_eq!(
                looks_like_date(s),
                normalise_date(s).as_deref() == Some(s),
                "{s:?}"
            );
        }
    }

    #[test]
    fn spans_are_whole_utc_periods() {
        let at = |s: &str| date_span(&format!("{s}T00:00:00Z")).unwrap().start;
        let span = |s: &str| date_span(s).unwrap();
        assert_eq!(
            span("2024"),
            DateSpan {
                start: at("2024-01-01"),
                end: at("2025-01-01")
            }
        );
        assert_eq!(
            span("2024-02"),
            DateSpan {
                start: at("2024-02-01"),
                end: at("2024-03-01")
            }
        );
        assert_eq!(
            span("2024-12"),
            DateSpan {
                start: at("2024-12-01"),
                end: at("2025-01-01")
            }
        );
        assert_eq!(
            span("2024-02-29"),
            DateSpan {
                start: at("2024-02-29"),
                end: at("2024-03-01")
            }
        );
        let t = at("2026-10-29") + 22 * 3600 + 40 * 60;
        assert_eq!(
            span("2026-10-30T09:40:00+11:00"),
            DateSpan {
                start: t,
                end: t + 1
            }
        );
    }

    /// One row per pair of precisions (year, month, day, date-time): how `a` stands against `b` in a comparison,
    /// and in the sort.
    const TABLE: &[(&str, &str, Ordering, Ordering)] = {
        use Ordering::{Equal as E, Greater as G, Less as L};
        &[
            ("2025", "2026", L, L),
            ("2025", "2025-02", E, L),
            ("2026", "2025-12-31", G, G),
            ("2025", "2025-01-01T00:00:00Z", E, L),
            ("2025-02", "2025", E, G),
            ("2025-01", "2025-02", L, L),
            ("2025-01", "2025-01-01", E, L),
            ("2025-01", "2025-01-31T23:59:59Z", E, L),
            ("2024-12-31", "2025", L, L),
            ("2025-01-15", "2025-01", E, G),
            ("2025-01-15", "2025-01-15", E, E),
            ("2026-10-30", "2026-10-29T22:40:00Z", G, G),
            ("2025-12-31T23:59:59Z", "2026", L, L),
            ("2025-01-01T00:00:00Z", "2025-01", E, G),
            ("2026-10-29T22:40:00Z", "2026-10-29", E, G),
            ("2026-10-29T22:40:00Z", "2026-10-29T22:40:01Z", L, L),
        ]
    };

    fn span_table_misses(relation: impl Fn(&str, &str) -> Ordering) -> usize {
        TABLE
            .iter()
            .filter(|(a, b, want, _)| relation(a, b) != *want)
            .count()
    }

    #[test]
    fn the_ordering_table_holds_and_text_compare_fails_it() {
        let span = |s: &str| date_span(s).unwrap();
        for (a, b, relation, order) in TABLE {
            assert_eq!(span(a).relation(&span(b)), *relation, "{a} vs {b}");
            assert_eq!(span(b).relation(&span(a)), relation.reverse(), "{b} vs {a}");
            assert_eq!(span(a).order(&span(b)), *order, "{a} vs {b}");
            // On normalised values the sort is their text order, so ORDER BY over stored values is unchanged.
            assert_eq!(a.cmp(b), *order, "{a} vs {b}");
        }
        assert_eq!(span_table_misses(|a, b| span(a).relation(&span(b))), 0);
        // The plant: comparing as text, as umx did before, gets the mixed-precision rows wrong.
        assert_eq!(span_table_misses(|a, b| a.cmp(b)), 8);
        assert_eq!(
            span("2026-10-30T09:40:00+11:00").relation(&span("2026-10-29")),
            Ordering::Equal
        );
    }
}

#[cfg(test)]
mod function_tests {
    use super::*;

    fn span(s: &str) -> DateSpan {
        date_span(s).unwrap_or_else(|| panic!("{s} is a date"))
    }

    fn unit(name: &str) -> Unit {
        Unit::parse(name).unwrap_or_else(|| panic!("{name} is a unit"))
    }

    #[test]
    fn precision_is_the_span_a_value_covers() {
        for (s, want) in [
            ("2025", Precision::Year),
            ("2024", Precision::Year),
            ("1066", Precision::Year),
            ("2025-02", Precision::Month),
            ("2024-02", Precision::Month),
            ("2025-01", Precision::Month),
            ("2025-01-01", Precision::Day),
            ("1066-10-14", Precision::Day),
            ("2026-10-29T22:40:00Z", Precision::Second),
            ("2026-10-30T09:40:00+11:00", Precision::Second),
        ] {
            assert_eq!(span(s).precision(), want, "{s}");
        }
    }

    #[test]
    fn units_parse_in_any_case_and_only_by_name() {
        assert_eq!(Unit::parse("YEAR"), Some(Unit::Year));
        assert_eq!(Unit::parse("DayOfWeek"), Some(Unit::DayOfWeek));
        for bad in ["fortnight", "days", "", "isodow", "epoch"] {
            assert_eq!(Unit::parse(bad), None, "{bad}");
        }
        assert!(!Unit::DayOfWeek.is_cut());
        assert!(
            Unit::ALL
                .iter()
                .all(|&(_, u)| u == Unit::DayOfWeek || u.is_cut())
        );
    }

    /// LadybugDB 0.22.3 gave the same numbers for the day rows (plan 201's probes); the rest are umx's own: a
    /// date-time in UTC, and null for a part finer than the value.
    #[test]
    fn date_part_reads_the_utc_calendar_and_is_null_finer_than_the_value() {
        for (u, s, want) in [
            ("year", "2026-12-01", Some(2026)),
            ("quarter", "2026-12-01", Some(4)),
            ("month", "2026-12-01", Some(12)),
            ("week", "2026-12-01", Some(49)),
            ("day", "2026-12-01", Some(1)),
            ("dayofweek", "2026-12-01", Some(2)),
            ("dayofweek", "2026-12-06", Some(0)),
            ("week", "2027-01-01", Some(53)),
            ("week", "2026-12-31", Some(53)),
            ("week", "2026-01-01", Some(1)),
            ("week", "2024-12-30", Some(1)),
            ("year", "1066-10-14", Some(1066)),
            ("week", "1066-10-14", Some(41)),
            ("dayofweek", "1066-10-14", Some(0)),
            ("hour", "2026-10-29T22:40:05Z", Some(22)),
            ("minute", "2026-10-29T22:40:05Z", Some(40)),
            ("second", "2026-10-29T22:40:05Z", Some(5)),
            ("day", "2026-10-30T09:40:00+11:00", Some(29)),
            ("hour", "2026-10-30T09:40:00-05:30", Some(15)),
            ("year", "2025", Some(2025)),
            ("month", "2025", None),
            ("quarter", "2025-05", Some(2)),
            ("month", "2025-01", Some(1)),
            ("week", "2025-01", None),
            ("day", "2025-01", None),
            ("dayofweek", "2025-01", None),
            ("hour", "2026-12-01", None),
            ("second", "2026-12-01", None),
        ] {
            assert_eq!(date_part(unit(u), span(s)), want, "date_part({u}, {s})");
        }
    }

    #[test]
    fn date_trunc_gives_the_bucket_start_and_is_null_finer_than_the_value() {
        for (u, s, want) in [
            ("year", "2026-12-17", Some("2026-01-01")),
            ("quarter", "2026-12-17", Some("2026-10-01")),
            ("month", "2026-12-17", Some("2026-12-01")),
            ("week", "2026-12-17", Some("2026-12-14")),
            ("week", "2026-12-13", Some("2026-12-07")),
            ("week", "2027-01-01", Some("2026-12-28")),
            ("day", "2026-12-17", Some("2026-12-17")),
            ("day", "2026-10-29T22:40:00Z", Some("2026-10-29")),
            ("week", "2026-10-29T22:40:00Z", Some("2026-10-26")),
            ("day", "2026-10-30T09:40:00+11:00", Some("2026-10-29")),
            ("hour", "2026-10-29T22:40:59Z", Some("2026-10-29T22:00:00Z")),
            (
                "minute",
                "2026-10-29T22:40:59Z",
                Some("2026-10-29T22:40:00Z"),
            ),
            (
                "second",
                "2026-10-29T22:40:59Z",
                Some("2026-10-29T22:40:59Z"),
            ),
            ("month", "1066-10-14", Some("1066-10-01")),
            ("month", "2025-01", Some("2025-01-01")),
            ("year", "2025-01", Some("2025-01-01")),
            ("year", "2025", Some("2025-01-01")),
            ("month", "2025", None),
            ("week", "2025-01", None),
            ("day", "2025-01", None),
            ("hour", "2026-12-01", None),
            ("dayofweek", "2026-12-01", None),
        ] {
            assert_eq!(
                date_trunc(unit(u), span(s)).as_deref(),
                want,
                "date_trunc({u}, {s})"
            );
        }
    }

    /// Boundaries crossed (DuckDB's `date_diff`), not complete units, except `week`: whole 7-day spans, as DuckDB.
    /// The sign says which way.
    #[test]
    fn date_diff_counts_unit_boundaries_from_a_to_b() {
        for (u, a, b, want) in [
            ("month", "2025-01-31", "2025-02-01", Some(1)),
            ("month", "2025-02-01", "2025-01-31", Some(-1)),
            ("month", "2025-01-01", "2025-01-31", Some(0)),
            ("year", "2025-12-31", "2026-01-01", Some(1)),
            ("year", "2025-01-01", "2025-12-31", Some(0)),
            ("quarter", "2025-03-31", "2025-04-01", Some(1)),
            ("quarter", "2024-12-31", "2026-01-01", Some(5)),
            ("month", "2025-03-15", "2024-12-31", Some(-3)),
            ("week", "2026-12-13", "2026-12-14", Some(0)),
            ("week", "2026-12-14", "2026-12-21", Some(1)),
            ("week", "2026-12-01", "2026-12-14", Some(1)),
            ("week", "2026-12-01", "2026-12-15", Some(2)),
            ("week", "2026-12-14", "2026-12-01", Some(-1)),
            ("week", "2026-12-14", "2026-12-20", Some(0)),
            ("week", "2026-10-29T22:40:00Z", "2026-11-05", Some(1)),
            ("week", "2025-01", "2025-03-01", None),
            ("day", "2026-11-01", "2026-12-01", Some(30)),
            ("day", "2026-12-01", "2026-11-01", Some(-30)),
            ("day", "2026-12-01", "2026-12-01", Some(0)),
            ("day", "1066-10-14", "2026-12-01", Some(350_681)),
            (
                "day",
                "2025-01-01T23:59:59Z",
                "2025-01-02T00:00:00Z",
                Some(1),
            ),
            ("day", "2026-10-30T09:40:00+11:00", "2026-10-30", Some(1)),
            (
                "hour",
                "2026-10-29T22:59:59Z",
                "2026-10-29T23:00:00Z",
                Some(1),
            ),
            (
                "minute",
                "2026-10-29T22:40:00Z",
                "2026-10-29T22:39:59Z",
                Some(-1),
            ),
            (
                "second",
                "2026-10-29T22:40:00Z",
                "2026-10-29T22:40:30Z",
                Some(30),
            ),
            ("month", "2025-01", "2025-03", Some(2)),
            ("year", "2025", "2027-06", Some(2)),
            ("day", "2025-01", "2025-03", None),
            ("month", "2025", "2025-03", None),
            ("hour", "2026-12-01", "2026-12-01T10:00:00Z", None),
            ("dayofweek", "2026-12-01", "2026-12-02", None),
        ] {
            assert_eq!(
                date_diff(unit(u), span(a), span(b)),
                want,
                "date_diff({u}, {a}, {b})"
            );
        }
    }

    #[test]
    fn date_and_timestamp_text() {
        for (s, date, timestamp) in [
            ("2025", Some("2025"), None),
            ("2025-01", Some("2025-01"), None),
            (
                "2026-12-01",
                Some("2026-12-01"),
                Some("2026-12-01T00:00:00Z"),
            ),
            (
                "2026-10-29T22:40:00Z",
                Some("2026-10-29"),
                Some("2026-10-29T22:40:00Z"),
            ),
            (
                "2026-10-30T09:40:00+11:00",
                Some("2026-10-29"),
                Some("2026-10-29T22:40:00Z"),
            ),
            (
                "2026-10-29 17:10:00.75-05:30",
                Some("2026-10-29"),
                Some("2026-10-29T22:40:00Z"),
            ),
            // Before 1970 too (F211): a day's UTC midnight, and a date-time as it is.
            (
                "1066-10-14",
                Some("1066-10-14"),
                Some("1066-10-14T00:00:00Z"),
            ),
            (
                "1969-07-20T20:17:40Z",
                Some("1969-07-20"),
                Some("1969-07-20T20:17:40Z"),
            ),
        ] {
            assert_eq!(date_text(span(s)).as_deref(), date, "date({s})");
            assert_eq!(
                timestamp_text(span(s)).as_deref(),
                timestamp,
                "timestamp({s})"
            );
        }
    }
}
