//! The store's side of the secret hold (req-49, SEC-10, ws004 s08): which fields of a node look like secrets, this
//! machine's allows (`$UMX_HOME/allows/secret-allows.json`, its own LocalState instance) and `umx embeddings allow`.
//!
//! The hold itself is decided from the text right before a hosted send (see
//! [`crate::embedding::EmbeddingService::still_sendable_where`]), so nothing about it is stored: a held field stays
//! held across restarts, pulls and hand edits. Only an explicit allow is stored, keyed by the hash of the exact text.

use super::evidence_redact::{custom_place, edge_place, list_place, members, named_place};
use super::{EdgeRow, GraphStore, alias_name, resolve_node};
use crate::{Ctx, Done, Outcome, problems};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::entity::ENTITY_KEY;
use umx_core::evidence::EVIDENCE_REMOVED;
use umx_core::ontology::FieldKind;
use umx_core::ports::local_state::REFUSED_MISSING;
use umx_core::ports::{LocalStateError, StateWritten};
use umx_core::secret_allows::SecretAllows;
use umx_core::secrets;
use umx_core::secrets::Rule;
use umx_core::vectors::{self, TextHash};
use umx_core::{Id, Node, Ontology, Problem, Value};

/// F188: the key of the read-side flags on a node object; reserved, so a stored field of that name is never scanned
/// or shown (R324-1).
const SECRET_SUSPECTED: &str = "secret_suspected";

pub(crate) struct WriteSecret {
    pub field: String,
    pub rule: Rule,
    embedded: bool,
}

impl WriteSecret {
    pub(crate) fn warning(
        &self,
        graph: &umx_core::ValidName,
        id: Id,
        record: &'static str,
    ) -> Problem {
        if self.embedded {
            problems::secret_suspected(graph, id, &self.field, self.rule.name())
        } else {
            problems::metadata_secret_suspected(graph, id, record, &self.field, self.rule.name())
        }
    }

    pub(crate) fn refusal(&self, line: u32) -> Problem {
        problems::batch_secret_invalid(line, &self.field, self.rule.name())
    }
}

/// Where on a node the secret scan found a suspected secret (F188): a place, never a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Spot<'n> {
    /// `title`, `note` or a string custom field.
    Field(&'n str),
    /// The resource locator's source (ws006 S3b).
    Locator,
    /// The i-th entry of the stored `entity_key` list.
    Key(usize),
    /// The i-th evidence pointer.
    Evidence(usize),
    /// The i-th status move's reason.
    StatusReason(usize),
    /// The i-th status move's evidence pointer (ws008 TM6), scanned as an evidence pointer is.
    StatusEvidence(usize),
    /// The i-th `evidence_removed` entry's reason.
    RemovalReason(usize),
    /// The i-th `evidence_removed` entry's removed pointer. Writes skip it: it was scanned when it was added.
    RemovalPointer(usize),
}

/// A value of a node the scan flags: where, by which rule, and whether the text is embedded.
pub(crate) struct Suspect<'n> {
    pub spot: Spot<'n>,
    pub rule: Rule,
    pub embedded: bool,
}

/// The text of field `field` on `n`: `title`, `note` or a string custom field.
fn text_of<'a>(n: &'a Node, field: &str) -> Option<&'a str> {
    match field {
        "title" => Some(n.title.as_str()),
        "note" => Some(n.note.as_str()),
        _ => n.fields.get(field).and_then(Value::as_str),
    }
}

/// The string entries of the stored `entity_key` list, by their position in it: what a reply shows, so `key:<i>`
/// indexes the reply's own array (F188). A value that is not a list is a plain field, scanned as one.
fn key_entries(n: &Node) -> Vec<(usize, &str)> {
    match n.fields.get(ENTITY_KEY) {
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .filter_map(|(i, v)| Some((i, v.as_str()?)))
            .collect(),
        _ => Vec::new(),
    }
}

fn removal_trail(n: &Node) -> &[Value] {
    match n.fields.get(EVIDENCE_REMOVED) {
        Some(Value::Array(entries)) => entries,
        _ => &[],
    }
}

