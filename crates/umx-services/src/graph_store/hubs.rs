//! Hubs (graph.json `hubs`): declared places where nodes gather. A named hub with a pattern joins matching nodes
//! with an edge marked `auto_added_by`, only on hub set, hub fill, node add, node edit and batch. umx never removes
//! such an edge, and never adds one on a read, load, check or edge write. A hub never warns about shape.

use super::edge::new_edge;
use super::rules::RuleMatcher;
use super::{GraphStore, alias_name, persist};
use crate::problems;
use crate::{Ctx, Done, Outcome};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use umx_core::codec::GraphFile;
use umx_core::hubs::{AUTO_ADDED_BY, DEFAULT_VIA, Hub, MAX_HUBS, RETIRED_STATUSES};
use umx_core::ontology::PROVENANCE_TYPES;
use umx_core::rules::Pattern;
use umx_core::ui::Touched;
use umx_core::{Command, Edge, GraphMeta, Id, Node, Problem, ValidName, Value};
use umx_graph::{Batch, Graph, NodeRef};

/// Node line bytes one hub set or fill reads before it stops and says where to carry on.
const SCAN_WORK: u64 = 16 << 20;
/// Edges one hub set or fill adds; past it the call stops and says where to carry on.
const MAX_JOINS: usize = 1_000;
/// Ids a reply lists of the nodes joined or not checked; the totals are always given.
const SHOWN: usize = 100;

