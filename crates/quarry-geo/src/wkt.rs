//! WKT / EWKT parsing and writing.
//!
//! The parser is a small hand-written recursive descent over the raw bytes so that
//! errors can carry exact offsets and malformed input never panics. It is tolerant
//! where PostGIS and other tools differ: keywords are case-insensitive, whitespace is
//! flexible, `Z`/`M`/`ZM` may be glued to the keyword (`POINTZ`) or separate
//! (`POINT Z`), and `MULTIPOINT` accepts both `(1 2, 3 4)` and `((1 2),(3 4))`.

use crate::error::GeoError;
use crate::geometry::{Coord, Feature, Geometry};

const MAX_DEPTH: usize = 32;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Point,
    LineString,
    Polygon,
    MultiPoint,
    MultiLineString,
    MultiPolygon,
    Collection,
}

/// Map a keyword such as `POINTZM` to its kind. Dimension suffixes carry no information
/// we keep (Z/M are dropped), so they are only validated.
fn classify(word: &str) -> Option<Kind> {
    const NAMES: [(&str, Kind); 7] = [
        ("POINT", Kind::Point),
        ("LINESTRING", Kind::LineString),
        ("POLYGON", Kind::Polygon),
        ("MULTIPOINT", Kind::MultiPoint),
        ("MULTILINESTRING", Kind::MultiLineString),
        ("MULTIPOLYGON", Kind::MultiPolygon),
        ("GEOMETRYCOLLECTION", Kind::Collection),
    ];
    let up = word.to_ascii_uppercase();
    NAMES.iter().find_map(|(name, kind)| {
        let rest = up.strip_prefix(name)?;
        matches!(rest, "" | "Z" | "M" | "ZM").then_some(*kind)
    })
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

type Res<T> = Result<T, GeoError>;

impl<'a> Parser<'a> {
    fn err<T>(&self, message: impl Into<String>) -> Res<T> {
        Err(GeoError::InvalidWkt { offset: self.pos, message: message.into() })
    }

    fn bytes(&self) -> &'a [u8] {
        self.text.as_bytes()
    }

    fn ws(&mut self) {
        while self.bytes().get(self.pos).is_some_and(u8::is_ascii_whitespace) {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.bytes().get(self.pos).copied()
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, b: u8) -> Res<()> {
        if self.eat(b) {
            Ok(())
        } else {
            let found = match self.bytes().get(self.pos) {
                Some(_) => self.text[self.pos..].chars().next().map(String::from).unwrap_or_default(),
                None => "end of input".to_string(),
            };
            self.err(format!("expected '{}', found {found}", b as char))
        }
    }

    /// A run of ASCII letters (possibly empty).
    fn word(&mut self) -> &'a str {
        self.ws();
        let start = self.pos;
        while self.bytes().get(self.pos).is_some_and(u8::is_ascii_alphabetic) {
            self.pos += 1;
        }
        &self.text[start..self.pos]
    }

    /// Consume the keyword `EMPTY` if it is next.
    fn try_empty(&mut self) -> bool {
        self.ws();
        let rest = &self.bytes()[self.pos..];
        let is_word = rest.len() >= 5
            && rest[..5].eq_ignore_ascii_case(b"EMPTY")
            && !rest.get(5).is_some_and(u8::is_ascii_alphanumeric);
        if is_word {
            self.pos += 5;
        }
        is_word
    }

    fn number(&mut self) -> Res<f64> {
        self.ws();
        let start = self.pos;
        while let Some(&b) = self.bytes().get(self.pos) {
            if b.is_ascii_whitespace() || matches!(b, b',' | b'(' | b')') {
                break;
            }
            self.pos += 1;
        }
        let tok = &self.text[start..self.pos];
        match tok.parse::<f64>() {
            Ok(v) => Ok(v),
            Err(_) => {
                self.pos = start;
                self.err(if tok.is_empty() { "expected a number".to_string() } else { format!("invalid number '{tok}'") })
            }
        }
    }

    /// One coordinate tuple: 2 to 4 ordinates (x y [z] [m]); extras are dropped.
    fn coord(&mut self) -> Res<Coord> {
        let start = self.pos;
        let x = self.number()?;
        let y = self.number()?;
        let mut extra = 0;
        while !matches!(self.peek(), None | Some(b',') | Some(b')')) {
            self.number()?;
            extra += 1;
            if extra > 2 {
                self.pos = start;
                return self.err("a coordinate has at most 4 ordinates");
            }
        }
        Ok([x, y])
    }

    /// `( item , item , ... )`; items returning `None` (EMPTY members) are skipped.
    fn list<T>(&mut self, mut item: impl FnMut(&mut Self) -> Res<Option<T>>) -> Res<Vec<T>> {
        self.expect(b'(')?;
        let mut out = Vec::new();
        loop {
            if let Some(v) = item(self)? {
                out.push(v);
            }
            if !self.eat(b',') {
                break;
            }
        }
        self.expect(b')')?;
        Ok(out)
    }

    fn line(&mut self) -> Res<Vec<Coord>> {
        self.list(|p| p.coord().map(Some))
    }

    fn polygon(&mut self) -> Res<Vec<Vec<Coord>>> {
        self.list(|p| if p.try_empty() { Ok(None) } else { p.line().map(Some) })
    }

    fn tagged(&mut self, depth: usize) -> Res<Geometry> {
        if depth > MAX_DEPTH {
            return Err(GeoError::TooDeep);
        }
        self.ws();
        let start = self.pos;
        let word = self.word();
        let Some(kind) = classify(word) else {
            self.pos = start;
            return self.err(if word.is_empty() {
                "expected a geometry type".to_string()
            } else {
                format!("unknown geometry type '{word}'")
            });
        };
        // Optional separate dimension tag, or EMPTY.
        let mut empty = self.try_empty();
        if !empty {
            let save = self.pos;
            let w = self.word();
            if w.eq_ignore_ascii_case("EMPTY") {
                empty = true;
            } else if ["Z", "M", "ZM"].iter().any(|t| w.eq_ignore_ascii_case(t)) {
                empty = self.try_empty();
            } else if !w.is_empty() {
                self.pos = save;
                return self.err(format!("unexpected '{w}'"));
            }
        }
        if empty {
            return Ok(match kind {
                Kind::Point => Geometry::Point(None),
                Kind::LineString => Geometry::LineString(vec![]),
                Kind::Polygon => Geometry::Polygon(vec![]),
                Kind::MultiPoint => Geometry::MultiPoint(vec![]),
                Kind::MultiLineString => Geometry::MultiLineString(vec![]),
                Kind::MultiPolygon => Geometry::MultiPolygon(vec![]),
                Kind::Collection => Geometry::GeometryCollection(vec![]),
            });
        }
        Ok(match kind {
            Kind::Point => {
                self.expect(b'(')?;
                let c = self.coord()?;
                self.expect(b')')?;
                Geometry::Point(Some(c))
            }
            Kind::LineString => Geometry::LineString(self.line()?),
            Kind::Polygon => Geometry::Polygon(self.polygon()?),
            Kind::MultiPoint => Geometry::MultiPoint(self.list(|p| {
                if p.try_empty() {
                    return Ok(None);
                }
                let wrapped = p.eat(b'(');
                let c = p.coord()?;
                if wrapped {
                    p.expect(b')')?;
                }
                Ok(Some(c))
            })?),
            Kind::MultiLineString => Geometry::MultiLineString(
                self.list(|p| if p.try_empty() { Ok(Some(vec![])) } else { p.line().map(Some) })?,
            ),
            Kind::MultiPolygon => Geometry::MultiPolygon(
                self.list(|p| if p.try_empty() { Ok(Some(vec![])) } else { p.polygon().map(Some) })?,
            ),
            Kind::Collection => Geometry::GeometryCollection(self.list(|p| p.tagged(depth + 1).map(Some))?),
        })
    }
}