/// The one secret scan of a node's values (writes warn about it, reads flag it; F188): each place `look` accepts is
/// scanned, in a fixed order. An embedded field whose exact text `allowed` lets through is not a suspect.
pub(crate) fn node_suspects<'n>(
    node: &'n Node,
    ontology: &Ontology,
    allowed: &dyn Fn(&str, &str) -> bool,
    look: &dyn Fn(Spot<'n>) -> bool,
) -> Vec<Suspect<'n>> {
    let mut hits = Vec::new();
    let mut hit = |spot, rule, embedded| {
        hits.push(Suspect {
            spot,
            rule,
            embedded,
        })
    };
    // F226: a custom field that is not a string (a hand-edited list or object) is scanned by every string inside it,
    // as the lint's term scan reads one. `entity_key` and `evidence_removed` lists have their own scans, below.
    let fields = ["title", "note"].into_iter().chain(
        node.fields
            .iter()
            .filter(|(field, value)| {
                !(value.is_array() && matches!(field.as_str(), ENTITY_KEY | EVIDENCE_REMOVED))
            })
            .map(|(field, _)| field.as_str()),
    );
    for field in fields {
        if !look(Spot::Field(field)) {
            continue;
        }
        let Some(text) = text_of(node, field) else {
            // Never embedded (only text is), so no allow clears it.
            if let Some(rule) = node.fields.get(field).and_then(|v| value_suspect(field, v)) {
                hit(Spot::Field(field), rule, false);
            }
            continue;
        };
        let embedded = matches!(field, "title" | "note")
            || ontology.field_kind(&node.kind, field) == Some(FieldKind::String);
        if let Some(rule) = secrets::field_suspect(field, text)
            && !(embedded && allowed(field, text))
        {
            hit(Spot::Field(field), rule, embedded);
        }
    }
    // ws008 E1 (security N2): entity_key is a list, so the text scan above never sees it. Keys are never embedded.
    for (i, key) in key_entries(node) {
        if look(Spot::Key(i))
            && let Some(rule) = key_suspect(key)
        {
            hit(Spot::Key(i), rule, false);
        }
    }
    // ws006 S3b (security): a locator's source is scanned as an evidence source is.
    if let Some(locator) = &node.locator
        && look(Spot::Locator)
        && let Some(rule) =
            umx_core::evidence::suspected_secret(&super::resources::pointer(&locator.source))
    {
        hit(Spot::Locator, rule, false);
    }
    for (i, pointer) in node.evidence.iter().enumerate() {
        if look(Spot::Evidence(i))
            && let Some(rule) = umx_core::evidence::suspected_secret(pointer)
        {
            hit(Spot::Evidence(i), rule, false);
        }
    }
    for (i, change) in node.status_history.iter().enumerate() {
        if look(Spot::StatusReason(i))
            && let Some(reason) = change.get("reason").and_then(Value::as_str)
            && let Some(rule) = secrets::scan(reason)
        {
            hit(Spot::StatusReason(i), rule, false);
        }
        if look(Spot::StatusEvidence(i))
            && let Some(pointer) = change.get("evidence")
            && let Some(rule) = umx_core::evidence::suspected_secret(pointer)
        {
            hit(Spot::StatusEvidence(i), rule, false);
        }
    }
    // Security N1: a removal's reason is graph text too; the trail is never embedded.
    for (i, entry) in removal_trail(node).iter().enumerate() {
        if look(Spot::RemovalReason(i))
            && let Some(reason) = entry.get("reason").and_then(Value::as_str)
            && let Some(rule) = secrets::scan(reason)
        {
            hit(Spot::RemovalReason(i), rule, false);
        }
        if look(Spot::RemovalPointer(i))
            && let Some(removed) = entry.get("removed")
            && let Some(rule) = umx_core::evidence::suspected_secret(removed)
        {
            hit(Spot::RemovalPointer(i), rule, false);
        }
    }
    hits
}

/// The scan of one entity key (keys are never embedded): the same check a write and a read make.
pub(crate) fn key_suspect(key: &str) -> Option<Rule> {
    secrets::field_suspect(ENTITY_KEY, key)
}

/// What a write would store that looks like a secret: [`node_suspects`] over the places this write changed (new
/// or changed text, keys and pointers not there before, status moves and removals it adds). New keys report once.
pub(crate) fn node_write_secrets(
    before: Option<&Node>,
    node: &Node,
    ontology: &Ontology,
    graph: Id,
    allows: &SecretAllows,
) -> Vec<WriteSecret> {
    // Keys are fresh by canonical identity, as umx stores and compares them (R324-4): a loaded key in an older form
    // that this write only rewrites canonically is not new. Raw positions are for read flags only.
    let canonical = |n: &Node| {
        n.fields
            .get(ENTITY_KEY)
            .map(umx_core::entity::stored_keys)
            .unwrap_or_default()
    };
    let old_keys = before.map(canonical).unwrap_or_default();
    let keys: BTreeMap<usize, &str> = key_entries(node).into_iter().collect();
    let fresh = |spot: Spot<'_>| match spot {
        Spot::Field(field @ ("title" | "note")) => {
            before.and_then(|old| text_of(old, field)) != text_of(node, field)
        }
        Spot::Field(field) => {
            before.and_then(|old| old.fields.get(field)) != node.fields.get(field)
        }
        Spot::Locator => before
            .and_then(|old| old.locator.as_ref())
            .zip(node.locator.as_ref())
            .is_none_or(|(old, new)| old.source != new.source),
        Spot::Key(i) => keys.get(&i).is_some_and(|k| {
            umx_core::entity::normalise_key(k).is_ok_and(|key| !old_keys.contains(&key))
        }),
        Spot::Evidence(i) => !before.is_some_and(|old| old.evidence.contains(&node.evidence[i])),
        Spot::StatusReason(i) | Spot::StatusEvidence(i) => {
            i >= before.map_or(0, |old| old.status_history.len())
        }
        Spot::RemovalReason(i) => i >= before.map_or(0, |old| removal_trail(old).len()),
        Spot::RemovalPointer(_) => false,
    };
    let allowed =
        |field: &str, text: &str| allows.allows(graph, node.id, field, TextHash::of(text));
    let mut key_reported = false;
    node_suspects(node, ontology, &allowed, &fresh)
        .into_iter()
        .filter(|s| !matches!(s.spot, Spot::Key(_)) || !core::mem::replace(&mut key_reported, true))
        .map(|s| WriteSecret {
            field: match s.spot {
                Spot::Field(field) => field.into(),
                Spot::Locator => "locator".into(),
                Spot::Key(_) => ENTITY_KEY.into(),
                Spot::Evidence(_) => "evidence".into(),
                Spot::StatusReason(_) => "status_reason".into(),
                Spot::StatusEvidence(_) => "status_evidence".into(),
                Spot::RemovalReason(_) | Spot::RemovalPointer(_) => {
                    "evidence_removed_reason".into()
                }
            },
            rule: s.rule,
            embedded: s.embedded,
        })
        .collect()
}

