use super::{GraphStore, Loaded, resolve_node, split_address};
use crate::{Ctx, Problem, problems};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::{Edge, Id};
use umx_graph::{EdgeRef, Graph};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct StoredEnd {
    pub graph: Option<Id>,
    pub node: Id,
}

pub(crate) struct ReadEnd<'a> {
    pub loaded: Option<&'a Loaded>,
    pub state: Option<&'static str>,
    pub candidates: Vec<String>,
}

impl<'a> ReadEnd<'a> {
    pub(crate) fn local(loaded: &'a Loaded) -> Self {
        Self {
            loaded: Some(loaded),
            state: None,
            candidates: Vec::new(),
        }
    }

    fn unresolved(state: &'static str, candidates: Vec<String>) -> Self {
        Self {
            loaded: None,
            state: Some(state),
            candidates,
        }
    }
}

fn selectors<'a>(ctx: &'a Ctx, refs: &'a [Option<&'a str>]) -> impl Iterator<Item = &'a str> {
    let addressed = refs
        .iter()
        .flatten()
        .any(|text| split_address(text).0.is_some());
    let default = ctx
        .default_graph
        .as_ref()
        .filter(|_| ctx.graph.is_none() && !addressed);
    ctx.graph
        .iter()
        .chain(default)
        .flat_map(|list| list.split(','))
        .chain(
            refs.iter()
                .flatten()
                .filter_map(|text| split_address(text).0),
        )
        .map(str::trim)
}

impl StoredEnd {
    pub fn local(node: Id) -> Self {
        Self { graph: None, node }
    }
    pub fn of(edge: EdgeRef<'_>, from: bool) -> Self {
        if from {
            Self {
                graph: edge.from_graph(),
                node: edge.from(),
            }
        } else {
            Self {
                graph: edge.to_graph(),
                node: edge.to(),
            }
        }
    }
}

pub(super) fn find_edge<'a>(
    graph: &'a Graph,
    from: StoredEnd,
    kind: &str,
    to: StoredEnd,
) -> Option<EdgeRef<'a>> {
    if from.graph.is_none() && to.graph.is_none() {
        return graph.find_edge(from.node, kind, to.node);
    }
    find_edge_where(graph, from, kind, to, |_| true)
}

/// The edge (from, kind, to) among `graph`'s edges whose id `live` accepts, matched by graph-qualified ends.
pub(super) fn find_edge_where<'a>(
    graph: &'a Graph,
    from: StoredEnd,
    kind: &str,
    to: StoredEnd,
    live: impl Fn(Id) -> bool,
) -> Option<EdgeRef<'a>> {
    graph.edges().find(|edge| {
        edge.kind() == kind
            && StoredEnd::of(*edge, true) == from
            && StoredEnd::of(*edge, false) == to
            && live(edge.id())
    })
}

/// Adds name whole nodes only. A passage edge at the indexed triple must not hide a whole-node edge.
pub(super) fn whole_node_edge<'a>(
    candidate: Option<EdgeRef<'a>>,
    mut edges: impl Iterator<Item = EdgeRef<'a>>,
) -> Option<EdgeRef<'a>> {
    let first = candidate?;
    let whole = |edge: EdgeRef<'_>| {
        edge.field("from_selector").is_none() && edge.field("to_selector").is_none()
    };
    if whole(first) {
        return Some(first);
    }
    edges.find(|edge| {
        edge.kind() == first.kind()
            && StoredEnd::of(*edge, true) == StoredEnd::of(first, true)
            && StoredEnd::of(*edge, false) == StoredEnd::of(first, false)
            && whole(*edge)
    })
}

pub(super) fn ordered_ends(
    owner: Id,
    symmetric: bool,
    from: StoredEnd,
    to: StoredEnd,
) -> (StoredEnd, StoredEnd) {
    if symmetric && (from.graph.unwrap_or(owner), from.node) > (to.graph.unwrap_or(owner), to.node)
    {
        (to, from)
    } else {
        (from, to)
    }
}

