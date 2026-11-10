//! Dialect keyword tables: reserved words (for identifier quoting), the keywords
//! the formatter upper-cases, and the keyword/function lists offered by completion.

use quarry_core::DatabaseKind;

const COMMON_RESERVED: &[&str] = &[
    "ALL", "AND", "ANY", "AS", "ASC", "BETWEEN", "BY", "CASE", "CAST", "CHECK", "COLLATE", "COLUMN",
    "CONSTRAINT", "CREATE", "CROSS", "CURRENT_DATE", "CURRENT_TIME", "CURRENT_TIMESTAMP", "DEFAULT",
    "DELETE", "DESC", "DISTINCT", "DROP", "ELSE", "END", "EXCEPT", "EXISTS", "FALSE", "FETCH", "FOR",
    "FOREIGN", "FROM", "FULL", "GRANT", "GROUP", "HAVING", "IN", "INNER", "INSERT", "INTERSECT", "INTO",
    "IS", "JOIN", "LEFT", "LIKE", "LIMIT", "NATURAL", "NOT", "NULL", "OFFSET", "ON", "OR", "ORDER",
    "OUTER", "PRIMARY", "REFERENCES", "RIGHT", "SELECT", "SET", "TABLE", "THEN", "TO", "TRUE", "UNION",
    "UNIQUE", "UPDATE", "USING", "VALUES", "WHEN", "WHERE", "WITH",
];

const PG_RESERVED: &[&str] = &[
    "ANALYSE", "ANALYZE", "ARRAY", "ASYMMETRIC", "BOTH", "CURRENT_CATALOG", "CURRENT_ROLE", "CURRENT_USER",
    "DEFERRABLE", "DO", "FREEZE", "ILIKE", "INITIALLY", "ISNULL", "LATERAL", "LEADING", "LOCALTIME",
    "LOCALTIMESTAMP", "NOTNULL", "ONLY", "OVERLAPS", "PLACING", "RETURNING", "SESSION_USER", "SOME",
    "SYMMETRIC", "TRAILING", "USER", "VARIADIC", "VERBOSE", "WINDOW", "AUTHORIZATION", "BINARY", "CONCURRENTLY",
    "LOCALTIME", "TABLESAMPLE", "OUTER",
];

const MYSQL_RESERVED: &[&str] = &[
    "ACCESSIBLE", "ADD", "ALTER", "ANALYZE", "ASENSITIVE", "BEFORE", "BIGINT", "BINARY", "BLOB", "BOTH", "CALL",
    "CASCADE", "CHANGE", "CHAR", "CONDITION", "CONTINUE", "CONVERT", "CURRENT_USER", "CURSOR", "DATABASE",
    "DATABASES", "DAY_HOUR", "DEC", "DECIMAL", "DECLARE", "DELAYED", "DESCRIBE", "DETERMINISTIC", "DIV",
    "DOUBLE", "DUAL", "EACH", "ELSEIF", "ENCLOSED", "ESCAPED", "EXIT", "EXPLAIN", "FLOAT", "FORCE",
    "FULLTEXT", "GENERATED", "HIGH_PRIORITY", "IF", "IGNORE", "INDEX", "INFILE", "INOUT", "INSENSITIVE", "INT",
    "INTEGER", "INTERVAL", "ITERATE", "KEY", "KEYS", "KILL", "LEAVE", "LIKE", "LINES", "LOAD", "LOCK", "LONG",
    "LOOP", "MATCH", "MOD", "MODIFIES", "OPTIMIZE", "OPTION", "OUT", "OUTFILE", "PARTITION", "PRECISION",
    "PROCEDURE", "PURGE", "RANGE", "READ", "READS", "REGEXP", "RELEASE", "RENAME", "REPEAT", "REPLACE",
    "REQUIRE", "RESTRICT", "RETURN", "REVOKE", "RLIKE", "ROW", "ROWS", "SCHEMA", "SCHEMAS", "SENSITIVE",
    "SEPARATOR", "SHOW", "SPATIAL", "SQL", "SSL", "STARTING", "STRAIGHT_JOIN", "TERMINATED", "TRIGGER",
    "UNDO", "UNLOCK", "UNSIGNED", "USAGE", "USE", "VARCHAR", "VARYING", "WHILE", "WRITE", "XOR", "ZEROFILL",
    "RANK", "GROUPS", "LATERAL", "LEAD", "LAG", "OVER", "WINDOW", "SYSTEM", "FUNCTION", "CUME_DIST",
    "DENSE_RANK", "FIRST_VALUE", "LAST_VALUE", "NTILE", "PERCENT_RANK", "ROW_NUMBER", "RECURSIVE",
];

