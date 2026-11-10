//! Hover-card content helpers.

/// Maximum property rows shown in the hover card.
pub const MAX_ROWS: usize = 8;
/// Maximum characters of a value before it is cut with an ellipsis.
pub const MAX_VALUE_CHARS: usize = 60;

/// Truncate to `max` characters (not bytes), appending an ellipsis when cut. Newlines
/// collapse to spaces so one value never makes a tall card.
pub fn truncate_value(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).collect();
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// The rows to display, plus how many were left out.
pub fn card_rows(props: &[(String, String)]) -> (Vec<(String, String)>, usize) {
    let rows = props
        .iter()
        .take(MAX_ROWS)
        .map(|(k, v)| (truncate_value(k, 24), truncate_value(v, MAX_VALUE_CHARS)))
        .collect();
    (rows, props.len().saturating_sub(MAX_ROWS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_values_pass_through() {
        assert_eq!(truncate_value("Vienna", 10), "Vienna");
        assert_eq!(truncate_value("exactly10!", 10), "exactly10!");
    }

    #[test]
    fn long_values_are_cut_on_chars_not_bytes() {
        let s = "é".repeat(100);
        let t = truncate_value(&s, 10);
        assert_eq!(t.chars().count(), 10);
        assert!(t.ends_with('\u{2026}'));
    }

    #[test]
    fn whitespace_is_flattened() {
        assert_eq!(truncate_value("a\nb\tc", 10), "a b c");
    }

    #[test]
    fn rows_are_capped_at_eight() {
        let props: Vec<(String, String)> = (0..11).map(|i| (format!("k{i}"), "v".into())).collect();
        let (rows, more) = card_rows(&props);
        assert_eq!(rows.len(), 8);
        assert_eq!(more, 3);
        assert_eq!(rows[0].0, "k0");
    }
}
