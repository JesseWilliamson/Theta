//! Number formatting for plan figures. Pure so the rules are unit-tested; every
//! view goes through these so a time or row count reads the same everywhere.

/// `0.04 ms`, `12.4 ms`, `123 ms`, `1.24 s`, `2 min 03 s`.
///
/// Precision shrinks as the value grows so the column stays narrow while small
/// values (which are the interesting ones in a fast query) keep their digits.
pub fn fmt_ms(ms: f64) -> String {
    if !ms.is_finite() || ms < 0.0 {
        return "–".into();
    }
    if ms == 0.0 {
        return "0 ms".into();
    }
    if ms < 0.005 {
        return "<0.01 ms".into();
    }
    if ms < 0.995 {
        return format!("{ms:.2} ms");
    }
    if ms < 99.95 {
        return format!("{ms:.1} ms");
    }
    if ms < 999.5 {
        return format!("{ms:.0} ms");
    }
    let s = ms / 1000.0;
    if s < 59.995 {
        return format!("{s:.2} s");
    }
    let total = s.round() as u64;
    format!("{} min {:02} s", total / 60, total % 60)
}

/// Thousands separators for an integer-valued figure: `1,204`.
pub fn group_digits(n: u64) -> String {
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

/// Row counts: exact with separators up to 999,999, then `1.2M` / `12M` / `1.2B`.
pub fn fmt_rows(v: f64) -> String {
    if !v.is_finite() || v < 0.0 {
        return "–".into();
    }
    let n = v.round();
    if n < 1_000_000.0 {
        return group_digits(n as u64);
    }
    let (scaled, unit) = if n < 1e9 { (n / 1e6, "M") } else { (n / 1e9, "B") };
    if scaled < 9.95 { format!("{scaled:.1}{unit}") } else { format!("{scaled:.0}{unit}") }
}

/// Planner cost units (unit-less, so only the magnitude matters).
pub fn fmt_cost(v: f64) -> String {
    if !v.is_finite() || v < 0.0 {
        return "–".into();
    }
    if v < 1000.0 {
        format!("{v:.2}")
    } else if v < 100_000.0 {
        group_digits(v.round() as u64)
    } else if v < 1e6 {
        format!("{:.0}k", v / 1e3)
    } else if v < 1e7 {
        format!("{:.1}M", v / 1e6)
    } else {
        format!("{:.0}M", v / 1e6)
    }
}

/// A percentage of a whole: `<1%`, `37%`, `100%`.
pub fn fmt_share(share: f64) -> String {
    let p = share * 100.0;
    if !p.is_finite() || p <= 0.0 {
        "0%".into()
    } else if p < 1.0 {
        "<1%".into()
    } else {
        format!("{:.0}%", p.min(100.0))
    }
}

/// Buffer counts are 8 kB blocks on PostgreSQL; show the byte size beside them.
pub fn fmt_blocks_size(blocks: u64) -> String {
    let kb = blocks * 8;
    if kb < 1024 {
        format!("{} kB", group_digits(kb))
    } else if kb < 1024 * 1024 {
        format!("{:.1} MB", kb as f64 / 1024.0)
    } else {
        format!("{:.2} GB", kb as f64 / 1024.0 / 1024.0)
    }
}

/// How far the planner's row estimate was from reality.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Misestimate {
    /// Always >= 1.
    pub factor: f64,
    /// The planner expected fewer rows than it got.
    pub under: bool,
}

impl Misestimate {
    /// `▲ 12×` (under-estimated) or `▼ 340×` (over-estimated).
    pub fn badge(&self) -> String {
        format!("{} {}", if self.under { "▲" } else { "▼" }, fmt_factor(self.factor))
    }
}

/// Estimates are compared per loop (both figures are per-loop in the contract)
/// and floored at one row so "0 vs 3" does not read as infinite.
pub fn misestimate(est: Option<f64>, actual: Option<f64>) -> Option<Misestimate> {
    let (est, act) = (est?.max(1.0), actual?.max(1.0));
    let factor = if act >= est { act / est } else { est / act };
    Some(Misestimate { factor, under: act > est })
}

/// A misestimate worth flagging in the table.
pub const MISESTIMATE_THRESHOLD: f64 = 10.0;

pub fn fmt_factor(f: f64) -> String {
    if f < 100.0 {
        format!("{f:.0}×")
    } else if f < 1000.0 {
        format!("{:.0}×", f.round())
    } else if f < 10_000.0 {
        format!("{:.1}k×", f / 1000.0)
    } else {
        format!("{:.0}k×", f / 1000.0)
    }
}

