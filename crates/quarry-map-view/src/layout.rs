//! Small layout helpers for overlays.

/// Top-left for a card of `card` size shown near `anchor` inside `container`: offset
/// down-right of the pointer and flipped to the other side when it would overflow. A card
/// that fits neither way keeps the side with more room and may extend past the container:
/// it is drawn on the window overlay layer, which clamps it to the window.
pub fn card_origin(anchor: [f64; 2], card: [f64; 2], container: [f64; 2], gap: f64) -> [f64; 2] {
    let place = |a: f64, c: f64, room: f64| {
        if a + gap + c <= room {
            a + gap
        } else if a - gap - c >= 0.0 {
            a - gap - c
        } else if a > room - a {
            // More room before the pointer than after it: end at the pointer.
            a - gap - c
        } else {
            a + gap
        }
    };
    [place(anchor[0], card[0], container[0]), place(anchor[1], card[1], container[1])]
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
    fn a_card_bigger_than_the_container_overflows_on_the_roomier_side() {
        // Near the top-left there is more room after the pointer: it extends down-right and
        // the window overlay clamps it, instead of being squeezed to (0, 0) and clipped.
        let o = card_origin([10.0, 10.0], [300.0, 300.0], [250.0, 250.0], 12.0);
        assert_eq!(o, [22.0, 22.0]);
        // Near the bottom-right it goes up-left, like a card that fits.
        let o = card_origin([240.0, 240.0], [300.0, 300.0], [250.0, 250.0], 12.0);
        assert_eq!(o, [-72.0, -72.0]);
    }
}
