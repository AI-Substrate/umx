//! The title find as it was before s06 (plan 027), kept as the reference the new find must equal to the bit: the
//! scoring is `crates/umx-graph/src/find.rs` at main b567a3c, verbatim; the scan is `Graph::find`'s, over the public
//! reads.
#![allow(dead_code)] // each test file uses part of it

use std::cmp::Ordering;
use umx_core::Id;
use umx_graph::Graph;

/// Scores below this are not returned by `find`.
pub const MIN_SCORE: f64 = 0.4;

fn normalise(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn bigrams(s: &str) -> Vec<(char, char)> {
    let padded: Vec<char> = format!(" {s} ").chars().collect();
    let mut out: Vec<(char, char)> = padded.windows(2).map(|w| (w[0], w[1])).collect();
    out.sort();
    out
}

/// Sørensen-Dice over character bigrams of sorted multisets.
fn dice(a: &str, b: &str) -> f64 {
    let (x, y) = (bigrams(a), bigrams(b));
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
    (2 * common) as f64 / (x.len() + y.len()) as f64
}

/// 0.0 to 1.0: the better of character-bigram similarity in the given word order and with the words sorted, so
/// typos and reordered words both score high. Rounded to 3 places so output is stable.
pub fn title_score(query: &str, title: &str) -> f64 {
    let (q, t) = (normalise(query), normalise(title));
    if q.is_empty() || t.is_empty() {
        return 0.0;
    }
    let in_order = dice(&q.join(" "), &t.join(" "));
    let (mut qs, mut ts) = (q, t);
    qs.sort();
    ts.sort();
    let sorted = dice(&qs.join(" "), &ts.join(" "));
    // Round half up without std's f64::round (scores are never negative).
    ((in_order.max(sorted) * 1000.0 + 0.5) as u64) as f64 / 1000.0
}

/// The old `Graph::find` over (id, type, title) rows in id order: best first, then by id; scores as bits.
pub fn find<'a>(
    rows: impl Iterator<Item = (Id, &'a str, &'a str)>,
    text: &str,
    kind: Option<&str>,
    limit: usize,
) -> Vec<(Id, u64)> {
    let mut scored: Vec<(Id, f64)> = rows
        .filter(|(_, k, _)| kind.is_none_or(|want| *k == want))
        .map(|(id, _, title)| (id, title_score(text, title)))
        .filter(|(_, s)| *s >= MIN_SCORE)
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.truncate(limit);
    scored
        .into_iter()
        .map(|(id, s)| (id, s.to_bits()))
        .collect()
}

/// The new find's answer in the same shape.
pub fn new(g: &Graph, text: &str, kind: Option<&str>, limit: usize) -> Vec<(Id, u64)> {
    g.find(text, kind, limit)
        .into_iter()
        .map(|(n, s)| (n.id(), s.to_bits()))
        .collect()
}

// ws004 s24's rule, written apart from the verbatim s06 code above: a title holding every query word (repeats
// counted) scores max(d, 0.5 + 0.2 d), where d is the old score. The reference is a scan over every title.

/// Whether `title` holds each of `query`'s words at least as often as `query` does.
pub fn holds_all(query: &str, title: &str) -> bool {
    let (q, mut t) = (normalise(query), normalise(title));
    !q.is_empty()
        && q.iter().all(|w| match t.iter().position(|x| x == w) {
            Some(i) => {
                t.swap_remove(i);
                true
            }
            None => false,
        })
}

/// The old score with ws004 s24's rule.
pub fn rule_score(query: &str, title: &str) -> f64 {
    let d = title_score(query, title);
    if !holds_all(query, title) {
        return d;
    }
    let lifted = ((0.5 + 0.2 * d) * 1000.0 + 0.5) as u64 as f64 / 1000.0;
    if lifted > d { lifted } else { d }
}

/// [`find`] with ws004 s24's rule and ws004 s30's keep-rule (`umx_graph::find_keeps`, a pure function of query,
/// title, score and containment, proven by its own tests): what `Graph::find` must answer, to the bit.
pub fn find_rule<'a>(
    rows: impl Iterator<Item = (Id, &'a str, &'a str)>,
    text: &str,
    kind: Option<&str>,
    limit: usize,
) -> Vec<(Id, u64)> {
    let mut scored: Vec<(Id, f64)> = rows
        .filter(|(_, k, _)| kind.is_none_or(|want| *k == want))
        .map(|(id, _, title)| (id, rule_score(text, title), title))
        .filter(|(_, s, title)| {
            *s >= MIN_SCORE && umx_graph::find_keeps(text, title, *s, holds_all(text, title))
        })
        .map(|(id, s, _)| (id, s))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.truncate(limit);
    scored
        .into_iter()
        .map(|(id, s)| (id, s.to_bits()))
        .collect()
}
