//! Syntax checks for the scalar text forms a delimited file can hold. They recognise forms; they do not
//! interpret them, because a database IDE must hand the server exactly the digits the file contained.

/// `YYYY-MM-DD` (a `/` separator is accepted and normalised) with a real calendar date.
pub fn parse_date(s: &str) -> Option<String> {
    let b = s.as_bytes();
    if b.len() != 10 || !matches!(b[4], b'-' | b'/') || b[7] != b[4] {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let t = &s[r];
        t.bytes().all(|c| c.is_ascii_digit()).then(|| t.parse().ok()).flatten()
    };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return None,
    };
    (d >= 1 && d <= dim).then(|| format!("{y:04}-{m:02}-{d:02}"))
}

/// `HH:MM`, `HH:MM:SS` or `HH:MM:SS.fraction` (up to nine digits).
pub fn parse_time(s: &str) -> Option<String> {
    let (main, frac) = match s.split_once('.') {
        Some((m, f)) => (m, Some(f)),
        None => (s, None),
    };
    let mut parts = main.split(':');
    let h = parts.next()?;
    let m = parts.next()?;
    let sec = parts.next();
    if parts.next().is_some() || (sec.is_none() && frac.is_some()) {
        return None;
    }
    let two = |t: &str| {
        (t.len() == 2 && t.bytes().all(|c| c.is_ascii_digit())).then(|| t.parse::<u32>().ok()).flatten()
    };
    let (h, m) = (two(h)?, two(m)?);
    let sec = match sec {
        Some(t) => two(t)?,
        None => 0,
    };
    // 24:00:00 is legal in ISO 8601 and in Postgres; leap second 60 is not accepted by every server.
    if h > 24 || m > 59 || sec > 59 || (h == 24 && (m != 0 || sec != 0)) {
        return None;
    }
    let mut out = format!("{h:02}:{m:02}:{sec:02}");
    if let Some(f) = frac {
        if f.is_empty() || f.len() > 9 || !f.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        out.push('.');
        out.push_str(f);
    }
    Some(out)
}

/// A timestamp: date, `T` or space, time, optional zone. Returns the normalised text (space separator,
/// `Z` written as `+00:00`) and whether it carried a zone.
pub fn parse_timestamp(s: &str) -> Option<(String, bool)> {
    let sep = s.find(['T', ' '])?;
    let date = parse_date(&s[..sep])?;
    let rest = &s[sep + 1..];
    let (time_part, zone) = split_zone(rest);
    let time = parse_time(time_part)?;
    if time.starts_with("24:") {
        return None;
    }
    match zone {
        None => Some((format!("{date} {time}"), false)),
        Some(z) => Some((format!("{date} {time}{z}"), true)),
    }
}

/// Split a trailing `Z`, `+HH`, `+HH:MM` or `-HH:MM` zone off a time. The returned zone is normalised.
fn split_zone(s: &str) -> (&str, Option<String>) {
    if let Some(t) = s.strip_suffix(['Z', 'z']) {
        return (t, Some("+00:00".into()));
    }
    if let Some(p) = s.rfind(['+', '-'])
        && p >= 5
    {
        let z = &s[p + 1..];
        let digits = |t: &str| t.len() == 2 && t.bytes().all(|c| c.is_ascii_digit());
        let ok = match z.split_once(':') {
            Some((h, m)) => digits(h) && digits(m),
            None => digits(z) || (z.len() == 4 && z.bytes().all(|c| c.is_ascii_digit())),
        };
        if ok {
            let norm = if z.len() == 4 && !z.contains(':') {
                format!("{}{}:{}", &s[p..p + 1], &z[..2], &z[2..])
            } else if z.len() == 2 {
                format!("{}{}:00", &s[p..p + 1], z)
            } else {
                s[p..].to_string()
            };
            return (&s[..p], Some(norm));
        }
    }
    (s, None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberKind {
    Integer,
    /// Plain decimal notation (`12.50`, `.5`).
    Decimal,
    /// Exponent notation, treated as floating point.
    Float,
}

#[derive(Debug, Clone, Copy)]
pub struct NumberInfo {
    pub kind: NumberKind,
    /// Digits before the decimal point (without leading zeros, at least 1).
    pub int_digits: u32,
    pub scale: u32,
    pub leading_zero: bool,
}

/// Classify `s` as a number. `s` must already be trimmed.
pub fn parse_number(s: &str) -> Option<NumberInfo> {
    let t = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (mantissa, exp) = match t.find(['e', 'E']) {
        Some(p) => (&t[..p], Some(&t[p + 1..])),
        None => (t, None),
    };
    if let Some(e) = exp {
        let d = e.strip_prefix(['+', '-']).unwrap_or(e);
        if d.is_empty() || d.len() > 4 || !d.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
    }
    let (int, frac) = match mantissa.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (mantissa, None),
    };
    if !int.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if let Some(f) = frac
        && !f.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    if int.is_empty() && frac.is_none_or(str::is_empty) {
        return None;
    }
    let trimmed = int.trim_start_matches('0');
    let kind = if exp.is_some() {
        NumberKind::Float
    } else if frac.is_some() {
        NumberKind::Decimal
    } else {
        NumberKind::Integer
    };
    Some(NumberInfo {
        kind,
        int_digits: trimmed.len().max(1) as u32,
        scale: frac.map_or(0, |f| f.len() as u32),
        // "0.5" and "0" are fine; "007" and "00.5" are identifiers, not numbers.
        leading_zero: int.len() > 1 && int.starts_with('0'),
    })
}

pub fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "t" | "yes" | "y" => Some(true),
        "false" | "f" | "no" | "n" => Some(false),
        _ => None,
    }
}