const SQLITE_RESERVED: &[&str] = &[
    "ABORT", "ACTION", "ADD", "AFTER", "ALTER", "ANALYZE", "AUTOINCREMENT", "BEFORE", "BEGIN", "CASCADE",
    "COMMIT", "CONFLICT", "CURRENT", "DATABASE", "DEFERRABLE", "DEFERRED", "DETACH", "EACH", "ESCAPE",
    "EXCLUSIVE", "EXPLAIN", "FAIL", "GLOB", "IF", "IGNORE", "IMMEDIATE", "INDEX", "INDEXED", "INITIALLY",
    "INSTEAD", "ISNULL", "KEY", "MATCH", "NOTNULL", "NO", "OF", "OVER", "PARTITION", "PLAN", "PRAGMA",
    "QUERY", "RAISE", "RANGE", "RECURSIVE", "REGEXP", "REINDEX", "RELEASE", "RENAME", "REPLACE", "RESTRICT",
    "ROLLBACK", "ROW", "ROWS", "SAVEPOINT", "TEMP", "TEMPORARY", "TRANSACTION", "TRIGGER", "VACUUM", "VIEW",
    "VIRTUAL", "WINDOW", "WITHOUT",
];

fn contains_ci(list: &[&str], word: &str) -> bool {
    // Lists are short (< 200) and this runs only for quoting decisions, so a
    // linear scan beats building a lazy set.
    list.iter().any(|k| k.eq_ignore_ascii_case(word))
}

/// Words that must be quoted when used as an identifier in this dialect.
pub fn is_reserved(word: &str, kind: DatabaseKind) -> bool {
    contains_ci(COMMON_RESERVED, word)
        || match kind {
            DatabaseKind::Postgres => contains_ci(PG_RESERVED, word),
            DatabaseKind::MySql => contains_ci(MYSQL_RESERVED, word),
            DatabaseKind::Sqlite => contains_ci(SQLITE_RESERVED, word),
        }
}

/// Whether `ident` needs quoting to round-trip as the same identifier.
pub fn needs_quote(ident: &str, kind: DatabaseKind) -> bool {
    let mut chars = ident.chars();
    let Some(first) = chars.next() else { return true };
    let first_ok = first == '_' || first.is_ascii_lowercase() || (kind != DatabaseKind::Postgres && first.is_ascii_uppercase());
    if !first_ok {
        return true;
    }
    for c in ident.chars() {
        let ok = c == '_'
            || c == '$'
            || c.is_ascii_digit()
            || c.is_ascii_lowercase()
            || (kind != DatabaseKind::Postgres && c.is_ascii_uppercase());
        if !ok {
            return true;
        }
    }
    is_reserved(ident, kind)
}

/// Quote only when necessary (reserved word, mixed case in Postgres, special characters).
pub fn quote_if_needed(ident: &str, kind: DatabaseKind) -> String {
    if needs_quote(ident, kind) { kind.quote_ident(ident) } else { ident.to_string() }
}

/// `schema.table` with each part quoted when needed.
pub fn qualified_name(schema: Option<&str>, name: &str, kind: DatabaseKind) -> String {
    match schema {
        Some(s) if !s.is_empty() => format!("{}.{}", quote_if_needed(s, kind), quote_if_needed(name, kind)),
        _ => quote_if_needed(name, kind),
    }
}

