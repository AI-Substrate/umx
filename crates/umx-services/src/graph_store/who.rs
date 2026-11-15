//! `umx who` and `umx node who` (ws007 s04, req-96): what an agent or session made, and who made a node. Both read
//! provenance whatever the request's `--provenance` (req-95): the stamps through the typed accessors only, never
//! `field()`, which falls back to a kept raw value (graph-addressing-expert N1). A stamp names a session only when it
//! is a provenance session node in the same graph (security R3); anything else reads as unknown. Graphs are chosen by
//! the one scope step (d-20, d-21) before any lookup, and identity across graphs is the prov_key, never the node id.

use super::{Ctx, Done, GraphStore, Outcome, ReadScope, problems};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::provenance::{ProvKey, model_key};
use umx_core::{Id, Problem, Value};
use umx_graph::columns::{FieldId, FieldRef};
use umx_graph::{Graph, NodeRef};

const AUTHORED: &str = "authored";
const EDITED: &str = "edited";
const EVIDENCE_ADDED: &str = "evidence_added";
const EVIDENCE_REMOVED: &str = "evidence_removed";

/// Whether a who row is a node or an edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhoRecord {
    Node,
    Edge,
}

impl WhoRecord {
    pub fn as_str(self) -> &'static str {
        match self {
            WhoRecord::Node => "node",
            WhoRecord::Edge => "edge",
        }
    }
}

/// One content node or edge the agent or session touched.
#[derive(Clone, Debug, PartialEq)]
pub struct WhoRow {
    pub alias: String,
    pub id: Id,
    pub record: WhoRecord,
    /// The node's type or the edge's relation.
    pub kind: String,
    /// The node's title; None for an edge (its ends are not rendered, so no far graph is named).
    pub title: Option<String>,
    /// `authored`, `edited`, `evidence_added`, `evidence_removed`, in that order.
    pub how: Vec<&'static str>,
    /// The distinct session prov_keys behind `how`, sorted.
    pub sessions: Vec<String>,
    /// F188: `field:title` when the shown title looks like a secret.
    pub secret_suspected: Vec<String>,
}

/// `umx who`'s answer: the key matched, the first `limit` rows and how many matched in all.
#[derive(Clone, Debug, PartialEq)]
pub struct WhoListed {
    pub key: String,
    pub rows: Vec<WhoRow>,
    pub total: usize,
    pub truncated: bool,
}

/// A provenance node shown by node who: its prov_key and id, in the node's own graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WhoNode {
    pub key: String,
    pub id: Id,
}

/// What a stamp says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StampState {
    /// It names a session node in the node's graph.
    Recorded,
    /// The node holds no such stamp (an older node, or never edited).
    None,
    /// The stamp is malformed and kept raw, or names no provenance session node in this graph.
    Unknown,
}

impl StampState {
    pub fn as_str(self) -> &'static str {
        match self {
            StampState::Recorded => "recorded",
            StampState::None => "none",
            StampState::Unknown => "unknown",
        }
    }
}

/// One of node who's two rows: who authored, or who last edited. Every missing piece is None, never a guess.
#[derive(Clone, Debug, PartialEq)]
pub struct WhoStamp {
    pub stamp: &'static str,
    pub state: StampState,
    pub session: Option<WhoNode>,
    pub agent: Option<WhoNode>,
    /// The session node's `role` and `model` fields: what that session claimed.
    pub role: Option<String>,
    pub model: Option<String>,
    pub model_node: Option<WhoNode>,
    /// The harness segment of the session key (none for `session/cli/..` and `session/unknown/..`).
    pub harness: Option<String>,
    pub harness_node: Option<WhoNode>,
}

/// `umx node who`'s answer.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeWho {
    pub alias: String,
    pub node: Id,
    pub stamps: Vec<WhoStamp>,
}

/// What `umx who` was given.
enum Wanted {
    Key(String),
    Bare(String),
}

fn wanted(raw: &str) -> Result<Wanted, Problem> {
    let bad = || {
        problems::usage_invalid(
            "umx who takes the prov_key of an agent or session, or a bare seat, user or session id",
            "who",
        )
    };
    if raw.contains('/') {
        let key = ProvKey::parse(raw).map_err(|_| bad())?;
        return match key.kind() {
            "agent" | "session" => Ok(Wanted::Key(raw.to_string())),
            _ => Err(bad()),
        };
    }
    let bare = raw.to_ascii_lowercase();
    let shaped = (1..=128).contains(&bare.len())
        && bare != "."
        && bare != ".."
        && bare
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
    if shaped {
        Ok(Wanted::Bare(bare))
    } else {
        Err(bad())
    }
}

