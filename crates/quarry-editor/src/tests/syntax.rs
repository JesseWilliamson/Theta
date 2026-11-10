use super::*;
use crate::{HighlightKind as K, HighlightSpan};

fn kinds(e: &mut Editor, line: usize) -> Vec<(String, K)> {
    let text = e.line(line);
    let cs: Vec<char> = text.chars().collect();
    e.highlights(line).into_iter().map(|s: HighlightSpan| (cs[s.range].iter().collect(), s.kind)).collect()
}

fn ed(text: &str) -> Editor {
    Editor::new(text, Keymap::Helix)
}

#[test]
fn keywords_functions_numbers_strings() {
    let mut e = ed("SELECT count(*), 'a''b', 42, 3.14e-2 FROM t WHERE x IS NULL;");
    let k = kinds(&mut e, 0);
    assert!(k.contains(&("SELECT".into(), K::Keyword)));
    assert!(k.contains(&("count".into(), K::Function)));
    assert!(k.contains(&("'a''b'".into(), K::String)));
    assert!(k.contains(&("42".into(), K::Number)));
    assert!(k.contains(&("3.14e-2".into(), K::Number)));
    assert!(k.contains(&("FROM".into(), K::Keyword)));
    assert!(k.contains(&("t".into(), K::Identifier)));
    assert!(k.contains(&("NULL".into(), K::Constant)));
    assert!(k.contains(&("*".into(), K::Operator)));
    assert!(k.contains(&(";".into(), K::Punctuation)));
}

#[test]
fn types_and_casts() {
    let mut e = ed("create table t (id int primary key, n varchar(10)); select x::text, y::my_type;");
    let k = kinds(&mut e, 0);
    assert!(k.contains(&("int".into(), K::Type)));
    assert!(k.contains(&("varchar".into(), K::Type)));
    assert!(k.contains(&("text".into(), K::Type)));
    assert!(k.contains(&("my_type".into(), K::Type)));
    assert!(k.contains(&("::".into(), K::Operator)));
}

#[test]
fn comments_line_and_nested_block() {
    let mut e =
        ed("select 1 -- trailing\n/* a /* b */ still */ select 2\n/* open\nstill comment\n*/ select 3\n");
    assert!(kinds(&mut e, 0).contains(&("-- trailing".into(), K::Comment)));
    let k1 = kinds(&mut e, 1);
    assert_eq!(k1[0], ("/* a /* b */ still */".into(), K::Comment));
    assert!(k1.contains(&("select".into(), K::Keyword)));
    assert_eq!(kinds(&mut e, 2), vec![("/* open".into(), K::Comment)]);
    assert_eq!(kinds(&mut e, 3), vec![("still comment".into(), K::Comment)]);
    let k4 = kinds(&mut e, 4);
    assert_eq!(k4[0], ("*/".into(), K::Comment));
    assert!(k4.contains(&("select".into(), K::Keyword)));
}

#[test]
fn dollar_quoted_strings_span_lines() {
    let mut e = ed("select $$ multi\nselect not keyword\n$$ || 'x';\nselect $tag$ a $$ b $tag$;\n");
    assert_eq!(kinds(&mut e, 1), vec![("select not keyword".into(), K::String)]);
    let k2 = kinds(&mut e, 2);
    assert_eq!(k2[0], ("$$".into(), K::String));
    assert!(k2.contains(&("'x'".into(), K::String)));
    let k3 = kinds(&mut e, 3);
    assert!(k3.contains(&("$tag$ a $$ b $tag$".into(), K::String)));
}

#[test]
fn multi_line_single_quotes() {
    let mut e = ed("select 'abc\nselect still string\n' from t");
    assert_eq!(kinds(&mut e, 1), vec![("select still string".into(), K::String)]);
    let k = kinds(&mut e, 2);
    assert_eq!(k[0], ("'".into(), K::String));
    assert!(k.contains(&("from".into(), K::Keyword)));
}

#[test]
fn parameters() {
    let mut e = ed("select * from t where a = $1 and b = :name and c = ? and d = @v and e @> f");
    let k = kinds(&mut e, 0);
    assert!(k.contains(&("$1".into(), K::Parameter)));
    assert!(k.contains(&(":name".into(), K::Parameter)));
    assert!(k.contains(&("?".into(), K::Parameter)));
    assert!(k.contains(&("@v".into(), K::Parameter)));
    assert!(k.contains(&("@>".into(), K::Operator)));
}

