//! A line diff: what separates two versions of a text, as a list of lines each marked kept, removed or added.
//!
//! This is the longest-common-subsequence diff, with the lines both texts share at the start and the end
//! peeled off first (the usual case for an edited console: the middle differs, the rest does not) so the
//! quadratic table only covers what changed. A table over `MAX_CELLS` cells is not built; the middle is then
//! reported as wholly removed and wholly added, which is a correct if coarse diff, and `Diff::coarse` says so.

/// What happened to a line going from the old text to the new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Same,
    Removed,
    Added,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub op: Op,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Diff {
    pub lines: Vec<DiffLine>,
    /// The changed middle was too large for an exact diff and is shown as removed-then-added.
    pub coarse: bool,
}

impl Diff {
    pub fn added(&self) -> usize {
        self.lines.iter().filter(|l| l.op == Op::Added).count()
    }

    pub fn removed(&self) -> usize {
        self.lines.iter().filter(|l| l.op == Op::Removed).count()
    }

    pub fn is_identical(&self) -> bool {
        self.lines.iter().all(|l| l.op == Op::Same)
    }
}

/// Cells of the LCS table (old lines times new lines) beyond which the exact diff is not attempted: 4 million
/// cells of `u32` is 16 MB, and a few hundred milliseconds.
pub const MAX_CELLS: usize = 4_000_000;

/// Split into lines the way a person counts them: `"a\nb"` and `"a\nb\n"` are both two lines, `""` is none.
pub fn lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    text.strip_suffix('\n').unwrap_or(text).split('\n').collect()
}

/// The diff from `old` to `new`.
pub fn line_diff(old: &str, new: &str) -> Diff {
    let (a, b) = (lines(old), lines(new));
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf] {
        suf += 1;
    }
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let mut out: Vec<DiffLine> =
        a[..pre].iter().map(|l| DiffLine { op: Op::Same, text: (*l).to_string() }).collect();
    let mut coarse = false;
    if am.is_empty() || bm.is_empty() || am.len().saturating_mul(bm.len()) <= MAX_CELLS {
        out.extend(lcs_middle(am, bm));
    } else {
        coarse = true;
        out.extend(am.iter().map(|l| DiffLine { op: Op::Removed, text: (*l).to_string() }));
        out.extend(bm.iter().map(|l| DiffLine { op: Op::Added, text: (*l).to_string() }));
    }
    out.extend(a[a.len() - suf..].iter().map(|l| DiffLine { op: Op::Same, text: (*l).to_string() }));
    Diff { lines: out, coarse }
}

