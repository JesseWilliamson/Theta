#!/usr/bin/env python3
"""Generate cases.txt (the inputs of the oracle run). Edit this file, then run:

    ./gen_cases.py > cases.txt && ./oracle.py generate cases.txt golden.tsv

`text` marks the cursor with `[x]` (or `‹x›` when the text contains brackets); `keys` use the DSL
of the Rust tests. Lines are `text<TAB>keys[<TAB>registers to compare]`.
"""
import sys

cases = []


def add(text, keys, regs=""):
    t = text.replace("\\", "\\\\").replace("\n", "\\n").replace("\t", "\\t").replace("\r", "\\r")
    cases.append("\t".join([t, keys, regs]) if regs else "\t".join([t, keys]))


def at(text, i):
    """Put the cursor marker on char i of text."""
    c = text[i] if i < len(text) and text[i] != "\n" else ""
    return text[:i] + "[" + c + "]" + text[i + (1 if c else 0):]


PROSE = "foo bar_baz  qux.quux (a, b)\n  indented line\n\nlast line here\n"
SHORT = "one two three\nfour five\nsix\n"

# ---- motions: cursor position after each -------------------------------------------------------------
MOTIONS = ["h", "l", "j", "k", "w", "b", "e", "W", "B", "E", "ge", "gE", "0", "^", "$", "g_", "gg", "G", "2w", "3b", "2e",
           "fa", "Fa", "ta", "Ta", "f(", "2fa", "}", "{", ")", "(", "%", "2j", "3k", "+", "-", "_",
           "|", "5|", "2$", "3l", "3h", "fa;", "fa,", "ta;", "ta,", "wwww", "bbbb", "eeee", "gg3j", "2G", "3G", "4G"]
for start in (0, 6, 15, 30, 36, 51, 60):
    for m in MOTIONS:
        add(at(PROSE, start), m)

# ---- operators x motions -------------------------------------------------------------------------------------
OPS = [("d", ""), ("y", ""), ("c", "X<esc>"), (">", ""), ("<lt>", ""), ("g~", ""), ("gu", ""), ("gU", "")]
OPMOT = ["w", "W", "b", "B", "e", "E", "ge", "$", "0", "^", "h", "l", "j", "k", "G", "gg", "}", "{", ")", "(", "fa", "ta", "Fa", "Ta", "%",
         "2w", "3l", "2j", "iw", "aw", "iW", "aW", "ip", "ap", "i(", "a(", "ib", "i\"", "a\"", "is", "as", "2iw", "2aw", "g_", "|", "+", "-"]
TXT = "foo bar_baz  qux.quux (a, b)\n  indented \"str ing\" line\n\nlast line here\n"
for start in (0, 4, 9, 26, 33, 52):
    for op, tail in OPS:
        for m in OPMOT:
            if op in ("<lt>", ">") and len(m) > 2:
                continue
            regs = "\" 0 1 -" if op in ("d", "y", "c") else ""
            add(at(TXT, start), op + m + tail, regs)

# ---- doubled operators and counts -------------------------------------------------------------------------------
for start in (0, 5, 30, 40):
    for k in ["dd", "2dd", "3dd", "yyp", "yyP", "2yyp", "cc<esc>", "ccX<esc>", "3ccX<esc>", ">>", "2>>", "<lt><lt>", "g~~", "gUU", "guu", "g~g~", "gUgU",
              "x", "3x", "X", "3X", "D", "2D", "C<esc>", "Y", "s<esc>", "3s<esc>", "S<esc>", "J", "3J", "gJ", "~", "3~", "rx", "3rx", "r<ret>", "R12<esc>", "p", "P", "xp", "ddp", "ddP"]:
        add(at(TXT, start), k, "\" 0 1 -")

# ---- text objects with ops ------------------------------------------------------------------------------------------
OBJ_TEXTS = [
    ("f(a, (b, c), d) + g[1]", [2, 5, 9, 18]),
    ("say \"hello there\" and 'it' ok `x`", [0, 6, 9, 15, 22, 30]),
    ("if (x) {\n    foo(1);\n    bar(2);\n}\nafter\n", [0, 12, 20, 30, 38]),
    ("one. Two three! Four? five.\n", [0, 7, 12, 17]),
    ("a b\n\nc d\ne f\n\n\ng h\n", [0, 5, 6, 10, 12, 14]),
    ("<a> [b] {c} (d)", [1, 5, 9, 13]),
]
for text, starts in OBJ_TEXTS:
    for s in starts:
        for o in ["iw", "aw", "iW", "aW", "is", "as", "ip", "ap", "i(", "a(", "ib", "ab", "i[", "a[", "i{", "a{", "iB", "aB", "i<", "a<", "i'", "a'",
                  "i\"", "a\"", "i`", "a`", "2i(", "2a(", "2iw", "3aw"]:
            for op in ["d", "y", "c"]:
                add(at(text, s), op + o + ("X<esc>" if op == "c" else ""), "\" 0 1")

