//! Word lists the grammar branches on. They are deliberately *unions over the dialects*: a word that is a
//! clause keyword anywhere is treated as one everywhere, because the cost of a wrong guess is a mis-shaped
//! tree (never an error), whereas the parser's other failure mode, reading a clause keyword as a column
//! name, would feed a bogus identifier to name resolution.

use crate::lexer::{Token, TokenKind};
use theta_core::Family;

fn contains_ci(list: &[&str], w: &str) -> bool {
    list.iter().any(|x| x.eq_ignore_ascii_case(w))
}

/// Words that cannot begin an expression in any dialect (nobody names a column `from`).
const HARD_STOP: &[&str] = &[
    "FROM",
    "WHERE",
    "GROUP",
    "HAVING",
    "ORDER",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "INTO",
    "ON",
    "JOIN",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "CROSS",
    "NATURAL",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "AND",
    "OR",
    "AS",
    "SELECT",
    "IS",
    "IN",
    "LIKE",
    "BETWEEN",
    "ASC",
    "DESC",
    "WITH",
    "USING",
    "COLLATE",
    "LIMIT",
    "VALUES",
];

/// Words that are reserved (so cannot be a bare column name) only in some dialects. A union over all of them
/// would make `SELECT format, option FROM t` fail in PostgreSQL, where those are ordinary names.
fn family_stop(fam: Family) -> &'static [&'static str] {
    match fam {
        Family::Postgres => &[
            "OFFSET",
            "FETCH",
            "WINDOW",
            "RETURNING",
            "ISNULL",
            "NOTNULL",
            "SIMILAR",
            "ILIKE",
            "OVERLAPS",
            "FOR",
        ],
        Family::DuckDb => &[
            "OFFSET",
            "FETCH",
            "WINDOW",
            "RETURNING",
            "ISNULL",
            "NOTNULL",
            "SIMILAR",
            "ILIKE",
            "QUALIFY",
            "FOR",
        ],
        Family::MySql => {
            &["WINDOW", "XOR", "DIV", "MOD", "REGEXP", "RLIKE", "STRAIGHT_JOIN", "RETURNING", "FOR"]
        }
        Family::Sqlite => &["ISNULL", "NOTNULL", "GLOB", "REGEXP", "MATCH"],
        Family::SqlServer => &["OPTION", "OUTPUT", "FETCH", "OVER"],
        Family::ClickHouse => &["PREWHERE", "SETTINGS"],
    }
}

/// Words that can never begin an expression (or continue one) when not followed by `(`.
pub fn cannot_start_expr(fam: Family, w: &str) -> bool {
    contains_ci(HARD_STOP, w) || contains_ci(family_stop(fam), w)
}

/// Reserved-looking words that are also function names: `LEFT(s, 2)`.
const FUNCTION_NAMES_OK: &[&str] =
    &["LEFT", "RIGHT", "VALUES", "MOD", "IF", "REPLACE", "FORMAT", "OVER", "ISNULL", "NOTNULL"];

pub fn is_function_name_ok(w: &str) -> bool {
    contains_ci(FUNCTION_NAMES_OK, w)
}

/// Words that cannot be a bare (no `AS`) alias *whatever follows them*: they start the next clause.
pub fn cannot_be_bare_alias(fam: Family, w: &str) -> bool {
    cannot_start_expr(fam, w)
        || contains_ci(
            &[
                "SET",
                "WINDOW",
                "UNION",
                "TABLESAMPLE",
                "LATERAL",
                "APPLY",
                "NOT",
                "OUTPUT",
                "OFFSET",
                "FETCH",
                "RETURNING",
                "OVERLAPS",
                "INDEXED",
            ],
            w,
        ) && !(fam == Family::Sqlite && w.eq_ignore_ascii_case("OFFSET"))
}

