//! `umx node evidence <node> list|add|remove` (req-126, ws006 S1b; graph-addressing-expert topic:evidence-maintenance).
//!
//! A pointer is matched on its source and its selector in the address grammar's canonical form (ws006 S2), never its
//! kind, so `quote=we ship` matches a stored `"we ship"` and old objects match without being rewritten. Matching is
//! text only: a `umx:` source is never resolved, so it says nothing about another graph, secured or not.
//!
//! Nothing is erased: a removed pointer moves to the node's `evidence_removed` trail as one entry,
//! `{removed, removed_at, reason, by, by_session}` (graph-addressing-expert ruling). `removed` is the pointer
//! verbatim, its own `by_session` (the session that added it, ws007 P2) included, or absent for a pointer from before
//! P2; nothing is invented for it. The entry's own `by_session` is the session that removed it, stamped by the write
//! (as P2 stamps new pointers). So `evidence_removed[i].by_session` is the remover and
//! `evidence_removed[i].removed.by_session` the adder. The trail is kept among the custom fields in name order, as
//! edge F75's stamps are, so there is no format change and an older umx keeps it byte for byte. Every evidence check
//! reads the live `evidence` only.

use super::node_more::{add_evidence, evidence_pointers, same_evidence};
use super::resolve::fold_scheme;
use super::{Ctx, Done, GraphStore, Outcome, alias_name, check_evidence, problems};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::address::{Selectors, split_repo};
use umx_core::evidence::EVIDENCE_REMOVED as TRAIL;
use umx_core::evidence::{EvidenceKind, PointerError, parse_pointer};
use umx_core::id::normalise_id_text;
use umx_core::ports::GraphDir;
use umx_core::problem::Candidate;
use umx_core::{Id, Node, Problem, ValidName, Value};

/// A trail entry's keys: the pointer verbatim, when, why, who, and the removing session.
pub(crate) const REMOVED: &str = "removed";
pub(crate) const REMOVED_AT: &str = "removed_at";
pub(super) const BY_SESSION: &str = "by_session";

/// The longest `--reason`, in characters.
pub const MAX_REASON: usize = 500;

