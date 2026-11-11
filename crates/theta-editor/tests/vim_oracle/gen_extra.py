"""More oracle cases: tabs and display columns, unicode, unterminated last lines, CRLF, ex commands."""


def add_all(add, at):
    # ---- tabs: vertical motions and `|` use display columns ---------------------------------------------
    TAB = "\tone two\n    four five six\n\t\tx\nabcdefghij\n\n  \tmixed\n"
    for start in (0, 1, 5, 14, 22, 30, 48, 55):
        for k in ["j", "jj", "k", "3j", "$", "$j", "$jj", "0j", "j0", "^", "|", "5|", "9|", "13|", "wj", "wjj", "eej", "jl", "jlk", "jh", "G", "Gk", "Gkk",
                  "<C-v>jIX<esc>", "<C-v>j$d", "dj", "dk", "yjP", "x", "~", "rx",
                  ">>", "<lt><lt>", "cwX<esc>", "ix<esc>", "ax<esc>", "Ax<esc>", "Ix<esc>", "ox<esc>", "Ox<esc>", "J", "dd", "yyp", "gUU", "i<tab>x<esc>", "a<tab>x<esc>"]:
            add(at(TAB, start), k, "\" 0 1 -")

    # ---- unicode -------------------------------------------------------------------------------------------
    UNI = "日本語 テスト ひらがな カタカナ abc한글 x\nfoo日本bar x，y。z\na🙂b🙂🙂c ñandú naïve\nα β-γ δ.ε 😀 é́x\n"
    for start in (0, 4, 8, 14, 20, 24, 30, 36, 44, 50, 56, 66):
        for k in ["w", "ww", "www", "b", "bb", "e", "ee", "W", "B", "E", "ge", "x", "dw", "dW", "de", "db", "cwX<esc>", "ciwX<esc>", "caw<esc>", "diw", "daw", "yiwP", "~", "gUiw",
                  "fa", "tb", "l", "h", "$", "0", "^", "rx", "j", "k", "*", "dis", "J"]:
            add(at(UNI, start), k, "\" 0 -")

    # ---- last line without a final newline -----------------------------------------------------------------
    for text in ["one\ntwo\nthree", "one\ntwo\n\nlast", "single", "a\n\nb"]:
        n = len(text)
        for start in sorted({0, n // 2, n - 1}):
            for k in ["G", "Gdd", "Gddp", "GddP", "dd", "ddGp", "ddGP", "Gyyp", "GyyP", "yyGp", "yyGP", "Go<esc>", "GO<esc>", "Gox<esc>", "GOx<esc>", "GJ", "gJ", "J", "Gp",
                      "GA!<esc>", "Gx", "GD", "Gcc<esc>", "GccX<esc>", "dG", "dgg", "dgg", "yG", "yGGp", "Gdk", "Gyk", "Gddk", "GddGp", ">>", ">G", "Gd}", "dj", "Gdj", "d}", "yipGp",
                      "G$xp", "Gx.", "G.", "ggjdd", "ggjddp", "ggjyyGp", "GddGddGdd", "ggVGd", "ggVGy", "GVd", "GVy$p", "GvpP", "VGp", "Gi<ret><esc>", "GA<ret>x<esc>"]:
                add(at(text, start), k, "\" 0 1")

    # ---- more :s ---------------------------------------------------------------------------------------------
    S = "Foo bar BAZ foo\nbar foo123 foo_bar\n  qux(x, y) f(oo)\nend foo.\n"
    for ex in [":%s/foo/X/g", ":%s/foo/X/gi", ":%s/Foo/X/g", ":%s/FOO/X/gI", ":%s/\\<foo\\>/X/g", ":%s/\\vfoo(\\d+)/<\\1>/", ":%s/\\(\\w\\+\\) \\(\\w\\+\\)/\\2 \\1/",
               ":%s/f(oo)/Y/", ":%s/f\\(oo\\)/Y/", ":%s/\\v\\(x, y\\)/Z/", ":%s/o\\{2}/00/g", ":%s/o\\{1,}/0/g", ":%s/\\d\\+/N/", ":%s/[aeiou]/_/g", ":%s/[^a-z ]/#/g", ":%s/\\s\\+/ /g",
               ":%s/^\\s*//", ":%s/\\s*$//", ":%s/\\w\\+/[&]/g", ":%s/\\w\\+/\\u&/g", ":%s/\\w\\+/\\U&/g", ":%s/\\(\\w\\)\\(\\w*\\)/\\U\\1\\E\\2/g", ":%s/foo/a\\/b/", ":%s#foo#a/b#", ":%s+foo+X+g",
               ":%s/foo/\\&/", ":%s/foo/\\\\/", ":%s/o/\\t/", ":%s/ /\\r/g", ":%s/foo/&&/", ":%s/bar/X/gn", ":%s/foo/X/ 2", ":1,2s/foo/X/g", ":2,$s/foo/X/", ":.,+1s/o/0/g", ":'<,'>s/o/0/",
               ":s/o/0/g", ":s/o/0/", ":&&", ":%s/\\n//", ":%s/\\n/,/", ":%s/o\\zsb/X/", ":%s/\\%(f\\)oo/Q/", ":%s/\\cFOO/Q/g", ":%s/foo\\|bar/W/g", ":%s/ba\\(r\\|z\\)/W/g",
               ":%s/\\v<foo>/X/g", ":%s/\\Vfoo/X/g", ":%s/\\M.*/X/", ":%s/*/X/", ":%s/\\*/X/", ":%s/^/> /", ":%s/$/ </", ":%s/a*/X/", ":%s/x*/-/g", ":%s/\\_s\\+/_/g"]:
        for start in (0, 20):
            add(at(S, start), ex)
    for ex in [":g/foo/s/o/0/g", ":g/foo/normal Ax", ":g/foo/normal dd", ":g/^$/d", ":g/foo/t$", ":g/foo/m0", ":g/foo/j", ":g!/foo/d", ":v/foo/s/^/# /", ":g/foo/d", ":g/o/y A", ":g/bar/>", ":g/foo/normal gUU", ":g/x/+1d", ":g/foo/.,+1j"]:
        add(at(S, 0), ex)
    for ex in [":sort", ":sort!", ":sort i", ":sort n", ":sort u", ":sort /\\a/", ":sort /\\a\\+/ r", ":%sort", ":2,3sort", ":sort x"]:
        add(at("b 10\nB 9\na 2\nc 1\na 2\n 3 z\nfoo\n", 0), ex)

    # ---- CRLF ---------------------------------------------------------------------------------------------------
    CR = "one two\r\nthree four\r\n\r\nfive\r\n"
    for start in (0, 4, 9, 20, 22):
        for k in ["j", "$", "w", "dd", "yyp", "x", "dw", "d$", "J", "A!<esc>", "o<esc>", "Ox<esc>", "ix<ret><esc>", "cc<esc>", "ccX<esc>", "dj", "p", "G", "Gdd", ">>", "~", "r<ret>", "ddp", "yjP", "2J", "gJ", "vjd", "Vjd", "<C-v>jd"]:
            add(at(CR, start), k, "\" 0 1")
    for ex in [":%s/o/0/g", ":%s/$/;/", ":%s/^/> /", ":%s/\\n//", ":g/e/d", ":sort", ":2m0", ":1t$", ":%norm Ax"]:
        add(at(CR, 0), ex)
