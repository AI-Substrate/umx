//! Everyday query verbs (req-21, req-28): neighbours, walk and path over local adjacency and the services read
//! space. A capability of its own (blueprint d-12), with no state or foreign umx-graph handles.
//!
//! Every result is deterministic: edges are taken out before in, each in edge id order (umx-graph's adjacency
//! order), and walks are breadth first. `similar_to` edges are followed only when the request names that relation
//! (workshop 008 section 9).

use crate::graph_store::ReadScope;
use crate::{Ctx, Done, GraphStore, Outcome, Page, lazy_page, page, problems};
use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use umx_core::{Id, Problem};
use umx_graph::{Graph, NodeIx, Step};

const SIMILAR_TO: &str = "similar_to";

/// Which edges to follow from a node: those leaving it, those arriving at it, or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Out,
    In,
    Both,
}

/// How an edge was taken: along it (`Out`, from its `from` to its `to`) or against it (`In`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    Out,
    In,
}

impl Way {
    pub fn as_str(self) -> &'static str {
        match self {
            Way::Out => "out",
            Way::In => "in",
        }
    }
}

/// A node as a row shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeCell {
    pub id: Id,
    pub kind: String,
    pub title: String,
    pub status: String,
}

/// An edge as a row shows it: its id, relation and which way it was taken. A similar_to edge from the stored lists
/// (workshop 008 section 9) is derived: it has no id (it is not a record, so `umx edge get` cannot name it) and
/// carries its score in ten-thousandths. It is identified by (from, to, similar_to), unique because a list holds
/// each other node at most once.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeCell {
    pub id: Option<Id>,
    pub relation: String,
    pub way: Way,
    pub score: Option<i32>,
    pub derived: bool,
}

/// One step from a node: the node at the other end and the edge taken.
#[derive(Clone, Debug)]
pub(crate) struct Hop {
    pub(crate) other: NodeIx,
    pub(crate) edge: EdgeCell,
}

/// The most steps a walk or path may take along similar_to (workshop 008 section 9: a ranking, not a fact, so long
/// chains of it mean little and cost much).
pub const SIMILAR_TO_DEPTH: usize = 2;

/// One edge at a node and the node at its other end.
#[derive(Clone, Debug, PartialEq)]
pub struct NeighbourRow {
    pub address: String,
    pub graph: Id,
    pub from_graph: Id,
    pub to_graph: Id,
    pub node_graph: Id,
    pub from: Id,
    pub to: Id,
    pub state: Option<&'static str>,
    pub candidates: Vec<String>,
    pub edge: EdgeCell,
    pub node: NodeCell,
}

/// One node reached by a walk: how far, from which node, by which edge.
#[derive(Clone, Debug, PartialEq)]
pub struct WalkRow {
    pub depth: usize,
    pub node: NodeCell,
    pub from: Id,
    pub edge: EdgeCell,
    pub graph: String,
    pub graph_id: Id,
    pub address: String,
    pub from_graph: Id,
    pub edge_graph: Id,
    pub state: Option<&'static str>,
    pub candidates: Vec<String>,
}

/// One step of a path: the node, and the edge that led to it (none for the first).
#[derive(Clone, Debug, PartialEq)]
pub struct PathRow {
    pub step: usize,
    pub node: NodeCell,
    pub edge: Option<EdgeCell>,
    pub graph: String,
    pub graph_id: Id,
    pub address: String,
    pub state: Option<&'static str>,
    pub candidates: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct NeighboursRequest {
    pub node: String,
    pub relation: Option<String>,
    pub direction: Direction,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Clone, Debug)]
pub struct WalkRequest {
    pub node: String,
    pub depth: usize,
    pub relation: Option<String>,
    pub direction: Direction,
    /// Only nodes under this one by part_of are listed; the walk itself goes anywhere.
    pub within: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Clone, Debug)]
pub struct PathRequest {
    pub from: String,
    pub to: String,
    pub max_depth: usize,
    pub relation: Option<String>,
    pub direction: Direction,
}

/// The query capability. Stateless; every call names the store it reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct QueryService;

