//! The Vim command line (`:`): ranges, `:s`, `:g`/`:v`, `:d :y :m :t :>` ..., `:sort`, `:normal`,
//! `:set`, `:registers`, `:marks`, and the commands that map to host effects (shared with the
//! Helix `:` commands through `Editor::exec_host_command`).

use super::geom::*;
use super::ops::*;
use super::vregex;
use super::{Op, Reg, RegKind};
use crate::editor::{Editor, Effect, Mode};
use crate::keys::KeyInput;
use crate::selection::{Selection, Selections};
use crate::transaction::Change;
use regex::Regex;

/// The last `:s` (for `:s` / `:&` without arguments).
#[derive(Clone, Debug)]
pub(crate) struct SubSpec {
    pub pat: String,
    pub rep: String,
    pub flags: String,
}

struct Range {
    /// Vim line numbers (1-based, 0 = before the first line)
    a: isize,
    b: isize,
    /// how many addresses were given
    n: usize,
}

type Res<T> = Result<T, String>;

fn skip_ws(s: &str) -> &str {
    s.trim_start_matches([' ', '\t'])
}

impl Editor {
    pub(crate) fn vim_ex(&mut self, line: &str) {
        let line = line.trim_start_matches([':', ' ', '\t']);
        if line.is_empty() {
            return;
        }
        if self.vim.ex_depth == 0 && !line.starts_with('@') {
            self.vim.last_ex = Some(line.to_string());
        }
        if self.vim.ex_depth > 20 {
            return;
        }
        self.vim.ex_depth += 1;
        let r = self.ex_run(line);
        self.vim.ex_depth -= 1;
        if let Err(e) = r {
            self.status(e);
        }
    }

    fn cur_lnum(&self) -> isize {
        let t = self.rope.slice(..);
        line_of(t, self.vcur()) as isize + 1
    }

    fn last_lnum(&self) -> isize {
        nlines(self.rope.slice(..)) as isize
    }

    fn ex_run(&mut self, line: &str) -> Res<()> {
        let (range, rest) = self.parse_range(line)?;
        let rest = skip_ws(rest);
        // command name
        let mut chars = rest.char_indices().peekable();
        let mut end = 0;
        if let Some(&(_, c)) = chars.peek() {
            if c.is_ascii_alphabetic() {
                while let Some(&(i, c)) = chars.peek() {
                    if c.is_ascii_alphabetic() {
                        end = i + 1;
                        chars.next();
                    } else {
                        break;
                    }
                }
                // `:s#a#b#`, `:g/x/d`: a one-letter command glued to its argument is fine; but
                // `:dl`-style compositions are not supported
            } else if "&<>!=@~*#".contains(c) {
                end = c.len_utf8();
                // `:>>` repeats
                if c == '>' || c == '<' {
                    while rest[end..].starts_with(c) {
                        end += 1;
                    }
                }
            }
        }
        let mut name = rest[..end].to_string();
        let mut args = &rest[end..];
        // `:s` family with letters glued: `:sno`, handled by exact names; `:g`, `:v` take a pattern
        if name.is_empty() {
            return self.ex_goto(&range);
        }
        let mut bang = false;
        if args.starts_with('!')
            && !matches!(name.as_str(), "s" | "substitute" | "g" | "global" | "v" | "vglobal")
            || (args.starts_with('!') && matches!(name.as_str(), "g" | "global"))
        {
            bang = true;
            args = &args[1..];
        }
        if name == "normal" || name == "norm" || name == "norma" {
            // the argument is taken verbatim (leading blanks are skipped by Vim once)
            args = args.strip_prefix(' ').unwrap_or(args);
        } else {
            args = skip_ws(args);
        }
        // `:k a`, `:ka`
        if name.len() == 2 && name.starts_with('k') && !matches!(name.as_str(), "ke") {
            args = &rest[1..];
            name = "k".into();
        }
        // `|` separates commands, except for those that take the rest of the line
        let takes_bar = (name.starts_with('g') && "global".starts_with(name.as_str()) && name != "go")
            || (name.starts_with('v') && "vglobal".starts_with(name.as_str()))
            || (name.len() >= 4 && "normal".starts_with(name.as_str()));
        if !takes_bar {
            if let Some(i) = find_bar(args) {
                let (head, tail) = (&args[..i], &args[i + 1..]);
                self.ex_dispatch(&name, bang, head.trim_end(), &range)?;
                return self.ex_run(tail.trim_start());
            }
        }
        self.ex_dispatch(&name, bang, args, &range)
    }