#[test]
fn quoted_identifiers_and_qualified_names() {
    let mut e = ed("select \"Odd Name\", t.order, `x` from public.users");
    let k = kinds(&mut e, 0);
    assert!(k.contains(&("\"Odd Name\"".into(), K::Identifier)));
    assert!(k.contains(&("order".into(), K::Identifier)), "{k:?}");
    assert!(k.contains(&("`x`".into(), K::Identifier)));
    assert!(k.contains(&("users".into(), K::Identifier)));
}

#[test]
fn half_typed_sql_still_highlights() {
    let mut e = ed("SELEC");
    assert_eq!(kinds(&mut e, 0), vec![("SELEC".into(), K::Identifier)]);
    let mut e = ed("select * fro");
    let k = kinds(&mut e, 0);
    assert!(k.contains(&("select".into(), K::Keyword)));
    let mut e = ed("select * from users where name = 'unterminated");
    let k = kinds(&mut e, 0);
    assert!(k.contains(&("from".into(), K::Keyword)));
    assert!(k.contains(&("where".into(), K::Keyword)));
    let mut e = ed("select ( from");
    let k = kinds(&mut e, 0);
    assert!(k.contains(&("from".into(), K::Keyword)));
}

#[test]
fn highlighting_follows_edits_incrementally() {
    let mut e = Editor::new("select 1;\nselect 2;\nselect 3;\n", Keymap::Standard);
    assert!(kinds(&mut e, 2).contains(&("select".into(), K::Keyword)));
    // open a block comment on line 0: later lines turn into comments
    e.replace_range(0..0, "/* ");
    assert_eq!(kinds(&mut e, 2), vec![("select 3;".into(), K::Comment)]);
    // close it again
    e.replace_range(0..3, "");
    assert!(kinds(&mut e, 2).contains(&("select".into(), K::Keyword)));
    // undo/redo keep it consistent too
    e.replace_range(0..0, "'");
    assert_eq!(kinds(&mut e, 1)[0].1, K::String);
    e.undo();
    assert_eq!(kinds(&mut e, 1)[0].1, K::Keyword);
}

#[test]
fn spans_are_sorted_non_overlapping_and_in_bounds() {
    let src = "SELECT a.b, count(*) FILTER (WHERE x > 1e3) AS \"q\" /* c */ FROM t -- end\n$$x$$ 'y' ::int";
    let mut e = ed(src);
    for line in 0..e.line_count() {
        let len = e.line(line).chars().count();
        let spans = e.highlights(line);
        let mut last = 0;
        for s in &spans {
            assert!(s.range.start >= last && s.range.end > s.range.start && s.range.end <= len, "{spans:?}");
            last = s.range.end;
        }
    }
}

#[test]
fn unicode_offsets_are_chars() {
    let mut e = ed("select 'héllo', 日本 from t");
    let text = e.line(0);
    let k = e.highlights(0);
    let s = k.iter().find(|s| s.kind == K::String).unwrap();
    assert_eq!(text.chars().skip(s.range.start).take(s.range.len()).collect::<String>(), "'héllo'");
}

#[test]
fn highlighting_is_fast_on_a_big_buffer() {
    let line = "SELECT a, b, count(*) FROM some_table WHERE id = 42 AND name LIKE 'x%' -- c\n";
    let text = line.repeat(20_000);
    let mut e = Editor::new(&text, Keymap::Standard);
    let t = std::time::Instant::now();
    for l in 19_900..20_000 {
        e.highlights(l);
    }
    let first = t.elapsed();
    // an edit near the end and re-highlighting the viewport must be cheap
    let at = e.line_to_char(19_950);
    e.replace_range(at..at, "x");
    let t = std::time::Instant::now();
    for l in 19_900..20_000 {
        e.highlights(l);
    }
    assert!(
        t.elapsed().as_millis() < 100,
        "re-highlight after edit: {:?} (first pass {first:?})",
        t.elapsed()
    );
}
