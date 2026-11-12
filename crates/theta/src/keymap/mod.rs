//! Default key bindings and the macOS application menu.
//!
//! `cmd` on macOS, `ctrl` elsewhere. Contexts: "Explorer" is set on the tree;
//! the extra "modal" identifier is added there while a modal keymap (Helix or Vim) is
//! selected, enabling `j/k/h/l`.
//!
//! # Who wins a key: the precedence rule
//!
//! GPUI resolves key *bindings* before the focused view's key-down handler. A plain global
//! binding on, say, `ctrl-w` would therefore pre-empt the editor's own `ctrl-w` (delete word
//! back in Helix Insert mode) and close the tab, losing work. So:
//!
//! > While the SQL editor is focused, the editing engine's keys for the active keymap, mode
//! > and prompt/popup state always win. Application bindings on the same keystroke apply only
//! > outside the editor, or in the editor states where the engine does not use the key.
//!
//! The editor publishes its state as key context (`ThetaEditor`, `keymap=helix|vim|standard`,
//! `mode=normal|insert|select|visual|visual-line|visual-block|replace|standard`, `prompt`,
//! `completion`, `pending`; see
//! `theta_editor_view::key_context`). [`ownership`] asks the engine, the prompt bar and the
//! completion popup which keystrokes they use in which state, and [`bind`] appends
//! `!(ThetaEditor && <states that own it>)` to every global binding on such a keystroke.
//! Nothing is listed by hand, so a key the engine gains later is protected automatically.
//! Other views with keys of their own (the grid, pickers, the explorer) are handled the same
//! way through [`scopes`]: `ctrl-0` is "set NULL" while the grid has focus, `ctrl-n` / `ctrl-p`
//! move the selection of an open picker.
//!
//! In the Standard keymap the engine emits the very same action as an `Effect` (`ctrl-w` is
//! `Effect::CloseTab`), which the workspace handles: one path, never two.

pub mod ownership;

use crate::actions::*;
use gpui_kit::{App, KeyBinding, Menu, MenuItem};

#[cfg(target_os = "macos")]
const MOD: &str = "cmd";
#[cfg(not(target_os = "macos"))]
const MOD: &str = "ctrl";

/// The first keystroke of `keys` with `mod` expanded, as the engine would see it.
fn first_stroke(keys: &str) -> String {
    keys.replace("mod", MOD).split_whitespace().next().unwrap_or_default().to_string()
}

/// A view that handles some keystrokes itself while it has focus, and the context its element
/// publishes. The `owns` predicate answers for a first stroke in GPUI's spelling.
pub struct Scope {
    pub context: &'static str,
    owns: Box<dyn Fn(&str) -> bool>,
}

impl Scope {
    pub fn owns(&self, stroke: &str) -> bool {
        (self.owns)(stroke)
    }
}

/// Every focusable view with keys of its own, other than the SQL editor (see [`ownership`]).
///
/// * the explorer tree: its bindings are in [`explorer_bindings`];
/// * pickers: `theta_ui::picker::PICKER_KEYSTROKES`;
/// * the results grid handles `ctrl`/`cmd` chords in a key-down handler, tabulated in
///   `theta_grid::owns_chord`;
/// * the map and the plan panel ignore every chord with `ctrl`, `alt` or `cmd` (and take no
///   F-keys), so they own none of the global bindings.
pub fn scopes() -> Vec<Scope> {
    let explorer: Vec<String> = explorer_bindings()
        .iter()
        .filter_map(|b| b.keystrokes().first().map(|k| k.unparse(theta_ui::host_os())))
        .collect();
    vec![
        Scope { context: "Explorer", owns: Box::new(move |s| explorer.iter().any(|e| e == s)) },
        Scope {
            context: theta_ui::picker::PICKER_CONTEXT,
            owns: Box::new(|s| theta_ui::picker::PICKER_KEYSTROKES.contains(&s)),
        },
        Scope {
            context: "ResultGrid",
            owns: Box::new(|s| {
                let Some(k) = ownership::engine_key(s) else { return false };
                if !(k.modifiers.ctrl || k.modifiers.platform) || k.modifiers.alt {
                    return false;
                }
                let name = match k.key {
                    theta_editor::Key::Char(c) => c.to_lowercase().to_string(),
                    theta_editor::Key::Enter => "enter".to_string(),
                    theta_editor::Key::Home => "home".to_string(),
                    theta_editor::Key::End => "end".to_string(),
                    _ => return false,
                };
                theta_grid::owns_chord(&name)
            }),
        },
    ]
}

/// The context predicate of a global binding: `base` (if any), "the SQL editor does not own
/// this keystroke right now" and "no other focused view owns it".
fn guarded_context(keys: &str, base: Option<&str>) -> Option<String> {
    let stroke = first_stroke(keys);
    let mut parts: Vec<String> = base.map(str::to_string).into_iter().collect();
    if let Some(owned) = ownership::engine_key(&stroke)
        .map(|k| ownership::owning_states(&k))
        .and_then(|states| ownership::owned_predicate(&states))
    {
        parts.push(format!("!({owned})"));
    }
    for scope in scopes() {
        if scope.owns(&stroke) {
            parts.push(format!("!{}", scope.context));
        }
    }
    (!parts.is_empty()).then(|| parts.join(" && "))
}

/// `mod` stands for the platform's primary modifier. Bindings without an own context are
/// guarded against the keys the editor and the other views own; see the module comment.
fn kb(keys: &str, action: impl gpui_kit::Action, context: Option<&str>) -> KeyBinding {
    // `KeyBinding::new` wants strings that outlive the binding; this runs once at
    // start-up, so leaking a few dozen short strings is the simplest correct thing.
    let expanded: &'static str = Box::leak(keys.replace("mod", MOD).into_boxed_str());
    let guarded = match context {
        None => guarded_context(keys, None),
        Some(c) => Some(c.to_string()),
    };
    KeyBinding::new(expanded, action, guarded.as_deref())
}