/// Where on an edge the secret scan found a suspected secret (F190): a place, never a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EdgeSpot<'e> {
    /// A string field, by its name.
    Field(&'e str),
    /// The i-th pointer of the evidence scanned.
    Evidence(usize),
}

/// The scan of one custom field's value: a string as it is, a list by every string inside it (F226), and an object by
/// each key as text, then its value (F229: a hand edit can put a credential in a key, `{"AKIA…": 1}`); the first hit.
/// Numbers, booleans and null hold no text.
fn value_suspect(field: &str, value: &Value) -> Option<Rule> {
    match value {
        Value::String(text) => secrets::field_suspect(field, text),
        Value::Array(items) => items.iter().find_map(|v| value_suspect(field, v)),
        Value::Object(map) => map.iter().find_map(|(key, v)| {
            secrets::field_suspect(field, key).or_else(|| value_suspect(field, v))
        }),
        _ => None,
    }
}

/// The one secret scan of an edge's values (writes warn about it, reads flag it; F190): each custom field in stored
/// order (the strings inside a list or object too, keys included: F226, F229), then each pointer of `evidence`. Edge
/// text is never embedded, so no allow clears a hit.
fn edge_suspects<'e>(
    fields: &'e BTreeMap<String, Value>,
    evidence: &[Value],
) -> Vec<(EdgeSpot<'e>, Rule)> {
    let field_hits = fields.iter().filter_map(|(field, value)| {
        value_suspect(field, value).map(|rule| (EdgeSpot::Field(field), rule))
    });
    let evidence_hits = evidence.iter().enumerate().filter_map(|(i, pointer)| {
        umx_core::evidence::suspected_secret(pointer).map(|rule| (EdgeSpot::Evidence(i), rule))
    });
    field_hits.chain(evidence_hits).collect()
}

/// The secrets an edge write would store: its custom string fields, and `new_evidence`, the evidence pointers this
/// write adds (req-124, ws006 S5b), scanned as a node's new pointers are. Edge text is never embedded.
pub(crate) fn edge_write_secrets(
    fields: &BTreeMap<String, Value>,
    new_evidence: &[Value],
) -> Vec<WriteSecret> {
    edge_suspects(fields, new_evidence)
        .into_iter()
        .map(|(spot, rule)| WriteSecret {
            field: match spot {
                EdgeSpot::Field(field) => field.into(),
                EdgeSpot::Evidence(_) => "evidence".into(),
            },
            rule,
            embedded: false,
        })
        .collect()
}

