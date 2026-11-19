//! The std tag index (ws008 t2b, req-117): one inverted index per std graph, on the shared term index
//! ([`crate::terms`]). Each tag (in stored form, [`umx_core::tags::stored_tags`]) has its rows in handle order, and
//! each row its tag numbers, so `--tag` and `--no-tag` filters are answered from the index, never by reading a line.
//! Built only while the graph is std ([`Graph::set_std_tags`]): a graph without std holds `tags` as an ordinary custom
//! field and pays nothing. Kept by `push_row`, `set_row`, `pop_row` and `clear_rows`, from the scanned line or the
//! codec's record alike.

use crate::terms::{TermSrc, Terms};
use crate::{Graph, NodeRef};
use alloc::borrow::Cow;
use alloc::vec::Vec;
use umx_core::Value;
use umx_core::tags::{Pattern, TAGS, stored_tags};

/// A row's `tags` value, when it has one.
#[derive(Clone, Copy)]
pub(crate) struct TagSrc<'a>(pub Option<&'a Value>);

impl<'a> TermSrc<'a> for TagSrc<'a> {
    fn each_term(self, mut f: impl FnMut(Cow<'a, str>)) {
        if let Some(v) = self.0 {
            for tag in stored_tags(v) {
                f(Cow::Owned(tag));
            }
        }
    }
}

/// The tag index, on while the graph is std.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tags {
    on: bool,
    terms: Terms,
    /// Tests only: the rows a tag filter looked at.
    #[cfg(test)]
    pub reads: core::cell::Cell<usize>,
}

impl Tags {
    /// An empty index, on or off.
    pub fn new(on: bool) -> Tags {
        Tags {
            on,
            ..Tags::default()
        }
    }

    pub fn push(&mut self, h: u32, src: TagSrc<'_>) {
        if self.on {
            self.terms.push(h, src);
        }
    }

    pub fn set(&mut self, h: u32, src: TagSrc<'_>) {
        if self.on {
            self.terms.set(h, src);
        }
    }

    pub fn pop(&mut self, h: u32) {
        if self.on {
            self.terms.pop(h);
        }
    }

    pub fn clear(&mut self) {
        self.terms.clear();
    }

    pub fn heap_bytes(&self) -> usize {
        self.terms.heap_bytes() + self.terms.forward_bytes()
    }

    pub fn shrink(&mut self) {
        self.terms.shrink();
    }

    /// Panics unless this index answers what `fresh` (built from the same image) answers.
    pub fn check(&self, fresh: &Tags) {
        assert_eq!(self.on, fresh.on, "tags: on");
        self.terms.check(&fresh.terms);
    }

    /// Whether row `h` holds a tag `group` matches.
    fn row_matches(&self, h: u32, group: &[Pattern]) -> bool {
        self.terms
            .kinds(h)
            .any(|tag| group.iter().any(|p| p.matches(tag)))
    }

    /// The rows holding a tag `group` matches, ascending: one list per tag, a facet's from a pass over the distinct
    /// tags (never the rows).
    fn rows_of(&self, group: &[Pattern]) -> Cow<'_, [u32]> {
        let mut lists: Vec<&[u32]> = Vec::new();
        for p in group {
            match p {
                Pattern::Tag(t) => lists.push(self.terms.rows(t)),
                Pattern::Facet(_) => lists.extend(
                    self.terms
                        .each()
                        .filter(|(t, _)| p.matches(t))
                        .map(|(_, r)| r),
                ),
            }
        }
        lists.retain(|l| !l.is_empty());
        match lists[..] {
            [] => Cow::Borrowed(&[]),
            [rows] => Cow::Borrowed(rows),
            _ => {
                let mut all = lists.concat();
                all.sort_unstable();
                all.dedup();
                Cow::Owned(all)
            }
        }
    }
}

/// A `--tag`/`--no-tag` filter: a node matches when it holds a tag each group in `all` matches (repeated flags are
/// AND, the patterns of one flag OR) and none `none` matches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagFilter {
    pub all: Vec<Vec<Pattern>>,
    pub none: Vec<Pattern>,
}

impl TagFilter {
    pub fn is_empty(&self) -> bool {
        self.all.is_empty() && self.none.is_empty()
    }
}

impl Graph {
    /// Turns the tag index on (the graph is std) or off. Turning it on builds it once from every row's `tags`
    /// (a graph that adopts std); a load that knows the graph is std builds it in its own pass instead.
    pub fn set_std_tags(&mut self, on: bool) {
        if on == self.tags.on {
            return;
        }
        self.tags.clear();
        self.tags.on = on;
        if on {
            for h in 0..self.ids.len() {
                let v = self.custom_value(h, TAGS);
                self.tags.push(h as u32, TagSrc(v.as_ref()));
            }
        }
        self.tags.shrink();
    }