/// One pointer as `list` shows it: its 1-based position, its text in canonical form, and the stored object.
#[derive(Clone, Debug, PartialEq)]
pub struct EvidenceRow {
    pub at: usize,
    pub text: String,
    pub pointer: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EvidenceListed {
    pub alias: String,
    pub id: Id,
    /// True when these are the trail's entries rather than the live pointers.
    pub removed: bool,
    pub rows: Vec<EvidenceRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EvidenceAdded {
    pub alias: String,
    pub id: Id,
    /// False when the node already held the pointer: nothing was written.
    pub added: bool,
    pub row: EvidenceRow,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EvidenceRemoved {
    pub alias: String,
    pub id: Id,
    /// The trail entry written, and the live position it came from; None when the pointer was already removed, and
    /// for a redact (whose entries hold no pointer; see `redacted`).
    pub removed: Option<EvidenceRow>,
    /// F183: what a redact scrubbed in this graph, as counts only. None for a plain remove.
    pub redacted: Option<Redacted>,
}

/// What `remove --redact` scrubbed in the graph (F183): counts only, never ids or the pointer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Redacted {
    /// Pointers redacted on nodes: live ones moved to their trail, trail ones redacted in place.
    pub pointers: usize,
    /// Nodes that held one (the named node included).
    pub nodes: usize,
    /// Edges whose evidence held one (dropped from the edge, which records the edit).
    pub edges: usize,
}

/// What a remove did to a node in memory.
pub(crate) enum Removal {
    /// The pointer at this live position moved to the trail as this entry.
    Removed(EvidenceRow),
    /// No live pointer matches, and the trail holds one that does: nothing changed.
    AlreadyRemoved,
}

/// A pointer's source (scheme folded, `?repo=` hint kept) and its selector in canonical form, or the stored selector
/// itself when the grammar cannot read it (a hand edit).
pub(super) fn key(p: &Value) -> (String, Result<String, Value>) {
    let source = p.get("source").and_then(Value::as_str).unwrap_or("");
    let selector = p.get("selector").cloned().unwrap_or(Value::Null);
    let canonical = Selectors::from_object(&selector).map(|s| s.canonical());
    (
        fold_scheme(source).into_owned(),
        canonical.map_err(|_| selector),
    )
}

/// The source as matched: scheme folded, and for `umx:` the `?repo=` fetch hint dropped, since a hint is not
/// identity (workshop 002 s7.9; graph-addressing-expert F1).
fn match_source(source: &str) -> &str {
    if source.starts_with("umx:") {
        split_repo(source).map_or(source, |(body, _)| body)
    } else {
        source
    }
}

/// `umx:<alias>/<id>` as its alias and normalised id text.
fn umx_parts(source: &str) -> Option<(&str, String)> {
    let (alias, id) = source.strip_prefix("umx:")?.split_once('/')?;
    Some((alias, normalise_id_text(id).filter(|id| !id.is_empty())?))
}

/// True if `given` (what the caller typed) names `stored`: the same selector in canonical form and the same source.
/// A `umx:` id given may be a prefix of the stored id under the same alias, compared as text (graph-addressing-expert
/// F2): S0 stores full ids, people type prefixes. Nothing is resolved or looked up outside this node.
pub(super) fn names(given: &Value, stored: &Value) -> bool {
    let ((given_source, given_selector), (stored_source, stored_selector)) =
        (key(given), key(stored));
    if given_selector != stored_selector {
        return false;
    }
    let (g, s) = (match_source(&given_source), match_source(&stored_source));
    g == s
        || matches!(
            (umx_parts(g), umx_parts(s)),
            (Some((ga, gid)), Some((sa, sid))) if ga == sa && sid.starts_with(&gid)
        )
}

/// A pointer as text in canonical form, for `list` and candidates: `<source>#<selectors>`, with a `umx:` source's
/// `?repo=` hint after the selectors (it is always last). A selector the grammar cannot read is shown as its JSON.
pub(crate) fn pointer_text(p: &Value) -> String {
    let (source, selector) = key(p);
    let selector = selector.unwrap_or_else(|object| object.to_string());
    let (body, hint) = if source.starts_with("umx:") {
        split_repo(&source).unwrap_or((&source, None))
    } else {
        (source.as_str(), None)
    };
    match hint {
        Some(url) => format!("{body}#{selector}?repo={url}"),
        None => format!("{body}#{selector}"),
    }
}

fn row(at: usize, pointer: &Value) -> EvidenceRow {
    EvidenceRow {
        at,
        text: pointer_text(pointer),
        pointer: pointer.clone(),
    }
}

/// A trail entry as `list --removed` shows it: the text of the pointer it holds (`(redacted)` for a redacted one,
/// F183), and the whole entry.
fn trail_row(at: usize, entry: &Value) -> EvidenceRow {
    let text = if entry.get(REDACTED) == Some(&Value::Bool(true)) {
        "(redacted)".to_string()
    } else {
        pointer_text(entry.get(REMOVED).unwrap_or(&Value::Null))
    };
    EvidenceRow {
        at,
        text,
        pointer: entry.clone(),
    }
}

/// The node's live pointers as candidates (they are this node's own; nothing about another graph).
fn candidates(n: &Node, positions: Option<&[usize]>) -> Vec<Candidate> {
    n.evidence
        .iter()
        .enumerate()
        .map(|(i, p)| (i + 1, p))
        .filter(|(at, _)| positions.is_none_or(|ps| ps.contains(at)))
        .map(|(at, p)| Candidate {
            id: at.to_string(),
            title: Some(pointer_text(p)),
            score: None,
        })
        .collect()
}

/// What a missing `--reason` on an evidence removal says.
pub(crate) const EVIDENCE_REASON_MISSING: &str =
    "removing evidence needs --reason (why it is wrong or stale)";

/// `--reason`: required, 1 to [`MAX_REASON`] characters once trimmed; `missing` says why one is needed.
pub(crate) fn check_reason(
    reason: Option<&str>,
    noun: &'static str,
    missing: &'static str,
) -> Result<String, Problem> {
    let reason = reason.map(str::trim).filter(|r| !r.is_empty());
    match reason {
        None => Err(problems::usage_invalid(missing, noun)),
        Some(r) if r.chars().count() > MAX_REASON => Err(problems::usage_invalid(
            "--reason is at most 500 characters",
            noun,
        )),
        // Security N2: the reason is shown to people and agents, so nothing in it may move, hide or reorder
        // text.
        Some(r) if r.chars().any(hidden_or_control) => Err(problems::usage_invalid(
            "--reason may not hold control, bidi or zero-width characters",
            noun,
        )),
        Some(r) => Ok(r.to_string()),
    }
}

/// Control characters (line breaks included), bidi controls and zero-width characters.
fn hidden_or_control(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{2069}'
                | '\u{FEFF}'
        )
}

/// Parses the pointer to match. Only its grammar is checked: a pointer whose root is gone, or whose `umx:` target is
/// missing, must still be removable, so neither is looked up.
pub(crate) fn pointer_to_match(dir: &GraphDir, text: &str) -> Result<Value, Problem> {
    parse_pointer(text, EvidenceKind::DEFAULT, dir).map_err(|e| match e {
        PointerError::Invalid(reason) => problems::evidence_pointer_invalid(reason),
        PointerError::OutsideRoots => problems::evidence_outside_roots(),
    })
}

/// The node's trail, or `node_reserved_field_in_use` when its `evidence_removed` is the graph's own field (not a
/// list of objects), which umx never overwrites.
pub(super) fn trail(n: &Node) -> Result<Vec<Value>, Problem> {
    match n.fields.get(TRAIL) {
        None => Ok(Vec::new()),
        Some(Value::Array(entries)) if entries.iter().all(Value::is_object) => Ok(entries.clone()),
        Some(_) => Err(problems::node_reserved_field_in_use(n.id)),
    }
}

/// How many entries the node's trail holds (0 when it has none or it is not a list).
pub(crate) fn trail_len(n: &Node) -> usize {
    n.fields
        .get(TRAIL)
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// Stamps the removing session on the trail entries a write adds (those after the first `from`), as ws007 P2 stamps
/// new pointers. The pointer under `removed` is never touched.
pub(crate) fn stamp_removals(n: &mut Node, from: usize, session: Id) {
    if let Some(Value::Array(entries)) = n.fields.get_mut(TRAIL) {
        for entry in entries.iter_mut().skip(from) {
            if let Value::Object(entry) = entry {
                entry.insert(BY_SESSION.into(), Value::String(session.to_text()));
            }
        }
    }
}

/// Moves the live pointer matching `given` to the trail (see the module docs). `at` picks among several matches.
#[allow(clippy::too_many_arguments)]
pub(crate) fn remove_pointer(
    graph: &ValidName,
    n: &mut Node,
    given: &Value,
    at: Option<usize>,
    reason: &str,
    now: &str,
    by: &str,
) -> Result<Removal, Problem> {
    let mut entries = trail(n)?;
    let matches: Vec<usize> = n
        .evidence
        .iter()
        .enumerate()
        .filter(|(_, p)| names(given, p))
        .map(|(i, _)| i + 1)
        .collect();
    let position = match (matches.as_slice(), at) {
        ([], _)
            if entries
                .iter()
                .any(|t| t.get(REMOVED).is_some_and(|p| names(given, p))) =>
        {
            return Ok(Removal::AlreadyRemoved);
        }
        ([], _) => {
            return Err(problems::evidence_not_found(graph, n.id).candidates(candidates(n, None)));
        }
        (found, Some(a)) if found.contains(&a) => a,
        (_, Some(_)) => {
            return Err(problems::usage_invalid(
                "--at names a position whose pointer does not match (see node evidence <node> list)",
                "evidence",
            ));
        }
        ([one], None) => *one,
        (found, None) => {
            return Err(problems::evidence_ambiguous(graph, n.id, found.len())
                .candidates(candidates(n, Some(found))));
        }
    };
    // The stored pointer verbatim under `removed` (never flattened, so none of its keys is renamed or shadowed),
    // plus when, why and by whom. The write stamps `by_session` ([`stamp_removals`]).
    let stored = n.evidence.remove(position - 1);
    let mut entry = Value::Object(Default::default());
    entry[REMOVED] = stored;
    entry[REMOVED_AT] = Value::String(now.to_string());
    entry["reason"] = Value::String(reason.to_string());
    entry["by"] = Value::String(by.to_string());
    entries.push(entry.clone());
    n.fields.insert(TRAIL.to_string(), Value::Array(entries));
    Ok(Removal::Removed(trail_row(position, &entry)))
}

/// A redacted trail entry's flag (F183).
pub(super) const REDACTED: &str = "redacted";

impl GraphStore {
    /// `umx node evidence <node> list [--removed]`: the live pointers, or the trail, each with its position.
    pub fn node_evidence_list(
        &self,
        ctx: &Ctx,
        node: &str,
        removed: bool,
    ) -> Outcome<EvidenceListed> {
        // A read: the graph as loaded, without F144's write check.
        let alias = self.select(ctx, &[Some(node)])?;
        let (alias, n) = self.node_in(&alias, node)?;
        let rows = if removed {
            trail(&n)?
                .iter()
                .enumerate()
                .map(|(i, entry)| trail_row(i + 1, entry))
                .collect()
        } else {
            n.evidence
                .iter()
                .enumerate()
                .map(|(i, p)| row(i + 1, p))
                .collect()
        };
        // F144: a read of a graph whose files changed on disk since the load says so (as node history does).
        let warnings = self.stale_note(&alias).into_iter().collect();
        Ok(Done {
            value: EvidenceListed {
                alias,
                id: n.id,
                removed,
                rows,
            },
            notes: warnings,
        })
    }

    /// `umx node evidence <node> add <pointer>`: node note's evidence without note text (the same parser and checks).
    /// A pointer the node already holds is not added twice, and nothing is written.
    pub fn node_evidence_add(
        &mut self,
        ctx: &Ctx,
        node: &str,
        pointer: &str,
        evidence_kind: Option<&str>,
    ) -> Outcome<EvidenceAdded> {
        let (alias, mut n) = self.node_for_change(ctx, node)?;
        let graphs = self.evidence_graphs(&alias);
        let lookup = |a: &str| graphs.get(a).copied();
        let sources = self.source_check(&alias);
        let mut pointers = evidence_pointers(
            &self.get(&alias).dir,
            &[pointer.to_string()],
            evidence_kind,
            &sources,
            &lookup,
        )?;
        let mut warnings = check_evidence(&mut pointers, &lookup);
        warnings.extend(sources.warnings(&pointers));
        let given = pointers[0].clone();
        let evidence_start = n.evidence.len();
        let added = add_evidence(&mut n.evidence, pointers) == 1;
        if added {
            n.updated = self.now();
            let done = self.write_node(ctx, &alias, n, evidence_start)?;
            warnings.extend(done.notes);
            n = done.value;
        }
        // The stored pointer as it is now (P2's `by_session` included), found as add found it.
        let at = n
            .evidence
            .iter()
            .position(|p| same_evidence(p, &given))
            .map_or(0, |i| i + 1);
        let stored = n.evidence.get(at.wrapping_sub(1)).unwrap_or(&given);
        Ok(Done {
            value: EvidenceAdded {
                alias,
                id: n.id,
                added,
                row: row(at, stored),
            },
            notes: warnings,
        })
    }

    /// `umx node evidence <node> remove <pointer> --reason <text> [--at <n>]` (see the module docs).
    pub fn node_evidence_remove(
        &mut self,
        ctx: &Ctx,
        node: &str,
        pointer: &str,
        reason: Option<&str>,
        at: Option<usize>,
    ) -> Outcome<EvidenceRemoved> {
        let reason = check_reason(reason, "evidence", EVIDENCE_REASON_MISSING)?;
        let (alias, mut n) = self.node_for_change(ctx, node)?;
        let given = pointer_to_match(&self.get(&alias).dir, pointer)?;
        let graph = alias_name(&alias);
        let now = self.now();
        let removal = remove_pointer(&graph, &mut n, &given, at, &reason, &now, &ctx.by)?;
        let id = n.id;
        let (removed, warnings) = match removal {
            Removal::AlreadyRemoved => (None, alloc::vec![problems::already_removed(&graph, id)]),
            Removal::Removed(written) => {
                n.updated = now;
                let evidence_start = n.evidence.len();
                let done = self.write_node(ctx, &alias, n, evidence_start)?;
                // The entry as stored: the write stamped the removing session on it.
                let entry = trail(&done.value)?.pop().unwrap_or(written.pointer);
                (Some(trail_row(written.at, &entry)), done.notes)
            }
        };
        Ok(Done {
            value: EvidenceRemoved {
                alias,
                id,
                removed,
                redacted: None,
            },
            notes: warnings,
        })
    }
}
