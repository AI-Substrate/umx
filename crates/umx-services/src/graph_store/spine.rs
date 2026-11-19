//! graph spine propose (time review B5, ws005 s07): the spines a graph could have, with cover and spread, and what
//! is in the way. Stores nothing. [`place`] is the one placement rule (brief 5.3: own field, then a link, then
//! evidence position, else not placed; a node in several bands sits in the first): the proposer runs it on every
//! anchor-type candidate it builds, and the declared spine (TM5) runs it on what the builder confirmed.
//!
//! One graph, local edges only (graph-addressing ruling): an edge with an end in another graph never places a node
//! and is never resolved. Provenance nodes are never counted. Cost: one scanner pass over the node lines for
//! evidence and status moves ([`Facts::read`]), then passes over the type column, the edge columns and the field
//! columns; never a whole node per row.

use super::{Ctx, Done, GraphStore, Outcome, alias_name, problems};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::sync::atomic::Ordering as AtomicOrdering;
use umx_core::ontology::{FieldKind, Ontology, PART_OF};
use umx_core::problem::Candidate as Named;
use umx_core::spine::{
    Anchors, Clock, Cover, INSIDE, PLACE_EVIDENCE, Position, SpineDecl, Spread, latest_move_start,
    overlap, positions,
};
use umx_core::time::{DateSpan, date_span};
use umx_core::{Command, Id, ValidName, Value};
use umx_graph::columns::FieldRef;
use umx_graph::{Graph, NodeIx, NodeRef};

/// An anchor type has at least 2 and at most `max(MAX_ANCHORS, total / ANCHOR_SHARE)` nodes (the spine design's rule).
pub(crate) const MAX_ANCHORS: usize = 60;
const ANCHOR_SHARE: usize = 20;
/// A relation places onto an anchor type when it reaches at least `max(MIN_PLACED, total / PLACED_SHARE)` distinct
/// other nodes.
pub(crate) const MIN_PLACED: usize = 5;
const PLACED_SHARE: usize = 20;
/// A spine whose largest band holds this share of its placed nodes or more has no spread, and is not offered.
pub(crate) const NO_SPREAD_SHARE: f64 = 0.95;
/// Steps held as field values: 2 to this many distinct values.
pub(crate) const MAX_STEP_VALUES: usize = 60;
/// At most this many example ids per finding.
const EXAMPLES: usize = 5;
/// The std order number inside a parent (R5) and umx's similarity relation: one source, umx-core's (ws008 T1).
use umx_core::ontology::SIMILAR_TO;
use umx_core::standard::STEP;
const DAY: i64 = 86_400;
/// Candidate names: an anchor type is `type:<t>`, a date field built-in `date_field:<f>`.
const ANCHOR_PREFIX: &str = "type:";
const DATE_FIELD_PREFIX: &str = "date_field:";

/// How one node is placed on one spine, strongest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum How {
    Field,
    Link,
    Evidence,
    /// By the service's rule ([`RULE_MIN_NEIGHBOURS`]): never by data, so drawn hollow.
    Rule,
    None,
}

/// By-rule placement's named settings (ws008 TM5; brief 5.3, the time review's proof of concept), fixed here so the
/// CLI and the UI give one answer; no expression is read from a graph. A node no field, link or evidence places sits
/// in the middle band (the lower median) of its neighbours one link away, either direction, that are placed by data
/// and sit in a band, when it has at least this many. A rule placement never feeds another; an anchor, and an
/// anchor's own parent, is never placed by rule.
pub(crate) const RULE_MIN_NEIGHBOURS: usize = 1;

/// One spine's placement of every node: how, and in which band (`NO_BAND` when the node sits in none).
pub(crate) struct Placement {
    pub how: Vec<How>,
    pub band: Vec<u64>,
    pub cover: Cover,
}

pub(crate) const NO_BAND: u64 = u64::MAX;

impl Placement {
    fn new(n: usize) -> Placement {
        Placement {
            how: vec![How::None; n],
            band: vec![NO_BAND; n],
            cover: Cover::default(),
        }
    }

    /// Fills `cover` from `how` over the content nodes.
    fn count(&mut self, f: &Facts<'_>) {
        let mut c = Cover::default();
        for (i, how) in self.how.iter().enumerate() {
            if !f.content[i] {
                continue;
            }
            c.total += 1;
            match how {
                How::Field => c.by_field += 1,
                How::Link => c.by_link += 1,
                How::Evidence => c.by_evidence += 1,
                How::Rule => c.by_rule += 1,
                How::None => c.not_placed += 1,
            }
        }
        self.cover = c;
    }

    /// The spread over the bands nodes sit in.
    /// Placements by data only (project manager review): a rule placement is hollow, so it never makes a spine look
    /// spread.
    fn spread(&self, distinct_values: usize) -> Spread {
        let mut sizes: BTreeMap<u64, usize> = BTreeMap::new();
        let data = self
            .band
            .iter()
            .zip(&self.how)
            .filter(|(_, h)| **h != How::Rule);
        for &b in data.map(|(b, _)| b).filter(|&&b| b != NO_BAND) {
            *sizes.entry(b).or_insert(0) += 1;
        }
        spread_of(sizes.into_values(), distinct_values)
    }
}

/// The spread of bands holding `sizes` nodes each (empty bands left out); the largest band's share to 3 places.
pub(crate) fn spread_of(sizes: impl IntoIterator<Item = usize>, distinct_values: usize) -> Spread {
    let (mut bands, mut largest, mut sum) = (0, 0, 0);
    for s in sizes.into_iter().filter(|&s| s > 0) {
        bands += 1;
        largest = largest.max(s);
        sum += s;
    }
    let largest_band_share = if sum == 0 {
        0.0
    } else {
        ((largest as f64 * 1000.0 / sum as f64) + 0.5) as u64 as f64 / 1000.0
    };
    Spread {
        bands,
        largest_band_share,
        distinct_values,
    }
}

/// What one read of the graph gives every spine: which rows count, each node's evidence positions and the time of
/// its latest status move, and the relations in use.
pub(crate) struct Facts<'g> {
    pub g: &'g Graph,
    content: Vec<bool>,
    pub total: usize,
    positions: Vec<Position>,
    /// Node `i`'s positions are `positions[pos_at[i]..pos_at[i + 1]]`.
    pos_at: Vec<u32>,
    last_move: Vec<Option<i64>>,
    relations: BTreeSet<&'g str>,
}

