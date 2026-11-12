//! Small formatters every crate shares, so a count, a duration or a rate reads the same in the
//! status bar, the grid footer, Messages and every dialog.

use std::time::Duration;

/// `1234567` -> `1,234,567`.
pub fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `1 row`, `2 rows`, `1,000 rows`: the number, grouped, then the matching noun.
pub fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { many })
}

/// The regular plural (`table` / `tables`).
pub fn plural_s(n: u64, noun: &str) -> String {
    let many = if noun.ends_with(['s', 'x', 'h']) { format!("{noun}es") } else { format!("{noun}s") };
    plural(n, noun, &many)
}

/// `<1 ms`, `42 ms`, `1.24 s`, `2 min 05 s`. The one duration format of the application.
pub fn duration(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1.0 {
        "<1 ms".into()
    } else if ms < 1000.0 {
        format!("{} ms", ms.round() as u64)
    } else if d.as_secs() < 60 {
        format!("{:.2} s", d.as_secs_f64())
    } else {
        format!("{} min {:02} s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

/// `1,200 rows/s`, `1 row/s`. The one throughput format; `None` when the run was too short to
/// tell (a rate computed from a few milliseconds is noise).
pub fn rate(rows: u64, elapsed: Duration) -> Option<String> {
    let secs = elapsed.as_secs_f64();
    if secs < 0.05 {
        return None;
    }
    let per = (rows as f64 / secs).round() as u64;
    Some(format!("{} /s", plural(per, "row", "rows")).replace(" /s", "/s"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_plurals() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1000), "1,000");
        assert_eq!(count(1_234_567), "1,234,567");
        assert_eq!(plural(1, "ragged row", "ragged rows"), "1 ragged row");
        assert_eq!(plural_s(0, "table"), "0 tables");
        assert_eq!(plural_s(1, "table"), "1 table");
        assert_eq!(plural_s(1500, "match"), "1,500 matches");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(Duration::from_micros(300)), "<1 ms");
        assert_eq!(duration(Duration::from_millis(831)), "831 ms");
        assert_eq!(duration(Duration::from_millis(540)), "540 ms");
        assert_eq!(duration(Duration::from_millis(1240)), "1.24 s");
        assert_eq!(duration(Duration::from_secs(125)), "2 min 05 s");
    }

    #[test]
    fn rates() {
        assert_eq!(rate(3, Duration::from_millis(500)).as_deref(), Some("6 rows/s"));
        assert_eq!(rate(1, Duration::from_secs(1)).as_deref(), Some("1 row/s"));
        assert_eq!(rate(5000, Duration::from_millis(10)), None);
    }
}
