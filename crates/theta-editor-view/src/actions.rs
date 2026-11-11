//! Actions behind the right-click menu, and the one-time key-binding setup.

use gpui_kit::{App, Global, KeyBinding, KeyContext, NoAction};
use theta_editor::{Key, KeyInput, Keymap, Mode};

/// The key context of the editor element. Apps that bind their own keys can scope them with
/// `!ThetaEditor` to keep them out of the editor, or bind inside this context to win over it.
pub const KEY_CONTEXT: &str = "ThetaEditor";

/// The key context of the editor element for the editor's current state.
///
/// Besides the `ThetaEditor` identifier it carries `keymap=helix|vim|standard`,
/// `mode=normal|insert|select|visual|visual-line|visual-block|replace|standard`, `prompt` while the `:` / `/` prompt bar is open and
/// `completion` while the completion popup is open and `pending` (Vim only) while a
/// multi-key command such as an operator (`d`), `g` or a register prefix waits for its rest. Application bindings use these in
/// predicates (`!(ThetaEditor && keymap == helix && mode == insert)`) to stay out of the way
/// of keys the editor owns; see `docs/KEYBINDINGS.md`.
pub fn key_context(keymap: Keymap, mode: Mode, prompt: bool, completion: bool, pending: bool) -> KeyContext {
    let mut ctx = KeyContext::default();
    ctx.add(KEY_CONTEXT);
    ctx.set("keymap", keymap_name(keymap));
    ctx.set("mode", mode_name(mode));
    if prompt {
        ctx.add("prompt");
    }
    if completion {
        ctx.add("completion");
    }
    if pending {
        ctx.add("pending");
    }
    ctx
}

/// The `keymap` value of [`key_context`].
pub fn keymap_name(keymap: Keymap) -> &'static str {
    match keymap {
        Keymap::Helix => "helix",
        Keymap::Standard => "standard",
        Keymap::Vim => "vim",
    }
}

/// The `mode` value of [`key_context`].
pub fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => "normal",
        Mode::Insert => "insert",
        Mode::Select => "select",
        Mode::Standard => "standard",
        Mode::Visual => "visual",
        Mode::VisualLine => "visual-line",
        Mode::VisualBlock => "visual-block",
        Mode::Replace => "replace",
    }
}

/// What an open completion popup does with a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompletionKey {
    Move(isize),
    Accept,
    /// Accepts, or (when the suggestion is a no-op or a keyword) falls through as a newline.
    Enter,
    Close,
}

/// The single table of popup keys, shared by the view (to act) and by [`completion_owns`]
/// (so the application can keep its own bindings off these keys).
pub(crate) fn classify_completion_key(input: &KeyInput) -> Option<CompletionKey> {
    let m = &input.modifiers;
    let plain = !m.ctrl && !m.alt && !m.shift && !m.platform;
    Some(match &input.key {
        Key::Up if plain => CompletionKey::Move(-1),
        Key::Down if plain => CompletionKey::Move(1),
        Key::PageUp if plain => CompletionKey::Move(-8),
        Key::PageDown if plain => CompletionKey::Move(8),
        Key::Char('p') if m.ctrl && !m.alt && !m.platform => CompletionKey::Move(-1),
        Key::Char('n') if m.ctrl && !m.alt && !m.platform => CompletionKey::Move(1),
        Key::Tab if plain => CompletionKey::Accept,
        Key::Enter if plain => CompletionKey::Enter,
        Key::Escape if plain => CompletionKey::Close,
        _ => return None,
    })
}

/// Whether `input` belongs to the completion popup while it is open.
pub fn completion_owns(input: &KeyInput) -> bool {
    classify_completion_key(input).is_some()
}

gpui_kit::actions!(
    theta_editor_view,
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