impl<'g> Facts<'g> {
    /// One pass over the node lines, reading only `evidence` and `status_history`.
    pub fn read(g: &'g Graph) -> Facts<'g> {
        Facts::read_for(g, true, true)
    }

    /// What [`place`] needs for one declaration (D71 step 12, graph-storage): the evidence positions only when its
    /// `place` names `evidence`, and never the status moves, which only the `status` built-in reads.
    pub fn read_for_decl(g: &'g Graph, decl: &SpineDecl) -> Facts<'g> {
        Facts::read_for(g, decl.place.iter().any(|p| p == PLACE_EVIDENCE), false)
    }

    /// [`Facts::read`] reading the node lines only for what is asked: no cold read at all when neither is.
    pub fn read_for(g: &'g Graph, evidence: bool, moves: bool) -> Facts<'g> {
        let n = g.node_bound();
        let mut f = Facts {
            g,
            content: vec![false; n],
            total: 0,
            positions: Vec::new(),
            pos_at: Vec::with_capacity(n + 1),
            last_move: vec![None; n],
            relations: g.relation_kinds().into_iter().map(|k| k.as_str()).collect(),
        };
        f.pos_at.push(0);
        for node in g.nodes() {
            let i = node.ix().index();
            if !g.is_provenance(node.ix()) {
                f.content[i] = true;
                f.total += 1;
                if evidence || moves {
                    let cold = node.evidence_and_moves();
                    if evidence {
                        positions(cold.evidence_json(), &mut f.positions);
                    }
                    if moves {
                        f.last_move[i] = latest_move_start(cold.moves_json());
                    }
                }
            }
            f.pos_at.push(f.positions.len() as u32);
        }
        f
    }

    /// Whether node `i` counts on a spine (provenance never does).
    pub(crate) fn is_content(&self, i: usize) -> bool {
        self.content[i]
    }

    /// When node `i`'s latest status move was made, if [`Facts::read_for`] read the moves.
    pub(crate) fn last_move(&self, i: usize) -> Option<i64> {
        self.last_move[i]
    }

    pub(crate) fn positions(&self, i: usize) -> &[Position] {
        &self.positions[self.pos_at[i] as usize..self.pos_at[i + 1] as usize]
    }

    fn is_relation(&self, name: &str) -> bool {
        self.relations.contains(name)
    }

    /// Every local edge between two content nodes: (from, relation, to).
    pub(crate) fn edges(&self) -> impl Iterator<Item = (NodeIx, &'g str, NodeIx)> + '_ {
        self.g.edges().filter_map(|e| {
            let (from, to) = (e.from_ix()?, e.to_ix()?);
            (self.content[from.index()] && self.content[to.index()] && e.kind() != SIMILAR_TO)
                .then(|| (from, e.kind(), to))
        })
    }
}

/// An anchor's order key: which `order` field it holds first, and its value there (a date's start, a number).
type Key = (usize, i64);

/// A number's value, else the start of the date the typed time column holds for the node (`date`), so a date the
/// codec decoded (a line the scanner leaves to it) orders as one the scanner read.
fn key_of(v: &FieldRef<'_>, date: impl FnOnce() -> Option<i64>) -> Option<i64> {
    match v {
        FieldRef::UInt(u) => i64::try_from(*u).ok(),
        FieldRef::Int(i) => Some(*i),
        FieldRef::Float(x) if x.is_finite() => Some(*x as i64),
        FieldRef::Str(_) | FieldRef::Other(_) => date(),
        _ => None,
    }
}

/// What [`place`] learnt about the anchors of a declaration, for the proposer's spread and findings.
pub(crate) struct AnchorView {
    /// Every anchor's handle index, first in order first.
    pub anchors: Vec<usize>,
    pub is_anchor: Vec<bool>,
    pub key: Vec<Option<Key>>,
    /// Anchors on a chain edge.
    pub chained: Vec<bool>,
}

/// The one placement rule, for one declared or proposed spine. Brief 5.3, strongest first: an anchor with a value
/// in an `order` field, and for a calendar spine (`order[0]` a date field, never `step`) with some anchor dated
/// there, any other node with a date there, is placed by its field; a node with a `place` relation edge to an
/// anchor, or an anchor with no order value on a chain edge, by link; a node whose evidence position overlaps an
/// anchor's (when `place` holds `evidence`), by evidence; else not placed. A node linked to several anchors sits in
/// the first: lowest order key, then the declared chain (worked out among the anchors under one `part_of` parent),
/// then lowest id.
pub(crate) fn place(f: &Facts<'_>, decl: &SpineDecl) -> (Placement, AnchorView) {
    let g = f.g;
    let n = g.node_bound();
    let mut v = AnchorView {
        anchors: Vec::new(),
        is_anchor: vec![false; n],
        key: vec![None; n],
        chained: vec![false; n],
    };
    for node in g.nodes() {
        let i = node.ix().index();
        if f.content[i] && node.kind() == decl.anchors.kind {
            v.is_anchor[i] = true;
            v.anchors.push(i);
        }
    }
    let mut p = Placement::new(n);
    let fields: Vec<&str> = decl
        .order
        .iter()
        .map(String::as_str)
        .filter(|o| !f.is_relation(o))
        .collect();
    let chains: BTreeSet<&str> = decl
        .order
        .iter()
        .map(String::as_str)
        .filter(|o| f.is_relation(o))
        .collect();
    // A step counts only inside its parent, so only a date orders nodes that are not anchors (review B1).
    let calendar = decl
        .order
        .first()
        .is_some_and(|o| o != STEP && fields.first() == Some(&o.as_str()));
    // The start of the date a non-anchor holds in `order[0]` on a calendar spine: it places the node by its field,
    // but only beside anchors dated there; with none (anchors ordered by step only, or no anchors) a date has nowhere
    // to sit, so it places nothing and the node is placed, or not, as any other (reviewer B3: show, `--unplaced` and
    // `--along` read the one answer).
    let mut own: Vec<Option<i64>> = Vec::new();
    for (at, name) in fields.iter().enumerate() {
        let Some(fid) = g.field_id(name) else {
            continue;
        };
        let date = |ix| g.node_at(ix).date_at(fid).map(|(d, _)| d.start);
        g.each_field(name, false, |ix, value| {
            let i = ix.index();
            if v.is_anchor[i] {
                if let Some(k) = key_of(&value, || date(ix))
                    && v.key[i].is_none()
                {
                    v.key[i] = Some((at, k));
                }
            } else if at == 0
                && calendar
                && let Some(start) = date(ix)
            {
                if own.is_empty() {
                    own = vec![None; n];
                }
                own[i] = Some(start);
            }
        });
    }
    if v.anchors.iter().any(|&a| matches!(v.key[a], Some((0, _)))) {
        for (i, _) in own.iter().enumerate().filter(|(_, s)| s.is_some()) {
            p.how[i] = How::Field;
        }
    } else {
        own.clear();
    }
    let places: BTreeSet<&str> = decl.place.iter().map(String::as_str).collect();
    let mut parent: Vec<Option<usize>> = vec![None; n];
    // Every node an anchor sits inside (R366-2), not only the one `parent` keeps for ordering: none is placed by rule.
    let mut is_parent = vec![false; n];
    let mut chain: Vec<(usize, usize)> = Vec::new();
    for (from, kind, to) in f.edges() {
        let (a, b) = (from.index(), to.index());
        if !v.is_anchor[a] {
            continue;
        }
        if decl.anchors.inside.as_deref() == Some(kind) && !v.is_anchor[b] {
            parent[a] = Some(parent[a].map_or(b, |p| p.min(b)));
            is_parent[b] = true;
        } else if chains.contains(kind) && v.is_anchor[b] && a != b {
            v.chained[a] = true;
            v.chained[b] = true;
            chain.push((a, b));
        }
    }
    let depth = chain_depth(n, &chain, &parent);
    v.anchors
        .sort_by_key(|&i| (v.key[i].is_none(), v.key[i], depth[i], i));
    let mut rank = vec![u32::MAX; n];
    for (r, &i) in v.anchors.iter().enumerate() {
        rank[i] = r as u32;
    }
    calendar_bands(&v, &own, &mut p);
    for &i in &v.anchors {
        p.how[i] = if v.key[i].is_some() {
            How::Field
        } else if v.chained[i] {
            How::Link
        } else {
            How::None
        };
    }
    let mut first = vec![u32::MAX; n];
    for (from, kind, to) in f.edges() {
        let (a, b) = (from.index(), to.index());
        if places.contains(kind) && v.is_anchor[b] && a != b {
            first[a] = first[a].min(rank[b]);
        }
    }
    for (i, &r) in first.iter().enumerate().filter(|(_, r)| **r != u32::MAX) {
        if p.how[i] == How::None {
            p.how[i] = How::Link;
        }
        if !v.is_anchor[i] && p.how[i] != How::Field {
            p.band[i] = u64::from(r);
        }
    }
    if places.contains(PLACE_EVIDENCE) {
        for i in 0..n {
            if p.how[i] != How::None || !f.content[i] || f.positions(i).is_empty() {
                continue;
            }
            if let Some(r) = v
                .anchors
                .iter()
                .position(|&a| meets(f.positions(i), f.positions(a)))
            {
                p.how[i] = How::Evidence;
                if !v.is_anchor[i] {
                    p.band[i] = r as u64;
                }
            }
        }
    }
    by_rule(f, &v, &is_parent, &rank, &mut p);
    p.count(f);
    (p, v)
}

/// A node a calendar spine places by its own date (D71 step 12, pm-001: in the one placement rule, so `show` and
/// `--along` agree) sits in the band of the last anchor whose `order[0]` date starts at or before the node's. The
/// anchors dated there come first in `v.anchors`, in date order, so this is a binary search. A node dated before
/// every such anchor keeps no band and sits before them all. `own` is empty when no anchor is dated there ([`place`]
/// places no node by its own date then).
fn calendar_bands(v: &AnchorView, own: &[Option<i64>], p: &mut Placement) {
    if own.is_empty() {
        return;
    }
    let dated = v
        .anchors
        .iter()
        .take_while(|&&a| matches!(v.key[a], Some((0, _))))
        .count();
    let starts: Vec<i64> = v.anchors[..dated]
        .iter()
        .filter_map(|&a| v.key[a].map(|(_, k)| k))
        .collect();
    for (i, start) in own.iter().enumerate() {
        if let Some(s) = start {
            let after = starts.partition_point(|&k| k <= *s);
            if after > 0 {
                p.band[i] = (after - 1) as u64;
            }
        }
    }
}

/// The rule phase of [`place`] (see [`RULE_MIN_NEIGHBOURS`]). Reads the placements by data only, so a node placed
/// here never places another.
fn by_rule(f: &Facts<'_>, v: &AnchorView, is_parent: &[bool], rank: &[u32], p: &mut Placement) {
    let n = p.how.len();
    // One unit for both: an anchor's band is its rank along the spine, and a non-anchor's `p.band` is the rank of
    // the anchor it was placed in, so the two compare.
    let band_of = |j: usize| -> Option<u64> {
        if v.is_anchor[j] {
            (rank[j] != u32::MAX && p.how[j] != How::None).then(|| u64::from(rank[j]))
        } else {
            (p.how[j] != How::None && p.band[j] != NO_BAND).then_some(p.band[j])
        }
    };
    // The placements by data, read once before any rule placement is made, so none feeds another.
    let data_band: Vec<Option<u64>> = (0..n).map(band_of).collect();
    // Who may be placed by rule: a content node nothing placed by data, never an anchor or an anchor's parent.
    let open = |i: usize| p.how[i] == How::None && f.content[i] && !v.is_anchor[i] && !is_parent[i];
    // Each (node, placed neighbour) pair once (R366-1), however many links (relations, directions) join the two:
    // only the pairs a rule placement reads, never a neighbour set per node (graph-storage, D71 step 12: a set per
    // node was most of a read's bytes at 100k).
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    for (from, _, to) in f.edges() {
        let (a, b) = (from.index(), to.index());
        if a == b {
            continue;
        }
        if open(a) && data_band[b].is_some() {
            pairs.push((a as u32, b as u32));
        }
        if open(b) && data_band[a].is_some() {
            pairs.push((b as u32, a as u32));
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    let mut bands: Vec<u64> = Vec::new();
    for run in pairs.chunk_by(|x, y| x.0 == y.0) {
        bands.clear();
        bands.extend(run.iter().filter_map(|&(_, j)| data_band[j as usize]));
        if bands.len() < RULE_MIN_NEIGHBOURS {
            continue;
        }
        bands.sort_unstable();
        let i = run[0].0 as usize;
        p.how[i] = How::Rule;
        p.band[i] = bands[(bands.len() - 1) / 2];
    }
}

fn meets(a: &[Position], b: &[Position]) -> bool {
    a.iter().any(|x| b.iter().any(|y| overlap(x, y)))
}

/// Each anchor's place along the declared chain inside its parent (review B2): the longest run of chain edges
/// that leads to it, 0 at a chain's start. An edge between anchors of different parents is not followed; an anchor on
/// no chain, or on a loop, gets `u32::MAX` and falls back to its id.
fn chain_depth(n: usize, chain: &[(usize, usize)], parent: &[Option<usize>]) -> Vec<u32> {
    let edges: Vec<(usize, usize)> = chain
        .iter()
        .copied()
        .filter(|&(a, b)| parent[a] == parent[b])
        .collect();
    let mut depth = vec![u32::MAX; n];
    let mut ins = vec![0u32; n];
    let mut outs: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &(a, b) in &edges {
        ins[b] += 1;
        outs.entry(a).or_default().push(b);
    }
    let mut ready: Vec<usize> = edges
        .iter()
        .map(|&(a, _)| a)
        .filter(|&a| ins[a] == 0)
        .collect();
    ready.sort_unstable();
    ready.dedup();
    for &a in &ready {
        depth[a] = 0;
    }
    while let Some(a) = ready.pop() {
        for &b in outs.get(&a).into_iter().flatten() {
            let d = depth[a] + 1;
            if depth[b] == u32::MAX || depth[b] < d {
                depth[b] = d;
            }
            ins[b] -= 1;
            if ins[b] == 0 {
                ready.push(b);
            }
        }
    }
    // A loop's members never reach in-degree 0; whatever depth they picked up on the way in is dropped.
    for (i, &left) in ins.iter().enumerate() {
        if left > 0 {
            depth[i] = u32::MAX;
        }
    }
    depth
}

/// Where a candidate came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpineSource {
    AnchorType,
    Builtin,
}

impl SpineSource {
    pub fn as_str(self) -> &'static str {
        match self {
            SpineSource::AnchorType => "anchor_type",
            SpineSource::Builtin => "builtin",
        }
    }
}

/// Why a candidate is not offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhyNot {
    NoCover,
    NoOrder,
    NoSpread,
}

impl WhyNot {
    pub fn as_str(self) -> &'static str {
        match self {
            WhyNot::NoCover => "no_cover",
            WhyNot::NoOrder => "no_order",
            WhyNot::NoSpread => "no_spread",
        }
    }
}

/// One spine the graph could have. `name` is unique in a proposal and is what `--unplaced` takes: `type:<t>` for an
/// anchor type (whose `decl.name` is `<t>`), else the built-in (`recorded`, `status`, `date_field:<f>`, `source`). A
/// type is a plain name, so `type:<t>` never meets a built-in (review B3).
#[derive(Clone, Debug, PartialEq)]
pub struct SpineCandidate {
    pub name: String,
    pub source: SpineSource,
    pub builtin: Option<String>,
    pub decl: Option<SpineDecl>,
    pub clock: Clock,
    pub cover: Cover,
    pub spread: Spread,
    pub offered: bool,
    pub why_not: Option<WhyNot>,
}

/// What is in the way of a good spine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum InTheWayKind {
    StepsAsFieldValues,
    OrderDisagrees,
    LinkEvidenceDisagree,
}

impl InTheWayKind {
    pub fn as_str(self) -> &'static str {
        match self {
            InTheWayKind::StepsAsFieldValues => "steps_as_field_values",
            InTheWayKind::OrderDisagrees => "order_disagrees",
            InTheWayKind::LinkEvidenceDisagree => "link_evidence_disagree",
        }
    }
}

