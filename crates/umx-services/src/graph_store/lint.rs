//! `umx check --lint` (F139): read-only checks of a graph's content for a graph others share. Every finding is data
//! (`data.lint`, a page of at most [`LINT_PAGE`] rows per graph and kind) and each kind found gives one hint (X1:
//! exit 0, a next). Nothing is refused and nothing is written.
//!
//! - `orphans`: content nodes with no edge at all (derived `similar_to` and provenance links do not count); the std
//!   anchor types (meeting, event, segment, transcript) are left out, since an anchor may stand alone.
//! - `proposed_keys`: a usable `entity_key` ([`stored_keys`]) on a node whose status is `proposed`.
//! - `terms`: this machine's lint terms (`umx_core::lint_terms`, security's shape), matched with the redact's case
//!   fold in every place a read shows, named as the F188 read flags name them; a custom field is named by its
//!   ordinal (`field 2`, as the redact counts), never its name. Never the term or the text.
//! - `double_negatives`: a title that says "lacks no" (or "lack no", "lacking no").
//! - `raised_in_evidence`: a `raised_in` link whose node's evidence and anchor's evidence sit in the same source and
//!   unit but never overlap ([`umx_core::spine::overlap`] on the step-7 [`Facts`] positions); one row per link,
//!   with the nearest pair of pointers.
//! - `sensitivity_label`: umx has no sensitivity label field, so it is always skipped, and `skipped` says so.

use super::evidence_redact::{
    custom_place, edge_place, fold, list_place, members, named_place, selector_values, source_of,
};
use super::spine::Facts;
use super::{GraphStore, alias_name, problems};
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::entity::{ENTITY_KEY, stored_keys};
use umx_core::evidence::EVIDENCE_REMOVED;
use umx_core::lint_terms::LintTerms;
use umx_core::name::{RESERVED_EDGE_FIELDS, RESERVED_NODE_FIELDS};
use umx_core::ontology::{DERIVED_RELATIONS, PROVENANCE_KEYS, PROVENANCE_RELATIONS};
use umx_core::spine::{Position, overlap};
use umx_core::standard::{ANCHOR_TYPES, RAISED_IN};
use umx_core::{Edge, GraphMeta, Id, Node, Problem, ValidName, Value};
use umx_graph::Graph;

/// The most rows one graph's kind gives in one reply; past it, `lint_truncated` names the next page.
pub const LINT_PAGE: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LintKind {
    Orphans,
    ProposedKeys,
    Terms,
    DoubleNegatives,
    RaisedInEvidence,
    SensitivityLabel,
}

impl LintKind {
    pub const ALL: [LintKind; 6] = [
        LintKind::Orphans,
        LintKind::ProposedKeys,
        LintKind::Terms,
        LintKind::DoubleNegatives,
        LintKind::RaisedInEvidence,
        LintKind::SensitivityLabel,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            LintKind::Orphans => "orphans",
            LintKind::ProposedKeys => "proposed_keys",
            LintKind::Terms => "terms",
            LintKind::DoubleNegatives => "double_negatives",
            LintKind::RaisedInEvidence => "raised_in_evidence",
            LintKind::SensitivityLabel => "sensitivity_label",
        }
    }

    pub fn parse(text: &str) -> Option<LintKind> {
        LintKind::ALL.into_iter().find(|k| k.as_str() == text)
    }
}

/// What `--lint` asks for: one kind or all, and where each graph's page of each kind starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LintRequest {
    pub kind: Option<LintKind>,
    pub offset: usize,
}

/// Two evidence ranges in one unit: the node's pointer, then its anchor's, the nearest pair that does not overlap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RangePair {
    pub unit: &'static str,
    pub node: (u64, u64),
    pub anchor: (u64, u64),
}