/// Parse WKT or PostGIS EWKT (`SRID=4326;POINT(1 2)`). Z/M ordinates are dropped.
pub fn parse_wkt(text: &str) -> Result<Feature, GeoError> {
    let mut p = Parser { text, pos: 0 };
    p.ws();
    let mut srid = None;
    let rest = &text.as_bytes()[p.pos..];
    if rest.len() >= 5 && rest[..5].eq_ignore_ascii_case(b"SRID=") {
        p.pos += 5;
        let start = p.pos;
        while p.bytes().get(p.pos).is_some_and(|b| *b != b';') {
            p.pos += 1;
        }
        let digits = text[start..p.pos].trim();
        srid = match digits.parse::<i32>() {
            Ok(v) => Some(v),
            Err(_) => {
                p.pos = start;
                return p.err(format!("invalid SRID '{digits}'"));
            }
        };
        p.expect(b';')?;
    }
    let geometry = p.tagged(0)?;
    if p.peek().is_some() {
        return p.err("unexpected trailing input");
    }
    Ok(Feature { geometry, srid })
}

/// Format one ordinate. `None` precision prints the shortest string that round-trips
/// the `f64`; `Some(n)` rounds to at most `n` decimals and strips trailing zeros.
fn num(v: f64, precision: Option<usize>) -> String {
    match precision {
        None => v.to_string(),
        Some(p) => {
            let mut s = format!("{v:.p$}");
            if s.contains('.') {
                s.truncate(s.trim_end_matches('0').trim_end_matches('.').len());
            }
            if s == "-0" { "0".to_string() } else { s }
        }
    }
}

