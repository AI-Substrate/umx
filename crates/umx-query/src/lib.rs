//! Cypher over umx-graph (blueprint 001, c-query; workstream 005): the vendored executor (umx-cypher) reads the
//! graphs in scope through [`GraphView`], and [`run`] is the one entry. `run` is not generic, so the generic executor is
//! instantiated here once and never rebuilt with umx-graph or umx-services (workshop 006 section 2.6).
//!
//! No type of the fork crosses this crate's API: callers get [`Rows`] and [`Cell`], and every refusal and
//! warning is a [`Problem`] with a stable `query_*` code and checked slots (see [`check`]).
//! Pure: no std (blueprint 001, d-3).
#![no_std]

extern crate alloc;

pub mod check;
mod dates;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::{OnceCell, RefCell};
use core::marker::PhantomData;
use umx_core::problem::{Problem, Slot};
use umx_core::{Id, Node};
use umx_cypher::ast::{Expr, Literal};
use umx_cypher::{CypherError, CypherValue, Float, GraphProvider, Limit, Limits, PropKey, Warning};
use umx_graph::columns::{FieldId, FieldRef};
use umx_graph::{EdgeIx, Graph, Kind, NodeFilter, NodeIx, NodeSet, WholeBlock};

use check::SIMILAR_TO;

/// Node fields decoded from the node's line, not read from a column: each read costs more work units, and goes through
/// the last-node cache in `cold_field`. A resource node's locator parts other than kind and format are cold too (ws006
/// S3b; kind and format are hot). `evidence_kinds` is a column (ws003 s22).
const COLD_FIELDS: [&str; 7] = [
    "note",
    "evidence",
    "status_history",
    "locator_source",
    "locator_hash",
    "locator_length",
    "locator_seq_max",
];

/// What a cold field read costs (a column read costs 1): decoding a line is several times a column read, and a
/// query that decodes every node of a big graph must meet the budget before the wall clock (cypher-expert).
pub const COLD_FIELD_COST: u64 = 8;

/// One graph in a query's scope: its alias, and the symmetric relations of its ontology (D49 R3), which the
/// caller reads once for the query: `Ontology::symmetric_kinds()` of the graph's meta.
pub struct Part<'g> {
    pub alias: &'g str,
    pub graph: &'g Graph,
    pub symmetric: Vec<String>,
}

/// A node in a read view, never a fabricated umx-graph handle.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ViewNode {
    Stored(NodeIx),
    Graph,
}

/// Read-only graph metadata supplied by services. It is not stored in the graph.
pub struct GraphTarget<'g> {
    pub id: Id,
    pub title: &'g str,
}

/// One external step, with opaque indexes into this read's parts and their borrowed graphs.
pub struct ExternalStep {
    pub edge: At<EdgeIx>,
    pub node: At<ViewNode>,
}

/// Pure, deterministic adjacency supplied by services' incoming index. Implementations perform no I/O,
/// locking or policy lookup. Returned steps enter the executor's normal work and row budgets.
pub trait ExternalSteps {
    fn steps(
        &self,
        node: At<ViewNode>,
        incoming: bool,
        relations: Option<&[String]>,
    ) -> Vec<ExternalStep>;
    fn take_unresolved_work(&self) -> u64 {
        0
    }
    fn graph_nodes(&self) -> Vec<At<ViewNode>> {
        Vec::new()
    }
}

/// A node or edge handle in a [`GraphView`]: which of its graphs, and the handle in that graph. Handle order is
/// graph order (as the caller listed them), then id order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct At<T> {
    part: u32,
    ix: T,
}

impl<T> At<T> {
    pub fn new(part: u32, ix: T) -> Self {
        Self { part, ix }
    }
}

impl<T: Copy> At<T> {
    pub fn in_parts<'a, P>(&self, parts: &'a [P]) -> (&'a P, T) {
        (&parts[self.part as usize], self.ix)
    }
}
/// An edge of a [`GraphView`]: a stored edge record, or one entry of a graph's similar_to lists (workshop 008
/// section 9). A similar_to entry is derived: no record and no id. It is named by the node whose list holds it and
/// its place in that list (k is at most 64), which gives both ends and the score. Kept this small so an edge
/// binding is no wider than before: every binding of every row carries one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ViewEdge {
    Stored(EdgeIx),
    Similar { holder: NodeIx, slot: u8 },
    Provenance { node: NodeIx, edited: bool },
}

type ProvenanceIncoming = BTreeMap<NodeIx, Vec<(NodeIx, bool)>>;

/// The graphs a query reads, as the executor sees them. A step never leaves its graph (cross-graph edges are
/// workstream 005 s07), and a relation symmetric in any of them has no direction (D49).
pub struct GraphView<'g> {
    parts: Vec<Part<'g>>,
    include_provenance: bool,
    provenance_in: RefCell<Vec<Option<ProvenanceIncoming>>>,
    provenance_work: core::cell::Cell<u64>,
    symmetric: BTreeSet<String>,
    /// The relations some edge of the view has, read once: the executor asks for them on every match.
    relations: OnceCell<BTreeSet<String>>,
    #[cfg(test)]
    relation_edge_visits: core::cell::Cell<usize>,
    /// The s15 differential's switch: set, `each_str` holds no list, so every read takes the executor's old path.
    #[cfg(test)]
    lists_off: core::cell::Cell<bool>,
    /// Each graph's stored similar_to kind (an older graph's edge records), never followed: a step that names
    /// similar_to reads the lists (cypher-expert F4), and an untyped step never follows it.
    similar: Vec<Option<Kind<'g>>>,
    /// Each graph's whole-node block in its lists' space, for telling which nodes have no vector.
    blocks: Vec<Option<WholeBlock<'g>>>,
    /// Per graph, set while a query runs: a similar_to step read from or reached a node with no whole-node vector,
    /// so its lists could not hold that node and the answer may miss rows.
    similar_missing: RefCell<Vec<bool>>,
    /// The last node whose line was decoded for a cold field: a row reads the same node several times (`n.note` in
    /// WHERE and RETURN). One record, never a map over the query's nodes (cypher-expert).
    cold: RefCell<Option<(u32, NodeIx, Node)>>,
    /// The relation names of the step being expanded, resolved to each graph's kinds once per step and reused
    /// while the next hop asks for the same names. Keyed by the names themselves: an address can be reused by
    /// another step's list.
    kinds: RefCell<Option<StepKinds<'g>>>,
    targets: Vec<GraphTarget<'g>>,
    external: Option<&'g dyn ExternalSteps>,
    virtual_scan: core::cell::Cell<bool>,
    virtual_unknown: RefCell<BTreeSet<String>>,
    external_edges: bool,
    /// `--along` (D71 step 12): the one graph's nodes in the spine range. Set, every data path sees only these
    /// (the induced subgraph: no step leaves the range, and nothing of another graph is there); the name checks
    /// do not (cypher-expert C1). None without `--along`.
    mask: Option<(u32, NodeSet<'g>)>,
}

/// A step's relation names, and the kinds they are in each graph of the view.
type StepKinds<'g> = (Vec<String>, Vec<Vec<Kind<'g>>>);

/// A node or edge's address, `umx:<graph>/<id>`: the same text as `umx_services::address` (services' test keeps
/// the two equal; umx-query cannot depend on services).
fn address(alias: &str, id: Id) -> String {
    alloc::format!("umx:{alias}/{id}")
}

impl<'g> GraphView<'g> {
    /// A view over `parts`, in the order given (rows with no ORDER BY follow it).
    pub fn new(parts: Vec<Part<'g>>) -> GraphView<'g> {
        GraphView {
            provenance_in: RefCell::new(alloc::vec![None; parts.len()]),
            provenance_work: core::cell::Cell::new(0),
            symmetric: parts
                .iter()
                .flat_map(|p| p.symmetric.iter().cloned())
                .collect(),
            similar: parts.iter().map(|p| p.graph.kind_of(SIMILAR_TO)).collect(),
            blocks: parts
                .iter()
                .map(|p| p.graph.similar_space().and_then(|s| p.graph.whole_block(s)))
                .collect(),
            similar_missing: RefCell::new(alloc::vec![false; parts.len()]),
            relations: OnceCell::new(),
            #[cfg(test)]
            relation_edge_visits: core::cell::Cell::new(0),
            #[cfg(test)]
            lists_off: core::cell::Cell::new(false),
            parts,
            include_provenance: false,
            cold: RefCell::new(None),
            kinds: RefCell::new(None),
            targets: Vec::new(),
            external: None,
            virtual_scan: core::cell::Cell::new(false),
            virtual_unknown: RefCell::new(BTreeSet::new()),
            external_edges: false,
            mask: None,
        }
    }

    pub fn with_external(
        mut self,
        targets: Vec<GraphTarget<'g>>,
        external: &'g dyn ExternalSteps,
        external_edges: bool,
    ) -> Self {
        assert_eq!(targets.len(), self.parts.len());
        self.targets = targets;
        self.external = Some(external);
        self.external_edges = external_edges;
        self
    }

    /// A view over one graph.
    pub fn one(alias: &'g str, graph: &'g Graph, symmetric: &[&str]) -> GraphView<'g> {
        GraphView::new(alloc::vec![Part {
            alias,
            graph,
            symmetric: symmetric.iter().map(|s| (*s).to_string()).collect(),
        }])
    }

    /// Include provenance metadata; a new view shows content only.
    pub fn with_provenance(mut self, include_provenance: bool) -> Self {
        if self.include_provenance != include_provenance {
            self.relations.take();
        }
        self.include_provenance = include_provenance;
        self
    }

    /// Narrows every data path to `kept`, nodes of part `part` (D71 step 12); the graph node stays.
    pub fn with_mask(mut self, part: u32, kept: NodeSet<'g>) -> Self {
        self.mask = Some((part, kept));
        self
    }

    fn relation_names(&self) -> &BTreeSet<String> {
        self.relations.get_or_init(|| {
            let mut names = BTreeSet::new();
            for part in &self.parts {
                if self.include_provenance || part.graph.provenance_count() == 0 {
                    names.extend(part.graph.relation_kinds().into_iter().map(Kind::as_str));
                } else {
                    names.extend(
                        part.graph
                            .edges()
                            .filter(|edge| {
                                #[cfg(test)]
                                self.relation_edge_visits
                                    .set(self.relation_edge_visits.get() + 1);
                                [edge.from_ix(), edge.to_ix()]
                                    .into_iter()
                                    .flatten()
                                    .all(|ix| !part.graph.is_provenance(ix))
                            })
                            .map(|edge| edge.kind()),
                    );
                }
            }
            names.remove(SIMILAR_TO);
            if self.include_provenance {
                names.extend(["authored_by", "edited_by"]);
            }
            names.into_iter().map(str::to_string).collect()
        })
    }

    /// Whether a node is in the view for the name checks (unknown label, relation, property): provenance only, never
    /// the `--along` range, so a range with no task does not warn "unknown label task" (cypher-expert C1).
    #[inline]
    fn named_visible(&self, part: u32, node: NodeIx) -> bool {
        self.include_provenance || !self.graph(part).is_provenance(node)
    }

    /// Whether a node is in the view for data: [`Self::named_visible`] and in the `--along` range when there is one.
    #[inline]
    fn visible(&self, part: u32, node: NodeIx) -> bool {
        self.named_visible(part, node)
            && match &self.mask {
                None => true,
                Some((p, kept)) => *p == part && kept.contains_ix(node),
            }
    }

    /// Whether a graph node is in the view: always without `--along`, else only the ranged graph's.
    fn graph_visible(&self, part: u32) -> bool {
        self.mask.as_ref().is_none_or(|(p, _)| *p == part)
    }

    fn identity(&self, part: u32, id: Id) -> String {
        if self.parts.len() == 1 {
            id.to_text()
        } else {
            alloc::format!("{part}:{id}")
        }
    }

    /// Symmetric in any graph of the view.
    fn is_symmetric(&self, rel_type: &str) -> bool {
        !matches!(rel_type, "authored_by" | "edited_by") && self.symmetric.contains(rel_type)
    }

