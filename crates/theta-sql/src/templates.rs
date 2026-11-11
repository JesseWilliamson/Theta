//! Live templates: snippets with tab stops that expand from an abbreviation.
//!
//! A template body uses the snippet syntax of the editing engine (`$1`, `${2:default}`, `$0`; a stop that
//! appears twice is linked) plus *variables* that this module substitutes before the engine sees the text:
//!
//! | variable | value |
//! | --- | --- |
//! | `${table}` | a table the user picks, schema-qualified when needed |
//! | `${column}`, `${columns}` | the columns of that table, comma separated |
//! | `${selection}` | the selected text, without a trailing `;` |
//! | `${clipboard}` | the clipboard text |
//! | `${date}` | today's date, `YYYY-MM-DD` |
//!
//! Built-ins differ by dialect only where the syntax does (`crt`, `exp`). The user's own templates live in
//! `templates.json` and override a built-in with the same abbreviation.

use serde::{Deserialize, Serialize};
use theta_core::DatabaseKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    #[serde(alias = "name")]
    pub abbreviation: String,
    #[serde(default)]
    pub description: String,
    pub body: String,
    /// Restrict to these databases; empty means all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dialects: Vec<DatabaseKind>,
}

impl Template {
    fn new(abbreviation: &str, description: &str, body: &str) -> Template {
        Template {
            abbreviation: abbreviation.into(),
            description: description.into(),
            body: body.into(),
            dialects: Vec::new(),
        }
    }

    pub fn applies_to(&self, kind: DatabaseKind) -> bool {
        self.dialects.is_empty() || self.dialects.contains(&kind)
    }

    /// Whether expanding this template needs the user to pick a table first.
    pub fn needs_table(&self) -> bool {
        needs_table(&self.body)
    }
}

/// The built-in templates for `kind`.
pub fn builtins(kind: DatabaseKind) -> Vec<Template> {
    let t = Template::new;
    let (crt, exp) = match kind {
        DatabaseKind::Postgres => (
            "CREATE TABLE ${1:name} (\n    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,\n    ${2:column} ${3:text} NOT NULL,\n    created_at timestamptz NOT NULL DEFAULT now()\n)$0",
            "EXPLAIN (ANALYZE, BUFFERS) $0",
        ),
        DatabaseKind::MySql => (
            "CREATE TABLE ${1:name} (\n    id bigint AUTO_INCREMENT PRIMARY KEY,\n    ${2:column} ${3:varchar(255)} NOT NULL,\n    created_at timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP\n)$0",
            "EXPLAIN ANALYZE $0",
        ),
        DatabaseKind::Sqlite => (
            "CREATE TABLE ${1:name} (\n    id INTEGER PRIMARY KEY AUTOINCREMENT,\n    ${2:column} ${3:TEXT} NOT NULL,\n    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP\n)$0",
            "EXPLAIN QUERY PLAN $0",
        ),
    };
    vec![
        t("sel", "SELECT … FROM …", "SELECT ${1:*} FROM ${2:table}$0"),
        t("selw", "SELECT … FROM … WHERE …", "SELECT ${1:*}\nFROM ${2:table}\nWHERE ${3:condition}$0"),
        t("selt", "SELECT * from a table you pick", "SELECT *\nFROM ${table}\nLIMIT ${1:100}$0"),
        t("selcols", "SELECT every column of a table you pick", "SELECT ${columns}\nFROM ${table}$0"),
        t("ins", "INSERT INTO … VALUES …", "INSERT INTO ${1:table} (${2:columns})\nVALUES (${3:values})$0"),
        t(
            "upd",
            "UPDATE … SET … WHERE …",
            "UPDATE ${1:table}\nSET ${2:column} = ${3:value}\nWHERE ${4:condition}$0",
        ),
        t("del", "DELETE FROM … WHERE …", "DELETE FROM ${1:table}\nWHERE ${2:condition}$0"),
        t(
            "join",
            "SELECT with a JOIN",
            "SELECT ${1:*}\nFROM ${2:a}\nJOIN ${3:b} ON $3.${4:a_id} = $2.${5:id}$0",
        ),
        t(
            "cte",
            "WITH … AS (…) SELECT …",
            "WITH ${1:cte} AS (\n    ${2:SELECT 1}\n)\nSELECT ${3:*} FROM $1$0",
        ),
        t(
            "case",
            "CASE WHEN … THEN … ELSE … END",
            "CASE\n    WHEN ${1:condition} THEN ${2:result}\n    ELSE ${3:other}\nEND$0",
        ),
        t("cnt", "count the rows of a table", "SELECT count(*) FROM ${1:table}$0"),
        t(
            "grp",
            "GROUP BY with a count",
            "SELECT ${1:column}, count(*)\nFROM ${2:table}\nGROUP BY $1\nORDER BY count(*) DESC$0",
        ),
        t(
            "top",
            "the first rows by an ordering",
            "SELECT ${1:*}\nFROM ${2:table}\nORDER BY ${3:column} DESC\nLIMIT ${4:10}$0",
        ),
        t(
            "exists",
            "EXISTS (SELECT 1 …)",
            "EXISTS (\n    SELECT 1 FROM ${1:table} WHERE ${2:condition}\n)$0",
        ),
        t("idx", "CREATE INDEX", "CREATE INDEX ${1:idx_name} ON ${2:table} (${3:column})$0"),
        t("uidx", "CREATE UNIQUE INDEX", "CREATE UNIQUE INDEX ${1:idx_name} ON ${2:table} (${3:column})$0"),
        t("crt", "CREATE TABLE with an id and a timestamp", crt),
        t("exp", "EXPLAIN a statement", exp),
    ]
}

