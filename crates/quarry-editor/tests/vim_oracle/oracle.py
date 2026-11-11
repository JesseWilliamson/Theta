#!/usr/bin/env python3
"""Run key sequences through real Vim and print the resulting buffer + cursor.

    oracle.py probe 'text with \\n escapes' 'keys<esc>' ['regs to dump']   # one case, human readable
    oracle.py generate cases.txt golden.tsv                                 # batch: see README.md

Keys use the same notation as the Rust test DSL (`<esc> <ret> <tab> <C-a> <lt> <space> <backspace>`).
Needs `vim` (9.x) on PATH; runs `vim -u NONE -N -es`, so no vimrc/plugins are involved.
"""
import json, os, re, subprocess, sys, tempfile

VIM = os.environ.get("VIM", "vim")

SPECIAL = {
    "esc": "\\<Esc>", "ret": "\\<CR>", "tab": "\\<Tab>", "backspace": "\\<BS>", "bs": "\\<BS>",
    "del": "\\<Del>", "space": " ", "lt": "<", "gt": ">", "left": "\\<Left>", "right": "\\<Right>",
    "up": "\\<Up>", "down": "\\<Down>", "home": "\\<Home>", "end": "\\<End>",
}


def unescape(s):
    out, i = [], 0
    while i < len(s):
        if s[i] == "\\" and i + 1 < len(s):
            out.append({"n": "\n", "t": "\t", "r": "\r", "\\": "\\"}.get(s[i + 1], "\\" + s[i + 1]))
            i += 2
        else:
            out.append(s[i])
            i += 1
    return "".join(out)


def escape(s):
    return s.replace("\\", "\\\\").replace("\n", "\\n").replace("\t", "\\t").replace("\r", "\\r")


def vim_string(keys):
    """Translate DSL keys into the body of a double-quoted Vim string literal."""
    out, i = [], 0
    while i < len(keys):
        c = keys[i]
        m = re.match(r"<([^<>]{1,14})>", keys[i:]) if c == "<" else None
        if m:
            name = m.group(1)
            if name in SPECIAL:
                out.append(SPECIAL[name])
                i += m.end()
                continue
            mm = re.fullmatch(r"(?:C|ctrl)-(.)", name)
            if mm:
                out.append("\\<C-%s>" % mm.group(1))
                i += m.end()
                continue
        out.append("\\\\" if c == "\\" else '\\"' if c == '"' else c)
        i += 1
    return "".join(out)


def split_marker(text):
    """`fo[o] bar` -> ('foo bar', line, byte_col): the cursor marker is `[x]` (or `‹x›` when the text itself
    contains brackets); `[]` marks an empty position. No marker means line 1, column 1."""
    o, c = ("‹", "›") if "‹" in text else ("[", "]")
    i = text.find(o)
    if i < 0:
        return text, 1, 1
    j = text.index(c, i)
    inner = text[i + 1:j]
    clean = text[:i] + inner + text[j + 1:]
    before = clean[:i]
    line = before.count("\n") + 1
    col = len(before.rsplit("\n", 1)[-1].encode()) + 1
    return clean, line, col


def body_ex(ex):
    return ex.replace("\\", "\\\\").replace('"', '\\"')


def run(text, keys, regs=('"',), setup=(), ex=None):
    """Returns dict(text, line, col, mode, regs={name: [text, type]}). `col` is Vim's 1-based byte column.

    `ex` runs an Ex command line instead of `normal!` keys (used for `:s`, `:g` ... cases)."""
    text, cl, cc = split_marker(text)
    with tempfile.TemporaryDirectory() as d:
        inp, outp, info = (os.path.join(d, n) for n in ("in.txt", "out.txt", "info.json"))
        with open(inp, "w", newline="") as f:
            f.write(text)
        regq = ",".join("'%s': [getreg('%s'), getregtype('%s')]" % ((r.replace("'", "''"),) * 3) for r in regs)
        # A broken bar (U+00A6) separates commands typed one after another: each is fed with
        # feedkeys(.., "xt") ("typed"), so undo sees separate changes; a whole `:normal` is one block.
        if ex is not None:
            line = 'silent! exe "%s"' % body_ex(ex)
        elif "\u00a6" in keys:
            line = "\n".join('silent! call feedkeys("%s", "xt")' % vim_string(seg) for seg in keys.split("\u00a6"))
        else:
            line = 'silent! exe "normal %s"' % vim_string(keys)
        script = [
            "set nocompatible noswapfile nofixeol wrapscan nohlsearch shortmess+=I ignorecase smartcase",
            "set autoindent shiftwidth=4 expandtab softtabstop=4",
            "nnoremap Y y$",
            "set nrformats=bin,hex tabstop=4",
            "call cursor(%d, %d)" % (cl, cc),
            "clearjumps",
            "call setpos(\"''\", [0, %d, %d, 0])" % (cl, cc),
            *setup,
            line,
            "let g:info = {'line': line('.'), 'col': col('.'), 'mode': mode(), 'regs': {%s}}" % regq,
            "call writefile([json_encode(g:info)], '%s')" % info,
            "silent! w! %s" % outp,
            "qa!",
        ]
        sp = os.path.join(d, "s.vim")
        with open(sp, "w") as f:
            f.write("\n".join(script) + "\n")
        env = dict(os.environ, LC_ALL="C.UTF-8")
        subprocess.run([VIM, "-u", "NONE", "-i", "NONE", "-N", "-es", "-S", sp, inp], env=env, stdin=subprocess.DEVNULL,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=20)
        with open(info) as f:
            res = json.load(f)
        with open(outp, newline="") as f:
            res["text"] = f.read()
        return res


def main():
    if len(sys.argv) >= 4 and sys.argv[1] == "probe":
        regs = tuple(sys.argv[4].split()) if len(sys.argv) > 4 else ('"',)
        r = run(unescape(sys.argv[2]), sys.argv[3], regs=regs)
        print(json.dumps(r, ensure_ascii=False))
    elif len(sys.argv) == 4 and sys.argv[1] == "generate":
        generate(sys.argv[2], sys.argv[3])
    else:
        print(__doc__)
        sys.exit(2)


def generate(cases_path, out_path):
    """cases.txt: `text <TAB> keys [<TAB> regs]` per line (text uses \\n \\t \\\\ escapes). `#` comments.
    A `keys` value starting with `:` is run as an Ex command line (without the colon)."""
    rows = []
    with open(cases_path, encoding="utf-8") as f:
        lines = f.read().split("\n")
    for ln in lines:
        if not ln.strip() or ln.lstrip().startswith("#"):
            continue
        parts = ln.split("\t")
        text, keys = unescape(parts[0]), parts[1]
        regs = tuple(parts[2].split()) if len(parts) > 2 and parts[2] else ()
        if keys.startswith(":"):
            r = run(text, keys, regs=regs or ('"',), ex=keys[1:])
        else:
            r = run(text, keys, regs=regs or ('"',))
        regs_out = ["%s=%s:%s" % (k, v[1], escape(v[0])) for k, v in r["regs"].items()] if regs else []
        rows.append("\t".join([escape(text), keys, escape(r["text"]), str(r["line"]), str(r["col"])] + regs_out))
    with open(out_path, "w", encoding="utf-8") as f:
        f.write("\n".join(rows) + "\n")
    print("wrote %d cases to %s" % (len(rows), out_path))


if __name__ == "__main__":
    main()