/// One finding. `id` is a node's (or an edge's, for an edge place); None for graph.json. `place` names where, as
/// the read flags do, list positions and custom fields counted from 1 (F217): `field:title`, `field 2` for the second
/// custom field, `key:1`, `evidence:2`, `edge:<id>/evidence:1`, `edge:<id>/field 1`, `graph:title`, `graph:purpose`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LintRow {
    pub graph: String,
    pub kind: LintKind,
    pub id: Option<Id>,
    pub place: Option<String>,
    /// For `raised_in_evidence`: the anchor and both ranges.
    pub anchor: Option<Id>,
    pub ranges: Option<RangePair>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lint {
    pub rows: Vec<LintRow>,
    /// Kinds that did not run, and why (umx's own words).
    pub skipped: Vec<(LintKind, &'static str)>,
}

struct Finding {
    id: Option<Id>,
    place: Option<String>,
    anchor: Option<Id>,
    ranges: Option<RangePair>,
}

impl Finding {
    fn at(id: Option<Id>, place: Option<String>) -> Finding {
        Finding {
            id,
            place,
            anchor: None,
            ranges: None,
        }
    }
}

const NO_LABEL: &str = "umx has no sensitivity label field";
const NO_TERMS_FILE: &str = "no lint terms file on this machine";
const TERMS_UNUSABLE: &str = "the lint terms file cannot be used";
/// umx's own flag (F188), which only repeats where other places hold a suspect.
const SECRET_SUSPECTED: &str = "secret_suspected";
/// An edge's own stamps: provenance, never content.
const EDGE_STAMPS: &[&str] = &["created", "created_by", "updated", "updated_by", "std"];

/// Content nodes: not provenance.
fn content(g: &Graph) -> impl Iterator<Item = umx_graph::NodeRef<'_>> {
    g.nodes().filter(|n| !g.is_provenance(n.ix()))
}

fn orphans(g: &Graph) -> Vec<Finding> {
    let mut linked = alloc::vec![false; g.node_bound()];
    for e in g.edges() {
        if DERIVED_RELATIONS.contains(&e.kind()) || PROVENANCE_RELATIONS.contains(&e.kind()) {
            continue;
        }
        for end in [e.from_ix(), e.to_ix()].into_iter().flatten() {
            linked[end.index()] = true;
        }
    }
    content(g)
        .filter(|n| !linked[n.ix().index()] && !ANCHOR_TYPES.contains(&n.kind()))
        .map(|n| Finding::at(Some(n.id()), None))
        .collect()
}

fn proposed_keys(g: &Graph) -> Vec<Finding> {
    content(g)
        .filter(|n| n.status() == "proposed")
        .filter(|n| {
            n.to_node()
                .fields
                .get(ENTITY_KEY)
                .is_some_and(|keys| !stored_keys(keys).is_empty())
        })
        .map(|n| Finding::at(Some(n.id()), Some(named_place(ENTITY_KEY))))
        .collect()
}

/// "lacks no", "lack no" or "lacking no", as words in any case.
fn double_negative(title: &str) -> bool {
    let words: Vec<String> = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(fold)
        .collect();
    words
        .windows(2)
        .any(|w| matches!(w[0].as_str(), "lack" | "lacks" | "lacking") && w[1] == "no")
}

fn double_negatives(g: &Graph) -> Vec<Finding> {
    content(g)
        .filter(|n| double_negative(n.title()))
        .map(|n| Finding::at(Some(n.id()), Some(named_place("title"))))
        .collect()
}

/// How far apart two pointers in one unit are (0 when they overlap).
fn gap(a: &Position, b: &Position) -> u64 {
    if a.end < b.start {
        b.start - a.end
    } else {
        a.start.saturating_sub(b.end)
    }
}

/// A `raised_in` link whose node and anchor both cite one source in one unit with no overlap there: one finding per
/// link (parallel edges once), at its first such source, with the nearest pair of pointers there (pm-003: a span
/// of several pointers would hide where they sit).
fn raised_in_evidence(g: &Graph) -> Vec<Finding> {
    let f = Facts::read(g);
    let mut seen: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut out = Vec::new();
    for (from, kind, to) in f.edges() {
        let (i, a) = (from.index(), to.index());
        if kind != RAISED_IN || i == a || !seen.insert((i, a)) {
            continue;
        }
        let (mine, theirs) = (f.positions(i), f.positions(a));
        let in_unit = |ps: &'_ [Position], like: &Position| -> Vec<Position> {
            ps.iter()
                .filter(|p| p.source == like.source && p.unit == like.unit)
                .cloned()
                .collect()
        };
        let apart = mine.iter().find_map(|m| {
            let (xs, ts) = (in_unit(mine, m), in_unit(theirs, m));
            let disjoint = !ts.is_empty() && !xs.iter().any(|x| ts.iter().any(|t| overlap(x, t)));
            disjoint
                .then(|| {
                    xs.iter()
                        .flat_map(|x| ts.iter().map(move |t| (x.clone(), t.clone())))
                        .min_by_key(|(x, t)| (gap(x, t), x.start, t.start))
                })
                .flatten()
        });
        if let Some((x, t)) = apart {
            out.push(Finding {
                id: Some(g.node_at(from).id()),
                place: None,
                anchor: Some(g.node_at(to).id()),
                ranges: Some(RangePair {
                    unit: x.unit.as_str(),
                    node: (x.start, x.end),
                    anchor: (t.start, t.end),
                }),
            });
        }
    }
    out
}