pub fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter()
            .enumerate()
            .all(|(i, &c)| if matches!(i, 8 | 13 | 18 | 23) { c == b'-' } else { c.is_ascii_hexdigit() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(parse_date("2024-02-29").as_deref(), Some("2024-02-29"));
        assert_eq!(parse_date("2023-02-29"), None);
        assert_eq!(parse_date("2024/03/01").as_deref(), Some("2024-03-01"));
        assert_eq!(parse_date("2024-13-01"), None);
        assert_eq!(parse_date("24-01-01"), None);
        assert_eq!(parse_date("2024-1-01"), None);
        assert_eq!(parse_date("2024-01/01"), None);
    }

    #[test]
    fn times() {
        assert_eq!(parse_time("07:05").as_deref(), Some("07:05:00"));
        assert_eq!(parse_time("23:59:59.123456").as_deref(), Some("23:59:59.123456"));
        assert_eq!(parse_time("24:00:00").as_deref(), Some("24:00:00"));
        assert_eq!(parse_time("25:00"), None);
        assert_eq!(parse_time("12:60"), None);
        assert_eq!(parse_time("12:30.5"), None);
        assert_eq!(parse_time("12:30:00."), None);
    }

    #[test]
    fn timestamps() {
        assert_eq!(parse_timestamp("2024-05-06T07:08:09"), Some(("2024-05-06 07:08:09".into(), false)));
        assert_eq!(
            parse_timestamp("2024-05-06 07:08:09.5Z"),
            Some(("2024-05-06 07:08:09.5+00:00".into(), true))
        );
        assert_eq!(
            parse_timestamp("2024-05-06 07:08+0530"),
            Some(("2024-05-06 07:08:00+05:30".into(), true))
        );
        assert_eq!(
            parse_timestamp("2024-05-06 07:08:09-08"),
            Some(("2024-05-06 07:08:09-08:00".into(), true))
        );
        assert_eq!(parse_timestamp("2024-05-06"), None);
        assert_eq!(parse_timestamp("2024-05-06 25:00:00"), None);
        assert_eq!(parse_timestamp("2024-05-06 24:00:00"), None);
    }

    #[test]
    fn numbers() {
        let n = parse_number("-12.50").unwrap();
        assert_eq!((n.kind, n.int_digits, n.scale), (NumberKind::Decimal, 2, 2));
        assert_eq!(parse_number("1e5").unwrap().kind, NumberKind::Float);
        assert_eq!(parse_number(".5").unwrap().kind, NumberKind::Decimal);
        assert!(parse_number("007").unwrap().leading_zero);
        assert!(!parse_number("0.5").unwrap().leading_zero);
        assert!(!parse_number("0").unwrap().leading_zero);
        for bad in ["", "-", ".", "1.2.3", "1,5", "abc", "1e", "0x10", "--1", "1_000", "NaN", "inf"] {
            assert!(parse_number(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn booleans_and_uuids() {
        assert_eq!(parse_bool("Yes"), Some(true));
        assert_eq!(parse_bool("F"), Some(false));
        assert_eq!(parse_bool("maybe"), None);
        assert!(is_uuid("123e4567-e89b-12d3-a456-426614174000"));
        assert!(!is_uuid("123e4567e89b12d3a456426614174000"));
    }
}