/// The window-wide application bindings, guarded against the editor's own keys.
pub fn global_bindings() -> Vec<KeyBinding> {
    vec![
        kb("mod-shift-p", OpenCommandPalette, None),
        kb("f1", OpenCommandPalette, None),
        kb("mod-p", OpenTablePicker, None),
        kb("mod-alt-c", OpenConnectionPicker, None),
        kb("mod-n", NewConsole, None),
        kb("mod-shift-n", NewConnection, None),
        kb("mod-b", ToggleExplorer, None),
        kb("mod-j", ToggleBottomPanel, None),
        kb("mod-w", CloseTab, None),
        kb("mod-shift-t", ReopenClosedTab, None),
        kb("mod-pagedown", NextTab, None),
        kb("mod-pageup", PreviousTab, None),
        kb("mod-shift-e", FocusExplorer, None),
        kb("mod-1", FocusEditor, None),
        kb("mod-2", FocusResults, None),
        kb("mod-shift-v", ToggleCellViewer, None),
        kb("mod-shift-y", CopyCellValue, None),
        kb("mod-alt-f", SearchTables, None),
        kb("mod-alt-g", ShowDiagram, None),
        kb("mod-alt-s", CompareSchemas, None),
        kb("mod-shift-g", ShowChart, None),
        kb("mod-alt-h", ShowLocalHistory, None),
        kb("mod-alt-enter", RunOnSeveral, None),
        // F5 is "run all" in the editor; the explorer (and a chord everywhere) refreshes.
        kb("f5", RefreshSchema, Some("Explorer")),
        kb("mod-alt-r", RefreshSchema, None),
        kb("mod-k mod-t", SelectTheme, None),
        kb("mod-=", ZoomIn, None),
        kb("mod-+", ZoomIn, None),
        kb("mod--", ZoomOut, None),
        kb("mod-0", ZoomReset, None),
        kb("mod-q", Quit, None),
        kb("mod-enter", RunStatement, None),
        kb("mod-shift-enter", RunAll, None),
        kb("mod-.", CancelQuery, None),
        kb("mod-e", Explain, None),
        kb("mod-shift-alt-e", ExplainAnalyze, None),
        kb("mod-shift-f", FormatDocument, None),
        kb("mod-,", OpenSettings, None),
        kb("mod-o", OpenSqlFile, None),
    ]
}

/// Keys of the explorer tree (context `Explorer`); the editor is never inside it.
pub fn explorer_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("down", ExplorerNext, Some("Explorer")),
        KeyBinding::new("up", ExplorerPrevious, Some("Explorer")),
        KeyBinding::new("right", ExplorerExpand, Some("Explorer")),
        KeyBinding::new("left", ExplorerCollapse, Some("Explorer")),
        KeyBinding::new("enter", ExplorerActivate, Some("Explorer")),
        KeyBinding::new("/", ExplorerFilter, Some("Explorer")),
        KeyBinding::new("escape", ExplorerBack, Some("Explorer")),
        KeyBinding::new("alt-up", ExplorerMoveUp, Some("Explorer")),
        KeyBinding::new("alt-down", ExplorerMoveDown, Some("Explorer")),
        KeyBinding::new("f2", ExplorerRename, Some("Explorer")),
        KeyBinding::new("delete", ExplorerDelete, Some("Explorer")),
        KeyBinding::new("ctrl-n", ExplorerNext, Some("Explorer")),
        KeyBinding::new("ctrl-p", ExplorerPrevious, Some("Explorer")),
        KeyBinding::new("j", ExplorerNext, Some("Explorer && modal")),
        KeyBinding::new("k", ExplorerPrevious, Some("Explorer && modal")),
        KeyBinding::new("l", ExplorerExpand, Some("Explorer && modal")),
        KeyBinding::new("h", ExplorerCollapse, Some("Explorer && modal")),
    ]
}

/// Every application binding, in precedence order (later wins on a tie).
pub fn bindings() -> Vec<KeyBinding> {
    let mut b = global_bindings();
    b.extend(explorer_bindings());
    b
}

pub fn bind(cx: &mut App) {
    cx.bind_keys(bindings());
    crate::table_editor::bind_keys(cx);
    crate::er::bind_keys(cx);
}

/// macOS application menu (a no-op on platforms without a global menu bar).
pub fn set_menus(cx: &mut App) {
    cx.set_menus(vec![
        Menu {
            name: "Theta".into(),
            items: vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::action("Settings File", OpenSettingsFile),
                MenuItem::separator(),
                MenuItem::action("Quit Theta", Quit),
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
                MenuItem::action("Reopen Closed Tab", ReopenClosedTab),
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
                MenuItem::action("Cancel Query", CancelQuery),
                MenuItem::separator(),
                MenuItem::action("Explain", Explain),
                MenuItem::action("Explain Analyze", ExplainAnalyze),
                MenuItem::action("Format Document", FormatDocument),
                MenuItem::separator(),
                MenuItem::action("Begin Transaction", BeginTransaction),
                MenuItem::action("Commit Transaction", CommitTransaction),
                MenuItem::action("Roll Back Transaction", RollbackTransaction),
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
                MenuItem::action("Select Keymap…", SelectKeymap),
                MenuItem::action("Import Zed Theme…", ImportZedTheme),
                MenuItem::separator(),
                MenuItem::action("Increase Editor Font Size", ZoomIn),
                MenuItem::action("Decrease Editor Font Size", ZoomOut),
                MenuItem::action("Reset Editor Font Size", ZoomReset),
            ],
            disabled: false,
        },
    ]);
}
