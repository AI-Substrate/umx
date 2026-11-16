//! Graph metadata, nodes and edges as stored in the graph's files. Field order here is file order.

use crate::id::Id;
use crate::ontology::Ontology;
use crate::ports::embedder::{Locality, ModelKey};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// graph.json's `umx` format numbers, one ordered sequence: each means "this umx reads every feature up to here"
/// (graph-storage-expert). A feature's number is written only when the feature (or a later one) is present, and a
/// write never lowers the number a file had. A new feature takes the next free number when its PR merges.
pub const FORMAT_EMBEDDING: u32 = 2;
/// External edge ends.
pub const FORMAT_EXTERNAL_ENDS: u32 = 3;
/// Field roles and whole-node text form 2 (D56, req-97): an older umx would embed data fields, so it must refuse.
pub const FORMAT_FIELD_ROLES: u32 = 4;
/// Resource nodes: a node with a `locator` (ws006 S3, workshop 010 9a).
pub const FORMAT_LOCATORS: u32 = 5;
/// Declared spines: graph.json's `spines` (ws008 TM5, architect). An older umx refuses the graph rather than drop
/// them on its next write.
pub const FORMAT_SPINES: u32 = 6;
/// A renamed or moved graph: graph.json's `former_aliases` and `former_paths` (ws006 S23, req-56, architect). An older
/// umx refuses the graph rather than drop its redirects on its next write.
pub const FORMAT_FORMER: u32 = 7;

/// The newest graph.json format this build reads and writes, as `"umx"`.
pub const FORMAT_VERSION: u32 = FORMAT_FORMER;

/// At most this many former aliases, and as many former paths: a rename past it drops the oldest.
pub const MAX_FORMER: usize = 16;

// Each feature has its own number, in merge order, and the build's format is the last (architect, B1).
const _: () = assert!(
    FORMAT_EMBEDDING < FORMAT_EXTERNAL_ENDS
        && FORMAT_EXTERNAL_ENDS < FORMAT_FIELD_ROLES
        && FORMAT_FIELD_ROLES < FORMAT_LOCATORS
        && FORMAT_LOCATORS < FORMAT_SPINES
        && FORMAT_SPINES < FORMAT_FORMER
        && FORMAT_VERSION == FORMAT_FORMER
);

/// The format the content needs: [`FORMAT_EMBEDDING`] with an `embedding` section, [`FORMAT_FIELD_ROLES`] with a
/// field role or text form 2 or later, [`FORMAT_EXTERNAL_ENDS`] with an external edge end, [`FORMAT_LOCATORS`] with
/// a resource node, [`FORMAT_SPINES`] with a declared spine, [`FORMAT_FORMER`] with a former alias or path. A write
/// never lowers the version a file already had.
impl GraphMeta {
    pub fn needed_format(&self) -> u32 {
        let roles =
            self.ontology.has_roles() || self.embedding.as_ref().is_some_and(|e| e.text_form >= 2);
        if !self.former_aliases.is_empty() || !self.former_paths.is_empty() {
            FORMAT_FORMER
        } else if !self.spines.is_empty() {
            FORMAT_SPINES
        } else if roles {
            FORMAT_FIELD_ROLES
        } else if self.embedding.is_some() {
            FORMAT_EMBEDDING
        } else {
            1
        }
    }

    /// The metadata's need raised by what the graph's edges and nodes hold: the one place each feature's number
    /// is applied.
    pub fn needed_format_for(&self, external_ends: bool, locators: bool) -> u32 {
        let mut format = self.needed_format();
        if external_ends {
            format = format.max(FORMAT_EXTERNAL_ENDS);
        }
        if locators {
            format = format.max(FORMAT_LOCATORS);
        }
        format
    }