/// The steps from `at` this request follows, in order. Stored edges: out before in, each in edge id order;
/// `similar_to` only when the request names it. Naming `similar_to` follows the stored lists: out is the node's top k
/// (best first), in is every node whose top k holds it (in id order), then any stored edge records named similar_to
/// (an older graph's), listed after and never merged into the lists. A stored edge on a relation in `symmetric` (D49)
/// is one edge both ways, stored once, so it is followed from either end whatever the direction; a symmetric
/// self-loop is one step (as out), never two (R2). Directed self-loops and parallel edges are unchanged.
pub(crate) fn hops(
    graph: &Graph,
    at: NodeIx,
    direction: Direction,
    relation: Option<&str>,
    symmetric: &[&str],
    include_provenance: bool,
) -> Vec<Hop> {
    let scope = ReadScope::new(graph).with_provenance(include_provenance);
    if !scope.contains_ix(at) {
        return Vec::new();
    }
    let out = matches!(direction, Direction::Out | Direction::Both);
    let inc = matches!(direction, Direction::In | Direction::Both);
    let mut hops = Vec::new();
    if relation == Some(SIMILAR_TO) {
        let derived = |e: &umx_graph::SimilarEdge, way| Hop {
            other: e.other,
            edge: EdgeCell {
                id: None,
                relation: SIMILAR_TO.to_string(),
                way,
                score: Some(e.score),
                derived: true,
            },
        };
        if out {
            hops.extend(graph.similar_out(at).iter().map(|e| derived(e, Way::Out)));
        }
        if inc {
            hops.extend(graph.similar_in(at).iter().map(|e| derived(e, Way::In)));
        }
    }
    let stored = |s: Step<'_>, way| Hop {
        other: s.other,
        edge: EdgeCell {
            id: Some(graph.edge_at(s.edge).id()),
            relation: s.kind.as_str().to_string(),
            way,
            score: None,
            derived: false,
        },
    };
    let keep = |s: &Step<'_>| match relation {
        Some(r) => s.kind.as_str() == r,
        None => s.kind.as_str() != SIMILAR_TO,
    };
    let either = |s: &Step<'_>| symmetric.contains(&s.kind.as_str());
    hops.extend(
        graph
            .edges_out(at)
            .filter(|s| (out || either(s)) && keep(s))
            .map(|s| stored(s, Way::Out)),
    );
    hops.extend(
        graph
            .edges_in(at)
            .filter(|s| (inc || either(s)) && !(either(s) && s.other == at) && keep(s))
            .map(|s| stored(s, Way::In)),
    );
    hops.retain(|h| scope.contains_ix(h.other));
    hops
}

/// Whether `node` has no whole-node vector in the lists' space (it has no list and is in none).
pub(crate) fn lacks_vector(graph: &Graph, node: NodeIx) -> bool {
    !graph
        .similar_space()
        .and_then(|s| graph.whole_block(s))
        .is_some_and(|b| b.has(node.index()))
}

/// `similar_to_incomplete` for a similar_to read whose lists are being rebuilt or that touches a node with no
/// whole-node vector yet (`missing`).
pub(crate) fn similar_warnings(
    store: &GraphStore,
    alias: &str,
    relation: Option<&str>,
    missing: bool,
) -> Vec<Problem> {
    if relation != Some(SIMILAR_TO) {
        return Vec::new();
    }
    if store.similar_stale(alias) || missing {
        vec![problems::similar_to_incomplete(
            &crate::graph_store::alias_name(alias),
        )]
    } else {
        Vec::new()
    }
}

/// A walk or path along similar_to deeper than [`SIMILAR_TO_DEPTH`] is refused.
fn similar_depth(relation: Option<&str>, depth: usize) -> Result<(), Problem> {
    if relation == Some(SIMILAR_TO) && depth > SIMILAR_TO_DEPTH {
        return Err(problems::usage_invalid(
            "walks and paths along similar_to go at most 2 steps (--depth 2 or --max-depth 2)",
            "start",
        ));
    }
    Ok(())
}

