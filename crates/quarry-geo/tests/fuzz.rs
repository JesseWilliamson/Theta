//! Fuzz-style robustness tests with a tiny deterministic PRNG (no dev-dependencies).
//! The decoders must return `Ok`/`Err` for any input, never panic or hang, and
//! valid geometries must survive every encode/decode round trip.

use quarry_geo::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn coord(&mut self) -> Coord {
        // Mix of "nice" and full-precision values.
        let v = |r: &mut Rng| -> f64 {
            if r.below(2) == 0 {
                (r.below(36_000) as f64 - 18_000.0) / 100.0
            } else {
                (r.next() as f64 / u64::MAX as f64 - 0.5) * 720.0
            }
        };
        [v(self), v(self)]
    }
    fn coords(&mut self, min: usize) -> Vec<Coord> {
        let n = min + self.below(6);
        (0..n).map(|_| self.coord()).collect()
    }
    fn ring(&mut self) -> Vec<Coord> {
        let mut r = self.coords(3);
        r.push(r[0]);
        r
    }
    fn geometry(&mut self, depth: usize) -> Geometry {
        match self.below(if depth > 2 { 6 } else { 7 }) {
            0 => Geometry::Point(if self.below(8) == 0 { None } else { Some(self.coord()) }),
            1 => Geometry::LineString(if self.below(8) == 0 { vec![] } else { self.coords(2) }),
            2 => Geometry::Polygon((0..self.below(3)).map(|_| self.ring()).collect()),
            3 => Geometry::MultiPoint(self.coords(0)),
            4 => Geometry::MultiLineString((0..self.below(4)).map(|_| self.coords(2)).collect()),
            5 => Geometry::MultiPolygon(
                (0..self.below(3)).map(|_| (0..1 + self.below(2)).map(|_| self.ring()).collect()).collect(),
            ),
            _ => Geometry::GeometryCollection((0..self.below(4)).map(|_| self.geometry(depth + 1)).collect()),
        }
    }
    fn feature(&mut self) -> Feature {
        let srid = [None, Some(4326), Some(3857), Some(32756)][self.below(4)];
        Feature { geometry: self.geometry(0), srid }
    }
}

fn corpus() -> Vec<Feature> {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    (0..60).map(|_| rng.feature()).collect()
}

#[test]
fn random_geometries_roundtrip_through_ewkb_and_wkt() {
    for (i, f) in corpus().into_iter().enumerate() {
        assert_eq!(parse_wkb(&to_ewkb(&f)).unwrap(), f, "ewkb #{i}");
        let text = to_ewkt(&f, None);
        assert_eq!(parse_wkt(&text).unwrap(), f, "ewkt #{i}: {text}");
        assert_eq!(parse_any_text(&text).as_ref(), Some(&f), "sniff #{i}");
        let hex: String = to_ewkb(&f).iter().map(|b| format!("{b:02X}")).collect();
        assert_eq!(parse_hex_wkb(&hex).unwrap(), f, "hex #{i}");
    }
}

#[test]
fn random_geometries_roundtrip_through_geojson() {
    for (i, f) in corpus().into_iter().enumerate() {
        // GeoJSON has no SRID (always 4326) and cannot express EMPTY members inside
        // MultiPoint, so compare geometry only.
        let text = to_geojson(&f).to_string();
        let back = parse_geojson(&text).unwrap_or_else(|e| panic!("#{i}: {e}: {text}"));
        assert_eq!(back[0].geometry, f.geometry, "#{i}: {text}");
    }
}

#[test]
fn big_endian_encodings_of_random_geometries_decode() {
    // Re-encode our little-endian output as big endian by hand for points/lines/polygons.
    fn be(g: &Geometry, out: &mut Vec<u8>) {
        let hdr = |out: &mut Vec<u8>, code: u32| {
            out.push(0);
            out.extend(code.to_be_bytes());
        };
        let coords = |out: &mut Vec<u8>, cs: &[Coord]| {
            out.extend((cs.len() as u32).to_be_bytes());
            for c in cs {
                out.extend(c[0].to_be_bytes());
                out.extend(c[1].to_be_bytes());
            }
        };
        match g {
            Geometry::Point(p) => {
                hdr(out, 1);
                let c = p.unwrap_or([f64::NAN, f64::NAN]);
                out.extend(c[0].to_be_bytes());
                out.extend(c[1].to_be_bytes());
            }
            Geometry::LineString(l) => {
                hdr(out, 2);
                coords(out, l);
            }
            Geometry::Polygon(r) => {
                hdr(out, 3);
                out.extend((r.len() as u32).to_be_bytes());
                r.iter().for_each(|r| coords(out, r));
            }
            Geometry::MultiPoint(p) => {
                hdr(out, 4);
                out.extend((p.len() as u32).to_be_bytes());
                p.iter().for_each(|c| be(&Geometry::Point(Some(*c)), out));
            }
            Geometry::MultiLineString(l) => {
                hdr(out, 5);
                out.extend((l.len() as u32).to_be_bytes());
                l.iter().for_each(|c| be(&Geometry::LineString(c.clone()), out));
            }
            Geometry::MultiPolygon(p) => {
                hdr(out, 6);
                out.extend((p.len() as u32).to_be_bytes());
                p.iter().for_each(|c| be(&Geometry::Polygon(c.clone()), out));
            }
            Geometry::GeometryCollection(g) => {
                hdr(out, 7);
                out.extend((g.len() as u32).to_be_bytes());
                g.iter().for_each(|c| be(c, out));
            }
        }
    }
    for (i, f) in corpus().into_iter().enumerate() {
        let mut b = Vec::new();
        be(&f.geometry, &mut b);
        assert_eq!(parse_wkb(&b).unwrap().geometry, f.geometry, "#{i}");
    }
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng(42);
    for _ in 0..20_000 {
        let n = rng.below(80);
        let mut bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        // Bias the first bytes towards plausible headers so deeper paths are reached.
        if n > 5 && rng.below(2) == 0 {
            bytes[0] = (rng.below(2)) as u8;
            bytes[1] = rng.below(8) as u8;
            bytes[2..4].fill(0);
            bytes[4] = [0, 0x20, 0x80, 0x40, 0xA0][rng.below(5)];
        }
        let _ = parse_wkb(&bytes);
    }
}

