//! The TZ Rule: a POSIX TZ string that converts UTC to the Wall Clock's local
//! time, DST included. See ADR-0003 for why it's a POSIX rule, and why only
//! this subset of the grammar is accepted:
//!
//! ```text
//! std offset [dst [offset] [,Mm.w.d[/time],Mm.w.d[/time]]]
//! ```
//!
//! Names are three or more letters, or `<…>`-quoted letters, digits, `+` and
//! `-` (e.g. `<+0530>`). Offsets are `[+-]hh[:mm[:ss]]` hours *west* of UTC,
//! as POSIX has them. A transition `/time` is local wall time, and may be
//! negative or past 24h (to 167h), as the tz database's own POSIX strings
//! use. The `Jn` and `n` day forms are rejected.

use heapless::String;

use crate::wall_clock::{civil_from_days, days_from_civil};

/// Longest TZ Rule accepted, comfortably over any real zone's (the tz
/// database's longest `M`-form rules are around 35 characters). Bounds the
/// `TIME` reply, which echoes the rule.
pub const MAX_TZ_LEN: usize = 48;

const SECS_PER_HOUR: i64 = 3600;
const SECS_PER_DAY: i64 = 86_400;

/// Largest `hh` in a UTC offset (POSIX) and in a transition `/time`
/// (RFC 8536's extension, which the tz database relies on).
const MAX_OFFSET_HOURS: i64 = 24;
const MAX_TIME_HOURS: i64 = 167;

/// A transition's `/time` when it has none: 02:00.
const DEFAULT_TIME: i64 = 2 * SECS_PER_HOUR;

/// What DST defaults to when a rule names a DST zone but no transitions,
/// e.g. `EST5EDT`: the US rules, as glibc does.
const DEFAULT_START: Transition = Transition {
    month: 3,
    week: 2,
    weekday: 0,
    time: DEFAULT_TIME,
};
const DEFAULT_END: Transition = Transition {
    month: 11,
    week: 1,
    weekday: 0,
    time: DEFAULT_TIME,
};

/// A parsed TZ Rule. Keeps the text it was parsed from, which is what gets
/// persisted and echoed by `TIME`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TzRule {
    text: String<MAX_TZ_LEN>,
    /// Standard time's offset, in seconds *east* of UTC.
    std_offset: i64,
    dst: Option<Dst>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Dst {
    /// Seconds east of UTC.
    offset: i64,
    /// In standard local time.
    start: Transition,
    /// In DST local time.
    end: Transition,
}

/// `Mm.w.d/time`: weekday `d` (0 = Sunday) of week `w` (5 = last) of month
/// `m`, at `time` seconds past that day's local midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Transition {
    month: u8,
    week: u8,
    weekday: u8,
    time: i64,
}

impl TzRule {
    /// `UTC0`: the rule in force until a `TZ` is ever sent.
    pub fn utc() -> Self {
        let mut text = String::new();
        // Can't fail: 4 bytes.
        let _ = text.push_str("UTC0");
        TzRule {
            text,
            std_offset: 0,
            dst: None,
        }
    }

    /// Parses a POSIX TZ string in the accepted subset (see the module
    /// docs), or `None` if it's malformed, uses a `J`/`n` day form, or is
    /// longer than [`MAX_TZ_LEN`].
    pub fn parse(s: &str) -> Option<Self> {
        let mut text = String::new();
        text.push_str(s).ok()?;
        let mut p = Parser(s.as_bytes());

        p.name()?;
        let std_offset = -p.hms(MAX_OFFSET_HOURS)?;
        let dst = if p.at_end() {
            None
        } else {
            p.name()?;
            let offset = match p.peek() {
                None | Some(b',') => std_offset + SECS_PER_HOUR,
                Some(_) => -p.hms(MAX_OFFSET_HOURS)?,
            };
            let (start, end) = if p.at_end() {
                (DEFAULT_START, DEFAULT_END)
            } else {
                p.expect(b',')?;
                let start = p.transition()?;
                p.expect(b',')?;
                (start, p.transition()?)
            };
            Some(Dst { offset, start, end })
        };
        if !p.at_end() {
            return None;
        }
        Some(TzRule {
            text,
            std_offset,
            dst,
        })
    }

