//! Approximate title match for `node find` and the near-duplicate check (req-57). The word index (req-68) finds the
//! candidates without scanning every title (ws004 s15, workshop 008 section 11): an upper bound on each title's score,
//! gathered over the term table and the title postings, keeps every title that could reach the threshold, so nothing
//! the scan found is lost. Each candidate is then scored with s06's scorer, unchanged: the same results, scores and
//! order as s06's scan.
//!
//! Since ws004 s30, two keep-rules drop some scored hits (never raise a score, so the bound stays complete): a
//! near duplicate's words that the two titles do not share exactly must be within a typo of each other
//! ([`near_duplicate_keeps`]), and a short query found on whole-title Dice alone must have each word close to a
//! title word ([`find_keeps`]).
//!
//! Since ws004 s24, `find` also returns a title holding every query word ([`contained_score`]): whole-title Dice
//! alone cannot reach [`MIN_SCORE`] once a title is more than about four times the query's length, however exactly
//! the query's words appear in it. The near-duplicate check stays whole-title Dice.

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

/// Scores below this are not returned by `find`.
pub const MIN_SCORE: f64 = 0.4;

/// A query of at most this many words is checked word by word when found on whole-title Dice alone (ws004 s30).
pub const SHORT_QUERY_WORDS: usize = 3;

/// A whole-title Dice score at or above this is kept for a short query without the word check (ws004 s30).
pub const FUZZY_SURE: f64 = 0.8;

/// `s`'s words, lower case, joined by one space, appended to `out`. A word is a run of alphanumeric characters
/// after lower-casing the whole text, so a word never holds a space.
pub(crate) fn normalise_into(s: &str, out: &mut String) {
    if s.is_ascii() {
        // The same text with no allocation: ASCII lower-cases byte by byte (ws004 s30 normalises hits per call).
        let (mut gap, mut any) = (false, false);
        for b in s.bytes() {
            if b.is_ascii_alphanumeric() {
                if gap && any {
                    out.push(' ');
                }
                out.push(b.to_ascii_lowercase() as char);
                gap = false;
                any = true;
            } else {
                gap = true;
            }
        }
        return;
    }
    let mut first = true;
    for w in s
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        if !first {
            out.push(' ');
        }
        out.push_str(w);
        first = false;
    }
}

/// A character bigram, ordered as the pair `(a, b)` is.
#[inline]
fn pack(a: char, b: char) -> u64 {
    u64::from(a as u32) << 32 | u64::from(b as u32)
}

/// The sorted bigrams of ` text `, for normalised text.
pub(crate) fn bigrams(text: &str, out: &mut Vec<u64>) {
    out.clear();
    let mut prev = ' ';
    for c in text.chars().chain([' ']) {
        out.push(pack(prev, c));
        prev = c;
    }
    out.sort_unstable();
}

/// Bigrams two sorted multisets share (the multiset intersection's size). Branch-free steps: equal counts one and
/// moves both, otherwise the smaller side moves, as a branching merge does (ws004 s18: random bigrams mispredict).
pub(crate) fn common(x: &[u64], y: &[u64]) -> usize {
    let (mut i, mut j, mut common) = (0, 0, 0);
    while i < x.len() && j < y.len() {
        let (a, b) = (x[i], y[j]);
        common += usize::from(a == b);
        i += usize::from(a <= b);
        j += usize::from(b <= a);
    }
    common
}

/// [`common`] by a branching merge: faster than the branch-free one on a query against one title's bigrams (ws004
/// s24 bench), whose order is predictable.
fn common_branching(x: &[u64], y: &[u64]) -> usize {
    let (mut i, mut j, mut common) = (0, 0, 0);
    while i < x.len() && j < y.len() {
        match x[i].cmp(&y[j]) {
            Ordering::Equal => {
                common += 1;
                i += 1;
                j += 1;
            }
            Ordering::Less => i += 1,
            Ordering::Greater => j += 1,
        }
    }
    common
}

/// A query's bigrams and its normalised text (its words joined by one space), made once per call.
pub(crate) struct Query(pub(crate) Vec<u64>, pub(crate) String);