/// F190: the places umx's secret scan (the one an edge write warns with) flags on an edge a read shows, named as the
/// lint and the redact name an edge's places, so the three agree (F217: counted from 1): `edge:<id>/evidence:<i>`,
/// `edge:<id>/field <n>` for the n-th custom field as the edge's line holds it (never its name, which a person wrote),
/// and `edge:<id>/field:<name>` for a reserved one umx names (a `not_same_as` reason). Never a value, hash or rule.
/// Only the stored fields in `fields` are scanned, the ones the reply shows (a stored field a computed column of the
/// same name hides is not, review on #385), each given as (stored name, the key the reply shows it under): a field umx
/// names is named after its shown key (edge show's `evidence_field_kept` for an older edge's prose `evidence`, F223).
/// The evidence is scanned when `evidence`; the numbers stay the stored ones. A derived row (no id) holds nothing to
/// flag. A stored field named `secret_suspected` is never scanned: the reply never shows it (R324-1).
pub fn edge_secret_flags(
    edge: &super::ReadEdge,
    fields: &[(String, String)],
    evidence: bool,
) -> Vec<String> {
    let Some(id) = edge.id else {
        return Vec::new();
    };
    let id = id.to_text();
    let shown_as = |field: &str| {
        fields
            .iter()
            .find(|(stored, _)| stored == field && field != SECRET_SUSPECTED)
            .map(|(_, shown)| shown.as_str())
    };
    let evidence: &[Value] = if evidence { &edge.evidence } else { &[] };
    // The custom-field numbers as the redact and the lint count them: over the edge's line as umx writes it (F214),
    // worked out once, at the first flagged field.
    let line = core::cell::OnceCell::new();
    let line = || {
        line.get_or_init(|| {
            let stored = edge.clone().into_stored().expect("an edge with an id");
            members(
                &umx_core::codec::encode_edges([&stored]),
                umx_core::name::RESERVED_EDGE_FIELDS,
            )
        })
    };
    edge_suspects(&edge.fields, evidence)
        .into_iter()
        .filter_map(|(spot, _)| match spot {
            EdgeSpot::Field(field) => {
                let shown = shown_as(field)?;
                Some(edge_place(
                    &id,
                    &match line() {
                        Some(members) => match members.iter().find(|m| m.key == field) {
                            Some(m) => m.custom.map_or_else(|| named_place(shown), custom_place),
                            None => named_place(shown),
                        },
                        // As the lint and the redact place a line that does not read as one object.
                        None => "record".into(),
                    },
                ))
            }
            EdgeSpot::Evidence(i) => Some(edge_place(&id, &list_place("evidence", i))),
        })
        .collect()
}

#[cfg(test)]
mod write_scan_tests {
    use super::*;

    #[test]
    fn exact_embedded_allows_never_override_metadata_with_the_same_field_name() {
        let token = alloc::format!("ghp_{}", "a".repeat(36));
        let mut node = Node {
            id: Id(2),
            kind: "problem".into(),
            title: "Clean title".into(),
            status: "open".into(),
            note: token.clone(),
            evidence: Vec::new(),
            status_history: Vec::new(),
            created: "2026-01-01T00:00:00Z".into(),
            updated: "2026-01-01T00:00:00Z".into(),
            created_by: "tester".into(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields: BTreeMap::new(),
        };
        let mut allows = SecretAllows::default();
        allows.add(Id(1), node.id, "note", TextHash::of(&token), "now");
        allows.add(Id(1), node.id, "status_reason", TextHash::of(&token), "now");
        let ontology = Ontology::starter();
        assert!(node_write_secrets(None, &node, &ontology, Id(1), &allows).is_empty());
        node.status_history
            .push(serde_json::json!({"reason": token}));
        let hits = node_write_secrets(None, &node, &ontology, Id(1), &allows);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].field, "status_reason");
        node.note.push_str(" changed");
        let hits = node_write_secrets(None, &node, &ontology, Id(1), &allows);
        assert!(hits.iter().any(|hit| hit.field == "note"));
    }

    /// Security N1: a new trail entry's reason is scanned; one already on the node before the write is not.
    #[test]
    fn a_new_evidence_removal_reason_is_scanned() {
        let token = alloc::format!("ghp_{}", "b".repeat(36));
        let mut before = Node {
            id: Id(3),
            kind: "problem".into(),
            title: "Clean".into(),
            status: "open".into(),
            note: String::new(),
            evidence: Vec::new(),
            status_history: Vec::new(),
            created: "2026-01-01T00:00:00Z".into(),
            updated: "2026-01-01T00:00:00Z".into(),
            created_by: "tester".into(),
            authored_by_session: None,
            edited_by_session: None,
            fields: BTreeMap::new(),
            locator: None,
        };
        let (ontology, allows) = (Ontology::starter(), SecretAllows::default());
        let mut node = before.clone();
        node.fields.insert(
            EVIDENCE_REMOVED.into(),
            serde_json::json!([{ "source": "a.md", "reason": token }]),
        );
        let hits = node_write_secrets(Some(&before), &node, &ontology, Id(1), &allows);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            (hits[0].field.as_str(), hits[0].embedded),
            ("evidence_removed_reason", false)
        );
        before = node.clone();
        node.fields.insert(
            EVIDENCE_REMOVED.into(),
            serde_json::json!([{ "source": "a.md", "reason": token }, { "source": "b.md", "reason": "stale" }]),
        );
        assert!(node_write_secrets(Some(&before), &node, &ontology, Id(1), &allows).is_empty());
    }

    /// F190, F217: a custom field is named by its number among the custom fields as the edge's line holds them
    /// (reserved ones such as `reason` and `updated` are not counted, and are named by their name), and a pointer by
    /// its position from 1; only the fields given are scanned, the numbers staying the stored ones, and the evidence
    /// only when asked; a derived row (no id) flags nothing.
    #[test]
    fn edge_flags_count_custom_fields_from_one_and_name_reserved_ones() {
        let token = alloc::format!("ghp_{}", "c".repeat(36));
        let pointer =
            serde_json::json!({ "source": alloc::format!("https://example.test/?t={token}") });
        let fields: BTreeMap<String, Value> = [
            ("alpha", "plain"),
            ("reason", token.as_str()),
            ("updated", "2026-01-01T00:00:00Z"),
            ("zeta", token.as_str()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.into())))
        .collect();
        let mut edge = super::super::ReadEdge {
            id: Some(Id(7)),
            kind: "supports".into(),
            from_graph: None,
            from: Id(1),
            to_graph: None,
            to: Id(2),
            created: "2026-01-01T00:00:00Z".into(),
            created_by: "tester".into(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: alloc::vec![serde_json::json!({ "source": "a.md" }), pointer],
            fields,
        };
        let at = Id(7).to_text();
        let names = |v: &[&str]| {
            v.iter()
                .map(|s| alloc::format!("edge:{at}/{s}"))
                .collect::<Vec<_>>()
        };
        let all: Vec<(String, String)> =
            edge.fields.keys().map(|k| (k.clone(), k.clone())).collect();
        assert_eq!(
            edge_secret_flags(&edge, &all, true),
            names(&["field:reason", "field 2", "evidence:2"])
        );
        let zeta = ("zeta".to_string(), "zeta".to_string());
        assert_eq!(
            edge_secret_flags(&edge, core::slice::from_ref(&zeta), false),
            names(&["field 2"])
        );
        // F223: a field umx names is named after the key the reply shows it under; a custom one keeps its number.
        let renamed = [("reason".to_string(), "why".to_string()), zeta];
        assert_eq!(
            edge_secret_flags(&edge, &renamed, false),
            names(&["field:why", "field 2"])
        );
        assert_eq!(edge_secret_flags(&edge, &[], true), names(&["evidence:2"]));
        assert!(edge_secret_flags(&edge, &[], false).is_empty());
        edge.id = None;
        assert!(edge_secret_flags(&edge, &all, true).is_empty());
    }
}

