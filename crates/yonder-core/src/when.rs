//! Times: Unix seconds inside, `YYYY-MM-DDTHH:MM:SSZ` in the ledger, the
//! local time zone (the C library's, so `TZ` too) for what people type and
//! read. Durations (`30m`, `1h30m`) and repeat rules (`5m`, `daily@09:00`,
//! `weekly:1@09:00`, Monday is 1).

use std::time::{SystemTime, UNIX_EPOCH};

/// The last second the ledger holds: 9999-12-31T23:59:59Z.
pub const MAX_TIME: i64 = 253_402_300_799;

/// `t` and `secs` later, if that is a time the ledger holds.
pub fn add(t: i64, secs: i64) -> Option<i64> {
    t.checked_add(secs).filter(|x| (0..=MAX_TIME).contains(x))
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// `t` as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_utc(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86400), t.rem_euclid(86400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

/// `YYYY-MM-DDTHH:MM[:SS]` and `Z` or `+00:00`.
pub fn parse_utc(s: &str) -> Option<i64> {
    let s = s.strip_suffix('Z').or_else(|| s.strip_suffix("+00:00"))?;
    let (date, time) = s.split_once('T')?;
    let (y, m, d) = parse_date(date)?;
    let (h, min, sec) = parse_clock(time)?;
    Some(days_from_civil(y, m, d) * 86400 + h * 3600 + min * 60 + sec)
}

/// A local `YYYY-MM-DD HH:MM[:SS]` (or with `T` between).
pub fn parse_local(s: &str) -> Option<i64> {
    let (date, time) = s.trim().split_once([' ', 'T'])?;
    let (y, m, d) = parse_date(date)?;
    let (h, min, sec) = parse_clock(time.trim_start())?;
    let mut tm = local_tm(0);
    tm.tm_year = (y - 1900) as i32;
    tm.tm_mon = (m - 1) as i32;
    tm.tm_mday = d as i32;
    tm.tm_hour = h as i32;
    tm.tm_min = min as i32;
    tm.tm_sec = sec as i32;
    from_local(tm)
}

/// `t` as local `YYYY-MM-DD HH:MM`.
pub fn format_local(t: i64) -> String {
    let tm = local_tm(t);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min
    )
}

/// Seconds in `30s`, `30m`, `2h`, `1d` or a run of them like `1h30m`.
pub fn parse_duration(s: &str) -> Option<i64> {
    let mut rest = s.trim();
    let mut total = 0i64;
    if rest.is_empty() {
        return None;
    }
    while !rest.is_empty() {
        let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        let n: i64 = rest[..digits].parse().ok()?;
        let unit = rest[digits..].chars().next()?;
        total = total.checked_add(n.checked_mul(unit_secs(unit)?)?)?;
        rest = &rest[digits + 1..];
    }
    (total > 0).then_some(total)
}

fn unit_secs(unit: char) -> Option<i64> {
    match unit {
        's' => Some(1),
        'm' => Some(60),
        'h' => Some(3600),
        'd' => Some(86400),
        _ => None,
    }
}

/// How a reminder repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// Every `n` minutes, hours or days (`unit` is `m`, `h` or `d`).
    Every { n: i64, unit: char },
    /// Each day at a local time.
    Daily { hour: u32, min: u32 },
    /// Each week on `day` (1 Monday … 7 Sunday) at a local time.
    Weekly { day: u32, hour: u32, min: u32 },
}

impl Rule {
    /// `5m`, `every 5m`, `daily@09:00`, `weekly:1@09:00`.
    pub fn parse(s: &str) -> Option<Rule> {
        let s = s.trim();
        if let Some(at) = s.strip_prefix("daily@") {
            let (hour, min) = parse_hhmm(at)?;
            return Some(Rule::Daily { hour, min });
        }
        if let Some(rest) = s.strip_prefix("weekly:") {
            let (day, at) = rest.split_once('@')?;
            let day: u32 = day.parse().ok().filter(|d| (1..=7).contains(d))?;
            let (hour, min) = parse_hhmm(at)?;
            return Some(Rule::Weekly { day, hour, min });
        }
        let s = s.strip_prefix("every").map_or(s, str::trim_start);
        let unit = s.chars().last().filter(|u| "mhd".contains(*u))?;
        let digits = &s[..s.len() - 1];
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n: i64 = digits.parse().ok().filter(|n| *n > 0)?;
        n.checked_mul(unit_secs(unit)?)?;
        Some(Rule::Every { n, unit })
    }

