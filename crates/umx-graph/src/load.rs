//! The load path (workshop 008 section 5; load rules 5.1): one pass per file. Node lines become the image and their
//! rows are derived from it; edge lines become edge columns. A file umx wrote (sorted, no repeats, no blank lines)
//! is used as it is. Otherwise every line is checked first, then the image is laid out as the next write will: id
//! order, a repeated id's earlier lines kept just before its line in use (req-67), blank lines dropped.

use crate::Graph;
use crate::json;
use crate::line::{self, HotEdge};
use crate::nodes::{self, LIMIT, Walk};
use alloc::vec::Vec;
use umx_core::Id;
use umx_core::codec::{DecodeError, DecodeReason, GraphFile, KeptLine, Repeat};
use umx_core::ontology::TextFields;

/// An edge with an end that is not a node of the graph (req-76): which edge, which end, the missing node and the
/// line (1-based) of the edge in edges.jsonl.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dangling {
    pub edge: Id,
    pub end: &'static str,
    pub node: Id,
    pub line: usize,
}

/// A symmetric relation's edge whose reverse is in the graph too (D49): `edge` (its `from` has the higher id) is kept
/// as its exact line and left out of the graph; `in_use` (the same pair, lower id as `from`) stays. `line` is the
/// 1-based line of `edge` in edges.jsonl.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reversed {
    pub edge: Id,
    pub in_use: Id,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidProvenance {
    pub file: GraphFile,
    pub record: Id,
    pub line: usize,
    pub key: &'static str,
}

/// A stored node using the owning graph's reserved id, including every physical line of that id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphIdNode {
    pub id: Id,
    pub lines: Vec<usize>,
}

/// Repeated ids, reserved graph-id lines, provenance findings and file order found while loading nodes.jsonl.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodesLoad {
    pub repeats: Vec<Repeat>,
    /// Every line using the owning graph's reserved id, kept aside rather than made into a row.
    pub graph_id_node: Option<GraphIdNode>,
    /// Kept duplicates, each losing node paired with the lower-id winner. Services surface the warning in P2.
    pub duplicate_prov_keys: Vec<crate::DuplicateProvKey>,
    pub invalid_provenance: Vec<InvalidProvenance>,
    /// The first line (1-based) that is blank or has a smaller id than the line before it (workshop 008 section 5.1
    /// rule 2): the file loads, and the next write puts it in id order without its blank lines. None if it is in
    /// order (a repeated id's lines next to each other are in order).
    pub unsorted: Option<usize>,
}

/// What loading edges.jsonl found besides the edges: repeated ids and dangling edges, in file order, and whether the
/// file is out of order (as `NodesLoad::unsorted`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgesLoad {
    pub repeats: Vec<Repeat>,
    pub dangling: Vec<Dangling>,
    pub unsorted: Option<usize>,
    /// Reversed pairs of symmetric edges, in file order (`load_edges_symmetric`).
    pub reversed: Vec<Reversed>,
    pub invalid_provenance: Vec<InvalidProvenance>,
}

fn too_large(file: GraphFile, line: usize) -> DecodeError {
    DecodeError {
        file,
        line,
        column: 1,
        reason: DecodeReason::TooLarge,
    }
}

/// Every line of `bytes`: (1-based number, start, end without the `\n`).
fn lines(bytes: &[u8]) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
    let mut at = 0;
    let mut no = 0;
    core::iter::from_fn(move || {
        if at >= bytes.len() {
            return None;
        }
        let end = json::newline(bytes, at);
        no += 1;
        let line = (no, at, end);
        at = end + 1;
        Some(line)
    })
}

/// What [`Graph::reset_edges`] goes back to: taken by [`Graph::edges_mark`] before the edges load.
#[derive(Clone, Copy, Debug)]
pub struct EdgesMark {
    names: usize,
}

impl Graph {
    /// Loads nodes.jsonl's bytes, which become the image. Returns the graph (no edges yet; see `load_edges`), the
    /// repeated ids and where the file is out of order. A line that does not decode is the codec's error for that
    /// line, first in file order.
    pub fn load_nodes(bytes: Vec<u8>) -> Result<(Graph, NodesLoad), DecodeError> {
        Graph::load_nodes_indexing(bytes, TextFields::default())
    }