/// Cool-to-hot ramp position for a node's share of the total (0..=1).
///
/// A square root keeps small-but-real shares visibly warmer than the coolest
/// end, while a node that owns half the runtime already reads as hot.
pub fn heat_level(share: f64) -> f32 {
    if !share.is_finite() {
        return 0.0;
    }
    share.clamp(0.0, 1.0).sqrt() as f32
}

/// Which two neighbouring ramp stops `level` (0..=1) falls between, and how far
/// along that segment it is. `stops` must be at least 2.
pub fn ramp_segment(level: f32, stops: usize) -> (usize, f32) {
    debug_assert!(stops >= 2);
    let level = level.clamp(0.0, 1.0);
    let pos = level * (stops - 1) as f32;
    let lo = (pos.floor() as usize).min(stops - 2);
    (lo, pos - lo as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milliseconds() {
        assert_eq!(fmt_ms(0.0), "0 ms");
        assert_eq!(fmt_ms(0.001), "<0.01 ms");
        assert_eq!(fmt_ms(0.04), "0.04 ms");
        assert_eq!(fmt_ms(0.9), "0.90 ms");
        assert_eq!(fmt_ms(12.4), "12.4 ms");
        assert_eq!(fmt_ms(12.44), "12.4 ms");
        assert_eq!(fmt_ms(99.94), "99.9 ms");
        assert_eq!(fmt_ms(99.96), "100 ms");
        assert_eq!(fmt_ms(123.4), "123 ms");
        assert_eq!(fmt_ms(999.6), "1.00 s");
        assert_eq!(fmt_ms(1240.0), "1.24 s");
        assert_eq!(fmt_ms(59_000.0), "59.00 s");
        assert_eq!(fmt_ms(123_000.0), "2 min 03 s");
        assert_eq!(fmt_ms(60_000.0), "1 min 00 s");
        assert_eq!(fmt_ms(f64::NAN), "–");
    }

    #[test]
    fn row_counts() {
        assert_eq!(fmt_rows(0.0), "0");
        assert_eq!(fmt_rows(7.4), "7");
        assert_eq!(fmt_rows(1204.0), "1,204");
        assert_eq!(fmt_rows(999_999.0), "999,999");
        assert_eq!(fmt_rows(1_200_000.0), "1.2M");
        assert_eq!(fmt_rows(12_400_000.0), "12M");
        assert_eq!(fmt_rows(2_500_000_000.0), "2.5B");
    }

    #[test]
    fn costs() {
        assert_eq!(fmt_cost(0.0), "0.00");
        assert_eq!(fmt_cost(162.774), "162.77");
        assert_eq!(fmt_cost(12_345.4), "12,345");
        assert_eq!(fmt_cost(250_000.0), "250k");
        assert_eq!(fmt_cost(1_500_000.0), "1.5M");
    }

    #[test]
    fn shares_and_sizes() {
        assert_eq!(fmt_share(0.0), "0%");
        assert_eq!(fmt_share(0.004), "<1%");
        assert_eq!(fmt_share(0.374), "37%");
        assert_eq!(fmt_share(1.2), "100%");
        assert_eq!(fmt_blocks_size(28), "224 kB");
        assert_eq!(fmt_blocks_size(1024), "8.0 MB");
    }

    #[test]
    fn misestimates_are_per_loop_and_directional() {
        let m = misestimate(Some(10.0), Some(1000.0)).unwrap();
        assert!(m.under && (m.factor - 100.0).abs() < 1e-9);
        assert_eq!(m.badge(), "▲ 100×");
        let m = misestimate(Some(5000.0), Some(5.0)).unwrap();
        assert!(!m.under);
        assert_eq!(m.badge(), "▼ 1.0k×");
        // Zero actual rows is treated as one so the factor stays finite.
        let m = misestimate(Some(0.0), Some(0.0)).unwrap();
        assert_eq!(m.factor, 1.0);
        assert!(misestimate(None, Some(3.0)).is_none());
        assert_eq!(fmt_factor(12.0), "12×");
        assert_eq!(fmt_factor(25_000.0), "25k×");
    }

    #[test]
    fn heat_ramp_selection() {
        assert_eq!(heat_level(0.0), 0.0);
        assert_eq!(heat_level(1.0), 1.0);
        assert!((heat_level(0.25) - 0.5).abs() < 1e-6);
        assert_eq!(heat_level(f64::NAN), 0.0);
        assert_eq!(heat_level(7.0), 1.0);
        assert_eq!(ramp_segment(0.0, 4), (0, 0.0));
        assert_eq!(ramp_segment(1.0, 4), (2, 1.0));
        let (lo, f) = ramp_segment(0.5, 4);
        assert_eq!(lo, 1);
        assert!((f - 0.5).abs() < 1e-6);
    }
}
