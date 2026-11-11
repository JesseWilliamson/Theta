#!/usr/bin/env python3
"""Record the oracle cases where this crate knowingly differs from Vim.

    ORACLE_REPORT=report.tsv cargo test -p quarry-editor golden     # writes every failing case
    ./mark_deviations.py report.tsv > deviations.tsv

Only failures that fall into one of the documented categories below are accepted; anything else
is printed to stderr and left out, so it keeps failing the golden test until it is fixed.
The golden test insists that every listed case still differs (a fixed deviation must be removed).
"""
import sys


def category(before, keys):
    """Name of the documented deviation this failing case belongs to, or None."""
    ends_in_newline = before.endswith("\\n")
    if not ends_in_newline:
        # An unterminated last line: Vim can keep an empty last line without a break, the rope cannot
        # (README: "buffers without a final newline").
        return "empty last line without a final newline"
    if "<C-v>" in keys and "\\t" in before:
        return "blocks cutting through a tab are not split into spaces"
    if keys in ("*", "#") and any(ord(c) > 0x2000 for c in before):
        return "* / # word boundaries use Unicode word characters, not Vim's character classes"
    return None


def main():
    seen, bad = set(), []
    for line in open(sys.argv[1], encoding="utf-8"):
        f = line.rstrip("\n").split("\t")
        before, keys = f[0], f[1]
        c = category(before, keys)
        if c is None:
            bad.append((before, keys))
            continue
        if (before, keys) not in seen:
            seen.add((before, keys))
            print("\t".join([before, keys, c]))
    for before, keys in bad:
        print("NOT A KNOWN DEVIATION: %r %r" % (before, keys), file=sys.stderr)


if __name__ == "__main__":
    main()
