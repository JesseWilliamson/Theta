//! Results tab: a slim sub-tab strip over the grid, or a one-line summary for
//! statements that return no rows.

use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, ScrollEvent, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};
use theta_grid::ResultGrid;
use theta_ui::{CHROME_TEXT_SIZE, EmptyState, tool_button};

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
    /// The strip scrolls sideways when 60 result sets do not fit; the active one is always brought into view.
    strip: ScrollHandle,
    revealed: Option<usize>,
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
            strip: ScrollHandle::new(),
            revealed: None,
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
        self.revealed = None;
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

    /// Move to the next (`1`) or previous (`-1`) result set, wrapping, as the keys do.
    pub fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.titles.len();
        if n < 2 || !self.show_grid {
            return;
        }
        let i = (self.active as isize + delta).rem_euclid(n as isize) as usize;
        self.active = i;
        cx.emit(ResultsEvent::ActivateTab(i));
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
            // Bring the active set into view once per change (the strip scrolls, it does not wrap).
            if self.revealed != Some(self.active) {
                self.revealed = Some(self.active);
                self.strip.scroll_to_item(self.active);
            }
            let max = self.strip.max_offset().x;
            let overflowing = max > px(1.);
            let all: Vec<(usize, SharedString)> =
                self.titles.iter().enumerate().map(|(i, t)| (i, SharedString::from(t.clone()))).collect();
            let (active_ix, this) = (self.active, cx.entity().downgrade());
            h_flex()
                .h(px(26.))
                .flex_shrink_0()
                .bg(theme.tab_bar)
                .border_b_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .id("result-strip")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .px_1()
                        .gap_0p5()
                        .items_center()
                        .overflow_x_scroll()
                        .track_scroll(&self.strip)
                        // A vertical wheel has nothing else to do over the strip: it scrolls the sets sideways.
                        .on_scroll(cx.listener(|this, e: &ScrollEvent, _, cx| {
                            let d = e.delta.pixel_delta(px(24.));
                            if d.y.abs() > d.x.abs() {
                                let mut o = this.strip.offset();
                                let max = this.strip.max_offset().x;
                                o.x = (o.x + d.y).clamp(-max, px(0.));
                                this.strip.set_offset(o);
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }))
                        .children(self.titles.iter().enumerate().map(|(i, t)| {
                            let active = i == self.active;
                            let full: SharedString = t.clone().into();
                            div()
                                .id(("result-tab", i))
                                .px_2()
                                .h(px(20.))
                                .flex_shrink_0()
                                .max_w(px(220.))
                                .flex()
                                .items_center()
                                .rounded_sm()
                                .text_size(CHROME_TEXT_SIZE)
                                .cursor_default()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(if active { theme.foreground } else { theme.muted_foreground })
                                .when(active, |d| d.bg(theme.list_active))
                                .when(!active, |d| d.hover(|s| s.bg(theme.list_hover)))
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx)
                                })
                                .child(SharedString::from(t.clone()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.active = i;
                                    cx.emit(ResultsEvent::ActivateTab(i));
                                    cx.notify();
                                }))
                        })),
                )
                .when(overflowing, |d| {
                    d.child(
                        h_flex().h_full().px_1().items_center().child(
                            tool_button("result-sets-menu", IconName::ChevronDown, "All result sets")
                                .dropdown_menu(move |menu, _, _| {
                                    let mut menu = menu.max_h(px(360.)).scrollable(true);
                                    for (i, title) in &all {
                                        let (i, this) = (*i, this.clone());
                                        menu = menu.item(
                                            PopupMenuItem::new(title.clone())
                                                .checked(active_ix == i)
                                                .on_click(move |_, _, cx| {
                                                    this.update(cx, |p, cx| {
                                                        p.active = i;
                                                        cx.emit(ResultsEvent::ActivateTab(i));
                                                        cx.notify();
                                                    })
                                                    .ok();
                                                }),
                                        );
                                    }
                                    menu
                                }),
                        ),
                    )
                })
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
