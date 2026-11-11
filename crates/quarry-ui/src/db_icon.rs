//! Database-kind glyphs. quarry-ui must not depend on quarry-core, so it has
//! its own tiny enum that the app maps `DatabaseKind` onto.

use gpui_kit::component::Icon;
use gpui_kit::{App, Hsla, IntoElement, Pixels, RenderOnce, Styled as _, Window, hsla, px};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DbIcon {
    Postgres,
    MySql,
    Sqlite,
    /// A kind without a glyph of its own.
    Generic,
}

impl DbIcon {
    /// The glyph for a descriptor's `icon` key (`quarry_core::KindInfo::icon`).
    pub fn from_key(key: &str) -> DbIcon {
        match key {
            "postgres" => DbIcon::Postgres,
            "mysql" => DbIcon::MySql,
            "sqlite" => DbIcon::Sqlite,
            _ => DbIcon::Generic,
        }
    }

    pub fn path(self) -> &'static str {
        match self {
            DbIcon::Postgres => "icons/db-postgres.svg",
            DbIcon::MySql => "icons/db-mysql.svg",
            DbIcon::Sqlite => "icons/db-sqlite.svg",
            DbIcon::Generic => "icons/db-generic.svg",
        }
    }

    /// Recognisable brand-ish hue per engine; the same in light and dark
    /// themes (saturation/lightness chosen to hold contrast on both).
    pub fn color(self) -> Hsla {
        match self {
            DbIcon::Postgres => hsla(212. / 360., 0.55, 0.58, 1.),
            DbIcon::MySql => hsla(32. / 360., 0.80, 0.52, 1.),
            DbIcon::Sqlite => hsla(172. / 360., 0.45, 0.46, 1.),
            DbIcon::Generic => hsla(220. / 360., 0.10, 0.55, 1.),
        }
    }
}

#[derive(IntoElement)]
pub struct DbKindIcon {
    kind: DbIcon,
    size: Pixels,
    color: Option<Hsla>,
}

impl DbKindIcon {
    pub fn new(kind: DbIcon) -> Self {
        Self { kind, size: px(14.), color: None }
    }
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }
    /// Override the brand colour (e.g. to dim the icon of a disconnected entry).
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for DbKindIcon {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        Icon::default()
            .path(self.kind.path())
            .size(self.size)
            .text_color(self.color.unwrap_or_else(|| self.kind.color()))
    }
}
