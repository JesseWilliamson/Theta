//! Small layout helpers for overlays.

/// Top-left for a card of `card` size shown near `anchor` inside `container`: offset
/// down-right of the pointer, flipped to the other side when it would overflow, and
/// finally clamped so it never leaves the container.
pub fn card_origin(anchor: [f64; 2], card: [f64; 2], container: [f64; 2], gap: f64) -> [f64; 2] {
    let x =
        if anchor[0] + gap + card[0] <= container[0] { anchor[0] + gap } else { anchor[0] - gap - card[0] };
    let y =
        if anchor[1] + gap + card[1] <= container[1] { anchor[1] + gap } else { anchor[1] - gap - card[1] };
    [x.clamp(0.0, (container[0] - card[0]).max(0.0)), y.clamp(0.0, (container[1] - card[1]).max(0.0))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_down_right() {
        assert_eq!(card_origin([100.0, 100.0], [200.0, 80.0], [800.0, 600.0], 12.0), [112.0, 112.0]);
    }

    #[test]
    fn flips_at_edges() {
        let o = card_origin([780.0, 590.0], [200.0, 80.0], [800.0, 600.0], 12.0);
        assert_eq!(o, [568.0, 498.0]);
    }

    #[test]
    fn clamps_when_card_is_bigger_than_room() {
        let o = card_origin([10.0, 10.0], [300.0, 300.0], [250.0, 250.0], 12.0);
        assert_eq!(o, [0.0, 0.0]);
    }
}