/// Keywords the formatter upper-cases. Deliberately conservative: type names and
/// words that are commonly column names (`key`, `name`, `type`, `date`, `value`...)
/// are left alone, because changing the case of an identifier can change meaning
/// (MySQL table names are case-sensitive on some filesystems).
const FORMAT_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "GROUP", "BY", "HAVING", "ORDER", "LIMIT", "OFFSET", "JOIN", "INNER", "LEFT",
    "RIGHT", "FULL", "OUTER", "CROSS", "NATURAL", "ON", "USING", "AS", "AND", "OR", "NOT", "IN", "IS", "NULL",
    "LIKE", "ILIKE", "BETWEEN", "EXISTS", "CASE", "WHEN", "THEN", "ELSE", "END", "DISTINCT", "UNION", "ALL",
    "INTERSECT", "EXCEPT", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "TABLE", "ALTER",
    "DROP", "INDEX", "VIEW", "WITH", "RECURSIVE", "OVER", "PARTITION", "ASC", "DESC", "NULLS", "RETURNING",
    "DEFAULT", "CONFLICT", "DO", "NOTHING", "PRIMARY", "FOREIGN", "REFERENCES", "CONSTRAINT", "UNIQUE", "CHECK",
    "IF", "TRUE", "FALSE", "UNBOUNDED", "PRECEDING", "FOLLOWING", "FETCH", "ONLY", "LATERAL", "ANY", "SOME",
    "BEGIN", "COMMIT", "ROLLBACK", "TRUNCATE", "EXPLAIN", "ANALYZE", "COLUMN", "ADD", "CASCADE", "RESTRICT",
    "TEMPORARY", "TEMP", "MATERIALIZED", "UNLOCK", "FOR", "ESCAPE", "COLLATE", "RANGE", "FILTER", "WINDOW",
    "SIMILAR", "GROUPS", "DUPLICATE", "IGNORE", "REPLACE", "USE", "SHOW", "DESCRIBE", "TO", "SCHEMA", "TRIGGER",
    "FUNCTION", "PROCEDURE", "RETURNS", "RETURN", "DECLARE", "LOOP", "WHILE", "THEN", "ELSEIF", "AUTO_INCREMENT",
    "AUTOINCREMENT", "PRAGMA", "VACUUM", "ATTACH", "DETACH", "WITHOUT", "ROWID", "OF", "EXCLUDE", "TIES", "OTHERS",
];

/// Words that double as function names; they stay lower-case when followed by `(`.
const FUNCTION_LIKE: &[&str] = &["LEFT", "RIGHT", "IF", "REPLACE", "INSERT", "TRUNCATE", "DEFAULT", "FILTER", "VALUES", "RANGE", "USE"];

pub fn is_format_keyword(word: &str) -> bool {
    contains_ci(FORMAT_KEYWORDS, word)
}

pub fn is_function_like(word: &str) -> bool {
    contains_ci(FUNCTION_LIKE, word)
}

/// Any word we know as SQL keyword (format list, reserved lists or common type names).
pub fn is_keyword_any(word: &str, kind: DatabaseKind) -> bool {
    is_format_keyword(word) || is_reserved(word, kind) || is_type_name(word) || contains_ci(EXTRA_WORDS, word)
}

const EXTRA_WORDS: &[&str] = &[
    "CURRENT_DATE", "CURRENT_TIME", "CURRENT_TIMESTAMP", "LOCALTIME", "LOCALTIMESTAMP", "CURRENT_USER",
    "SESSION_USER", "INTERVAL", "AT", "ZONE", "FIRST", "LAST", "ROW", "ROWS", "CURRENT", "EXTRACT", "YEAR",
    "MONTH", "DAY", "HOUR", "MINUTE", "SECOND", "EPOCH", "DOW", "DOY", "WEEK", "QUARTER", "CAST", "COALESCE",
    "NULLIF", "ESCAPE", "SYMMETRIC", "KEY", "SIMILAR", "TO", "SUBSTRING", "FROM", "FOR", "TRIM", "LEADING",
    "TRAILING", "BOTH", "POSITION", "OVERLAY", "PLACING", "TIES", "OTHERS", "NO", "ACTION", "DOUBLE",
    "PRECISION", "VARYING", "WITHOUT", "TIME", "ZONE", "UNKNOWN", "GLOB", "REGEXP", "RLIKE", "MATCH", "TEXT",
];

