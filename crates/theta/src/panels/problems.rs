//! Problems tab: what the linter and the resolver found in the active console, with jump-to.
//!
//! The list is the editor's own (`EditorView::problems`); the panel only shows it and says where the user
//! chose to go. Keyboard: Up / Down (or `j` / `k`) move, Enter jumps and returns to the editor, Escape returns.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, px,
};
use theta_editor_view::Problem;
use theta_sql::Severity;
use theta_ui::{CHROME_TEXT_SIZE, EmptyState};

#[derive(Debug, Clone, PartialEq)]
pub enum ProblemsEvent {
    /// Go to this byte offset of the active console and put the focus back in the editor.
    Jump(usize),
    /// Escape: back to the editor.
    Leave,
}

pub struct ProblemsPanel {
    problems: Vec<Problem>,
    selected: usize,
    scroll: ScrollHandle,
    focus: FocusHandle,
}

impl EventEmitter<ProblemsEvent> for ProblemsPanel {}

impl Focusable for ProblemsPanel {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

/// `(errors, warnings, infos)` of a list.
pub fn counts(problems: &[Problem]) -> (usize, usize, usize) {
    let c = |s: Severity| problems.iter().filter(|p| p.severity == s).count();
    (c(Severity::Critical), c(Severity::Warning), c(Severity::Info))
}

impl ProblemsPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self { problems: Vec::new(), selected: 0, scroll: ScrollHandle::new(), focus: cx.focus_handle() }
    }

    pub fn set(&mut self, mut problems: Vec<Problem>, cx: &mut Context<Self>) {
        // the most severe first, then in reading order
        problems.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.range.start.cmp(&b.range.start)));
        self.selected = self.selected.min(problems.len().saturating_sub(1));
        self.problems = problems;
        cx.notify();
    }

    pub fn len(&self) -> usize {
        self.problems.len()
    }

    pub fn is_empty(&self) -> bool {
        self.problems.is_empty()
    }

    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.problems.is_empty() {
            return;
        }
        let n = self.problems.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
        self.scroll.scroll_to_item(self.selected);
        cx.notify();
    }

    fn jump(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(p) = self.problems.get(ix) {
            self.selected = ix;
            cx.emit(ProblemsEvent::Jump(p.range.start));
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = ev.keystroke.key.as_str();
        match k {
            "down" | "j" => self.step(1, cx),
            "up" | "k" => self.step(-1, cx),
            "enter" => self.jump(self.selected, cx),
            "escape" => cx.emit(ProblemsEvent::Leave),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for ProblemsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        if self.problems.is_empty() {
            return EmptyState::new(IconName::Check, "No problems")
                .description(
                    "Syntax errors, unknown names and other findings in the active console are listed here.",
                )
                .into_any_element();
        }
        let mono = theta_ui::Settings::get(cx).buffer_font_family.clone();
        let _ = mono;
        let rows = self.problems.iter().enumerate().map(|(i, p)| {
            let selected = i == self.selected;
            let (icon, color) = match p.severity {
                Severity::Critical => (IconName::CircleX, theme.danger),
                Severity::Warning => (IconName::TriangleAlert, theme.warning),
                Severity::Info => (IconName::Info, theme.muted_foreground),
            };
            h_flex()
                .id(("problem", i))
                .h(px(24.))
                .px_2()
                .gap_2()
                .items_center()
                .text_size(CHROME_TEXT_SIZE)
                .when(selected, |d| d.bg(theme.list_active))
                .when(!selected, |d| d.hover(|s| s.bg(theme.list_hover)))
                .child(Icon::new(icon).size(px(13.)).text_color(color))
                .child(
                    div()
                        .w(px(56.))
                        .flex_shrink_0()
                        .text_color(theme.muted_foreground)
                        .child(format!("line {}", p.line + 1)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .child(SharedString::from(p.message.clone())),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.jump(i, cx)))
        });
        v_flex()
            .id("problems")
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev, window, cx| this.on_key(ev, window, cx)))
            .child(
                div()
                    .id("problems-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(v_flex().children(rows)),
            )
            .into_any_element()
    }
}