/// Every string in a pointer's source and selector (what a person wrote, not umx's own words).
fn pointer_texts(p: &Value) -> Vec<String> {
    let mut texts = alloc::vec![source_of(p).to_string()];
    if let Some(selector) = p.get("selector") {
        selector_values(selector, &mut texts);
    }
    texts
}

/// Every string inside a value.
fn strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|i| strings(i, out)),
        Value::Object(map) => map.values().for_each(|i| strings(i, out)),
        _ => {}
    }
}

/// A record's fields (a node's or an edge's) as places with their texts. A reserved field by its name, which is
/// umx's own word, from the decoded record. A custom field by its number among the record's custom fields as `line`
/// holds them, from the redact's own count ([`members`], F214), so both name one field alike; never its name (R377-6:
/// a name may hold a term, and is scanned with its value). The removal trail, umx's own flag, provenance keys and
/// `skip` are left to their own places or out.
fn field_places(
    fields: &alloc::collections::BTreeMap<String, Value>,
    line: &[u8],
    reserved: &[&str],
    skip: &[&str],
) -> Vec<(String, Vec<String>)> {
    let left_out = |name: &str| {
        name == EVIDENCE_REMOVED
            || name == SECRET_SUSPECTED
            || skip.contains(&name)
            || PROVENANCE_KEYS.contains(&name)
    };
    let mut out = Vec::new();
    for (name, value) in fields {
        if reserved.contains(&name.as_str()) && !left_out(name) {
            let mut texts = Vec::new();
            strings(value, &mut texts);
            out.push((named_place(name), texts));
        }
    }
    match members(line, reserved) {
        Some(stored) => {
            for m in stored {
                if let Some(n) = m.custom.filter(|_| !left_out(&m.key)) {
                    let mut texts = alloc::vec![m.key];
                    texts.extend(m.parts.into_iter().flatten());
                    out.push((custom_place(n), texts));
                }
            }
        }
        // A line that does not read as one object is one place, as the redact places it.
        None => {
            let mut texts = Vec::new();
            for (name, value) in fields
                .iter()
                .filter(|(n, _)| !reserved.contains(&n.as_str()))
            {
                texts.push(name.clone());
                strings(value, &mut texts);
            }
            out.push(("record".into(), texts));
        }
    }
    out
}

/// An edge's places: its evidence and its fields (R377-3), each under `edge:<id>/`.
fn edge_places(e: &Edge) -> Vec<(String, Vec<String>)> {
    // As the redact reads an edge: edges.jsonl is always written again from its edges, so as umx encodes it.
    let line = umx_core::codec::encode_edges([e]);
    let mut out: Vec<(String, Vec<String>)> = e
        .evidence
        .iter()
        .enumerate()
        .map(|(i, p)| (list_place("evidence", i), pointer_texts(p)))
        .collect();
    out.extend(field_places(
        &e.fields,
        &line,
        RESERVED_EDGE_FIELDS,
        EDGE_STAMPS,
    ));
    let id = e.id.to_text();
    out.into_iter()
        .map(|(place, texts)| (edge_place(&id, &place), texts))
        .collect()
}

