# Key bindings

`mod` means `cmd` on macOS and `ctrl` on Linux and Windows. Where this document says `ctrl`, the
Standard editor keymap accepts `cmd` as well. The editor keymap is chosen in the settings
(`"keymap": "helix"` or `"standard"`) or with *Toggle Keymap* in the command palette; the grid, map, plan
panel and explorer also adapt: in Helix mode they accept `h j k l`.

Sources of truth: `crates/quarry/src/keymap.rs` (application), `crates/quarry-editor/README.md` (editor),
and the view crates named in each section. Descriptions such as `Effect::RunStatementAtCursor` name
what the editor asks the application to do.

## Application (global)

Defined in `crates/quarry/src/keymap.rs`. The macOS menu bar repeats the same actions.

| Keys | Action |
| --- | --- |
| `mod-shift-p`, `F1` | command palette |
| `mod-p` | go to table |
| `mod-alt-c` | switch connection |
| `mod-n` | new console |
| `mod-shift-n` | new connection |
| `mod-w` | close tab |
| `mod-pagedown`, `mod-pageup` | next / previous tab |
| `mod-b` | toggle explorer |
| `mod-j` | toggle bottom panel |
| `mod-shift-e` | focus explorer |
| `F5` | refresh schema (the editor core also reports `F5` as Run All; see the note below) |
| `mod-k mod-t` | select theme |
| `mod-=`, `mod-+` / `mod--` / `mod-0` | zoom in / out / reset |
| `mod-enter` | run statement (selection if there is one) |
| `mod-shift-enter` | run all |
| `mod-.` | cancel the running query |
| `mod-e` | explain |
| `mod-shift-alt-e` | explain analyze |
| `mod-shift-f` | format document |
| `mod-q` | quit |

> **Note.** The application is still being integrated, so the table above is the binding set in `keymap.rs`
> today. Run-statement shortcuts are listed twice on purpose: the editor core handles `ctrl-enter` / `cmd-enter`,
> `ctrl-shift-enter`, `ctrl-space` and `F5` itself (see "Global" in the Helix section), and the application binds
> the same chords as actions.

## Explorer

Context `Explorer` (the connection tree).

| Keys | Action |
| --- | --- |
| `up` / `down`, `ctrl-p` / `ctrl-n` | previous / next row |
| `left` / `right` | collapse / expand |
| `enter` | activate (connect, open table) |
| `/` | filter |
| `h j k l` | collapse, next, previous, expand (Helix keymap only) |

## Completion popup (editor view)

| Keys | Action |
| --- | --- |
| `ctrl-space` (or `ctrl-x` in Helix insert mode, `space k` in Helix normal mode) | open completion |
| `up` / `down`, `ctrl-p` / `ctrl-n` | move selection |
| `pageup` / `pagedown` | move by 8 |
| `tab`, `enter` | accept |
| `esc` | dismiss |

## Pickers and the command palette

| Keys | Action |
| --- | --- |
| type | fuzzy filter |
| `up` / `down`, `ctrl-p` / `ctrl-n` | previous / next |
| `pageup` / `pagedown` | move a page |
| `ctrl-home` / `ctrl-end` | first / last |
| `enter` / `esc` | choose / close |

## Results grid

Handled in `crates/quarry-grid/src/input.rs` when the grid has focus. `mod` is `ctrl` or `cmd`.

| Keys | Action |
| --- | --- |
| arrows, `home` / `end`, `pageup` / `pagedown` | move the cell cursor (`shift` extends the selection) |
| `tab` / `shift-tab` | next / previous cell |
| `mod-home` / `mod-end` | first / last row |
| `mod-a` | select all |
| `mod-c`, `mod-shift-c` | copy cells as TSV; with `shift`, with the column headers |
| `mod-f`, `/` | open the filter box (`esc` closes it) |
| `enter`, `F2` | edit the cell if it is editable; otherwise follow a foreign-key reference (`enter` on a non-NULL value), or open the inspector |
| typing | start editing the cell with that character (Standard keymap only) |
| `delete` | mark the selected rows for deletion (editable grids) |
| `mod-0` | set the cell to NULL |
| `mod-z` | undo the last staged edit |
| `mod-enter` | submit the staged changes |
| `space` | toggle the value inspector |
| `esc` | collapse the selection |

