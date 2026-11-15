//! The English (Porter2) stemmer, written from the published Snowball algorithm
//! (<https://snowballstem.org/algorithms/english/stemmer.html>, Snowball 3.1.1's `english.sbl`): word search's light
//! stemming (ws004 s21, req-92). It runs on lower-case ASCII letters only; any other word is its own stem, so words
//! with digits (`iso27001`, hex) and other scripts match exactly. Apostrophes never reach it (words split on them).

use alloc::borrow::Cow;
use alloc::vec::Vec;

fn v(c: u8) -> bool {
    matches!(c, b'a' | b'e' | b'i' | b'o' | b'u' | b'y')
}

fn v_wxy(c: u8) -> bool {
    v(c) || matches!(c, b'w' | b'x' | b'Y')
}

/// Prefixes that end R1 where they end (Snowball's exceptions to the region rule).
const R1_PREFIXES: [&[u8]; 9] = [
    b"arsen", b"commun", b"emerg", b"gener", b"inter", b"later", b"organ", b"past", b"univers",
];

/// Whole words with a fixed stem (themselves when the stem is `None`).
const EXCEPTIONS: [(&str, Option<&str>); 15] = [
    ("skis", Some("ski")),
    ("skies", Some("sky")),
    ("idly", Some("idl")),
    ("gently", Some("gentl")),
    ("ugly", Some("ugli")),
    ("early", Some("earli")),
    ("only", Some("onli")),
    ("singly", Some("singl")),
    ("sky", None),
    ("news", None),
    ("howe", None),
    ("atlas", None),
    ("cosmos", None),
    ("bias", None),
    ("andes", None),
];

/// Whether word search stems `word` and expands it as a prefix: all lower-case ASCII letters (req-92: words with
/// digits or other letters match exactly only).
pub fn stemmed(word: &str) -> bool {
    !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase())
}

/// `word`'s Porter2 stem; `word` itself when it is not all lower-case ASCII letters or is under three letters.
pub fn stem(word: &str) -> Cow<'_, str> {
    if !stemmed(word) {
        return Cow::Borrowed(word);
    }
    if let Some((_, fixed)) = EXCEPTIONS.iter().find(|(w, _)| *w == word) {
        return Cow::Borrowed(fixed.unwrap_or(word));
    }
    if word.len() < 3 {
        return Cow::Borrowed(word);
    }
    let mut w: Vec<u8> = word.as_bytes().to_vec();
    // Prelude: an initial y, and a y after a vowel, are consonants (Y).
    let mut y_found = false;
    if w[0] == b'y' {
        w[0] = b'Y';
        y_found = true;
    }
    for i in 0..w.len() - 1 {
        if v(w[i]) && w[i + 1] == b'y' {
            w[i + 1] = b'Y';
            y_found = true;
        }
    }
    let (p1, p2) = regions(&w);
    step_1a(&mut w);
    step_1b(&mut w, p1);
    step_1c(&mut w);
    step_2(&mut w, p1);
    step_3(&mut w, p1, p2);
    step_4(&mut w, p2);
    step_5(&mut w, p1, p2);
    if y_found {
        for c in &mut w {
            if *c == b'Y' {
                *c = b'y';
            }
        }
    }
    if w == word.as_bytes() {
        return Cow::Borrowed(word);
    }
    Cow::Owned(w.into_iter().map(char::from).collect())
}

/// Where the region after the first non-vowel following a vowel starts, from `from`.
fn past_vc(w: &[u8], from: usize) -> Option<usize> {
    let i = (from..w.len()).find(|&i| v(w[i]))?;
    let j = (i + 1..w.len()).find(|&j| !v(w[j]))?;
    Some(j + 1)
}

/// R1 and R2's starts (the word's length when empty).
fn regions(w: &[u8]) -> (usize, usize) {
    let n = w.len();
    let p1 = R1_PREFIXES
        .iter()
        .filter(|p| w.starts_with(p))
        .map(|p| p.len())
        .max()
        .or_else(|| past_vc(w, 0));
    match p1 {
        Some(p1) => (p1, past_vc(w, p1).unwrap_or(n)),
        None => (n, n),
    }
}