    /// Whether the tag index is on.
    pub fn std_tags(&self) -> bool {
        self.tags.on
    }

    /// The handles `f` admits, ascending, from the tag index alone: the smallest `all` group's rows drive, and every
    /// other group and `none` are tested on each of those rows' tags. With no `all` group every handle is looked at,
    /// as an unfiltered list does. Empty while the index is off.
    pub(crate) fn tagged_rows(&self, f: &TagFilter) -> Vec<u32> {
        if !self.tags.on {
            return Vec::new();
        }
        let lists: Vec<Cow<'_, [u32]>> = f.all.iter().map(|g| self.tags.rows_of(g)).collect();
        let driver = lists
            .iter()
            .enumerate()
            .min_by_key(|(_, l)| l.len())
            .map(|(i, _)| i);
        let keep = |h: u32| {
            #[cfg(test)]
            self.tags.reads.set(self.tags.reads.get() + 1);
            f.all
                .iter()
                .enumerate()
                .all(|(i, g)| Some(i) == driver || self.tags.row_matches(h, g))
                && !self.tags.row_matches(h, &f.none)
        };
        match driver {
            Some(i) => lists[i].iter().copied().filter(|&h| keep(h)).collect(),
            None => (0..self.ids.len() as u32).filter(|&h| keep(h)).collect(),
        }
    }

    /// Whether node `n` passes `f`, from its row's tags in the index (for reads that filter rows they found another
    /// way). Never, while the index is off.
    pub fn tags_pass(&self, n: NodeRef<'_>, f: &TagFilter) -> bool {
        self.tags.on
            && f.all.iter().all(|g| self.tags.row_matches(n.h, g))
            && !self.tags.row_matches(n.h, &f.none)
    }

    /// Each node whose `tags` a hand edit left out of stored form, or holds an entry that cannot be used, with what
    /// it holds (ws008 t2b), while the index is on. Only the field's holders are read.
    pub fn tag_findings(&self) -> Vec<(umx_core::Id, umx_core::tags::StoredFindings)> {
        let Some(col) = self
            .names
            .get(TAGS)
            .and_then(|f| self.f_cols.get(&f))
            .filter(|_| self.tags.on)
        else {
            return Vec::new();
        };
        (0..self.ids.len() as u32)
            .filter(|h| col.get(*h).is_some())
            .filter_map(|h| {
                let value = NodeRef { g: self, h }.field(TAGS)?;
                let f = umx_core::tags::stored_findings(&value);
                (f.not_canonical || f.unusable).then_some((self.ids[h as usize], f))
            })
            .collect()
    }

