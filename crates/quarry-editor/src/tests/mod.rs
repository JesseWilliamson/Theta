//! Behaviour tests. The DSL mirrors Helix's own test style:
//! `#[h|]#ello` is the primary selection (anchor..head, `|` marks the head),
//! `#(w|)#orld` a secondary selection, `#[|]#` an empty selection (end of text only).
//! Key strings use `<ret> <esc> <C-d> <A-;> <space> <lt>` for special keys.

mod cases;
mod core;
mod fuzz;
mod harness;
mod perf;
mod helix_edit;
mod helix_insert;
mod helix_motion;
mod helix_objects;
mod helix_prompt;
mod standard;
mod syntax;

pub(crate) use harness::*;
