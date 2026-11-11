# Vim oracle

The Vim keymap is checked against real Vim (9.1 here). Nothing in this directory runs in CI:
CI replays the recorded answers (`golden.tsv`), so `cargo test` needs no `vim` binary.

## How a case is run

`oracle.py` starts `vim -u NONE -i NONE -N -es` (no vimrc, no viminfo, no plugins), loads the
text, places the cursor, feeds the keys and reads back the buffer, the cursor (line and byte
column) and the registers named in the case. The profile is fixed to what the engine
implements (see "Deviations from Vim" in the crate README):

    set ignorecase smartcase wrapscan autoindent shiftwidth=4 expandtab softtabstop=4
    set nrformats=bin,hex tabstop=4 nojoinspaces backspace=indent,eol,start
    nnoremap Y y$

Details that matter for fidelity:

* Keys are fed with `feedkeys(..., "xt")` per command (the `¦` separator in a case splits
  commands). "Typed" semantics are what a person gets: undo and macros behave differently
  inside one `:normal`, which has a single undo block.
* A case starting with `:` is run as an Ex command line.
* A failing command beeps and drops the keys behind it, so the fuzzer feeds each command in
  its own segment.

## Files

| file | role |
| --- | --- |
| `oracle.py` | `probe 'text' 'keys' ['regs']` shows one case; `generate cases.txt golden.tsv` records many |
| `gen_cases.py`, `gen_extra.py` | write `cases.txt` (inputs): motions, operators, objects, registers, Visual, dot, macros, marks, search, Ex, insert, tabs and wide characters, unicode, CRLF, unterminated last lines |
| `golden.tsv` | recorded answers: `text<TAB>keys<TAB>vim's text<TAB>line<TAB>col[<TAB>reg=kind:text]...` |
| `deviations.tsv` | the cases where this crate knowingly differs (`text<TAB>keys<TAB>why`) |
| `mark_deviations.py` | turns a failure report into `deviations.tsv`, accepting only documented categories |
| `gen_dsl_tests.py` | writes the table-driven Rust tests `src/tests/vim_*_cases.rs` with Vim's answers |
| `fuzzdiff.py` | differential fuzzer: random key sequences through Vim and through the engine, minimal differing prefix per sequence |

`src/tests/vim_oracle.rs` replays `golden.tsv` in `cargo test` (text, cursor and registers
must match, except for rows in `deviations.tsv`, which must still differ so that a fixed
deviation gets removed from the list). Two ignored helpers drive the engine from Python:
`probe` (one case) and `engine_runner` (a batch for `fuzzdiff.py`).

## Workflow

    cd crates/theta-editor/tests/vim_oracle
    ./gen_cases.py > cases.txt && ./oracle.py generate cases.txt golden.tsv   # re-record
    ORACLE_REPORT=/tmp/report.tsv cargo test -p theta-editor golden          # list differences
    ./mark_deviations.py /tmp/report.tsv > deviations.tsv                     # accept documented ones
    ./gen_dsl_tests.py                                                        # regenerate the Rust tables
    ./fuzzdiff.py 2000 1                                                      # differential fuzzing

`PROBE_TEXT='fo[o] bar' PROBE_KEYS='dw' cargo test -p theta-editor probe -- --ignored --nocapture`
shows what the engine does with one case; compare with `./oracle.py probe 'fo[o] bar' 'dw'`.

## Status

* **8 287 golden cases** recorded from Vim 9.1; **8 150 agree** with the engine (text, cursor
  and registers); **137 are documented deviations** in `deviations.tsv`.
* About 3 500 more cases are generated into Rust tests by `gen_dsl_tests.py`, with their
  expected values taken from Vim at generation time.
* `fuzzdiff.py` was run repeatedly during development; every disagreement it found was either
  fixed or became a golden case. Random key soup also produces sequences Vim itself treats
  inconsistently (beeps that drop typeahead), which are not counted.

## Deviations kept

| why | cases |
| --- | --- |
| A buffer whose last line is empty and has no final line break is not representable (the editor models a final break as the end of the last line), so motions onto that line differ | 134 |
| Visual-block does not split tabs into spaces when a block edge cuts through one | 1 |
| `*` / `#` use Unicode word characters; Vim uses its character classes (`iskeyword`) for the word boundaries | 2 |

Everything else that differs from Vim on purpose (leader key, `Y`, auto-pairs, scan limits,
unsupported regex constructs, ...) is not an oracle deviation: the oracle profile is set up
to match those choices, or the cases are not generated. They are listed in the crate README
under "Deviations from Vim".
