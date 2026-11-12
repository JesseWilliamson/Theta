//! Axis ticks: "nice" numbers for a linear axis and calendar-aware steps for a time axis.

use crate::time::{self, Resolution};

/// Round `x` to 1, 2, 5 or 10 times a power of ten (Heckbert's nice numbers). `round` picks the nearest;
/// otherwise the next one up.
fn nice(x: f64, round: bool) -> f64 {
    let exp = x.log10().floor();
    let f = x / 10f64.powf(exp);
    let nf = if round {
        match f {
            f if f < 1.5 => 1.0,
            f if f < 3.0 => 2.0,
            f if f < 7.0 => 5.0,
            _ => 10.0,
        }
    } else {
        match f {
            f if f <= 1.0 => 1.0,
            f if f <= 2.0 => 2.0,
            f if f <= 5.0 => 5.0,
            _ => 10.0,
        }
    };
    nf * 10f64.powf(exp)
}

/// A linear axis: the ticks and the range they span (which contains `[min, max]`).
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    pub ticks: Vec<f64>,
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

/// Ticks for `[min, max]` aiming at about `target` of them. A degenerate range (one value) is widened
/// so there is something to draw.
pub fn linear(min: f64, max: f64, target: usize) -> Axis {
    let (mut lo, mut hi) = if min <= max { (min, max) } else { (max, min) };
    if !lo.is_finite() || !hi.is_finite() {
        (lo, hi) = (0.0, 1.0);
    }
    if (hi - lo).abs() < f64::EPSILON * lo.abs().max(1.0) {
        let pad = if lo == 0.0 { 1.0 } else { lo.abs() * 0.1 };
        (lo, hi) = (lo - pad, hi + pad);
    }
    let target = target.max(2) as f64;
    let range = nice(hi - lo, false);
    let step = nice(range / (target - 1.0), true);
    let start = (lo / step).floor() * step;
    let end = (hi / step).ceil() * step;
    let n = ((end - start) / step).round() as usize;
    let ticks: Vec<f64> = (0..=n).map(|i| clean(start + i as f64 * step, step)).collect();
    Axis { min: ticks[0], max: ticks[n], ticks, step }
}

/// Remove binary noise (0.30000000000000004) by rounding to the step's precision.
fn clean(v: f64, step: f64) -> f64 {
    let k = 10f64.powi(decimals_for(step) as i32);
    (v * k).round() / k
}

fn decimals_for(step: f64) -> usize {
    if step >= 1.0 { 0 } else { (-step.log10().floor()) as usize }
}

/// A tick or readout number: thousands separators, only as many decimals as the step needs, and an
/// SI-style suffix for very large values so labels stay short.
pub fn format_number(v: f64, step: f64) -> String {
    let a = v.abs();
    let (div, suffix) = if a >= 1e9 && step >= 1e8 {
        (1e9, "B")
    } else if a >= 1e6 && step >= 1e5 {
        (1e6, "M")
    } else if a >= 1e4 && step >= 1e3 {
        (1e3, "k")
    } else {
        (1.0, "")
    };
    let decimals = decimals_for(step / div).min(8);
    let mut s = format!("{:.decimals$}", v / div);
    if suffix.is_empty() {
        s = group_thousands(&s);
    }
    format!("{s}{suffix}")
}

fn group_thousands(s: &str) -> String {
    let (sign, rest) = s.strip_prefix('-').map_or(("", s), |r| ("-", r));
    let (int, frac) = rest.split_once('.').map_or((rest, None), |(i, f)| (i, Some(f)));
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    match frac {
        Some(f) => format!("{sign}{out}.{f}"),
        None => format!("{sign}{out}"),
    }
}

/// A value for a hover readout: all the digits that matter, no suffix.
pub fn format_exact(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        group_thousands(&format!("{v:.0}"))
    } else {
        let s = format!("{v:.4}");
        group_thousands(s.trim_end_matches('0').trim_end_matches('.'))
    }
}

const MIN: f64 = 60.0;
const HOUR: f64 = 3600.0;
const DAY: f64 = 86_400.0;

/// Candidate steps in seconds. Months and years are not fixed lengths and are handled separately.
const STEPS: [(f64, Resolution); 15] = [
    (1.0, Resolution::Seconds),
    (5.0, Resolution::Seconds),
    (15.0, Resolution::Seconds),
    (30.0, Resolution::Seconds),
    (MIN, Resolution::Minutes),
    (5.0 * MIN, Resolution::Minutes),
    (15.0 * MIN, Resolution::Minutes),
    (30.0 * MIN, Resolution::Minutes),
    (HOUR, Resolution::Minutes),
    (3.0 * HOUR, Resolution::Minutes),
    (6.0 * HOUR, Resolution::Minutes),
    (12.0 * HOUR, Resolution::Minutes),
    (DAY, Resolution::Days),
    (7.0 * DAY, Resolution::Days),
    (14.0 * DAY, Resolution::Days),
];

#[derive(Debug, Clone, PartialEq)]
pub struct TimeAxis {
    pub ticks: Vec<f64>,
    pub resolution: Resolution,
    pub min: f64,
    pub max: f64,
}

