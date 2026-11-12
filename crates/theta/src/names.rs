//! One name for every command that is reachable from more than one place.
//!
//! The command palette, the explorer's context menus, the dialog titles and the tab titles all print
//! these constants, so "Export Table Data" cannot be "Export Data" in one of them. Menus and the palette
//! append an ellipsis where the command asks for more input ([`command`]); titles do not.

pub const EXPORT_TABLE_DATA: &str = "Export Table Data";
pub const EXPORT_DATABASE: &str = "Export Database";
pub const IMPORT_DATA: &str = "Import Data";
pub const IMPORT_NEW_TABLE: &str = "Import Data as New Table";
pub const NEW_TABLE: &str = "New Table";
pub const MODIFY_TABLE: &str = "Modify Table";
pub const COPY_TABLE: &str = "Copy Table";
pub const ROLES: &str = "Roles and Users";
pub const SESSIONS: &str = "Sessions";
pub const RUN_SQL_SCRIPT: &str = "Run SQL Script";
pub const SEARCH_TABLES: &str = "Search Across Tables";
pub const ER_DIAGRAM: &str = "ER Diagram";
pub const COMPARE_SCHEMAS: &str = "Compare Schemas";
pub const COMPARE_DATA: &str = "Compare Table Data";
pub const LOCAL_HISTORY: &str = "Local History";
pub const RUN_ON_SEVERAL: &str = "Run on Several Connections";

/// The menu and palette spelling of a command that opens a dialog or picker: the name and an ellipsis.
pub fn command(name: &str) -> String {
    format!("{name}…")
}

/// Every name above, for the test that keeps them in Title Case.
#[cfg(test)]
const ALL: &[&str] = &[
    EXPORT_TABLE_DATA,
    EXPORT_DATABASE,
    IMPORT_DATA,
    IMPORT_NEW_TABLE,
    NEW_TABLE,
    MODIFY_TABLE,
    COPY_TABLE,
    ROLES,
    SESSIONS,
    RUN_SQL_SCRIPT,
    SEARCH_TABLES,
    ER_DIAGRAM,
    COMPARE_SCHEMAS,
    COMPARE_DATA,
    LOCAL_HISTORY,
    RUN_ON_SEVERAL,
];

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: &[&str] = &["and", "on", "to", "of", "from", "in", "a", "the", "as"];

    #[test]
    fn names_are_title_case_without_ellipsis() {
        for name in ALL {
            assert!(!name.ends_with('…') && !name.contains("..."), "{name}");
            for (i, word) in name.split(' ').enumerate() {
                let first = word.chars().next().unwrap();
                assert!(
                    first.is_uppercase() || (i > 0 && SMALL.contains(&word)),
                    "`{name}`: `{word}` should be capitalised"
                );
            }
        }
    }

    #[test]
    fn every_palette_command_that_has_a_shared_name_uses_it() {
        let labels = crate::palette::command_labels();
        for name in ALL {
            let shown = command(name);
            assert!(
                labels.iter().any(|l| *l == shown || l == name),
                "the palette has no command called `{name}`"
            );
        }
    }
}
