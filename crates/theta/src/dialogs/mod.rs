pub mod compare_pick;
pub mod confirm;
pub mod connection;
pub mod copy_table;
pub mod dump;
pub mod export;
pub mod import;
pub mod local_history;
pub mod multi_run;
pub mod name;
pub mod params;
pub mod script;
pub mod search;
pub mod settings;

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::v_flex;
use gpui_kit::{IntoElement, ParentElement as _, Styled as _, Window, div, px};

/// The one layout every dialog follows: the title is the dialog's own header, the `body` scrolls when the
/// window is short, and the `footer` with the buttons stays in view. `reserve` is the height (in pixels) of
/// everything around the body, the footer included, so the body never makes the dialog taller than the
/// window minus its margins.
pub fn framed(
    window: &Window,
    body: impl gpui_kit::IntoElement,
    footer: impl gpui_kit::IntoElement,
    reserve: f32,
) -> impl IntoElement {
    let max_h = (f32::from(window.viewport_size().height) - reserve).max(160.);
    v_flex()
        .gap_3()
        .child(div().max_h(px(max_h)).overflow_y_scrollbar().child(body))
        .child(footer)
}