    /// The form it is stored in.
    pub fn spec(&self) -> String {
        match *self {
            Rule::Every { n, unit } => format!("{n}{unit}"),
            Rule::Daily { hour, min } => format!("daily@{hour:02}:{min:02}"),
            Rule::Weekly { day, hour, min } => format!("weekly:{day}@{hour:02}:{min:02}"),
        }
    }

    /// The first time after `t` it goes off; none past `MAX_TIME`.
    pub fn next_after(&self, t: i64) -> Option<i64> {
        if !(0..=MAX_TIME).contains(&t) {
            return None;
        }
        let base = local_tm(t);
        let at = |days: i32, hour: u32, min: u32| {
            let mut tm = base;
            tm.tm_mday += days;
            tm.tm_hour = hour as i32;
            tm.tm_min = min as i32;
            tm.tm_sec = 0;
            from_local(tm).filter(|x| (0..=MAX_TIME).contains(x))
        };
        match *self {
            Rule::Every { n, unit } => add(t, n.checked_mul(unit_secs(unit)?)?),
            Rule::Daily { hour, min } => {
                let today = at(0, hour, min)?;
                if today > t { Some(today) } else { at(1, hour, min) }
            }
            Rule::Weekly { day, hour, min } => {
                let weekday = if base.tm_wday == 0 { 7 } else { base.tm_wday as u32 };
                let ahead = ((day + 7 - weekday) % 7) as i32;
                let first = at(ahead, hour, min)?;
                if first > t { Some(first) } else { at(ahead + 7, hour, min) }
            }
        }
    }
}

/// A number written in digits only: no sign, no spaces.
fn number(s: &str) -> Option<i64> {
    (!s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok())?
}

fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.split_once(':')?;
    let (h, m) = (number(h)?, number(m)?);
    ((0..24).contains(&h) && (0..60).contains(&m) && s.len() <= 5).then_some((h as u32, m as u32))
}

fn parse_date(s: &str) -> Option<(i64, i64, i64)> {
    let mut it = s.splitn(3, '-');
    let y = number(it.next()?)?;
    let m = number(it.next()?)?;
    let d = number(it.next()?)?;
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    ((1970..=9999).contains(&y) && (1..=12).contains(&m) && d >= 1 && d <= days[m as usize - 1])
        .then_some((y, m, d))
}

fn parse_clock(s: &str) -> Option<(i64, i64, i64)> {
    let mut it = s.split(':');
    let h = number(it.next()?)?;
    let m = number(it.next()?)?;
    let sec = it.next().map_or(Some(0), number)?;
    let ok = (0..24).contains(&h) && (0..60).contains(&m) && (0..60).contains(&sec);
    (it.next().is_none() && ok).then_some((h, m, sec))
}

// Howard Hinnant's days_from_civil and civil_from_days.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

fn local_tm(t: i64) -> libc::tm {
    // SAFETY: tm is plain data; localtime_r only writes into it.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let tt = t as libc::time_t;
        libc::localtime_r(&tt, &mut tm);
        tm
    }
}

