//! Robustness of the syntax tree: no panic, no endless loop, always a tree that tiles the input, and parse
//! time that grows roughly linearly. The inputs are the whole differential corpus (every statement of it),
//! truncated at every token boundary, mutated at random, and made of random token soup.

use std::time::Instant;
use theta_core::DatabaseKind;
use theta_sql::lexer::tokenize;
use theta_sql::syntax::parse;

const KINDS: &[DatabaseKind] = &[
    DatabaseKind::Postgres,
    DatabaseKind::MySql,
    DatabaseKind::Sqlite,
    DatabaseKind::DuckDb,
    DatabaseKind::SqlServer,
    DatabaseKind::ClickHouse,
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn corpus() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut out = Vec::new();
    for f in [
        "common.sql",
        "pg.sql",
        "mysql.sql",
        "sqlite.sql",
        "duckdb.sql",
        "clickhouse.sql",
        "tsql.sql",
        "broken.sql",
    ] {
        let text = std::fs::read_to_string(dir.join(f)).unwrap();
        out.extend(
            text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("--")).map(String::from),
        );
    }
    let extracted = std::fs::read_to_string(dir.join("extracted.sql")).unwrap();
    out.extend(extracted.split("-- ---\n").skip(1).map(|s| s.trim().to_string()));
    out
}

fn check(sql: &str, kind: DatabaseKind) {
    let p = parse(sql, kind);
    if let Err(e) = p.root.check_tiling(sql) {
        panic!("tree does not tile {sql:?} ({kind:?}): {e}");
    }
    for e in &p.errors {
        assert!(e.span.start <= e.span.end && e.span.end <= sql.len(), "bad error span {e:?} for {sql:?}");
        assert!(
            sql.is_char_boundary(e.span.start) && sql.is_char_boundary(e.span.end),
            "error span inside a char"
        );
    }
}

#[test]
fn whole_corpus_tiles_in_every_dialect() {
    let corpus = corpus();
    assert!(corpus.len() > 2500, "corpus unexpectedly small: {}", corpus.len());
    for sql in &corpus {
        for &k in KINDS {
            check(sql, k);
        }
    }
}

#[test]
fn every_truncation_of_every_statement() {
    // Cut at each token boundary (and one byte into each token, which splits strings and words).
    let mut cuts = 0usize;
    for sql in corpus().iter().filter(|s| s.len() < 400) {
        for &k in &[DatabaseKind::Postgres, DatabaseKind::MySql, DatabaseKind::SqlServer] {
            for t in tokenize(sql, k) {
                for at in [t.start, t.start + 1] {
                    if at <= sql.len() && sql.is_char_boundary(at) {
                        check(&sql[..at], k);
                        cuts += 1;
                    }
                }
            }
        }
    }
    eprintln!("{cuts} truncations parsed");
    assert!(cuts > 100_000);
}

#[test]
fn random_deletions_and_insertions() {
    let corpus = corpus();
    let mut rng = Rng(0x5eed_1234_abcd_ef01);
    let junk = [
        "(", ")", ",", ";", "'", "\"", "`", "/*", "--", "select", "from", "where", "on", "join", "::", "[",
        "]", "{", "}", "$1", "$$", "é", "😀", "\\", "case", "end",
    ];
    for _ in 0..40_000 {
        let sql = &corpus[rng.below(corpus.len())];
        let mut s = sql.clone();
        for _ in 0..1 + rng.below(4) {
            if s.is_empty() {
                break;
            }
            let mut at = rng.below(s.len());
            while !s.is_char_boundary(at) {
                at -= 1;
            }
            if rng.below(2) == 0 {
                let mut end = (at + 1 + rng.below(8)).min(s.len());
                while !s.is_char_boundary(end) {
                    end += 1;
                }
                s.replace_range(at..end, "");
            } else {
                s.insert_str(at, junk[rng.below(junk.len())]);
            }
        }
        let k = KINDS[rng.below(KINDS.len())];
        check(&s, k);
    }
}