const TYPE_NAMES: &[&str] = &[
    "INT", "INTEGER", "BIGINT", "SMALLINT", "TINYINT", "MEDIUMINT", "SERIAL", "BIGSERIAL", "SMALLSERIAL",
    "DECIMAL", "NUMERIC", "REAL", "FLOAT", "DOUBLE", "FLOAT4", "FLOAT8", "INT2", "INT4", "INT8", "BOOLEAN",
    "BOOL", "BIT", "CHAR", "VARCHAR", "CHARACTER", "TEXT", "TINYTEXT", "MEDIUMTEXT", "LONGTEXT", "CITEXT",
    "BYTEA", "BLOB", "BINARY", "VARBINARY", "DATE", "TIME", "TIMESTAMP", "TIMESTAMPTZ", "TIMETZ", "DATETIME",
    "YEAR", "INTERVAL", "UUID", "JSON", "JSONB", "XML", "MONEY", "INET", "CIDR", "MACADDR", "GEOMETRY",
    "GEOGRAPHY", "POINT", "ENUM", "ARRAY", "TSVECTOR", "TSQUERY", "OID", "REGCLASS", "NAME", "UNSIGNED",
];

pub fn is_type_name(word: &str) -> bool {
    contains_ci(TYPE_NAMES, word)
}

// ----- completion tables --------------------------------------------------

/// Words that start a statement.
pub fn statement_keywords(kind: DatabaseKind) -> Vec<&'static str> {
    let mut v = vec![
        "SELECT", "WITH", "INSERT INTO", "UPDATE", "DELETE FROM", "CREATE", "ALTER", "DROP", "TRUNCATE",
        "EXPLAIN", "EXPLAIN ANALYZE", "BEGIN", "COMMIT", "ROLLBACK", "SET", "VALUES", "GRANT", "REVOKE",
        "SAVEPOINT",
    ];
    match kind {
        DatabaseKind::Postgres => v.extend([
            "COPY", "ANALYZE", "VACUUM", "DO", "CALL", "LISTEN", "NOTIFY", "REFRESH MATERIALIZED VIEW", "TABLE",
            "COMMENT ON", "PREPARE", "EXECUTE", "DEALLOCATE", "DISCARD", "LOCK", "RESET", "RELEASE", "SHOW",
            "START TRANSACTION", "REINDEX", "CLUSTER",
        ]),
        DatabaseKind::MySql => v.extend([
            "USE", "DESCRIBE", "SHOW", "REPLACE INTO", "CALL", "LOCK TABLES", "UNLOCK TABLES", "OPTIMIZE TABLE",
            "ANALYZE TABLE", "LOAD DATA", "START TRANSACTION", "RENAME TABLE", "PREPARE", "EXECUTE", "RELEASE",
        ]),
        DatabaseKind::Sqlite => v.extend(["PRAGMA", "ATTACH DATABASE", "DETACH DATABASE", "VACUUM", "REINDEX", "REPLACE INTO", "ANALYZE", "RELEASE"]),
    }
    v
}

/// Keywords valid right after a complete operand, by clause name.
pub fn follow_keywords(clause: &str, kind: DatabaseKind) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = match clause {
        "select" => vec!["FROM", "AS", "INTO", "UNION", "UNION ALL"],
        "from" | "join" => vec![
            "WHERE", "JOIN", "INNER JOIN", "LEFT JOIN", "LEFT OUTER JOIN", "RIGHT JOIN", "FULL JOIN", "CROSS JOIN",
            "NATURAL JOIN", "ON", "USING", "GROUP BY", "ORDER BY", "HAVING", "LIMIT", "UNION", "UNION ALL", "AS",
        ],
        "where" | "having" | "on" => vec![
            "AND", "OR", "NOT", "IN", "LIKE", "BETWEEN", "IS NULL", "IS NOT NULL", "GROUP BY", "ORDER BY", "HAVING",
            "LIMIT", "UNION", "UNION ALL",
        ],
        "group_by" => vec!["HAVING", "ORDER BY", "LIMIT", "UNION", "UNION ALL"],
        "order_by" => vec!["ASC", "DESC", "NULLS FIRST", "NULLS LAST", "LIMIT", "OFFSET", "UNION"],
        "limit" => vec!["OFFSET", "UNION"],
        "set" => vec!["WHERE", "FROM", "RETURNING"],
        "update" => vec!["SET", "AS"],
        "delete" => vec!["WHERE", "USING", "RETURNING"],
        "into" => vec!["VALUES", "SELECT", "DEFAULT VALUES", "AS"],
        "values" => vec!["ON CONFLICT", "RETURNING"],
        _ => vec!["AND", "OR", "AS", "FROM", "WHERE"],
    };
    if kind == DatabaseKind::MySql && clause == "values" {
        v.retain(|k| *k != "ON CONFLICT");
        v.push("ON DUPLICATE KEY UPDATE");
    }
    if kind == DatabaseKind::Postgres && matches!(clause, "where" | "having" | "on") {
        v.extend(["ILIKE", "IS DISTINCT FROM"]);
    }
    v
}