/// The node behind `id` when it is a provenance node of type `kind` in `graph` (reserved type plus a valid prov_key
/// of that kind), with its key.
fn provenance_of<'g>(graph: &'g Graph, id: Id, kind: &str) -> Option<(NodeRef<'g>, String)> {
    let node = graph.node(id)?;
    if node.kind() != kind || !graph.is_provenance(node.ix()) {
        return None;
    }
    let Some(Value::String(key)) = node.custom_field("prov_key") else {
        return None;
    };
    Some((node, key))
}

/// The agent a session belongs to: the first `session_of` edge out of it, as P2 keeps it (session_agent_conflict
/// keeps the first link).
fn agent_of(graph: &Graph, session: NodeRef<'_>) -> Option<(Id, String)> {
    let step = graph
        .edges_out(session.ix())
        .find(|s| s.kind.as_str() == "session_of")?;
    let id = graph.node_at(step.other).id();
    provenance_of(graph, id, "agent").map(|(_, key)| (id, key))
}

/// Every provenance node of `kind` in `graph` holding `key`, a merge's duplicates included (duplicate_prov_key keeps
/// both). Identity is the key, never the node id, within a graph as across graphs.
fn holders<'g>(graph: &'g Graph, key: &str, kind: &str) -> Vec<NodeRef<'g>> {
    graph
        .provenance_holders(key)
        .filter(|n| n.kind() == kind)
        .collect()
}

/// The sessions `key` stands for in `graph`, sorted by id, each with its key; None when the graph has no such node.
/// A session key gives every session node holding it; an agent key, the sessions whose agent holds it.
fn sessions_for(graph: &Graph, key: &str) -> Option<Vec<(Id, String)>> {
    let kind = ProvKey::parse(key).ok()?.kind();
    let found = holders(graph, key, kind);
    if found.is_empty() {
        return None;
    }
    let mut out: Vec<(Id, String)> = if kind == "session" {
        found.iter().map(|n| (n.id(), key.to_string())).collect()
    } else {
        found
            .iter()
            .flat_map(|agent| graph.edges_in(agent.ix()))
            .filter(|s| s.kind.as_str() == "session_of")
            .filter_map(|s| {
                let id = graph.node_at(s.other).id();
                let (session, session_key) = provenance_of(graph, id, "session")?;
                // The session's own agent link (its first session_of, as P2 keeps it) must name this key.
                (agent_of(graph, session).map(|(_, k)| k).as_deref() == Some(key))
                    .then_some((id, session_key))
            })
            .collect()
    };
    out.sort();
    out.dedup_by_key(|(id, _)| *id);
    Some(out)
}

/// The session ids of `sessions` that an evidence item names in `by_session` (an Id string; anything else counts for
/// nobody).
fn by_session<'s>(item: &Value, sessions: &'s [(Id, String)]) -> Option<&'s str> {
    let id: Id = item.get("by_session")?.as_str()?.parse().ok()?;
    lookup(sessions, Some(id))
}

fn lookup(sessions: &[(Id, String)], id: Option<Id>) -> Option<&str> {
    let id = id?;
    sessions
        .binary_search_by_key(&id, |(s, _)| *s)
        .ok()
        .map(|i| sessions[i].1.as_str())
}

/// Collects a record's tags and, while the page still has room, the sessions behind them. Past the page a hit is only
/// counted: no session text, title or flag is built for it (review F1).
struct Touch<'s> {
    /// One bit per tag, in [`TAGS`] order.
    how: u8,
    sessions: Vec<&'s str>,
    keep: bool,
}

const TAGS: [&str; 4] = [AUTHORED, EDITED, EVIDENCE_ADDED, EVIDENCE_REMOVED];

impl<'s> Touch<'s> {
    fn new(keep: bool) -> Self {
        Touch {
            how: 0,
            sessions: Vec::new(),
            keep,
        }
    }

    fn add(&mut self, tag: &'static str, session: Option<&'s str>) {
        let Some(session) = session else {
            return;
        };
        let bit = TAGS.iter().position(|t| *t == tag).expect("a known tag");
        self.how |= 1 << bit;
        if self.keep && !self.sessions.contains(&session) {
            self.sessions.push(session);
        }
    }

    fn hit(&self) -> bool {
        self.how != 0
    }

    /// The tags in [`TAGS`] order and the distinct session keys, sorted.
    fn finish(mut self) -> (Vec<&'static str>, Vec<String>) {
        let how = (0..TAGS.len())
            .filter(|bit| self.how & (1 << bit) != 0)
            .map(|bit| TAGS[bit])
            .collect();
        self.sessions.sort_unstable();
        (
            how,
            self.sessions.into_iter().map(ToString::to_string).collect(),
        )
    }
}