# ---- registers, put -------------------------------------------------------------------------------------------------------
for start in (0, 4, 14):
    for k in ["yw$p", "yw$P", "yy$p", "yyjP", "2yyjp", "dwp", "dwP", "yiwwviwp", "yiwwviwP", "\"ayw\"Aywj\"ap", "\"add\"Add\"ap", "yy3p", "yw3P", "ywgp", "yygp", "yyGp", "yyGP",
              "dd\"1p", "dd¦dd¦u¦\"2p", "yw\"_dw\"0P", "\"qyw@q<esc>", "xxxp", "vey$p", "Vjyp", "VjyP", "vey$gp", "Vjy$gP"]:
        add(at(SHORT, start), k, "\" 0 1 2 - a")

# ---- visual -------------------------------------------------------------------------------------------------------------------
for start in (0, 5, 15, 21):
    for k in ["vld", "vjd", "vey", "Vd", "Vjd", "Vjy", "vjcX<esc>", "VjcX<esc>", "vwd", "v$d", "v$y", "vj$d", "vU", "vu", "v~", "Vjg~", "vlr-", "VjrX", "vjJ", "Vjj>", "Vj>", "Vj<lt>",
              "vio", "vawd", "viwd", "vipd", "vi(d", "vawawd", "viwiwd", "vlloh", "vjokd", "vlld.", "vly$p", "vlyjvp", "Vyjvp", "vlyjVp",
              "v$hd", "vggd", "vGd", "vjvd", "vVd", "vlVvd", "vl<esc>x", "vlgvd", "vly`>x", "vly'<x", "v<esc>"]:
        add(at(SHORT + "seven eight\n", start), k, "\" 0 1 -")

# ---- visual block ----------------------------------------------------------------------------------------------------------------------
BLK = "abcdef\nghijkl\nmnopqr\nstuvwx\n"
for start in (0, 2, 7, 9):
    for k in ["<C-v>jld", "<C-v>jly", "<C-v>jjlld", "<C-v>jlIX<esc>", "<C-v>jlAX<esc>", "<C-v>jj$AX<esc>", "<C-v>jjlcX<esc>", "<C-v>jlrx", "<C-v>jjly$p", "<C-v>jlyP",
              "<C-v>jlU", "<C-v>jl~", "<C-v>jjoOd", "<C-v>j$d", "<C-v>jjD", "<C-v>jlJ", "<C-v>jl>", "<C-v>jlo<esc>x", "<C-v>jlhd", "<C-v>llly0P", "<C-v>jjIab<esc>", "<C-v>jjAab<esc>"]:
        add(at(BLK, start), k, "\" 0 1 -")
add("[a]bc\nd\nefghi\nj\n", "l<C-v>jjjAX<esc>")
add("[a]bc\nd\nefghi\nj\n", "l<C-v>jjjIX<esc>")
add("[a]bc\nd\nefghi\nj\n", "ll<C-v>jjjAX<esc>")
add("[a]bc\nd\nefghi\nj\n", "$<C-v>jjj$AX<esc>")

# ---- insert / replace --------------------------------------------------------------------------------------------------------------------
for start in (0, 5, 14):
    for k in ["ifoo<esc>", "afoo<esc>", "Ifoo<esc>", "Afoo<esc>", "ofoo<esc>", "Ofoo<esc>", "gIfoo<esc>", "3ifoo<esc>", "3afoo<esc>", "3ofoo<esc>", "3Ofoo<esc>", "3Ax<esc>",
              "ix<ret>y<esc>", "i<ret><esc>", "ia<bs>b<esc>", "ia<C-w>b<esc>", "ifoo bar<C-w><C-w>x<esc>", "ifoo<C-u>x<esc>", "ix<esc>.", "ix<esc>j.", "3ix<esc>j.", "ix<esc>3.",
              "Rab<esc>", "Rabcdefghijkl<esc>", "Rab<bs><bs><esc>", "Rab<bs><esc>", "R<ret>x<esc>", "3Rab<esc>", "rx", "3rx", "5rx", "99rx", "ix<C-o>0y<esc>", "ix<C-o>$y<esc>",
              "ia<C-r>\"b<esc>", "ia<C-t>b<esc>", "ia<C-t><C-d>b<esc>", "i<tab>x<esc>", "ia<left>b<esc>", "ia<right>b<esc>", "ia<up>b<esc>", "ia<down>b<esc>", "ia<home>b<esc>", "ia<end>b<esc>"]:
        add(at(SHORT, start), k, "\" 0")

