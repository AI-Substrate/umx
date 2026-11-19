//! Qualified read space. Selection and policy happen before adjacency is read, and again before publication.

mod bfs;
#[cfg(test)]
mod differential;

use crate::graph_store::{Searched, resolve_node_in, split_address};
use crate::query::{
    self, Direction, EdgeCell, NodeCell, PathRequest, PathRow, WalkRequest, WalkRow, Way,
};
use crate::{Ctx, Done, GraphStore, Outcome, Page, address, lazy_page, page, problems};
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use umx_core::{Id, Problem};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct At {
    pub part: Option<usize>,
    pub graph: Id,
    pub node: Id,
}

#[derive(Clone, PartialEq, Eq)]
struct Target {
    part: Option<usize>,
    state: Option<&'static str>,
    candidates: Vec<String>,
}

type ProvenanceIncoming = BTreeMap<Id, Vec<(Id, &'static str)>>;

pub(crate) struct ReadSpace<'a> {
    pub store: &'a GraphStore,
    pub aliases: Vec<String>,
    access: Ctx,
    refs: Vec<Option<&'a str>>,
    targets: BTreeMap<Id, Target>,
    provenance_in: RefCell<BTreeMap<usize, ProvenanceIncoming>>,
}

pub(crate) struct Hop {
    pub to: At,
    pub owner: usize,
    pub edge: EdgeCell,
}

fn unavailable(state: Option<&str>) -> bool {
    matches!(state, Some("graph_not_loaded" | "copy_ambiguous"))
}

fn placeholder_names(store: &GraphStore, needed: bool) -> (BTreeMap<Id, String>, Vec<Problem>) {
    if needed {
        store.placeholder_aliases()
    } else {
        (BTreeMap::new(), Vec::new())
    }
}

impl<'a> ReadSpace<'a> {
    pub fn from_aliases(
        store: &'a GraphStore,
        ctx: &Ctx,
        aliases: Vec<String>,
        named: bool,
    ) -> Self {
        let access = Ctx {
            graph: named.then(|| aliases.join(",")),
            scope: None,
            default_graph: None,
            ..ctx.clone()
        };
        let mut space = Self {
            store,
            aliases,
            access,
            refs: Vec::new(),
            targets: BTreeMap::new(),
            provenance_in: RefCell::new(BTreeMap::new()),
        };
        space.resolve_targets();
        space
    }