    /// Row `h`'s tags in stored form, from the index (sorted).
    pub fn tags_of(&self, n: NodeRef<'_>) -> Vec<&str> {
        if !self.tags.on {
            return Vec::new();
        }
        self.tags.terms.kinds(n.h).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::TagFilter;
    use crate::{Graph, NodeFilter};
    use alloc::vec::Vec;
    use alloc::{format, vec};
    use umx_core::tags::Pattern;
    use umx_core::{Id, Node};

    fn node(i: u128, tags: Option<serde_json::Value>) -> Node {
        let mut n = Node {
            id: Id(i),
            kind: "risk".into(),
            title: format!("n{i}"),
            status: "open".into(),
            note: Default::default(),
            evidence: vec![],
            status_history: vec![],
            created: "2026-11-10T01:00:00Z".into(),
            updated: "2026-11-10T01:00:00Z".into(),
            created_by: "tester".into(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields: Default::default(),
        };
        if let Some(t) = tags {
            n.fields.insert("tags".into(), t);
        }
        n
    }

    fn std_graph(nodes: Vec<Node>) -> Graph {
        let mut g = Graph::from_records(nodes, vec![]);
        g.set_std_tags(true);
        g
    }

    fn p(t: &str) -> Pattern {
        Pattern::parse(t).unwrap()
    }

    fn ids(g: &Graph, f: TagFilter) -> Vec<u128> {
        g.list_nodes(&NodeFilter {
            tags: f,
            ..NodeFilter::default()
        })
        .iter()
        .map(|n| n.id().0)
        .collect()
    }

    /// 1,000 nodes, three of them tagged: a `--tag` filter looks at the tagged rows only, never the rest.
    #[test]
    fn a_tag_filter_reads_only_the_rows_the_index_lists() {
        let mut nodes: Vec<Node> = (1..=1000).map(|i| node(i, None)).collect();
        nodes[9] = node(10, Some(serde_json::json!(["origin:a", "x"])));
        nodes[99] = node(100, Some(serde_json::json!(["origin:b"])));
        nodes[499] = node(500, Some(serde_json::json!(["X", "y"])));
        let g = std_graph(nodes);
        let all = |groups: &[&[&str]]| TagFilter {
            all: groups
                .iter()
                .map(|g| g.iter().map(|t| p(t)).collect())
                .collect(),
            none: vec![],
        };
        assert_eq!(ids(&g, all(&[&["x"]])), vec![10, 500]);
        assert_eq!(g.tags.reads.take(), 2);
        assert_eq!(ids(&g, all(&[&["x"], &["y"]])), vec![500]);
        assert_eq!(g.tags.reads.take(), 1, "the smaller group drives");
        assert_eq!(ids(&g, all(&[&["origin:a", "y"]])), vec![10, 500]);
        assert_eq!(g.tags.reads.take(), 2);
        assert_eq!(ids(&g, all(&[&["origin:*"]])), vec![10, 100]);
        assert_eq!(g.tags.reads.take(), 2);
        let mut f = all(&[&["origin:*"]]);
        f.none = vec![p("x")];
        assert_eq!(ids(&g, f), vec![100]);
        assert_eq!(g.tags.reads.take(), 2);
        // A complement alone looks at every handle, as an unfiltered list does, reading each row's tags from the
        // index.
        let none = TagFilter {
            all: vec![],
            none: vec![p("x"), p("origin:*")],
        };
        let got = ids(&g, none);
        assert_eq!(got.len(), 997);
        assert!(!got.contains(&10) && !got.contains(&100) && !got.contains(&500));
        assert_eq!(g.tags.reads.take(), 1000);
    }

    /// A line the scanner takes and one only the codec reads (a field nested past the scanner's depth) both give
    /// their row's tags to the index, at load and on a write.
    #[test]
    fn tags_are_indexed_from_the_scanned_line_and_from_the_codec_record() {
        let hot = node(1, Some(serde_json::json!(["b", "A"])));
        let mut cold = node(2, Some(serde_json::json!(["a", "c"])));
        let deep = (0..110).fold(serde_json::json!(1), |v, _| serde_json::json!([v]));
        cold.fields.insert("deep".into(), deep);
        let file = umx_core::codec::encode_nodes([&hot, &cold]);
        let mut fields = Vec::new();
        let lines: Vec<&[u8]> = file.split(|b| *b == b'\n').take(2).collect();
        assert!(matches!(
            crate::nodes::derive(lines[0], 1, &mut fields),
            Ok(crate::nodes::Derived::Hot(_))
        ));
        assert!(matches!(
            crate::nodes::derive(lines[1], 2, &mut fields),
            Ok(crate::nodes::Derived::Record(_))
        ));
        let (mut g, _) = Graph::load_nodes_indexing_for_graph(
            file,
            umx_core::ontology::TextFields::default(),
            Id(9),
            true,
        )
        .unwrap();
        g.load_edges(&[]).unwrap();
        let a = || TagFilter {
            all: vec![vec![p("a")]],
            none: vec![],
        };
        assert_eq!(ids(&g, a()), vec![1, 2]);
        let mut cold = g.node(Id(2)).unwrap().to_node();
        cold.fields.insert("tags".into(), serde_json::json!(["d"]));
        g.insert_node(cold).unwrap();
        let mut hot = g.node(Id(1)).unwrap().to_node();
        hot.fields.insert("tags".into(), serde_json::json!(["d"]));
        g.insert_node(hot).unwrap();
        assert!(ids(&g, a()).is_empty());
        let d = TagFilter {
            all: vec![vec![p("d")]],
            none: vec![],
        };
        assert_eq!(ids(&g, d), vec![1, 2]);
        g.check_rows();
    }

    /// A graph that is not std indexes nothing and its filters match nothing; turning std on builds the index once.
    #[test]
    fn the_index_is_off_until_the_graph_is_std() {
        let nodes = vec![node(1, Some(serde_json::json!(["a"])))];
        let mut g = Graph::from_records(nodes, vec![]);
        assert_eq!(g.memory_bytes().tags, 0);
        let f = || TagFilter {
            all: vec![vec![p("a")]],
            none: vec![],
        };
        assert!(ids(&g, f()).is_empty());
        g.set_std_tags(true);
        assert_eq!(ids(&g, f()), vec![1]);
        assert!(g.memory_bytes().tags > 0);
        g.set_std_tags(false);
        assert!(ids(&g, f()).is_empty());
    }
}
