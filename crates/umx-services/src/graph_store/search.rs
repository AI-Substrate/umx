//! Which graphs `umx search` covers (SEC-5, blueprint d-20) and where its `--within` root is; the search itself is
//! `crate::hybrid`.

use super::{Ctx, GraphStore, problems};
use crate::Scope;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use umx_core::{Id, Problem};
use umx_graph::{EdgeRef, Graph, NodeIx, NodeRef, NodeSet};

/// The node scope shared by default reads, after d-20/d-21 select the graphs.
#[derive(Clone, Copy)]
pub(crate) struct ReadScope<'g> {
    graph: &'g Graph,
    include_provenance: bool,
}

impl<'g> ReadScope<'g> {
    pub(crate) fn new(graph: &'g Graph) -> Self {
        Self {
            graph,
            include_provenance: false,
        }
    }

    pub(crate) fn with_provenance(mut self, include: bool) -> Self {
        self.include_provenance = include;
        self
    }

    pub(crate) fn contains_ix(self, node: NodeIx) -> bool {
        self.include_provenance || !self.graph.is_provenance(node)
    }

    pub(crate) fn contains(self, id: Id) -> bool {
        self.graph
            .node_ix(id)
            .is_some_and(|ix| self.contains_ix(ix))
    }

    pub(crate) fn nodes(self) -> impl Iterator<Item = NodeRef<'g>> {
        self.graph.nodes().filter(move |n| self.contains_ix(n.ix()))
    }

    pub(crate) fn node_count(self) -> usize {
        self.graph.node_count()
            - if self.include_provenance {
                0
            } else {
                self.graph.provenance_count()
            }
    }

    pub(crate) fn census(self) -> umx_graph::Census<'g> {
        if self.include_provenance {
            self.graph.census()
        } else {
            self.graph.content_census()
        }
    }

    pub(crate) fn counts(self) -> (usize, usize) {
        if self.include_provenance {
            (self.graph.node_count(), self.graph.edge_count())
        } else {
            self.graph.content_counts()
        }
    }

    pub(crate) fn stored_edge(self, edge: EdgeRef<'_>) -> bool {
        [edge.from_ix(), edge.to_ix()]
            .into_iter()
            .flatten()
            .all(|ix| self.contains_ix(ix))
    }

    pub(crate) fn stored_record(self, edge: &umx_core::Edge) -> bool {
        [(edge.from_graph, edge.from), (edge.to_graph, edge.to)]
            .into_iter()
            .all(|(graph, node)| graph.is_some() || self.contains(node))
    }

    pub(crate) fn edges(self) -> impl Iterator<Item = EdgeRef<'g>> {
        self.graph.edges().filter(move |e| self.stored_edge(*e))
    }

    pub(crate) fn within(self, root: Id) -> NodeSet<'g> {
        self.graph.within_where(root, |ix| self.contains_ix(ix))
    }
}

/// The graphs a scoped read covers (d-20, d-21), and what it left out.
pub(crate) struct Searched {
    pub(crate) aliases: Vec<String>,
    /// Secured graphs an unnamed read left out, counted only (d-20).
    pub(crate) skipped: usize,
    /// Other copies of graphs an unnamed read took the primary of, named in copies_skipped (d-21).
    pub(crate) copies_skipped: Vec<String>,
    /// Named graphs that are copies of another graph named (query_graph_twice, d-21).
    pub(crate) twice: Vec<String>,
    /// Whether the request named its graphs (`--graph`, `UMX_GRAPH`, an address): then a secured graph is searched;
    /// otherwise only open ones, checked again before results are built (SEC-5, d-20).
    pub(crate) named: bool,
    pub(crate) saved: Option<super::SavedRead>,
}

