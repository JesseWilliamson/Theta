# theta-editor

The text-editing engine behind Theta's SQL consoles: a rope buffer, multiple
selections, undo history, a SQL highlighter and three complete keymaps, **Helix**
(modal, selection-first), **Vim** (modal, verb-object, checked against real Vim) and
**Standard** (non-modal, Zed's default bindings).

The crate is pure Rust: no GPUI, no I/O, no async. A view feeds it `KeyInput`s and
mouse positions, paints `text()` + `selections()` + `highlights()`, and reacts to
the `Effect`s returned from `handle_key` (run a statement, open a picker, set the
clipboard, scroll ...).

```text
 key / click / paste                                         Effects
 ───────────────────▶ Editor ──────────────────────────────▶ (run, save, picker,
                       │  handle_key → keymap → command        clipboard, scroll,
                       ▼                                       status ...)
        Transaction ──▶ Rope + Selections + History + Syntax cache
```

## Architecture

| module | role |
| --- | --- |
| `text.rs` | `ropey` helpers: grapheme navigation (`unicode-segmentation`, CRLF never split), character classes, line geometry. Everything is **char indexes**. |
| `selection.rs` | `Selection { anchor, head }` and `Selections` (sorted, non-overlapping, with a primary that follows its selection through normalisation). |
| `transaction.rs` | `Transaction`: sorted, non-overlapping `Change { from, to, text }` list. Applies to the rope, returns its inverse, maps positions (`Assoc::Before/After`). Every edit in the crate goes through it. |
| `history.rs` | Undo/redo stacks of `Step`s. A step is a *group* of transactions plus the selections to restore on either side. |
| `editor.rs` | `Editor` state and the public API. `apply_with` is the single choke point for text changes: it maps selections, records undo information, notifies the syntax cache and bumps `revision`. |
| `keys.rs` | `KeyInput`, `KeyInput::parse` and canonicalisation (the case of a character is carried by the char; the `shift` flag is dropped for `Key::Char`). |
| `helix.rs` | Key trie (`Sub`/`Node`, built once into a `OnceLock`), the Normal/Select state machine (counts, `"` registers, await-a-char commands) and all Helix commands. |
| `edit.rs` | Insert-style editing shared by Helix insert mode and the Standard keymap: typing, auto-pairs, auto-indent, backspace/delete variants, word/line kills. |
| `standard.rs` | The Standard keymap (Zed-like). |
| `vim/` | The Vim keymap, on the same engine: `mod.rs` (state, command grammar, dot repeat, macros, mouse, undo cursor), `geom.rs` (Vim's line and display-column model), `motion.rs` / `object.rs` (ports of Vim's word, sentence, paragraph and text-object code), `nav.rs` (motion evaluation, marks, jumplist, search), `ops.rs` (registers, operators, put, join, numbers), `visual.rs`, `insert.rs`, `ex.rs` (command line), `vregex.rs` (Vim regex to the `regex` crate), `hints.rs`, `keyenc.rs` (macro key encoding). |
| `prompt.rs` | The bottom prompt (`:` commands, `/` `?` search, `s` `S` `K` regex prompts, Standard `ctrl-g`), live previews, history, tab completion, and the regex search machinery (smart-case, windowed scans). |
| `motion.rs`, `objects.rs` | Pure motions, text objects and surround finders. No editor state. |
| `syntax.rs` | Hand-written SQL lexer with per-line carry-over state (see below). |
| `tests/` | The behaviour tests (see "Tests"). |

### Selections

The model is Helix's. A selection is `anchor..head`, half-open, and a forward
selection's *cursor* is the last grapheme inside it (a 1-wide block). A bare cursor
on character `i` is therefore `Selection { anchor: i, head: i + 1 }`; only at the
very end of the text (or in an empty buffer) can a selection be empty. The editor
enforces this on every change of selections (`set_sels`): in range, grapheme
aligned and, for Helix, at least one grapheme wide.

`Selection::range()` is always the half-open highlight range. `Selection::cursor()`
is the cheap char-based Helix rule (`head - 1` for a forward selection); hosts that
want the exact cursor for the active keymap (grapheme-exact for Helix, `head` for
the bar cursor of Standard) call `Editor::cursor_of(&sel)`.

The Standard keymap uses the same type with bar cursors: `head` is the cursor and
selections are the usual text-editor kind.

### Transactions and undo

Commands build a `Transaction` and call `apply_with(tx, Some(selections))`, or
`apply(tx)` to map the existing selections through it. Mapping follows Helix: for a
forward selection the anchor sticks after an insertion at its position and the head
before it; the start of a replacement stays put so same-size replacements (`r`, `~`,
indent) keep the selection on the new text.

Undo history is a list of groups:

* **Helix**: every Normal-mode command is one step; an insert session (`i` ... `Esc`,
  including the `c` that started it) is one step.
* **Standard**: a run of typing is one step; the run breaks on any non-typing key,
  cursor movement, click, or a whitespace -> non-whitespace transition.
* Programmatic edits (`replace_range`, `set_text`) are one step each.

Undo restores the selections from before the group, redo the ones after it. The
history is linear (redo is discarded by new edits), not Helix's undo tree.

### Key handling

`handle_key` canonicalises the key, runs the prompt if one is open, then the global
bindings (`ctrl-enter`, `ctrl-shift-enter`, `ctrl-space`, `F5`), then the keymap.

Inside Theta the application binds chords of its own (`ctrl-b`, `ctrl-w`, `F5` ...) and GPUI matches bound
chords before the editor view sees a key, so those keys never reach `handle_key`; `F5` there is *Refresh
Schema*, not run all. `docs/KEYBINDINGS.md` lists the conflicts.

The two key tables below (`## Helix keymap`, `## Standard keymap`) are embedded in the application and parsed when
the settings dialog opens (`crates/theta/src/exec/keyref.rs`) to fill Settings › Keymap, so keep their `###` headings and two-column
`| keys | action |` rows.

Helix Normal/Select keys walk a trie (`helix.rs::build_trie`); leaves are `Cmd`
values with a description, which doubles as the data for `key_hints()`. Counts
(`1`-`9` then `0`-`9`, capped at 10 000), `"x` register prefixes and
await-a-character commands (`f t F T r mi ma ms md mr`) are small states in front
of the trie. Select mode shares the trie; every motion simply passes
`extend = true`. Insert mode is a direct `match` (`edit.rs`).

`.` repeat records the *keys* of the last text-changing command (including the
insert-mode keys typed after `i`/`c`/`o` ...) and feeds them through the normal key
path again. Macros (`Q`/`q`) are recorded and replayed the same way.

### Registers and the clipboard

Registers are `char -> Vec<String>` (one value per selection). `_` is the black
hole; `+`/`*` talk to the system clipboard through `Effect::SetClipboard` (write)
and `Editor::set_clipboard` (read); `/` reads the last search pattern. In the Helix keymap
the default register `"` is mirrored to the system clipboard on yank (see deviations). The
Vim keymap does so only after `Editor::set_vim_clipboard_unnamed(true)` (see "Deviations from
Vim"). `Editor::recording_register()` says which Vim macro register is being recorded.

### Syntax highlighting

`syntax.rs` is a hand-written line lexer instead of tree-sitter: half-typed SQL must
colour sensibly while you type, and a lexer never fails to produce tokens. It knows
keywords, types (also after `::`), functions (identifier followed by `(`),
`NULL/TRUE/FALSE`, single/double/backtick quoting with doubled-quote escapes,
`E'..'` strings, `$$`/`$tag$` dollar quoting, numbers (decimal, exponent, hex),
`--` and nested `/* */` comments, operators, punctuation and parameters (`$1`, `:name`,
`?`, `@var`). The only state carried between lines is "inside a block comment of depth
n", "inside an open quote" or "inside a dollar quote". Line start states are cached;
an edit truncates the cache at the edited line, so repainting after a keystroke
re-lexes only the lines between the edit and the lines being painted. Per-line spans
are cached per revision.

### Performance

No command calls `text()`. Scans are iterator based (`chars_fwd/chars_back`) and
bracket/quote searches give up after 100 000 chars. Regex search works on
line-aligned windows that grow geometrically, so a hit near the cursor does not copy
the buffer. `tests/perf.rs` asserts < 5 ms per key on a 20 000-line document.

## Tests

`cargo test -p theta-editor` (about 3 900 tests: 330+ hand-written for Helix and
Standard, about 3 500 generated Vim cases and 80 hand-written Vim API tests). The
harness (`tests/harness.rs`) mirrors Helix's own test style:

```rust
hx("#[h|]#ello world", "w", "#[hello |]#world");   // Helix keymap
st("a#[|]#b\nc", "<C-a>", "#[ab\nc|]#");            // Standard keymap
```

`#[`...`|]#` is the primary selection (the `|` marks the head, so `#[|ab]#` is
backward), `#(`...`|)#` secondary ones, `#[|]#` an empty selection. Keys are plain
characters plus `<ret> <esc> <tab> <C-d> <A-;> <C-S-z> <cmd-s> <F3> <lt> <space>`.
`tests/fuzz.rs` throws thousands of random key sequences (plus clicks, pastes,
programmatic edits and viewport changes) at both keymaps with a fixed seed and checks
that nothing panics, selections stay valid and undoing everything restores the
original text. `THETA_FUZZ_SEED` and `THETA_FUZZ_ITERS` run a longer campaign;
`THETA_FUZZ_TRACE=<iteration>` prints every step of one iteration.

The Vim tests use their own notation (`tests/vim_dsl.rs`):

```rust
vim("fo[o] bar", "dw", "fo[b]ar");              // [x] is the block cursor on x
vim("[]", "ihi<esc>", "h[i]");                  // [] alone: empty line / end of text
vis("fo[o] bar", "vl", "fo«o[ ]»bar");           // Visual mode: «...» is the selection
```

`‹x›` replaces `[x]` as the cursor marker when the text itself contains brackets, and
`‸` marks the bar cursor in Insert and Replace mode. Generated suites
(`tests/vim_*_cases.rs`, by area: motion, operator, object, register, visual, dot,
macro and mark, search, ex, insert, edge) take their expected values from real Vim.
`tests/vim_oracle/` holds the harness that talks to a real `vim` (not run in CI; see
its README) and `golden.tsv`, 8 287 recorded cases that `cargo test` replays.

## Helix keymap

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

## Vim keymap

A Vim keymap on the shared engine (rope, transactions, undo, registers, prompts,
`Effect`s), implemented in `src/vim/`. Where Vim and Neovim differ, Neovim wins; every
deviation is listed under "Deviations from Vim" below. `Editor::mode_label()` gives
`NORMAL`, `INSERT`, `VISUAL`, `V-LINE`, `V-BLOCK` or `REPLACE`, and
`Editor::cursor_style()` is `Block` in Normal/Visual, `Bar` in Insert and `Underline`
in Replace. `space` is the leader in Normal and Visual mode.

Grammar: `["x][count]{operator}[count]{motion | text object}`; the counts multiply.
`.` repeats the last change (a new count replaces the old one), `q{a-z}` records a
macro, `@{a-z}` / `@@` / `@:` replay one.

### Motions (Normal / Visual / after an operator)

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

### Operators

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

### Text objects (after an operator or in Visual mode)

| keys | action |
| --- | --- |
| `iw` `aw` `iW` `aW` | word, WORD |
| `is` `as` `ip` `ap` | sentence, paragraph |
| `i(` `a(` `i)` `a)` `ib` `ab` | parentheses |
| `i{` `a{` `i}` `a}` `iB` `aB` | braces |
| `i[` `a[` `i]` `a]` `i<lt>` `a<lt>` `i>` `a>` | brackets, angle brackets |
| `i"` `a"` `i'` `a'` `` i` `` `` a` `` | quotes (on the line; the next pair is used when the cursor is before one) |
| counts, and repeating `iw` `ap` `i(` ... in Visual mode | grow the selection |

### Changes

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

### Insert and Replace mode

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

### Visual mode (`v`, `V`, `ctrl-v`)

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

### Registers

`"{name}` before a command. `a`-`z` set, `A`-`Z` append, `0` last yank, `1`-`9`
numbered deletes (shifted like Vim: multi-line deletes and small deletes with `%` `(`
`)` `` ` `` `/` `?` `n` `N` `{` `}`), `-` small deletes, `_` black hole, `"` unnamed, `+`
and `*` the system clipboard (`Effect::SetClipboard` to write; `p` reads the
clipboard the host last reported), `/` last search, `.` last inserted text, `:` last
command line. `:registers` / `:display` list them in the status line.

### Search

`/` and `?` open the prompt (`enter` accepts, `esc` cancels, a live preview moves the
view). Patterns are Vim regex (`\v` `\m` `\M` `\V`, `\<` `\>`, `\{n,m}`, `\(` `\)`,
`\|`, `\+` `\?` `\=`, classes `\s \d \w \a \l \u \x`, `[]`, `\c` `\C`) translated to the
`regex` crate; unsupported constructs are reported as errors, never silently
reinterpreted. `ignorecase` and `smartcase` are on, `wrapscan` is on, `hlsearch` is on
(`:noh` hides the highlights until the next search). Offsets (`/pat/e`, `/pat/+1`)
work. `n` / `N` keep the direction; `*` `#` search the word under the cursor.

### Space menu (`key_hints()` shows it)

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

### Command line (`:`)

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

### Global (every mode)

`ctrl-enter` / `cmd-enter` RunStatementAtCursor (RunSelection with a selection) ·
`ctrl-shift-enter` RunAll · `ctrl-space` RequestCompletion · `F5` RunAll.

### Deviations from Vim

Deliberate:

* **`space` is the leader** in Normal and Visual mode, so `space` no longer moves right;
  `d<space>` is `dl`.
* **`Y` is `y$`** (Neovim). Options are fixed to `nojoinspaces`, `nrformats=bin,hex`
  (no octal; `-` is always a sign), `shiftwidth=4`, `expandtab`, `softtabstop=4`, tab
  width 4, `autoindent`, `backspace=indent,eol,start`, `ignorecase smartcase wrapscan
  hlsearch`.
* **Auto-pairs and auto-indent** are the shared ones: typing `(` inserts `()`. Enter
  and auto-indent normalise indentation to spaces.
* By default the unnamed register is **not** mirrored to the clipboard (as in plain Vim): `"+`
  and `"*` are the clipboard, and so is `space y`; `Effect::SetClipboard` is emitted for them.
  `Editor::set_vim_clipboard_unnamed(true)` is `set clipboard=unnamedplus`: yanks and deletes
  into the unnamed register also emit `SetClipboard`, and `p` pastes text another application put
  on the clipboard (while the clipboard is still what the editor last wrote, the register keeps
  its line / block kind). Theta turns it on through the `vim.clipboard_unnamed` setting.
* Arrow keys in Insert mode do not start a new undo step.
* Counts are capped at 10 000, and replay (`.`, macros, `:normal`) at 200 000 keys.
* Paragraph (`{` `}` `ip` `ap`) scans give up after 2 000 lines and sentence (`(` `)`)
  scans after 1 500 characters without a boundary, so that every key stays fast on a
  20 000-line document without blank lines; the motion then fails.
* No-op operators (nothing changed) create no undo entry.
* `*` / `#` word boundaries use Unicode word characters instead of `iskeyword`.
* `:normal` takes plain characters (no `<esc>`-style notation). `:s///c` is accepted
  and ignored (no confirmation prompt).

Model limits (the oracle cases that differ are listed in
`tests/vim_oracle/deviations.tsv`, 137 of 8 287):

* A buffer whose last line is empty **and** has no final line break cannot be
  represented, and an emptied buffer loses its final newline.
* Visual-block does not cut tabs or wide characters: a block edge inside one takes the
  whole character.
* `gv` after edits can differ from Vim in the cursor column.

Not implemented (out of scope): Vimscript, `:map` and friends, folds, windows and
tabs (`ctrl-w`, `:split`), digraphs and `ctrl-v` literal insert, spell checking, tags,
`gq` / `gw` formatting, `virtualedit`, the `it` / `at` tag objects, the change list
(`g;`), `gn`, `]p`, `[[` `]]` section motions, `&` / `g&`, `U`, `ga`, `g ctrl-a`,
`:s` with `\=`, `~`, `\zs` and back-references in patterns (`\1`, `\@=`), the
command-line window, `:!` shell commands and `K`.

## Standard keymap

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

## Deviations

From the public API brief:

* **`Selection::cursor()`** cannot know the keymap or grapheme boundaries (the struct
  is just `anchor`/`head`). It applies the Helix rule on chars. `Editor::cursor_of(&Selection)`
  is the exact, keymap-aware version and is what the view should paint.
* The editor adds `Editor::cursor_of`; the Vim keymap adds `Keymap::Vim`, the `Mode`
  variants `Visual`, `VisualLine`, `VisualBlock` and `Replace`, `Editor::mode_label()`
  and `Editor::cursor_style() -> CursorStyle { Block, Bar, Underline }` (valid for every
  keymap). Code that matches `Mode` exhaustively (status bars) needs the new variants.

From Helix:

* **Default yank mirrors to the system clipboard.** `y` (and `"+y`, `space y`) emit
  `Effect::SetClipboard`. `d`/`c` write the default register without touching the
  clipboard. When `p` reads the default register and the clipboard the host reports
  differs from what we last mirrored, another application changed it and the
  clipboard text is pasted instead.
* **`space p` / `space P` paste the clipboard** (as in Helix); the command palette is
  `space ?` / `space space`. `space R` is RunAll (Helix: replace with clipboard); use
  `"+R` for that.
* **Append at the very end of a buffer without a trailing newline** (`a` on the last
  char) does not add a newline to the document as Helix does; the cursor sits at the
  end of the text and `Esc` puts it back on the last char.
* **Undo is linear**, not a tree; `alt-u`/`alt-U` (earlier/later) are not implemented.
* **Jumplist** entries are plain selections and are not mapped through later edits;
  they are clamped on use.
* **Vertical motion** counts chars, not tab-expanded visual columns, and does not soft
  wrap (the view owns wrapping).
* `ma`/`mi` text objects exist for words, paragraphs, brackets, quotes and `m`;
  the tree-sitter objects (`f t a c e g x`) and tree-sitter motions (`alt-o/i/p/n`) do not.
* `*` uses the word under a bare cursor (Helix needs an explicit selection).
* Window, buffer, picker, LSP, DAP, shell-pipe (`| ! $`), `:`-commands for files and
  `Z` sticky mode are out of scope.
* Counts are capped at 10 000.
* `:sort` also sorts the lines of a multi-line selection.

From Zed:

* Only the bindings listed above exist (no multi-buffer, no fold, no snippets, no
  `ctrl-up/down` scrolling beyond a one-line `Scroll` request).
* `ctrl-alt-up/down` and `alt-shift-up` add cursors; `alt-shift-down` duplicates the
  line (Zed binds duplicate to both alt-shift directions).
* `Tab` always inserts four spaces rather than aligning to the next tab stop.
* Typing groups for undo break on whitespace -> non-whitespace transitions, not on a
  timer (the crate has no clock).