    /// External ends require [`FORMAT_EXTERNAL_ENDS`], even without an embedding section.
    pub fn needed_format_for_edges<'a>(&self, edges: impl IntoIterator<Item = &'a Edge>) -> u32 {
        self.needed_format_for(edges.into_iter().any(Edge::has_external_end), false)
    }

    /// The policy graph.json states: `open` when the section or its `policy` key is missing (D32). Only the file's
    /// word; the policy umx acts on also counts this machine's record (see [`crate::local_state`]).
    pub fn file_policy(&self) -> Policy {
        self.embedding.as_ref().map_or(Policy::OPEN, |e| e.policy)
    }

    /// Whether `former_aliases` and `former_paths` are ones umx could have written: at most [`MAX_FORMER`] each, no
    /// repeats, each alias valid and never the live one, each path a repo-relative folder in normal form
    /// (`locator::repo_path`, R241-2). Anything else is a hand edit, refused at load as any bad content is.
    pub fn former_ok(&self) -> bool {
        let unique = |list: &[String]| list.iter().enumerate().all(|(i, a)| !list[..i].contains(a));
        self.former_aliases.len() <= MAX_FORMER
            && self.former_paths.len() <= MAX_FORMER
            && unique(&self.former_aliases)
            && unique(&self.former_paths)
            && self
                .former_aliases
                .iter()
                .all(|a| crate::ValidName::alias(a).is_some() && *a != self.alias)
            && self
                .former_paths
                .iter()
                .all(|p| crate::locator::repo_path(p).is_ok_and(|n| n == *p))
    }
}

/// graph.json: identity, display alias and vocabulary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphMeta {
    pub umx: u32,
    pub id: Id,
    pub alias: String,
    pub title: String,
    pub created: String,
    /// The graph's embedding model and policy (D32); absent until one is chosen or the graph is secured.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_section"
    )]
    pub embedding: Option<GraphEmbedding>,
    pub ontology: Ontology,
    /// The builder's declared spines (ws008 TM5), in the order declared; at most one is `main`. Left out when there
    /// are none, so a graph that declares none keeps its format.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spines: Vec<crate::spine::SpineDecl>,
    /// The aliases this graph had before (ws006 S23, req-56), oldest first: `umx:<former>/<id>` resolves to it
    /// (`redirected`). Left out when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former_aliases: Vec<String>,
    /// The repo-relative folders `umx graph move` moved it from, oldest first, so `umx check` can name the
    /// `root:repo` pointers into them. Left out when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former_paths: Vec<String>,
}

/// graph.json's `embedding` section. When present, all three known keys are written, so a secured or open graph
/// shows in the file. The two settings are written as `policy` alone when they are a legacy pair (`open` = open +
/// any, `secured` = named + local), else as `visibility` and `embedder` with `policy: "secured"`, so an older umx
/// that knows only `policy` reads every other pair as secured (D53, security and architect, plan 071).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawEmbedding", into = "RawEmbedding")]
pub struct GraphEmbedding {
    /// The vector space every vector of this graph is in, `model@dims[+pN]`; `null` until chosen. Never adopted
    /// silently from whatever embedder is configured.
    pub key: Option<ModelKey>,
    /// Version of how umx turns a node into text for its whole-node vector: the N of the `@node.v<N>` field name in
    /// the embedding shards (workshop 008 2.5), so the two never disagree. Separate from the key's `+pN`.
    pub text_form: u32,
    pub policy: Policy,
    /// The similar_to settings, `{"k": 10, "floor": 0.6}` (workshop 008 section 9), as the file has them: absent
    /// means the model's defaults ([`crate::similar::Settings::of`]); a value out of range is warned and the
    /// default used, but kept here as written so a later write never changes it. Written only when someone sets
    /// it.
    pub similarity: Option<Value>,
    /// `umx graph set --related on|off` (req-123, D80): this graph's related check on node add and batch, over the
    /// machine's `related` setting; absent (never set) leaves it to the machine. Written only when someone sets it.
    pub related: Option<bool>,
    /// Keys this build does not know, kept through load and save.
    pub extra: BTreeMap<String, Value>,
}

/// The `embedding` section as the file spells it.
#[derive(Serialize, Deserialize)]
struct RawEmbedding {
    #[serde(default)]
    key: Option<ModelKey>,
    #[serde(default = "first_text_form")]
    text_form: u32,
    #[serde(default)]
    policy: Legacy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    visibility: Option<Visibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    embedder: Option<EmbedderRule>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_value"
    )]
    similarity: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    related: Option<bool>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

impl TryFrom<RawEmbedding> for GraphEmbedding {
    type Error = &'static str;

    fn try_from(raw: RawEmbedding) -> Result<Self, Self::Error> {
        Ok(GraphEmbedding {
            key: raw.key,
            text_form: raw.text_form,
            policy: Policy::read(raw.policy, raw.visibility, raw.embedder)?,
            similarity: raw.similarity,
            related: raw.related,
            extra: raw.extra,
        })
    }
}

