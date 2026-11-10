//! WKB / EWKB decoding (binary and hex) and EWKB encoding.
//!
//! Two dialects share one layout: `byte-order(1) | type(u32) | [srid(i32)] | payload`.
//! * ISO WKB encodes dimensions in the type code: `+1000` Z, `+2000` M, `+3000` ZM.
//! * PostGIS EWKB uses flag bits on the type: `0x8000_0000` Z, `0x4000_0000` M,
//!   `0x2000_0000` "an SRID follows".
//!
//! Every length field is validated against the bytes that remain *before* anything is
//! allocated, and collection nesting is capped, so hostile input costs at most O(input).

use crate::error::GeoError;
use crate::geometry::{Coord, Feature, Geometry};

const MAX_DEPTH: usize = 32;
const FLAG_Z: u32 = 0x8000_0000;
const FLAG_M: u32 = 0x4000_0000;
const FLAG_SRID: u32 = 0x2000_0000;

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], GeoError> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.bytes.len());
        let end = end.ok_or(GeoError::Truncated { offset: self.pos })?;
        let s = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn u8(&mut self) -> Result<u8, GeoError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self, le: bool) -> Result<u32, GeoError> {
        let b: [u8; 4] = self.take(4)?.try_into().expect("take(4) returns 4 bytes");
        Ok(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    }

    fn f64(&mut self, le: bool) -> Result<f64, GeoError> {
        let b: [u8; 8] = self.take(8)?.try_into().expect("take(8) returns 8 bytes");
        Ok(if le { f64::from_le_bytes(b) } else { f64::from_be_bytes(b) })
    }

    /// Read an element count and reject it if `count * min_elem_bytes` exceeds what is left.
    fn count(&mut self, le: bool, min_elem_bytes: usize) -> Result<usize, GeoError> {
        let offset = self.pos;
        let n = self.u32(le)? as u64;
        if n.saturating_mul(min_elem_bytes as u64) > self.remaining() as u64 {
            return Err(GeoError::BadCount { offset, count: n });
        }
        Ok(n as usize)
    }

    /// Read one coordinate with `ndim` ordinates, keeping x and y.
    fn coord(&mut self, le: bool, ndim: usize) -> Result<Coord, GeoError> {
        let x = self.f64(le)?;
        let y = self.f64(le)?;
        for _ in 2..ndim {
            self.f64(le)?;
        }
        Ok([x, y])
    }

    fn coords(&mut self, le: bool, ndim: usize) -> Result<Vec<Coord>, GeoError> {
        let n = self.count(le, ndim * 8)?;
        (0..n).map(|_| self.coord(le, ndim)).collect()
    }

    fn rings(&mut self, le: bool, ndim: usize) -> Result<Vec<Vec<Coord>>, GeoError> {
        let n = self.count(le, 4)?;
        (0..n).map(|_| self.coords(le, ndim)).collect()
    }
}

fn read_geometry(r: &mut Reader, depth: usize) -> Result<(Geometry, Option<i32>), GeoError> {
    if depth > MAX_DEPTH {
        return Err(GeoError::TooDeep);
    }
    let le = match r.u8()? {
        0 => false,
        1 => true,
        b => return Err(GeoError::BadByteOrder(b)),
    };
    let raw = r.u32(le)?;
    // Strip the three EWKB flags; what remains is `base + 1000 * iso_dimension`.
    let code = raw & !(FLAG_Z | FLAG_M | FLAG_SRID);
    let iso_dim = code / 1000;
    if iso_dim > 3 {
        return Err(GeoError::UnknownType(raw));
    }
    let base = code % 1000;
    let has_z = raw & FLAG_Z != 0 || iso_dim == 1 || iso_dim == 3;
    let has_m = raw & FLAG_M != 0 || iso_dim == 2 || iso_dim == 3;
    let ndim = 2 + usize::from(has_z) + usize::from(has_m);
    let srid = if raw & FLAG_SRID != 0 { Some(r.u32(le)? as i32) } else { None };

    let geometry = match base {
        1 => {
            let c = r.coord(le, ndim)?;
            // PostGIS encodes POINT EMPTY as a point whose ordinates are all NaN.
            Geometry::Point(if c[0].is_nan() && c[1].is_nan() { None } else { Some(c) })
        }
        2 => Geometry::LineString(r.coords(le, ndim)?),
        3 => Geometry::Polygon(r.rings(le, ndim)?),
        4 | 5 | 6 | 7 => {
            // The smallest possible child is an empty collection: 1 + 4 + 4 bytes.
            let n = r.count(le, 9)?;
            read_multi(r, base, n, depth)?
        }
        _ => return Err(GeoError::UnknownType(raw)),
    };
    Ok((geometry, srid))
}