/// One finding: `about` is a vocabulary name (the field or the anchor type), never graph text; `examples` are this
/// graph's full ids, lowest first.
#[derive(Clone, Debug, PartialEq)]
pub struct InTheWay {
    pub kind: InTheWayKind,
    pub about: String,
    pub count: usize,
    pub examples: Vec<Id>,
    pub next: Command,
}

/// A count and the command that shows what it counts (B4a).
#[derive(Clone, Debug, PartialEq)]
pub struct SpineHint {
    pub count: usize,
    pub next: Command,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpineHints {
    /// Candidates listed but not offered.
    pub not_offered: SpineHint,
    /// Nodes with no place on the best offered candidate, and that candidate; None when none is offered.
    pub not_placed: Option<(String, SpineHint)>,
    /// Nodes the best offered candidate places only by rule (B4a, project manager review); None when it places none so.
    pub by_rule: Option<(String, SpineHint)>,
}

/// `--unplaced <candidate>`: one page of the ids of the nodes that candidate does not place.
#[derive(Clone, Debug, PartialEq)]
pub struct Unplaced {
    pub candidate: String,
    pub total: usize,
    pub offset: usize,
    pub ids: Vec<Id>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpineProposal {
    pub alias: String,
    pub total: usize,
    pub candidates: Vec<SpineCandidate>,
    pub in_the_way: Vec<InTheWay>,
    pub hints: SpineHints,
    pub unplaced: Option<Unplaced>,
    /// `--by-rule <candidate>`: one page of the ids that candidate places only by rule (same shape as `unplaced`).
    pub by_rule: Option<Unplaced>,
}

/// The one line `umx graph guide` and `umx check` show: the best offered candidate and its cover, or none, with the
/// command that lists them all. A missing spine is a hint, never a warning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpineLine {
    pub best: Option<(String, Clock, Cover)>,
    pub next: Command,
    /// The command that lists the nodes the best places only by rule (B4a); None when it places none so.
    pub by_rule_next: Option<Command>,
}

fn propose_cmd(graph: &ValidName) -> Command {
    Command::new("graph spine propose").name(graph)
}

/// An anchor-type candidate found in the graph, with the counts that ranked it.
struct Found<'g> {
    kind: &'g str,
    place: Vec<(&'g str, usize)>,
    chains: Vec<(&'g str, usize)>,
    inside: bool,
}

/// Anchor types (the spine design's rule): 2 to `max(MAX_ANCHORS, total / ANCHOR_SHARE)` nodes, and a relation from at
/// least `max(MIN_PLACED, total / PLACED_SHARE)` distinct nodes of other types onto them. One pass over the edges.
fn anchor_types<'g>(f: &Facts<'g>) -> Vec<Found<'g>> {
    let g = f.g;
    let max = MAX_ANCHORS.max(f.total / ANCHOR_SHARE);
    let min = MIN_PLACED.max(f.total / PLACED_SHARE);
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for node in g.nodes().filter(|n| f.content[n.ix().index()]) {
        *count.entry(node.kind()).or_insert(0) += 1;
    }
    let types: Vec<&str> = count
        .iter()
        .filter(|(_, c)| (2..=max).contains(*c))
        .map(|(t, _)| *t)
        .collect();
    if types.is_empty() {
        return Vec::new();
    }
    let kind = |ix: NodeIx| g.node_at(ix).kind();
    let mut sources: BTreeMap<(&str, &str), Vec<u32>> = BTreeMap::new();
    let mut chains: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    let mut inside: BTreeSet<&str> = BTreeSet::new();
    for (from, rel, to) in f.edges() {
        let (ft, tt) = (kind(from), kind(to));
        if ft == tt {
            if rel != PART_OF && types.contains(&tt) {
                *chains.entry((tt, rel)).or_insert(0) += 1;
            }
        } else {
            if types.contains(&tt) {
                sources
                    .entry((tt, rel))
                    .or_default()
                    .push(from.index() as u32);
            }
            if rel == PART_OF && types.contains(&ft) {
                inside.insert(ft);
            }
        }
    }
    let mut found: BTreeMap<&str, Found<'_>> = BTreeMap::new();
    for ((t, rel), mut from) in sources {
        from.sort_unstable();
        from.dedup();
        if from.len() >= min {
            found
                .entry(t)
                .or_insert_with(|| Found {
                    kind: t,
                    place: Vec::new(),
                    chains: Vec::new(),
                    inside: inside.contains(t),
                })
                .place
                .push((rel, from.len()));
        }
    }
    for ((t, rel), c) in chains {
        if let Some(fd) = found.get_mut(t) {
            fd.chains.push((rel, c));
        }
    }
    let mut out: Vec<Found<'_>> = found.into_values().collect();
    for fd in &mut out {
        fd.place.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        fd.chains.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    }
    out
}

