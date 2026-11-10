//! Small shared drawing helpers: palette lookups and the chip primitive.

use crate::format::ramp_segment;
use crate::model::Family;
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, FontWeight, Hsla, IntoElement, ParentElement as _, Rgba, SharedString, Styled as _, div, px,
};
use quarry_sql::Severity;

pub fn family_color(f: Family, cx: &App) -> Hsla {
    let t = cx.theme();
    match f {
        Family::Scan => t.yellow,
        Family::Index => t.green,
        Family::Join => t.blue,
        Family::Sort => t.magenta,
        Family::Aggregate => t.cyan,
        Family::Other => t.muted_foreground,
    }
}

pub fn family_icon(f: Family) -> IconName {
    match f {
        Family::Scan => IconName::Rows3,
        Family::Index => IconName::ScanSearch,
        Family::Join => IconName::Merge,
        Family::Sort => IconName::ArrowDownUp,
        Family::Aggregate => IconName::Sigma,
        Family::Other => IconName::Dot,
    }
}

pub fn severity_color(s: Severity, cx: &App) -> Hsla {
    let t = cx.theme();
    match s {
        Severity::Critical => t.danger,
        Severity::Warning => t.warning,
        Severity::Info => t.info,
    }
}

pub fn severity_icon(s: Severity) -> IconName {
    match s {
        Severity::Critical => IconName::CircleAlert,
        Severity::Warning => IconName::TriangleAlert,
        Severity::Info => IconName::Info,
    }
}

fn lerp(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b): (Rgba, Rgba) = (a.into(), b.into());
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
    .into()
}

/// Cool-to-hot ramp (blue, green, yellow, red) taken from the theme so it follows
/// light and dark palettes. `level` comes from [`crate::format::heat_level`].
pub fn heat_color(level: f32, cx: &App) -> Hsla {
    let t = cx.theme();
    let stops = [t.blue, t.green, t.yellow, t.red];
    let (lo, f) = ramp_segment(level, stops.len());
    lerp(stops[lo], stops[lo + 1], f)
}

/// A compact rounded label: optional leading icon, then text.
pub fn chip(icon: Option<(IconName, Hsla)>, text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    let t = cx.theme();
    h_flex()
        .h(px(20.))
        .px(px(6.))
        .gap(px(4.))
        .items_center()
        .flex_shrink_0()
        .rounded(px(4.))
        .bg(t.muted.opacity(0.7))
        .text_size(px(11.))
        .text_color(t.muted_foreground)
        .when_some(icon, |this, (i, c)| this.child(Icon::new(i).size(px(12.)).text_color(c)))
        .child(div().font_weight(FontWeight::MEDIUM).child(text.into()))
}
