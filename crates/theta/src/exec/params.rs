//! Applying query parameters to the statements a run picked (pure; the dialog lives in `dialogs/params`).

use super::pick::Picked;
use theta_core::DatabaseKind;
use theta_sql::params::{Detected, ParamKey, ParamValue, detect, substitute};

/// The placeholders of `picks` (all statements together, so a script prompts once).
pub fn detect_picks(picks: &[Picked], kind: DatabaseKind) -> Detected {
    let sqls: Vec<&str> = picks.iter().map(|p| p.sql.as_str()).collect();
    detect(&sqls, kind)
}

/// `picks` with every placeholder replaced by its value. The statement ranges stay those of the
/// document: they only locate the status squiggle, never the text that runs.
pub fn apply(
    picks: &[Picked],
    detected: &Detected,
    values: &[ParamValue],
    kind: DatabaseKind,
) -> Result<Vec<Picked>, String> {
    picks
        .iter()
        .zip(&detected.statements)
        .map(|(p, occ)| {
            let sql = substitute(&p.sql, occ, &detected.slots, values, kind)?;
            Ok(Picked { sql, ..p.clone() })
        })
        .collect()
}

/// Values to pre-fill: what this console used last time for the same placeholder, else empty.
pub fn prefill(
    slots: &[ParamKey],
    remembered: &std::collections::HashMap<String, ParamValue>,
) -> Vec<ParamValue> {
    slots.iter().map(|k| remembered.get(&k.label()).cloned().unwrap_or_default()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::pick;
    use theta_sql::params::ParamType;

    const PG: DatabaseKind = DatabaseKind::Postgres;

    #[test]
    fn run_all_prompts_once_for_the_union() {
        let picks = pick::all("select :a, $1;\nselect :a, :b;", PG);
        let d = detect_picks(&picks, PG);
        assert_eq!(d.slots.iter().map(ParamKey::label).collect::<Vec<_>>(), [":a", "$1", ":b"]);
        let vals: Vec<ParamValue> =
            ["x", "2", "y"].iter().map(|t| ParamValue { ty: ParamType::Auto, text: (*t).into() }).collect();
        let out = apply(&picks, &d, &vals, PG).unwrap();
        assert_eq!(out[0].sql, "select 'x', 2");
        assert_eq!(out[1].sql, "select 'x', 'y'");
        assert_eq!(out[0].range, picks[0].range);
    }

    #[test]
    fn prefill_uses_remembered_values() {
        let mut m = std::collections::HashMap::new();
        m.insert(":a".to_string(), ParamValue { ty: ParamType::Number, text: "5".into() });
        let slots = [ParamKey::Named { sigil: ':', name: "a".into() }, ParamKey::Numbered(1)];
        let v = prefill(&slots, &m);
        assert_eq!(v[0].text, "5");
        assert_eq!(v[1], ParamValue::default());
    }
}