impl From<GraphEmbedding> for RawEmbedding {
    fn from(e: GraphEmbedding) -> Self {
        let (policy, visibility, embedder) = e.policy.written();
        RawEmbedding {
            key: e.key,
            text_form: e.text_form,
            policy,
            visibility,
            embedder,
            similarity: e.similarity,
            related: e.related,
            extra: e.extra,
        }
    }
}

/// A present key keeps its value, `null` included, so it is written back as it was.
fn present_value<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

/// A present `embedding` key must hold an object: `null` is refused, never read as "no section" (which would let a
/// format 1 file carry the key and lose it on the next write).
fn present_section<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<GraphEmbedding>, D::Error> {
    GraphEmbedding::deserialize(d).map(Some)
}

fn first_text_form() -> u32 {
    1
}

impl GraphEmbedding {
    /// A new section records the current whole-node text form: a new graph's vectors are all made in it.
    pub fn new(key: Option<ModelKey>, policy: Policy) -> GraphEmbedding {
        GraphEmbedding {
            key,
            text_form: crate::vectors::WHOLE_NODE_VERSION,
            policy,
            similarity: None,
            related: None,
            extra: BTreeMap::new(),
        }
    }
}

/// Whether a graph shows in reads that do not name it (d-20, D53). Ordered from least to most strict.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    #[default]
    Open,
    /// Left out of unscoped search and multi-graph reads; read only when named.
    Named,
}

/// Which embedder instances may get a graph's text (req-46, req-91). Ordered from least to most strict.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbedderRule {
    #[default]
    Any,
    /// Local instances, or hosted ones the operator bound to their endpoint and confirmed at a terminal.
    Trusted,
    /// Only instances whose texts never leave this machine.
    Local,
}

impl EmbedderRule {
    /// The one routing rule (D53): whether an instance with this locality and trust may get the graph's text.
    /// `trusted` is the operator's confirmed statement about a hosted instance; it never widens `Local`.
    pub fn admits(self, locality: Locality, trusted: bool) -> bool {
        match self {
            EmbedderRule::Any => true,
            EmbedderRule::Trusted => locality == Locality::Local || trusted,
            EmbedderRule::Local => locality == Locality::Local,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            EmbedderRule::Any => "any",
            EmbedderRule::Trusted => "trusted",
            EmbedderRule::Local => "local",
        }
    }
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Open => "open",
            Visibility::Named => "named",
        }
    }
}

/// The one word graph.json had before D53, and still writes: `secured` for every pair but open + any.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Legacy {
    #[default]
    Open,
    Secured,
}

/// A graph's two settings (D53). Not totally ordered: one pair can be stricter on one setting and looser on the
/// other, so compare with [`Policy::exceeds`] and combine with [`Policy::join`], never `<`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    pub visibility: Visibility,
    pub embedder: EmbedderRule,
}

/// Why a file's `visibility` and `embedder` keys are refused: they sit beside `policy: "open"` (an older umx that
/// knows only `policy` may have rewritten it), or with a pair `policy` names alone. The codec maps it to
/// [`crate::codec::DecodeReason::PolicyPair`].
pub const POLICY_PAIR_REFUSED: &str = "visibility and embedder are written only beside policy secured for a pair that is not open or secured";

impl Policy {
    pub const OPEN: Policy = Policy {
        visibility: Visibility::Open,
        embedder: EmbedderRule::Any,
    };
    /// Secured as before D53: named, and only local instances (req-46).
    pub const SECURED: Policy = Policy {
        visibility: Visibility::Named,
        embedder: EmbedderRule::Local,
    };

    /// The stricter of the two on each setting.
    pub fn join(self, other: Policy) -> Policy {
        Policy {
            visibility: self.visibility.max(other.visibility),
            embedder: self.embedder.max(other.embedder),
        }
    }

    /// Whether `self` is stricter than `other` on either setting (so moving from `self` to `other` loosens one).
    pub fn exceeds(self, other: Policy) -> bool {
        self.visibility > other.visibility || self.embedder > other.embedder
    }

