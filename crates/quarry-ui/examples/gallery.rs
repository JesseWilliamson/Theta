//! Component gallery: every quarry-ui primitive in the active theme.
//!
//!     cargo run -p quarry-ui --example gallery -- --theme "Tokyo Night" [--picker]
//!
//! `QUARRY_WINDOW_SIZE=WxH` sets the window size, like the app.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, WindowBounds, WindowOptions, div, px, size,
};
use quarry_ui::picker::{Picker, PickerDelegate, PickerEvent, PickerItem};
use quarry_ui::{
    DbIcon, DbKindIcon, EditorColors, EmptyState, KeyHint, PanelHeader, Settings, SyntaxColors, tool_button,
};

struct Demo;

impl PickerDelegate for Demo {
    fn placeholder(&self) -> SharedString {
        "Type a command or search…".into()
    }
    fn items(&mut self, _: &mut App) -> Vec<PickerItem> {
        let cmds: [(&str, &str, &str); 10] = [
            ("New Console", "ctrl-n", "File"),
            ("New Connection…", "ctrl-shift-n", "File"),
            ("Connect…", "ctrl-alt-c", "Connection"),
            ("Disconnect", "", "Connection"),
            ("Refresh Schema", "f5", "Connection"),
            ("Toggle Explorer", "ctrl-b", "View"),
            ("Toggle Bottom Panel", "ctrl-j", "View"),
            ("Select Theme…", "ctrl-k ctrl-t", "View"),
            ("Run Statement", "ctrl-enter", "Query"),
            ("Format Document", "ctrl-shift-f", "Query"),
        ];
        cmds.iter()
            .map(|(label, key, section)| {
                let mut item = PickerItem::new(*label, *label).section(*section).icon(IconName::Terminal);
                if !key.is_empty() {
                    item = item.key_hint(quarry_ui::format_keystrokes(key, quarry_ui::is_mac()));
                }
                item
            })
            .collect()
    }
    fn confirm(&mut self, _: &PickerItem, _: &mut Window, _: &mut App) {}
}

struct Gallery {
    picker: Option<Entity<Picker<Demo>>>,
    _subs: Vec<Subscription>,
}

impl Gallery {
    fn open_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picker = cx.new(|cx| Picker::new(Demo, window, cx));
        let sub = cx.subscribe(&picker, |this, _, _: &PickerEvent, cx| {
            this.picker = None;
            cx.notify();
        });
        self._subs.push(sub);
        self.picker = Some(picker);
        cx.notify();
    }
}

fn swatch(name: &'static str, color: Hsla, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .gap_1()
        .child(div().w(px(56.)).h(px(28.)).rounded(px(4.)).border_1().border_color(theme.border).bg(color))
        .child(div().text_size(px(11.)).text_color(theme.muted_foreground).child(name))
}