pub(crate) fn node_cell(graph: &Graph, ix: NodeIx) -> NodeCell {
    let n = graph.node_at(ix);
    NodeCell {
        id: n.id(),
        kind: n.kind().to_string(),
        title: n.title().to_string(),
        status: n.status().to_string(),
    }
}

fn handle(graph: &Graph, id: Option<Id>) -> NodeIx {
    graph
        .node_ix(id.expect("asked for"))
        .expect("a resolved node is in its graph")
}

impl QueryService {
    /// `umx neighbours <node>`: every edge at the node (both ways unless `direction` says otherwise) and the node
    /// at its other end.
    pub fn neighbours(
        &self,
        store: &GraphStore,
        ctx: &Ctx,
        req: NeighboursRequest,
    ) -> Outcome<Page<NeighbourRow>> {
        if req.relation.as_deref() != Some(SIMILAR_TO)
            || ctx.scope.is_some()
            || ctx.graph.as_deref().is_some_and(|g| g.contains(','))
            || req.node.starts_with("umx:")
        {
            return crate::traversal::neighbours(store, ctx, req);
        }
        let refs = [Some(req.node.as_str())];
        let access = store.external_access(ctx)?;
        let alias = store.external_read_alias(ctx, &refs)?;
        let single = Ctx {
            graph: Some(alias.clone()),
            scope: None,
            default_graph: None,
            ..ctx.clone()
        };
        let (_, l, ids) = store.resolve_nodes(&single, &refs)?;
        let g = &l.graph;
        let at = handle(g, ids[0]);
        let relation = req.relation.as_deref();
        let symmetric = l.ontology().symmetric_kinds();
        let hops = hops(
            g,
            at,
            req.direction,
            relation,
            &symmetric,
            ctx.include_provenance,
        );
        let mut touched: Vec<NodeIx> = vec![at];
        touched.extend(hops.iter().map(|h| h.other));
        let missing = touched.iter().any(|&ix| lacks_vector(g, ix));
        let mut warnings = similar_warnings(store, &alias, relation, missing);
        let mut rows: Vec<NeighbourRow> = hops
            .into_iter()
            .filter(|hop| hop.edge.derived)
            .map(|h| NeighbourRow {
                address: crate::address(&alias, g.node_at(h.other).id()),
                graph: l.meta.id,
                from_graph: l.meta.id,
                to_graph: l.meta.id,
                node_graph: l.meta.id,
                from: if h.edge.way == Way::Out {
                    ids[0].expect("start")
                } else {
                    g.node_at(h.other).id()
                },
                to: if h.edge.way == Way::Out {
                    g.node_at(h.other).id()
                } else {
                    ids[0].expect("start")
                },
                state: None,
                candidates: Vec::new(),
                node: node_cell(g, h.other),
                edge: h.edge,
            })
            .collect();
        let incident = store.incident_edges(&access, &refs, &alias, ids[0].expect("start"));
        store.clock().now_ms();
        if !store.external_visible(&access, &refs, &alias) {
            rows.clear();
        }
        let derived = rows.len();
        for (owner, id) in incident {
            let Some(row) = store.edge_read_row(&access, &refs, &owner, id) else {
                continue;
            };
            if row.edge.kind == SIMILAR_TO
                && (row.from_graph != row.graph || row.to_graph != row.graph)
            {
                continue;
            }
            if relation.is_some_and(|kind| row.edge.kind != kind)
                || (relation.is_none() && row.edge.kind == SIMILAR_TO)
            {
                continue;
            }
            let source = store.loaded(&owner).expect("edge owner loaded");
            let symmetric = source.ontology().is_symmetric(&row.edge.kind);
            let here = (l.meta.id, ids[0].expect("start"));
            let outgoing = (row.from_graph, row.edge.from) == here;
            let incoming = (row.to_graph, row.edge.to) == here;
            for (way, take) in [
                (
                    Way::Out,
                    outgoing
                        && (symmetric || matches!(req.direction, Direction::Out | Direction::Both)),
                ),
                (
                    Way::In,
                    incoming
                        && (symmetric || matches!(req.direction, Direction::In | Direction::Both))
                        && !(symmetric && outgoing),
                ),
            ] {
                if !take {
                    continue;
                }
                let (graph, id, external) = if way == Way::Out {
                    (row.to_graph, row.edge.to, row.edge.to_graph.is_some())
                } else {
                    (row.from_graph, row.edge.from, row.edge.from_graph.is_some())
                };
                let resolved = if external {
                    store.read_end(&access, &refs, graph, id)
                } else {
                    crate::graph_store::ReadEnd::local(source)
                };
                let state = resolved.state;
                let node = resolved
                    .loaded
                    .filter(|_| state.is_none())
                    .and_then(|loaded| loaded.graph.node(id));
                if state == Some("edge_broken") {
                    warnings.push(problems::edge_broken(
                        row.edge.id.expect("stored incident edge"),
                        if way == Way::Out { "to" } else { "from" },
                        graph,
                        id,
                    ));
                }
                rows.push(NeighbourRow {
                    address: crate::address(&graph.to_text(), id),
                    graph: row.graph,
                    from_graph: row.from_graph,
                    to_graph: row.to_graph,
                    node_graph: graph,
                    from: row.edge.from,
                    to: row.edge.to,
                    state,
                    candidates: resolved.candidates,
                    node: node.map_or_else(
                        || NodeCell {
                            id,
                            kind: String::new(),
                            title: String::new(),
                            status: String::new(),
                        },
                        |node| NodeCell {
                            id,
                            kind: node.kind().into(),
                            title: node.title().into(),
                            status: node.status().into(),
                        },
                    ),
                    edge: EdgeCell {
                        id: row.edge.id,
                        relation: row.edge.kind.clone(),
                        way,
                        score: None,
                        derived: false,
                    },
                });
            }
            rows[derived..].sort_by_key(|row| {
                (
                    if row.edge.way == Way::Out { 0 } else { 1 },
                    row.graph,
                    row.edge.id,
                )
            });
        }
        Ok(Done {
            value: page(&alias, rows, req.limit, req.offset),
            notes: warnings,
        })
    }