    /// `load_nodes`, with the custom fields word search indexes besides title and note: those the ontology
    /// declares as text (`Ontology::text_fields`). The word index is built in this same pass (req-68).
    pub fn load_nodes_indexing(
        bytes: Vec<u8>,
        text_fields: TextFields,
    ) -> Result<(Graph, NodesLoad), DecodeError> {
        Graph::load_nodes_within(bytes, LIMIT, text_fields)
    }

    /// Graph-aware load: keep the graph's reserved id aside, without indexing it as a node.
    pub fn load_nodes_indexing_for_graph(
        bytes: Vec<u8>,
        text_fields: TextFields,
        graph_id: Id,
    ) -> Result<(Graph, NodesLoad), DecodeError> {
        Graph::load_nodes_excluding(bytes, LIMIT, text_fields, Some(graph_id))
    }

    /// `load_nodes` with the image size limit given (tests use a small one).
    pub(crate) fn load_nodes_within(
        bytes: Vec<u8>,
        limit: usize,
        text_fields: TextFields,
    ) -> Result<(Graph, NodesLoad), DecodeError> {
        Graph::load_nodes_excluding(bytes, limit, text_fields, None)
    }

    fn load_nodes_excluding(
        mut bytes: Vec<u8>,
        limit: usize,
        text_fields: TextFields,
        graph_id: Option<Id>,
    ) -> Result<(Graph, NodesLoad), DecodeError> {
        let tail = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1);
        if !json::blank(&bytes[tail..]) {
            bytes.push(b'\n');
        }
        if bytes.len() > limit {
            return Err(too_large(GraphFile::Nodes, 1));
        }
        let mut g = Graph {
            image: bytes,
            text_fields,
            ..Graph::default()
        };
        let mut invalid_provenance = Vec::new();
        if g.derive_rows(Walk::File(graph_id), Some(&mut invalid_provenance))? {
            g.shrink();
            let duplicate_prov_keys = g.duplicate_prov_keys().collect();
            return Ok((
                g,
                NodesLoad {
                    duplicate_prov_keys,
                    invalid_provenance,
                    ..NodesLoad::default()
                },
            ));
        }
        // Unsorted, repeated or blank lines: check every line in file order, then lay the image out.
        let bytes = core::mem::take(&mut g.image);
        let text_fields = core::mem::take(&mut g.text_fields);
        let mut found = Vec::new();
        let mut fields = Vec::new();
        let mut order = Order::default();
        invalid_provenance.clear();
        for (no, start, end) in lines(&bytes) {
            let line = &bytes[start..end];
            if json::blank(line) {
                order.blank(no);
                continue;
            }
            let derived = nodes::derive(line, no, &mut fields)?;
            let id = derived.id();
            invalid_provenance.extend(derived.invalid_keys().map(|key| InvalidProvenance {
                file: GraphFile::Nodes,
                record: id,
                line: no,
                key,
            }));
            order.id(id, no);
            found.push((id, no, start, end));
        }
        let (groups, mut repeats) = group_with(GraphFile::Nodes, found);
        repeats.retain(|repeat| Some(repeat.id) != graph_id);
        let mut image = Vec::with_capacity(bytes.len() + 1);
        let mut kept = Vec::new();
        let mut graph_id_node = None;
        for (id, lines) in &groups {
            let reserved = Some(*id) == graph_id;
            if reserved {
                graph_id_node = Some(GraphIdNode {
                    id: *id,
                    lines: lines.iter().map(|(no, ..)| *no).collect(),
                });
            }
            for (j, &(_, start, end)) in lines.iter().enumerate() {
                if reserved || j + 1 < lines.len() {
                    kept.push((*id, image.len() as u32));
                }
                image.extend_from_slice(&bytes[start..end]);
                image.push(b'\n');
            }
        }
        drop(bytes);
        if image.len() > limit {
            return Err(too_large(GraphFile::Nodes, 1));
        }
        let mut g = Graph {
            image,
            kept_nodes: kept,
            text_fields,
            ..Graph::default()
        };
        g.derive_rows(Walk::Image, None)?;
        g.shrink();
        let duplicate_prov_keys = g.duplicate_prov_keys().collect();
        Ok((
            g,
            NodesLoad {
                repeats,
                graph_id_node,
                duplicate_prov_keys,
                invalid_provenance,
                unsorted: order.first_out,
            },
        ))
    }

    /// Loads edges.jsonl's bytes into a graph that has nodes and no edges yet. An edge with a missing local end is
    /// kept as its exact line and reported (req-76); external ends are never dangling. A repeated id's earlier
    /// lines are kept (req-67).
    pub fn load_edges(&mut self, bytes: &[u8]) -> Result<EdgesLoad, DecodeError> {
        self.load_edges_within(bytes, LIMIT)
    }

    /// Where the graph stands before [`Self::load_edges`], for [`Self::reset_edges`].
    pub fn edges_mark(&self) -> EdgesMark {
        EdgesMark {
            names: self.names.len(),
        }
    }

    /// Undoes a [`Self::load_edges`] (one that failed part way too), back to the state `mark` was taken in, so the
    /// edges can load again: no edge, kept line, external end or adjacency, and no relation or author name the load
    /// added. Only for a graph changed by nothing but that load since `mark` (services' own-end fallback, s20).
    pub fn reset_edges(&mut self, mark: EdgesMark) {
        self.names.truncate(mark.names);
        self.e_ids = Vec::new();
        self.e_kind = Default::default();
        self.e_by = Default::default();
        self.e_from = Vec::new();
        self.e_to = Vec::new();
        self.ext = Vec::new();
        self.ext_dirty = false;
        self.ext_out = Default::default();
        self.ext_in = Default::default();
        self.e_created = Default::default();
        self.e_fields = Default::default();
        self.e_evidence = Default::default();
        self.out = Default::default();
        self.inc = Default::default();
        self.kept_edges = Vec::new();
        self.provenance.edge_authored = Default::default();
        self.provenance.edge_edited = Default::default();
    }

    /// `load_edges`, then D49: for each relation in `symmetric`, an edge whose reverse is also in the graph is one
    /// edge both ways. The one with the lower id as `from` stays; the other is kept as its exact line (written back
    /// unchanged, like a dangling edge) and reported in `reversed`. A lone edge stored either way is used as it is.
    pub fn load_edges_symmetric(
        &mut self,
        bytes: &[u8],
        symmetric: &[&str],
    ) -> Result<EdgesLoad, DecodeError> {
        let mut out = self.load_edges(bytes)?;
        let pairs = self.reversed_pairs(symmetric);
        if !pairs.is_empty() {
            let lines = edge_lines(bytes, &pairs)?;
            out.reversed = self.keep_reversed(&pairs, lines);
        }
        Ok(out)
    }

    /// Reversed pairs of `symmetric` relations in the graph, as (the edge with the higher id as `from`, the edge in
    /// use), sorted by the first. Empty when every symmetric fact is one edge.
    pub fn reversed_pairs(&self, symmetric: &[&str]) -> Vec<(Id, Id)> {
        if symmetric.is_empty() {
            return Vec::new();
        }
        let mut pairs: Vec<(Id, Id)> = self
            .edges()
            .filter(|e| e.from_graph().is_none() && e.to_graph().is_none())
            .filter(|e| e.from() > e.to() && symmetric.contains(&e.kind()))
            .filter_map(|e| Some((e.id(), self.find_edge(e.to(), e.kind(), e.from())?.id())))
            .collect();
        pairs.sort_unstable();
        pairs
    }

    /// Leaves each pair's first edge out of the graph and keeps its line (`lines`: (id, 1-based line, bytes), from
    /// [`edge_lines`]; an edge with no line found is kept as the line umx writes for it). Returns the report.
    pub fn keep_reversed(
        &mut self,
        pairs: &[(Id, Id)],
        lines: Vec<(Id, usize, Vec<u8>)>,
    ) -> Vec<Reversed> {
        let mut out = Vec::new();
        for &(id, in_use) in pairs {
            let Some(h) = self.edge_handle(id) else {
                continue;
            };
            let (no, line) = match lines.iter().find(|(l, ..)| *l == id) {
                Some((_, no, line)) => (*no, line.clone()),
                None => {
                    let mut line = umx_core::codec::encode_edges(&[self.edge_record(h)]);
                    line.pop();
                    (0, line)
                }
            };
            self.remove_edge_row(h);
            self.kept_edges.push(KeptLine { id, line });
            out.push(Reversed {
                edge: id,
                in_use,
                line: no,
            });
        }
        out.sort_by_key(|r| r.line);
        self.rebuild_adjacency();
        out
    }

    /// `load_edges` with the edge count limit given (tests use a small one).
    pub(crate) fn load_edges_within(
        &mut self,
        bytes: &[u8],
        limit: usize,
    ) -> Result<EdgesLoad, DecodeError> {
        assert!(
            self.e_ids.is_empty() && self.kept_edges.is_empty(),
            "edges load once"
        );
        let mut out = EdgesLoad::default();
        let mut sorted = true;
        // Every line's id counts, a dangling one too (R1): a repeat or a step back anywhere takes the path that
        // groups lines by id, so only an id's last line is loaded or reported dangling (req-67, req-76).
        let mut last: Option<Id> = None;
        for (no, start, end) in lines(bytes) {
            let line = &bytes[start..end];
            if json::blank(line) {
                out.unsorted = out.unsorted.or(Some(no));
                continue;
            }
            let e = edge_of(line, no)?;
            if last.is_some_and(|l| l >= e.id) {
                sorted = false;
                break;
            }
            last = Some(e.id);
            out.invalid_provenance.extend(
                umx_core::provenance::invalid_provenance_fields(&e.fields).map(|key| {
                    InvalidProvenance {
                        file: GraphFile::Edges,
                        record: e.id,
                        line: no,
                        key,
                    }
                }),
            );
            self.add_loaded_edge(e, line, no, &mut out.dangling, limit)?;
        }
        if !sorted {
            for h in (0..self.e_ids.len()).rev() {
                self.remove_edge_row(h);
            }
            self.kept_edges.clear();
            self.ext.clear();
            self.ext_dirty = false;
            out.dangling.clear();
            out.invalid_provenance.clear();
            let mut found = Vec::new();
            let mut order = Order::default();
            for (no, start, end) in lines(bytes) {
                let line = &bytes[start..end];
                if json::blank(line) {
                    order.blank(no);
                } else {
                    let edge = edge_of(line, no)?;
                    let id = edge.id;
                    out.invalid_provenance.extend(
                        umx_core::provenance::invalid_provenance_fields(&edge.fields).map(|key| {
                            InvalidProvenance {
                                file: GraphFile::Edges,
                                record: id,
                                line: no,
                                key,
                            }
                        }),
                    );
                    order.id(id, no);
                    found.push((id, no, start, end));
                }
            }
            out.unsorted = order.first_out;
            let (groups, repeats) = group_with(GraphFile::Edges, found);
            out.repeats = repeats;
            for (id, lines) in groups {
                let (&(no, start, end), earlier) = lines.split_last().expect("a group");
                for &(_, s, e) in earlier {
                    self.kept_edges.push(KeptLine {
                        id,
                        line: bytes[s..e].to_vec(),
                    });
                }
                let line = &bytes[start..end];
                self.add_loaded_edge(edge_of(line, no)?, line, no, &mut out.dangling, limit)?;
            }
            out.dangling.sort_by_key(|d| d.line);
        }
        self.rebuild_adjacency();
        self.e_ids.shrink_to_fit();
        self.e_kind.shrink();
        self.e_by.shrink();
        self.e_from.shrink_to_fit();
        self.e_to.shrink_to_fit();
        self.e_created.shrink();
        self.out.shrink();
        self.inc.shrink();
        self.ext.shrink_to_fit();
        self.ext_out.shrink();
        self.ext_in.shrink();
        Ok(out)
    }

    fn add_loaded_edge(
        &mut self,
        e: HotEdge<'_>,
        line: &[u8],
        no: usize,
        dangling: &mut Vec<Dangling>,
        limit: usize,
    ) -> Result<(), DecodeError> {
        if self.e_ids.len() >= limit {
            return Err(too_large(GraphFile::Edges, no));
        }
        match self.edge_handles(e.from_graph, e.from, e.to_graph, e.to) {
            Some((f, t)) => self.push_edge_parts(e, f, t),
            None => {
                for (end, missing, node) in [
                    (
                        "from",
                        e.from_graph.is_none() && self.handle(e.from).is_none(),
                        e.from,
                    ),
                    (
                        "to",
                        e.to_graph.is_none() && self.handle(e.to).is_none(),
                        e.to,
                    ),
                ] {
                    if missing {
                        dangling.push(Dangling {
                            edge: e.id,
                            end,
                            node,
                            line: no,
                        });
                    }
                }
                self.kept_edges.push(KeptLine {
                    id: e.id,
                    line: line.to_vec(),
                });
            }
        }
        Ok(())
    }
}