    pub fn new(
        store: &'a GraphStore,
        ctx: &Ctx,
        refs: &[Option<&'a str>],
        noun: &'static str,
    ) -> Result<(Self, Searched), Problem> {
        if refs
            .iter()
            .flatten()
            .any(|r| r.starts_with("umx:") && (r.contains('#') || r.contains('?')))
        {
            return Err(problems::usage_invalid(
                "a traversal address has no selector or fragment",
                noun,
            ));
        }
        let addressed: Vec<_> = refs
            .iter()
            .enumerate()
            .filter_map(|(index, r)| r.and_then(|r| split_address(r).0).map(|name| (index, name)))
            .collect();
        let explicit = ctx.scope.is_some() || ctx.graph.is_some();
        let scope_ctx = Ctx {
            default_graph: if addressed.is_empty() {
                ctx.default_graph.clone()
            } else {
                None
            },
            ..ctx.clone()
        };
        let mut names = Vec::new();
        if !explicit {
            for &(index, selector) in &addressed {
                match store.read_selector_alias(&scope_ctx, selector) {
                    Ok(alias) if !names.contains(&alias) => names.push(alias),
                    Ok(_) => {}
                    Err(p)
                        if index != 0
                            && selector.parse::<Id>().is_ok()
                            && p.code == "graph_not_loaded" => {}
                    Err(p) => return Err(p),
                }
            }
        }
        let mut scope = store.graphs_in_scope_with_names(&scope_ctx, None, noun, &names)?;
        let named_aliases = scope.aliases.clone();
        // Neighbours has always shown far ends outside its single author selection.
        if noun == "neighbours"
            && ctx.scope.is_none()
            && ctx.graph.as_deref().is_none_or(|g| !g.contains(','))
        {
            for alias in store.aliases().filter(|a| {
                let id = store.loaded(a).expect("loaded").meta.id;
                store.primary_of(id) == Some(*a) && store.shown_unnamed(a)
            }) {
                if !scope.aliases.iter().any(|a| a == alias) {
                    scope.aliases.push(alias.into());
                }
            }
        }
        let access = if scope.named {
            Ctx {
                graph: Some(named_aliases.join(",")),
                scope: None,
                default_graph: None,
                ..ctx.clone()
            }
        } else {
            Ctx {
                graph: None,
                scope: None,
                default_graph: None,
                ..ctx.clone()
            }
        };
        let mut space = Self {
            store,
            aliases: scope.aliases.clone(),
            access,
            refs: refs.to_vec(),
            targets: BTreeMap::new(),
            provenance_in: RefCell::new(BTreeMap::new()),
        };
        space.resolve_targets();
        Ok((space, scope))
    }

    fn resolve_targets(&mut self) {
        self.targets.clear();
        let graphs: BTreeSet<Id> = self
            .store
            .aliases()
            .map(|a| self.store.loaded(a).expect("loaded").meta.id)
            .collect();
        for graph in graphs {
            let end = self.store.read_end(&self.access, &self.refs, graph, graph);
            let part = end.loaded.and_then(|loaded| {
                self.aliases
                    .iter()
                    .position(|a| core::ptr::eq(self.store.loaded(a).expect("selected"), loaded))
            });
            self.targets.insert(
                graph,
                Target {
                    part,
                    state: if part.is_some() {
                        None
                    } else {
                        Some(end.state.unwrap_or("graph_not_loaded"))
                    },
                    candidates: end.candidates,
                },
            );
        }
    }

    pub fn at(&self, part: usize, node: Id) -> At {
        At {
            part: Some(part),
            graph: self
                .store
                .loaded(&self.aliases[part])
                .expect("selected")
                .meta
                .id,
            node,
        }
    }

    pub fn end(&self, owner: usize, graph: Option<Id>, node: Id) -> At {
        match graph {
            None => self.at(owner, node),
            Some(graph) => At {
                part: self.targets.get(&graph).and_then(|t| t.part),
                graph,
                node,
            },
        }
    }

    pub fn state(&self, at: At) -> Option<&'static str> {
        match at.part {
            Some(part)
                if at.node != at.graph
                    && self
                        .store
                        .loaded(&self.aliases[part])
                        .expect("selected")
                        .graph
                        .node(at.node)
                        .is_none() =>
            {
                Some("edge_broken")
            }
            Some(_) => None,
            None => Some(
                self.targets
                    .get(&at.graph)
                    .and_then(|t| t.state)
                    .unwrap_or("graph_not_loaded"),
            ),
        }
    }

    fn visible(&self, at: At) -> bool {
        let loaded = match at.part {
            Some(part) => Some(self.store.loaded(&self.aliases[part]).expect("selected")),
            None => {
                self.store
                    .read_end(&self.access, &self.refs, at.graph, at.node)
                    .loaded
            }
        };
        loaded.is_none_or(|loaded| {
            at.node == at.graph
                || loaded.graph.node_ix(at.node).is_none_or(|ix| {
                    self.access
                        .read_scope(&loaded.graph)
                        .with_provenance(self.store.provenance_in_read(
                            &self.access,
                            &self.refs,
                            at.graph,
                        ))
                        .contains_ix(ix)
                })
        })
    }