    /// `umx walk <node> --depth N`: every node reachable in 1 to N steps, breadth first, each once, at its
    /// shortest depth, with the node and edge it was first reached by. The start node is not listed. Rows are made
    /// lazily and the walk stops after `offset + limit + 1` of them (F41), so a small page of a big graph visits
    /// few nodes; `total` is then unknown (`None`) and `truncated` says more rows exist.
    pub fn walk(&self, store: &GraphStore, ctx: &Ctx, req: WalkRequest) -> Outcome<Page<WalkRow>> {
        if req.depth == 0 {
            return Err(depth_invalid());
        }
        similar_depth(req.relation.as_deref(), req.depth)?;
        if req.relation.as_deref() != Some(SIMILAR_TO)
            || ctx.scope.is_some()
            || ctx.graph.as_deref().is_some_and(|g| g.contains(','))
            || req.node.starts_with("umx:")
        {
            return crate::traversal::walk(store, ctx, req);
        }
        let (alias, l, ids) =
            store.resolve_nodes(ctx, &[Some(&req.node), req.within.as_deref()])?;
        let g = &l.graph;
        let start = handle(g, ids[0]);
        let mut walk = Walk::new(
            g,
            start,
            &req,
            ids[1],
            l.ontology().symmetric_kinds(),
            ctx.include_provenance,
        );
        let value = lazy_page(&alias, &mut walk, req.limit, req.offset);
        let value = value.map(|mut row| {
            row.graph = alias.clone();
            row.graph_id = l.meta.id;
            row.from_graph = l.meta.id;
            row.edge_graph = l.meta.id;
            row.address = crate::address(&alias, row.node.id);
            row
        });
        // The start and every node the walk reached, though only those it got to before the page was full (R102-4).
        let missing = lacks_vector(g, start) || walk.reached_without_vector;
        let warnings = similar_warnings(store, &alias, req.relation.as_deref(), missing);
        Ok(Done {
            value,
            notes: warnings,
        })
    }