/// The declaration a found anchor type proposes: its declared date fields the anchors hold (most held first), then
/// `step` when they hold it as a number, then its chains; `inside` is `part_of` when an anchor has a parent.
fn decl_of(f: &Facts<'_>, o: &Ontology, fd: &Found<'_>) -> SpineDecl {
    let g = f.g;
    let mut dates: Vec<(&str, usize)> = o
        .type_decl(fd.kind)
        .map(|t| {
            t.fields
                .iter()
                .filter(|d| d.kind == FieldKind::Date)
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
        .into_iter()
        .map(|name| {
            let mut held = 0;
            let fid = g.field_id(name);
            g.each_field(name, false, |ix, v| {
                let node = g.node_at(ix);
                let date = || fid.and_then(|f| node.date_at(f)).map(|(d, _)| d.start);
                if node.kind() == fd.kind && key_of(&v, date).is_some() {
                    held += 1;
                }
            });
            (name, held)
        })
        .filter(|(_, held)| *held > 0)
        .collect();
    dates.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let mut order: Vec<String> = dates.into_iter().map(|(d, _)| d.to_string()).collect();
    let mut step = false;
    g.each_field(STEP, false, |ix, v| {
        step |=
            g.node_at(ix).kind() == fd.kind && matches!(v, FieldRef::UInt(_) | FieldRef::Int(_));
    });
    if step && !order.iter().any(|o| o == STEP) {
        order.push(STEP.into());
    }
    order.extend(fd.chains.iter().map(|(c, _)| c.to_string()));
    let mut place: Vec<String> = fd.place.iter().map(|(r, _)| r.to_string()).collect();
    let anchored = g.nodes().any(|n| {
        n.kind() == fd.kind && f.content[n.ix().index()] && !f.positions(n.ix().index()).is_empty()
    });
    if anchored {
        place.push(PLACE_EVIDENCE.into());
    }
    let title = match order.first() {
        Some(first) => format!("{} by {first}", fd.kind),
        None => fd.kind.to_string(),
    };
    SpineDecl {
        name: fd.kind.to_string(),
        title,
        main: false,
        anchors: Anchors {
            kind: fd.kind.to_string(),
            inside: fd.inside.then(|| INSIDE.to_string()),
        },
        order,
        place,
    }
}

/// A rule placement needs a neighbour placed by data, so `placed()` is never all rule placements; spread counts data
/// placements only ([`Placement::spread`]).
fn judge(cover: Cover, spread: Spread, no_order: bool) -> Option<WhyNot> {
    if cover.placed() == 0 {
        Some(WhyNot::NoCover)
    } else if no_order {
        Some(WhyNot::NoOrder)
    } else if spread.bands < 2 || spread.largest_band_share >= NO_SPREAD_SHARE {
        Some(WhyNot::NoSpread)
    } else {
        None
    }
}

fn candidate(
    name: String,
    builtin: Option<String>,
    decl: Option<SpineDecl>,
    clock: Clock,
    p: &Placement,
    spread: Spread,
    no_order: bool,
) -> SpineCandidate {
    let why_not = judge(p.cover, spread, no_order);
    SpineCandidate {
        name,
        source: if decl.is_some() {
            SpineSource::AnchorType
        } else {
            SpineSource::Builtin
        },
        builtin,
        decl,
        clock,
        cover: p.cover,
        spread,
        offered: why_not.is_none(),
        why_not,
    }
}

/// An anchor-type candidate: its placement through [`place`], its spread over the anchors.
pub(crate) fn anchor_candidate(
    f: &Facts<'_>,
    decl: SpineDecl,
) -> (SpineCandidate, Placement, AnchorView) {
    let (p, v) = place(f, &decl);
    let keyed: BTreeSet<Key> = v.anchors.iter().filter_map(|&a| v.key[a]).collect();
    let distinct = if keyed.is_empty() {
        v.anchors.iter().filter(|&&a| v.chained[a]).count()
    } else {
        keyed.len()
    };
    let spread = p.spread(distinct);
    let clock = match decl.order.first() {
        Some(o) if !f.is_relation(o) && o != STEP => Clock::Calendar,
        _ => Clock::Order,
    };
    let no_order = decl.order.is_empty();
    let c = candidate(
        format!("{ANCHOR_PREFIX}{}", decl.name),
        None,
        Some(decl),
        clock,
        &p,
        spread,
        no_order,
    );
    (c, p, v)
}

fn day(start: i64) -> u64 {
    start.div_euclid(DAY) as u64
}

/// The `recorded` built-in's span for a node: its created second, read from the time column with no text built
/// (graph-storage, D71 step 12), else (a time in a form umx does not write) `date_span` of the text.
pub(crate) fn recorded_span(node: NodeRef<'_>) -> Option<DateSpan> {
    match node.created_secs() {
        Some(s) => Some(DateSpan {
            start: i64::from(s),
            end: i64::from(s) + 1,
        }),
        None => date_span(&node.created()),
    }
}

/// The built-ins every graph has (brief 5.2): `recorded`, `status`, one `date_field:<f>` per declared node date
/// field some node holds, and `source`.
fn builtins(f: &Facts<'_>, o: &Ontology) -> Vec<(SpineCandidate, Placement)> {
    let g = f.g;
    let n = g.node_bound();
    let mut out = Vec::new();

    let mut p = Placement::new(n);
    let mut seconds = BTreeSet::new();
    for node in g.nodes() {
        let i = node.ix().index();
        if f.content[i] {
            p.how[i] = How::Field;
            if let Some(s) = recorded_span(node) {
                p.band[i] = day(s.start);
                seconds.insert(s.start);
            }
        }
    }
    p.count(f);
    let s = p.spread(seconds.len());
    out.push((builtin(String::from("recorded"), Clock::Calendar, &p, s), p));

    let mut p = Placement::new(n);
    let mut seconds = BTreeSet::new();
    for i in 0..n {
        if let Some(at) = f.last_move[i] {
            p.how[i] = How::Field;
            p.band[i] = day(at);
            seconds.insert(at);
        }
    }
    p.count(f);
    let s = p.spread(seconds.len());
    out.push((builtin(String::from("status"), Clock::Calendar, &p, s), p));

    let mut dates: BTreeSet<&str> = BTreeSet::new();
    for t in o.type_names() {
        for d in o
            .type_decl(t)
            .map(|t| t.fields.as_slice())
            .unwrap_or_default()
        {
            if d.kind == FieldKind::Date {
                dates.insert(d.name.as_str());
            }
        }
    }
    for field in dates {
        let mut p = Placement::new(n);
        let mut values = BTreeSet::new();
        g.each_date(field, false, |ix, span, _| {
            p.how[ix.index()] = How::Field;
            p.band[ix.index()] = day(span.start);
            values.insert((span.start, span.end));
        });
        p.count(f);
        if p.cover.placed() > 0 {
            let s = p.spread(values.len());
            let name = format!("{DATE_FIELD_PREFIX}{field}");
            out.push((builtin(name, Clock::Calendar, &p, s), p));
        }
    }

    let mut p = Placement::new(n);
    let mut at: BTreeMap<(&str, u8, u64), u64> = BTreeMap::new();
    let mut sources = BTreeSet::new();
    for i in 0..n {
        if let Some(first) = f.positions(i).first() {
            p.how[i] = How::Evidence;
            let next = at.len() as u64;
            p.band[i] = *at
                .entry((first.source.as_str(), first.unit as u8, first.start))
                .or_insert(next);
            sources.insert(first.source.as_str());
        }
    }
    p.count(f);
    let s = p.spread(sources.len());
    out.push((builtin(String::from("source"), Clock::Local, &p, s), p));
    out
}

fn builtin(name: String, clock: Clock, p: &Placement, spread: Spread) -> SpineCandidate {
    candidate(name.clone(), Some(name), None, clock, p, spread, false)
}

/// Offered first, then more placed, then a smaller largest band, then more bands, then name.
fn rank(a: &SpineCandidate, b: &SpineCandidate) -> Ordering {
    b.offered
        .cmp(&a.offered)
        .then(b.cover.placed().cmp(&a.cover.placed()))
        .then(
            a.spread
                .largest_band_share
                .partial_cmp(&b.spread.largest_band_share)
                .unwrap_or(Ordering::Equal),
        )
        .then(b.spread.bands.cmp(&a.spread.bands))
        .then(a.name.cmp(&b.name))
}

/// The lowest `EXAMPLES` ids of the handle indexes in `set`.
fn examples(g: &Graph, set: &BTreeSet<usize>) -> Vec<Id> {
    g.nodes()
        .filter(|n| set.contains(&n.ix().index()))
        .take(EXAMPLES)
        .map(|n| n.id())
        .collect()
}

/// A string or int custom field (not a declared date field) with 2 to `MAX_STEP_VALUES` distinct values, two or
/// more holders per value on average, on half the nodes or more: steps held as values, which should be nodes.
fn steps_as_field_values(f: &Facts<'_>, o: &Ontology) -> Vec<InTheWay> {
    let g = f.g;
    let mut names: BTreeSet<&str> = BTreeSet::new();
    for node in g.nodes().filter(|n| f.content[n.ix().index()]) {
        names.extend(node.custom_field_names());
    }
    let dated: BTreeSet<&str> = o
        .type_names()
        .filter_map(|t| o.type_decl(t))
        .flat_map(|t| t.fields.iter())
        .filter(|d| d.kind == FieldKind::Date)
        .map(|d| d.name.as_str())
        .collect();
    let mut out = Vec::new();
    for name in names {
        if dated.contains(name) || ValidName::new(name).is_none() {
            continue;
        }
        let mut values: BTreeMap<Result<i64, alloc::borrow::Cow<'_, str>>, usize> = BTreeMap::new();
        let mut holders = BTreeSet::new();
        let mut too_many = false;
        g.each_field(name, false, |ix, v| {
            let key = match v {
                FieldRef::Str(s) => Err(s),
                FieldRef::UInt(u) => match i64::try_from(u) {
                    Ok(i) => Ok(i),
                    Err(_) => return,
                },
                FieldRef::Int(i) => Ok(i),
                // F227: a line the scanner leaves to the codec holds its values decoded, so a text or a whole number
                // there groups with the same value on a scanned line (as #392 reads such a line's dates).
                FieldRef::Other(Value::String(s)) => Err(alloc::borrow::Cow::Owned(s)),
                FieldRef::Other(Value::Number(n)) => match n.as_i64() {
                    Some(i) => Ok(i),
                    None => return,
                },
                _ => return,
            };
            if too_many {
                return;
            }
            *values.entry(key).or_insert(0) += 1;
            too_many = values.len() > MAX_STEP_VALUES;
            holders.insert(ix.index());
        });
        let distinct = values.len();
        if !too_many
            && distinct >= 2
            && holders.len() >= 2 * distinct
            && 2 * holders.len() >= f.total
        {
            out.push(InTheWay {
                kind: InTheWayKind::StepsAsFieldValues,
                about: name.to_string(),
                count: holders.len(),
                examples: examples(g, &holders),
                next: Command::new("docs get").word("time"),
            });
        }
    }
    out
}

/// Per parent (graph-addressing ruling T1: a step orders only among the anchors with the same `part_of` parent;
/// same-step siblings are allowed): a chain edge that runs against the steps, or a chain that splits into separate
/// runs or branches while the steps of its anchors run in one line.
fn order_disagrees(
    f: &Facts<'_>,
    decl: &SpineDecl,
    v: &AnchorView,
    graph: &ValidName,
) -> Option<InTheWay> {
    let g = f.g;
    let chains: BTreeSet<&str> = decl
        .order
        .iter()
        .map(String::as_str)
        .filter(|o| f.is_relation(o))
        .collect();
    if chains.is_empty() || !decl.order.iter().any(|o| o == STEP) {
        return None;
    }
    let n = g.node_bound();
    let mut step: Vec<Option<i64>> = vec![None; n];
    g.each_field(STEP, false, |ix, value| {
        if v.is_anchor[ix.index()] {
            step[ix.index()] = match value {
                FieldRef::UInt(u) => i64::try_from(u).ok(),
                FieldRef::Int(i) => Some(i),
                _ => None,
            };
        }
    });
    let mut parent: Vec<Option<usize>> = vec![None; n];
    let mut chain: Vec<(usize, usize)> = Vec::new();
    for (from, kind, to) in f.edges() {
        let (a, b) = (from.index(), to.index());
        if !v.is_anchor[a] {
            continue;
        }
        if kind == PART_OF && !v.is_anchor[b] {
            parent[a] = Some(parent[a].map_or(b, |p| p.min(b)));
        } else if chains.contains(kind) && v.is_anchor[b] && a != b {
            chain.push((a, b));
        }
    }
    let mut bad: BTreeSet<usize> = BTreeSet::new();
    // Chain edges inside each parent, by parent.
    let mut groups: BTreeMap<Option<usize>, Vec<(usize, usize)>> = BTreeMap::new();
    for &(a, b) in &chain {
        if parent[a] == parent[b] {
            groups.entry(parent[a]).or_default().push((a, b));
        }
    }
    for edges in groups.values() {
        for &(a, b) in edges {
            if let (Some(x), Some(y)) = (step[a], step[b])
                && x > y
            {
                bad.insert(a);
                bad.insert(b);
            }
        }
        let members: BTreeSet<usize> = edges.iter().flat_map(|&(a, b)| [a, b]).collect();
        let steps: Vec<i64> = members.iter().filter_map(|&m| step[m]).collect();
        let one_line = steps.len() == members.len() && {
            let distinct: BTreeSet<i64> = steps.iter().copied().collect();
            distinct.len() == steps.len()
        };
        if !one_line {
            continue;
        }
        let mut outs: BTreeMap<usize, usize> = BTreeMap::new();
        let mut ins: BTreeMap<usize, usize> = BTreeMap::new();
        for &(a, b) in edges {
            *outs.entry(a).or_insert(0) += 1;
            *ins.entry(b).or_insert(0) += 1;
        }
        let branches = outs.values().chain(ins.values()).any(|&c| c > 1);
        let starts = members.iter().filter(|m| !ins.contains_key(m)).count();
        if branches || starts > 1 {
            bad.extend(members.iter().copied());
        }
    }
    if bad.is_empty() {
        return None;
    }
    let relation = ValidName::new(chains.iter().next().copied()?)?;
    Some(InTheWay {
        kind: InTheWayKind::OrderDisagrees,
        about: decl.anchors.kind.clone(),
        count: bad.len(),
        examples: examples(g, &bad),
        next: Command::new("edge list")
            .word("--graph")
            .name(graph)
            .word("--type")
            .name(&relation),
    })
}

/// A node linked by a `place` relation to anchor A whose evidence overlaps another anchor B and not A (when A has a
/// position to compare with).
fn link_evidence_disagree(
    f: &Facts<'_>,
    decl: &SpineDecl,
    v: &AnchorView,
    graph: &ValidName,
) -> Option<InTheWay> {
    let g = f.g;
    let places: BTreeSet<&str> = decl
        .place
        .iter()
        .map(String::as_str)
        .filter(|p| *p != PLACE_EVIDENCE)
        .collect();
    let mut bad: BTreeSet<usize> = BTreeSet::new();
    for (from, kind, to) in f.edges() {
        let (i, a) = (from.index(), to.index());
        if !places.contains(kind) || !v.is_anchor[a] || i == a {
            continue;
        }
        let mine = f.positions(i);
        if mine.is_empty() || f.positions(a).is_empty() || meets(mine, f.positions(a)) {
            continue;
        }
        if v.anchors
            .iter()
            .any(|&b| b != a && meets(mine, f.positions(b)))
        {
            bad.insert(i);
        }
    }
    let first = *bad.iter().next()?;
    let first = g.nodes().find(|n| n.ix().index() == first)?.id();
    Some(InTheWay {
        kind: InTheWayKind::LinkEvidenceDisagree,
        about: decl.anchors.kind.clone(),
        count: bad.len(),
        examples: examples(g, &bad),
        next: Command::new("node get").address(graph, first),
    })
}

/// Everything the proposer works out: the ranked candidates with each one's placement.
pub(crate) struct Proposed {
    pub candidates: Vec<(SpineCandidate, Placement)>,
    pub in_the_way: Vec<InTheWay>,
    pub total: usize,
}

pub(crate) fn propose(g: &Graph, o: &Ontology, graph: &ValidName) -> Proposed {
    let f = Facts::read(g);
    let mut candidates = Vec::new();
    let mut in_the_way = steps_as_field_values(&f, o);
    for fd in anchor_types(&f) {
        let decl = decl_of(&f, o, &fd);
        if decl.check().is_err() {
            continue;
        }
        let (c, p, v) = anchor_candidate(&f, decl.clone());
        in_the_way.extend(order_disagrees(&f, &decl, &v, graph));
        in_the_way.extend(link_evidence_disagree(&f, &decl, &v, graph));
        candidates.push((c, p));
    }
    candidates.extend(builtins(&f, o));
    candidates.sort_by(|a, b| rank(&a.0, &b.0));
    in_the_way.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.about.cmp(&b.about)));
    Proposed {
        candidates,
        in_the_way,
        total: f.total,
    }
}