    pub fn candidates(&self, at: At) -> Vec<String> {
        if at.part.is_none() {
            self.targets
                .get(&at.graph)
                .map(|t| t.candidates.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    pub fn resolve(&self, text: &str) -> Result<At, Problem> {
        if text.starts_with("umx:") && (text.contains('#') || text.contains('?')) {
            return Err(problems::usage_invalid(
                "a traversal address has no selector or fragment",
                "walk",
            ));
        }
        let (selector, node) = split_address(text);
        // ws006 S23 (review R10): a former alias is its graph's live alias here too.
        if let Some(selector) = selector.map(|s| self.store.live_in(&self.access, s)) {
            if let Some(part) = self.aliases.iter().position(|a| a == selector) {
                let loaded = self.store.loaded(selector).expect("selected");
                let id =
                    crate::graph_store::resolve_read_node(loaded, &self.access, selector, text)?;
                return Ok(self.at(part, id));
            }
            if let Ok(graph) = selector.parse::<Id>() {
                if let Some(part) = self.targets.get(&graph).and_then(|target| target.part) {
                    let alias = &self.aliases[part];
                    return crate::graph_store::resolve_read_node(
                        self.store.loaded(alias).expect("selected"),
                        &self.access,
                        alias,
                        text,
                    )
                    .map(|node| self.at(part, node));
                }
                let id = if node.is_empty() {
                    Ok(graph)
                } else {
                    node.parse::<Id>()
                }
                .map_err(|_| {
                    problems::usage_invalid("an unavailable endpoint needs complete ids", "path")
                })?;
                return Ok(self.end(0, Some(graph), id));
            }
            return Err(problems::graph_mismatch(selector, &self.aliases.join(",")));
        }
        if let Some(alias) = self.access.graph.as_deref().filter(|g| !g.contains(',')) {
            let part = self
                .aliases
                .iter()
                .position(|a| a == alias)
                .expect("selected alias");
            let id = crate::graph_store::resolve_read_node(
                self.store.loaded(alias).expect("selected"),
                &self.access,
                alias,
                node,
            )?;
            return Ok(self.at(part, id));
        }
        let mut matches = Vec::new();
        for (part, alias) in self.aliases.iter().enumerate() {
            let loaded = self.store.loaded(alias).expect("selected");
            match loaded.graph.lookup_where(node, |ix| {
                self.access.read_scope(&loaded.graph).contains_ix(ix)
            }) {
                umx_graph::Lookup::Found(node) => matches.push(self.at(part, node)),
                umx_graph::Lookup::Ambiguous(_) => {
                    return resolve_node_in(loaded, &self.access, alias, node)
                        .map(|id| self.at(part, id));
                }
                umx_graph::Lookup::NotFound(_) => {}
            }
        }
        match matches.as_slice() {
            [only] => Ok(*only),
            [] if self.aliases.len() == 1 => {
                let alias = &self.aliases[0];
                resolve_node_in(
                    self.store.loaded(alias).expect("selected"),
                    &self.access,
                    alias,
                    node,
                )
                .map(|id| self.at(0, id))
            }
            [] => Err(problems::usage_invalid(
                "no node matches in the read scope (use an address)",
                "walk",
            )),
            _ => Err(problems::usage_invalid(
                "more than one node matches (use umx:<graph>/<id>)",
                "walk",
            )),
        }
    }

    pub fn cell(&self, at: At) -> NodeCell {
        if self.state(at).is_some() {
            return NodeCell {
                id: at.node,
                kind: String::new(),
                title: String::new(),
                status: String::new(),
            };
        }
        let alias = &self.aliases[at.part.expect("resolved")];
        if at.node == at.graph {
            let node = self.store.virtual_node(alias);
            NodeCell {
                id: node.id,
                kind: node.kind,
                title: node.title,
                status: node.status,
            }
        } else {
            let loaded = self.store.loaded(alias).expect("selected");
            query::node_cell(
                &loaded.graph,
                loaded.graph.node_ix(at.node).expect("resolved"),
            )
        }
    }

    pub fn graph_name(&self, at: At) -> String {
        at.part
            .map(|p| self.aliases[p].clone())
            .unwrap_or_else(|| at.graph.to_text())
    }

    /// The loaded graph the read shows `at` in, with its id; None when it cannot show it (a graph not shown or not
    /// loaded, a copy it cannot choose): a tag filter never tests such a row (ws008 t2b, d-20).
    fn shown(&self, at: At) -> Option<(&umx_graph::Graph, Id)> {
        let part = at.part?;
        Some((
            &self
                .store
                .loaded(&self.aliases[part])
                .expect("selected")
                .graph,
            at.graph,
        ))
    }

    /// The tag filter of a read from `start` (ws008 t2b), None when it has none. A read of one graph that does not use
    /// std is refused (`tag_needs_std`); several graphs skip any without std. A refused tag's next repeats `noun`,
    /// the start's address, the graph and every tag.
    fn tag_rows(
        &self,
        ctx: &Ctx,
        noun: &'static str,
        start: At,
        tags: &[String],
        no_tags: &[String],
    ) -> Result<Option<crate::graph_store::tags::RowTags>, Problem> {
        use crate::graph_store::tags;
        if tags.is_empty() && no_tags.is_empty() {
            return Ok(None);
        }
        let is_std = |a: &str| tags::is_std(self.store.loaded(a).expect("selected").ontology());
        let alias = &self.aliases[start.part.expect("a start is loaded")];
        let loaded = self.store.loaded_count();
        if !is_std(alias) && !tags::several(ctx, loaded) {
            return Err(problems::tag_needs_std(&crate::graph_store::alias_name(
                alias,
            )));
        }
        let cmd = match umx_core::ValidName::alias(alias) {
            Some(g) => umx_core::Command::new(noun).address(&g, start.node),
            None => umx_core::Command::new(noun).word("<node>"),
        };
        // The next names the start by address, so it adds no `--graph` that could disagree with it: a read naming no
        // graph is repeated as given; a list or scope as the start's graph when that is std, else it gives the docs
        // (one graph without std is tag_needs_std).
        let base = if ctx.graph.is_none() && ctx.scope.is_none() {
            Some(cmd)
        } else if is_std(alias) {
            Some(tags::read_graph(ctx, cmd, Some(alias), loaded))
        } else {
            None
        };
        let Some(filter) = tags::read_filter_or_docs(tags, no_tags, base)? else {
            return Ok(None);
        };
        let mut rows = tags::RowTags::new(filter);
        // A start in a graph without std, in a read of several: that graph is skipped and counted, as search does.
        if !is_std(alias) {
            rows.skip(start.graph);
        }
        Ok(Some(rows))
    }

    fn start(&self, text: &str) -> Result<At, Problem> {
        let at = self.resolve(text)?;
        if at.part.is_some() {
            return Ok(at);
        }
        if self.state(at) == Some("copy_ambiguous") {
            return Err(problems::copy_ambiguous(&self.candidates(at)));
        }
        Err(problems::graph_not_loaded(&at.graph.to_text()))
    }

    fn lacks_vector(&self, at: At) -> bool {
        at.part
            .and_then(|part| {
                let graph = &self
                    .store
                    .loaded(&self.aliases[part])
                    .expect("selected")
                    .graph;
                graph
                    .node_ix(at.node)
                    .map(|ix| query::lacks_vector(graph, ix))
            })
            .unwrap_or(false)
    }

    fn similar_warnings(&self, at: At, relation: Option<&str>, missing: bool) -> Vec<Problem> {
        at.part
            .map(|part| query::similar_warnings(self.store, &self.aliases[part], relation, missing))
            .unwrap_or_default()
    }

    pub fn address(&self, at: At) -> String {
        if self.state(at).is_none() && at.node == at.graph {
            alloc::format!("umx:{}", self.graph_name(at))
        } else {
            address(&self.graph_name(at), at.node)
        }
    }

    pub fn external_steps(
        &self,
        at: At,
        direction: Direction,
        relation: Option<&str>,
        follow_symmetric: bool,
    ) -> Vec<Hop> {
        if !self.visible(at) {
            return Vec::new();
        }
        let Some(part) = at.part.filter(|_| self.state(at).is_none()) else {
            return Vec::new();
        };
        let mut steps = Vec::new();
        for (owner, id) in self
            .store
            .incident_selected(&self.aliases, &self.aliases[part], at.node)
        {
            let loaded = self.store.loaded(&self.aliases[owner]).expect("selected");
            let edge = loaded.graph.edge(id).expect("indexed edge");
            if edge.kind() == "similar_to" || relation.is_some_and(|r| r != edge.kind()) {
                continue;
            }
            let from = self.end(owner, edge.from_graph(), edge.from());
            let to = self.end(owner, edge.to_graph(), edge.to());
            if !self.visible(from) || !self.visible(to) {
                continue;
            }
            let symmetric = follow_symmetric && loaded.ontology().is_symmetric(edge.kind());
            for (here, other, way, take) in [
                (
                    from,
                    to,
                    Way::Out,
                    symmetric || matches!(direction, Direction::Out | Direction::Both),
                ),
                (
                    to,
                    from,
                    Way::In,
                    symmetric || matches!(direction, Direction::In | Direction::Both),
                ),
            ] {
                // Exact copy identity matters even when two aliases share the same graph id.
                if here == at && take && !(way == Way::In && symmetric && from == to) {
                    steps.push(Hop {
                        to: other,
                        owner,
                        edge: EdgeCell {
                            id: Some(id),
                            relation: edge.kind().into(),
                            way,
                            score: None,
                            derived: false,
                        },
                    });
                }
            }
        }
        steps.sort_by_key(|h| {
            (
                if h.edge.way == Way::Out { 0 } else { 1 },
                h.owner,
                h.edge.id,
            )
        });
        steps
    }

    pub fn steps(&self, at: At, direction: Direction, relation: Option<&str>) -> Vec<Hop> {
        if !self.visible(at) {
            return Vec::new();
        }
        let Some(part) = at.part.filter(|_| self.state(at).is_none()) else {
            return Vec::new();
        };
        let loaded = self.store.loaded(&self.aliases[part]).expect("selected");
        let mut steps = self.provenance_steps(at, direction, relation);
        steps.extend(
            loaded
                .graph
                .node_ix(at.node)
                .map(|ix| {
                    query::hops(
                        &loaded.graph,
                        ix,
                        direction,
                        relation,
                        &loaded.ontology().symmetric_kinds(),
                        self.access.include_provenance,
                    )
                    .into_iter()
                    .filter(|hop| self.access.read_scope(&loaded.graph).contains_ix(hop.other))
                    .map(|hop| Hop {
                        to: self.at(part, loaded.graph.node_at(hop.other).id()),
                        owner: part,
                        edge: hop.edge,
                    })
                    .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        );
        steps.extend(self.external_steps(at, direction, relation, true));
        let derived = steps.iter().take_while(|hop| hop.edge.derived).count();
        steps[derived..].sort_by_key(|hop| {
            (
                if hop.edge.way == Way::Out { 0 } else { 1 },
                self.store
                    .loaded(&self.aliases[hop.owner])
                    .expect("owner")
                    .meta
                    .id,
                hop.edge.id,
            )
        });
        steps
    }

    fn provenance_steps(&self, at: At, direction: Direction, relation: Option<&str>) -> Vec<Hop> {
        if !self.access.include_provenance
            || relation.is_some_and(|kind| !matches!(kind, "authored_by" | "edited_by"))
        {
            return Vec::new();
        }
        let part = at.part.expect("available start");
        let graph = &self
            .store
            .loaded(&self.aliases[part])
            .expect("selected")
            .graph;
        let mut steps = Vec::new();
        let mut push = |node, kind: &'static str, way| {
            if relation.is_none_or(|wanted| wanted == kind) {
                steps.push(Hop {
                    to: self.at(part, node),
                    owner: part,
                    edge: EdgeCell {
                        id: None,
                        relation: kind.into(),
                        way,
                        score: None,
                        derived: true,
                    },
                });
            }
        };
        if matches!(direction, Direction::Out | Direction::Both)
            && let Some(ix) = graph.node_ix(at.node)
        {
            for edge in graph.node_provenance_edges(ix) {
                push(edge.to.id(), edge.kind, Way::Out);
            }
        }
        if matches!(direction, Direction::In | Direction::Both) {
            let mut incoming = self.provenance_in.borrow_mut();
            let index = incoming.entry(part).or_insert_with(|| {
                let mut index: BTreeMap<Id, Vec<(Id, &'static str)>> = BTreeMap::new();
                for edge in graph.provenance_edges() {
                    index
                        .entry(edge.to.id())
                        .or_default()
                        .push((edge.from.id(), edge.kind));
                }
                index
            });
            for &(node, kind) in index.get(&at.node).into_iter().flatten() {
                push(node, kind, Way::In);
            }
        }
        steps
    }

    pub fn finish(&mut self) -> Option<Vec<String>> {
        self.store.clock().now_ms();
        let revoked: Vec<_> = self
            .aliases
            .iter()
            .filter(|a| !self.store.external_visible(&self.access, &self.refs, a))
            .cloned()
            .collect();
        let before = core::mem::take(&mut self.targets);
        self.resolve_targets();
        (!revoked.is_empty() || before != self.targets).then_some(revoked)
    }

    pub fn exclude(&mut self, revoked: &[String]) {
        self.aliases.retain(|a| !revoked.contains(a));
        self.provenance_in.borrow_mut().clear();
        self.resolve_targets();
    }
}

/// The other members of a neighbours start's entity whose edges the read lists (ws008 E3, req-127), with each one's
/// `match_status`, and the graphs the entity read covered. Built as `umx entity show` builds the entity, from the
/// start's address and the same request, so both always agree: confirmed members in its order, then (unless
/// `--entities confirmed`) the one-hop proposals it lists. A member the read space does not hold is left out.
struct Inherit {
    covered: Vec<String>,
    members: Vec<(At, &'static str)>,
    proposed: usize,
}

fn inherit(
    store: &GraphStore,
    ctx: &Ctx,
    space: &ReadSpace<'_>,
    start: At,
    entities: crate::Entities,
) -> Result<Inherit, Problem> {
    let shown = store.entity_show(ctx, &space.address(start))?.value;
    let at = |alias: &str, id: Id| {
        space
            .aliases
            .iter()
            .position(|a| a == alias)
            .map(|part| space.at(part, id))
    };
    let mut members: Vec<(At, &'static str)> = shown
        .members
        .iter()
        .filter_map(|m| at(&m.alias, m.id))
        .filter(|m| (m.graph, m.node) != (start.graph, start.node))
        .map(|m| (m, "confirmed"))
        .collect();
    let mut proposed = 0;
    if entities == crate::Entities::On {
        for p in &shown.proposed {
            if let Some(p) = at(&p.alias, p.id) {
                members.push((p, "proposed"));
                proposed += 1;
            }
        }
    }
    Ok(Inherit {
        covered: shown.graphs,
        members,
        proposed,
    })
}

/// `cmd` with the request's `--scope`, else its `--graph` (a next that reads what this read covered).
fn same_read(ctx: &Ctx, cmd: umx_core::Command) -> umx_core::Command {
    match &ctx.scope {
        Some(crate::Scope::Saved(name)) => match umx_core::ValidName::alias(name) {
            Some(name) => cmd.word("--scope").name(&name),
            None => cmd.word("--scope").word("<scope>"),
        },
        _ => ctx.same_graph(cmd),
    }
}

pub(crate) fn neighbours(
    store: &GraphStore,
    ctx: &Ctx,
    req: crate::NeighboursRequest,
) -> Outcome<Page<crate::NeighbourRow>> {
    let refs = [Some(req.node.as_str())];
    let (mut space, scope) = ReadSpace::new(store, ctx, &refs, "neighbours")?;
    // similar_to is a ranking, not a stored fact: never inherited (ws008 E3).
    let entities = if req.relation.as_deref() == Some("similar_to") {
        crate::Entities::Off
    } else {
        req.entities
    };
    for pass in 0..2 {
        let at = space.start(&req.node)?;
        let mut tagged = space.tag_rows(ctx, "neighbours", at, &req.tags, &req.no_tags)?;
        // Loaded copies of a graph with no primary in the covered set (`entity show` refuses with copy_ambiguous) never
        // fail neighbours: the reply is the node's own rows, as with `--entities off`. Any other refusal is the read's.
        let group = if entities == crate::Entities::Off {
            None
        } else {
            match inherit(store, ctx, &space, at, entities) {
                Ok(group) => Some(group),
                Err(p) if p.code == "copy_ambiguous" => None,
                Err(p) => return Err(p),
            }
        };
        let grouped = group.as_ref().is_some_and(|g| !g.members.is_empty());
        let row = |space: &ReadSpace<'_>,
                   here: At,
                   hop: Hop,
                   via: Option<String>,
                   status: Option<&'static str>| {
            let owner = store.loaded(&space.aliases[hop.owner]).expect("owner");
            let (from, to) = if hop.edge.way == Way::Out {
                (here, hop.to)
            } else {
                (hop.to, here)
            };
            crate::NeighbourRow {
                address: space.address(hop.to),
                graph: owner.meta.id,
                from_graph: from.graph,
                to_graph: to.graph,
                node_graph: hop.to.graph,
                from: from.node,
                to: to.node,
                state: space.state(hop.to),
                candidates: space.candidates(hop.to),
                node: space.cell(hop.to),
                edge: hop.edge,
                via,
                match_status: status,
            }
        };
        let own_status = grouped.then_some("own");
        let mut rows: Vec<_> = space
            .steps(at, req.direction, req.relation.as_deref())
            .into_iter()
            .filter(|hop| {
                tagged
                    .as_mut()
                    .is_none_or(|t| t.keeps(space.shown(hop.to), hop.to.node))
            })
            .map(|hop| row(&space, at, hop, None, own_status))
            .collect();
        if let Some(group) = group.as_ref().filter(|_| grouped) {
            // Inherited rows: each member's real stored edges, never an identity edge or one back into the group, and
            // each stored edge once.
            let inside: BTreeSet<(Id, Id)> = core::iter::once(at)
                .chain(group.members.iter().map(|m| m.0))
                .map(|m| (m.graph, m.node))
                .collect();
            let mut seen: BTreeSet<(Id, Id)> = rows
                .iter()
                .filter_map(|r| r.edge.id.map(|e| (r.graph, e)))
                .collect();
            for &(member, status) in &group.members {
                let via = space.address(member);
                for hop in space.steps(member, req.direction, req.relation.as_deref()) {
                    let Some(edge) = hop.edge.id.filter(|_| !hop.edge.derived) else {
                        continue;
                    };
                    let owner = store
                        .loaded(&space.aliases[hop.owner])
                        .expect("owner")
                        .meta
                        .id;
                    if [
                        umx_core::ontology::SAME_AS,
                        umx_core::ontology::POSSIBLY_SAME_AS,
                        umx_core::ontology::NOT_SAME_AS,
                    ]
                    .contains(&hop.edge.relation.as_str())
                        || inside.contains(&(hop.to.graph, hop.to.node))
                        || !seen.insert((owner, edge))
                    {
                        continue;
                    }
                    if let Some(t) = tagged.as_mut()
                        && !t.keeps(space.shown(hop.to), hop.to.node)
                    {
                        continue;
                    }
                    rows.push(row(&space, member, hop, Some(via.clone()), Some(status)));
                }
            }
        }
        let count = rows.len();
        let missing = req.relation.as_deref() == Some("similar_to")
            && (space.lacks_vector(at)
                || rows.iter().any(|row| {
                    space.lacks_vector(
                        space.at(at.part.expect("neighbour start loaded"), row.node.id),
                    )
                }));
        let mut warnings = space.similar_warnings(at, req.relation.as_deref(), missing);
        for row in &rows {
            if row.state == Some("edge_broken") {
                warnings.push(problems::edge_broken(
                    row.edge.id.expect("stored"),
                    if row.edge.way == Way::Out {
                        "to"
                    } else {
                        "from"
                    },
                    row.node_graph,
                    row.node.id,
                ));
            }
        }
        let mut value = page(
            &space.graph_name(at),
            rows,
            req.limit.min(count),
            req.offset,
        );
        value.covered = group.as_ref().map(|g| g.covered.clone());
        let changed = space.finish();
        if changed.is_none() {
            let (names, notices) =
                placeholder_names(store, value.rows.iter().any(|r| unavailable(r.state)));
            for row in &mut value.rows {
                if unavailable(row.state)
                    && let Some(alias) = names.get(&row.node_graph)
                {
                    row.address = address(alias, row.node.id);
                }
            }
            warnings.extend(notices);
            warnings.extend(store.stale_notes(&space.aliases));
            if let Some(saved) = &scope.saved {
                warnings.extend(store.saved_warnings(saved, &space.aliases)?);
            }
            warnings.extend(tagged.iter().flat_map(|t| t.notes()));
            if let Some(group) = group.as_ref().filter(|_| grouped) {
                let show = umx_core::Command::new("entity show");
                let show = match umx_core::ValidName::alias(&space.graph_name(at)) {
                    Some(g) => show.address(&g, at.node),
                    None => show.word("<node>"),
                };
                warnings.push(problems::entities_grouped(
                    1 + group.members.len(),
                    group.proposed,
                    same_read(ctx, show),
                ));
            }
            return Ok(Done {
                value,
                notes: warnings,
            });
        }
        if pass == 1 {
            break;
        }
        space.exclude(&changed.expect("changed"));
    }
    Err(problems::usage_invalid(
        "graph policy changed during the read (retry)",
        "neighbours",
    ))
}

pub(crate) fn walk(store: &GraphStore, ctx: &Ctx, req: WalkRequest) -> Outcome<Page<WalkRow>> {
    let refs = [Some(req.node.as_str()), req.within.as_deref()];
    let (mut space, scope) = ReadSpace::new(store, ctx, &refs, "walk")?;
    for pass in 0..2 {
        let start = space.start(&req.node)?;
        let mut tagged = space.tag_rows(ctx, "walk", start, &req.tags, &req.no_tags)?;
        let within = req.within.as_ref().map(|w| space.resolve(w)).transpose()?;
        if within.is_some_and(|root| space.state(root).is_some() || root.node == root.graph) {
            return Err(problems::usage_invalid(
                "--within needs a loaded hierarchy node",
                "walk",
            ));
        }
        let within_nodes = within.map(|root| {
            space
                .store
                .loaded(&space.aliases[root.part.expect("within loaded")])
                .expect("selected")
                .graph
                .within(root.node)
        });
        let relation = req.relation.as_deref();
        let mut missing = relation == Some("similar_to") && space.lacks_vector(start);
        let mut value = {
            let mut handles = bfs::Handles::new(&space);
            let first = handles.node(start);
            let mut seen = bfs::Bits::new(handles.keys());
            seen.insert(handles.key(first));
            let mut queue = VecDeque::from([(first, 0usize)]);
            let mut ready = VecDeque::new();
            let mut steps = Vec::new();
            let rows = core::iter::from_fn(|| {
                loop {
                    if let Some(row) = ready.pop_front() {
                        return Some(row);
                    }
                    let (at, depth) = queue.pop_front()?;
                    if depth == req.depth {
                        continue;
                    }
                    handles.steps(at, req.direction, relation, &mut steps);
                    let from = handles.at(at);
                    for step in &steps {
                        if relation == Some("similar_to") {
                            missing |= handles.lacks_vector(step.to);
                        }
                        if !seen.insert(handles.key(step.to)) {
                            continue;
                        }
                        let state = handles.state(step.to);
                        if state.is_none() {
                            queue.push_back((step.to, depth + 1));
                        }
                        let to = handles.at(step.to);
                        if within.is_some_and(|root| {
                            root.part != to.part
                                || !within_nodes.as_ref().expect("within set").contains(to.node)
                        }) {
                            continue;
                        }
                        // ws008 t2b: like --within, the tags filter the listed nodes; the walk goes on through.
                        if let Some(t) = tagged.as_mut()
                            && !t.keeps(space.shown(to), to.node)
                        {
                            continue;
                        }
                        ready.push_back(WalkRow {
                            depth: depth + 1,
                            node: handles.cell(step.to),
                            from: from.node,
                            edge: step.cell(),
                            graph: handles.graph_name(step.to),
                            graph_id: to.graph,
                            address: handles.address(step.to),
                            from_graph: from.graph,
                            edge_graph: handles.graph_id(step.owner),
                            state,
                            candidates: handles.candidates(step.to),
                        });
                    }
                }
            });
            lazy_page(&space.graph_name(start), rows, req.limit, req.offset)
        };
        let changed = space.finish();
        if changed.is_none() {
            let mut warnings = scope
                .saved
                .as_ref()
                .map(|s| store.saved_warnings(s, &space.aliases))
                .transpose()?
                .unwrap_or_default();
            warnings.extend(space.similar_warnings(start, req.relation.as_deref(), missing));
            let (names, notices) =
                placeholder_names(store, value.rows.iter().any(|r| unavailable(r.state)));
            for row in &mut value.rows {
                if unavailable(row.state)
                    && let Some(alias) = names.get(&row.graph_id)
                {
                    row.graph = alias.clone();
                    row.address = address(alias, row.node.id);
                }
            }
            warnings.extend(notices);
            warnings.extend(store.stale_notes(&space.aliases));
            warnings.extend(tagged.iter().flat_map(|t| t.notes()));
            return Ok(Done {
                value,
                notes: warnings,
            });
        }
        if pass == 1 {
            break;
        }
        space.exclude(&changed.expect("changed"));
    }
    Err(problems::usage_invalid(
        "graph policy changed during the read (retry)",
        "walk",
    ))
}

pub(crate) fn path(store: &GraphStore, ctx: &Ctx, req: PathRequest) -> Outcome<Page<PathRow>> {
    let refs = [Some(req.from.as_str()), Some(req.to.as_str())];
    let (mut space, scope) = ReadSpace::new(store, ctx, &refs, "path")?;
    for pass in 0..2 {
        let from = space.start(&req.from)?;
        let to = space.resolve(&req.to)?;
        let rows: Vec<(At, Option<EdgeCell>)> = {
            let mut handles = bfs::Handles::new(&space);
            let (start, end) = (handles.node(from), handles.node(to));
            handles
                .path(
                    start,
                    end,
                    req.max_depth,
                    req.direction,
                    req.relation.as_deref(),
                )
                .into_iter()
                .map(|(node, step)| (handles.at(node), step.map(|s| s.cell())))
                .collect()
        };
        let missing = req.relation.as_deref() == Some("similar_to")
            && (space.lacks_vector(from)
                || (to.part == from.part && space.lacks_vector(to))
                || rows.iter().any(|(at, _)| space.lacks_vector(*at)));
        let rows: Vec<_> = rows
            .into_iter()
            .enumerate()
            .map(|(step, (at, edge))| PathRow {
                step,
                node: space.cell(at),
                edge,
                graph: space.graph_name(at),
                graph_id: at.graph,
                address: space.address(at),
                state: space.state(at),
                candidates: space.candidates(at),
            })
            .collect();
        let count = rows.len();
        let mut value = page(&space.graph_name(from), rows, count, 0);
        let changed = space.finish();
        if changed.is_none() {
            let mut warnings = space.similar_warnings(from, req.relation.as_deref(), missing);
            if let Some(saved) = &scope.saved {
                warnings.extend(store.saved_warnings(saved, &space.aliases)?);
            }
            let (names, notices) =
                placeholder_names(store, value.rows.iter().any(|r| unavailable(r.state)));
            for row in &mut value.rows {
                if unavailable(row.state)
                    && let Some(alias) = names.get(&row.graph_id)
                {
                    row.graph = alias.clone();
                    row.address = address(alias, row.node.id);
                }
            }
            warnings.extend(notices);
            warnings.extend(store.stale_notes(&space.aliases));
            return Ok(Done {
                value,
                notes: warnings,
            });
        }
        if pass == 1 {
            break;
        }
        space.exclude(&changed.expect("changed"));
    }
    Err(problems::usage_invalid(
        "graph policy changed during the read (retry)",
        "path",
    ))
}
