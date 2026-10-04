//! Pure Wall Clock arithmetic on Unix time: what the grid shows for a given
//! local instant ([`face`]), when that next changes ([`ms_until_change`]), the
//! `TIME` reply's timestamp ([`iso8601`]), and the earliest time a Sync may
//! report ([`year_start`]). Shifting UTC to local time is `tz`'s job.

use core::fmt::Write as _;

use heapless::String;

const SECS_PER_DAY: u64 = 86_400;

/// How long the colon stays lit, then dark, in each UTC second.
const BLINK_MS: u64 = 500;

/// What the grid shows at one instant: `HH:MM` digits, and whether the
/// colon is lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Face {
    pub digits: [u8; 4],
    pub colon: bool,
}

/// The 24-hour `HH:MM` face at `local_ms` (Unix milliseconds shifted to local
/// time by [`crate::tz::TzRule::local_ms`]). The colon is lit for the first
/// half of each second and dark for the second half — the same phase as
/// UTC's, since TZ Rule offsets are whole seconds.
pub fn face(local_ms: u64) -> Face {
    let secs_of_day = (local_ms / 1000) % SECS_PER_DAY;
    let hh = secs_of_day / 3600;
    let mm = secs_of_day / 60 % 60;
    Face {
        digits: [
            (hh / 10) as u8,
            (hh % 10) as u8,
            (mm / 10) as u8,
            (mm % 10) as u8,
        ],
        colon: local_ms % 1000 < BLINK_MS,
    }
}

/// Milliseconds from `utc_ms` until [`face`] may next differ: the next
/// colon toggle (minute changes always land on one too).
pub fn ms_until_change(utc_ms: u64) -> u64 {
    BLINK_MS - utc_ms % BLINK_MS
}

/// `utc_secs` (Unix seconds) as ISO-8601 UTC, e.g. `2026-10-03T12:34:56Z`.
pub fn iso8601(utc_secs: u64) -> String<20> {
    let (year, month, day) = civil_from_days(utc_secs / SECS_PER_DAY);
    let secs_of_day = utc_secs % SECS_PER_DAY;
    let mut s = String::new();
    // Can't fail: four-digit years fit exactly, and a Sync can't produce a
    // later one before the year 10000.
    let _ = write!(
        s,
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        secs_of_day / 60 % 60,
        secs_of_day % 60
    );
    s
}

/// Unix seconds at 00:00:00 UTC on 1 January of the year containing
/// `unix_secs`.
pub fn year_start(unix_secs: u64) -> u64 {
    let (year, _, _) = civil_from_days(unix_secs / SECS_PER_DAY);
    days_from_civil(year, 1, 1) * SECS_PER_DAY
}

/// Days since 1970-01-01 → (year, month 1-12, day 1-31), proleptic
/// Gregorian. Howard Hinnant's `civil_from_days`, restricted to dates on or
/// after the Unix epoch.
pub(crate) fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// Inverse of [`civil_from_days`], for years from 1970.
pub(crate) fn days_from_civil(year: u64, month: u64, day: u64) -> u64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y / 400;
    let yoe = y % 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-03T12:34:56Z.
    const NOW: u64 = 1_791_030_896;

    #[test]
    fn face_shows_24h_hh_mm() {
        assert_eq!(face(NOW * 1000).digits, [1, 2, 3, 4]);
    }

    #[test]
    fn face_leading_zeros() {
        // 2026-10-03T07:05:00Z.
        let t = (NOW - 12 * 3600 - 34 * 60 - 56) + 7 * 3600 + 5 * 60;
        assert_eq!(face(t * 1000).digits, [0, 7, 0, 5]);
    }

    #[test]
    fn face_midnight_and_last_minute() {
        let midnight = NOW - NOW % SECS_PER_DAY;
        assert_eq!(face(midnight * 1000).digits, [0, 0, 0, 0]);
        assert_eq!(face((midnight - 1) * 1000).digits, [2, 3, 5, 9]);
    }

    #[test]
    fn colon_lit_first_half_of_each_second() {
        let ms = NOW * 1000;
        assert!(face(ms).colon);
        assert!(face(ms + 499).colon);
        assert!(!face(ms + 500).colon);
        assert!(!face(ms + 999).colon);
        assert!(face(ms + 1000).colon);
    }

    #[test]
    fn next_change_is_next_half_second() {
        let ms = NOW * 1000;
        assert_eq!(ms_until_change(ms), 500);
        assert_eq!(ms_until_change(ms + 1), 499);
        assert_eq!(ms_until_change(ms + 499), 1);
        assert_eq!(ms_until_change(ms + 500), 500);
        assert_eq!(ms_until_change(ms + 750), 250);
    }

    #[test]
    fn iso8601_formats_utc() {
        assert_eq!(iso8601(NOW).as_str(), "2026-10-03T12:34:56Z");
        assert_eq!(iso8601(0).as_str(), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn iso8601_leap_day_and_era_1() {
        // 2028-02-29T23:59:59Z, and NTP era 1's first second.
        assert_eq!(iso8601(1_835_481_599).as_str(), "2028-02-29T23:59:59Z");
        assert_eq!(iso8601(2_085_978_496).as_str(), "2036-02-07T06:28:16Z");
    }

    #[test]
    fn year_start_of_build_time() {
        assert_eq!(year_start(NOW), 1_767_225_600);
        assert_eq!(year_start(1_767_225_600), 1_767_225_600);
        assert_eq!(year_start(1_767_225_599), 1_735_689_600);
    }
}