/// The spine line for one graph (guide and check).
pub(crate) fn spine_line(g: &Graph, o: &Ontology, graph: &ValidName) -> SpineLine {
    let p = propose(g, o, graph);
    let best = p
        .candidates
        .into_iter()
        .map(|(c, _)| c)
        .find(|c| c.offered)
        .map(|c| (c.name, c.clock, c.cover));
    let by_rule_next = best
        .as_ref()
        .filter(|(_, _, c)| c.by_rule > 0)
        .and_then(|(name, _, _)| listing_cmd(graph, "--by-rule", name));
    SpineLine {
        best,
        next: propose_cmd(graph),
        by_rule_next,
    }
}

/// `--unplaced <candidate>`: the command for the B4a hint. A candidate name is `type:<t>` or `date_field:<f>` with a
/// valid name after the prefix, or a plain built-in.
fn unplaced_cmd(graph: &ValidName, name: &str) -> Option<Command> {
    listing_cmd(graph, "--unplaced", name)
}

/// `propose --unplaced <candidate>` or `--by-rule <candidate>`.
fn listing_cmd(graph: &ValidName, flag: &'static str, name: &str) -> Option<Command> {
    let cmd = propose_cmd(graph).word(flag);
    for prefix in [ANCHOR_PREFIX, DATE_FIELD_PREFIX] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return Some(cmd.qualified(prefix, &ValidName::new(rest)?));
        }
    }
    Some(cmd.name(&ValidName::new(name)?))
}