    fn graph(&self, part: u32) -> &'g Graph {
        self.parts[part as usize].graph
    }

    fn alias(&self, part: u32) -> &'g str {
        self.parts[part as usize].alias
    }

    /// The kinds of `names` that graph `part` uses, resolved for every graph once per list.
    fn kinds_of(&self, part: u32, names: &[String]) -> Vec<Kind<'g>> {
        if let Some((cached, kinds)) = &*self.kinds.borrow()
            && cached.as_slice() == names
        {
            return kinds[part as usize].clone();
        }
        let kinds: Vec<Vec<Kind<'g>>> = self
            .parts
            .iter()
            .map(|p| names.iter().filter_map(|n| p.graph.kind_of(n)).collect())
            .collect();
        let mine = kinds[part as usize].clone();
        *self.kinds.borrow_mut() = Some((names.to_vec(), kinds));
        mine
    }

    /// The similar_to steps from node `ix` of graph `part`, from its lists: out is its top k (best first), in is
    /// every node whose top k holds it (in id order). Notes a node with no vector (see `similar_missing`).
    fn similar_steps(&self, part: u32, ix: NodeIx, incoming: bool) -> Vec<(ViewEdge, NodeIx)> {
        let graph = self.graph(part);
        let list = if incoming {
            graph.similar_in(ix)
        } else {
            graph.similar_out(ix)
        };
        if !self.similar_missing.borrow()[part as usize] {
            let has = |n: NodeIx| self.blocks[part as usize].is_some_and(|b| b.has(n.index()));
            if !has(ix) || list.iter().any(|e| !has(e.other)) {
                self.similar_missing.borrow_mut()[part as usize] = true;
            }
        }
        // A slot is a place in one node's own list (at most k, at most 64, so it fits a u8). The in list is not bounded
        // by k (any number of nodes may hold one node), so it is counted as usize, never as a slot (reviewer R134-1).
        let slot_of = |held: &[umx_graph::SimilarEdge], other: NodeIx| {
            held.iter()
                .position(|h| h.other == other)
                .and_then(|i| u8::try_from(i).ok())
        };
        list.iter()
            .enumerate()
            .filter_map(|(place, e)| {
                // --along: a similar node outside the range is not in the view (D71 step 12).
                if self.mask.is_some() && !self.visible(part, e.other) {
                    return None;
                }
                let (holder, slot) = if incoming {
                    (e.other, slot_of(graph.similar_out(e.other), ix)?)
                } else {
                    (ix, u8::try_from(place).ok()?)
                };
                Some((ViewEdge::Similar { holder, slot }, e.other))
            })
            .collect()
    }

    /// A similar_to entry's ends and score: (from, to, score in ten-thousandths).
    fn similar_entry(&self, part: u32, holder: NodeIx, slot: u8) -> (NodeIx, NodeIx, i32) {
        let e = self.graph(part).similar_out(holder)[slot as usize];
        (holder, e.other, e.score)
    }

    fn cold_field(&self, part: u32, ix: NodeIx, prop: &str) -> Option<umx_core::Value> {
        let mut cold = self.cold.borrow_mut();
        if !matches!(&*cold, Some((p, n, _)) if *p == part && *n == ix) {
            *cold = Some((part, ix, self.graph(part).node_at(ix).to_node()));
        }
        cold.as_ref().and_then(|(_, _, record)| record.field(prop))
    }

    fn provenance_entry(&self, part: u32, node: NodeIx, edited: bool) -> Option<(NodeIx, NodeIx)> {
        self.graph(part)
            .node_provenance_edges(node)
            .find(|edge| (edge.kind == "edited_by") == edited)
            .map(|edge| (edge.from.ix(), edge.to.ix()))
    }

    fn provenance_steps(
        &self,
        part: u32,
        node: NodeIx,
        incoming: bool,
        names: &[String],
    ) -> Vec<(ViewEdge, NodeIx)> {
        if !self.include_provenance
            || !names
                .iter()
                .any(|name| matches!(name.as_str(), "authored_by" | "edited_by"))
        {
            return Vec::new();
        }
        let wanted = |edited| {
            names
                .iter()
                .any(|name| name == if edited { "edited_by" } else { "authored_by" })
        };
        if !incoming {
            return self
                .graph(part)
                .node_provenance_edges(node)
                .filter_map(|edge| {
                    let edited = edge.kind == "edited_by";
                    wanted(edited).then_some((ViewEdge::Provenance { node, edited }, edge.to.ix()))
                })
                .collect();
        }
        let mut incoming = self.provenance_in.borrow_mut();
        let index = incoming[part as usize].get_or_insert_with(|| {
            let graph = self.graph(part);
            let mut work = graph.node_count() as u64;
            let mut index: BTreeMap<NodeIx, Vec<(NodeIx, bool)>> = BTreeMap::new();
            for edge in graph.provenance_edges() {
                work = work.saturating_add(1);
                index
                    .entry(edge.to.ix())
                    .or_default()
                    .push((edge.from.ix(), edge.kind == "edited_by"));
            }
            self.provenance_work
                .set(self.provenance_work.get().saturating_add(work));
            index
        });
        index
            .get(&node)
            .into_iter()
            .flatten()
            .filter_map(|&(from, edited)| {
                wanted(edited).then_some((ViewEdge::Provenance { node: from, edited }, from))
            })
            .collect()
    }

    /// A result value as a [`Cell`]: a whole node or edge becomes umx's shape for it.
    fn cell(&self, v: CypherValue) -> Cell {
        match v {
            CypherValue::Null => Cell::Null,
            CypherValue::Bool(b) => Cell::Bool(b),
            CypherValue::Int(i) => Cell::Int(i),
            CypherValue::Float(f) => Cell::Float(f.0),
            CypherValue::Str(s) => Cell::Str(s),
            CypherValue::Node { id, .. } => self.node_cell(&id).map_or(Cell::Null, Cell::Node),
            CypherValue::Relationship { id, .. } => self
                .provenance_cell(&id)
                .map(Cell::Provenance)
                .or_else(|| self.similar_cell(&id).map(Cell::Similar))
                .or_else(|| self.edge_cell(&id).map(Cell::Edge))
                .unwrap_or(Cell::Null),
            CypherValue::List(items) => {
                Cell::List(items.into_iter().map(|v| self.cell(v)).collect())
            }
            CypherValue::Map(entries) => Cell::Map(
                entries
                    .into_iter()
                    .map(|(k, v)| (k, self.cell(v)))
                    .collect(),
            ),
        }
    }

    /// `<part>:<id>`, or the bare id in a one-graph view, as [`GraphProvider::node_id`] made it.
    fn split(&self, handle: &str) -> Option<(u32, Id)> {
        let (part, id) = handle.split_once(':').unwrap_or(("0", handle));
        let part: u32 = part.parse().ok()?;
        if part as usize >= self.parts.len() {
            return None;
        }
        Some((part, id.parse().ok()?))
    }

    fn node_cell(&self, handle: &str) -> Option<NodeCell> {
        let (part, id) = self.split(handle)?;
        if self
            .targets
            .get(part as usize)
            .is_some_and(|target| target.id == id)
        {
            let target = &self.targets[part as usize];
            return Some(NodeCell {
                graph: self.alias(part).into(),
                address: alloc::format!("umx:{}", self.alias(part)),
                id,
                kind: "graph".into(),
                title: if target.title.is_empty() {
                    self.alias(part)
                } else {
                    target.title
                }
                .into(),
                status: String::new(),
            });
        }
        let n = self.graph(part).node(id)?;
        if !self.visible(part, n.ix()) {
            return None;
        }
        Some(NodeCell {
            graph: self.alias(part).to_string(),
            address: address(self.alias(part), id),
            id,
            kind: n.kind().to_string(),
            title: n.title().to_string(),
            status: n.status().to_string(),
        })
    }

    /// A similar_to entry's handle, `<from>><to>` (after `<part>:` in a view of several graphs), as its cell.
    fn similar_cell(&self, handle: &str) -> Option<SimilarCell> {
        let (part, ends) = match handle.split_once(':') {
            Some((part, ends)) => (part.parse::<u32>().ok()?, ends),
            None => (0, handle),
        };
        let (from, to) = ends.split_once('>')?;
        let (from, to): (Id, Id) = (from.parse().ok()?, to.parse().ok()?);
        let graph = self.parts.get(part as usize)?.graph;
        let to_ix = graph.node_ix(to)?;
        let entry = graph
            .similar_out(graph.node_ix(from)?)
            .iter()
            .find(|e| e.other == to_ix)?;
        Some(SimilarCell {
            graph: self.alias(part).to_string(),
            from,
            to,
            score: entry.score,
        })
    }

    fn edge_cell(&self, handle: &str) -> Option<EdgeCell> {
        let (part, id) = self.split(handle)?;
        let e = self.graph(part).edge(id)?;
        if [e.from_ix(), e.to_ix()]
            .into_iter()
            .flatten()
            .any(|ix| !self.visible(part, ix))
        {
            return None;
        }
        Some(EdgeCell {
            graph: self.alias(part).to_string(),
            address: address(self.alias(part), id),
            id,
            kind: e.kind().to_string(),
            from: e.from(),
            to: e.to(),
            from_selector: e.field("from_selector"),
            to_selector: e.field("to_selector"),
        })
    }

    fn provenance_cell(&self, handle: &str) -> Option<ProvenanceCell> {
        let (part, handle) = handle.split_once(':').unwrap_or(("0", handle));
        let part: u32 = part.parse().ok()?;
        let (kind, from) = handle.strip_prefix("p/")?.split_once('/')?;
        let edited = match kind {
            "edited_by" => true,
            "authored_by" => false,
            _ => return None,
        };
        let graph = self.parts.get(part as usize)?.graph;
        let (from, to) = self.provenance_entry(part, graph.node_ix(from.parse().ok()?)?, edited)?;
        Some(ProvenanceCell {
            graph: self.alias(part).into(),
            kind: kind.into(),
            from: graph.node_at(from).id(),
            to: graph.node_at(to).id(),
        })
    }
}

/// A field value as the executor sees it. Integers stay integers; other numbers are floats.
fn cypher_value(v: &umx_core::Value) -> CypherValue {
    use umx_core::Value as J;
    match v {
        J::Null => CypherValue::Null,
        J::Bool(b) => CypherValue::Bool(*b),
        J::Number(n) => match n.as_i64() {
            Some(i) => CypherValue::Int(i),
            None => CypherValue::Float(Float(n.as_f64().unwrap_or(f64::NAN))),
        },
        J::String(s) => CypherValue::Str(s.clone()),
        J::Array(items) => CypherValue::List(items.iter().map(cypher_value).collect()),
        J::Object(map) => CypherValue::Map(
            map.iter()
                .map(|(k, v)| (k.clone(), cypher_value(v)))
                .collect(),
        ),
    }
}

/// A custom field value straight from its column, as `cypher_value` reads the same value through JSON: an unsigned
/// integer past `i64::MAX` is a float, as serde's `as_i64`-then-`as_f64` gives it.
fn field_ref_value(v: FieldRef<'_>) -> CypherValue {
    match v {
        FieldRef::UInt(u) => {
            i64::try_from(u).map_or(CypherValue::Float(Float(u as f64)), CypherValue::Int)
        }
        FieldRef::Int(i) => CypherValue::Int(i),
        FieldRef::Float(x) => CypherValue::Float(Float(x)),
        FieldRef::Bool(b) => CypherValue::Bool(b),
        FieldRef::Str(s) => CypherValue::Str(s.into_owned()),
        FieldRef::Other(v) => cypher_value(&v),
    }
}

/// One custom field name's handle in each graph of the view, by part: each graph interns its own names.
type FieldIds = Vec<Option<FieldId>>;

