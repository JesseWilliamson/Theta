//! Data compare: the rows of two tables that share a key, merged in key order.
//!
//! This module is the pure half. It maps the columns of the two tables, decides the key, and holds the
//! [`Comparer`], a state machine fed pages of rows in key order: it consumes what it can and says which side it
//! needs more of. It never holds a table: only the unmatched head of each stream, the counts, and up to `cap`
//! differing rows for display. The runner that reads the pages is `theta_import::compare`; the script that makes
//! the target equal the source is [`sync_script`].
//!
//! # Left and right
//!
//! *Left* is the **source** (the table you want), *right* the **target** (the table you have). A row only on the
//! left is missing from the target (the script inserts it), a row only on the right is extra (the script deletes
//! it), and a row on both whose values differ is updated.
//!
//! # Why the order of keys is checked
//!
//! A merge is only right if both streams are in the same order. Two products order text differently
//! (PostgreSQL by its collation, MariaDB case-insensitively, SQLite by bytes), so asking each for `ORDER BY key`
//! may give two orders. The [`Comparer`] therefore checks every row against the previous one *on the client*,
//! with one order for everything ([`value::cmp_value`]: numbers by value, text by UTF-8 bytes, times
//! chronologically). A stream that is not in that order is a [`MergeError::Unsorted`] and the runner asks the
//! servers again with a binary collation, which is that order; a key that is a duplicate or NULL is refused.
//!
//! # What a comparison does not see
//!
//! Columns that exist on one side only are listed and ignored; generated columns are never compared (they
//! cannot be written). Values are compared by [`values_equal`], which documents what two products' spellings of
//! one value are forgiven.

pub mod value;

#[cfg(test)]
mod tests;

pub use value::{KeyClass, Rules, cmp_keys, cmp_value, key_class, values_equal};

use crate::generate::sql_literal;
use crate::keywords::{qualified_name, quote_if_needed};
use std::collections::VecDeque;
use theta_core::{ColumnInfo, DatabaseKind, Family, Identity, Table, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct MappedColumn {
    /// The source (left) column.
    pub left: ColumnInfo,
    /// The target (right) column with the same name, case-insensitively.
    pub right: ColumnInfo,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mapping {
    /// Compared columns, in the source's order. Row vectors on both sides follow this order.
    pub columns: Vec<MappedColumn>,
    /// Indexes into `columns` of the key, in key order.
    pub key: Vec<usize>,
    pub classes: Vec<KeyClass>,
    /// Columns only the source has, only the target has, and generated columns of either (never compared).
    pub left_only: Vec<String>,
    pub right_only: Vec<String>,
    pub generated: Vec<String>,
}

impl Mapping {
    pub fn left_names(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.left.name.clone()).collect()
    }
    pub fn right_names(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.right.name.clone()).collect()
    }
    pub fn key_left(&self) -> Vec<String> {
        self.key.iter().map(|&i| self.columns[i].left.name.clone()).collect()
    }
    pub fn key_right(&self) -> Vec<String> {
        self.key.iter().map(|&i| self.columns[i].right.name.clone()).collect()
    }
}

