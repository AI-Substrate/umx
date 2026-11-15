//! ws008 TM4 (req-148, D71): `--raised-in <anchor>` on `umx node add` and `umx batch`. Each node the write adds gets a
//! `raised_in` edge to the anchor, in the same write as the node. The anchor is resolved as `umx edge add` resolves
//! an end ([`GraphStore::write_end`]), so it is refused, or warned about, exactly as edge add's would be.

use super::edge::new_edge;
use super::external::{StoredEnd, set_ends};
use super::{GraphStore, alias_name};
use crate::problems;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::standard::{RAISED_IN, std_relation};
use umx_core::{Edge, Id, Ontology, Problem, ValidName};

/// A resolved anchor: the stored end, and the address a reply names it by.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Anchor {
    pub end: StoredEnd,
    pub address: String,
}

impl GraphStore {
    /// The anchor `text` names, for a node added to `alias`, and its warnings: edge add's end warnings, then
    /// `relation_endpoint_unexpected` when its type is one raised_in does not expect. A far anchor's type is read only
    /// from an open graph (d-20): a secured one says nothing of its node.
    pub(super) fn raised_in_anchor(
        &self,
        alias: &str,
        text: &str,
    ) -> Result<(Anchor, Vec<Problem>), Problem> {
        let (end, mut warnings) = self.write_end(alias, text)?;
        let kind = match end.graph {
            None => self.graphs[alias]
                .graph
                .node(end.node)
                .map(|n| n.kind().to_string()),
            Some(_) => self
                .end_alias(alias, text)
                .filter(|copy| self.shown_unnamed(copy))
                .and_then(|copy| {
                    self.graphs
                        .get(&copy)?
                        .graph
                        .node(end.node)
                        .map(|n| n.kind().to_string())
                }),
        };
        warnings.extend(kind.and_then(|k| unexpected(&alias_name(alias), &k)));
        let address = match end.graph {
            None => crate::address(alias, end.node),
            Some(g) => format!("umx:{}/{}", g.to_text(), end.node.to_text()),
        };
        Ok((Anchor { end, address }, warnings))
    }
}

/// `relation_endpoint_unexpected` when an anchor of type `kind` is not one raised_in expects (soft: never refused).
pub(crate) fn unexpected(graph: &ValidName, kind: &str) -> Option<Problem> {
    let rel = std_relation(RAISED_IN).expect("raised_in is std");
    if rel.to_types.contains(&kind) {
        return None;
    }
    let expected = rel
        .to_types
        .iter()
        .filter_map(|t| ValidName::new(t))
        .collect();
    Some(problems::relation_endpoint_unexpected(
        graph,
        &ValidName::new(RAISED_IN).expect("a std name"),
        &ValidName::new(kind)?,
        expected,
    ))
}

/// `off_vocabulary_relation` once per write, when the graph does not declare raised_in (as `--parent` does for
/// part_of).
pub(crate) fn vocabulary(o: &Ontology, graph: &ValidName) -> Option<Problem> {
    (!o.has_relation(RAISED_IN)).then(|| {
        let raised_in = ValidName::new(RAISED_IN).expect("a std name");
        problems::off_vocabulary_relation(graph, &raised_in, Vec::new())
    })
}

/// The raised_in edge `id` from the new node `node` to `anchor`, stamped as edge add stamps one.
pub(crate) fn edge(id: Id, node: Id, anchor: &Anchor, now: &str, by: &str) -> Edge {
    set_ends(
        new_edge(
            id,
            node,
            RAISED_IN,
            anchor.end.node,
            now,
            by,
            BTreeMap::new(),
        ),
        StoredEnd::local(node),
        anchor.end,
    )
}
