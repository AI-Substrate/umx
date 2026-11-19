//! `--along <spine> [--after <end>] [--before <end>]` (D71 step 12, plan 206): the spine range filter on reads, in
//! place of an "as at" feature (R3). One graph only, refused before any work when a read names a list, a scope,
//! `--all` or `--provenance`. A declared spine (TM5) is placed by the one placement rule ([`place`]) and its ends are
//! anchors it places; a built-in (`recorded`, `status`, `date_field:<f>`) is a date per node and its ends are dates.
//! Both ends are inclusive: on a declared spine by band (an anchor's own band is in), on a built-in by instant
//! ([`DateSpan::relation`], so an overlap is in). A node the spine does not place, or places only by rule, is left
//! out and counted, and the reply says so with the command that lists them (B4a, Kind Hint).
//!
//! Each verb builds the [`Kept`] set once per request, keeps its rows through it before paging, and counts what it
//! left out: node list, node find and edge list over the rows their other filters kept; search and query over the
//! graph's nodes (cypher-expert C3: a query's own match is not known without a second run).

use super::spine::{AnchorView, Facts, How, NO_BAND, Placement, place, recorded_span};
use super::{GraphStore, Loaded, alias_name, problems, split_address};
use crate::Ctx;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use umx_core::ontology::FieldKind;
use umx_core::problem::Candidate;
use umx_core::spine::{Clock, SpineDecl};
use umx_core::time::{DateSpan, Precision, date_span, normalise_date};
use umx_core::{Command, Id, IdText, Problem, SafeText, Slot, ValidName};
use umx_graph::{Graph, NodeIx, NodeSet};

/// The built-in spines' names (brief 5.2), reserved: TM5 refuses a declaration under one (pm-001).
pub const RECORDED: &str = "recorded";
pub const STATUS: &str = "status";
pub const DATE_FIELD_PREFIX: &str = "date_field:";

/// Whether `name` is a built-in spine's name, which a declaration may not take.
pub fn reserved_spine_name(name: &str) -> bool {
    name == RECORDED || name == STATUS || name.starts_with(DATE_FIELD_PREFIX)
}

/// What a read asks for (the wire's `Along`, mapped once by the daemon).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AlongRequest {
    pub spine: String,
    pub after: Option<String>,
    pub before: Option<String>,
}

/// Where the spine came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlongSource {
    Declared,
    Builtin,
}

impl AlongSource {
    pub fn as_str(self) -> &'static str {
        match self {
            AlongSource::Declared => "declared",
            AlongSource::Builtin => "builtin",
        }
    }
}

/// The reply's `along` object: the spine and ends used, and what the filter left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlongInfo {
    pub spine: String,
    pub source: AlongSource,
    pub clock: Clock,
    /// The ends as resolved: an anchor's full id, or a date in umx's stored form.
    pub after: Option<String>,
    pub before: Option<String>,
    /// Rows left out because the spine does not place them (for edge list, edges with such an end).
    pub not_placed: usize,
    /// Rows left out because the spine places them only by rule.
    pub by_rule: usize,
    /// Rows a built-in compared at month or year precision (B4a).
    pub coarse: usize,
    /// How many other spines `--along` could read on this graph (declared, then built-ins).
    pub spines: usize,
}

/// Why a node is in or out of the range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Why {
    In,
    Out,
    NotPlaced,
    ByRule,
}

/// One request's range: why each node (by handle) is in or out, and the counts so far.
pub(crate) struct Kept {
    why: Vec<Why>,
    /// Built-ins only: the node's value is a month or a year.
    coarse: Vec<bool>,
    info: AlongInfo,
    unplaced_next: Option<Command>,
    by_rule_next: Option<Command>,
}

/// What search and query count, as their hints say it (cypher-expert N1): the graph's nodes, not the rows returned.
pub(crate) const GRAPH_WIDE: &str = "of the graph's nodes";

impl Kept {
    pub(crate) fn why(&self, ix: NodeIx) -> Why {
        self.why.get(ix.index()).copied().unwrap_or(Why::NotPlaced)
    }

    pub(crate) fn keeps(&self, ix: NodeIx) -> bool {
        self.why(ix) == Why::In
    }

