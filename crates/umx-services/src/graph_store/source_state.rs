//! Whether cited files changed (usage 220). `umx check --sources` reads again every local file a node cites (its
//! evidence, its locator) that umx tried to hash when the reference was added, and reports `source_changed` (the
//! hash differs, or a file that could not be read then can be now) and `source_missing` (it cannot be read now). It
//! is read-only and never refuses: findings are data plus one hint per graph and kind. A pointer stored before umx
//! hashed sources has no `hash_state` and is not read; a URL or `umx:` source is never read.
//!
//! `umx node evidence <node> refresh <pointer> | --at <n> | --all` and `umx resource refresh <node>` accept what a file
//! holds now: they store its hash again, or record a state without one (missing, outside...) as accepted, which
//! `check --sources` then leaves out until it changes. A reference whose `root:` is not usable here is left as it was.
//! Refreshing is bookkeeping (req-77): it does not change the node's `updated`.

use super::evidence_edit::{EvidenceRow, names, pointer_text, pointer_to_match};
use super::{Ctx, Done, GraphStore, Outcome, alias_name, problems};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::locator::{Locator, LocatorKind};
use umx_core::source_hash::{
    HashState, SourceDigest, pointer_accepted, pointer_state, stamp_pointer,
};
use umx_core::{Command, Id, Node, Problem, ValidName, Value};
use umx_graph::Graph;

/// The most rows one graph's kind gives in one reply; the hint counts them all.
pub const SOURCES_PAGE: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceFinding {
    Changed,
    Missing,
}

impl SourceFinding {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceFinding::Changed => "source_changed",
            SourceFinding::Missing => "source_missing",
        }
    }
}

/// One cited file that changed or cannot be read. `place` is `evidence:<i>` (counted from 1) or `locator`; `state`
/// is what reading it gave now; the lengths are the stored one and the one now, when there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRow {
    pub graph: String,
    pub kind: SourceFinding,
    pub id: Id,
    pub place: String,
    pub state: HashState,
    pub old_length: Option<u64>,
    pub new_length: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourcesChecked {
    pub rows: Vec<SourceRow>,
    /// Cited files read.
    pub checked: usize,
    /// Cited files not read because their `root:` is not usable on this machine.
    pub unchecked: usize,
    /// Every changed and every missing file found, beyond the rows' page too.
    pub changed: usize,
    pub missing: usize,
}

/// What a node's stored reference said, for comparing with what its file holds now.
struct Stored {
    state: HashState,
    hash: Option<String>,
    length: Option<u64>,
    /// A refresh accepted `state` without a hash.
    accepted: bool,
}

fn stored_pointer(p: &Value) -> Option<Stored> {
    Some(Stored {
        state: pointer_state(p)?,
        hash: p.get("hash").and_then(Value::as_str).map(String::from),
        length: p.get("length").and_then(Value::as_u64),
        accepted: pointer_accepted(p),
    })
}

fn stored_locator(l: &Locator) -> Option<Stored> {
    Some(Stored {
        state: HashState::parse(l.hash_state.as_deref()?)?,
        hash: l.hash.clone(),
        length: l.length,
        accepted: l.hash_accepted == Some(true),
    })
}

/// The finding, if any, for a reference stored as `old` whose file reads as `now`.
fn compare(old: &Stored, now: HashState, digest: Option<&SourceDigest>) -> Option<SourceFinding> {
    match (now, digest) {
        (HashState::Hashed, Some(d)) => (old.state != HashState::Hashed
            || old.hash.as_deref() != Some(d.hash.as_str()))
        .then_some(SourceFinding::Changed),
        (HashState::Remote, _) => None,
        (now, _) if old.accepted && old.state == now => None,
        _ => Some(SourceFinding::Missing),
    }
}

/// The command that shows or accepts a row.
fn fix(graph: &ValidName, row: &SourceRow) -> Command {
    match (row.place.as_str(), row.kind) {
        ("locator", SourceFinding::Changed) => {
            Command::new("resource refresh").address(graph, row.id)
        }
        ("locator", SourceFinding::Missing) => Command::new("resource show").address(graph, row.id),
        (_, SourceFinding::Changed) => {
            let at = row.place.trim_start_matches("evidence:");
            Command::new("node evidence")
                .address(graph, row.id)
                .word("refresh")
                .word("--at")
                .count(at.parse().unwrap_or(1))
        }
        (_, SourceFinding::Missing) => Command::new("node evidence")
            .address(graph, row.id)
            .word("list"),
    }
}

