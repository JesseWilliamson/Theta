//! `EditorView`: the GPUI entity that owns a `quarry_editor::Editor`, turns input into engine
//! calls and engine effects into view state. Painting lives in `element.rs`, popups and
//! chrome in `render.rs`; this file is the controller.

use crate::actions::{CompletionKey, classify_completion_key};
use crate::completion::{CompletionState, auto_trigger_after_space, is_noop_accept, is_word_char};
use crate::display::byte_range_to_chars;
use crate::geometry;
use crate::keys::{KeyMods, translate_key};
use crate::lines::{LineCache, LineEntry, Style};
use crate::status::{EditorStatus, StatementStatus};
use gpui_kit::{
    App, Bounds, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, Font, KeyDownEvent, Pixels,
    Point, SharedString, Subscription, Task, UTF16Selection, Window, WindowTextSystem, font, px,
};
use quarry_core::DatabaseKind;
use quarry_editor::{Editor, Effect, Key, KeyInput, Keymap, Mode, ScrollRequest, Selection};
use quarry_sql::{CompletionList, Diagnostic, Severity};
use quarry_ui::settings::{KeymapMode, Settings, SettingsStore};
use quarry_ui::{EditorColors, SyntaxColors};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Cursor blink half-period.
pub const BLINK: Duration = Duration::from_millis(530);
/// How long a status message stays in the bottom line.
pub const MESSAGE_TTL: Duration = Duration::from_secs(4);
/// Pending keys must be held this long before the which-key box appears (`space` is immediate).
pub const HINT_DELAY: Duration = Duration::from_millis(250);
/// Hovering a diagnostic this long opens its popover.
pub const HOVER_DELAY: Duration = Duration::from_millis(400);
const DIAGNOSTICS_DEBOUNCE: Duration = Duration::from_millis(300);
/// How long after the last keystroke "unfinished statement" diagnostics stay hidden.
const TYPING_IDLE: Duration = Duration::from_millis(1500);
/// Buffers longer than this (chars) get throttled whole-script lookups.
const BIG_DOCUMENT_CHARS: usize = 150_000;
const STATEMENT_THROTTLE: Duration = Duration::from_millis(120);
/// Scrollbars stay visible this long after the last scroll.
pub const SCROLLBAR_LINGER: Duration = Duration::from_millis(1200);

#[derive(Clone, Debug)]
pub enum EditorEvent {
    /// Every `Effect` the engine emitted that the view does not handle itself (the view handles
    /// `SetClipboard`, `RevealCursor`, `Scroll`, `RequestCompletion`/`DismissCompletion` when a
    /// provider is set, and `Status`, which goes to its own bottom line).
    Effect(Effect),
    /// Text changed (debounce is the host's business); `revision` is the engine's.
    Edited {
        revision: u64,
    },
    /// Selections, mode or pending keys changed: the status bar should refresh.
    StateChanged,
    Focused,
    Blurred,
}

/// Supplies completions on the UI thread; must be fast (quarry-sql takes 1-2 ms).
pub trait CompletionProvider: 'static {
    fn complete(&self, text: &str, byte_offset: usize) -> CompletionList;
}

/// Supplies diagnostics for squiggles, called ~300 ms after the last edit on a background thread.
pub type DiagnosticsProvider = Arc<dyn Fn(&str) -> Vec<Diagnostic> + Send + Sync>;

/// (revision, primary cursor char): what cursor-dependent caches are keyed by.
pub(crate) type CacheKey = (u64, usize);