impl Query {
    /// None if the query has no words: it matches nothing.
    pub(crate) fn new(text: &str) -> Option<Query> {
        let mut norm = String::new();
        normalise_into(text, &mut norm);
        if norm.is_empty() {
            return None;
        }
        let mut grams = Vec::new();
        bigrams(&norm, &mut grams);
        Some(Query(grams, norm))
    }

    /// The query's words, in order, repeats kept.
    pub(crate) fn words(&self) -> impl Iterator<Item = &str> {
        self.1.split(' ')
    }
}

/// `x` rounded half up to 3 places, so output is stable (std's f64::round is not in core; scores are never negative).
#[inline]
fn round3(x: f64) -> f64 {
    ((x * 1000.0 + 0.5) as u64) as f64 / 1000.0
}

/// find's Dice score from the shared bigram count and the two sizes, rounded: the one expression every score uses.
#[inline]
fn rounded_dice(common: usize, q: usize, t: usize) -> f64 {
    round3((2 * common) as f64 / (q + t) as f64)
}

/// The Dice score of a title holding every query word (ws004 s24), from sizes alone: such a title holds each of
/// the query's ` w ` bigrams at least as often as the query does, so they share all |Q| of them, and Dice is
/// 2|Q| / (|Q| + |T|). Equal to [`score_title`]'s to the bit.
#[inline]
pub(crate) fn contained_dice(q: usize, t: usize) -> f64 {
    rounded_dice(q, q, t)
}

/// The score of a title holding every query word (ws004 s24), from its Dice score `d`: max(d, 0.5 + 0.2 d). At least
/// 0.5, so it is found; rising with `d`, so shorter titles rank first; and `d` itself from 0.625 up, so an exact
/// title stays 1.0 and first and a close match keeps its score.
pub(crate) fn contained_score(d: f64) -> f64 {
    let lifted = round3(0.5 + 0.2 * d);
    if lifted > d { lifted } else { d }
}

/// The sorted bigrams of ` normalised(title) `, computed from the title itself: for ASCII without building the
/// normalised text (the find hot path, no allocation), otherwise through [`normalise_into`] (full Unicode
/// lower-casing, as Greek final sigma needs). Equal to `bigrams(normalised(title))` either way.
pub(crate) fn title_bigrams(title: &str, text: &mut String, out: &mut Vec<u64>) {
    if !title.is_ascii() {
        text.clear();
        normalise_into(title, text);
        bigrams(text, out);
        return;
    }
    out.clear();
    let (mut prev, mut gap, mut any) = (' ', false, false);
    for b in title.bytes() {
        if b.is_ascii_alphanumeric() {
            let c = b.to_ascii_lowercase() as char;
            if gap && any {
                out.push(pack(prev, ' '));
                prev = ' ';
            }
            out.push(pack(prev, c));
            prev = c;
            gap = false;
            any = true;
        } else {
            gap = true;
        }
    }
    if any {
        out.push(pack(prev, ' '));
    }
    out.sort_unstable();
}

/// [`score`] for a raw title: its bigrams made by [`title_bigrams`].
pub(crate) fn score_title(q: &Query, title: &str, text: &mut String, buf: &mut Vec<u64>) -> f64 {
    title_bigrams(title, text, buf);
    if buf.is_empty() {
        return 0.0;
    }
    rounded_dice(common_branching(&q.0, buf), q.0.len(), buf.len())
}

/// 0.0 to 1.0 for a normalised title: Sørensen-Dice over the character bigrams of ` query ` and ` title `, rounded
/// to 3 places so output is stable. Typos cost a few bigrams. Word order costs nothing: words hold no space, so the
/// bigrams of ` w1 w2 ` are those of ` w1 ` and ` w2 ` together, in any order. (Before s06 the score also compared
/// the texts with their words sorted and took the better; that is always the same number, so it is not computed.)
/// `buf` is reused across the titles of one call.
pub(crate) fn score(q: &Query, title: &str, buf: &mut Vec<u64>) -> f64 {
    if title.is_empty() {
        return 0.0;
    }
    bigrams(title, buf);
    rounded_dice(common_branching(&q.0, buf), q.0.len(), buf.len())
}

/// 0.0 to 1.0: how well `title` matches `query`, typos and word order tolerated, as `find` scores it.
pub fn title_score(query: &str, title: &str) -> f64 {
    let Some(q) = Query::new(query) else {
        return 0.0;
    };
    let mut t = String::new();
    normalise_into(title, &mut t);
    score(&q, &t, &mut Vec::new())
}