impl GraphStore {
    /// `check --sources` over the checked graphs (see the module docs).
    pub(crate) fn check_sources(
        &self,
        graphs: &[(&str, &Graph)],
    ) -> (SourcesChecked, Vec<Problem>) {
        let mut out = SourcesChecked::default();
        let mut hints = Vec::new();
        for (alias, g) in graphs {
            let graph = alias_name(alias);
            let sources = self.source_check(alias);
            let dir = self.get(alias).dir.clone();
            let mut found: Vec<SourceRow> = Vec::new();
            let mut look = |id: Id, place: String, source: &str, old: Stored| {
                let Some((now, digest)) = sources.read(&dir, source) else {
                    out.unchecked += 1;
                    return;
                };
                out.checked += 1;
                if let Some(kind) = compare(&old, now, digest.as_ref()) {
                    found.push(SourceRow {
                        graph: graph.as_str().to_string(),
                        kind,
                        id,
                        place,
                        state: now,
                        old_length: old.length,
                        new_length: digest.map(|d| d.length),
                    });
                }
            };
            for n in g.nodes() {
                let node = n.to_node();
                if let Some(l) = &node.locator
                    && let Some(old) = stored_locator(l)
                    && old.state.is_local()
                {
                    look(node.id, "locator".into(), &l.source, old);
                }
                for (i, p) in node.evidence.iter().enumerate() {
                    if let (Some(source), Some(old)) =
                        (p.get("source").and_then(Value::as_str), stored_pointer(p))
                        && old.state.is_local()
                    {
                        look(node.id, alloc::format!("evidence:{}", i + 1), source, old);
                    }
                }
            }
            for kind in [SourceFinding::Changed, SourceFinding::Missing] {
                let rows: Vec<SourceRow> =
                    found.iter().filter(|r| r.kind == kind).cloned().collect();
                let Some(first) = rows.first() else {
                    continue;
                };
                let make = match kind {
                    SourceFinding::Changed => problems::source_changed,
                    SourceFinding::Missing => problems::source_missing,
                };
                hints.push(make(
                    &graph,
                    rows.len(),
                    first.id,
                    &first.place,
                    fix(&graph, first),
                ));
                match kind {
                    SourceFinding::Changed => out.changed += rows.len(),
                    SourceFinding::Missing => out.missing += rows.len(),
                }
                out.rows.extend(rows.into_iter().take(SOURCES_PAGE));
            }
        }
        (out, hints)
    }
}

