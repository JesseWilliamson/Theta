//! Code intelligence for the editor, as functions of `(text, cursor, dialect, catalog)`: quick fixes, go to
//! definition, usages, occurrence highlights and rename, all built on `theta_sql::analysis` and
//! `theta_sql::assist`.
//!
//! Everything works on the *statement under the cursor* (analysing a 5,000-line script for a key press would
//! be wasted work: aliases, CTEs and their columns never leave their statement). Only the usages of a catalog
//! table or column look at whole scripts, so that they can be collected across tabs. Offsets are byte
//! offsets of the full text; a statement is analysed as a slice and its results are shifted back.

use std::ops::Range;
use theta_core::{Catalog, DatabaseKind};
use theta_sql::analysis::{Analysis, analyze};
use theta_sql::assist::{self, Definition, Edit, Fix, Target};

/// A statement cut out of a script, analysed on its own.
pub struct Local {
    pub analysis: Analysis,
    /// Byte offset of the statement in the full text.
    pub start: usize,
    pub text: String,
}

impl Local {
    /// Analyse the statement that contains (or touches) `cursor`.
    pub fn at(text: &str, cursor: usize, kind: DatabaseKind, catalog: Option<&Catalog>) -> Option<Local> {
        let span = theta_sql::statement_at(text, cursor, kind)?;
        let stmt = text.get(span.range.clone())?.to_string();
        let analysis = analyze(&stmt, kind, catalog);
        Some(Local { analysis, start: span.range.start, text: stmt })
    }

    /// The cursor as an offset into the statement (clamped, since a cursor in trailing whitespace is past it).
    pub fn rel(&self, cursor: usize) -> usize {
        cursor.saturating_sub(self.start).min(self.text.len())
    }

    pub fn shift(&self, r: Range<usize>) -> Range<usize> {
        r.start + self.start..r.end + self.start
    }

    fn shift_edits(&self, edits: Vec<Edit>) -> Vec<(Range<usize>, String)> {
        edits.into_iter().map(|e| (self.shift(e.range), e.text)).collect()
    }
}

/// A quick fix or refactoring with its edits in the coordinates of the full text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub title: String,
    pub kind: assist::FixKind,
    pub edits: Vec<(Range<usize>, String)>,
}

pub fn code_actions(text: &str, cursor: usize, kind: DatabaseKind, catalog: Option<&Catalog>) -> Vec<Action> {
    let Some(local) = Local::at(text, cursor, kind, catalog) else { return Vec::new() };
    let fixes: Vec<Fix> =
        assist::code_actions(&local.text, kind, catalog, &local.analysis, local.rel(cursor));
    fixes
        .into_iter()
        .map(|f| Action { title: f.title, kind: f.kind, edits: local.shift_edits(f.edits) })
        .collect()
}

/// Where the name under the cursor is defined; a place in the text is given in full-text offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Where {
    Here(Range<usize>),
    Table(theta_sql::analysis::TableId),
    Column { table: theta_sql::analysis::TableId, column: String },
    Function(String),
}

pub fn definition(text: &str, cursor: usize, kind: DatabaseKind, catalog: Option<&Catalog>) -> Option<Where> {
    let local = Local::at(text, cursor, kind, catalog)?;
    Some(match assist::definition_at(&local.analysis, local.rel(cursor))? {
        Definition::Here(r) => Where::Here(local.shift(r)),
        Definition::Table(t) => Where::Table(t),
        Definition::Column { table, column } => Where::Column { table, column },
        Definition::Function(f) => Where::Function(f),
    })
}

/// The occurrences of the name under the cursor, for highlighting. Empty for a name used once.
pub fn highlights(
    text: &str,
    cursor: usize,
    kind: DatabaseKind,
    catalog: Option<&Catalog>,
) -> Vec<Range<usize>> {
    let Some(local) = Local::at(text, cursor, kind, catalog) else { return Vec::new() };
    assist::highlights_at(&local.analysis, local.rel(cursor)).into_iter().map(|r| local.shift(r)).collect()
}

/// What the usages list is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageTarget {
    /// A name of this statement: its occurrences are in this text only.
    Local { name: String, ranges: Vec<Range<usize>> },
    /// A table or column of the catalog: look for it in every script.
    Catalog(Target),
}

pub fn usage_target(
    text: &str,
    cursor: usize,
    kind: DatabaseKind,
    catalog: Option<&Catalog>,
) -> Option<UsageTarget> {
    let local = Local::at(text, cursor, kind, catalog)?;
    let (target, span) = assist::target_at(&local.analysis, local.rel(cursor))?;
    match target {
        Target::Symbol(_) => {
            let ranges = assist::usages_of(&local.analysis, &target)
                .into_iter()
                .map(|u| local.shift(u.range))
                .collect();
            Some(UsageTarget::Local { name: local.text[span].to_string(), ranges })
        }
        Target::Table(_) | Target::Column { .. } => Some(UsageTarget::Catalog(target)),
        Target::Function(_) => None,
    }
}