    /// `open`, `secured`, or the pair spelled `named+trusted`, for messages.
    pub fn name(self) -> &'static str {
        match (self.visibility, self.embedder) {
            (Visibility::Open, EmbedderRule::Any) => "open",
            (Visibility::Named, EmbedderRule::Local) => "secured",
            (Visibility::Open, EmbedderRule::Trusted) => "open+trusted",
            (Visibility::Open, EmbedderRule::Local) => "open+local",
            (Visibility::Named, EmbedderRule::Any) => "named+any",
            (Visibility::Named, EmbedderRule::Trusted) => "named+trusted",
        }
    }

    fn of_legacy(word: Legacy) -> Policy {
        match word {
            Legacy::Open => Policy::OPEN,
            Legacy::Secured => Policy::SECURED,
        }
    }

    /// The pair a file states. Each setting is its own key if present, else what `policy` means. A file that
    /// spells either key must say `policy: "secured"` and hold a pair that is not a legacy one, which is all the
    /// writer ever writes: anything else is refused (security, plan 071), so a file can never read as secured to
    /// an older umx or a person while this one reads it as open.
    pub fn read(
        word: Legacy,
        visibility: Option<Visibility>,
        embedder: Option<EmbedderRule>,
    ) -> Result<Policy, &'static str> {
        let base = Policy::of_legacy(word);
        if visibility.is_none() && embedder.is_none() {
            return Ok(base);
        }
        let pair = Policy {
            visibility: visibility.unwrap_or(base.visibility),
            embedder: embedder.unwrap_or(base.embedder),
        };
        if word != Legacy::Secured || pair == Policy::OPEN || pair == Policy::SECURED {
            return Err(POLICY_PAIR_REFUSED);
        }
        Ok(pair)
    }

    /// What the file says for this pair: `policy` alone for a legacy pair, else both keys with `policy: "secured"`.
    pub fn written(self) -> (Legacy, Option<Visibility>, Option<EmbedderRule>) {
        match self {
            Policy::OPEN => (Legacy::Open, None, None),
            Policy::SECURED => (Legacy::Secured, None, None),
            _ => (Legacy::Secured, Some(self.visibility), Some(self.embedder)),
        }
    }
}

/// One line of nodes.jsonl. Custom fields follow the standard ones, sorted by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "NodeLine")]
pub struct Node {
    pub id: Id,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub status: String,
    pub note: String,
    pub evidence: Vec<Value>,
    pub status_history: Vec<Value>,
    pub created: String,
    pub updated: String,
    pub created_by: String,
    /// Where a resource node's thing lives (ws006 S3, req-77): right after `created_by`, before the session keys
    /// (graph-storage-expert, workshop 010 9a). A node without one keeps its bytes. A stored `locator` that is not a
    /// locator is none here and kept in `fields` (see [`NodeLine`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<crate::Locator>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_id"
    )]
    pub authored_by_session: Option<Id>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_id"
    )]
    pub edited_by_session: Option<Id>,
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

/// One line of edges.jsonl. Incoming edges are never stored; umx-graph works them out at load. `evidence` (req-124,
/// ws006 S5b) sits after the standard keys and before the custom fields, and is left out when empty. A line's
/// `evidence` value that is not an evidence list ([`is_evidence_list`]) stays a custom field of that name and is never
/// read as evidence, so nothing is lost (graph-storage-expert's layout); a record never holds both.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "EdgeInput")]
pub struct Edge {
    pub id: Id,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_id"
    )]
    pub from_graph: Option<Id>,
    pub from: Id,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_id"
    )]
    pub to_graph: Option<Id>,
    pub to: Id,
    pub created: String,
    pub created_by: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_id"
    )]
    pub authored_by_session: Option<Id>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_id"
    )]
    pub edited_by_session: Option<Id>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Value>,
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

/// An edge line's other keys. A repeated `evidence` key is serde's `duplicate field` error, as it is for the
/// standard keys and for a node's `evidence` (graph-storage-expert); other keys keep serde's map rule.
struct RawFields(BTreeMap<String, Value>);

impl<'de> Deserialize<'de> for RawFields {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<RawFields, D::Error> {
        struct Fields;
        impl<'de> serde::de::Visitor<'de> for Fields {
            type Value = RawFields;
            fn expecting(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
                f.write_str("an edge's fields")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<RawFields, A::Error> {
                let mut fields = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    let repeated_evidence = key == EVIDENCE && fields.contains_key(EVIDENCE);
                    if repeated_evidence {
                        return Err(serde::de::Error::duplicate_field(EVIDENCE));
                    }
                    fields.insert(key, value);
                }
                Ok(RawFields(fields))
            }
        }
        d.deserialize_map(Fields)
    }
}