/// Words that appear inside a call's parentheses as syntax rather than as operands:
/// `substring(x FROM 1 FOR 2)`, `string_agg(x, ',' ORDER BY y)`, `count(DISTINCT x)`, window frames.
const LOOSE_KEYWORDS: &[&str] = &[
    "AS",
    "FROM",
    "FOR",
    "IN",
    "USING",
    "BOTH",
    "LEADING",
    "TRAILING",
    "PLACING",
    "ORDER",
    "BY",
    "SEPARATOR",
    "DISTINCT",
    "ALL",
    "ASC",
    "DESC",
    "NULLS",
    "FIRST",
    "LAST",
    "ON",
    "OVERFLOW",
    "ERROR",
    "TRUNCATE",
    "WITH",
    "WITHOUT",
    "ESCAPE",
    "PRECEDING",
    "FOLLOWING",
    "UNBOUNDED",
    "CURRENT",
    "ROW",
    "ROWS",
    "RANGE",
    "GROUPS",
    "BETWEEN",
    "AND",
    "EXCLUDE",
    "TIES",
    "OTHERS",
    "NO",
    "GROUP",
    "PARTITION",
    "KEYS",
    "VALUE",
    "RETURNING",
    "FORMAT",
    "ENCODING",
    "PASSING",
    "COLUMNS",
    "PATH",
    "DEFAULT",
    "KEY",
    "NULL",
    "IGNORE",
    "RESPECT",
    "NAME",
    "UNIQUE",
    "ABSENT",
    "SCALAR",
    "OBJECT",
    "ARRAY",
    "MATCHED",
    "THEN",
    "WHEN",
    "ELSE",
    "END",
    "CASE",
    "INTERVAL",
    "TO",
    "OVER",
    "TIME",
    "ZONE",
    "YEAR",
    "MONTH",
    "DAY",
    "HOUR",
    "MINUTE",
    "SECOND",
    "STYLE",
    "BOOLEAN",
    "MODE",
    "NATURAL",
    "LANGUAGE",
    "QUERY",
    "EXPANSION",
];

pub fn is_loose_keyword(w: &str) -> bool {
    contains_ci(LOOSE_KEYWORDS, w)
}

/// Loose keywords that are also common column names (`name`, `value`, `key`, `day`...): they only count as
/// syntax when something operand-like follows (`xmlelement(name foo)`), not in `count(name)`.
pub fn is_ambiguous_loose(w: &str) -> bool {
    const AMBIGUOUS: &[&str] = &[
        "NAME",
        "VALUE",
        "KEY",
        "KEYS",
        "FIRST",
        "LAST",
        "TIME",
        "YEAR",
        "MONTH",
        "DAY",
        "HOUR",
        "MINUTE",
        "SECOND",
        "PATH",
        "MODE",
        "STYLE",
        "ERROR",
        "ZONE",
        "TO",
        "QUERY",
        "LANGUAGE",
        "OBJECT",
        "SCALAR",
        "ARRAY",
        "ABSENT",
        "NO",
        "OTHERS",
        "UNIQUE",
        "FORMAT",
        "ENCODING",
        "RETURNING",
        "DEFAULT",
        "COLUMNS",
        "NULL",
        "TRUNCATE",
        "BOOLEAN",
        "EXPANSION",
        "MATCHED",
        "ROW",
        "CURRENT",
        "GROUP",
        "PARTITION",
        "PASSING",
        "TIES",
        "EXCLUDE",
    ];
    contains_ci(AMBIGUOUS, w)
}

/// Words that begin a join (the modifiers before `JOIN`).
const JOIN_MODIFIERS: &[&str] = &[
    "NATURAL",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "OUTER",
    "CROSS",
    "ASOF",
    "SEMI",
    "ANTI",
    "POSITIONAL",
    "GLOBAL",
    "ANY",
    "ALL",
];

pub fn is_join_modifier(w: &str) -> bool {
    contains_ci(JOIN_MODIFIERS, w)
}

/// Time-unit words after `INTERVAL`.
pub fn is_interval_unit(w: &str) -> bool {
    const UNITS: &[&str] = &[
        "YEAR",
        "YEARS",
        "MONTH",
        "MONTHS",
        "WEEK",
        "WEEKS",
        "DAY",
        "DAYS",
        "HOUR",
        "HOURS",
        "MINUTE",
        "MINUTES",
        "SECOND",
        "SECONDS",
        "MILLISECOND",
        "MILLISECONDS",
        "MICROSECOND",
        "MICROSECONDS",
        "QUARTER",
        "QUARTERS",
        "DECADE",
        "CENTURY",
        "MILLENNIUM",
        "EPOCH",
        "SECOND_MICROSECOND",
        "MINUTE_MICROSECOND",
        "MINUTE_SECOND",
        "HOUR_MICROSECOND",
        "HOUR_SECOND",
        "HOUR_MINUTE",
        "DAY_MICROSECOND",
        "DAY_SECOND",
        "DAY_MINUTE",
        "DAY_HOUR",
        "YEAR_MONTH",
    ];
    contains_ci(UNITS, w)
}

