# Key bindings

`mod` means `cmd` on macOS and `ctrl` on Linux and Windows. Everything is checked against the code
(`crates/theta/src/keymap/`, `crates/theta/src/actions.rs`, the editor and view crates) and against the
running application. Platform differences are marked where they exist.

- **Key maps.** The editor has three key maps: Helix (default), Vim and Standard. Choose in Settings › Editor ›
  *Keymap*, with *Select Keymap…* (a small picker) or *Use Helix / Vim / Standard Keymap* in the command palette,
  in the buttons on the welcome view, or with `"keymap": "helix" | "vim" | "standard"` in `settings.json`. The
  grid, map, plan panel and explorer also take `h j k l` (and the other Helix keys of the grid) in either
  modal key map, Helix or Vim.
- **Where a key is handled.** The application binds the chords in [Application](#application-global),
  but a focused view that uses the same keystroke itself always wins inside that view: in the SQL editor
  the editing engine's keys for the active key map win, in the results grid the grid's keys win. See
  [Who wins a key](#who-wins-a-key).
- **Look it up in the app.** Settings › Keymap is a searchable list of every action and the editor keys
  for the current key map. The command palette shows the key next to each command.
- **macOS.** The application menu bar repeats the same actions. Linux and Windows have no menu bar; the
  menu button in the title bar and the gear menu offer the same items.

## Application (global)

Defined in `crates/theta/src/keymap/mod.rs`. `mod` is `ctrl` or `cmd`. Each chord is active everywhere except
where a focused view owns the keystroke (see [Who wins a key](#who-wins-a-key)).

| Keys | Action |
| --- | --- |
| `mod-shift-p`, `F1` | command palette |
| `mod-p` | go to table |
| `mod-alt-c` | switch connection |
| `mod-n` | new console |
| `mod-shift-n` | new connection |
| `mod-o` | open SQL file |
| `mod-w` | close tab (the console goes on the reopen stack) |
| `mod-shift-t` | reopen the last closed tab (text, connection and cursor; the last 20 are kept across restarts) |
| `mod-pagedown`, `mod-pageup` | next / previous tab |
| `mod-b` | toggle explorer |
| `mod-j` | toggle bottom panel |
| `mod-shift-e` | focus explorer |
| `mod-1` | focus editor |
| `mod-2` | focus results (opens the bottom panel and the Results tab if needed) |
| `F5` | refresh schema, only while the explorer has focus (in the editor `F5` is *run all*) |
| `mod-alt-r` | refresh schema, from anywhere |
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

These actions have no key: *Surround Selection with…*, *Edit Live Templates…*, *Transactions: Auto-commit*, *Transactions: Manual*, *Transaction Isolation Level…*, *New Connection Folder…*, *Move Connection to Folder…*, *Toggle Query Parameters*, *Connect…*, *Disconnect*, *Save Console As…*, *Begin Transaction*, *Commit
Transaction*, *Roll Back Transaction*, *Import Zed Theme…*, *Select Keymap…* and *Use Helix / Vim / Standard
Keymap*, *Open Settings File*. Use the
command palette. *Refresh Schema* and *Reopen Closed Tab* are also in the palette and menus; `:reopen` does the
latter from the Helix and Vim command lines.

### Who wins a key

GPUI matches key bindings before the focused view's own key-down handler. Left alone, a global binding
would steal the editor's keys: `ctrl-w` would close the tab (and the console with it) instead of deleting a
word. So there is one rule:

> **While the SQL editor is focused, the editing engine's keys for the active key map, mode and prompt/popup
> state always win.** An application chord on the same keystroke applies only outside the editor, or in the
> editor states where the engine does not use that key. In the Standard key map the engine emits the very
> same action itself (`ctrl-w` is *close tab* there), so every keystroke has exactly one path.

The same holds for the other focused views: the results grid keeps its `mod-0` (set NULL), `mod-enter`
(submit), `mod-a`, `mod-c`, `mod-f`, `mod-z`, `mod-home` / `mod-end` and (Helix, Vim) `mod-d` / `mod-u`; the explorer
and the pickers keep `ctrl-n` / `ctrl-p`. The map and the plan panel use no chords, so the application's apply
there.

How it is enforced: the editor publishes its state as key context (`ThetaEditor`,
`keymap=helix|vim|standard`, `mode=normal|insert|select|visual|visual-line|visual-block|replace|standard`,
`prompt` while the `:` or `/` bar is open, `completion` while the popup is open, and in Vim `pending` while an
operator, `g`, `z`, a register or a count waits for the rest of its command). The application asks the engine which keystrokes it uses in which state and adds
`!(ThetaEditor && <those states>)` to every global binding on such a keystroke
(`crates/theta/src/keymap/ownership.rs`). Nothing is a hand-written list, and a headless test
(`every_global_binding_is_handled_by_exactly_one_side`, `focused_views_win_their_own_keys…`) presses every
application binding in every state of all three key maps (Vim: Normal, Insert, Visual, V-LINE, V-BLOCK, Replace,
operator-pending, prompt open, popup open) and checks that exactly one side handled it.

What that means on Linux and Windows (on macOS the application uses `cmd`, so Helix's `ctrl-` keys never
collide):

| Keystroke | Helix, editor focused | Standard, editor focused | Elsewhere |
| --- | --- | --- | --- |
| `ctrl-w` | Insert: delete word back; prompt: delete word; Normal: close tab | close tab (engine) | close tab |
| `ctrl-b` | Normal / Select: page up; prompt: move back | toggle explorer (engine) | toggle explorer |
| `ctrl-k` | Insert: delete to end of line (no chord wait) | `ctrl-k ctrl-t` theme chord (engine) | theme chord |
| `ctrl-o` | Normal / Select: jumplist back; Insert: open SQL file | open SQL file (not an engine key) | open SQL file |
| `ctrl-n` / `ctrl-p` | completion popup open or prompt open: next / previous; otherwise new console / go to table | same (popup and prompt first) | new console / go to table |
| `ctrl-e` | prompt open: move to end; otherwise explain | prompt open: move to end; otherwise explain | explain |
| `F5` | run all | run all | explorer focused: refresh schema |
| `ctrl-pageup` / `ctrl-pagedown` | previous / next tab | previous / next tab (engine) | previous / next tab |

**Vim.** The editor owns every plain `ctrl` letter that Vim gives a meaning in some mode, in every Vim state: `ctrl-w`
(Vim's window prefix; the engine has no windows, so it does nothing in Normal and Visual mode and deletes a word in
Insert mode), `ctrl-b` `ctrl-f` `ctrl-d` `ctrl-u` `ctrl-e` `ctrl-y` (scrolling), `ctrl-r` (redo), `ctrl-o` `ctrl-i` (jumplist),
`ctrl-a` `ctrl-x` (numbers), `ctrl-v` (block mode) and `ctrl-n` `ctrl-p` (down / up, completion in Insert mode). On
Linux and Windows that means `mod-w`, `mod-b`, `mod-e`, `mod-n`, `mod-o` and `mod-p` do not reach the application while a Vim
editor has focus: use the palette, the space menu (`space e` explain, `space f` tables, `space b` connections) or the
chords the engine does not use (`mod-shift-e`, `mod-shift-n`, `mod-shift-t`, `mod-shift-f`, `mod-alt-c`, `mod-alt-r`). The
engine takes `ctrl-shift-<letter>` for no Vim key (Vim cannot tell `ctrl-shift-f` from `ctrl-f`; the engine used to
and swallowed *Format Document*), so those chords always reach the application. In Visual
mode a half-typed `g` command takes whatever key comes next, an application chord included, and cancels itself
(as Vim does with an unknown key); a Normal-mode operator such as `d` does not swallow chords. On macOS the
application uses `cmd`, which Vim never uses. `ctrl-enter` runs the statement (the selection in Visual modes) in
every Vim mode.

With the completion popup open, `ctrl-n` / `ctrl-p` / `up` / `down` / `tab` / `enter` / `esc` belong to the
popup in all three key maps. In Helix the application actions that lost a key remain reachable through the space
menu, `:` commands, the palette (`space ?`, `F1`, `ctrl-shift-p`) and the chords nothing else uses
(`mod-alt-r` for *Refresh Schema*, `mod-shift-t` for *Reopen Closed Tab*, `mod-shift-n`, `mod-alt-c`).

## Explorer

Context `Explorer` (the connection tree). `ctrl-n` / `ctrl-p` are literal `ctrl` on every platform.

| Keys | Action |
| --- | --- |
| `up` / `down`, `ctrl-p` / `ctrl-n` | previous / next row |
| `left` / `right` | collapse / expand |
| `enter` | activate (connect, open table, expand) |
| `/` | focus the filter box |
| `esc` | return to the editor |
| `h j k l` | collapse, next, previous, expand (Helix and Vim key maps) |
| `alt-up` / `alt-down` | move the selected connection (within its folder) or folder up / down |
| `f2` | rename the selected folder |
| `delete` | delete the selected folder (asks first; its connections move to the top level) |

In the filter box: `enter` or `down` moves into the tree, `esc` clears the filter and moves into the tree.
Double-click activates a row.

## Import, export and table dialogs

None of these has a global chord (they are palette commands, explorer context-menu entries and, in the Helix and
Vim key maps, `:import` / `:export`), so they cannot conflict with an editor key. Inside the dialogs:

| Keys | Action |
| --- | --- |
| `tab` / `shift-tab` | next / previous control |
| `enter` in the file field | read the file (Import); start the export (Export) |
| `esc` | close the dialog; while an import, validation or export runs, cancel the run (the dialog stays so the result is seen) |
| `enter` in a confirmation | confirm the primary action; destructive confirmations (Truncate, Drop, a table change that drops something) need a click, or `tab` to the button and `space` |

Palette commands: *Import Data from CSV/TSV…*, *Import CSV/TSV as New Table…*, *Export Table Data…*,
*New Table…*, *Modify Table…*, *Rename Table…*, *Duplicate Table Structure…*, *Truncate Table…*, *Drop Table…*.

## Table editor

The Modify Table / New Table tab (see [Change a table](USER_GUIDE.md#the-table-editor)). It adds **no** global
chord: Execute is the existing `mod-enter` (*Run Statement*) and `mod-w` still closes the tab. The editor
handles these in its own key handler while its grid has focus; a test (`no_global_binding_takes_a_chord_the_table_editor_uses`)
checks that none of them is also a global binding.

| Keys | Action |
| --- | --- |
| arrows, `home` / `end`, `pageup` / `pagedown` | move the cell cursor |
| `h` `j` `k` `l`, `gg`, `G` (Helix, Vim) | move the cell cursor, first row, last row |
| `tab` / `shift-tab` | next / previous cell (wraps to the next row); inside a cell editor, apply and open the next one |
| `enter`, `f2`, `i` / `a` (Helix, Vim) | edit the cell (open a list, pick a table, flip a box) |
| `space` / `shift-space` | flip a checkbox; step a choice (identity, `ON DELETE`, method) forward / back |
| `insert` / `shift-insert`, `o` / `O` (Helix, Vim) | add a row below / above |
| `delete`, `dd` (Helix, Vim) | remove the row (a column of an existing table stays struck through; again restores it) |
| `yy` (Helix, Vim) | duplicate the column |
| `alt-up` / `alt-down` | move the column (tables that do not exist yet) |
| `ctrl-z`, `u` (Helix, Vim) | undo |
| `ctrl-shift-z`, `ctrl-y`, `ctrl-r` (Helix, Vim), `U` | redo |
| `alt-left` / `alt-right`, `[` / `]` (Helix, Vim) | previous / next section |
| `alt-n` | put the cursor in the table name field (`enter` or `esc` returns to the grid) |
| `alt-a` | choose a suggested foreign key |
| `alt-p` | show / hide the DDL preview |
| `alt-c` / `alt-o` / `alt-r` | copy the script / open it in a console / revert every change |
| `mod-enter` | Execute (confirmation first) |
| `mod-w` | close the tab (asks when there are unapplied changes) |

In a cell editor: `enter` applies, `esc` cancels, `tab` / `shift-tab` apply and move on; in the type cell the
up / down arrows choose a suggestion. In the column list (`enter` on a *Columns* cell): `up` / `down` (`k` /
`j`), `space` or `x` chooses, `alt-up` / `alt-down` orders the chosen columns, `enter` or `esc` closes.

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
| `ctrl-space` (Helix: `ctrl-x` in Insert mode, `space k` in Normal mode; Vim: `ctrl-n` / `ctrl-p` in Insert mode, `space k` in Normal mode) | open completion by hand |
| `up` / `down` | move the selection |
| `pageup` / `pagedown` | move by 8 |
| `tab` | accept |
| `enter` | accept after you moved the selection, when the popup was opened by hand, or when the top suggestion is not a keyword; otherwise insert a newline |
| `esc` | close the popup (in Helix and Vim Insert mode a second `esc` leaves Insert mode) |

The popup opens by itself while typing a word or `.`, and after a space that follows `FROM`, `JOIN`, `ON`,
`INTO`, `UPDATE`, `TABLE` or a comma in a `FROM` list. (`ctrl-p` / `ctrl-n` move the selection like `up` / `down`:
owned by the popup in both key maps; see [Who wins a key](#who-wins-a-key).)

### Live templates (every key map)

| Keys | Action |
| --- | --- |
| `tab` after an abbreviation | expand the template (while typing: Helix / Vim Insert mode, Standard) |
| `tab` / `shift-tab` in a template | next / previous placeholder; past the last one the template ends |
| `esc` | leave the template (and, in Helix and Vim, Insert mode) |
| `enter` with the completion popup open | accept the suggestion (inside a template `tab` is *not* accept) |

## Results grid

Handled in `crates/theta-grid/src/input.rs` when the grid has focus. `mod` is `ctrl` or `cmd`.

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
| `F4`, `r` (Helix, Vim) | toggle the record view (the current row as a list of fields) |
| `F3`, `a` (Helix, Vim) | toggle the selection statistics popover (also: click the summary in the footer) |
| `esc` | collapse the selection; when nothing is left to collapse, return to the editor |
| `mod-1` | return to the editor |

In the record view `up` / `down` (`k` / `j`) go to the previous / next row, `pageup` / `pagedown` jump ten rows, `left` / `right` (`h` / `l`), `tab` / `shift-tab`, `home` and `end` move between the fields of the row, `mod-home` / `mod-end` (`gg` / `G`) go to the first / last row, `enter` / `F2` / `i` edit the highlighted field, `shift-y` (Helix, Vim) copies the whole row, and `esc` returns to the grid. `esc` also closes the statistics popover.

`mod-0` is *Set NULL* while the grid has focus (it is *Reset Editor Font Size* everywhere else).

Inside the cell editor: `enter` keeps the value, `tab` / `shift-tab` keep it and move to the next / previous
cell, `esc` cancels.

Helix and Vim key map extras: `h j k l` move, `gg` / `G` first / last row, `v` toggles visual (extend) mode, `x`
selects the row, `y` copies, `u` undoes, `i` edits, `%` selects all, `mod-d` / `mod-u` half page down / up.

Mouse: click, shift-click, drag; row numbers and headers select rows and columns; drag a header edge to
resize, double-click it to fit; `mod`-click a foreign-key value follows it; double-click edits; right-click
opens the context menu.

## Map

Handled in `crates/theta-map-view/src/panel.rs` when the map has focus (chords with `ctrl`, `alt` or `cmd`
are ignored).

| Keys | Action |
| --- | --- |
| arrows (`h j k l` in Helix and Vim) | pan |
| `+` / `=`, `-` | zoom in / out |
| `0` | fit to data |
| `m` | toggle the measure tool |
| `esc` | close menus, leave the measure tool, or clear the selected feature |

Mouse: drag pans, wheel zooms, double-click zooms in (closes a measurement in the measure tool), click
picks a feature, right-click opens a menu (*Copy coordinates*, *Copy as WKT point*, *Copy feature WKT*).

## Plan panel

Handled in `crates/theta-plan-view/src/panel.rs` in the tree and flame views (not in *Raw*).

| Keys | Action |
| --- | --- |
| `up` / `down` (`k` / `j` in Helix and Vim) | previous / next node |
| `pageup` / `pagedown` | move by 8 |
| `home` / `end` | first / last node |
| `left` / `right` (`h` / `l` in Helix and Vim) | collapse or go to parent / expand or go to child |
| `enter` | toggle the node details |
| `esc` | close the details |

## Editor

The tables below are the editor engine's own (`crates/theta-editor/README.md`), which the application also
shows in Settings › Keymap. Read them together with [Who wins a key](#who-wins-a-key).

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
| `gg` `ge` `G`, `<n>G` `<n>gg`, `ctrl-home` `ctrl-end` | file start / last line / goto line (the ctrl keys are `gg` / `ge`; in Insert mode they put the caret at the very start / end of the document) |
| `gt` `gc` `gb` | top / middle / bottom line of the viewport |
| `g\|` | goto column |
| `gj` `gk` | line down / up |
| `]p` `[p` | next / previous paragraph |
| `[<space>` `]<space>` | add newline above / below |
| `ctrl-d` `ctrl-u`, `ctrl-f` `ctrl-b`, `pagedown` `pageup` | half page / page |
| `zz zc zm` `zt` `zb` `zj` `zk` (and `Z`) | scroll the view (centre, top, bottom, line down / up) |
| `ctrl-o` `ctrl-i`/`tab` `ctrl-s` | jumplist back / forward / save (Normal and Select mode; in Insert mode `ctrl-o` is the application's *Open SQL File* on Linux and Windows) |
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
| `ctrl-w`, `alt-backspace` / `alt-d` | delete word back / forward |
| `ctrl-u` `ctrl-k` | delete to line start / end |
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
| `:import` `:import FILE` | open Import Data (as a new table) on the active connection, optionally with `FILE` filled in |
| `:export` `:export [SCHEMA.]TABLE` | open Export Data for the table (a picker without an argument) |
| `:open` `:e` | open `.sql` files |
| `:goto N` `:N` | go to line `N` |
| `:sort` | sort the selections' contents, or the lines of a multi-line selection |
| `:new` | new console |
| `:cancel` | cancel the running query |
| `:reopen` | reopen the last closed tab |

`tab` completes command names (repeat to cycle). An unknown command shows "Unknown command".

Prompt editing: `left right home end ctrl-a ctrl-e ctrl-b ctrl-f alt-b alt-f` movement,
`backspace del ctrl-w ctrl-u ctrl-k alt-backspace` deletion, `up down ctrl-p ctrl-n` history, `esc ctrl-c`
cancel (backspace on an empty prompt cancels too). While a prompt is open all of these belong to the prompt,
including `ctrl-e`, `ctrl-b`, `ctrl-w`, `ctrl-k`, `ctrl-p` and `ctrl-n`.

#### Global (every mode of both keymaps)

The engine itself handles `ctrl-enter` / `cmd-enter` (run statement; run selection when there is a
selection), `ctrl-shift-enter` (run all), `ctrl-space` (completion) and `F5` (run all). Inside the editor these
are the engine's, in both key maps. See [Who wins a key](#who-wins-a-key).

### Vim key map

A Vim keymap on the shared engine (rope, transactions, undo, registers, prompts,
`Effect`s), implemented in `src/vim/`. Where Vim and Neovim differ, Neovim wins; every
deviation is listed under "Deviations from Vim" below. `Editor::mode_label()` gives
`NORMAL`, `INSERT`, `VISUAL`, `V-LINE`, `V-BLOCK` or `REPLACE`, and
`Editor::cursor_style()` is `Block` in Normal/Visual, `Bar` in Insert and `Underline`
in Replace. `space` is the leader in Normal and Visual mode.

Grammar: `["x][count]{operator}[count]{motion | text object}`; the counts multiply.
`.` repeats the last change (a new count replaces the old one), `q{a-z}` records a
macro, `@{a-z}` / `@@` / `@:` replay one.

#### Motions (Normal / Visual / after an operator)

| keys | action |
| --- | --- |
| `h` `j` `k` `l`, arrows, `backspace`, `ctrl-h` `ctrl-j` `ctrl-n` `ctrl-p` | character / line motions (`j` `k` keep the wanted display column) |
| `0` `^` `$` `g_` `g0` `g^` `g$` `home` `end` | line start / first non-blank / end / last non-blank |
| `\|` | go to display column N |
| `+` `-` `_` `enter` `ctrl-m` | first non-blank of the next / previous / count-1th line |
| `gj` `gk` | as `j` `k` (the view owns soft wrapping) |
| `w` `W` `b` `B` `e` `E` `ge` `gE` | word motions (`cw` behaves as `ce`) |
| `f` `F` `t` `T` `;` `,` | find a character on the line; repeat / reverse |
| `(` `)` `{` `}` | sentences, paragraphs |
| `%` `[(` `[{` `])` `]}` | matching bracket, unmatched bracket |
| `gg` `G` `{count}G` `H` `M` `L` | document / window motions |
| `/` `?` `n` `N` `*` `#` `g*` `g#` | search |
| `'x` `` `x `` `''` ``` `` ``` `'<` `'>` `'[` `']` | marks (`a`-`z`, `A`-`Z`, and the automatic `'` `<` `>` `[` `]` `.` `^` `"`) |
| `ctrl-o` `ctrl-i` `tab` | jumplist |
| `ctrl-d` `ctrl-u` `ctrl-f` `ctrl-b` `ctrl-e` `ctrl-y` `pageup` `pagedown` `zt` `zz` `zb` `z.` `z-` | scrolling: the cursor moves like Vim's and `Effect::Scroll` asks the host to scroll |

#### Operators

| keys | action |
| --- | --- |
| `d` `c` `y` | delete / change / yank |
| `<lt>` `>` | shift left / right (`shiftwidth` 4) |
| `=` | format the lines (`Effect::FormatSelection`; the host formats the selected lines) |
| `g~` `gu` `gU` `g?` | toggle case / lower / upper / rot13 |
| `gc` | toggle line comments (also `space c`) |
| `dd` `cc` `yy` `<lt><lt>` `>>` `==` `g~~` `guu` `gUU` `g??` `gcc` | doubled operator acts on lines (`g~g~`, `gugu`, ... too) |
| `D` `C` `Y` `S` `s` `x` `X` `delete` | `d$` `c$` `y$` `cc` `cl` `dl` `dh` `dl` (`Y` is `y$`, as in Neovim) |
| `v` `V` `ctrl-v` after an operator | force the motion characterwise / linewise / blockwise |
| `U` | Normal mode: no-op (line undo is not implemented); in Visual mode it upper-cases |

#### Text objects (after an operator or in Visual mode)

| keys | action |
| --- | --- |
| `iw` `aw` `iW` `aW` | word, WORD |
| `is` `as` `ip` `ap` | sentence, paragraph |
| `i(` `a(` `i)` `a)` `ib` `ab` | parentheses |
| `i{` `a{` `i}` `a}` `iB` `aB` | braces |
| `i[` `a[` `i]` `a]` `i<lt>` `a<lt>` `i>` `a>` | brackets, angle brackets |
| `i"` `a"` `i'` `a'` `` i` `` `` a` `` | quotes (on the line; the next pair is used when the cursor is before one) |
| counts, and repeating `iw` `ap` `i(` ... in Visual mode | grow the selection |

#### Changes

| keys | action |
| --- | --- |
| `x` `X` `~` `r{c}` `R` | delete / toggle case / replace char (`r<ret>` splits the line) / Replace mode |
| `J` `gJ` | join lines (with / without space handling) |
| `p` `P` `gp` `gP` | put (characterwise, linewise and block registers) |
| `u` `ctrl-r` | undo / redo (one step per command; an insert session is one step) |
| `ctrl-a` `ctrl-x` | increment / decrement the number at or after the cursor (decimal, `0x`, `0b`) |
| `.` | repeat the last change |
| `ZZ` `ZQ` | `Save` + `CloseTab` / `CloseTab` |
| `ctrl-g` `ctrl-l` | no-ops |

#### Insert and Replace mode

| keys | action |
| --- | --- |
| `i` `a` `I` `A` `o` `O` `gi` `gI` | enter Insert (counts repeat the insertion) |
| `esc` `ctrl-c` `ctrl-[` | back to Normal, cursor one left |
| `ctrl-w` `ctrl-u` `ctrl-h` `backspace` `del` | delete word / to the start of the insert / char |
| `ctrl-t` `ctrl-d` | indent / outdent the line |
| `ctrl-r {reg}` | insert a register |
| `ctrl-o` | run one Normal-mode command |
| `ctrl-j` `ctrl-m` `enter` | new line (with auto-indent) |
| `ctrl-n` `ctrl-p` `ctrl-space` | `Effect::RequestCompletion` |
| `tab` | spaces up to the next multiple of four |
| arrows, `home`, `end`, `pageup`, `pagedown` | move (Vim starts a new undo step; here they do not) |

Auto-pairs and auto-indent are the shared insert behaviour of the other keymaps. In
Replace mode `backspace` restores the original text. `ctrl-k` (digraphs), `ctrl-v`
(literal) and `ctrl-a` / `ctrl-@` (insert last text) are not implemented.

#### Visual mode (`v`, `V`, `ctrl-v`)

| keys | action |
| --- | --- |
| `v` `V` `ctrl-v` | switch to / leave the kind; `gv` reselects the last Visual area |
| `o` `O` | swap the cursor with the other end (`O`: the other corner in Visual-block) |
| `d` `x` `c` `s` `y` `r{c}` `~` `u` `U` `J` `gJ` `>` `<lt>` `=` `p` `P` | operate on the selection |
| `gu` `gU` `g~` `g?` `gc` | case / comment operators on the selection |
| `D` `X` `Y` `C` `S` `R` | operate on whole lines (in Visual-block `D` and `C` go to the end of the lines) |
| `I` `A` | Visual-block: insert / append on every line; characterwise: insert at the start / append at the end |
| `$` in Visual-block | the block follows each line end |
| `:` | command line with `'<,'>` |
| `aw` `ip` `i(` ... | text objects extend the selection |
| `ctrl-a` `ctrl-x` | increment / decrement numbers on the selected lines |
| `esc` `ctrl-c` | back to Normal |

#### Registers

`"{name}` before a command. `a`-`z` set, `A`-`Z` append, `0` last yank, `1`-`9`
numbered deletes (shifted like Vim: multi-line deletes and small deletes with `%` `(`
`)` `` ` `` `/` `?` `n` `N` `{` `}`), `-` small deletes, `_` black hole, `"` unnamed, `+`
and `*` the system clipboard (`Effect::SetClipboard` to write; `p` reads the
clipboard the host last reported), `/` last search, `.` last inserted text, `:` last
command line. `:registers` / `:display` list them in the status line.

#### Search

`/` and `?` open the prompt (`enter` accepts, `esc` cancels, a live preview moves the
view). Patterns are Vim regex (`\v` `\m` `\M` `\V`, `\<` `\>`, `\{n,m}`, `\(` `\)`,
`\|`, `\+` `\?` `\=`, classes `\s \d \w \a \l \u \x`, `[]`, `\c` `\C`) translated to the
`regex` crate; unsupported constructs are reported as errors, never silently
reinterpreted. `ignorecase` and `smartcase` are on, `wrapscan` is on, `hlsearch` is on
(`:noh` hides the highlights until the next search). Offsets (`/pat/e`, `/pat/+1`)
work. `n` / `N` keep the direction; `*` `#` search the word under the cursor.

#### Space menu (`key_hints()` shows it)

| keys | action |
| --- | --- |
| `space f` | `OpenTablePicker` ("go to table") |
| `space b` | `OpenConnectionPicker` |
| `space ?` `space space` | `OpenCommandPalette` |
| `space r` | `RunStatementAtCursor`, or `RunSelection` in Visual mode |
| `space R` | `RunAll` |
| `space e` `space E` | `ExplainStatement { analyze: false / true }` |
| `space k` | `RequestCompletion` |
| `space t` | `OpenThemePicker` |
| `space c` | toggle line comments (current line, or the Visual lines) |
| `space y` `space Y` | yank to the system clipboard (operator / whole line) |
| `space p` `space P` | paste the system clipboard after / before |
| `space x` | `CancelQuery` |
| `space /` | `OpenTablePicker` |
| `space g` `space d` | `FocusResults` / `FocusExplorer` |
| `space T` `space j` | `ToggleExplorer` / `ToggleBottomPanel` |

#### Command line (`:`)

Ranges: `N`, `.`, `$`, `%`, `'x`, `/pat/`, `?pat?`, `+N` `-N`, `,` pairs, `'<,'>`
(inserted by `:` in Visual mode); `|` separates commands. A bare `:N` goes to line N.

| keys | action |
| --- | --- |
| `:s/pat/rep/[flags]` `:&` `:&&` | substitute (`g` `i` `I` `n` `e` flags; `&` `\0`-`\9` `\u` `\U` `\l` `\L` `\e` `\E` `\r` `\n` in the replacement; any delimiter) |
| `:g/pat/cmd` `:g!/pat/cmd` `:v/pat/cmd` | run an Ex command on the matching / other lines |
| `:d [x]` `:y [x]` `:pu [x]` `:pu! [x]` | delete / yank / put lines (with a register) |
| `:m {addr}` `:t {addr}` `:co {addr}` | move / copy lines |
| `:>` `:<lt>` | shift lines (repeat the character to shift more) |
| `:j` `:j!` | join lines |
| `:norm[al][!] keys` | run plain keys on every line of the range |
| `:sort[!] [n] [u] [i] [/pat/]` | sort the lines |
| `:k{x}` `:mark {x}` `:marks` | set a mark / list marks |
| `:reg[isters]` `:di[splay]` | list registers (status line) |
| `:noh[lsearch]` | hide search highlights |
| `:se[t] [no]ic` `scs` `hls` | the supported options (`[opt]?` queries, `[opt]!` / `inv[opt]` toggle); anything else is `HostCommand("set ...")` |
| `:u[ndo]` `:red[o]` | undo / redo |
| `:h[elp]` | `HostCommand("help ...")` (the host opens the keymap reference) |
| `:wa` `:wqa` `:xa` `:qa` `:up` `:tabnew` `:tabn` `:tabp` `:tabc` | Vim's spellings of the host commands |
| `:w` `:wq` `:q` `:run` `:run-all` `:explain` `:analyze` `:format` `:theme` `:connect` `:goto N` `:new` `:cancel` | the shared host command table (see "Command line" of the Helix keymap) |

Anything else becomes `Effect::HostCommand(text)`. `tab` completes command names;
`up` `down` `ctrl-p` `ctrl-n` browse the history; the usual prompt editing keys apply.

#### Global (every mode)

`ctrl-enter` / `cmd-enter` RunStatementAtCursor (RunSelection with a selection) ·
`ctrl-shift-enter` RunAll · `ctrl-space` RequestCompletion · `F5` RunAll.

The deviations from real Vim (what `space` means, fixed options, no `:map` or Vimscript, no windows) are listed in
[the engine README](../crates/theta-editor/README.md#deviations-from-vim) and summarised in the
[user guide](USER_GUIDE.md#vim-keymap).

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
| `ctrl-shift-t` | reopen the last closed tab (application binding) |
| `ctrl-tab` `ctrl-shift-tab` | next / previous tab |
| `ctrl-shift-p` `F1` | command palette |
| `ctrl-p` | go to table |
| `ctrl-shift-i` `ctrl-alt-l` | format document (`mod-shift-f` also works) |
| `ctrl-b` `ctrl-j` `ctrl-shift-e` | toggle explorer / toggle bottom panel / focus explorer |
| `ctrl-k ctrl-t` | theme picker (the status bar shows `ctrl-k` while it waits for the second chord) |