/// A node property name resolved once per query (ws005 s14), so a read matches no name. Resolved in the order
/// `property` matched names before: `address`, `graph`, the locator parts, the cold fields, `NodeRef::field`'s
/// standard names, then a custom field.
enum Prop {
    Address,
    Graph,
    /// A locator part: null on a plain node (from the hot kind column). A cold one is decoded from the line through
    /// the last-node cache; the rest (kind and format hot) are `NodeRef::field`'s.
    Locator {
        name: &'static str,
        cold: bool,
    },
    Cold(&'static str),
    Id,
    Type,
    Title,
    Status,
    Created,
    Updated,
    CreatedBy,
    /// The session column, else a custom field of that name, as `NodeRef::field` falls back.
    Authored(FieldIds),
    Edited(FieldIds),
    EvidenceKinds,
    /// Each live evidence pointer's source as written, in pointer order; null when there is none. Decoded from the
    /// line, as `evidence` is.
    EvidenceSources,
    /// `tags` (ws008 t2b): in a std graph the canonical list from its tag index, so `'x' IN n.tags` matches a
    /// hand-edited value as a tag filter does; in a graph without std an ordinary custom field.
    Tags(FieldIds),
    Custom(FieldIds),
}

impl Prop {
    fn of(view: &GraphView<'_>, name: &str) -> Prop {
        let fields = || view.parts.iter().map(|p| p.graph.field_id(name)).collect();
        let cold = COLD_FIELDS.iter().find(|c| **c == name);
        if let Some(&locator) = umx_core::locator::LOCATOR_NAMES
            .iter()
            .find(|l| **l == name)
        {
            return Prop::Locator {
                name: locator,
                cold: cold.is_some(),
            };
        }
        if let Some(&cold) = cold {
            return Prop::Cold(cold);
        }
        match name {
            "address" => Prop::Address,
            "graph" => Prop::Graph,
            "id" => Prop::Id,
            "type" => Prop::Type,
            "title" => Prop::Title,
            "status" => Prop::Status,
            "created" => Prop::Created,
            "updated" => Prop::Updated,
            "created_by" => Prop::CreatedBy,
            "authored_by_session" => Prop::Authored(fields()),
            "edited_by_session" => Prop::Edited(fields()),
            "evidence_kinds" => Prop::EvidenceKinds,
            "evidence_sources" => Prop::EvidenceSources,
            umx_core::tags::TAGS => Prop::Tags(fields()),
            _ => Prop::Custom(fields()),
        }
    }

    /// The work one read costs: a field decoded from the line costs `COLD_FIELD_COST`, a column read 1.
    fn cost(&self) -> u64 {
        match self {
            Prop::Cold(_) | Prop::Locator { cold: true, .. } | Prop::EvidenceSources => {
                COLD_FIELD_COST
            }
            _ => 1,
        }
    }
}

/// How a provider names nodes and edges: one graph's own handles ([`One`], the common case and the fastest, as s02
/// had), or (graph, handle) across graphs ([`Many`]). The executor is built once for each.
trait Space {
    type Node: Copy + Ord;
    type Edge: Copy + Ord;
    const EXTERNAL: bool;
    fn node(part: u32, ix: ViewNode) -> Self::Node;
    fn edge(part: u32, ix: ViewEdge) -> Self::Edge;
    fn node_at(n: Self::Node) -> (u32, ViewNode);
    fn edge_at(e: Self::Edge) -> (u32, ViewEdge);
}

/// One graph: a handle is the graph's own (graph-storage-expert: no wider handle where one graph is read).
struct One;

impl Space for One {
    const EXTERNAL: bool = false;
    type Node = NodeIx;
    type Edge = ViewEdge;
    fn node(_: u32, ix: ViewNode) -> NodeIx {
        let ViewNode::Stored(ix) = ix else {
            unreachable!("One contains only stored nodes")
        };
        ix
    }
    fn edge(_: u32, ix: ViewEdge) -> ViewEdge {
        ix
    }
    fn node_at(n: NodeIx) -> (u32, ViewNode) {
        (0, ViewNode::Stored(n))
    }
    fn edge_at(e: ViewEdge) -> (u32, ViewEdge) {
        (0, e)
    }
}

/// Several graphs: a handle carries its graph.
struct Many;

impl Space for Many {
    const EXTERNAL: bool = true;
    type Node = At<ViewNode>;
    type Edge = At<ViewEdge>;
    fn node(part: u32, ix: ViewNode) -> At<ViewNode> {
        At { part, ix }
    }
    fn edge(part: u32, ix: ViewEdge) -> At<ViewEdge> {
        At { part, ix }
    }
    fn node_at(n: At<ViewNode>) -> (u32, ViewNode) {
        (n.part, n.ix)
    }
    fn edge_at(e: At<ViewEdge>) -> (u32, ViewEdge) {
        (e.part, e.ix)
    }
}

/// The executor's view of a [`GraphView`] in one handle space. Built per query, so it holds the query's resolved
/// property names and its one instant.
struct Provider<'v, 'g, S> {
    view: &'v GraphView<'g>,
    space: PhantomData<S>,
    /// Each name `property_key` resolved, at its key's index.
    slots: RefCell<Vec<(String, Prop)>>,
    /// [`RunOptions::now_ms`]: what `current_date()` and `current_timestamp()` answer on every row.
    now_ms: Option<u64>,
}

impl<'v, 'g, S> Provider<'v, 'g, S> {
    fn new(view: &'v GraphView<'g>, now_ms: Option<u64>) -> Self {
        Provider {
            view,
            space: PhantomData,
            slots: RefCell::new(Vec::new()),
            now_ms,
        }
    }
}

impl<S: Space> Provider<'_, '_, S> {
    /// Property `name` of `node`, read through `slot` (resolved from `name`): the one read path behind `property` and
    /// `property_at`. A virtual :graph node answers by name.
    fn read(&self, node: S::Node, name: &str, slot: &Prop) -> CypherValue {
        let (part, ix) = S::node_at(node);
        let ix = match ix {
            ViewNode::Stored(ix) => ix,
            ViewNode::Graph => return self.graph_node_property(part, name),
        };
        let n = self.view.graph(part).node_at(ix);
        let text = |s: &str| CypherValue::Str(s.into());
        let id = |id: Id| CypherValue::Str(id.to_text());
        let json = |v: Option<umx_core::Value>| v.map_or(CypherValue::Null, |v| cypher_value(&v));
        let custom = |fields: &FieldIds| {
            fields[part as usize]
                .and_then(|f| n.field_ref(f))
                .map_or(CypherValue::Null, field_ref_value)
        };
        match slot {
            Prop::Address => CypherValue::Str(address(self.view.alias(part), n.id())),
            Prop::Graph => text(self.view.alias(part)),
            // A plain node has no locator parts: answer null from the hot kind column, never decoding its line.
            Prop::Locator { .. } if self.view.graph(part).locator_kind(ix).is_none() => {
                CypherValue::Null
            }
            Prop::Locator { name, cold: true } | Prop::Cold(name) => {
                json(self.view.cold_field(part, ix, name))
            }
            Prop::Locator { name, cold: false } => json(n.field(name)),
            Prop::Id => id(n.id()),
            Prop::Type => text(n.kind()),
            Prop::Title => text(n.title()),
            Prop::Status => text(n.status()),
            Prop::Created => CypherValue::Str(n.created()),
            Prop::Updated => CypherValue::Str(n.updated()),
            Prop::CreatedBy => text(n.created_by()),
            Prop::Authored(fields) => n.authored_by_session().map_or_else(|| custom(fields), id),
            Prop::Edited(fields) => n.edited_by_session().map_or_else(|| custom(fields), id),
            Prop::EvidenceKinds => CypherValue::List(n.evidence_kinds().map(text).collect()),
            Prop::EvidenceSources => match self.view.cold_field(part, ix, "evidence") {
                Some(umx_core::Value::Array(list)) => evidence_sources(&list),
                _ => CypherValue::Null,
            },
            Prop::Tags(_) if self.view.graph(part).std_tags() => {
                let tags = self.view.graph(part).tags_of(n);
                if tags.is_empty() {
                    CypherValue::Null
                } else {
                    CypherValue::List(tags.into_iter().map(text).collect())
                }
            }
            Prop::Tags(fields) | Prop::Custom(fields) => custom(fields),
        }
    }

    /// A virtual :graph node's property: its target's id, address, type, title and alias; any other name is null
    /// and noted for the unknown-property warning.
    fn graph_node_property(&self, part: u32, prop: &str) -> CypherValue {
        let target = &self.view.targets[part as usize];
        match prop {
            "id" => CypherValue::Str(target.id.to_text()),
            "address" => CypherValue::Str(alloc::format!("umx:{}", self.view.alias(part))),
            "type" => CypherValue::Str("graph".into()),
            "title" => CypherValue::Str(
                if target.title.is_empty() {
                    self.view.alias(part)
                } else {
                    target.title
                }
                .into(),
            ),
            "alias" => CypherValue::Str(self.view.alias(part).into()),
            _ => {
                self.view.virtual_unknown.borrow_mut().insert(prop.into());
                CypherValue::Null
            }
        }
    }
}

/// `evidence_sources` (who cites a source): each pointer's `source` as stored, in pointer order, null when none
/// has one, as a node with no tags reads null. A redacted entry has no source and adds nothing; removed pointers and
/// status-move evidence live elsewhere and are never read here.
fn evidence_sources(evidence: &[umx_core::Value]) -> CypherValue {
    let sources: Vec<CypherValue> = evidence
        .iter()
        .filter_map(|e| e.get("source")?.as_str())
        .map(|s| CypherValue::Str(s.into()))
        .collect();
    if sources.is_empty() {
        CypherValue::Null
    } else {
        CypherValue::List(sources)
    }
}

/// A similar_to score as Cypher reads it (`s.score`): ten-thousandths as a float, so 8000 reads as 0.8 exactly as
/// the literal does.
fn score_value(units: i32) -> CypherValue {
    CypherValue::Float(Float(f64::from(units) / 10_000.0))
}

impl<S: Space> GraphProvider for Provider<'_, '_, S> {
    type NodeId = S::Node;
    type EdgeId = S::Edge;

    /// Graph by graph, through each graph's type column (interned names), one list per label merged in handle
    /// order.
    fn scan(&self, labels: &[String]) -> Vec<S::Node> {
        let mut out = Vec::new();
        for (part, p) in (0u32..).zip(&self.view.parts) {
            if labels.is_empty() {
                out.extend(
                    p.graph
                        .nodes()
                        .filter(|n| self.view.visible(part, n.ix()))
                        .map(|n| S::node(part, ViewNode::Stored(n.ix()))),
                );
                continue;
            }
            if labels.iter().any(|label| label == "graph")
                && self.view.targets.get(part as usize).is_some()
            {
                out.push(S::node(part, ViewNode::Graph));
            }
            let mut nodes: Vec<NodeIx> = labels
                .iter()
                .flat_map(|l| {
                    p.graph.list_nodes(&NodeFilter {
                        kind: Some(l.clone()),
                        ..Default::default()
                    })
                })
                .filter(|n| self.view.visible(part, n.ix()))
                .map(|n| n.ix())
                .collect();
            nodes.sort_unstable();
            nodes.dedup();
            out.extend(
                nodes
                    .into_iter()
                    .map(|ix| S::node(part, ViewNode::Stored(ix))),
            );
        }
        out
    }

    fn scan_step_start(&self, labels: &[String]) -> Vec<S::Node> {
        let mut nodes = self.scan(labels);
        if S::EXTERNAL
            && labels.is_empty()
            && let Some(external) = self.view.external
        {
            nodes.extend(
                external
                    .graph_nodes()
                    .into_iter()
                    .map(|n| S::node(n.part, n.ix)),
            );
        }
        nodes
    }

    fn matches_label(&self, node: S::Node, label: &str) -> bool {
        let (part, ix) = S::node_at(node);
        match ix {
            ViewNode::Stored(ix) => self.view.graph(part).node_at(ix).kind() == label,
            ViewNode::Graph => label == "graph",
        }
    }

    fn relationship_types(&self) -> Vec<String> {
        self.view.relation_names().iter().cloned().collect()
    }

    fn has_relationship_type(&self, name: &str) -> bool {
        self.view.relation_names().contains(name)
    }

    /// similar_to: an untyped step never follows it; a step that names it reads the lists.
    fn named_only_types(&self) -> Vec<String> {
        alloc::vec![
            SIMILAR_TO.to_string(),
            "authored_by".into(),
            "edited_by".into()
        ]
    }