    /// Counts one row the verb's other filters kept, by why it is in or out.
    pub(crate) fn count(&mut self, why: Why, ix: Option<NodeIx>) {
        match why {
            Why::NotPlaced => self.info.not_placed += 1,
            Why::ByRule => self.info.by_rule += 1,
            Why::In | Why::Out => {}
        }
        if let Some(ix) = ix
            && self.coarse.get(ix.index()).copied().unwrap_or(false)
        {
            self.info.coarse += 1;
        }
    }

    /// Counts a node row, and says whether it is kept.
    pub(crate) fn admit(&mut self, ix: NodeIx) -> bool {
        let why = self.why(ix);
        self.count(why, Some(ix));
        why == Why::In
    }

    /// Counts every content node of the graph (search and query).
    pub(crate) fn count_graph(&mut self, g: &Graph) {
        for n in g.nodes() {
            if !g.is_provenance(n.ix()) {
                self.admit(n.ix());
            }
        }
    }

    /// The kept nodes as a set, for search's legs and the query view.
    pub(crate) fn set<'g>(&self, g: &'g Graph) -> NodeSet<'g> {
        g.node_set_where(|ix| self.keeps(ix))
    }

    /// The reply's object, and the two hints when they count anything; `unplaced` and `ruled` name the rows each
    /// counts (`nodes`, `edges`, or [`GRAPH_WIDE`] for search and query).
    pub(crate) fn finish(
        self,
        unplaced: &'static str,
        ruled: &'static str,
        notes: &mut Vec<Problem>,
    ) -> AlongInfo {
        let spine = spine_slot(&self.info.spine);
        if self.info.not_placed > 0 {
            notes.push(problems::along_not_placed(
                self.info.not_placed,
                unplaced,
                spine.clone(),
                self.unplaced_next,
            ));
        }
        if self.info.by_rule > 0 {
            notes.push(problems::along_by_rule(
                self.info.by_rule,
                ruled,
                spine,
                self.by_rule_next,
            ));
        }
        self.info
    }
}

/// A spine's name in a message: a declared name or `recorded`/`status` as is; `date_field:<f>` is umx's prefix and
/// a checked name, so it is safe text.
fn spine_slot(name: &str) -> Slot {
    match ValidName::new(name) {
        Some(n) => Slot::Name(n),
        None => Slot::Path(SafeText::lossy(name)),
    }
}

/// An end as a message shows it: id text, or a date in umx's stored form, never the typed text.
fn end_slot(text: &str) -> Slot {
    if let Some(t) = IdText::new(text) {
        return Slot::IdText(t);
    }
    match normalise_date(text) {
        Some(d) => Slot::Path(SafeText::lossy(&d)),
        None => Slot::Word("(that end)"),
    }
}

/// An edge's place in the range from its two ends': out of the spine if either end is, by rule if either end is,
/// else in only when both are.
pub(crate) fn edge_why(a: Why, b: Why) -> Why {
    match (a, b) {
        (Why::NotPlaced, _) | (_, Why::NotPlaced) => Why::NotPlaced,
        (Why::ByRule, _) | (_, Why::ByRule) => Why::ByRule,
        (Why::In, Why::In) => Why::In,
        _ => Why::Out,
    }
}

/// The refusals `--along` makes before any work (architect condition 3): one graph, content only.
pub fn one_graph(ctx: &Ctx) -> Result<(), Problem> {
    let why = if matches!(ctx.scope, Some(crate::Scope::All { .. })) {
        Some("--along reads one graph; drop --all")
    } else if ctx.scope.is_some() {
        Some("--along reads one graph; drop --scope")
    } else if ctx.graph.as_deref().is_some_and(|g| g.contains(',')) {
        Some("--along reads one graph; give --graph one name")
    } else if ctx.include_provenance {
        Some("--along reads content nodes only (provenance is never on a spine); drop --provenance")
    } else {
        None
    };
    why.map_or(Ok(()), |w| Err(problems::usage_invalid(w, "time")))
}

/// The date fields a built-in may read: declared on some type with kind date (as the proposer's built-ins).
fn date_fields(g: &Graph, o: &umx_core::Ontology) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in o.type_names() {
        for d in o
            .type_decl(t)
            .map(|t| t.fields.as_slice())
            .unwrap_or_default()
        {
            if d.kind == FieldKind::Date && !out.iter().any(|f| f == &d.name) {
                out.push(d.name.clone());
            }
        }
    }
    out.sort();
    out.retain(|f| held(g, f));
    out
}