    /// `umx path <from> <to>`: a shortest path of at most `max_depth` edges, as the nodes in order with the edge
    /// that led to each. Ties go to the path found first (edges out before in, in edge id order). No path within
    /// `max_depth` is an empty list, not an error.
    pub fn path(&self, store: &GraphStore, ctx: &Ctx, req: PathRequest) -> Outcome<Page<PathRow>> {
        if req.max_depth == 0 {
            return Err(depth_invalid());
        }
        similar_depth(req.relation.as_deref(), req.max_depth)?;
        if req.relation.as_deref() != Some(SIMILAR_TO)
            || ctx.scope.is_some()
            || ctx.graph.as_deref().is_some_and(|g| g.contains(','))
            || req.from.starts_with("umx:")
            || req.to.starts_with("umx:")
        {
            return crate::traversal::path(store, ctx, req);
        }
        let (alias, l, ids) = store.resolve_nodes(ctx, &[Some(&req.from), Some(&req.to)])?;
        let g = &l.graph;
        let (from, to) = (handle(g, ids[0]), handle(g, ids[1]));
        let symmetric = l.ontology().symmetric_kinds();
        // How each node was first reached: the node before it, the edge and the way.
        let mut prev: Vec<Option<(NodeIx, EdgeCell)>> = vec![None; g.node_bound()];
        let mut seen = vec![false; g.node_bound()];
        seen[from.index()] = true;
        let mut queue = VecDeque::from([(from, 0usize)]);
        let mut found = from == to;
        while let Some((at, depth)) = queue.pop_front() {
            if found || depth == req.max_depth {
                break;
            }
            for h in hops(
                g,
                at,
                req.direction,
                req.relation.as_deref(),
                &symmetric,
                ctx.include_provenance,
            ) {
                if core::mem::replace(&mut seen[h.other.index()], true) {
                    continue;
                }
                let other = h.other;
                prev[other.index()] = Some((at, h.edge));
                if other == to {
                    found = true;
                    break;
                }
                queue.push_back((other, depth + 1));
            }
        }
        let mut rows = Vec::new();
        if found {
            let mut at = to;
            while let Some((before, edge)) = prev[at.index()].take() {
                rows.push((at, Some(edge)));
                at = before;
            }
            rows.push((from, None));
            rows.reverse();
        }
        let touched: Vec<NodeIx> = rows.iter().map(|(ix, _)| *ix).chain([from, to]).collect();
        let missing = touched.iter().any(|&ix| lacks_vector(g, ix));
        let warnings = similar_warnings(store, &alias, req.relation.as_deref(), missing);
        let rows: Vec<PathRow> = rows
            .into_iter()
            .enumerate()
            .map(|(step, (ix, edge))| PathRow {
                step,
                node: node_cell(g, ix),
                edge,
                graph: alias.clone(),
                graph_id: l.meta.id,
                address: crate::address(&alias, g.node_at(ix).id()),
                state: None,
                candidates: Vec::new(),
            })
            .collect();
        let total = rows.len();
        Ok(Done {
            value: page(&alias, rows, total, 0),
            notes: warnings,
        })
    }
}

/// A breadth-first walk that yields rows as it reaches nodes, in the order `umx walk` lists them: a node's edges
/// out before in, each in edge id order. It expands a node (lists its edges) only when the rows before it are used
/// up, so stopping early leaves most of a big graph unvisited.
pub(crate) struct Walk<'g> {
    graph: &'g Graph,
    include_provenance: bool,
    depth: usize,
    relation: Option<&'g str>,
    direction: Direction,
    /// The graph's symmetric relations (D49), followed from either end.
    symmetric: Vec<&'g str>,
    within: Option<umx_graph::NodeSet<'g>>,
    seen: Vec<bool>,
    queue: VecDeque<(NodeIx, usize)>,
    /// Rows found at the node being expanded, not yet handed out.
    ready: VecDeque<WalkRow>,
    /// Nodes whose edges were listed so far.
    pub(crate) expanded: usize,
    /// A similar_to walk reached a node with no whole-node vector (its results are then incomplete).
    reached_without_vector: bool,
}