/// The instant of a local broken-down time; mktime normalises overflowing
/// days and works out summer time.
fn from_local(mut tm: libc::tm) -> Option<i64> {
    tm.tm_isdst = -1;
    // SAFETY: mktime reads and normalises the tm we own.
    let t = unsafe { libc::mktime(&mut tm) };
    (t != -1).then_some(t as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_round_trip() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(parse_utc("2026-11-26T04:45:22Z"), Some(1795668322));
        assert_eq!(format_utc(1795668322), "2026-11-26T04:45:22Z");
        assert_eq!(parse_utc("2028-02-29T23:59Z").map(format_utc).as_deref(), Some("2028-02-29T23:59:00Z"));
        assert_eq!(parse_utc("2026-11-26T04:45:22+00:00"), Some(1795668322));
        for bad in [
            "2026-11-26T04:45:22",
            "2026-11-26 04:45Z",
            "2027-02-29T00:00Z",
            "2026-11-26T24:00Z",
            "2026-11-26T-1:00Z",
            "2026-11-26T04:-5Z",
            "2026-11-26T04:45:-1Z",
            "2026-11-26T+4:45Z",
            "10000-01-01T00:00Z",
        ] {
            assert_eq!(parse_utc(bad), None, "{bad}");
        }
    }

    #[test]
    fn local_round_trip() {
        let t = parse_local("2026-03-14 15:09").unwrap();
        assert_eq!(format_local(t), "2026-03-14 15:09");
        assert_eq!(parse_local("2026-03-14T15:09:30"), Some(t + 30));
        assert_eq!(parse_local("2026-02-30 10:00"), None);
        for bad in ["2026-03-14 -1:00", "2026-03-14 10:-1", "2026-03-14 -0:30", "2026-03-14 10:00:-1"] {
            assert_eq!(parse_local(bad), None, "{bad}");
        }
        assert_eq!(parse_local("tomorrow"), None);
    }

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("30m"), Some(1800));
        assert_eq!(parse_duration("1h30m"), Some(5400));
        assert_eq!(parse_duration("2d"), Some(172800));
        assert_eq!(parse_duration("45s"), Some(45));
        for bad in ["", "30", "m", "0m", "3w", "1h 30m", "-5m"] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
    }

    #[test]
    fn parses_rules() {
        assert_eq!(Rule::parse("5m"), Some(Rule::Every { n: 5, unit: 'm' }));
        assert_eq!(Rule::parse("every 2h").map(|r| r.spec()).as_deref(), Some("2h"));
        assert_eq!(Rule::parse("daily@9:05").map(|r| r.spec()).as_deref(), Some("daily@09:05"));
        assert_eq!(Rule::parse("weekly:7@21:00"), Some(Rule::Weekly { day: 7, hour: 21, min: 0 }));
        for bad in [
            "",
            "0m",
            "30s",
            "daily@24:00",
            "daily@9",
            "daily@-1:00",
            "daily@09:-1",
            "daily@-0:00",
            "daily@+9:00",
            "+5m",
            "every -5m",
            "weekly:0@09:00",
            "weekly:8@09:00",
            "weekly:1@-1:30",
            "hourly",
        ] {
            assert_eq!(Rule::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn daily_is_the_next_local_time() {
        let rule = Rule::parse("daily@09:00").unwrap();
        for start in ["2026-03-14 08:59", "2026-03-14 09:00", "2026-03-14 23:30", "2026-12-31 12:00"] {
            let t = parse_local(start).unwrap();
            let next = rule.next_after(t).unwrap();
            assert!(next > t && next <= t + 25 * 3600, "{start}");
            assert!(format_local(next).ends_with(" 09:00"), "{start}: {}", format_local(next));
        }
        let t = parse_local("2026-03-14 08:59").unwrap();
        assert_eq!(format_local(rule.next_after(t).unwrap()), "2026-03-14 09:00");
        assert_eq!(format_local(rule.next_after(t + 60).unwrap()), "2026-03-15 09:00");
        assert_eq!(format_local(rule.next_after(parse_local("2026-12-31 12:00").unwrap()).unwrap()), "2027-01-01 09:00");
    }

    #[test]
    fn weekly_counts_monday_as_one() {
        // 2026-03-14 is a Saturday.
        let sat = parse_local("2026-03-14 10:00").unwrap();
        let next = |spec: &str, t| format_local(Rule::parse(spec).unwrap().next_after(t).unwrap());
        assert_eq!(next("weekly:1@09:00", sat), "2026-03-16 09:00");
        assert_eq!(next("weekly:6@11:00", sat), "2026-03-14 11:00");
        assert_eq!(next("weekly:6@10:00", sat), "2026-03-21 10:00");
        assert_eq!(next("weekly:7@08:00", sat), "2026-03-15 08:00");
    }

    #[test]
    fn every_adds_its_interval() {
        assert_eq!(Rule::parse("5m").unwrap().next_after(1000), Some(1300));
        assert_eq!(Rule::parse("1d").unwrap().next_after(1000), Some(1000 + 86400));
    }

    #[test]
    fn stops_at_year_9999() {
        assert_eq!(format_utc(MAX_TIME), "9999-12-31T23:59:59Z");
        assert_eq!(add(MAX_TIME - 60, 60), Some(MAX_TIME));
        assert_eq!(add(MAX_TIME, 1), None);
        assert_eq!(add(1000, i64::MAX), None);
        assert_eq!(add(1000, -2000), None);
        let huge = parse_duration("9223372036854775807s").unwrap();
        assert_eq!(add(now(), huge), None);
        // A rule big enough to parse, too big to go off.
        let rule = Rule::parse("99999999999d").unwrap();
        assert_eq!(rule.next_after(now()), None);
        assert_eq!(Rule::parse("1m").unwrap().next_after(MAX_TIME), None);
        assert_eq!(Rule::parse("daily@09:00").unwrap().next_after(MAX_TIME), None);
        assert_eq!(Rule::parse("weekly:1@09:00").unwrap().next_after(-5), None);
    }
}