#[test]
fn random_token_soup() {
    let words = [
        "select",
        "from",
        "where",
        "group",
        "by",
        "order",
        "having",
        "limit",
        "offset",
        "union",
        "all",
        "with",
        "recursive",
        "as",
        "insert",
        "into",
        "values",
        "update",
        "set",
        "delete",
        "merge",
        "using",
        "on",
        "join",
        "left",
        "inner",
        "cross",
        "natural",
        "case",
        "when",
        "then",
        "else",
        "end",
        "and",
        "or",
        "not",
        "in",
        "is",
        "null",
        "like",
        "between",
        "exists",
        "over",
        "partition",
        "filter",
        "create",
        "table",
        "alter",
        "drop",
        "begin",
        "returning",
        "conflict",
        "do",
        "nothing",
        "t",
        "u",
        "x",
        "a.b",
        "1",
        "2.5",
        "'s'",
        "\"q\"",
        "`b`",
        "(",
        ")",
        "(",
        ")",
        ",",
        ",",
        ";",
        "=",
        "+",
        "*",
        "::",
        "[",
        "]",
        "$1",
        "?",
        ":n",
        "@v",
        "{",
        "}",
        "->",
        "||",
        "lateral",
        "distinct",
        "top",
        "go",
        "declare",
        "explain",
        "for",
        "window",
        "cast",
        "interval",
    ];
    let mut rng = Rng(0xfeed_beef_0123_4567);
    for _ in 0..30_000 {
        let n = 1 + rng.below(60);
        let mut s = String::new();
        for _ in 0..n {
            s.push_str(words[rng.below(words.len())]);
            s.push(if rng.below(8) == 0 { '\n' } else { ' ' });
        }
        let k = KINDS[rng.below(KINDS.len())];
        check(&s, k);
    }
}

#[test]
fn deep_nesting_does_not_overflow_the_stack() {
    // The editor parses on a worker thread; use a small stack to prove the depth guard, not luck.
    let handle = std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            let n = 20_000;
            let cases = [
                "(".repeat(n),
                format!("select {}1{}", "(".repeat(n), ")".repeat(n)),
                format!("select {}", "-".repeat(n)),
                format!("select {} 1", "not ".repeat(n)),
                format!("select {}1{}", "f(".repeat(n), ")".repeat(n)),
                format!("{}select 1{}", "select * from (".repeat(n), ")".repeat(n)),
                format!("select {}", "case when ".repeat(n)),
                format!("select 1{}", " + 1".repeat(n)),
                format!("select {}", "a and ".repeat(n)),
                format!("with a as (select 1) {}", "select 1 union ".repeat(n)),
                format!("select {}1{}", "[".repeat(n), "]".repeat(n)),
                format!("select {}x{}", "(select ".repeat(n), ")".repeat(n)),
                format!("select 1 {}", "join t on ".repeat(n)),
            ];
            for (i, sql) in cases.iter().enumerate() {
                for &k in KINDS {
                    eprintln!("deep case {i} {k:?}");
                    check(sql, k);
                }
            }
        })
        .unwrap();
    handle.join().expect("parser overflowed the stack or panicked on deep nesting");
}

#[test]
fn parse_time_grows_linearly() {
    // Repeat a statement n times; parsing 8x the text must cost far less than the 64x a quadratic parser
    // would need. (Generous bound: noisy machines.)
    let unit = "select a, b + 1, count(*) from t join u on t.id = u.id where x in (1, 2, 3) and y like 'z%' group by a, b order by a;\ninsert into t (a, b) values (1, 2) on conflict (a) do update set b = excluded.b;\n";
    let time = |n: usize| {
        let sql = unit.repeat(n);
        let t = Instant::now();
        for _ in 0..3 {
            let p = parse(&sql, DatabaseKind::Postgres);
            assert!(!p.root.children.is_empty());
        }
        t.elapsed().as_secs_f64()
    };
    let _ = time(50);
    let small = time(500).max(1e-4);
    let big = time(4000);
    eprintln!("500 units: {small:.4}s, 4000 units: {big:.4}s, ratio {:.1} (linear = 8)", big / small);
    assert!(big / small < 24.0, "parse time looks super-linear: {small:.4}s -> {big:.4}s");
    // Single statements with long lists must also stay linear.
    let list = |n: usize| {
        format!("select {} from t", (0..n).map(|i| format!("c{i}")).collect::<Vec<_>>().join(", "))
    };
    let a = list(2000);
    let b = list(16000);
    let ta = Instant::now();
    parse(&a, DatabaseKind::Postgres);
    let ta = ta.elapsed().as_secs_f64().max(1e-5);
    let tb = Instant::now();
    parse(&b, DatabaseKind::Postgres);
    let tb = tb.elapsed().as_secs_f64();
    assert!(tb / ta < 24.0, "a long select list is super-linear: {ta:.5}s -> {tb:.5}s");
    let chain = |n: usize| {
        format!(
            "select 1 from t where {}",
            (0..n).map(|i| format!("a{i} = {i}")).collect::<Vec<_>>().join(" and ")
        )
    };
    let (a, b) = (chain(2000), chain(16000));
    let ta = Instant::now();
    parse(&a, DatabaseKind::Postgres);
    let ta = ta.elapsed().as_secs_f64().max(1e-5);
    let tb = Instant::now();
    parse(&b, DatabaseKind::Postgres);
    let tb = tb.elapsed().as_secs_f64();
    assert!(tb / ta < 24.0, "a long AND chain is super-linear: {ta:.5}s -> {tb:.5}s");
}
