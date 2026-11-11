//! # theta-import
//!
//! Getting data into and out of tables in bulk: a streaming delimited-text parser, format sniffing, type
//! inference, conversion to typed values, and the import and export pipelines that drive a
//! [`theta_core::Connection`]. Pure Rust, no UI; the application only shows progress and asks questions.

pub mod convert;
pub mod decode;
pub mod dump;
pub mod export;
pub mod infer;
pub mod job;
pub mod parser;
pub mod plan;
pub mod run;
pub mod scalar;
pub mod scan;
pub mod script;
pub mod setup;
pub mod sniff;

pub use convert::{ConvertError, ConvertOptions, Target, convert};
pub use decode::Encoding;
pub use dump::{DumpOptions, DumpOutcome, DumpProgress, DumpReport, dump, dump_to_path, not_captured};
pub use export::{
    ExportJob, ExportOutcome, ExportProgress, ExportReport, Paging, export_table, export_table_to_path,
};
pub use infer::{ColumnType, TypeInferer};
pub use job::*;
pub use parser::{CsvOptions, CsvReader, Field, ParseError, Record};
pub use run::{Item, RowReader, run_import, validate};
pub use script::{ScriptOptions, ScriptOutcome, ScriptProgress, ScriptReport, StatementStream, run_script};
pub use sniff::{Sniffed, sniff};