/// Whether `w[..c]` ends in a short syllable.
fn short_v(w: &[u8], c: usize) -> bool {
    (c >= 3 && !v_wxy(w[c - 1]) && v(w[c - 2]) && !v(w[c - 3]))
        || (c == 2 && !v(w[1]) && v(w[0]))
        || w[..c].ends_with(b"past")
}

fn replace_end(w: &mut Vec<u8>, cut: usize, with: &[u8]) {
    w.truncate(w.len() - cut);
    w.extend_from_slice(with);
}

fn step_1a(w: &mut Vec<u8>) {
    let n = w.len();
    if w.ends_with(b"sses") {
        replace_end(w, 4, b"ss");
    } else if w.ends_with(b"ied") || w.ends_with(b"ies") {
        replace_end(w, 3, if n - 3 >= 2 { b"i" } else { b"ie" });
    } else if w.ends_with(b"ss") || w.ends_with(b"us") {
    } else if w.ends_with(b"s") && n >= 2 && w[..n - 2].iter().any(|&c| v(c)) {
        w.pop();
    }
}

fn step_1b(w: &mut Vec<u8>, p1: usize) {
    let n = w.len();
    let (len, kind) = if w.ends_with(b"eedly") {
        (5, 1)
    } else if w.ends_with(b"ingly") {
        (5, 2)
    } else if w.ends_with(b"edly") {
        (4, 2)
    } else if w.ends_with(b"eed") {
        (3, 1)
    } else if w.ends_with(b"ing") {
        (3, 3)
    } else if w.ends_with(b"ed") {
        (2, 2)
    } else {
        return;
    };
    let start = n - len;
    let pre = &w[..start];
    if kind == 1 {
        if start >= p1 && ![&b"succ"[..], b"proc", b"exc"].contains(&pre) {
            replace_end(w, len, b"ee");
        }
        return;
    }
    if kind == 3 {
        if [&b"even"[..], b"cann", b"inn", b"earr", b"herr", b"out"].contains(&pre) {
            return;
        }
        if pre.len() == 2 && pre[1] == b'y' && !v(pre[0]) {
            replace_end(w, 4, b"ie");
            return;
        }
    }
    if !pre.iter().any(|&c| v(c)) {
        return;
    }
    w.truncate(start);
    let m = w.len();
    if w.ends_with(b"at") || w.ends_with(b"bl") || w.ends_with(b"iz") {
        w.push(b'e');
    } else if m >= 2
        && w[m - 1] == w[m - 2]
        && matches!(
            w[m - 1],
            b'b' | b'd' | b'f' | b'g' | b'm' | b'n' | b'p' | b'r' | b't'
        )
    {
        if !(m == 3 && matches!(w[0], b'a' | b'e' | b'o')) {
            w.pop();
        }
    } else if m == p1 && short_v(w, m) {
        w.push(b'e');
    }
}

fn step_1c(w: &mut [u8]) {
    let n = w.len();
    if n >= 3 && matches!(w[n - 1], b'y' | b'Y') && !v(w[n - 2]) {
        w[n - 1] = b'i';
    }
}

/// What a suffix needs besides its region before it is replaced.
#[derive(Clone, Copy)]
enum When {
    Always,
    AfterL,
    AfterValidLi,
    InR2,
    AfterSOrT,
}

/// Finds the longest of `table`'s suffixes on `w` (longest first in the table) and, if it starts at or after
/// `region` and meets its condition, replaces it. As in Snowball, a longest match that fails is not retried shorter.
fn suffix_step(w: &mut Vec<u8>, region: usize, p2: usize, table: &[(&[u8], &[u8], When)]) {
    let Some(&(suffix, with, when)) = table.iter().find(|(s, _, _)| w.ends_with(s)) else {
        return;
    };
    let start = w.len() - suffix.len();
    if start < region {
        return;
    }
    let ok = match when {
        When::Always => true,
        When::AfterL => start >= 1 && w[start - 1] == b'l',
        When::AfterValidLi => {
            start >= 1
                && matches!(
                    w[start - 1],
                    b'c' | b'd' | b'e' | b'g' | b'h' | b'k' | b'm' | b'n' | b'r' | b't'
                )
        }
        When::InR2 => start >= p2,
        When::AfterSOrT => start >= 1 && matches!(w[start - 1], b's' | b't'),
    };
    if ok {
        replace_end(w, suffix.len(), with);
    }
}