    fn ex_dispatch(&mut self, name: &str, bang: bool, args: &str, range: &Range) -> Res<()> {
        let is = |full: &str, min: usize| -> bool { name.len() >= min && full.starts_with(name) };
        if range.n > 0 {
            let last = self.last_lnum();
            let (a, b) = (range.a.min(range.b), range.a.max(range.b));
            if b > last || a < 0 {
                return Err("E16: Invalid range".into());
            }
        }
        if name == "s"
            || is("substitute", 2)
                && name != "se"
                && name != "set"
                && name != "sor"
                && name != "sort"
                && name != "so"
        {
            return self.ex_substitute(range, args);
        }
        if is("global", 1) && name != "go" && name != "goto" {
            return self.ex_global(range, bang, args);
        }
        if is("vglobal", 1) {
            return self.ex_global(range, true, args);
        }
        if is("delete", 1) {
            return self.ex_delete_yank(range, args, true);
        }
        if is("yank", 1) {
            return self.ex_delete_yank(range, args, false);
        }
        if is("move", 1) {
            return self.ex_move(range, args, false);
        }
        if name == "t" || is("copy", 2) {
            return self.ex_move(range, args, true);
        }
        if name.starts_with('>') || name.starts_with('<') {
            let (a, b) = self.lines_of(range);
            let times = name.len();
            let cur_col = col_of(self.rope.slice(..), self.vcur());
            let rev = self.revision;
            self.shift_lines(a, b, times, name.starts_with('>'));
            let rope = self.rp();
            let t = rope.slice(..);
            // the cursor ends on the last line: at its indent when something was shifted
            let pos = if self.revision != rev { first_nonblank(t, b) } else { pos_at_col(t, b, cur_col) };
            self.vset(pos);
            return Ok(());
        }
        if is("join", 1) {
            let (a, mut b) = self.lines_of(range);
            if range.n < 2 {
                b = (a + 1).min(self.last_lnum() as usize - 1);
            }
            self.join_lines_range(a, b, !bang);
            let rope = self.rp();
            let t = rope.slice(..);
            self.vset(first_nonblank(t, a.min(nlines(t) - 1)));
            return Ok(());
        }
        if is("put", 2) {
            return self.ex_put(range, bang, args);
        }
        if name == "k" || name == "mark" || name == "ma" {
            let (_, b) = self.lines_of(range);
            let rope = self.rp();
            let t = rope.slice(..);
            if let Some(c) = args.trim().chars().next() {
                self.vim.marks.insert(c, lstart(t, b));
            }
            return Ok(());
        }
        if is("nohlsearch", 3) {
            self.vim.hl_off = true;
            return Ok(());
        }
        if is("sort", 3) {
            return self.ex_sort(range, bang, args);
        }
        if is("normal", 4) {
            return self.ex_normal(range, args);
        }
        if is("registers", 3) || is("display", 2) {
            let s = self.registers_summary();
            self.status(s);
            return Ok(());
        }
        if is("marks", 5) {
            let s = self.marks_summary();
            self.status(s);
            return Ok(());
        }
        if name == "se" || is("set", 3) || is("setlocal", 4) {
            return self.ex_set(args);
        }
        if is("undo", 1) && !is("unmap", 3) {
            self.vim_undo(1, false);
            return Ok(());
        }
        if is("redo", 3) {
            self.vim_undo(1, true);
            return Ok(());
        }
        if is("help", 1) || is("helpgrep", 5) {
            self.effects.push(Effect::HostCommand(if args.is_empty() {
                name.to_string()
            } else {
                format!("{name} {args}")
            }));
            return Ok(());
        }
        if name == "&" || name == "&&" || name == "~" {
            return self.ex_substitute(range, &(if name == "&&" { "&&" } else { "" }).to_string());
        }
        if is("gotoline", 4) {
            return Ok(());
        }
        let full = if args.is_empty() { name.to_string() } else { format!("{name} {args}") };
        if bang && self.exec_host_command(&format!("{name}!"), args) {
            return Ok(());
        }
        if self.exec_host_command(name, args) {
            return Ok(());
        }
        // Vim's own spellings of the host commands
        match name {
            "wa" | "wall" => self.effects.push(Effect::Save),
            "wqa" | "wqall" | "xa" | "xall" => {
                self.effects.push(Effect::Save);
                self.effects.push(Effect::Quit);
            }
            "qall" | "quita" | "quitall" => self.effects.push(Effect::Quit),
            "tabnew" | "tabe" | "tabedit" | "enew" | "ene" => self.effects.push(Effect::NewTab),
            "tabn" | "tabnext" => self.effects.push(Effect::NextTab),
            "tabp" | "tabprevious" | "tabN" => self.effects.push(Effect::PreviousTab),
            "tabc" | "tabclose" | "close" | "clo" | "bd" | "bdelete" => self.effects.push(Effect::CloseTab),
            "up" | "update" | "sav" | "saveas" => self.effects.push(Effect::Save),
            "reg" => {}
            _ => self.effects.push(Effect::HostCommand(full)),
        }
        Ok(())
    }