/// Keywords that can start an operand in an expression.
pub fn operand_keywords(kind: DatabaseKind) -> Vec<&'static str> {
    let mut v = vec!["NOT", "NULL", "TRUE", "FALSE", "CASE", "EXISTS", "DISTINCT", "ANY", "ALL", "CAST", "INTERVAL"];
    if kind == DatabaseKind::Postgres {
        v.push("ARRAY");
    }
    v
}

pub struct FunctionDef {
    pub name: &'static str,
    /// Argument list as shown to the user, without the parentheses.
    pub args: &'static str,
    pub returns: &'static str,
}

macro_rules! f {
    ($n:literal, $a:literal, $r:literal) => {
        FunctionDef { name: $n, args: $a, returns: $r }
    };
}

const COMMON_FUNCTIONS: &[FunctionDef] = &[
    f!("count", "expr", "bigint"),
    f!("sum", "expr", "numeric"),
    f!("avg", "expr", "numeric"),
    f!("min", "expr", "same as input"),
    f!("max", "expr", "same as input"),
    f!("coalesce", "value, ...", "same as input"),
    f!("nullif", "a, b", "same as a"),
    f!("abs", "x", "numeric"),
    f!("round", "x, digits", "numeric"),
    f!("ceil", "x", "numeric"),
    f!("floor", "x", "numeric"),
    f!("length", "string", "integer"),
    f!("lower", "string", "text"),
    f!("upper", "string", "text"),
    f!("trim", "string", "text"),
    f!("replace", "string, from, to", "text"),
    f!("substring", "string, start, length", "text"),
    f!("row_number", "", "bigint"),
    f!("rank", "", "bigint"),
    f!("dense_rank", "", "bigint"),
    f!("lag", "value, offset, default", "same as input"),
    f!("lead", "value, offset, default", "same as input"),
    f!("first_value", "value", "same as input"),
    f!("last_value", "value", "same as input"),
    f!("ntile", "buckets", "integer"),
    f!("cast", "expr AS type", "type"),
];

const PG_FUNCTIONS: &[FunctionDef] = &[
    f!("now", "", "timestamptz"),
    f!("date_trunc", "field, source", "timestamp"),
    f!("extract", "field FROM source", "numeric"),
    f!("to_char", "value, format", "text"),
    f!("to_date", "text, format", "date"),
    f!("to_timestamp", "text, format", "timestamptz"),
    f!("age", "timestamp, timestamp", "interval"),
    f!("string_agg", "expr, delimiter", "text"),
    f!("array_agg", "expr", "anyarray"),
    f!("json_agg", "expr", "json"),
    f!("jsonb_agg", "expr", "jsonb"),
    f!("jsonb_build_object", "key, value, ...", "jsonb"),
    f!("jsonb_extract_path_text", "from_json, path, ...", "text"),
    f!("jsonb_array_elements", "jsonb", "setof jsonb"),
    f!("generate_series", "start, stop, step", "setof"),
    f!("unnest", "anyarray", "setof"),
    f!("regexp_replace", "source, pattern, replacement, flags", "text"),
    f!("regexp_matches", "source, pattern, flags", "setof text[]"),
    f!("split_part", "string, delimiter, n", "text"),
    f!("left", "string, n", "text"),
    f!("right", "string, n", "text"),
    f!("position", "substring IN string", "integer"),
    f!("gen_random_uuid", "", "uuid"),
    f!("greatest", "value, ...", "same as input"),
    f!("least", "value, ...", "same as input"),
    f!("array_length", "anyarray, dimension", "integer"),
    f!("cardinality", "anyarray", "integer"),
    f!("md5", "text", "text"),
    f!("lpad", "string, length, fill", "text"),
    f!("rpad", "string, length, fill", "text"),
    f!("initcap", "string", "text"),
    f!("concat", "value, ...", "text"),
    f!("concat_ws", "separator, value, ...", "text"),
    f!("format", "formatstr, value, ...", "text"),
    f!("percentile_cont", "fraction", "double precision"),
    f!("bool_and", "expr", "boolean"),
    f!("bool_or", "expr", "boolean"),
    f!("current_setting", "name", "text"),
    f!("pg_size_pretty", "bigint", "text"),
    f!("pg_total_relation_size", "regclass", "bigint"),
    f!("clock_timestamp", "", "timestamptz"),
    f!("date_part", "field, source", "double precision"),
    f!("make_date", "year, month, day", "date"),
    f!("random", "", "double precision"),
];

