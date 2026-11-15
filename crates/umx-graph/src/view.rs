//! Views over the graph (workshop 008, section 2.8). Callers outside this crate read the graph only through these,
//! so the layout behind them can change without touching a caller. From s04 a view is the graph plus a handle.

use crate::Graph;
use crate::cols::field_in;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use umx_core::{Edge, Id, Node, Value};

/// A node handle, made only by `Graph`: the node's position in id order. Good only for the `&Graph` borrow it came
/// from: never store one across a write, never serialise it. Handle order is id order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeIx(pub(crate) u32);

impl NodeIx {
    /// The handle as a dense index below `Graph::node_bound()`, for bitsets and per-node arrays.
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// An edge handle, made only by `Graph`. Same rules as `NodeIx`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct EdgeIx(pub(crate) u32);

/// One stored endpoint: a local handle or an unresolved graph id and node id.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EdgeEnd {
    Local(NodeIx),
    External(Id, Id),
}

impl EdgeIx {
    /// The handle as a dense index below `Graph::edge_count()`.
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A relation or node type name as the graph holds it, made only by `Graph`. Cheap to copy and compare. Good only
/// for the `&Graph` borrow it came from, like a handle. Not `Ord` on purpose: sort by `as_str` when order matters.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Kind<'a>(pub(crate) &'a str);

impl<'a> Kind<'a> {
    #[inline]
    pub fn as_str(self) -> &'a str {
        self.0
    }
}

/// One edge seen from one of its ends: the edge, the node at the other end and the relation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Step<'a> {
    pub edge: EdgeIx,
    pub other: NodeIx,
    pub kind: Kind<'a>,
}

/// The names `field` answers before custom fields (`Node::field`'s standard names).
pub(crate) fn is_standard(name: &str) -> bool {
    matches!(
        name,
        "id" | "type"
            | "title"
            | "status"
            | "note"
            | "evidence"
            | "evidence_kinds"
            | "status_history"
            | "created"
            | "updated"
            | "created_by"
            | "authored_by_session"
            | "edited_by_session"
    ) || umx_core::locator::LOCATOR_NAMES.contains(&name)
}

/// A read-only view of one node.
#[derive(Clone, Copy)]
pub struct NodeRef<'a> {
    pub(crate) g: &'a Graph,
    pub(crate) h: u32,
}

impl<'a> NodeRef<'a> {
    #[inline]
    fn row(self) -> usize {
        self.h as usize
    }

    #[inline]
    pub fn id(self) -> Id {
        self.g.ids[self.row()]
    }

    #[inline]
    pub fn ix(self) -> NodeIx {
        NodeIx(self.h)
    }

