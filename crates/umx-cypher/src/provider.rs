//! The [`GraphProvider`] trait: the read-only property-graph interface the [`crate::executor`]
//! runs queries against. Implement it for your own graph or data structure to make it queryable
//! with this crate's Cypher subset.
//!
//! The model is the standard Cypher property graph: nodes carry one or more **labels** and scalar
//! **properties**, and are connected by typed, directed **relationships**. The executor only ever
//! reads through this trait, so any backend that can answer these questions can be queried.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::value::CypherValue;

/// A node property name the provider resolved once for a query ([`GraphProvider::property_key`]), so each read of it
/// skips matching the name. Opaque to the executor; good only for the provider and query that made it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PropKey(pub u32);

/// A read-only property-graph data source.
///
/// All methods are read-only. Node identity is an associated type so implementers can use whatever
/// handle is natural (an index, an interned id, an enum, …), as long as it is cheap to copy and to
/// order (the executor keys its sets and maps by it, so order, not a hash, decides lookups).
pub trait GraphProvider {
    /// A lightweight handle identifying a node.
    type NodeId: Copy + Ord;

    /// A lightweight handle identifying one edge. Parallel edges have distinct handles.
    type EdgeId: Copy + Ord;

    /// Returns all nodes matching **any** of the given labels. An empty slice matches every node.
    fn scan(&self, labels: &[String]) -> Vec<Self::NodeId>;

    /// Candidates first bound as an end of a relationship pattern, rather than a standalone node pattern.
    fn scan_step_start(&self, labels: &[String]) -> Vec<Self::NodeId> {
        self.scan(labels)
    }

    /// Returns whether `node` has the given label.
    fn matches_label(&self, node: Self::NodeId, label: &str) -> bool;

    /// Returns the names of all relationship types this graph exposes. Used to expand untyped
    /// relationship patterns (`-->`, `-[]->`) and to validate relationship-type names in queries.
    fn relationship_types(&self) -> Vec<String>;

    /// The `rel_type` edges leaving `node`, one entry per edge (parallel edges and self-loops each
    /// listed), as `(edge, the node at the other end)`, in a stable order.
    fn expand(&self, node: Self::NodeId, rel_type: &str) -> Vec<(Self::EdgeId, Self::NodeId)>;

    /// The `rel_type` edges arriving at `node`, as `(edge, the node at the other end)`. Same rules as
    /// [`Self::expand`]; a self-loop is listed here too.
    fn expand_in(&self, node: Self::NodeId, rel_type: &str) -> Vec<(Self::EdgeId, Self::NodeId)>;

    /// The edges leaving `node` (or arriving at it, when `incoming`) whose type is one of `rel_types`,
    /// or of any type [`Self::relationship_types`] lists when `rel_types` is `None` (an untyped step).
    /// The executor calls this once per hop, so a provider can answer from one pass over the node's
    /// adjacency instead of one per type. The default asks [`Self::expand`] or [`Self::expand_in`]
    /// once per type, in type order.
    fn expand_types(
        &self,
        node: Self::NodeId,
        rel_types: Option<&[String]>,
        incoming: bool,
    ) -> Vec<(Self::EdgeId, Self::NodeId)> {
        let all;
        let rel_types = match rel_types {
            Some(types) => types,
            None => {
                all = self.relationship_types();
                &all
            }
        };
        let mut edges = Vec::new();
        for rel in rel_types {
            if incoming {
                edges.extend(self.expand_in(node, rel));
            } else {
                edges.extend(self.expand(node, rel));
            }
        }
        edges
    }

    /// Additional steps read but not returned, such as unavailable external endpoints. Drained once per
    /// expansion and charged before the executor consumes any returned rows.
    fn take_expansion_work(&self) -> u64 {
        0
    }

    /// Relationship types followed only when a step names them (derived relations such as umx's similar_to): never
    /// listed by [`Self::relationship_types`], so an untyped step never follows them, yet a step that names one
    /// reaches the provider's [`Self::expand_types`] like a stored type. Default: none.
    fn named_only_types(&self) -> Vec<String> {
        Vec::new()
    }

    /// Whether relation `rel_type` is symmetric in this graph (`a r b` is `b r a`; D49). The provider
    /// reads its set once per query. A directed step naming one is refused; an untyped directed step
    /// that crosses one runs and warns. Default: none is.
    fn is_symmetric(&self, rel_type: &str) -> bool {
        let _ = rel_type;
        false
    }

    /// Whether some edge has relationship type `name`. The executor asks it for each name of a typed step on every
    /// match (an EXISTS row matches once per row), so a provider should answer without building a list. Default:
    /// [`Self::relationship_types`] contains it.
    fn has_relationship_type(&self, name: &str) -> bool {
        self.relationship_types().iter().any(|t| t == name)
    }

    /// Whether this edge's relation is symmetric where the edge lives, for the untyped-directed warning (D49 R2). A
    /// provider over several graphs answers from the edge's own graph, where [`Self::is_symmetric`] answers for
    /// any. Default: its relation's [`Self::is_symmetric`].
    fn edge_is_symmetric(&self, edge: Self::EdgeId) -> bool {
        self.is_symmetric(&self.edge_label(edge))
    }

    /// Whether [`Self::lookup`] answers equality on node property `prop` from an index. The executor
    /// then starts a path at an endpoint written with that property rather than scanning.
    fn indexed(&self, prop: &str) -> bool {
        let _ = prop;
        false
    }

