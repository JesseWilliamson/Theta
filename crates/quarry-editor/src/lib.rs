//! `quarry-editor`: the pure text-editing engine behind Quarry's SQL consoles.
//!
//! A rope buffer, multiple selections, undo history and two keymaps (Helix and Standard).
//! No GPUI, no I/O, no async: the host feeds it `KeyInput`s and reacts to `Effect`s.

mod edit;
mod editor;
mod helix;
mod history;
mod keys;
mod motion;
mod objects;
mod prompt;
mod selection;
mod standard;
mod syntax;
mod text;
mod transaction;

#[cfg(test)]
mod tests;

pub use editor::{Editor, Effect, KeyHint, KeyOutcome, Keymap, Mode, Prompt, ScrollRequest};
pub use keys::{Key, KeyInput, Modifiers};
pub use selection::Selection;
pub use syntax::{HighlightKind, HighlightSpan};