impl GraphStore {
    /// `umx graph spine propose <graph> [--unplaced <candidate>]`. Reads only; a graph with no candidate offered is
    /// not a failure.
    pub fn graph_spine_propose(
        &self,
        _ctx: &Ctx,
        alias: &str,
        unplaced: Option<&str>,
        by_rule: Option<&str>,
        limit: usize,
        offset: Option<usize>,
    ) -> Outcome<SpineProposal> {
        if unplaced.is_some() && by_rule.is_some() {
            return Err(problems::usage_invalid(
                "give --unplaced or --by-rule, not both",
                "time",
            ));
        }
        let Some(l) = self.graphs.get(alias) else {
            return Err(problems::graph_not_loaded(alias));
        };
        l.last_used
            .store(self.ports.clock.now_ms(), AtomicOrdering::Relaxed);
        let graph = alias_name(alias);
        let g = &l.graph;
        let p = propose(g, l.ontology(), &graph);
        let listing = |name: &str, how: How| -> Result<Unplaced, umx_core::Problem> {
            {
                let Some((c, placed)) = p.candidates.iter().find(|(c, _)| c.name == name) else {
                    let names = p
                        .candidates
                        .iter()
                        .map(|(c, _)| Named {
                            id: c.name.clone(),
                            title: None,
                            score: None,
                        })
                        .collect();
                    return Err(problems::usage_invalid("unknown spine candidate", "time")
                        .candidates(names));
                };
                let offset = offset.unwrap_or(0);
                let rows = g
                    .nodes()
                    .filter(|n| {
                        let i = n.ix().index();
                        !g.is_provenance(n.ix()) && placed.how[i] == how
                    })
                    .map(|n| n.id());
                let ids: Vec<Id> = rows.skip(offset).take(limit).collect();
                Ok(Unplaced {
                    candidate: c.name.clone(),
                    total: if how == How::Rule {
                        c.cover.by_rule
                    } else {
                        c.cover.not_placed
                    },
                    offset,
                    ids,
                })
            }
        };
        let unplaced = unplaced.map(|n| listing(n, How::None)).transpose()?;
        let by_rule = by_rule.map(|n| listing(n, How::Rule)).transpose()?;
        let not_offered = p.candidates.iter().filter(|(c, _)| !c.offered).count();
        let rule_hint = p
            .candidates
            .iter()
            .map(|(c, _)| c)
            .find(|c| c.offered)
            .filter(|c| c.cover.by_rule > 0)
            .and_then(|c| {
                Some((
                    c.name.clone(),
                    SpineHint {
                        count: c.cover.by_rule,
                        next: listing_cmd(&graph, "--by-rule", &c.name)?,
                    },
                ))
            });
        let not_placed = p
            .candidates
            .iter()
            .map(|(c, _)| c)
            .find(|c| c.offered)
            .and_then(|c| {
                Some((
                    c.name.clone(),
                    SpineHint {
                        count: c.cover.not_placed,
                        next: unplaced_cmd(&graph, &c.name)?,
                    },
                ))
            });
        Ok(Done {
            value: SpineProposal {
                alias: graph.as_str().to_string(),
                total: p.total,
                candidates: p.candidates.into_iter().map(|(c, _)| c).collect(),
                in_the_way: p.in_the_way,
                hints: SpineHints {
                    not_offered: SpineHint {
                        count: not_offered,
                        next: propose_cmd(&graph).word("--json"),
                    },
                    not_placed,
                    by_rule: rule_hint,
                },
                unplaced,
                by_rule,
            },
            notes: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use umx_core::ontology::{FieldDecl, TypeDecl};
    use umx_core::{Edge, Node};

    const DAY1: &str = "2026-11-12T09:00:00Z";

    fn node(id: u128, kind: &str) -> Node {
        Node {
            id: Id(id),
            kind: kind.into(),
            title: format!("{kind} {id}"),
            status: "open".into(),
            note: String::new(),
            evidence: vec![],
            status_history: vec![],
            created: DAY1.into(),
            updated: DAY1.into(),
            created_by: "t".into(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields: Default::default(),
        }
    }

    fn set(mut n: Node, field: &str, v: Value) -> Node {
        n.fields.insert(field.into(), v);
        n
    }

    fn seq(a: u64, b: u64) -> Value {
        json!({ "source": "./t/day.ndjson", "selector": { "seq": [a, b] }, "kind": "room", "fidelity": "x", "hash": null })
    }

    fn edge(id: u128, from: u128, kind: &str, to: u128) -> Edge {
        Edge {
            id: Id(id),
            kind: kind.into(),
            from_graph: None,
            from: Id(from),
            to_graph: None,
            to: Id(to),
            created: DAY1.into(),
            created_by: "t".into(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: Default::default(),
        }
    }

    fn ontology() -> Ontology {
        let mut o = Ontology::default();
        o.types.push(TypeDecl::new(
            "segment",
            vec![
                FieldDecl::new("starts_on", FieldKind::Date),
                FieldDecl::new("step", FieldKind::Int),
            ],
        ));
        o
    }

    /// One meeting (1), four segments (10..13) `part_of` it with a start each and steps 1 to 4, and 20 points
    /// (100..119) `raised_in` the segments, five each.
    struct Day {
        nodes: Vec<Node>,
        edges: Vec<Edge>,
    }

    fn day() -> Day {
        let mut nodes = vec![node(1, "meeting")];
        let mut edges = Vec::new();
        for s in 0..4u128 {
            let seg = set(
                set(
                    node(10 + s, "segment"),
                    "starts_on",
                    json!(format!("2026-11-12T0{}:00:00Z", s + 1)),
                ),
                "step",
                json!(s as u64 + 1),
            );
            nodes.push(seg);
            edges.push(edge(1000 + s, 10 + s, "part_of", 1));
        }
        for p in 0..20u128 {
            nodes.push(node(100 + p, "point"));
            edges.push(edge(2000 + p, 100 + p, "raised_in", 10 + p / 5));
        }
        Day { nodes, edges }
    }

    fn graph(d: &Day) -> Graph {
        Graph::from_records(d.nodes.clone(), d.edges.clone())
    }

    fn run(d: &Day) -> Proposed {
        propose(&graph(d), &ontology(), &ValidName::alias("demo").unwrap())
    }

    fn find<'a>(p: &'a Proposed, name: &str) -> &'a SpineCandidate {
        &p.candidates.iter().find(|(c, _)| c.name == name).unwrap().0
    }

    fn kinds(p: &Proposed) -> Vec<(&'static str, String, usize)> {
        p.in_the_way
            .iter()
            .map(|w| (w.kind.as_str(), w.about.clone(), w.count))
            .collect()
    }

    #[test]
    fn every_node_is_counted_once_on_every_candidate() {
        let p = run(&day());
        assert_eq!(p.total, 25);
        for (c, _) in &p.candidates {
            let v = c.cover;
            assert_eq!(
                v.by_field + v.by_link + v.by_evidence + v.by_rule + v.not_placed,
                v.total,
                "{}",
                c.name
            );
            assert_eq!(v.total, 25, "{}", c.name);
        }
        let seg = find(&p, "type:segment");
        let decl = seg.decl.as_ref().unwrap();
        assert_eq!(
            (
                decl.order.clone(),
                decl.place.clone(),
                decl.anchors.inside.clone(),
                decl.main
            ),
            (
                vec!["starts_on".into(), "step".into()],
                vec!["raised_in".into()],
                Some("part_of".into()),
                false
            )
        );
        // Anchors by their date, points by their link, the meeting nowhere.
        let c = seg.cover;
        assert_eq!(
            (c.by_field, c.by_link, c.by_evidence, c.not_placed),
            (4, 20, 0, 1)
        );
        assert_eq!(
            (seg.clock, seg.spread.bands, seg.spread.largest_band_share),
            (Clock::Calendar, 4, 0.25)
        );
        assert!(seg.offered);
        assert_eq!(
            p.candidates[0].0.name, "type:segment",
            "offered and most placed first"
        );
    }

    #[test]
    fn a_spine_with_one_band_is_not_offered() {
        let mut d = day();
        // Every point in the first segment; and the graph was written in one sitting.
        for e in d.edges.iter_mut().filter(|e| e.kind == "raised_in") {
            e.to = Id(10);
        }
        let p = run(&d);
        let seg = find(&p, "type:segment");
        assert_eq!(
            (seg.spread.bands, seg.offered, seg.why_not),
            (1, false, Some(WhyNot::NoSpread))
        );
        let rec = find(&p, "recorded");
        assert_eq!(
            (rec.cover.by_field, rec.why_not),
            (25, Some(WhyNot::NoSpread))
        );
        // Spread the points again and write them on 19 other days: both are offered.
        let mut d = day();
        for (i, n) in d.nodes.iter_mut().enumerate().skip(5) {
            n.created = format!("2026-10-{:02}T09:00:00Z", i % 28 + 1);
        }
        let p = run(&d);
        assert!(find(&p, "type:segment").offered && find(&p, "recorded").offered);
    }

    #[test]
    fn steps_held_as_field_values_are_named_until_they_are_gone() {
        let mut d = day();
        for (i, n) in d.nodes.iter_mut().enumerate().skip(5) {
            n.fields
                .insert("phase".into(), json!(["open", "argue", "close"][i % 3]));
        }
        let p = run(&d);
        assert_eq!(
            kinds(&p),
            vec![("steps_as_field_values", "phase".into(), 20)]
        );
        assert_eq!(
            p.in_the_way[0].examples,
            (100..105).map(Id).collect::<Vec<_>>()
        );
        assert_eq!(p.in_the_way[0].next.as_str(), "umx docs get time");
        for n in &mut d.nodes {
            n.fields.remove("phase");
        }
        assert!(run(&d).in_the_way.is_empty());
    }

    #[test]
    fn steps_on_lines_the_codec_decodes_group_with_the_scanned_ones() {
        // F227: half the holders' lines carry a custom `locator` that is not one, so the scanner leaves them to the
        // codec and their values come back decoded. Text steps and whole-number steps group as on scanned lines.
        for steps in [
            [json!("open"), json!("argue"), json!("close")],
            [json!(1), json!(2), json!(3)],
        ] {
            let mut d = day();
            for (i, n) in d.nodes.iter_mut().enumerate().skip(5) {
                n.fields.insert("phase".into(), steps[i % 3].clone());
                if i % 2 == 0 {
                    n.fields.insert("locator".into(), json!(7));
                }
            }
            let g = graph(&d);
            let f = g.field_id("phase").unwrap();
            let decoded = g
                .nodes()
                .filter(|n| matches!(n.field_ref(f), Some(FieldRef::Other(_))))
                .count();
            assert_eq!(
                decoded, 10,
                "{steps:?}: half the holders are decoded by the codec"
            );
            let p = propose(&g, &ontology(), &ValidName::alias("demo").unwrap());
            assert_eq!(
                kinds(&p),
                vec![("steps_as_field_values", "phase".into(), 20)],
                "{steps:?}"
            );
        }
    }

    fn chained(steps: &[(u128, u64, u128)], chain: &[(u128, u128)]) -> Day {
        // (segment, step, parent meeting), then precedes edges between segments.
        let mut d = Day {
            nodes: vec![node(1, "meeting"), node(2, "meeting")],
            edges: Vec::new(),
        };
        for &(s, step, parent) in steps {
            d.nodes.push(set(node(s, "segment"), "step", json!(step)));
            d.edges.push(edge(1000 + s, s, "part_of", parent));
        }
        for p in 0..20u128 {
            d.nodes.push(node(100 + p, "point"));
            d.edges.push(edge(
                2000 + p,
                100 + p,
                "raised_in",
                steps[p as usize % steps.len()].0,
            ));
        }
        for (i, &(a, b)) in chain.iter().enumerate() {
            d.edges.push(edge(3000 + i as u128, a, "precedes", b));
        }
        d.nodes.sort_by_key(|n| n.id);
        d
    }

    #[test]
    fn step_order_is_worked_out_per_parent_and_same_step_siblings_are_fine() {
        // Steps restart under each meeting; two side-by-side segments share step 2; one chain edge crosses parents
        // against the steps (2 then 1), which is no disagreement.
        let steps = [(10, 1, 1), (11, 2, 1), (12, 2, 1), (13, 1, 2), (14, 2, 2)];
        let ok = chained(&steps, &[(10, 11), (10, 12), (13, 14), (11, 13)]);
        let p = run(&ok);
        assert_eq!(kinds(&p), vec![]);
        assert_eq!(
            find(&p, "type:segment").decl.as_ref().unwrap().order,
            vec!["step".to_string(), "precedes".to_string()]
        );
        // Within one parent a chain edge against the steps is reported; putting it the right way round clears it.
        let p = run(&chained(&steps, &[(11, 10), (13, 14)]));
        assert_eq!(kinds(&p), vec![("order_disagrees", "segment".into(), 2)]);
        assert_eq!(p.in_the_way[0].examples, vec![Id(10), Id(11)]);
        assert_eq!(
            p.in_the_way[0].next.as_str(),
            "umx edge list --graph demo --type precedes"
        );
        assert_eq!(kinds(&run(&chained(&steps, &[(10, 11), (13, 14)]))), vec![]);
    }

    #[test]
    fn a_chain_that_splits_while_the_steps_run_in_one_line_disagrees() {
        let steps = [(10, 1, 1), (11, 2, 1), (12, 3, 1), (13, 4, 1)];
        let p = run(&chained(&steps, &[(10, 11), (12, 13)]));
        assert_eq!(kinds(&p), vec![("order_disagrees", "segment".into(), 4)]);
        assert_eq!(
            kinds(&run(&chained(&steps, &[(10, 11), (11, 12), (12, 13)]))),
            vec![]
        );
    }

    #[test]
    fn evidence_places_the_unlinked_and_a_link_against_the_evidence_is_named() {
        let mut d = day();
        for n in d.nodes.iter_mut() {
            if (10..14).contains(&n.id.0) {
                let s = (n.id.0 - 10) as u64;
                n.evidence = vec![seq(s * 10 + 1, s * 10 + 10)];
            }
        }
        // A point with no link whose passage is in the third segment.
        d.nodes.push(set(node(200, "point"), "x", json!(1)));
        d.nodes.last_mut().unwrap().evidence = vec![seq(25, 26)];
        // Point 100 is raised_in segment 10 (seq 1-10) but cites seq 15, inside segment 11.
        d.nodes
            .iter_mut()
            .find(|n| n.id == Id(100))
            .unwrap()
            .evidence = vec![seq(15, 15)];
        let p = run(&d);
        let seg = find(&p, "type:segment");
        assert_eq!(
            seg.decl.as_ref().unwrap().place,
            vec!["raised_in".to_string(), "evidence".to_string()]
        );
        assert_eq!((seg.cover.by_evidence, seg.cover.not_placed), (1, 1));
        assert_eq!(
            kinds(&p),
            vec![("link_evidence_disagree", "segment".into(), 1)]
        );
        assert_eq!(p.in_the_way[0].examples, vec![Id(100)]);
        assert!(
            p.in_the_way[0]
                .next
                .as_str()
                .starts_with("umx node get umx:demo/")
        );
        let src = find(&p, "source");
        assert_eq!((src.clock, src.cover.by_evidence), (Clock::Local, 6));
        // Cite a passage inside its own segment: the finding is gone.
        d.nodes
            .iter_mut()
            .find(|n| n.id == Id(100))
            .unwrap()
            .evidence = vec![seq(5, 5)];
        assert!(run(&d).in_the_way.is_empty());
    }

    #[test]
    fn an_empty_graph_has_only_built_ins_and_none_offered() {
        let p = propose(
            &Graph::new(),
            &ontology(),
            &ValidName::alias("demo").unwrap(),
        );
        let names: Vec<&str> = p.candidates.iter().map(|(c, _)| c.name.as_str()).collect();
        assert_eq!(names, vec!["recorded", "source", "status"]);
        assert!(
            p.candidates
                .iter()
                .all(|(c, _)| c.why_not == Some(WhyNot::NoCover))
        );
        assert_eq!((p.total, p.in_the_way.len()), (0, 0));
    }

    #[test]
    fn a_graph_with_no_anchors_offers_only_built_ins() {
        let mut nodes: Vec<Node> = (0..10u128).map(|i| node(100 + i, "point")).collect();
        for (i, n) in nodes.iter_mut().enumerate() {
            n.created = format!("2026-10-{:02}T09:00:00Z", i + 1);
        }
        let d = Day {
            nodes,
            edges: vec![],
        };
        let p = run(&d);
        assert!(
            p.candidates
                .iter()
                .all(|(c, _)| c.source == SpineSource::Builtin)
        );
        let best = &p.candidates[0].0;
        assert_eq!(
            (best.name.as_str(), best.offered, best.spread.bands),
            ("recorded", true, 10)
        );
    }

    /// How candidate `name` places node `id`, and the band it puts it in.
    fn placed_on(d: &Day, p: &Proposed, name: &str, id: u128) -> (How, u64) {
        let i = graph(d).node_ix(Id(id)).unwrap().index();
        let (_, pl) = p.candidates.iter().find(|(c, _)| c.name == name).unwrap();
        (pl.how[i], pl.band[i])
    }

    /// ws008 TM5 (brief 5.3): a node nothing else places sits by rule in the middle band of its neighbours placed by
    /// data, one link away either way; a rule placement never feeds another, and the anchors' parent (the meeting,
    /// `part_of` from every segment) is never placed by rule.
    #[test]
    fn a_node_linked_to_placed_nodes_sits_in_their_middle_band_by_rule() {
        let mut d = day();
        // 300 supports points in bands 0, 1 and 3; 301 is supported by segment 12 (band 2); 302 links only to 300.
        for n in [300, 301, 302] {
            d.nodes.push(node(n, "point"));
        }
        for (i, (from, to)) in [(300, 100), (300, 105), (300, 115), (12, 301), (302, 300)]
            .into_iter()
            .enumerate()
        {
            d.edges.push(edge(4000 + i as u128, from, "supports", to));
        }
        let p = run(&d);
        let on = |id| placed_on(&d, &p, "type:segment", id);
        assert_eq!(on(300), (How::Rule, 1), "the lower median of 0, 1 and 3");
        assert_eq!(
            on(301),
            (How::Rule, 2),
            "an anchor counts, either direction"
        );
        assert_eq!(on(302).0, How::None, "a rule placement never feeds another");
        assert_eq!(
            on(1).0,
            How::None,
            "the anchors' parent is never placed by rule"
        );
        let c = find(&p, "type:segment").cover;
        assert_eq!(
            (c.by_field, c.by_link, c.by_rule, c.not_placed, c.total),
            (4, 20, 2, 2, 28)
        );
    }

    /// R366-1 and R366-2, project manager point 2: a neighbour linked by two relations, either way, votes once; every
    /// node an anchor sits inside (a second meeting too) is never placed by rule; and rule placements leave the
    /// spread as the data made it.
    #[test]
    fn rule_placement_counts_each_neighbour_once_and_skips_every_anchor_parent() {
        let base = run(&day());
        let mut d = day();
        // 310 supports 100 (band 0) and 115 (band 3); 115 also blocks 310. Neighbours 0 and 3: the lower median 0.
        d.nodes.push(node(310, "point"));
        for (i, (from, kind, to)) in [
            (310, "supports", 100),
            (310, "supports", 115),
            (115, "blocks", 310),
        ]
        .into_iter()
        .enumerate()
        {
            d.edges.push(edge(5000 + i as u128, from, kind, to));
        }
        // Segment 10 sits in a second meeting too, after meeting 1 in id order.
        d.nodes.push(node(2, "meeting"));
        d.edges.push(edge(5100, 10, "part_of", 2));
        let p = run(&d);
        let on = |id| placed_on(&d, &p, "type:segment", id);
        assert_eq!(on(310), (How::Rule, 0), "each neighbour once");
        assert_eq!(on(2).0, How::None, "a second parent is still a parent");
        let (was, now) = (find(&base, "type:segment"), find(&p, "type:segment"));
        assert_eq!(now.cover.by_rule, 1);
        assert_eq!(
            (now.spread.bands, now.spread.largest_band_share),
            (was.spread.bands, was.spread.largest_band_share),
            "a rule placement never spreads a spine"
        );
    }

    #[test]
    fn a_step_places_only_anchors_by_field_never_other_step_holders() {
        // review B1: with no date, the segments order by step alone; a point holding a step is placed only by
        // its link, and an unlinked one is not placed.
        let mut d = day();
        for n in d.nodes.iter_mut() {
            n.fields.remove("starts_on");
        }
        d.nodes.push(set(node(200, "point"), "step", json!(1)));
        d.nodes
            .iter_mut()
            .find(|n| n.id == Id(100))
            .unwrap()
            .fields
            .insert("step".into(), json!(2));
        let p = run(&d);
        let seg = find(&p, "type:segment");
        assert_eq!(seg.decl.as_ref().unwrap().order, vec!["step".to_string()]);
        assert_eq!(seg.clock, Clock::Order);
        let c = seg.cover;
        assert_eq!(
            (c.by_field, c.by_link, c.by_evidence, c.not_placed),
            (4, 20, 0, 2)
        );
        assert_eq!(placed_on(&d, &p, "type:segment", 200).0, How::None);
        assert_eq!(placed_on(&d, &p, "type:segment", 100).0, How::Link);
    }

    #[test]
    fn with_no_field_the_declared_chain_orders_the_anchors_not_their_ids() {
        // review B2: segments 10, 11, 12 under one meeting, chained 12 then 11 then 10. Point 100 is linked to
        // 10 and 12, so it sits in 12, the first along the chain.
        let mut d = Day {
            nodes: vec![node(1, "meeting")],
            edges: vec![],
        };
        for s in 10..13u128 {
            d.nodes.push(node(s, "segment"));
            d.edges.push(edge(1000 + s, s, "part_of", 1));
        }
        for (i, (from, to)) in [
            (100, 10),
            (100, 12),
            (101, 12),
            (102, 12),
            (103, 11),
            (104, 11),
        ]
        .into_iter()
        .enumerate()
        {
            if !d.nodes.iter().any(|n| n.id == Id(from)) {
                d.nodes.push(node(from, "point"));
            }
            d.edges.push(edge(2000 + i as u128, from, "raised_in", to));
        }
        d.edges.push(edge(3000, 12, "precedes", 11));
        d.edges.push(edge(3001, 11, "precedes", 10));
        let p = run(&d);
        let seg = find(&p, "type:segment");
        assert_eq!(
            seg.decl.as_ref().unwrap().order,
            vec!["precedes".to_string()]
        );
        let band = |p: &Proposed, id| placed_on(&d, p, "type:segment", id).1;
        assert_eq!((band(&p, 100), band(&p, 101), band(&p, 103)), (0, 0, 1));
        assert_eq!((seg.spread.bands, seg.spread.largest_band_share), (2, 0.6));
        // The same chain the other way round puts point 100 in segment 10, now first.
        let mut back = Day {
            nodes: d.nodes.clone(),
            edges: d.edges.clone(),
        };
        for e in back.edges.iter_mut().filter(|e| e.kind == "precedes") {
            core::mem::swap(&mut e.from, &mut e.to);
        }
        let p = run(&back);
        assert_eq!((band(&p, 100), band(&p, 101)), (0, 2));
        assert_eq!(find(&p, "type:segment").spread.bands, 3);
    }

    #[test]
    fn an_anchor_type_named_like_a_built_in_keeps_its_own_name() {
        // review B3: a type called `recorded` is listed as `type:recorded`, beside the `recorded` built-in.
        let mut d = day();
        for n in d.nodes.iter_mut().filter(|n| n.kind == "segment") {
            n.kind = "recorded".into();
        }
        let p = run(&d);
        let names: Vec<&str> = p.candidates.iter().map(|(c, _)| c.name.as_str()).collect();
        let unique: BTreeSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), names.len(), "{names:?}");
        let anchor = find(&p, "type:recorded");
        assert_eq!(
            (anchor.source, anchor.decl.as_ref().unwrap().name.as_str()),
            (SpineSource::AnchorType, "recorded")
        );
        assert_eq!(find(&p, "recorded").source, SpineSource::Builtin);
        assert_eq!(
            unplaced_cmd(&ValidName::alias("demo").unwrap(), "type:recorded")
                .unwrap()
                .as_str(),
            "umx graph spine propose demo --unplaced type:recorded"
        );
    }
}