Inside the cell editor: `tab` / `shift-tab` commit and move to the next / previous cell, `esc` cancels.

Helix keymap extras: `h j k l` move, `gg` / `G` first / last row, `v` toggles visual (extend) mode, `x` selects
the row, `y` copies, `u` undoes, `i` edits, `%` selects all, `mod-d` / `mod-u` half page down / up.

## Map

Handled in `crates/quarry-map-view/src/panel.rs` when the map has focus.

| Keys | Action |
| --- | --- |
| arrows (`h j k l` in Helix) | pan |
| `+` / `=`, `-` | zoom in / out |
| `0` | fit to data |
| `m` | toggle the measure tool |
| `esc` | close menus, leave the measure tool, or clear the selected feature |

Mouse: drag pans, wheel zooms, double-click zooms in (closes a measurement in the measure tool),
right-click opens a menu (copy coordinates, copy as WKT point, copy feature WKT).

## Plan panel

Handled in `crates/quarry-plan-view/src/panel.rs` in the tree and flame views.

| Keys | Action |
| --- | --- |
| `up` / `down` (`k` / `j` in Helix) | previous / next node |
| `pageup` / `pagedown` | move by 8 |
| `home` / `end` | first / last node |
| `left` / `right` (`h` / `l` in Helix) | collapse or go to parent / expand or go to child |
| `enter` | toggle the node details |
| `esc` | close the details |

## Editor: Helix keymap

Counts work wherever Helix takes one. In Select mode (`v`) every motion extends.

### Movement (Normal / Select)

| keys | action |
| --- | --- |
| `h j k l`, arrows | char left / line down / line up / char right (vertical moves keep the goal column) |
| `w b e` | next word start / previous word start / next word end |
| `W B E` | same for WORDs (whitespace separated); `alt-e` previous word end |
| `f t F T` + char | find / till char forward / backward (count = n-th); `alt-.` repeats |
| `home` `end`, `gh` `gl` `gs` | line start / line end / first non-blank |
| `gg` `ge` `G`, `<n>G` `<n>gg` | file start / last line / goto line |
| `gt` `gc` `gb` | top / middle / bottom line of the viewport |
| `g\|` | goto column |
| `gj` `gk` | line down / up |
| `]p` `[p` | next / previous paragraph |
| `[<space>` `]<space>` | add newline above / below |
| `ctrl-d` `ctrl-u`, `ctrl-f` `ctrl-b`, `pagedown` `pageup` | half page / page (uses `set_viewport`) |
| `zz zc zm` `zt` `zb` `zj` `zk` (and `Z`) | `Effect::Scroll` requests |
| `ctrl-o` `ctrl-i`/`tab` `ctrl-s` | jumplist back / forward / save |
| `mm` | jump to the matching bracket |

### Selections

| keys | action |
| --- | --- |
| `%` | select all |
| `x` | select line (repeat or count extends to following lines) |
| `X` `alt-x` | extend to line bounds / shrink to line bounds |
| `;` `alt-;` `alt-:` | collapse / flip / ensure forward |
| `,` `alt-,` | keep primary / remove primary |
| `(` `)` | rotate primary selection back / forward |
| `alt-(` `alt-)` | rotate selection *contents* |
| `C` `alt-C` | copy selection to next / previous line(s) |
| `s` `S` | select regex matches inside / split on regex (prompt, live preview) |
| `alt-s` | split on newlines |
| `K` `alt-K` | keep / remove selections matching a regex |
| `_` | trim whitespace from selections |
| `&` | align selections into columns |
| `alt--` `alt-_` | merge all / merge consecutive selections |
| `v` | toggle Select mode |

### Changes

