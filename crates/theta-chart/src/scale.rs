//! Mapping data to pixels.

/// A linear map from a data interval to a pixel interval. The pixel interval may run backwards (the y
/// axis, where larger values are higher, which is a smaller pixel y).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Linear {
    pub d0: f64,
    pub d1: f64,
    pub r0: f64,
    pub r1: f64,
}

impl Linear {
    pub fn new(d0: f64, d1: f64, r0: f64, r1: f64) -> Self {
        Self { d0, d1, r0, r1 }
    }

    /// An empty data interval maps everything to the middle of the range.
    pub fn map(&self, v: f64) -> f64 {
        if self.d1 == self.d0 {
            return (self.r0 + self.r1) / 2.0;
        }
        self.r0 + (v - self.d0) / (self.d1 - self.d0) * (self.r1 - self.r0)
    }

    pub fn invert(&self, px: f64) -> f64 {
        if self.r1 == self.r0 {
            return self.d0;
        }
        self.d0 + (px - self.r0) / (self.r1 - self.r0) * (self.d1 - self.d0)
    }
}

/// Equal slots for categories: slot `i` of `n` over `[r0, r1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub n: usize,
    pub r0: f64,
    pub r1: f64,
}

impl Band {
    pub fn width(&self) -> f64 {
        if self.n == 0 { 0.0 } else { (self.r1 - self.r0) / self.n as f64 }
    }

    pub fn start(&self, i: usize) -> f64 {
        self.r0 + self.width() * i as f64
    }

    pub fn centre(&self, i: usize) -> f64 {
        self.start(i) + self.width() / 2.0
    }

    /// The slot that contains pixel `px`, clamped to the ends.
    pub fn index_at(&self, px: f64) -> Option<usize> {
        if self.n == 0 {
            return None;
        }
        let i = ((px - self.r0) / self.width()).floor();
        Some(i.clamp(0.0, (self.n - 1) as f64) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_maps_both_ways_even_when_flipped() {
        let s = Linear::new(0.0, 100.0, 300.0, 0.0);
        assert_eq!(s.map(0.0), 300.0);
        assert_eq!(s.map(100.0), 0.0);
        assert_eq!(s.map(25.0), 225.0);
        assert_eq!(s.invert(225.0), 25.0);
        for v in [-10.0, 3.3, 99.9] {
            assert!((s.invert(s.map(v)) - v).abs() < 1e-9);
        }
    }

    #[test]
    fn an_empty_interval_does_not_divide_by_zero() {
        let s = Linear::new(5.0, 5.0, 0.0, 100.0);
        assert_eq!(s.map(5.0), 50.0);
        assert_eq!(Linear::new(0.0, 1.0, 7.0, 7.0).invert(7.0), 0.0);
    }

    #[test]
    fn bands_split_evenly_and_find_the_slot_under_a_pixel() {
        let b = Band { n: 4, r0: 100.0, r1: 200.0 };
        assert_eq!((b.width(), b.start(1), b.centre(1)), (25.0, 125.0, 137.5));
        assert_eq!(b.index_at(100.0), Some(0));
        assert_eq!(b.index_at(149.9), Some(1));
        assert_eq!(b.index_at(1000.0), Some(3), "clamped");
        assert_eq!(b.index_at(-5.0), Some(0));
        assert_eq!(Band { n: 0, r0: 0.0, r1: 1.0 }.index_at(0.5), None);
    }
}