fn card(title: &'static str, cx: &App) -> gpui_kit::Div {
    let theme = cx.theme();
    v_flex().gap_3().p_4().rounded(px(8.)).border_1().border_color(theme.border).bg(theme.sidebar).child(
        div()
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.muted_foreground)
            .child(title),
    )
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let syn = SyntaxColors::get(cx);
        let ed = EditorColors::get(cx);
        let settings = Settings::get(cx).clone();
        let mono = theme.mono_font_family.clone();

        let code_line = |parts: Vec<(&'static str, Hsla)>| {
            h_flex().children(parts.into_iter().map(|(t, c)| div().text_color(c).child(t)))
        };

        let page = v_flex()
            .size_full()
            .id("gallery-scroll")
            .overflow_y_scroll()
            .p_4()
            .gap_4()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        v_flex()
                            .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child("Quarry UI gallery"))
                            .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child(format!(
                                "{} · {} {}px · {} {}px",
                                theme.theme_name(),
                                settings.ui_font_family,
                                settings.ui_font_size,
                                settings.buffer_font_family,
                                settings.buffer_font_size
                            ))),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Button::new("toggle-theme").label("Toggle light/dark").on_click(|_, _, cx| {
                                let next = if gpui_kit::component::Theme::global(cx).is_dark() {
                                    quarry_ui::themes::QUARRY_LIGHT
                                } else {
                                    quarry_ui::themes::QUARRY_DARK
                                };
                                quarry_ui::themes::set_theme(next, cx);
                            }))
                            .child(
                                Button::new("open-picker")
                                    .label("Open picker")
                                    .on_click(cx.listener(|this, _, window, cx| this.open_picker(window, cx))),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .gap_4()
                    .items_start()
                    .child(
                        card("TYPOGRAPHY", cx)
                            .flex_1()
                            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child("IBM Plex Sans SemiBold — Quarry"))
                            .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child("IBM Plex Sans Medium — SELECT * FROM users"))
                            .child(div().text_size(px(13.)).child("IBM Plex Sans Regular — the quick brown fox jumps over 0123456789"))
                            .child(div().text_size(px(13.)).italic().child("IBM Plex Sans Italic — the quick brown fox"))
                            .child(div().font_family(mono.clone()).text_size(px(13.)).child("Lilex Regular — SELECT id, name FROM users; -> => != <= >="))
                            .child(div().font_family(mono.clone()).text_size(px(13.)).font_weight(FontWeight::BOLD).child("Lilex Bold — WHERE created_at > now()"))
                            .child(div().font_family(mono.clone()).text_size(px(13.)).italic().child("Lilex Italic — -- a comment")),
                    )
                    .child(
                        card("SQL PALETTE", cx).flex_1().child(
                            v_flex()
                                .p_3()
                                .gap_0p5()
                                .rounded(px(6.))
                                .bg(ed.background)
                                .font_family(mono.clone())
                                .text_size(px(13.))
                                .child(code_line(vec![("-- Top customers", syn.comment)]))
                                .child(code_line(vec![
                                    ("SELECT ", syn.keyword),
                                    ("c", syn.identifier),
                                    (".", syn.punctuation),
                                    ("name", syn.identifier),
                                    (", ", syn.punctuation),
                                    ("count", syn.function),
                                    ("(", syn.punctuation),
                                    ("*", syn.operator),
                                    (")", syn.punctuation),
                                    (" AS ", syn.keyword),
                                    ("orders", syn.identifier),
                                ]))
                                .child(code_line(vec![
                                    ("FROM ", syn.keyword),
                                    ("customers ", syn.identifier),
                                    ("c", syn.identifier),
                                ]))
                                .child(
                                    code_line(vec![
                                        ("WHERE ", syn.keyword),
                                        ("c", syn.identifier),
                                        (".", syn.punctuation),
                                        ("country", syn.identifier),
                                        (" = ", syn.operator),
                                        ("'DE'", syn.string),
                                        (" AND ", syn.keyword),
                                        ("total", syn.identifier),
                                        (" > ", syn.operator),
                                        ("100", syn.number),
                                    ])
                                    .bg(ed.active_line),
                                )
                                .child(code_line(vec![
                                    ("CAST", syn.function),
                                    ("(x ", syn.punctuation),
                                    ("AS ", syn.keyword),
                                    ("numeric", syn.type_),
                                    (") ", syn.punctuation),
                                    ("NULL ", syn.constant),
                                    ("oops", syn.error),
                                ])),
                        ),
                    ),
            )
            .child(
                card("PALETTE", cx).child(
                    h_flex()
                        .gap_3()
                        .flex_wrap()
                        .child(swatch("background", theme.background, cx))
                        .child(swatch("sidebar", theme.sidebar, cx))
                        .child(swatch("title_bar", theme.title_bar, cx))
                        .child(swatch("tab_bar", theme.tab_bar, cx))
                        .child(swatch("border", theme.border, cx))
                        .child(swatch("foreground", theme.foreground, cx))
                        .child(swatch("muted fg", theme.muted_foreground, cx))
                        .child(swatch("accent/ring", theme.ring, cx))
                        .child(swatch("hover", theme.list_hover, cx))
                        .child(swatch("selected", theme.list_active, cx))
                        .child(swatch("selection", theme.selection, cx))
                        .child(swatch("danger", theme.danger, cx))
                        .child(swatch("warning", theme.warning, cx))
                        .child(swatch("success", theme.success, cx))
                        .child(swatch("info", theme.info, cx)),
                ),
            )
            .child(
                h_flex()
                    .gap_4()
                    .items_start()
                    .child(
                        card("BUTTONS", cx)
                            .flex_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(Button::new("b1").label("Default"))
                                    .child(Button::new("b2").primary().label("Primary"))
                                    .child(Button::new("b3").ghost().label("Ghost"))
                                    .child(Button::new("b4").danger().label("Danger"))
                                    .child(Button::new("b5").label("Disabled").disabled(true)),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(Button::new("b6").small().icon(IconName::Play).label("Run"))
                                    .child(Button::new("b7").small().primary().loading(true).label("Connecting"))
                                    .child(Button::new("b8").small().selected(true).label("Selected")),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(tool_button("t1", IconName::PanelLeft, "Toggle Explorer"))
                                    .child(tool_button("t2", IconName::PanelBottom, "Toggle Bottom Panel"))
                                    .child(tool_button("t3", IconName::RefreshCw, "Refresh"))
                                    .child(tool_button("t4", IconName::Settings, "Settings"))
                                    .child(tool_button("t5", IconName::Plus, "Add").disabled(true)),
                            ),
                    )
                    .child(
                        card("KEY HINTS & DB ICONS", cx)
                            .flex_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(KeyHint::new("ctrl-enter"))
                                    .child(KeyHint::new("ctrl-shift-p"))
                                    .child(KeyHint::new("f1"))
                                    .child(KeyHint::from_label("⌃↵"))
                                    .child(KeyHint::from_label("⇧⌘P")),
                            )
                            .child(
                                h_flex()
                                    .gap_4()
                                    .items_center()
                                    .child(h_flex().gap_2().items_center().child(DbKindIcon::new(DbIcon::Postgres).size(px(16.))).child("PostgreSQL"))
                                    .child(h_flex().gap_2().items_center().child(DbKindIcon::new(DbIcon::MySql).size(px(16.))).child("MySQL"))
                                    .child(h_flex().gap_2().items_center().child(DbKindIcon::new(DbIcon::Sqlite).size(px(16.))).child("SQLite")),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .gap_4()
                    .items_start()
                    .child(
                        v_flex()
                            .flex_1()
                            .h(px(220.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.sidebar)
                            .child(
                                PanelHeader::new("Explorer")
                                    .action(tool_button("h1", IconName::Plus, "Add Connection"))
                                    .action(tool_button("h2", IconName::RefreshCw, "Refresh")),
                            )
                            .child(div().h(px(1.)).bg(theme.border))
                            .child(
                                EmptyState::new(IconName::Database, "No connections yet")
                                    .description("Add a database connection to browse its schemas and tables.")
                                    .action(Button::new("e1").primary().small().label("Add Connection")),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .h(px(220.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.background)
                            .child(
                                EmptyState::new(IconName::Play, "Run a query to see results")
                                    .description("Results appear here. Run the statement under the cursor with the shortcut below.")
                                    .key_hint(KeyHint::new("ctrl-enter")),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .h(px(220.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.background)
                            .child(
                                EmptyState::new(IconName::CircleAlert, "Connection failed")
                                    .error()
                                    .description("password authentication failed for user \"postgres\"")
                                    .action(Button::new("e2").small().label("Retry")),
                            ),
                    ),
            );

        div().size_full().relative().child(page).when_some(self.picker.clone(), |this, p| this.child(p))
    }
}

fn main() {
    let mut theme_arg: Option<String> = None;
    let mut open_picker = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--theme" => theme_arg = args.next(),
            "--picker" => open_picker = true,
            _ => {}
        }
    }
    quarry_ui::application().run(move |cx| {
        quarry_ui::init(cx);
        if let Some(name) = theme_arg.clone() {
            quarry_ui::themes::set_theme(&name, cx);
        }
        let (w, h) = quarry_ui::window_size_from_env().unwrap_or((1100., 820.));
        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        let (_, view) = gpui_kit::open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            cx,
            |_, cx| cx.new(|_| Gallery { picker: None, _subs: vec![] }),
        )
        .expect("open window");
        if open_picker {
            cx.defer(move |cx| {
                let window = cx.windows().into_iter().next();
                if let Some(window) = window {
                    window
                        .update(cx, |_, window, cx| view.update(cx, |g, cx| g.open_picker(window, cx)))
                        .ok();
                }
            });
        }
    });
}
