//! Results tab: a slim sub-tab strip over the grid, or a one-line summary for
//! statements that return no rows.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use theta_grid::ResultGrid;
use theta_ui::{CHROME_TEXT_SIZE, EmptyState};

#[derive(Debug, Clone, PartialEq)]
pub enum ResultsEvent {
    ActivateTab(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub text: String,
    pub error: bool,
}

pub struct ResultsPanel {
    /// The grid on show: the active console's own grid (each console keeps its grid so staged
    /// edits, sorting and scroll survive switching tabs, and an async answer for a console that
    /// is not on screen lands in that console's grid).
    pub grid: Entity<ResultGrid>,
    /// Shown when there is no console.
    default_grid: Entity<ResultGrid>,
    titles: Vec<String>,
    active: usize,
    summary: Option<Summary>,
    show_grid: bool,
    running: bool,
}

impl EventEmitter<ResultsEvent> for ResultsPanel {}

impl ResultsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let grid = cx.new(|cx| ResultGrid::new(window, cx));
        Self {
            default_grid: grid.clone(),
            grid,
            titles: vec![],
            active: 0,
            summary: None,
            show_grid: false,
            running: false,
        }
    }

    /// Display `grid` (a console's), or the empty default one for no console.
    pub fn use_grid(&mut self, grid: Option<Entity<ResultGrid>>, cx: &mut Context<Self>) {
        let grid = grid.unwrap_or_else(|| self.default_grid.clone());
        if self.grid != grid {
            self.grid = grid;
            cx.notify();
        }
    }

    /// The console on show has a statement in flight and nothing to display yet.
    pub fn show_running(&mut self, cx: &mut Context<Self>) {
        self.titles.clear();
        self.summary = None;
        self.show_grid = false;
        self.running = true;
        cx.notify();
    }

    /// Show the grid with these sub-tab titles (the strip appears for more than one).
    pub fn show_sets(&mut self, titles: Vec<String>, active: usize, cx: &mut Context<Self>) {
        self.titles = titles;
        self.active = active;
        self.summary = None;
        self.show_grid = true;
        self.running = false;
        cx.notify();
    }

    pub fn show_summary(&mut self, text: String, error: bool, cx: &mut Context<Self>) {
        self.titles.clear();
        self.summary = Some(Summary { text, error });
        self.show_grid = false;
        self.running = false;
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.titles.clear();
        self.summary = None;
        self.show_grid = false;
        self.running = false;
        cx.notify();
    }

    pub fn set_active(&mut self, i: usize, cx: &mut Context<Self>) {
        self.active = i;
        cx.notify();
    }

    pub fn titles(&self) -> &[String] {
        &self.titles
    }
    pub fn summary(&self) -> Option<&Summary> {
        self.summary.as_ref()
    }
    pub fn is_running(&self) -> bool {
        self.running
    }
}

impl Render for ResultsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let strip = (self.show_grid && self.titles.len() > 1).then(|| {
            h_flex()
                .h(px(26.))
                .flex_shrink_0()
                .px_1()
                .gap_0p5()
                .items_center()
                .bg(theme.tab_bar)
                .border_b_1()
                .border_color(theme.border)
                .children(self.titles.iter().enumerate().map(|(i, t)| {
                    let active = i == self.active;
                    div()
                        .id(("result-tab", i))
                        .px_2()
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .rounded_sm()
                        .text_size(CHROME_TEXT_SIZE)
                        .cursor_default()
                        .text_color(if active { theme.foreground } else { theme.muted_foreground })
                        .when(active, |d| d.bg(theme.list_active))
                        .when(!active, |d| d.hover(|s| s.bg(theme.list_hover)))
                        .child(SharedString::from(t.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.active = i;
                            cx.emit(ResultsEvent::ActivateTab(i));
                            cx.notify();
                        }))
                }))
        });
        let body = if self.show_grid {
            div().flex_1().min_h_0().child(self.grid.clone()).into_any_element()
        } else if self.running {
            h_flex()
                .flex_1()
                .px_3()
                .pt_3()
                .gap_2()
                .items_start()
                .child(gpui_kit::component::spinner::Spinner::new().xsmall().color(theme.warning))
                .child(div().text_size(px(13.)).text_color(theme.muted_foreground).child("Running…"))
                .into_any_element()
        } else if let Some(s) = &self.summary {
            let (icon, color) = if s.error {
                (IconName::CircleAlert, theme.danger)
            } else {
                (IconName::Check, theme.success)
            };
            h_flex()
                .flex_1()
                .px_3()
                .pt_3()
                .gap_2()
                .items_start()
                .child(Icon::new(icon).size(px(14.)).text_color(color))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme.foreground)
                        .child(SharedString::from(s.text.clone())),
                )
                .into_any_element()
        } else {
            EmptyState::new(IconName::Table, "Run a query to see results")
                .description("Rows returned by a statement appear here.")
                .into_any_element()
        };
        v_flex().size_full().children(strip).child(body)
    }
}
