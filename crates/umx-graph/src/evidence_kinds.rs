//! The evidence kinds columns (ws003 s22 and s29, review A4). A row's kinds are exactly `Node::evidence_kinds()`:
//! the distinct string `kind` members of its evidence items, in name order. Each kind text has a number. The inverted
//! index holds each kind's rows, in handle order (the `--where evidence_kinds=` filter); the forward column holds each
//! row's kind numbers (every read of a row's kinds). Both are derived from the line by `push_row` and `set_row` and
//! kept by `pop_row` and `clear_rows`, the four row functions every load, write, rollback and renumbering reaches, so
//! there is no other update path. `check_rows` compares them with a fresh build.

use crate::terms::TermSrc;
#[cfg(test)]
use crate::terms::{Forward, MASK_BACK, MASK_MAX};
pub(crate) use crate::terms::{Terms as EvidenceKinds, TermsOf as KindsOf};
use crate::{Graph, NodeRef, json};
use alloc::borrow::Cow;
use umx_core::Value;

/// Where a row's evidence is read from: the array's text in a line the scanner took, or the codec's record.
#[derive(Clone, Copy)]
pub(crate) enum EvidenceSrc<'a> {
    /// The `evidence` array as it stands in the line, brackets included, already checked by the scanner.
    Line(&'a str),
    Record(&'a [Value]),
}

impl<'a> TermSrc<'a> for EvidenceSrc<'a> {
    /// Each item's string `kind`, in item order, repeats kept (`Node::evidence_kinds` before its sort).
    fn each_term(self, mut f: impl FnMut(Cow<'a, str>)) {
        match self {
            EvidenceSrc::Line(array) => json::evidence_kinds(array, f),
            EvidenceSrc::Record(items) => {
                for item in items {
                    if let Some(kind) = item.get("kind").and_then(Value::as_str) {
                        f(Cow::Borrowed(kind));
                    }
                }
            }
        }
    }
}

impl Graph {
    /// Every node whose evidence holds an item of kind `kind`, in id order; none for a kind no node holds.
    pub fn nodes_with_evidence_kind(
        &self,
        kind: &str,
    ) -> impl ExactSizeIterator<Item = NodeRef<'_>> + '_ {
        self.ev_kinds
            .rows(kind)
            .iter()
            .map(move |&h| NodeRef { g: self, h })
    }

    /// Row `h`'s kinds, in name order, from the forward column.
    #[inline]
    pub(crate) fn evidence_kinds_of(&self, h: u32) -> KindsOf<'_> {
        self.ev_kinds.kinds(h)
    }

    /// The evidence kinds forward column's form, for tests: `mask`, `narrow` or `wide` (ws003 s29).
    #[doc(hidden)]
    pub fn evidence_kinds_layout(&self) -> &'static str {
        self.ev_kinds.layout()
    }
}

#[cfg(test)]
mod tests {
    use super::{EvidenceKinds, EvidenceSrc, MASK_BACK, MASK_MAX};
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;
    use serde_json::{Value, json};

    fn items(kinds: &[String]) -> Vec<Value> {
        kinds
            .iter()
            .map(|k| json!({"source": "a.md", "kind": k}))
            .collect()
    }

    /// A column built from `rows` (each row's kinds), one push a row.
    fn built(rows: &[Vec<String>]) -> EvidenceKinds {
        let mut c = EvidenceKinds::default();
        for (h, kinds) in rows.iter().enumerate() {
            c.push(h as u32, EvidenceSrc::Record(&items(kinds)));
        }
        c
    }

    /// The column's reads equal `rows` (sorted, deduplicated), and it checks against a fresh build of them.
    fn same(c: &EvidenceKinds, rows: &[Vec<String>]) {
        for (h, kinds) in rows.iter().enumerate() {
            let mut want: Vec<&str> = kinds.iter().map(String::as_str).collect();
            want.sort_unstable();
            want.dedup();
            let got: Vec<&str> = c.kinds(h as u32).collect();
            assert_eq!(got, want, "row {h}");
            assert_eq!(c.kinds(h as u32).len(), want.len(), "row {h}: size_hint");
        }
        c.check(&built(rows));
    }

    fn k(i: usize) -> String {
        format!("k{i:05}")
    }