fn step_2(w: &mut Vec<u8>, p1: usize) {
    use When::*;
    const TABLE: [(&[u8], &[u8], When); 25] = [
        (b"ational", b"ate", Always),
        (b"ization", b"ize", Always),
        (b"iveness", b"ive", Always),
        (b"fulness", b"ful", Always),
        (b"ousness", b"ous", Always),
        (b"tional", b"tion", Always),
        (b"biliti", b"ble", Always),
        (b"lessli", b"less", Always),
        (b"entli", b"ent", Always),
        (b"ousli", b"ous", Always),
        (b"fulli", b"ful", Always),
        (b"aliti", b"al", Always),
        (b"iviti", b"ive", Always),
        (b"alism", b"al", Always),
        (b"ation", b"ate", Always),
        (b"ogist", b"og", Always),
        (b"anci", b"ance", Always),
        (b"enci", b"ence", Always),
        (b"abli", b"able", Always),
        (b"alli", b"al", Always),
        (b"izer", b"ize", Always),
        (b"ator", b"ate", Always),
        (b"ogi", b"og", AfterL),
        (b"bli", b"ble", Always),
        (b"li", b"", AfterValidLi),
    ];
    suffix_step(w, p1, 0, &TABLE);
}

fn step_3(w: &mut Vec<u8>, p1: usize, p2: usize) {
    use When::*;
    const TABLE: [(&[u8], &[u8], When); 9] = [
        (b"ational", b"ate", Always),
        (b"tional", b"tion", Always),
        (b"icate", b"ic", Always),
        (b"ative", b"", InR2),
        (b"alize", b"al", Always),
        (b"iciti", b"ic", Always),
        (b"ical", b"ic", Always),
        (b"ness", b"", Always),
        (b"ful", b"", Always),
    ];
    suffix_step(w, p1, p2, &TABLE);
}

fn step_4(w: &mut Vec<u8>, p2: usize) {
    use When::*;
    const TABLE: [(&[u8], &[u8], When); 18] = [
        (b"ement", b"", Always),
        (b"ment", b"", Always),
        (b"ance", b"", Always),
        (b"ence", b"", Always),
        (b"able", b"", Always),
        (b"ible", b"", Always),
        (b"ate", b"", Always),
        (b"ive", b"", Always),
        (b"ize", b"", Always),
        (b"iti", b"", Always),
        (b"ism", b"", Always),
        (b"ion", b"", AfterSOrT),
        (b"ous", b"", Always),
        (b"ant", b"", Always),
        (b"ent", b"", Always),
        (b"al", b"", Always),
        (b"ic", b"", Always),
        (b"er", b"", Always),
    ];
    suffix_step(w, p2, p2, &TABLE);
}

fn step_5(w: &mut Vec<u8>, p1: usize, p2: usize) {
    let n = w.len();
    if n == 0 {
        return;
    }
    let start = n - 1;
    match w[start] {
        b'e' if start >= p2 || (start >= p1 && !short_v(w, start)) => {
            w.pop();
        }
        b'l' if start >= p2 && start >= 1 && w[start - 1] == b'l' => {
            w.pop();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Snowball's own English vocabulary and stems (BSD-3, see THIRD_PARTY_NOTICES): every 10th all-letter pair.
    const PAIRS: &str = include_str!("../tests/fixtures/porter2-sample.txt");

    #[test]
    fn stems_equal_snowballs_sample() {
        let mut checked = 0;
        for line in PAIRS
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let (word, want) = line.split_once(' ').expect("word stem");
            assert_eq!(stem(word), want, "{word}");
            checked += 1;
        }
        assert!(checked > 3_000, "{checked}");
    }

    #[test]
    fn viper_s_words_meet() {
        for w in ["approve", "approval", "approved", "approving", "approv"] {
            assert_eq!(stem(w), "approv", "{w}");
        }
        assert_eq!(stem("vendors"), "vendor");
        assert_eq!(stem("vendor"), "vendor");
    }

    #[test]
    fn words_with_digits_or_other_letters_are_their_own_stems() {
        for w in ["iso27001", "v2", "3f9ac0d1", "cafés", "naïve", "ab"] {
            assert_eq!(stem(w), w);
        }
    }
}
