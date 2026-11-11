//! First-run view: shown in the console area while no connection exists.

use super::Workspace;
use crate::actions::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{Context, FontWeight, IntoElement, ParentElement as _, Styled as _, div, px};
use quarry_ui::settings::KeymapMode;
use quarry_ui::{KeyHint, Settings};

/// (label, key) pairs for the current keymap, in the order shown.
pub fn key_list(keymap: KeymapMode) -> Vec<(&'static str, &'static str)> {
    let m = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let mut v = vec![
        ("Run statement", leak(format!("{m}-enter"))),
        ("Run all statements", leak(format!("{m}-shift-enter"))),
        ("Command palette", leak(format!("{m}-shift-p"))),
        ("Go to table", leak(format!("{m}-p"))),
        ("Explain", leak(format!("{m}-e"))),
    ];
    if keymap.is_modal() {
        v.push(("Insert mode / back to normal", "i  esc"));
        v.push(("Command menu (which-key)", "space"));
        v.push(("Save console to a file", ":w"));
    } else {
        v.push(("Save console to a file", leak(format!("{m}-s"))));
    }
    v.push(("Settings", leak(format!("{m}-,"))));
    v
}

impl Workspace {
    pub(super) fn render_welcome(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let keymap = Settings::get(cx).keymap;
        let keys = key_list(keymap);
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_5()
            .child(
                v_flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::default().path(quarry_ui::MARK_ICON).size(px(44.)).text_color(theme.foreground))
                    .child(div().text_size(px(28.)).font_weight(FontWeight::SEMIBOLD).child("Quarry"))
                    .child(
                        div().text_size(px(14.)).text_color(theme.muted_foreground).child(
                            "A keyboard-first database IDE for PostgreSQL, MySQL, MariaDB and SQLite.",
                        ),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        Button::new("welcome-add")
                            .primary()
                            .icon(Icon::new(IconName::Plus))
                            .label("Add Connection")
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(NewConnection), cx)),
                    )
                    .child(
                        Button::new("welcome-demo")
                            .icon(Icon::new(IconName::Database))
                            .label("Try the Demo Database")
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(WelcomeDemo), cx)),
                    ),
            )
            .child(
                v_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("How do you like to edit? Change it any time in Settings."),
                    )
                    .child(h_flex().gap_2().children(KeymapMode::ALL.into_iter().map(|mode| {
                        let b = Button::new(match mode {
                            KeymapMode::Helix => "welcome-km-helix",
                            KeymapMode::Vim => "welcome-km-vim",
                            KeymapMode::Standard => "welcome-km-std",
                        })
                        .small()
                        .label(mode.label());
                        let b = if keymap == mode { b.primary() } else { b };
                        b.on_click(move |_, _, cx| Settings::update(cx, |s| s.keymap = mode))
                    }))),
            )
            .child(
                v_flex()
                    .w(px(340.))
                    .gap_1p5()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .children(keys.into_iter().map(|(label, key)| {
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(div().text_size(px(12.5)).text_color(theme.muted_foreground).child(label))
                            .child(KeyHint::new(key))
                    })),
            )
            .into_any_element()
    }
}