    #[inline]
    pub fn kind(self) -> &'a str {
        self.g.names.text(self.g.n_kind.get(self.row()))
    }

    #[inline]
    pub fn title(self) -> &'a str {
        self.g.title(self.row())
    }

    #[inline]
    pub fn status(self) -> &'a str {
        self.g.names.text(self.g.n_status.get(self.row()))
    }

    #[inline]
    pub fn created_by(self) -> &'a str {
        self.g.names.text(self.g.n_by.get(self.row()))
    }

    #[inline]
    pub fn created(self) -> String {
        self.g.n_created.get(self.row(), self.id())
    }

    /// `created` as whole seconds since 1970, read from the time column with no text built: `Some` when it is in the
    /// form umx writes (`YYYY-MM-DDTHH:MM:SSZ`, from 1970), `None` for any other text (read `created` then).
    #[inline]
    pub fn created_secs(self) -> Option<u32> {
        self.g.n_created.secs(self.row())
    }

    #[inline]
    pub fn updated(self) -> String {
        self.g.n_updated.get(self.row(), self.id())
    }

    pub fn authored_by_session(self) -> Option<Id> {
        self.g.provenance.authored.get(self.row())
    }

    pub fn edited_by_session(self) -> Option<Id> {
        self.g.provenance.edited.get(self.row())
    }

    /// A standard or custom field, with the same names and values as `Node::field`.
    pub fn field(self, name: &str) -> Option<Value> {
        let text = |s: &str| Some(Value::String(s.into()));
        match name {
            "id" => Some(Value::String(self.id().to_text())),
            "type" => text(self.kind()),
            "title" => text(self.title()),
            "status" => text(self.status()),
            "created" => Some(Value::String(self.created())),
            "updated" => Some(Value::String(self.updated())),
            "created_by" => text(self.created_by()),
            "authored_by_session" => self
                .authored_by_session()
                .map(|id| Value::String(id.to_text()))
                .or_else(|| self.custom_field(name)),
            "edited_by_session" => self
                .edited_by_session()
                .map(|id| Value::String(id.to_text()))
                .or_else(|| self.custom_field(name)),
            "evidence_kinds" => Some(Value::Array(
                self.evidence_kinds()
                    .map(|k| Value::String(k.into()))
                    .collect(),
            )),
            // Cold text: decoded from the line, so the answer is `Node::field`'s by construction.
            "note" | "evidence" | "status_history" => self.to_node().field(name),
            // Kind and format are hot (ws006 S3); the other locator parts stay cold in the line.
            "locator_kind" => self
                .g
                .locator_kind(self.ix())
                .map(|(kind, _)| Value::String(kind.as_str().into())),
            "locator_format" => self
                .g
                .locator_kind(self.ix())
                .and_then(|(_, format)| format)
                .map(|f| Value::String(f.as_str().into())),
            // `locator` answers nothing in a read, and the passage parts have no value until S6 (`Locator::field`):
            // null without decoding the line (F186).
            "locator" | "locator_of" | "locator_selector" => None,
            name if umx_core::locator::LOCATOR_NAMES.contains(&name) => {
                self.g.locator_kind(self.ix())?;
                self.to_node().field(name)
            }
            other => self.custom_field(other),
        }
    }

    /// A custom (not standard) field only: from the field's column, or from the node's line for a list, object or
    /// null.
    pub fn custom_field(self, name: &str) -> Option<Value> {
        self.g.custom_value(self.row(), name)
    }

    /// The names of the custom fields this node holds, in name order.
    pub fn custom_field_names(self) -> impl Iterator<Item = &'a str> {
        let g = self.g;
        g.field_names(self.row()).map(move |f| g.names.text(f))
    }

    /// The distinct string `kind`s of this node's evidence items, in name order (`Node::evidence_kinds`), from the
    /// evidence kinds forward column (ws003 s29): no search and no line decode, O(kinds of the node).
    pub fn evidence_kinds(self) -> impl Iterator<Item = &'a str> {
        self.g.evidence_kinds_of(self.h)
    }

    /// The whole record, owned, decoded from the node's line. The only way to the note, evidence and status history
    /// besides `field`.
    pub fn to_node(self) -> Node {
        self.g.node_record(self.row())
    }

    /// The node's line as nodes.jsonl holds it (no newline): the bytes the next write keeps while umx does not change
    /// the node, so a hand edit's duplicate member, which the decoded record drops, is still in it (F203 N2).
    pub fn line(self) -> &'a [u8] {
        self.g.line(self.row())
    }
}

impl core::fmt::Debug for NodeRef<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("NodeRef").field(&self.id()).finish()
    }
}

/// A read-only view of one edge.
#[derive(Clone, Copy)]
pub struct EdgeRef<'a> {
    pub(crate) g: &'a Graph,
    pub(crate) h: u32,
}

impl<'a> EdgeRef<'a> {
    #[inline]
    fn row(self) -> usize {
        self.h as usize
    }

    #[inline]
    pub fn id(self) -> Id {
        self.g.e_ids[self.row()]
    }

    #[inline]
    pub fn ix(self) -> EdgeIx {
        EdgeIx(self.h)
    }