fn read_multi(r: &mut Reader, base: u32, n: usize, depth: usize) -> Result<Geometry, GeoError> {
    let mut points = Vec::new();
    let mut lines = Vec::new();
    let mut polys = Vec::new();
    let mut geoms = Vec::new();
    for _ in 0..n {
        let at = r.pos;
        let (child, _) = read_geometry(r, depth + 1)?;
        let mismatch = |want: &str, got: &Geometry| {
            GeoError::InvalidWkb(format!("{want} child expected at byte {at}, found {}", got.type_name()))
        };
        match (base, child) {
            (4, Geometry::Point(p)) => points.extend(p),
            (4, g) => return Err(mismatch("Point", &g)),
            (5, Geometry::LineString(l)) => lines.push(l),
            (5, g) => return Err(mismatch("LineString", &g)),
            (6, Geometry::Polygon(p)) => polys.push(p),
            (6, g) => return Err(mismatch("Polygon", &g)),
            (_, g) => geoms.push(g),
        }
    }
    Ok(match base {
        4 => Geometry::MultiPoint(points),
        5 => Geometry::MultiLineString(lines),
        6 => Geometry::MultiPolygon(polys),
        _ => Geometry::GeometryCollection(geoms),
    })
}

/// Decode ISO WKB or PostGIS EWKB (either byte order, nested multi-geometries each
/// carrying their own byte-order mark). Z and M ordinates are parsed and dropped.
/// Trailing bytes after the geometry are ignored.
pub fn parse_wkb(bytes: &[u8]) -> Result<Feature, GeoError> {
    let mut r = Reader { bytes, pos: 0 };
    let (geometry, srid) = read_geometry(&mut r, 0)?;
    Ok(Feature { geometry, srid })
}