/// The edge line key that holds evidence.
pub const EVIDENCE: &str = "evidence";

/// Whether `list` is an evidence list an edge line may hold in its `evidence` slot: at least one pointer, each an
/// object with a string `source` (the shape node and edge writes store). Anything else on an edge line is kept as a
/// custom field, warned `evidence_field_kept`, and never read as evidence.
pub fn is_evidence_list(list: &[Value]) -> bool {
    !list.is_empty()
        && list
            .iter()
            .all(|p| p.get("source").is_some_and(Value::is_string))
}

/// An evidence list from the canonical bytes umx-graph keeps for an edge's slot; empty if they do not parse (they
/// always do: umx-graph writes them).
pub fn decode_evidence(bytes: &[u8]) -> Vec<Value> {
    serde_json::from_slice(bytes).unwrap_or_default()
}

/// One status move's cited pointer as `umx who` reads it (plan 188): the session its `by_session` names (the
/// adder, or a redacted entry's redactor) and whether it is a redacted entry (`redacted: true`, never another value).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveEvidence {
    pub by_session: Option<String>,
    pub redacted: bool,
}

/// [`MoveEvidence`] for each move of a `status_history` array that cites a pointer, read typed so no value tree is
/// built for the moves' other keys (a read over every node's history, graph-storage). Moves and pointers are read from
/// JSON objects only: serde would read a derived struct from an array, positionally, and invent a `by_session` the
/// value read never finds (review, round 2). None when the bytes do not take that shape (a hand edit: a move
/// or pointer that is not an object, a `by_session` that is not text, a repeated key), for the caller to read them as
/// values instead.
pub fn status_move_evidence(bytes: &[u8]) -> Option<Vec<MoveEvidence>> {
    let moves: Vec<status_moves::Move> = serde_json::from_slice(bytes).ok()?;
    Some(moves.into_iter().filter_map(|m| m.0).collect())
}

/// Map-only readers for [`status_move_evidence`].
mod status_moves {
    use super::MoveEvidence;
    use alloc::string::String;
    use core::fmt;
    use serde::Deserialize;
    use serde::de::{self, Deserializer, IgnoredAny, MapAccess, Visitor};

    /// A key, read without allocating: the names a read needs, anything else ignored.
    enum Key {
        Evidence,
        BySession,
        Redacted,
        Other,
    }

    impl<'de> Deserialize<'de> for Key {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = Key;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a key")
                }
                fn visit_str<E: de::Error>(self, s: &str) -> Result<Key, E> {
                    Ok(match s {
                        "evidence" => Key::Evidence,
                        "by_session" => Key::BySession,
                        "redacted" => Key::Redacted,
                        _ => Key::Other,
                    })
                }
            }
            d.deserialize_identifier(V)
        }
    }

    /// One move: an object, its `evidence` a pointer object or null.
    pub(super) struct Move(pub(super) Option<MoveEvidence>);

    impl<'de> Deserialize<'de> for Move {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = Move;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a status move object")
                }
                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Move, A::Error> {
                    let mut evidence: Option<Option<Cited>> = None;
                    while let Some(key) = map.next_key::<Key>()? {
                        match key {
                            Key::Evidence if evidence.is_some() => {
                                return Err(de::Error::duplicate_field("evidence"));
                            }
                            Key::Evidence => evidence = Some(map.next_value()?),
                            _ => {
                                map.next_value::<IgnoredAny>()?;
                            }
                        }
                    }
                    Ok(Move(evidence.flatten().map(|c| c.0)))
                }
            }
            d.deserialize_map(V)
        }
    }

    /// A cited pointer: an object; `by_session` text or null, `redacted` a boolean or null.
    struct Cited(MoveEvidence);

    impl<'de> Deserialize<'de> for Cited {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = Cited;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("an evidence pointer object")
                }
                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Cited, A::Error> {
                    let mut by_session: Option<Option<String>> = None;
                    let mut redacted: Option<Option<bool>> = None;
                    while let Some(key) = map.next_key::<Key>()? {
                        match key {
                            Key::BySession if by_session.is_some() => {
                                return Err(de::Error::duplicate_field("by_session"));
                            }
                            Key::BySession => by_session = Some(map.next_value()?),
                            Key::Redacted if redacted.is_some() => {
                                return Err(de::Error::duplicate_field("redacted"));
                            }
                            Key::Redacted => redacted = Some(map.next_value()?),
                            _ => {
                                map.next_value::<IgnoredAny>()?;
                            }
                        }
                    }
                    Ok(Cited(MoveEvidence {
                        by_session: by_session.flatten(),
                        redacted: redacted.flatten() == Some(true),
                    }))
                }
            }
            d.deserialize_map(V)
        }
    }
}

