//! The in-memory geometry model: always 2D, Z and M ordinates are dropped on decode
//! because the map viewer and inspector never use them.

/// A 2D coordinate: `[x, y]` = `[lon/easting, lat/northing]`.
pub type Coord = [f64; 2];

/// A geometry value. Mirrors the OGC simple-features model.
#[derive(Clone, Debug, PartialEq)]
pub enum Geometry {
    /// A single point; `None` is `POINT EMPTY`.
    Point(Option<Coord>),
    /// An ordered list of vertices.
    LineString(Vec<Coord>),
    /// Rings; the first is the exterior, the rest are holes. Rings are closed (first == last).
    Polygon(Vec<Vec<Coord>>),
    /// Several points.
    MultiPoint(Vec<Coord>),
    /// Several line strings.
    MultiLineString(Vec<Vec<Coord>>),
    /// Several polygons, each a list of rings.
    MultiPolygon(Vec<Vec<Vec<Coord>>>),
    /// A heterogeneous collection (may nest).
    GeometryCollection(Vec<Geometry>),
}

/// A geometry plus the SRID it was tagged with (EWKB/EWKT/GeoJSON `crs`), if any.
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    /// The geometry.
    pub geometry: Geometry,
    /// Spatial reference id, `None` when the source carried none.
    pub srid: Option<i32>,
}

impl Feature {
    /// Convenience constructor.
    pub fn new(geometry: Geometry, srid: Option<i32>) -> Feature {
        Feature { geometry, srid }
    }
}

/// An axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BBox {
    /// Lower-left corner (smallest x, smallest y).
    pub min: Coord,
    /// Upper-right corner (largest x, largest y).
    pub max: Coord,
}

impl BBox {
    /// The smallest box containing both boxes.
    pub fn union(self, other: BBox) -> BBox {
        BBox {
            min: [self.min[0].min(other.min[0]), self.min[1].min(other.min[1])],
            max: [self.max[0].max(other.max[0]), self.max[1].max(other.max[1])],
        }
    }

    /// Extent along x.
    pub fn width(&self) -> f64 {
        self.max[0] - self.min[0]
    }

    /// Extent along y.
    pub fn height(&self) -> f64 {
        self.max[1] - self.min[1]
    }

    /// Midpoint of the box.
    pub fn center(&self) -> Coord {
        [(self.min[0] + self.max[0]) / 2.0, (self.min[1] + self.max[1]) / 2.0]
    }

    /// Whether `c` lies inside or on the border.
    pub fn contains(&self, c: Coord) -> bool {
        c[0] >= self.min[0] && c[0] <= self.max[0] && c[1] >= self.min[1] && c[1] <= self.max[1]
    }

    /// Whether the boxes overlap or touch.
    pub fn intersects(&self, other: &BBox) -> bool {
        self.min[0] <= other.max[0]
            && self.max[0] >= other.min[0]
            && self.min[1] <= other.max[1]
            && self.max[1] >= other.min[1]
    }

    /// Grow the box on every side by `factor` times its own size, so the width becomes
    /// `width * (1 + 2 * factor)`. A degenerate (zero-size) axis stays degenerate.
    pub fn expand(self, factor: f64) -> BBox {
        let (dx, dy) = (self.width() * factor, self.height() * factor);
        BBox { min: [self.min[0] - dx, self.min[1] - dy], max: [self.max[0] + dx, self.max[1] + dy] }
    }

    fn of_point(c: Coord) -> BBox {
        BBox { min: c, max: c }
    }
}

impl Geometry {
    /// OGC type name, e.g. `"MultiPolygon"`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Geometry::Point(_) => "Point",
            Geometry::LineString(_) => "LineString",
            Geometry::Polygon(_) => "Polygon",
            Geometry::MultiPoint(_) => "MultiPoint",
            Geometry::MultiLineString(_) => "MultiLineString",
            Geometry::MultiPolygon(_) => "MultiPolygon",
            Geometry::GeometryCollection(_) => "GeometryCollection",
        }
    }

    /// True when the geometry has no coordinates at all.
    pub fn is_empty(&self) -> bool {
        self.coord_count() == 0
    }

    /// Total number of vertices (ring closing vertices included).
    pub fn coord_count(&self) -> usize {
        match self {
            Geometry::Point(p) => usize::from(p.is_some()),
            Geometry::LineString(l) | Geometry::MultiPoint(l) => l.len(),
            Geometry::Polygon(r) | Geometry::MultiLineString(r) => r.iter().map(Vec::len).sum(),
            Geometry::MultiPolygon(p) => p.iter().flatten().map(Vec::len).sum(),
            Geometry::GeometryCollection(g) => g.iter().map(Geometry::coord_count).sum(),
        }
    }

    /// Bounding box over all vertices, `None` when empty.
    pub fn bbox(&self) -> Option<BBox> {
        let mut out: Option<BBox> = None;
        self.for_each_coord(&mut |c| {
            out = Some(match out {
                None => BBox::of_point(c),
                Some(b) => b.union(BBox::of_point(c)),
            });
        });
        out
    }

    /// Visit every vertex in document order.
    pub fn for_each_coord(&self, f: &mut dyn FnMut(Coord)) {
        match self {
            Geometry::Point(p) => p.iter().for_each(|c| f(*c)),
            Geometry::LineString(l) | Geometry::MultiPoint(l) => l.iter().for_each(|c| f(*c)),
            Geometry::Polygon(r) | Geometry::MultiLineString(r) => r.iter().flatten().for_each(|c| f(*c)),
            Geometry::MultiPolygon(p) => p.iter().flatten().flatten().for_each(|c| f(*c)),
            Geometry::GeometryCollection(g) => g.iter().for_each(|g| g.for_each_coord(f)),
        }
    }

    /// A copy with every vertex replaced by `f(vertex)` (used for reprojection).
    pub fn map_coords(&self, f: &dyn Fn(Coord) -> Coord) -> Geometry {
        let line = |l: &Vec<Coord>| l.iter().map(|c| f(*c)).collect::<Vec<_>>();
        let rings = |r: &Vec<Vec<Coord>>| r.iter().map(line).collect::<Vec<_>>();
        match self {
            Geometry::Point(p) => Geometry::Point(p.map(f)),
            Geometry::LineString(l) => Geometry::LineString(line(l)),
            Geometry::MultiPoint(l) => Geometry::MultiPoint(line(l)),
            Geometry::Polygon(r) => Geometry::Polygon(rings(r)),
            Geometry::MultiLineString(r) => Geometry::MultiLineString(rings(r)),
            Geometry::MultiPolygon(p) => Geometry::MultiPolygon(p.iter().map(rings).collect()),
            Geometry::GeometryCollection(g) => {
                Geometry::GeometryCollection(g.iter().map(|g| g.map_coords(f)).collect())
            }
        }
    }
}