/// Whether some content node holds a date in `field`, read from the typed time column (as the codec decodes it).
fn held(g: &Graph, field: &str) -> bool {
    let mut any = false;
    g.each_date(field, false, |_, _, _| any = true);
    any
}

/// Every name `--along` takes on this graph: the declared spines, then the built-ins.
fn names(l: &Loaded) -> Vec<String> {
    let mut out: Vec<String> = l.meta.spines.iter().map(|s| s.name.clone()).collect();
    for b in [RECORDED, STATUS] {
        if !out.iter().any(|n| n == b) {
            out.push(b.to_string());
        }
    }
    for f in date_fields(&l.graph, l.ontology()) {
        let name = alloc::format!("{DATE_FIELD_PREFIX}{f}");
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

impl GraphStore {
    /// Builds one request's range on graph `alias` (already chosen, one graph). Reads only.
    pub(crate) fn along(
        &self,
        ctx: &Ctx,
        alias: &str,
        req: &AlongRequest,
    ) -> Result<Kept, Problem> {
        one_graph(ctx)?;
        let l = self.get(alias);
        let graph = alias_name(alias);
        let names = names(l);
        let spines = names.len().saturating_sub(1);
        if let Some(decl) = l.meta.spines.iter().find(|s| s.name == req.spine) {
            // ws006 S23 (graph-addressing-expert N3): an end may name this graph by a former alias, with the hint.
            let here = |selector: &str| self.live_in(ctx, selector) == alias;
            return declared(ctx, l, alias, &graph, decl, req, spines, &here);
        }
        let field = req.spine.strip_prefix(DATE_FIELD_PREFIX);
        let known = names.iter().any(|n| n == &req.spine);
        if !known || field.is_some_and(|f| ValidName::new(f).is_none()) {
            let candidates = names
                .iter()
                .map(|n| Candidate {
                    id: n.clone(),
                    title: None,
                    score: None,
                })
                .collect();
            return Err(problems::spine_not_found(&graph, &req.spine, candidates));
        }
        builtin(l, &graph, req, field, spines)
    }
}

/// A declared spine: [`place`] once, positions by band, ends are anchors it places. `here` says whether an address's
/// selector names this graph.
#[allow(clippy::too_many_arguments)]
fn declared(
    ctx: &Ctx,
    l: &Loaded,
    alias: &str,
    graph: &ValidName,
    decl: &SpineDecl,
    req: &AlongRequest,
    spines: usize,
    here: &dyn Fn(&str) -> bool,
) -> Result<Kept, Problem> {
    let g = &l.graph;
    let f = Facts::read_for_decl(g, decl);
    let (p, v) = place(&f, decl);
    let (position, rank) = positions(&p, &v);
    let name = ValidName::new(&decl.name);
    let end = |text: &str| -> Result<(i64, Id), Problem> {
        let invalid = |why: &'static str| {
            // The spine's first anchors in order (an error path, so one pass over the nodes is fine).
            let mut first: Vec<(u32, Candidate)> = g
                .nodes()
                .filter(|node| {
                    let i = node.ix().index();
                    v.is_anchor[i] && p.how[i] != How::None
                })
                .map(|node| {
                    let c = Candidate {
                        id: node.id().to_text(),
                        title: Some(node.title().to_string()),
                        score: None,
                    };
                    (rank[node.ix().index()], c)
                })
                .collect();
            first.sort_by_key(|(r, _)| *r);
            let first = first.into_iter().take(5).map(|(_, c)| c).collect();
            along_end_invalid(graph, &decl.name, text, why).candidates(first)
        };
        if let Some(why) = foreign(l, here, text) {
            return Err(invalid(why));
        }
        let id = super::resolve_node_in(l, ctx, alias, text)?;
        let ix = g.node_ix(id).expect("resolved");
        let i = ix.index();
        if !v.is_anchor[i] {
            return Err(invalid("it is not one of the spine's anchors"));
        }
        if p.how[i] == How::None {
            return Err(invalid("the spine has no order for that anchor"));
        }
        Ok((i64::from(rank[i]), id))
    };
    let after = req.after.as_deref().map(end).transpose()?;
    let before = req.before.as_deref().map(end).transpose()?;
    if let (Some((a, _)), Some((b, _))) = (after, before)
        && a > b
    {
        return Err(reversed());
    }
    let why = range(&f, &position, after.map(|(a, _)| a), before.map(|(b, _)| b));
    let clock = match decl.order.first() {
        Some(o)
            if !g.relation_kinds().iter().any(|k| k.as_str() == o)
                && o != umx_core::standard::STEP =>
        {
            Clock::Calendar
        }
        _ => Clock::Order,
    };
    let show = Command::new("graph spine show").name(graph);
    Ok(Kept {
        why,
        coarse: Vec::new(),
        info: AlongInfo {
            spine: decl.name.clone(),
            source: AlongSource::Declared,
            clock,
            after: after.map(|(_, id)| id.to_text()),
            before: before.map(|(_, id)| id.to_text()),
            not_placed: 0,
            by_rule: 0,
            coarse: 0,
            spines,
        },
        unplaced_next: name
            .as_ref()
            .map(|s| show.clone().word("--unplaced").name(s)),
        by_rule_next: name.as_ref().map(|s| show.word("--by-rule").name(s)),
    })
}

/// Each node's position along a declared spine, from [`place`]'s output only: an anchor's rank, else the band it sits
/// in; a node a calendar spine places by its own date before every anchor sits at -1 ([`place`] places by own date
/// only beside dated anchors, so every placement has a position). A node placed only by rule, or not at all, has
/// none. Also each anchor's rank (`u32::MAX` for a non-anchor).
pub(crate) fn positions(p: &Placement, v: &AnchorView) -> (Vec<Result<i64, Why>>, Vec<u32>) {
    let n = p.how.len();
    let mut rank = vec![u32::MAX; n];
    for (r, &a) in v.anchors.iter().enumerate() {
        rank[a] = r as u32;
    }
    let position = (0..n)
        .map(|i| match p.how[i] {
            How::None => Err(Why::NotPlaced),
            How::Rule => Err(Why::ByRule),
            _ if v.is_anchor[i] => Ok(i64::from(rank[i])),
            _ if p.band[i] != NO_BAND => Ok(p.band[i] as i64),
            _ => Ok(-1),
        })
        .collect();
    (position, rank)
}

/// Why each content node is in or out of `[after, before]` (both inclusive, either open) along a declared spine.
pub(crate) fn range(
    f: &Facts<'_>,
    position: &[Result<i64, Why>],
    after: Option<i64>,
    before: Option<i64>,
) -> Vec<Why> {
    position
        .iter()
        .enumerate()
        .map(|(i, at)| match at {
            _ if !f.is_content(i) => Why::NotPlaced,
            Ok(at) if after.is_none_or(|a| *at >= a) && before.is_none_or(|b| *at <= b) => Why::In,
            Ok(_) => Why::Out,
            Err(w) => *w,
        })
        .collect()
}

/// Why an end names something other than a node of this graph (graph-addressing ruling), with no lookup: another
/// graph's address (the selector is compared with this graph's id, and through `here` with its alias or a former
/// alias of it), the bare graph address, a fragment or a repo hint.
fn foreign(l: &Loaded, here: &dyn Fn(&str) -> bool, text: &str) -> Option<&'static str> {
    if text.contains('#') || text.contains("?repo=") {
        return Some("an end is a node, never a fragment or a repo hint");
    }
    match split_address(text) {
        (Some(_), "") => Some("the graph itself is never on a spine"),
        (Some(sel), _) if !here(sel) && sel.parse::<Id>().ok() != Some(l.meta.id) => {
            Some("--along reads one graph, and that address names another")
        }
        _ => None,
    }
}