    fn expand(&self, node: S::Node, rel_type: &str) -> Vec<(S::Edge, S::Node)> {
        self.expand_types(node, Some(&[rel_type.to_string()]), false)
    }

    fn expand_in(&self, node: S::Node, rel_type: &str) -> Vec<(S::Edge, S::Node)> {
        self.expand_types(node, Some(&[rel_type.to_string()]), true)
    }

    /// A named similar_to first, from the lists (as `umx neighbours` lists them); then one pass over the node's
    /// adjacency in its graph, in edge id order, comparing that graph's kinds (resolved once per step). A stored
    /// similar_to record is never followed, named or not.
    fn expand_types(
        &self,
        node: S::Node,
        rel_types: Option<&[String]>,
        incoming: bool,
    ) -> Vec<(S::Edge, S::Node)> {
        let (part, ix) = S::node_at(node);
        let graph = self.view.graph(part);
        let external = || {
            if !S::EXTERNAL
                || rel_types.is_some_and(|names| names.iter().all(|name| name == SIMILAR_TO))
            {
                return Vec::new();
            }
            self.view
                .external
                .map(|external| {
                    external
                        .steps(At::new(part, ix), incoming, rel_types)
                        .into_iter()
                        .filter(|step| {
                            let edge = self.view.graph(step.edge.part).edge_at(step.edge.ix);
                            // P2: a hidden provenance far end is not in the view, so the step is not either; under
                            // --along neither is a node or graph outside the ranged graph (D71 step 12).
                            let far_visible = match step.node.ix {
                                ViewNode::Stored(far) => self.view.visible(step.node.part, far),
                                ViewNode::Graph => self.view.graph_visible(step.node.part),
                            };
                            far_visible
                                && edge.kind() != SIMILAR_TO
                                && rel_types.is_none_or(|names| {
                                    names.iter().any(|name| name == edge.kind())
                                })
                        })
                        .map(|step| {
                            (
                                S::edge(step.edge.part, ViewEdge::Stored(step.edge.ix)),
                                S::node(step.node.part, step.node.ix),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let ViewNode::Stored(ix) = ix else {
            return external();
        };
        if !self.view.visible(part, ix) {
            return Vec::new();
        }
        let similar = self.view.similar[part as usize];
        let mut out = Vec::new();
        if let Some(names) = rel_types {
            out.extend(
                self.view
                    .provenance_steps(part, ix, incoming, names)
                    .into_iter()
                    .map(|(edge, other)| {
                        (S::edge(part, edge), S::node(part, ViewNode::Stored(other)))
                    }),
            );
        }
        if rel_types.is_some_and(|names| names.iter().any(|n| n == SIMILAR_TO)) {
            out.extend(
                self.view
                    .similar_steps(part, ix, incoming)
                    .into_iter()
                    .map(|(e, other)| (S::edge(part, e), S::node(part, ViewNode::Stored(other))))
                    .collect::<Vec<_>>(),
            );
        }
        let wanted = rel_types.map(|names| self.view.kinds_of(part, names));
        if wanted
            .as_ref()
            .is_some_and(|kinds| kinds.iter().all(|k| Some(*k) == similar))
        {
            out.extend(external());
            return out;
        }
        let keep = |k: Kind<'_>| {
            Some(k) != similar
                && !matches!(k.as_str(), "authored_by" | "edited_by")
                && match &wanted {
                    Some(kinds) => kinds.contains(&k),
                    None => true,
                }
        };
        let steps: Vec<_> = if incoming {
            graph.edges_in(ix).collect()
        } else {
            graph.edges_out(ix).collect()
        };
        out.extend(
            steps
                .into_iter()
                .filter(|s| keep(s.kind) && self.view.visible(part, s.other))
                .map(|s| {
                    (
                        S::edge(part, ViewEdge::Stored(s.edge)),
                        S::node(part, ViewNode::Stored(s.other)),
                    )
                }),
        );
        out.extend(external());
        out
    }

    fn take_expansion_work(&self) -> u64 {
        let external = if S::EXTERNAL {
            self.view
                .external
                .map_or(0, ExternalSteps::take_unresolved_work)
        } else {
            0
        };
        external.saturating_add(self.view.provenance_work.replace(0))
    }

    /// Symmetric in any graph of the view: a directed step naming it is refused (D49 R1).
    fn is_symmetric(&self, rel_type: &str) -> bool {
        self.view.is_symmetric(rel_type)
    }

    /// Symmetric in the edge's own graph: only then does an untyped directed step that crosses it warn (D49 R2,
    /// reviewer R127-1).
    fn edge_is_symmetric(&self, edge: S::Edge) -> bool {
        let (part, ViewEdge::Stored(ix)) = S::edge_at(edge) else {
            return false;
        };
        let kind = self.view.graph(part).edge_at(ix).kind();
        self.view.parts[part as usize]
            .symmetric
            .iter()
            .any(|s| s == kind)
    }

    fn indexed(&self, prop: &str) -> bool {
        prop == "id" && !self.view.virtual_scan.get()
    }

    /// `id` through each graph's id index. The text must be the id exactly as `n.id` reads (upper case, 26
    /// characters), so the answer equals a scan's.
    fn lookup(&self, prop: &str, value: &CypherValue) -> Option<Vec<S::Node>> {
        if prop != "id" {
            return None;
        }
        let CypherValue::Str(text) = value else {
            return Some(Vec::new());
        };
        let Some(id) = text.parse::<Id>().ok().filter(|id| id.to_text() == *text) else {
            return Some(Vec::new());
        };
        Some(
            (0u32..)
                .zip(&self.view.parts)
                .filter_map(|(part, p)| {
                    p.graph
                        .node_ix(id)
                        .filter(|ix| self.view.visible(part, *ix))
                        .map(|ix| S::node(part, ViewNode::Stored(ix)))
                })
                .collect(),
        )
    }

    fn lookup_step_start(&self, prop: &str, value: &CypherValue) -> Option<Vec<S::Node>> {
        let mut nodes = self.lookup(prop, value)?;
        if S::EXTERNAL
            && prop == "id"
            && let Some(external) = self.view.external
        {
            nodes.extend(
                external
                    .graph_nodes()
                    .into_iter()
                    .filter(|n| {
                        *value == CypherValue::Str(self.view.targets[n.part as usize].id.to_text())
                    })
                    .map(|n| S::node(n.part, n.ix)),
            );
        }
        Some(nodes)
    }

    fn property_cost(&self, prop: &str) -> u64 {
        Prop::of(self.view, prop).cost()
    }

    fn property_key(&self, prop: &str) -> Option<PropKey> {
        let mut slots = self.slots.borrow_mut();
        let key = u32::try_from(slots.len()).ok()?;
        slots.push((prop.into(), Prop::of(self.view, prop)));
        Some(PropKey(key))
    }

    fn key_cost(&self, key: PropKey) -> u64 {
        self.slots.borrow()[key.0 as usize].1.cost()
    }

    fn property_at(&self, node: S::Node, key: PropKey) -> CypherValue {
        let slots = self.slots.borrow();
        let (name, slot) = &slots[key.0 as usize];
        self.read(node, name, slot)
    }

    /// evidence_kinds of a stored node, from its column without building the list (ws005 s15). Every other slot, and
    /// a virtual :graph node (which notes the name as unknown), is read by `property_at`.
    fn each_str(
        &self,
        node: S::Node,
        key: PropKey,
        visit: &mut dyn FnMut(&str) -> core::ops::ControlFlow<()>,
    ) -> bool {
        #[cfg(test)]
        if self.view.lists_off.get() {
            return false;
        }
        let tags = match self.slots.borrow()[key.0 as usize].1 {
            Prop::EvidenceKinds => false,
            Prop::Tags(_) => true,
            _ => return false,
        };
        let (part, ViewNode::Stored(ix)) = S::node_at(node) else {
            return false;
        };
        let g = self.view.graph(part);
        if tags {
            // ws008 t2b: a std graph's tags from its index, as `property_at` reads them; any other graph reads the field.
            let held = g.tags_of(g.node_at(ix));
            // A node with no tags reads null, as `property_at` gives it, never an empty list.
            if !g.std_tags() || held.is_empty() {
                return false;
            }
            for tag in held {
                if visit(tag).is_break() {
                    break;
                }
            }
            return true;
        }
        for kind in g.node_at(ix).evidence_kinds() {
            if visit(kind).is_break() {
                break;
            }
        }
        true
    }

    /// Two dates compare by instant (ws008 TM2), through the one rule `--where` uses.
    fn compare_text(&self, a: &str, b: &str) -> Option<core::cmp::Ordering> {
        umx_core::value::date_relation(a, b)
    }

    /// A date written other than canonically sorts at its instant (ws008 TM2); every other string as itself.
    fn text_sort_key(&self, s: &str) -> Option<alloc::string::String> {
        umx_core::value::text_sort_key(s)
    }

    /// The date functions (D71 step 10), over `umx_core::time`.
    fn has_function(&self, name: &str) -> bool {
        dates::NAMES.contains(&name)
    }

    fn function(&self, name: &str, args: &[CypherValue]) -> Result<CypherValue, String> {
        dates::call(name, args, self.now_ms)
    }

    /// A node's field; `address` and `graph` answer as read-only fields, as in the node's shape (cypher-expert F3).
    fn property(&self, node: S::Node, prop: &str) -> CypherValue {
        self.read(node, prop, &Prop::of(self.view, prop))
    }

    /// A stored edge's field; a similar_to entry answers `score`, its ends, `type` and `graph`, and null for `id`
    /// and `address` (it is not a record).
    fn edge_property(&self, edge: S::Edge, prop: &str) -> CypherValue {
        let (part, edge) = S::edge_at(edge);
        let graph = self.view.graph(part);
        let ix = match edge {
            ViewEdge::Stored(ix) => ix,
            ViewEdge::Provenance { node, edited } => {
                let Some((from, to)) = self.view.provenance_entry(part, node, edited) else {
                    return CypherValue::Null;
                };
                return match prop {
                    "from" => CypherValue::Str(graph.node_at(from).id().to_text()),
                    "to" => CypherValue::Str(graph.node_at(to).id().to_text()),
                    "type" => {
                        CypherValue::Str(if edited { "edited_by" } else { "authored_by" }.into())
                    }
                    "graph" => CypherValue::Str(self.view.alias(part).into()),
                    "derived" => CypherValue::Bool(true),
                    _ => CypherValue::Null,
                };
            }
            ViewEdge::Similar { holder, slot } => {
                let (from, to, score) = self.view.similar_entry(part, holder, slot);
                return match prop {
                    "score" => score_value(score),
                    "from" => CypherValue::Str(graph.node_at(from).id().to_text()),
                    "to" => CypherValue::Str(graph.node_at(to).id().to_text()),
                    "type" => CypherValue::Str(SIMILAR_TO.to_string()),
                    "graph" => CypherValue::Str(self.view.alias(part).to_string()),
                    _ => CypherValue::Null,
                };
            }
        };
        let e = graph.edge_at(ix);
        match prop {
            "address" => CypherValue::Str(address(self.view.alias(part), e.id())),
            "graph" => CypherValue::Str(self.view.alias(part).to_string()),
            "evidence_sources" => evidence_sources(&e.evidence()),
            _ => e
                .field(prop)
                .map_or(CypherValue::Null, |v| cypher_value(&v)),
        }
    }

    fn edge_label(&self, edge: S::Edge) -> String {
        match S::edge_at(edge) {
            (part, ViewEdge::Stored(ix)) => self.view.graph(part).edge_at(ix).kind().to_string(),
            (_, ViewEdge::Similar { .. }) => SIMILAR_TO.to_string(),
            (_, ViewEdge::Provenance { edited, .. }) => {
                if edited { "edited_by" } else { "authored_by" }.into()
            }
        }
    }

    /// The edge's identity for equality and grouping, never shown: see [`GraphProvider::node_id`]. A similar_to
    /// entry's is `<from>><to>`, which no id can be.
    fn edge_id(&self, edge: S::Edge) -> String {
        let (part, edge) = S::edge_at(edge);
        let graph = self.view.graph(part);
        match edge {
            ViewEdge::Stored(ix) => self.view.identity(part, graph.edge_at(ix).id()),
            ViewEdge::Provenance { node, edited } => {
                let handle = alloc::format!(
                    "p/{}/{}",
                    if edited { "edited_by" } else { "authored_by" },
                    graph.node_at(node).id()
                );
                if self.view.parts.len() == 1 {
                    handle
                } else {
                    alloc::format!("{part}:{handle}")
                }
            }
            ViewEdge::Similar { holder, slot } => {
                let (from, to, _) = self.view.similar_entry(part, holder, slot);
                let ends =
                    alloc::format!("{}>{}", graph.node_at(from).id(), graph.node_at(to).id());
                if self.view.parts.len() == 1 {
                    ends
                } else {
                    alloc::format!("{part}:{ends}")
                }
            }
        }
    }

    /// The node's identity for equality and grouping, never shown: `<graph index>:<id>` when the view has several
    /// graphs, so the same id in two graphs is two nodes; the bare id in a one-graph view, which compares nodes
    /// once per row (`a <> z`) and must stay cheap.
    fn node_id(&self, node: S::Node) -> String {
        let (part, ix) = S::node_at(node);
        let id = match ix {
            ViewNode::Stored(ix) => self.view.graph(part).node_at(ix).id(),
            ViewNode::Graph => self.view.targets[part as usize].id,
        };
        self.view.identity(part, id)
    }

    fn label(&self, node: S::Node) -> String {
        let (part, ix) = S::node_at(node);
        match ix {
            ViewNode::Stored(ix) => self.view.graph(part).node_at(ix).kind().to_string(),
            ViewNode::Graph => "graph".into(),
        }
    }

    fn name(&self, node: S::Node) -> String {
        let (part, ix) = S::node_at(node);
        match ix {
            ViewNode::Stored(ix) => self.view.graph(part).node_at(ix).title().to_string(),
            ViewNode::Graph => {
                let title = self.view.targets[part as usize].title;
                if title.is_empty() {
                    self.view.alias(part)
                } else {
                    title
                }
                .into()
            }
        }
    }
}

/// A whole node in a result: umx's node shape (cypher-expert section 1.2). Name a field (`n.note`) for more.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeCell {
    pub id: Id,
    pub address: String,
    pub graph: String,
    pub kind: String,
    pub title: String,
    pub status: String,
}

/// A whole edge in a result: umx's edge shape, with its selectors when set.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeCell {
    pub id: Id,
    pub address: String,
    pub graph: String,
    pub kind: String,
    pub from: Id,
    pub to: Id,
    pub from_selector: Option<umx_core::Value>,
    pub to_selector: Option<umx_core::Value>,
}

/// A whole similar_to edge in a result (workshop 008 section 9): derived from the graph's lists, so it has no id or
/// address; `score` is in ten-thousandths (format it with `umx_core::similar::format_units`).
#[derive(Clone, Debug, PartialEq)]
pub struct SimilarCell {
    pub graph: String,
    pub from: Id,
    pub to: Id,
    pub score: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProvenanceCell {
    pub graph: String,
    pub kind: String,
    pub from: Id,
    pub to: Id,
}

/// One result value.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Cell>),
    Map(Vec<(String, Cell)>),
    Node(NodeCell),
    Edge(EdgeCell),
    Similar(SimilarCell),
    Provenance(ProvenanceCell),
}

/// The fork counts positions in characters; query_syntax reports the byte.
fn byte_at(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(b, _)| b)
}

/// A `$name` value: a scalar as `--param` / `--param-str` give it, or a list of them (`--param-json`).
#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Param>),
}

impl Param {
    fn expr(&self) -> Expr {
        let literal = |l| Expr::Literal(l);
        match self {
            Param::Null => literal(Literal::Null),
            Param::Bool(b) => literal(Literal::Bool(*b)),
            Param::Int(i) => literal(Literal::Int(*i)),
            Param::Float(f) => literal(Literal::Float(*f)),
            Param::Str(s) => literal(Literal::Str(s.clone())),
            Param::List(items) => Expr::List(items.iter().map(Param::expr).collect()),
        }
    }
}

/// The most rows a query returns by default, and the most a caller may ask for (cypher-expert's section 4).
pub const DEFAULT_LIMIT: usize = 1_000;
pub const MAX_LIMIT: usize = 100_000;

/// How one query runs: which rows come back (`offset`, `limit`) and what it may spend.
pub struct RunOptions<'a> {
    /// Rows returned, 1 to [`MAX_LIMIT`] (larger is taken as the max).
    pub limit: usize,
    /// Rows skipped first.
    pub offset: usize,
    /// The executor's work budget and row cap.
    pub max_work: u64,
    pub max_rows: usize,
    /// `$name` values (`--param name=value`, typed by the caller like `--set`).
    pub params: BTreeMap<String, Param>,
    /// The service's wall-clock backstop, asked every few thousand work units: `true` stops the query with
    /// `query_timed_out`. umx-query reads no clock.
    pub cancel: Option<&'a dyn Fn() -> bool>,
    /// The query's one instant, read once from the service's clock before it runs: every `current_date()` and
    /// `current_timestamp()` on every row answers from it. None leaves them an error.
    pub now_ms: Option<u64>,
}

impl Default for RunOptions<'_> {
    fn default() -> Self {
        RunOptions {
            limit: DEFAULT_LIMIT,
            offset: 0,
            max_work: umx_cypher::DEFAULT_MAX_WORK,
            max_rows: umx_cypher::DEFAULT_MAX_ROWS,
            params: BTreeMap::new(),
            cancel: None,
            now_ms: None,
        }
    }
}