/// Evidence pointers by path (graph-addressing-expert): `evidence[i].by_session` added it; in the trail,
/// `evidence_removed[i].by_session` removed it and `evidence_removed[i].removed.by_session` had added it. A status
/// move's pointer (ws008 TM6) names its adder in `status_history[i].evidence.by_session`; once a redact made it a
/// redacted entry in place (`redacted: true`), that session is the redactor, so it counts as evidence removed
/// (graph-storage-expert).
fn node_evidence<'s>(
    node: NodeRef<'_>,
    trail: Option<FieldId>,
    sessions: &'s [(Id, String)],
    touch: &mut Touch<'s>,
) {
    let cold = node.evidence_and_moves();
    let text = cold.evidence_json();
    if text.trim() != "[]" {
        for item in &umx_core::model::decode_evidence(text.as_bytes()) {
            touch.add(EVIDENCE_ADDED, by_session(item, sessions));
        }
    }
    // Decoded, never searched as text: a key may be written escaped (`\u0065vidence`, review F3). Read typed, so
    // no value tree is built for every history (graph-storage); a shape the typed read refuses is read as values.
    // Only `redacted: true` marks a redacted entry; a stored `false` is a pointer (graph-storage-expert).
    let moves = cold.moves_json();
    if moves.trim() != "[]" {
        let tag = |redacted: bool| {
            if redacted {
                EVIDENCE_REMOVED
            } else {
                EVIDENCE_ADDED
            }
        };
        match umx_core::model::status_move_evidence(moves.as_bytes()) {
            Some(cited) => {
                for c in &cited {
                    let id = c.by_session.as_deref().and_then(|s| s.parse::<Id>().ok());
                    touch.add(tag(c.redacted), lookup(sessions, id));
                }
            }
            None => {
                for change in &umx_core::model::decode_evidence(moves.as_bytes()) {
                    if let Some(pointer) = change.get("evidence") {
                        let redacted =
                            pointer.get(super::evidence_edit::REDACTED) == Some(&Value::Bool(true));
                        touch.add(tag(redacted), by_session(pointer, sessions));
                    }
                }
            }
        }
    }
    let Some(FieldRef::Other(Value::Array(entries))) = trail.and_then(|f| node.field_ref(f)) else {
        return;
    };
    for entry in &entries {
        touch.add(EVIDENCE_REMOVED, by_session(entry, sessions));
        if let Some(removed) = entry.get("removed") {
            touch.add(EVIDENCE_ADDED, by_session(removed, sessions));
        }
    }
}

impl GraphStore {
    /// `umx who <key|seat|session>`: every content node and edge the agent or session authored, last edited, or added
    /// or removed evidence on, over the graphs the read covers (d-20: an unnamed secured graph is only counted). A full
    /// key is used as given; a bare id must end exactly one agent or session key across those graphs. Rows run in
    /// scope order, then nodes and edges each in id order; `limit` caps the rows, `total` counts them all.
    pub fn who(&self, ctx: &Ctx, who: &str, limit: usize) -> Outcome<WhoListed> {
        let wanted = wanted(who)?;
        let scope = self.graphs_in_scope(ctx, None, "who")?;
        let secured = self.secured_count(&scope);
        let graphs = scope.aliases.len();
        let key = match wanted {
            Wanted::Key(key) => key,
            Wanted::Bare(bare) => {
                let mut keys: Vec<String> = Vec::new();
                for alias in &scope.aliases {
                    let graph = &self.get(alias).graph;
                    graph.each_field("prov_key", true, |ix, v| {
                        let FieldRef::Str(text) = v else {
                            return;
                        };
                        let Ok(key) = ProvKey::parse(&text) else {
                            return;
                        };
                        if graph.is_provenance(ix)
                            && matches!(key.kind(), "agent" | "session")
                            && text.rsplit('/').next() == Some(bare.as_str())
                            && !keys.iter().any(|k| k.as_str() == &*text)
                        {
                            keys.push(text.to_string());
                        }
                    });
                }
                keys.sort();
                match keys.len() {
                    0 => return Err(problems::who_unknown(graphs, secured)),
                    1 => keys.remove(0),
                    _ => return Err(problems::who_ambiguous(&keys, graphs, secured)),
                }
            }
        };
        let allows = core::cell::OnceCell::new();
        let (mut rows, mut total, mut found) = (Vec::new(), 0, false);
        for alias in &scope.aliases {
            let graph = &self.get(alias).graph;
            let Some(sessions) = sessions_for(graph, &key) else {
                continue;
            };
            found = true;
            if sessions.is_empty() {
                continue;
            }
            let content = ReadScope::new(graph);
            let trail = graph.field_id(EVIDENCE_REMOVED);
            for node in content.nodes() {
                let mut touch = Touch::new(rows.len() < limit);
                touch.add(AUTHORED, lookup(&sessions, node.authored_by_session()));
                touch.add(EDITED, lookup(&sessions, node.edited_by_session()));
                node_evidence(node, trail, &sessions, &mut touch);
                if !touch.hit() {
                    continue;
                }
                total += 1;
                if !touch.keep {
                    continue;
                }
                let (how, keys) = touch.finish();
                let title = node.title().to_string();
                rows.push(WhoRow {
                    alias: alias.clone(),
                    id: node.id(),
                    record: WhoRecord::Node,
                    kind: node.kind().to_string(),
                    secret_suspected: self.member_flags(&allows, alias, node.id(), &title, &[]),
                    title: Some(title),
                    how,
                    sessions: keys,
                });
            }
            for edge in content.edges() {
                let mut touch = Touch::new(rows.len() < limit);
                touch.add(AUTHORED, lookup(&sessions, edge.authored_by_session()));
                touch.add(EDITED, lookup(&sessions, edge.edited_by_session()));
                for item in &edge.evidence() {
                    touch.add(EVIDENCE_ADDED, by_session(item, &sessions));
                }
                if !touch.hit() {
                    continue;
                }
                total += 1;
                if !touch.keep {
                    continue;
                }
                let (how, keys) = touch.finish();
                rows.push(WhoRow {
                    alias: alias.clone(),
                    id: edge.id(),
                    record: WhoRecord::Edge,
                    kind: edge.kind().to_string(),
                    title: None,
                    how,
                    sessions: keys,
                    secret_suspected: Vec::new(),
                });
            }
        }
        if !found {
            return Err(problems::who_unknown(graphs, secured));
        }
        Ok(Done {
            value: WhoListed {
                key,
                truncated: total > rows.len(),
                rows,
                total,
            },
            notes: self.scope_warnings(&scope)?,
        })
    }