#[test]
fn every_truncation_of_valid_wkb_is_handled() {
    for f in corpus() {
        let bytes = to_ewkb(&f);
        for cut in 0..bytes.len() {
            let r = parse_wkb(&bytes[..cut]);
            assert!(r.is_err(), "prefix {cut}/{} of {:?} parsed", bytes.len(), f.geometry.type_name());
        }
    }
}

#[test]
fn random_mutations_of_valid_wkb_never_panic() {
    let mut rng = Rng(7);
    for f in corpus() {
        let bytes = to_ewkb(&f);
        for _ in 0..200 {
            let mut m = bytes.clone();
            for _ in 0..1 + rng.below(4) {
                let i = rng.below(m.len());
                match rng.below(3) {
                    0 => m[i] = rng.next() as u8,
                    1 => m[i] ^= 1 << rng.below(8),
                    _ => m[i] = 0xFF,
                }
            }
            let _ = parse_wkb(&m);
            let hex: String = m.iter().map(|b| format!("{b:02x}")).collect();
            let _ = parse_hex_wkb(&hex);
            let _ = parse_any_text(&hex);
        }
    }
}

#[test]
fn random_text_never_panics() {
    let alphabet: Vec<char> = "POINTLSRGMUYCEZ()(),;= 0123456789.-+eE\n\té€𝄞{}[]\":".chars().collect();
    let mut rng = Rng(99);
    for _ in 0..30_000 {
        let n = rng.below(60);
        let s: String = (0..n).map(|_| alphabet[rng.below(alphabet.len())]).collect();
        let _ = parse_wkt(&s);
        let _ = parse_any_text(&s);
        let _ = parse_geojson(&s);
        let _ = parse_hex_wkb(&s);
    }
}

#[test]
fn mutated_valid_wkt_never_panics_and_truncations_fail_or_parse() {
    let mut rng = Rng(1234);
    let junk = ['(', ')', ',', ' ', 'é', '𝄞', '-', 'e', ';', '0', 'Z', '\0'];
    for f in corpus() {
        let text = to_ewkt(&f, None);
        let chars: Vec<char> = text.chars().collect();
        for cut in 0..chars.len() {
            let t: String = chars[..cut].iter().collect();
            let _ = parse_wkt(&t);
        }
        for _ in 0..100 {
            let mut m = chars.clone();
            for _ in 0..1 + rng.below(3) {
                let i = rng.below(m.len());
                match rng.below(3) {
                    0 => m[i] = junk[rng.below(junk.len())],
                    1 => m.insert(i, junk[rng.below(junk.len())]),
                    _ => {
                        m.remove(i);
                        if m.is_empty() {
                            m.push('(');
                        }
                    }
                }
            }
            let s: String = m.into_iter().collect();
            let _ = parse_wkt(&s);
            let _ = parse_any_text(&s);
        }
    }
}

#[test]
fn mutated_geojson_never_panics() {
    let mut rng = Rng(555);
    for f in corpus() {
        let text = to_geojson(&f).to_string();
        let bytes = text.as_bytes();
        for _ in 0..60 {
            let mut m = bytes.to_vec();
            let i = rng.below(m.len());
            m[i] = b"{}[],:\"0-e.x"[rng.below(12)];
            if let Ok(s) = String::from_utf8(m) {
                let _ = parse_geojson(&s);
                let _ = parse_any_text(&s);
            }
        }
        for cut in (0..bytes.len()).step_by(3) {
            let _ = parse_geojson(&text[..cut]);
        }
    }
}

#[test]
fn adversarial_inputs_are_cheap() {
    // Huge declared counts with no payload.
    for code in [2u32, 3, 4, 5, 6, 7] {
        let mut b = vec![1];
        b.extend(code.to_le_bytes());
        b.extend(u32::MAX.to_le_bytes());
        assert!(parse_wkb(&b).is_err());
        let mut b = vec![0];
        b.extend(code.to_be_bytes());
        b.extend(u32::MAX.to_be_bytes());
        assert!(parse_wkb(&b).is_err());
    }
    // Deeply nested collections in every format.
    let wkt = "GEOMETRYCOLLECTION(".repeat(100_000);
    assert!(parse_wkt(&wkt).is_err());
    let json = r#"{"type":"GeometryCollection","geometries":["#.repeat(100_000);
    assert!(parse_geojson(&json).is_err());
    let mut wkb = Vec::new();
    for _ in 0..100_000 {
        wkb.extend([1u8, 7, 0, 0, 0, 1, 0, 0, 0]);
    }
    assert!(parse_wkb(&wkb).is_err());
    // A long run of coordinates without a closing paren, and a very long number.
    let long = format!("LINESTRING({}", "1 2,".repeat(200_000));
    assert!(parse_wkt(&long).is_err());
    assert!(parse_wkt(&format!("POINT({} 2)", "9".repeat(100_000))).is_ok());
    // Non-UTF8 / odd hex never reaches a panic.
    assert!(parse_hex_wkb(&"0".repeat(1001)).is_err());
    assert!(parse_hex_wkb(&"zz".repeat(10)).is_err());
}