fn present_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Id>, D::Error> {
    Id::deserialize(d).map(Some)
}

/// A node line as read. `locator` is taken raw: a value that is not a locator (a custom field named `locator` from
/// before it was reserved, `null`, a newer umx's kind) loads as that custom field, kept and written back, and the
/// load warns `locator_field_kept`; it never fails the load (graph-storage-expert, S241-1).
#[derive(Deserialize)]
struct NodeLine {
    id: Id,
    #[serde(rename = "type")]
    kind: String,
    title: String,
    status: String,
    note: String,
    evidence: Vec<Value>,
    status_history: Vec<Value>,
    created: String,
    updated: String,
    created_by: String,
    #[serde(default, deserialize_with = "present_value")]
    locator: Option<Value>,
    #[serde(default, deserialize_with = "present_value")]
    authored_by_session: Option<Value>,
    #[serde(default, deserialize_with = "present_value")]
    edited_by_session: Option<Value>,
    #[serde(flatten)]
    fields: BTreeMap<String, Value>,
}

/// An edge line as read: session ids taken raw (a bad one is kept as a custom field), and `evidence` among the
/// custom fields; [`Edge`] lifts it out when it is a list.
#[derive(Deserialize)]
struct EdgeInput {
    id: Id,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, deserialize_with = "present_id")]
    from_graph: Option<Id>,
    from: Id,
    #[serde(default, deserialize_with = "present_id")]
    to_graph: Option<Id>,
    to: Id,
    created: String,
    created_by: String,
    #[serde(default, deserialize_with = "present_value")]
    authored_by_session: Option<Value>,
    #[serde(default, deserialize_with = "present_value")]
    edited_by_session: Option<Value>,
    #[serde(flatten)]
    fields: RawFields,
}

fn session_or_kept(
    value: Option<Value>,
    fields: &mut BTreeMap<String, Value>,
    key: &str,
) -> Option<Id> {
    let value = value?;
    if let Some(id) = value.as_str().and_then(|s| s.parse().ok()) {
        Some(id)
    } else {
        fields.insert(key.into(), value);
        None
    }
}

impl From<NodeLine> for Node {
    fn from(mut input: NodeLine) -> Self {
        let locator = input.locator.and_then(|raw| {
            let locator = crate::Locator::from_value(&raw);
            if locator.is_none() {
                input
                    .fields
                    .insert(crate::locator::LOCATOR_FIELD.into(), raw);
            }
            locator
        });
        let authored_by_session = session_or_kept(
            input.authored_by_session,
            &mut input.fields,
            "authored_by_session",
        );
        let edited_by_session = session_or_kept(
            input.edited_by_session,
            &mut input.fields,
            "edited_by_session",
        );
        let node = Self {
            id: input.id,
            kind: input.kind,
            title: input.title,
            status: input.status,
            note: input.note,
            evidence: input.evidence,
            status_history: input.status_history,
            created: input.created,
            updated: input.updated,
            created_by: input.created_by,
            locator,
            authored_by_session,
            edited_by_session,
            fields: input.fields,
        };
        node.assert_provenance_fields();
        node
    }
}

impl From<EdgeInput> for Edge {
    fn from(input: EdgeInput) -> Self {
        let mut fields = input.fields.0;
        let authored_by_session = session_or_kept(
            input.authored_by_session,
            &mut fields,
            "authored_by_session",
        );
        let edited_by_session =
            session_or_kept(input.edited_by_session, &mut fields, "edited_by_session");
        let evidence = match fields.remove(EVIDENCE) {
            Some(Value::Array(list)) if is_evidence_list(&list) => list,
            Some(other) => {
                fields.insert(EVIDENCE.into(), other);
                Vec::new()
            }
            None => Vec::new(),
        };
        let edge = Self {
            id: input.id,
            kind: input.kind,
            from_graph: input.from_graph,
            from: input.from,
            to_graph: input.to_graph,
            to: input.to,
            created: input.created,
            created_by: input.created_by,
            authored_by_session,
            edited_by_session,
            evidence,
            fields,
        };
        edge.assert_provenance_fields();
        edge
    }
}

