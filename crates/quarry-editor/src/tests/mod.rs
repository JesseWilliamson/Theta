//! Behaviour tests. The DSL mirrors Helix's own test style:
//! `#[h|]#ello` is the primary selection (anchor..head, `|` marks the head),
//! `#(w|)#orld` a secondary selection, `#[|]#` an empty selection (end of text only).
//! Key strings use `<ret> <esc> <C-d> <A-;> <space> <lt>` for special keys.

mod cases;
mod core;
mod fuzz;
mod harness;
mod helix_edit;
mod helix_insert;
mod helix_motion;
mod helix_objects;
mod helix_prompt;
mod perf;
mod standard;
mod syntax;
mod vim_dot_cases;
mod vim_dsl;
mod vim_edge_cases;
mod vim_ex_cases;
mod vim_insert_cases;
mod vim_macro_mark_cases;
mod vim_motion_cases;
mod vim_object_cases;
mod vim_operator_cases;
mod vim_oracle;
mod vim_register_cases;
mod vim_search_cases;
mod vim_visual_cases;

pub(crate) use harness::*;
pub(crate) use vim_dsl::*;