/// A node's places as a read shows them, named as F188's read flags name them, with their texts.
fn node_places(n: &Node, line: &[u8]) -> Vec<(String, Vec<String>)> {
    let mut out = alloc::vec![
        (named_place("title"), alloc::vec![n.title.clone()]),
        (named_place("note"), alloc::vec![n.note.clone()]),
    ];
    out.extend(field_places(
        &n.fields,
        line,
        RESERVED_NODE_FIELDS,
        &[ENTITY_KEY],
    ));
    if let Some(locator) = &n.locator {
        out.push((named_place("locator"), alloc::vec![locator.source.clone()]));
    }
    if let Some(keys) = n.fields.get(ENTITY_KEY) {
        let keys: Vec<&Value> = match keys {
            Value::Array(items) => items.iter().collect(),
            other => alloc::vec![other],
        };
        for (i, key) in keys.into_iter().enumerate() {
            let mut texts = Vec::new();
            strings(key, &mut texts);
            out.push((list_place("key", i), texts));
        }
    }
    for (i, p) in n.evidence.iter().enumerate() {
        out.push((list_place("evidence", i), pointer_texts(p)));
    }
    for (i, change) in n.status_history.iter().enumerate() {
        if let Some(reason) = change.get("reason").and_then(Value::as_str) {
            out.push((
                list_place("status_reason", i),
                alloc::vec![reason.to_string()],
            ));
        }
        if let Some(p) = change.get("evidence") {
            // R377-5: a redacted pointer keeps only the reason its redactor wrote.
            let mut texts = pointer_texts(p);
            texts.extend(p.get("reason").and_then(Value::as_str).map(String::from));
            out.push((list_place("status_evidence", i), texts));
        }
    }
    if let Some(Value::Array(trail)) = n.fields.get(EVIDENCE_REMOVED) {
        for (i, entry) in trail.iter().enumerate() {
            let mut texts: Vec<String> = entry
                .get("reason")
                .and_then(Value::as_str)
                .map(String::from)
                .into_iter()
                .collect();
            if let Some(p) = entry.get("removed") {
                texts.extend(pointer_texts(p));
            }
            out.push((list_place(EVIDENCE_REMOVED, i), texts));
        }
    }
    out
}

fn holds(texts: &[String], terms: &[String]) -> bool {
    texts.iter().any(|t| {
        let folded = fold(t);
        terms.iter().any(|term| folded.contains(term.as_str()))
    })
}

fn term_hits(g: &Graph, meta: &GraphMeta, terms: &[String]) -> Vec<Finding> {
    let mut out = Vec::new();
    for n in content(g) {
        let node = n.to_node();
        // As the redact reads a node it leaves alone: its line as stored.
        for (place, texts) in node_places(&node, n.line()) {
            if holds(&texts, terms) {
                out.push(Finding::at(Some(node.id), Some(place)));
            }
        }
    }
    for e in g.edges() {
        if PROVENANCE_RELATIONS.contains(&e.kind()) || DERIVED_RELATIONS.contains(&e.kind()) {
            continue;
        }
        let edge = e.to_edge();
        for (place, texts) in edge_places(&edge) {
            if holds(&texts, terms) {
                out.push(Finding::at(Some(edge.id), Some(place)));
            }
        }
    }
    let o = &meta.ontology;
    let descriptions: Vec<String> = o
        .types
        .iter()
        .flat_map(|t| {
            t.description
                .iter()
                .chain(t.fields.iter().filter_map(|f| f.description.as_ref()))
        })
        .chain(
            o.relation_fields
                .values()
                .flatten()
                .filter_map(|f| f.description.as_ref()),
        )
        .chain(o.status_descriptions.values())
        .chain(o.relation_descriptions.values())
        .cloned()
        .collect();
    for (place, texts) in [
        // R377-4: the title readers see on the graph's own node.
        ("graph:title", alloc::vec![meta.title.clone()]),
        (
            "graph:purpose",
            o.purpose.iter().cloned().collect::<Vec<_>>(),
        ),
        ("graph:conventions", o.conventions.iter().cloned().collect()),
        ("graph:descriptions", descriptions),
    ] {
        if holds(&texts, terms) {
            out.push(Finding::at(None, Some(place.into())));
        }
    }
    out
}

impl GraphStore {
    /// This machine's lint terms: Ok(None) when there is no file, Err(()) when there is one umx cannot use (read
    /// fresh; a file others can read is refused by the port, and counts as unusable).
    fn lint_terms(&self) -> Result<Option<LintTerms>, ()> {
        match self.ports.lint_terms.read() {
            Ok(None) => Ok(None),
            Ok(Some(bytes)) => LintTerms::decode(&bytes).map(Some).ok_or(()),
            Err(_) => Err(()),
        }
    }