    /// `umx node who <node>`: the session, agent, role, model and harness behind the node's authored and last edited
    /// stamps, each provenance node from the node's own graph. The node resolves as any default read does: content
    /// only, whatever the request's provenance flag.
    pub fn node_who(&self, ctx: &Ctx, node: &str) -> Outcome<NodeWho> {
        let Done {
            value: (alias, n),
            notes: warnings,
        } = self.node_get(&ctx.clone().with_provenance(false), node)?;
        let graph = &self.get(&alias).graph;
        let at = graph.node(n.id);
        let stamps = [
            (
                AUTHORED,
                at.map(NodeRef::authored_by_session),
                "authored_by_session",
            ),
            (
                EDITED,
                at.map(NodeRef::edited_by_session),
                "edited_by_session",
            ),
        ]
        .into_iter()
        .map(|(stamp, typed, name)| {
            let typed = typed.flatten();
            // A malformed stamp is kept raw (req-67): it says nothing, but it is not "no stamp".
            let kept = at.is_some_and(|n| n.custom_field(name).is_some());
            stamp_row(graph, stamp, typed, kept)
        })
        .collect();
        Ok(Done {
            value: NodeWho {
                alias,
                node: n.id,
                stamps,
            },
            notes: warnings,
        })
    }
}

fn stamp_row(graph: &Graph, stamp: &'static str, typed: Option<Id>, kept: bool) -> WhoStamp {
    let mut row = WhoStamp {
        stamp,
        state: if kept {
            StampState::Unknown
        } else {
            StampState::None
        },
        session: None,
        agent: None,
        role: None,
        model: None,
        model_node: None,
        harness: None,
        harness_node: None,
    };
    let Some(id) = typed else {
        return row;
    };
    let Some((session, key)) = provenance_of(graph, id, "session") else {
        row.state = StampState::Unknown;
        return row;
    };
    row.state = StampState::Recorded;
    let text = |name: &str| match session.custom_field(name) {
        Some(Value::String(s)) => Some(s),
        _ => None,
    };
    row.role = text("role");
    row.model = text("model");
    row.model_node = row
        .model
        .as_deref()
        .and_then(|m| model_key(m).ok())
        .and_then(|k| provenance_at(graph, &k));
    let mut parts = key.split('/');
    row.harness = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("session"), Some(h), Some(_), None) if h != "cli" && h != "unknown" => {
            Some(h.to_string())
        }
        _ => None,
    };
    row.harness_node = row
        .harness
        .as_deref()
        .and_then(|h| provenance_at(graph, &alloc::format!("harness/{h}")));
    row.agent = agent_of(graph, session).map(|(id, key)| WhoNode { key, id });
    row.session = Some(WhoNode { key, id });
    row
}

fn provenance_at(graph: &Graph, key: &str) -> Option<WhoNode> {
    graph.provenance_node(key).map(|n| WhoNode {
        key: key.to_string(),
        id: n.id(),
    })
}
