//! Whether a graph folder's `.gitignore` hides every temp and marker file fs.graph-files can make, by git's rules:
//! lines are globs, `!` un-hides, and the last line that matches a name wins. Exact over the whole set of names
//! (`.umx-tmp-set.json` and `.umx-tmp-<counter>-<file>` for every counter and file), not a sample of them
//! (review): the names are a small automaton, each line a glob automaton, and their product is walked once.
//!
//! `.gitignore` arrives by git pull, so the cost is bounded (security): a line longer than [`MAX_LINE`], more
//! than [`MAX_LINES`] lines that can match a temp name, or more than [`MAX_STATES`] product states each count as
//! "not hidden". A warning is cheap; a slow `umx check` is not.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec;
use alloc::vec::Vec;

const MAX_LINE: usize = 256;
const MAX_LINES: usize = 64;
const MAX_STATES: usize = 50_000;

/// The names fs.graph-files makes in a graph folder, as a deterministic automaton over bytes: the marker
/// `.umx-tmp-set.json`, and `.umx-tmp-<n>-<file>` where `<n>` is a counter in decimal (no leading zero) and
/// `<file>` a graph file name or `set.json` (the marker's own temp).
struct Names {
    next: Vec<BTreeMap<u8, usize>>,
    accept: Vec<bool>,
}

impl Names {
    fn new() -> Names {
        let mut n = Names {
            next: vec![BTreeMap::new()],
            accept: vec![false],
        };
        let prefix = n.word(0, b".umx-tmp-");
        let marker = n.word(prefix, b"set.json");
        n.accept[marker] = true;
        let (zero, digits, dash) = (n.state(), n.state(), n.state());
        n.next[prefix].insert(b'0', zero);
        for d in b'1'..=b'9' {
            n.next[prefix].insert(d, digits);
        }
        for d in b'0'..=b'9' {
            n.next[digits].insert(d, digits);
        }
        n.next[zero].insert(b'-', dash);
        n.next[digits].insert(b'-', dash);
        for file in [
            &b"graph.json"[..],
            b"nodes.jsonl",
            b"edges.jsonl",
            b".gitignore",
            b"set.json",
        ] {
            let end = n.word(dash, file);
            n.accept[end] = true;
        }
        n
    }

    fn state(&mut self) -> usize {
        self.next.push(BTreeMap::new());
        self.accept.push(false);
        self.next.len() - 1
    }

    fn word(&mut self, mut at: usize, word: &[u8]) -> usize {
        for &b in word {
            at = match self.next[at].get(&b) {
                Some(&s) => s,
                None => {
                    let s = self.state();
                    self.next[at].insert(b, s);
                    s
                }
            };
        }
        at
    }
}

#[derive(Clone, Debug)]
enum Tok {
    Star,
    One,
    /// A byte as written: under core.ignorecase it matches either case.
    Byte(u8),
    /// A byte after `\`: git compares it as written even under core.ignorecase.
    Escaped(u8),
    /// A `[...]` expression: the bytes it matches, without and with core.ignorecase.
    Class(Box<[Set; 2]>),
}

/// A set of bytes, one bit each.
type Set = [u64; 4];

fn set_has(set: &Set, b: u8) -> bool {
    set[usize::from(b / 64)] & (1 << (b % 64)) != 0
}

struct Line {
    negated: bool,
    toks: Vec<Tok>,
}

/// One part of a `[...]` expression.
#[derive(Clone, Copy)]
enum Item {
    Byte(u8),
    /// `lo-hi`, both ends included (empty if `lo > hi`).
    Range(u8, u8),
    /// `[:name:]`, a POSIX class in the C locale.
    Named(Named),
}

#[derive(Clone, Copy)]
enum Named {
    Alnum,
    Alpha,
    Blank,
    Cntrl,
    Digit,
    Graph,
    Lower,
    Print,
    Punct,
    Space,
    Upper,
    Xdigit,
}

