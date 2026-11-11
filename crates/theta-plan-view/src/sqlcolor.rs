//! Syntax colouring for the small SQL fragments a plan carries (filters, index
//! conditions, suggested DDL). Uses the SQL crate's lexer so quoting rules match
//! the editor's, and the theme's syntax palette so it looks like the editor too.

use gpui_kit::{HighlightStyle, Hsla};
use std::ops::Range;
use theta_core::DatabaseKind;
use theta_sql::keywords::is_keyword_any;
use theta_sql::lexer::{TokenKind, tokenize};

/// The colours the highlighter picks from (a subset of `theta_ui::SyntaxColors`
/// so tests can build one without a GPUI app).
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub keyword: Hsla,
    pub type_: Hsla,
    pub function: Hsla,
    pub string: Hsla,
    pub number: Hsla,
    pub comment: Hsla,
    pub operator: Hsla,
    pub punctuation: Hsla,
    pub identifier: Hsla,
}

impl From<theta_ui::SyntaxColors> for Palette {
    fn from(s: theta_ui::SyntaxColors) -> Self {
        Palette {
            keyword: s.keyword,
            type_: s.type_,
            function: s.function,
            string: s.string,
            number: s.number,
            comment: s.comment,
            operator: s.operator,
            punctuation: s.punctuation,
            identifier: s.identifier,
        }
    }
}

/// Ascending, non-overlapping colour runs for `sql`. Text between runs keeps the
/// element's own colour.
pub fn highlights(sql: &str, kind: DatabaseKind, p: &Palette) -> Vec<(Range<usize>, HighlightStyle)> {
    let toks = tokenize(sql, kind);
    let mut out = Vec::with_capacity(toks.len());
    for (i, t) in toks.iter().enumerate() {
        let text = t.text(sql);
        let color = match t.kind {
            TokenKind::Whitespace => continue,
            TokenKind::LineComment | TokenKind::BlockComment => p.comment,
            TokenKind::String | TokenKind::DollarString => p.string,
            TokenKind::Number => p.number,
            TokenKind::Operator => p.operator,
            TokenKind::Punct => p.punctuation,
            TokenKind::Param => p.identifier,
            TokenKind::QuotedIdent => p.identifier,
            TokenKind::Word => {
                let after_cast =
                    i > 0 && toks[i - 1].kind == TokenKind::Operator && toks[i - 1].text(sql) == "::";
                let is_call =
                    toks.get(i + 1).is_some_and(|n| n.kind == TokenKind::Punct && n.text(sql) == "(");
                if after_cast {
                    p.type_
                } else if !is_call && is_keyword_any(text, kind) && !is_type_word(text) {
                    p.keyword
                } else if is_call {
                    p.function
                } else {
                    p.identifier
                }
            }
        };
        out.push((t.range(), HighlightStyle { color: Some(color), ..Default::default() }));
    }
    out
}

/// Words that are both a type name and a plausible column name (`name`, `text`) read
/// better as identifiers outside a cast.
fn is_type_word(w: &str) -> bool {
    theta_sql::keywords::is_type_name(w) && !w.eq_ignore_ascii_case("interval")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::hsla;

    fn pal() -> Palette {
        let c = |h| hsla(h, 1.0, 0.5, 1.0);
        Palette {
            keyword: c(0.1),
            type_: c(0.2),
            function: c(0.3),
            string: c(0.4),
            number: c(0.5),
            comment: c(0.6),
            operator: c(0.7),
            punctuation: c(0.8),
            identifier: c(0.9),
        }
    }

    fn colour_of(sql: &str, needle: &str) -> Hsla {
        let at = sql.find(needle).unwrap();
        highlights(sql, DatabaseKind::Postgres, &pal())
            .into_iter()
            .find(|(r, _)| r.start == at)
            .and_then(|(_, s)| s.color)
            .unwrap_or_else(|| panic!("no colour for {needle}"))
    }

    #[test]
    fn colours_filter_expressions() {
        let sql = "((status)::text = 'open'::text) AND id > 42";
        let p = pal();
        assert_eq!(colour_of(sql, "'open'"), p.string);
        assert_eq!(colour_of(sql, "42"), p.number);
        assert_eq!(colour_of(sql, "AND"), p.keyword);
        assert_eq!(colour_of(sql, "status"), p.identifier);
        // The word after `::` is a type, whatever it is spelled like.
        let at = sql.find("::text").unwrap() + 2;
        let ty = highlights(sql, DatabaseKind::Postgres, &p)
            .into_iter()
            .find(|(r, _)| r.start == at)
            .and_then(|(_, s)| s.color);
        assert_eq!(ty, Some(p.type_));
    }

    #[test]
    fn runs_are_ascending_and_on_char_boundaries() {
        let sql = "(name = 'Zoë''s café') OR lower(city) = 'münchen'";
        let runs = highlights(sql, DatabaseKind::Postgres, &pal());
        let mut end = 0;
        for (r, _) in &runs {
            assert!(r.start >= end && r.end > r.start);
            assert!(sql.is_char_boundary(r.start) && sql.is_char_boundary(r.end));
            end = r.end;
        }
        assert_eq!(colour_of(sql, "lower"), pal().function);
    }

    #[test]
    fn handles_ddl_suggestions() {
        let sql = "CREATE INDEX idx_orders_status ON orders (status);";
        assert_eq!(colour_of(sql, "CREATE"), pal().keyword);
        assert_eq!(colour_of(sql, "orders ("), pal().identifier);
    }
}
