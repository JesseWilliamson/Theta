//! The grid's public data types (events, metadata, staged changes).

use std::time::Duration;
use theta_core::Value;

/// Everything the host knows about a result that the grid cannot derive.
#[derive(Debug, Clone, Default)]
pub struct GridMeta {
    pub elapsed: Option<Duration>,
    /// `None` makes the grid read-only.
    pub editable: Option<EditTarget>,
    /// Statement text: shown in the footer tooltip and used for export file names.
    pub statement: Option<String>,
    /// Why the grid is read-only ("Results from more than one table"); shown
    /// next to the lock icon. Ignored when `editable` is `Some`.
    pub read_only_reason: Option<String>,
    /// Columns that are foreign keys (indexes into `ResultSet::columns`): they
    /// get a link icon, and ctrl-click / Enter / double-click on a cell emits
    /// `GridEvent::FollowReference`.
    pub reference_columns: Vec<usize>,
    /// "Load all" already ran and still hit its cap: asking again would change nothing, so the grid
    /// stops offering it and says the result must be narrowed instead.
    pub load_cap_reached: bool,
    /// The rows may no longer be true (a transaction that produced them was rolled back): shown in
    /// the footer, with a Refresh button when `refreshable`.
    pub stale: Option<String>,
    /// The host can run the statement again (`GridEvent::Refresh`).
    pub refreshable: bool,
}

impl GridMeta {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = Some(elapsed);
        self
    }
    pub fn with_editable(mut self, target: EditTarget) -> Self {
        self.editable = Some(target);
        self
    }
    pub fn with_statement(mut self, statement: impl Into<String>) -> Self {
        self.statement = Some(statement.into());
        self
    }
    pub fn with_reference_columns(mut self, columns: Vec<usize>) -> Self {
        self.reference_columns = columns;
        self
    }
    pub fn with_refreshable(mut self) -> Self {
        self.refreshable = true;
        self
    }
    pub fn with_load_cap_reached(mut self) -> Self {
        self.load_cap_reached = true;
        self
    }
    pub fn with_read_only_reason(mut self, reason: impl Into<String>) -> Self {
        self.read_only_reason = Some(reason.into());
        self
    }
}

/// The table edited cells are written back to. The app derives this from
/// `Column::origin` plus the catalog's primary key.
#[derive(Debug, Clone, PartialEq)]
pub struct EditTarget {
    pub schema: Option<String>,
    pub table: String,
    /// Indexes into `ResultSet::columns` forming the primary key.
    pub key_columns: Vec<usize>,
    pub editable_columns: Vec<usize>,
}

impl EditTarget {
    pub fn new(
        schema: Option<String>,
        table: impl Into<String>,
        key_columns: Vec<usize>,
        editable_columns: Vec<usize>,
    ) -> Self {
        EditTarget { schema, table: table.into(), key_columns, editable_columns }
    }

    /// `schema.table` or just `table`.
    pub fn qualified_name(&self) -> String {
        match &self.schema {
            Some(s) => format!("{s}.{}", self.table),
            None => self.table.clone(),
        }
    }
}

/// One staged change. Column names are the *source table's* names
/// (`Column::origin.column` when known) so the app can build SQL directly.
#[derive(Clone, Debug, PartialEq)]
pub enum RowChange {
    Update {
        /// Index into the ORIGINAL result rows.
        row: usize,
        /// Primary-key values taken from the original (pre-edit) row.
        key: Vec<(String, Value)>,
        set: Vec<(String, Value)>,
    },
    Insert {
        /// Only the columns the user filled in.
        values: Vec<(String, Value)>,
    },
    Delete {
        row: usize,
        key: Vec<(String, Value)>,
    },
}

/// Where a staged change sits, for pointing at the one a submit failed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeLocation {
    /// A row of the result (index into the original rows).
    Row(usize),
    /// The n-th (0-based) row added in the grid.
    NewRow(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormatChoice {
    Csv,
    Tsv,
    Json,
    Markdown,
    SqlInserts,
}

impl ExportFormatChoice {
    pub fn label(self) -> &'static str {
        match self {
            ExportFormatChoice::Csv => "CSV",
            ExportFormatChoice::Tsv => "TSV",
            ExportFormatChoice::Json => "JSON",
            ExportFormatChoice::Markdown => "Markdown",
            ExportFormatChoice::SqlInserts => "SQL INSERTs",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportRequest {
    pub format: ExportFormatChoice,
    pub selection_only: bool,
    pub to_clipboard: bool,
}

#[derive(Clone, Debug)]
pub enum GridEvent {
    /// The user pressed Submit: the app turns these into SQL, runs them in a
    /// transaction and then calls `changes_applied` or `changes_failed`.
    SubmitChanges(Vec<RowChange>),
    /// Text was put on the clipboard by the grid itself; informational.
    Copied { cells: usize },
    /// Format chosen in the export menu; the app shows the file dialog and writes.
    Export(ExportRequest),
    /// The user asked to see a geometry cell (`row` = original row index) or
    /// every geometry of the column (`row == None`) on the map.
    ShowOnMap { column: usize, row: Option<usize> },
    /// Current cell or range changed (the status bar shows the summary).
    SelectionChanged,
    /// Double-click / Enter on a cell of a foreign-key column.
    FollowReference { column: usize, value: Value },
    /// The user asked to load the rows beyond the row limit (the result is truncated). The host runs
    /// the statement again with a higher limit and hands the answer back with
    /// `ResultGrid::replace_result`; the grid keeps its sort and filter meanwhile.
    LoadAll,
    /// The user asked to run the statement again (F5 in the grid, or the footer's Refresh).
    Refresh,
    /// Escape with nothing left to dismiss: keyboard focus should go back to the editor.
    ReturnFocus,
}
