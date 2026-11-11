//! Query parameters: finding `?`, `$1`, `:name` and `@name` placeholders and substituting values for them.
//!
//! Substitution is **client-side**: the value is rendered as an escaped SQL literal and spliced into the
//! statement text, and the server then sees an ordinary statement with no parameters. That is deliberate
//! (a console runs scripts, and `Connection::execute` takes one text statement), and it means a parameter
//! can stand wherever a literal can, but not for an identifier, which needs the `Raw SQL` type.
//!
//! Detection is driven by the lexer, so a placeholder inside a string, comment or dollar-quoted body is
//! never seen. What the lexer cannot tell apart is decided here:
//! * `::type` casts and `a[1:n]` slices are not named parameters;
//! * on Postgres `?`, `?|` and `?&` are the jsonb operators, so only `$n` and `:name`/`@name` count there;
//! * on MySQL `@x := 1` and `SET @x = 1` assign a *user variable*, and every `@x` of an assigned name is a
//!   variable rather than a parameter, as is every `@@system_var`.

use crate::lexer::{Token, TokenKind, tokenize};
use std::ops::Range;
use theta_core::Family;
use theta_core::{DatabaseKind, Value};

/// What identifies a placeholder: two occurrences with equal keys receive the same value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ParamKey {
    /// A bare `?`: numbered by position across the whole set of statements.
    Positional(usize),
    /// `$3` or `?3`.
    Numbered(u32),
    /// `:name` or `@name`; the sigil is part of the identity (`:id` and `@id` are different slots).
    Named { sigil: char, name: String },
}

impl ParamKey {
    /// The text shown in the dialog and the log: `?2`, `$1`, `:name`.
    pub fn label(&self) -> String {
        match self {
            ParamKey::Positional(n) => format!("?{n}"),
            ParamKey::Numbered(n) => format!("${n}"),
            ParamKey::Named { sigil, name } => format!("{sigil}{name}"),
        }
    }
}

/// One placeholder in a statement: its byte range and the slot it fills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub range: Range<usize>,
    pub slot: usize,
}

/// The placeholders of one or more statements. `slots` holds each distinct key once, in order of first
/// appearance; `statements[i]` lists the occurrences inside statement `i`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detected {
    pub slots: Vec<ParamKey>,
    pub statements: Vec<Vec<Occurrence>>,
}

