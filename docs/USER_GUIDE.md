# Theta user guide

Task-oriented notes for using Theta 0.1.0-alpha.4. Key bindings are summarised where they matter and
listed in full in [KEYBINDINGS.md](KEYBINDINGS.md). `mod` means `cmd` on macOS and `ctrl` elsewhere.
Names of menu items, buttons and dialog fields are written the way the application shows them. The
examples use the demo database (`theta --demo`).

Contents: [The window](#the-window) · [Connect](#connect-to-a-database) · [SSH tunnel](#connect-through-an-ssh-tunnel) ·
[Explorer](#the-explorer) · [Write and run SQL](#write-and-run-sql) · [Transactions](#transactions) ·
[Editor keys](#editor-keys) · [Vim keymap](#vim-keymap) · [Results grid](#the-results-grid) · [Edit data](#edit-data-safely) ·
[Explain](#explain-and-query-plans) · [Map](#the-map) · [Messages and history](#messages-and-history) ·
[Themes](#themes) · [Settings](#settings) · [Command palette](#the-command-palette) ·
[Troubleshooting](#troubleshooting)

## The window

```text
┌ title bar:  ☰ menu · connection switcher                        TXN open [Commit] [Roll Back] ▷ Run  ■  ⚙ ┐
├ Explorer ─┬ console tabs  [console 1] [console 2] [+]                                                       │
│ (left)    │ editor                                                                                          │
│           ├ Results | Messages | Plan | Map | History   (bottom panel)                                      │
│           │ grid, plan or map                                                                               │
└ status bar:  [explorer] [bottom panel]          Running 1.5 s · 200 rows · 0.2 ms · Ln 1, Col 1 · NOR · ● Demo (SQLite) · SQLite 3.54.1 ┘
```

- **Title bar.** The menu button (☰) has *New Console*, *New Connection…*, *Command Palette…*, *Go to
  Table…* and *Quit*. The connection switcher shows the active connection with a status dot (green:
  connected, amber: connecting, red: failed) and opens the connection picker. **Run** runs the statement
  under the cursor; the square button cancels the running query. **TXN open**, *Commit* and *Roll Back*
  appear only while a transaction is open. The gear opens a menu with *Settings…*, *Select Theme…*,
  *Select Keymap…*, *Import Zed Theme…*, the font-size items and *Open Settings File*.
  On macOS the same actions are also in the menu bar.
- **Explorer** (left): `mod-b` shows or hides it. Drag its right edge to resize.
- **Console tabs**: one SQL editor per tab. A dot on a tab marks text that changed since the console was
  opened or saved to a file (the text is kept for the next start either way).
- **Bottom panel**: `mod-j` shows or hides it. Drag its top edge to resize. Tabs: *Results*, *Messages*,
  *Plan*, *Map* (with a count of features when a result has geometry) and *History*.
- **Status bar**: the two buttons toggle the explorer and the bottom panel. On the right: the running time
  of the current query, the outcome of the last one ("200 rows · 0.2 ms" or the error text), the cursor
  position (`Ln`, `Col`), the editor mode as a coloured chip (`NOR`, `INS`, `SEL` in the Helix key map; `NORMAL`, `INSERT`, `VISUAL`, `V-LINE`, `V-BLOCK`, `REPLACE` in the Vim key map; none in Standard), the keys typed so far toward a multi-key command, `recording @a` while a Vim macro is being recorded, the number of
  selections or selected characters, and the active connection with the server version.

Panel sizes and visibility, and the selected bottom tab, are remembered in `workspace.json`.

When there is no connection the console area shows a welcome screen with **Add Connection** and **Try the
Demo Database** (the same as `theta --demo`), three buttons to choose the key map (*Helix*, *Vim*, *Standard*; the first thing a new user has to decide, and the choice can be changed any time) and a short list of key bindings for the selected one.

## Connect to a database

Try it without a server first: `theta --demo` creates `demo.sqlite` in the configuration directory, saves
it as *Demo (SQLite)* and connects to it.

For your own database choose *New Connection…* (`mod-shift-n`, the command palette, or the **+** in the
explorer header). Choose the **Database type** from the dropdown (*PostgreSQL*, *MySQL / MariaDB*, *SQLite*; arrow keys and
`enter`, or type to filter), fill in the fields that type needs, then
press *Test Connection*, *Save* or *Save & Connect*. A successful test reports the server version and the
round-trip time ("Connection successful · PostgreSQL 16.15 · 53 ms").

| Field | Notes |
| --- | --- |
| Connection URL | paste `postgres://user:pass@host:5432/db?sslmode=require`, `mysql://…`, `mariadb://…` or `sqlite:///path/to.db` to fill the other fields; the same parser serves `--connect` |
| Name | shown in the explorer, the title bar and the pickers |
| Host, Port | the port defaults to 5432 for PostgreSQL and 3306 for MySQL/MariaDB |
| User, Password | the eye button shows the password while you type |
| Database | database name; empty means the server default |
| SSL mode | *Disable*; *Prefer* (try TLS, fall back to plain); *Require* (TLS, certificate not verified); *Verify full* (TLS, certificate chain and host name verified) |
| Database file (SQLite) | *Browse…*, or turn on *Create if missing* |
| SSH tunnel | see the next section |
| Read-only | Theta refuses statements that change data or schema on this connection (see [Read-only connections](#read-only-connections)) |
| Colour | none, red, orange, green, blue or purple; a coloured bar marks the connection in the title bar and explorer, handy for production |

Passwords go to the operating-system keychain, never to `connections.json`. If there is no keychain (see
[Troubleshooting](#troubleshooting)) the dialog says "Password will not be saved (no system keychain)" and
the password lives in memory until you quit.

Manage saved connections from the explorer: right-click a connection for *Connect* or *Disconnect*, *New
Console*, *Refresh*, *Edit…*, *Duplicate* and *Delete…* (the stored password is deleted too; the database
is not touched). Editing a connected connection closes its session. Switch the active connection with
`mod-alt-c` (*Connect…* in the palette) or the title-bar switcher; this also rebinds the current console.

Saved connections are not opened when Theta starts. Double-click the connection in the explorer, pick it
in the switcher, or press run in a console bound to it: Theta shows "Connecting…", waits for the
connection and then runs the statement (a connection error appears in *Messages*; *Explain* behaves the
same). Start-up flags open connections from the command line (`theta --help` lists all of them):

```sh
theta --connect postgres://postgres:postgres@127.0.0.1:5432/quarry_demo     # temporary, not saved
theta --demo --run "select count(*) from orders"                            # run SQL after connecting
theta --connect mysql://quarry:quarry@127.0.0.1:3306/quarry_demo --script report.sql
```

`--run SQL` and `--script FILE` open a console on the first start-up connection and run every statement in it.

### Read-only connections

Before running a statement Theta classifies it. On a read-only connection, anything classified as a data
change (`INSERT`, `UPDATE`, `DELETE`, `MERGE`, `COPY`, `TRUNCATE` …) or a schema change (`CREATE`, `ALTER`,
`DROP` …) is refused with "This connection is read-only: statements that modify data or schema are not
run." Edits made in the grid are refused too. This is a safety net in the application, not a permission in
the database: `SELECT` of a function with side effects, `CALL` and similar statements run. Use a read-only
database account when that matters.

## Connect through an SSH tunnel

Turn on **SSH tunnel** in the connection dialog and fill in *SSH host*, *Port* (22), *User* and
*Authentication*:

- **Key file**: choose the private key under *Private key* (*Browse…*); *Passphrase* is optional and is
  kept in the keychain.
- **Password**: *SSH password*, kept in the keychain.
- **SSH agent**: uses the agent named by `SSH_AUTH_SOCK` (on Windows, the OpenSSH agent).

The main *Host* and *Port* are then read **from the SSH server's point of view**: for a database on the SSH
machine itself use `localhost`. Theta opens one SSH session and one local listener for the connection and
closes them when the connection closes. Host keys are checked against `~/.ssh/known_hosts`: an unknown host
is accepted and recorded (Theta tells you), a *changed* key is refused. SQLite connections have no tunnel.

## The explorer

The tree shows each connection, its schemas, and under each schema the groups *Tables*, *Views*, *Routines*
and *Sequences*. A table expands to its columns (with the data type, a key icon for primary keys and a
dot for NOT NULL columns), *Keys*, *Foreign Keys* and *Indexes*. The number at the right of a table is the
row estimate the server reports. Under *Foreign Keys*, *Suggested* lists keys that the database does not
declare but that column names imply (`orders.customer_id` → `customers.id`), each with a confidence.

- Click the filter box (or press `/` in the tree) and type: the tree narrows to fuzzy matches. `enter` or
  `down` moves into the tree; `esc` clears the filter.
- `mod-shift-e` focuses the tree. `up`/`down` (or `ctrl-p`/`ctrl-n`), `left`/`right` to collapse and expand,
  `enter` to activate, `esc` to return to the editor. With the Helix key map `j k h l` work too.
- **Open a table**: double-click it, or select it and press `enter`. Theta opens a console titled with the
  table name, fills it with a `SELECT` of every column limited to 100 rows, and runs it. `mod-p` (*Go to
  Table…*) does the same from anywhere by fuzzy name.
- **Refresh** the catalog after a schema change with `F5` (*Refresh Schema*) or the refresh button. The
  catalog also feeds completion and diagnostics.
- **Right-click a table**:
  - *Open Table*, *Copy Name*, *Copy Qualified Name*;
  - *Generate* › `SELECT`, `INSERT`, `UPDATE`, `DELETE`, which open a console with the statement (the
    `UPDATE` and `DELETE` templates use `WHERE key = NULL`, so an unedited template changes nothing);
  - *Show DDL*;
  - *Related Tables…*: a picker of the tables reachable through foreign keys, in two groups: *Foreign keys*
    (declared) and *Suggested* (inferred, with a confidence percentage). Choosing one opens a console with a
    `JOIN` query between the two tables.
- **Right-click a suggested key**: *Copy ALTER TABLE … ADD FOREIGN KEY* or *Generate JOIN Query*.
- **Folders.** Group connections with *New Folder* (the folder button in the explorer header, or *New
  Connection Folder…* in the palette). A folder holds connections one level deep. Right-click a connection and
  choose *Move to Folder…* (or run *Move Connection to Folder…* from the palette) to file it; choose *No folder* to
  take it out again. Right-click a folder for *Rename…* (`f2`), *Move Up / Down* and *Delete Folder…* (`delete`;
  its connections go back to the top level, nothing is deleted). *Move Up* and *Move Down* on a connection
  (`alt-up` / `alt-down`) reorder it within its folder. Which folders are collapsed, their order and the order of
  the connections are saved in `connections.json`; an older file simply has no folders. The connection picker
  shows `Folder / Name` and matches either part. There is no drag and drop: GPUI's list does not give the tree a
  drop target, so the menus and keys above do the job.
- A connection that failed to connect, or whose schema could not be read, shows the error under it.
  *Retry*, *Edit Connection…* and *Copy Error* are in its context menu.

## Write and run SQL

Open a console with `mod-n` (*New Console*). A console is bound to the connection that was active when you
opened it; the title-bar switcher (or `:connect NAME` in Helix) rebinds it. Consoles are tabs:

- Drag a tab to reorder; drop it on the empty strip to move it to the end. Middle-click or the **x** closes
  it (`mod-w`); `mod-pagedown` / `mod-pageup` switch tabs (the Standard key map also has `ctrl-tab`). When
  there are more tabs than fit, a drop-down at the right lists them all.
- Text, cursor, title, connection and tab order are saved as you type and restored at the next start.
  Results are not.
- **Open SQL File…** (`mod-o`) opens one or more `.sql` files as consoles. **Save Console As…** (`:w` in
  Helix, `ctrl-s` in Standard) asks for a file name the first time and writes straight to that file
  afterwards.

### Running

- **Run Statement**: `mod-enter`, the **Run** button, or Helix `space r`. It runs the statement under the
  cursor. With a selection it runs the statements inside the selection. A statement is found by a lexer
  that understands comments, quotes and dollar-quoted bodies, so a script can hold many statements.
- **Run All**: `mod-shift-enter` (Helix `space R`). Statements run in order and the run stops at the first
  error.
- **Cancel Query**: `mod-.`, the stop button, or Helix `space x`. The server is asked to abort the statement.
- A statement that returns rows gets a sub-tab in *Results* ("Result 1 · stores", "Result 2"). A statement
  that does not (an `UPDATE`) reports "UPDATE — 1 row affected · 0.1 ms" in the footer and in *Messages*.
- The gutter marks the last run of each statement: a spinner while it runs, a green dot when it worked, a
  red dot when it failed. A failed statement is underlined at the position the server reports, and the
  *Messages* tab comes to the front with the error, its code, detail and hint.
- One query runs at a time per connection. A second run is refused with "A query is already running on …
  Cancel it first". All consoles bound to the same connection share one server session.
- **Row limit**: results stop at 1,000 rows by default (Settings › Data › *Row limit*). The server stops
  producing rows at the limit, and *Messages* says "stopped at the row limit".
- If the connection was lost (the server terminated the session, the network dropped, "server has gone
  away"), the first failing statement already marks the session lost and reconnects. A plain query
  outside a transaction is then re-run once automatically and *Messages* says "Connection lost —
  reconnected and re-ran the statement". Anything else (a write, a DDL statement, a statement inside a
  transaction) is not re-run: *Messages* says so, and open transactions are gone with the session.

### Query parameters

A statement that contains placeholders asks for their values before it runs: `?` (MySQL, SQLite), `$1`
(Postgres, and SQLite `?1`), and the named forms `:name` and `@name`. **Run**, **Run Selection**, **Run All** (one
prompt for every placeholder of the script) and **Explain** all show the **Parameters** dialog: one row per
distinct placeholder with a type (*Auto*, *Text*, *Number*, *Boolean*, *NULL*, *Raw SQL*) and a value. *Auto*
keeps numbers and `true`/`false` as such and quotes everything else. *Raw SQL* inserts the text verbatim, which is how
you parameterise an identifier. Enter runs, Escape cancels, Tab moves value → type → next value. The values you used
are remembered per console and pre-filled next time.

Substitution is **client-side**: Theta writes the value into the statement as an escaped literal and sends ordinary
SQL, so the history and the Messages tab (which lists the values used) show exactly what ran. Placeholders inside
strings, comments and dollar-quoted bodies, Postgres casts (`x::int`) and the jsonb operators `?`, `?|`, `?&`, and
MySQL user variables (`@x := 1`, `SET @x = 1`, `@@version`) are not parameters. Turn the prompt off with
`"editor": {"parameters": false}` in settings, Settings, or the palette command **Toggle Query Parameters**.

### Asking before something risky happens

- `UPDATE` or `DELETE` without `WHERE` opens **Run this statement?** with the statement and the buttons
  *Cancel* and *Run Anyway*. Nothing is wrapped in a transaction unless you opened one.
- *Explain Analyze* of a statement that writes opens **Explain Analyze runs the statement**.
- Submitting grid edits opens **Apply changes?** (see [Edit data safely](#edit-data-safely)).
- Closing a tab, or quitting, while a transaction is open offers *Roll Back* or *Commit* (see below).

### Completion, formatting and diagnostics

- **Completion** opens while you type a word or a `.`, and after a space that follows `FROM`, `JOIN`,
  `ON`, `INTO`, `UPDATE`, `TABLE` or a comma in a `FROM` list. `ctrl-space` opens it by hand (Helix: `ctrl-x` in
  Insert mode, `space k` in Normal mode). It knows the tables and columns of the connection, what is in
  scope (aliases, CTEs), dialect functions and snippets. After `JOIN`, `JOIN other ON …` clauses built from
  foreign keys come first; declared keys rank above keys Theta infers from column names, and the footer of
  the popup shows the whole `ON` clause.
  - `up`/`down` move, `tab` accepts, `esc` closes the popup (in Helix a second `esc` leaves Insert mode).
  - `enter` accepts only after you moved the selection, or when the popup was opened by hand, or when the
    top suggestion is not a keyword. Otherwise `enter` inserts a newline, so typing an alias and pressing
    `enter` does not turn the alias into `CROSS JOIN`.
- **Format Document**: `mod-shift-f` (Helix `:format`). With a selection, Helix `=` formats the selection.
  Indent width (default 4) and keyword upper-casing (default on) are in Settings › Editor.
- **Diagnostics** underline syntax problems, `UPDATE`/`DELETE` without `WHERE`, suspicious predicates and,
  when the catalog is loaded, unknown tables and columns. The linter is heuristic and deliberately quiet:
  an underline is worth a look, but the absence of one proves nothing. The database has the last word.

### Files, history and the palette

- *History*: every executed statement is recorded in `history.jsonl` and listed in the History tab (see
  [Messages and history](#messages-and-history)).
- `mod-shift-p` or `F1` opens the [command palette](#the-command-palette).

## Transactions

Each connection is one server session, so `BEGIN`, `COMMIT` and `ROLLBACK` behave as in `psql` or `mysql`.
The palette has *Begin Transaction*, *Commit Transaction* and *Roll Back Transaction*, and you can type the
statements yourself. While a transaction is open (Theta asks the server after every run) the title bar
shows **TXN open** with **Commit** and **Roll Back** buttons. Grid edits submitted inside an open transaction
run in it and are not committed ("Applied 2 changes (in your open transaction, not yet committed)"). Closing
the last console of a connection or quitting with an open transaction opens a dialog with *Roll Back and
Close* / *Commit and Close* (or *Roll Back and Quit* / *Commit and Quit*).

## Editor keys

The editor has three key maps: [Helix](https://helix-editor.com/)'s (the default: *select first, then act*), a
[Vim](#vim-keymap) map (*verb, then motion*) and a Standard one. `mod-enter` runs from any mode. This section
describes Helix; Vim has [its own section](#vim-keymap).

| Do this | Keys |
| --- | --- |
| Start typing | `i` (before), `a` (after), `o` / `O` (new line below / above); `esc` returns to Normal mode |
| Move | `h j k l`; `w` / `b` / `e` by word; `gg` first line, `G` last, `5G` line 5; `gh` / `gl` line start / end |
| Select | motions select (`w` selects the next word); `x` the current line (again to extend); `%` everything; `v` toggles Select mode so motions extend |
| Act on the selection | `d` delete, `c` change, `y` yank, `p` paste after, `u` undo, `U` redo, `.` repeat |
| Multiple cursors | `C` copies the selection to the next line; `s` selects all matches of a regex *inside* the selection; `,` keeps only the primary |
| Surround and objects | `mi(` inside parentheses, `ma"` around a string, `ms)` surround, `md(` delete the surround |
| Search | `/` and `?`, `n` / `N`, `*` searches for the word under the cursor |
| Comment | `ctrl-c` (or `space c`) toggles `--` comments |
| Command line | `:w` save, `:q` close tab, `:format`, `:run`, `:explain`, `:analyze`, `:goto 120`, `:theme tokyo`, `:connect NAME`, `:open` |
| Space menu | `space` shows what follows (a which-key box): `r` run, `e` explain, `f` tables, `b` connections, `k` completion, `x` cancel, `t` themes, `?` command palette |

Three recipes: delete a line with `x` `d`; change a word with `w` `c`, type the replacement, `esc`; rename
every occurrence of `name` in the statement by selecting the statement (`%` for everything), `s`, typing
`name`, `enter` (one cursor per match), then `c`, the new name, `esc`.

Prefer the **Standard** key map if modal editing is unfamiliar: *Select Keymap…* (or *Use Standard Keymap*) in the
command palette, Settings › Editor › *Keymap*, the welcome view, or `"keymap": "standard"` in `settings.json`. It follows
Zed's defaults: `ctrl-z`, `ctrl-d` (next occurrence), `ctrl-/` (comment), `alt-up/down` (move line),
`ctrl-f` find, `ctrl-g` go to line, `ctrl-s` save.

While the editor has focus its own keys always win: in Helix Insert mode `ctrl-w` deletes a word (it does
not close the tab), `ctrl-k` kills to the end of the line, `ctrl-b` pages up in Normal mode, `F5` runs all
statements, and with the completion popup open `ctrl-n` / `ctrl-p` move the selection. The application's
shortcuts for the same keystrokes work outside the editor, and every application action is still reachable
through the space menu, `:` commands and the command palette (`F1`, `space ?`, `ctrl-shift-p`). The rule
and the exact table are in [KEYBINDINGS.md](KEYBINDINGS.md#who-wins-a-key).

Closing a tab never loses the console: *Reopen Closed Tab* (`mod-shift-t`, the palette, or `:reopen` in
Helix and Vim) brings back the text, connection and cursor of the last 20 closed consoles, also after a restart.

## Vim keymap

A Vim keymap that follows Neovim where Vim and Neovim differ, tested against real Vim (about 3,900 tests, most of
them generated from it). Switch to it with *Select Keymap…* (or *Use Vim Keymap*) in the command palette,
Settings › Editor › *Keymap*, the **Vim** button on the welcome view, or `"keymap": "vim"` in `settings.json`. It
applies at once to every open console; text and undo history stay. Helix remains the default.

**Modes.** The status bar shows the mode as a chip: `NORMAL` (blue), `INSERT` (green), `VISUAL` (yellow),
`V-LINE` (magenta), `V-BLOCK` (cyan) and `REPLACE` (red). The cursor is a block in Normal and Visual modes, a
bar in Insert mode and an underline in Replace mode. Half-typed commands (`"a2d`, `g`, `z`) show next to the
chip, and a macro being recorded shows as `recording @a`. After a quarter of a second a half-typed command pops up
the keys that can follow, like the Helix space menu.

**What is there.** The grammar `["x][count]{operator}[count]{motion | text object}`; the motions, operators
(`d c y < > = g~ gu gU g? gc`), text objects (`iw aw i( a" ip` and the rest of Vim's set), registers (`"a`-`"z`, numbered
and small-delete registers, `"+` and `"*`, `".` `":` `"/`), marks, the jumplist, `.` repeat, macros (`q{a-z}`,
`@a`, `@@`), `v` `V` `ctrl-v` with block insert and append (`I`, `A`), `R` Replace mode, `/` and `?` search with
Vim regex and highlights (`:noh`), and ex commands (`:s`, `:g`, `:normal`, `:sort`, ranges, `:m` `:t` `:d` `:y`
...). The whole table is in [KEYBINDINGS.md](KEYBINDINGS.md#vim-key-map) and in Settings › Keymap while Vim is the
active key map.

**Space is the leader.** In Normal and Visual mode `space` opens the same menu as in Helix, which is how the
application's actions are reached without leaving the keyboard:

| Keys | Action |
| --- | --- |
| `space r` / `space R` | run the statement (the selection in Visual mode) / run all |
| `space e` / `space E` | explain / explain analyze |
| `space f` / `space b` | go to table / switch connection |
| `space ?` or `space space` | command palette |
| `space k` | completion |
| `space t` | theme picker |
| `space c` | toggle `--` comments on the line or the Visual lines |
| `space y` / `space Y` / `space p` / `space P` | yank / paste through the system clipboard explicitly |
| `space x` | cancel the running query |
| `space g` / `space d` / `space T` / `space j` | focus results / focus explorer / toggle explorer / toggle bottom panel |

**Ex commands that act on the application.** Besides Vim's own (`:s`, `:g`, `:d`, ...): `:w` save the console to
a file, `:q` close the tab, `:wq`, `:qa` quit, `:run` run the statement, `:run-all`, `:explain`, `:analyze`,
`:format` / `:fmt` format the document, `:theme [name]`, `:connect [name]`, `:goto N`, `:new` new console, `:cancel`,
`:reopen`, `:open`, and `:help`, which opens Settings › Keymap. `=` formats the lines of its motion (`==`, `=ip`,
`V=`) or the whole document when the motion covers all of it. An option the engine does not know
(`:set number`) answers "Unknown option" in a toast; only `ignorecase`, `smartcase` and `hlsearch` exist.

**Completion works as in Helix Insert mode.** The popup opens while you type a word or after a dot, `tab` accepts,
`enter` accepts only a useful suggestion (otherwise it is a newline), `ctrl-n` / `ctrl-p` open it by hand and then
move, and the first `esc` closes the popup while the second leaves Insert mode. In Normal mode nothing opens it.

**Clipboard.** Real Vim keeps yanks to itself unless told otherwise. Theta's setting `vim.clipboard_unnamed`
(Settings › Editor › *Vim: share the clipboard*, default **on**, like `set clipboard=unnamedplus`) makes yanks and
deletes land on the system clipboard, and `p` paste what you copied in another application (a multi-line copy from
another console pastes line by line). Turn it off and only `"+y` and `"+p` (and `space y` / `space p`) use the
clipboard. Whatever the setting, `"_` is the black hole.

**The mouse** works as in a GUI Vim: a click moves the cursor, a drag or double-click starts Visual mode, a
triple-click selects the line (V-LINE).

**Deviations worth knowing.** The full list is in [the engine README](../crates/theta-editor/README.md#deviations-from-vim).

- `space` is the leader, so it no longer moves right (`d<space>` is `dl`).
- The options are fixed: `shiftwidth=4`, `expandtab`, `autoindent`, `ignorecase smartcase wrapscan hlsearch`,
  numbers without octal. There is no `:set` for anything else, and there is no `:map`, no `.vimrc` and no
  Vimscript.
- Typing `(` inserts `()`: auto-pairs and auto-indent are the editor's, the same in all three key maps.
- Not implemented: windows and tabs (`ctrl-w`, `:split`), folds, digraphs and `ctrl-v` literal insert,
  `gq` / `gw`, `g;` (change list), `gn`, the `it` / `at` tag objects, `:!` shell commands, `:s` back-references.
- `ctrl-w`, `ctrl-b`, `ctrl-f`, `ctrl-d`, `ctrl-u`, `ctrl-e`, `ctrl-y`, `ctrl-r`, `ctrl-o`, `ctrl-i`, `ctrl-a`, `ctrl-x`,
  `ctrl-v`, `ctrl-n` and `ctrl-p` belong to the editor while it has focus, so on Linux and Windows the application
  chords on the same keys (`mod-w`, `mod-b`, `mod-e`, `mod-n`, `mod-o`, `mod-p`) do not reach it there; the space
  menu and the palette have every action. See [KEYBINDINGS.md](KEYBINDINGS.md#who-wins-a-key).


## The results grid

Results appear in the bottom panel (`mod-j` toggles it, `mod-2` focuses the grid, `mod-1` returns to the
editor). Above the grid, the title shows the table the rows came from (a pencil when editable, a lock when
not) and the buttons **Add row** (+), **Delete selected rows** (−), **Filter rows**, **Export results** and
**Cell inspector**. The footer shows the row count and time, the statement (hover the info icon) and a
summary of the selected cells.

- **Select**: click a cell, shift-click or drag for a rectangle, click row numbers or column headers to
  select whole rows and columns, `mod-a` for all. Arrow keys move; `shift` extends; `tab` / `shift-tab` go
  to the next / previous cell; `home` / `end` go to the first / last column and `mod-home` / `mod-end` to the
  first / last row.
- **Sort**: the arrow at the right of a header cycles ascending, descending, off (shift-click adds a
  secondary key). NULLs sort last in both directions; numbers, text and dates sort by type, with natural
  ordering for strings with numbers in them. Right-click a header for *Sort Ascending*, *Sort Descending*,
  *Clear Sort*, *Auto-fit Width* and *Copy Column Name*.
- **Filter**: `mod-f` or `/` opens a filter box that matches any cell; the footer shows "2 of 8 rows";
  `esc` closes it and restores the rows.
- **Columns**: drag a header edge to resize; double-click the edge to fit.
- **Inspector**: `space` (or the **Cell inspector** button) opens a pane with the full value of the current
  cell: long text, pretty-printed JSON, and geometry as EWKT.
- **Copy**: `mod-c` copies the selection as tab-separated text, `mod-shift-c` adds the column headers. The
  context menu also has *Copy as JSON* and *Copy as INSERT*.
- **Export**: **Export results** › CSV, TSV, JSON, Markdown or SQL INSERTs, then *To file…* or *To
  clipboard*; with a selection also *Selection to file…* and *Selection to clipboard*. Exports contain the
  rows that were loaded (see *Row limit*).
- **Foreign keys**: a column that references another table shows a link icon in its header. `mod`-click
  (ctrl-click, cmd-click on macOS) a value, or choose *Follow Reference* in the context menu, to open the
  referenced row in a new console (`SELECT * FROM customers WHERE id = 47`, run at once). `enter` follows
  the reference too, but only where the cell is not editable: on an editable result `enter` starts editing.
- **Geometry**: geometry cells are shown as text with a pin icon. *Show on Map* and *Show Column on Map*
  in the context menu switch to the [map](#the-map).
- Hand the keyboard back to the editor with `esc` (the first `esc` narrows the selection, the next one
  returns focus) or `mod-1`.

The grid draws only the visible window, and sorting or filtering more than 20,000 rows happens in the
background.

With the Helix key map the grid also takes `h j k l`, `gg` / `G`, `v` (extend), `x` (select row), `y` (copy),
`u` (undo an edit), `i` (edit) and `%` (select all).

### Record view

The record view shows one row at a time as a list: the column name and type on the left, the complete value on the
right. Long text is wrapped, JSON is pretty-printed, NULL is dimmed and a geometry is summarised. Toggle it with
the button in the grid toolbar, `F4`, or `r` in Helix and Vim. Move between rows with `up` / `down` (`k` / `j`; the
counter reads `12 / 600`), between fields with `tab`, `left` / `right` or the mouse, and edit the highlighted field
with `enter`, `F2` or a double-click: edits go through the same staging, *Submit* and confirmation as in the grid, so
a record view of a read-only result is read-only. *Value* and *Row* in the strip copy the highlighted value or the
whole row (name and value per line). The list is virtualised, so a row of 500 columns is as fast as one of five.

### Selection statistics

The footer summarises a selection (`3 rows × 2 cols · sum 1,234.5 · avg 411.5`). Click it, or press `F3` (`a` in
Helix and Vim), for the details: cells, count, distinct values, NULLs, min, max, sum and average for numbers, and the
shortest and longest text. It follows the selection, is computed on a background thread when the selection is large,
and *Copy* puts the figures on the clipboard.

## Edit data safely

A result is editable only when every column comes from one table of the loaded schema, that table is a
real table (not a view), no column is computed, and the primary key columns are in the result. Otherwise
the grid shows a lock and says why when you hover it ("The result joins several tables", "Only tables can
be edited, not views", …). Connections marked *Read-only* never write.

1. Double-click a cell, or press `enter` / `F2`. In the Standard key map you can also start typing; in
   Helix press `i`. Type the new value; `enter` keeps it, `esc` cancels, `tab` / `shift-tab` keep it and
   move to the next / previous cell. Edits are **staged**, not sent: a changed cell gets a tint and a corner
   marker, **Add row** (+) adds a row at the bottom, and `delete` (or **Delete selected rows**) strikes
   rows through. The bar above the grid shows "2 pending changes", **Revert** and **Submit**.
2. *Set NULL* in the context menu (cell or menu, not `mod-0`; see below) sets the cell to NULL. `mod-z`
   undoes the last staged edit.
3. Press **Submit** (or `mod-enter` while the grid has focus). Theta shows **Apply changes?** with the
   generated statements:

   ```sql
   UPDATE main.customers SET last_name = 'Zed' WHERE id = 1;
   DELETE FROM main.customers WHERE id = 3;
   ```

   *Apply 2 changes* runs them in one transaction (inside yours, if one is open). If anything fails, nothing
   is applied and the edits stay staged so you can fix them. *Cancel* leaves them staged.

The `WHERE` clauses use the primary key values the rows had **before** your edit. Values you type are
parsed by the column type (integers, decimals, booleans, dates, JSON …); text that does not parse is
rejected at the cell instead of reaching the database. Use a hand-written statement for anything the grid
cannot express.

> `mod-0` is bound globally to *Reset Editor Font Size*, which wins over the grid's own `mod-0` (set cell to
> NULL). Use the context menu's *Set NULL*.

## Explain and query plans

Press `mod-e` (Helix `space e`) to explain the statement under the cursor and `mod-shift-alt-e`
(`space E`) to explain **analyze**, which really executes it. The writes of an analysed statement are rolled
back, but triggers, sequences and other side effects can still happen, which is why Theta asks first.
The Plan tab also has **Explain** and **Explain Analyze** buttons that rerun the same statement.

The plan panel always shows the **Insights** list next to one of three views of the plan:

- **Tree**: an indented table of operations with exclusive time, rows (estimated, or actual with "est"
  next to it when analysed), cost, loops and buffers (columns drop out when the panel is narrow). `enter`
  opens a node's full property list.
- **Flame**: a flame graph sized by time (or cost when the plan was not analysed).
- **Raw**: the server's own output (PostgreSQL JSON, MariaDB/MySQL JSON, SQLite `EXPLAIN QUERY PLAN`).

A summary bar shows execution and planning time, total cost and the node count. **Insights** are specific
statements about *this* plan, ranked by severity: a sequential scan that reads many rows to return few; row
estimates that are off by orders of magnitude; a nested loop rescanning a table; sorts and hashes that
spill to disk; cartesian products and joins without a condition; the node that takes most of the runtime;
for MySQL/MariaDB `filesort`, temporary tables and full scans; for SQLite full-table scans and temporary
B-trees. Where an index is the likely cure the insight carries a `CREATE INDEX` statement with **Open in
Console** and **Copy SQL** buttons. Plain *Explain* shows estimates only; *Explain Analyze* adds real
timings and row counts. (SQLite reports a wall-clock time and no per-node figures.)

Try it on the demo database: `select c.first_name, count(*) from customers c join orders o on
o.customer_id = c.id group by c.id`, then `mod-e`.

## The map

Result columns that hold geometry draw on the map: PostGIS and MySQL geometry, WKB/EWKB bytes, and
WKT/EWKT or GeoJSON text (the demo database stores WKT in `location_wkt`, `area_wkt` and `path_wkt`). The
*Map* tab shows a count badge when the last result has geometry. Open it, or use **Show on Map** in the
grid.

- Pan by dragging or with the arrow keys (`h j k l` in the Helix and Vim key maps), zoom with the wheel or `+` / `-`, `0` fits
  everything, double-click zooms in.
- **Zooming out and *Fit*** show the whole data extent, also world-wide data in a short, wide panel: you can
  zoom out until the world is 80% as tall as the map (or, in a tall narrow panel, as wide as it). When the world
  is then narrower than the panel the basemap repeats sideways, the repeats are dimmed, and every feature is
  drawn exactly once, on the copy of the world centred on the map. Hover, click, selection and the measure
  tool all use that copy.
- Coordinates in WGS 84, Web-Mercator and UTM SRIDs are placed on a basemap (OpenStreetMap by default).
  Data with an unknown coordinate system is drawn on a plain planar grid, without tiles.
- Click a feature to see the other columns of its row; this also selects the row in the grid. In the grid,
  *Show on Map* selects the feature and zooms to it. Each geometry column is a layer (the **Layers**
  button).
- The toolbar has *Fit to data*, *Zoom in*, *Zoom out*, the **Basemap** menu, *Dim tiles in dark themes*
  (not offered for dark basemaps), *Measure distance and area*, *Layers* and *Export GeoJSON*.
- `m` starts the **measure tool**: click points for a distance, double-click to close a polygon and get the
  area. `esc` leaves it.
- Right-click for *Copy coordinates*, *Copy as WKT point* and *Copy feature WKT*.
- Tiles are cached on disk, so panning back is instant and areas seen before still draw offline.
  "Basemap unavailable" with a **Retry** button appears when nothing can be fetched.
- **Basemaps** (toolbar menu or Settings › Map; setting `map_basemap`): *OpenStreetMap Standard* (default),
  *OpenTopoMap* and *None (graticule)*. Each shows its own attribution while it is on the map and has its own
  deepest zoom (OpenTopoMap 17): beyond it the last level's tiles are scaled up. Tiles are cached per
  basemap. Dark themes dim light basemaps; a basemap marked *dark* is not dimmed.
- **Custom basemaps** (Settings › Map › *Custom basemaps*, or `map_custom_basemaps` in `settings.json`): a
  name, an XYZ URL with `{z}` `{x}` `{y}` and optionally `{s}` (subdomain `a`, `b` or `c`, rotated per
  tile) and `{r}` (retina suffix; always empty, standard tiles), an attribution, a max zoom and a *dark*
  flag. An old `map_tile_url` that was changed is migrated into a custom entry named *Custom tiles*.
- **Why only OpenStreetMap and OpenTopoMap are built in** (checked 2026-11-11). The OpenStreetMap tile
  policy allows interactive use by a client that identifies itself (Theta sends `Theta/<version>` as the
  User-Agent), shows attribution and does not prefetch or download in bulk; availability is best-effort.
  OpenTopoMap is CC-BY-SA with attribution. CARTO's Positron, Dark Matter and Voyager basemaps are reachable,
  but CARTO's current terms (version 2026-09-29) require a free API key on every request (`?key=`), so they
  are not built in and *Auto* (dark/light by theme) is not offered. To use them add a custom basemap, for
  example `https://{s}.basemaps.cartocdn.com/dark_all/{z}/{x}/{y}{r}.png?key=YOUR_KEY` with attribution
  `© OpenStreetMap contributors © CARTO`, max zoom 20 and *dark* on. Providers that need a key or forbid
  this use (Google, Mapbox, Esri imagery) are not offered.
- A small **scale bar** (a 1px rule with end ticks) sits above the attribution in the bottom-right corner; it
  is hidden on planar data.

Try it: `select name, city, location_wkt from stores`, then open the Map tab.

## Messages and history

**Messages** lists, newest last, every statement you ran with the time, the connection, the first line of
the statement and the outcome ("SELECT — 92 rows · 2.1 ms", "[3/5] UPDATE — 1 row affected"), server
notices, and errors in red with their code, detail and hint. Its toolbar has *Copy All* and *Clear*.

**History** keeps up to 5,000 statements in `history.jsonl` with time, connection, duration, row count and
success. Type in *Search history* to filter. Double-click an entry to insert it into the current console;
right-click for *Insert into Console*, *Open in New Console* and *Copy*. Applied grid changes are recorded
too.

## Themes

Open the theme picker with `mod-k mod-t` (Helix: `space t`, `:theme`). The picker groups themes as
*Theta*, *Light* and *Dark* and previews each as you move. `:theme tokyo` in Helix picks the only theme
whose name contains that text. *Theta Dark* is the default; its text area (editor, results grid, plan) is the darkest surface and the
chrome around it (explorer, tabs, toolbars) one step lighter, so the code and data stand out. 38 themes are bundled (the two Theta themes
and 36 from 21 gpui-kit theme files: Catppuccin, Gruvbox, Solarized, Tokyo Night, Ayu, Everforest and
others), plus gpui-kit's own *Default Light* and *Default Dark*.

To follow the operating system's light/dark setting turn on *Follow system appearance* in Settings ›
Appearance (`follow_system_appearance` in the file); `light_theme` and `dark_theme` decide which two
themes alternate.

**Zed themes**: choose *Import Zed Theme…* in the command palette and select a Zed theme JSON file (the
format of `~/.config/zed/themes/*.json`). Theta converts it, saves it to `<config>/themes/` and opens the
picker. Colours Zed does not define fall back to the Theta theme of the same appearance. You can also drop
a gpui-kit theme file into that folder yourself.

## Settings

*Settings…* (`mod-,`, the gear menu, or the command palette) opens a dialog with six sections. Changes apply
immediately and are written to `settings.json`.

| Section | What it has |
| --- | --- |
| Appearance | *Theme* (opens the picker), *Follow system appearance*, *Interface font*, *Editor font*, *Editor font size*, *Editor line height* |
| Editor | *Keymap* (*Helix*, *Vim* or *Standard*), *Vim: share the clipboard* (shown while Vim is selected), *Format: indent width*, *Format: uppercase keywords* |
| Data | *Row limit*: 100, 500, 1,000, 5,000, 10,000, 50,000, 100,000 or No limit |
| Map | *Basemap* and the list of *Custom basemaps* |
| Keymap | a searchable list of every action with its key, plus the editor keys for the current key map |
| About | version, repository link and the licences of the bundled fonts, themes and map data |

The file is also hand-editable (*Open Settings File* in the palette). Unknown keys are ignored and a key
with a wrong type is ignored with a warning, so one typo never resets the rest.

| Key | Default | Meaning |
| --- | --- | --- |
| `theme` | `"Theta Dark"` | current theme |
| `follow_system_appearance` | `false` | switch between `light_theme` and `dark_theme` with the OS |
| `light_theme`, `dark_theme` | `"Theta Light"`, `"Theta Dark"` | used when following the system |
| `keymap` | `"helix"` | `"helix"`, `"vim"` or `"standard"` |
| `vim.clipboard_unnamed` | `true` | Vim key map: yanks and deletes also go to the system clipboard and `p` pastes from it |
| `ui_font_family`, `ui_font_size` | `"IBM Plex Sans"`, `14` | interface font (size 8 to 40; file only) |
| `buffer_font_family`, `buffer_font_size` | `"Lilex"`, `14` | editor and grid font (size 8 to 40) |
| `buffer_line_height` | `1.5` | multiplier of the buffer font size (1 to 3) |
| `row_limit` | `1000` | maximum rows fetched per query (the dialog also offers No limit, stored as `0`) |
| `map_basemap` | `"osm"` | `"osm"`, `"opentopomap"`, `"none"` or the `id` of a custom basemap |
| `map_custom_basemaps` | `[]` | list of `{ "id", "name", "url", "attribution", "max_zoom", "dark" }` |
| `map_tile_url` | `https://tile.openstreetmap.org/{z}/{x}/{y}.png` | legacy; a changed value is migrated into a custom basemap |
| `format.indent` | `4` | formatter indent width |
| `format.uppercase_keywords` | `true` | formatter keyword case |

**Zoom** (`mod-=`, `mod--`, `mod-0`; *Increase / Decrease / Reset Editor Font Size*) changes the editor and
results font size only (8 to 32). The rest of the interface keeps its size; change *Interface font* in the
dialog or `ui_font_size` in the file for that.

## The command palette

`mod-shift-p` or `F1` (Helix and Vim `space ?`) opens it; type to filter, `enter` runs the command, `esc` closes.
The palette lists, with the key each has:

| Group | Commands |
| --- | --- |
| Console | *New Console* `mod-n`; *Open SQL File…* `mod-o`; *Save Console As…* |
| Connection | *New Connection…* `mod-shift-n`; *Connect…*; *Disconnect*; *Refresh Schema* `F5`; *Go to Table…* `mod-p` |
| Query | *Run Statement* `mod-enter`; *Run All* `mod-shift-enter`; *Cancel Query* `mod-.`; *Explain* `mod-e`; *Explain Analyze* `mod-shift-alt-e`; *Format Document* `mod-shift-f`; *Begin Transaction*; *Commit Transaction*; *Roll Back Transaction* |
| View | *Toggle Explorer* `mod-b`; *Toggle Bottom Panel* `mod-j`; *Focus Explorer* `mod-shift-e`; *Focus Editor* `mod-1`; *Focus Results* `mod-2`; *Close Tab* `mod-w`; *Reopen Closed Tab* `mod-shift-t` |
| Appearance | *Select Theme…* `mod-k mod-t`; *Import Zed Theme…*; *Select Keymap…* (a picker); *Use Helix Keymap*; *Use Vim Keymap*; *Use Standard Keymap*; *Increase Editor Font Size* `mod-=`; *Decrease Editor Font Size* `mod--`; *Reset Editor Font Size* `mod-0` |
| App | *Settings…* `mod-,`; *Open Settings File*; *Quit* `mod-q` |

*Connect…* opens the connection picker (the same as `mod-alt-c`). *Disconnect* closes the active
connection's session.

## Troubleshooting

**The window does not open on Linux, or says no Vulkan device was found.** Theta draws through Vulkan.
Install a driver: `mesa-vulkan-drivers` (Debian/Ubuntu), `vulkan-radeon` / `vulkan-intel` (Arch, Fedora's
`mesa-vulkan-drivers`), or your GPU vendor's driver. Check with `vulkaninfo --summary`. In a virtual
machine or container with no GPU, Mesa's software renderer (`lavapipe`, part of `mesa-vulkan-drivers`) works,
slowly. The AppImage does not carry these libraries; it uses the host's.

**Passwords are not remembered on Linux.** Theta stores secrets through the freedesktop Secret Service (GNOME
Keyring, KDE Wallet, KeePassXC) over D-Bus. In a minimal window manager session, over plain SSH, or in a
container there is none, so Theta keeps secrets in memory for that run only, and the connection dialog
tells you ("Password will not be saved"). Start a keyring daemon (`gnome-keyring-daemon --start
--components=secrets`) and unlock it, or type the password again at each start.

**macOS says the app "is damaged" or "cannot be opened because the developer cannot be verified".** Releases
are ad-hoc signed but not notarised. Right-click `Theta.app`, choose *Open* and confirm once; or, after
copying the app to Applications, run `xattr -dr com.apple.quarantine /Applications/Theta.app`. Building
from source avoids the quarantine flag altogether.

**Windows SmartScreen says "Windows protected your PC".** The executable is not code-signed. Choose *More
info*, then *Run anyway*. Verify the download against `SHA256SUMS` if you want to be careful.

**The map is grey or says "Basemap unavailable".** The tile server could not be reached (offline, proxy,
firewall) or rejected the request. Theta trusts the operating system's certificate store, so corporate
proxies with their own CA normally work. Click *Retry*, or choose another basemap or add a custom one for a server you can reach.
Geometries are still drawn without a basemap.

**A connection to PostgreSQL or MySQL fails with a TLS error.** Try SSL mode *Prefer* or *Disable* for servers
without TLS, *Require* for self-signed certificates, and *Verify full* only when the server certificate
chains to a root your system trusts and matches the host name.

**A query seems stuck.** Press `mod-.` to cancel. Theta asks the server to abort the statement (a cancel
request for PostgreSQL, `KILL QUERY` for MySQL/MariaDB, `sqlite3_interrupt` for SQLite) and the console stays
usable.

**A key does not do what the editor tables say.** Check which view has focus: the editor, the results grid,
the explorer and pickers each own some keys, and the application's chords apply where they do not. See
[KEYBINDINGS.md](KEYBINDINGS.md#who-wins-a-key).

**Settings or connections seem lost.** A corrupt `connections.json` is renamed to `connections.json.broken` and
a fresh file started. A settings file that is not valid JSON falls back to the defaults with a warning.
Keep a backup of the configuration directory if you need to restore either.
