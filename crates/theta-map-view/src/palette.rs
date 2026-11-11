//! Categorical layer colours.
//!
//! Six hues spread around the wheel with mid lightness and high saturation: readable on
//! both a pale OSM basemap and a dimmed dark one. Pure data so it can be tested; the
//! renderer converts to `Hsla`.

/// Hue/saturation/lightness, each `0..=1` (hue as a fraction of the wheel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsl {
    /// Hue.
    pub h: f32,
    /// Saturation.
    pub s: f32,
    /// Lightness.
    pub l: f32,
}

const HUES_DEG: [f32; 6] = [212.0, 28.0, 148.0, 320.0, 262.0, 180.0];

/// Colour of layer `layer` (wraps after six). Dark themes get slightly lighter colours.
pub fn layer_color(layer: usize, dark: bool) -> Hsl {
    Hsl { h: HUES_DEG[layer % HUES_DEG.len()] / 360.0, s: 0.78, l: if dark { 0.60 } else { 0.50 } }
}

impl Hsl {
    /// A brighter, slightly more saturated variant, for the hovered feature.
    pub fn brighter(self) -> Hsl {
        Hsl { h: self.h, s: (self.s + 0.1).min(1.0), l: (self.l + 0.12).min(0.85) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_distinct_hues_then_wraps() {
        let hues: Vec<u32> = (0..6).map(|i| (layer_color(i, false).h * 360.0).round() as u32).collect();
        let mut sorted = hues.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 6);
        assert_eq!(layer_color(6, false), layer_color(0, false));
    }

    #[test]
    fn hues_are_well_separated() {
        let mut h: Vec<f32> = (0..6).map(|i| layer_color(i, false).h * 360.0).collect();
        h.sort_by(f32::total_cmp);
        for w in h.windows(2) {
            assert!(w[1] - w[0] >= 30.0, "{h:?}");
        }
    }

    #[test]
    fn dark_is_lighter_and_hover_is_brighter() {
        assert!(layer_color(0, true).l > layer_color(0, false).l);
        let c = layer_color(2, false);
        assert!(c.brighter().l > c.l);
        assert!(c.brighter().l <= 0.85 && c.brighter().s <= 1.0);
    }
}
