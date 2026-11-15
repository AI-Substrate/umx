//! The array search behind `umx walk` and `umx path` (workstream 003 s17, review A2). The read space's nodes are
//! dense handles over umx-graph's own adjacency, so a step costs array reads, not keyed searches. The steps and
//! their order are [`ReadSpace::steps`]'s exactly (the differential tests hold the two together). `path` also
//! searches back from its end when the read allows it, then follows the forward order to the end, so the path it
//! returns, ties included, is the one the forward search alone returns.

use super::{At, ReadSpace};
use crate::query::{self, Direction, EdgeCell, NodeCell, Way};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use umx_core::Id;
use umx_graph::{Graph, NodeIx};

const SIMILAR_TO: &str = "similar_to";

/// A node of the read space. `Local` is a loaded node of a selected part, `Graph` a part's own graph node and
/// `Other` an end that is not a loaded node (its graph unloaded, hidden or ambiguous, or the node missing). Only
/// `Other` ends carry a state, and they are never expanded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Node {
    Local(usize, NodeIx),
    Graph(usize),
    Other(usize),
}

/// One step from a node: the node reached, the part storing the edge and the edge, borrowed until a row shows it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Step<'a> {
    pub to: Node,
    pub owner: usize,
    kind: &'a str,
    id: Option<Id>,
    way: Way,
    score: Option<i32>,
    derived: bool,
}

impl Step<'_> {
    pub fn cell(&self) -> EdgeCell {
        EdgeCell {
            id: self.id,
            relation: self.kind.into(),
            way: self.way,
            score: self.score,
            derived: self.derived,
        }
    }
}

struct Part<'a> {
    graph: &'a Graph,
    id: Id,
    /// `ReadSpace::visible` for this graph's provenance nodes (provenance_in_read), checked before a node expands.
    expand_provenance: bool,
    symmetric: Vec<&'a str>,
    /// Provenance edges by the node they point to, as (to, from, kind) in `provenance_edges` order; made on first use.
    provenance_in: Option<Vec<(NodeIx, NodeIx, &'static str)>>,
    /// This graph stores an edge with a graph-qualified end (`has_external_edges`).
    stores_external: bool,
    /// Another selected graph (another graph id) stores one, so links into this graph's nodes may exist.
    pointed_into: bool,
}

/// One bit per node key. Keys past the end (late `Other` ends) grow it.
pub(crate) struct Bits(Vec<u64>);

impl Bits {
    pub fn new(len: usize) -> Bits {
        Bits(alloc::vec![0; len.div_ceil(64)])
    }

    /// Marks `key`; true the first time.
    #[inline]
    pub fn insert(&mut self, key: usize) -> bool {
        let (word, bit) = (key / 64, 1u64 << (key % 64));
        if word >= self.0.len() {
            self.0.resize(word + 1, 0);
        }
        let fresh = self.0[word] & bit == 0;
        self.0[word] |= bit;
        fresh
    }
}

/// One value per node key, 0 for none. Keys past the end (late `Other` ends) read 0 and grow it when set.
struct Marks(Vec<u32>);

impl Marks {
    fn new(len: usize) -> Marks {
        Marks(alloc::vec![0; len])
    }

    #[inline]
    fn get(&self, key: usize) -> u32 {
        self.0.get(key).copied().unwrap_or(0)
    }

    #[inline]
    fn set(&mut self, key: usize, value: u32) {
        if key >= self.0.len() {
            self.0.resize(key + 1, 0);
        }
        self.0[key] = value;
    }
}

/// A node reached by the forward search, the visit it was first reached from and the step taken.
struct Visit<'a> {
    node: Node,
    parent: usize,
    step: Option<Step<'a>>,
}

/// The handles of one read pass. Parts, graph ids and visibility are resolved once here, not per step.
pub(crate) struct Handles<'s, 'a> {
    space: &'s ReadSpace<'a>,
    parts: Vec<Part<'a>>,
    /// Where each part's keys start; a part has `node_bound() + 1` keys, the last its graph node.
    base: Vec<usize>,
    others: Vec<At>,
    other_keys: BTreeMap<At, usize>,
    /// Some selected graph stores an edge with a graph-qualified end. Otherwise no step is external, as
    /// `incident_selected` only reads edges the selected graphs store.
    external: bool,
    include_provenance: bool,
}

