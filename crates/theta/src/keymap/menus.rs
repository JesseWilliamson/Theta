//! The application menu, written once: the hamburger menu in the title bar and the macOS menu bar are both
//! built from [`groups`].
//!
//! The rule that keeps the two menus apart: the hamburger (and the menu bar) is the *application* menu and
//! holds every command, sorted into File, Edit, View, Connection, Query, Tools and Help; the gear menu holds
//! only Settings and appearance (theme, keymap, font size). Key hints come from the bindings in force, the
//! same source as the command palette, so a command shows one key everywhere.

use crate::actions::*;
use crate::names;
use gpui_kit::Action;

pub enum Entry {
    Item(String, fn() -> Box<dyn Action>),
    Separator,
}

pub struct Group {
    pub name: &'static str,
    pub entries: Vec<Entry>,
}

macro_rules! item {
    ($label:expr, $action:expr) => {
        Entry::Item($label.to_string(), || Box::new($action))
    };
}

pub fn groups() -> Vec<Group> {
    use Entry::Separator as Sep;
    vec![
        Group {
            name: "File",
            entries: vec![
                item!("New Console", NewConsole),
                item!(names::command("New Connection"), NewConnection),
                item!(names::command("Open SQL File"), OpenSqlFile),
                item!(names::command("Save Console As"), SaveConsole),
                Sep,
                item!("Close Tab", CloseTab),
                item!("Reopen Closed Tab", ReopenClosedTab),
                Sep,
                item!("Quit", Quit),
            ],
        },
        Group {
            name: "Edit",
            entries: vec![
                item!("Format Document", FormatDocument),
                item!(names::command("Surround Selection with"), SurroundWith),
                item!(names::command("Edit Live Templates"), EditLiveTemplates),
                Sep,
                item!("Copy Cell Value", CopyCellValue),
            ],
        },
        Group {
            name: "View",
            entries: vec![
                item!(names::command("Command Palette"), OpenCommandPalette),
                item!(names::command("Go to Table"), OpenTablePicker),
                Sep,
                item!("Toggle Explorer", ToggleExplorer),
                item!("Toggle Bottom Panel", ToggleBottomPanel),
                item!("Focus Explorer", FocusExplorer),
                item!("Focus Editor", FocusEditor),
                item!("Focus Results", FocusResults),
                Sep,
                item!("Show Chart", ShowChart),
                item!("Show History", ShowHistory),
                item!("Toggle Cell Viewer", ToggleCellViewer),
            ],
        },
        Group {
            name: "Connection",
            entries: vec![
                item!(names::command("Connect"), ConnectTo),
                item!("Disconnect", Disconnect),
                item!("Refresh Schema", RefreshSchema),
                Sep,
                item!(names::command(names::SESSIONS), OpenSessions),
                item!(names::command(names::ROLES), OpenRoles),
                item!(names::command(names::EXPORT_DATABASE), ExportDatabase),
                item!(names::command(names::RUN_SQL_SCRIPT), RunSqlScript),
            ],
        },
        Group {
            name: "Query",
            entries: vec![
                item!("Run Statement", RunStatement),
                item!("Run All", RunAll),
                item!(names::command(names::RUN_ON_SEVERAL), RunOnSeveral),
                item!("Cancel Query", CancelQuery),
                Sep,
                item!("Explain", Explain),
                item!("Explain Analyze", ExplainAnalyze),
                Sep,
                item!("Begin Transaction", BeginTransaction),
                item!("Commit Transaction", CommitTransaction),
                item!("Roll Back Transaction", RollbackTransaction),
                Sep,
                item!(names::command(names::LOCAL_HISTORY), ShowLocalHistory),
            ],
        },
        Group {
            name: "Tools",
            entries: vec![
                item!(names::command(names::IMPORT_DATA), ImportData),
                item!(names::command(names::IMPORT_NEW_TABLE), ImportAsNewTable),
                item!(names::command(names::EXPORT_TABLE_DATA), ExportTableData),
                item!(names::command(names::SEARCH_TABLES), SearchTables),
                Sep,
                item!(names::command(names::ER_DIAGRAM), ShowDiagram),
                item!(names::command(names::COMPARE_SCHEMAS), CompareSchemas),
                item!(names::command(names::COMPARE_DATA), CompareData),
                Sep,
                item!(names::command(names::NEW_TABLE), NewTable),
                item!(names::command(names::MODIFY_TABLE), ModifyTable),
                item!(names::command(names::COPY_TABLE), CopyTable),
            ],
        },
        Group {
            name: "Help",
            entries: vec![
                item!("Open User Guide", OpenUserGuide),
                item!("Open Key Reference", OpenKeyReference),
                item!("Open Changelog", OpenChangelog),
                item!("Open Website", OpenWebsite),
                item!(names::command("Report an Issue"), OpenIssueTracker),
                Sep,
                item!("Open Config Folder", OpenConfigFolder),
                item!("About Theta", ShowAbout),
            ],
        },
    ]
}

/// The gear menu: settings and how the application looks.
pub fn appearance() -> Vec<Entry> {
    use Entry::Separator as Sep;
    vec![
        item!(names::command("Settings"), OpenSettings),
        item!(names::command("Select Theme"), SelectTheme),
        item!(names::command("Select Keymap"), SelectKeymap),
        item!(names::command("Import Zed Theme"), ImportZedTheme),
        Sep,
        item!("Increase Editor Font Size", ZoomIn),
        item!("Decrease Editor Font Size", ZoomOut),
        item!("Reset Editor Font Size", ZoomReset),
        Sep,
        item!("Open Settings File", OpenSettingsFile),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_command_is_in_both_menus_and_menu_names_are_unique() {
        let gear: Vec<String> = appearance().into_iter().filter_map(|e| match e { Entry::Item(l, _) => Some(l), _ => None }).collect();
        let mut seen = std::collections::HashSet::new();
        for g in groups() {
            for e in g.entries {
                if let Entry::Item(label, _) = e {
                    assert!(!gear.contains(&label), "`{label}` is in the gear menu and the application menu");
                    assert!(seen.insert(label.clone()), "`{label}` appears twice in the application menu");
                }
            }
        }
    }

    #[test]
    fn every_menu_label_is_a_palette_command() {
        let palette = crate::palette::command_labels();
        let all = groups().into_iter().flat_map(|g| g.entries).chain(appearance());
        for e in all {
            if let Entry::Item(label, _) = e {
                // The palette does not list itself.
                if label == "Command Palette…" {
                    continue;
                }
                assert!(palette.iter().any(|p| *p == label), "the palette has no `{label}`");
            }
        }
    }
}
