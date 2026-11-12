//! The Settings dialog: Appearance, Editor, Templates, Map, Keymap reference, About.
//! Every control writes straight to `Settings`, which persists and applies live.

use crate::exec::keyref;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, IndexPath, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, px,
};
use std::sync::OnceLock;
use theta_ui::settings::{KeymapMode, MAX_FONT_SIZE, MIN_FONT_SIZE};
use theta_ui::{CHROME_TEXT_SIZE, KeyHint, Settings};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Appearance,
    Editor,
    Templates,
    Map,
    Keymap,
    About,
}

impl Section {
    const ALL: [Section; 6] = [
        Section::Appearance,
        Section::Editor,
        Section::Templates,
        Section::Map,
        Section::Keymap,
        Section::About,
    ];
    fn label(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Editor => "Editor",
            Section::Templates => "Templates",
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
        v.push(theta_ui::settings::DEFAULT_UI_FONT.into());
        v.push(theta_ui::settings::DEFAULT_BUFFER_FONT.into());
        v.retain(|n| !n.starts_with('.'));
        v.sort_by_key(|a| a.to_lowercase());
        v.dedup();
        v.into_iter().map(|s| &*Box::leak(s.into_boxed_str())).collect()
    })
}

pub struct SettingsView {
    section: Section,
    ui_font: Entity<SelectState<SearchableVec<&'static str>>>,
    buf_font: Entity<SelectState<SearchableVec<&'static str>>>,
    basemap: Entity<SelectState<Vec<String>>>,
    bm_name: Entity<InputState>,
    bm_url: Entity<InputState>,
    bm_attr: Entity<InputState>,
    bm_zoom: Entity<InputState>,
    bm_dark: bool,
    bm_error: Option<String>,
    filter: Entity<InputState>,
    /// (section, label, keys) rows of the application's actions.
    rows: Vec<(String, String, String)>,
    /// The editor key reference of the active keymap, re-read when the keymap changes.
    editor_rows: (KeymapMode, Vec<(String, String, String)>),
    _subs: Vec<Subscription>,
}

pub fn open(window: &mut Window, cx: &mut App) {
    open_at(Section::Appearance, window, cx)
}

/// Open Settings on `section` (the Vim `:help` command lands on the key reference).
pub fn open_at(section: Section, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| {
        let mut v = SettingsView::new(window, cx);
        v.section = section;
        v
    });
    window.open_dialog(cx, move |dialog, _, cx| {
        dialog.bg(cx.theme().popover).title("Settings").w(px(820.)).child(view.clone())
    });
}