impl GraphStore {
    pub(crate) fn read_census(&self, ctx: &Ctx, alias: &str) -> umx_graph::Census<'_> {
        let loaded = self.get(alias);
        let mut census = ctx.read_scope(&loaded.graph).census();
        let mut relations = alloc::collections::BTreeMap::new();
        for edge in ctx.read_scope(&loaded.graph).edges() {
            if self.edge_read_row(ctx, &[], alias, edge.id()).is_some() {
                let tally = relations.entry(edge.kind()).or_insert(umx_graph::Tally {
                    name: edge.kind(),
                    count: 0,
                    first: edge.id(),
                });
                tally.count += 1;
            }
        }
        census.relations = relations.into_values().collect();
        census
    }

    /// The graphs a read that names none covers: d-20 first (secured graphs left out and counted, whatever their
    /// copies), then one copy per graph id (d-21), so copies_skipped never names a secured graph's copies. The one
    /// scope step for every scoped verb: search and query read all of it; a single-graph verb reads it when it is one
    /// graph (`choose`).
    pub(crate) fn unnamed_scope(&self) -> Result<Searched, Problem> {
        self.unnamed_scope_with_names(&[])
    }

    pub(crate) fn unnamed_scope_with_names(&self, named: &[String]) -> Result<Searched, Problem> {
        let named_ids: Vec<Id> = named.iter().map(|a| self.graphs[a].meta.id).collect();
        let (mut open, mut named_copies, mut secured) = (Vec::new(), Vec::new(), 0);
        for alias in self.graphs.keys() {
            if named.contains(alias) {
                continue;
            }
            if !self.shown_unnamed(alias) {
                secured += 1;
            } else if named_ids.contains(&self.graphs[alias].meta.id) {
                named_copies.push(alias.clone());
            } else {
                open.push(alias.clone());
            }
        }
        let (mut open, mut copies_skipped, hidden) = self.one_copy_each(open)?;
        open.extend_from_slice(named);
        copies_skipped.extend(named_copies);
        Ok(Searched {
            aliases: open,
            skipped: secured + hidden,
            copies_skipped,
            twice: self.named_twice(named),
            named: false,
            saved: None,
        })
    }

    /// The graphs a search covers (SEC-5, blueprint d-20). `--graph a,b` names them; one name (or an address in
    /// `within`, or `UMX_GRAPH`) is the usual one graph; a named graph is searched whatever its policy. With none
    /// named, every loaded graph whose effective policy is open: a secured graph, or one whose policy cannot be read,
    /// is left out and only counted. Each graph searched counts as used (idle unload).
    pub(crate) fn search_graphs(
        &self,
        ctx: &Ctx,
        within: Option<&str>,
    ) -> Result<Searched, Problem> {
        self.graphs_in_scope(ctx, within, "search")
    }

    /// [`Self::search_graphs`] for any verb that reads several graphs (`noun` names its docs topic in a usage
    /// error): `umx search` and `umx query`.
    pub(crate) fn graphs_in_scope(
        &self,
        ctx: &Ctx,
        within: Option<&str>,
        noun: &'static str,
    ) -> Result<Searched, Problem> {
        self.graphs_in_scope_with_names(ctx, within, noun, &[])
    }

    pub(crate) fn graphs_in_scope_with_names(
        &self,
        ctx: &Ctx,
        within: Option<&str>,
        noun: &'static str,
        address_names: &[String],
    ) -> Result<Searched, Problem> {
        if ctx.graph.is_some() && ctx.scope.is_some() {
            return Err(problems::usage_invalid(
                "--graph and --scope cannot be used together",
                noun,
            ));
        }
        let explicit = ctx.scope.clone().or_else(|| {
            ctx.graph
                .as_deref()
                .filter(|g| g.contains(','))
                .map(|list| {
                    Scope::Graphs(
                        list.split(',')
                            .map(str::trim)
                            .map(ToString::to_string)
                            .collect(),
                    )
                })
        });
        let aliases = match explicit {
            Some(Scope::All { include_named }) => {
                // `embeddings`: `umx similarity pairs`, documented in the embeddings topic.
                if !matches!(noun, "search" | "query" | "embeddings") {
                    return Err(problems::usage_invalid(
                        "--all is only supported by search, query and similarity pairs",
                        noun,
                    ));
                }
                if self.graphs.is_empty() {
                    return Err(problems::graph_none_loaded());
                }
                if include_named {
                    let (aliases, copies_skipped, _) =
                        self.one_copy_each(self.graphs.keys().cloned().collect())?;
                    Searched {
                        aliases,
                        named: true,
                        skipped: 0,
                        copies_skipped,
                        twice: Vec::new(),
                        saved: None,
                    }
                } else {
                    self.unnamed_scope()?
                }
            }
            Some(Scope::Saved(name)) => self.saved_scope(&name)?,
            Some(Scope::Graphs(list)) => {
                let mut out: Vec<String> = Vec::new();
                for a in &list {
                    if a.is_empty() {
                        return Err(problems::usage_invalid(
                            "--graph contains an empty graph name",
                            noun,
                        ));
                    }
                    // ws006 S23: a former alias names its graph, so `--graph a,c` with `a` a former alias of
                    // `c` is one graph (graph-addressing-expert).
                    let a = self.canonical(ctx, a)?;
                    if !self.graphs.contains_key(&a) {
                        return Err(self.not_loaded(&a));
                    }
                    if !out.contains(&a) {
                        out.push(a);
                    }
                }
                if out.is_empty() {
                    return Err(problems::usage_invalid("--graph names no graph", noun));
                }
                Searched {
                    twice: self.named_twice(&out),
                    aliases: out,
                    skipped: 0,
                    copies_skipped: Vec::new(),
                    named: true,
                    saved: None,
                }
            }
            None => {
                let named = ctx.graph.is_some()
                    || ctx.default_graph.is_some()
                    || self.addressed(ctx, &[within])?.is_some();
                if named {
                    Searched {
                        aliases: alloc::vec![self.select(ctx, &[within])?],
                        skipped: 0,
                        copies_skipped: Vec::new(),
                        twice: Vec::new(),
                        named: true,
                        saved: None,
                    }
                } else if self.graphs.is_empty() {
                    return Err(problems::graph_none_loaded());
                } else {
                    let unnamed = self.unnamed_scope_with_names(address_names)?;
                    let open = unnamed.aliases;
                    // A bare id with no graph named: the one open graph holds it, or the caller names one. (Never
                    // select's ambiguity error, which would list secured aliases.)
                    if within.is_some() && open.len() != 1 {
                        return Err(problems::usage_invalid(
                            "--within needs --graph when more or less than one open graph is loaded",
                            "search",
                        ));
                    }
                    Searched {
                        aliases: open,
                        ..unnamed
                    }
                }
            }
        };
        let now = self.ports.clock.now_ms();
        for a in &aliases.aliases {
            self.graphs[a].last_used.store(now, Ordering::Relaxed);
        }
        Ok(aliases)
    }

    pub(crate) fn named_graphs_read(&self, aliases: &[String]) -> Option<Problem> {
        let named: Vec<String> = aliases
            .iter()
            .filter(|a| self.graphs.contains_key(a.as_str()) && !self.shown_unnamed(a))
            .cloned()
            .collect();
        (!named.is_empty()).then(|| problems::secured_graphs_read(&named))
    }

    /// The graphs d-20 left out of `scope`, copies hidden since included: the count `secured_graphs_skipped` reports.
    pub(crate) fn secured_count(&self, scope: &Searched) -> usize {
        let shown = scope
            .copies_skipped
            .iter()
            .filter(|a| self.shown_unnamed(a))
            .count();
        scope.skipped + scope.copies_skipped.len() - shown
    }

    pub(crate) fn scope_warnings(&self, scope: &Searched) -> Result<Vec<Problem>, Problem> {
        let others: Vec<String> = scope
            .copies_skipped
            .iter()
            .filter(|a| self.shown_unnamed(a))
            .cloned()
            .collect();
        let secured = self.secured_count(scope);
        let mut warnings = Vec::new();
        if secured > 0 {
            warnings.push(problems::secured_graphs_skipped(secured));
        }
        if !others.is_empty() {
            warnings.push(problems::copies_skipped(&others));
        }
        if !scope.twice.is_empty() {
            warnings.push(problems::graph_twice(&scope.twice));
        }
        if let Some(saved) = &scope.saved {
            warnings.extend(self.saved_warnings(saved, &scope.aliases)?);
        }
        Ok(warnings)
    }

    /// The node `within` names and the graph it is in, among `aliases`. An address must name one of them; a bare id
    /// is taken only when one graph is searched.
    pub(crate) fn search_root(
        &self,
        ctx: &Ctx,
        aliases: &[String],
        within: &str,
    ) -> Result<(String, Id), Problem> {
        let alias = match self.addressed(ctx, &[Some(within)])? {
            Some(a) if aliases.contains(&a) => a,
            Some(a) => return Err(problems::graph_mismatch(&a, &aliases.join(","))),
            None if aliases.len() == 1 => aliases[0].clone(),
            None => {
                return Err(problems::usage_invalid(
                    "--within needs an address (umx:<graph>/<id>) when several graphs are searched",
                    "search",
                ));
            }
        };
        let id = super::resolve_node_in(self.get(&alias), ctx, &alias, within)?;
        Ok((alias, id))
    }
}
