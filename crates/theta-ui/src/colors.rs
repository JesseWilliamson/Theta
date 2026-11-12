//! Colour accessors for custom-painted views (SQL editor, plan tree, map).
//!
//! Those views paint with raw `Hsla` values instead of components, so they need
//! the theme's *editor* palette. Both structs prefer what the active theme's
//! `highlight` section says and derive a sensible value from the UI palette
//! when a theme (often an imported or third-party one) leaves it out.
//!
//! # The surface rule
//!
//! Data surfaces (the SQL editor and its gutter, the results grid rows, the plan tree, the messages
//! and history lists, the map panel, the active tab) all use one colour, [`data_surface`]: the
//! theme's editor background, or its window background when it names no separate one. Chrome
//! (explorer, tab strip, panel tab strips, toolbars, column headers) uses the panel colours, and
//! the title and status bars sit one step above those. In Theta Dark that makes the text area
//! the darkest, calmest surface, so the code and the data recede into the page rather than the
//! frame around them. A theme that defines no panel colours falls back to the same colour for
//! chrome and data, which is how every bundled third-party theme looked before this rule existed.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Hsla};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyntaxColors {
    pub keyword: Hsla,
    pub type_: Hsla,
    pub function: Hsla,
    pub string: Hsla,
    pub number: Hsla,
    pub comment: Hsla,
    pub operator: Hsla,
    pub punctuation: Hsla,
    pub identifier: Hsla,
    pub constant: Hsla,
    pub parameter: Hsla,
    pub error: Hsla,
    /// Whether the theme styles comments in italics.
    pub comment_italic: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorColors {
    pub background: Hsla,
    pub foreground: Hsla,
    pub cursor: Hsla,
    pub selection: Hsla,
    pub active_line: Hsla,
    pub line_number: Hsla,
    pub active_line_number: Hsla,
    pub gutter_background: Hsla,
    pub search_match: Hsla,
    pub matching_bracket: Hsla,
    /// Other occurrences of the name under the cursor.
    pub occurrence: Hsla,
    pub indent_guide: Hsla,
}

/// The colour of every surface that shows data rather than controls; see the module docs.
pub fn data_surface(cx: &App) -> Hsla {
    let theme = cx.theme();
    theme.highlight_theme.style.editor_background.unwrap_or(theme.background)
}

impl SyntaxColors {
    pub fn get(cx: &App) -> Self {
        let theme = cx.theme();
        let syntax = &theme.highlight_theme.style.syntax;
        let pick = |names: &[&str], fallback: Hsla| -> Hsla {
            names.iter().find_map(|n| syntax.style(n).and_then(|s| s.color)).unwrap_or(fallback)
        };
        let comment_style = syntax.style("comment");
        SyntaxColors {
            keyword: pick(&["keyword"], theme.magenta),
            type_: pick(&["type", "enum"], theme.cyan),
            function: pick(&["function"], theme.blue),
            string: pick(&["string"], theme.green),
            number: pick(&["number"], theme.yellow),
            comment: pick(&["comment"], theme.muted_foreground),
            operator: pick(&["operator"], theme.cyan),
            punctuation: pick(&["punctuation", "punctuation.delimiter"], theme.muted_foreground),
            identifier: pick(&["variable", "property", "primary"], theme.foreground),
            constant: pick(&["constant", "boolean"], theme.yellow),
            parameter: pick(&["property", "variable.special", "label"], theme.red),
            error: theme.highlight_theme.style.status.error(cx),
            comment_italic: comment_style
                .map(|s| s.font_style == Some(gpui_kit::FontStyle::Italic))
                .unwrap_or(true),
        }
    }
}

impl EditorColors {
    pub fn get(cx: &App) -> Self {
        let theme = cx.theme();
        let style = &theme.highlight_theme.style;
        let background = data_surface(cx);
        let foreground = style.editor_foreground.unwrap_or(theme.foreground);
        let accent = theme.ring;
        EditorColors {
            background,
            foreground,
            cursor: theme.caret,
            selection: theme.selection,
            active_line: style.editor_active_line.unwrap_or_else(|| background.blend(foreground.alpha(0.06))),
            line_number: style.editor_line_number.unwrap_or(theme.muted_foreground.opacity(0.7)),
            active_line_number: style.editor_active_line_number.unwrap_or(foreground),
            gutter_background: style.editor_gutter_background.unwrap_or(background),
            search_match: style.status.warning(cx).alpha(0.35),
            matching_bracket: accent.alpha(0.3),
            occurrence: accent.alpha(0.16),
            indent_guide: theme.border.alpha(0.6),
        }
    }
}
