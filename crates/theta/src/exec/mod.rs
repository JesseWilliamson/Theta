//! Pure (GPUI-free) logic behind running queries: which statements to run,
//! whether a result is editable, how staged edits become SQL, which result
//! columns are geometry, and the on-disk history / console stores.

pub mod changes;
pub mod consoles;
pub mod editable;
pub mod fk_ui;
pub mod geo;
pub mod history;
pub mod intel;
pub mod keyref;
pub mod local_history;
pub mod multi;
pub mod params;
pub mod pick;