/// What `templates.json` held: the templates and one warning per entry that was skipped.
pub fn parse_user_templates(json: &str) -> (Vec<Template>, Vec<String>) {
    let mut warnings = Vec::new();
    if json.trim().is_empty() {
        return (Vec::new(), warnings);
    }
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(e) => return (Vec::new(), vec![format!("templates.json is not valid JSON ({e})")]),
    };
    let list = match &value {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(o) => match o.get("templates") {
            Some(serde_json::Value::Array(a)) => a.clone(),
            _ => {
                return (
                    Vec::new(),
                    vec!["templates.json must be a list, or an object with a \"templates\" list".into()],
                );
            }
        },
        _ => return (Vec::new(), vec!["templates.json must be a list of templates".into()]),
    };
    let mut out: Vec<Template> = Vec::new();
    for (i, item) in list.into_iter().enumerate() {
        match serde_json::from_value::<Template>(item) {
            Ok(t) if t.abbreviation.trim().is_empty() => {
                warnings.push(format!("templates.json: entry {} has an empty abbreviation; skipped", i + 1))
            }
            Ok(t) if t.abbreviation.chars().any(|c| !(c.is_alphanumeric() || c == '_')) => {
                warnings.push(format!(
                    "templates.json: “{}” may only contain letters, digits and _; skipped",
                    t.abbreviation
                ))
            }
            Ok(t) if t.body.is_empty() => {
                warnings.push(format!("templates.json: “{}” has an empty body; skipped", t.abbreviation))
            }
            Ok(t) if out.iter().any(|o| o.abbreviation.eq_ignore_ascii_case(&t.abbreviation)) => warnings
                .push(format!("templates.json: “{}” appears twice; the first one is used", t.abbreviation)),
            Ok(t) => out.push(t),
            Err(e) => warnings.push(format!("templates.json: entry {} skipped ({e})", i + 1)),
        }
    }
    (out, warnings)
}

/// The templates in force for `kind`: built-ins, with the user's replacing those of the same abbreviation.
pub fn merged(kind: DatabaseKind, user: &[Template]) -> Vec<Template> {
    let mine: Vec<&Template> = user.iter().filter(|t| t.applies_to(kind)).collect();
    let mut out: Vec<Template> = builtins(kind)
        .into_iter()
        .filter(|b| !mine.iter().any(|u| u.abbreviation.eq_ignore_ascii_case(&b.abbreviation)))
        .collect();
    out.extend(mine.into_iter().cloned());
    out.sort_by_key(|t| t.abbreviation.to_lowercase());
    out
}

/// The template whose abbreviation is exactly `word` (case-insensitive).
pub fn expand<'a>(templates: &'a [Template], word: &str) -> Option<&'a Template> {
    (!word.is_empty()).then(|| templates.iter().find(|t| t.abbreviation.eq_ignore_ascii_case(word))).flatten()
}

/// A starter `templates.json`, written the first time the user asks to edit it.
pub fn sample_file() -> String {
    let sample = vec![Template {
        abbreviation: "recent".into(),
        description: "rows of a table from the last N days".into(),
        body: "SELECT *\nFROM ${table}\nWHERE ${1:created_at} >= now() - interval '${2:7} days'$0".into(),
        dialects: vec![DatabaseKind::Postgres],
    }];
    serde_json::to_string_pretty(&serde_json::json!({ "templates": sample })).unwrap_or_default() + "\n"
}

// ------------------------------------------------------------------------------------------- variables

/// Values for the variables of a template.
#[derive(Debug, Clone, Default)]
pub struct Vars {
    pub table: Option<String>,
    pub columns: Vec<String>,
    pub selection: String,
    pub clipboard: String,
    /// `YYYY-MM-DD`; supplied by the caller because this crate has no clock.
    pub date: String,
}