impl Node {
    /// Whether the node holds a kept custom field named `locator` (one that is not a locator): such a node cannot
    /// be made a resource until the field is renamed (`locator_field_invalid`).
    pub fn has_kept_locator_field(&self) -> bool {
        self.fields.contains_key(crate::locator::LOCATOR_FIELD)
    }
}

impl Edge {
    pub fn has_external_end(&self) -> bool {
        self.from_graph.is_some() || self.to_graph.is_some()
    }

    /// The distinct `kind`s in `evidence`, sorted (derived and never stored, as on nodes).
    pub fn evidence_kinds(&self) -> Vec<&str> {
        let kinds: BTreeSet<&str> = self
            .evidence
            .iter()
            .filter_map(|e| e.get("kind")?.as_str())
            .collect();
        kinds.into_iter().collect()
    }

    /// A custom `evidence` field that is not an evidence list (an older edge's own field): kept, never read as
    /// evidence.
    pub fn invalid_evidence_field(&self) -> Option<&Value> {
        self.fields.get(EVIDENCE)
    }

    pub fn record_author(&mut self, session: Id) {
        self.fields.remove("authored_by_session");
        self.authored_by_session = Some(session);
        self.assert_provenance_fields();
    }

    pub fn record_edit(&mut self, session: Id) {
        self.fields.remove("edited_by_session");
        self.edited_by_session = Some(session);
        self.assert_provenance_fields();
    }

    pub fn assert_provenance_fields(&self) {
        debug_assert!(
            self.authored_by_session.is_none() || !self.fields.contains_key("authored_by_session"),
            "typed and raw authored_by_session overlap"
        );
        debug_assert!(
            self.edited_by_session.is_none() || !self.fields.contains_key("edited_by_session"),
            "typed and raw edited_by_session overlap"
        );
    }
}

impl Node {
    pub fn record_author(&mut self, session: Id) {
        self.fields.remove("authored_by_session");
        self.authored_by_session = Some(session);
        self.record_edit(session);
    }

    pub fn record_edit(&mut self, session: Id) {
        self.fields.remove("edited_by_session");
        self.edited_by_session = Some(session);
        self.assert_provenance_fields();
    }

    pub fn assert_provenance_fields(&self) {
        debug_assert!(
            self.authored_by_session.is_none() || !self.fields.contains_key("authored_by_session"),
            "typed and raw authored_by_session overlap"
        );
        debug_assert!(
            self.edited_by_session.is_none() || !self.fields.contains_key("edited_by_session"),
            "typed and raw edited_by_session overlap"
        );
    }

    /// The distinct `kind`s in `evidence`, sorted (workshop 007 rule 4: derived and read-only, never stored).
    /// Evidence without a string `kind` (a hand edit) adds nothing.
    pub fn evidence_kinds(&self) -> Vec<&str> {
        let kinds: BTreeSet<&str> = self
            .evidence
            .iter()
            .filter_map(|e| e.get("kind")?.as_str())
            .collect();
        kinds.into_iter().collect()
    }