    fn lines_of(&self, r: &Range) -> (usize, usize) {
        let cur = self.cur_lnum();
        let (a, b) = if r.n == 0 {
            (cur, cur)
        } else if r.n == 1 {
            (r.a, r.a)
        } else {
            (r.a.min(r.b), r.a.max(r.b))
        };
        (a.max(1) as usize - 1, b.max(1) as usize - 1)
    }

    fn lines_or_all(&self, r: &Range) -> (usize, usize) {
        if r.n == 0 { (0, self.last_lnum() as usize - 1) } else { self.lines_of(r) }
    }

    // ---- ranges ---------------------------------------------------------------------------------------

    fn parse_range<'a>(&mut self, s: &'a str) -> Res<(Range, &'a str)> {
        let mut s = skip_ws(s);
        let last = self.last_lnum();
        let mut cur = self.cur_lnum();
        if let Some(r) = s.strip_prefix('%') {
            return Ok((Range { a: 1, b: last, n: 2 }, r));
        }
        let mut addrs: Vec<isize> = Vec::new();
        loop {
            let (addr, rest) = self.parse_addr(s, cur, last)?;
            s = skip_ws(rest);
            match s.chars().next() {
                Some(c @ (',' | ';')) => {
                    addrs.push(addr.unwrap_or(cur));
                    if c == ';' {
                        cur = *addrs.last().unwrap();
                    }
                    s = &s[1..];
                    // `5,` means `5,.`
                    let t2 = skip_ws(s);
                    let (next, _) = self.parse_addr(t2, cur, last).unwrap_or((None, t2));
                    if next.is_none() {
                        addrs.push(cur);
                        break;
                    }
                }
                _ => {
                    if let Some(a) = addr {
                        addrs.push(a);
                    }
                    break;
                }
            }
        }
        let n = addrs.len().min(2);
        let (a, b) = match addrs.len() {
            0 => (cur, cur),
            1 => (addrs[0], addrs[0]),
            k => (addrs[k - 2], addrs[k - 1]),
        };
        let (a, b) = if n == 2 && a > b { (b, a) } else { (a, b) };
        Ok((Range { a, b, n }, s))
    }

    fn parse_addr<'a>(&mut self, s: &'a str, cur: isize, last: isize) -> Res<(Option<isize>, &'a str)> {
        let s = skip_ws(s);
        let mut chars = s.chars();
        let Some(c) = chars.next() else { return Ok((None, s)) };
        let mut rest = &s[c.len_utf8()..];
        let mut v: isize = match c {
            '0'..='9' => {
                let end = s.find(|ch: char| !ch.is_ascii_digit()).unwrap_or(s.len());
                rest = &s[end..];
                s[..end].parse().unwrap_or(0)
            }
            '.' => cur,
            '$' => last,
            '\'' => {
                let m = rest.chars().next().ok_or("E20: Mark not set")?;
                rest = &rest[m.len_utf8()..];
                let p = match m {
                    '<' | '>' | 'a'..='z' | 'A'..='Z' | '.' | '[' | ']' | '\'' | '`' | '^' => {
                        let key = if m == '`' { '\'' } else { m };
                        self.vim.marks.get(&key).copied()
                    }
                    _ => None,
                };
                let p = p.ok_or("E20: Mark not set")?;
                let t = self.rope.slice(..);
                line_of(t, p.min(t.len_chars())) as isize + 1
            }
            '/' | '?' => {
                let (pat, r) = take_delimited(rest, c);
                rest = r;
                let pat = if pat.is_empty() {
                    self.search.pattern.clone().ok_or("E35: No previous regular expression")?
                } else {
                    pat
                };
                self.search.pattern = Some(pat.clone());
                self.addr_search(&pat, c == '/', cur)?
            }
            '+' | '-' => {
                rest = s;
                cur
            }
            _ => return Ok((None, s)),
        };
        // offsets
        loop {
            let t = skip_ws(rest);
            let Some(sign @ ('+' | '-')) = t.chars().next() else { break };
            let body = &t[1..];
            let end = body.find(|ch: char| !ch.is_ascii_digit()).unwrap_or(body.len());
            let n: isize = if end == 0 { 1 } else { body[..end].parse().unwrap_or(1) };
            v += if sign == '+' { n } else { -n };
            rest = &body[end..];
        }
        Ok((Some(v), rest))
    }

    /// Next line (after/before `cur`, wrapping) that contains a match of `pat`.
    fn addr_search(&mut self, pat: &str, fwd: bool, cur: isize) -> Res<isize> {
        let re = self.vim_regex(pat)?;
        let rope = self.rp();
        let t = rope.slice(..);
        let n = nlines(t);
        for i in 1..=n {
            let l = if fwd { (cur as usize - 1 + i) % n } else { (cur as usize - 1 + n - (i % n)) % n };
            let s = t.slice(lstart(t, l)..lend(t, l)).to_string();
            if re.is_match(&s) {
                return Ok(l as isize + 1);
            }
        }
        Err(format!("E486: Pattern not found: {pat}"))
    }

    fn ex_goto(&mut self, r: &Range) -> Res<()> {
        if r.n == 0 {
            return Ok(());
        }
        let last = self.last_lnum();
        let l = r.b.clamp(1, last) as usize - 1;
        let rope = self.rp();
        let t = rope.slice(..);
        self.vim_push_jump();
        self.vset(first_nonblank(t, l));
        Ok(())
    }

    // ---- :d :y -----------------------------------------------------------------------------------------

    fn ex_delete_yank(&mut self, range: &Range, args: &str, delete: bool) -> Res<()> {
        let (mut a, mut b) = self.lines_of(range);
        let mut reg = None;
        let mut rest = args.trim();
        if let Some(c) = rest.chars().next() {
            if !c.is_ascii_digit() {
                reg = Some(c);
                rest = rest[c.len_utf8()..].trim();
            }
        }
        if let Ok(n) = rest.parse::<usize>() {
            a = b;
            b = (a + n - 1).min(self.last_lnum() as usize - 1);
        }
        let rope = self.rp();
        let t = rope.slice(..);
        let cur = self.vcur();
        let r = region_lines(t, a, b, cur);
        self.operate(if delete { Op::Delete } else { Op::Yank }, r, reg, false);
        if !delete {
            self.vset(cur);
        }
        Ok(())
    }

    // ---- :m :t ------------------------------------------------------------------------------------------

    fn ex_move(&mut self, range: &Range, args: &str, copy: bool) -> Res<()> {
        let (a, b) = self.lines_of(range);
        let cur = self.cur_lnum();
        let last = self.last_lnum();
        let (addr, _) = self.parse_addr(args, cur, last)?;
        let dest = addr.ok_or("E14: Invalid address")?;
        if dest < 0 || dest > last {
            return Err("E16: Invalid range".into());
        }
        let dest = dest as usize; // insert after this 1-based line (0 = top)
        let rope = self.rp();
        let t = rope.slice(..);
        let le = self.line_ending;
        let nl = nlines(t);
        let mut body = String::new();
        for l in a..=b {
            body.push_str(&t.slice(lstart(t, l)..lend(t, l)).to_string());
            body.push('\n');
        }
        let body = body.replace('\n', le);
        if !copy {
            if dest > a && dest <= b {
                return if dest == b + 1 - 1 + 0 && false {
                    Ok(())
                } else {
                    Err("E134: Cannot move a range of lines into itself".into())
                };
            }
            if dest == a || dest == b + 1 {
                // moving onto itself: nothing changes
                let l = if dest == a { b } else { b };
                let _ = l;
                self.vset(first_nonblank(t, b));
                return Ok(());
            }
        }
        let ins = |at_line: Option<usize>, body: &str| -> Change {
            match at_line {
                None => Change { from: 0, to: 0, text: body.to_string() },
                Some(l) => {
                    if l + 1 == nl && !has_term(t, l) {
                        let b2 = body.strip_suffix(le).unwrap_or(body);
                        Change { from: t.len_chars(), to: t.len_chars(), text: format!("{le}{b2}") }
                    } else {
                        Change { from: lnext(t, l), to: lnext(t, l), text: body.to_string() }
                    }
                }
            }
        };
        let dest_line = if dest == 0 { None } else { Some(dest - 1) };
        let mut changes = vec![ins(dest_line, &body)];
        if !copy {
            changes.push(line_delete_change(t, a, b));
        }
        self.vapply(changes);
        let rope = self.rp();
        let t2 = rope.slice(..);
        let count = b - a + 1;
        // the cursor lands on the last moved / copied line
        let first_new = if copy {
            if dest <= b { dest } else { dest }
        } else if dest > b {
            dest - count
        } else {
            dest
        };
        let last_new = (first_new + count - 1).min(nlines(t2) - 1);
        self.vset(first_nonblank(t2, last_new));
        Ok(())
    }

    fn ex_put(&mut self, range: &Range, bang: bool, args: &str) -> Res<()> {
        let reg = args.trim().chars().next();
        let (_, b) = self.lines_of(range);
        let rope = self.rp();
        let t = rope.slice(..);
        let Some(r) = self.reg_get(reg) else {
            // the cursor already moved to the target line (same column)
            let col = col_of(t, self.vcur());
            self.vset(pos_at_col(t, if range.n > 0 && range.b == 0 { 0 } else { b }, col));
            return Err(format!("E353: Nothing in register {}", reg.unwrap_or('"')));
        };
        let target = if range.n > 0 && range.b == 0 { None } else { Some(b) };
        let mut text = r.text.clone();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        let body = self.nl_str(&text);
        let nl = nlines(t);
        let change = match (target, bang) {
            (None, _) | (_, true) => {
                let l = target.unwrap_or(0);
                Change { from: lstart(t, l), to: lstart(t, l), text: body.clone() }
            }
            (Some(l), false) => {
                if l + 1 == nl && !has_term(t, l) {
                    let le = self.line_ending;
                    Change {
                        from: t.len_chars(),
                        to: t.len_chars(),
                        text: format!("{le}{}", body.strip_suffix(le).unwrap_or(&body)),
                    }
                } else {
                    Change { from: lnext(t, l), to: lnext(t, l), text: body.clone() }
                }
            }
        };
        let at = change.from;
        self.vapply(vec![change]);
        let rope = self.rp();
        let t2 = rope.slice(..);
        let nput = text.matches('\n').count();
        let first = line_of(t2, at.min(t2.len_chars()))
            + usize::from(at == t.len_chars() && !has_term(t, nl - 1) && nl > 0 && at > 0 && !bang);
        let last = (first + nput - 1).min(nlines(t2) - 1);
        self.vset(first_nonblank(t2, last));
        Ok(())
    }

    // ---- :sort ----------------------------------------------------------------------------------------------

    fn ex_sort(&mut self, range: &Range, reverse: bool, args: &str) -> Res<()> {
        let (a, b) = self.lines_or_all(range);
        if a == b {
            return Ok(());
        }
        let mut unique = false;
        let mut numeric = false;
        let mut use_match = false;
        let mut icase = false;
        let mut pat: Option<String> = None;
        let mut s = args.trim();
        while let Some(c) = s.chars().next() {
            match c {
                'u' => unique = true,
                'n' => numeric = true,
                'r' => use_match = true,
                'i' => icase = true,
                ' ' => {}
                '/' => {
                    let (p, r) = take_delimited(&s[1..], '/');
                    pat = Some(p);
                    s = r;
                    continue;
                }
                _ => return Err(format!("E474: Invalid argument: {s}")),
            }
            s = &s[c.len_utf8()..];
        }
        let re = match &pat {
            Some(p) if !p.is_empty() => Some(self.vim_regex(p)?),
            Some(_) => match self.search.pattern.clone() {
                Some(p) => Some(self.vim_regex(&p)?),
                None => None,
            },
            None => None,
        };
        let rope = self.rp();
        let t = rope.slice(..);
        let lines: Vec<String> = (a..=b).map(|l| t.slice(lstart(t, l)..lend(t, l)).to_string()).collect();
        let key_text = |l: &str| -> String {
            match &re {
                Some(re) => match re.find(l) {
                    Some(m) => {
                        if use_match {
                            l[m.start()..m.end()].to_string()
                        } else {
                            l[m.end()..].to_string()
                        }
                    }
                    None => {
                        if use_match {
                            String::new()
                        } else {
                            l.to_string()
                        }
                    }
                },
                None => l.to_string(),
            }
        };
        let mut keyed: Vec<(String, Option<i128>, String)> = lines
            .into_iter()
            .map(|l| {
                let k = key_text(&l);
                let num = if numeric { first_number(&k) } else { None };
                let k = if icase { k.to_lowercase() } else { k };
                (k, num, l)
            })
            .collect();
        keyed.sort_by(|x, y| if numeric { x.1.cmp(&y.1) } else { x.0.cmp(&y.0) });
        if reverse {
            keyed.reverse();
        }
        if unique {
            keyed.dedup_by(|y, x| if numeric { x.1 == y.1 } else { x.0 == y.0 });
        }
        let le = self.line_ending;
        let new = keyed.into_iter().map(|k| k.2).collect::<Vec<_>>().join(le);
        let from = lstart(t, a);
        let to = lend(t, b);
        self.vapply(vec![Change { from, to, text: new }]);
        let rope = self.rp();
        let t2 = rope.slice(..);
        self.vset(first_nonblank(t2, a));
        Ok(())
    }

    // ---- :normal --------------------------------------------------------------------------------------------

    fn ex_normal(&mut self, range: &Range, args: &str) -> Res<()> {
        let keys: Vec<KeyInput> = args.chars().map(KeyInput::ch).collect();
        let mut run = |ed: &mut Editor| {
            ed.vim.replaying_macro += 1;
            ed.vim_feed_keys(&keys);
            // an incomplete command is aborted, like Escape
            if ed.mode != Mode::Normal || !ed.vim.pend.idle() || ed.vim.one_shot > 0 {
                let esc = KeyInput::new(crate::keys::Key::Escape);
                for _ in 0..3 {
                    ed.vim_feed(&esc);
                }
            }
            ed.vim.replaying_macro -= 1;
        };
        if range.n == 0 {
            run(self);
            return Ok(());
        }
        let (a, b) = self.lines_of(range);
        let mut l = a;
        // like Vim: walk the original line numbers; once past the end the last line is used
        while l <= b {
            let rope = self.rp();
            let t = rope.slice(..);
            self.vset(lstart(t, l.min(nlines(t) - 1)));
            run(self);
            l += 1;
        }
        Ok(())
    }

    // ---- :set ------------------------------------------------------------------------------------------------------

    fn ex_set(&mut self, args: &str) -> Res<()> {
        if args.trim().is_empty() {
            return Ok(());
        }
        for opt in args.split_whitespace() {
            let (name, query, inv) = match opt.strip_suffix('?') {
                Some(n) => (n, true, false),
                None => match opt.strip_suffix('!') {
                    Some(n) => (n, false, true),
                    None => (opt, false, false),
                },
            };
            let (base, val) = match name.strip_prefix("no") {
                Some(b) if matches!(b, "ic" | "ignorecase" | "scs" | "smartcase" | "hls" | "hlsearch") => {
                    (b, false)
                }
                _ => match name.strip_prefix("inv") {
                    Some(b) => (b, !self.opt_value(b).unwrap_or(false)),
                    None => (name, true),
                },
            };
            let val = if inv { !self.opt_value(base).unwrap_or(false) } else { val };
            match base {
                "ic" | "ignorecase" => {
                    if query {
                        let v = self.vim.opts.ignorecase;
                        self.status(if v { "  ignorecase" } else { "noignorecase" });
                    } else {
                        self.vim.opts.ignorecase = val;
                    }
                }
                "scs" | "smartcase" => {
                    if query {
                        let v = self.vim.opts.smartcase;
                        self.status(if v { "  smartcase" } else { "nosmartcase" });
                    } else {
                        self.vim.opts.smartcase = val;
                    }
                }
                "hls" | "hlsearch" => {
                    if query {
                        let v = self.vim.opts.hlsearch;
                        self.status(if v { "  hlsearch" } else { "nohlsearch" });
                    } else {
                        self.vim.opts.hlsearch = val;
                        if val {
                            self.vim.hl_off = false;
                        }
                    }
                }
                _ => {
                    self.effects.push(Effect::HostCommand(format!("set {args}")));
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn opt_value(&self, name: &str) -> Option<bool> {
        match name {
            "ic" | "ignorecase" => Some(self.vim.opts.ignorecase),
            "scs" | "smartcase" => Some(self.vim.opts.smartcase),
            "hls" | "hlsearch" => Some(self.vim.opts.hlsearch),
            _ => None,
        }
    }

    fn registers_summary(&self) -> String {
        let mut names: Vec<char> = self.vim.regs.keys().copied().collect();
        names.sort_by_key(|c| match c {
            '"' => (0, *c),
            '0'..='9' => (1, *c),
            'a'..='z' => (2, *c),
            _ => (3, *c),
        });
        let mut out = String::from("Registers: ");
        let parts: Vec<String> = names
            .iter()
            .map(|c| {
                let r = &self.vim.regs[c];
                let tag = match r.kind {
                    RegKind::Char => 'c',
                    RegKind::Line => 'l',
                    RegKind::Block(_) => 'b',
                };
                let mut t: String = r.text.chars().map(|x| if x == '\n' { '\u{23ce}' } else { x }).collect();
                if t.chars().count() > 30 {
                    t = t.chars().take(30).collect::<String>() + "…";
                }
                format!("\"{c} {tag} {t}")
            })
            .collect();
        out.push_str(&parts.join(" | "));
        out
    }

    fn marks_summary(&self) -> String {
        let t = self.rope.slice(..);
        let mut names: Vec<char> = self.vim.marks.keys().copied().collect();
        names.sort();
        let parts: Vec<String> = names
            .iter()
            .map(|c| {
                let p = self.vim.marks[c].min(t.len_chars());
                format!("{c} {}:{}", line_of(t, p) + 1, col_of(t, p) + 1)
            })
            .collect();
        format!("marks: {}", parts.join(" | "))
    }

    // ---- :global ---------------------------------------------------------------------------------------------------

    fn ex_global(&mut self, range: &Range, invert: bool, args: &str) -> Res<()> {
        let mut chars = args.chars();
        let delim = chars.next().ok_or("E35: No previous regular expression")?;
        if delim.is_alphanumeric() || delim == '\\' || delim == '"' || delim == '|' {
            return Err("E146: Regular expressions can't be delimited by letters".into());
        }
        let (pat, cmd) = take_delimited(&args[delim.len_utf8()..], delim);
        let pat = if pat.is_empty() {
            self.search.pattern.clone().ok_or("E35: No previous regular expression")?
        } else {
            pat
        };
        self.search.pattern = Some(pat.clone());
        let re = self.vim_regex(&pat)?;
        let cmd = if cmd.trim().is_empty() { "p".to_string() } else { cmd.trim().to_string() };
        if self.vim.in_global {
            return Err("E147: Cannot do :global recursive".into());
        }
        let (a, b) = self.lines_or_all(range);
        let rope = self.rp();
        let t = rope.slice(..);
        let mut hits = Vec::new();
        for l in a..=b {
            let s = t.slice(lstart(t, l)..lend(t, l)).to_string();
            if re.is_match(&s) != invert {
                hits.push(lstart(t, l));
            }
        }
        if hits.is_empty() {
            return Err(if invert {
                format!("E538: Pattern found in every line: {pat}")
            } else {
                format!("E486: Pattern not found: {pat}")
            });
        }
        self.vim.in_global = true;
        let saved = std::mem::replace(&mut self.vim.tracked, hits);
        let n = self.vim.tracked.len();
        for i in 0..n {
            let p = self.vim.tracked[i];
            if p == usize::MAX {
                continue;
            }
            let rope = self.rp();
            let t = rope.slice(..);
            if p > t.len_chars() {
                continue;
            }
            self.vset(lstart(t, line_of(t, p)));
            self.vim_ex_inner(&cmd);
        }
        self.vim.tracked = saved;
        self.vim.in_global = false;
        Ok(())
    }

    fn vim_ex_inner(&mut self, cmd: &str) {
        if let Err(e) = self.ex_run(cmd.trim_start_matches([':', ' '])) {
            self.status(e);
        }
    }

    // ---- :substitute ------------------------------------------------------------------------------------------------

    fn ex_substitute(&mut self, range: &Range, args: &str) -> Res<()> {
        let (mut pat, mut rep, mut flags, mut rest): (String, String, String, String);
        let a0 = args;
        let first = a0.chars().next();
        match first {
            Some(d) if !d.is_alphanumeric() && d != '\\' && d != '"' && d != '|' && d != ' ' && d != '&' => {
                let (p, r1) = take_delimited(&a0[d.len_utf8()..], d);
                let (rp, r2) = take_delimited_keep(r1, d);
                pat = p;
                rep = rp;
                let tail = r2.trim_start();
                let fl_end = tail.find(|c: char| !"&cegiInp#lr".contains(c)).unwrap_or(tail.len());
                flags = tail[..fl_end].to_string();
                rest = tail[fl_end..].trim().to_string();
                if pat.is_empty() {
                    pat = self.search.pattern.clone().ok_or("E35: No previous regular expression")?;
                }
                self.vim.last_sub =
                    Some(SubSpec { pat: pat.clone(), rep: rep.clone(), flags: flags.clone() });
                self.search.pattern = Some(pat.clone());
            }
            _ => {
                // `:s`, `:&`, `:s g` repeat the last substitute
                let last = self.vim.last_sub.clone().ok_or("E35: No previous regular expression")?;
                pat = last.pat;
                rep = last.rep;
                let tail = a0.trim_start();
                let fl_end = tail.find(|c: char| !"&cegiInp#lr".contains(c)).unwrap_or(tail.len());
                flags = tail[..fl_end].to_string();
                if flags.starts_with('&') {
                    flags = format!("{}{}", last.flags, &flags[1..]);
                }
                rest = tail[fl_end..].trim().to_string();
            }
        }
        if flags.contains('c') {
            self.status("confirm flag is not supported: substituting without asking");
        }
        let (mut a, mut b) = self.lines_of(range);
        if let Ok(n) = rest.parse::<usize>() {
            a = b;
            b = (a + n - 1).min(self.last_lnum() as usize - 1);
        }
        let global = flags.contains('g');
        let count_only = flags.contains('n');
        let ic = if flags.contains('I') {
            Some(false)
        } else if flags.contains('i') {
            Some(true)
        } else {
            None
        };
        let (rs, force) = vregex::translate(&pat)?;
        let icase = match (ic, force) {
            (Some(v), _) => v,
            (None, Some(v)) => v,
            _ => self.vim.opts.ignorecase && !(self.vim.opts.smartcase && pat_has_upper(&pat)),
        };
        let re: Regex = vregex::build(&rs, icase)?;
        let parts = vregex::parse_replacement(&rep);
        let rope = self.rp();
        let t = rope.slice(..);
        let le = self.line_ending;
        // work on the text of the whole range (terminator of the last line excluded)
        let from = lstart(t, a);
        let to = lend(t, b);
        let hay = t.slice(from..to).to_string();
        let mut out = String::new();
        let mut last = 0usize;
        let mut subs = 0usize;
        let mut last_line_hit: Option<usize> = None;
        let mut last_joined = false;
        let mut last_repl_end = 0usize;
        let mut pos = 0usize;
        let multiline = hay.contains('\n') && re.as_str().contains(r"\n");
        let line_of_pos = |p: usize| hay[..p].matches('\n').count();
        let mut done_line: Option<usize> = None;
        while pos <= hay.len() {
            let Some(caps) = re.captures_at(&hay, pos) else { break };
            let m = caps.get(0).unwrap();
            let ln = line_of_pos(m.start());
            if !global {
                if let Some(d) = done_line {
                    if ln <= d {
                        // already substituted on this line: skip to the next line
                        match hay[m.start()..].find('\n') {
                            Some(i) => {
                                pos = m.start() + i + 1;
                                continue;
                            }
                            None => break,
                        }
                    }
                }
            }
            subs += 1;
            out.push_str(&hay[last..m.start()]);
            if !count_only {
                out.push_str(&vregex::expand(&parts, &caps));
            } else {
                out.push_str(m.as_str());
            }
            last = m.end();
            let end_ln = line_of_pos(m.end());
            let ends_at_line_start = m.end() > 0 && hay.as_bytes()[m.end() - 1] == b'\n';
            last_line_hit = Some(out.matches('\n').count());
            last_joined = m.as_str().contains('\n');
            last_repl_end = out.chars().count();
            // a match that ran into a later line uses that line up; one that only swallowed a
            // line break leaves the next line to be processed
            done_line = Some(if end_ln > ln && !ends_at_line_start { end_ln } else { ln });
            if m.end() == m.start() {
                // an empty match: step over one char so the loop makes progress
                let step = hay[m.end()..].chars().next().map_or(1, |c| c.len_utf8());
                if m.end() >= hay.len() {
                    break;
                }
                out.push_str(&hay[m.end()..m.end() + step]);
                last = m.end() + step;
                pos = last;
            } else {
                pos = m.end();
            }
            let _ = multiline;
        }
        if subs == 0 {
            if flags.contains('e') {
                return Ok(());
            }
            return Err(format!("E486: Pattern not found: {pat}"));
        }
        if count_only {
            self.status(format!("{subs} match{} on lines", if subs == 1 { "" } else { "es" }));
            return Ok(());
        }
        out.push_str(&hay[last..]);
        let new = out.replace('\0', "\n").replace('\n', "\n");
        let new = if le == "\r\n" { new.replace("\r\n", "\n").replace('\n', "\r\n") } else { new };
        if new != hay {
            self.vapply(vec![Change { from, to, text: new.clone() }]);
        }
        let _ = (&mut rep, &mut flags, &mut pat, last_line_hit);
        // the cursor goes to the last line where something was substituted; after a match that
        // joined lines it stays at the join
        let rope = self.rp();
        let t2 = rope.slice(..);
        let end_pos = (from + last_repl_end).min(t2.len_chars());
        if last_joined {
            self.vset(end_pos);
        } else {
            let l = line_of(t2, end_pos);
            self.vset(first_nonblank(t2, l));
        }
        Ok(())
    }
}

/// Index of the first `|` that is not escaped with a backslash.
fn find_bar(s: &str) -> Option<usize> {
    let mut it = s.char_indices();
    while let Some((i, c)) = it.next() {
        if c == '\\' {
            it.next();
        } else if c == '|' {
            return Some(i);
        }
    }
    None
}

fn pat_has_upper(p: &str) -> bool {
    let mut it = p.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            it.next();
        } else if c.is_uppercase() {
            return true;
        }
    }
    false
}

fn first_number(s: &str) -> Option<i128> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && !b[i].is_ascii_digit() {
        i += 1;
    }
    if i >= b.len() {
        return None;
    }
    let neg = i > 0 && b[i - 1] == b'-';
    let mut j = i;
    while j < b.len() && b[j].is_ascii_digit() {
        j += 1;
    }
    let v: i128 = s[i..j].parse().ok()?;
    Some(if neg { -v } else { v })
}

/// Text up to the next unescaped `delim` (a `\<delim>` stands for the delimiter itself, other
/// escapes are kept) and the rest after it.
fn take_delimited(s: &str, delim: char) -> (String, &str) {
    let mut out = String::new();
    let mut it = s.char_indices();
    while let Some((i, c)) = it.next() {
        if c == '\\' {
            match it.next() {
                Some((_, d)) if d == delim => out.push(d),
                Some((_, d)) => {
                    out.push('\\');
                    out.push(d);
                }
                None => out.push('\\'),
            }
        } else if c == delim {
            return (out, &s[i + c.len_utf8()..]);
        } else {
            out.push(c);
        }
    }
    (out, "")
}

/// Like `take_delimited` for the replacement part: the closing delimiter is optional.
fn take_delimited_keep(s: &str, delim: char) -> (String, &str) {
    take_delimited(s, delim)
}
