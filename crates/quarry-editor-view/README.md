# quarry-editor-view

The GPUI widget that shows a [`quarry-editor`](../quarry-editor/README.md) `Editor`: gutter with
statement status, syntax colours, several selections, block and bar carets, diagnostics,
completion popup, which-key box, prompt bar, scrollbars, IME and the right-click menu. The
engine decides what a key *does*; this crate decides what the user *sees* and routes input in
and effects out. It has no knowledge of connections, tabs or results: the host (the `quarry`
crate) gets `EditorEvent`s and gives the view providers.

## Layout of the code

| Module | Role | Pure (no GPUI)? |
|---|---|---|
| `view.rs` | `EditorView`, the controller: owns the `Editor`, handles keys, applies effects, runs completion and diagnostics, keeps the caches | no |
| `element.rs` | the custom GPUI `Element`: prepaint builds a frame of draw lists, paint replays them | no |
| `render.rs` | the `Render` impl: key context, actions, context menu, which-key, prompt and message bars | no |
| `input.rs` | `EntityInputHandler`: IME composition, dictation, text committed outside key events | no |
| `keys.rs` | GPUI keystroke to `quarry_editor::KeyInput` (layout and AltGr rules are in its header) | yes |
| `actions.rs` | key context, the completion-popup key table, the binding overrides, menu actions | partly |
| `completion.rs`, `geometry.rs`, `display.rs`, `selection_geom.rs`, `util.rs`, `status.rs` | popup state, scroll arithmetic, tab/wide-char display mapping, selection outlines, range shifting | yes |
| `lines.rs` | the shaped-line cache | no |
| `tests.rs` | headless GPUI tests (typing, popup, scrolling, key context) | |

## Controller versus element

Painting is split in two phases so that event handlers never depend on paint state they cannot
see:

* **Controller (`view.rs`)** changes state: text, selections, `pending_reveal` /
  `pending_scroll`, the completion popup, diagnostics. It never measures text.
* **`prepaint`** (`EditorView::compute_frame`) is the only place where scroll and layout are
  resolved. It applies the pending scroll request or "reveal the caret", shapes the visible lines
  (plus two lines of overscan) through the line cache, and turns the state into plain draw lists
  (`Frame`): selections, carets, squiggles, gutter marks, statement band, scrollbars, popups.
  It also records `LayoutInfo` (bounds, gutter width, cell width, viewport) for hit-testing and IME.
* **`paint`** replays the draw lists and registers the mouse and text-input handlers. Popups are
  deferred draws so they can leave the editor's bounds.

Cost per frame is independent of the buffer size: only visible lines are touched. The reveal is
requested (`pending_reveal`) after *every* edit path: key presses (the engine emits
`Effect::RevealCursor`), `insert_text` from the input method, menu cut/paste, completion
acceptance and `update_editor`. `prepaint` clamps the horizontal scroll to the widest line it
knows; it therefore folds the caret line's own width in first, otherwise typing at the end of a
line longer than the last known maximum would clamp the reveal away.

## Caches

* **Shaped lines** (`lines.rs`): keyed by a hash of the line text, its highlight spans and the
  style epoch (font, size, syntax colours). Scrolling, selecting and caret movement never
  re-shape; an edit re-shapes only the lines whose text or colouring changed. Past 4096 entries those not used in the
  last two frames are evicted.
* **Text snapshot**: one `Arc<str>` per engine revision, shared by completion, diagnostics and
  statement lookup, so they do not each copy the buffer.
* **Statement under the caret** (the gutter band): cached per (revision, caret). On buffers over
  150,000 characters recomputation is throttled to every 120 ms with one refresh when input
  pauses, so the band lags instead of every key stroke paying for a lex of the whole script.
* **Bracket match** and **line numbers** are cached by (revision, caret) and by the style epoch.

`QUARRY_EDITOR_PERF=1` prints frame costs (`FrameStats`).

## Key dispatch, and who wins

GPUI resolves key *bindings* (actions) before the focused element's `on_key_down`. A global
binding on `ctrl-w` would therefore close the tab while the engine wants `ctrl-w` as "delete
word back". The rule, enforced by the host and tested end to end:

> While the SQL editor is focused, the engine's keys for the active keymap, mode and
> prompt/popup state always win. Application bindings on the same keystroke only apply
> outside the editor (or in states where the engine does not use the key).

How the pieces fit:

1. The editor element publishes a **key context** (`actions::key_context`):
   `QuarryEditor`, `keymap=helix|standard`, `mode=normal|insert|select|standard`, plus the
   identifiers `prompt` while the `:` / `/` bar is open and `completion` while the popup is
   open. The host predicates its own bindings with it (`!(QuarryEditor && keymap == helix &&
   mode == insert)`); see `crates/quarry/src/keymap/` and `docs/KEYBINDINGS.md`.
