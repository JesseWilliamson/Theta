//! Property-style robustness: every public entry point must survive half-typed,
//! truncated, mutated and non-ASCII input without panicking, and the formatter must
//! stay idempotent on all of it.

use quarry_core::{DatabaseKind, Value};
use quarry_sql::fixtures;
use quarry_sql::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*: deterministic, so a failure is reproducible from the seed.
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const SCRIPTS: &[(&str, DatabaseKind)] = &[
    (
        "-- customers report\nselect c.id, c.name as \"Näme\", count(o.id) as n, sum(o.total)::numeric(10,2) /* 日本語 */\nfrom customers c left join orders o on o.customer_id = c.id\nwhere c.tier in ('gold', 'it''s') and c.created_at > now() - interval '30 days'\ngroup by c.id, c.name having count(*) > 1 order by n desc limit 10;\n",
        DatabaseKind::Postgres,
    ),
    (
        "with recent as (select id, total from orders where placed_at > now()), big as (select * from recent where total > 100)\nselect r.id, case when r.total > 1 then 'é' else '😀' end from recent r join big b on b.id = r.id;\ninsert into customers (id, email) values (1, 'a@b.c'), (2, $$x;y$$) on conflict (id) do update set email = excluded.email returning id;\n",
        DatabaseKind::Postgres,
    ),
    (
        "create function f(a int) returns int as $fn$ begin update t set a = 1; return a; end; $fn$ language plpgsql;\nupdate orders o set status = 'paid' from customers c where o.customer_id = c.id;\ndelete from orders where id in (select order_id from order_items where quantity = 0);\nselect data->>'k', arr[1], x::int, E'a\\'b' from t where a is distinct from null;",
        DatabaseKind::Postgres,
    ),
    (
        "select `order`.id, ifnull(a, 0) from `order` where name = \"x\\\"y\" # note; here\nlimit 5, 10;\ninsert into t (a, b) values (1, 2) on duplicate key update b = values(b);\ncreate procedure p() begin select 1; if x then update t set a = 1; end if; end;\nselect 1;",
        DatabaseKind::MySql,
    ),
    (
        "select [my col], ?1, :name, @v from t where x glob 'a*';\ncreate trigger tr after insert on t begin update u set a = 1; insert into log values (1); end;\ninsert or replace into t (a) values (x'ff');\nselect * from t /* open",
        DatabaseKind::Sqlite,
    ),
];

/// Formatting may change whitespace and keyword case, never the tokens themselves.
fn assert_tokens_preserved(input: &str, output: &str, kind: DatabaseKind) {
    use quarry_sql::lexer::{TokenKind, tokenize};
    let sig = |s: &str| -> Vec<(TokenKind, String)> {
        tokenize(s, kind)
            .into_iter()
            .filter(|t| t.kind != TokenKind::Whitespace)
            .map(|t| (t.kind, t.text(s).to_string()))
            .collect()
    };
    let (a, b) = (sig(input), sig(output));
    assert_eq!(a.len(), b.len(), "token count changed for {input:?} -> {output:?}");
    for ((ka, ta), (kb, tb)) in a.iter().zip(&b) {
        assert_eq!(ka, kb, "token kind changed for {input:?} -> {output:?}");
        assert!(
            ta == tb || (*ka == TokenKind::Word && ta.eq_ignore_ascii_case(tb)),
            "token {ta:?} became {tb:?} in {input:?} -> {output:?}"
        );
    }
}

fn catalog() -> quarry_core::Catalog {
    fixtures::shop()
}

fn exercise(sql: &str, kind: DatabaseKind, rng: &mut Rng, cat: &quarry_core::Catalog) {
    let _ = split_statements(sql, kind);
    let _ = classify(sql, kind);
    let _ = tables_in_scope(sql, kind);
    let _ = lint(sql, kind, None);
    let _ = lint(sql, kind, Some(cat));
    for _ in 0..3 {
        let off = rng.below(sql.len() + 3);
        let _ = statement_at(sql, off, kind);
        let list = complete(&CompletionRequest { sql, offset: off, kind, catalog: Some(cat) });
        assert!(list.replace_range.start <= list.replace_range.end && list.replace_range.end <= sql.len());
        assert!(
            sql.is_char_boundary(list.replace_range.start) && sql.is_char_boundary(list.replace_range.end)
        );
        assert!(list.items.len() <= 200);
        let _ = complete(&CompletionRequest { sql, offset: off, kind, catalog: None });
    }
    let refs = tables_in_scope(sql, kind);
    let _ = suggest_joins(cat, &refs, kind);
    for opts in [
        FormatOptions::default(),
        FormatOptions { indent_width: 2, uppercase_keywords: false, max_line_width: 30 },
    ] {
        let once = format(sql, kind, &opts);
        let twice = format(&once, kind, &opts);
        assert_eq!(once, twice, "format not idempotent for {sql:?}");
        assert_tokens_preserved(sql, &once, kind);
    }
}

