//! Name resolution over the syntax tree.

use theta_core::{Catalog, DatabaseKind};

/// A diagnostic about a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameDiagnostic {
    pub range: std::ops::Range<usize>,
    pub message: String,
}

pub fn name_diagnostics(_sql: &str, _kind: DatabaseKind, _catalog: &Catalog) -> Vec<NameDiagnostic> {
    Vec::new()
}
