# quarry-grid

The query-results data grid: `ResultGrid`, a GPUI entity that displays one
`quarry_core::ResultSet`. It knows nothing about connections or SQL; it emits
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
`DropdownMenu`, `Button`, `Tooltip`, `Scrollbar`, `Progress`, and `quarry-ui`'s
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
  columns: link icon in the header, ctrl-click / Enter / double-click emits
  `FollowReference`.
- JSON in the inspector is syntax-coloured but not mouse-selectable (gpui-kit's
  selectable text areas cannot take per-range colours); the copy button copies it
  and plain text/EWKT use a read-only selectable `Textarea`.

## Debugging

`QUARRY_GRID_TIMING=1` prints the time spent building the element tree in
`render` every 60 frames.
