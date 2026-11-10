//! # quarry-editor-view
//!
//! The GPUI text-editor widget of Quarry: paints a `quarry_editor::Editor` (gutter, syntax
//! colours, multiple selections, block/bar cursors, diagnostics, completion popup, which-key
//! box, prompt bar) and feeds it keyboard, mouse and IME input.
//!
//! The pure parts (key translation, scroll maths, selection geometry, completion state,
//! display-line mapping) live in GPUI-free modules with unit tests; `view` is the controller,
//! `element` the painter, `render` the chrome around it.

mod actions;
pub mod completion;
mod display;
mod element;
mod geometry;
mod input;
mod keys;
mod lines;
mod popup;
mod render;
mod selection_geom;
mod status;
mod util;
mod view;

pub use actions::{KEY_CONTEXT, init};
pub use status::{EditorStatus, StatementStatus};
pub use view::{CompletionProvider, DiagnosticsProvider, EditorEvent, EditorView, FrameStats};

#[cfg(test)]
mod tests;
