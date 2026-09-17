//! UTC timestamps, without a date crate.
//!
//! A snapshot needs one string per capture, so the two dozen kilobytes of a calendar crate (and a
//! new entry in the Rust 1.82 floor audit) would buy a civil-from-days conversion that fits on a
//! screen. The algorithm is Howard Hinnant's `civil_from_days`, valid for every date this will see.

use std::time::{SystemTime, UNIX_EPOCH};

/// The current time as RFC 3339 in UTC, to the second: `2026-09-17T20:15:00Z`.
pub fn now_rfc3339() -> String {
    rfc3339(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64))
}

/// Milliseconds since the epoch, which snapshot ids are built from.
pub fn now_millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

/// Seconds since the epoch as RFC 3339 in UTC.
pub fn rfc3339(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (h, m, s) = (time / 3600, (time % 3600) / 60, time % 60);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Days since 1970-01-01 as a civil date. Howard Hinnant's `civil_from_days`, which counts from an
/// era starting on 1 March so that the leap day falls at the end of a year.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_instants_render_as_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        // 2026-09-17T20:15:00Z, the day this was written.
        assert_eq!(rfc3339(1_789_676_100), "2026-09-17T20:15:00Z");
        // A leap day, which the March-first era exists to get right.
        assert_eq!(rfc3339(1_709_164_800), "2024-02-29T00:00:00Z");
        // The last second of a century that is not a leap year.
        assert_eq!(rfc3339(4_107_542_399), "2100-02-28T23:59:59Z");
    }

    #[test]
    fn now_is_sortable_and_this_century() {
        let now = now_rfc3339();
        assert!(now.starts_with("20"), "got {now}");
        assert!(now.ends_with('Z') && now.len() == 20, "got {now}");
        // Lexicographic order is chronological order, which the snapshot list relies on.
        assert!(rfc3339(0) < now);
    }
}
