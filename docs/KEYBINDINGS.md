# Key bindings

`mod` means `cmd` on macOS and `ctrl` on Linux and Windows. Everything is checked against the code
(`crates/quarry/src/keymap.rs`, `crates/quarry/src/actions.rs`, the editor and view crates) and against the
running application. Platform differences are marked where they exist.

- **Key maps.** The editor has two key maps, Helix (default) and Standard. Choose in Settings › Editor ›
  *Keymap*, with *Toggle Keymap (Helix / Standard)* in the command palette, or `"keymap"` in
  `settings.json`. The grid, map, plan panel and explorer also take `h j k l` in Helix mode.
- **Where a key is handled.** The application binds the chords in [Application](#application-global)
  everywhere. GPUI delivers bound chords before the focused view sees its own key-down, so on a conflict the
  application's binding wins. [Shadowed editor keys](#shadowed-editor-keys) lists the conflicts.
- **Look it up in the app.** Settings › Keymap is a searchable list of every action and the editor keys
  for the current key map. The command palette shows the key next to each command.
- **macOS.** The application menu bar repeats the same actions. Linux and Windows have no menu bar; the
  menu button in the title bar and the gear menu offer the same items.

## Application (global)

Defined in `crates/quarry/src/keymap.rs`. `mod` is `ctrl` or `cmd`.

| Keys | Action |
| --- | --- |
| `mod-shift-p`, `F1` | command palette |
| `mod-p` | go to table |
| `mod-alt-c` | switch connection |
| `mod-n` | new console |
| `mod-shift-n` | new connection |
| `mod-o` | open SQL file |
| `mod-w` | close tab |
| `mod-pagedown`, `mod-pageup` | next / previous tab |
| `mod-b` | toggle explorer |
| `mod-j` | toggle bottom panel |
| `mod-shift-e` | focus explorer |
| `mod-1` | focus editor |
| `mod-2` | focus results (opens the bottom panel and the Results tab if needed) |
| `F5` | refresh schema |
| `mod-k mod-t` | select theme (a two-chord sequence) |
| `mod-=`, `mod-+` | zoom in (editor and results font) |
| `mod--` | zoom out |
| `mod-0` | reset zoom |
| `mod-,` | settings |
| `mod-enter` | run statement (selection if there is one); not bound while the results grid has focus, where it submits staged edits |
| `mod-shift-enter` | run all |
| `mod-.` | cancel the running query |
| `mod-e` | explain |
| `mod-shift-alt-e` | explain analyze |
| `mod-shift-f` | format document |
| `mod-q` | quit |

These actions have no key: *Connect…*, *Disconnect*, *Save Console As…*, *Begin Transaction*, *Commit
Transaction*, *Roll Back Transaction*, *Import Zed Theme…*, *Toggle Keymap*, *Open Settings File*. Use the
command palette. (`F5` also belongs here as *Refresh Schema*; it does **not** run all statements, see
below.)

### Shadowed editor keys

Because the application chords above are matched first, these editor keys never reach the editor. The
list was checked in the running application for `ctrl-b`, `ctrl-w`, `ctrl-k`, `ctrl-n` (in the completion
popup) and `F5`; the others follow from the same binding rule.

| Key | Application action that wins | Editor action it hides |
| --- | --- | --- |
| `F5` (all platforms, both key maps) | refresh schema | run all (the editor engine's own `F5`) |
| `ctrl-b` (Helix; Linux, Windows) | toggle explorer | page up |
| `ctrl-o` (Helix; Linux, Windows) | open SQL file | jumplist back |
| `ctrl-w` (Helix Insert mode; Linux, Windows) | close tab | delete word back |
| `ctrl-k` (Helix Insert mode; Linux, Windows) | first key of `ctrl-k ctrl-t` | delete to end of line |
| `ctrl-n`, `ctrl-p` (completion popup and prompt history; Linux, Windows) | new console / go to table | next / previous suggestion or history entry |
| `ctrl-e`, `ctrl-b`, `ctrl-w`, `ctrl-k` in the `:` and `/` prompts (Linux, Windows) | explain / toggle explorer / close tab / theme chord | move to end / move back / delete word / delete to end |

On macOS the application chords use `cmd`, so the Helix `ctrl-` keys and the popup's `ctrl-n` / `ctrl-p`
are not affected there; only `F5` is. In the Standard key map the conflicting chords are the same action
in both places (`ctrl-b`, `ctrl-j`, `ctrl-n`, `ctrl-w`, `ctrl-p`, `ctrl-shift-p`, `ctrl-shift-e`,
`ctrl-k ctrl-t`), so on Linux and Windows only `F5` and the popup's `ctrl-n` / `ctrl-p` differ. Use the
arrow keys in the completion popup.

## Explorer

Context `Explorer` (the connection tree). `ctrl-n` / `ctrl-p` are literal `ctrl` on every platform.

| Keys | Action |
| --- | --- |
| `up` / `down`, `ctrl-p` / `ctrl-n` | previous / next row |
| `left` / `right` | collapse / expand |
| `enter` | activate (connect, open table, expand) |
| `/` | focus the filter box |
| `esc` | return to the editor |
| `h j k l` | collapse, next, previous, expand (Helix key map only) |

In the filter box: `enter` or `down` moves into the tree, `esc` clears the filter and moves into the tree.
Double-click activates a row.

## Pickers and the command palette

The command palette, table picker, connection picker, theme picker and related-tables picker share these
keys. `ctrl-n` / `ctrl-p` are literal `ctrl` on every platform.

| Keys | Action |
| --- | --- |
| type | fuzzy filter |
| `up` / `down`, `ctrl-p` / `ctrl-n` | previous / next |
| `pageup` / `pagedown` | previous / next (the same as `ctrl-p` / `ctrl-n`) |
| `ctrl-home` / `ctrl-end` | first / last |
| `enter` / `esc` | choose / close |

## Completion popup

| Keys | Action |
| --- | --- |
| `ctrl-space` (Helix: `ctrl-x` in Insert mode, `space k` in Normal mode) | open completion by hand |
| `up` / `down` | move the selection |
| `pageup` / `pagedown` | move by 8 |
| `tab` | accept |
| `enter` | accept after you moved the selection, when the popup was opened by hand, or when the top suggestion is not a keyword; otherwise insert a newline |
| `esc` | close the popup (in Helix Insert mode a second `esc` leaves Insert mode) |

The popup opens by itself while typing a word or `.`, and after a space that follows `FROM`, `JOIN`, `ON`,
`INTO`, `UPDATE`, `TABLE` or a comma in a `FROM` list. (`ctrl-p` / `ctrl-n` are listed by the view crate but
shadowed in the application, see above.)

## Results grid

Handled in `crates/quarry-grid/src/input.rs` when the grid has focus. `mod` is `ctrl` or `cmd`.

| Keys | Action |
| --- | --- |
| arrows, `home` / `end`, `pageup` / `pagedown` | move the cell cursor (`shift` extends the selection) |
| `tab` / `shift-tab` | next / previous cell |
| `mod-home` / `mod-end` | first / last row |
| `mod-a` | select all |
| `mod-c` / `mod-shift-c` | copy cells as TSV / with the column headers |
| `mod-f`, `/` | open the filter box (`esc` closes it) |
| `enter`, `F2` | edit the cell if it is editable; otherwise follow a foreign-key reference (`enter` on a non-NULL value) or open the inspector |
| typing | start editing the cell with that character (Standard key map only) |
| `delete` | mark the selected rows for deletion (editable results) |
| `mod-z` | undo the last staged edit |
| `mod-enter` | submit the staged changes (opens **Apply changes?**) |
| `space` | toggle the value inspector |
| `esc` | collapse the selection; when nothing is left to collapse, return to the editor |
| `mod-1` | return to the editor |

Not available from the keyboard: the grid's `mod-0` ("set NULL") is shadowed by the global *Reset Editor
Font Size*; use *Set NULL* in the context menu.

Inside the cell editor: `enter` keeps the value, `tab` / `shift-tab` keep it and move to the next / previous
cell, `esc` cancels.

Helix key map extras: `h j k l` move, `gg` / `G` first / last row, `v` toggles visual (extend) mode, `x`
selects the row, `y` copies, `u` undoes, `i` edits, `%` selects all, `mod-d` / `mod-u` half page down / up.

Mouse: click, shift-click, drag; row numbers and headers select rows and columns; drag a header edge to
resize, double-click it to fit; `mod`-click a foreign-key value follows it; double-click edits; right-click
opens the context menu.

## Map

Handled in `crates/quarry-map-view/src/panel.rs` when the map has focus (chords with `ctrl`, `alt` or `cmd`
are ignored).

| Keys | Action |
| --- | --- |
| arrows (`h j k l` in Helix) | pan |
| `+` / `=`, `-` | zoom in / out |
| `0` | fit to data |
| `m` | toggle the measure tool |
| `esc` | close menus, leave the measure tool, or clear the selected feature |

Mouse: drag pans, wheel zooms, double-click zooms in (closes a measurement in the measure tool), click
picks a feature, right-click opens a menu (*Copy coordinates*, *Copy as WKT point*, *Copy feature WKT*).

## Plan panel

Handled in `crates/quarry-plan-view/src/panel.rs` in the tree and flame views (not in *Raw*).

| Keys | Action |
| --- | --- |
| `up` / `down` (`k` / `j` in Helix) | previous / next node |
| `pageup` / `pagedown` | move by 8 |
| `home` / `end` | first / last node |
| `left` / `right` (`h` / `l` in Helix) | collapse or go to parent / expand or go to child |
| `enter` | toggle the node details |
| `esc` | close the details |

## Editor

The tables below are the editor engine's own (`crates/quarry-editor/README.md`), which the application also
shows in Settings › Keymap. Read them together with [Shadowed editor keys](#shadowed-editor-keys).

### Helix key map

Counts work wherever Helix takes one. In Select mode (`v`) every motion extends.

#### Movement (Normal / Select)

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
| `ctrl-d` `ctrl-u`, `ctrl-f` `ctrl-b`, `pagedown` `pageup` | half page / page (`ctrl-b` is shadowed on Linux and Windows, see below) |
| `zz zc zm` `zt` `zb` `zj` `zk` (and `Z`) | scroll the view (centre, top, bottom, line down / up) |
| `ctrl-o` `ctrl-i`/`tab` `ctrl-s` | jumplist back / forward / save (`ctrl-o` is shadowed on Linux and Windows) |
| `mm` | jump to the matching bracket |

#### Selections

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

#### Changes

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
| `=` | format the selection |
| `J` `alt-J` | join lines / join and select the inserted spaces |
| `u` `U` | undo / redo (counts repeat) |
| `.` | repeat the last change (insert sessions, `r d c > <` ...) |
| `ctrl-a` `ctrl-x` | increment / decrement the number at or after the cursor (decimal with sign and zero padding, `0x` hex) |
| `ctrl-c` | toggle `--` line comments |
| `"<reg>` | choose a register for the next command |
| `Q` `q` | record / replay a macro (`"aQ` ... `"aq` for named ones) |

#### Match mode

| keys | action |
| --- | --- |
| `ms<char>` | surround selections (`( ) [ ] { } < >` pairs, any other char doubled; `<ret>` = newlines) |
| `md<char>` / `mdm` | delete the surrounding pair / the closest pair |
| `mr<from><to>` | replace a surround |
| `mi<obj>` `ma<obj>` | select inside / around: `w W p ( ) b [ ] { } B < > ' " `` ` `` m`; counts pick outer levels; direction is preserved |

#### Search

| keys | action |
| --- | --- |
| `/` `?` | search forward / backward (prompt with live preview; `Esc` restores selections; `up`/`down` = history) |
| `n` `N` | next / previous match (in Select mode the match is *added*) |
| `*` `alt-*` | use the selection (or the word under a bare cursor) as the search pattern, with / without `\b` |

Patterns are `regex`-crate syntax with smart-case (no uppercase letter in the pattern =
case-insensitive) and multi-line anchors.

#### Insert mode

| keys | action |
| --- | --- |
| characters | insert at every cursor; `( [ { ' " `` ` `` auto-pair (not before a word char; quotes not after one), closers type over |
| `ret` | newline with auto-indent (copies leading whitespace, +4 after a line ending in `(` `[` `{`; between a pair it opens a block) |
| `tab` | four spaces (never triggers completion) |
| `backspace`, `ctrl-h` | delete a grapheme; a whole indent level in leading whitespace; both halves of an empty pair |
| `del`, `ctrl-d` | delete forward |
| `ctrl-w`, `alt-backspace` / `alt-d` | delete word back / forward (`ctrl-w` is shadowed on Linux and Windows: it closes the tab) |
| `ctrl-u` `ctrl-k` | delete to line start / end (`ctrl-k` is shadowed on Linux and Windows: it waits for the `ctrl-k ctrl-t` chord) |
| arrows, `home`, `end`, `pageup`, `pagedown`, `alt-left/right` | move |
| `ctrl-x` | open completion |
| `esc` | back to Normal (see deviations for the cursor rule) |

#### Space menu

Press `space` in Normal mode; a which-key box lists what follows.

| keys | action |
| --- | --- |
| `space f` | go to table (picker) |
| `space b` | switch connection (picker) |
| `space ?` `space space` | command palette |
| `space r` | run statement under the cursor, or the selection when there is one |
| `space R` | run all |
| `space e` `space E` | explain / explain analyze |
| `space k` | completion |
| `space t` | theme picker |
| `space c` | toggle line comments |
| `space y` `space Y` | yank (all selections / primary) to the system clipboard |
| `space p` `space P` | paste the system clipboard after / before |
| `space x` | cancel the running query |
| `space /` | go to table (picker) |
| `space g` `space d` | focus results / focus explorer |
| `space T` `space j` | toggle explorer / toggle bottom panel |

#### Command line (`:`)

| command | action |
| --- | --- |
| `:w` `:write` | save the console to its `.sql` file (asks for a name the first time) |
| `:q` `:quit` | close the tab |
| `:qa` `:quit-all` | quit |
| `:wq` | save and close the tab |
| `:run` `:r` | run the statement under the cursor |
| `:run-all` | run all |
| `:explain` `:analyze` | explain / explain analyze |
| `:format` `:fmt` | format the document |
| `:theme` `:theme NAME` | theme picker / switch to the theme whose name matches `NAME` (`:theme tokyo`) |
| `:connect` `:connect NAME` | connection picker / switch this console to the connection called `NAME` |
| `:open` `:e` | open `.sql` files |
| `:goto N` `:N` | go to line `N` |
| `:sort` | sort the selections' contents, or the lines of a multi-line selection |
| `:new` | new console |
| `:cancel` | cancel the running query |

`tab` completes command names (repeat to cycle). An unknown command shows "Unknown command".

Prompt editing: `left right home end ctrl-a ctrl-e ctrl-b ctrl-f alt-b alt-f` movement,
`backspace del ctrl-w ctrl-u ctrl-k alt-backspace` deletion, `up down ctrl-p ctrl-n` history, `esc ctrl-c`
cancel (backspace on an empty prompt cancels too). In the application `ctrl-e`, `ctrl-b`, `ctrl-w`, `ctrl-k`,
`ctrl-p` and `ctrl-n` are bound globally on Linux and Windows and do not reach the prompt.

#### Global (every mode of both keymaps)

The engine itself handles `ctrl-enter` / `cmd-enter` (run statement; run selection when there is a
selection), `ctrl-shift-enter` (run all), `ctrl-space` (completion) and `F5` (run all). Inside Quarry the
application binds `mod-enter`, `mod-shift-enter` and `F5` before the editor sees them, and `F5` is *Refresh
Schema* there, not run all. See [Shadowed editor keys](#shadowed-editor-keys).

### Standard key map

`ctrl` and `cmd` are interchangeable. Shift extends any motion.

| keys | action |
| --- | --- |
| typing | auto-pairs, auto-indent, typing an opener over a selection wraps it |
| arrows, `home`/`end` | move; `home` is smart (first non-blank, then column 0) |
| `ctrl-left/right`, `alt-left/right` | word motion |
| `ctrl-home/end` (`cmd-up/down`) | document start / end; up on the first line goes to the start, down on the last to the end |
| `ctrl-up` `ctrl-down` | scroll the view by one line (on macOS `cmd-up/down` moves to the document start / end instead) |
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
| `esc` | closes the completion popup; collapses to the primary selection, then clears the selection |
| `ctrl-f` | search prompt (`find:`, prefilled with a single-line selection); `enter` accepts, `F3` / `shift-F3` next / previous |
| `ctrl-g` | goto-line prompt (`N` or `N:col`) |
| `ctrl-s` `ctrl-w` `ctrl-n` / `ctrl-t` | save / close tab / new console |
| `ctrl-tab` `ctrl-shift-tab` | next / previous tab |
| `ctrl-shift-p` `F1` | command palette |
| `ctrl-p` | go to table |
| `ctrl-shift-i` `ctrl-alt-l` | format document (`mod-shift-f` also works) |
| `ctrl-b` `ctrl-j` `ctrl-shift-e` | toggle explorer / toggle bottom panel / focus explorer |
| `ctrl-k ctrl-t` | theme picker (the status bar shows `ctrl-k` while it waits for the second chord) |