impl Item {
    /// Whether text byte `t` matches, as wildmatch decides it (`fold`: core.ignorecase, under which git lowers
    /// the text byte, lets a range match an uppercase form, and lets `[:upper:]` match lowercase).
    fn matches(self, t: u8, fold: bool) -> bool {
        let t = if fold { t.to_ascii_lowercase() } else { t };
        match self {
            Item::Byte(c) => t == c,
            Item::Range(lo, hi) => {
                (lo..=hi).contains(&t)
                    || (fold
                        && t.is_ascii_lowercase()
                        && (lo..=hi).contains(&t.to_ascii_uppercase()))
            }
            Item::Named(n) => match n {
                Named::Alnum => t.is_ascii_alphanumeric(),
                Named::Alpha => t.is_ascii_alphabetic(),
                Named::Blank => t == b' ' || t == b'\t',
                Named::Cntrl => t.is_ascii_control(),
                Named::Digit => t.is_ascii_digit(),
                Named::Graph => t.is_ascii_graphic(),
                Named::Lower => t.is_ascii_lowercase(),
                Named::Print => t.is_ascii_graphic() || t == b' ',
                Named::Punct => t.is_ascii_punctuation(),
                Named::Space => matches!(t, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'),
                Named::Upper => t.is_ascii_uppercase() || (fold && t.is_ascii_lowercase()),
                Named::Xdigit => t.is_ascii_hexdigit(),
            },
        }
    }
}

fn named(name: &[u8]) -> Option<Named> {
    Some(match name {
        b"alnum" => Named::Alnum,
        b"alpha" => Named::Alpha,
        b"blank" => Named::Blank,
        b"cntrl" => Named::Cntrl,
        b"digit" => Named::Digit,
        b"graph" => Named::Graph,
        b"lower" => Named::Lower,
        b"print" => Named::Print,
        b"punct" => Named::Punct,
        b"space" => Named::Space,
        b"upper" => Named::Upper,
        b"xdigit" => Named::Xdigit,
        _ => return None,
    })
}

/// The `[` at `p[at]` read as git's wildmatch reads it, step for step (wildmatch.c, `case '['`): `!` or `^` negates,
/// a first `]` is itself, `\\` escapes, `a-z` is a range (a `-` first, last or right after a range or class is
/// itself), `[:name:]` is a POSIX class, and `[:` with no `:]` before the next `]` is a `[`. Returns whether it is
/// negated, its parts and the index after its `]`; None where git aborts the whole match (no closing `]`, an unknown
/// class name, a `\\` at the end), so the line matches nothing.
fn bracket(p: &[u8], at: usize) -> Option<(bool, Vec<Item>, usize)> {
    let get = |i: usize| p.get(i).copied().unwrap_or(0);
    let mut i = at + 1;
    let mut pc = get(i);
    let negated = matches!(pc, b'!' | b'^');
    if negated {
        i += 1;
        pc = get(i);
    }
    let (mut prev, mut items) = (0u8, Vec::new());
    loop {
        if pc == 0 {
            return None;
        }
        if pc == b'\\' {
            i += 1;
            pc = get(i);
            if pc == 0 {
                return None;
            }
            items.push(Item::Byte(pc));
        } else if pc == b'-' && prev != 0 && get(i + 1) != 0 && get(i + 1) != b']' {
            i += 1;
            pc = get(i);
            if pc == b'\\' {
                i += 1;
                pc = get(i);
                if pc == 0 {
                    return None;
                }
            }
            items.push(Item::Range(prev, pc));
            pc = 0;
        } else if pc == b'[' && get(i + 1) == b':' {
            let s = i + 2;
            let mut j = s;
            while get(j) != 0 && get(j) != b']' {
                j += 1;
            }
            if get(j) == 0 {
                return None;
            }
            if j == s || get(j - 1) != b':' {
                // No `:]`: the `[` is itself, and the walk goes on from the `:`.
                items.push(Item::Byte(b'['));
            } else {
                items.push(Item::Named(named(&p[s..j - 1])?));
                i = j;
                pc = 0;
            }
        } else {
            items.push(Item::Byte(pc));
        }
        prev = pc;
        i += 1;
        pc = get(i);
        if pc == b']' {
            return Some((negated, items, i + 1));
        }
    }
}

/// The `[` at `p[at]` as a token and the index after it, or None if git aborts there (the line matches nothing).
fn class(p: &[u8], at: usize) -> Option<(Tok, usize)> {
    let (negated, items, end) = bracket(p, at)?;
    let mut sets = [[0u64; 4]; 2];
    for (fold, set) in [false, true].into_iter().zip(&mut sets) {
        for t in 0..=255u8 {
            if items.iter().any(|i| i.matches(t, fold)) != negated {
                set[usize::from(t / 64)] |= 1 << (t % 64);
            }
        }
    }
    Some((Tok::Class(Box::new(sets)), end))
}

/// One `.gitignore` line as git reads it (dir.c `add_patterns_from_buffer`, `trim_trailing_spaces`,
/// `parse_path_pattern`), as the pattern git applies to a file at the folder's top. None if it cannot match one:
/// blank, a comment, a folder-only pattern (trailing `/`), a path into a subfolder, or a pattern git aborts on.
fn parse(raw: &[u8]) -> Option<Line> {
    if raw.first() == Some(&b'#') {
        return None;
    }
    // One `\r` before the newline goes; the line is a C string, so it ends at a NUL; then unescaped trailing spaces
    // (only spaces) go.
    let mut line = raw.strip_suffix(b"\r").unwrap_or(raw);
    if let Some(nul) = line.iter().position(|b| *b == 0) {
        line = &line[..nul];
    }
    let mut keep = line.len();
    let mut i = 0;
    let mut space = None;
    while i < line.len() {
        match line[i] {
            b' ' => space = space.or(Some(i)),
            b'\\' => {
                i += 1;
                if i == line.len() {
                    space = None;
                    break;
                }
                space = None;
            }
            _ => space = None,
        }
        i += 1;
    }
    if let Some(at) = space {
        keep = at;
    }
    let line = &line[..keep];
    if line.is_empty() {
        return None;
    }
    let (negated, mut p) = match line.strip_prefix(b"!") {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    if p.ends_with(b"/") {
        return None;
    }
    p = p.strip_prefix(b"/").unwrap_or(p);
    while let Some(rest) = p.strip_prefix(b"**/") {
        p = rest;
    }
    if p.contains(&b'/') {
        return None;
    }
    let mut toks = Vec::new();
    let mut i = 0;
    while i < p.len() {
        match p[i] {
            b'*' => {
                if !matches!(toks.last(), Some(Tok::Star)) {
                    toks.push(Tok::Star);
                }
                i += 1;
            }
            b'?' => {
                toks.push(Tok::One);
                i += 1;
            }
            // A trailing `\` compares against the end of the name, which never matches.
            b'\\' => {
                toks.push(Tok::Escaped(*p.get(i + 1)?));
                i += 2;
            }
            b'[' => {
                let (tok, after) = class(p, i)?;
                toks.push(tok);
                i = after;
            }
            c => {
                toks.push(Tok::Byte(c));
                i += 1;
            }
        }
    }
    Some(Line { negated, toks })
}

/// The set of glob positions still alive, as a bitset (position `toks.len()` means a whole match).
type Bits = Vec<u64>;

fn has(bits: &Bits, p: usize) -> bool {
    bits[p / 64] & (1 << (p % 64)) != 0
}

fn add(bits: &mut Bits, p: usize) {
    bits[p / 64] |= 1 << (p % 64);
}

/// `*` may match nothing, so every live `*` also makes the position after it live.
fn closure(toks: &[Tok], bits: &mut Bits) {
    for (p, tok) in toks.iter().enumerate() {
        if has(bits, p) && matches!(tok, Tok::Star) {
            add(bits, p + 1);
        }
    }
}

fn start(toks: &[Tok]) -> Bits {
    let mut bits = vec![0; (toks.len() + 1).div_ceil(64)];
    add(&mut bits, 0);
    closure(toks, &mut bits);
    bits
}

fn step(toks: &[Tok], bits: &Bits, b: u8, fold: bool) -> Bits {
    let mut out = vec![0; bits.len()];
    for (p, tok) in toks.iter().enumerate() {
        if !has(bits, p) {
            continue;
        }
        let taken = match tok {
            Tok::Star => {
                add(&mut out, p);
                false
            }
            Tok::One => true,
            Tok::Byte(c) if fold => c.eq_ignore_ascii_case(&b),
            Tok::Byte(c) => *c == b,
            Tok::Escaped(c) if fold => *c == b.to_ascii_lowercase(),
            Tok::Escaped(c) => *c == b,
            Tok::Class(sets) => set_has(&sets[usize::from(fold)], b),
        };
        if taken {
            add(&mut out, p + 1);
        }
    }
    closure(toks, &mut out);
    out
}

/// Walks the names automaton together with `lines`, matching as git does with core.ignorecase `fold`. `judge` sees
/// each reachable whole name, as the lines' live positions, and stops the walk by returning false. None if the walk
/// passed MAX_STATES.
fn walk(
    names: &Names,
    lines: &[&Line],
    fold: bool,
    mut judge: impl FnMut(&[Bits]) -> bool,
) -> Option<bool> {
    let first = (
        0,
        lines.iter().map(|l| start(&l.toks)).collect::<Vec<Bits>>(),
    );
    let mut seen = BTreeSet::from([first.clone()]);
    let mut todo = vec![first];
    while let Some((s, live)) = todo.pop() {
        if names.accept[s] && !judge(&live) {
            return Some(false);
        }
        for (&b, &t) in &names.next[s] {
            let next = (
                t,
                lines
                    .iter()
                    .zip(&live)
                    .map(|(l, bits)| step(&l.toks, bits, b, fold))
                    .collect(),
            );
            if seen.insert(next.clone()) {
                if seen.len() > MAX_STATES {
                    return None;
                }
                todo.push(next);
            }
        }
    }
    Some(true)
}

/// Whether `line` matches some name fs.graph-files can make; None if the walk passed MAX_STATES.
fn can_match(names: &Names, line: &Line, fold: bool) -> Option<bool> {
    let whole = line.toks.len();
    let never = walk(names, &[line], fold, |live| !has(&live[0], whole))?;
    Some(!never)
}

/// The lines as git applies them to a file at the folder's top, in order; None if a line is over MAX_LINE.
fn lines(gitignore: &[u8]) -> Option<Vec<Line>> {
    // git skips a UTF-8 byte order mark at the start of the file.
    let text = gitignore.strip_prefix(b"\xef\xbb\xbf").unwrap_or(gitignore);
    let mut out = Vec::new();
    for raw in text.split(|b| *b == b'\n') {
        if raw.len() > MAX_LINE {
            return None;
        }
        out.extend(parse(raw));
    }
    Some(out)
}

/// Whether, matching as git does with core.ignorecase `fold`, every name is hidden: the last line that matches it
/// is not a `!` line, and some line matches it. False past a cap.
fn hides_with(names: &Names, lines: &[Line], fold: bool) -> bool {
    let mut can = Vec::new();
    for line in lines {
        match can_match(names, line, fold) {
            Some(false) => {}
            Some(true) => can.push(line),
            None => return false,
        }
        if can.len() > MAX_LINES {
            return false;
        }
    }
    let ends: Vec<usize> = can.iter().map(|l| l.toks.len()).collect();
    walk(names, &can, fold, |live| {
        let last = (0..can.len()).rev().find(|&i| has(&live[i], ends[i]));
        last.is_some_and(|i| !can[i].negated)
    })
    .unwrap_or(false)
}

/// True if git hides every temp and marker name fs.graph-files can make in the folder, whatever the repository's
/// core.ignorecase (false on Linux, true by default on macOS): for each name, the last `.gitignore` line that matches
/// it is not a `!` line, and some line matches it.
pub(crate) fn ignore_hides_temp_files(gitignore: Option<&[u8]>) -> bool {
    let names = Names::new();
    let Some(lines) = lines(gitignore.unwrap_or_default()) else {
        return false;
    };
    hides_with(&names, &lines, false) && hides_with(&names, &lines, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::string::String;

    fn hides(text: &str) -> bool {
        ignore_hides_temp_files(Some(text.as_bytes()))
    }

    /// The reviewer's native-git matrix (`git check-ignore` over real adapter temp names), case for case.
    #[test]
    fn the_answer_matches_native_git_on_the_review_matrix() {
        for (case, gitignore, git_hides_all) in [
            ("bare-positive", ".umx-tmp-*\n", true),
            ("rooted-trailing-space", "/.umx-tmp-*   \n", true),
            ("leading-space", "  .umx-tmp-*\n", false),
            ("negation-before", "!.umx-tmp-*\n.umx-tmp-*\n", true),
            ("negation-after", ".umx-tmp-*\n!.umx-tmp-*\n", false),
            (
                "later-nodes-counter",
                ".umx-tmp-*\n!.umx-tmp-1-nodes.jsonl\n",
                false,
            ),
            (
                "edges-negation",
                ".umx-tmp-*\n!.umx-tmp-*-edges.jsonl\n",
                false,
            ),
            (
                "graph-negation",
                ".umx-tmp-*\n!.umx-tmp-*-graph.json\n",
                false,
            ),
            (
                "unrelated-directory-negation",
                ".umx-tmp-*\n!docs/*.md\n",
                true,
            ),
            (
                "unrelated-class-negation",
                ".umx-tmp-*\n!keep[0-9].txt\n",
                true,
            ),
            (
                "directory-only-negation",
                ".umx-tmp-*\n!.umx-tmp-*/\n",
                true,
            ),
            (
                "reignore-marker",
                ".umx-tmp-*\n!.umx-tmp-set.json\n.umx-tmp-set.json\n",
                true,
            ),
        ] {
            assert_eq!(hides(gitignore), git_hides_all, "{case}: {gitignore:?}");
        }
    }

    #[test]
    fn every_name_shape_counts() {
        assert!(!ignore_hides_temp_files(None));
        assert!(!hides(""));
        for exposes in [
            "!.umx-tmp-12345-.gitignore",
            "!.umx-tmp-0-set.json",
            "!.umx-tmp-set.json",
            "!.umx-tmp-9*",
            "!*.jsonl",
            "!**/.umx-tmp-?-graph.json",
            "!/.umx-tmp-[0-9]-edges.jsonl",
            "!*",
            "!.*",
        ] {
            assert!(!hides(&format!(".umx-tmp-*\n{exposes}\n")), "{exposes}");
        }
        for harmless in [
            "!.umx-tmp-01-nodes.jsonl",
            "!.umx-tmp--nodes.jsonl",
            "!.umx-tmp-1-other.txt",
            "!x/.umx-tmp-*",
            "!*.md",
            "# !.umx-tmp-*",
            "!.umx-tmp-[!0-9]*-nodes.jsonl",
        ] {
            assert!(hides(&format!(".umx-tmp-*\n{harmless}\n")), "{harmless}");
        }
        // Lines other than umx's own can hide the names too; this follows git, not the wording of one line.
        assert!(hides(".umx-*\n"));
        assert!(hides(".umx-tmp-set.json\n.umx-tmp-*-*\n"));
        assert!(!hides(".umx-tmp-set.json\n"));
    }

    #[test]
    fn hostile_lines_stay_fast_and_count_as_not_hidden_past_the_caps() {
        let hostile = format!("!{}Z\n", "*?".repeat(12));
        assert!(hides(&format!(".umx-tmp-*\n{hostile}")));
        assert!(hides(&format!(".umx-tmp-*\n{}", hostile.repeat(2000))));
        let long = format!("!{}Z\n", "*?".repeat(200));
        assert!(!hides(&format!(".umx-tmp-*\n{long}")), "too long a line");
        let many: String = (0..70).map(|i| format!(".umx-tmp-{i}*\n")).collect();
        assert!(hides(&format!(
            ".umx-tmp-*\n{}",
            &many[..many.find("\n.umx-tmp-60").unwrap()]
        )));
        assert!(
            !hides(&format!(".umx-tmp-*\n{many}")),
            "too many lines that can match"
        );
    }
}