/// Whether a word holds a digit. Such words (versions, years, phases, numbered parts) match only exactly: "v2" and
/// "v3" are one edit apart and name different things (ws004 s30, the embeddings-expert's rule, as s21's stems).
fn has_digit(w: &str) -> bool {
    w.chars().any(char::is_numeric)
}

/// [`letters`]'s bit for a digit: bits 26 to 35.
const DIGITS: u64 = ((1 << 10) - 1) << 26;

/// The letters a lower-cased word holds, as a set (a to z, 0 to 9, every other character one shared bit), and its
/// length in characters.
#[inline]
fn letters(w: &str) -> (u64, usize) {
    let (mut set, mut n) = (0u64, 0);
    for c in w.chars() {
        set |= letter_bit(c);
        n += 1;
    }
    (set, n)
}

/// A lower-cased character's bit in [`letters`]'s set.
#[inline]
fn letter_bit(c: char) -> u64 {
    1 << match c {
        'a'..='z' => c as u32 - 'a' as u32,
        '0'..='9' => 26 + c as u32 - '0' as u32,
        _ if c.is_numeric() => 26,
        _ => 63,
    }
}

/// Buffers the keep-rules reuse across the hits of one call, so a hit allocates nothing once they have grown.
#[derive(Default)]
pub(crate) struct Words {
    a: Vec<char>,
    b: Vec<char>,
    rows: Vec<usize>,
    used: Vec<bool>,
    left_a: String,
    left_b: String,
    /// The query's words with their letter sets and lengths, longest first: made on the first [`Words::covered`].
    query: Vec<(String, u64, usize)>,
    /// A title's words as byte ranges with their letter sets.
    spans: Vec<(usize, usize, u64, usize)>,
}

impl Words {
    /// Whether two normalised words are within a typo: equal, or neither holds a digit and their optimal string
    /// alignment distance (insert, delete, substitute, swap two neighbours), in characters, is at most
    /// max(1, longer / 4).
    pub(crate) fn close(&mut self, a: &str, b: &str) -> bool {
        if a == b {
            return true;
        }
        let ((ma, n), (mb, m)) = (letters(a), letters(b));
        if (ma | mb) & DIGITS != 0 {
            return false;
        }
        let k = (n.max(m) / 4).max(1);
        // Cheap tests first (ws004 s30 bench): one edit changes the length by at most one and the set of letters
        // present by at most two, so words further apart on either cannot be close.
        if n.abs_diff(m) > k || ((ma ^ mb).count_ones() as usize) > 2 * k {
            return false;
        }
        self.a.clear();
        self.a.extend(a.chars());
        self.b.clear();
        self.b.extend(b.chars());
        self.edits_within(k)
    }

    /// Whether the optimal string alignment distance of `self.a` and `self.b` is at most `k`.
    fn edits_within(&mut self, k: usize) -> bool {
        let m = self.b.len();
        let n = self.a.len();
        // Three rows of m + 1: the one before last, the last and this one. Values above k are capped at k + 1.
        let w = m + 1;
        self.rows.clear();
        self.rows.resize(3 * w, 0);
        for (j, cell) in self.rows[w..2 * w].iter_mut().enumerate() {
            *cell = j.min(k + 1);
        }
        for i in 1..=n {
            let (before, rest) = self.rows.split_at_mut(w);
            let (last, this) = rest.split_at_mut(w);
            this[0] = i.min(k + 1);
            let mut low = this[0];
            for j in 1..=m {
                let cost = usize::from(self.a[i - 1] != self.b[j - 1]);
                let mut d = (last[j] + 1).min(this[j - 1] + 1).min(last[j - 1] + cost);
                if i > 1
                    && j > 1
                    && self.a[i - 1] == self.b[j - 2]
                    && self.a[i - 2] == self.b[j - 1]
                {
                    d = d.min(before[j - 2] + 1);
                }
                this[j] = d.min(k + 1);
                low = low.min(this[j]);
            }
            if low > k {
                return false;
            }
            self.rows.rotate_left(w);
        }
        self.rows[w + m] <= k
    }