impl<'g> Walk<'g> {
    fn new(
        graph: &'g Graph,
        start: NodeIx,
        req: &'g WalkRequest,
        within: Option<Id>,
        symmetric: Vec<&'g str>,
        include_provenance: bool,
    ) -> Walk<'g> {
        let mut seen = vec![false; graph.node_bound()];
        seen[start.index()] = true;
        Walk {
            graph,
            include_provenance,
            depth: req.depth,
            relation: req.relation.as_deref(),
            direction: req.direction,
            symmetric,
            within: within.map(|root| {
                ReadScope::new(graph)
                    .with_provenance(include_provenance)
                    .within(root)
            }),
            seen,
            queue: VecDeque::from([(start, 0usize)]),
            ready: VecDeque::new(),
            expanded: 0,
            reached_without_vector: false,
        }
    }
}

impl Iterator for Walk<'_> {
    type Item = WalkRow;

    fn next(&mut self) -> Option<WalkRow> {
        loop {
            if let Some(row) = self.ready.pop_front() {
                return Some(row);
            }
            let (at, depth) = self.queue.pop_front()?;
            if depth == self.depth {
                continue;
            }
            self.expanded += 1;
            let g = self.graph;
            for h in hops(
                g,
                at,
                self.direction,
                self.relation,
                &self.symmetric,
                self.include_provenance,
            ) {
                if core::mem::replace(&mut self.seen[h.other.index()], true) {
                    continue;
                }
                self.queue.push_back((h.other, depth + 1));
                if self.relation == Some(SIMILAR_TO) && lacks_vector(g, h.other) {
                    self.reached_without_vector = true;
                }
                let node = node_cell(g, h.other);
                if self.within.as_ref().is_some_and(|w| !w.contains(node.id)) {
                    continue;
                }
                self.ready.push_back(WalkRow {
                    depth: depth + 1,
                    from: g.node_at(at).id(),
                    edge: h.edge,
                    node,
                    graph: String::new(),
                    graph_id: Id(0),
                    address: String::new(),
                    from_graph: Id(0),
                    edge_graph: Id(0),
                    state: None,
                    candidates: Vec::new(),
                });
            }
        }
    }
}

/// `usage_invalid` for a depth that walks nowhere.
fn depth_invalid() -> Problem {
    problems::usage_invalid("--depth and --max-depth are 1 or more", "start")
}

#[cfg(test)]
mod tests {
    use super::*;
    use umx_core::{Edge, Node};

    fn node(id: u128) -> Node {
        Node {
            id: Id(id),
            kind: "problem".into(),
            title: alloc::format!("n{id}"),
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
            fields: Default::default(),
        }
    }

    fn edge(id: u128, from: u128, to: u128) -> Edge {
        Edge {
            id: Id(id),
            kind: "blocks".into(),
            from_graph: None,
            from: Id(from),
            to_graph: None,
            to: Id(to),
            created: String::new(),
            created_by: String::new(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: Default::default(),
        }
    }

    /// Node 1 with `fan` children, each with `fan` children, and so on to `n` nodes; edges numbered after nodes.
    fn tree(n: u128, fan: u128) -> Graph {
        let nodes = (1..=n).map(node).collect();
        let edges = (2..=n).map(|c| edge(n + c, (c - 2) / fan + 1, c)).collect();
        Graph::from_records(nodes, edges)
    }

    fn request(depth: usize, limit: usize, offset: usize) -> WalkRequest {
        WalkRequest {
            node: String::new(),
            depth,
            relation: None,
            direction: Direction::Out,
            within: None,
            limit,
            offset,
        }
    }

    #[test]
    fn a_small_page_of_a_big_graph_expands_few_nodes() {
        // F41: 50k nodes, every one reachable; one page of 10 must not walk the graph.
        let g = tree(50_000, 3);
        let start = g.node_ix(Id(1)).unwrap();
        let req = request(50_000, 10, 0);
        let mut walk = Walk::new(&g, start, &req, None, Vec::new(), false);
        let page = lazy_page("t", walk.by_ref(), req.limit, req.offset);
        assert_eq!(
            (page.rows.len(), page.total, page.truncated),
            (10, None, true)
        );
        assert!(
            walk.expanded <= 5,
            "expanded {} nodes for 11 rows",
            walk.expanded
        );
        // The whole walk, for scale: every node but the start is a row, and every node with children is expanded.
        let mut all = Walk::new(&g, start, &req, None, Vec::new(), false);
        assert_eq!(all.by_ref().count(), 49_999);
        assert!(all.expanded > 16_000, "{}", all.expanded);
    }

    #[test]
    fn lazy_pages_are_slices_of_the_whole_walk() {
        let g = tree(200, 4);
        let start = g.node_ix(Id(1)).unwrap();
        for depth in [1, 2, 3, 10] {
            let req = request(depth, 0, 0);
            let all: Vec<WalkRow> = Walk::new(&g, start, &req, None, Vec::new(), false).collect();
            for offset in 0..all.len() + 2 {
                for limit in [0, 1, 3, 7, all.len(), all.len() + 1] {
                    let lazy = lazy_page(
                        "t",
                        Walk::new(&g, start, &req, None, Vec::new(), false),
                        limit,
                        offset,
                    );
                    let eager = page("t", all.clone(), limit, offset);
                    assert_eq!((&lazy.rows, lazy.truncated), (&eager.rows, eager.truncated));
                    let total = if eager.truncated { None } else { eager.total };
                    assert_eq!(
                        lazy.total, total,
                        "depth {depth} offset {offset} limit {limit}"
                    );
                }
            }
        }
    }
}

/// PRD 002 search-08: how many of a search hit's links it carries.
pub const HIT_LINKS: usize = 3;

/// One stored link a search hit carries: its relation, which way it points from the hit, and the other node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitLink {
    pub relation: String,
    pub way: Way,
    pub id: Id,
    pub title: String,
}

