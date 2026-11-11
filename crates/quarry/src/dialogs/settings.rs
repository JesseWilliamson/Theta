//! The Settings dialog: Appearance, Editor, Data, Map, Keymap reference, About.
//! Every control writes straight to `Settings`, which persists and applies live.

use crate::exec::keyref;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, IndexPath, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, px,
};
use quarry_ui::settings::{DEFAULT_TILE_URL, KeymapMode, MAX_FONT_SIZE, MIN_FONT_SIZE};
use quarry_ui::{CHROME_TEXT_SIZE, KeyHint, Settings};
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Appearance,
    Editor,
    Data,
    Map,
    Keymap,
    About,
}

impl Section {
    const ALL: [Section; 6] =
        [Section::Appearance, Section::Editor, Section::Data, Section::Map, Section::Keymap, Section::About];
    fn label(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Editor => "Editor",
            Section::Data => "Data",
            Section::Map => "Map",
            Section::Keymap => "Keymap",
            Section::About => "About",
        }
    }
}

const ROW_LIMITS: [usize; 8] = [100, 500, 1000, 5000, 10_000, 50_000, 100_000, 0];

fn font_names(cx: &App) -> &'static [&'static str] {
    static NAMES: OnceLock<Vec<&'static str>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut v: Vec<String> = cx.text_system().all_font_names();
        v.push(quarry_ui::settings::DEFAULT_UI_FONT.into());
        v.push(quarry_ui::settings::DEFAULT_BUFFER_FONT.into());
        v.retain(|n| !n.starts_with('.'));
        v.sort_by_key(|a| a.to_lowercase());
        v.dedup();
        v.into_iter().map(|s| &*Box::leak(s.into_boxed_str())).collect()
    })
}

pub struct SettingsView {
    section: Section,
    ui_font: Entity<SelectState<Vec<&'static str>>>,
    buf_font: Entity<SelectState<Vec<&'static str>>>,
    tile: Entity<InputState>,
    filter: Entity<InputState>,
    /// (section, label, keys) rows of the app and editor key reference.
    rows: Vec<(String, String, String)>,
    _subs: Vec<Subscription>,
}

pub fn open(window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| SettingsView::new(window, cx));
    window.open_dialog(cx, move |dialog, _, _| dialog.title("Settings").w(px(820.)).child(view.clone()));
}

fn font_select(
    current: &str,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Entity<SelectState<Vec<&'static str>>> {
    let names = font_names(cx).to_vec();
    let ix = names.iter().position(|n| n.eq_ignore_ascii_case(current)).map(IndexPath::new);
    cx.new(|cx| SelectState::new(names, ix, window, cx).searchable(true))
}

