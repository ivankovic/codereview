/*  This file is part of the stop-bots project.
 *
 *  Copyright (C) 2026 Marko Ivankovic
 *
 *  This program is free software: you can redistribute it and/or modify
 *  it under the terms of the GNU Affero General Public License as published
 *  by the Free Software Foundation, either version 3 of the License, or
 *  (at your option) any later version.
 *
 *  This program is distributed in the hope that it will be useful,
 *  but WITHOUT ANY WARRANTY; without even the implied warranty of
 *  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *  GNU Affero General Public License for more details.
 *
 *  You should have received a copy of the GNU Affero General License
 *  along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

//! When a log line says it happened, in Unix seconds.
//!
//! Every detector used to count over the whole file since the last
//! rotation, because nothing here read a timestamp. That is what made a
//! block's expiry meaningless: the lines that earned it were still in the
//! file when it lapsed, so the next pass added it again. Windows need
//! times, and these are the four spellings of a time the logs this project
//! reads actually carry:
//!
//! - NGINX's `$time_local`, `28/Sep/2026:06:33:01 +0000`, in the combined
//!   format and in a JSON format's `time_local`;
//! - ISO 8601, `2026-09-28T06:33:01+00:00`, in a JSON format's
//!   `time_iso8601`, at the start of an `auth.log` written by an rsyslog
//!   with high-precision timestamps (Debian 12's default), and at the start
//!   of `journalctl -o short-iso`, which writes the offset without a colon;
//! - the classic syslog prefix, `Sep 28 06:33:01`, which has neither a
//!   year nor a zone.
//!
//! The syslog one is the only one that takes judgement. It is local time,
//! so it is read with the host's offset at that moment; and it has no year,
//! so the year is the one that puts it closest to now without being more
//! than a day in the future. A line from `Dec 31 23:59:59` read on the 1st
//! of January is last year's, not next December's.
//!
//! No date library: the arithmetic is the civil-from-days algorithm below,
//! and the one thing it cannot do, the local zone offset, comes from
//! `localtime_r`, which libc already has.

/// Seconds since the Unix epoch for a proleptic Gregorian `year-month-day`
/// at midnight UTC. Howard Hinnant's `days_from_civil`, which is exact for
/// every date this will ever see.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = i64::from(month);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

/// UTC seconds for a wall-clock time, or `None` for a date that does not
/// exist (a 31st of September, an hour 24).
fn civil_to_unix(year: i64, month: u32, day: u32, h: u32, m: u32, s: u32) -> Option<i64> {
    if !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || h > 23
        || m > 59
        || s > 60
    {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + i64::from(h * 3600 + m * 60 + s))
}

fn month_number(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MONTHS.iter().position(|m| *m == name).map(|i| i as u32 + 1)
}

fn digits(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `+0000`, `-0530`, `+05:30` or `Z`, as seconds east of UTC.
fn zone_offset(text: &str) -> Option<i64> {
    if text == "Z" {
        return Some(0);
    }
    let (sign, rest) = match text.as_bytes().first()? {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => return None,
    };
    let rest: String = rest.chars().filter(|c| *c != ':').collect();
    if rest.len() != 4 {
        return None;
    }
    let hours = i64::from(digits(&rest[..2])?);
    let minutes = i64::from(digits(&rest[2..])?);
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 3600 + minutes * 60))
}

/// NGINX's `$time_local`: `28/Sep/2026:06:33:01 +0000`.
pub fn nginx_time_local(text: &str) -> Option<i64> {
    let (stamp, zone) = text.trim().split_once(' ')?;
    let mut parts = stamp.splitn(3, '/');
    let day = digits(parts.next()?)?;
    let month = month_number(parts.next()?)?;
    let rest = parts.next()?;
    let mut fields = rest.split(':');
    let year = i64::from(digits(fields.next()?)?);
    let h = digits(fields.next()?)?;
    let m = digits(fields.next()?)?;
    let s = digits(fields.next()?)?;
    if fields.next().is_some() {
        return None;
    }
    Some(civil_to_unix(year, month, day, h, m, s)? - zone_offset(zone)?)
}

/// ISO 8601 with a zone: `2026-09-28T06:33:01+00:00`, with or without a
/// colon in the offset, with or without fractional seconds, or with `Z`.
/// A time with no zone at all is refused rather than guessed at.
pub fn iso8601(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date, time) = text.split_once('T')?;
    let mut date_parts = date.split('-');
    let year = i64::from(digits(date_parts.next()?)?);
    let month = digits(date_parts.next()?)?;
    let day = digits(date_parts.next()?)?;
    if date_parts.next().is_some() {
        return None;
    }
    // The zone starts at the first `+`, `-` or `Z` after the clock.
    let zone_at = time.find(['+', '-', 'Z'])?;
    let (clock, zone) = time.split_at(zone_at);
    let clock = clock.split('.').next()?;
    let mut fields = clock.split(':');
    let h = digits(fields.next()?)?;
    let m = digits(fields.next()?)?;
    let s = digits(fields.next()?)?;
    if fields.next().is_some() {
        return None;
    }
    Some(civil_to_unix(year, month, day, h, m, s)? - zone_offset(zone)?)
}

/// The host's offset from UTC, in seconds east, at the instant `utc`.
///
/// From `localtime_r`, which reads `/etc/localtime` (or `$TZ`) the way the
/// syslog daemon that wrote the line did. `0` if it cannot say, which is
/// right on the servers this runs on: they are nearly all on UTC.
pub fn local_offset(utc: i64) -> i64 {
    // Typed by `localtime_r`'s parameter rather than by naming
    // `libc::time_t`, which the libc crate deprecates on musl — the target
    // the release binaries are built for — pending its move to 64 bits
    // there. On every 64-bit target this is already an i64.
    let time = utc as _;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are to live locals of the right type, and
    // `localtime_r` is the reentrant form that writes only into `tm`.
    let ok = unsafe { !libc::localtime_r(&time, &mut tm).is_null() };
    if ok {
        tm.tm_gmtoff
    } else {
        0
    }
}

/// The classic syslog timestamp, `Sep 28 06:33:01` (the day may be padded
/// with a space: `Sep  8`), which says neither the year nor the zone.
///
/// Read as local time through `offset` (seconds east of UTC at a given
/// instant; [`local_offset`] outside tests), in whichever year puts it no
/// more than a day after `now`. The day of slack is for a clock that was
/// slightly ahead when the line was written, not for a line from the
/// future.
pub fn syslog(text: &str, now: i64, offset: &dyn Fn(i64) -> i64) -> Option<i64> {
    let mut parts = text.split_whitespace();
    let month = month_number(parts.next()?)?;
    let day = digits(parts.next()?)?;
    let clock = parts.next()?;
    let mut fields = clock.split(':');
    let h = digits(fields.next()?)?;
    let m = digits(fields.next()?)?;
    let s = digits(fields.next()?.split('.').next()?)?;
    if fields.next().is_some() {
        return None;
    }
    // The current year in UTC is close enough to pick candidates from:
    // the offset is at most a day, and both neighbours are tried.
    let this_year = year_of(now);
    let mut best: Option<i64> = None;
    for year in [this_year + 1, this_year, this_year - 1] {
        // A 29th of February only exists in some years; skip the others.
        let Some(naive) = civil_to_unix(year, month, day, h, m, s) else {
            continue;
        };
        let at = naive - offset(naive);
        if at <= now + 86_400 && best.is_none_or(|b| at > b) {
            best = Some(at);
        }
    }
    best
}

/// The UTC calendar year `unix` falls in.
fn year_of(unix: i64) -> i64 {
    civil(unix).0
}

/// The UTC date and time of `unix`: `(year, month, day, hour, minute,
/// second)`. Hinnant's `civil_from_days`, the inverse of
/// [`days_from_civil`].
pub fn civil(unix: i64) -> (i64, u32, u32, u32, u32, u32) {
    let secs = unix.rem_euclid(86_400);
    let z = unix.div_euclid(86_400) + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (
        yoe + era * 400 + i64::from(month <= 2),
        month,
        day,
        (secs / 3600) as u32,
        (secs / 60 % 60) as u32,
        (secs % 60) as u32,
    )
}

/// When a syslog-style line (an `auth.log`, `secure`, or `journalctl -o
/// short-iso` output) says it was written: an ISO 8601 first field, or the
/// three-field classic prefix. `None` for a line with neither, such as the
/// bare message `journalctl -o cat` prints.
pub fn syslog_line(line: &str, now: i64, offset: &dyn Fn(i64) -> i64) -> Option<i64> {
    let first = line.split(' ').next()?;
    if first.len() >= 19 && first.as_bytes().get(4) == Some(&b'-') {
        return iso8601(first);
    }
    // `Sep 28 06:33:01 host ...` or `Sep  8 06:33:01 host ...`: the first
    // three whitespace-separated fields. Taken from a bounded prefix so a
    // long line is not scanned twice.
    let head = line.get(..16.min(line.len())).unwrap_or(line);
    syslog(head, now, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-28T06:33:01Z, the example in every doc comment here.
    const SEP_28: i64 = 1_790_577_181;

    fn utc(_: i64) -> i64 {
        0
    }

    #[test]
    fn the_epoch_arithmetic_agrees_with_a_known_date() {
        assert_eq!(civil_to_unix(1970, 1, 1, 0, 0, 0), Some(0));
        assert_eq!(civil_to_unix(2000, 3, 1, 0, 0, 0), Some(951_868_800));
        assert_eq!(civil_to_unix(2026, 9, 28, 6, 33, 1), Some(SEP_28));
    }

    #[test]
    fn nginx_time_local_reads_the_combined_format_and_its_offset() {
        assert_eq!(nginx_time_local("28/Sep/2026:06:33:01 +0000"), Some(SEP_28));
        // The same instant written two hours east of UTC.
        assert_eq!(nginx_time_local("28/Sep/2026:08:33:01 +0200"), Some(SEP_28));
        assert_eq!(nginx_time_local("28/Sep/2026:01:03:01 -0530"), Some(SEP_28));
    }

    #[test]
    fn nginx_time_local_refuses_what_is_not_a_time() {
        for text in [
            "",
            "x",
            "28/Foo/2026:06:33:01 +0000",
            "31/Sep/2026:06:33:01 +0000",
            "28/Sep/2026:06:33:01",
            "28/Sep/2026:24:00:00 +0000",
            "28/Sep/2026:06:33:01 +2400",
        ] {
            assert_eq!(nginx_time_local(text), None, "{text:?}");
        }
    }

    #[test]
    fn iso8601_reads_every_spelling_the_logs_use() {
        for text in [
            "2026-09-28T06:33:01+00:00",
            "2026-09-28T06:33:01+0000",
            "2026-09-28T06:33:01Z",
            "2026-09-28T06:33:01.123456+00:00",
            "2026-09-28T08:33:01+02:00",
            "2026-09-28T01:03:01-05:30",
        ] {
            assert_eq!(iso8601(text), Some(SEP_28), "{text:?}");
        }
        assert_eq!(iso8601("2026-09-28T06:33:01"), None, "no zone, no guess");
    }

    #[test]
    fn a_syslog_time_is_read_in_the_current_year() {
        let now = SEP_28 + 3600;
        assert_eq!(syslog("Sep 28 06:33:01", now, &utc), Some(SEP_28));
        assert_eq!(syslog("Sep 28 06:33:01.5", now, &utc), Some(SEP_28));
    }

    #[test]
    fn a_space_padded_syslog_day_is_read() {
        let sep_8 = civil_to_unix(2026, 9, 8, 6, 33, 1).unwrap();
        assert_eq!(syslog("Sep  8 06:33:01", SEP_28, &utc), Some(sep_8));
    }

    /// The year boundary: read on New Year's morning, a December line is
    /// last year's, and a January line this year's.
    #[test]
    fn a_december_line_read_in_january_is_last_year_s() {
        let new_year = civil_to_unix(2027, 1, 1, 0, 10, 0).unwrap();
        assert_eq!(
            syslog("Dec 31 23:59:59", new_year, &utc),
            civil_to_unix(2026, 12, 31, 23, 59, 59)
        );
        assert_eq!(
            syslog("Jan  1 00:05:00", new_year, &utc),
            civil_to_unix(2027, 1, 1, 0, 5, 0)
        );
    }

    /// And a line a clock slightly ahead wrote is still this year's rather
    /// than last year's.
    #[test]
    fn a_line_from_a_few_hours_ahead_is_not_moved_a_year_back() {
        let now = civil_to_unix(2026, 12, 31, 20, 0, 0).unwrap();
        assert_eq!(
            syslog("Dec 31 23:00:00", now, &utc),
            civil_to_unix(2026, 12, 31, 23, 0, 0)
        );
    }

    /// Syslog writes local time; the offset turns it into UTC.
    #[test]
    fn a_syslog_time_is_local_time() {
        let cest = |_: i64| 2 * 3600;
        assert_eq!(syslog("Sep 28 08:33:01", SEP_28, &cest), Some(SEP_28));
    }

    #[test]
    fn a_leap_day_is_placed_in_the_last_leap_year() {
        let now = civil_to_unix(2029, 3, 1, 0, 0, 0).unwrap();
        // 2029 and 2030 have no 29th of February; 2028 does.
        assert_eq!(
            syslog("Feb 29 12:00:00", now, &utc),
            civil_to_unix(2028, 2, 29, 12, 0, 0)
        );
    }

    #[test]
    fn a_syslog_line_is_read_in_either_prefix_style() {
        let now = SEP_28 + 60;
        for line in [
            "Sep 28 06:33:01 host sshd[1]: Failed password for root from 1.2.3.4 port 1 ssh2",
            "2026-09-28T06:33:01.000123+00:00 host sshd[1]: Failed password",
            "2026-09-28T06:33:01+0000 host sshd[1]: Failed password",
        ] {
            assert_eq!(syslog_line(line, now, &utc), Some(SEP_28), "{line}");
        }
        assert_eq!(
            syslog_line(
                "Failed password for root from 1.2.3.4 port 1 ssh2",
                now,
                &utc
            ),
            None,
            "journalctl -o cat carries no time"
        );
    }

    #[test]
    fn civil_is_the_inverse_of_the_epoch_arithmetic() {
        assert_eq!(civil(SEP_28), (2026, 9, 28, 6, 33, 1));
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil(-1), (1969, 12, 31, 23, 59, 59));
    }

    #[test]
    fn the_year_of_an_instant_is_its_utc_year() {
        assert_eq!(year_of(0), 1970);
        assert_eq!(year_of(SEP_28), 2026);
        assert_eq!(
            year_of(civil_to_unix(2026, 12, 31, 23, 59, 59).unwrap()),
            2026
        );
        assert_eq!(year_of(civil_to_unix(2027, 1, 1, 0, 0, 0).unwrap()), 2027);
    }

    #[test]
    fn the_local_offset_is_a_sane_number_of_hours() {
        let offset = local_offset(SEP_28);
        assert!((-14 * 3600..=14 * 3600).contains(&offset), "{offset}");
    }
}