    #[inline]
    pub fn kind(self) -> &'a str {
        self.g.names.text(self.g.e_kind.get(self.row()))
    }

    #[inline]
    /// The bare node id. It names a node in this graph only when `from_graph()` is `None`.
    pub fn from(self) -> Id {
        self.g.end_parts(self.g.e_from[self.row()]).1
    }

    #[inline]
    /// The bare node id. It names a node in this graph only when `to_graph()` is `None`.
    pub fn to(self) -> Id {
        self.g.end_parts(self.g.e_to[self.row()]).1
    }

    #[inline]
    pub fn from_graph(self) -> Option<Id> {
        self.g.end_parts(self.g.e_from[self.row()]).0
    }

    #[inline]
    pub fn to_graph(self) -> Option<Id> {
        self.g.end_parts(self.g.e_to[self.row()]).0
    }

    #[inline]
    pub fn from_ix(self) -> Option<NodeIx> {
        self.from_graph()
            .is_none()
            .then(|| NodeIx(self.g.e_from[self.row()]))
    }

    #[inline]
    pub fn to_ix(self) -> Option<NodeIx> {
        self.to_graph()
            .is_none()
            .then(|| NodeIx(self.g.e_to[self.row()]))
    }

    #[inline]
    pub fn created_by(self) -> &'a str {
        self.g.names.text(self.g.e_by.get(self.row()))
    }

    #[inline]
    pub fn created(self) -> String {
        self.g.e_created.get(self.row(), self.id())
    }

    pub fn authored_by_session(self) -> Option<Id> {
        self.g.provenance.edge_authored.get(self.row())
    }

    /// The local session that last edited this edge (the project manager B); None for an edge never edited.
    pub fn edited_by_session(self) -> Option<Id> {
        self.g.provenance.edge_edited.get(self.id())
    }

    /// A standard (`id`, `type`, `from`, `to`, `created`, `created_by`) or custom field as JSON.
    pub fn field(self, name: &str) -> Option<Value> {
        Some(match name {
            "id" => Value::String(self.id().to_text()),
            "type" => Value::String(self.kind().into()),
            "from" => Value::String(self.from().to_text()),
            "from_graph" => return self.from_graph().map(|id| Value::String(id.to_text())),
            "to" => Value::String(self.to().to_text()),
            "to_graph" => return self.to_graph().map(|id| Value::String(id.to_text())),
            "created" => Value::String(self.created()),
            "created_by" => Value::String(self.created_by().into()),
            "authored_by_session" => {
                return self
                    .authored_by_session()
                    .map(|id| Value::String(id.to_text()))
                    .or_else(|| field_in(self.g.e_fields.get(&self.id())?, name).cloned());
            }
            "edited_by_session" => {
                return self
                    .edited_by_session()
                    .map(|id| Value::String(id.to_text()))
                    .or_else(|| field_in(self.g.e_fields.get(&self.id())?, name).cloned());
            }
            other => return field_in(self.g.e_fields.get(&self.id())?, other).cloned(),
        })
    }

    /// The edge's evidence pointers (req-124); empty when it has none.
    pub fn evidence(self) -> Vec<Value> {
        self.g.edge_evidence(self.id())
    }

    pub fn to_edge(self) -> Edge {
        self.g.edge_record(self.row())
    }
}

impl core::fmt::Debug for EdgeRef<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("EdgeRef").field(&self.id()).finish()
    }
}

/// A set of nodes of one graph: a bit per node handle. Iterated in id order. Good only for the `&Graph` borrow it
/// came from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeSet<'a> {
    ids: &'a [Id],
    bits: Vec<u64>,
    len: usize,
}

impl<'a> NodeSet<'a> {
    pub(crate) fn empty(ids: &'a [Id]) -> NodeSet<'a> {
        NodeSet {
            ids,
            bits: vec![0; ids.len().div_ceil(64)],
            len: 0,
        }
    }

    /// Adds handle `h`; true if it was not there.
    pub(crate) fn insert(&mut self, h: u32) -> bool {
        let (w, b) = (h as usize / 64, 1u64 << (h % 64));
        let new = self.bits[w] & b == 0;
        self.bits[w] |= b;
        self.len += usize::from(new);
        new
    }

    #[inline]
    pub(crate) fn has(&self, h: u32) -> bool {
        self.bits[h as usize / 64] & (1 << (h % 64)) != 0
    }

    #[inline]
    pub fn contains(&self, id: Id) -> bool {
        self.ids
            .binary_search(&id)
            .is_ok_and(|h| self.has(h as u32))
    }

    #[inline]
    pub fn contains_ix(&self, ix: NodeIx) -> bool {
        ix.index() < self.ids.len() && self.has(ix.0)
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The members' ids, in id order.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = Id> + '_ {
        self.bits.iter().enumerate().flat_map(move |(w, &word)| {
            let mut word = word;
            core::iter::from_fn(move || {
                (word != 0).then(|| {
                    let b = word.trailing_zeros() as usize;
                    word &= word - 1;
                    self.ids[w * 64 + b]
                })
            })
        })
    }
}
