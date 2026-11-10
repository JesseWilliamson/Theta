//! Small helpers shared by the three drivers.

use quarry_core::TypeKind;

/// Skip whitespace, `-- line` comments, `/* block */` comments and opening
/// parentheses so that [`leading_keyword`] sees the first real word.
fn skip_trivia(mut s: &str) -> &str {
    loop {
        let t = s.trim_start_matches(|c: char| c.is_whitespace() || c == '(');
        if let Some(rest) = t.strip_prefix("--") {
            s = rest.split_once('\n').map(|(_, r)| r).unwrap_or("");
        } else if let Some(rest) = t.strip_prefix('#') {
            // MySQL-style comment; harmless for the other dialects because no
            // statement starts with '#'.
            s = rest.split_once('\n').map(|(_, r)| r).unwrap_or("");
        } else if let Some(rest) = t.strip_prefix("/*") {
            s = rest.split_once("*/").map(|(_, r)| r).unwrap_or("");
        } else {
            return t;
        }
    }
}

/// First word of a statement, upper-cased (`"SELECT"`, `"INSERT"`, ...).
pub fn leading_keyword(sql: &str) -> String {
    skip_trivia(sql)
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_uppercase()
}

/// Whether the statement is one whose "rows affected" count is meaningful.
///
/// The wire protocols do not tell clients whether a command tag was `INSERT 0 5`
/// or `CREATE TABLE`, and a DDL statement reporting "0 rows affected" would be
/// noise, so we decide from the statement text.
pub fn reports_rows_affected(sql: &str) -> bool {
    matches!(
        leading_keyword(sql).as_str(),
        "INSERT" | "UPDATE" | "DELETE" | "REPLACE" | "MERGE" | "COPY" | "LOAD"
    )
}

/// True for statements that can change the catalog; used to invalidate caches.
pub fn is_ddl(sql: &str) -> bool {
    matches!(leading_keyword(sql).as_str(), "CREATE" | "ALTER" | "DROP" | "RENAME" | "TRUNCATE" | "COMMENT")
}

/// Convert a 1-based **character** position (what PostgreSQL reports) into a
/// 0-based **byte** offset into `sql`.
pub fn char_pos_to_byte_offset(sql: &str, pos_1based: usize) -> usize {
    sql.char_indices().nth(pos_1based.saturating_sub(1)).map(|(i, _)| i).unwrap_or(sql.len())
}

/// Strip insignificant whitespace from JSON text without re-ordering keys or
/// touching number formatting (a round-trip through `serde_json::Value` would
/// sort keys and could change numbers).
pub fn compact_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_str = false;
    let mut escaped = false;
    for c in text.chars() {
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if !c.is_whitespace() {
            out.push(c);
        }
    }
    out
}

pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Coarse family from a (lower-cased) SQL type name, for dialects that expose
/// only a textual type (SQLite declared types, Postgres `format_type` output).
pub fn kind_from_type_name(name: &str) -> TypeKind {
    let n = name.trim().to_ascii_lowercase();
    if n.ends_with("[]") || n.starts_with('_') {
        return TypeKind::Array;
    }
    let base = n.split(['(', ' ']).next().unwrap_or("");
    match base {
        "bool" | "boolean" => TypeKind::Bool,
        "int" | "integer" | "int2" | "int4" | "int8" | "smallint" | "bigint" | "tinyint" | "mediumint" | "oid"
        | "serial" | "bigserial" | "smallserial" | "year" => TypeKind::Integer,
        "float" | "float4" | "float8" | "real" | "double" => TypeKind::Float,
        "numeric" | "decimal" | "money" | "dec" | "fixed" => TypeKind::Decimal,
        "text" | "varchar" | "char" | "character" | "bpchar" | "name" | "citext" | "clob" | "nvarchar" | "nchar"
        | "tinytext" | "mediumtext" | "longtext" | "string" | "enum" | "set" | "xml" => TypeKind::Text,
        "bytea" | "blob" | "binary" | "varbinary" | "tinyblob" | "mediumblob" | "longblob" | "bit" => TypeKind::Bytes,
        "json" | "jsonb" => TypeKind::Json,
        "uuid" => TypeKind::Uuid,
        "date" => TypeKind::Date,
        "time" | "timetz" => TypeKind::Time,
        "timestamp" | "timestamptz" | "datetime" => TypeKind::Timestamp,
        "interval" => TypeKind::Interval,
        "geometry" | "geography" | "point" | "linestring" | "polygon" | "multipoint" | "multilinestring"
        | "multipolygon" | "geometrycollection" => TypeKind::Geometry,
        _ => TypeKind::Other,
    }
}

/// True when `sql` contains a `;` followed by more non-comment text, outside
/// quotes and comments. Used by drivers whose client library would otherwise
/// happily run several statements from one `execute` call.
pub fn has_multiple_statements(sql: &str) -> bool {
    let b = sql.as_bytes();
    let mut i = 0;
    let mut seen_semicolon = false;
    while i < b.len() {
        match b[i] {
            q @ (b'\'' | b'"' | b'`') => {
                if seen_semicolon {
                    return true;
                }
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' && q != b'`' {
                        i += 2;
                        continue;
                    }
                    if b[i] == q {
                        // A doubled quote is an escaped quote.
                        if b.get(i + 1) == Some(&q) {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b';' => seen_semicolon = true,
            c if c.is_ascii_whitespace() => {}
            _ => {
                if seen_semicolon {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_skips_comments() {
        assert_eq!(leading_keyword("  -- hi\n /* x */ insert into t values (1)"), "INSERT");
        assert_eq!(leading_keyword("(select 1)"), "SELECT");
        assert!(reports_rows_affected("DELETE FROM t"));
        assert!(!reports_rows_affected("CREATE TABLE t(a int)"));
    }

    #[test]
    fn char_pos_is_converted_to_byte_offset() {
        // 'é' is two bytes; the server counts it as one character.
        let sql = "SELECT 'é', bogus";
        let pos = sql.chars().position(|c| c == 'b').unwrap() + 1;
        assert_eq!(char_pos_to_byte_offset(sql, pos), sql.find("bogus").unwrap());
    }

    #[test]
    fn json_compaction_keeps_strings() {
        assert_eq!(compact_json(r#"{"a": [1, 2], "b": "x  y\" z"}"#), r#"{"a":[1,2],"b":"x  y\" z"}"#);
    }

    #[test]
    fn multiple_statements_are_detected() {
        assert!(!has_multiple_statements("select 1"));
        assert!(!has_multiple_statements("select 1;  -- done\n"));
        assert!(!has_multiple_statements("select ';' , \"a;b\", `c;d` /* ; */"));
        assert!(has_multiple_statements("select 1; select 2"));
        assert!(has_multiple_statements("select 1; /* x */ drop table t"));
        assert!(!has_multiple_statements("select 'it''s; fine'"));
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_from_type_name("VARCHAR(20)"), TypeKind::Text);
        assert_eq!(kind_from_type_name("int4[]"), TypeKind::Array);
        assert_eq!(kind_from_type_name("DOUBLE PRECISION"), TypeKind::Float);
    }
}