fn edge_of(line: &[u8], no: usize) -> Result<HotEdge<'_>, DecodeError> {
    match line::edge(line) {
        Ok(e) => Ok(e),
        Err(_) => Ok(HotEdge::of(line::decode_edge(line, no)?)),
    }
}

/// Finds the first line a sorted file without blank lines would not have: a blank one, or one whose id is smaller
/// than the line before it.
#[derive(Default)]
struct Order {
    last: Option<Id>,
    first_out: Option<usize>,
}

impl Order {
    fn blank(&mut self, no: usize) {
        self.first_out = self.first_out.or(Some(no));
    }

    fn id(&mut self, id: Id, no: usize) {
        if self.last.is_some_and(|l| id < l) {
            self.first_out = self.first_out.or(Some(no));
        }
        self.last = Some(id);
    }
}

type Lines = Vec<(usize, usize, usize)>;

/// The lines found, by id in id order, each id's lines in file order (the last is in use), and the repeated ids as
/// the codec reports them.
fn group_with(
    file: GraphFile,
    mut found: Vec<(Id, usize, usize, usize)>,
) -> (Vec<(Id, Lines)>, Vec<Repeat>) {
    found.sort_by_key(|f| f.0);
    let mut groups: Vec<(Id, Lines)> = Vec::new();
    for (id, no, start, end) in found {
        match groups.last_mut() {
            Some((last, lines)) if *last == id => lines.push((no, start, end)),
            _ => groups.push((id, alloc::vec![(no, start, end)])),
        }
    }
    let repeats = groups
        .iter()
        .filter(|(_, l)| l.len() > 1)
        .map(|(id, l)| Repeat {
            file,
            id: *id,
            lines: l.iter().map(|x| x.0).collect(),
        })
        .collect();
    (groups, repeats)
}