/// Every use of a catalog table or column in a whole script.
pub fn catalog_usages(
    text: &str,
    kind: DatabaseKind,
    catalog: Option<&Catalog>,
    target: &Target,
) -> Vec<Range<usize>> {
    let analysis = analyze(text, kind, catalog);
    assist::usages_of(&analysis, target).into_iter().map(|u| u.range).collect()
}

/// The name that rename would change, or why nothing can be renamed here.
pub fn rename_prepare(
    text: &str,
    cursor: usize,
    kind: DatabaseKind,
    catalog: Option<&Catalog>,
) -> Result<String, String> {
    let local = Local::at(text, cursor, kind, catalog).ok_or("There is no statement here")?;
    assist::rename_prepare(&local.analysis, local.rel(cursor)).map(|(name, _)| name)
}

/// The edits that rename the name under the cursor, in full-text offsets, or the reason they cannot be made.
pub fn rename(
    text: &str,
    cursor: usize,
    kind: DatabaseKind,
    catalog: Option<&Catalog>,
    new_name: &str,
) -> Result<Vec<(Range<usize>, String)>, String> {
    let local = Local::at(text, cursor, kind, catalog).ok_or("There is no statement here")?;
    let edits = assist::rename_at(&local.analysis, &local.text, kind, local.rel(cursor), new_name)?;
    Ok(local.shift_edits(edits))
}

/// Apply byte-range edits to `text` (for tests and for previews).
pub fn apply(text: &str, edits: &[(Range<usize>, String)]) -> String {
    let e: Vec<Edit> = edits.iter().map(|(r, t)| Edit::replace(r.clone(), t.clone())).collect();
    assist::apply_edits(text, &e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use DatabaseKind::Postgres as Pg;
    use theta_sql::fixtures::shop;

    const SCRIPT: &str = "select 1;\n\nselect emial from customers c where c.id = 1;\n\nwith t as (select id from orders) select t.id from t;\n";

    #[test]
    fn actions_are_found_in_the_statement_under_the_cursor_and_shifted_back() {
        let cat = shop();
        let at = SCRIPT.find("emial").unwrap() + 2;
        let actions = code_actions(SCRIPT, at, Pg, Some(&cat));
        let fix =
            actions.iter().find(|a| a.title.contains("Change 'emial' to 'email'")).expect("did-you-mean");
        assert_eq!(apply(SCRIPT, &fix.edits), SCRIPT.replace("emial", "email"));
        // the first statement has nothing to fix
        assert!(code_actions(SCRIPT, 3, Pg, Some(&cat)).iter().all(|a| a.kind != assist::FixKind::QuickFix));
    }

    #[test]
    fn definition_and_rename_work_on_the_statement_only() {
        let cat = shop();
        let at = SCRIPT.rfind("t.id").unwrap();
        match definition(SCRIPT, at, Pg, Some(&cat)) {
            Some(Where::Here(r)) => assert_eq!(&SCRIPT[r], "t"),
            other => panic!("{other:?}"),
        }
        let edits = rename(SCRIPT, at, Pg, Some(&cat), "ids").unwrap();
        assert_eq!(
            apply(SCRIPT, &edits),
            SCRIPT.replace("with t as", "with ids as").replace("t.id from t;", "ids.id from ids;")
        );
        assert_eq!(rename_prepare(SCRIPT, at, Pg, Some(&cat)).unwrap(), "t");
        // catalog objects are refused with the pointer to the table editor
        let at = SCRIPT.find("customers").unwrap() + 1;
        assert!(rename(SCRIPT, at, Pg, Some(&cat), "x").unwrap_err().contains("table editor"));
    }

    #[test]
    fn usages_of_catalog_objects_are_collected_per_script() {
        let cat = shop();
        let at = SCRIPT.find("customers").unwrap() + 1;
        let Some(UsageTarget::Catalog(t)) = usage_target(SCRIPT, at, Pg, Some(&cat)) else { panic!() };
        assert_eq!(catalog_usages(SCRIPT, Pg, Some(&cat), &t).len(), 1);
        let other = "select * from customers; select name from customers;";
        assert_eq!(catalog_usages(other, Pg, Some(&cat), &t).len(), 2);
        let at = SCRIPT.find("c.id").unwrap() + 2;
        assert!(matches!(
            usage_target(SCRIPT, at, Pg, Some(&cat)),
            Some(UsageTarget::Catalog(Target::Column { .. }))
        ));
        // an alias is local
        let at = SCRIPT.find("c.id").unwrap();
        match usage_target(SCRIPT, at, Pg, Some(&cat)) {
            Some(UsageTarget::Local { name, ranges }) => {
                assert_eq!(name, "c");
                assert_eq!(ranges.len(), 2);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn highlights_cover_the_name() {
        let cat = shop();
        let at = SCRIPT.find("c.id").unwrap();
        let h = highlights(SCRIPT, at, Pg, Some(&cat));
        assert_eq!(h.len(), 2);
        assert!(h.iter().all(|r| &SCRIPT[r.clone()] == "c"));
    }
}
