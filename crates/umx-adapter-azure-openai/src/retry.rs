// Copyright (c) 2026 AI-Substrate. MIT. Adapted from flowspace3 crates/providers/src/retry.rs,
// ported to blocking sleeps.

//! Retry only transient statuses (429, 500, 502, 503, 504); everything else fails on first sight.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) fn is_transient(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

/// The wait the service asked for, as of `now`: Azure's `retry-after-ms` first, then
/// `Retry-After` as delta-seconds or an HTTP-date (`Sun, 06 Nov 1994 08:49:37 GMT`). A date
/// already gone is no wait (clock skew, not a failure). Junk is no advice; the backoff decides.
pub(crate) fn retry_after(
    retry_after_ms: Option<&str>,
    retry_after: Option<&str>,
    now: SystemTime,
) -> Option<Duration> {
    if let Some(ms) = retry_after_ms.and_then(|v| v.trim().parse::<u64>().ok()) {
        return Some(Duration::from_millis(ms));
    }
    let header = retry_after?.trim();
    if let Ok(seconds) = header.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = UNIX_EPOCH + Duration::from_secs(http_date(header)?);
    Some(at.duration_since(now).unwrap_or(Duration::ZERO))
}

/// Epoch seconds for an IMF-fixdate, the HTTP-date form senders must use.
fn http_date(text: &str) -> Option<u64> {
    let (_weekday, rest) = text.split_once(", ")?;
    let mut parts = rest.split(' ');
    let (day, month, year, time, zone) = (
        parts.next()?,
        parts.next()?,
        parts.next()?,
        parts.next()?,
        parts.next()?,
    );
    if zone != "GMT" || parts.next().is_some() {
        return None;
    }
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = MONTHS.iter().position(|m| *m == month)? as i64 + 1;
    let mut hms = time.splitn(3, ':').map(|p| p.parse::<i64>().ok());
    let (hh, mm, ss) = (hms.next()??, hms.next()??, hms.next()??);
    utc_epoch(year.parse().ok()?, month, day.parse().ok()?, hh, mm, ss)
}

/// Epoch seconds for a UTC civil time (Howard Hinnant's days-from-civil), or `None` if out of range.
pub(crate) fn utc_epoch(y: i64, m: i64, d: i64, hh: i64, mm: i64, ss: i64) -> Option<u64> {
    if !(1..=12).contains(&m)
        || !(1..=31).contains(&d)
        || !(0..24).contains(&hh)
        || !(0..60).contains(&mm)
        || !(0..=60).contains(&ss)
    {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Total attempts including the first. `1` disables retrying.
    pub attempts: u32,
    /// The first backoff; each later one doubles.
    pub base: Duration,
    /// The longest any single sleep may be, even when the service asks for more.
    pub cap: Duration,
}

impl Default for RetryPolicy {
    /// Three attempts, half a second, twenty-second ceiling (Flowspace's measured choice).
    fn default() -> Self {
        Self {
            attempts: 3,
            base: Duration::from_millis(500),
            cap: Duration::from_secs(20),
        }
    }
}

impl RetryPolicy {
    /// The wait before attempt `index + 1`. The service's advice wins, capped; otherwise
    /// exponential backoff with full jitter, so many callers do not wake together.
    pub(crate) fn wait(&self, index: u32, advised: Option<Duration>) -> Duration {
        if let Some(advised) = advised {
            return advised.min(self.cap);
        }
        let backoff = self
            .base
            .saturating_mul(1u32 << index.min(16))
            .min(self.cap);
        backoff.mul_f64(jitter())
    }
}

/// A fraction in `[0, 1)` from the clock's low bits; nothing here is security-sensitive.
fn jitter() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    f64::from(nanos) / 1e9
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_transient_statuses_are_retried() {
        for code in [429, 500, 502, 503, 504] {
            assert!(is_transient(code), "{code}");
        }
        for code in [300, 302, 400, 401, 403, 404, 408, 422, 501] {
            assert!(!is_transient(code), "{code}");
        }
    }

    #[test]
    fn retry_after_prefers_ms_then_seconds_or_an_http_date() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        assert_eq!(
            retry_after(Some("1500"), Some("9"), now),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(
            retry_after(None, Some(" 5 "), now),
            Some(Duration::from_secs(5))
        );
        // 2001-09-09T01:47:40Z is 1_000_000_060 (checked with Python's datetime).
        assert_eq!(
            retry_after(None, Some("Sun, 09 Sep 2001 01:47:40 GMT"), now),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            retry_after(Some("soon"), Some("Sun, 09 Sep 2001 01:47:40 GMT"), now),
            Some(Duration::from_secs(60)),
            "a junk ms header falls through to Retry-After"
        );
    }

    #[test]
    fn an_elapsed_http_date_means_no_wait_and_junk_means_no_advice() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        assert_eq!(
            retry_after(None, Some("Sun, 06 Nov 1994 08:49:37 GMT"), now),
            Some(Duration::ZERO)
        );
        assert_eq!(
            http_date("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(784_111_777)
        );
        for junk in [
            "soon",
            "",
            "Sun, 06 Nov 1994 08:49:37 PST",
            "Sun, 06 Foo 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 25:49:37 GMT",
        ] {
            assert_eq!(retry_after(None, Some(junk), now), None, "{junk}");
        }
        assert_eq!(retry_after(None, None, now), None);
    }

    #[test]
    fn advice_wins_but_never_past_the_cap() {
        let policy = RetryPolicy::default();
        assert_eq!(
            policy.wait(0, Some(Duration::from_secs(7))),
            Duration::from_secs(7)
        );
        assert_eq!(policy.wait(0, Some(Duration::from_secs(3600))), policy.cap);
    }

    #[test]
    fn backoff_grows_is_capped_and_never_overflows() {
        let policy = RetryPolicy {
            attempts: 8,
            base: Duration::from_secs(1),
            cap: Duration::from_secs(10),
        };
        for (index, ceiling) in [(0, 1), (1, 2), (2, 4), (3, 8), (4, 10), (64, 10)] {
            assert!(
                policy.wait(index, None) <= Duration::from_secs(ceiling),
                "{index}"
            );
        }
    }
}
