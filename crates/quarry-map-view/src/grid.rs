//! Tick and graticule spacing.

/// The smallest "nice" step (1, 2 or 5 times a power of ten) that is at least `raw`.
pub fn nice_step(raw: f64) -> f64 {
    if !(raw.is_finite() && raw > 0.0) {
        return 1.0;
    }
    let pow = 10f64.powf(raw.log10().floor());
    [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * pow).find(|s| *s >= raw * (1.0 - 1e-9)).unwrap_or(10.0 * pow)
}

/// Multiples of `step` inside `[min, max]`, at most `cap` of them.
pub fn ticks(min: f64, max: f64, step: f64, cap: usize) -> Vec<f64> {
    if !(step.is_finite() && step > 0.0 && min.is_finite() && max.is_finite() && max >= min) {
        return vec![];
    }
    let first = (min / step).ceil() as i64;
    let last = (max / step).floor() as i64;
    if last < first || (last - first) as usize >= cap {
        return vec![];
    }
    (first..=last).map(|i| i as f64 * step).collect()
}

/// Graticule step in degrees for a span of `span_deg` shown over `px` pixels, aiming for
/// lines roughly `target_px` apart. Steps below one degree stay decimal; larger ones
/// use 1/2/5 multiples too (so 10, 20, 50).
pub fn degree_step(span_deg: f64, px: f64, target_px: f64) -> f64 {
    nice_step(span_deg / (px / target_px).max(1.0))
}

/// Label for a tick value given the step (no more decimals than the step needs).
pub fn tick_label(v: f64, step: f64) -> String {
    let decimals = if step >= 1.0 { 0 } else { (-step.log10().floor()) as usize }.min(8);
    let s = format!("{v:.decimals$}");
    match s.strip_prefix('-') {
        Some(rest) if rest.chars().all(|c| c == '0' || c == '.') => rest.to_string(),
        _ => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_steps() {
        assert_eq!(nice_step(1.0), 1.0);
        assert_eq!(nice_step(1.3), 2.0);
        assert_eq!(nice_step(3.0), 5.0);
        assert_eq!(nice_step(7.0), 10.0);
        assert_eq!(nice_step(0.034), 0.05);
        assert_eq!(nice_step(-1.0), 1.0);
    }

    #[test]
    fn tick_ranges() {
        assert_eq!(ticks(0.5, 5.5, 1.0, 100), vec![1.0, 2.0, 3.0, 4.0, 5.0]);
        assert!(ticks(0.0, 1e9, 1.0, 100).is_empty(), "too many ticks are refused");
        assert!(ticks(5.0, 1.0, 1.0, 100).is_empty());
    }

    #[test]
    fn labels() {
        assert_eq!(tick_label(20.0, 10.0), "20");
        assert_eq!(tick_label(0.25, 0.05), "0.25");
        assert_eq!(tick_label(-0.0, 0.5), "0.0");
        assert_eq!(tick_label(-0.0, 5.0), "0");
    }
}