pub(super) fn reversed_pairs(graph: &Graph, owner: Id, symmetric: &[&str]) -> Vec<(Id, Id)> {
    graph
        .edges()
        .filter(|edge| symmetric.contains(&edge.kind()))
        .filter(|edge| {
            (edge.from_graph().unwrap_or(owner), edge.from())
                > (edge.to_graph().unwrap_or(owner), edge.to())
        })
        .filter_map(|edge| {
            find_edge(
                graph,
                StoredEnd::of(edge, false),
                edge.kind(),
                StoredEnd::of(edge, true),
            )
            .map(|other| (edge.id(), other.id()))
        })
        .collect()
}

pub(super) fn set_ends(mut edge: Edge, from: StoredEnd, to: StoredEnd) -> Edge {
    edge.from_graph = from.graph;
    edge.to_graph = to.graph;
    edge
}

impl GraphStore {
    pub(crate) fn read_selector_alias(&self, ctx: &Ctx, selector: &str) -> Result<String, Problem> {
        // ws006 S23 (review R10): a former alias reads as its graph's live alias; two graphs holding it refuse.
        let live = self.canonical(ctx, selector)?;
        if self.graphs.contains_key(&live) {
            return Ok(live);
        }
        if let Ok(graph) = selector.parse::<Id>() {
            let text = alloc::format!("umx:{selector}");
            let resolved = self.read_end(ctx, &[Some(&text)], graph, graph);
            if let Some(loaded) = resolved.loaded {
                return Ok(self
                    .graphs
                    .iter()
                    .find(|(_, value)| core::ptr::eq(*value, loaded))
                    .expect("resolved load belongs to store")
                    .0
                    .clone());
            }
            if resolved.state == Some("copy_ambiguous") {
                return Err(problems::copy_ambiguous(&resolved.candidates));
            }
        }
        Err(self.not_loaded(selector))
    }

    pub(crate) fn has_external_edges(&self, alias: &str) -> bool {
        self.incoming.has_source(self.graphs[alias].instance)
    }

    pub(crate) fn external_access(&self, ctx: &Ctx) -> Result<Ctx, Problem> {
        if ctx.scope.is_some()
            || ctx
                .graph
                .as_deref()
                .is_some_and(|graph| graph.contains(','))
        {
            let scope = self.graphs_in_scope(ctx, None, "edge")?;
            Ok(Ctx {
                graph: scope.named.then(|| scope.aliases.join(",")),
                scope: None,
                default_graph: None,
                ..ctx.clone()
            })
        } else {
            Ok(ctx.clone())
        }
    }

    pub(crate) fn external_read_alias(
        &self,
        ctx: &Ctx,
        refs: &[Option<&str>],
    ) -> Result<String, Problem> {
        if ctx.scope.is_some()
            || ctx
                .graph
                .as_deref()
                .is_some_and(|graph| graph.contains(','))
        {
            let scope = self.graphs_in_scope(ctx, None, "edge")?;
            let addressed = self.addressed(ctx, refs)?;
            let alias = match addressed {
                Some(alias) if scope.aliases.contains(&alias) => alias,
                Some(alias) => {
                    return Err(problems::graph_mismatch(&alias, &scope.aliases.join(",")));
                }
                None if scope.aliases.len() == 1 => scope.aliases[0].clone(),
                _ => {
                    return Err(problems::usage_invalid(
                        "a node in several graphs needs an address",
                        "edge",
                    ));
                }
            };
            let single = Ctx {
                scope: None,
                graph: Some(alias),
                default_graph: None,
                ..ctx.clone()
            };
            return self.select(&single, refs);
        }
        let named = ctx.graph.is_some()
            || ctx.default_graph.is_some()
            || refs
                .iter()
                .flatten()
                .any(|text| split_address(text).0.is_some());
        if named {
            return self.select(ctx, refs);
        }
        let scope = self.unnamed_scope()?;
        match scope.aliases.as_slice() {
            [only] => self.select(&ctx.clone().graph(only), &[]),
            [] => Err(problems::graph_none_loaded()),
            _ => Err(problems::usage_invalid(
                "name one graph with --graph",
                "edge",
            )),
        }
    }