# ---- dot repeat --------------------------------------------------------------------------------------------------------------------------------
DOTS = ["x.", "x..", "3x.", "3x2.", "dw.", "dw..", "2dw.", "d2w.", "dd.", "dd..", "2dd.", "3dd2.", "cwX<esc>w.", "cwX<esc>j0.", "ciwX<esc>w.", "A!<esc>j.", "I-<esc>j.",
        "ofoo<esc>.", "Obar<esc>j.", "rxl.", "~.", "~~.", "J.", "gJ.", ">>.", "<lt><lt>.", "p.", "P.", "yyp.", "ddp.", "g~w.", "guiw.", "gUiww.", "vly.", "vlcX<esc>j0.",
        "vlldj.", "Vdj.", "Vjdj.", "<C-v>jdl.", "Rab<esc>.", "3ix<esc>.", "ix<esc>3.", "A<C-r>.<esc>", "x¦u¦.", "dd¦u¦.", "cwab<esc>¦u¦.", "5x.3.", "d}.", "df.", "dtx.", "d/o<ret>."]
for start in (0, 6, 22):
    for k in DOTS:
        add(at(SHORT + "seven eight\nnine ten\n", start), k, "\" 0 1")

# ---- marks, jumps, search ---------------------------------------------------------------------------------------------------------------------------
for k in ["majmaj'a", "ma$`a", "majd'a", "majd`a", "majy`a$p", "mbjjmc'b'c", "Gjj''", "GG``", "/two<ret>", "/two<ret>n", "/e<ret>nn", "/e<ret>N", "?e<ret>", "?e<ret>n", "?e<ret>N",
          "*", "#", "*n", "*N", "#n", "g*", "g#", "2*", "/t\\w*<ret>", "/\\<t<ret>", "/e\\>/<ret>", "d/two<ret>", "c/two<ret>X<esc>", "y/e<ret>P", "d?e<ret>", "/e/e<ret>", "/e/+1<ret>", "/e/s-1<ret>",
          "/thr/b+1<ret>", "d/e/e<ret>", "/zzz<ret>x", "/\\v(o|e)+<ret>", "/o\\{2}<ret>", "/\\(o\\|e\\)<ret>n", "/^$<ret>", "/$<ret>", "/e$<ret>", "/^s<ret>", "<C-o>", "Gx<C-o>x", "G<C-o><C-i>x",
          "3G<C-o>", "ggG<C-o>x", "ggGgg<C-o><C-o>x", "G``x", "jmaggd'a", "jjmaggy`a$p", "`.", "x`.", "ix<esc>j`.", "ix<esc>j`^", "yw'[", "yy']", "yw`]"]:
    for start in (0, 5):
        add(at("one two three\nfour five\nsix\nseven two\n\nlast\n", start), k, "\" 0 /")

# ---- joins, case, indent, increment -----------------------------------------------------------------------------------------------------------
JT = "a\n  b\nc \nd\n\ne\n(f\n)g\n"
for start in (0, 2, 4, 6, 10, 12, 14):
    for k in ["J", "2J", "3J", "4J", "gJ", "2gJ", "J.", "VjJ", "VjjJ", "VjgJ"]:
        add(at(JT, start), k)
for t, ks in [
    ("a 5 b -3 c 007 d 0x0f e x9 f 99\n", ["<C-a>", "5<C-a>", "<C-x>", "w<C-a>", "ww<C-a>", "www<C-x>", "wwww<C-a>", "wwwww<C-a>", "wwwwww<C-a>", "$<C-a>", "9<C-a>", "99<C-x>", "<C-a>.", "<C-a>j<C-a>", "wwwwwww<C-a>", "wwwwwww<C-x>"]),
    ("foo-1 bar-2\n  -5\n9\n10\n", ["<C-a>", "w<C-a>", "j<C-a>", "jj<C-x>", "jjj<C-a>", "jjjj<C-a>", "Vjj<C-a>", "VG<C-x>"]),
]:
    for k in ks:
        add("[" + t[0] + "]" + t[1:], k)
for k in ["gUU", "gUw", "guw", "g~w", "g~~", "gUiw", "gUap", "~", "5~", "g?w", "g??", "guu", "gUGp"]:
    add("[f]oo Bar baZ\nsecond LINE\n\nthird\n", k)