| keys | action |
| --- | --- |
| `i a I A o O` | insert before / append after / at line start / at line end / open below / open above |
| `d` `alt-d` | delete (yank / no yank) |
| `c` `alt-c` | change (yank / no yank); changing whole lines leaves an empty line |
| `y p P` | yank / paste after / paste before (linewise values paste on whole lines) |
| `R` | replace selections with the register |
| `r<char>` | replace every grapheme (`r<ret>` = newline) |
| `~` `` ` `` `alt-`` ` | switch case / lowercase / uppercase |
| `>` `<` | indent / unindent by 4 spaces (count = levels, never indents empty lines) |
| `=` | `Effect::FormatSelection` |
| `J` `alt-J` | join lines / join and select the inserted spaces |
| `u` `U` | undo / redo (counts repeat) |
| `.` | repeat the last change (insert sessions, `r d c > <` ...) |
| `ctrl-a` `ctrl-x` | increment / decrement the number at or after the cursor (decimal with sign and zero padding, `0x` hex) |
| `ctrl-c` | toggle `--` line comments |
| `"<reg>` | choose a register for the next command |
| `Q` `q` | record / replay a macro (`"aQ` ... `"aq` for named ones) |

### Match mode

| keys | action |
| --- | --- |
| `ms<char>` | surround selections (`( ) [ ] { } < >` pairs, any other char doubled; `<ret>` = newlines) |
| `md<char>` / `mdm` | delete the surrounding pair / the closest pair |
| `mr<from><to>` | replace a surround |
| `mi<obj>` `ma<obj>` | select inside / around: `w W p ( ) b [ ] { } B < > ' " `` ` `` m`; counts pick outer levels; direction is preserved |

### Search

| keys | action |
| --- | --- |
| `/` `?` | search forward / backward (prompt with live preview; `Esc` restores selections; `up`/`down` = history) |
| `n` `N` | next / previous match (in Select mode the match is *added*) |
| `*` `alt-*` | use the selection (or the word under a bare cursor) as the search pattern, with / without `\b` |

Patterns are `regex`-crate syntax with smart-case (no uppercase letter in the pattern =
case-insensitive) and multi-line anchors.

### Insert mode

| keys | action |
| --- | --- |
| characters | insert at every cursor; `( [ { ' " `` ` `` auto-pair (not before a word char; quotes not after one), closers type over |
| `ret` | newline with auto-indent (copies leading whitespace, +4 after a line ending in `(` `[` `{`; between a pair it opens a block) |
| `tab` | four spaces (never triggers completion) |
| `backspace`, `ctrl-h` | delete a grapheme; a whole indent level in leading whitespace; both halves of an empty pair |
| `del`, `ctrl-d` | delete forward |
| `ctrl-w`, `alt-backspace` / `alt-d` | delete word back / forward |
| `ctrl-u` `ctrl-k` | delete to line start / end |
| arrows, `home`, `end`, `pageup`, `pagedown`, `alt-left/right` | move |
| `ctrl-x` | `Effect::RequestCompletion` |
| `esc` | back to Normal (see deviations for the cursor rule) |

### Space menu (`key_hints()` shows it)

| keys | action |
| --- | --- |
| `space f` | `OpenTablePicker` ("go to table") |
| `space b` | `OpenConnectionPicker` |
| `space ?` `space space` | `OpenCommandPalette` |
| `space r` | `RunStatementAtCursor`, or `RunSelection` when `selected_text()` is `Some` |
| `space R` | `RunAll` |
| `space e` `space E` | `ExplainStatement { analyze: false / true }` |
| `space k` | `RequestCompletion` |
| `space t` | `OpenThemePicker` |
| `space c` | toggle line comments |
| `space y` `space Y` | yank (all selections / primary) to the system clipboard |
| `space p` `space P` | paste the system clipboard after / before |
| `space x` | `CancelQuery` |
| `space /` | `OpenTablePicker` |
| `space g` `space d` | `FocusResults` / `FocusExplorer` |
| `space T` `space j` | `ToggleExplorer` / `ToggleBottomPanel` |

### Command line (`:`)

`:w :write` Save · `:q :quit` CloseTab · `:qa :quit-all` Quit · `:wq` Save + CloseTab ·
`:run :r` RunStatementAtCursor · `:run-all` RunAll · `:explain` · `:analyze` ·
`:format :fmt` FormatDocument · `:theme [name]` (picker, or `HostCommand` with a
name) · `:connect` OpenConnectionPicker · `:goto N`, `:N` go to line · `:sort`
(sort the selections' contents, or the lines of a multi-line selection) · `:new`
NewTab · `:cancel` CancelQuery. Anything else becomes `Effect::HostCommand(text)`.
`tab` completes command names (repeat to cycle).

Prompt editing: `left right home end ctrl-a ctrl-e ctrl-b ctrl-f alt-b alt-f`
movement, `backspace del ctrl-w ctrl-u ctrl-k alt-backspace` deletion,
`up down ctrl-p ctrl-n` history, `esc ctrl-c` cancel (backspace on an empty prompt
cancels too).

### Global (every mode of both keymaps)

`ctrl-enter` / `cmd-enter` RunStatementAtCursor (RunSelection with a selection) ·
`ctrl-shift-enter` RunAll · `ctrl-space` RequestCompletion · `F5` RunAll.

## Editor: Standard keymap

`ctrl` and `cmd` are interchangeable. Shift extends any motion.

| keys | action |
| --- | --- |
| typing | auto-pairs, auto-indent, typing an opener over a selection wraps it |
| arrows, `home`/`end` | move; `home` is smart (first non-blank, then column 0) |
| `ctrl-left/right`, `alt-left/right` | word motion |
| `ctrl-home/end` (`cmd-up/down`) | document start / end; up on the first line goes to the start, down on the last to the end |
| `pageup` `pagedown` | page (viewport) |
| `ctrl-a` | select all |
| `ctrl-c` `ctrl-x` `ctrl-v` | copy / cut / paste; with no selection copy and cut take the whole line, and pasting a whole-line copy inserts above the cursor line |
| `ctrl-z`, `ctrl-shift-z` `ctrl-y` | undo / redo |
| `backspace` `del`, `ctrl-backspace` `ctrl-del`, `cmd-backspace` | delete char / word / to line start |
| `tab` `shift-tab` | four spaces or indent selected lines / outdent |
| `ctrl-/` | toggle `--` comments |
| `ctrl-d` | select the word, then add the next occurrence as a new selection |
| `ctrl-shift-l` | select all occurrences |
| `ctrl-l` | select line (repeat extends) |
| `alt-up` `alt-down` | move line(s) |
| `ctrl-shift-d`, `alt-shift-down` | duplicate line(s) down |
| `ctrl-shift-k` | delete line(s) |
| `ctrl-alt-up/down`, `alt-shift-up` | add a cursor above / below |
| `esc` | `DismissCompletion`; collapses to the primary selection, then clears the selection |
| `ctrl-f` | search prompt (`find:`, prefilled with a single-line selection); `enter` accepts, `F3` / `shift-F3` next / previous |
| `ctrl-g` | goto-line prompt (`N` or `N:col`) |
| `ctrl-s` `ctrl-w` `ctrl-n` / `ctrl-t` | Save / CloseTab / NewTab |
| `ctrl-tab` `ctrl-shift-tab` | NextTab / PreviousTab |
| `ctrl-shift-p` `F1` | OpenCommandPalette |
| `ctrl-p` | OpenTablePicker |
| `ctrl-shift-i` `ctrl-alt-l` | FormatDocument |
| `ctrl-b` `ctrl-j` `ctrl-shift-e` | ToggleExplorer / ToggleBottomPanel / FocusExplorer |
| `ctrl-k ctrl-t` | OpenThemePicker (`pending_keys()` shows `ctrl-k`, `key_hints()` the follow-up) |