/// Ticks for a time range in epoch seconds: fixed steps up to two weeks, then whole months and years
/// on month starts, so a tick always lands on a date a person would name.
pub fn time_axis(min: f64, max: f64, target: usize) -> TimeAxis {
    let (mut lo, mut hi) = if min <= max { (min, max) } else { (max, min) };
    if hi - lo < 1.0 {
        (lo, hi) = (lo - 30.0, hi + 30.0);
    }
    let target = target.max(2) as f64;
    let want = (hi - lo) / (target - 1.0);
    if let Some(&(step, res)) = STEPS.iter().find(|(s, _)| *s >= want) {
        let start = (lo / step).floor() * step;
        let mut ticks = Vec::new();
        let mut t = start;
        while t < hi + step * 0.999 {
            ticks.push(t);
            t += step;
        }
        let (a, b) = (ticks[0], *ticks.last().unwrap());
        return TimeAxis { ticks, resolution: res, min: a, max: b };
    }
    // Whole months: 1, 2, 3, 6, 12, then years.
    let months_wanted = want / (30.4375 * DAY);
    let step_months = [1u32, 2, 3, 6, 12, 24, 60, 120, 240, 600, 1200]
        .into_iter()
        .find(|m| f64::from(*m) >= months_wanted)
        .unwrap_or(2400);
    let (y0, m0, _) = time::civil_from_days((lo / DAY).floor() as i64);
    let mut index = i64::from(m0) - 1 + y0 * 12;
    index -= index.rem_euclid(i64::from(step_months));
    let mut ticks = Vec::new();
    loop {
        let (y, m) = (index.div_euclid(12), index.rem_euclid(12) as u32 + 1);
        let t = time::days_from_civil(y, m, 1) as f64 * DAY;
        ticks.push(t);
        if t >= hi {
            break;
        }
        index += i64::from(step_months);
    }
    let res = if step_months >= 12 { Resolution::Years } else { Resolution::Months };
    TimeAxis { min: ticks[0], max: *ticks.last().unwrap(), ticks, resolution: res }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_ticks_cover_the_range_with_round_steps() {
        let a = linear(0.0, 97.0, 6);
        assert_eq!((a.min, a.max, a.step), (0.0, 100.0, 20.0));
        assert_eq!(a.ticks, vec![0.0, 20.0, 40.0, 60.0, 80.0, 100.0]);
        let b = linear(-3.2, 7.9, 5);
        assert!(b.min <= -3.2 && b.max >= 7.9);
        assert!(b.ticks.windows(2).all(|w| ((w[1] - w[0]) - b.step).abs() < 1e-9));
        assert!(b.ticks.contains(&0.0), "a range through zero has a tick at zero: {:?}", b.ticks);
    }

    #[test]
    fn fractions_do_not_pick_up_binary_noise() {
        let a = linear(0.0, 0.5, 6);
        assert_eq!(a.ticks, vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5]);
    }

    #[test]
    fn a_single_value_still_has_an_axis() {
        let a = linear(5.0, 5.0, 5);
        assert!(a.min < 5.0 && a.max > 5.0 && a.ticks.len() >= 2);
        let z = linear(0.0, 0.0, 5);
        assert!(z.min < 0.0 && z.max > 0.0);
        let bad = linear(f64::NAN, f64::INFINITY, 5);
        assert_eq!((bad.min, bad.max), (0.0, 1.0));
    }

    #[test]
    fn reversed_input_is_accepted() {
        assert_eq!(linear(10.0, 0.0, 6), linear(0.0, 10.0, 6));
    }

    #[test]
    fn numbers_are_short_and_exact_where_it_matters() {
        assert_eq!(format_number(1500.0, 500.0), "1,500");
        assert_eq!(format_number(0.25, 0.05), "0.25");
        assert_eq!(format_number(2_500_000.0, 500_000.0), "2.5M");
        assert_eq!(format_number(-12_000.0, 2_000.0), "-12k");
        assert_eq!(format_number(3.0, 1.0), "3");
        assert_eq!(format_exact(1234567.0), "1,234,567");
        assert_eq!(format_exact(12.340), "12.34");
        assert_eq!(format_exact(-0.5), "-0.5");
    }

    #[test]
    fn short_time_ranges_tick_on_clock_boundaries() {
        let day0 = time::parse("2024-03-05").unwrap();
        let a = time_axis(day0, day0 + 6.0 * HOUR, 6);
        assert_eq!(a.resolution, Resolution::Minutes);
        assert!(a.ticks.iter().all(|t| (t % HOUR) == 0.0), "{:?}", a.ticks);
        assert!(a.min <= day0 && a.max >= day0 + 6.0 * HOUR);
    }

    #[test]
    fn long_time_ranges_tick_on_month_and_year_starts() {
        let a = time::parse("2022-02-10").unwrap();
        let b = time::parse("2023-01-20").unwrap();
        let ax = time_axis(a, b, 6);
        assert!(matches!(ax.resolution, Resolution::Months), "{:?}", ax.resolution);
        for t in &ax.ticks {
            let (_, _, d) = time::civil_from_days((t / DAY) as i64);
            assert_eq!(d, 1, "ticks are month starts");
        }
        assert!(ax.min <= a && ax.max >= b);
        let years = time_axis(time::parse("1990-01-01").unwrap(), time::parse("2024-01-01").unwrap(), 6);
        assert_eq!(years.resolution, Resolution::Years);
        assert!(years.ticks.len() >= 3 && years.ticks.len() <= 12, "{}", years.ticks.len());
    }

    #[test]
    fn a_two_week_range_uses_day_labels() {
        let d0 = time::parse("2024-03-01").unwrap();
        let ax = time_axis(d0, d0 + 40.0 * DAY, 6);
        assert_eq!(ax.resolution, Resolution::Days);
    }

    #[test]
    fn a_single_instant_still_has_an_axis() {
        let t = time::parse("2024-03-05 10:00:00").unwrap();
        let ax = time_axis(t, t, 5);
        assert!(ax.ticks.len() >= 2 && ax.min <= t && ax.max >= t);
    }
}