/// Decode hex-encoded (E)WKB, which is how PostGIS prints geometry columns as text.
/// Surrounding whitespace and a leading `\x` (bytea output) are tolerated.
pub fn parse_hex_wkb(text: &str) -> Result<Feature, GeoError> {
    let trimmed = text.trim();
    let skip = if trimmed.starts_with("\\x") || trimmed.starts_with("0x") { 2 } else { 0 };
    let base = text.len() - text.trim_start().len() + skip;
    let digits = trimmed[skip..].as_bytes();
    if digits.len() % 2 != 0 {
        return Err(GeoError::InvalidHex { offset: base + digits.len() });
    }
    let nibble = |i: usize| -> Result<u8, GeoError> {
        match digits[i] {
            c @ b'0'..=b'9' => Ok(c - b'0'),
            c @ b'a'..=b'f' => Ok(c - b'a' + 10),
            c @ b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => Err(GeoError::InvalidHex { offset: base + i }),
        }
    };
    let mut bytes = Vec::with_capacity(digits.len() / 2);
    for i in (0..digits.len()).step_by(2) {
        bytes.push((nibble(i)? << 4) | nibble(i + 1)?);
    }
    parse_wkb(&bytes)
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_coord(out: &mut Vec<u8>, c: Coord) {
    out.extend_from_slice(&c[0].to_le_bytes());
    out.extend_from_slice(&c[1].to_le_bytes());
}

fn put_coords(out: &mut Vec<u8>, cs: &[Coord]) {
    put_u32(out, cs.len() as u32);
    cs.iter().for_each(|c| put_coord(out, *c));
}

fn put_rings(out: &mut Vec<u8>, rings: &[Vec<Coord>]) {
    put_u32(out, rings.len() as u32);
    rings.iter().for_each(|r| put_coords(out, r));
}

fn put_header(out: &mut Vec<u8>, code: u32, srid: Option<i32>) {
    out.push(1);
    match srid {
        Some(s) => {
            put_u32(out, code | FLAG_SRID);
            put_u32(out, s as u32);
        }
        None => put_u32(out, code),
    }
}

fn write_geometry(out: &mut Vec<u8>, g: &Geometry, srid: Option<i32>) {
    match g {
        Geometry::Point(p) => {
            put_header(out, 1, srid);
            put_coord(out, p.unwrap_or([f64::NAN, f64::NAN]));
        }
        Geometry::LineString(l) => {
            put_header(out, 2, srid);
            put_coords(out, l);
        }
        Geometry::Polygon(r) => {
            put_header(out, 3, srid);
            put_rings(out, r);
        }
        Geometry::MultiPoint(pts) => {
            put_header(out, 4, srid);
            put_u32(out, pts.len() as u32);
            for p in pts {
                write_geometry(out, &Geometry::Point(Some(*p)), None);
            }
        }
        Geometry::MultiLineString(ls) => {
            put_header(out, 5, srid);
            put_u32(out, ls.len() as u32);
            for l in ls {
                put_header(out, 2, None);
                put_coords(out, l);
            }
        }
        Geometry::MultiPolygon(ps) => {
            put_header(out, 6, srid);
            put_u32(out, ps.len() as u32);
            for p in ps {
                put_header(out, 3, None);
                put_rings(out, p);
            }
        }
        Geometry::GeometryCollection(gs) => {
            put_header(out, 7, srid);
            put_u32(out, gs.len() as u32);
            for g in gs {
                write_geometry(out, g, None);
            }
        }
    }
}

/// Encode as little-endian 2D EWKB. The SRID (if any) is written on the outermost
/// geometry only, matching what PostGIS emits. Round-trips with [`parse_wkb`].
pub fn to_ewkb(feature: &Feature) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + feature.geometry.coord_count() * 16);
    write_geometry(&mut out, &feature.geometry, feature.srid);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Feature {
        parse_hex_wkb(s).unwrap()
    }

    #[test]
    fn point_le_iso() {
        let f = hex("0101000000000000000000F03F0000000000000040");
        assert_eq!(f.geometry, Geometry::Point(Some([1.0, 2.0])));
        assert_eq!(f.srid, None);
    }

    #[test]
    fn point_be_iso() {
        let f = hex("00000000013FF00000000000004000000000000000");
        assert_eq!(f.geometry, Geometry::Point(Some([1.0, 2.0])));
    }

    #[test]
    fn ewkb_srid_point() {
        let f = hex("0101000020E6100000000000000000F03F0000000000000040");
        assert_eq!(f.srid, Some(4326));
        assert_eq!(f.geometry, Geometry::Point(Some([1.0, 2.0])));
    }

    #[test]
    fn ewkb_z_flag_drops_z() {
        // type 0x80000001, x=1 y=2 z=3
        let f = hex("0101000080000000000000F03F00000000000000400000000000000840");
        assert_eq!(f.geometry, Geometry::Point(Some([1.0, 2.0])));
    }

    #[test]
    fn iso_zm_codes() {
        for (code, extra) in [(1001u32, 1), (2001, 1), (3001, 2)] {
            let mut b = vec![1];
            b.extend(code.to_le_bytes());
            for v in [1.0f64, 2.0].into_iter().chain((0..extra).map(|i| 9.0 + i as f64)) {
                b.extend(v.to_le_bytes());
            }
            assert_eq!(parse_wkb(&b).unwrap().geometry, Geometry::Point(Some([1.0, 2.0])), "code {code}");
        }
    }

    #[test]
    fn empty_point_is_nan() {
        let f = parse_wkb(&to_ewkb(&Feature::new(Geometry::Point(None), None))).unwrap();
        assert_eq!(f.geometry, Geometry::Point(None));
    }

    #[test]
    fn bad_byte_order() {
        assert_eq!(parse_wkb(&[2, 1, 0, 0, 0]), Err(GeoError::BadByteOrder(2)));
    }

    #[test]
    fn unknown_type() {
        assert!(matches!(parse_wkb(&[1, 99, 0, 0, 0]), Err(GeoError::UnknownType(99))));
        assert!(matches!(parse_wkb(&[1, 0xB8, 0x0B, 0, 0]), Err(GeoError::UnknownType(_)))); // 3000 -> base 0
        assert!(matches!(parse_wkb(&[1, 1, 0, 0, 0x10]), Err(GeoError::UnknownType(_))));
    }

    #[test]
    fn truncated() {
        assert!(matches!(parse_wkb(&[]), Err(GeoError::Truncated { offset: 0 })));
        assert!(matches!(parse_wkb(&[1, 1, 0, 0, 0, 1, 2]), Err(GeoError::Truncated { .. })));
    }

    #[test]
    fn hostile_count_rejected_without_allocating() {
        // LineString claiming 4 billion points with no data.
        let b = [1, 2, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF];
        assert!(matches!(parse_wkb(&b), Err(GeoError::BadCount { count: 4294967295, .. })));
        // Polygon claiming 4 billion rings.
        let b = [1, 3, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF];
        assert!(matches!(parse_wkb(&b), Err(GeoError::BadCount { .. })));
        let b = [1, 7, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0x7F];
        assert!(matches!(parse_wkb(&b), Err(GeoError::BadCount { .. })));
    }

    #[test]
    fn depth_cap() {
        let mut b = Vec::new();
        for _ in 0..200 {
            b.extend([1, 7, 0, 0, 0, 1, 0, 0, 0]);
        }
        assert_eq!(parse_wkb(&b), Err(GeoError::TooDeep));
    }

    #[test]
    fn nested_depth_within_cap_ok() {
        let mut g = Geometry::Point(Some([1.0, 1.0]));
        for _ in 0..10 {
            g = Geometry::GeometryCollection(vec![g]);
        }
        let f = Feature::new(g, None);
        assert_eq!(parse_wkb(&to_ewkb(&f)).unwrap(), f);
    }

    #[test]
    fn multi_child_type_mismatch() {
        // MultiPoint containing a LineString
        let mut b = vec![1, 4, 0, 0, 0, 1, 0, 0, 0];
        b.extend([1, 2, 0, 0, 0, 0, 0, 0, 0]);
        assert!(matches!(parse_wkb(&b), Err(GeoError::InvalidWkb(_))));
    }

    #[test]
    fn multi_children_with_mixed_byte_order() {
        // little-endian MultiPoint with one big-endian Point child
        let mut b = vec![1, 4, 0, 0, 0, 1, 0, 0, 0];
        b.extend([0, 0, 0, 0, 1]);
        b.extend(5.0f64.to_be_bytes());
        b.extend(6.0f64.to_be_bytes());
        assert_eq!(parse_wkb(&b).unwrap().geometry, Geometry::MultiPoint(vec![[5.0, 6.0]]));
    }

    #[test]
    fn hex_prefixes_and_whitespace() {
        let h = "0101000000000000000000F03F0000000000000040";
        assert!(parse_hex_wkb(&format!("\\x{h}")).is_ok());
        assert!(parse_hex_wkb(&format!("  {h}\n")).is_ok());
        assert!(parse_hex_wkb(&h.to_lowercase()).is_ok());
    }

    #[test]
    fn hex_errors() {
        assert!(matches!(parse_hex_wkb("010"), Err(GeoError::InvalidHex { offset: 3 })));
        assert!(matches!(parse_hex_wkb("01zz"), Err(GeoError::InvalidHex { offset: 2 })));
        assert!(parse_hex_wkb("").is_err());
    }

    #[test]
    fn trailing_bytes_tolerated() {
        let mut b = to_ewkb(&Feature::new(Geometry::Point(Some([1.0, 2.0])), None));
        b.extend([9, 9, 9]);
        assert!(parse_wkb(&b).is_ok());
    }

    #[test]
    fn encode_srid_only_on_outer() {
        let f = Feature::new(Geometry::MultiPoint(vec![[1.0, 2.0], [3.0, 4.0]]), Some(4326));
        let b = to_ewkb(&f);
        assert_eq!(&b[..9], &[1, 4, 0, 0, 0x20, 0xE6, 0x10, 0, 0]);
        assert_eq!(parse_wkb(&b).unwrap(), f);
    }

    #[test]
    fn encode_all_types_roundtrip() {
        let sq = vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 0.0]];
        let geoms = vec![
            Geometry::Point(Some([1.5, -2.5])),
            Geometry::LineString(vec![[0.0, 0.0], [1.0, 1.0]]),
            Geometry::Polygon(vec![sq.clone(), sq.clone()]),
            Geometry::MultiPoint(vec![[1.0, 1.0]]),
            Geometry::MultiLineString(vec![vec![[0.0, 0.0], [1.0, 1.0]], vec![]]),
            Geometry::MultiPolygon(vec![vec![sq.clone()], vec![]]),
            Geometry::GeometryCollection(vec![Geometry::Point(None), Geometry::LineString(vec![])]),
            Geometry::Polygon(vec![]),
        ];
        for g in geoms {
            let f = Feature::new(g, Some(3857));
            assert_eq!(parse_wkb(&to_ewkb(&f)).unwrap(), f);
        }
    }
}