#[derive(Clone, Debug)]
pub(crate) struct Diag {
    /// Chars.
    pub range: Range<usize>,
    pub severity: Severity,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Drag {
    Text,
    VThumb { grab: f32 },
    HThumb { grab: f32 },
}

#[derive(Clone, Debug)]
pub(crate) struct Hover {
    pub diag: usize,
    pub shown: bool,
}

/// Geometry from the last paint, used by event handlers (hit-testing) and IME.
#[derive(Clone, Debug)]
pub(crate) struct LayoutInfo {
    pub bounds: Bounds<Pixels>,
    pub gutter_w: f32,
    pub line_h: f32,
    pub cell_w: f32,
    /// Text area size (gutter excluded).
    pub viewport: (f32, f32),
    pub max_scroll: (f32, f32),
    pub v_track: Option<Bounds<Pixels>>,
    pub v_thumb: Option<Bounds<Pixels>>,
    pub h_track: Option<Bounds<Pixels>>,
    pub h_thumb: Option<Bounds<Pixels>>,
}

/// Perf counters for `QUARRY_EDITOR_PERF=1`.
#[derive(Default, Clone, Debug)]
pub struct FrameStats {
    pub frames: u64,
    pub prepaint_us: f64,
    pub paint_us: f64,
    pub shaped: usize,
    pub cache_hits: usize,
}

pub struct EditorView {
    pub(crate) editor: Editor,
    pub(crate) focus_handle: FocusHandle,
    // scrolling
    pub(crate) scroll_x: f32,
    pub(crate) scroll_y: f32,
    pub(crate) pending_reveal: bool,
    pub(crate) pending_scroll: Option<ScrollRequest>,
    pub(crate) scroll_activity: Option<Instant>,
    pub(crate) longest: (u64, f32),
    pub(crate) last_viewport: (usize, usize),
    // layout and shaping
    pub(crate) layout: Option<LayoutInfo>,
    pub(crate) cache: LineCache,
    pub(crate) style: Option<Style>,
    pub(crate) text_system: Rc<WindowTextSystem>,
    pub(crate) font_family: SharedString,
    pub(crate) font_size: f32,
    pub(crate) line_height_mult: f32,
    pub(crate) stats: FrameStats,
    pub(crate) numbers: std::collections::HashMap<(usize, bool), gpui_kit::ShapedLine>,
    pub(crate) numbers_epoch: u64,
    pub(crate) bracket_cache: Option<(CacheKey, Option<(usize, usize)>)>,
    pub(crate) ime_anchor: Option<Bounds<Pixels>>,
    pub(crate) near_edge: bool,
    // text snapshot shared by completion/diagnostics/statement lookup
    text_snapshot: Option<(u64, Arc<str>)>,
    // cursor blink
    pub(crate) blink_visible: bool,
    blink_epoch: u64,
    blink_task: Option<Task<()>>,
    pub(crate) focused: bool,
    // completion
    completion_provider: Option<Rc<dyn CompletionProvider>>,
    pub(crate) completion: Option<CompletionState>,
    // diagnostics
    diagnostics_provider: Option<DiagnosticsProvider>,
    pub(crate) diagnostics: Vec<Diag>,
    pub(crate) transient: Option<(u64, Diag)>,
    diag_task: Option<Task<()>>,
    /// The char range of the statement being typed (set by an edit in an insert-like mode, cleared when typing pauses): its "unfinished" diagnostics stay hidden until typing pauses.
    typing: Option<Range<usize>>,
    typing_task: Option<Task<()>>,
    pub(crate) pending_initial_diagnostics: bool,
    pub(crate) hover: Option<Hover>,
    // statements
    pub(crate) statement_status: Option<(Range<usize>, StatementStatus)>,
    stmt_cache: Option<(CacheKey, Option<(usize, usize)>)>,
    stmt_at: Instant,
    stmt_refresh_pending: bool,
    pub(crate) db_kind: DatabaseKind,
    pub(crate) placeholder: SharedString,
    pub(crate) read_only: bool,
    // mouse
    pub(crate) drag: Option<Drag>,
    pub(crate) pointer: Option<Point<Pixels>>,
    pub(crate) drag_moved: bool,
    // ime
    pub(crate) marked: Option<Range<usize>>,
    // messages / hints
    /// Latest engine status text and a serial number so only its own timer clears it.
    pub(crate) message: Option<(String, u64)>,
    message_serial: u64,
    pub(crate) hints_since: Option<Instant>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<EditorEvent> for EditorView {}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn keymap_of(mode: KeymapMode) -> Keymap {
    match mode {
        KeymapMode::Helix => Keymap::Helix,
        KeymapMode::Vim => Keymap::Vim,
        KeymapMode::Standard => Keymap::Standard,
    }
}

fn read_settings(cx: &App) -> Settings {
    if cx.has_global::<SettingsStore>() { Settings::get(cx).clone() } else { Settings::default() }
}

impl EditorView {
    pub fn new(text: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = read_settings(cx);
        let mut editor = Editor::new(text, keymap_of(settings.keymap));
        editor.set_vim_clipboard_unnamed(settings.vim.clipboard_unnamed);
        let focus_handle = cx.focus_handle();
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.on_focus(&focus_handle, window, |this, _window, cx| this.on_focus(cx)));
        subscriptions.push(cx.on_blur(&focus_handle, window, |this, _window, cx| this.on_blur(cx)));
        if cx.has_global::<SettingsStore>() {
            subscriptions.push(cx.observe_global::<SettingsStore>(|this, cx| this.on_settings_changed(cx)));
        }
        EditorView {
            editor,
            focus_handle,
            scroll_x: 0.0,
            scroll_y: 0.0,
            pending_reveal: false,
            pending_scroll: None,
            scroll_activity: None,
            longest: (0, 0.0),
            last_viewport: (usize::MAX, 0),
            layout: None,
            cache: LineCache::default(),
            style: None,
            text_system: window.text_system().clone(),
            font_family: settings.buffer_font_family.clone().into(),
            font_size: settings.buffer_font_size,
            line_height_mult: settings.buffer_line_height,
            stats: FrameStats::default(),
            numbers: Default::default(),
            numbers_epoch: 0,
            bracket_cache: None,
            ime_anchor: None,
            near_edge: false,
            text_snapshot: None,
            blink_visible: true,
            blink_epoch: 0,
            blink_task: None,
            focused: false,
            completion_provider: None,
            completion: None,
            diagnostics_provider: None,
            diagnostics: Vec::new(),
            transient: None,
            diag_task: None,
            typing: None,
            typing_task: None,
            pending_initial_diagnostics: false,
            hover: None,
            statement_status: None,
            stmt_cache: None,
            stmt_at: Instant::now(),
            stmt_refresh_pending: false,
            db_kind: DatabaseKind::Postgres,
            placeholder: "Write SQL \u{2014} Ctrl+Enter runs the statement under the cursor".into(),
            read_only: false,
            drag: None,
            pointer: None,
            drag_moved: false,
            marked: None,
            message: None,
            message_serial: 0,
            hints_since: None,
            _subscriptions: subscriptions,
        }
    }

    // ---- public API ---------------------------------------------------------------------------

    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    /// Run `f` on the engine (format result, insert suggestion ...). The view re-syncs afterwards:
    /// text changes raise `Edited`, selection changes `StateChanged`, and the cursor is revealed.
    pub fn update_editor<R>(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Editor) -> R) -> R {
        let before = self.state_key();
        let rev0 = self.editor.revision();
        let result = f(&mut self.editor);
        self.after_external_change(rev0, before, cx);
        result
    }

    pub fn text(&self) -> String {
        self.editor.text()
    }

    /// Whether the completion popup is open.
    pub fn completion_open(&self) -> bool {
        self.completion.is_some()
    }

