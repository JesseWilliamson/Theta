//! WCAG contrast helpers. Themes (especially imported third-party ones) can
//! ship a "muted" colour that is unreadable on their own background; the grid
//! draws lots of muted text (types, NULL, row numbers), so it nudges such
//! colours toward the foreground until they reach a minimum ratio.

use gpui_kit::{Hsla, Rgba};

fn channel(c: f32) -> f32 {
    if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2])
}

/// WCAG contrast ratio, 1.0 (identical) to 21.0 (black on white).
pub fn ratio(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Blend `color` toward `toward` in 5% steps until it reaches `min` contrast
/// against `background` (or is `toward` itself). Returns `color` unchanged
/// when it already passes.
pub fn ensure_rgb(color: [f32; 3], background: [f32; 3], toward: [f32; 3], min: f32) -> [f32; 3] {
    if ratio(color, background) >= min {
        return color;
    }
    let mut out = color;
    for step in 1..=20 {
        out = mix(color, toward, step as f32 * 0.05);
        if ratio(out, background) >= min {
            break;
        }
    }
    out
}

fn rgb_of(c: Hsla) -> [f32; 3] {
    let r: Rgba = c.to_rgb();
    [r.r, r.g, r.b]
}

pub fn ensure_contrast(color: Hsla, background: Hsla, toward: Hsla, min: f32) -> Hsla {
    let adjusted = ensure_rgb(rgb_of(color), rgb_of(background), rgb_of(toward), min);
    let mut out: Hsla = Rgba { r: adjusted[0], g: adjusted[1], b: adjusted[2], a: color.a }.into();
    out.a = color.a;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: [f32; 3] = [0.0; 3];
    const WHITE: [f32; 3] = [1.0; 3];

    #[test]
    fn known_ratios() {
        assert!((ratio(BLACK, WHITE) - 21.0).abs() < 0.01);
        assert!((ratio(WHITE, WHITE) - 1.0).abs() < 0.001);
        // #767676 on white is the classic 4.54:1 AA boundary grey
        let grey = [0x76 as f32 / 255.0; 3];
        assert!((ratio(grey, WHITE) - 4.54).abs() < 0.02);
    }

    #[test]
    fn passing_colours_are_untouched() {
        let c = [0.1, 0.1, 0.1];
        assert_eq!(ensure_rgb(c, WHITE, BLACK, 4.5), c);
    }

    #[test]
    fn weak_colours_are_pulled_toward_the_foreground() {
        // Tokyo Night's muted foreground (#565f89) on its background (#1a1b26): 2.76:1
        let bg = [0x1a as f32 / 255.0, 0x1b as f32 / 255.0, 0x26 as f32 / 255.0];
        let muted = [0x56 as f32 / 255.0, 0x5f as f32 / 255.0, 0x89 as f32 / 255.0];
        let fg = [0xc0 as f32 / 255.0, 0xca as f32 / 255.0, 0xf5 as f32 / 255.0];
        assert!(ratio(muted, bg) < 3.0);
        let fixed = ensure_rgb(muted, bg, fg, 4.5);
        assert!(ratio(fixed, bg) >= 4.5, "{}", ratio(fixed, bg));
        // and it is still a "muted" colour, not the full foreground
        assert!(ratio(fixed, bg) < ratio(fg, bg));
    }

    #[test]
    fn terminates_when_unreachable() {
        // light grey text toward white on white can never pass; must not loop
        let out = ensure_rgb([0.9; 3], WHITE, WHITE, 4.5);
        assert_eq!(out, WHITE);
    }
}