const MYSQL_FUNCTIONS: &[FunctionDef] = &[
    f!("now", "", "datetime"),
    f!("curdate", "", "date"),
    f!("date_format", "date, format", "varchar"),
    f!("ifnull", "a, b", "same as a"),
    f!("if", "cond, then, else", "same as then"),
    f!("group_concat", "expr SEPARATOR sep", "text"),
    f!("concat", "str, ...", "varchar"),
    f!("concat_ws", "separator, str, ...", "varchar"),
    f!("substring_index", "str, delim, count", "varchar"),
    f!("str_to_date", "str, format", "datetime"),
    f!("date_add", "date, INTERVAL expr unit", "datetime"),
    f!("date_sub", "date, INTERVAL expr unit", "datetime"),
    f!("datediff", "a, b", "int"),
    f!("timestampdiff", "unit, a, b", "bigint"),
    f!("json_extract", "doc, path", "json"),
    f!("json_unquote", "json", "varchar"),
    f!("json_object", "key, value, ...", "json"),
    f!("uuid", "", "varchar"),
    f!("last_insert_id", "", "bigint"),
    f!("found_rows", "", "bigint"),
    f!("left", "str, n", "varchar"),
    f!("right", "str, n", "varchar"),
    f!("lpad", "str, len, pad", "varchar"),
    f!("rpad", "str, len, pad", "varchar"),
    f!("rand", "", "double"),
    f!("greatest", "value, ...", "same as input"),
    f!("least", "value, ...", "same as input"),
    f!("year", "date", "int"),
    f!("month", "date", "int"),
    f!("day", "date", "int"),
];

const SQLITE_FUNCTIONS: &[FunctionDef] = &[
    f!("datetime", "time-value, modifier, ...", "text"),
    f!("date", "time-value, modifier, ...", "text"),
    f!("time", "time-value, modifier, ...", "text"),
    f!("strftime", "format, time-value, ...", "text"),
    f!("julianday", "time-value, ...", "real"),
    f!("ifnull", "a, b", "same as a"),
    f!("iif", "cond, then, else", "same as then"),
    f!("group_concat", "expr, separator", "text"),
    f!("json_extract", "json, path", "any"),
    f!("json_object", "label, value, ...", "text"),
    f!("json_group_array", "value", "text"),
    f!("random", "", "integer"),
    f!("randomblob", "n", "blob"),
    f!("typeof", "expr", "text"),
    f!("instr", "string, substring", "integer"),
    f!("substr", "string, start, length", "text"),
    f!("printf", "format, ...", "text"),
    f!("total", "expr", "real"),
    f!("glob", "pattern, string", "integer"),
    f!("zeroblob", "n", "blob"),
    f!("last_insert_rowid", "", "integer"),
    f!("changes", "", "integer"),
    f!("max", "a, b, ...", "same as input"),
    f!("min", "a, b, ...", "same as input"),
];