2. `init` (also called when the view first renders) registers `NoAction` bindings in the editor context for `tab`,
   `shift-tab`, `ctrl-c` and `cmd-c`, which the gpui-kit root binds one level up for focus
   movement and copying UI text.
3. Remaining keys reach `EditorView::on_key_down`, which translates them (`keys.rs`) and calls
   `handle_input`: **the completion popup first** (when open), then the engine
   (`Editor::handle_key`), then the effects.
4. The popup's keys are tabulated once in `actions::classify_completion_key`: up/down,
   page up/down and `ctrl-p`/`ctrl-n` move; `tab` accepts; `enter` accepts (or falls through
   as a newline, see below); `escape` closes. The same table is exported as
   `completion_owns(&KeyInput)`, so the host keeps its bindings off exactly these keys while
   the popup is open, in both keymaps.

Effects the view does not consume itself (run, explain, format, tab switching, open file ...)
leave as `EditorEvent::Effect`; the Standard keymap emits the same actions as the application
bindings (`ctrl-w` is `Effect::CloseTab`), so there is exactly one path per keystroke.

## Completion

* A **provider** (`CompletionProvider::complete(text, byte_offset)`) answers on the UI thread; it
  must take milliseconds.
* **Auto-trigger**: after typing a word character or `.` in an insert-like mode (Helix Insert,
  Standard), when the word under the caret is non-empty and not purely numeric, or right after a
  `.`; and after typing a space that follows `FROM`, `JOIN`, `ON`, `INTO`, `UPDATE` or `TABLE`, or a comma
  inside a FROM list.
  Not in Helix Normal/Select, and not while a prompt is open. `ctrl-space` opens it by hand and
  may offer suggestions for an empty word.
* **While open** it is re-queried after each edit: typing on in the same word keeps it, any other
  word (space, paren, new statement) closes it; a `.` starts a fresh list. Moving the caret
  out of the word closes it.
* **Tab** always accepts the selected item. **Enter** accepts, except: when the item equals what
  is already typed (accepting would only swallow the newline), or when the popup was opened by
  typing, nothing was picked with the arrows, and the top suggestion is a keyword (an alias
  `FROM customers c` would otherwise turn into `CROSS JOIN`). Then the popup closes and Enter
  inserts the newline. Once the user navigated, Enter accepts.
* Acceptance is one `replace_range` (one undo step; no auto-pairing, so `count(` stays `count(`).
  Accepting `schema.` or `alias.` continues straight into the member list.
* A popup that would offer only the word already typed is not shown.

## Diagnostics

A `DiagnosticsProvider` (`Fn(&str) -> Vec<Diagnostic>`) is called on a background thread 300 ms
after the last edit (and once for the initial text). A result is applied only if the revision it
was computed for is still current.

* **Shifting**: on every edit the existing squiggles are carried through the change
  (`util::diff_edit` + `shift_range`) so they never sit on the wrong text while the next lint
  runs; ranges the edit touched are dropped.
* **Holding**: complaints that only mean "this statement is not finished" are hidden while the
  user types that statement in an insert-like mode and for 1.5 s after the last key stroke
  (`held_back`); everything else is shown at once. Large buffers skip the bookkeeping.
* **Transient diagnostics** (`set_transient_diagnostic`) show the server's error position after
  a failed run until the next edit; `set_statement_status` paints running / ok / failed in the gutter
  and is cleared by the next edit.

## Embedding

```rust
quarry_editor_view::init(cx);                       // once; optional, rendering installs it too
let editor = cx.new(|cx| EditorView::new("select 1;", window, cx));
editor.update(cx, |e, _| {
    e.set_completion_provider(Some(Rc::new(MyProvider)));
    e.set_diagnostics_provider(Some(Arc::new(|sql| lint(sql))));
    e.set_database_kind(DatabaseKind::Postgres);
});
cx.subscribe(&editor, |_, _, ev: &EditorEvent, cx| match ev {
    EditorEvent::Effect(effect) => { /* run, explain, format, switch tab ... */ }
    EditorEvent::Edited { .. } => { /* debounce autosave */ }
    _ => {}
}).detach();
div().child(editor.clone())                         // it fills its parent
```

The view reads `quarry_ui::Settings` (keymap, buffer font, size, line height) when a settings
store exists and follows its changes; without one it uses defaults. Bind application shortcuts
with the key-context predicates above, never with a bare global binding on a key the engine
uses. `examples/editor_demo.rs` is a runnable embedding (`cargo run -p quarry-editor-view
--example editor_demo`).

## Tests

`cargo test -p quarry-editor-view`. Pure modules have unit tests next to the code; `tests.rs`
drives a real `EditorView` in a headless GPUI window (typing, popup rules, horizontal reveal
after edits, key-context contents).
