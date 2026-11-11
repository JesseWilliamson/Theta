//! Server administration, as pure functions: the SQL each product needs and how to read its answer.
//!
//! The app runs the statements (on a connection of its own, see `crates/theta/src/admin`); this module only
//! knows what to ask and how to interpret rows, so every dialect difference is unit-tested without a server.
//! Which products have which tool is decided once, in `theta_core::Features` (`Feature::Sessions`, `Roles`,
//! `RoleEdit`), and each entry point here repeats that check so a caller cannot send SQL a product rejects.

pub mod roles;
pub mod sessions;

use theta_core::Value;

/// Cell text; `NULL` is an empty string.
pub(crate) fn text(v: &Value) -> String {
    v.display()
}

pub(crate) fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Int(i) => Some(*i as f64),
        Value::UInt(u) => Some(*u as f64),
        Value::Float(f) => Some(*f),
        Value::Decimal(s) | Value::Text(s) | Value::Unknown(s) => s.trim().parse().ok(),
        _ => None,
    }
}

pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Int(i) => *i != 0,
        Value::UInt(u) => *u != 0,
        Value::Text(s) | Value::Unknown(s) => {
            matches!(s.trim().to_ascii_lowercase().as_str(), "y" | "yes" | "t" | "true" | "1")
        }
        _ => false,
    }
}

/// A single-quoted SQL string literal for the dialects here. Backslashes are doubled for MySQL, whose
/// default `sql_mode` treats them as escapes (`'a\'` would otherwise end the string early).
pub fn string_literal(s: &str, mysql: bool) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\'' => out.push_str("''"),
            '\\' if mysql => out.push_str("\\\\"),
            '\0' => {}
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Find the column named `name` (case-insensitive) in a result.
pub(crate) fn col(rs: &theta_core::ResultSet, name: &str) -> Option<usize> {
    rs.columns.iter().position(|c| c.name.eq_ignore_ascii_case(name))
}
