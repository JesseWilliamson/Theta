//! Go to definition, find usages and occurrence highlighting.

use crate::analysis::{Analysis, RefRole, Resolution, SymbolId, TableId};
use std::ops::Range;

/// What an identifier under the cursor stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A name this script introduced: alias, CTE, an output column of a query.
    Symbol(SymbolId),
    Table(TableId),
    Column {
        table: TableId,
        column: String,
    },
    Function(String),
}

/// Where a name is defined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Definition {
    /// In this text: the identifier (or, for a column that passes through, the select item) that introduces it.
    Here(Range<usize>),
    /// A table or view of the catalog: show it in the explorer.
    Table(TableId),
    /// A column of a catalog table.
    Column {
        table: TableId,
        column: String,
    },
    Function(String),
}

/// The target of the identifier at `offset` and the span of that identifier.
pub fn target_at(a: &Analysis, offset: usize) -> Option<(Target, Range<usize>)> {
    let r = a.ref_at(offset)?;
    let t = match &r.resolution {
        Resolution::Symbol(s) => Target::Symbol(*s),
        Resolution::Table(t) => Target::Table(t.clone()),
        Resolution::Column { table, column } => {
            Target::Column { table: table.clone(), column: column.clone() }
        }
        Resolution::Function(f) => Target::Function(f.clone()),
        _ => return None,
    };
    Some((t, r.span.clone()))
}

/// Go to definition for the identifier at `offset`.
pub fn definition_at(a: &Analysis, offset: usize) -> Option<Definition> {
    let (target, _) = target_at(a, offset)?;
    Some(match target {
        Target::Symbol(s) => Definition::Here(a.symbols[s].span.clone()),
        Target::Table(t) => Definition::Table(t),
        Target::Column { table, column } => Definition::Column { table, column },
        Target::Function(f) => Definition::Function(f),
    })
}

/// One place a name is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    pub range: Range<usize>,
    /// The identifier that introduces the name (an alias, a CTE name), as opposed to a use of it.
    pub declaration: bool,
}

/// Every occurrence of `target` in the analysed text, in order. For a table, uses of its columns are not
/// included; for a column, only that column.
pub fn usages_of(a: &Analysis, target: &Target) -> Vec<Usage> {
    let mut out: Vec<Usage> = Vec::new();
    match target {
        Target::Symbol(s) => {
            let sym = &a.symbols[*s];
            let mut saw_decl = false;
            for r in a.refs_to_symbol(*s) {
                saw_decl |= r.role == RefRole::Declaration;
                out.push(Usage { range: r.span.clone(), declaration: r.role == RefRole::Declaration });
            }
            if !saw_decl {
                // a passthrough column has no declaring identifier of its own: its select item stands in
                out.push(Usage { range: sym.span.clone(), declaration: true });
            }
        }
        Target::Table(t) => {
            for r in &a.refs {
                if matches!(&r.resolution, Resolution::Table(x) if x == t) {
                    out.push(Usage { range: r.span.clone(), declaration: false });
                }
            }
        }
        Target::Column { table, column } => {
            for r in &a.refs {
                if matches!(&r.resolution, Resolution::Column { table: tt, column: c } if tt == table && c == column)
                {
                    out.push(Usage { range: r.span.clone(), declaration: false });
                }
            }
        }
        Target::Function(f) => {
            for r in &a.refs {
                if matches!(&r.resolution, Resolution::Function(x) if x.eq_ignore_ascii_case(f)) {
                    out.push(Usage { range: r.span.clone(), declaration: false });
                }
            }
        }
    }
    out.sort_by_key(|u| u.range.start);
    out.dedup();
    out
}

/// The ranges to highlight while the cursor rests at `offset`: every occurrence of the name under it. Empty
/// when the cursor is not on a resolved identifier (or the name occurs only once: nothing to show).
pub fn highlights_at(a: &Analysis, offset: usize) -> Vec<Range<usize>> {
    let Some((target, _)) = target_at(a, offset) else { return Vec::new() };
    if matches!(target, Target::Function(_)) {
        return Vec::new();
    }
    let u = usages_of(a, &target);
    if u.len() < 2 {
        return Vec::new();
    }
    u.into_iter().map(|u| u.range).collect()
}
