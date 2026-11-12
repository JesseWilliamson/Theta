//! Staged edits: what the user changed but has not submitted yet.
//!
//! Original rows are never mutated; edits live in side tables keyed by the
//! original row index so key values for `WHERE` clauses always come from the
//! pre-edit data. Pure bookkeeping, no GPUI types.

use crate::types::{EditTarget, RowChange};
use std::collections::{BTreeMap, BTreeSet};
use theta_core::{ResultSet, Value};

/// A row as the grid sees it: an original result row or a staged new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowRef {
    Orig(usize),
    New(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellState {
    Normal,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    Normal,
    New,
    Deleted,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct NewRow {
    /// `None` = the user did not fill this column in (the database default applies).
    values: Vec<Option<Value>>,
}

#[derive(Debug, Clone, PartialEq)]
enum Undo {
    Edit { row: usize, col: usize, prev: Option<Value> },
    NewCell { idx: usize, col: usize, prev: Option<Value> },
    AddedRow,
    DeletedOrig(usize),
    DeletedNew { idx: usize, row: NewRow },
}

#[derive(Debug, Clone)]
pub struct Pending {
    target: EditTarget,
    ncols: usize,
    edits: BTreeMap<(usize, usize), Value>,
    deleted: BTreeSet<usize>,
    new_rows: Vec<NewRow>,
    undo: Vec<Undo>,
}

static NULL: Value = Value::Null;

impl Pending {
    pub fn new(target: EditTarget, ncols: usize) -> Self {
        Pending {
            target,
            ncols,
            edits: BTreeMap::new(),
            deleted: BTreeSet::new(),
            new_rows: Vec::new(),
            undo: Vec::new(),
        }
    }

    pub fn target(&self) -> &EditTarget {
        &self.target
    }

    pub fn can_edit_column(&self, col: usize) -> bool {
        self.target.editable_columns.contains(&col)
    }

    pub fn new_row_count(&self) -> usize {
        self.new_rows.len()
    }

    pub fn row_state(&self, row: RowRef) -> RowState {
        match row {
            RowRef::New(_) => RowState::New,
            RowRef::Orig(r) if self.deleted.contains(&r) => RowState::Deleted,
            RowRef::Orig(_) => RowState::Normal,
        }
    }

    pub fn cell_state(&self, row: RowRef, col: usize) -> CellState {
        match row {
            RowRef::Orig(r) if self.edits.contains_key(&(r, col)) => CellState::Modified,
            _ => CellState::Normal,
        }
    }

    /// For new rows: whether the user has filled this column in.
    pub fn new_cell_filled(&self, idx: usize, col: usize) -> bool {
        self.new_rows.get(idx).and_then(|r| r.values.get(col)).is_some_and(Option::is_some)
    }

    /// The value to display: staged edit, else the original, else NULL.
    pub fn effective<'a>(&'a self, rs: &'a ResultSet, row: RowRef, col: usize) -> &'a Value {
        match row {
            RowRef::Orig(r) => self
                .edits
                .get(&(r, col))
                .or_else(|| rs.rows.get(r).and_then(|row| row.get(col)))
                .unwrap_or(&NULL),
            RowRef::New(i) => {
                self.new_rows.get(i).and_then(|r| r.values.get(col)).and_then(Option::as_ref).unwrap_or(&NULL)
            }
        }
    }

    /// Stage `value` for a cell. Setting a cell back to its original value
    /// removes the edit, so a no-op round trip leaves no pending change.
    /// Returns false (and stages nothing) when the column is not editable or
    /// the row is deleted.
    pub fn set_cell(&mut self, rs: &ResultSet, row: RowRef, col: usize, value: Value) -> bool {
        if !self.can_edit_column(col) || col >= self.ncols {
            return false;
        }
        match row {
            RowRef::Orig(r) => {
                if self.deleted.contains(&r) || r >= rs.rows.len() {
                    return false;
                }
                let original = rs.rows[r].get(col).unwrap_or(&NULL);
                let prev = self.edits.get(&(r, col)).cloned();
                let current = prev.as_ref().unwrap_or(original);
                if *current == value {
                    return true;
                }
                if value == *original {
                    self.edits.remove(&(r, col));
                } else {
                    self.edits.insert((r, col), value);
                }
                self.undo.push(Undo::Edit { row: r, col, prev });
                true
            }
            RowRef::New(i) => {
                let ncols = self.ncols;
                let Some(nr) = self.new_rows.get_mut(i) else { return false };
                nr.values.resize(ncols, None);
                let new = if value.is_null() { None } else { Some(value) };
                if nr.values[col] == new {
                    return true;
                }
                let prev = std::mem::replace(&mut nr.values[col], new);
                self.undo.push(Undo::NewCell { idx: i, col, prev });
                true
            }
        }
    }

    pub fn add_row(&mut self) -> usize {
        self.new_rows.push(NewRow { values: vec![None; self.ncols] });
        self.undo.push(Undo::AddedRow);
        self.new_rows.len() - 1
    }

    /// Mark an original row deleted, or drop a staged new row outright.
    pub fn delete_row(&mut self, row: RowRef) {
        match row {
            RowRef::Orig(r) => {
                if self.deleted.insert(r) {
                    self.undo.push(Undo::DeletedOrig(r));
                }
            }
            RowRef::New(i) => {
                if i < self.new_rows.len() {
                    let row = self.new_rows.remove(i);
                    self.undo.push(Undo::DeletedNew { idx: i, row });
                }
            }
        }
    }

    /// Undo the most recent staged edit; false when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(op) = self.undo.pop() else { return false };
        match op {
            Undo::Edit { row, col, prev } => match prev {
                Some(v) => {
                    self.edits.insert((row, col), v);
                }
                None => {
                    self.edits.remove(&(row, col));
                }
            },
            Undo::NewCell { idx, col, prev } => {
                if let Some(nr) = self.new_rows.get_mut(idx) {
                    nr.values[col] = prev;
                }
            }
            Undo::AddedRow => {
                self.new_rows.pop();
            }
            Undo::DeletedOrig(r) => {
                self.deleted.remove(&r);
            }
            Undo::DeletedNew { idx, row } => {
                self.new_rows.insert(idx.min(self.new_rows.len()), row);
            }
        }
        true
    }

    /// Discard everything.
    pub fn revert(&mut self) {
        self.edits.clear();
        self.deleted.clear();
        self.new_rows.clear();
        self.undo.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty() && self.deleted.is_empty() && self.new_rows.is_empty()
    }

    /// Number of `RowChange`s `changes` would produce.
    pub fn count(&self) -> usize {
        let edited_rows: BTreeSet<usize> =
            self.edits.keys().map(|(r, _)| *r).filter(|r| !self.deleted.contains(r)).collect();
        edited_rows.len() + self.deleted.len() + self.new_rows.len()
    }

    fn col_name(rs: &ResultSet, col: usize) -> String {
        rs.columns
            .get(col)
            .map(|c| c.origin.as_ref().map_or_else(|| c.name.clone(), |o| o.column.clone()))
            .unwrap_or_default()
    }

    fn key_of(&self, rs: &ResultSet, row: usize) -> Vec<(String, Value)> {
        self.target
            .key_columns
            .iter()
            .map(|&c| (Self::col_name(rs, c), rs.rows[row].get(c).cloned().unwrap_or(Value::Null)))
            .collect()
    }

    /// The staged changes in submit order: updates, deletes, inserts.
    /// Edits of a row that is also deleted are dropped (the delete wins).
    pub fn changes(&self, rs: &ResultSet) -> Vec<RowChange> {
        let mut out = Vec::new();
        let mut updates: BTreeMap<usize, Vec<(String, Value)>> = BTreeMap::new();
        for ((r, c), v) in &self.edits {
            if self.deleted.contains(r) {
                continue;
            }
            updates.entry(*r).or_default().push((Self::col_name(rs, *c), v.clone()));
        }
        for (row, set) in updates {
            out.push(RowChange::Update { row, key: self.key_of(rs, row), set });
        }
        for &row in &self.deleted {
            out.push(RowChange::Delete { row, key: self.key_of(rs, row) });
        }
        for nr in &self.new_rows {
            let values = nr
                .values
                .iter()
                .enumerate()
                .filter_map(|(c, v)| v.clone().map(|v| (Self::col_name(rs, c), v)))
                .collect();
            out.push(RowChange::Insert { values });
        }
        out
    }

    /// Fold the staged changes into `rs` after a successful submit: edits
    /// overwrite, deleted rows disappear, new rows are appended (unfilled
    /// columns become NULL since the database's defaults are unknown here).
    /// Clears all pending state. Returns the old-to-new row index mapping is
    /// not needed: callers rebuild their view.
    pub fn apply_to(&mut self, rs: &mut ResultSet) {
        self.apply_stored(rs, &[]);
    }

    /// Like [`Pending::apply_to`], with what the database stored for each inserted row, in insert
    /// order: a `Some` cell replaces what the user typed or left empty (a generated key, a default,
    /// a computed column), a `None` cell or row keeps it.
    pub fn apply_stored(&mut self, rs: &mut ResultSet, stored: &[Option<Vec<Option<Value>>>]) {
        for ((r, c), v) in std::mem::take(&mut self.edits) {
            if let Some(cell) = rs.rows.get_mut(r).and_then(|row| row.get_mut(c)) {
                *cell = v;
            }
        }
        if !self.deleted.is_empty() {
            let deleted = std::mem::take(&mut self.deleted);
            let mut i = 0;
            rs.rows.retain(|_| {
                let keep = !deleted.contains(&i);
                i += 1;
                keep
            });
        }
        for (k, nr) in std::mem::take(&mut self.new_rows).into_iter().enumerate() {
            let mut row: Vec<Value> = nr.values.into_iter().map(|v| v.unwrap_or(Value::Null)).collect();
            if let Some(Some(back)) = stored.get(k) {
                for (cell, v) in row.iter_mut().zip(back) {
                    if let Some(v) = v {
                        *cell = v.clone();
                    }
                }
            }
            rs.rows.push(row);
        }
        self.undo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theta_core::{Column, ColumnOrigin, TypeKind};

    fn col(name: &str, origin: Option<&str>) -> Column {
        Column {
            name: name.into(),
            type_name: "t".into(),
            kind: TypeKind::Text,
            origin: origin.map(|c| ColumnOrigin { schema: None, table: "t".into(), column: c.into() }),
        }
    }

    fn fixture() -> (ResultSet, Pending) {
        let rs = ResultSet {
            columns: vec![col("id", Some("id")), col("full_name", Some("name")), col("note", None)],
            rows: vec![
                vec![Value::Int(1), Value::Text("Ann".into()), Value::Null],
                vec![Value::Int(2), Value::Text("Bob".into()), Value::Text("x".into())],
                vec![Value::Int(3), Value::Text("Cy".into()), Value::Null],
            ],
            truncated: false,
        };
        let p = Pending::new(EditTarget::new(None, "t", vec![0], vec![1, 2]), 3);
        (rs, p)
    }

    #[test]
    fn stored_values_replace_the_typed_row() {
        let (mut rs, mut p) = fixture();
        p.add_row();
        p.set_cell(&rs, RowRef::New(0), 1, Value::Text("New".into()));
        p.apply_stored(&mut rs, &[Some(vec![Some(Value::Int(4)), None, Some(Value::Text("dflt".into()))])]);
        assert_eq!(rs.rows[3], vec![Value::Int(4), Value::Text("New".into()), Value::Text("dflt".into())]);
    }

    #[test]
    fn edit_then_revert_to_original_is_no_change() {
        let (rs, mut p) = fixture();
        assert!(p.set_cell(&rs, RowRef::Orig(0), 1, Value::Text("Anna".into())));
        assert_eq!(p.count(), 1);
        assert_eq!(p.cell_state(RowRef::Orig(0), 1), CellState::Modified);
        assert!(p.set_cell(&rs, RowRef::Orig(0), 1, Value::Text("Ann".into())));
        assert!(p.is_empty());
        assert_eq!(p.count(), 0);
        assert!(p.changes(&rs).is_empty());
        assert_eq!(p.cell_state(RowRef::Orig(0), 1), CellState::Normal);
    }

    #[test]
    fn setting_the_current_value_stages_nothing() {
        let (rs, mut p) = fixture();
        p.set_cell(&rs, RowRef::Orig(1), 1, Value::Text("Bob".into()));
        assert!(p.is_empty());
        assert!(!p.undo(), "a no-op leaves no undo entry");
    }

    #[test]
    fn key_values_come_from_the_original_row() {
        let (rs, mut p) = fixture();
        // edit the primary key itself and another column
        let mut pk = Pending::new(EditTarget::new(None, "t", vec![0], vec![0, 1]), 3);
        pk.set_cell(&rs, RowRef::Orig(1), 0, Value::Int(99));
        pk.set_cell(&rs, RowRef::Orig(1), 1, Value::Text("Rob".into()));
        assert_eq!(
            pk.changes(&rs),
            vec![RowChange::Update {
                row: 1,
                key: vec![("id".into(), Value::Int(2))],
                set: vec![("id".into(), Value::Int(99)), ("name".into(), Value::Text("Rob".into()))],
            }]
        );
        // names use the origin column ("name"), not the alias ("full_name")
        p.set_cell(&rs, RowRef::Orig(0), 1, Value::Text("Z".into()));
        let RowChange::Update { set, .. } = &p.changes(&rs)[0] else { panic!() };
        assert_eq!(set[0].0, "name");
        // a column without origin falls back to its result name
        p.set_cell(&rs, RowRef::Orig(0), 2, Value::Text("n".into()));
        let RowChange::Update { set, .. } = &p.changes(&rs)[0] else { panic!() };
        assert_eq!(set[1].0, "note");
    }

    #[test]
    fn multiple_cells_of_one_row_make_one_update() {
        let (rs, mut p) = fixture();
        p.set_cell(&rs, RowRef::Orig(2), 1, Value::Text("C".into()));
        p.set_cell(&rs, RowRef::Orig(2), 2, Value::Text("n".into()));
        assert_eq!(p.count(), 1);
        assert_eq!(p.changes(&rs).len(), 1);
    }

    #[test]
    fn readonly_columns_and_deleted_rows_reject_edits() {
        let (rs, mut p) = fixture();
        assert!(!p.set_cell(&rs, RowRef::Orig(0), 0, Value::Int(5)), "id is not editable");
        p.delete_row(RowRef::Orig(0));
        assert!(!p.set_cell(&rs, RowRef::Orig(0), 1, Value::Text("x".into())));
    }

    #[test]
    fn delete_wins_over_edit_and_undo_restores_both() {
        let (rs, mut p) = fixture();
        p.set_cell(&rs, RowRef::Orig(0), 1, Value::Text("Anna".into()));
        p.delete_row(RowRef::Orig(0));
        assert_eq!(p.count(), 1);
        assert_eq!(
            p.changes(&rs),
            vec![RowChange::Delete { row: 0, key: vec![("id".into(), Value::Int(1))] }]
        );
        assert_eq!(p.row_state(RowRef::Orig(0)), RowState::Deleted);
        assert!(p.undo());
        assert_eq!(p.row_state(RowRef::Orig(0)), RowState::Normal);
        assert_eq!(p.count(), 1);
        assert!(p.undo());
        assert!(p.is_empty());
        assert!(!p.undo());
    }

    #[test]
    fn new_rows_stage_only_filled_columns() {
        let (rs, mut p) = fixture();
        let i = p.add_row();
        assert_eq!(p.count(), 1);
        p.set_cell(&rs, RowRef::New(i), 1, Value::Text("Dee".into()));
        assert!(p.new_cell_filled(i, 1));
        assert!(!p.new_cell_filled(i, 2));
        assert_eq!(
            p.changes(&rs),
            vec![RowChange::Insert { values: vec![("name".into(), Value::Text("Dee".into()))] }]
        );
        assert_eq!(p.effective(&rs, RowRef::New(i), 1), &Value::Text("Dee".into()));
        assert_eq!(p.effective(&rs, RowRef::New(i), 2), &Value::Null);
        // clearing a value unfills the column
        p.set_cell(&rs, RowRef::New(i), 1, Value::Null);
        assert!(!p.new_cell_filled(i, 1));
    }

    #[test]
    fn deleting_a_new_row_removes_it_and_undo_brings_it_back() {
        let (rs, mut p) = fixture();
        let i = p.add_row();
        p.set_cell(&rs, RowRef::New(i), 1, Value::Text("Dee".into()));
        p.delete_row(RowRef::New(i));
        assert_eq!(p.new_row_count(), 0);
        assert!(p.is_empty());
        assert!(p.undo());
        assert_eq!(p.new_row_count(), 1);
        assert_eq!(p.effective(&rs, RowRef::New(0), 1), &Value::Text("Dee".into()));
    }

    #[test]
    fn undo_is_ordered() {
        let (rs, mut p) = fixture();
        p.set_cell(&rs, RowRef::Orig(0), 1, Value::Text("A1".into()));
        p.set_cell(&rs, RowRef::Orig(0), 1, Value::Text("A2".into()));
        assert_eq!(p.effective(&rs, RowRef::Orig(0), 1), &Value::Text("A2".into()));
        p.undo();
        assert_eq!(p.effective(&rs, RowRef::Orig(0), 1), &Value::Text("A1".into()));
        p.undo();
        assert_eq!(p.effective(&rs, RowRef::Orig(0), 1), &Value::Text("Ann".into()));
    }

    #[test]
    fn change_order_is_update_delete_insert() {
        let (rs, mut p) = fixture();
        let i = p.add_row();
        p.set_cell(&rs, RowRef::New(i), 1, Value::Text("N".into()));
        p.delete_row(RowRef::Orig(2));
        p.set_cell(&rs, RowRef::Orig(1), 2, Value::Text("y".into()));
        let kinds: Vec<&str> = p
            .changes(&rs)
            .iter()
            .map(|c| match c {
                RowChange::Update { .. } => "u",
                RowChange::Delete { .. } => "d",
                RowChange::Insert { .. } => "i",
            })
            .collect();
        assert_eq!(kinds, ["u", "d", "i"]);
        assert_eq!(p.count(), 3);
    }

    #[test]
    fn apply_folds_changes_into_the_result() {
        let (mut rs, mut p) = fixture();
        p.set_cell(&rs, RowRef::Orig(0), 1, Value::Text("Anna".into()));
        p.delete_row(RowRef::Orig(1));
        let i = p.add_row();
        p.set_cell(&rs, RowRef::New(i), 1, Value::Text("Dee".into()));
        p.apply_to(&mut rs);
        assert!(p.is_empty());
        assert_eq!(rs.rows.len(), 3);
        assert_eq!(rs.rows[0][1], Value::Text("Anna".into()));
        assert_eq!(rs.rows[1][0], Value::Int(3));
        assert_eq!(rs.rows[2], vec![Value::Null, Value::Text("Dee".into()), Value::Null]);
    }

    #[test]
    fn revert_clears_everything() {
        let (rs, mut p) = fixture();
        p.set_cell(&rs, RowRef::Orig(0), 1, Value::Text("x".into()));
        p.add_row();
        p.delete_row(RowRef::Orig(2));
        p.revert();
        assert!(p.is_empty());
        assert!(!p.undo());
    }
}
