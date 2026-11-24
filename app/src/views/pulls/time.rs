//! The few date pieces the PR views show (Swift's Date formatting): ISO 8601 parsing, "3 hours ago", the timeline's
//! day markers. Local time comes from the system's UTC offset, read once.
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since 1970 of "2026-11-20T09:15:00Z", "…+02:00" or "….123Z"; None when it isn't one.
pub fn parse(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 { return None }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    let mut rest = &s[19..];
    if let Some(r) = rest.strip_prefix('.') { rest = r.trim_start_matches(|c: char| c.is_ascii_digit()) }
    let off = match rest.as_bytes().first() {
        None | Some(b'Z') | Some(b'z') => 0,
        Some(&sign @ (b'+' | b'-')) => {
            let hh: i64 = rest.get(1..3)?.parse().ok()?;
            let mm: i64 = rest.get(rest.len() - 2..)?.parse().ok()?;
            let o = hh * 3600 + mm * 60;
            if sign == b'+' { o } else { -o }
        }
        _ => return None,
    };
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se - off)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
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
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

pub fn now() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64) }

/// The system's offset from UTC in seconds (`date +%z`), once per launch.
fn offset() -> i64 {
    static O: OnceLock<i64> = OnceLock::new();
    *O.get_or_init(|| {
        let out = std::process::Command::new("/bin/date").arg("+%z").output().ok();
        let s = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let v: i64 = s.get(1..).and_then(|x| x.parse().ok()).unwrap_or(0);
        let o = (v / 100) * 3600 + (v % 100) * 60;
        if s.starts_with('-') { -o } else { o }
    })
}

fn local_day(t: i64) -> i64 { (t + offset()).div_euclid(86400) }

/// "just now", "5 minutes ago", "yesterday", "3 weeks ago" (Swift's `.relative(presentation: .named)`).
pub fn relative(t: i64, now: i64) -> String {
    let d = (now - t).max(0);
    let unit = |n: i64, w: &str| if n == 1 { format!("1 {w} ago") } else { format!("{n} {w}s ago") };
    match d {
        0..60 => "now".into(),
        60..3600 => unit(d / 60, "minute"),
        3600..86400 => unit(d / 3600, "hour"),
        _ if local_day(now) - local_day(t) == 1 => "yesterday".into(),
        86400..604800 => unit(d / 86400, "day"),
        604800..2_592_000 => unit(d / 604800, "week"),
        2_592_000..31_536_000 => unit(d / 2_592_000, "month"),
        _ => unit(d / 31_536_000, "year"),
    }
}

const WEEKDAYS: [&str; 7] = ["Thursday", "Friday", "Saturday", "Sunday", "Monday", "Tuesday", "Wednesday"];
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// The timeline's day capsule: "Today", "Yesterday", "Friday, 20 November", or "Earlier" for an unknown date.
pub fn day_marker(t: Option<i64>, now: i64) -> String {
    let Some(t) = t else { return "Earlier".into() };
    let (day, today) = (local_day(t), local_day(now));
    if day == today { return "Today".into() }
    if day == today - 1 { return "Yesterday".into() }
    let (_, m, d) = civil_from_days(day);
    format!("{}, {d} {}", WEEKDAYS[day.rem_euclid(7) as usize], MONTHS[(m - 1) as usize])
}

pub fn same_day(a: Option<i64>, b: Option<i64>) -> bool {
    match (a, b) { (Some(a), Some(b)) => local_day(a) == local_day(b), (None, None) => true, _ => false }
}

/// "Friday, 20 November 2026 at 09:15" (the full date in a tooltip).
pub fn full(t: i64) -> String {
    let lt = t + offset();
    let day = lt.div_euclid(86400);
    let (y, m, d) = civil_from_days(day);
    let s = lt.rem_euclid(86400);
    format!("{}, {d} {} {y} at {:02}:{:02}", WEEKDAYS[day.rem_euclid(7) as usize], MONTHS[(m - 1) as usize], s / 3600, s / 60 % 60)
}

/// "9:15": a joined comment's time.
pub fn clock(t: i64) -> String {
    let s = (t + offset()).rem_euclid(86400);
    format!("{}:{:02}", s / 3600, s / 60 % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iso_and_formats() {
        assert_eq!(parse("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse("2026-11-20T09:15:00Z"), parse("2026-11-20T11:15:00+02:00"));
        assert_eq!(parse("2026-11-20T09:15:00.250Z"), parse("2026-11-20T09:15:00Z"));
        assert_eq!(parse("nope"), None);
        let t = parse("2026-11-20T09:15:00Z").unwrap();
        assert_eq!(civil_from_days(t.div_euclid(86400)), (2026, 11, 20));
        assert_eq!(relative(t, t + 30), "now");
        assert_eq!(relative(t, t + 7200), "2 hours ago");
        assert_eq!(relative(t, t + 60), "1 minute ago");
        assert_eq!(day_marker(None, t), "Earlier");
        assert_eq!(day_marker(Some(t), t), "Today");
        // 1970-01-01 was a Thursday.
        assert_eq!(WEEKDAYS[0], "Thursday");
    }
}
