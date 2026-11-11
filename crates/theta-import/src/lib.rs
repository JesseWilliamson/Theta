//! # theta-import
//!
//! Getting data into and out of tables in bulk: a streaming delimited-text parser, format sniffing, type
//! inference, conversion to typed values, and the import and export pipelines that drive a
//! [`theta_core::Connection`]. Pure Rust, no UI; the application only shows progress and asks questions.

pub mod convert;
pub mod decode;
pub mod infer;
pub mod parser;
pub mod plan;
pub mod scalar;
pub mod sniff;

pub use convert::{ConvertError, ConvertOptions, Target, convert};
pub use decode::Encoding;
pub use infer::{ColumnType, TypeInferer};
pub use parser::{CsvOptions, CsvReader, Field, ParseError, Record};
pub use sniff::{Sniffed, sniff};