impl SettingsView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let s = Settings::get(cx).clone();
        let ui_font = font_select(&s.ui_font_family, window, cx);
        let buf_font = font_select(&s.buffer_font_family, window, cx);
        let tile = cx.new(|cx| InputState::new(window, cx).default_value(s.map_tile_url.clone()));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search actions and keys"));
        let mut rows: Vec<(String, String, String)> = crate::palette::command_list(window);
        let mode = s.keymap;
        rows.extend(
            keyref::editor_keys(mode)
                .into_iter()
                .map(|k| (format!("Editor · {}", k.section), k.action, k.keys)),
        );
        let subs = vec![
            cx.subscribe_in(&ui_font, window, |_, _, e: &SelectEvent<Vec<&'static str>>, _, cx| {
                if let SelectEvent::Confirm(Some(name)) = e {
                    let n = name.to_string();
                    Settings::update(cx, |s| s.ui_font_family = n);
                }
            }),
            cx.subscribe_in(&buf_font, window, |_, _, e: &SelectEvent<Vec<&'static str>>, _, cx| {
                if let SelectEvent::Confirm(Some(name)) = e {
                    let n = name.to_string();
                    Settings::update(cx, |s| s.buffer_font_family = n);
                }
            }),
            cx.subscribe_in(&tile, window, |_, input, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::Change) {
                    let v = input.read(cx).value().to_string();
                    if v.starts_with("http") && v.contains("{z}") {
                        Settings::update(cx, |s| s.map_tile_url = v);
                    }
                }
            }),
            cx.subscribe_in(&filter, window, |_, _, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.observe_global::<quarry_ui::settings::SettingsStore>(|_, cx| cx.notify()),
        ];
        Self { section: Section::Appearance, ui_font, buf_font, tile, filter, rows, _subs: subs }
    }

    fn row(&self, label: &str, hint: Option<&str>, control: impl IntoElement, cx: &App) -> AnyElement {
        let theme = cx.theme();
        h_flex()
            .py_2()
            .gap_4()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(theme.border.opacity(0.5))
            .child(
                v_flex()
                    .gap_0p5()
                    .child(div().text_size(px(13.)).child(SharedString::from(label.to_string())))
                    .when_some(hint, |d, h| {
                        d.child(
                            div()
                                .text_size(CHROME_TEXT_SIZE)
                                .text_color(theme.muted_foreground)
                                .child(SharedString::from(h.to_string())),
                        )
                    }),
            )
            .child(div().flex_shrink_0().child(control))
            .into_any_element()
    }

    fn stepper(
        &self,
        id: &'static str,
        text: String,
        dec: impl Fn(&mut Settings) + 'static,
        inc: impl Fn(&mut Settings) + 'static,
    ) -> AnyElement {
        h_flex()
            .gap_1()
            .items_center()
            .child(
                Button::new(SharedString::from(format!("{id}-dec")))
                    .small()
                    .label("−")
                    .on_click(move |_, _, cx| Settings::update(cx, |s| dec(s))),
            )
            .child(div().w(px(64.)).flex().justify_center().text_size(px(13.)).child(text))
            .child(
                Button::new(SharedString::from(format!("{id}-inc")))
                    .small()
                    .label("+")
                    .on_click(move |_, _, cx| Settings::update(cx, |s| inc(s))),
            )
            .into_any_element()
    }

    fn appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let s = Settings::get(cx).clone();
        let theme_name = quarry_ui::themes::current_name(cx).to_string();
        let clamp = |v: f32| v.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        v_flex()
            .child(self.row(
                "Theme",
                Some("Changes apply immediately"),
                Button::new("settings-theme").label(SharedString::from(format!("{theme_name} …"))).on_click(
                    |_, window, cx| {
                        window.close_dialog(cx);
                        window.dispatch_action(Box::new(crate::actions::SelectTheme), cx);
                    },
                ),
                cx,
            ))
            .child(self.row(
                "Follow system appearance",
                Some("Switch between the light and dark theme with the OS"),
                Switch::new("follow-system").checked(s.follow_system_appearance).on_change(|v, _, cx| {
                    let v = *v;
                    Settings::update(cx, |s| s.follow_system_appearance = v)
                }),
                cx,
            ))
            .child(self.row("Interface font", None, div().w(px(230.)).child(Select::new(&self.ui_font)), cx))
            .child(self.row("Editor font", None, div().w(px(230.)).child(Select::new(&self.buf_font)), cx))
            .child(self.row(
                "Editor font size",
                None,
                self.stepper(
                    "buf-size",
                    format!("{:.0}", s.buffer_font_size),
                    move |s| s.buffer_font_size = clamp(s.buffer_font_size - 1.0),
                    move |s| s.buffer_font_size = clamp(s.buffer_font_size + 1.0),
                ),
                cx,
            ))
            .child(self.row(
                "Editor line height",
                Some("Multiple of the font size"),
                self.stepper(
                    "line-h",
                    format!("{:.2}", s.buffer_line_height),
                    |s| s.buffer_line_height = (s.buffer_line_height - 0.05).clamp(1.0, 2.5),
                    |s| s.buffer_line_height = (s.buffer_line_height + 0.05).clamp(1.0, 2.5),
                ),
                cx,
            ))
            .into_any_element()
    }

    fn editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let s = Settings::get(cx).clone();
        let seg = |id: &'static str, mode: KeymapMode| {
            let b = Button::new(id).small().label(mode.label());
            let b = if s.keymap == mode { b.primary() } else { b };
            b.on_click(move |_, _, cx| Settings::update(cx, |s| s.keymap = mode))
        };
        v_flex()
            .child(
                self.row(
                    "Keymap",
                    Some("Helix is modal; Standard behaves like a conventional editor"),
                    h_flex()
                        .gap_1()
                        .child(seg("km-helix", KeymapMode::Helix))
                        .child(seg("km-std", KeymapMode::Standard)),
                    cx,
                ),
            )
            .child(self.row(
                "Format: indent width",
                None,
                self.stepper(
                    "fmt-indent",
                    s.format.indent.to_string(),
                    |s| s.format.indent = s.format.indent.saturating_sub(1).max(1),
                    |s| s.format.indent = (s.format.indent + 1).min(8),
                ),
                cx,
            ))
            .child(self.row(
                "Format: uppercase keywords",
                None,
                Switch::new("fmt-upper").checked(s.format.uppercase_keywords).on_change(|v, _, cx| {
                    let v = *v;
                    Settings::update(cx, |s| s.format.uppercase_keywords = v)
                }),
                cx,
            ))
            .into_any_element()
    }

    fn data(&self, cx: &mut Context<Self>) -> AnyElement {
        let s = Settings::get(cx).clone();
        let pos = ROW_LIMITS.iter().position(|&n| n == s.row_limit).unwrap_or(2);
        let label = if s.row_limit == 0 {
            "No limit".to_string()
        } else {
            crate::workspace::console::fmt_int(s.row_limit as u64)
        };
        v_flex()
            .child(self.row(
                "Row limit",
                Some("Results stop after this many rows; the grid shows a truncated note"),
                self.stepper(
                    "row-limit",
                    label,
                    move |s| s.row_limit = ROW_LIMITS[pos.saturating_sub(1)],
                    move |s| s.row_limit = ROW_LIMITS[(pos + 1).min(ROW_LIMITS.len() - 1)],
                ),
                cx,
            ))
            .into_any_element()
    }

    fn map(&self, cx: &mut Context<Self>) -> AnyElement {
        let tile = self.tile.clone();
        let theme = cx.theme().clone();
        v_flex()
            .gap_2()
            .child(div().text_size(px(13.)).child("Tile URL template"))
            .child(
                div()
                    .text_size(CHROME_TEXT_SIZE)
                    .text_color(theme.muted_foreground)
                    .child("XYZ template with {z}, {x} and {y}. Respect the tile provider's usage policy."),
            )
            .child(Input::new(&self.tile))
            .child(div().pt_3().child(Button::new("tile-reset").label("Reset to OpenStreetMap").on_click(
                move |_, window, cx| {
                    Settings::update(cx, |s| s.map_tile_url = DEFAULT_TILE_URL.to_string());
                    tile.update(cx, |t, cx| t.set_value(DEFAULT_TILE_URL, window, cx));
                },
            )))
            .into_any_element()
    }

    fn keymap(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let q = self.filter.read(cx).value().to_lowercase();
        let rows: Vec<&(String, String, String)> = self
            .rows
            .iter()
            .filter(|(s, a, k)| q.is_empty() || [s, a, k].iter().any(|t| t.to_lowercase().contains(&q)))
            .collect();
        let mode = Settings::get(cx).keymap;
        v_flex()
            .gap_2()
            .child(Input::new(&self.filter).small().cleanable(true))
            .child(
                div().text_size(CHROME_TEXT_SIZE).text_color(theme.muted_foreground).child(format!(
                    "{} actions and {} editor keys ({} rows shown). Switch keymaps in Editor settings; reopen to refresh editor keys.",
                    self.rows.iter().filter(|r| !r.0.starts_with("Editor")).count(),
                    mode.label(),
                    rows.len()
                )),
            )
            .child(
                div().id("keymap-rows").h(px(380.)).overflow_y_scroll().child(v_flex().children(
                    rows.into_iter().take(400).map(|(section, action, keys)| {
                        h_flex()
                            .py_1()
                            .gap_3()
                            .items_center()
                            .border_b_1()
                            .border_color(theme.border.opacity(0.4))
                            .child(
                                div()
                                    .w(px(140.))
                                    .flex_shrink_0()
                                    .text_size(CHROME_TEXT_SIZE)
                                    .text_color(theme.muted_foreground)
                                    .child(SharedString::from(section.clone())),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.5))
                                    .child(SharedString::from(action.clone())),
                            )
                            .child(div().flex_shrink_0().when(!keys.is_empty(), |d| d.child(KeyHint::new(keys))))
                    }),
                )),
            )
            .into_any_element()
    }

    fn about(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let line = |t: String| div().text_size(px(13.)).text_color(theme.muted_foreground).child(t);
        v_flex()
            .gap_2()
            .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child("Quarry"))
            .child(line(format!("Version {}", env!("CARGO_PKG_VERSION"))))
            .child(line("A keyboard-first database IDE for PostgreSQL, MySQL, MariaDB and SQLite.".into()))
            .child(
                Button::new("about-repo")
                    .label("github.com/JesseWilliamson/Quarry")
                    .on_click(|_, _, cx| cx.open_url("https://github.com/JesseWilliamson/Quarry")),
            )
            .child(div().pt_3().font_weight(FontWeight::MEDIUM).text_size(px(13.)).child("Bundled assets"))
            .child(line("Fonts: IBM Plex Sans and Lilex, both under the SIL Open Font License 1.1.".into()))
            .child(line(
                "Themes and UI components: gpui-kit (Apache-2.0); each theme credits its author in NOTICE."
                    .into(),
            ))
            .child(line(
                "Map tiles: © OpenStreetMap contributors (ODbL) when the default URL is used.".into(),
            ))
            .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let body = match self.section {
            Section::Appearance => self.appearance(cx),
            Section::Editor => self.editor(cx),
            Section::Data => self.data(cx),
            Section::Map => self.map(cx),
            Section::Keymap => self.keymap(cx),
            Section::About => self.about(cx),
        };
        h_flex()
            .gap_4()
            .items_start()
            .h(px(470.))
            .child(v_flex().w(px(150.)).flex_shrink_0().gap_0p5().children(Section::ALL.into_iter().map(
                |sec| {
                    let active = sec == self.section;
                    div()
                        .id(sec.label())
                        .px_2()
                        .py_1p5()
                        .rounded_md()
                        .text_size(px(13.))
                        .cursor_default()
                        .when(active, |d| d.bg(theme.list_active).font_weight(FontWeight::MEDIUM))
                        .when(!active, |d| {
                            d.hover(|s| s.bg(theme.list_hover)).text_color(theme.muted_foreground)
                        })
                        .child(sec.label())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.section = sec;
                            cx.notify();
                        }))
                },
            )))
            .child(
                div().id("settings-body").flex_1().min_w_0().h_full().overflow_y_scroll().pr_1().child(body),
            )
    }
}
