//! Colour palettes for layer styling: sequential, diverging and categorical ramps.
//!
//! Stops are the published viridis/magma/cividis maps (sampled), ColorBrewer Blues and
//! RdBu, and the Tableau 10 categorical set. Pure data so the legend, the renderer and the
//! tests share one definition.

use serde::{Deserialize, Serialize};

/// A named palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PaletteId {
    /// Perceptually uniform, dark purple to yellow.
    Viridis,
    /// Black to cream through purple and orange.
    Magma,
    /// Colour-blind safe blue to yellow.
    Cividis,
    /// Light to dark blue.
    Blues,
    /// Diverging red, white, blue.
    RdBu,
    /// Ten categorical colours.
    Tableau10,
}

const VIRIDIS: &[[u8; 3]] = &[
    [68, 1, 84],
    [72, 40, 120],
    [62, 74, 137],
    [49, 104, 142],
    [38, 130, 142],
    [31, 158, 137],
    [53, 183, 121],
    [110, 206, 88],
    [181, 222, 43],
    [253, 231, 37],
];
const MAGMA: &[[u8; 3]] = &[
    [0, 0, 4],
    [28, 16, 68],
    [79, 18, 123],
    [129, 37, 129],
    [181, 54, 122],
    [229, 80, 100],
    [251, 135, 97],
    [254, 194, 135],
    [252, 253, 191],
];
const CIVIDIS: &[[u8; 3]] =
    &[[0, 32, 77], [42, 60, 111], [87, 92, 109], [124, 123, 120], [166, 156, 117], [211, 191, 98], [255, 234, 70]];
const BLUES: &[[u8; 3]] = &[
    [247, 251, 255],
    [222, 235, 247],
    [198, 219, 239],
    [158, 202, 225],
    [107, 174, 214],
    [66, 146, 198],
    [33, 113, 181],
    [8, 81, 156],
    [8, 48, 107],
];
const RDBU: &[[u8; 3]] = &[
    [103, 0, 31],
    [178, 24, 43],
    [214, 96, 77],
    [244, 165, 130],
    [253, 219, 199],
    [247, 247, 247],
    [209, 229, 240],
    [146, 197, 222],
    [67, 147, 195],
    [33, 102, 172],
    [5, 48, 97],
];
const TABLEAU10: &[[u8; 3]] = &[
    [78, 121, 167],
    [242, 142, 43],
    [225, 87, 89],
    [118, 183, 178],
    [89, 161, 79],
    [237, 201, 72],
    [176, 122, 161],
    [255, 157, 167],
    [156, 117, 95],
    [186, 176, 172],
];

/// Neutral grey for "Other" and missing values.
pub const OTHER_GREY: [u8; 3] = [140, 140, 148];

impl PaletteId {
    /// Every palette, in menu order.
    pub const ALL: [PaletteId; 6] = [
        PaletteId::Viridis,
        PaletteId::Magma,
        PaletteId::Cividis,
        PaletteId::Blues,
        PaletteId::RdBu,
        PaletteId::Tableau10,
    ];

    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            PaletteId::Viridis => "Viridis",
            PaletteId::Magma => "Magma",
            PaletteId::Cividis => "Cividis",
            PaletteId::Blues => "Blues",
            PaletteId::RdBu => "Red-Blue",
            PaletteId::Tableau10 => "Tableau 10",
        }
    }

    /// Categorical palettes pick discrete colours instead of interpolating.
    pub fn is_categorical(self) -> bool {
        self == PaletteId::Tableau10
    }

    fn stops(self) -> &'static [[u8; 3]] {
        match self {
            PaletteId::Viridis => VIRIDIS,
            PaletteId::Magma => MAGMA,
            PaletteId::Cividis => CIVIDIS,
            PaletteId::Blues => BLUES,
            PaletteId::RdBu => RDBU,
            PaletteId::Tableau10 => TABLEAU10,
        }
    }

    /// Colour at `t` in `0..=1` (clamped), linearly interpolated between stops. Categorical
    /// palettes return the stop nearest `t`.
    pub fn sample(self, t: f32, reversed: bool) -> [u8; 3] {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        let t = if reversed { 1.0 - t } else { t };
        let s = self.stops();
        let x = t * (s.len() - 1) as f32;
        if self.is_categorical() {
            return s[x.round() as usize];
        }
        let i = (x.floor() as usize).min(s.len() - 2);
        let f = x - i as f32;
        let mix = |k: usize| (f32::from(s[i][k]) * (1.0 - f) + f32::from(s[i + 1][k]) * f).round() as u8;
        [mix(0), mix(1), mix(2)]
    }

    /// `n` class colours evenly spread over the ramp (or the first `n` categorical colours).
    pub fn classes(self, n: usize, reversed: bool) -> Vec<[u8; 3]> {
        if self.is_categorical() {
            let s = self.stops();
            let mut v: Vec<[u8; 3]> = (0..n).map(|i| s[i % s.len()]).collect();
            if reversed {
                v.reverse();
            }
            return v;
        }
        (0..n).map(|i| self.sample(if n > 1 { i as f32 / (n - 1) as f32 } else { 0.5 }, reversed)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ends_match_the_published_stops() {
        assert_eq!(PaletteId::Viridis.sample(0.0, false), [68, 1, 84]);
        assert_eq!(PaletteId::Viridis.sample(1.0, false), [253, 231, 37]);
        assert_eq!(PaletteId::Viridis.sample(1.0, true), [68, 1, 84]);
    }

    #[test]
    fn midpoints_interpolate_and_clamp() {
        let a = PaletteId::Blues.sample(0.5, false);
        assert!(a[0] < 247 && a[0] > 8);
        assert_eq!(PaletteId::Blues.sample(-3.0, false), PaletteId::Blues.sample(0.0, false));
        assert_eq!(PaletteId::Blues.sample(f32::NAN, false), PaletteId::Blues.sample(0.0, false));
    }

    #[test]
    fn diverging_centre_is_near_white() {
        let c = PaletteId::RdBu.sample(0.5, false);
        assert!(c.iter().all(|v| *v > 240));
    }

    #[test]
    fn class_colours_are_distinct_and_ordered() {
        let c = PaletteId::Viridis.classes(7, false);
        assert_eq!(c.len(), 7);
        for w in c.windows(2) {
            assert_ne!(w[0], w[1]);
        }
        assert_eq!(PaletteId::Viridis.classes(7, true), c.iter().rev().copied().collect::<Vec<_>>());
    }

    #[test]
    fn categorical_picks_discrete_colours() {
        let c = PaletteId::Tableau10.classes(4, false);
        assert_eq!(c[0], [78, 121, 167]);
        assert_eq!(c[1], [242, 142, 43]);
        assert_eq!(PaletteId::Tableau10.classes(12, false).len(), 12);
    }
}
