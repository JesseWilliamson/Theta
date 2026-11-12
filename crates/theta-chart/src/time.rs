//! Dates and timestamps as numbers (seconds since 1970-01-01 UTC) and back, with no calendar dependency.
//!
//! The result set carries dates as text (`2024-03-05`, `2024-03-05 12:34:56.5+02`); a chart needs a number to
//! place them on an axis. Time zones are honoured when the text has an offset and ignored otherwise (a
//! `timestamp without time zone` is plotted as if it were UTC, which only matters for the labels).

/// Days from 1970-01-01 to the given civil date (proleptic Gregorian). Howard Hinnant's algorithm.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse of [`days_from_civil`].
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
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

/// Parse `YYYY-MM-DD`, optionally followed by `T` or a space and `HH:MM[:SS[.fff]]`, optionally followed by
/// `Z` or `+HH[:MM]` / `-HH[:MM]`. `None` for anything else.
pub fn parse(text: &str) -> Option<f64> {
    let t = text.trim();
    let (date, rest) = match t.find(['T', ' ']) {
        Some(i) => (&t[..i], t[i + 1..].trim()),
        None => (t, ""),
    };
    let mut parts = date.splitn(3, '-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let mut secs = days_from_civil(y, m, d) as f64 * 86_400.0;
    if rest.is_empty() {
        return Some(secs);
    }
    // Split the clock from a trailing zone designator.
    let (clock, offset) = match rest.find(['Z', 'z', '+']).or_else(|| rest[1..].find('-').map(|i| i + 1)) {
        Some(i) => (&rest[..i], Some(&rest[i..])),
        None => (rest, None),
    };
    let mut c = clock.trim().splitn(3, ':');
    let h: f64 = c.next()?.parse().ok()?;
    let mi: f64 = c.next().unwrap_or("0").parse().ok()?;
    let s: f64 = c.next().unwrap_or("0").parse().ok()?;
    if h >= 24.0 || mi >= 60.0 || s >= 61.0 {
        return None;
    }
    secs += h * 3600.0 + mi * 60.0 + s;
    if let Some(off) = offset {
        let off = off.trim();
        if !off.eq_ignore_ascii_case("z") {
            let sign = if off.starts_with('-') { -1.0 } else { 1.0 };
            let body = &off[1..];
            let mut o = body.splitn(2, ':');
            let oh: f64 = o.next()?.parse().ok()?;
            let om: f64 = o.next().unwrap_or("0").parse().ok()?;
            secs -= sign * (oh * 3600.0 + om * 60.0);
        }
    }
    Some(secs)
}

const MONTHS: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// How much detail a time label carries; chosen from the tick step so neighbouring labels differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Seconds,
    Minutes,
    Days,
    Months,
    Years,
}

pub fn format(secs: f64, res: Resolution) -> String {
    let days = (secs / 86_400.0).floor() as i64;
    let rem = secs - days as f64 * 86_400.0;
    let (y, m, d) = civil_from_days(days);
    let (h, mi, s) = ((rem / 3600.0) as u32, ((rem % 3600.0) / 60.0) as u32, (rem % 60.0) as u32);
    match res {
        Resolution::Seconds => format!("{h:02}:{mi:02}:{s:02}"),
        Resolution::Minutes => format!("{} {d} {h:02}:{mi:02}", MONTHS[m as usize - 1]),
        Resolution::Days => format!("{} {d}", MONTHS[m as usize - 1]),
        Resolution::Months => format!("{} {y}", MONTHS[m as usize - 1]),
        Resolution::Years => format!("{y}"),
    }
}

/// The full form for a tooltip: `2024-03-05` for midnight, else `2024-03-05 12:34:56`.
pub fn format_full(secs: f64) -> String {
    let days = (secs / 86_400.0).floor() as i64;
    let rem = secs - days as f64 * 86_400.0;
    let (y, m, d) = civil_from_days(days);
    if rem < 0.5 {
        format!("{y:04}-{m:02}-{d:02}")
    } else {
        let (h, mi, s) = ((rem / 3600.0) as u32, ((rem % 3600.0) / 60.0) as u32, (rem % 60.0) as u32);
        format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_conversion_round_trips_and_matches_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        for z in (-800_000..800_000).step_by(997) {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        assert_eq!(civil_from_days(11_016), (2000, 2, 29), "2000 is a leap year");
    }

    #[test]
    fn parses_what_the_drivers_print() {
        assert_eq!(parse("1970-01-02"), Some(86_400.0));
        assert_eq!(parse("1970-01-01 00:00:01"), Some(1.0));
        assert_eq!(parse("1970-01-01T00:00:01.5"), Some(1.5));
        assert_eq!(parse("1970-01-01 02:00:00+02"), Some(0.0));
        assert_eq!(parse("1970-01-01 00:00:00-05:30"), Some(19_800.0));
        assert_eq!(parse("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse("2024-02-30x"), None);
        assert_eq!(parse("not a date"), None);
        assert_eq!(parse("2024-13-01"), None);
        assert_eq!(parse("2024-01-01 25:00:00"), None);
    }

    #[test]
    fn labels_follow_the_resolution() {
        let t = parse("2024-03-05 12:34:56").unwrap();
        assert_eq!(format(t, Resolution::Seconds), "12:34:56");
        assert_eq!(format(t, Resolution::Minutes), "Mar 5 12:34");
        assert_eq!(format(t, Resolution::Days), "Mar 5");
        assert_eq!(format(t, Resolution::Months), "Mar 2024");
        assert_eq!(format(t, Resolution::Years), "2024");
        assert_eq!(format_full(t), "2024-03-05 12:34:56");
        assert_eq!(format_full(parse("2024-03-05").unwrap()), "2024-03-05");
    }
}