impl Detected {
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

/// Detect the placeholders of `statements`. Bare `?` is numbered continuously across statements and
/// `$n` / named keys are shared, so "Run All" can ask once for the union.
pub fn detect(statements: &[&str], kind: DatabaseKind) -> Detected {
    let lexed: Vec<Vec<Token>> = statements.iter().map(|s| tokenize(s, kind)).collect();
    let assigned = if session_vars(kind) { assigned_variables(statements, &lexed) } else { Vec::new() };
    let mut out = Detected::default();
    let mut positional = 0usize;
    for (sql, tokens) in statements.iter().zip(&lexed) {
        let mut found = Vec::new();
        for (range, key) in candidates(sql, tokens, kind, &assigned) {
            let key = match key {
                Raw::Positional => {
                    positional += 1;
                    ParamKey::Positional(positional)
                }
                Raw::Key(k) => k,
            };
            let slot = match out.slots.iter().position(|s| *s == key) {
                Some(i) => i,
                None => {
                    out.slots.push(key);
                    out.slots.len() - 1
                }
            };
            found.push(Occurrence { range, slot });
        }
        out.statements.push(found);
    }
    out
}

enum Raw {
    Positional,
    Key(ParamKey),
}

/// MySQL's `@x` and T-SQL's `@x` (after DECLARE or SET) are session variables, not parameters to ask for.
fn session_vars(kind: DatabaseKind) -> bool {
    matches!(kind.family(), Family::MySql | Family::SqlServer)
}

fn candidates(
    sql: &str,
    tokens: &[Token],
    kind: DatabaseKind,
    assigned: &[String],
) -> Vec<(Range<usize>, Raw)> {
    let mut out = Vec::new();
    let in_set =
        session_vars(kind) && (first_word_is(sql, tokens, "set") || first_word_is(sql, tokens, "declare"));
    let significant: Vec<usize> = (0..tokens.len()).filter(|&i| !tokens[i].is_trivia()).collect();
    for (pos, &i) in significant.iter().enumerate() {
        let t = tokens[i];
        let text = t.text(sql);
        match t.kind {
            TokenKind::Param => {
                if let Some(rest) = text.strip_prefix('$') {
                    if let Ok(n) = rest.parse::<u32>() {
                        out.push((t.range(), Raw::Key(ParamKey::Numbered(n))));
                    }
                } else if let Some(rest) = text.strip_prefix('?') {
                    if kind.family().pg_like() {
                        continue;
                    }
                    out.push((
                        t.range(),
                        match rest.parse::<u32>() {
                            Ok(n) => Raw::Key(ParamKey::Numbered(n)),
                            Err(_) => Raw::Positional,
                        },
                    ));
                } else if let Some(name) = text.strip_prefix(':') {
                    out.push((t.range(), Raw::Key(ParamKey::Named { sigil: ':', name: name.into() })));
                } else if let Some(name) = text.strip_prefix('@') {
                    let plain = !name.is_empty()
                        && name
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80);
                    if !plain || (session_vars(kind) && (in_set || assigned.iter().any(|a| a == name))) {
                        continue;
                    }
                    out.push((t.range(), Raw::Key(ParamKey::Named { sigil: '@', name: name.into() })));
                }
            }
            // Postgres has no `@name` token: `@` is the absolute-value operator. It is a parameter only
            // when glued to the following word and not in operand position (`a @b` is not valid SQL for
            // abs, and `x =@id` is how people write it).
            TokenKind::Operator
                if kind.family().pg_like() && text.ends_with('@') && !text.ends_with("@@") =>
            {
                let Some(&next) = significant.get(pos + 1).map(|&j| &tokens[j]) else { continue };
                if next.kind == TokenKind::Word && next.start == t.end {
                    let prev_operand =
                        pos > 0 && text.len() == 1 && ends_operand(sql, &tokens[significant[pos - 1]]);
                    if !prev_operand {
                        let start = t.end - 1;
                        out.push((
                            start..next.end,
                            Raw::Key(ParamKey::Named { sigil: '@', name: next.text(sql).into() }),
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether `t` can end an operand, i.e. a following `@` would be a binary operator, not a placeholder.
fn ends_operand(sql: &str, t: &Token) -> bool {
    match t.kind {
        TokenKind::Word => !is_clause_keyword(t.text(sql)),
        TokenKind::Number
        | TokenKind::String
        | TokenKind::QuotedIdent
        | TokenKind::DollarString
        | TokenKind::Param => true,
        TokenKind::Punct => matches!(t.text(sql), ")" | "]"),
        _ => false,
    }
}

fn is_clause_keyword(w: &str) -> bool {
    [
        "where", "and", "or", "not", "select", "on", "when", "then", "else", "in", "limit", "offset", "by",
        "having", "set", "values",
    ]
    .iter()
    .any(|k| w.eq_ignore_ascii_case(k))
}

fn first_word_is(sql: &str, tokens: &[Token], word: &str) -> bool {
    tokens.iter().find(|t| !t.is_trivia()).is_some_and(|t| t.is_kw(sql, word))
}

/// MySQL user variables that the script assigns (`@x := …`, `SET @x = …`, `SELECT … INTO @x`).
fn assigned_variables(statements: &[&str], lexed: &[Vec<Token>]) -> Vec<String> {
    let mut names = Vec::new();
    for (sql, tokens) in statements.iter().zip(lexed) {
        let is_set = first_word_is(sql, tokens, "set") || first_word_is(sql, tokens, "declare");
        let sig: Vec<&Token> = tokens.iter().filter(|t| !t.is_trivia()).collect();
        for (i, t) in sig.iter().enumerate() {
            if t.kind != TokenKind::Param {
                continue;
            }
            let Some(name) = t.text(sql).strip_prefix('@').filter(|n| !n.is_empty() && !n.starts_with('@'))
            else {
                continue;
            };
            let walrus = sql[t.end..].trim_start().starts_with(":=");
            let into = i > 0 && sig[i - 1].is_kw(sql, "into");
            if (walrus || is_set || into) && !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// How a typed value is turned into SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParamType {
    /// Numbers stay numbers, `true`/`false` booleans, everything else is a quoted string.
    #[default]
    Auto,
    Text,
    Number,
    Boolean,
    Null,
    /// Inserted verbatim: for identifiers, expressions and sub-queries. The user is responsible for it.
    Raw,
}

impl ParamType {
    pub const ALL: [ParamType; 6] = [
        ParamType::Auto,
        ParamType::Text,
        ParamType::Number,
        ParamType::Boolean,
        ParamType::Null,
        ParamType::Raw,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ParamType::Auto => "Auto",
            ParamType::Text => "Text",
            ParamType::Number => "Number",
            ParamType::Boolean => "Boolean",
            ParamType::Null => "NULL",
            ParamType::Raw => "Raw SQL",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParamValue {
    pub ty: ParamType,
    pub text: String,
}

fn is_number(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let s = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i - s
    };
    let int = digits(&mut i);
    let mut frac = 0;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        frac = digits(&mut i);
    }
    if int + frac == 0 {
        return false;
    }
    if i < b.len() && matches!(b[i], b'e' | b'E') {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return false;
        }
    }
    i == b.len()
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "t" | "yes" | "y" | "on" | "1" => Some(true),
        "false" | "f" | "no" | "n" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn number_value(s: &str) -> Value {
    match s.parse::<i64>() {
        Ok(i) => Value::Int(i),
        // `sql_literal` emits a Decimal verbatim only when it is plain digits; anything with an
        // exponent is quoted there, so those are emitted by the caller.
        Err(_) => Value::Decimal(s.to_string()),
    }
}

/// The SQL text for one value, or the reason it is not valid for its type.
pub fn literal(v: &ParamValue, kind: DatabaseKind) -> Result<String, String> {
    let t = v.text.trim();
    match v.ty {
        ParamType::Null => Ok(crate::sql_literal(&Value::Null, kind)),
        ParamType::Raw => {
            if t.is_empty() {
                Err("Raw SQL is empty".into())
            } else {
                Ok(v.text.clone())
            }
        }
        ParamType::Text => Ok(crate::sql_literal(&Value::Text(v.text.clone()), kind)),
        ParamType::Number => {
            if !is_number(t) {
                return Err(format!("“{t}” is not a number"));
            }
            Ok(number_sql(t, kind))
        }
        ParamType::Boolean => parse_bool(t)
            .map(|b| crate::sql_literal(&Value::Bool(b), kind))
            .ok_or_else(|| format!("“{t}” is not a boolean (true/false, 1/0, yes/no)")),
        ParamType::Auto => {
            if is_number(t) {
                Ok(number_sql(t, kind))
            } else if t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("false") {
                Ok(crate::sql_literal(&Value::Bool(t.eq_ignore_ascii_case("true")), kind))
            } else {
                Ok(crate::sql_literal(&Value::Text(v.text.clone()), kind))
            }
        }
    }
}

fn number_sql(t: &str, kind: DatabaseKind) -> String {
    let t = t.strip_prefix('+').unwrap_or(t);
    // A leading zero (`007`) is a number to the user; keep what they typed except for the bare sign.
    match number_value(t) {
        Value::Int(i) => crate::sql_literal(&Value::Int(i), kind),
        _ if t.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-')) => {
            crate::sql_literal(&Value::Decimal(t.to_string()), kind)
        }
        // Exponent form (`1e5`): valid verbatim in all three dialects.
        _ => t.to_string(),
    }
}

/// Replace every occurrence in `sql` with the literal of `values[occurrence.slot]`.
pub fn substitute(
    sql: &str,
    occurrences: &[Occurrence],
    slots: &[ParamKey],
    values: &[ParamValue],
    kind: DatabaseKind,
) -> Result<String, String> {
    let mut out = sql.to_string();
    for o in occurrences.iter().rev() {
        let value = values.get(o.slot).ok_or_else(|| "missing value".to_string())?;
        let lit = literal(value, kind).map_err(|e| format!("{}: {e}", slots[o.slot].label()))?;
        out.replace_range(o.range.clone(), &lit);
    }
    Ok(out)
}

/// One line per slot for the Messages log: `:id = 42 (Number)`.
pub fn describe(slots: &[ParamKey], values: &[ParamValue]) -> Vec<String> {
    slots
        .iter()
        .zip(values)
        .map(|(k, v)| match v.ty {
            ParamType::Null => format!("{} = NULL", k.label()),
            ParamType::Auto => format!("{} = {}", k.label(), v.text),
            ty => format!("{} = {} ({})", k.label(), v.text, ty.label()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PG: DatabaseKind = DatabaseKind::Postgres;
    const MY: DatabaseKind = DatabaseKind::MySql;
    const SL: DatabaseKind = DatabaseKind::Sqlite;

    fn labels(sql: &str, kind: DatabaseKind) -> Vec<String> {
        detect(&[sql], kind).slots.iter().map(ParamKey::label).collect()
    }

    #[test]
    fn positional_mysql_and_sqlite() {
        assert_eq!(labels("select * from t where a = ? and b = ?", MY), ["?1", "?2"]);
        assert_eq!(labels("select ?1, ?2, ?1", SL), ["$1", "$2"]);
    }

    #[test]
    fn numbered_postgres() {
        let d = detect(&["select $1, $2, $1"], PG);
        assert_eq!(d.slots, [ParamKey::Numbered(1), ParamKey::Numbered(2)]);
        assert_eq!(d.statements[0].len(), 3);
        assert_eq!(d.statements[0][2].slot, 0);
    }

    #[test]
    fn named_colon_and_at() {
        assert_eq!(labels("select :id, @name, :id", PG), [":id", "@name"]);
        assert_eq!(labels("select :id, @name", SL), [":id", "@name"]);
        assert_eq!(labels("select :id", MY), [":id"]);
        assert_eq!(labels("select * from t where a >=@lo", PG), ["@lo"]);
    }

    #[test]
    fn false_positives_strings_comments_dollar_bodies() {
        assert!(labels("select '?', ':id', '$1', '@x'", PG).is_empty());
        assert!(labels("select 1 -- :id ? $1\n", PG).is_empty());
        assert!(labels("select 1 /* :id ? @x */", MY).is_empty());
        assert!(
            labels("create function f() returns int as $$ select $1 + :x $$ language sql", PG).is_empty()
        );
        assert!(labels("select \"?\" from t", PG).is_empty());
        assert!(labels("select `:a` from t", MY).is_empty());
        assert!(labels("select E'it\\'s :id' ", PG).is_empty());
    }

    #[test]
    fn false_positives_postgres_casts_and_slices() {
        assert!(labels("select a::int, b::text, c :: date from t", PG).is_empty());
        assert!(labels("select arr[1:n], arr[i:j] from t", PG).is_empty());
        assert!(labels("select x::numeric(10,2)", PG).is_empty());
        assert_eq!(labels("select :a::int", PG), [":a"]);
    }

    #[test]
    fn false_positives_postgres_json_operators() {
        assert!(labels("select * from t where doc ? 'k'", PG).is_empty());
        assert!(labels("select * from t where doc ?| array['a','b']", PG).is_empty());
        assert!(labels("select * from t where doc ?& array['a','b']", PG).is_empty());
        assert!(labels("select doc @> '{}'::jsonb from t", PG).is_empty());
        assert!(labels("select * from t where a @@ b", PG).is_empty());
        assert!(labels("select @ -5, a @ b", PG).is_empty());
    }

    #[test]
    fn false_positives_mysql_user_variables() {
        assert!(labels("select @x := 1", MY).is_empty());
        assert!(labels("select @x := @x + 1", MY).is_empty());
        assert!(labels("set @a = 1, @b = @a", MY).is_empty());
        assert!(labels("select @@version, @@global.max_connections", MY).is_empty());
        // Assigned in an earlier statement of the same run: a variable, not a parameter.
        let d = detect(&["set @n = 5", "select * from t limit @n", "select :k"], MY);
        assert_eq!(d.slots, [ParamKey::Named { sigil: ':', name: "k".into() }]);
        // An unassigned `@name` is a parameter.
        assert_eq!(labels("select * from t limit @n", MY), ["@n"]);
        assert_eq!(labels("select x into @out from t where a = @n", MY), ["@n"]);
    }

    #[test]
    fn walrus_is_not_a_named_parameter() {
        assert!(labels("select @a:=1", MY).is_empty());
    }

    #[test]
    fn union_across_statements() {
        let d = detect(&["select ?, :a", "select ?, :a, :b"], MY);
        let l: Vec<_> = d.slots.iter().map(ParamKey::label).collect();
        assert_eq!(l, ["?1", ":a", "?2", ":b"]);
        assert_eq!(d.statements[1].iter().map(|o| o.slot).collect::<Vec<_>>(), [2, 1, 3]);
    }

    fn run(sql: &str, kind: DatabaseKind, vals: &[(ParamType, &str)]) -> Result<String, String> {
        let d = detect(&[sql], kind);
        let values: Vec<ParamValue> =
            vals.iter().map(|(ty, t)| ParamValue { ty: *ty, text: t.to_string() }).collect();
        substitute(sql, &d.statements[0], &d.slots, &values, kind)
    }

    #[test]
    fn substitutes_with_escaping() {
        assert_eq!(
            run(
                "select * from t where n = :n and id = :id",
                PG,
                &[(ParamType::Auto, "O'Brien"), (ParamType::Auto, "42")]
            )
            .unwrap(),
            "select * from t where n = 'O''Brien' and id = 42"
        );
        assert_eq!(run("select ?", MY, &[(ParamType::Text, "a\\b'c")]).unwrap(), "select 'a\\\\b''c'");
        assert_eq!(run("select ?", SL, &[(ParamType::Boolean, "yes")]).unwrap(), "select 1");
        assert_eq!(run("select $1", PG, &[(ParamType::Boolean, "no")]).unwrap(), "select FALSE");
        assert_eq!(run("select :x", PG, &[(ParamType::Null, "ignored")]).unwrap(), "select NULL");
        assert_eq!(
            run("select * from :t", PG, &[(ParamType::Raw, "public.items")]).unwrap(),
            "select * from public.items"
        );
    }

    #[test]
    fn repeated_slot_gets_one_value_everywhere() {
        assert_eq!(run("select $1, $1", PG, &[(ParamType::Number, "7")]).unwrap(), "select 7, 7");
    }

    #[test]
    fn auto_infers() {
        let v = |s: &str| literal(&ParamValue { ty: ParamType::Auto, text: s.into() }, PG).unwrap();
        assert_eq!(v("12"), "12");
        assert_eq!(v("-3.5"), "-3.5");
        assert_eq!(v("1e3"), "1e3");
        assert_eq!(v("TRUE"), "TRUE");
        assert_eq!(v("abc"), "'abc'");
        assert_eq!(v(""), "''");
        assert_eq!(v("12abc"), "'12abc'");
        assert_eq!(v("2024-01-01"), "'2024-01-01'");
    }

    #[test]
    fn typed_values_are_validated() {
        assert!(literal(&ParamValue { ty: ParamType::Number, text: "1; drop table t".into() }, PG).is_err());
        assert!(literal(&ParamValue { ty: ParamType::Boolean, text: "maybe".into() }, PG).is_err());
        assert!(literal(&ParamValue { ty: ParamType::Raw, text: "  ".into() }, PG).is_err());
        let e = run("select :a", PG, &[(ParamType::Number, "x")]).unwrap_err();
        assert!(e.starts_with(":a:"), "{e}");
    }

    #[test]
    fn injection_stays_inside_the_literal() {
        assert_eq!(
            run("select :a", PG, &[(ParamType::Auto, "x'; drop table t; --")]).unwrap(),
            "select 'x''; drop table t; --'"
        );
    }

    #[test]
    fn describe_lists_values() {
        let d = detect(&["select :a, :b"], PG);
        let v = vec![
            ParamValue { ty: ParamType::Auto, text: "1".into() },
            ParamValue { ty: ParamType::Null, text: String::new() },
        ];
        assert_eq!(describe(&d.slots, &v), [":a = 1", ":b = NULL"]);
    }
}