#[test]
fn byte_truncated_prefixes_never_panic() {
    let cat = catalog();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for (script, kind) in SCRIPTS {
        let bytes = script.as_bytes();
        for end in 0..=bytes.len() {
            // Arbitrary byte cut: lossy decoding yields valid UTF-8 containing U+FFFD.
            let lossy = String::from_utf8_lossy(&bytes[..end]).into_owned();
            exercise(&lossy, *kind, &mut rng, &cat);
            // Character-boundary cut: the exact prefix the editor would hold mid-typing.
            if script.is_char_boundary(end) {
                exercise(&script[..end], *kind, &mut rng, &cat);
            }
        }
    }
}

#[test]
fn random_mutations_never_panic() {
    let cat = catalog();
    let mut rng = Rng(42);
    // Raise QUARRY_FUZZ_ITERS for a longer soak run.
    let iterations: usize =
        std::env::var("QUARRY_FUZZ_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(120);
    let junk = [
        "'", "\"", "`", "(", ")", ";", "--", "/*", "*/", "$$", "$a$", "\\", ",", ".", "::", "é", "日", "😀",
        " ", "\n", "E'", "[", "]",
    ];
    for (script, kind) in SCRIPTS {
        for _ in 0..iterations {
            let mut s = (*script).to_string();
            for _ in 0..1 + rng.below(4) {
                let mut at = rng.below(s.len() + 1);
                while !s.is_char_boundary(at) {
                    at -= 1;
                }
                if rng.below(3) == 0 && at < s.len() {
                    let ch = s[at..].chars().next().map_or(0, |c| c.len_utf8());
                    s.replace_range(at..at + ch, "");
                } else {
                    s.insert_str(at, junk[rng.below(junk.len())]);
                }
            }
            exercise(&s, *kind, &mut rng, &cat);
        }
    }
}

#[test]
fn degenerate_inputs() {
    let cat = catalog();
    let mut rng = Rng(7);
    for s in [
        "",
        " ",
        ";",
        ";;;",
        "\0",
        "\u{feff}select 1",
        "((((((((",
        "))))))",
        "'",
        "\"",
        "$",
        "$$",
        "/*",
        "--",
        "select\u{0}1",
        "\r\n\r\n",
    ] {
        for kind in DatabaseKind::ALL {
            exercise(s, *kind, &mut rng, &cat);
        }
    }
    // Very deep nesting must not overflow the stack.
    let deep = format!("select {}1{}", "(".repeat(200), ")".repeat(200));
    exercise(&deep, DatabaseKind::Postgres, &mut rng, &cat);
}

#[test]
fn generators_accept_any_value() {
    let values = [
        Value::Null,
        Value::Bool(true),
        Value::Int(i64::MIN),
        Value::UInt(u64::MAX),
        Value::Float(f64::NAN),
        Value::Float(f64::INFINITY),
        Value::Decimal("1e400".into()),
        Value::Text("it's \\ \0 \u{1F600}".into()),
        Value::Bytes(vec![]),
        Value::Json("{".into()),
        Value::Array(vec![Value::Array(vec![Value::Null])]),
        Value::Geometry(vec![1, 2, 3]),
    ];
    for kind in DatabaseKind::ALL {
        for v in &values {
            let lit = sql_literal(v, *kind);
            assert!(!lit.is_empty());
            // A generated literal must always lex as a single balanced expression.
            assert!(
                lint(&format!("select {lit}"), *kind, None).iter().all(|d| !d
                    .message
                    .contains("Unterminated")
                    && !d.message.contains("Unclosed")
                    && !d.message.contains("Unmatched")),
                "{lit}"
            );
        }
        let _ = build_update(
            *kind,
            Some("s"),
            "t",
            &[("a".into(), values[7].clone())],
            &[("k".into(), Value::Null)],
        );
        let _ = build_insert(*kind, None, "t", &["a".into()], &[vec![values[10].clone()]]);
        let _ = build_delete(*kind, None, "t", &[]);
        let _ = select_top(*kind, None, "t", 0);
    }
}

#[test]
fn table_ddl_and_export_do_not_panic() {
    let cat = catalog();
    for s in &cat.schemas {
        for t in &s.tables {
            for kind in DatabaseKind::ALL {
                let ddl = table_ddl(*kind, &s.name, t);
                assert!(!ddl.is_empty());
            }
        }
    }
    let rs = quarry_core::ResultSet::default();
    for f in [
        ExportFormat::Csv,
        ExportFormat::Tsv,
        ExportFormat::Json,
        ExportFormat::Markdown,
        ExportFormat::SqlInserts { table: "t".into() },
    ] {
        let _ = export(&rs, &f, DatabaseKind::Postgres);
    }
}