impl<'s, 'a> Handles<'s, 'a> {
    pub fn new(space: &'s ReadSpace<'a>) -> Self {
        let mut base = Vec::with_capacity(space.aliases.len() + 1);
        let mut next = 0;
        let mut parts: Vec<Part<'a>> = space
            .aliases
            .iter()
            .map(|alias| {
                let loaded = space.store.loaded(alias).expect("selected");
                base.push(next);
                next += loaded.graph.node_bound() + 1;
                Part {
                    graph: &loaded.graph,
                    id: loaded.meta.id,
                    expand_provenance: space.store.provenance_in_read(
                        &space.access,
                        &space.refs,
                        loaded.meta.id,
                    ),
                    symmetric: loaded.meta.ontology.symmetric_kinds(),
                    provenance_in: None,
                    stores_external: space.store.has_external_edges(alias),
                    pointed_into: false,
                }
            })
            .collect();
        base.push(next);
        for p in 0..parts.len() {
            let id = parts[p].id;
            parts[p].pointed_into = parts.iter().any(|q| q.stores_external && q.id != id);
        }
        Handles {
            space,
            external: parts.iter().any(|p| p.stores_external),
            parts,
            base,
            others: Vec::new(),
            other_keys: BTreeMap::new(),
            include_provenance: space.access.include_provenance,
        }
    }

    /// Whether `node` can have external steps. A loaded node has them only through an external edge its own graph
    /// stores at it (its external adjacency) or a link another selected graph stores into it: a graph's own edges
    /// never name its own id (`normalize_own_ends` on load, `write_end` on writes), so copies and the graph itself
    /// add none. A graph node is always asked.
    fn may_be_external(&self, node: Node) -> bool {
        self.external
            && match node {
                Node::Local(part, ix) => {
                    let p = &self.parts[part];
                    p.pointed_into
                        || (p.stores_external
                            && (p.graph.edges_out_external(ix).next().is_some()
                                || p.graph.edges_in_external(ix).next().is_some()))
                }
                Node::Graph(_) => true,
                Node::Other(_) => false,
            }
    }

    /// Whether `steps` expands `node` (`ReadSpace::visible` and no state).
    fn expands(&self, node: Node) -> bool {
        match node {
            Node::Local(part, ix) => {
                self.parts[part].expand_provenance || !self.parts[part].graph.is_provenance(ix)
            }
            Node::Graph(_) => true,
            Node::Other(_) => false,
        }
    }

    pub fn keys(&self) -> usize {
        self.base[self.parts.len()]
    }

    #[inline]
    pub fn key(&self, node: Node) -> usize {
        match node {
            Node::Local(part, ix) => self.base[part] + ix.index(),
            Node::Graph(part) => self.base[part + 1] - 1,
            Node::Other(i) => self.keys() + i,
        }
    }

    pub fn node(&mut self, at: At) -> Node {
        if let Some(part) = at.part {
            if at.node == at.graph {
                return Node::Graph(part);
            }
            if let Some(ix) = self.parts[part].graph.node_ix(at.node) {
                return Node::Local(part, ix);
            }
        }
        if let Some(&i) = self.other_keys.get(&at) {
            return Node::Other(i);
        }
        let i = self.others.len();
        self.others.push(at);
        self.other_keys.insert(at, i);
        Node::Other(i)
    }

    pub fn at(&self, node: Node) -> At {
        match node {
            Node::Local(part, ix) => At {
                part: Some(part),
                graph: self.parts[part].id,
                node: self.parts[part].graph.node_at(ix).id(),
            },
            Node::Graph(part) => At {
                part: Some(part),
                graph: self.parts[part].id,
                node: self.parts[part].id,
            },
            Node::Other(i) => self.others[i],
        }
    }

    pub fn graph_id(&self, part: usize) -> Id {
        self.parts[part].id
    }