fn lcs_middle(a: &[&str], b: &[&str]) -> Vec<DiffLine> {
    let (n, m) = (a.len(), b.len());
    // table[i][j] = length of the LCS of a[i..] and b[j..].
    let mut table = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[at(i, j)] = if a[i] == b[j] {
                table[at(i + 1, j + 1)] + 1
            } else {
                table[at(i + 1, j)].max(table[at(i, j + 1)])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(n + m);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(DiffLine { op: Op::Same, text: a[i].to_string() });
            i += 1;
            j += 1;
        // On a tie prefer removing first, so a replaced line reads as "- old, + new".
        } else if table[at(i + 1, j)] >= table[at(i, j + 1)] {
            out.push(DiffLine { op: Op::Removed, text: a[i].to_string() });
            i += 1;
        } else {
            out.push(DiffLine { op: Op::Added, text: b[j].to_string() });
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| DiffLine { op: Op::Removed, text: (*l).to_string() }));
    out.extend(b[j..].iter().map(|l| DiffLine { op: Op::Added, text: (*l).to_string() }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(d: &Diff) -> String {
        d.lines
            .iter()
            .map(|l| {
                format!(
                    "{}{}",
                    match l.op {
                        Op::Same => ' ',
                        Op::Removed => '-',
                        Op::Added => '+',
                    },
                    l.text
                )
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    #[test]
    fn identical_texts_have_no_changes() {
        let d = line_diff("select 1;\nselect 2;\n", "select 1;\nselect 2;\n");
        assert!(d.is_identical());
        assert_eq!(shape(&d), " select 1;| select 2;");
        assert!(line_diff("", "").lines.is_empty());
    }

    #[test]
    fn a_replaced_line_is_removed_then_added() {
        let d = line_diff("a\nb\nc", "a\nB\nc");
        assert_eq!(shape(&d), " a|-b|+B| c");
        assert_eq!((d.removed(), d.added()), (1, 1));
    }

    #[test]
    fn inserted_and_deleted_lines() {
        assert_eq!(shape(&line_diff("a\nc", "a\nb\nc")), " a|+b| c");
        assert_eq!(shape(&line_diff("a\nb\nc", "a\nc")), " a|-b| c");
        assert_eq!(shape(&line_diff("", "x\ny")), "+x|+y");
        assert_eq!(shape(&line_diff("x\ny", "")), "-x|-y");
    }

    #[test]
    fn a_trailing_newline_does_not_count_as_a_line() {
        assert!(line_diff("a\nb", "a\nb\n").is_identical());
        assert_eq!(lines("a\n\nb\n"), ["a", "", "b"], "a blank line in the middle is a line");
        assert_eq!(lines("\n"), [""], "a text that is one newline is one empty line");
    }

    #[test]
    fn the_diff_reproduces_both_texts() {
        // Rebuild old from Same+Removed and new from Same+Added, over a pseudo-random pair of texts.
        let mut seed = 12345u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize
        };
        for _ in 0..200 {
            let mk = |n: usize, next: &mut dyn FnMut() -> usize| {
                (0..n).map(|_| format!("line {}", next() % 6)).collect::<Vec<_>>().join("\n")
            };
            let (la, lb) = (next() % 12, next() % 12);
            let (a, b) = (mk(la, &mut next), mk(lb, &mut next));
            let d = line_diff(&a, &b);
            let old: Vec<&str> =
                d.lines.iter().filter(|l| l.op != Op::Added).map(|l| l.text.as_str()).collect();
            let new: Vec<&str> =
                d.lines.iter().filter(|l| l.op != Op::Removed).map(|l| l.text.as_str()).collect();
            assert_eq!(old, lines(&a), "old side of {a:?} -> {b:?}");
            assert_eq!(new, lines(&b), "new side of {a:?} -> {b:?}");
        }
    }

    #[test]
    fn the_diff_is_minimal_on_a_known_case() {
        // LCS of (a b c d e f) and (a c b d f e) has length 4 (a, b/c, d, e/f): 2 removed, 2 added.
        let d = line_diff("a\nb\nc\nd\ne\nf", "a\nc\nb\nd\nf\ne");
        assert_eq!((d.removed(), d.added()), (2, 2), "{}", shape(&d));
    }

    #[test]
    fn a_huge_changed_middle_is_reported_coarsely_instead_of_allocating_a_huge_table() {
        let a: String = (0..3000).map(|i| format!("old {i}\n")).collect();
        let b: String = (0..3000).map(|i| format!("new {i}\n")).collect();
        let d = line_diff(&a, &b);
        assert!(d.coarse);
        assert_eq!((d.removed(), d.added()), (3000, 3000));
        // The shared head and tail are still kept even then.
        let a2 = format!("same\n{a}end\n");
        let b2 = format!("same\n{b}end\n");
        let d2 = line_diff(&a2, &b2);
        assert!(
            d2.coarse && d2.lines.first().unwrap().op == Op::Same && d2.lines.last().unwrap().op == Op::Same
        );
        // Below the limit it is exact.
        assert!(!line_diff("a\nb", "b\na").coarse);
    }

    #[test]
    fn a_large_change_in_a_large_file_stays_fast_because_the_ends_are_trimmed() {
        let head: String = (0..50_000).map(|i| format!("row {i}\n")).collect();
        let a = format!("{head}old line\n{head}");
        let b = format!("{head}new line\n{head}");
        let d = line_diff(&a, &b);
        assert!(!d.coarse);
        assert_eq!((d.removed(), d.added()), (1, 1));
        assert_eq!(d.lines.len(), 100_002);
    }
}
