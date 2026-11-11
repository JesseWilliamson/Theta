# theta-grid

The query-results data grid: `ResultGrid`, a GPUI entity that displays one
`theta_core::ResultSet`. It knows nothing about connections or SQL; it emits
`GridEvent`s (submit staged changes, export, show on map, follow a foreign key)
and the host acts on them.

## Why a custom grid instead of gpui-kit's `DataTable`

`DataTable` (virtualised through `uniform_list` and its own horizontal window)
is built around *row* and *column* selection with at most a single "selected
cell" and a delegate that renders one cell at a time. The requirements here
needed things its delegate seam cannot express:

- rectangular **cell ranges** (click, shift-click, drag, keyboard extension) and
  one translucent range overlay rather than per-cell selected state;
- a frozen **row-number gutter that is itself a selection surface** (click and
  drag select rows) next to a sticky header with its own click targets (sort
  affordance, column select, resize edge, auto-fit on double click);
- per-cell **pending-edit styling** (modified tint and corner marker, new rows,
  struck-through deleted rows) decided from a side table, not the data;
- an inline editor positioned over a cell, auto-scroll to keep the cursor in view
  and drag auto-scroll, which need to own the scroll offset.

So the grid owns its scroll offset (`scroll.rs`, which implements gpui-kit's
`ScrollbarHandle` so the stock `Scrollbar` still draws the bars) and builds only
the visible window of rows and columns every frame: a visible-window computation
over `ColumnLayout`/`visible_rows`, absolutely positioned row elements, overlay
rectangles for selection, and a bounded cell-text cache. Frame cost is
proportional to the viewport (about 40 x 15 cells), not to the 6,000,000 cells of
a 100,000 x 60 result.

gpui-kit is still used for everything that is not the grid surface: `Input` and
`Textarea` (filter, cell editor, inspector), `PopupMenu`/`ContextMenu`/
`DropdownMenu`, `Button`, `Tooltip`, `Scrollbar`, `Progress`, and `theta-ui`'s
`tool_button`, `PanelHeader`, theme colours and `Settings`.

## Layout of the crate

Pure logic (no GPUI types, unit-tested): `compare` (type-aware and natural order),
`filter`, `view` (filter then sort into an index vector), `selection`, `layout`,
`format`, `parse` (text to `Value` per `TypeKind`), `pending` (staged edits),
`copy` (TSV), `inspect` (inspector content, JSON re-indenter), `summary`.

GPUI layer: `grid` (state), `render`, `input` (keys/pointer), `editing`,
`inspector`, `scroll`, `actions`.

## Behaviour notes

- Sorting and filtering produce an index vector over the original rows; results
  larger than 20,000 rows are processed on the background executor and a newer
  request supersedes an older one.
- NULLs sort last in both directions.
- Header click targets: the arrow at the right of the header cycles sort
  (shift adds a secondary key); the rest of the header selects the column.
  Right-click on a header offers explicit sort/auto-fit/copy-name.
- Edits are staged against the *original* rows; `Update`/`Delete` keys come from
  the pre-edit values. Names in `RowChange` are the source table's column names
  (`Column::origin.column` when present).
- `GridMeta::reference_columns` (added to the brief's API) marks foreign-key
  columns: link icon in the header. Ctrl-click (cmd-click on macOS) and the
  context menu's *Follow Reference* always emit `FollowReference`; Enter and
  double-click do so only where the cell cannot be edited, because on an
  editable cell they start the cell editor.
- `ResultGrid::select_row(row, cx)` selects and scrolls to a row of the result
  set by its original index, whatever the current sort or filter shows (it does
  nothing when a filter hides the row) and does not take focus. The application
  uses it to follow a click on the map.
- Keys: `mod-enter` submits staged edits while the grid has focus (the
  application does not bind its own `mod-enter` there). `mod-0` ("set NULL") is
  only reachable from the context menu in the application, because the
  application's global `mod-0` (reset zoom) is matched first.
- JSON in the inspector is syntax-coloured but not mouse-selectable (gpui-kit's
  selectable text areas cannot take per-range colours); the copy button copies it
  and plain text/EWKT use a read-only selectable `Textarea`.

## Performance (100,000 rows x 60 columns, software rendering)

Measured with `examples/grid_demo --rows 100000 --cols 60 --autoscroll` under
Xvfb + llvmpipe (no GPU, shared 4-core box under load), debug-profile with the
workspace's `opt-level = 1/2`:

| What | Result |
| --- | --- |
| `set_result` (column auto-fit from the first 200 rows, layout) | 1.1 ms |
| element-tree build per frame (`render`), ~450-550 visible cells | 1.3-2.1 ms avg, 2.7-5.8 ms worst |
| scripted diagonal scroll, 300 steps | frames every 47-180 ms; wall time is GPUI layout plus llvmpipe rasterising 1600x1000, not the grid |
| idle CPU | ~0 |
| sort 100k rows by int / decimal / text | 52 / 172 / 264 ms (on the background executor above 20k rows) |
| filter 100k rows x 3 cells | 10 ms |

The number of built cells does not depend on the result size (it is the
viewport), which is the point of the windowing; `THETA_GRID_TIMING=1` prints
build time and frame period every 20 frames so this can be re-checked.

## Not done / known gaps

- Column reordering and hiding.
- The export file dialogs and writing are the host's job (`GridEvent::Export`).
- Column auto-fit is estimated from character counts (monospace body, a fixed
  per-character estimate for the proportional header), not measured per header.
- Geometry cells are not rendered as thumbnails; `describe()` text with a pin icon.
- Only tested on Linux (X11).

## Debugging

`THETA_GRID_TIMING=1` prints the time spent building the element tree in
`render` and the wall-clock frame period every 20 frames.