    /// The rule as it was sent.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Local time's offset from UTC at `utc_secs` (Unix seconds), in seconds
    /// east of UTC.
    pub fn utc_offset(&self, utc_secs: u64) -> i32 {
        let utc = utc_secs as i64;
        let offset = match &self.dst {
            None => self.std_offset,
            Some(dst) => {
                // Which year's transitions apply is decided in standard local
                // time, so a southern-hemisphere DST spanning the new year
                // flips years at local midnight, not UTC's.
                let local_days = (utc + self.std_offset).div_euclid(SECS_PER_DAY);
                let year = civil_from_days(local_days.max(0) as u64).0;
                let start = dst.start.local_secs(year) - self.std_offset;
                let end = dst.end.local_secs(year) - dst.offset;
                let in_dst = if start < end {
                    start <= utc && utc < end
                } else {
                    utc < end || start <= utc
                };
                if in_dst { dst.offset } else { self.std_offset }
            }
        };
        offset as i32
    }

    /// `utc_ms` (Unix milliseconds) shifted to local wall time — what the
    /// Wall Clock's face shows.
    pub fn local_ms(&self, utc_ms: u64) -> u64 {
        let offset_ms = i64::from(self.utc_offset(utc_ms / 1000)) * 1000;
        utc_ms.saturating_add_signed(offset_ms)
    }
}