/// A query's result: the columns in RETURN order, the rows in the window asked for, how many rows the query
/// made (`total`, exact), whether rows past the window were cut, the work it used, and its warnings.
#[derive(Clone, Debug, PartialEq)]
pub struct Rows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
    pub total: usize,
    pub truncated: bool,
    pub work: u64,
    pub warnings: Vec<Problem>,
    /// Present when the query names similar_to: per graph of the view, in order, whether a similar_to step read
    /// from or reached a node with no whole-node vector, so the lists could not hold it. The caller warns
    /// (`similar_to_incomplete`, a services code) and adds the lists' model, k and floor.
    pub similar_missing: Option<Vec<bool>>,
}

/// F241 (R407-1): the refusal umx makes from a query's text alone, for a caller to run before any graph work of its
/// own (services' `--along` range): a column named like the key of umx's secret flags
/// ([`check::COLUMN_RESERVED`]). A query that does not parse passes here, and [`run`] says why; `run` makes the same
/// check, so a caller with no graph work before it need not call this.
///
/// # Errors
///
/// `query_column_reserved`.
pub fn preflight(text: &str) -> Result<(), Problem> {
    umx_cypher::parse(text)
        .ok()
        .and_then(|query| check::reserved_column(&query))
        .map_or(Ok(()), Err)
}

/// Parses, checks and runs one read-only Cypher query over one graph. Rows with no ORDER BY come in handle order
/// (id order), the same on every run and load order.
///
/// # Errors
///
/// A [`Problem`] with a `query_*` code: a refusal (the subset, a bound, a symmetric direction), a syntax error at
/// a byte position, `query_too_large` past the budget or the row cap, or `query_timed_out` when `cancel` said so.
/// A stopped query returns no rows.
pub fn run(view: &GraphView<'_>, text: &str, options: &RunOptions<'_>) -> Result<Rows, Problem> {
    view.provenance_in.borrow_mut().fill(None);
    view.provenance_work.set(0);
    let tokens = umx_cypher::lexer::tokenize(text).map_err(|e| match e {
        CypherError::Syntax { message, position } => {
            check::lex_failure(text, byte_at(text, position), &message)
        }
        _ => check::syntax(0, check::UNEXPECTED_CHARACTER),
    })?;
    // Parse first: a query that parses is judged on its AST only, so a keyword used as a name ((use:option),
    // AS set) is fine, as in LadybugDB. The token rules only say why a query did not parse (cypher-expert F1).
    let mut query = umx_cypher::parse(text).map_err(|e| match e {
        CypherError::Syntax { message, position } => {
            check::parse_failure(byte_at(text, position), &message, &tokens)
        }
        _ => check::syntax(0, check::DOES_NOT_PARSE),
    })?;
    let params: BTreeMap<String, Expr> = options
        .params
        .iter()
        .map(|(k, v)| (k.clone(), v.expr()))
        .collect();
    query
        .bind_params(&params)
        .map_err(|name| Problem::new(check::PARAM_MISSING).with("name", check::name_slot(&name)))?;
    let gathered = check::Gathered::of(&query);
    view.virtual_unknown.borrow_mut().clear();
    let names_graph = gathered
        .nodes
        .iter()
        .any(|n| n.labels.iter().any(|label| label == "graph"));
    view.virtual_scan.set(names_graph);
    let symmetric = |r: &str| view.is_symmetric(r);
    if let Some(p) = check::query(&gathered, &tokens, &symmetric)
        .or_else(|| check::unknown_variable(&query))
        .or_else(|| check::reserved_column(&query))
    {
        return Err(p);
    }

    let names_similar = gathered
        .rels
        .iter()
        .any(|r| r.types.iter().any(|t| t == SIMILAR_TO));
    view.similar_missing
        .borrow_mut()
        .iter_mut()
        .for_each(|m| *m = false);

    let limits = Limits {
        max_work: options.max_work,
        max_rows: options.max_rows,
        cancel: options.cancel,
    };
    // One graph runs with its own handles; several with handles that carry their graph.
    let result = if view.parts.len() == 1 && !names_graph && !view.external_edges {
        umx_cypher::execute_limited(&Provider::<One>::new(view, options.now_ms), &query, &limits)
    } else {
        umx_cypher::execute_limited(
            &Provider::<Many>::new(view, options.now_ms),
            &query,
            &limits,
        )
    };
    let result = result.map_err(|e| match e {
        CypherError::TooLarge { cause, used } => Problem::new(check::TOO_LARGE)
            .with(
                "cause",
                Slot::Word(match cause {
                    Limit::Work => "work",
                    Limit::Rows => "rows",
                }),
            )
            .with("units", Slot::Count(used)),
        CypherError::Cancelled { used } => {
            Problem::new(check::TIMED_OUT).with("units", Slot::Count(used))
        }
        CypherError::SymmetricDirection { relation } => {
            Problem::new(check::SYMMETRIC_DIRECTION).with("relation", check::name_slot(&relation))
        }
        CypherError::Syntax { position, .. } => check::syntax(position, check::DOES_NOT_PARSE),
        CypherError::Execution { message } => check::invalid(&message),
    })?;

    let mut warnings = vocabulary_warnings(view, &gathered);
    for w in result.warnings {
        match w {
            Warning::SymmetricInDirectedStep { relations } => {
                warnings.extend(relations.iter().map(|r| {
                    Problem::new(check::SYMMETRIC_IN_DIRECTED_STEP)
                        .with("relation", check::name_slot(r))
                }));
            }
        }
    }

    let total = result.rows.len();
    let limit = options.limit.clamp(1, MAX_LIMIT);
    let rows: Vec<Vec<Cell>> = result
        .rows
        .into_iter()
        .skip(options.offset)
        .take(limit)
        .map(|row| row.into_iter().map(|v| view.cell(v)).collect())
        .collect();
    let truncated = total > options.offset.saturating_add(limit);
    if truncated {
        warnings.push(Problem::new(check::TRUNCATED).with("shown", Slot::Count(rows.len() as u64)));
    }
    Ok(Rows {
        columns: result.columns,
        rows,
        total,
        truncated,
        work: result.work,
        warnings,
        similar_missing: names_similar.then(|| view.similar_missing.borrow().clone()),
    })
}

