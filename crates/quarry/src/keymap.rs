//! Default key bindings and the macOS application menu.
//!
//! `cmd` on macOS, `ctrl` elsewhere. Contexts: "Explorer" is set on the tree;
//! the extra "helix" identifier is added there while the Helix keymap is
//! selected, enabling `j/k/h/l`.

use crate::actions::*;
use gpui_kit::{App, KeyBinding, Menu, MenuItem};

#[cfg(target_os = "macos")]
const MOD: &str = "cmd";
#[cfg(not(target_os = "macos"))]
const MOD: &str = "ctrl";

/// `mod` stands for the platform's primary modifier.
fn kb(keys: &str, action: impl gpui_kit::Action, context: Option<&str>) -> KeyBinding {
    // `KeyBinding::new` wants strings that outlive the binding; this runs once at
    // start-up, so leaking a few dozen short strings is the simplest correct thing.
    let keys: &'static str = Box::leak(keys.replace("mod", MOD).into_boxed_str());
    KeyBinding::new(keys, action, context)
}

pub fn bind(cx: &mut App) {
    let mut b: Vec<KeyBinding> = vec![
        kb("mod-shift-p", OpenCommandPalette, None),
        KeyBinding::new("f1", OpenCommandPalette, None),
        kb("mod-p", OpenTablePicker, None),
        kb("mod-alt-c", OpenConnectionPicker, None),
        kb("mod-n", NewConsole, None),
        kb("mod-shift-n", NewConnection, None),
        kb("mod-b", ToggleExplorer, None),
        kb("mod-j", ToggleBottomPanel, None),
        kb("mod-w", CloseTab, None),
        kb("mod-pagedown", NextTab, None),
        kb("mod-pageup", PreviousTab, None),
        kb("mod-shift-e", FocusExplorer, None),
        kb("mod-1", FocusEditor, None),
        kb("mod-2", FocusResults, None),
        KeyBinding::new("f5", RefreshSchema, None),
        kb("mod-k mod-t", SelectTheme, None),
        kb("mod-=", ZoomIn, None),
        kb("mod-+", ZoomIn, None),
        kb("mod--", ZoomOut, None),
        kb("mod-0", ZoomReset, None),
        kb("mod-q", Quit, None),
        kb("mod-enter", RunStatement, Some("!ResultGrid")),
        kb("mod-shift-enter", RunAll, None),
        kb("mod-.", CancelQuery, None),
        kb("mod-e", Explain, None),
        kb("mod-shift-alt-e", ExplainAnalyze, None),
        kb("mod-shift-f", FormatDocument, None),
        kb("mod-,", OpenSettings, None),
        kb("mod-o", OpenSqlFile, None),
    ];
    // Explorer navigation.
    b.extend([
        KeyBinding::new("down", ExplorerNext, Some("Explorer")),
        KeyBinding::new("up", ExplorerPrevious, Some("Explorer")),
        KeyBinding::new("right", ExplorerExpand, Some("Explorer")),
        KeyBinding::new("left", ExplorerCollapse, Some("Explorer")),
        KeyBinding::new("enter", ExplorerActivate, Some("Explorer")),
        KeyBinding::new("/", ExplorerFilter, Some("Explorer")),
        KeyBinding::new("escape", ExplorerBack, Some("Explorer")),
        KeyBinding::new("ctrl-n", ExplorerNext, Some("Explorer")),
        KeyBinding::new("ctrl-p", ExplorerPrevious, Some("Explorer")),
        KeyBinding::new("j", ExplorerNext, Some("Explorer && helix")),
        KeyBinding::new("k", ExplorerPrevious, Some("Explorer && helix")),
        KeyBinding::new("l", ExplorerExpand, Some("Explorer && helix")),
        KeyBinding::new("h", ExplorerCollapse, Some("Explorer && helix")),
    ]);
    cx.bind_keys(b);
}

/// macOS application menu (a no-op on platforms without a global menu bar).
pub fn set_menus(cx: &mut App) {
    cx.set_menus(vec![
        Menu {
            name: "Quarry".into(),
            items: vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::action("Settings File", OpenSettingsFile),
                MenuItem::separator(),
                MenuItem::action("Quit Quarry", Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Console", NewConsole),
                MenuItem::action("New Connection…", NewConnection),
                MenuItem::action("Open SQL File…", OpenSqlFile),
                MenuItem::action("Save Console As…", SaveConsole),
                MenuItem::separator(),
                MenuItem::action("Close Tab", CloseTab),
            ],
            disabled: false,
        },
        Menu {
            name: "Connection".into(),
            items: vec![
                MenuItem::action("Connect…", OpenConnectionPicker),
                MenuItem::action("Disconnect", Disconnect),
                MenuItem::action("Refresh Schema", RefreshSchema),
            ],
            disabled: false,
        },
        Menu {
            name: "Query".into(),
            items: vec![
                MenuItem::action("Run Statement", RunStatement),
                MenuItem::action("Run All", RunAll),
                MenuItem::action("Cancel", CancelQuery),
                MenuItem::separator(),
                MenuItem::action("Explain", Explain),
                MenuItem::action("Explain Analyze", ExplainAnalyze),
                MenuItem::action("Format Document", FormatDocument),
                MenuItem::separator(),
                MenuItem::action("Begin Transaction", BeginTransaction),
                MenuItem::action("Commit", CommitTransaction),
                MenuItem::action("Roll Back", RollbackTransaction),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Command Palette…", OpenCommandPalette),
                MenuItem::action("Go to Table…", OpenTablePicker),
                MenuItem::separator(),
                MenuItem::action("Toggle Explorer", ToggleExplorer),
                MenuItem::action("Toggle Bottom Panel", ToggleBottomPanel),
                MenuItem::separator(),
                MenuItem::action("Select Theme…", SelectTheme),
                MenuItem::action("Toggle Keymap", ToggleKeymap),
                MenuItem::action("Import Zed Theme…", ImportZedTheme),
                MenuItem::separator(),
                MenuItem::action("Zoom In", ZoomIn),
                MenuItem::action("Zoom Out", ZoomOut),
                MenuItem::action("Reset Zoom", ZoomReset),
            ],
            disabled: false,
        },
    ]);
}