fn coord(c: Coord, p: Option<usize>) -> String {
    format!("{} {}", num(c[0], p), num(c[1], p))
}

fn coords(cs: &[Coord], p: Option<usize>) -> String {
    cs.iter().map(|c| coord(*c, p)).collect::<Vec<_>>().join(",")
}

/// `((..),(..))` for rings, or `EMPTY` when there are none.
fn rings(rs: &[Vec<Coord>], p: Option<usize>) -> String {
    if rs.is_empty() {
        return "EMPTY".into();
    }
    format!("({})", rs.iter().map(|r| format!("({})", coords(r, p))).collect::<Vec<_>>().join(","))
}

fn body(g: &Geometry, p: Option<usize>) -> String {
    let tag = g.type_name().to_ascii_uppercase();
    match g {
        Geometry::Point(None) => format!("{tag} EMPTY"),
        Geometry::Point(Some(c)) => format!("{tag}({})", coord(*c, p)),
        Geometry::LineString(l) | Geometry::MultiPoint(l) if l.is_empty() => format!("{tag} EMPTY"),
        Geometry::LineString(l) => format!("{tag}({})", coords(l, p)),
        Geometry::MultiPoint(l) => {
            format!("{tag}({})", l.iter().map(|c| format!("({})", coord(*c, p))).collect::<Vec<_>>().join(","))
        }
        Geometry::Polygon(r) if r.is_empty() => format!("{tag} EMPTY"),
        Geometry::Polygon(r) => format!("{tag}{}", rings(r, p)),
        Geometry::MultiLineString(ls) if ls.is_empty() => format!("{tag} EMPTY"),
        Geometry::MultiLineString(ls) => format!(
            "{tag}({})",
            ls.iter()
                .map(|l| if l.is_empty() { "EMPTY".to_string() } else { format!("({})", coords(l, p)) })
                .collect::<Vec<_>>()
                .join(",")
        ),
        Geometry::MultiPolygon(ps) if ps.is_empty() => format!("{tag} EMPTY"),
        Geometry::MultiPolygon(ps) => {
            format!("{tag}({})", ps.iter().map(|r| rings(r, p)).collect::<Vec<_>>().join(","))
        }
        Geometry::GeometryCollection(gs) if gs.is_empty() => format!("{tag} EMPTY"),
        Geometry::GeometryCollection(gs) => {
            format!("{tag}({})", gs.iter().map(|g| body(g, p)).collect::<Vec<_>>().join(","))
        }
    }
}

/// Write PostGIS-style WKT (no spaces after commas), ignoring the SRID.
pub fn to_wkt(feature: &Feature, precision: Option<usize>) -> String {
    body(&feature.geometry, precision)
}