/// The basemap dropdown: every basemap by name, the selected one preselected.
fn basemap_select(
    s: &theta_ui::Settings,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Entity<SelectState<Vec<String>>> {
    let all = theta_map_view::basemap::all(s);
    let current = theta_map_view::basemap::selected(s).id;
    let ix = all.iter().position(|b| b.id == current).map(IndexPath::new);
    let names: Vec<String> = all.into_iter().map(|b| b.name).collect();
    cx.new(|cx| SelectState::new(names, ix, window, cx))
}

fn font_select(
    current: &str,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Entity<SelectState<SearchableVec<&'static str>>> {
    let names = font_names(cx).to_vec();
    let ix = names.iter().position(|n| n.eq_ignore_ascii_case(current)).map(IndexPath::new);
    cx.new(|cx| SelectState::new(SearchableVec::new(names), ix, window, cx).searchable(true))
}

fn editor_reference(mode: KeymapMode) -> Vec<(String, String, String)> {
    keyref::editor_keys(mode)
        .into_iter()
        .map(|k| (format!("Editor · {}", k.section), k.action, k.keys))
        .collect()
}

impl SettingsView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let s = Settings::get(cx).clone();
        let ui_font = font_select(&s.ui_font_family, window, cx);
        let buf_font = font_select(&s.buffer_font_family, window, cx);
        let basemap = basemap_select(&s, window, cx);
        let bm_name = cx.new(|cx| InputState::new(window, cx).placeholder("Name, e.g. My tiles"));
        let bm_url =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://{s}.example.com/{z}/{x}/{y}.png"));
        let bm_attr =
            cx.new(|cx| InputState::new(window, cx).placeholder("\u{a9} the provider and OpenStreetMap"));
        let bm_zoom = cx.new(|cx| InputState::new(window, cx).default_value("19"));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search actions and keys"));
        let rows: Vec<(String, String, String)> = crate::palette::command_list(window);
        let editor_rows = (s.keymap, editor_reference(s.keymap));
        let subs = vec![
            cx.subscribe_in(&ui_font, window, |_, _, e: &SelectEvent<SearchableVec<&'static str>>, _, cx| {
                if let SelectEvent::Confirm(Some(name)) = e {
                    let n = name.to_string();
                    Settings::update(cx, |s| s.ui_font_family = n);
                }
            }),
            cx.subscribe_in(
                &buf_font,
                window,
                |_, _, e: &SelectEvent<SearchableVec<&'static str>>, _, cx| {
                    if let SelectEvent::Confirm(Some(name)) = e {
                        let n = name.to_string();
                        Settings::update(cx, |s| s.buffer_font_family = n);
                    }
                },
            ),
            cx.subscribe_in(&basemap, window, |_, _, e: &SelectEvent<Vec<String>>, _, cx| {
                if let SelectEvent::Confirm(Some(name)) = e {
                    let name = name.clone();
                    Settings::update(cx, |s| {
                        if let Some(b) = theta_map_view::basemap::all(s).into_iter().find(|b| b.name == name)
                        {
                            s.map_basemap = b.id;
                        }
                    });
                }
            }),
            cx.subscribe_in(&filter, window, |_, _, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.observe_global::<theta_ui::settings::SettingsStore>(|this, cx| {
                let mode = Settings::get(cx).keymap;
                if this.editor_rows.0 != mode {
                    this.editor_rows = (mode, editor_reference(mode));
                }
                cx.notify()
            }),
        ];
        Self {
            section: Section::Appearance,
            ui_font,
            buf_font,
            basemap,
            bm_name,
            bm_url,
            bm_attr,
            bm_zoom,
            bm_dark: false,
            bm_error: None,
            filter,
            rows,
            editor_rows,
            _subs: subs,
        }
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
                    // A long hint wraps instead of pushing the control out of the row.
                    .flex_1()
                    .min_w_0()
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
        let theme_name = theta_ui::themes::current_name(cx).to_string();
        v_flex()
            .child(self.row(
                "Theme",
                Some("Changes apply immediately"),
                Button::new("settings-theme").label(SharedString::from(format!("{theme_name}…"))).on_click(
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
            .child(self.row("Interface font", Some("The editor font is under Editor"), div().w(px(230.)).child(Select::new(&self.ui_font)), cx))
            .into_any_element()
    }

    fn editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let s = Settings::get(cx).clone();
        let clamp = |v: f32| v.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        let limit_pos = ROW_LIMITS.iter().position(|&n| n == s.row_limit).unwrap_or(2);
        let limit_label = if s.row_limit == 0 {
            "No limit".to_string()
        } else {
            crate::workspace::console::fmt_int(s.row_limit as u64)
        };
        let seg = |id: &'static str, mode: KeymapMode| {
            let b = Button::new(id).small().label(mode.label());
            let b = if s.keymap == mode { b.primary() } else { b };
            b.on_click(move |_, _, cx| Settings::update(cx, |s| s.keymap = mode))
        };
        v_flex()
            .child(
                self.row(
                    "Keymap",
                    Some("Helix and Vim are modal; Standard behaves like a conventional editor"),
                    h_flex()
                        .gap_1()
                        .child(seg("km-helix", KeymapMode::Helix))
                        .child(seg("km-vim", KeymapMode::Vim))
                        .child(seg("km-std", KeymapMode::Standard)),
                    cx,
                ),
            )
            .when(s.keymap == KeymapMode::Vim, |d| {
                d.child(self.row(
                    "Vim: share the clipboard",
                    Some("Yanks and deletes go to the system clipboard and p pastes from it (clipboard=unnamedplus)"),
                    Switch::new("vim-clipboard").checked(s.vim.clipboard_unnamed).on_change(|v, _, cx| {
                        let v = *v;
                        Settings::update(cx, |s| s.vim.clipboard_unnamed = v)
                    }),
                    cx,
                ))
            })
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
            .child(self.row(
                "Ask for query parameters",
                Some("Prompt for values when a statement contains ?, $1, :name or @name; they are substituted into the SQL as literals"),
                Switch::new("editor-parameters").checked(s.editor.parameters).on_change(|v, _, cx| {
                    let v = *v;
                    Settings::update(cx, |s| s.editor.parameters = v)
                }),
                cx,
            ))
            .child(self.row(
                "Row limit",
                Some("Results stop after this many rows; the grid says so and Load All fetches the rest"),
                self.stepper(
                    "row-limit",
                    limit_label,
                    move |s| s.row_limit = ROW_LIMITS[limit_pos.saturating_sub(1)],
                    move |s| s.row_limit = ROW_LIMITS[(limit_pos + 1).min(ROW_LIMITS.len() - 1)],
                ),
                cx,
            ))
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

    fn add_basemap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = |e: &Entity<InputState>, cx: &App| e.read(cx).value().trim().to_string();
        let (name, url, attribution) =
            (value(&self.bm_name, cx), value(&self.bm_url, cx), value(&self.bm_attr, cx));
        let max_zoom = value(&self.bm_zoom, cx).parse::<u8>().ok().filter(|z| (1..=22).contains(z));
        self.bm_error = if name.is_empty() {
            Some("Give the basemap a name.".into())
        } else if !theta_ui::settings::CustomBasemap::url_is_valid(&url) {
            Some("The URL must start with http(s):// and contain {z}, {x} and {y}.".into())
        } else if max_zoom.is_none() {
            Some("Max zoom is a number from 1 to 22.".into())
        } else {
            None
        };
        if self.bm_error.is_none() {
            let dark = self.bm_dark;
            Settings::update(cx, |s| {
                let mut id = format!("custom-{}", name.to_lowercase().replace(' ', "-"));
                while s.map_custom_basemaps.iter().any(|b| b.id == id) {
                    id.push('x');
                }
                s.map_basemap = id.clone();
                s.map_custom_basemaps.push(theta_ui::settings::CustomBasemap {
                    id,
                    name,
                    url,
                    attribution,
                    max_zoom: max_zoom.unwrap_or(19),
                    dark,
                });
            });
            for e in [&self.bm_name, &self.bm_url, &self.bm_attr] {
                e.update(cx, |i, cx| i.set_value("", window, cx));
            }
            self.refresh_basemaps(window, cx);
        }
        cx.notify();
    }

    /// Rebuild the dropdown after the list of basemaps changed.
    fn refresh_basemaps(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = Settings::get(cx).clone();
        self.basemap = basemap_select(&s, window, cx);
        cx.notify();
    }

    fn map(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = Settings::get(cx).clone();
        let current = theta_map_view::basemap::selected(&s);
        let note = |t: String| div().text_size(CHROME_TEXT_SIZE).text_color(theme.muted_foreground).child(t);
        v_flex()
            .gap_2()
            .child(self.row(
                "Basemap",
                Some("Shown under geographic results; the attribution stays on the map"),
                div().w(px(250.)).child(Select::new(&self.basemap)),
                cx,
            ))
            .child(note(format!(
                "Attribution: {}",
                match theta_map_view::basemap::attribution_text(&current) {
                    t if t.is_empty() => "none (no basemap)".to_string(),
                    t => t,
                }
            )))
            .child(div().pt_3().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child("Custom basemaps"))
            .child(note(
                "XYZ templates with {z}, {x}, {y}, and optionally {s} (a/b/c) and {r}. Respect the provider's \
                 usage policy; providers that need an API key take it in the URL."
                    .to_string(),
            ))
            .children(s.map_custom_basemaps.iter().map(|b| {
                let id = b.id.clone();
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        v_flex().flex_1().min_w_0().child(div().text_size(px(13.)).child(b.name.clone())).child(
                            div()
                                .text_size(CHROME_TEXT_SIZE)
                                .text_color(theme.muted_foreground)
                                .overflow_hidden()
                                .child(format!("{} \u{b7} max zoom {}{}", b.url, b.max_zoom, if b.dark { " \u{b7} dark" } else { "" })),
                        ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("bm-del-{id}"))).label("Remove").on_click(
                            cx.listener(move |this, _, window, cx| {
                                let id = id.clone();
                                Settings::update(cx, move |s| {
                                    s.map_custom_basemaps.retain(|b| b.id != id);
                                    if s.map_basemap == id {
                                        s.map_basemap = theta_map_view::basemap::DEFAULT_ID.into();
                                    }
                                });
                                this.refresh_basemaps(window, cx);
                            }),
                        ),
                    )
            }))
            .child(Input::new(&self.bm_name))
            .child(Input::new(&self.bm_url))
            .child(Input::new(&self.bm_attr))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().w(px(90.)).child(Input::new(&self.bm_zoom)))
                    .child(note("max zoom".into()))
                    .child(
                        Switch::new("bm-dark").label("Dark basemap").checked(self.bm_dark).on_change(
                            cx.listener(|this, v, _, cx| {
                                this.bm_dark = *v;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(div().flex_1())
                    .child(Button::new("bm-add").label("Add basemap").on_click(cx.listener(
                        |this, _, window, cx| this.add_basemap(window, cx),
                    ))),
            )
            .children(self.bm_error.clone().map(|e| div().text_size(CHROME_TEXT_SIZE).text_color(theme.danger).child(e)))
            .into_any_element()
    }

    fn keymap(&self, list_h: f32, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let q = self.filter.read(cx).value().to_lowercase();
        let rows: Vec<&(String, String, String)> = self
            .rows
            .iter()
            .chain(self.editor_rows.1.iter())
            .filter(|(s, a, k)| q.is_empty() || [s, a, k].iter().any(|t| t.to_lowercase().contains(&q)))
            .collect();
        let mode = Settings::get(cx).keymap;
        v_flex()
            .gap_2()
            .child(Input::new(&self.filter).small().cleanable(true))
            .child(div().text_size(CHROME_TEXT_SIZE).text_color(theme.muted_foreground).child(format!(
                "{} actions and {} editor keys ({} shown). Switch keymaps in Editor settings.",
                self.rows.len(),
                mode.label(),
                theta_core::text::plural(rows.len() as u64, "row", "rows")
            )))
            .child(div().id("keymap-rows").h(px(list_h)).overflow_y_scroll().child(v_flex().children(
                rows.into_iter().take(600).map(|(section, action, keys)| {
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
            )))
            .into_any_element()
    }

    /// The live templates: what each abbreviation expands to, with the built-ins and the user's side by side.
    fn templates(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mono = theta_ui::Settings::get(cx).buffer_font_family.clone();
        let store = crate::templates::TemplateStore::global();
        let user = store.user();
        let warnings = store.warnings();
        // The built-ins differ by dialect only for `crt` and `exp`; show PostgreSQL's and say so.
        let builtins = theta_sql::templates::builtins(theta_core::DatabaseKind::Postgres);
        let row = |abbr: String, desc: String, tag: &'static str, body: String| {
            h_flex()
                .gap_3()
                .items_start()
                .py_1p5()
                .border_b_1()
                .border_color(theme.border.opacity(0.5))
                .child(
                    div().w(px(84.)).flex_shrink_0().font_family(mono.clone()).text_size(px(13.)).child(abbr),
                )
                .child(
                    v_flex().flex_1().min_w_0().child(div().text_size(px(13.)).child(desc)).child(
                        div()
                            .font_family(mono.clone())
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(body.replace('\n', " ↵ "))),
                    ),
                )
                .child(div().flex_shrink_0().text_size(px(11.)).text_color(theme.muted_foreground).child(tag))
        };
        v_flex()
            .gap_2()
            .child(
                div().text_size(px(13.)).text_color(theme.muted_foreground).child(
                    "Type an abbreviation and press Tab (or accept it from the completion list) to expand it. Tab and Shift+Tab move between the stops; Escape leaves the template. Your own templates live in templates.json and replace a built-in of the same name.",
                ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("tpl-edit")
                            .label("Edit templates.json")
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::actions::EditLiveTemplates), cx)),
                    )
                    .child(Button::new("tpl-reload").label("Reload").on_click(cx.listener(|_, _, _, cx| {
                        crate::templates::TemplateStore::global().reload();
                        cx.notify();
                    }))),
            )
            .children(warnings.into_iter().map(|w| div().text_size(px(12.)).text_color(theme.danger).child(w)))
            .child(div().pt_2().font_weight(FontWeight::MEDIUM).text_size(px(13.)).child(format!("Your templates ({})", user.len())))
            .when(user.is_empty(), |d| {
                d.child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("None yet: “Edit templates.json” creates the file with an example."))
            })
            .children(user.iter().map(|t| {
                let tag = if t.dialects.is_empty() { "yours" } else { "yours · some databases" };
                row(t.abbreviation.clone(), t.description.clone(), tag, t.body.clone())
            }))
            .child(div().pt_2().font_weight(FontWeight::MEDIUM).text_size(px(13.)).child("Built in"))
            .children(builtins.into_iter().filter(|b| !user.iter().any(|u| u.abbreviation.eq_ignore_ascii_case(&b.abbreviation))).map(|t| {
                row(t.abbreviation, t.description, "built in", t.body)
            }))
            .child(
                div().pt_1().text_size(px(11.)).text_color(theme.muted_foreground).child(
                    "Variables: ${table} (asks which table), ${columns}, ${selection}, ${clipboard}, ${date}. Stops: $1, ${2:default}, $0; a repeated stop is edited together. crt and exp differ by database.",
                ),
            )
            .into_any_element()
    }

    fn about(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let line = |t: String| div().text_size(px(13.)).text_color(theme.muted_foreground).child(t);
        let heading = |t: &'static str| div().pt_3().font_weight(FontWeight::MEDIUM).text_size(px(13.)).child(t);
        let link = |id: &'static str, label: &'static str, url: &'static str| {
            Button::new(id).small().label(label).on_click(move |_, _, cx| cx.open_url(url))
        };
        let folder = |id: &'static str, caption: &'static str, path: std::path::PathBuf| {
            let shown = path.display().to_string();
            h_flex()
                .gap_3()
                .items_center()
                .child(div().w(px(90.)).flex_shrink_0().text_size(px(13.)).child(caption))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(shown),
                )
                .child(Button::new(id).small().label("Open").on_click(move |_, _, cx| {
                    // The folder may not exist yet on a first run; opening it then would fail silently.
                    let _ = std::fs::create_dir_all(&path);
                    cx.open_with_system(&path);
                }))
        };
        let config = theta_ui::settings::config_dir();
        v_flex()
            .gap_2()
            .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child("Theta"))
            .child(line(format!("Version {}", crate::links::version_line())))
            .child(line("A keyboard-first database IDE for PostgreSQL, MySQL, SQLite and more.".into()))
            .child(line(
                "The name comes from the theta join (\u{3b8}-join) of relational algebra, the general form of JOIN."
                    .into(),
            ))
            .child(line("Released under the MIT licence. Copyright (c) 2026 Jesse Williamson".into()))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .pt_1()
                    .child(link("about-web", "Website", crate::links::WEBSITE))
                    .child(link("about-guide", "User Guide", crate::links::USER_GUIDE))
                    .child(link("about-keys", "Key Reference", crate::links::KEY_REFERENCE))
                    .child(link("about-changes", "Changelog", crate::links::CHANGELOG))
                    .child(link("about-issues", "Issue Tracker", crate::links::ISSUES))
                    .child(link("about-source", "Source Code", crate::links::REPOSITORY)),
            )
            .child(heading("Files"))
            .child(folder("about-config", "Settings", config.clone()))
            .child(folder("about-logs", "Logs", config.join("logs")))
            .child(heading("Third-party notices"))
            .child(line("Fonts: IBM Plex Sans and Lilex, both under the SIL Open Font License 1.1.".into()))
            .child(line(
                "Themes and UI components: gpui-kit (Apache-2.0); each theme credits its author in NOTICE."
                    .into(),
            ))
            .child(line(
                "Map tiles: attribution is shown on the map for the selected basemap (OpenStreetMap by default).".into(),
            ))
            .child(
                h_flex().pt_1().child(link(
                    "about-third-party",
                    "Licences of every bundled library",
                    crate::links::THIRD_PARTY,
                )),
            )
            .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // The dialog takes what the window can spare, within reason, so a small window scrolls the page
        // instead of clipping the dialog and a large one does not float a postage stamp.
        let height = (f32::from(window.viewport_size().height) - 190.).clamp(300., 560.);
        let body = match self.section {
            Section::Appearance => self.appearance(cx),
            Section::Editor => self.editor(cx),
            Section::Templates => self.templates(cx),
            Section::Map => self.map(cx),
            Section::Keymap => self.keymap(height - 96., cx),
            Section::About => self.about(cx),
        };
        h_flex()
            .gap_4()
            .items_start()
            .h(px(height))
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
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .px_1()
                    .pb_1()
                    .child(body),
            )
    }
}
