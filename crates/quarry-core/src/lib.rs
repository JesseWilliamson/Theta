//! # quarry-core
//!
//! The vocabulary every other Quarry crate speaks. This crate deliberately has
//! no UI code and no database client libraries: it only defines *shapes*
//! (values, result sets, schema metadata, query plans, connection settings) and
//! the [`Driver`]/[`Connection`] traits that concrete drivers implement.
//!
//! Dependency direction (arrows point at dependencies):
//!
//! ```text
//! quarry (GPUI app) ──► quarry-drivers ──► quarry-core
//!        │──► quarry-sql ───────────────► quarry-core
//!        │──► quarry-geo
//!        └──► quarry-editor
//! ```
//!
//! Keeping the contracts here means the UI can be developed and tested against
//! a fake [`Connection`], and drivers can be tested with no window at all.

pub mod config;
pub mod driver;
pub mod error;
pub mod plan;
pub mod schema;
pub mod value;

pub use config::*;
pub use driver::*;
pub use error::*;
pub use plan::*;
pub use schema::*;
pub use value::*;