    /// The author graph of a write that may name other graphs' nodes, once its files are as its load left them (F144).
    pub(super) fn select_author(
        &self,
        ctx: &Ctx,
        refs: &[Option<&str>],
    ) -> Result<String, Problem> {
        if ctx.graph.is_some() || ctx.default_graph.is_some() || self.graphs.len() == 1 {
            self.select_write(ctx, &[])
        } else {
            self.select_write(ctx, refs).map_err(|problem| {
                if problem.code == "cross_graph_not_yet" {
                    problems::usage_invalid(
                        "external edges need --graph naming the author graph",
                        "edge",
                    )
                } else {
                    problem
                }
            })
        }
    }

    /// The graph an address's selector names: a loaded alias, a graph id, or (ws006 S23) a former alias, as its live
    /// alias would. Two graphs holding that former alias: `former_alias_ambiguous`, never `graph_not_loaded` (review
    /// R16).
    pub(super) fn addressed_graph(&self, selector: &str) -> Result<Option<Id>, Problem> {
        if let Some(loaded) = self.graphs.get(selector) {
            return Ok(Some(loaded.meta.id));
        }
        if let Ok(id) = selector.parse() {
            return Ok(Some(id));
        }
        Ok(self
            .former_owner(selector, &|_| true)?
            .map(|live| self.graphs[&live].meta.id))
    }

    pub(super) fn local_end(&self, owner: &str, text: &str) -> bool {
        let (selector, node) = split_address(text);
        selector.is_none_or(|selector| {
            !node.is_empty()
                && node.parse::<Id>().ok() != Some(self.graphs[owner].meta.id)
                && self.addressed_graph(selector).ok().flatten() == Some(self.graphs[owner].meta.id)
        })
    }

    /// Resolution for writes. Unloaded targets require complete stored identities, never alias guesses.
    pub(super) fn write_end(
        &self,
        owner: &str,
        text: &str,
    ) -> Result<(StoredEnd, Vec<Problem>), Problem> {
        let author = &self.graphs[owner];
        let (selector, node) = split_address(text);
        if text.starts_with("umx:") && (text.contains('#') || text.contains('?')) {
            return Err(problems::usage_invalid(
                "a graph endpoint has no selector or fragment",
                "edge",
            ));
        }
        let Some(selector) = selector else {
            if text.starts_with("umx:") {
                return Err(problems::usage_invalid(
                    "an address is umx:<graph>/<id>",
                    "edge",
                ));
            }
            return resolve_node(author, owner, text)
                .map(|node| (StoredEnd::local(node), Vec::new()));
        };
        let graph = self
            .addressed_graph(selector)?
            .ok_or_else(|| problems::graph_not_loaded(selector))?;
        if graph == author.meta.id {
            if node.is_empty() || node.parse::<Id>().ok() == Some(graph) {
                return Err(problems::usage_invalid(
                    "a graph target must be external (use your hierarchy root)",
                    "edge",
                ));
            }
            return resolve_node(author, owner, node)
                .map(|node| (StoredEnd::local(node), Vec::new()));
        }
        let target = self
            .graphs
            .get_key_value(selector)
            .map(|(alias, _)| alias.as_str())
            .or_else(|| self.primary_of(graph));
        let mut warnings = Vec::new();
        let node = match target {
            Some(alias) => {
                let id = if node.is_empty() || node.parse::<Id>().ok() == Some(graph) {
                    graph
                } else {
                    resolve_node(&self.graphs[alias], alias, node)?
                };
                if self.shown_unnamed(owner) && !self.shown_unnamed(alias) {
                    warnings.push(problems::link_into_secured(text));
                }
                id
            }
            None => {
                let copies = self.copies_of(graph);
                if !copies.is_empty() {
                    return Err(problems::copy_ambiguous(
                        &copies
                            .into_iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>(),
                    ));
                }
                let id = if node.is_empty() {
                    Ok(graph)
                } else {
                    node.parse::<Id>()
                }
                .map_err(|_| {
                    problems::usage_invalid(
                        "an unloaded external end needs the graph id and a complete node id",
                        "edge",
                    )
                })?;
                warnings.push(problems::external_end_unchecked(text));
                id
            }
        };
        Ok((
            StoredEnd {
                graph: Some(graph),
                node,
            },
            warnings,
        ))
    }