    fn set(c: &mut EvidenceKinds, rows: &mut [Vec<String>], h: usize, kinds: Vec<String>) {
        c.set(h as u32, EvidenceSrc::Record(&items(&kinds)));
        rows[h] = kinds;
    }

    /// The 65th kind turns the mask into a CSR; one row flipping between 64 and 65 kinds then changes nothing more
    /// (graph-storage's hysteresis, C1) until the graph is down to 56 kinds, when it goes back to a mask.
    #[test]
    fn the_mask_becomes_a_csr_at_65_kinds_and_returns_only_at_56() {
        let mut rows: Vec<Vec<String>> = (0..MASK_MAX).map(|i| alloc::vec![k(i), k(0)]).collect();
        let mut c = built(&rows);
        assert_eq!(c.layout(), "mask");
        same(&c, &rows);
        set(&mut c, &mut rows, 3, alloc::vec![k(3), k(900)]);
        assert_eq!((c.layout(), c.switches), ("narrow", 1));
        same(&c, &rows);
        for round in 0..20 {
            let kinds = if round % 2 == 0 {
                alloc::vec![k(3)]
            } else {
                alloc::vec![k(3), k(900 + round)]
            };
            set(&mut c, &mut rows, 3, kinds);
            assert_eq!((c.layout(), c.switches), ("narrow", 1), "round {round}");
            same(&c, &rows);
        }
        // Rows 55 to 63 take kind 0 only, one at a time: the CSR stays until 56 kinds are left.
        for h in (MASK_BACK - 1..MASK_MAX).rev() {
            set(&mut c, &mut rows, h, alloc::vec![k(0)]);
            let want = if c.index.len() > MASK_BACK {
                "narrow"
            } else {
                "mask"
            };
            assert_eq!(c.layout(), want, "row {h}");
            same(&c, &rows);
        }
        assert_eq!((c.index.len(), c.switches), (MASK_BACK, 2));
    }

    /// A kind that empties moves every higher rank down one, in every row; a new kind moves them up.
    #[test]
    fn an_emptied_kind_moves_the_ranks_above_it() {
        let names = ["b", "d", "f", "h", "j", "l", "n", "p", "r"];
        let mut rows: Vec<Vec<String>> = (0..40)
            .map(|i| {
                names
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| (i + j) % 3 != 0)
                    .map(|(_, n)| String::from(*n))
                    .collect()
            })
            .collect();
        let mut c = built(&rows);
        same(&c, &rows);
        assert_eq!(c.layout(), "mask");
        // Kind "d" leaves every row: the 9 kinds become 8, one byte a row.
        for h in 0..rows.len() {
            let kinds: Vec<String> = rows[h].iter().filter(|n| *n != "d").cloned().collect();
            set(&mut c, &mut rows, h, kinds);
            same(&c, &rows);
        }
        assert!(matches!(c.fwd, super::Forward::Mask { stride: 1, .. }));
        // A kind below every other, and one between two, then the last row popped.
        set(&mut c, &mut rows, 7, alloc::vec!["a".into(), "g".into()]);
        same(&c, &rows);
        c.pop(rows.len() as u32 - 1);
        rows.pop();
        same(&c, &rows);
    }

    /// Past 65,536 kinds a CSR's numbers no longer fit `u16`: the column widens, and reads, edits and pops still
    /// answer each row's kinds.
    #[test]
    fn past_65536_kinds_the_csr_widens_to_u32() {
        let n = usize::from(u16::MAX) + 1;
        let mut rows: Vec<Vec<String>> = (0..n).map(|i| alloc::vec![k(i)]).collect();
        let mut c = built(&rows);
        assert_eq!(c.layout(), "narrow");
        rows.push(alloc::vec![k(n), k(5)]);
        c.push(n as u32, EvidenceSrc::Record(&items(&rows[n])));
        assert_eq!(c.layout(), "wide");
        set(
            &mut c,
            &mut rows,
            10,
            alloc::vec![k(n + 1), k(n + 2), k(10)],
        );
        set(&mut c, &mut rows, 11, alloc::vec![]);
        same(&c, &rows);
        c.pop(n as u32);
        rows.pop();
        same(&c, &rows);
    }
}