    /// The lint of the graphs a check read, each as its files hold it now (`graphs`: alias, decoded graph and decoded
    /// graph.json, None when the folder could not be read), with the hints for what it found. Only those graphs are
    /// scanned (d-20), and only as read: never the loaded copy (R377-1).
    pub(crate) fn lint(
        &self,
        graphs: &[(&str, &Graph, Option<&GraphMeta>)],
        req: &LintRequest,
    ) -> (Lint, Vec<Problem>) {
        let wanted = |k: LintKind| req.kind.is_none_or(|w| w == k);
        let mut lint = Lint::default();
        let mut hints = Vec::new();
        if wanted(LintKind::SensitivityLabel) {
            lint.skipped.push((LintKind::SensitivityLabel, NO_LABEL));
        }
        let terms = if wanted(LintKind::Terms) {
            match self.lint_terms() {
                Ok(Some(t)) => Some(t),
                Ok(None) => {
                    lint.skipped.push((LintKind::Terms, NO_TERMS_FILE));
                    None
                }
                Err(()) => {
                    lint.skipped.push((LintKind::Terms, TERMS_UNUSABLE));
                    hints.push(problems::lint_terms_unusable(&self.ports.lint_terms.path()));
                    None
                }
            }
        } else {
            None
        };
        for (alias, g, meta) in graphs {
            let graph = alias_name(alias);
            for kind in LintKind::ALL.into_iter().filter(|k| wanted(*k)) {
                let mut found = match kind {
                    LintKind::Orphans => orphans(g),
                    LintKind::ProposedKeys => proposed_keys(g),
                    LintKind::DoubleNegatives => double_negatives(g),
                    LintKind::RaisedInEvidence => raised_in_evidence(g),
                    LintKind::Terms => match (&terms, meta) {
                        (Some(t), Some(meta)) => {
                            let folded: Vec<String> = t.for_graph(meta.id).map(fold).collect();
                            if folded.is_empty() {
                                continue;
                            }
                            term_hits(g, meta, &folded)
                        }
                        _ => continue,
                    },
                    LintKind::SensitivityLabel => continue,
                };
                if found.is_empty() {
                    continue;
                }
                found.sort_by(|a, b| (a.id, &a.place).cmp(&(b.id, &b.place)));
                hints.push(found_hint(&graph, kind, &found));
                let total = found.len();
                let end = req.offset.saturating_add(LINT_PAGE);
                if total > end {
                    hints.push(problems::lint_truncated(
                        &graph,
                        kind.as_str(),
                        total - end,
                        end,
                    ));
                }
                lint.rows
                    .extend(
                        found
                            .into_iter()
                            .skip(req.offset)
                            .take(LINT_PAGE)
                            .map(|f| LintRow {
                                graph: graph.as_str().to_string(),
                                kind,
                                id: f.id,
                                place: f.place,
                                anchor: f.anchor,
                                ranges: f.ranges,
                            }),
                    );
            }
        }
        (lint, hints)
    }
}

/// The one hint a graph's kind gives: the count and the first finding.
fn found_hint(graph: &ValidName, kind: LintKind, found: &[Finding]) -> Problem {
    let first = &found[0];
    let count = found.len();
    match kind {
        LintKind::Orphans => problems::lint_orphans(graph, count, first.id.expect("a node")),
        LintKind::ProposedKeys => {
            problems::lint_proposed_keys(graph, count, first.id.expect("a node"))
        }
        LintKind::DoubleNegatives => {
            problems::lint_double_negatives(graph, count, first.id.expect("a node"))
        }
        LintKind::Terms => {
            let place = first.place.as_deref().unwrap_or("");
            let at = match first.id {
                Some(id) if !place.starts_with("edge:") => format!("{} {place}", id.to_text()),
                _ => place.to_string(),
            };
            problems::lint_terms(graph, count, first.id, &at)
        }
        LintKind::RaisedInEvidence => {
            let r = first.ranges.expect("ranges");
            problems::lint_raised_in_evidence(
                graph,
                count,
                first.id.expect("a node"),
                first.anchor.expect("an anchor"),
                r.unit,
                r.node,
                r.anchor,
            )
        }
        LintKind::SensitivityLabel => unreachable!("never run"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_double_negative_is_lacks_no_as_words_in_any_case() {
        for yes in ["Plan lacks No owner", "It LACKING no budget", "lack: no"] {
            assert!(double_negative(yes), "{yes}");
        }
        for no in [
            "Plan lacks an owner",
            "No lack of owners",
            "lacksno",
            "Slack no",
        ] {
            assert!(!double_negative(no), "{no}");
        }
    }
}