const POSTGIS_FUNCTIONS: &[FunctionDef] = &[
    f!("ST_AsText", "geometry", "text"),
    f!("ST_AsEWKT", "geometry", "text"),
    f!("ST_AsGeoJSON", "geometry, maxdecimaldigits", "text"),
    f!("ST_AsBinary", "geometry", "bytea"),
    f!("ST_Area", "geometry", "double precision"),
    f!("ST_Length", "geometry", "double precision"),
    f!("ST_Perimeter", "geometry", "double precision"),
    f!("ST_Distance", "geomA, geomB", "double precision"),
    f!("ST_DWithin", "geomA, geomB, distance", "boolean"),
    f!("ST_Intersects", "geomA, geomB", "boolean"),
    f!("ST_Contains", "geomA, geomB", "boolean"),
    f!("ST_Within", "geomA, geomB", "boolean"),
    f!("ST_Covers", "geomA, geomB", "boolean"),
    f!("ST_Touches", "geomA, geomB", "boolean"),
    f!("ST_Crosses", "geomA, geomB", "boolean"),
    f!("ST_Overlaps", "geomA, geomB", "boolean"),
    f!("ST_Equals", "geomA, geomB", "boolean"),
    f!("ST_Disjoint", "geomA, geomB", "boolean"),
    f!("ST_Transform", "geometry, srid", "geometry"),
    f!("ST_SetSRID", "geometry, srid", "geometry"),
    f!("ST_SRID", "geometry", "integer"),
    f!("ST_MakePoint", "x, y", "geometry"),
    f!("ST_MakeEnvelope", "xmin, ymin, xmax, ymax, srid", "geometry"),
    f!("ST_MakeLine", "geomA, geomB", "geometry"),
    f!("ST_Buffer", "geometry, radius", "geometry"),
    f!("ST_Centroid", "geometry", "geometry"),
    f!("ST_PointOnSurface", "geometry", "geometry"),
    f!("ST_Union", "geomA, geomB", "geometry"),
    f!("ST_Intersection", "geomA, geomB", "geometry"),
    f!("ST_Difference", "geomA, geomB", "geometry"),
    f!("ST_Collect", "geometry", "geometry"),
    f!("ST_Envelope", "geometry", "geometry"),
    f!("ST_Boundary", "geometry", "geometry"),
    f!("ST_Simplify", "geometry, tolerance", "geometry"),
    f!("ST_Segmentize", "geometry, max_length", "geometry"),
    f!("ST_Expand", "geometry, units", "geometry"),
    f!("ST_ClosestPoint", "geomA, geomB", "geometry"),
    f!("ST_X", "point", "double precision"),
    f!("ST_Y", "point", "double precision"),
    f!("ST_NPoints", "geometry", "integer"),
    f!("ST_Dimension", "geometry", "integer"),
    f!("ST_IsValid", "geometry", "boolean"),
    f!("ST_GeomFromText", "wkt, srid", "geometry"),
    f!("ST_GeomFromEWKT", "ewkt", "geometry"),
    f!("ST_GeomFromGeoJSON", "geojson", "geometry"),
    f!("ST_GeomFromEWKB", "bytea", "geometry"),
];

const MYSQL_SPATIAL_FUNCTIONS: &[FunctionDef] = &[
    f!("ST_AsText", "geometry", "text"),
    f!("ST_AsGeoJSON", "geometry", "json"),
    f!("ST_Area", "polygon", "double"),
    f!("ST_Distance", "geomA, geomB", "double"),
    f!("ST_Contains", "geomA, geomB", "int"),
    f!("ST_Within", "geomA, geomB", "int"),
    f!("ST_Intersects", "geomA, geomB", "int"),
    f!("ST_Buffer", "geometry, distance", "geometry"),
    f!("ST_Centroid", "geometry", "point"),
    f!("ST_GeomFromText", "wkt, srid", "geometry"),
    f!("ST_SRID", "geometry", "int"),
    f!("ST_X", "point", "double"),
    f!("ST_Y", "point", "double"),
];