    /// [`near_duplicate_keeps`] for two normalised titles.
    pub(crate) fn leftovers_close(&mut self, a: &str, b: &str) -> bool {
        self.used.clear();
        self.used.resize(b.split(' ').count(), false);
        // Whether a word left on either side is shorter than four letters: such a token ("a", "b", "eu") is checked
        // by itself, never through the pooled words' edit budget (R294 B1).
        let mut short = false;
        self.left_a.clear();
        for x in a.split(' ') {
            match b
                .split(' ')
                .enumerate()
                .position(|(i, y)| !self.used[i] && x == y)
            {
                Some(i) => self.used[i] = true,
                None => {
                    short |= x.chars().count() < 4;
                    self.left_a.push_str(x);
                }
            }
        }
        self.left_b.clear();
        for (y, used) in b.split(' ').zip(&self.used) {
            if !used {
                short |= y.chars().count() < 4;
                self.left_b.push_str(y);
            }
        }
        let (la, lb) = (
            core::mem::take(&mut self.left_a),
            core::mem::take(&mut self.left_b),
        );
        let keep = match (la.is_empty(), lb.is_empty()) {
            (true, true) => true,
            (true, false) => !has_digit(&lb),
            (false, true) => !has_digit(&la),
            (false, false) if has_digit(&la) || has_digit(&lb) => la == lb,
            // A short token one letter off ("unit a", "unit b") names another thing, not a typo (the project manager):
            // with one left on either side, the words left must be the same letters, only joined or split differently.
            (false, false) if short => la == lb,
            (false, false) => self.close(&la, &lb),
        };
        (self.left_a, self.left_b) = (la, lb);
        keep
    }

    /// Whether every word of the normalised query is close to a word of `title` (raw or normalised), or (four
    /// characters or more, no digit) is held inside the title's words joined: a query word that joins title words.
    /// The query's words are read once per call ([`Words`] lives for one call), longest first, since a dropped hit
    /// is usually dropped by its longest word (a typo or a join).
    pub(crate) fn covered(&mut self, query: &str, title: &str) -> bool {
        if self.query.is_empty() {
            let mut words: Vec<(String, u64, usize)> = query
                .split(' ')
                .map(|w| {
                    let (set, n) = letters(w);
                    (w.into(), set, n)
                })
                .collect();
            words.sort_by_key(|w| core::cmp::Reverse(w.2));
            self.query = words;
        }
        if !title.is_ascii() {
            let mut text = core::mem::take(&mut self.left_a);
            text.clear();
            normalise_into(title, &mut text);
            let keep = self.covered_ascii_or_normalised(&text, false);
            self.left_a = text;
            return keep;
        }
        self.covered_ascii_or_normalised(title, true)
    }

    /// [`Self::covered`] for ASCII text: `raw` splits on every non-alphanumeric byte and compares without case;
    /// otherwise `title` is normalised (words joined by one space).
    fn covered_ascii_or_normalised(&mut self, title: &str, raw: bool) -> bool {
        // The title's words as byte ranges with their letter sets and lengths in characters, the union of the sets
        // and the total length.
        self.spans.clear();
        let (mut all, mut chars) = (0u64, 0usize);
        if raw {
            let bytes = title.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if !bytes[i].is_ascii_alphanumeric() {
                    i += 1;
                    continue;
                }
                let (start, mut set) = (i, 0u64);
                while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
                    set |= letter_bit(bytes[i].to_ascii_lowercase() as char);
                    i += 1;
                }
                self.spans.push((start, i, set, i - start));
            }
        } else {
            let mut start = 0;
            for w in title.split(' ') {
                let (set, n) = letters(w);
                self.spans.push((start, start + w.len(), set, n));
                start += w.len() + 1;
            }
        }
        for &(_, _, set, n) in &self.spans {
            all |= set;
            chars += n;
        }
        let query = core::mem::take(&mut self.query);
        let mut joined_ready = false;
        let mut keep = true;
        for (q, qset, qn) in &query {
            let qb = q.as_bytes();
            let mut found = false;
            for s in 0..self.spans.len() {
                let (start, end, set, n) = self.spans[s];
                let t = &title[start..end];
                if t.len() == qb.len() && t.as_bytes().eq_ignore_ascii_case(qb) {
                    found = true;
                    break;
                }
                let k = (n.max(*qn) / 4).max(1);
                if (set | qset) & DIGITS != 0
                    || n.abs_diff(*qn) > k
                    || ((set ^ qset).count_ones() as usize) > 2 * k
                {
                    continue;
                }
                self.a.clear();
                self.a.extend(q.chars());
                self.b.clear();
                self.b.extend(t.chars().map(|c| c.to_ascii_lowercase()));
                if self.edits_within(k) {
                    found = true;
                    break;
                }
            }
            if found {
                continue;
            }
            // A query word joining title words: every letter of it is in the title, and so is the word itself.
            if qset & DIGITS != 0 || *qn < 4 || *qn > chars || qset & !all != 0 {
                keep = false;
                break;
            }
            if !joined_ready {
                self.left_b.clear();
                for &(start, end, _, _) in &self.spans {
                    self.left_b
                        .extend(title[start..end].chars().map(|c| c.to_ascii_lowercase()));
                }
                joined_ready = true;
            }
            if !self.left_b.contains(q.as_str()) {
                keep = false;
                break;
            }
        }
        self.query = query;
        keep
    }
}