for k in [">>", "<lt><lt>", "2>>", ">j", "<lt>j", ">ip", "<lt>ip", ">G", ">>>>", ">>.", "<lt><lt>.", "Vj>.", ">ap", ">}", "<lt>}", "3>>", ">gg"]:
    add("[a]\n  b\n\n\t c\n    d\n", k)

# ---- undo / redo (a broken bar separates commands: each is typed on its own, see oracle.py) -------------
for k in ["x¦u", "x¦x¦u", "x¦x¦u¦u", "x¦x¦u¦<C-r>", "x¦x¦u¦u¦<C-r>¦<C-r>", "3x¦u", "dd¦u", "dd¦j¦dd¦u¦u", "dd¦j¦dd¦u¦u¦<C-r>", "ifoo<esc>¦u", "ofoo<esc>¦u",
          "ofoo<esc>¦u¦<C-r>", "yyp¦u", "yyp¦u¦<C-r>", "dw¦u", "dw¦u¦<C-r>", "cwx<esc>¦u", "J¦x¦u", "J¦u", ">>¦u", "ix<esc>¦iy<esc>¦u¦u", "ix<esc>¦iy<esc>¦u",
          "ddG¦p¦u", "x¦3u", "x¦j¦x¦j¦x¦u¦u¦u", "x¦u¦u", "x¦u¦.", "Gx¦u", "G¦dd¦G¦u", "dG¦u", "dgg¦u", "dG¦u¦u", "x¦x¦x¦2u", "x¦x¦x¦3u¦2<C-r>", "ix<esc>¦u¦.",
          "cwab<esc>¦j¦cwcd<esc>¦u¦u", "ddjp¦u", "ddjp¦u¦u", "A!<esc>¦u", "o<esc>¦u", "Ofoo<esc>¦u", "3ofoo<esc>¦u", "~¦~¦u", "gUU¦u", "gUU¦u¦<C-r>"]:
    for start in (0, 7):
        add(at("one two\nthree four\nfive\n", start), k)

# ---- ex commands ------------------------------------------------------------------------------------------------------------------------------------
EXT = "foo bar\nfoo baz\nqux foo\nbar\n\nlast foo\n"
for ex in [":s/foo/X/", ":s/foo/X/g", ":%s/foo/X/", ":%s/foo/X/g", ":2,3s/foo/X/", ":2s/o/0/g", ":.,$s/foo/[&]/", ":%s/\\(f\\)oo/\\1-/", ":%s/o\\+/0/", ":%s/\\<foo\\>/F/g", ":%s/bar$/B/", ":%s/^/> /", ":%s/$/;/",
           ":%s/foo//", ":%s/\\v(f)(o)+/\\2\\1/", ":s/foo/\\U&/", ":%s/\\w\\+/\\u&/g", ":%s/ /\\r/", ":%s/\\n//", ":%s/o/0/gi", ":%s/FOO/x/", ":%s/FOO/x/I", ":%s/x/y/e", ":%s/a/b/", ":%s/\\s\\+$//e",
           ":g/foo/d", ":v/foo/d", ":g/foo/s/o/0/g", ":g/^$/d", ":g/foo/m0", ":g/foo/t.", ":g/foo/normal Ax", ":g/foo/j", ":g!/foo/d", ":g/bar/+1d", ":2d", ":2,3d", ":d", ":2,3y\n", ":2,3m0", ":1m$", ":2t0", ":2,3t$", ":1,2co4", ":3m1", ":2,3m1",
           ":sort", ":sort!", ":sort u", ":sort n", ":%sort", ":2,4sort", ":%norm Ax", ":2,3norm Ix", ":%norm dw", ":%norm dd", ":3", ":$", ":1", ":0", ":.+2", ":$-1", ":2;+1d", ":/qux/d", ":?foo?d", ":'a,'bd", ":>", ":2,3>", ":<lt>", ":%>", ":j", ":2,3j", ":j!", ":3,4j!",
           ":2ka", ":2ka|3d", ":pu", ":2pu", ":0pu", ":2pu!", ":1,2y|3pu"]:
    for start in (0, 12):
        add(at(EXT, start), ex.strip("\n"))
add(at(EXT, 0), "yy:2pu<ret>")
add(at(EXT, 0), "yyj:pu<ret>")
add(at(EXT, 0), "yyj:pu!<ret>")

import gen_extra

gen_extra.add_all(add, at)

for c in cases:
    print(c)