/// The line in use (an id's last line; earlier ones are kept already, req-67) of each pair's first edge in
/// edges.jsonl's `bytes`, as (id, 1-based line, bytes).
pub fn edge_lines(
    bytes: &[u8],
    pairs: &[(Id, Id)],
) -> Result<Vec<(Id, usize, Vec<u8>)>, DecodeError> {
    let mut found: Vec<(Id, usize, Vec<u8>)> = Vec::new();
    for (no, start, end) in lines(bytes) {
        let line = &bytes[start..end];
        if json::blank(line) {
            continue;
        }
        let id = edge_of(line, no)?.id;
        if pairs.binary_search_by_key(&id, |(e, _)| *e).is_ok() {
            found.retain(|(f, ..)| *f != id);
            found.push((id, no, line.to_vec()));
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::string::String;

    /// graph-storage follow-up: `reset_edges` puts the name table back by content, not only by size: every
    /// relation and author name the edges brought is gone, a name nodes and edges share keeps its number with no
    /// edge use, and a second load interns them as the first did.
    #[test]
    fn reset_edges_puts_the_name_table_back_by_content() {
        let nodes = umx_core::codec::encode_nodes([
            &crate::tests::node(1, "problem", "A"),
            &crate::tests::node(2, "idea", "B"),
        ]);
        let edge = |id: u128, kind: &str, by: &str| umx_core::Edge {
            id: Id(id),
            kind: kind.into(),
            from_graph: None,
            from: Id(1),
            to_graph: None,
            to: Id(2),
            created: String::new(),
            created_by: by.into(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: Default::default(),
        };
        let mut external = edge(12, "cites", "an external author");
        external.to_graph = Some(Id(90));
        let file = umx_core::codec::encode_edges([
            &edge(10, "fresh_relation", "a new author"),
            &edge(11, "problem", ""),
            &external,
        ]);
        let names = [
            "problem",
            "idea",
            "open",
            "",
            "fresh_relation",
            "cites",
            "a new author",
            "an external author",
        ];
        let table = |g: &Graph| -> Vec<(Option<u32>, (u32, u32))> {
            names
                .iter()
                .map(|n| {
                    let at = g.names.get(n);
                    (at, at.map_or((0, 0), |at| g.names.uses(at)))
                })
                .collect()
        };
        let fresh = Graph::load_nodes(nodes.clone()).unwrap().0;
        let mut whole = fresh.clone();
        whole.load_edges(&file).unwrap();
        let mut g = fresh.clone();
        let mark = g.edges_mark();
        g.load_edges(&file).unwrap();
        assert_ne!(table(&g), table(&fresh), "the edges bring names");
        g.reset_edges(mark);
        assert_eq!(table(&g), table(&fresh));
        assert_eq!(g.names.len(), fresh.names.len());
        assert!(g.names.in_name_order().eq(fresh.names.in_name_order()));
        assert_eq!(g.edges().count(), 0);
        assert!(g.encode_edges().is_empty());
        g.load_edges(&file).unwrap();
        assert_eq!(table(&g), table(&whole));
        assert!(g.names.in_name_order().eq(whole.names.in_name_order()));
        assert_eq!(g.edges().count(), whole.edges().count());
        assert_eq!(g.encode_edges(), whole.encode_edges());
    }

    #[test]
    fn graph_id_lines_stay_aside_through_rederive_batch_and_save() {
        let reserved = format!(
            "{{ \"id\":\"{}\", \"type\":\"problem\", \"title\":\"kept\", \"status\":\"open\", \"note\":\"\", \"evidence\":[], \"status_history\":[], \"created\":\"\", \"updated\":\"\", \"created_by\":\"\", \"payload\":{{\"raw\":\"  unchanged  \"}} }}\r\n",
            Id(5)
        );
        for ids in [&[5, 7, 9][..], &[3, 5, 7], &[1, 3, 5], &[1, 5, 5, 9]] {
            let mut bytes = Vec::new();
            let mut reserved_lines = Vec::new();
            for (position, &id) in ids.iter().enumerate() {
                if id == 5 {
                    let line = reserved.replace("unchanged", &format!("unchanged {position}"));
                    bytes.extend_from_slice(line.as_bytes());
                    reserved_lines.push(line.into_bytes());
                } else {
                    bytes.extend(umx_core::codec::encode_nodes([&crate::tests::node(
                        id, "problem", "live",
                    )]));
                }
            }
            let (legacy, _) = Graph::load_nodes(bytes.clone()).unwrap();
            assert!(
                legacy.node(Id(5)).is_some(),
                "the input is valid without the graph reservation"
            );
            let (mut graph, report) =
                Graph::load_nodes_indexing_for_graph(bytes, TextFields::default(), Id(5)).unwrap();
            let kept = report.graph_id_node.unwrap();
            let lines: Vec<_> = ids
                .iter()
                .enumerate()
                .filter_map(|(i, id)| (*id == 5).then_some(i + 1))
                .collect();
            assert_eq!((kept.id, kept.lines), (Id(5), lines.clone()));
            assert!(
                report.repeats.is_empty(),
                "a reserved id is not a last-line-wins duplicate"
            );
            assert!(graph.node(Id(5)).is_none());
            assert_eq!(graph.node_count(), ids.len() - lines.len());
            let live = graph.nodes().next().unwrap().id();
            let edge = format!(
                "{{\"id\":\"{}\",\"type\":\"blocks\",\"from\":\"{}\",\"to\":\"{}\",\"created\":\"\",\"created_by\":\"\",\"custom\":\" retained \"}}\n",
                Id(50),
                Id(5),
                live
            );
            assert_eq!(graph.load_edges(edge.as_bytes()).unwrap().dangling.len(), 1);
            let mut edited = graph.node(live).unwrap().to_node();
            edited.title = "changed".into();
            graph.insert_node(edited).unwrap();
            graph
                .insert_node(crate::tests::node(0, "problem", "out of order"))
                .unwrap();
            let before_batch = graph.encode_nodes();
            {
                let mut batch = graph.batch();
                batch
                    .insert_node(crate::tests::node(2, "problem", "rolled back"))
                    .unwrap();
            }
            assert_eq!(graph.encode_nodes(), before_batch);
            assert!(graph.node(Id(5)).is_none());
            let mut batch = graph.batch();
            batch
                .insert_node(crate::tests::node(2, "problem", "batched"))
                .unwrap();
            batch.commit();
            let written = graph.encode_nodes();
            let kept_lines: Vec<_> = written
                .split_inclusive(|b| *b == b'\n')
                .filter(|line| reserved_lines.iter().any(|kept| kept.as_slice() == *line))
                .collect();
            assert_eq!(
                kept_lines,
                reserved_lines.iter().map(Vec::as_slice).collect::<Vec<_>>()
            );
            assert!(graph.node(Id(5)).is_none());
            assert_eq!(graph.encode_edges(), edge.as_bytes());
            let (mut fresh, _) =
                Graph::load_nodes_indexing_for_graph(written.clone(), TextFields::default(), Id(5))
                    .unwrap();
            assert_eq!(
                fresh
                    .load_edges(&graph.encode_edges())
                    .unwrap()
                    .dangling
                    .len(),
                1
            );
            assert_eq!(fresh.node_count(), graph.node_count());
            assert_eq!(fresh.encode_nodes(), written);
            assert!(fresh.node(Id(5)).is_none());
        }
    }

    /// Load rule 2 (workshop 008 section 5.1): a file out of id order or with a blank line loads, and the load names
    /// the first such line; a repeated id's lines next to each other are in order.
    #[test]
    fn the_first_line_out_of_order_is_reported_for_nodes_and_edges() {
        let node = |n: u128| {
            format!(
                r#"{{"id":"{}","type":"r","title":"T","status":"s","note":"","evidence":[],"status_history":[],"created":"","updated":"","created_by":""}}"#,
                Id(n).to_text()
            )
        };
        let edge = |n: u128| {
            format!(
                r#"{{"id":"{}","type":"blocks","from":"{}","to":"{}","created":"","created_by":""}}"#,
                Id(n).to_text(),
                Id(1).to_text(),
                Id(2).to_text()
            )
        };
        for (lines, unsorted) in [
            (&[1, 2, 3][..], None),
            (&[1, 2, 2, 3][..], None),
            (&[1, 3, 2][..], Some(3)),
            (&[2, 1, 3][..], Some(2)),
            (&[1, 2, 1][..], Some(3)),
            (&[1, 0, 2][..], Some(2)),
        ] {
            let text = |f: &dyn Fn(u128) -> String| -> String {
                lines
                    .iter()
                    .map(|&n| {
                        if n == 0 {
                            "  \n".into()
                        } else {
                            format!("{}\n", f(n))
                        }
                    })
                    .collect()
            };
            let nodes = text(&node);
            let (mut g, load) = Graph::load_nodes(nodes.clone().into_bytes()).unwrap();
            assert_eq!(load.unsorted, unsorted, "nodes {lines:?}");
            assert_eq!(
                load.unsorted.is_some(),
                g.encode_nodes() != nodes.as_bytes(),
                "{lines:?}"
            );
            let edges = text(&|n| edge(n + 10));
            let loaded = g.load_edges(edges.as_bytes()).unwrap();
            assert_eq!(loaded.unsorted, unsorted, "edges {lines:?}");
        }
    }

    /// Security: offsets and handles are u32, so a file past them is refused at load, not wrapped.
    #[test]
    fn a_file_past_the_u32_limits_is_refused_as_too_large() {
        let line = |n: u128| {
            let t = Id(n).to_text();
            format!(
                r#"{{"id":"{t}","type":"r","title":"T","status":"s","note":"","evidence":[],"status_history":[],"created":"","updated":"","created_by":""}}"#
            )
        };
        let file = format!("{}\n{}\n", line(1), line(2)).into_bytes();
        let err = Graph::load_nodes_within(file.clone(), file.len() - 1, TextFields::default())
            .unwrap_err();
        assert_eq!(
            (err.file, err.reason),
            (GraphFile::Nodes, DecodeReason::TooLarge)
        );
        // Unsorted: the laid-out image is checked too.
        let swapped = format!("{}\n{}", line(2), line(1)).into_bytes();
        let err =
            Graph::load_nodes_within(swapped, file.len() - 1, TextFields::default()).unwrap_err();
        assert_eq!(err.reason, DecodeReason::TooLarge);
        let (mut g, _) =
            Graph::load_nodes_within(file.clone(), file.len(), TextFields::default()).unwrap();
        let edge = |n: u128| {
            format!(
                r#"{{"id":"{}","type":"blocks","from":"{}","to":"{}","created":"","created_by":""}}"#,
                Id(n).to_text(),
                Id(1).to_text(),
                Id(2).to_text()
            )
        };
        let edges = format!("{}\n{}\n{}\n", edge(10), edge(11), edge(12));
        let err = g.load_edges_within(edges.as_bytes(), 2).unwrap_err();
        assert_eq!(
            (err.file, err.line, err.reason),
            (GraphFile::Edges, 3, DecodeReason::TooLarge)
        );
        let (mut g, _) = Graph::load_nodes(file).unwrap();
        assert_eq!(
            g.load_edges_within(edges.as_bytes(), 3)
                .map(|l| l.dangling.len()),
            Ok(0)
        );
        assert_eq!(g.edge_count(), 3);
    }
}