/// One refreshed pointer: its row as stored now, whether its hash keys changed, and whether its source was read
/// (false: its `root:` is not usable here, so it was left as it was).
#[derive(Clone, Debug, PartialEq)]
pub struct Refreshed {
    pub row: EvidenceRow,
    pub changed: bool,
    pub read: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EvidenceRefreshed {
    pub alias: String,
    pub id: Id,
    pub rows: Vec<Refreshed>,
    /// Every local source read, once each (security: a shared graph's pointers name what a refresh reads).
    pub hashed: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceRefreshed {
    pub alias: String,
    pub node: Node,
    pub changed: bool,
    /// The locator's source when it was read; empty for a URL, a folder, or a root not usable here.
    pub hashed: Vec<String>,
}

/// Which pointers a refresh names: one by text, one by position, or every one (`--all`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshPick<'a> {
    Pointer(&'a str),
    At(usize),
    All,
}

impl GraphStore {
    /// `umx node evidence <node> refresh <pointer> | --at <n> | --all`: hashes again the pointer named (matched as
    /// remove matches it), the one at position `n`, or every pointer, and writes the node if any hash key changed. A
    /// pointer whose `root:` is not usable here is left as it was, with `source_not_refreshed`. A state without a hash
    /// (missing, outside...) is recorded as accepted. There is no bare refresh (security): a shared graph may hold
    /// pointers its reader did not write, so each read is named, or `--all` asked for and listed in `hashed`.
    pub fn node_evidence_refresh(
        &mut self,
        ctx: &Ctx,
        node: &str,
        pick: Option<RefreshPick<'_>>,
    ) -> Outcome<EvidenceRefreshed> {
        let pick = pick.ok_or_else(|| {
            problems::usage_invalid(
                "name the pointer to refresh, or --at <n>, or --all (see node evidence <node> list)",
                "evidence",
            )
        })?;
        let (pointer, at) = match pick {
            RefreshPick::Pointer(p) => (Some(p), None),
            RefreshPick::At(a) => (None, Some(a)),
            RefreshPick::All => (None, None),
        };
        let (alias, mut n) = self.node_for_change(ctx, node)?;
        let graph = alias_name(&alias);
        let dir = self.get(&alias).dir.clone();
        let given = pointer.map(|p| pointer_to_match(&dir, p)).transpose()?;
        let mut picked: Vec<usize> = (1..=n.evidence.len())
            .filter(|i| at.is_none_or(|a| a == *i))
            .filter(|i| given.as_ref().is_none_or(|g| names(g, &n.evidence[i - 1])))
            .collect();
        if picked.is_empty() && (given.is_some() || at.is_some()) {
            return Err(match (at, n.evidence.len()) {
                (Some(a), len) if a == 0 || a > len => problems::usage_invalid(
                    "--at names no pointer (see node evidence <node> list)",
                    "evidence",
                ),
                (Some(_), _) => problems::usage_invalid(
                    "--at names a position whose pointer does not match (see node evidence <node> list)",
                    "evidence",
                ),
                (None, _) => problems::evidence_not_found(&graph, n.id),
            });
        }
        picked.sort_unstable();
        let sources = self.source_check(&alias);
        let (mut rows, mut hashed, mut kept) = (Vec::new(), Vec::<String>::new(), 0);
        for i in picked {
            let p = &mut n.evidence[i - 1];
            let before = p.clone();
            let mut read = true;
            if let Some(source) = p.get("source").and_then(Value::as_str).map(String::from) {
                match sources.read(&dir, &source) {
                    Some((state, digest)) => {
                        if state != HashState::Remote && !hashed.contains(&source) {
                            hashed.push(source);
                        }
                        stamp_pointer(p, state, digest.as_ref(), true);
                    }
                    None => {
                        read = false;
                        kept += 1;
                    }
                }
            }
            rows.push((i, *p != before, read));
        }
        let mut warnings = Vec::new();
        if kept > 0 {
            warnings.push(problems::source_not_refreshed(&graph, kept));
        }
        if rows.iter().any(|(_, changed, _)| *changed) {
            let evidence_start = n.evidence.len();
            let done = self.write_node(ctx, &alias, n, evidence_start)?;
            warnings.extend(done.notes);
            n = done.value;
        }
        let rows = rows
            .into_iter()
            .map(|(i, changed, read)| Refreshed {
                row: EvidenceRow {
                    at: i,
                    text: pointer_text(&n.evidence[i - 1]),
                    pointer: n.evidence[i - 1].clone(),
                },
                changed,
                read,
            })
            .collect();
        Ok(Done {
            value: EvidenceRefreshed {
                alias,
                id: n.id,
                rows,
                hashed,
            },
            notes: warnings,
        })
    }

    /// `umx resource refresh <node>`: hashes the node's locator source again, and writes the node if it changed.
    pub fn resource_refresh(&mut self, ctx: &Ctx, node: &str) -> Outcome<ResourceRefreshed> {
        let (alias, mut n) = self.node_for_change(ctx, node)?;
        let Some(mut locator) = n.locator.clone() else {
            return Err(problems::not_a_resource(n.id));
        };
        let sources = self.source_check(&alias);
        let dir = self.get(&alias).dir.clone();
        let mut warnings = Vec::new();
        let mut hashed = Vec::new();
        if !sources.refresh_locator(&dir, &mut locator) {
            warnings.push(problems::source_not_refreshed(&alias_name(&alias), 1));
        } else if locator.kind == LocatorKind::File {
            hashed.push(locator.source.clone());
        }
        let changed = n.locator.as_ref() != Some(&locator);
        if changed {
            n.locator = Some(locator);
            let evidence_start = n.evidence.len();
            let done = self.write_node(ctx, &alias, n, evidence_start)?;
            warnings.extend(done.notes);
            n = done.value;
        }
        Ok(Done {
            value: ResourceRefreshed {
                alias,
                node: n,
                changed,
                hashed,
            },
            notes: warnings,
        })
    }
}