    /// Close the completion popup if it is open.
    pub fn close_completion(&mut self, cx: &mut Context<Self>) {
        if self.completion.take().is_some() {
            cx.notify();
        }
    }

    /// The index of the highlighted suggestion while the popup is open.
    pub fn completion_selected(&self) -> Option<usize> {
        self.completion.as_ref().map(|c| c.selected)
    }

    /// Open the completion popup by hand (what `ctrl-space` does). A no-op without a provider.
    pub fn request_completion(&mut self, cx: &mut Context<Self>) {
        if self.completion_provider.is_some() {
            self.completion = self.query_completion(true);
            cx.notify();
        }
    }

    pub fn set_completion_provider(&mut self, provider: Option<Rc<dyn CompletionProvider>>) {
        self.completion_provider = provider;
        if self.completion_provider.is_none() {
            self.completion = None;
        }
    }

    pub(crate) fn diagnostics_provider_clone(&self) -> Option<DiagnosticsProvider> {
        self.diagnostics_provider.clone()
    }

    pub fn set_diagnostics_provider(&mut self, provider: Option<DiagnosticsProvider>) {
        let has = provider.is_some();
        self.diagnostics_provider = provider;
        if has {
            // Show squiggles for the initial text without waiting for the first edit.
            self.schedule_diagnostics_in(Duration::ZERO, None);
        } else {
            self.diagnostics.clear();
            self.diag_task = None;
        }
    }

    /// The SQL dialect used to find the statement under the cursor (default PostgreSQL).
    pub fn set_database_kind(&mut self, kind: DatabaseKind) {
        self.db_kind = kind;
        self.stmt_cache = None;
    }

    /// An extra diagnostic from outside (e.g. the server's error position after a failed run),
    /// shown until the next edit.
    pub fn set_transient_diagnostic(&mut self, range: Range<usize>, message: String, cx: &mut Context<Self>) {
        let chars = self.byte_range_to_chars(range);
        self.transient =
            Some((self.editor.revision(), Diag { range: chars, severity: Severity::Critical, message }));
        cx.notify();
    }

    /// Remove the error squiggle of a failed run (the statement has since succeeded).
    pub fn clear_transient_diagnostic(&mut self, cx: &mut Context<Self>) {
        if self.transient.take().is_some() {
            cx.notify();
        }
    }

    /// Mark the statement being / last executed in the gutter. Cleared by the next edit.
    pub fn set_statement_status(
        &mut self,
        status: Option<(Range<usize>, StatementStatus)>,
        cx: &mut Context<Self>,
    ) {
        self.statement_status = status.map(|(r, s)| (self.byte_range_to_chars(r), s));
        cx.notify();
    }

    pub fn set_placeholder(&mut self, text: impl Into<SharedString>) {
        self.placeholder = text.into();
    }

    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    pub fn status(&self) -> EditorStatus {
        let sel = self.editor.primary();
        let cursor = self.editor.cursor_of(&sel);
        let line = self.editor.char_to_line(cursor);
        let selected_chars = self
            .editor
            .selections()
            .iter()
            .filter(|s| crate::util::selection_is_real(&self.editor, s))
            .map(|s| s.len())
            .sum();
        EditorStatus {
            keymap: self.editor.keymap(),
            mode: self.editor.mode(),
            pending_keys: self.editor.pending_keys(),
            line: line + 1,
            column: cursor - self.editor.line_to_char(line) + 1,
            selection_count: self.editor.selections().len(),
            selected_chars,
            message: self.message.as_ref().map(|(m, _)| m.clone()),
            modified: self.editor.is_modified(),
            recording: self.editor.recording_register(),
        }
    }

    pub fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// Frame cost counters (prepaint+paint), maintained always, printed with `QUARRY_EDITOR_PERF=1`.
    pub fn frame_stats(&self) -> &FrameStats {
        &self.stats
    }