/// Like [`to_wkt`] but prefixed with `SRID=n;` when the feature has an SRID.
pub fn to_ewkt(feature: &Feature, precision: Option<usize>) -> String {
    match feature.srid {
        Some(s) => format!("SRID={s};{}", to_wkt(feature, precision)),
        None => to_wkt(feature, precision),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(s: &str) -> Geometry {
        parse_wkt(s).unwrap().geometry
    }

    fn w(s: &str) -> String {
        to_wkt(&parse_wkt(s).unwrap(), None)
    }

    #[test]
    fn point() {
        assert_eq!(g("POINT(1 2)"), Geometry::Point(Some([1.0, 2.0])));
        assert_eq!(g("  point ( 1.5   -2e1 ) "), Geometry::Point(Some([1.5, -20.0])));
    }

    #[test]
    fn dimension_tags_are_dropped() {
        for s in ["POINT Z (1 2 3)", "POINTZ(1 2 3)", "POINT M (1 2 3)", "POINT ZM (1 2 3 4)", "pointzm(1 2 3 4)"] {
            assert_eq!(g(s), Geometry::Point(Some([1.0, 2.0])), "{s}");
        }
        // Untagged 3D is tolerated too (PostGIS writes it that way for some tools).
        assert_eq!(g("POINT(1 2 3)"), Geometry::Point(Some([1.0, 2.0])));
    }

    #[test]
    fn empties() {
        assert_eq!(g("POINT EMPTY"), Geometry::Point(None));
        assert_eq!(g("point empty"), Geometry::Point(None));
        assert_eq!(g("LINESTRING EMPTY"), Geometry::LineString(vec![]));
        assert_eq!(g("POLYGON EMPTY"), Geometry::Polygon(vec![]));
        assert_eq!(g("MULTIPOLYGON EMPTY"), Geometry::MultiPolygon(vec![]));
        assert_eq!(g("GEOMETRYCOLLECTION EMPTY"), Geometry::GeometryCollection(vec![]));
        assert_eq!(g("POINT Z EMPTY"), Geometry::Point(None));
    }

    #[test]
    fn linestring_and_polygon() {
        assert_eq!(g("LINESTRING(0 0, 1 1,2 0)"), Geometry::LineString(vec![[0.0, 0.0], [1.0, 1.0], [2.0, 0.0]]));
        let Geometry::Polygon(r) = g("POLYGON((0 0,4 0,4 4,0 0),(1 1,2 1,2 2,1 1))") else { panic!() };
        assert_eq!(r.len(), 2);
        assert_eq!(r[1].len(), 4);
    }

    #[test]
    fn multipoint_both_syntaxes() {
        let want = Geometry::MultiPoint(vec![[1.0, 2.0], [3.0, 4.0]]);
        assert_eq!(g("MULTIPOINT(1 2, 3 4)"), want);
        assert_eq!(g("MULTIPOINT((1 2),(3 4))"), want);
        assert_eq!(g("MULTIPOINT((1 2), EMPTY, (3 4))"), want);
    }

    #[test]
    fn multis_and_collection() {
        assert!(matches!(g("MULTILINESTRING((0 0,1 1),(2 2,3 3))"), Geometry::MultiLineString(l) if l.len() == 2));
        assert!(matches!(g("MULTIPOLYGON(((0 0,1 0,1 1,0 0)),((5 5,6 5,6 6,5 5)))"), Geometry::MultiPolygon(p) if p.len() == 2));
        let Geometry::GeometryCollection(c) = g("GEOMETRYCOLLECTION(POINT(1 2),LINESTRING(0 0,1 1),GEOMETRYCOLLECTION(POINT EMPTY))") else { panic!() };
        assert_eq!(c.len(), 3);
        assert!(matches!(&c[2], Geometry::GeometryCollection(i) if i.len() == 1));
    }

    #[test]
    fn ewkt() {
        let f = parse_wkt("SRID=4326;POINT(1 2)").unwrap();
        assert_eq!(f.srid, Some(4326));
        let f = parse_wkt("srid = 3857 ;POINT(1 2)");
        assert!(f.is_err() || f.unwrap().srid == Some(3857));
        assert!(parse_wkt("SRID=abc;POINT(1 2)").is_err());
        assert!(parse_wkt("SRID=4326 POINT(1 2)").is_err());
    }

    #[test]
    fn errors_have_offsets() {
        let GeoError::InvalidWkt { offset, .. } = parse_wkt("POINT(1 x)").unwrap_err() else { panic!() };
        assert_eq!(offset, 8);
        let GeoError::InvalidWkt { offset, .. } = parse_wkt("POINT(1 2").unwrap_err() else { panic!() };
        assert_eq!(offset, 9);
        let GeoError::InvalidWkt { message, .. } = parse_wkt("CIRCLE(1 2)").unwrap_err() else { panic!() };
        assert!(message.contains("CIRCLE"));
        assert!(parse_wkt("").is_err());
        assert!(parse_wkt("POINT(1)").is_err());
        assert!(parse_wkt("POINT(1 2 3 4 5)").is_err());
        assert!(parse_wkt("POINT(1 2) garbage").is_err());
        assert!(parse_wkt("POINT(1 2)))").is_err());
        assert!(parse_wkt("LINESTRING(0 0,)").is_err());
    }

    #[test]
    fn nesting_cap() {
        let s = "GEOMETRYCOLLECTION(".repeat(500) + "POINT EMPTY" + &")".repeat(500);
        assert_eq!(parse_wkt(&s), Err(GeoError::TooDeep));
    }

    #[test]
    fn multibyte_garbage_does_not_panic() {
        for s in ["POINT(é 2)", "POINT(1 2é", "é", "POINT é", "SRID=é;POINT(1 2)", "POLYGON((1 2,€))"] {
            assert!(parse_wkt(s).is_err(), "{s}");
        }
    }

    #[test]
    fn writer_shapes() {
        assert_eq!(w("POINT(1 2)"), "POINT(1 2)");
        assert_eq!(w("POINT EMPTY"), "POINT EMPTY");
        assert_eq!(w("LINESTRING(0 0, 1 1)"), "LINESTRING(0 0,1 1)");
        assert_eq!(w("POLYGON((0 0,1 0,1 1,0 0))"), "POLYGON((0 0,1 0,1 1,0 0))");
        assert_eq!(w("MULTIPOINT(1 2,3 4)"), "MULTIPOINT((1 2),(3 4))");
        assert_eq!(w("MULTIPOLYGON(((0 0,1 0,1 1,0 0)))"), "MULTIPOLYGON(((0 0,1 0,1 1,0 0)))");
        assert_eq!(w("MULTILINESTRING((0 0,1 1),EMPTY)"), "MULTILINESTRING((0 0,1 1),EMPTY)");
        assert_eq!(w("GEOMETRYCOLLECTION(POINT(1 2),POLYGON EMPTY)"), "GEOMETRYCOLLECTION(POINT(1 2),POLYGON EMPTY)");
        assert_eq!(w("GEOMETRYCOLLECTION EMPTY"), "GEOMETRYCOLLECTION EMPTY");
    }

    #[test]
    fn precision() {
        let f = parse_wkt("POINT(151.20930001 -33.86880009)").unwrap();
        assert_eq!(to_wkt(&f, Some(4)), "POINT(151.2093 -33.8688)");
        assert_eq!(to_wkt(&f, Some(0)), "POINT(151 -34)");
        assert_eq!(to_wkt(&f, None), "POINT(151.20930001 -33.86880009)");
        let f = parse_wkt("POINT(-0.00001 1.50000)").unwrap();
        assert_eq!(to_wkt(&f, Some(2)), "POINT(0 1.5)");
    }

    #[test]
    fn ewkt_output() {
        let f = parse_wkt("SRID=4326;POINT(1 2)").unwrap();
        assert_eq!(to_ewkt(&f, None), "SRID=4326;POINT(1 2)");
        assert_eq!(to_wkt(&f, None), "POINT(1 2)");
        let f = Feature::new(f.geometry, None);
        assert_eq!(to_ewkt(&f, None), "POINT(1 2)");
    }

    #[test]
    fn roundtrip_through_ewkb_all_types() {
        let cases = [
            "POINT(1 2)",
            "POINT EMPTY",
            "LINESTRING(0 0,1 1,2 0)",
            "LINESTRING EMPTY",
            "POLYGON((0 0,4 0,4 4,0 0),(1 1,2 1,2 2,1 1))",
            "POLYGON EMPTY",
            "MULTIPOINT((1 2),(3 4))",
            "MULTIPOINT EMPTY",
            "MULTILINESTRING((0 0,1 1),(2 2,3 3))",
            "MULTIPOLYGON(((0 0,1 0,1 1,0 0)),((5 5,6 5,6 6,5 5)))",
            "MULTIPOLYGON EMPTY",
            "GEOMETRYCOLLECTION(POINT(1 2),LINESTRING(0 0,1 1),GEOMETRYCOLLECTION(POINT(3 3),POLYGON((0 0,1 0,1 1,0 0))))",
            "GEOMETRYCOLLECTION EMPTY",
            "POINT Z (1 2 3)",
            "LINESTRING M (0 0 5,1 1 6)",
            "POLYGON ZM ((0 0 1 2,1 0 1 2,1 1 1 2,0 0 1 2))",
        ];
        for c in cases {
            let a = parse_wkt(&format!("SRID=4326;{c}")).unwrap();
            let b = crate::wkb::parse_wkb(&crate::wkb::to_ewkb(&a)).unwrap();
            assert_eq!(a, b, "{c}");
            let again = parse_wkt(&to_ewkt(&b, None)).unwrap();
            assert_eq!(again, b, "{c}");
        }
    }
}
