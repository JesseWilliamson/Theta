//! Actions used only where a plain key handler cannot see the key: while the
//! inline editor or the filter input has focus, gpui-kit's `Input` binds Tab
//! and Escape itself, so the grid has to bind more specific contexts.

use gpui_kit::{App, KeyBinding, actions};

actions!(quarry_grid, [EditNext, EditPrev, EditCancel, FilterClose, NextCell, PrevCell]);

struct Bound;
impl gpui_kit::Global for Bound {}

/// Idempotent: every grid calls it, only the first binds.
pub(crate) fn bind_keys(cx: &mut App) {
    if cx.has_global::<Bound>() {
        return;
    }
    cx.set_global(Bound);
    cx.bind_keys([
        KeyBinding::new("tab", EditNext, Some("GridCellEditor > Input")),
        KeyBinding::new("shift-tab", EditPrev, Some("GridCellEditor > Input")),
        KeyBinding::new("escape", EditCancel, Some("GridCellEditor > Input")),
        KeyBinding::new("escape", FilterClose, Some("GridFilter > Input")),
        // gpui-kit's Root binds Tab to focus traversal; inside the grid it
        // must move the cell cursor instead.
        KeyBinding::new("tab", NextCell, Some("ResultGrid")),
        KeyBinding::new("shift-tab", PrevCell, Some("ResultGrid")),
    ]);
}