/// Why a new allow is refused when the file is full (architect, R120-A1 condition 3).
const FULL: &str = "it is full at 256 allows or 60 KiB (remove allows: redact or change the allowed texts to drop them or delete the file to hold every text again)";

/// What `umx embeddings allow` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretAllowed {
    pub alias: String,
    pub node: Id,
    pub field: String,
    /// Whether the field's text may now go to a hosted embedder (false: nothing to allow).
    pub allowed: bool,
    /// The rule its text trips, when it trips one.
    pub rule: Option<&'static str>,
    /// Why nothing was allowed, in umx's words.
    pub why: Option<&'static str>,
}

/// A graph as a read row names it: by id (an edge's ends, a neighbour, walk or path row) or by loaded alias (a
/// search, find or pairs row).
#[derive(Clone, Copy, Debug)]
pub enum GraphKey<'a> {
    Id(Id),
    Alias(&'a str),
}

/// The title rule of [`GraphStore::title_flagged`] over an allows snapshot `allows` gives, read only for a suspect
/// title: a batch holds its snapshot, a read reads one lazily.
pub(crate) fn title_suspect<'a>(
    allows: impl FnOnce() -> &'a SecretAllows,
    graph: Id,
    node: Id,
    title: &str,
) -> bool {
    secrets::field_suspect("title", title).is_some()
        && !allows().allows(graph, node, "title", TextHash::of(title))
}

impl GraphStore {
    /// The raw values of a node's embedded fields (see [`vectors::field_values`]).
    pub fn node_values(&self, alias: &str, id: Id) -> Option<Vec<(String, String)>> {
        let loaded = self.graphs.get(alias)?;
        let node = loaded.graph.node(id)?;
        Some(vectors::field_values(
            &node.to_node(),
            &loaded.meta.ontology,
        ))
    }

    /// The node's whole-node text now, and the same text rebuilt with every field `drop(field, value)` picks left out
    /// (a blank title reads `none`). None if the graph or node is gone.
    pub fn whole_texts(
        &self,
        alias: &str,
        id: Id,
        drop: &dyn Fn(&str, &str) -> bool,
    ) -> Option<(String, String)> {
        let loaded = self.graphs.get(alias)?;
        let node = loaded.graph.node(id)?.to_node();
        let ontology = &loaded.meta.ontology;
        let whole = |n: &umx_core::Node| {
            let field = vectors::whole_node_field();
            vectors::node_texts(n, ontology)
                .into_iter()
                .find(|(f, _)| *f == field)
                .map(|(_, t)| t)
                .unwrap_or_default()
        };
        let mut without = node.clone();
        for (field, value) in vectors::field_values(&node, ontology) {
            if drop(&field, &value) {
                match field.as_str() {
                    "title" => without.title.clear(),
                    "note" => without.note.clear(),
                    _ => {
                        without.fields.remove(&field);
                    }
                }
            }
        }
        Some((whole(&node), whole(&without)))
    }

