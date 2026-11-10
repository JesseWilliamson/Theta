//! Actions behind the right-click menu, and the one-time key-binding setup.

use gpui_kit::{App, Global, KeyBinding, NoAction};

/// The key context of the editor element. Apps that bind their own keys can scope them with
/// `!QuarryEditor` to keep them out of the editor, or bind inside this context to win over it.
pub const KEY_CONTEXT: &str = "QuarryEditor";

gpui_kit::actions!(
    quarry_editor_view,
    [EditorCut, EditorCopy, EditorPaste, EditorRunStatement, EditorRunSelection, EditorExplain, EditorFormat,]
);

struct BindingsInstalled;
impl Global for BindingsInstalled {}

/// gpui-kit's `Root` binds Tab, Shift-Tab and Ctrl-C (focus navigation, copy of selected UI
/// text) on an ancestor context, and GPUI dispatches key *bindings* before key-down listeners,
/// so without this the engine would never see those keys. A `NoAction` binding in the editor's
/// own (deeper) context suppresses the shallower ones and lets the key reach the editor.
pub(crate) fn ensure_bindings(cx: &mut App) {
    if cx.has_global::<BindingsInstalled>() {
        return;
    }
    cx.set_global(BindingsInstalled);
    cx.bind_keys([
        KeyBinding::new("tab", NoAction, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-tab", NoAction, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-c", NoAction, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-c", NoAction, Some(KEY_CONTEXT)),
    ]);
}

/// Install the editor's key-binding overrides. Called automatically by `EditorView::new`;
/// exposed so apps can install them before any editor exists.
pub fn init(cx: &mut App) {
    ensure_bindings(cx);
}