/// Whether the near-duplicate check keeps `other` for `title` once whole-title Dice has reached its threshold (ws004
/// s30): take away the words both hold exactly (repeats counted); if both have words left, those left on each side,
/// joined with no space in title order, must be within a typo of each other, so a word joined, split or misspelled
/// stays a duplicate while a shared scaffold with a different word ("Data Science team", "Geoscience Data team") does
/// not. Words left that hold a digit must match exactly, and a word left on one side alone may not hold one: a phase,
/// version, period or year makes a new thing, not a copy ("FY24", "FY25"). When a word left on either side is shorter
/// than four letters, the words left must be the same letters (joined or split only): one letter off in a short token
/// is another thing, not a typo, and it never borrows a longer word's edit budget.
pub fn near_duplicate_keeps(title: &str, other: &str) -> bool {
    let (mut a, mut b) = (String::new(), String::new());
    normalise_into(title, &mut a);
    normalise_into(other, &mut b);
    Words::default().leftovers_close(&a, &b)
}

/// Whether `find` keeps a hit scoring `score` for `query` (ws004 s30): always for a title holding every query word,
/// a score of at least [`FUZZY_SURE`], or a query of more than [`SHORT_QUERY_WORDS`] words; otherwise only if each
/// query word is close to a title word (a typo, by edit distance; digits exact) or joins title words. A short name
/// query no longer finds an unrelated short name through shared letters.
pub fn find_keeps(query: &str, title: &str, score: f64, holds_all: bool) -> bool {
    let (mut q, mut t) = (String::new(), String::new());
    normalise_into(query, &mut q);
    normalise_into(title, &mut t);
    holds_all
        || score >= FUZZY_SURE
        || q.split(' ').count() > SHORT_QUERY_WORDS
        || Words::default().covered(&q, &t)
}

#[cfg(test)]
mod tests {
    //! s21 (review C1, C2): title find over the title words only, against a scan of every title. The scan scores
    //! each title with the public scorer and applies the public keep-rules (s30) and provenance rule, so it
    //! shares no candidate step with `find_in`. Graphs churn through inserts, edits, removes, provenance nodes, batch
    //! commits and rollbacks, with words that are in notes only, in both, and that move between title and note;
    //! `check_rows` after every round compares the word index, its kept counts and its title words with a rebuild.