    /// This machine's allows, read fresh. A file that cannot be used means none: every flagged text stays held.
    pub fn secret_allows(&self) -> SecretAllows {
        match self.ports.allows.read() {
            Ok(Some(bytes)) => SecretAllows::decode(&bytes).unwrap_or_default(),
            _ => SecretAllows::default(),
        }
    }

    /// F188: for each `(alias, node)` a read shows, the places the secret scan flags, named as the reply names them
    /// (`field:<name>`, `key:<i>`, `evidence:<i>`, `status_reason:<i>`, `status_evidence:<i>`, `evidence_removed:<i>`), in
    /// scan order. Never
    /// a value, hash or rule. `columns` None means the whole node is shown (node get); otherwise only those columns
    /// are, and nothing else is scanned. An allow clears a field only while the field is still embedded (its exact
    /// text among the node's embedded values, R324-5): an allow for a field later made data no longer hides it. The
    /// allows file is read at most once, at the first hit. A stored field named `secret_suspected` is never scanned:
    /// the reply never shows it (R324-1).
    pub fn secret_flags(
        &self,
        rows: &[(&str, &Node)],
        columns: Option<&[String]>,
    ) -> Vec<Vec<String>> {
        let allows: core::cell::OnceCell<SecretAllows> = core::cell::OnceCell::new();
        let shown = |column: &str| columns.is_none_or(|cols| cols.iter().any(|c| c == column));
        let locator_names: Vec<&str> = ["locator", "locator_source"]
            .into_iter()
            .filter(|c| match columns {
                None => *c == "locator",
                Some(_) => shown(c),
            })
            .collect();
        rows.iter()
            .map(|(alias, node)| {
                let Some(loaded) = self.graphs.get(*alias) else {
                    return Vec::new();
                };
                // R324-5: as on a write, but only for a field the node still embeds with this exact text.
                let allowed = |field: &str, text: &str| {
                    vectors::field_values(node, &loaded.meta.ontology)
                        .iter()
                        .any(|(f, t)| f == field && t == text)
                        && allows.get_or_init(|| self.secret_allows()).allows(
                            loaded.meta.id,
                            node.id,
                            field,
                            TextHash::of(text),
                        )
                };
                let look = |spot: Spot<'_>| match spot {
                    Spot::Field(field) => field != SECRET_SUSPECTED && shown(field),
                    Spot::Locator => !locator_names.is_empty(),
                    Spot::Key(_) => shown(ENTITY_KEY),
                    Spot::Evidence(_) => shown("evidence"),
                    Spot::StatusReason(_) | Spot::StatusEvidence(_) => shown("status_history"),
                    Spot::RemovalReason(_) | Spot::RemovalPointer(_) => shown(EVIDENCE_REMOVED),
                };
                let mut names: Vec<String> = Vec::new();
                for s in node_suspects(node, &loaded.meta.ontology, &allowed, &look) {
                    let spot_names = match s.spot {
                        Spot::Field(field) => alloc::vec![named_place(field)],
                        Spot::Locator => locator_names.iter().map(|c| named_place(c)).collect(),
                        Spot::Key(i) => alloc::vec![list_place("key", i)],
                        Spot::Evidence(i) => alloc::vec![list_place("evidence", i)],
                        Spot::StatusReason(i) => alloc::vec![list_place("status_reason", i)],
                        Spot::StatusEvidence(i) => alloc::vec![list_place("status_evidence", i)],
                        Spot::RemovalReason(i) | Spot::RemovalPointer(i) => {
                            alloc::vec![list_place(EVIDENCE_REMOVED, i)]
                        }
                    };
                    for name in spot_names {
                        if !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }
                names
            })
            .collect()
    }

    /// F188: the positions in a list of stored keys (entity show's `keys`) the scan flags, as `key:<i>`.
    pub(crate) fn key_flags(keys: &[String]) -> Vec<String> {
        keys.iter()
            .enumerate()
            .filter(|(_, key)| key_suspect(key).is_some())
            .map(|(i, _)| list_place("key", i))
            .collect()
    }

    /// F188: entity show's flags for a member it shows by `title` and stored `keys` (from the node's hot fields; no
    /// line is decoded). The title is embedded, so an allow for its exact text clears it, as on a write. `allows` is
    /// one lazy snapshot for the whole read (R324-3): the file is read at most once, at the first suspect title.
    pub(crate) fn member_flags(
        &self,
        allows: &core::cell::OnceCell<SecretAllows>,
        alias: &str,
        id: Id,
        title: &str,
        keys: &[String],
    ) -> Vec<String> {
        let mut flags = Vec::new();
        if let Some(loaded) = self.graphs.get(alias)
            && self.title_flagged(allows, loaded.meta.id, id, title)
        {
            flags.push(named_place("title"));
        }
        flags.extend(Self::key_flags(keys));
        flags
    }