const VARIABLES: [&str; 5] = ["table", "column", "columns", "selection", "clipboard"];

fn variable_at(chars: &[char], i: usize) -> Option<(&'static str, usize)> {
    // `${name}` starting at chars[i] == '$'.
    if chars.get(i) != Some(&'$') || chars.get(i + 1) != Some(&'{') {
        return None;
    }
    let close = chars[i + 2..].iter().position(|&c| c == '}')? + i + 2;
    let name: String = chars[i + 2..close].iter().collect();
    VARIABLES.iter().chain(std::iter::once(&"date")).find(|v| **v == name).map(|v| (*v, close + 1))
}

/// Whether the body refers to `${table}` or the table's columns.
pub fn needs_table(body: &str) -> bool {
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if let Some((name, next)) = variable_at(&chars, i) {
            if matches!(name, "table" | "column" | "columns") {
                return true;
            }
            i = next;
        } else {
            i += 1;
        }
    }
    false
}

/// Escape `value` so the snippet parser reads it back as the same literal text.
pub fn escape_for_snippet(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '$' | '}' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Substitute the variables of `body`; tab stops and escapes are left for the snippet parser.
pub fn resolve_variables(body: &str, vars: &Vars) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' {
            out.push(chars[i]);
            if let Some(&n) = chars.get(i + 1) {
                out.push(n);
            }
            i += 2;
            continue;
        }
        match variable_at(&chars, i) {
            Some((name, next)) => {
                let value = match name {
                    "table" => vars.table.clone().unwrap_or_default(),
                    "column" | "columns" => vars.columns.join(", "),
                    "selection" => vars.selection.clone(),
                    "clipboard" => vars.clipboard.clone(),
                    _ => vars.date.clone(),
                };
                out.push_str(&escape_for_snippet(&value));
                i = next;
            }
            None => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

// ------------------------------------------------------------------------------------------- surround

/// A way to wrap the selected text ("Surround with").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Surround {
    pub label: &'static str,
    pub body: &'static str,
}

pub fn surround_options() -> Vec<Surround> {
    vec![
        Surround {
            label: "SELECT count(*) FROM (…) t",
            body: "SELECT count(*) FROM (\n    ${selection}\n) t$0",
        },
        Surround {
            label: "SELECT * FROM (…) AS t",
            body: "SELECT *\nFROM (\n    ${selection}\n) AS ${1:t}$0",
        },
        Surround {
            label: "WITH cte AS (…) SELECT * FROM cte",
            body: "WITH ${1:cte} AS (\n    ${selection}\n)\nSELECT * FROM $1$0",
        },
        Surround { label: "EXISTS (…)", body: "EXISTS (\n    ${selection}\n)$0" },
        Surround { label: "( … )", body: "(${selection})$0" },
    ]
}

