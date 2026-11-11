#!/usr/bin/env python3
"""Differential fuzzing of the Rust Vim keymap against real Vim.

    ./fuzzdiff.py [N [SEED]]

Generates N random key sequences over a few documents, runs each through Vim (feedkeys, i.e. as typed
keys) and through the engine (`cargo test engine_runner`), and prints the minimal prefix of every
sequence that makes them disagree, grouped by the last command typed. Not run in CI: it needs `vim`.
"""
import collections
import os
import random
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import oracle

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.abspath(os.path.join(HERE, "..", ".."))

DOCS = [
    "foo bar baz\n  qux (a, b) 'c d'\n\nlast_line here.\n",
    "one two three. Four five!\n  six {seven} [eight]\n\tnine \"ten eleven\"\n",
    "x\n\n\ny z\n",
    "alpha beta\ngamma delta\nepsilon\nzeta eta theta\niota\n",
]
MOTIONS = ["h", "j", "k", "l", "w", "b", "e", "W", "B", "E", "0", "^", "$", "g_", "gg", "G", "ge", "gE", "}", "{", ")", "(", "%", "+", "-", "_",
           "fo", "Fa", "to", "Ta", "f ", ";", ",", "2w", "3l", "2j", "2b", "3e", "|", "5|", "*", "#", "n", "N", "/e<ret>", "?a<ret>", "2G", "3G"]
TEXTOBJ = ["iw", "aw", "iW", "aW", "is", "as", "ip", "ap", "i(", "a(", "i{", "a{", "i[", "a[", "i'", "a'", 'i"', 'a"', "ib", "iB", "2iw", "2aw"]
OPS = ["d", "c", "y", ">", "<lt>", "g~", "gu", "gU"]
SIMPLE = ["x", "X", "D", "C<esc>", "s<esc>", "S<esc>", "J", "gJ", "~", "p", "P", "gp", "gP", "u", "<C-r>", ".", "rx", "r<ret>", "2x", "3dd", "dd", "yy", "cc<esc>",
          "<C-a>", "<C-x>", "5<C-a>", "ma", "'a", "`a", "gv", "Y", ">>", "<lt><lt>", "gUU", "guu", "g~~", "2dd", "3J", "yyp", "ddp"]
TEXTS = ["foo", "x_y", "--a", "é", "12", "end."]


def rand_cmd(rng):
    r = rng.random()
    if r < 0.40:
        return rng.choice(MOTIONS)
    if r < 0.58:
        op = rng.choice(OPS)
        mot = rng.choice(MOTIONS + TEXTOBJ)
        s = (str(rng.randint(2, 3)) if rng.random() < 0.15 else "") + op + mot
        if op == "c":
            s += rng.choice(TEXTS) + "<esc>"
        return s
    if r < 0.66:
        return rng.choice(["i", "a", "A", "I", "o", "O", "gI", "R", "3i", "2a"]) + rng.choice(TEXTS) + "<esc>"
    if r < 0.72:
        ends = ["d", "y", "c" + rng.choice(TEXTS) + "<esc>", "~", "U", "u", ">", "J", "r-", "p", "<esc>", "od", "I" + rng.choice(TEXTS) + "<esc>",
                "A" + rng.choice(TEXTS) + "<esc>"]
        return rng.choice(["v", "V", "<C-v>"]) + "".join(rng.choice(MOTIONS[:20]) for _ in range(rng.randint(1, 3))) + rng.choice(ends)
    return rng.choice(SIMPLE)


def gen(n, seed):
    rng = random.Random(seed)
    cases = []
    for _ in range(n):
        doc = rng.choice(DOCS)
        pos = rng.randint(0, max(0, len(doc) - 1)) if doc else 0
        c = doc[pos] if doc and doc[pos] != "\n" else ""
        text = doc[:pos] + "[" + c + "]" + doc[pos + (1 if c else 0):] if doc else "[]"
        cmds = [rand_cmd(rng) for _ in range(rng.randint(1, 8))]
        cases.append((text, cmds))
    return cases


def run_vim(text, cmds):
    # each command is fed separately ("typed"), so a failing command does not flush the rest
    r = oracle.run(text, "¦".join(list(cmds) + ["<esc><esc>"]), regs=())
    return r["text"], r["line"], r["col"]


def same(e, v, doc):
    """Compare (text, line, col); an emptied buffer keeps Vim's final newline, this crate's does not."""
    et, vt = e[0], v[0]
    if "\ufffd" in vt:
        return True  # Vim wrote invalid UTF-8 (a multibyte key typed through feedkeys): not comparable
    if doc.endswith("\n") and not et.endswith("\n") and vt.endswith("\n") and "\n" not in et:
        et += "\n"
    return (et, e[1], e[2]) == (vt, v[1], v[2])


def run_engine(cases):
    with tempfile.TemporaryDirectory() as d:
        inp, out = os.path.join(d, "in.tsv"), os.path.join(d, "out.tsv")
        with open(inp, "w") as f:
            for text, keys in cases:
                f.write(oracle.escape(text) + "\t" + keys + "\n")
        env = dict(os.environ, ENGINE_CASES=inp, ENGINE_OUT=out, CARGO_TARGET_DIR=os.environ.get("CARGO_TARGET_DIR", "/home/user/targets/vim"))
        subprocess.run(["cargo", "test", "-q", "-p", "theta-editor", "engine_runner", "--", "--ignored"], cwd=CRATE, env=env, check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        res = []
        with open(out) as f:
            for line in f:
                t, l, c = line.rstrip("\n").split("\t")
                res.append((oracle.unescape(t), int(l), int(c)))
        return res


def main():
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 500
    seed = int(sys.argv[2]) if len(sys.argv) > 2 else 1
    cases = gen(n, seed)
    eng = run_engine([(t, "".join(c)) for t, c in cases])
    bad = []
    for (text, cmds), e in zip(cases, eng):
        v = run_vim(text, cmds)
        if not same(e, v, text.replace("[", "").replace("]", "")):
            bad.append((text, cmds))
    print("%d of %d sequences differ" % (len(bad), n))
    # shrink: shortest command prefix that still differs
    pref = []
    for text, cmds in bad:
        for k in range(1, len(cmds) + 1):
            pref.append((text, cmds[:k]))
    eng = run_engine([(t, "".join(c)) for t, c in pref])
    seen = collections.OrderedDict()
    for (text, cmds), e in zip(pref, eng):
        if any(k[0] == text and tuple(cmds[:len(k[1])]) == k[1] for k in seen):
            continue
        v = run_vim(text, cmds)
        if not same(e, v, text.replace("[", "").replace("]", "")):
            seen[(text, tuple(cmds))] = (e, v)
    groups = collections.defaultdict(list)
    for (text, cmds), (e, v) in seen.items():
        groups[cmds[-1]].append((text, cmds, e, v))
    for last, items in sorted(groups.items(), key=lambda kv: -len(kv[1])):
        print("== %s (%d)" % (last, len(items)))
        for text, cmds, e, v in items[:2]:
            print("   text %r\n   keys %s\n   engine %r %d:%d\n   vim    %r %d:%d" % (text, "".join(cmds), e[0], e[1], e[2], v[0], v[1], v[2]))


if __name__ == "__main__":
    main()