    /// Scroll so that `line` (0-based) is the first visible line.
    pub fn scroll_to_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let lh = self.line_height_px();
        self.scroll_y = line as f32 * lh;
        self.scroll_activity = Some(Instant::now());
        cx.notify();
    }

    // ---- shared helpers -------------------------------------------------------------------------

    pub(crate) fn line_height_px(&self) -> f32 {
        (self.font_size * self.line_height_mult).round().max(1.0)
    }

    pub(crate) fn state_key(&self) -> (Selection, usize, Mode, String) {
        (
            self.editor.primary(),
            self.editor.selections().len(),
            self.editor.mode(),
            self.editor.pending_keys(),
        )
    }

    pub(crate) fn text_snapshot(&mut self) -> Arc<str> {
        let rev = self.editor.revision();
        if let Some((r, t)) = &self.text_snapshot
            && *r == rev
        {
            return t.clone();
        }
        let t: Arc<str> = Arc::from(self.editor.text());
        self.text_snapshot = Some((rev, t.clone()));
        t
    }

    pub(crate) fn byte_range_to_chars(&self, r: Range<usize>) -> Range<usize> {
        let len = self.editor.char_to_byte(self.editor.len_chars());
        byte_range_to_chars(len, &|b| self.editor.byte_to_char(b), r)
    }

    pub(crate) fn primary_cursor(&self) -> usize {
        self.editor.cursor_of(&self.editor.primary())
    }

    pub(crate) fn refresh_style(&mut self, cx: &App) {
        let colors = SyntaxColors::get(cx);
        let ec = EditorColors::get(cx);
        let font: Font = font(self.font_family.clone());
        let style = Style::new(font, px(self.font_size), colors, ec.foreground);
        if self.style.as_ref().map(|s| s.epoch) != Some(style.epoch) {
            self.style = Some(style);
        }
    }

    /// Shaped line `line`, from the cache.
    pub(crate) fn line_entry(&mut self, line: usize, cx: &App) -> Rc<LineEntry> {
        if self.style.is_none() {
            self.refresh_style(cx);
        }
        let src = self.editor.line(line);
        let spans = self.editor.highlights(line);
        let style = self.style.as_ref().expect("style set above");
        self.cache.get(&self.text_system, &src, &spans, style)
    }

    /// Pixel width of one monospace cell.
    pub(crate) fn cell_width(&self, cx: &App) -> f32 {
        let font = font(self.font_family.clone());
        let ts = cx.text_system();
        let id = ts.resolve_font(&font);
        ts.advance(id, px(self.font_size), 'm').map(|s| f32::from(s.width)).unwrap_or(self.font_size * 0.6)
    }

    pub(crate) fn pause_blink(&mut self, cx: &mut Context<Self>) {
        self.blink_epoch = self.blink_epoch.wrapping_add(1);
        self.blink_visible = true;
        if self.focused {
            self.schedule_blink(cx);
        }
    }

    fn schedule_blink(&mut self, cx: &mut Context<Self>) {
        let epoch = self.blink_epoch;
        let timer = cx.background_executor().timer(BLINK);
        self.blink_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            this.update(cx, |this, cx| {
                if this.blink_epoch != epoch || !this.focused {
                    return;
                }
                this.blink_visible = !this.blink_visible;
                cx.notify();
                this.schedule_blink(cx);
            })
            .ok();
        }));
    }

    fn on_focus(&mut self, cx: &mut Context<Self>) {
        self.focused = true;
        self.pause_blink(cx);
        cx.emit(EditorEvent::Focused);
        cx.notify();
    }

    fn on_blur(&mut self, cx: &mut Context<Self>) {
        self.focused = false;
        self.blink_visible = true;
        self.blink_task = None;
        self.completion = None;
        self.drag = None;
        cx.emit(EditorEvent::Blurred);
        cx.notify();
    }

    fn on_settings_changed(&mut self, cx: &mut Context<Self>) {
        let s = read_settings(cx);
        self.font_family = s.buffer_font_family.clone().into();
        self.font_size = s.buffer_font_size;
        self.line_height_mult = s.buffer_line_height;
        self.editor.set_vim_clipboard_unnamed(s.vim.clipboard_unnamed);
        let km = keymap_of(s.keymap);
        if km != self.editor.keymap() {
            self.editor.set_keymap(km);
            self.completion = None;
            cx.emit(EditorEvent::StateChanged);
        }
        self.style = None;
        cx.notify();
    }

    // ---- key input -------------------------------------------------------------------------------

    pub(crate) fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let mods = KeyMods {
            control: ks.modifiers.control,
            alt: ks.modifiers.alt,
            shift: ks.modifiers.shift,
            platform: ks.modifiers.platform,
        };
        let Some(input) = translate_key(&ks.key, ks.key_char.as_deref(), mods) else { return };
        if self.handle_input(&input, window, cx) {
            cx.stop_propagation();
        }
    }

    /// Feed one translated key; returns whether the engine (or the completion popup) used it.
    pub(crate) fn handle_input(
        &mut self,
        input: &KeyInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.pause_blink(cx);
        if self.completion.is_some() && self.completion_key(input, window, cx) {
            return true;
        }
        let before = self.state_key();
        let rev0 = self.editor.revision();
        self.editor.set_clipboard(cx.read_from_clipboard().and_then(|i| i.text()));
        let outcome = self.editor.handle_key(input);
        if self.read_only && self.editor.revision() != rev0 {
            // The engine has no read-only mode: take the edit back before anyone sees it.
            self.editor.undo();
        }
        let edited = self.editor.revision() != rev0 && !self.read_only;
        self.finish_input(outcome.effects, Some(input), rev0, before, edited, window, cx);
        outcome.handled
    }

    /// Keys that belong to an open completion popup. Returns true when consumed.
    fn completion_key(&mut self, input: &KeyInput, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(key) = classify_completion_key(input) else { return false };
        match key {
            CompletionKey::Move(delta) => self.move_completion(delta, cx),
            CompletionKey::Accept => self.accept_completion(window, cx),
            CompletionKey::Enter => {
                // Accepting an identical word would swallow the newline: dismiss and let the
                // engine insert it. Tab still accepts.
                let current = self.completion.as_ref().map(|c| self.editor.slice(c.replace.clone()));
                let noop = self.completion.as_ref().and_then(|c| c.selected_item()).is_some_and(|i| {
                    current.as_deref().is_some_and(|cur| is_noop_accept(&i.insert_text, cur))
                });
                if noop || self.completion.as_ref().is_some_and(|c| c.enter_means_newline()) {
                    self.completion = None;
                    cx.notify();
                    return false;
                }
                self.accept_completion(window, cx)
            }
            CompletionKey::Close => {
                self.completion = None;
                cx.notify();
            }
        }
        true
    }

    fn move_completion(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(c) = &mut self.completion {
            c.move_selection(delta);
            cx.notify();
        }
    }

    /// Text arriving from the platform input method (committed IME text, dictation, a paste
    /// routed through the text-input path).
    pub(crate) fn insert_from_platform(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only || text.is_empty() {
            return;
        }
        let before = self.state_key();
        let rev0 = self.editor.revision();
        let outcome = self.editor.insert_text(text);
        let edited = self.editor.revision() != rev0;
        self.finish_input(outcome.effects, None, rev0, before, edited, window, cx);
    }

    /// Common tail of every input path: apply effects, raise events, refresh popups.
    #[allow(clippy::too_many_arguments)]
    fn finish_input(
        &mut self,
        effects: Vec<Effect>,
        key: Option<&KeyInput>,
        rev0: u64,
        before: (Selection, usize, Mode, String),
        edited: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for effect in effects {
            self.apply_effect(effect, window, cx);
        }
        if edited {
            self.on_edited(cx);
        }
        let after = self.state_key();
        if after != before || edited {
            cx.emit(EditorEvent::StateChanged);
        }
        // Which-key timing: start the clock when a pending sequence appears.
        if self.editor.key_hints().is_some() {
            if self.hints_since.is_none() || before.3.is_empty() {
                self.hints_since = Some(Instant::now());
                let delay = if self.editor.pending_keys() == "space" { Duration::ZERO } else { HINT_DELAY };
                if !delay.is_zero() {
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(HINT_DELAY).await;
                        this.update(cx, |_, cx| cx.notify()).ok();
                    })
                    .detach();
                }
            }
        } else {
            self.hints_since = None;
        }
        self.update_completion(key, edited, rev0, before.0, window, cx);
        self.hover = None;
        cx.notify();
    }

    fn on_edited(&mut self, cx: &mut Context<Self>) {
        let rev = self.editor.revision();
        self.statement_status = None;
        if self.transient.as_ref().is_some_and(|(r, _)| *r != rev) {
            self.transient = None;
        }
        self.shift_diagnostics();
        self.note_typing(cx);
        self.schedule_diagnostics_in(DIAGNOSTICS_DEBOUNCE, Some(cx));
        cx.emit(EditorEvent::Edited { revision: rev });
    }

    /// Carry the current squiggles through the edit that just happened, so they do not sit on
    /// the wrong text until the next lint result arrives; ranges the edit touched are dropped.
    fn shift_diagnostics(&mut self) {
        let old = self.text_snapshot.as_ref().map(|(_, t)| t.clone());
        let new = self.text_snapshot();
        let Some(old) = old else { return };
        let Some((start, removed, inserted)) = crate::util::diff_edit(&old, &new) else { return };
        self.diagnostics.retain_mut(|d| {
            match crate::util::shift_range(d.range.clone(), start, removed, inserted) {
                Some(r) => {
                    d.range = r;
                    true
                }
                None => false,
            }
        });
    }

    /// Remember the statement being typed (insert-like modes only) and repaint once typing pauses.
    fn note_typing(&mut self, cx: &mut Context<Self>) {
        let insertish = matches!(self.editor.mode(), Mode::Insert | Mode::Standard);
        if !insertish || self.diagnostics_provider.is_none() || self.editor.len_chars() > BIG_DOCUMENT_CHARS {
            self.typing = None;
            return;
        }
        let cursor = self.primary_cursor();
        let text = self.text_snapshot();
        let span = quarry_sql::statement_at(&text, self.editor.char_to_byte(cursor), self.db_kind);
        self.typing = span.map(|s| {
            let mut r = self.byte_range_to_chars(s.range);
            // The cursor usually sits in whitespace typed after the statement's last token.
            let len = self.editor.len_chars();
            while r.end < len && self.editor.slice(r.end..r.end + 1).trim().is_empty() {
                r.end += 1;
            }
            r
        });
        self.typing_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TYPING_IDLE).await;
            this.update(cx, |v, cx| {
                v.typing = None;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Whether `d` is an "unfinished statement" complaint about the statement still being typed.
    fn held_back(&self, d: &Diag) -> bool {
        let Some(stmt) = &self.typing else { return false };
        let cursor = self.primary_cursor();
        matches!(self.editor.mode(), Mode::Insert | Mode::Standard)
            && cursor >= stmt.start
            && cursor <= stmt.end
            && d.range.start >= stmt.start
            && d.range.end <= stmt.end + 1
            && quarry_sql::is_incomplete_while_typing(&quarry_sql::Diagnostic {
                range: 0..0,
                severity: d.severity,
                message: d.message.clone(),
            })
    }

    /// Text changed outside the key path (IME, menu commands): raise the usual events.
    pub(crate) fn edited_externally(&mut self, cx: &mut Context<Self>) {
        self.on_edited(cx);
        cx.emit(EditorEvent::StateChanged);
        cx.notify();
    }

    fn after_external_change(
        &mut self,
        rev0: u64,
        before: (Selection, usize, Mode, String),
        cx: &mut Context<Self>,
    ) {
        let edited = self.editor.revision() != rev0;
        if edited {
            self.completion = None;
            self.on_edited(cx);
        }
        if edited || self.state_key() != before {
            self.pending_reveal = true;
            cx.emit(EditorEvent::StateChanged);
        }
        cx.notify();
    }

    fn apply_effect(&mut self, effect: Effect, window: &mut Window, cx: &mut Context<Self>) {
        match effect {
            Effect::SetClipboard(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            Effect::RevealCursor => self.pending_reveal = true,
            Effect::Scroll(req) => self.pending_scroll = Some(req),
            Effect::Status(msg) => {
                self.message_serial += 1;
                let id = self.message_serial;
                self.message = Some((msg, id));
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(MESSAGE_TTL).await;
                    this.update(cx, |v, cx| {
                        if v.message.as_ref().is_some_and(|m| m.1 == id) {
                            v.message = None;
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
            Effect::RequestCompletion if self.completion_provider.is_some() => {
                self.completion = self.query_completion(true);
            }
            Effect::DismissCompletion if self.completion_provider.is_some() => {
                self.completion = None;
            }
            other => {
                let _ = window;
                cx.emit(EditorEvent::Effect(other));
            }
        }
    }

    // ---- completion ---------------------------------------------------------------------------

    /// Ask the provider at the primary cursor. `manual` skips the "typed an identifier char" gate.
    fn query_completion(&mut self, manual: bool) -> Option<CompletionState> {
        let provider = self.completion_provider.clone()?;
        let cursor = self.primary_cursor();
        let text = self.text_snapshot();
        let list = provider.complete(&text, self.editor.char_to_byte(cursor));
        let replace = self.byte_range_to_chars(list.replace_range.clone());
        let prefix = self.editor.slice(replace.clone());
        if !manual {
            let after_dot = replace.is_empty() && cursor > 0 && self.editor.slice(cursor - 1..cursor) == ".";
            let numeric = !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit());
            if (prefix.is_empty() && !after_dot) || numeric {
                return None;
            }
        }
        let mut state = CompletionState::new(list, replace, prefix)?;
        // The word is already complete and nothing else is on offer: the popup would only be in
        // the way (and Enter would have nothing to do).
        if !manual && state.items.len() == 1 && is_noop_accept(&state.items[0].insert_text, &state.prefix) {
            return None;
        }
        state.manual = manual;
        Some(state)
    }

    fn update_completion(
        &mut self,
        key: Option<&KeyInput>,
        edited: bool,
        _rev0: u64,
        _sel0: Selection,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if self.completion_provider.is_none() {
            return;
        }
        let insertish =
            matches!(self.editor.mode(), Mode::Insert | Mode::Standard) && self.editor.prompt().is_none();
        if !insertish {
            self.completion = None;
            return;
        }
        let typed = match key.map(|k| (&k.key, &k.modifiers)) {
            Some((Key::Char(c), m)) if !m.ctrl && !m.alt && !m.platform => Some(*c),
            _ => None,
        };
        let is_trigger_char = typed.is_some_and(|c| is_word_char(c) || c == '.');
        let dot = typed == Some('.');
        if edited && typed == Some(' ') {
            let cursor = self.primary_cursor();
            let before = self.editor.slice(cursor.saturating_sub(200)..cursor);
            if auto_trigger_after_space(&before) {
                // Replaces a popup still open for the keyword itself (`from|` -> `from |`).
                self.completion = self.query_completion(true);
                return;
            }
        }
        match (&self.completion, edited) {
            (Some(open), true) => {
                let start = open.replace.start;
                let next = self.query_completion(dot);
                // Typing on in the same word keeps the popup; any other word (a space, a paren,
                // a new statement) closes it. A `.` always starts a fresh completion.
                self.completion = match next {
                    Some(n) if dot || n.replace.start == start => Some(n),
                    _ => None,
                };
            }
            (Some(open), false) => {
                // Cursor moves: stay open only while the cursor is still inside the word.
                let cursor = self.primary_cursor();
                let inside = cursor >= open.replace.start && cursor <= open.replace.end;
                if !inside {
                    self.completion = None;
                } else {
                    let manual = open.manual;
                    self.completion = self.query_completion(manual);
                }
            }
            (None, true) if is_trigger_char => {
                self.completion = self.query_completion(false);
            }
            _ => {}
        }
    }

    pub(crate) fn accept_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.completion.take() else { return };
        let Some(item) = state.selected_item().cloned() else { return };
        if self.read_only {
            return;
        }
        let before = self.state_key();
        let rev0 = self.editor.revision();
        // The popup is re-queried after every edit, so `replace` always ends at the cursor; one
        // `replace_range` makes the whole acceptance a single undo step and bypasses auto-pairs,
        // which is what keeps `count(` from becoming `count()` + a typed `)`.
        self.editor.replace_range(state.replace.clone(), &item.insert_text);
        self.pending_reveal = true;
        let _ = window;
        self.on_edited(cx);
        cx.emit(EditorEvent::StateChanged);
        let _ = (rev0, before);
        // `schema.` / `alias.` continue straight into the member list.
        if item.insert_text.ends_with('.') {
            self.completion = self.query_completion(false);
        }
        cx.notify();
    }

    pub(crate) fn select_completion(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(c) = &mut self.completion
            && c.selected != index
        {
            c.select(index);
            cx.notify();
        }
    }

    // ---- context-menu commands ----------------------------------------------------------------

    /// The selected text as the context menu should copy it: Vim's block-wise Visual mode is one
    /// selection per line, joined here by newlines like a blockwise yank.
    fn menu_selection_text(&self) -> Option<String> {
        if self.editor.keymap() == Keymap::Vim && self.editor.mode() == Mode::VisualBlock {
            let rows: Vec<String> = self.editor.selections().iter().map(|s| self.editor.slice(s.range())).collect();
            return (!rows.is_empty()).then(|| rows.join("\n"));
        }
        self.editor.selected_text()
    }

    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.menu_selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(crate) fn cut_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(text) = self.menu_selection_text() else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if self.editor.keymap() == Keymap::Vim {
            // `d` removes the Visual area the way Vim does (all rows of a block, registers updated).
            self.handle_input(&KeyInput::ch('d'), window, cx);
            return;
        }
        let r = self.editor.primary().range();
        self.editor.replace_range(r, "");
        self.pending_reveal = true;
        self.completion = None;
        self.edited_externally(cx);
    }

    pub(crate) fn paste_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) else { return };
        match (self.editor.keymap(), self.editor.mode()) {
            (Keymap::Helix, Mode::Normal | Mode::Select)
            | (Keymap::Vim, Mode::Normal | Mode::Visual | Mode::VisualLine | Mode::VisualBlock) => {
                // Neither modal keymap has a "paste" command that reads the system clipboard other than `space p`.
                self.handle_input(&KeyInput::ch(' '), window, cx);
                self.handle_input(&KeyInput::ch('p'), window, cx);
            }
            _ => self.insert_from_platform(&text, window, cx),
        }
    }

    pub(crate) fn emit_effect(&mut self, effect: Effect, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::Effect(effect));
    }

    /// The effect a "Run" menu entry maps to: the selection when there is one.
    pub(crate) fn run_effect(&self) -> Effect {
        match self.editor.selected_text() {
            Some(t) => Effect::RunSelection(t),
            None => Effect::RunStatementAtCursor,
        }
    }

    // ---- diagnostics -----------------------------------------------------------------------------

    fn schedule_diagnostics_in(&mut self, delay: Duration, cx: Option<&mut Context<Self>>) {
        let Some(provider) = self.diagnostics_provider.clone() else { return };
        let rev = self.editor.revision();
        let Some(cx) = cx else {
            // Constructed without a context at hand: the next edit or render schedules it.
            self.diag_task = None;
            self.pending_initial_diagnostics = true;
            return;
        };
        self.spawn_diagnostics(provider, rev, delay, cx);
    }

    pub(crate) fn spawn_diagnostics(
        &mut self,
        provider: DiagnosticsProvider,
        rev: u64,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        self.diag_task = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let Ok(text) = this.update(cx, |v, _| v.text_snapshot()) else { return };
            let result = cx.background_executor().spawn(async move { provider(&text) }).await;
            this.update(cx, |v, cx| v.apply_diagnostics(rev, result, cx)).ok();
        }));
    }

    fn apply_diagnostics(&mut self, rev: u64, result: Vec<Diagnostic>, cx: &mut Context<Self>) {
        if self.editor.revision() != rev {
            return; // a newer job is already on its way
        }
        let mut diags: Vec<Diag> = result
            .into_iter()
            .map(|d| Diag {
                range: self.byte_range_to_chars(d.range.clone()),
                severity: d.severity,
                message: d.message,
            })
            .collect();
        diags.sort_by_key(|d| (d.range.start, d.range.end));
        self.diagnostics = diags;
        cx.notify();
    }

    /// Diagnostics at a char index (transient first), as indexes into `all_diagnostics`.
    pub(crate) fn all_diagnostics(&self) -> Vec<&Diag> {
        let mut v: Vec<&Diag> = self.diagnostics.iter().filter(|d| !self.held_back(d)).collect();
        if let Some((rev, d)) = &self.transient
            && *rev == self.editor.revision()
        {
            v.push(d);
        }
        v
    }

    // ---- statement under cursor ------------------------------------------------------------------

    /// Lines `(first, last)` of the statement under the primary cursor, cached per
    /// (revision, cursor).
    ///
    /// Finding the statement lexes the whole script, which costs milliseconds on a huge buffer.
    /// While keys repeat on such a buffer the previous answer is reused (at most every 120 ms is
    /// recomputed) and one refresh is scheduled for when input pauses, so the gutter band lags
    /// by a blink instead of making every keystroke slow.
    pub(crate) fn statement_lines(&mut self, cx: &mut Context<Self>) -> Option<(usize, usize)> {
        let cursor = self.primary_cursor();
        let key = (self.editor.revision(), cursor);
        if let Some((k, v)) = &self.stmt_cache
            && *k == key
        {
            return *v;
        }
        if self.editor.len_chars() > BIG_DOCUMENT_CHARS
            && let Some((_, stale)) = &self.stmt_cache
            && self.stmt_at.elapsed() < STATEMENT_THROTTLE
        {
            let stale = *stale;
            if !self.stmt_refresh_pending {
                self.stmt_refresh_pending = true;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(STATEMENT_THROTTLE + Duration::from_millis(10)).await;
                    this.update(cx, |v, cx| {
                        v.stmt_refresh_pending = false;
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
            return stale;
        }
        self.stmt_at = Instant::now();
        let text = self.text_snapshot();
        let span = quarry_sql::statement_at(&text, self.editor.char_to_byte(cursor), self.db_kind);
        let lines = span.map(|s| {
            let r = self.byte_range_to_chars(s.range);
            let first = self.editor.char_to_line(r.start);
            // A range ending at a line start (statement followed by `;` on its own line) stops above it.
            let end = r.end.saturating_sub(1).max(r.start);
            (first, self.editor.char_to_line(end))
        });
        self.stmt_cache = Some((key, lines));
        lines
    }

    // ---- coordinates --------------------------------------------------------------------------------

    /// Char index under a window-space point; `None` before the first paint.
    pub(crate) fn char_at(&mut self, pos: Point<Pixels>, cx: &App) -> Option<usize> {
        let layout = self.layout.clone()?;
        let line_count = self.editor.line_count();
        let y = f32::from(pos.y - layout.bounds.origin.y);
        let line = geometry::line_at_y(y, self.scroll_y, layout.line_h, line_count);
        let entry = self.line_entry(line, cx);
        let x = f32::from(pos.x - layout.bounds.origin.x) - layout.gutter_w + self.scroll_x;
        let nearest = self.editor.keymap() == Keymap::Standard || self.editor.mode() == Mode::Insert;
        let col = entry.col_at_x(px(x), nearest);
        Some(self.editor.line_to_char(line) + col)
    }

    /// Window-space bounds of the char at `idx` (for IME candidate windows), when it is laid out.
    pub(crate) fn bounds_of_char(&mut self, idx: usize, cx: &App) -> Option<Bounds<Pixels>> {
        let layout = self.layout.clone()?;
        let line = self.editor.char_to_line(idx);
        let col = idx - self.editor.line_to_char(line);
        let entry = self.line_entry(line, cx);
        let x = f32::from(entry.x_of_col(col)) - self.scroll_x + layout.gutter_w;
        let next = f32::from(entry.x_of_col(col + 1));
        let w = (next - f32::from(entry.x_of_col(col))).max(layout.cell_w * 0.2);
        let y = line as f32 * layout.line_h - self.scroll_y;
        Some(Bounds::new(
            Point::new(layout.bounds.origin.x + px(x), layout.bounds.origin.y + px(y)),
            gpui_kit::size(px(w), px(layout.line_h)),
        ))
    }

    // ---- mouse -----------------------------------------------------------------------------------------

    pub(crate) fn on_scroll(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        let Some(layout) = self.layout.clone() else { return };
        let (mx, my) = layout.max_scroll;
        self.scroll_x = (self.scroll_x - dx).clamp(0.0, mx);
        self.scroll_y = (self.scroll_y - dy).clamp(0.0, my);
        self.scroll_activity = Some(Instant::now());
        self.hover = None;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SCROLLBAR_LINGER + Duration::from_millis(250)).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
        cx.notify();
    }

    /// Whether `idx` lies inside one of the real selections.
    fn inside_selection(&self, idx: usize) -> bool {
        self.editor
            .selections()
            .iter()
            .any(|s| crate::util::selection_is_real(&self.editor, s) && s.from() <= idx && idx < s.to())
    }

    pub(crate) fn on_mouse_down_text(
        &mut self,
        idx: usize,
        button: gpui_kit::MouseButton,
        click_count: usize,
        mods: gpui_kit::Modifiers,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::MouseButton;
        self.pause_blink(cx);
        self.completion = None;
        let before = self.state_key();
        match button {
            MouseButton::Left => {
                let add = mods.alt || mods.platform;
                match click_count {
                    0 | 1 => self.editor.click(idx, mods.shift, add),
                    2 => self.editor.double_click(idx),
                    _ => self.editor.triple_click(idx),
                }
                self.drag = Some(Drag::Text);
                self.drag_moved = false;
            }
            MouseButton::Right => {
                if !self.inside_selection(idx) {
                    self.editor.click(idx, false, false);
                }
            }
            // The primary selection only exists on Linux/FreeBSD (gpui has no API for it elsewhere).
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            MouseButton::Middle => {
                // Primary-selection paste: put the caret where the click landed and insert.
                if let Some(text) = cx.read_from_primary().and_then(|i| i.text())
                    && !self.read_only
                {
                    self.editor.click(idx, false, false);
                    let rev0 = self.editor.revision();
                    let outcome = self.editor.insert_text(&text);
                    let edited = self.editor.revision() != rev0;
                    for e in outcome.effects {
                        if !matches!(e, Effect::RevealCursor) {
                            cx.emit(EditorEvent::Effect(e));
                        }
                    }
                    if edited {
                        self.on_edited(cx);
                    }
                }
            }
            _ => {}
        }
        self.pending_reveal = false;
        if self.state_key() != before {
            cx.emit(EditorEvent::StateChanged);
        }
        cx.notify();
    }

    pub(crate) fn on_mouse_drag_text(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.drag_moved = true;
        self.editor.drag_to(idx);
        self.pause_blink(cx);
        cx.emit(EditorEvent::StateChanged);
        cx.notify();
    }

    pub(crate) fn on_mouse_up(&mut self, cx: &mut Context<Self>) {
        let was_text_drag = self.drag == Some(Drag::Text);
        self.drag = None;
        if was_text_drag
            && let Some(sel) = self.editor.selected_text()
            && sel.len() < 1_000_000
        {
            // X11/Wayland convention: whatever is selected becomes the primary selection.
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            cx.write_to_primary(ClipboardItem::new_string(sel));
            #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
            let _ = sel;
        }
        cx.notify();
    }

    // ---- scrollbar helpers used by the element ---------------------------------------------------------

    pub(crate) fn scrollbar_alpha(&self, hovered: bool) -> f32 {
        if self.drag.is_some_and(|d| matches!(d, Drag::VThumb { .. } | Drag::HThumb { .. })) || hovered {
            return 1.0;
        }
        match self.scroll_activity {
            Some(t) => {
                let e = t.elapsed();
                if e < SCROLLBAR_LINGER {
                    1.0
                } else if e < SCROLLBAR_LINGER + Duration::from_millis(200) {
                    1.0 - (e - SCROLLBAR_LINGER).as_secs_f32() / 0.2
                } else {
                    0.0
                }
            }
            None => 0.0,
        }
    }

    // ---- IME helpers ---------------------------------------------------------------------------------------

    pub(crate) fn utf16_to_char(&self, utf16: usize) -> usize {
        let text = self.text_snapshot_ref();
        let mut count = 0usize;
        for (i, c) in text.chars().enumerate() {
            if count >= utf16 {
                return i;
            }
            count += c.len_utf16();
        }
        text.chars().count()
    }

    pub(crate) fn char_to_utf16(&self, ch: usize) -> usize {
        self.text_snapshot_ref().chars().take(ch).map(char::len_utf16).sum()
    }

    fn text_snapshot_ref(&self) -> Arc<str> {
        match &self.text_snapshot {
            Some((r, t)) if *r == self.editor.revision() => t.clone(),
            _ => Arc::from(self.editor.text()),
        }
    }

    pub(crate) fn ime_selection(&self) -> UTF16Selection {
        let sel = self.editor.primary();
        let (from, to) = if self.editor.keymap() == Keymap::Standard || self.editor.mode() == Mode::Insert {
            let c = self.primary_cursor();
            if self.editor.keymap() == Keymap::Standard { (sel.from(), sel.to()) } else { (c, c) }
        } else {
            (sel.from(), sel.to())
        };
        UTF16Selection {
            range: self.char_to_utf16(from)..self.char_to_utf16(to),
            reversed: !sel.is_forward() && sel.from() != sel.to(),
        }
    }
}