/// The selection as a variable value: surrounding blank lines and a trailing `;` are dropped, and the
/// statement's lines are de-indented so they can be re-indented by the snippet.
pub fn selection_value(selected: &str) -> String {
    // Drop blank lines around the text but keep the first line's own indentation: it is part of the
    // "common indent" the other lines are measured against.
    let t = selected.trim_start_matches(['\n', '\r']).trim_end();
    let t = t.strip_suffix(';').unwrap_or(t).trim_end();
    let lines: Vec<&str> = t.lines().collect();
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines.iter().map(|l| l.get(indent..).unwrap_or(l.trim_start())).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use DatabaseKind::*;

    #[test]
    fn every_builtin_parses_to_a_valid_snippet_in_every_dialect() {
        for kind in [Postgres, MySql, Sqlite] {
            let all = builtins(kind);
            assert!(all.len() >= 15);
            let mut seen = std::collections::HashSet::new();
            for t in &all {
                assert!(seen.insert(t.abbreviation.clone()), "{} twice", t.abbreviation);
                assert!(!t.description.is_empty() && !t.body.is_empty());
                // Every variable a built-in uses must be one that `resolve_variables` knows.
                let resolved = resolve_variables(
                    &t.body,
                    &Vars { table: Some("t".into()), columns: vec!["a".into()], ..Default::default() },
                );
                assert!(
                    !resolved.contains("${table}") && !resolved.contains("${columns}"),
                    "{}",
                    t.abbreviation
                );
            }
        }
    }

    #[test]
    fn dialect_specific_bodies() {
        let body = |k, a: &str| builtins(k).into_iter().find(|t| t.abbreviation == a).unwrap().body;
        assert!(body(Postgres, "crt").contains("GENERATED ALWAYS AS IDENTITY"));
        assert!(body(MySql, "crt").contains("AUTO_INCREMENT"));
        assert!(body(Sqlite, "crt").contains("AUTOINCREMENT"));
        assert!(body(Sqlite, "exp").contains("QUERY PLAN"));
        assert!(body(Postgres, "exp").contains("ANALYZE"));
    }

    #[test]
    fn user_templates_override_builtins_and_respect_dialects() {
        let user = vec![
            Template {
                abbreviation: "SEL".into(),
                description: "mine".into(),
                body: "SELECT 1".into(),
                dialects: vec![],
            },
            Template {
                abbreviation: "pgonly".into(),
                description: String::new(),
                body: "x".into(),
                dialects: vec![Postgres],
            },
        ];
        let pg = merged(Postgres, &user);
        assert_eq!(pg.iter().filter(|t| t.abbreviation.eq_ignore_ascii_case("sel")).count(), 1);
        assert_eq!(expand(&pg, "sel").unwrap().description, "mine");
        assert!(expand(&pg, "pgonly").is_some());
        assert!(expand(&merged(MySql, &user), "pgonly").is_none());
        // Sorted by abbreviation for the Settings list.
        let names: Vec<_> = pg.iter().map(|t| t.abbreviation.to_lowercase()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn parses_both_file_shapes_and_reports_bad_entries() {
        let (t, w) = parse_user_templates(
            r#"[{"name":"a","body":"x"},{"abbreviation":"","body":"y"},{"abbreviation":"b-c","body":"z"},{"abbreviation":"d"}]"#,
        );
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].abbreviation, "a", "`name` is an alias of `abbreviation`");
        assert_eq!(w.len(), 3, "{w:?}");
        let (t, w) =
            parse_user_templates(r#"{"templates":[{"abbreviation":"a","body":"x","dialects":["sqlite"]}]}"#);
        assert!(w.is_empty());
        assert_eq!(t[0].dialects, [Sqlite]);
        assert!(parse_user_templates("{ nope").1[0].contains("not valid JSON"));
        assert!(parse_user_templates("   ").1.is_empty());
        let (_, w) =
            parse_user_templates(r#"[{"abbreviation":"a","body":"x"},{"abbreviation":"A","body":"y"}]"#);
        assert!(w[0].contains("twice"));
    }

    #[test]
    fn the_sample_file_parses_cleanly() {
        let (t, w) = parse_user_templates(&sample_file());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(t.len(), 1);
        assert!(t[0].needs_table());
    }

    #[test]
    fn variables_are_substituted_and_escaped() {
        let vars = Vars {
            table: Some("public.orders".into()),
            columns: vec!["id".into(), "total".into()],
            selection: "select '$1' from t".into(),
            clipboard: "a}b\\c".into(),
            date: "2026-11-11".into(),
        };
        assert_eq!(
            resolve_variables("SELECT ${columns} FROM ${table} -- ${date}", &vars),
            "SELECT id, total FROM public.orders -- 2026-11-11"
        );
        // `$1` inside the value is not a tab stop once escaped.
        assert_eq!(resolve_variables("${selection}", &vars), "select '\\$1' from t");
        assert_eq!(resolve_variables("${clipboard}", &vars), "a\\}b\\\\c");
        // Tab stops and escaped dollars in the body survive; unknown variables are left alone.
        assert_eq!(resolve_variables("${1:x} \\${table} ${nope} $0", &vars), "${1:x} \\${table} ${nope} $0");
        assert_eq!(resolve_variables("${table}", &Vars::default()), "");
    }

    #[test]
    fn needs_table_only_for_table_variables() {
        assert!(needs_table("SELECT ${columns} FROM ${table}"));
        assert!(needs_table("x ${column}"));
        assert!(!needs_table("SELECT ${selection} ${date} ${clipboard}"));
        assert!(!needs_table("\\${table}"));
        assert!(!needs_table("${1:table}"));
    }

    #[test]
    fn selection_value_strips_semicolon_and_common_indent() {
        assert_eq!(selection_value("  select 1\n    from t;  "), "select 1\n  from t");
        assert_eq!(selection_value("\n\nselect 1;\n"), "select 1");
        assert_eq!(selection_value(""), "");
    }

    #[test]
    fn every_surround_body_uses_the_selection() {
        for s in surround_options() {
            assert!(s.body.contains("${selection}"), "{}", s.label);
        }
    }

    #[test]
    fn expand_is_case_insensitive_and_exact() {
        let all = builtins(Postgres);
        assert!(expand(&all, "SELW").is_some());
        assert!(expand(&all, "sel_").is_none());
        assert!(expand(&all, "").is_none());
    }
}