    pub fn state(&self, node: Node) -> Option<&'static str> {
        match node {
            Node::Other(i) => self.space.state(self.others[i]),
            _ => None,
        }
    }

    pub fn cell(&self, node: Node) -> NodeCell {
        match node {
            Node::Local(part, ix) => query::node_cell(self.parts[part].graph, ix),
            _ => self.space.cell(self.at(node)),
        }
    }

    pub fn graph_name(&self, node: Node) -> String {
        match node {
            Node::Local(part, _) | Node::Graph(part) => self.space.aliases[part].clone(),
            Node::Other(i) => self.space.graph_name(self.others[i]),
        }
    }

    pub fn address(&self, node: Node) -> String {
        match node {
            Node::Local(part, ix) => crate::address(
                &self.space.aliases[part],
                self.parts[part].graph.node_at(ix).id(),
            ),
            _ => self.space.address(self.at(node)),
        }
    }

    pub fn candidates(&self, node: Node) -> Vec<String> {
        match node {
            Node::Other(i) => self.space.candidates(self.others[i]),
            _ => Vec::new(),
        }
    }

    pub fn lacks_vector(&self, node: Node) -> bool {
        match node {
            Node::Local(part, ix) => query::lacks_vector(self.parts[part].graph, ix),
            Node::Graph(_) => false,
            Node::Other(i) => self.space.lacks_vector(self.others[i]),
        }
    }

    /// The provenance edges into `ix` as (from, kind), in the order `ReadSpace::provenance_steps` lists them.
    fn provenance_into(&mut self, part: usize, ix: NodeIx) -> &[(NodeIx, NodeIx, &'static str)] {
        let p = &mut self.parts[part];
        let graph = p.graph;
        let index = p.provenance_in.get_or_insert_with(|| {
            let mut index: Vec<_> = graph
                .provenance_edges()
                .map(|e| (e.to.ix(), e.from.ix(), e.kind))
                .collect();
            index.sort_by_key(|&(to, _, _)| to);
            index
        });
        let start = index.partition_point(|&(to, _, _)| to < ix);
        let end = index.partition_point(|&(to, _, _)| to <= ix);
        &index[start..end]
    }

    /// `ReadSpace::steps` for `at`, into `out`: the same steps in the same order, as borrowed parts.
    pub fn steps(
        &mut self,
        at: Node,
        direction: Direction,
        relation: Option<&str>,
        out: &mut Vec<Step<'a>>,
    ) {
        out.clear();
        let (part, ix) = match at {
            Node::Local(part, ix) => (part, Some(ix)),
            Node::Graph(part) => (part, None),
            Node::Other(_) => return,
        };
        let graph = self.parts[part].graph;
        if ix.is_some_and(|ix| !self.parts[part].expand_provenance && graph.is_provenance(ix)) {
            return;
        }
        let wants_out = matches!(direction, Direction::Out | Direction::Both);
        let wants_in = matches!(direction, Direction::In | Direction::Both);
        let derived = |to, kind, way, score| Step {
            to,
            owner: part,
            kind,
            id: None,
            way,
            score,
            derived: true,
        };
        if let Some(ix) = ix {
            if self.include_provenance
                && relation.is_none_or(|kind| matches!(kind, "authored_by" | "edited_by"))
            {
                if wants_out {
                    for e in graph.node_provenance_edges(ix) {
                        if relation.is_none_or(|r| r == e.kind) {
                            out.push(derived(
                                Node::Local(part, e.to.ix()),
                                e.kind,
                                Way::Out,
                                None,
                            ));
                        }
                    }
                }
                if wants_in {
                    for &(_, from, kind) in self.provenance_into(part, ix) {
                        if relation.is_none_or(|r| r == kind) {
                            out.push(derived(Node::Local(part, from), kind, Way::In, None));
                        }
                    }
                }
            }
            let shown = |n: NodeIx| self.include_provenance || !graph.is_provenance(n);
            if relation == Some(SIMILAR_TO) {
                if wants_out {
                    for e in graph.similar_out(ix).iter().filter(|e| shown(e.other)) {
                        let to = Node::Local(part, e.other);
                        out.push(derived(to, SIMILAR_TO, Way::Out, Some(e.score)));
                    }
                }
                if wants_in {
                    for e in graph.similar_in(ix).iter().filter(|e| shown(e.other)) {
                        let to = Node::Local(part, e.other);
                        out.push(derived(to, SIMILAR_TO, Way::In, Some(e.score)));
                    }
                }
            }
            let symmetric = &self.parts[part].symmetric;
            let keep = |kind: &str| match relation {
                Some(r) => kind == r,
                None => kind != SIMILAR_TO,
            };
            let stored = |s: umx_graph::Step<'a>, way| Step {
                to: Node::Local(part, s.other),
                owner: part,
                kind: s.kind.as_str(),
                id: Some(graph.edge_at(s.edge).id()),
                way,
                score: None,
                derived: false,
            };
            for s in graph.edges_out(ix) {
                let kind = s.kind.as_str();
                if (wants_out || symmetric.contains(&kind)) && keep(kind) && shown(s.other) {
                    out.push(stored(s, Way::Out));
                }
            }
            for s in graph.edges_in(ix) {
                let kind = s.kind.as_str();
                let either = symmetric.contains(&kind);
                if (wants_in || either)
                    && !(either && s.other == ix)
                    && keep(kind)
                    && shown(s.other)
                {
                    out.push(stored(s, Way::In));
                }
            }
        }
        if self.may_be_external(at) {
            let here = self.at(at);
            for hop in self.space.external_steps(here, direction, relation, true) {
                let id = hop.edge.id.expect("a stored edge");
                let kind = self.parts[hop.owner]
                    .graph
                    .edge(id)
                    .expect("indexed edge")
                    .kind();
                let to = self.node(hop.to);
                out.push(Step {
                    to,
                    owner: hop.owner,
                    kind,
                    id: Some(id),
                    way: hop.edge.way,
                    score: None,
                    derived: false,
                });
            }
        }
        let lead = out.iter().take_while(|s| s.derived).count();
        let parts = &self.parts;
        out[lead..].sort_by_key(|s| (s.way == Way::In, parts[s.owner].id, s.id));
    }

    /// The nodes `x` is one forward step from: every `w` that [`steps`](Self::steps) expands and whose steps reach
    /// `x`. Not for similar_to (see [`path`](Self::path)).
    fn sources(
        &mut self,
        x: Node,
        direction: Direction,
        relation: Option<&str>,
        out: &mut Vec<Node>,
    ) {
        out.clear();
        if let Node::Local(part, ix) = x {
            let graph = self.parts[part].graph;
            let expand_provenance = self.parts[part].expand_provenance;
            let expands = |w: NodeIx| expand_provenance || !graph.is_provenance(w);
            let wants_out = matches!(direction, Direction::Out | Direction::Both);
            let wants_in = matches!(direction, Direction::In | Direction::Both);
            if self.include_provenance
                && relation.is_none_or(|kind| matches!(kind, "authored_by" | "edited_by"))
            {
                if wants_out {
                    for &(_, from, kind) in self.provenance_into(part, ix) {
                        if relation.is_none_or(|r| r == kind) && expands(from) {
                            out.push(Node::Local(part, from));
                        }
                    }
                }
                if wants_in {
                    for e in graph.node_provenance_edges(ix) {
                        if relation.is_none_or(|r| r == e.kind) && expands(e.to.ix()) {
                            out.push(Node::Local(part, e.to.ix()));
                        }
                    }
                }
            }
            if self.include_provenance || !graph.is_provenance(ix) {
                let symmetric = &self.parts[part].symmetric;
                let keep = |kind: &str| match relation {
                    Some(r) => kind == r,
                    None => kind != SIMILAR_TO,
                };
                for s in graph.edges_in(ix) {
                    let kind = s.kind.as_str();
                    if (wants_out || symmetric.contains(&kind)) && keep(kind) && expands(s.other) {
                        out.push(Node::Local(part, s.other));
                    }
                }
                for s in graph.edges_out(ix) {
                    let kind = s.kind.as_str();
                    let either = symmetric.contains(&kind);
                    if (wants_in || either)
                        && !(either && s.other == ix)
                        && keep(kind)
                        && expands(s.other)
                    {
                        out.push(Node::Local(part, s.other));
                    }
                }
            }
        }
        // An external step from w to x is the same edge taken the other way from x: `external_steps` resolves both
        // ends and checks both are visible whichever end it starts from.
        if self.may_be_external(x) {
            let back = match direction {
                Direction::Out => Direction::In,
                Direction::In => Direction::Out,
                Direction::Both => Direction::Both,
            };
            for hop in self.space.external_steps(self.at(x), back, relation, true) {
                let w = self.node(hop.to);
                if self.expands(w) {
                    out.push(w);
                }
            }
        }
    }

    /// A shortest path from `from` to `to` of at most `max_depth` steps, as the nodes in order with the step that led
    /// to each: the path a breadth-first search from `from` finds first (ties go to the earlier step). Searches from
    /// both ends unless the relation is similar_to (two steps at most) or an end is not a loaded node or graph node.
    pub fn path(
        &mut self,
        from: Node,
        to: Node,
        max_depth: usize,
        direction: Direction,
        relation: Option<&str>,
    ) -> Vec<(Node, Option<Step<'a>>)> {
        if from == to {
            return alloc::vec![(from, None)];
        }
        if relation != Some(SIMILAR_TO)
            && !matches!(from, Node::Other(_))
            && !matches!(to, Node::Other(_))
        {
            self.two_sided(from, to, max_depth, direction, relation)
        } else {
            self.one_sided(from, to, max_depth, direction, relation)
        }
    }

    fn one_sided(
        &mut self,
        from: Node,
        to: Node,
        max_depth: usize,
        direction: Direction,
        relation: Option<&str>,
    ) -> Vec<(Node, Option<Step<'a>>)> {
        let mut seen = Bits::new(self.keys());
        seen.insert(self.key(from));
        let mut visits = alloc::vec![Visit {
            node: from,
            parent: 0,
            step: None,
        }];
        let mut queue = alloc::collections::VecDeque::from([(0usize, 0usize)]);
        let mut buf = Vec::new();
        while let Some((v, depth)) = queue.pop_front() {
            if depth == max_depth {
                continue;
            }
            self.steps(visits[v].node, direction, relation, &mut buf);
            for step in &buf {
                if !seen.insert(self.key(step.to)) {
                    continue;
                }
                visits.push(Visit {
                    node: step.to,
                    parent: v,
                    step: Some(*step),
                });
                if step.to == to {
                    return chain(&visits, visits.len() - 1, Vec::new());
                }
                if self.state(step.to).is_none() {
                    queue.push_back((visits.len() - 1, depth + 1));
                }
            }
        }
        Vec::new()
    }

    /// Whole layers from both ends, the smaller frontier first, until they meet at length `d = a + b`. The forward
    /// layers are the one-sided search's own, in its queue order. Past layer `a`, the one-sided search reaches the end
    /// along firsts: a node one step before a node on a shortest path is on one too, so a node on a shortest path is
    /// first reached from the earliest such node of the layer before, and the earliest such node of a layer is the
    /// first such node that the earliest one of the layer before reaches. So the path is the first node of layer `a`
    /// that is `b` steps from the end, then at each layer the first step to a node one step nearer.
    fn two_sided(
        &mut self,
        from: Node,
        to: Node,
        max_depth: usize,
        direction: Direction,
        relation: Option<&str>,
    ) -> Vec<(Node, Option<Step<'a>>)> {
        // fwd: visit index + 1. back: steps to `to` + 1.
        let mut fwd = Marks::new(self.keys());
        let mut back = Marks::new(self.keys());
        fwd.set(self.key(from), 1);
        back.set(self.key(to), 1);
        let mut visits = alloc::vec![Visit {
            node: from,
            parent: 0,
            step: None,
        }];
        // The forward frontier as visit indexes, in queue order, and the backward one.
        let mut front: Vec<usize> = alloc::vec![0];
        let mut rear = alloc::vec![to];
        let (mut a, mut b) = (0usize, 0usize);
        let mut steps = Vec::new();
        let mut sources = Vec::new();
        loop {
            if a + b == max_depth || front.is_empty() || rear.is_empty() {
                return Vec::new();
            }
            let mut met = false;
            if front.len() <= rear.len() {
                let mut next = Vec::new();
                for &v in &front {
                    self.steps(visits[v].node, direction, relation, &mut steps);
                    for step in &steps {
                        let key = self.key(step.to);
                        if fwd.get(key) != 0 {
                            continue;
                        }
                        visits.push(Visit {
                            node: step.to,
                            parent: v,
                            step: Some(*step),
                        });
                        fwd.set(key, visits.len() as u32);
                        met |= back.get(key) != 0;
                        next.push(visits.len() - 1);
                    }
                }
                front = next;
                a += 1;
            } else {
                let mut next = Vec::new();
                for &x in &rear {
                    self.sources(x, direction, relation, &mut sources);
                    for &w in &sources {
                        let key = self.key(w);
                        if back.get(key) != 0 {
                            continue;
                        }
                        back.set(key, b as u32 + 2);
                        met |= fwd.get(key) != 0;
                        next.push(w);
                    }
                }
                rear = next;
                b += 1;
            }
            if met {
                break;
            }
        }
        let d = a + b;
        let near = |handles: &Self, node: Node| back.get(handles.key(node)) as usize;
        let first = *front
            .iter()
            .find(|&&v| near(self, visits[v].node) == b + 1)
            .expect("the layers met");
        let mut tail = Vec::with_capacity(b);
        let mut at = visits[first].node;
        for k in a..d {
            self.steps(at, direction, relation, &mut steps);
            let step = *steps
                .iter()
                .find(|s| near(self, s.to) == d - k)
                .expect("a node on a shortest path has a next one");
            tail.push((step.to, Some(step)));
            at = step.to;
        }
        tail.reverse();
        chain(&visits, first, tail)
    }
}

/// The forward visits from the start to `end`, start first, then `tail` (the rest of the path, end first).
fn chain<'a>(
    visits: &[Visit<'a>],
    end: usize,
    mut tail: Vec<(Node, Option<Step<'a>>)>,
) -> Vec<(Node, Option<Step<'a>>)> {
    let mut v = end;
    loop {
        tail.push((visits[v].node, visits[v].step));
        if visits[v].step.is_none() {
            break;
        }
        v = visits[v].parent;
    }
    tail.reverse();
    tail
}