#[derive(Clone, Debug, PartialEq)]
pub struct HubJoin {
    pub hub: String,
    pub edge: Id,
    pub node: Id,
    pub to: Id,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HubFilled {
    pub graph: String,
    pub hub: Hub,
    pub joined: Vec<HubJoin>,
    pub joined_total: usize,
    pub not_checked: Vec<Id>,
    pub not_checked_total: usize,
    /// Where the scan stopped, when it did: `hub fill <name> --after <it>` carries on.
    pub after: Option<Id>,
    pub unchanged: bool,
    pub format: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HubRow {
    pub hub: Hub,
    pub hub_nodes: usize,
    pub members: usize,
    /// A named hub whose node is no longer in the graph.
    pub missing: bool,
    pub add: Command,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HubsListed {
    pub graph: String,
    pub rows: Vec<HubRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HubNodeRow {
    pub node: Id,
    pub members: Vec<Id>,
    pub members_total: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HubShown {
    pub graph: String,
    pub hub: Hub,
    pub nodes: Vec<HubNodeRow>,
    pub nodes_total: usize,
    pub offset: usize,
    pub limit: usize,
    pub next: Option<Command>,
    /// A named hub's matching nodes that are not members (first `limit`), and whether the scan saw every node.
    pub candidates: Vec<Id>,
    pub candidates_complete: bool,
    pub add: Command,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HubRemoved {
    pub graph: String,
    pub name: String,
    pub unchanged: bool,
    pub remaining: usize,
    pub format: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HubSetRequest {
    pub name: String,
    pub node: Option<String>,
    pub type_hub: Option<String>,
    pub via: Option<String>,
    pub pattern: Option<Value>,
}

fn hub_name(name: &str) -> ValidName {
    ValidName::new(name).unwrap_or_else(|| ValidName::new("hub").expect("a name"))
}

/// The edge that puts a node in a hub by hand, as `hub list` and `hub show` print it.
fn add_command(hub: &Hub, graph: &ValidName) -> Command {
    let via = ValidName::new(&hub.via).expect("checked via");
    let c = Command::new("edge add").word("<node>").name(&via);
    let c = match hub.node {
        Some(id) => c.id(id),
        None => c.word("<hub>"),
    };
    c.word("--graph").name(graph)
}

fn fill_command(name: &str, after: Option<Id>, graph: &ValidName) -> Command {
    let c = Command::new("hub fill").name(&hub_name(name));
    let c = match after {
        Some(id) => c.word("--after").id(id),
        None => c,
    };
    c.word("--graph").name(graph)
}

fn quick_miss(p: &Pattern, node: NodeRef<'_>) -> bool {
    (!p.types.is_empty() && !p.types.iter().any(|t| t == node.kind()))
        || (!p.status.is_empty() && !p.status.iter().any(|s| s == node.status()))
}

/// The edge umx adds to put `node` in hub `name`.
#[allow(clippy::too_many_arguments)]
pub(super) fn hub_edge(
    id: Id,
    node: Id,
    name: &str,
    via: &str,
    to: Id,
    now: &str,
    by: &str,
    session: Id,
) -> Edge {
    let mut fields = BTreeMap::new();
    fields.insert(AUTO_ADDED_BY.into(), Value::String(name.into()));
    let mut edge = new_edge(id, node, via, to, now, by, fields);
    edge.record_author(session);
    edge
}

/// One `hub_joined` hint per hub, counting the nodes a write joined to it.
pub(super) fn joined_hints(joins: &[HubJoin], graph: &ValidName) -> Vec<Problem> {
    let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
    for j in joins {
        *counts.entry(&j.hub).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(hub, n)| problems::hub_joined(hub, n, graph))
        .collect()
}

/// The named hubs with a pattern whose node is in the graph, compiled once for a write. A hub whose pattern no
/// longer compiles, or whose node is gone, adds nothing.
pub(super) struct Joiner {
    hubs: Vec<(Hub, Id, RuleMatcher)>,
    /// Every named hub's (name, node), for the retired warning.
    named: Vec<(String, Id)>,
}

impl Joiner {
    pub(super) fn new(meta: &GraphMeta, g: &Graph) -> Self {
        let hubs = meta
            .hubs
            .iter()
            .filter_map(|h| {
                let node = h.node?;
                g.node(node)?;
                let matcher = RuleMatcher::new(&h.as_rule()?).ok()?;
                Some((h.clone(), node, matcher))
            })
            .collect();
        let named = meta
            .hubs
            .iter()
            .filter_map(|h| Some((h.name.clone(), h.node?)))
            .collect();
        Self { hubs, named }
    }

    /// `hub_node_retired` for each named hub whose node `node` is, when its status retires it.
    pub(super) fn retired(&self, node: &Node, graph: &ValidName) -> Vec<Problem> {
        if !RETIRED_STATUSES.contains(&node.status.as_str()) {
            return Vec::new();
        }
        self.named
            .iter()
            .filter(|(_, id)| *id == node.id)
            .map(|(hub, _)| problems::hub_node_retired(hub, &node.status, graph))
            .collect()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.hubs.is_empty()
    }

    /// The hubs `node` now belongs in: it matches, and for an edit (`before`) it did not match before, so a hand
    /// delete is never undone by an unrelated edit. Never a node's own hub, never a provenance record. Each is
    /// (hub name, via, hub node); the caller skips the ones the node is already a member of.
    pub(super) fn wanted(
        &mut self,
        node: &Node,
        before: Option<&Node>,
    ) -> Vec<(String, String, Id)> {
        if PROVENANCE_TYPES.contains(&node.kind.as_str()) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (hub, to, matcher) in &mut self.hubs {
            if node.id == *to {
                continue;
            }
            matcher.reset_work();
            if matcher.matches(node) != Some(true) {
                continue;
            }
            if let Some(before) = before {
                matcher.reset_work();
                if matcher.matches(before) == Some(true) {
                    continue;
                }
            }
            out.push((hub.name.clone(), hub.via.clone(), *to));
        }
        out
    }
}

/// Joins `node` to the hubs it now belongs in, inside a single-node write: `held` are the (relation, node) edges
/// this write already gives it; anything else is looked up in the batch. Returns what it added.
#[allow(clippy::too_many_arguments)]
pub(super) fn join_in(
    batch: &mut Batch<'_>,
    joiner: &mut Joiner,
    node: &Node,
    before: Option<&Node>,
    held: &[(&str, Id)],
    session: Id,
    next: &mut dyn FnMut(&Graph) -> Id,
    now: &str,
    by: &str,
) -> Vec<HubJoin> {
    let mut out = Vec::new();
    for (hub, via, to) in joiner.wanted(node, before) {
        if held.iter().any(|(v, t)| *v == via && *t == to)
            || batch.find_edge(node.id, &via, to).is_some()
            || out.iter().any(|j: &HubJoin| {
                j.to == to && batch.edge_record(j.edge).is_some_and(|e| e.kind == via)
            })
        {
            continue;
        }
        let id = next(batch.graph());
        batch.insert_edge(hub_edge(id, node.id, &hub, &via, to, now, by, session));
        out.push(HubJoin {
            hub,
            edge: id,
            node: node.id,
            to,
        });
    }
    out
}

#[derive(Default)]
struct Scan {
    join: Vec<Id>,
    not_checked: Vec<Id>,
    last: Option<Id>,
    cut: bool,
}

fn scan(g: &Graph, hub: &Hub, after: Option<Id>, max_joins: usize) -> Result<Scan, Problem> {
    let to = hub.node.expect("a named hub");
    let pattern = hub.pattern.as_ref().expect("a hub with a pattern");
    let mut matcher = RuleMatcher::new(&hub.as_rule().expect("a hub with a pattern"))?;
    let mut ids: Vec<Id> = g
        .nodes()
        .map(|n| n.id())
        .filter(|id| after.is_none_or(|a| *id > a))
        .collect();
    ids.sort_unstable();
    let mut work = SCAN_WORK;
    let mut out = Scan::default();
    for id in ids {
        let node = g.node(id).expect("a listed node");
        // At least one node per call, so `--after` always moves on.
        if out.last.is_some() {
            match work.checked_sub(node.line().len() as u64 + 1) {
                Some(left) if out.join.len() < max_joins && matcher.can_check_more() => work = left,
                _ => {
                    out.cut = true;
                    break;
                }
            }
        }
        out.last = Some(id);
        if id == to || PROVENANCE_TYPES.contains(&node.kind()) || quick_miss(pattern, node) {
            continue;
        }
        match matcher.matches(&node.to_node()) {
            Some(true) if g.find_edge(id, &hub.via, to).is_none() => out.join.push(id),
            Some(_) => {}
            None => out.not_checked.push(id),
        }
    }
    Ok(out)
}

fn auto_edges(g: &Graph, name: &str) -> usize {
    g.edges()
        .filter(|e| e.field(AUTO_ADDED_BY).as_ref().and_then(Value::as_str) == Some(name))
        .count()
}

/// The hub nodes of `hub` in `g`, by id.
fn hub_nodes(g: &Graph, hub: &Hub) -> Vec<Id> {
    let mut ids: Vec<Id> = match (&hub.node, &hub.kind) {
        (Some(id), _) => g.node(*id).map(|n| n.id()).into_iter().collect(),
        (None, Some(kind)) => g
            .nodes()
            .filter(|n| n.kind() == kind)
            .map(|n| n.id())
            .collect(),
        _ => Vec::new(),
    };
    ids.sort_unstable();
    ids
}

/// The local nodes with a `via` edge into hub node `id`, by id, each once.
fn members(g: &Graph, id: Id, via: &str) -> Vec<Id> {
    let Some(ix) = g.node_ix(id) else {
        return Vec::new();
    };
    let set: BTreeSet<Id> = g
        .edges_in(ix)
        .filter(|s| s.kind.as_str() == via)
        .map(|s| g.node_at(s.other).id())
        .filter(|from| *from != id)
        .collect();
    set.into_iter().collect()
}

/// Each declared hub with its counts and the edge that adds a member by hand (`hub list`, `graph guide`).
pub(super) fn hub_rows(meta: &GraphMeta, g: &Graph, graph: &ValidName) -> Vec<HubRow> {
    meta.hubs
        .iter()
        .map(|hub| {
            let nodes = hub_nodes(g, hub);
            let members: BTreeSet<Id> = nodes
                .iter()
                .flat_map(|id| members(g, *id, &hub.via))
                .collect();
            HubRow {
                hub: hub.clone(),
                hub_nodes: nodes.len(),
                members: members.len(),
                missing: hub.node.is_some() && nodes.is_empty(),
                add: add_command(hub, graph),
            }
        })
        .collect()
}

/// `hub_node_missing` for each named hub whose node is not in the graph (a hand edit or a merge removed it), and
/// `hub_node_retired` for each whose node was moved to a retiring status: load, `umx check`, `hub list`, `hub show`
/// and `hub fill` say so; the graph always loads.
pub(super) fn missing_notes(meta: &GraphMeta, g: &Graph, graph: &ValidName) -> Vec<Problem> {
    meta.hubs
        .iter()
        .filter_map(|h| {
            let id = h.node?;
            match g.node(id) {
                None => Some(problems::hub_node_missing(&h.name, graph)),
                Some(n) if RETIRED_STATUSES.contains(&n.status()) => {
                    Some(problems::hub_node_retired(&h.name, n.status(), graph))
                }
                Some(_) => None,
            }
        })
        .collect()
}

/// [`Joiner::retired`] for one node, after a status move.
pub(super) fn retired_notes(meta: &GraphMeta, node: &Node, graph: &ValidName) -> Vec<Problem> {
    if !RETIRED_STATUSES.contains(&node.status.as_str()) {
        return Vec::new();
    }
    meta.hubs
        .iter()
        .filter(|h| h.node == Some(node.id))
        .map(|h| problems::hub_node_retired(&h.name, &node.status, graph))
        .collect()
}

impl GraphStore {
    /// The graph's hub declarations, for `ontology show` (the view draws declared hubs from them).
    pub fn hubs_of(&self, alias: &str) -> Vec<Hub> {
        self.graphs
            .get(alias)
            .map(|l| l.meta.hubs.clone())
            .unwrap_or_default()
    }

    pub fn hub_list(&self, ctx: &Ctx) -> Outcome<HubsListed> {
        let alias = self.select(ctx, &[])?;
        let l = &self.graphs[&alias];
        let graph = alias_name(&alias);
        let rows = hub_rows(&l.meta, &l.graph, &graph);
        let mut notes = missing_notes(&l.meta, &l.graph, &graph);
        notes.extend(self.stale_note(&alias));
        Ok(Done {
            value: HubsListed { graph: alias, rows },
            notes,
        })
    }

    pub fn hub_show(
        &self,
        ctx: &Ctx,
        name: &str,
        limit: usize,
        offset: usize,
    ) -> Outcome<HubShown> {
        if !(1..=1000).contains(&limit) {
            return Err(problems::hub_invalid(
                "hub show --limit is between 1 and 1000",
            ));
        }
        let alias = self.select(ctx, &[])?;
        let l = &self.graphs[&alias];
        let graph = alias_name(&alias);
        let hub = l
            .meta
            .hubs
            .iter()
            .find(|h| h.name == name)
            .ok_or_else(|| problems::hub_not_found(name))?;
        let all = hub_nodes(&l.graph, hub);
        let end = offset.saturating_add(limit).min(all.len());
        let nodes = all
            .iter()
            .skip(offset)
            .take(limit)
            .map(|id| {
                let m = members(&l.graph, *id, &hub.via);
                HubNodeRow {
                    node: *id,
                    members_total: m.len(),
                    members: m.into_iter().take(limit).collect(),
                }
            })
            .collect();
        let next = (end < all.len()).then(|| {
            Command::new("hub show")
                .name(&hub_name(name))
                .word("--offset")
                .count(end as u64)
                .word("--limit")
                .count(limit as u64)
                .word("--graph")
                .name(&graph)
        });
        let mut notes = Vec::new();
        let (candidates, candidates_complete) = match (&hub.pattern, &all[..]) {
            (Some(_), [_]) => {
                let s = scan(&l.graph, hub, None, limit)?;
                if !s.join.is_empty() {
                    notes.push(problems::hub_unjoined(
                        name,
                        fill_command(name, None, &graph),
                    ));
                }
                (s.join, !s.cut)
            }
            _ => (Vec::new(), true),
        };
        notes.extend(
            missing_notes(&l.meta, &l.graph, &graph)
                .into_iter()
                .filter(|p| p.subject.as_deref() == Some(name)),
        );
        notes.extend(self.stale_note(&alias));
        Ok(Done {
            value: HubShown {
                graph: alias,
                hub: hub.clone(),
                nodes,
                nodes_total: all.len(),
                offset,
                limit,
                next,
                candidates,
                candidates_complete,
                add: add_command(hub, &graph),
            },
            notes,
        })
    }

    pub fn hub_set(&mut self, ctx: &Ctx, req: HubSetRequest) -> Outcome<HubFilled> {
        let target = req.node.as_deref().map(super::resolve::fold_scheme);
        let alias = self.select_write(ctx, &[target.as_deref()])?;
        let graph = alias_name(&alias);
        let node = match target.as_deref() {
            Some(text) => Some(super::resolve_node_in(self.get(&alias), ctx, &alias, text)?),
            None => None,
        };
        let via = match (req.via, node) {
            (Some(via), _) => via,
            (None, Some(_)) => DEFAULT_VIA.into(),
            (None, None) => {
                return Err(problems::hub_invalid(
                    "a type hub needs --via, the relation its members join it by",
                ));
            }
        };
        let pattern = match req.pattern {
            Some(value) => Some(umx_core::hubs::pattern_from_value(&value).ok_or_else(|| {
                problems::hub_invalid("a pattern has only type, status, tag, field and regex")
            })?),
            None => None,
        };
        let hub = Hub {
            name: req.name,
            node,
            kind: req.type_hub,
            via,
            pattern,
        };
        hub.check().map_err(problems::hub_invalid)?;
        if let Some(rule) = hub.as_rule() {
            RuleMatcher::new(&rule)?;
        }
        let l = &self.graphs[&alias];
        let existing = l.meta.hubs.iter().find(|h| h.name == hub.name);
        if existing == Some(&hub) {
            return Ok(Done {
                value: HubFilled {
                    graph: alias.clone(),
                    hub,
                    joined: Vec::new(),
                    joined_total: 0,
                    not_checked: Vec::new(),
                    not_checked_total: 0,
                    after: None,
                    unchanged: true,
                    format: l.meta.umx,
                },
                notes: self.stale_note(&alias).into_iter().collect(),
            });
        }
        if existing.is_none() && l.meta.hubs.len() >= MAX_HUBS {
            return Err(problems::hub_invalid("a graph holds at most 64 hubs"));
        }
        let mut notes = Vec::new();
        if let Some(old) = existing
            && (old.node != hub.node || old.via != hub.via)
        {
            let remaining = auto_edges(&l.graph, &hub.name);
            if remaining > 0 {
                notes.push(problems::hub_edges_remain(
                    &hub.name,
                    remaining as u64,
                    &graph,
                ));
            }
        }
        let found = match hub.pattern {
            Some(_) => scan(&l.graph, &hub, None, MAX_JOINS)?,
            None => Scan::default(),
        };
        let before = l.meta.clone();
        let l = self.graphs.get_mut(&alias).expect("selected");
        l.meta.hubs.retain(|h| h.name != hub.name);
        l.meta.hubs.push(hub.clone());
        l.meta.hubs.sort_by(|a, b| a.name.cmp(&b.name));
        let done = if found.join.is_empty() {
            let files = self.ports.files.clone();
            persist(
                files.as_ref(),
                &alias,
                l,
                &[GraphFile::Graph],
                Touched::default(),
            )
            .map(|notes| Done {
                value: Vec::new(),
                notes,
            })
        } else {
            self.join_all(ctx, &alias, &hub, &found.join, &[GraphFile::Graph])
        };
        let done = match done {
            Ok(done) => done,
            Err(e) => {
                self.graphs.get_mut(&alias).expect("selected").meta = before;
                return Err(e);
            }
        };
        notes.extend(done.notes);
        Ok(self.filled(&alias, hub, done.value, found, false, notes))
    }

    pub fn hub_fill(&mut self, ctx: &Ctx, name: &str, after: Option<&str>) -> Outcome<HubFilled> {
        let alias = self.select_write(ctx, &[])?;
        let graph = alias_name(&alias);
        let l = &self.graphs[&alias];
        let hub = l
            .meta
            .hubs
            .iter()
            .find(|h| h.name == name)
            .cloned()
            .ok_or_else(|| problems::hub_not_found(name))?;
        if hub.pattern.is_none() {
            return Err(problems::hub_invalid(
                "only a named hub with a pattern can be filled; add members with the edge hub list prints",
            ));
        }
        let after = match after {
            Some(text) => Some(match text.parse::<Id>() {
                Ok(id) => id,
                Err(_) => super::resolve_node_in(l, ctx, &alias, text)?,
            }),
            None => None,
        };
        if hub.node.is_some_and(|id| l.graph.node(id).is_none()) {
            let notes = alloc::vec![problems::hub_node_missing(name, &graph)];
            return Ok(self.filled(&alias, hub, Vec::new(), Scan::default(), false, notes));
        }
        let found = scan(&l.graph, &hub, after, MAX_JOINS)?;
        let done = if found.join.is_empty() {
            Done {
                value: Vec::new(),
                notes: Vec::new(),
            }
        } else {
            self.join_all(ctx, &alias, &hub, &found.join, &[])?
        };
        Ok(self.filled(&alias, hub, done.value, found, false, done.notes))
    }

    fn filled(
        &self,
        alias: &str,
        hub: Hub,
        joined: Vec<HubJoin>,
        found: Scan,
        unchanged: bool,
        mut notes: Vec<Problem>,
    ) -> Done<HubFilled> {
        let graph = alias_name(alias);
        notes.extend(joined_hints(&joined, &graph));
        // The edges umx just added can meet or break a graph rule at either end, as an edge add's would.
        if !joined.is_empty() {
            let mut ids: Vec<Id> = joined.iter().map(|j| j.node).collect();
            ids.extend(hub.node);
            notes.extend(self.rule_write_notes(alias, &ids));
        }
        let after = found.cut.then_some(found.last).flatten();
        if let Some(at) = after {
            notes.push(problems::hub_scan_cut(
                &hub.name,
                fill_command(&hub.name, Some(at), &graph),
            ));
        }
        Done {
            value: HubFilled {
                graph: alias.into(),
                joined_total: joined.len(),
                joined: joined.into_iter().take(SHOWN).collect(),
                not_checked_total: found.not_checked.len(),
                not_checked: found.not_checked.into_iter().take(SHOWN).collect(),
                after,
                unchanged,
                format: self.graphs[alias].meta.umx,
                hub,
            },
            notes,
        }
    }

    /// One write that joins each of `ids` to the named `hub`, with `extra` files (graph.json for hub set).
    fn join_all(
        &mut self,
        ctx: &Ctx,
        alias: &str,
        hub: &Hub,
        ids: &[Id],
        extra: &[GraphFile],
    ) -> Outcome<Vec<HubJoin>> {
        let to = hub.node.expect("a named hub");
        self.attributed_write_with(ctx, alias, extra, |batch, session, next, now, _| {
            let mut joined = Vec::with_capacity(ids.len());
            for &node in ids {
                let id = next(batch.graph());
                batch.insert_edge(hub_edge(
                    id, node, &hub.name, &hub.via, to, now, &ctx.by, session,
                ));
                joined.push(HubJoin {
                    hub: hub.name.clone(),
                    edge: id,
                    node,
                    to,
                });
            }
            Ok(joined)
        })
    }

    pub fn hub_remove(&mut self, ctx: &Ctx, name: &str) -> Outcome<HubRemoved> {
        let alias = self.select_write(ctx, &[])?;
        let graph = alias_name(&alias);
        let before = self.graphs[&alias].meta.clone();
        let unchanged = !before.hubs.iter().any(|h| h.name == name);
        let remaining = auto_edges(&self.graphs[&alias].graph, name);
        let files = self.ports.files.clone();
        let l = self.graphs.get_mut(&alias).expect("selected");
        let mut notes = if unchanged {
            Vec::new()
        } else {
            l.meta.hubs.retain(|h| h.name != name);
            persist(
                files.as_ref(),
                &alias,
                l,
                &[GraphFile::Graph],
                Touched::default(),
            )
            .inspect_err(|_| l.meta = before)?
        };
        if remaining > 0 && ValidName::new(name).is_some() {
            notes.push(problems::hub_edges_remain(name, remaining as u64, &graph));
        }
        Ok(Done {
            value: HubRemoved {
                graph: alias,
                name: name.into(),
                unchanged,
                remaining,
                format: l.meta.umx,
            },
            notes,
        })
    }
}