    /// The one rule for a node's title wherever a read shows it (F188, F228): umx's secret check flags it, and no
    /// allow covers its exact text (a title is embedded, so an allow clears it, as on a write). `graph` is the graph's
    /// id. `allows` is one lazy snapshot for the whole read: the file is read at most once, at the first suspect.
    pub(crate) fn title_flagged(
        &self,
        allows: &core::cell::OnceCell<SecretAllows>,
        graph: Id,
        node: Id,
        title: &str,
    ) -> bool {
        title_suspect(
            || allows.get_or_init(|| self.secret_allows()),
            graph,
            node,
            title,
        )
    }

    /// F228, F234: for each edge row, whether the `from` and `to` titles it shows are flagged by
    /// [`Self::title_flagged`]. An end in a graph the read does not show (a state) shows its address, not a title.
    pub fn edge_titles_flagged(&self, rows: &[EdgeRow]) -> Vec<[bool; 2]> {
        fn visible<'t>(state: Option<&str>, title: &'t str) -> &'t str {
            if state.is_none() { title } else { "" }
        }
        let titles: Vec<(GraphKey<'_>, Id, &str)> = rows
            .iter()
            .flat_map(|r| {
                [
                    (
                        GraphKey::Id(r.from_graph),
                        r.edge.from,
                        visible(r.from_state, &r.from_title),
                    ),
                    (
                        GraphKey::Id(r.to_graph),
                        r.edge.to,
                        visible(r.to_state, &r.to_title),
                    ),
                ]
            })
            .collect();
        self.titles_flagged(&titles)
            .chunks(2)
            .map(|ends| [ends[0], ends[1]])
            .collect()
    }

    /// F234: whether edge `e`'s sentence as graph `alias` stores it (`stored_sentence`: a local end by its title, a
    /// far end by its address) holds a title [`Self::title_flagged`] flags.
    pub(crate) fn stored_sentence_flagged(&self, alias: &str, e: &umx_core::Edge) -> bool {
        let Some(l) = self.graphs.get(alias) else {
            return false;
        };
        let allows = core::cell::OnceCell::new();
        [(e.from_graph, e.from), (e.to_graph, e.to)]
            .into_iter()
            .filter(|(graph, _)| graph.is_none())
            .filter_map(|(_, node)| l.graph.node(node).map(|n| (node, n.title())))
            .any(|(node, title)| self.title_flagged(&allows, l.meta.id, node, title))
    }

    /// F234: for each problem candidate `(id, title)` a reply shows (a near duplicate, an ambiguous or missing node's
    /// matches, a spine's anchors), whether its title is flagged by the rule of [`Self::title_flagged`]. A candidate
    /// does not say which graph it came from, so it is flagged unless every loaded graph holding a node of that id
    /// with that exact title allows it; a title no loaded node holds (a spine's, a pointer's) has no allow to clear
    /// it. It reads the holders and allows as they are now, which may be later than the read that made the candidates
    /// (the daemon asks after the reply is built, F242), so changes to the loaded holders or to allows in between can
    /// clear a flag or add one: with copies G1 (not allowing the title) and G2 (allowing it), unloading G1 or
    /// retitling its node clears the flag (R405). A known limit (security): the window is one request; a cleared flag
    /// always means text some graph's owner explicitly allowed; the reply carries the title either way; and the flag
    /// is advisory.
    pub fn candidate_titles_flagged(&self, candidates: &[(&str, &str)]) -> Vec<bool> {
        let allows = core::cell::OnceCell::new();
        candidates
            .iter()
            .map(|(id, title)| {
                if secrets::field_suspect("title", title).is_none() {
                    return false;
                }
                let Ok(id) = id.parse::<Id>() else {
                    return true;
                };
                let mut holders = self
                    .graphs
                    .values()
                    .filter(|l| l.graph.node(id).is_some_and(|n| n.title() == *title))
                    .peekable();
                holders.peek().is_none()
                    || holders.any(|l| self.title_flagged(&allows, l.meta.id, id, title))
            })
            .collect()
    }

    /// F228: for each node title a read shows on another record (an edge row's ends, a neighbour, walk, path, search,
    /// find or pairs row), whether [`Self::title_flagged`] flags it. A graph is named by id or by loaded alias; a title
    /// in a graph this store does not know by that alias is not flagged (it is not shown either).
    pub fn titles_flagged(&self, titles: &[(GraphKey<'_>, Id, &str)]) -> Vec<bool> {
        let allows = core::cell::OnceCell::new();
        titles
            .iter()
            .map(|(graph, node, title)| {
                let graph = match graph {
                    GraphKey::Id(id) => Some(*id),
                    GraphKey::Alias(alias) => self.graphs.get(*alias).map(|l| l.meta.id),
                };
                graph.is_some_and(|graph| self.title_flagged(&allows, graph, *node, title))
            })
            .collect()
    }

    /// A `secret_suspected` warning for each field of `nodes` whose text looks like a secret and is not allowed.
    /// Names the graph, node, field and rule; never the text.
    pub fn secret_warnings(&self, alias: &str, nodes: &[Id]) -> Vec<Problem> {
        let Some(loaded) = self.graphs.get(alias) else {
            return Vec::new();
        };
        let graph = alias_name(alias);
        let mut allows: Option<SecretAllows> = None;
        let mut out = Vec::new();
        for id in nodes {
            // Every text field, data ones too: a data field is never sent, but it lands in tracked files and agents'
            // context, so a write still warns (security, s22).
            let values = loaded
                .graph
                .node(*id)
                .map(|n| vectors::text_values(&n.to_node(), &loaded.meta.ontology))
                .unwrap_or_default();
            for (field, value) in values {
                let Some(rule) = secrets::field_suspect(&field, &value) else {
                    continue;
                };
                let allows = allows.get_or_insert_with(|| self.secret_allows());
                if !allows.allows(loaded.meta.id, *id, &field, TextHash::of(&value)) {
                    out.push(problems::secret_suspected(&graph, *id, &field, rule.name()));
                }
            }
        }
        out
    }

    /// `umx embeddings allow --node <id> --field <f>`: lets the field's current text go to a hosted embedder on this
    /// machine although the scan flags it. Only that exact text: any change holds it again. Entries for nodes or
    /// texts that are gone from loaded graphs are pruned on every write; a new one that would take the file past
    /// its caps (count or bytes) is refused, never written, so the file always reads (R120-2).
    pub fn allow_secret(&mut self, ctx: &Ctx, node: &str, field: &str) -> Outcome<SecretAllowed> {
        let alias = self.select(ctx, &[Some(node)])?;
        let loaded = self.get(&alias);
        let (graph, id) = (loaded.meta.id, resolve_node(loaded, &alias, node)?);
        let mut out = SecretAllowed {
            alias: alias.clone(),
            node: id,
            field: field.to_string(),
            allowed: false,
            rule: None,
            why: None,
        };
        let values = self.node_values(&alias, id).unwrap_or_default();
        let Some((_, value)) = values.iter().find(|(f, _)| f == field) else {
            let node = self.get(&alias).graph.node(id).map(|n| n.to_node());
            let written = node.is_some_and(|n| {
                vectors::text_values(&n, &self.get(&alias).meta.ontology)
                    .iter()
                    .any(|(f, _)| f == field)
            });
            out.why = Some(if written {
                "the field is data, so it is never embedded or sent"
            } else {
                "the node has no text in that field"
            });
            return Ok(Done::ok(out));
        };
        let Some(rule) = secrets::field_suspect(field, value) else {
            out.why = Some("its text does not look like a secret, so it is not held");
            return Ok(Done::ok(out));
        };
        out.rule = Some(rule.name());
        let file = self.ports.allows.path();
        let refused = |reason: &str| problems::secret_allow_refused(&file, reason);
        let mut allows = match self.ports.allows.read() {
            Ok(None) => SecretAllows::default(),
            Ok(Some(bytes)) => SecretAllows::decode(&bytes).ok_or_else(|| {
                refused("it does not parse or is a newer format (fix or remove it)")
            })?,
            // Deleted while its folder stays: nothing to keep, so a new file is started.
            Err(LocalStateError::Refused { reason, .. }) if reason == REFUSED_MISSING => {
                SecretAllows::default()
            }
            Err(LocalStateError::Refused { reason, .. }) => return Err(refused(reason)),
            Err(LocalStateError::Io(m)) => return Err(refused(&m)),
        };
        self.prune_allows(&mut allows);
        let at = self.now();
        if !allows.add(graph, id, field, TextHash::of(value), &at) {
            out.allowed = true;
            return Ok(Done::ok(out));
        }
        let Some(bytes) = allows.encode_within_caps() else {
            return Err(refused(FULL));
        };
        let mut warnings = Vec::new();
        match self.ports.allows.write(&bytes) {
            Ok(StateWritten::Durable) => {}
            Ok(StateWritten::NotSynced(why)) => {
                warnings.push(problems::local_state_not_durable(&file, &why))
            }
            Err(e) => return Err(refused(&e.to_string())),
        }
        out.allowed = true;
        Ok(Done {
            value: out,
            notes: warnings,
        })
    }

    /// Drops allows whose node or exact text is gone from a loaded graph. Allows for graphs not loaded now are kept.
    fn prune_allows(&self, allows: &mut SecretAllows) {
        let by_id: BTreeMap<Id, &str> = self
            .graphs
            .iter()
            .map(|(alias, l)| (l.meta.id, alias.as_str()))
            .collect();
        allows.allows.retain(|a| {
            let Some(alias) = by_id.get(&a.graph) else {
                return true;
            };
            self.node_values(alias, a.node).is_some_and(|values| {
                values
                    .iter()
                    .any(|(f, v)| *f == a.field && alloc::format!("{}", TextHash::of(v)) == a.hash)
            })
        });
    }
}