/// Map the columns of `left` and `right` and fix the key. `key` empty means the source's primary key.
/// `Err` is a sentence for the user.
pub fn map_columns(left: &Table, right: &Table, key: &[String]) -> Result<Mapping, String> {
    let key: Vec<String> = if key.is_empty() { left.primary_key.clone() } else { key.to_vec() };
    if key.is_empty() {
        return Err(format!(
            "{} has no primary key, so its rows cannot be paired with the other table's. Choose the key columns (columns that identify a row) in the dialog.",
            left.name
        ));
    }
    let mut columns = Vec::new();
    let mut left_only = Vec::new();
    let mut generated = Vec::new();
    for c in &left.columns {
        match right.columns.iter().find(|d| d.name.eq_ignore_ascii_case(&c.name)) {
            Some(d) if c.generated.is_some() || d.generated.is_some() => generated.push(c.name.clone()),
            Some(d) => columns.push(MappedColumn { left: c.clone(), right: d.clone() }),
            None => left_only.push(c.name.clone()),
        }
    }
    let right_only: Vec<String> = right
        .columns
        .iter()
        .filter(|d| !left.columns.iter().any(|c| c.name.eq_ignore_ascii_case(&d.name)))
        .map(|d| d.name.clone())
        .collect();
    let mut idx = Vec::new();
    let mut classes = Vec::new();
    for k in &key {
        let Some(i) = columns.iter().position(|c| c.left.name.eq_ignore_ascii_case(k)) else {
            return Err(format!(
                "The key column {k} is not a column of both tables (columns are matched by name, ignoring case)."
            ));
        };
        let class = key_class(columns[i].left.kind, columns[i].right.kind)
            .map_err(|why| format!("The key column {k} cannot be used: {why}."))?;
        idx.push(i);
        classes.push(class);
    }
    Ok(Mapping { columns, key: idx, classes, left_only, right_only, generated })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffKind {
    OnlyLeft,
    OnlyRight,
    Different,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffRow {
    pub kind: DiffKind,
    pub key: Vec<Value>,
    pub left: Option<Vec<Value>>,
    pub right: Option<Vec<Value>>,
    /// Indexes into `Mapping::columns` of the cells that differ (`Different` only).
    pub changed: Vec<usize>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub only_left: u64,
    pub only_right: u64,
    pub different: u64,
    pub same: u64,
}

impl Counts {
    pub fn differences(&self) -> u64 {
        self.only_left + self.only_right + self.different
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MergeError {
    /// A row's key is not greater than the previous one under the client's order.
    Unsorted {
        side: Side,
        previous: Vec<Value>,
        current: Vec<Value>,
    },
    /// The same key twice in one table: the key does not identify a row.
    Duplicate {
        side: Side,
        key: Vec<Value>,
    },
    NullKey {
        side: Side,
    },
}

impl MergeError {
    pub fn describe(&self, left: &str, right: &str) -> String {
        let name = |s: &Side| if *s == Side::Left { left } else { right };
        let show = |k: &[Value]| k.iter().map(Value::display).collect::<Vec<_>>().join(", ");
        match self {
            MergeError::Unsorted { side, previous, current } => format!(
                "{} returned its rows in an order that is not the order this comparison merges in: ({}) came after ({}). The two products sort this key differently.",
                name(side),
                show(current),
                show(previous)
            ),
            MergeError::Duplicate { side, key } => format!(
                "The key ({}) occurs more than once in {}: the chosen key does not identify a row.",
                show(key),
                name(side)
            ),
            MergeError::NullKey { side } => {
                format!(
                    "A row of {} has NULL in a key column: rows without a key cannot be paired.",
                    name(side)
                )
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Left,
    Right,
    Done,
}

pub struct Comparer {
    pub mapping: Mapping,
    pub rules: Rules,
    cap: usize,
    pub counts: Counts,
    /// The first `cap` differing rows in key order.
    pub rows: Vec<DiffRow>,
    /// More rows differed than were kept; `counts` is still exact.
    pub capped: bool,
    prev: [Option<Vec<Value>>; 2],
    /// The row at the front of each stream has been checked against the one before it.
    head_checked: [bool; 2],
}

impl Comparer {
    pub fn new(mapping: Mapping, rules: Rules, cap: usize) -> Comparer {
        Comparer {
            mapping,
            rules,
            cap,
            counts: Counts::default(),
            rows: Vec::new(),
            capped: false,
            prev: [None, None],
            head_checked: [false, false],
        }
    }

    fn key_of(&self, row: &[Value]) -> Vec<Value> {
        self.mapping.key.iter().map(|&i| row[i].clone()).collect()
    }

    fn check(&mut self, side: Side, key: &[Value]) -> Result<(), MergeError> {
        if key.iter().any(Value::is_null) {
            return Err(MergeError::NullKey { side });
        }
        let slot = usize::from(side == Side::Right);
        if let Some(prev) = &self.prev[slot] {
            match cmp_keys(prev, key, &self.mapping.classes) {
                std::cmp::Ordering::Less => {}
                std::cmp::Ordering::Equal => return Err(MergeError::Duplicate { side, key: key.to_vec() }),
                std::cmp::Ordering::Greater => {
                    return Err(MergeError::Unsorted { side, previous: prev.clone(), current: key.to_vec() });
                }
            }
        }
        self.prev[slot] = Some(key.to_vec());
        Ok(())
    }

    fn keep(&mut self, row: DiffRow) {
        if self.rows.len() < self.cap {
            self.rows.push(row);
        } else {
            self.capped = true;
        }
    }

    /// Consume every row that can be decided without more data from either side. `l` and `r` are the rows read
    /// and not yet consumed; `*_done` says the table has no more pages.
    pub fn feed(
        &mut self,
        l: &mut VecDeque<Vec<Value>>,
        l_done: bool,
        r: &mut VecDeque<Vec<Value>>,
        r_done: bool,
    ) -> Result<Need, MergeError> {
        loop {
            match (l.front(), r.front()) {
                (Some(a), Some(b)) => {
                    let (ka, kb) = (self.key_of(a), self.key_of(b));
                    // A row waiting at the front for the other side's next page is checked once.
                    if !self.head_checked[0] {
                        self.check(Side::Left, &ka)?;
                        self.head_checked[0] = true;
                    }
                    if !self.head_checked[1] {
                        self.check(Side::Right, &kb)?;
                        self.head_checked[1] = true;
                    }
                    match cmp_keys(&ka, &kb, &self.mapping.classes) {
                        std::cmp::Ordering::Less => {
                            self.head_checked[0] = false;
                            self.only_left(l.pop_front().expect("front"), ka)
                        }
                        std::cmp::Ordering::Greater => {
                            self.head_checked[1] = false;
                            self.only_right(r.pop_front().expect("front"), kb)
                        }
                        std::cmp::Ordering::Equal => {
                            self.head_checked = [false, false];
                            let (a, b) = (l.pop_front().expect("front"), r.pop_front().expect("front"));
                            self.both(a, b, ka);
                        }
                    }
                }
                (Some(a), None) => {
                    if !r_done {
                        // The head may already have been checked; checking it again would call it a duplicate.
                        if !self.head_checked[0] {
                            let k = self.key_of(a);
                            self.check(Side::Left, &k)?;
                            self.head_checked[0] = true;
                        }
                        return Ok(Need::Right);
                    }
                    let k = self.key_of(a);
                    if !self.head_checked[0] {
                        self.check(Side::Left, &k)?;
                    }
                    self.head_checked[0] = false;
                    self.only_left(l.pop_front().expect("front"), k);
                }
                (None, Some(b)) => {
                    if !l_done {
                        if !self.head_checked[1] {
                            let k = self.key_of(b);
                            self.check(Side::Right, &k)?;
                            self.head_checked[1] = true;
                        }
                        return Ok(Need::Left);
                    }
                    let k = self.key_of(b);
                    if !self.head_checked[1] {
                        self.check(Side::Right, &k)?;
                    }
                    self.head_checked[1] = false;
                    self.only_right(r.pop_front().expect("front"), k);
                }
                (None, None) => {
                    return Ok(match (l_done, r_done) {
                        (true, true) => Need::Done,
                        (false, _) => Need::Left,
                        (true, false) => Need::Right,
                    });
                }
            }
        }
    }

    fn only_left(&mut self, row: Vec<Value>, key: Vec<Value>) {
        self.counts.only_left += 1;
        self.keep(DiffRow { kind: DiffKind::OnlyLeft, key, left: Some(row), right: None, changed: vec![] });
    }

    fn only_right(&mut self, row: Vec<Value>, key: Vec<Value>) {
        self.counts.only_right += 1;
        self.keep(DiffRow { kind: DiffKind::OnlyRight, key, left: None, right: Some(row), changed: vec![] });
    }

    fn both(&mut self, a: Vec<Value>, b: Vec<Value>, key: Vec<Value>) {
        let changed: Vec<usize> = (0..a.len()).filter(|&i| !values_equal(&a[i], &b[i], self.rules)).collect();
        if changed.is_empty() {
            self.counts.same += 1;
        } else {
            self.counts.different += 1;
            self.keep(DiffRow { kind: DiffKind::Different, key, left: Some(a), right: Some(b), changed });
        }
    }
}

// ------------------------------------------------------------------------------------------ the script

#[derive(Debug, Clone, PartialEq)]
pub struct SyncStatement {
    pub sql: String,
    pub kind: DiffKind,
    /// A `DELETE` while deletes are not included.
    pub commented: bool,
    /// How many rows it covers (an `INSERT` carries several).
    pub rows: usize,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SyncScript {
    pub statements: Vec<SyncStatement>,
}

impl SyncScript {
    pub fn render(&self, source: &str, target: &str, capped: bool) -> String {
        let mut out = format!("-- Sync script: make the rows of {target} equal the rows of {source}.\n");
        out.push_str(
            "-- Theta generated this text; nothing in it has been run. Read it before you run it.\n",
        );
        let deletes =
            self.statements.iter().filter(|s| s.kind == DiffKind::OnlyRight).map(|s| s.rows).sum::<usize>();
        if self.statements.iter().any(|s| s.commented) {
            out.push_str(&format!(
                "-- {deletes} DELETE{} {} commented out (rows only in the target); include them in the dialog's toggle to run them.\n",
                if deletes == 1 { "" } else { "s" },
                if deletes == 1 { "is" } else { "are" }
            ));
        }
        if capped {
            out.push_str("-- The comparison kept only the first differences it found, so this script covers those. Run it and compare again for the rest.\n");
        }
        out.push('\n');
        for s in &self.statements {
            let sql = s.sql.trim_end_matches(';');
            if s.commented {
                out.push_str("-- destructive: delete a row that exists only in the target\n");
                for line in sql.lines() {
                    out.push_str(&format!("-- {line}\n"));
                }
                out.push_str("-- ;\n");
            } else {
                out.push_str(&format!("{sql};\n"));
            }
        }
        if self.statements.is_empty() {
            out.push_str("-- Nothing to do: the rows are equal.\n");
        }
        out
    }
}

/// A source value in the form the target column takes (see `copy::convert`, which this mirrors for one column).
fn for_target(v: &Value, col: &ColumnInfo, dst: DatabaseKind) -> Value {
    use crate::copy::to_utc_wall;
    match v {
        Value::Int(i) if col.kind == theta_core::TypeKind::Bool && (*i == 0 || *i == 1) => {
            Value::Bool(*i == 1)
        }
        Value::UInt(u) if col.kind == theta_core::TypeKind::Bool && *u <= 1 => Value::Bool(*u == 1),
        Value::TimestampTz(s) if dst.family() == Family::MySql => Value::Timestamp(to_utc_wall(s)),
        Value::Timestamp(s) if dst.family() == Family::MySql && s.contains('+') => {
            Value::Timestamp(to_utc_wall(s))
        }
        other => other.clone(),
    }
}

/// `DELETE`s (commented unless `include_deletes`), then `UPDATE`s, then `INSERT`s: a key that moved is a delete
/// and an insert, so inserts come last and cannot collide with the row they replace.
pub fn sync_script(
    mapping: &Mapping,
    rows: &[DiffRow],
    dst: DatabaseKind,
    schema: &str,
    table: &str,
    include_deletes: bool,
) -> SyncScript {
    let target = qualified_name(Some(schema).filter(|s| !s.is_empty()), table, dst);
    let key_where = |row: &[Value]| -> String {
        mapping
            .key
            .iter()
            .map(|&i| {
                let c = quote_if_needed(&mapping.columns[i].right.name, dst);
                match &row[i] {
                    Value::Null => format!("{c} IS NULL"),
                    v => format!("{c} = {}", sql_literal(v, dst)),
                }
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    };
    let mut out = SyncScript::default();
    for r in rows.iter().filter(|r| r.kind == DiffKind::OnlyRight) {
        let right = r.right.as_ref().expect("right row");
        out.statements.push(SyncStatement {
            sql: format!("DELETE FROM {target} WHERE {}", key_where(right)),
            kind: DiffKind::OnlyRight,
            commented: !include_deletes,
            rows: 1,
        });
    }
    for r in rows.iter().filter(|r| r.kind == DiffKind::Different) {
        let (left, right) = (r.left.as_ref().expect("left row"), r.right.as_ref().expect("right row"));
        let set: Vec<String> = r
            .changed
            .iter()
            .map(|&i| {
                let c = &mapping.columns[i];
                format!(
                    "{} = {}",
                    quote_if_needed(&c.right.name, dst),
                    sql_literal(&for_target(&left[i], &c.right, dst), dst)
                )
            })
            .collect();
        out.statements.push(SyncStatement {
            sql: format!("UPDATE {target} SET {} WHERE {}", set.join(", "), key_where(right)),
            kind: DiffKind::Different,
            commented: false,
            rows: 1,
        });
    }
    // A target column that is `GENERATED ALWAYS` refuses an explicit value unless told to override.
    let overriding = dst.family() == Family::Postgres
        && mapping.columns.iter().any(|c| c.right.identity == Some(Identity::Always));
    let cols =
        mapping.columns.iter().map(|c| quote_if_needed(&c.right.name, dst)).collect::<Vec<_>>().join(", ");
    let inserts: Vec<&DiffRow> = rows.iter().filter(|r| r.kind == DiffKind::OnlyLeft).collect();
    for chunk in inserts.chunks(100) {
        let values: Vec<String> = chunk
            .iter()
            .map(|r| {
                let left = r.left.as_ref().expect("left row");
                let lits: Vec<String> = mapping
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(i, c)| sql_literal(&for_target(&left[i], &c.right, dst), dst))
                    .collect();
                format!("({})", lits.join(", "))
            })
            .collect();
        out.statements.push(SyncStatement {
            sql: format!(
                "INSERT INTO {target} ({cols}){} VALUES\n    {}",
                if overriding { " OVERRIDING SYSTEM VALUE" } else { "" },
                values.join(",\n    ")
            ),
            kind: DiffKind::OnlyLeft,
            commented: false,
            rows: chunk.len(),
        });
    }
    out
}