    use super::*;
    use crate::Graph;
    use alloc::format;
    use alloc::string::ToString;
    use alloc::vec;
    use umx_core::{Id, Node};

    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 33) % n as u64) as usize
        }

        fn word(&mut self, lo: usize, hi: usize) -> String {
            (0..lo + self.below(hi - lo + 1))
                .map(|_| (b'a' + self.below(26) as u8) as char)
                .collect()
        }

        fn pick<'a>(&mut self, from: &'a [String]) -> &'a str {
            &from[self.below(from.len())]
        }
    }

    const KINDS: [&str; 2] = ["risk", "problem"];

    fn node(id: u128, kind: &str, title: &str, note: &str) -> Node {
        Node {
            id: Id(id),
            kind: kind.into(),
            title: title.into(),
            status: "open".into(),
            note: note.into(),
            evidence: vec![],
            status_history: vec![],
            created: String::new(),
            updated: String::new(),
            created_by: String::new(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields: Default::default(),
        }
    }

    /// Title words, note-only words (some reach a title later, some leave one), and digit words; since ws003 s27 also
    /// 64-hex and short hex tokens and one- and two-letter words.
    struct Vocab {
        title: Vec<String>,
        note: Vec<String>,
    }

    impl Vocab {
        fn new(rng: &mut Rng) -> Vocab {
            let mut title: Vec<String> = (0..60).map(|_| rng.word(2, 10)).collect();
            title.extend(["v2", "v3", "fy24", "fy25", "launch", "lunch"].map(String::from));
            let hex = |rng: &mut Rng, len: usize| -> String {
                (0..len)
                    .map(|_| char::from_digit(rng.below(16) as u32, 16).unwrap())
                    .collect()
            };
            for _ in 0..8 {
                title.push(hex(rng, 64));
                let len = 4 + rng.below(9);
                title.push(hex(rng, len));
                title.push(rng.word(1, 2));
            }
            let note = (0..60).map(|_| rng.word(3, 12)).collect();
            Vocab { title, note }
        }

        /// 1 to 5 title words; one in five titles also takes a note word, so a note word becomes a title word.
        fn title(&self, rng: &mut Rng) -> String {
            let mut words: Vec<&str> = (0..1 + rng.below(5))
                .map(|_| rng.pick(&self.title))
                .collect();
            if rng.below(5) == 0 {
                words.push(rng.pick(&self.note));
            }
            words.join(" ")
        }

        /// 0 to 8 words, mostly note words, some title words.
        fn note(&self, rng: &mut Rng) -> String {
            (0..rng.below(9))
                .map(|_| {
                    if rng.below(4) == 0 {
                        rng.pick(&self.title)
                    } else {
                        rng.pick(&self.note)
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
    }

    /// `text` with up to two typos (a letter changed, dropped or added, a space added), sometimes its spaces dropped.
    fn typo(rng: &mut Rng, text: &str) -> String {
        let mut c: Vec<char> = text.chars().collect();
        for _ in 0..rng.below(3) {
            if c.is_empty() {
                break;
            }
            let i = rng.below(c.len());
            match rng.below(4) {
                0 => c[i] = (b'a' + rng.below(26) as u8) as char,
                1 if c.len() > 1 => {
                    c.remove(i);
                }
                2 => c.insert(i, ' '),
                _ => c.insert(i, (b'a' + rng.below(26) as u8) as char),
            }
        }
        let q: String = c.into_iter().collect();
        if rng.below(5) == 0 {
            q.replace(' ', "")
        } else {
            q
        }
    }

    /// Whether normalised `title` holds each normalised query word at least as often as the query does.
    fn holds_all(q: &Query, title: &str) -> bool {
        let mut t = String::new();
        normalise_into(title, &mut t);
        q.words().all(|w| {
            t.split(' ').filter(|x| *x == w).count() >= q.words().filter(|x| *x == w).count()
        })
    }

    fn ranked(mut hits: Vec<(Id, f64)>, limit: usize) -> Vec<(Id, f64)> {
        hits.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        hits.truncate(limit);
        hits
    }

    /// `find` (and with `provenance`, `find_with_provenance`) by scoring every title.
    fn scan_find(
        g: &Graph,
        text: &str,
        kind: Option<&str>,
        limit: usize,
        provenance: bool,
    ) -> Vec<(Id, f64)> {
        let Some(q) = Query::new(text) else {
            return Vec::new();
        };
        let hits = g
            .nodes()
            .filter(|n| provenance || !g.is_provenance(n.ix()))
            .filter(|n| kind.is_none_or(|k| n.kind() == k))
            .filter_map(|n| {
                let d = title_score(text, n.title());
                let all = holds_all(&q, n.title());
                let s = if all { contained_score(d) } else { d };
                (s >= MIN_SCORE && find_keeps(text, n.title(), s, all)).then_some((n.id(), s))
            })
            .collect();
        ranked(hits, limit)
    }

    /// `near_duplicates` by scoring every title (provenance is never near).
    fn scan_near(
        g: &Graph,
        title: &str,
        kind: Option<&str>,
        min: f64,
        other_min: f64,
        limit: usize,
    ) -> Vec<(Id, f64)> {
        let hits = g
            .nodes()
            .filter(|n| !g.is_provenance(n.ix()))
            .filter_map(|n| {
                let s = title_score(title, n.title());
                let same = kind.is_none_or(|k| n.kind() == k);
                (s >= min && (same || s >= other_min) && near_duplicate_keeps(title, n.title()))
                    .then_some((n.id(), s))
            })
            .collect();
        ranked(hits, limit)
    }

    fn found(hits: Vec<(crate::NodeRef<'_>, f64)>) -> Vec<(Id, f64)> {
        hits.into_iter().map(|(n, s)| (n.id(), s)).collect()
    }

    /// What the checks reached, so a run that never meets a case fails rather than passing vacuously.
    #[derive(Default, Debug)]
    struct Reached {
        hits: usize,
        contained: usize,
        provenance_hits: usize,
        cross_type_band: usize,
        note_only_queries: usize,
        rollbacks: usize,
        removes: usize,
        /// ws003 s27: near-duplicate hits mid-batch on a node an earlier op of the same batch wrote.
        in_batch_near: usize,
    }

    /// Every query form against the scan: find with and without a kind, at a small and a large limit, with and
    /// without provenance, and the near-duplicate check with and without a kind.
    fn check_queries(g: &Graph, rng: &mut Rng, vocab: &Vocab, titles: &[String], r: &mut Reached) {
        let mut queries: Vec<String> = Vec::new();
        for _ in 0..12 {
            let t = rng.pick(titles).to_string();
            queries.push(typo(rng, &t));
        }
        for _ in 0..4 {
            queries.push(rng.pick(titles).into());
            queries.push(rng.pick(&vocab.title).into());
        }
        // Words a note holds and maybe no title does: no title candidate, and containment finds nothing extra.
        for _ in 0..4 {
            let w = rng.pick(&vocab.note).to_string();
            if !titles.iter().any(|t| t.split(' ').any(|x| x == w)) {
                r.note_only_queries += 1;
            }
            queries.push(typo(rng, &w));
            queries.push(w);
        }
        for q in &queries {
            for (kind, limit) in [(None, 10), (None, 1000), (Some("risk"), 5)] {
                let want = scan_find(g, q, kind, limit, false);
                assert_eq!(
                    found(g.find(q, kind, limit)),
                    want,
                    "find {q:?} {kind:?} {limit}"
                );
                r.hits += want.len();
                if let Some(q) = Query::new(q) {
                    r.contained += want
                        .iter()
                        .filter(|(id, _)| holds_all(&q, g.node(*id).unwrap().title()))
                        .count();
                }
                let want_p = scan_find(g, q, kind, limit, true);
                assert_eq!(
                    found(g.find_with_provenance(q, kind, limit)),
                    want_p,
                    "find_with_provenance {q:?} {kind:?} {limit}"
                );
                r.provenance_hits += want_p
                    .iter()
                    .filter(|(id, _)| g.is_provenance(g.node(*id).unwrap().ix()))
                    .count();
            }
            for kind in [None, Some("risk")] {
                let want = scan_near(g, q, kind, 0.85, 0.95, 1000);
                assert_eq!(
                    found(g.near_duplicates(q, kind, 0.85, 0.95, 1000)),
                    want,
                    "near {q:?} {kind:?}"
                );
                // s30's other-type rule: hits of another type scoring 0.85 to 0.95 the scan drops.
                if let Some(k) = kind {
                    r.cross_type_band += g
                        .nodes()
                        .filter(|n| !g.is_provenance(n.ix()) && n.kind() != k)
                        .filter(|n| (0.85..0.95).contains(&title_score(q, n.title())))
                        .count();
                }
            }
        }
    }

    /// One seed: a graph built, then churned round by round, checked after each round.
    fn run(seed: u64, r: &mut Reached) {
        let mut rng = Rng(seed);
        let vocab = Vocab::new(&mut rng);
        let mut g = Graph::default();
        let mut next = 1u128;
        let add = |g: &mut Graph, rng: &mut Rng, next: &mut u128| {
            let kind = KINDS[rng.below(2)];
            let (title, note) = (vocab.title(rng), vocab.note(rng));
            g.insert_node(node(*next, kind, &title, &note)).unwrap();
            *next += 1;
        };
        for _ in 0..120 {
            add(&mut g, &mut rng, &mut next);
        }
        for round in 0..16 {
            let ids: Vec<Id> = g.nodes().map(|n| n.id()).collect();
            match round % 4 {
                // Edits: a title word moves to the note and a note word to the title, and some words leave both.
                0 => {
                    for _ in 0..20 {
                        let id = ids[rng.below(ids.len())];
                        let n = g.node(id).unwrap();
                        if g.is_provenance(n.ix()) {
                            continue;
                        }
                        let (kind, old) = (n.kind().to_string(), n.title().to_string());
                        let mut title = vocab.title(&mut rng);
                        let note = if rng.below(2) == 0 {
                            title.push(' ');
                            title.push_str(rng.pick(&vocab.note));
                            old
                        } else {
                            vocab.note(&mut rng)
                        };
                        g.insert_node(node(id.0, &kind, &title, &note)).unwrap();
                    }
                }
                1 => {
                    for _ in 0..15 {
                        let id = ids[rng.below(ids.len())];
                        if g.remove_node(id).is_some() {
                            r.removes += 1;
                        }
                    }
                    for _ in 0..10 {
                        add(&mut g, &mut rng, &mut next);
                    }
                }
                // Provenance nodes: titles the word index never holds, found only by find_with_provenance.
                2 => {
                    for _ in 0..3 {
                        let mut n = node(next, "agent", &vocab.title(&mut rng), "");
                        n.fields.insert(
                            "prov_key".into(),
                            serde_json::json!(format!("agent/pij/pij-{}", rng.word(4, 8))),
                        );
                        g.insert_node(n).unwrap();
                        next += 1;
                    }
                }
                _ => {}
            }
            // A batch of adds and edits, committed or rolled back. Before each, the near-duplicate check of its title
            // on the batch's graph, as `umx batch` runs it (ws003 s27): a title an earlier op wrote is found, and
            // some titles are typos of one.
            let ids: Vec<Id> = g.nodes().map(|n| n.id()).collect();
            let mut b = g.batch();
            let mut wrote: Vec<(Id, String)> = Vec::new();
            for _ in 0..12 {
                let kind = KINDS[rng.below(2)];
                let (mut title, note) = (vocab.title(&mut rng), vocab.note(&mut rng));
                if !wrote.is_empty() && rng.below(3) == 0 {
                    let k = rng.below(wrote.len());
                    title = typo(&mut rng, &wrote[k].1);
                }
                let id = if rng.below(2) == 0 {
                    next += 1;
                    next - 1
                } else {
                    ids[rng.below(ids.len())].0
                };
                if b.graph()
                    .node(Id(id))
                    .is_some_and(|n| b.graph().is_provenance(n.ix()))
                {
                    continue;
                }
                let want = scan_near(b.graph(), &title, Some(kind), 0.85, 0.95, 3);
                assert_eq!(
                    found(b.graph().near_duplicates(&title, Some(kind), 0.85, 0.95, 3)),
                    want,
                    "near {title:?} mid-batch"
                );
                r.in_batch_near += want
                    .iter()
                    .filter(|(id, _)| wrote.iter().any(|(w, _)| w == id))
                    .count();
                b.insert_node(node(id, kind, &title, &note)).unwrap();
                wrote.push((Id(id), title));
            }
            if rng.below(2) == 0 {
                b.rollback();
                r.rollbacks += 1;
            } else {
                b.commit();
            }
            g.check_rows();
            let titles: Vec<String> = g.nodes().map(|n| n.title().to_string()).collect();
            check_queries(&g, &mut rng, &vocab, &titles, r);
        }
    }

    /// s21: title find over the title words gives the scan's hits, scores and order, through every kind of write,
    /// for find, find_with_provenance and the near-duplicate check.
    #[test]
    fn title_find_equals_a_scan_of_every_title() {
        let mut r = Reached::default();
        for seed in 1..=6 {
            run(seed, &mut r);
        }
        assert!(
            r.hits > 5000
                && r.contained > 500
                && r.provenance_hits > 50
                && r.cross_type_band > 20
                && r.note_only_queries > 50
                && r.rollbacks > 20
                && r.removes > 50
                && r.in_batch_near > 20,
            "{r:?}"
        );
    }
}