/// Type names that, in front of a string, make a typed literal (`DATE '2020-01-01'`).
pub fn is_typed_literal_word(w: &str) -> bool {
    const TYPES: &[&str] = &[
        "DATE",
        "TIME",
        "TIMESTAMP",
        "TIMESTAMPTZ",
        "TIMETZ",
        "DATETIME",
        "BOOLEAN",
        "BIT",
        "BYTEA",
        "UUID",
        "JSON",
        "JSONB",
        "DECIMAL",
        "NUMERIC",
        "INT",
        "INTEGER",
        "BIGINT",
        "SMALLINT",
        "FLOAT",
        "REAL",
        "TEXT",
        "CHAR",
        "VARCHAR",
        "POINT",
        "INET",
        "CIDR",
        "MACADDR",
        "XML",
        "MONEY",
        "BLOB",
    ];
    contains_ci(TYPES, w)
}

/// Words that continue a multi-word type name.
pub fn is_type_continuation(w: &str) -> bool {
    const WORDS: &[&str] = &[
        "PRECISION",
        "VARYING",
        "UNSIGNED",
        "SIGNED",
        "ZEROFILL",
        "ARRAY",
        "NATIONAL",
        "LONG",
        "VARCHAR",
        "CHAR",
    ];
    contains_ci(WORDS, w)
}

/// The operators `+ - * / ...` whose missing right operand is certainly a syntax error in every dialect.
/// Anything else (`!`, which is DuckDB's postfix factorial, `?`, `:`) is not flagged.
pub fn is_certain_binary_operator(op: &str, duckdb: bool) -> bool {
    if duckdb && matches!(op, "%" | "!") {
        // `LIMIT 10%`, `4 !`
        return false;
    }
    matches!(
        op,
        "+" | "-"
            | "*"
            | "/"
            | "%"
            | "="
            | "<"
            | ">"
            | "<="
            | ">="
            | "<>"
            | "!="
            | "||"
            | "&&"
            | "|"
            | "&"
            | "^"
            | "<<"
            | ">>"
            | "->"
            | "->>"
            | "#>"
            | "#>>"
            | "@>"
            | "<@"
            | "~"
            | "~*"
            | "!~"
            | "!~*"
            | "~~"
            | "~~*"
            | "=="
    )
}

/// PostgreSQL 16's keywords that cannot be a column label without `AS` (`pg_get_keywords() WHERE NOT
/// barelabel`). Every other word, reserved or not, is a valid bare label there (`SELECT 1 AND` is an alias).
pub fn pg_not_bare_label(w: &str) -> bool {
    const LIST: &[&str] = &[
        "ARRAY",
        "AS",
        "CHAR",
        "CHARACTER",
        "CREATE",
        "DAY",
        "EXCEPT",
        "FETCH",
        "FILTER",
        "FOR",
        "FROM",
        "GRANT",
        "GROUP",
        "HAVING",
        "HOUR",
        "INTERSECT",
        "INTO",
        "ISNULL",
        "LIMIT",
        "MINUTE",
        "MONTH",
        "NOTNULL",
        "OFFSET",
        "ON",
        "ORDER",
        "OVER",
        "OVERLAPS",
        "PRECISION",
        "RETURNING",
        "SECOND",
        "TO",
        "UNION",
        "VARYING",
        "WHERE",
        "WINDOW",
        "WITH",
        "WITHIN",
        "WITHOUT",
        "YEAR",
    ];
    contains_ci(LIST, w)
}

pub fn tok_is_word(sql: &str, t: &Token, w: &str) -> bool {
    t.kind == TokenKind::Word && t.text(sql).eq_ignore_ascii_case(w)
}