fn along_end_invalid(graph: &ValidName, spine: &str, end: &str, why: &'static str) -> Problem {
    problems::along_end_invalid(graph, spine_slot(spine), end_slot(end), why)
}

fn reversed() -> Problem {
    problems::usage_invalid("--after is later than --before along this spine", "time")
}

/// A built-in: one date per node, ends are dates, compared by instant.
fn builtin(
    l: &Loaded,
    graph: &ValidName,
    req: &AlongRequest,
    field: Option<&str>,
    spines: usize,
) -> Result<Kept, Problem> {
    let g = &l.graph;
    let n = g.node_bound();
    let end = |text: &str| -> Result<DateSpan, Problem> {
        date_span(text).ok_or_else(|| {
            along_end_invalid(graph, &req.spine, text, "a built-in spine's end is a date")
        })
    };
    let after = req.after.as_deref().map(end).transpose()?;
    let before = req.before.as_deref().map(end).transpose()?;
    if let (Some(a), Some(b)) = (after, before)
        && a.relation(&b) == core::cmp::Ordering::Greater
    {
        return Err(reversed());
    }
    let mut span: Vec<Option<DateSpan>> = vec![None; n];
    match field {
        Some(f) => g.each_date(f, false, |ix, d, _| span[ix.index()] = Some(d)),
        None if req.spine == RECORDED => {
            for node in g.nodes() {
                span[node.ix().index()] = recorded_span(node);
            }
        }
        None => {
            let f = Facts::read_for(g, false, true);
            for (i, s) in span.iter_mut().enumerate() {
                *s = f.last_move(i).map(|at| DateSpan {
                    start: at,
                    end: at + 1,
                });
            }
        }
    }
    let mut why = vec![Why::NotPlaced; n];
    let mut coarse = vec![false; n];
    for node in g.nodes() {
        let i = node.ix().index();
        if g.is_provenance(node.ix()) {
            continue;
        }
        if let Some(s) = span[i] {
            coarse[i] = s.precision() >= Precision::Month;
            let ok = after.is_none_or(|a| s.relation(&a) != core::cmp::Ordering::Less)
                && before.is_none_or(|b| s.relation(&b) != core::cmp::Ordering::Greater);
            why[i] = if ok { Why::In } else { Why::Out };
        }
    }
    let stored = |s: Option<&str>| s.and_then(normalise_date);
    let propose = Command::new("graph spine propose")
        .name(graph)
        .word("--unplaced");
    let unplaced_next = match field {
        Some(f) => ValidName::new(f).map(|f| propose.qualified(DATE_FIELD_PREFIX, &f)),
        None => ValidName::new(&req.spine).map(|s| propose.name(&s)),
    };
    Ok(Kept {
        why,
        coarse,
        info: AlongInfo {
            spine: req.spine.clone(),
            source: AlongSource::Builtin,
            clock: Clock::Calendar,
            after: stored(req.after.as_deref()),
            before: stored(req.before.as_deref()),
            not_placed: 0,
            by_rule: 0,
            coarse: 0,
            spines,
        },
        unplaced_next,
        by_rule_next: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeSet;
    use serde_json::{Value, json};
    use umx_core::spine::Anchors;
    use umx_core::{Edge, Node};

    /// A small deterministic generator (no rand dependency).
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    fn node(id: u128, kind: &str) -> Node {
        Node {
            id: Id(id),
            kind: kind.into(),
            title: alloc::format!("{kind} {id}"),
            status: "open".into(),
            note: String::new(),
            evidence: vec![],
            status_history: vec![],
            created: "2026-11-12T09:00:00Z".into(),
            updated: "2026-11-12T09:00:00Z".into(),
            created_by: "t".into(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields: Default::default(),
        }
    }

    fn edge(id: u128, from: u128, kind: &str, to: u128) -> Edge {
        Edge {
            id: Id(id),
            kind: kind.into(),
            from_graph: None,
            from: Id(from),
            to_graph: None,
            to: Id(to),
            created: "2026-11-12T09:00:00Z".into(),
            created_by: "t".into(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: Default::default(),
        }
    }

    fn seq(a: u64, b: u64) -> Value {
        json!({ "source": "./t/day.ndjson", "selector": { "seq": [a, b] }, "kind": "room", "fidelity": "x", "hash": null })
    }

    fn day(h: u64) -> String {
        alloc::format!("2026-11-{:02}T09:00:00Z", 10 + h)
    }

    /// A generated meeting: segments (some undated, some sharing a date) `part_of` it with steps and evidence
    /// ranges; points raised in zero to two segments, some dated themselves (before, among and after the
    /// segments), some with evidence inside a segment's, some linked only to another point, some on nothing.
    fn generated(seed: u64) -> Graph {
        let mut r = Lcg(seed);
        let mut nodes = vec![node(1, "meeting")];
        let mut edges = Vec::new();
        let mut e = 10_000u128;
        let segs = 2 + r.below(7) as u128;
        // One graph in four dates no anchor (reviewer B3): a calendar declaration's anchors then order by step only.
        let dated = r.below(4) > 0;
        for s in 0..segs {
            let mut n = node(10 + s, "segment");
            if dated && r.below(5) > 0 {
                n.fields.insert("starts_on".into(), json!(day(r.below(6))));
            }
            n.fields.insert("step".into(), json!(s as u64 + 1));
            n.evidence = vec![seq(s as u64 * 10, s as u64 * 10 + 9)];
            nodes.push(n);
            edges.push(edge(e, 10 + s, "part_of", 1));
            e += 1;
        }
        let points = 20 + r.below(40) as u128;
        for p in 0..points {
            let id = 100 + p;
            let mut n = node(id, "point");
            match r.below(6) {
                0 => {
                    n.fields.insert("starts_on".into(), json!(day(r.below(8))));
                }
                1 => {
                    let at = r.below(segs as u64 * 10 + 5);
                    n.evidence = vec![seq(at, at)];
                }
                2 if p > 0 => {
                    edges.push(edge(e, id, "relates_to", 100 + r.below(p as u64) as u128));
                    e += 1;
                }
                _ => {}
            }
            for _ in 0..r.below(3) {
                edges.push(edge(e, id, "raised_in", 10 + r.below(segs as u64) as u128));
                e += 1;
            }
            nodes.push(n);
        }
        Graph::from_records(nodes, edges)
    }

    fn decl(order: &[&str], place: &[&str]) -> SpineDecl {
        SpineDecl {
            name: "day".into(),
            title: "The day".into(),
            main: false,
            anchors: Anchors {
                kind: "segment".into(),
                inside: Some("part_of".into()),
            },
            order: order.iter().map(|s| s.to_string()).collect(),
            place: place.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn decls() -> [SpineDecl; 3] {
        [
            decl(&["starts_on", "step"], &["raised_in", "evidence"]),
            decl(&["starts_on"], &["raised_in"]),
            decl(&["step"], &["raised_in", "evidence"]),
        ]
    }

    #[test]
    fn the_skipped_passes_place_every_node_as_the_full_read_does() {
        // graph-storage: Facts read for one declaration (no evidence pass unless `place` names it, never the
        // status moves) places every node in the same band as the full read.
        for seed in 0..60 {
            let g = generated(seed);
            for d in decls() {
                let (full, _) = place(&Facts::read(&g), &d);
                let (skip, _) = place(&Facts::read_for_decl(&g, &d), &d);
                assert_eq!(
                    (&full.how, &full.band, full.cover),
                    (&skip.how, &skip.band, skip.cover),
                    "seed {seed}, {:?}",
                    d.order
                );
            }
        }
    }

    /// The range by hand, from `place()`'s output: band by band, the anchors of that rank and the nodes placed
    /// in that band by data; the nodes placed by their own date before every anchor only when there is no `after`.
    fn by_hand(
        f: &Facts<'_>,
        p: &Placement,
        v: &AnchorView,
        after: Option<usize>,
        before: Option<usize>,
    ) -> BTreeSet<usize> {
        let last = v.anchors.len().saturating_sub(1);
        let (a, b) = (after.unwrap_or(0), before.unwrap_or(last));
        let mut out = BTreeSet::new();
        for (r, &anchor) in v.anchors.iter().enumerate() {
            if (a..=b).contains(&r) && p.how[anchor] != How::None {
                out.insert(anchor);
            }
        }
        for i in 0..p.how.len() {
            if !f.is_content(i) || v.is_anchor[i] || matches!(p.how[i], How::None | How::Rule) {
                continue;
            }
            if p.band[i] != NO_BAND {
                if (a as u64..=b as u64).contains(&p.band[i]) {
                    out.insert(i);
                }
            } else if after.is_none() {
                out.insert(i);
            }
        }
        out
    }

    #[test]
    fn the_range_equals_place_filtered_by_hand_for_every_pair_of_anchors() {
        for seed in 0..40 {
            let g = generated(seed);
            for d in decls() {
                let f = Facts::read_for_decl(&g, &d);
                let (p, v) = place(&f, &d);
                let (pos, _) = positions(&p, &v);
                let ends: Vec<Option<usize>> = core::iter::once(None)
                    .chain((0..v.anchors.len()).map(Some))
                    .collect();
                for &a in &ends {
                    for &b in &ends {
                        if let (Some(a), Some(b)) = (a, b)
                            && a > b
                        {
                            continue;
                        }
                        let got: BTreeSet<usize> =
                            range(&f, &pos, a.map(|x| x as i64), b.map(|x| x as i64))
                                .iter()
                                .enumerate()
                                .filter(|(_, w)| **w == Why::In)
                                .map(|(i, _)| i)
                                .collect();
                        assert_eq!(
                            got,
                            by_hand(&f, &p, &v, a, b),
                            "seed {seed}, {:?}, {a:?}..{b:?}",
                            d.order
                        );
                    }
                }
                // Every content node is in, not placed or by rule exactly as place() says (reviewer B3: no placement
                // without a position, whether or not any anchor is dated).
                let whole = range(&f, &pos, None, None);
                for (i, (how, why)) in p.how.iter().zip(&whole).enumerate() {
                    let want = match how {
                        How::None => Why::NotPlaced,
                        How::Rule => Why::ByRule,
                        _ => Why::In,
                    };
                    if f.is_content(i) {
                        assert_eq!(*why, want, "seed {seed}, {:?}, node {i}", d.order);
                    }
                }
            }
        }
    }

    #[test]
    fn a_node_dated_on_a_calendar_spine_sits_with_the_last_anchor_at_or_before_it() {
        // Segments at days 1 and 3; points dated day 0, day 1, day 2 and day 4.
        let mut nodes = vec![node(1, "meeting")];
        let mut edges = Vec::new();
        for (s, d) in [(10u128, 1u64), (11, 3)] {
            let mut n = node(s, "segment");
            n.fields.insert("starts_on".into(), json!(day(d)));
            nodes.push(n);
            edges.push(edge(1000 + s, s, "part_of", 1));
        }
        for (p, d) in [(100u128, 0u64), (101, 1), (102, 2), (103, 4)] {
            let mut n = node(p, "point");
            n.fields.insert("starts_on".into(), json!(day(d)));
            nodes.push(n);
        }
        let g = Graph::from_records(nodes, edges);
        let d = decl(&["starts_on"], &["raised_in"]);
        let f = Facts::read_for_decl(&g, &d);
        let (p, v) = place(&f, &d);
        let ix = |id: u128| g.node_ix(Id(id)).unwrap().index();
        let bands: Vec<u64> = [100, 101, 102, 103].map(|id| p.band[ix(id)]).to_vec();
        assert_eq!(bands, vec![NO_BAND, 0, 0, 1]);
        let (pos, _) = positions(&p, &v);
        assert_eq!(pos[ix(100)], Ok(-1), "before every anchor");
        let kept = |a, b| {
            let why = range(&f, &pos, a, b);
            g.nodes()
                .filter(|n| why[n.ix().index()] == Why::In)
                .map(|n| n.id().0)
                .collect::<Vec<u128>>()
        };
        // Inclusive at both ends: the anchor's own band is in.
        assert_eq!(kept(Some(1), Some(1)), vec![11, 103]);
        assert_eq!(kept(None, Some(0)), vec![10, 100, 101, 102]);
        assert_eq!(kept(Some(0), None), vec![10, 11, 101, 102, 103]);
    }

    #[test]
    fn recorded_reads_the_created_second_as_date_span_reads_its_text() {
        // graph-storage: the recorded built-in reads the created column's seconds, with no text built; a time umx
        // did not write is read from its text. Both give what date_span gives for the text.
        let created = [
            "2026-11-12T09:00:00Z",
            "1970-01-01T00:00:00Z",
            "2026-11-12T09:00:00.5Z",
            "1969-07-20T20:17:40Z",
            "2026-11-12T09:00:00+11:00",
            "2026-11-12",
            "not a time",
        ];
        let nodes = created
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut n = node(100 + i as u128, "point");
                n.created = (*c).into();
                n
            })
            .collect();
        let g = Graph::from_records(nodes, vec![]);
        let mut read_as_seconds = 0;
        for n in g.nodes() {
            read_as_seconds += usize::from(n.created_secs().is_some());
            assert_eq!(recorded_span(n), date_span(&n.created()), "{}", n.created());
        }
        assert_eq!(read_as_seconds, 2);
    }

    #[test]
    fn with_no_dated_anchor_a_point_s_own_date_places_nothing() {
        // Reviewer B3: anchors ordered by starts_on then step but holding only step, and with no anchors at all. A
        // point with its own starts_on is not placed by it: unlinked it is not placed (so show --unplaced lists it
        // and --along counts it), linked it sits with its anchor by link.
        let mut nodes = vec![node(1, "meeting")];
        let mut edges = Vec::new();
        for s in [10u128, 11] {
            let mut n = node(s, "segment");
            n.fields.insert("step".into(), json!(s as u64 - 9));
            nodes.push(n);
            edges.push(edge(1000 + s, s, "part_of", 1));
        }
        for p in [100u128, 101] {
            let mut n = node(p, "point");
            n.fields.insert("starts_on".into(), json!(day(1)));
            nodes.push(n);
        }
        edges.push(edge(2000, 101, "raised_in", 11));
        let g = Graph::from_records(nodes, edges);
        let ix = |id: u128| g.node_ix(Id(id)).unwrap().index();
        let mut d = decl(&["starts_on", "step"], &["raised_in"]);
        let f = Facts::read_for_decl(&g, &d);
        let (p, v) = place(&f, &d);
        let (pos, _) = positions(&p, &v);
        assert_eq!(
            (p.how[ix(100)], pos[ix(100)]),
            (How::None, Err(Why::NotPlaced))
        );
        assert_eq!((p.how[ix(101)], pos[ix(101)]), (How::Link, Ok(1)));
        assert_eq!(p.cover.not_placed, 2, "the point and the meeting");
        d.anchors.kind = "chapter".into();
        let (p, v) = place(&f, &d);
        let (pos, _) = positions(&p, &v);
        for id in [100, 101] {
            assert_eq!(
                (p.how[ix(id)], pos[ix(id)]),
                (How::None, Err(Why::NotPlaced))
            );
        }
    }

    #[test]
    fn a_rule_placement_is_the_lower_median_of_each_placed_neighbour_once() {
        // place()'s rule phase against the rule stated plainly: a set of neighbours per node.
        for seed in 0..40 {
            let g = generated(seed);
            for d in decls() {
                let f = Facts::read_for_decl(&g, &d);
                let (p, v) = place(&f, &d);
                let (_, rank) = positions(&p, &v);
                let n = p.how.len();
                let data = |j: usize| -> Option<u64> {
                    match p.how[j] {
                        How::None | How::Rule => None,
                        _ if v.is_anchor[j] => Some(u64::from(rank[j])),
                        _ => (p.band[j] != NO_BAND).then_some(p.band[j]),
                    }
                };
                let mut near: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
                let mut parent = vec![false; n];
                for (a, kind, b) in f.edges() {
                    let (a, b) = (a.index(), b.index());
                    if v.is_anchor[a] && kind == "part_of" && !v.is_anchor[b] {
                        parent[b] = true;
                    }
                    if a != b {
                        near[a].insert(b);
                        near[b].insert(a);
                    }
                }
                for i in 0..n {
                    if !f.is_content(i) || v.is_anchor[i] || parent[i] || data(i).is_some() {
                        continue;
                    }
                    if !matches!(p.how[i], How::Rule | How::None) {
                        continue;
                    }
                    let mut bands: Vec<u64> = near[i].iter().filter_map(|&j| data(j)).collect();
                    bands.sort_unstable();
                    let want = bands.get(bands.len().saturating_sub(1) / 2).copied();
                    let got = (p.how[i] == How::Rule).then_some(p.band[i]);
                    assert_eq!(got, want, "seed {seed}, node {i}, {:?}", d.order);
                }
            }
        }
    }

    #[test]
    fn an_edge_is_in_only_when_both_ends_are() {
        use Why::*;
        assert_eq!(edge_why(In, In), In);
        assert_eq!(edge_why(In, Out), Out);
        assert_eq!(edge_why(Out, ByRule), ByRule);
        assert_eq!(edge_why(ByRule, NotPlaced), NotPlaced);
        assert_eq!(edge_why(NotPlaced, In), NotPlaced);
    }
}