/// A search hit's place in its graph (PRD 002 search-08): up to [`HIT_LINKS`] of its stored links and how many more
/// there are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HitLinks {
    pub shown: Vec<HitLink>,
    pub more: usize,
}

/// PRD 002 search-08: the stored links of node `id` in loaded graph `alias`, read through the same adjacency
/// `neighbours` reads ([`hops`]), so only that one node's edges are read. Kept: links a person or agent stored, both
/// ends in this graph and visible to a read (provenance nodes hidden). Left out and not counted: derived relations
/// (`similar_to`, `authored_by`, `edited_by`), provenance relations, and any link with an end in another graph,
/// which this graph's adjacency does not hold. Order: `part_of` (the hierarchy `--parent` writes and `--within`
/// walks) first, then by relation, out before in, then the other node's id; titles are read only for those shown.
/// None when the graph is not loaded or the node is not in it.
pub(crate) fn hit_links(store: &GraphStore, alias: &str, id: Id) -> Option<HitLinks> {
    let l = store.loaded(alias)?;
    let g = &l.graph;
    let at = g.node_ix(id)?;
    let symmetric = l.ontology().symmetric_kinds();
    let mut links: Vec<(bool, String, Way, Id, NodeIx)> =
        hops(g, at, Direction::Both, None, &symmetric, false)
            .into_iter()
            .filter(|h| {
                // A link from the node to itself is one link: `hops` lists it out and in, so keep the out one.
                !(h.edge.way == Way::In && h.other == at)
                    && !h.edge.derived
                    && !umx_core::ontology::is_derived_relation(&h.edge.relation)
                    && !umx_core::ontology::PROVENANCE_RELATIONS.contains(&h.edge.relation.as_str())
            })
            .map(|h| {
                let other = g.node_at(h.other).id();
                (
                    h.edge.relation != umx_core::ontology::PART_OF,
                    h.edge.relation,
                    h.edge.way,
                    other,
                    h.other,
                )
            })
            .collect();
    links.sort_by(|a, b| (a.0, &a.1, a.2 == Way::In, a.3).cmp(&(b.0, &b.1, b.2 == Way::In, b.3)));
    let more = links.len().saturating_sub(HIT_LINKS);
    let shown = links
        .into_iter()
        .take(HIT_LINKS)
        .map(|(_, relation, way, id, ix)| HitLink {
            relation,
            way,
            id,
            title: g.node_at(ix).title().to_string(),
        })
        .collect();
    Some(HitLinks { shown, more })
}