    /// The nodes whose property `prop` equals `value`, from an index, or `None` when `prop` has none
    /// (the executor then scans). Must equal a scan filtered on `prop = value`.
    fn lookup(&self, prop: &str, value: &CypherValue) -> Option<Vec<Self::NodeId>> {
        let _ = (prop, value);
        None
    }

    fn lookup_step_start(&self, prop: &str, value: &CypherValue) -> Option<Vec<Self::NodeId>> {
        self.lookup(prop, value)
    }

    /// The work units one read of node property `prop` costs (see the executor's `Limits`). A field
    /// the provider decodes from a stored record should cost more than a column read. Default: 1.
    fn property_cost(&self, prop: &str) -> u64 {
        let _ = prop;
        1
    }

    /// How string `a` stands against string `b` when the provider reads both as more than text (umx: two dates,
    /// ws008 TM2), for `=`, `<>`, `<`, `<=`, `>`, `>=`, `IN`, the simple `CASE` and property maps: `Less` wholly
    /// before, `Greater` wholly after, `Equal` overlapping (so `=` holds). `None` compares them as text, as
    /// openCypher does. Free: it charges no work. Default: `None`.
    fn compare_text(&self, a: &str, b: &str) -> Option<Ordering> {
        let _ = (a, b);
        None
    }

    /// The text string `s` sorts by in `ORDER BY`, `min` and `max`, when not itself (umx: the canonical form of a
    /// date written another way). Strings sort by this key, else themselves, then by themselves. Asked once per
    /// value sorted; free: it charges no work. Default: `None` (strings in text order).
    fn text_sort_key(&self, s: &str) -> Option<String> {
        let _ = s;
        None
    }

    /// Whether the provider answers scalar function `name` (lower case) with [`Self::function`]. The executor asks
    /// only for a name none of its own functions has, so a provider can never shadow one. Default: none.
    fn has_function(&self, name: &str) -> bool {
        let _ = name;
        false
    }

    /// Calls provider function `name` (lower case; [`Self::has_function`] said yes) on its evaluated arguments.
    /// `Err` is an execution error with that message. The executor charges one work unit per call. Default: never
    /// reached.
    fn function(&self, name: &str, args: &[CypherValue]) -> Result<CypherValue, String> {
        let _ = args;
        Err(format!("unknown function `{name}`"))
    }

    /// Returns the value of property `prop` on `node`, or [`CypherValue::Null`] if it is absent.
    fn property(&self, node: Self::NodeId, prop: &str) -> CypherValue;

    /// Resolves node property `prop` once for this query: the executor then reads it with [`Self::property_at`] and
    /// charges [`Self::key_cost`]. `None` keeps [`Self::property`] and [`Self::property_cost`] for that name.
    /// Default: `None`.
    fn property_key(&self, prop: &str) -> Option<PropKey> {
        let _ = prop;
        None
    }

    /// The value of the property `key` names on `node`, exactly as [`Self::property`] answers for that name. Called
    /// only with keys [`Self::property_key`] returned, so the default (null) is never reached.
    fn property_at(&self, node: Self::NodeId, key: PropKey) -> CypherValue {
        let _ = (node, key);
        CypherValue::Null
    }

    /// The work one [`Self::property_at`] read costs: must equal [`Self::property_cost`] of the key's name. Default: 1.
    fn key_cost(&self, key: PropKey) -> u64 {
        let _ = key;
        1
    }

    /// Visits the items of the list the property `key` names on `node`, in list order, until `visit` breaks, without
    /// building the list (`x IN n.prop`, `size(n.prop)`). `true` promises the property is a list whose items are
    /// exactly the strings visited; `false` means nothing was visited and the executor reads the value with
    /// [`Self::property_at`] instead. Charges no work (the executor charges [`Self::key_cost`]). Default: `false`.
    fn each_str(
        &self,
        node: Self::NodeId,
        key: PropKey,
        visit: &mut dyn FnMut(&str) -> core::ops::ControlFlow<()>,
    ) -> bool {
        let _ = (node, key, visit);
        false
    }

    /// Returns the value of property `prop` on `edge`, or [`CypherValue::Null`] if it is absent.
    fn edge_property(&self, edge: Self::EdgeId, prop: &str) -> CypherValue;

    /// Returns the edge's relationship type, for `label(r)` and a returned relationship.
    fn edge_label(&self, edge: Self::EdgeId) -> String;

    /// A stable, opaque identifier for `edge`, like [`Self::node_id`] for nodes.
    fn edge_id(&self, edge: Self::EdgeId) -> String;

    /// Returns a stable, opaque identifier for `node`, carried through to returned nodes so
    /// consumers can map a result back to their own object.
    ///
    /// The crate never interprets this string — it only compares it for identity (so distinct
    /// nodes get distinct ids) and never includes it in table or JSON output. Encode whatever the
    /// consumer needs to round-trip a node (for example a type tag plus a primary key).
    fn node_id(&self, node: Self::NodeId) -> String;

    /// Returns the node's primary label, used when a bound node is returned directly.
    fn label(&self, node: Self::NodeId) -> String;

    /// Returns the node's display name, used when a bound node is returned directly.
    fn name(&self, node: Self::NodeId) -> String;
}