    /// A standard or custom field's value as JSON, for filters and list columns. `address` and `graph` are
    /// added by the caller, which knows the alias.
    pub fn field(&self, name: &str) -> Option<Value> {
        Some(match name {
            "id" => Value::String(self.id.to_text()),
            "type" => Value::String(self.kind.clone()),
            "title" => Value::String(self.title.clone()),
            "status" => Value::String(self.status.clone()),
            "note" => Value::String(self.note.clone()),
            "evidence" => Value::Array(self.evidence.clone()),
            "evidence_kinds" => Value::Array(
                self.evidence_kinds()
                    .into_iter()
                    .map(|k| Value::String(k.into()))
                    .collect(),
            ),
            "status_history" => Value::Array(self.status_history.clone()),
            "created" => Value::String(self.created.clone()),
            "updated" => Value::String(self.updated.clone()),
            "created_by" => Value::String(self.created_by.clone()),
            "authored_by_session" => {
                return self
                    .authored_by_session
                    .map(|id| Value::String(id.to_text()))
                    .or_else(|| self.fields.get(name).cloned());
            }
            "edited_by_session" => {
                return self
                    .edited_by_session
                    .map(|id| Value::String(id.to_text()))
                    .or_else(|| self.fields.get(name).cloned());
            }
            name if name.starts_with("locator")
                && crate::locator::LOCATOR_NAMES.contains(&name) =>
            {
                return self.locator.as_ref().and_then(|l| l.field(name));
            }
            other => return self.fields.get(other).cloned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_move_evidence_reads_cited_pointers_typed_or_refuses_the_shape() {
        let cited = |by: &str, redacted: bool| MoveEvidence {
            by_session: Some(by.into()),
            redacted,
        };
        let history = br#"[{"at":"t","to":"a"},
            {"\u0065vidence":{"source":"a.md","by_session":"S1","redacted":false},"to":"b"},
            {"evidence":{"kind":null,"redacted":true,"by_session":"S2","reason":"r"}},
            {"evidence":{"by_session":"S3","redacted":null}},
            {"evidence":null}]"#;
        assert_eq!(
            status_move_evidence(history),
            Some(alloc::vec![
                cited("S1", false),
                cited("S2", true),
                cited("S3", false)
            ])
        );
        assert_eq!(status_move_evidence(b"[]"), Some(Vec::new()));
        for refused in [
            br#"[{"evidence":{"redacted":"no","by_session":"S1"}}]"#.as_slice(),
            br#"[{"evidence":{"by_session":7}}]"#.as_slice(),
            br#"[{"evidence":"docs/a.md"}]"#.as_slice(),
            // Arrays are never read positionally (review, round 2).
            br#"[{"evidence":["S1",true]}]"#.as_slice(),
            br#"[[{"by_session":"S1","redacted":true}]]"#.as_slice(),
            br#"[{"evidence":{"by_session":"S1"},"evidence":null}]"#.as_slice(),
        ] {
            assert_eq!(status_move_evidence(refused), None);
        }
    }

    #[test]
    fn editing_supersedes_bad_editor_but_never_invents_original_authorship() {
        let text = alloc::format!(
            "{{\"id\":\"{}\",\"type\":\"problem\",\"title\":\"t\",\"status\":\"open\",\"note\":\"\",\"evidence\":[],\"status_history\":[],\"created\":\"\",\"updated\":\"\",\"created_by\":\"t\",\"locator\":{{\"kind\":\"file\",\"format\":\"markdown\",\"source\":\"docs/source.md\"}},\"authored_by_session\":null,\"edited_by_session\":\"bad\"}}",
            Id(1)
        );
        let mut node: Node = serde_json::from_str(&text).unwrap();
        assert_eq!(
            node.locator,
            Some(crate::Locator::parse("docs/source.md").unwrap())
        );
        assert_eq!(node.authored_by_session, None);
        assert_eq!(node.edited_by_session, None);
        assert_eq!(node.fields["authored_by_session"], Value::Null);
        assert_eq!(node.fields["edited_by_session"], json!("bad"));
        node.record_edit(Id(2));
        assert_eq!(node.authored_by_session, None);
        assert_eq!(node.edited_by_session, Some(Id(2)));
        assert_eq!(node.fields["authored_by_session"], Value::Null);
        assert!(!node.fields.contains_key("edited_by_session"));
        let encoded = serde_json::to_string(&node).unwrap();
        assert_eq!(encoded.matches("\"authored_by_session\":").count(), 1);
        assert_eq!(encoded.matches("\"edited_by_session\":").count(), 1);
        assert_eq!(serde_json::from_str::<Node>(&encoded).unwrap(), node);
    }

    #[test]
    fn evidence_kinds_are_the_distinct_kinds_in_evidence() {
        let mut node = Node {
            id: Id(1),
            kind: "problem".into(),
            title: "t".into(),
            status: "open".into(),
            note: String::new(),
            evidence: Vec::new(),
            status_history: Vec::new(),
            created: String::new(),
            updated: String::new(),
            created_by: String::new(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields: BTreeMap::new(),
        };
        assert_eq!(node.field("evidence_kinds"), Some(json!([])));
        node.evidence = alloc::vec![
            json!({ "source": "a", "kind": "room" }),
            json!({ "source": "b", "kind": "analyst" }),
            json!({ "source": "c", "kind": "room" }),
            json!({ "source": "d" }),
        ];
        assert_eq!(
            node.field("evidence_kinds"),
            Some(json!(["analyst", "room"]))
        );
    }
}