/// Soft vocabulary (D7): a label, relation or property no graph in scope uses is a warning, and the query still
/// runs (it matched nothing, or read null). similar_to is always known (read from the lists). With no graph in scope there is nothing to
/// compare with, so no warning.
fn vocabulary_warnings(view: &GraphView<'_>, g: &check::Gathered<'_>) -> Vec<Problem> {
    let mut out = Vec::new();
    let labels: BTreeSet<&str> = g
        .nodes
        .iter()
        .flat_map(|n| n.labels.iter().map(String::as_str))
        .collect();
    // The name a provenance_hidden warning gives is umx's own, from the fixed list, never the query's text.
    let hidden_label = |name: &str| {
        (!view.include_provenance)
            .then(|| {
                umx_core::ontology::PROVENANCE_TYPES
                    .iter()
                    .find(|t| **t == name)
            })
            .flatten()
    };
    for label in labels {
        if let Some(kind) = hidden_label(label) {
            out.push(Problem::new(check::PROVENANCE_HIDDEN).with("name", check::name_slot(kind)));
        } else if !(label == "graph" && !view.targets.is_empty())
            && !view.parts.iter().enumerate().any(|(part, p)| {
                p.graph
                    .nodes()
                    .any(|node| node.kind() == label && view.named_visible(part as u32, node.ix()))
            })
        {
            out.push(Problem::new(check::UNKNOWN_LABEL).with("name", check::name_slot(label)));
        }
    }
    let relations: BTreeSet<&str> = g
        .rels
        .iter()
        .flat_map(|r| r.types.iter().map(String::as_str))
        .collect();
    for relation in relations {
        let derived = matches!(relation, "authored_by" | "edited_by");
        // The stored provenance relations (session_of, runs_on, uses_model, registered_in) join only provenance
        // records, and user edges cannot carry them (reserved_relation): hidden, a step's type naming one warns by
        // name, whatever the data (cypher N1, security on ws008 T2b): decided from the relationship types of the
        // parsed patterns, and the name is the fixed list's.
        let hidden = umx_core::ontology::PROVENANCE_DERIVED
            .iter()
            .chain(umx_core::ontology::PROVENANCE_RELATIONS)
            .find(|r| **r == relation);
        if let Some(fixed) = hidden.filter(|_| !view.include_provenance) {
            out.push(Problem::new(check::PROVENANCE_HIDDEN).with("name", check::name_slot(fixed)));
        } else if !derived
            && relation != SIMILAR_TO
            && !view.parts.iter().enumerate().any(|(part, p)| {
                p.graph.edges().any(|edge| {
                    edge.kind() == relation
                        && [edge.from_ix(), edge.to_ix()]
                            .into_iter()
                            .flatten()
                            .all(|node| view.named_visible(part as u32, node))
                })
            })
        {
            out.push(
                Problem::new(check::UNKNOWN_RELATION).with("name", check::name_slot(relation)),
            );
        }
    }
    let mut unknown_properties = view.virtual_unknown.borrow().clone();
    for property in g
        .properties()
        .into_iter()
        .filter(|p| !check::standard_field(p))
    {
        // `n.locator` is never a property (ws006 S3): its parts are the flat locator_* names, even on a graph that
        // keeps an older custom field of that name.
        let used = property != "locator"
            && ((property == "alias" && !view.targets.is_empty())
                || view.parts.iter().enumerate().any(|(part, p)| {
                    p.graph
                        .nodes()
                        .filter(|node| view.named_visible(part as u32, node.ix()))
                        .any(|n| n.custom_field_names().any(|f| f == property))
                        || p.graph
                            .edges()
                            .filter(|edge| {
                                [edge.from_ix(), edge.to_ix()]
                                    .into_iter()
                                    .flatten()
                                    .all(|node| view.named_visible(part as u32, node))
                            })
                            .any(|e| e.field(property).is_some())
                }));
        if !used {
            unknown_properties.insert(property.into());
        }
    }
    out.extend(unknown_properties.iter().map(|property| {
        Problem::new(check::UNKNOWN_PROPERTY).with("name", check::name_slot(property))
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance_graph(metadata: bool, ordinary_session: bool) -> Graph {
        let mut content = node("Content");
        content.authored_by_session = Some(Id(9));
        content.edited_by_session = Some(Id(9));
        let mut nodes = alloc::vec![content];
        if ordinary_session {
            let mut ordinary = node("Ordinary session");
            ordinary.id = Id(2);
            ordinary.kind = "session".into();
            nodes.push(ordinary);
        }
        if metadata {
            let mut session = node("Recorded session");
            session.id = Id(9);
            session.kind = "session".into();
            session.fields.insert(
                "prov_key".into(),
                umx_core::Value::String("session/unknown/unknown".into()),
            );
            nodes.push(session);
        }
        Graph::from_records(nodes, Vec::new())
    }

    #[test]
    fn hidden_provenance_matches_absent_rows_work_and_lexical_warnings() {
        for ordinary in [false, true] {
            let bare = provenance_graph(false, ordinary);
            let metadata = provenance_graph(true, ordinary);
            for text in [
                "MATCH (n) RETURN n.id",
                "MATCH (n:session) RETURN n.id",
                "MATCH (n)-[r:authored_by]->(s) RETURN n.id",
                "MATCH (n)-[r:edited_by]->(s) RETURN n.id",
                "MATCH (n)-[r]->(s) RETURN count(r)",
            ] {
                let expected = run(
                    &GraphView::one("g", &bare, &[]),
                    text,
                    &RunOptions::default(),
                )
                .unwrap();
                let actual = run(
                    &GraphView::one("g", &metadata, &[]),
                    text,
                    &RunOptions::default(),
                )
                .unwrap();
                assert_eq!(
                    (actual.rows, actual.work, actual.warnings),
                    (expected.rows, expected.work, expected.warnings),
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn hidden_stored_provenance_relations_warn_provenance_hidden_whatever_the_data() {
        let codes = |view: &GraphView<'_>, text: &str| -> Vec<&'static str> {
            run(view, text, &RunOptions::default())
                .unwrap()
                .warnings
                .iter()
                .map(|w| w.code)
                .collect()
        };
        for relation in umx_core::ontology::PROVENANCE_RELATIONS {
            let text = alloc::format!("MATCH (a)-[r:{relation}]->(b) RETURN a.id");
            for metadata in [false, true] {
                let graph = provenance_graph(metadata, true);
                assert_eq!(
                    codes(&GraphView::one("g", &graph, &[]), &text),
                    [check::PROVENANCE_HIDDEN],
                    "{relation}, metadata {metadata}"
                );
            }
            // Opted in, a relation no visible edge carries is unknown as before: the warning names hidden data only.
            let graph = provenance_graph(false, false);
            assert_eq!(
                codes(
                    &GraphView::one("g", &graph, &[]).with_provenance(true),
                    &text
                ),
                [check::UNKNOWN_RELATION],
                "{relation} with provenance"
            );
        }
    }

    #[test]
    fn relation_vocabulary_is_lazy_and_scans_only_hidden_provenance_graphs_once() {
        for metadata in [false, true] {
            let nodes = provenance_graph(metadata, true)
                .nodes()
                .map(|node| node.to_node())
                .collect();
            let mut edges: Vec<_> = (10..74)
                .map(|id| {
                    let mut edge = edge();
                    edge.id = Id(id);
                    edge.to = Id(2);
                    edge.kind = "supports".into();
                    edge
                })
                .collect();
            let mut similar = edge();
            similar.id = Id(99);
            similar.kind = SIMILAR_TO.into();
            edges.push(similar);
            if metadata {
                let mut hidden = edge();
                hidden.id = Id(100);
                hidden.to = Id(9);
                hidden.kind = "session_of".into();
                edges.push(hidden);
            }
            let graph = Graph::from_records(nodes, edges);
            for flag in [false, true] {
                let view = GraphView::one("g", &graph, &[]);
                assert!(view.relations.get().is_none());
                assert_eq!(view.relation_edge_visits.get(), 0);
                let view = view.with_provenance(flag);
                assert!(view.relations.get().is_none());
                assert_eq!(view.relation_edge_visits.get(), 0);
                let visits = if metadata && !flag {
                    graph.edge_count()
                } else {
                    0
                };
                let lookup = alloc::format!("MATCH (n {{id:'{}'}}) RETURN n.id", Id(1));
                assert_eq!(
                    run(&view, &lookup, &RunOptions::default()).unwrap().rows,
                    [alloc::vec![Cell::Str(Id(1).to_text())]]
                );
                assert_eq!(view.relation_edge_visits.get(), visits);
                let names = view.relation_names();
                assert!(names.contains("supports"));
                assert!(!names.contains(SIMILAR_TO));
                assert_eq!(names.contains("session_of"), metadata && flag);
                assert_eq!(names.contains("authored_by"), flag);
                assert_eq!(view.relation_edge_visits.get(), visits);
                let provider = Provider::<One>::new(&view, None);
                assert!(provider.has_relationship_type("supports"));
                assert_eq!(provider.relationship_types().len(), names.len());
                assert_eq!(view.relation_edge_visits.get(), visits);
                let view = view.with_provenance(!flag);
                assert!(view.relations.get().is_none());
                assert_eq!(
                    view.relation_names().contains("session_of"),
                    metadata && !flag
                );
                assert_eq!(
                    view.relation_edge_visits.get(),
                    visits
                        + if metadata && flag {
                            graph.edge_count()
                        } else {
                            0
                        }
                );
            }
        }
    }

    #[test]
    fn provenance_relations_are_named_node_to_session_views_in_both_directions() {
        let graph = provenance_graph(true, false);
        let view = GraphView::one("g", &graph, &["authored_by"]).with_provenance(true);
        for text in [
            "MATCH (n)-[r:authored_by]->(s) RETURN r.id, r.address, r.type, r.from, r.to, r.graph, r.derived",
            "MATCH (s:session)<-[r:authored_by]-(n) RETURN r.id, r.address, r.type, r.from, r.to, r.graph, r.derived",
        ] {
            let rows = run(&view, text, &RunOptions::default()).unwrap();
            assert_eq!(
                rows.rows,
                [alloc::vec![
                    Cell::Null,
                    Cell::Null,
                    Cell::Str("authored_by".into()),
                    Cell::Str(Id(1).to_text()),
                    Cell::Str(Id(9).to_text()),
                    Cell::Str("g".into()),
                    Cell::Bool(true)
                ]]
            );
            assert!(rows.warnings.is_empty(), "{:?}", rows.warnings);
        }
        let rows = run(
            &view,
            "MATCH (n)-[r:edited_by]->(s) RETURN r",
            &RunOptions::default(),
        )
        .unwrap();
        assert!(
            matches!(&rows.rows[0][0], Cell::Provenance(edge) if edge.kind == "edited_by" && edge.to == Id(9))
        );
        assert_eq!(
            run(
                &view,
                "MATCH (n)-[r]->(s) RETURN count(r)",
                &RunOptions::default()
            )
            .unwrap()
            .rows,
            [alloc::vec![Cell::Int(0)]]
        );
        let dangling = provenance_graph(false, false);
        assert!(
            run(
                &GraphView::one("g", &dangling, &[]).with_provenance(true),
                "MATCH ()-[r:authored_by]->() RETURN r",
                &RunOptions::default()
            )
            .unwrap()
            .rows
            .is_empty()
        );
    }

    #[test]
    fn provenance_reverse_index_is_charged_once_and_var_length_is_refused() {
        let graph = provenance_graph(true, false);
        let view = GraphView::one("g", &graph, &[]).with_provenance(true);
        let provider = Provider::<One>::new(&view, None);
        let session = graph.node_ix(Id(9)).unwrap();
        assert_eq!(provider.expand_in(session, "authored_by").len(), 1);
        assert!(provider.take_expansion_work() >= graph.node_count() as u64);
        assert_eq!(provider.expand_in(session, "authored_by").len(), 1);
        assert_eq!(provider.take_expansion_work(), 0);
        for flag in [false, true] {
            let view = GraphView::one("g", &graph, &[]).with_provenance(flag);
            for kind in ["authored_by", "edited_by"] {
                assert_eq!(
                    run(
                        &view,
                        &alloc::format!("MATCH ()-[:{kind}*1..2]->(n) RETURN n.id"),
                        &RunOptions::default()
                    )
                    .unwrap_err()
                    .code,
                    check::FEATURE_NOT_SUPPORTED
                );
            }
        }
    }

    fn node(title: &str) -> Node {
        Node {
            id: Id(1),
            kind: "problem".into(),
            title: title.into(),
            status: "open".into(),
            note: String::new(),
            evidence: Vec::new(),
            status_history: Vec::new(),
            created: String::new(),
            updated: String::new(),
            created_by: String::new(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields: BTreeMap::new(),
        }
    }

    fn edge() -> umx_core::Edge {
        umx_core::Edge {
            id: Id(9),
            kind: "blocks".into(),
            from_graph: None,
            from: Id(1),
            to_graph: None,
            to: Id(1),
            created: String::new(),
            created_by: String::new(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: BTreeMap::new(),
        }
    }

    struct Never;
    impl ExternalSteps for Never {
        fn steps(&self, _: At<ViewNode>, _: bool, _: Option<&[String]>) -> Vec<ExternalStep> {
            panic!("One called the callback")
        }
        fn take_unresolved_work(&self) -> u64 {
            panic!("One consulted callback work")
        }
        fn graph_nodes(&self) -> Vec<At<ViewNode>> {
            panic!("One scanned external graph nodes")
        }
    }

    #[test]
    fn node_session_fields_are_known_without_any_stamped_edges() {
        let mut stamped = node("Attributed");
        stamped.authored_by_session = Some(Id(2));
        stamped.edited_by_session = Some(Id(3));
        let graph = Graph::from_records(alloc::vec![stamped], Vec::new());
        let view = GraphView::one("one", &graph, &[]);
        let rows = run(
            &view,
            "MATCH (n) RETURN n.authored_by_session AS authored, n.edited_by_session AS edited",
            &RunOptions::default(),
        )
        .unwrap();
        assert_eq!(
            rows.rows,
            alloc::vec![alloc::vec![
                Cell::Str(Id(2).to_text()),
                Cell::Str(Id(3).to_text())
            ]]
        );
        assert!(rows.warnings.is_empty(), "{:?}", rows.warnings);
    }

    #[test]
    fn one_never_calls_external_steps_even_when_a_callback_is_present() {
        let graph = Graph::from_records(alloc::vec![node("Local")], alloc::vec![edge()]);
        let view = GraphView::one("one", &graph, &[]).with_external(
            alloc::vec![GraphTarget {
                id: Id(2),
                title: "Graph"
            }],
            &Never,
            false,
        );
        let rows = run(
            &view,
            "MATCH (a)-[r]->(b) RETURN b.title AS title",
            &RunOptions::default(),
        )
        .unwrap();
        assert_eq!(
            rows.rows,
            alloc::vec![alloc::vec![Cell::Str("Local".into())]]
        );
    }

    struct Cross {
        edge: EdgeIx,
        other: NodeIx,
    }

    impl ExternalSteps for Cross {
        fn steps(
            &self,
            node: At<ViewNode>,
            incoming: bool,
            _: Option<&[String]>,
        ) -> Vec<ExternalStep> {
            let part = node.part;
            if (part == 0 && !incoming) || (part == 1 && incoming) {
                alloc::vec![ExternalStep {
                    edge: At::new(0, self.edge),
                    node: At::new(1 - part, ViewNode::Stored(self.other)),
                }]
            } else {
                Vec::new()
            }
        }
    }

    #[test]
    fn crossing_handles_with_colliding_node_indexes_keep_their_part_and_budget() {
        let mut external = edge();
        external.to_graph = Some(Id(3));
        let left = Graph::from_records(alloc::vec![node("Left")], alloc::vec![external]);
        let right = Graph::from_records(alloc::vec![node("Right")], Vec::new());
        assert_eq!(left.node_ix(Id(1)), right.node_ix(Id(1)));
        let cross = Cross {
            edge: left.edge(Id(9)).unwrap().ix(),
            other: right.node_ix(Id(1)).unwrap(),
        };
        let view = GraphView::new(alloc::vec![
            Part {
                alias: "left",
                graph: &left,
                symmetric: Vec::new()
            },
            Part {
                alias: "right",
                graph: &right,
                symmetric: Vec::new()
            },
        ])
        .with_external(
            alloc::vec![
                GraphTarget {
                    id: Id(2),
                    title: ""
                },
                GraphTarget {
                    id: Id(3),
                    title: ""
                },
            ],
            &cross,
            true,
        );
        let text = "MATCH (a)-[r]->(b) RETURN a.title AS a_title, b.title AS b_title";
        assert_eq!(
            run(&view, text, &RunOptions::default()).unwrap().rows,
            alloc::vec![alloc::vec![
                Cell::Str("Left".into()),
                Cell::Str("Right".into())
            ]]
        );
        let mut local_target = node("Right");
        local_target.id = Id(2);
        let mut local_edge = edge();
        local_edge.to = Id(2);
        let local = Graph::from_records(
            alloc::vec![node("Left"), local_target],
            alloc::vec![local_edge],
        );
        let local_view = GraphView::one("local", &local, &[]);
        let local_rows = run(&local_view, text, &RunOptions::default()).unwrap();
        let crossing_rows = run(&view, text, &RunOptions::default()).unwrap();
        assert_eq!(
            (crossing_rows.rows, crossing_rows.work),
            (local_rows.rows, local_rows.work)
        );
        let limited = RunOptions {
            max_work: local_rows.work - 1,
            ..RunOptions::default()
        };
        assert_eq!(
            run(&view, text, &limited).unwrap_err().code,
            "query_too_large"
        );
        let cancelled = || true;
        let options = RunOptions {
            cancel: Some(&cancelled),
            ..RunOptions::default()
        };
        let mut local_loop = edge();
        local_loop.id = Id(10);
        let mut external = edge();
        external.to_graph = Some(Id(3));
        let left_cycle = Graph::from_records(
            alloc::vec![node("Left")],
            alloc::vec![external, local_loop.clone()],
        );
        let right_cycle = Graph::from_records(alloc::vec![node("Right")], alloc::vec![local_loop]);
        let cycle = Cross {
            edge: left_cycle.edge(Id(9)).unwrap().ix(),
            other: right_cycle.node_ix(Id(1)).unwrap(),
        };
        let cycle_view = GraphView::new(alloc::vec![
            Part {
                alias: "left",
                graph: &left_cycle,
                symmetric: Vec::new()
            },
            Part {
                alias: "right",
                graph: &right_cycle,
                symmetric: Vec::new()
            },
        ])
        .with_external(
            alloc::vec![
                GraphTarget {
                    id: Id(2),
                    title: ""
                },
                GraphTarget {
                    id: Id(3),
                    title: ""
                },
            ],
            &cycle,
            true,
        );
        assert_eq!(
            run(
                &cycle_view,
                "MATCH (a)-[:blocks*1..10]-(b) RETURN b.title AS title",
                &options
            )
            .unwrap_err()
            .code,
            "query_timed_out"
        );
    }

    /// A row binding holds an edge handle; a similar_to entry must not make it wider than before (graph-storage-
    /// expert: peak memory of every query scales with it). One graph: 8 bytes; several: 12.
    #[test]
    fn an_edge_handle_stays_small() {
        assert!(core::mem::size_of::<super::ViewEdge>() <= 8);
        assert!(core::mem::size_of::<super::At<super::ViewEdge>>() <= 12);
    }

    /// ws006 S3b (cypher-expert): a cold locator part of a plain node is null from the hot kind column, so the
    /// node's line is never decoded; a resource node's part is still read from its line.
    #[test]
    fn a_cold_locator_part_never_decodes_a_plain_node() {
        let text = "MATCH (n) RETURN n.locator_source, n.locator_hash";
        let plain = Graph::from_records(alloc::vec![node("Plain")], Vec::new());
        let view = GraphView::one("g", &plain, &[]);
        let rows = run(&view, text, &RunOptions::default()).unwrap();
        assert_eq!(rows.rows, [alloc::vec![Cell::Null, Cell::Null]]);
        assert!(
            view.cold.borrow().is_none(),
            "a plain node's line was decoded"
        );

        let mut brief = node("Brief");
        brief.locator = Some(umx_core::Locator::parse("docs/brief.md").unwrap());
        let resource = Graph::from_records(alloc::vec![brief], Vec::new());
        let view = GraphView::one("g", &resource, &[]);
        let rows = run(&view, text, &RunOptions::default()).unwrap();
        assert_eq!(
            rows.rows,
            [alloc::vec![Cell::Str("docs/brief.md".into()), Cell::Null]]
        );
        assert!(
            view.cold.borrow().is_some(),
            "a resource node's line is decoded"
        );
    }

    /// `property` as it read before ws005 s14 (names matched on every read, values through JSON): the oracle the
    /// slot path must equal.
    fn property_before<S: Space>(
        p: &Provider<'_, '_, S>,
        node: S::Node,
        prop: &str,
    ) -> CypherValue {
        let (part, ix) = S::node_at(node);
        if ix == ViewNode::Graph {
            let target = &p.view.targets[part as usize];
            return match prop {
                "id" => CypherValue::Str(target.id.to_text()),
                "address" => CypherValue::Str(alloc::format!("umx:{}", p.view.alias(part))),
                "type" => CypherValue::Str("graph".into()),
                "title" => CypherValue::Str(
                    if target.title.is_empty() {
                        p.view.alias(part)
                    } else {
                        target.title
                    }
                    .into(),
                ),
                "alias" => CypherValue::Str(p.view.alias(part).into()),
                _ => {
                    p.view.virtual_unknown.borrow_mut().insert(prop.into());
                    CypherValue::Null
                }
            };
        }
        let ViewNode::Stored(ix) = ix else {
            unreachable!()
        };
        let n = p.view.graph(part).node_at(ix);
        let value = match prop {
            "address" => return CypherValue::Str(address(p.view.alias(part), n.id())),
            "graph" => return CypherValue::Str(p.view.alias(part).to_string()),
            _ if umx_core::locator::LOCATOR_NAMES.contains(&prop)
                && p.view.graph(part).locator_kind(ix).is_none() =>
            {
                return CypherValue::Null;
            }
            _ if COLD_FIELDS.contains(&prop) => p.view.cold_field(part, ix, prop),
            _ => n.field(prop),
        };
        value.map_or(CypherValue::Null, |v| cypher_value(&v))
    }

    fn cost_before(prop: &str) -> u64 {
        if COLD_FIELDS.contains(&prop) {
            COLD_FIELD_COST
        } else {
            1
        }
    }

    /// Every name the differential reads: the standard fields, the view's own, every locator part, the cold fields,
    /// each custom value kind, a name no graph interns, and names interned in one part only.
    fn differential_names() -> Vec<&'static str> {
        let mut names = alloc::vec![
            "id",
            "type",
            "title",
            "status",
            "created",
            "updated",
            "created_by",
            "authored_by_session",
            "edited_by_session",
            "evidence_kinds",
            "address",
            "graph",
            "alias",
            "c_uint",
            "c_big",
            "c_neg",
            "c_float",
            "c_str",
            "c_esc",
            "c_true",
            "c_list",
            "c_obj",
            "c_null",
            "absent",
            "only0",
            "only1",
        ];
        names.extend(umx_core::locator::LOCATOR_NAMES);
        names.extend(COLD_FIELDS);
        names
    }

    /// Checks `property`, `property_at` by `property_key`, `property_cost` and `key_cost` against the oracle for
    /// every node and every name, including the unknown-property notes a virtual :graph node leaves.
    fn assert_reads_as_before<S: Space>(p: &Provider<'_, '_, S>, nodes: &[S::Node])
    where
        S::Node: core::fmt::Debug,
    {
        for name in differential_names() {
            let key = p.property_key(name).expect("every name resolves");
            assert_eq!(p.key_cost(key), cost_before(name), "{name}");
            assert_eq!(p.property_cost(name), cost_before(name), "{name}");
            for &node in nodes {
                p.view.virtual_unknown.borrow_mut().clear();
                let before = property_before(p, node, name);
                let noted = p.view.virtual_unknown.replace(BTreeSet::new());
                assert_eq!(p.property(node, name), before, "{name} on {node:?}");
                assert_eq!(p.property_at(node, key), before, "{name} on {node:?}");
                let noted_now = p.view.virtual_unknown.replace(BTreeSet::new());
                assert_eq!(noted_now, noted, "{name} on {node:?}");
            }
        }
    }

    struct NoSteps;
    impl ExternalSteps for NoSteps {
        fn steps(&self, _: At<ViewNode>, _: bool, _: Option<&[String]>) -> Vec<ExternalStep> {
            Vec::new()
        }
        fn take_unresolved_work(&self) -> u64 {
            0
        }
        fn graph_nodes(&self) -> Vec<At<ViewNode>> {
            Vec::new()
        }
    }

    /// ws005 s14 (cypher-expert): a property read through its resolved slot answers exactly as the name-matching
    /// read did, at the same cost, on plain, resource, stamped and virtual :graph nodes, in One and Many views.
    #[test]
    fn property_reads_through_slots_equal_the_name_matching_read_at_the_same_cost() {
        use serde_json::json;
        let mut plain = node("Plain");
        for (k, v) in [
            ("c_uint", json!(7)),
            ("c_big", json!(u64::MAX)),
            ("c_neg", json!(-3)),
            ("c_float", json!(2.5)),
            ("c_str", json!("plain")),
            ("c_esc", json!("tab\there \"q\" \u{e9}")),
            ("c_true", json!(true)),
            ("c_list", json!([1, "a"])),
            ("c_obj", json!({"k": 1})),
            ("c_null", json!(null)),
            ("only0", json!("zero")),
        ] {
            plain.fields.insert(k.into(), v);
        }
        plain.note = "a \"note\"".into();
        plain.evidence = alloc::vec![json!({"source": "a.md", "kind": "room"})];
        plain.status_history = alloc::vec![json!({"from": "open", "to": "done"})];
        let mut resource = node("Resource");
        resource.id = Id(2);
        resource.locator = Some(umx_core::Locator::parse("docs/brief.md").unwrap());
        resource.evidence = alloc::vec![
            json!({"source": "b.md", "kind": "transcript"}),
            json!({"source": "c.md", "kind": "client"}),
        ];
        resource.fields.insert("c_str".into(), json!("other"));
        let mut stamped = node("Stamped");
        stamped.id = Id(3);
        stamped.authored_by_session = Some(Id(7));
        stamped.edited_by_session = Some(Id(8));
        let zero = Graph::from_records(alloc::vec![plain, resource, stamped], Vec::new());
        // Part 1 interns `only1` where part 0 interns `c_big` (its first custom name): reading part 1 by part 0's
        // handle would answer part 0's field.
        let mut one_node = node("Other part");
        one_node.fields.insert("only1".into(), json!("one"));
        let one = Graph::from_records(alloc::vec![one_node], Vec::new());
        assert_eq!(zero.field_id("c_big"), one.field_id("only1"));

        for g in [&zero, &one] {
            let view = GraphView::one("g", g, &[]);
            let nodes: Vec<NodeIx> = g.nodes().map(|n| n.ix()).collect();
            assert_reads_as_before(&Provider::<One>::new(&view, None), &nodes);
        }
        let view = GraphView::new(alloc::vec![
            Part {
                alias: "zero",
                graph: &zero,
                symmetric: Vec::new(),
            },
            Part {
                alias: "one",
                graph: &one,
                symmetric: Vec::new(),
            },
        ])
        .with_external(
            alloc::vec![
                GraphTarget {
                    id: Id(100),
                    title: "",
                },
                GraphTarget {
                    id: Id(101),
                    title: "Part one",
                },
            ],
            &NoSteps,
            false,
        );
        let mut nodes = Vec::new();
        for (part, g) in (0u32..).zip([&zero, &one]) {
            nodes.extend(
                g.nodes()
                    .map(|n| Many::node(part, ViewNode::Stored(n.ix()))),
            );
            nodes.push(Many::node(part, ViewNode::Graph));
        }
        assert_reads_as_before(&Provider::<Many>::new(&view, None), &nodes);
    }

    /// External steps that only add each part's virtual :graph node to an unlabelled scan.
    struct GraphNodes;
    impl ExternalSteps for GraphNodes {
        fn steps(&self, _: At<ViewNode>, _: bool, _: Option<&[String]>) -> Vec<ExternalStep> {
            Vec::new()
        }
        fn take_unresolved_work(&self) -> u64 {
            0
        }
        fn graph_nodes(&self) -> Vec<At<ViewNode>> {
            alloc::vec![
                Many::node(0, ViewNode::Graph),
                Many::node(1, ViewNode::Graph)
            ]
        }
    }

    /// Runs `text` three ways and asserts one answer: with `each_str` answering from the column, with it switched off
    /// (the executor's fallback), and as an oracle with every `<<x>>` read as `coalesce(x)`, which no fast path
    /// takes, so it evaluates the property exactly as before ws005 s15. Rows, errors, warnings and work must agree,
    /// at the full budget and at half the work it took.
    fn assert_lists_as_before(view: &GraphView<'_>, text: &str) -> Result<Rows, Problem> {
        let fast = text.replace("<<", "").replace(">>", "");
        let oracle = text.replace("<<", "coalesce(").replace(">>", ")");
        let mut options = RunOptions::default();
        options
            .params
            .insert("d".into(), Param::Str("2025-01-15".into()));
        options
            .params
            .insert("p".into(), Param::Str("tab\tq\"k\\".into()));
        let three = |options: &RunOptions<'_>| {
            view.lists_off.set(true);
            let fallback = run(view, &fast, options);
            view.lists_off.set(false);
            let before = run(view, &oracle, options);
            let now = run(view, &fast, options);
            assert_eq!(fallback, before, "fallback against the oracle: {fast}");
            assert_eq!(now, before, "provider list against the oracle: {fast}");
            now
        };
        let now = three(&options);
        if let Ok(rows) = &now {
            options.max_work = rows.work / 2;
            let _ = three(&options);
        }
        now
    }

    /// ws005 s15 (cypher-expert): `x IN n.evidence_kinds` and `size(n.evidence_kinds)` answered from the column give
    /// the rows, errors, warnings and work of the built list, for every kind of left value, in every place a
    /// membership test is written, on One and Many views with virtual :graph nodes; dates keep IN's `=` (TM2).
    #[test]
    fn list_membership_and_size_from_the_column_equal_the_built_list() {
        use serde_json::json;
        let kinds = |n: &mut Node, kinds: &[&str]| {
            n.evidence = kinds
                .iter()
                .map(|k| json!({"source": "a.md", "kind": k}))
                .collect();
        };
        let mut none = node("None");
        none.fields.insert("c_num".into(), json!(3));
        let mut dup = node("Dup");
        dup.id = Id(2);
        kinds(&mut dup, &["room", "analyst", "room"]);
        dup.fields.insert("c_list".into(), json!(["a", 1]));
        let mut dated = node("Dated");
        dated.id = Id(3);
        kinds(&mut dated, &["2025-01", "client"]);
        let mut escaped = node("Caf\u{e9}");
        escaped.id = Id(4);
        escaped.status = "done".into();
        kinds(&mut escaped, &["tab\tq\"k\\", "transcript"]);
        let mut analyst = node("Analyst");
        analyst.id = Id(5);
        kinds(&mut analyst, &["analyst"]);
        let mut edges = Vec::new();
        for (id, from, to) in [(20, 2, 3), (21, 3, 4), (22, 5, 2)] {
            let mut e = edge();
            e.id = Id(id);
            e.from = Id(from);
            e.to = Id(to);
            edges.push(e);
        }
        let zero = Graph::from_records(alloc::vec![none, dup, dated, escaped, analyst], edges);
        let mut other = node("Other");
        kinds(&mut other, &["analyst", "2025"]);
        let one = Graph::from_records(alloc::vec![other], Vec::new());

        let lefts = [
            "'room'", "'nope'", "'2025'", "$d", "$p", "1", "null", "['room']", "n.title",
        ];
        let templates = [
            "MATCH (n) WHERE LEFT IN <<n.evidence_kinds>> RETURN n.id AS id ORDER BY id",
            "MATCH (n) RETURN n.id AS id, LEFT IN <<n.evidence_kinds>> AS hit ORDER BY id",
            "MATCH (n) RETURN n.id AS id, CASE WHEN LEFT IN <<n.evidence_kinds>> THEN 'y' ELSE 'n' END AS c \
             ORDER BY id",
            "MATCH (n) WHERE NOT LEFT IN <<n.evidence_kinds>> RETURN n.id AS id ORDER BY id",
            "MATCH (n)-[:blocks]->(m) WHERE LEFT IN <<n.evidence_kinds>> AND m.status = 'open' RETURN n.id AS a, \
             m.id AS b ORDER BY a, b",
            "MATCH (n) OPTIONAL MATCH (n)-[:blocks]->(m) RETURN n.id AS id, LEFT IN <<m.evidence_kinds>> AS hit, \
             size(<<m.evidence_kinds>>) AS s ORDER BY id",
            // n is one row, so the IN's right side is hoisted: that path still answers first.
            "MATCH (n {title: 'Dup'}) WITH n MATCH (m) WHERE LEFT IN <<n.evidence_kinds>> RETURN m.id AS id \
             ORDER BY id",
            "MATCH (n:graph) RETURN n.id AS id, LEFT IN <<n.evidence_kinds>> AS hit, \
             size(<<n.evidence_kinds>>) AS s ORDER BY id",
            "MATCH (n) WHERE LEFT IN <<n.c_list>> RETURN n.id AS id ORDER BY id",
        ];
        let fixed = [
            "MATCH (n) WHERE size(<<n.evidence_kinds>>) > 0 AND NOT 'client' IN <<n.evidence_kinds>> AND NOT \
             'transcript' IN <<n.evidence_kinds>> RETURN n.id AS id ORDER BY id",
            "MATCH (n) WHERE 'analyst' IN <<n.evidence_kinds>> AND size(<<n.evidence_kinds>>) = 1 RETURN n.id AS id \
             ORDER BY id",
            "MATCH (n) WHERE size(<<n.evidence_kinds>>) = 2 RETURN n.id AS id ORDER BY id",
            "MATCH (n) RETURN n.id AS id, size(<<n.evidence_kinds>>) AS k, size(<<n.title>>) AS t, \
             size(<<n.c_list>>) AS c, size(<<n.absent>>) AS a ORDER BY id",
            "MATCH (n) RETURN size(<<n.c_num>>) AS s",
            "MATCH (n)-[r:blocks]->(m) RETURN size(<<r.evidence_kinds>>) AS s, 'room' IN <<r.evidence_kinds>> AS h",
        ];

        let single = GraphView::one("g", &zero, &[]);
        let many = GraphView::new(alloc::vec![
            Part {
                alias: "zero",
                graph: &zero,
                symmetric: Vec::new(),
            },
            Part {
                alias: "one",
                graph: &one,
                symmetric: Vec::new(),
            },
        ])
        .with_external(
            alloc::vec![
                GraphTarget {
                    id: Id(100),
                    title: "",
                },
                GraphTarget {
                    id: Id(101),
                    title: "Part one",
                },
            ],
            &GraphNodes,
            false,
        );
        for view in [&single, &many] {
            for template in templates {
                for left in lefts {
                    let _ = assert_lists_as_before(view, &template.replace("LEFT", left));
                }
            }
            for text in fixed {
                let _ = assert_lists_as_before(view, text);
            }
        }

        // The column answers, not only the fallback: a date-shaped string meets the date-like kind by instant
        // (TM2), and a virtual :graph node, which holds no list, still warns that the name is unknown to it.
        let dated = assert_lists_as_before(
            &single,
            "MATCH (n) WHERE $d IN <<n.evidence_kinds>> RETURN n.title AS t",
        )
        .unwrap();
        assert_eq!(dated.rows, [[Cell::Str("Dated".into())]]);
        let graphs = assert_lists_as_before(
            &many,
            "MATCH (n:graph) RETURN null IN <<n.evidence_kinds>> AS hit",
        )
        .unwrap();
        assert!(
            graphs
                .warnings
                .iter()
                .any(|w| w.code == check::UNKNOWN_PROPERTY),
            "{:?}",
            graphs.warnings
        );
    }
}