    fn named_copy<'a>(&'a self, ctx: &Ctx, refs: &[Option<&str>], graph: Id) -> Option<&'a str> {
        for selector in selectors(ctx, refs) {
            let selector = self.live_in(ctx, selector);
            if let Some((alias, loaded)) = self.graphs.get_key_value(selector) {
                if loaded.meta.id == graph {
                    return Some(alias);
                }
            } else if selector.parse::<Id>().ok() == Some(graph) {
                return self.primary_of(graph);
            }
        }
        None
    }

    fn explicitly_named_copy(&self, ctx: &Ctx, refs: &[Option<&str>], alias: &str) -> bool {
        selectors(ctx, refs).any(|selector| {
            let selector = self.live_in(ctx, selector);
            selector == alias
                || (selector.parse::<Id>().ok() == Some(self.graphs[alias].meta.id)
                    && self.primary_of(self.graphs[alias].meta.id) == Some(alias))
        })
    }

    /// Whether the request names graph `graph` (by alias or id), as `read_end` judges it (F113 link_targets_unnamed).
    pub(crate) fn target_named(&self, ctx: &Ctx, graph: Id) -> bool {
        self.named_copy(ctx, &[], graph).is_some()
    }

    pub(crate) fn provenance_in_read(&self, ctx: &Ctx, refs: &[Option<&str>], graph: Id) -> bool {
        ctx.include_provenance
            && (selectors(ctx, refs).next().is_none()
                || self.named_copy(ctx, refs, graph).is_some())
    }

    pub(crate) fn external_visible(&self, ctx: &Ctx, refs: &[Option<&str>], alias: &str) -> bool {
        self.shown_unnamed(alias) || self.explicitly_named_copy(ctx, refs, alias)
    }

    /// Policy is checked before looking up a node, so hidden and unloaded targets have identical states.
    pub(crate) fn read_end(
        &self,
        ctx: &Ctx,
        refs: &[Option<&str>],
        graph: Id,
        node: Id,
    ) -> ReadEnd<'_> {
        let Some(alias) = self
            .named_copy(ctx, refs, graph)
            .or_else(|| self.primary_of(graph))
        else {
            let candidates: Vec<String> = self
                .copies_of(graph)
                .into_iter()
                .filter(|alias| self.external_visible(ctx, refs, alias))
                .map(ToString::to_string)
                .collect();
            return if candidates.is_empty() {
                ReadEnd::unresolved("graph_not_loaded", candidates)
            } else {
                ReadEnd::unresolved("copy_ambiguous", candidates)
            };
        };
        if !self.external_visible(ctx, refs, alias) {
            return ReadEnd::unresolved("graph_not_loaded", Vec::new());
        }
        let loaded = &self.graphs[alias];
        if node != graph && loaded.graph.node(node).is_none() {
            ReadEnd {
                loaded: Some(loaded),
                state: Some("edge_broken"),
                candidates: Vec::new(),
            }
        } else {
            ReadEnd::local(loaded)
        }
    }

    /// Stored edges incident to one qualified node, including links owned by a third graph, whose owner the read may
    /// show. Each owner's visibility is a policy read, made once per owner here, not once per edge.
    pub(crate) fn incident_edges(
        &self,
        ctx: &Ctx,
        refs: &[Option<&str>],
        alias: &str,
        node: Id,
    ) -> Vec<(String, Id)> {
        let mut owners: Vec<(&str, bool)> = Vec::new();
        self.incident(ctx, refs, alias, node, |owner| {
            if let Some(&(_, shown)) = owners.iter().find(|(o, _)| *o == owner) {
                return shown;
            }
            let shown = self.external_visible(ctx, refs, owner);
            owners.push((owner, shown));
            shown
        })
    }

    /// [`incident_edges`](Self::incident_edges) before any policy read: every incoming link of the selected copy,
    /// whatever its owner's visibility. Edge list checks each owner itself, once, after its clock hook.
    pub(crate) fn incident_candidates(
        &self,
        ctx: &Ctx,
        refs: &[Option<&str>],
        alias: &str,
        node: Id,
    ) -> Vec<(String, Id)> {
        self.incident(ctx, refs, alias, node, |_| true)
    }

    fn incident<'a>(
        &'a self,
        ctx: &Ctx,
        refs: &[Option<&str>],
        alias: &str,
        node: Id,
        mut shown: impl FnMut(&'a str) -> bool,
    ) -> Vec<(String, Id)> {
        let loaded = &self.graphs[alias];
        let mut edges = Vec::new();
        if let Some(local) = loaded.graph.node_ix(node) {
            edges.extend(
                loaded
                    .graph
                    .edges_out(local)
                    .chain(loaded.graph.edges_in(local))
                    .map(|step| (alias.to_string(), loaded.graph.edge_at(step.edge).id())),
            );
            edges.extend(
                loaded
                    .graph
                    .edges_out_external(local)
                    .chain(loaded.graph.edges_in_external(local))
                    .map(|(edge, _, _, _)| (alias.to_string(), loaded.graph.edge_at(edge).id())),
            );
        }
        for &(instance, edge) in self.incoming.entries(loaded.meta.id, node) {
            if let Some((owner, _)) = self
                .graphs
                .iter()
                .find(|(_, source)| source.instance == instance)
            {
                let source_id = self.graphs[owner].meta.id;
                let selected = if self.named_copy(ctx, refs, source_id).is_some() {
                    self.explicitly_named_copy(ctx, refs, owner)
                } else {
                    self.primary_of(source_id) == Some(owner.as_str())
                };
                if selected && shown(owner) {
                    edges.push((owner.clone(), edge));
                }
            }
        }
        edges.sort();
        edges.dedup();
        edges
    }

    pub(crate) fn edge_record(&self, owner: &str, edge: Id) -> Option<(Id, EdgeRef<'_>)> {
        let loaded = self.graphs.get(owner)?;
        Some((loaded.meta.id, loaded.graph.edge(edge)?))
    }

    /// Pure adjacency over a read's already authorized, copy-selected aliases.
    pub(crate) fn incident_selected(
        &self,
        aliases: &[String],
        alias: &str,
        node: Id,
    ) -> Vec<(usize, Id)> {
        let loaded = &self.graphs[alias];
        let mut edges = Vec::new();
        if let Some(local) = loaded.graph.node_ix(node) {
            let part = aliases
                .iter()
                .position(|a| a == alias)
                .expect("selected alias");
            edges.extend(
                loaded
                    .graph
                    .edges_out_external(local)
                    .chain(loaded.graph.edges_in_external(local))
                    .map(|(edge, _, _, _)| (part, loaded.graph.edge_at(edge).id())),
            );
        }
        for &(instance, edge) in self.incoming.entries(loaded.meta.id, node) {
            if let Some(part) = aliases
                .iter()
                .position(|a| self.graphs[a].instance == instance)
            {
                edges.push((part, edge));
            }
        }
        edges.sort_unstable();
        edges.dedup();
        edges
    }

    pub(crate) fn virtual_node(&self, alias: &str) -> umx_core::Node {
        let loaded = &self.graphs[alias];
        umx_core::Node {
            id: loaded.meta.id,
            kind: "graph".into(),
            title: if loaded.meta.title.is_empty() {
                alias.into()
            } else {
                loaded.meta.title.clone()
            },
            status: String::new(),
            note: String::new(),
            evidence: Vec::new(),
            status_history: Vec::new(),
            created: String::new(),
            updated: String::new(),
            created_by: String::new(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields: alloc::collections::BTreeMap::from([
                ("alias".into(), umx_core::Value::String(alias.into())),
                (
                    "address".into(),
                    umx_core::Value::String(alloc::format!("umx:{alias}")),
                ),
            ]),
        }
    }
}