impl Transition {
    /// When this transition falls in `year`, as seconds since the Unix
    /// epoch in the local time it's written in.
    fn local_secs(&self, year: u64) -> i64 {
        let month = u64::from(self.month);
        let first = days_from_civil(year, month, 1);
        // 1970-01-01 was a Thursday.
        let first_weekday = (first + 4) % 7;
        let mut day = (u64::from(self.weekday) + 7 - first_weekday) % 7
            + 7 * (u64::from(self.week) - 1);
        let (next_year, next_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
        let month_len = days_from_civil(next_year, next_month, 1) - first;
        while day >= month_len {
            day -= 7;
        }
        (first + day) as i64 * SECS_PER_DAY + self.time
    }
}

/// A cursor over the TZ string's bytes.
struct Parser<'a>(&'a [u8]);

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.0.first().copied()
    }

    fn at_end(&self) -> bool {
        self.0.is_empty()
    }

    fn bump(&mut self) -> Option<u8> {
        let (&b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(b)
    }

    fn expect(&mut self, want: u8) -> Option<()> {
        (self.bump()? == want).then_some(())
    }

    /// Consumes bytes while `pred` holds, returning how many.
    fn take_while(&mut self, pred: impl Fn(u8) -> bool) -> usize {
        let n = self.0.iter().take_while(|&&b| pred(b)).count();
        self.0 = &self.0[n..];
        n
    }

    /// A zone name: three or more letters, or `<…>` around three or more
    /// letters, digits, `+` or `-`. The name itself is never needed.
    fn name(&mut self) -> Option<()> {
        let len = if self.peek()? == b'<' {
            self.bump();
            let n = self.take_while(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-');
            self.expect(b'>')?;
            n
        } else {
            self.take_while(|b| b.is_ascii_alphabetic())
        };
        (len >= 3).then_some(())
    }

    /// `[+-]hh[:mm[:ss]]` as signed seconds, with `hh` at most `max_hours`
    /// (and so at most as many digits as `max_hours` has).
    fn hms(&mut self, max_hours: i64) -> Option<i64> {
        let sign = match self.peek()? {
            b'-' => {
                self.bump();
                -1
            }
            b'+' => {
                self.bump();
                1
            }
            _ => 1,
        };
        let hours = self.number(if max_hours < 100 { 2 } else { 3 })?;
        if hours > max_hours {
            return None;
        }
        let mut secs = hours * SECS_PER_HOUR;
        for unit in [60, 1] {
            if self.peek() != Some(b':') {
                break;
            }
            self.bump();
            let n = self.number(2)?;
            if n > 59 {
                return None;
            }
            secs += n * unit;
        }
        Some(sign * secs)
    }

    /// One to `max_digits` decimal digits.
    fn number(&mut self, max_digits: usize) -> Option<i64> {
        let digits = &self.0[..self.0.iter().take_while(|b| b.is_ascii_digit()).count()];
        if digits.is_empty() || digits.len() > max_digits {
            return None;
        }
        self.0 = &self.0[digits.len()..];
        Some(digits.iter().fold(0, |n, &d| n * 10 + i64::from(d - b'0')))
    }

    /// `Mm.w.d[/time]` — the only transition form accepted.
    fn transition(&mut self) -> Option<Transition> {
        self.expect(b'M')?;
        let month = self.number(2)?;
        self.expect(b'.')?;
        let week = self.number(1)?;
        self.expect(b'.')?;
        let weekday = self.number(1)?;
        if !(1..=12).contains(&month) || !(1..=5).contains(&week) || weekday > 6 {
            return None;
        }
        let time = if self.peek() == Some(b'/') {
            self.bump();
            self.hms(MAX_TIME_HOURS)?
        } else {
            DEFAULT_TIME
        };
        Some(Transition {
            month: month as u8,
            week: week as u8,
            weekday: weekday as u8,
            time,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unix seconds at `y-m-d hh:mm:ss` UTC.
    fn utc(y: u64, m: u64, d: u64, hh: i64, mm: i64, ss: i64) -> u64 {
        (days_from_civil(y, m, d) as i64 * SECS_PER_DAY + hh * 3600 + mm * 60 + ss) as u64
    }

    fn rule(s: &str) -> TzRule {
        TzRule::parse(s).unwrap_or_else(|| panic!("{s} should parse"))
    }

    const H: i32 = 3600;

    #[test]
    fn utc_default() {
        let r = TzRule::utc();
        assert_eq!(r.as_str(), "UTC0");
        assert_eq!(r.utc_offset(utc(2026, 7, 1, 12, 0, 0)), 0);
    }

    #[test]
    fn fixed_offset_is_west_positive() {
        assert_eq!(rule("EST5").utc_offset(utc(2026, 7, 1, 12, 0, 0)), -5 * H);
        assert_eq!(rule("JST-9").utc_offset(utc(2026, 1, 1, 0, 0, 0)), 9 * H);
        assert_eq!(rule("ABC+3").utc_offset(0), -3 * H);
    }

    #[test]
    fn offset_minutes_and_seconds() {
        assert_eq!(rule("NST3:30").utc_offset(0), -(3 * H + 30 * 60));
        assert_eq!(rule("XYZ-5:45:15").utc_offset(0), 5 * H + 45 * 60 + 15);
    }

    #[test]
    fn quoted_names() {
        assert_eq!(rule("<+0530>-5:30").utc_offset(0), 5 * H + 30 * 60);
        let nuuk = rule("<-02>2<-01>,M3.5.0/-1,M10.5.0/0");
        assert_eq!(nuuk.utc_offset(utc(2026, 1, 15, 12, 0, 0)), -2 * H);
        assert_eq!(nuuk.utc_offset(utc(2026, 7, 15, 12, 0, 0)), -H);
    }

    #[test]
    fn keeps_original_text() {
        assert_eq!(rule("PST8PDT,M3.2.0,M11.1.0").as_str(), "PST8PDT,M3.2.0,M11.1.0");
    }

    #[test]
    fn spring_forward_gap() {
        // 2026-03-08 02:00 PST (10:00Z) jumps to 03:00 PDT: 02:xx never shows.
        let la = rule("PST8PDT,M3.2.0,M11.1.0");
        let before = utc(2026, 3, 8, 9, 59, 59);
        let at = utc(2026, 3, 8, 10, 0, 0);
        assert_eq!(la.utc_offset(before), -8 * H);
        assert_eq!(la.utc_offset(at), -7 * H);
        assert_eq!(la.local_ms(before * 1000), utc(2026, 3, 8, 1, 59, 59) * 1000);
        assert_eq!(la.local_ms(at * 1000), utc(2026, 3, 8, 3, 0, 0) * 1000);
    }

    #[test]
    fn fall_back_overlap() {
        // 2026-11-01 02:00 PDT (09:00Z) falls back to 01:00 PST: 01:xx shows
        // twice, an hour of UTC apart.
        let la = rule("PST8PDT,M3.2.0,M11.1.0");
        let first = utc(2026, 11, 1, 8, 30, 0);
        let second = utc(2026, 11, 1, 9, 30, 0);
        assert_eq!(la.utc_offset(utc(2026, 11, 1, 8, 59, 59)), -7 * H);
        assert_eq!(la.utc_offset(utc(2026, 11, 1, 9, 0, 0)), -8 * H);
        assert_eq!(la.local_ms(first * 1000), utc(2026, 11, 1, 1, 30, 0) * 1000);
        assert_eq!(la.local_ms(second * 1000), utc(2026, 11, 1, 1, 30, 0) * 1000);
    }

    #[test]
    fn southern_hemisphere() {
        // Sydney: DST from the first Sunday of October 02:00 AEST to the
        // first Sunday of April 03:00 AEDT, spanning the new year.
        let syd = rule("AEST-10AEDT,M10.1.0,M4.1.0/3");
        assert_eq!(syd.utc_offset(utc(2026, 1, 15, 0, 0, 0)), 11 * H);
        assert_eq!(syd.utc_offset(utc(2026, 7, 15, 0, 0, 0)), 10 * H);
        assert_eq!(syd.utc_offset(utc(2026, 12, 31, 13, 30, 0)), 11 * H);
        // 2026-04-05 03:00 AEDT = 2026-04-04 16:00Z.
        assert_eq!(syd.utc_offset(utc(2026, 4, 4, 15, 59, 59)), 11 * H);
        assert_eq!(syd.utc_offset(utc(2026, 4, 4, 16, 0, 0)), 10 * H);
        // 2026-10-04 02:00 AEST = 2026-10-03 16:00Z.
        assert_eq!(syd.utc_offset(utc(2026, 10, 3, 15, 59, 59)), 10 * H);
        assert_eq!(syd.utc_offset(utc(2026, 10, 3, 16, 0, 0)), 11 * H);
    }

    #[test]
    fn non_default_transition_time() {
        // EU: both transitions at 01:00 UTC, written as local times.
        let paris = rule("CET-1CEST,M3.5.0,M10.5.0/3");
        // 2026-03-29 02:00 CET = 01:00Z; 2026-10-25 03:00 CEST = 01:00Z.
        assert_eq!(paris.utc_offset(utc(2026, 3, 29, 0, 59, 59)), H);
        assert_eq!(paris.utc_offset(utc(2026, 3, 29, 1, 0, 0)), 2 * H);
        assert_eq!(paris.utc_offset(utc(2026, 10, 25, 0, 59, 59)), 2 * H);
        assert_eq!(paris.utc_offset(utc(2026, 10, 25, 1, 0, 0)), H);
    }

    #[test]
    fn transition_time_past_24h() {
        // Israel: DST starts Friday before the last Sunday of March at 02:00,
        // written as 26:00 on the fourth Thursday — 2026-03-27 02:00 IST.
        let jlm = rule("IST-2IDT,M3.4.4/26,M10.5.0");
        assert_eq!(jlm.utc_offset(utc(2026, 3, 26, 23, 59, 59)), 2 * H);
        assert_eq!(jlm.utc_offset(utc(2026, 3, 27, 0, 0, 0)), 3 * H);
    }

    #[test]
    fn transition_time_with_minutes_and_seconds() {
        let r = rule("AAA0BBB,M6.1.0/1:30:15,M9.1.0");
        // 2026-06-07 is the first Sunday of June.
        assert_eq!(r.utc_offset(utc(2026, 6, 7, 1, 30, 14)), 0);
        assert_eq!(r.utc_offset(utc(2026, 6, 7, 1, 30, 15)), H);
    }

    #[test]
    fn week_five_means_last() {
        // March 2026's Sundays are the 1st, 8th, 15th, 22nd and 29th: week 5
        // is the 29th. February 2026 has no fifth Sunday, so week 5 is the
        // fourth (the 22nd).
        let r = rule("AAA0BBB,M2.5.0,M3.5.0");
        assert_eq!(r.utc_offset(utc(2026, 2, 21, 23, 59, 59)), 0);
        assert_eq!(r.utc_offset(utc(2026, 2, 22, 2, 0, 0)), H);
        // Ends 2026-03-29 02:00 BBB = 01:00Z.
        assert_eq!(r.utc_offset(utc(2026, 3, 29, 0, 59, 59)), H);
        assert_eq!(r.utc_offset(utc(2026, 3, 29, 1, 0, 0)), 0);
    }

    #[test]
    fn dst_offset_explicit() {
        // Lord Howe: half-hour DST.
        let r = rule("<+1030>-10:30<+11>-11,M10.1.0,M4.1.0");
        assert_eq!(r.utc_offset(utc(2026, 7, 1, 0, 0, 0)), 10 * H + 30 * 60);
        assert_eq!(r.utc_offset(utc(2026, 1, 1, 0, 0, 0)), 11 * H);
    }

    #[test]
    fn dst_without_rules_uses_us_rules() {
        let r = rule("EST5EDT");
        assert_eq!(r.utc_offset(utc(2026, 1, 15, 12, 0, 0)), -5 * H);
        assert_eq!(r.utc_offset(utc(2026, 7, 15, 12, 0, 0)), -4 * H);
    }

    #[test]
    fn rejects_day_of_year_forms() {
        assert_eq!(TzRule::parse("<+0330>-3:30<+0430>,J79/24,J263/24"), None);
        assert_eq!(TzRule::parse("AAA0BBB,J60,J300"), None);
        assert_eq!(TzRule::parse("AAA0BBB,60,300"), None);
        assert_eq!(TzRule::parse("AAA0BBB,M3.2.0,J300"), None);
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "",
            "UTC",
            "0",
            "AB0",
            "A1B0",
            "<AB>0",
            "<ABC0",
            "<A_C>0",
            "UTC+",
            "UTC25",
            "UTC024",
            "UTC1:60",
            "UTC1:00:60",
            "UTC1:",
            "UTC1x",
            "UTC 0",
            "AAA0BB",
            "AAA0BBB,",
            "AAA0BBB,M3.2.0",
            "AAA0BBB,M3.2.0,",
            "AAA0BBB,M3.2.0,M11.1.0,",
            "AAA0BBB,M0.2.0,M11.1.0",
            "AAA0BBB,M13.2.0,M11.1.0",
            "AAA0BBB,M3.0.0,M11.1.0",
            "AAA0BBB,M3.6.0,M11.1.0",
            "AAA0BBB,M3.2.7,M11.1.0",
            "AAA0BBB,M3.2,M11.1.0",
            "AAA0BBB,M3.2.0/168,M11.1.0",
            "AAA0BBB,M3.2.0/,M11.1.0",
            "AAA0,M3.2.0,M11.1.0",
            "AAA0BBB1x",
        ] {
            assert_eq!(TzRule::parse(bad), None, "{bad:?} should be rejected");
        }
    }

    #[test]
    fn rejects_too_long() {
        let long = std::format!("<{}>0", "A".repeat(MAX_TZ_LEN));
        assert_eq!(TzRule::parse(&long), None);
    }

    #[test]
    fn local_ms_keeps_sub_second() {
        assert_eq!(rule("JST-9").local_ms(1_500), 9 * 3_600_000 + 1_500);
    }
}