/// Built-in functions for the dialect. Spatial functions are included only when
/// the connected catalog actually has geometry columns, to keep the list short.
pub fn builtin_functions(kind: DatabaseKind, has_geometry: bool) -> Vec<&'static FunctionDef> {
    let mut out: Vec<&'static FunctionDef> = Vec::new();
    let specific: &[FunctionDef] = match kind {
        DatabaseKind::Postgres => PG_FUNCTIONS,
        DatabaseKind::MySql => MYSQL_FUNCTIONS,
        DatabaseKind::Sqlite => SQLITE_FUNCTIONS,
    };
    out.extend(specific.iter());
    for f in COMMON_FUNCTIONS {
        if !out.iter().any(|g| g.name.eq_ignore_ascii_case(f.name)) {
            out.push(f);
        }
    }
    if has_geometry {
        match kind {
            DatabaseKind::Postgres => out.extend(POSTGIS_FUNCTIONS.iter()),
            DatabaseKind::MySql => out.extend(MYSQL_SPATIAL_FUNCTIONS.iter()),
            DatabaseKind::Sqlite => {}
        }
    }
    out
}

/// Known function names (lower-case) for lint to avoid flagging them as columns.
pub fn is_known_function(name: &str) -> bool {
    [COMMON_FUNCTIONS, PG_FUNCTIONS, MYSQL_FUNCTIONS, SQLITE_FUNCTIONS, POSTGIS_FUNCTIONS, MYSQL_SPATIAL_FUNCTIONS]
        .iter()
        .any(|l| l.iter().any(|f| f.name.eq_ignore_ascii_case(name)))
}

/// Statement-start words per dialect, for lint's "unknown statement" check.
pub fn is_statement_start(word: &str) -> bool {
    const WORDS: &[&str] = &[
        "SELECT", "WITH", "INSERT", "UPDATE", "DELETE", "MERGE", "CREATE", "ALTER", "DROP", "TRUNCATE", "EXPLAIN",
        "BEGIN", "START", "COMMIT", "ROLLBACK", "END", "ABORT", "SAVEPOINT", "RELEASE", "SET", "RESET", "SHOW",
        "VALUES", "TABLE", "GRANT", "REVOKE", "COPY", "ANALYZE", "ANALYSE", "VACUUM", "DO", "CALL", "LISTEN",
        "UNLISTEN", "NOTIFY", "REFRESH", "COMMENT", "PREPARE", "EXECUTE", "DEALLOCATE", "DISCARD", "LOCK", "REINDEX",
        "CLUSTER", "USE", "DESCRIBE", "DESC", "REPLACE", "UNLOCK", "OPTIMIZE", "LOAD", "RENAME", "PRAGMA",
        "ATTACH", "DETACH", "UPSERT", "CHECKPOINT", "SECURITY", "IMPORT", "FLUSH", "KILL", "HANDLER", "XA",
        "BACKUP", "RESTORE", "REPAIR", "CHECK", "CHECKSUM", "HELP", "SIGNAL", "RESIGNAL", "GET", "STOP", "SHUTDOWN",
        "FETCH", "MOVE", "CLOSE", "DECLARE", "TRUNCATE", "ROLLBACK", "PURGE", "RESET", "INSTALL", "UNINSTALL",
        "DELIMITER", "SOURCE",
    ];
    WORDS.iter().any(|w| w.eq_ignore_ascii_case(word))
}

#[cfg(test)]
mod tests {
    use super::*;
    use DatabaseKind::*;

    #[test]
    fn quoting_rules() {
        assert!(!needs_quote("customers", Postgres));
        assert!(needs_quote("Customers", Postgres));
        assert!(!needs_quote("Customers", MySql));
        assert!(needs_quote("order", Postgres));
        assert!(needs_quote("has space", Sqlite));
        assert!(needs_quote("1abc", Postgres));
        assert!(needs_quote("", Postgres));
        assert!(needs_quote("naïve", Postgres));
        assert_eq!(quote_if_needed("group", MySql), "`group`");
        assert_eq!(quote_if_needed("a\"b", Postgres), "\"a\"\"b\"");
    }

    #[test]
    fn qualified() {
        assert_eq!(qualified_name(Some("public"), "orders", Postgres), "public.orders");
        assert_eq!(qualified_name(Some("My S"), "user", Postgres), "\"My S\".\"user\"");
        assert_eq!(qualified_name(None, "t", MySql), "t");
    }

    #[test]
    fn spatial_functions_only_with_geometry() {
        let without = builtin_functions(Postgres, false);
        assert!(!without.iter().any(|f| f.name == "ST_Distance"));
        let with = builtin_functions(Postgres, true);
        assert!(with.iter().any(|f| f.name == "ST_Distance"));
    }
}
