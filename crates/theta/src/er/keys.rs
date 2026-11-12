//! The diagram's key map as a table: key and shift in, command out. A table (rather than a `match` in the
//! view) because the documentation, the tests and the view all read the same list.

/// Pixels the view moves per h/j/k/l press; shift moves four times as far.
pub const PAN_STEP: f32 = 64.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cmd {
    /// Move the view by this many screen pixels (positive x is right, positive y is down).
    Pan(f32, f32),
    ZoomIn,
    ZoomOut,
    /// Back to 100%.
    ZoomReset,
    Fit,
    Next,
    Prev,
    /// Open the selected table's rows.
    Open,
    /// Fuzzy jump to a table by name.
    Jump,
    /// Clear the selection (or close the jump box).
    Cancel,
    SaveSvg,
    CopyMermaid,
    /// Read the catalog again.
    Reload,
}

pub fn command(key: &str, shift: bool) -> Option<Cmd> {
    let step = if shift { PAN_STEP * 4.0 } else { PAN_STEP };
    Some(match key {
        "h" | "left" => Cmd::Pan(-step, 0.0),
        "l" | "right" => Cmd::Pan(step, 0.0),
        "k" | "up" => Cmd::Pan(0.0, -step),
        "j" | "down" => Cmd::Pan(0.0, step),
        "+" | "=" => Cmd::ZoomIn,
        "-" | "_" => Cmd::ZoomOut,
        "0" => Cmd::ZoomReset,
        "f" => Cmd::Fit,
        "tab" if shift => Cmd::Prev,
        "tab" => Cmd::Next,
        "enter" => Cmd::Open,
        "/" => Cmd::Jump,
        "escape" => Cmd::Cancel,
        "s" => Cmd::SaveSvg,
        "m" => Cmd::CopyMermaid,
        "r" => Cmd::Reload,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vi_keys_and_arrows_pan_the_same_way() {
        for (a, b) in [("h", "left"), ("l", "right"), ("j", "down"), ("k", "up")] {
            assert_eq!(command(a, false), command(b, false));
            assert!(command(a, false).is_some());
        }
        assert_eq!(command("h", false), Some(Cmd::Pan(-PAN_STEP, 0.0)));
        assert_eq!(command("j", true), Some(Cmd::Pan(0.0, PAN_STEP * 4.0)));
    }

    #[test]
    fn the_documented_keys_exist() {
        let table = [
            ("+", Cmd::ZoomIn),
            ("=", Cmd::ZoomIn),
            ("-", Cmd::ZoomOut),
            ("0", Cmd::ZoomReset),
            ("f", Cmd::Fit),
            ("tab", Cmd::Next),
            ("enter", Cmd::Open),
            ("/", Cmd::Jump),
            ("escape", Cmd::Cancel),
            ("s", Cmd::SaveSvg),
            ("m", Cmd::CopyMermaid),
            ("r", Cmd::Reload),
        ];
        for (k, c) in table {
            assert_eq!(command(k, false), Some(c), "{k}");
        }
        assert_eq!(command("tab", true), Some(Cmd::Prev));
        assert_eq!(command("x", false), None);
    }
}
