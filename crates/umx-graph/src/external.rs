use crate::cols::Entry;
use crate::{EdgeEnd, EdgeIx, Graph, Id, Kind, NodeIx};
use alloc::vec::Vec;
use core::mem::size_of;

pub(crate) const EXTERNAL: u32 = 1 << 31;

/// Sparse adjacency: only local nodes that touch external ends occupy space.
#[derive(Clone, Debug, Default)]
pub(crate) struct ExternalAdj {
    entries: Vec<(u32, Entry)>,
}

impl ExternalAdj {
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn push(&mut self, local: u32, edge: u32, external: u32) {
        self.entries.push((
            local,
            Entry {
                edge,
                other: external,
            },
        ));
    }

    pub fn finish(&mut self) {
        self.entries
            .sort_unstable_by_key(|(local, entry)| (*local, entry.edge));
    }

    pub fn of(&self, node: u32) -> &[(u32, Entry)] {
        let start = self.entries.partition_point(|(local, _)| *local < node);
        let end = self.entries[start..].partition_point(|(local, _)| *local == node);
        &self.entries[start..start + end]
    }

    pub fn shrink(&mut self) {
        self.entries.shrink_to_fit();
    }

    pub fn heap_bytes(&self) -> usize {
        self.entries.capacity() * size_of::<(u32, Entry)>()
    }
}

impl Graph {
    pub(crate) fn end_parts(&self, end: u32) -> (Option<Id>, Id) {
        if end & EXTERNAL == 0 {
            (None, self.ids[end as usize])
        } else {
            let (graph, node) = self.ext[(end & !EXTERNAL) as usize];
            (Some(graph), node)
        }
    }

    pub(crate) fn end_handle(&mut self, graph: Option<Id>, node: Id) -> Option<u32> {
        match graph {
            None => self.handle(node),
            Some(graph) => {
                assert!(self.ext.len() < EXTERNAL as usize, "too many external ends");
                let end = EXTERNAL | self.ext.len() as u32;
                self.ext.push((graph, node));
                Some(end)
            }
        }
    }

    pub(crate) fn edge_handles(
        &mut self,
        from_graph: Option<Id>,
        from: Id,
        to_graph: Option<Id>,
        to: Id,
    ) -> Option<(u32, u32)> {
        // Validate both local ends before extending the table.
        if (from_graph.is_none() && self.handle(from).is_none())
            || (to_graph.is_none() && self.handle(to).is_none())
        {
            return None;
        }
        Some((
            self.end_handle(from_graph, from)?,
            self.end_handle(to_graph, to)?,
        ))
    }

    pub(crate) fn build_external(&mut self) {
        if self.ext_dirty {
            let old = core::mem::take(&mut self.ext);
            for end in self.e_from.iter_mut().chain(self.e_to.iter_mut()) {
                if *end & EXTERNAL != 0 {
                    let value = old[(*end & !EXTERNAL) as usize];
                    *end = EXTERNAL | self.ext.len() as u32;
                    self.ext.push(value);
                }
            }
            self.ext.shrink_to_fit();
            self.ext_dirty = false;
        }
        self.ext_out.clear();
        self.ext_in.clear();
        if !self.ext.is_empty() {
            for (edge, (&from, &to)) in self.e_from.iter().zip(&self.e_to).enumerate() {
                match (from & EXTERNAL != 0, to & EXTERNAL != 0) {
                    (false, true) => self.ext_out.push(from, edge as u32, to & !EXTERNAL),
                    (true, false) => self.ext_in.push(to, edge as u32, from & !EXTERNAL),
                    _ => {}
                }
            }
            self.ext_out.finish();
            self.ext_in.finish();
        } else {
            self.ext_out.shrink();
            self.ext_in.shrink();
        }
    }

    /// The graph id of each edge end in another graph, one per end, from the end columns (s20: graph.json's format
    /// and the own-graph check need no scan of edges.jsonl). Kept lines are not edges: see `kept_edge_lines`.
    pub fn external_end_graphs(&self) -> impl Iterator<Item = Id> + '_ {
        self.e_from
            .iter()
            .chain(&self.e_to)
            .filter(|&&end| end & EXTERNAL != 0)
            .map(|&end| self.ext[(end & !EXTERNAL) as usize].0)
    }

    /// Both ends of any stored edge. External ids never become local node handles.
    pub fn edge_ends(&self, edge: EdgeIx) -> (EdgeEnd, EdgeEnd) {
        let end = |h| {
            let (graph, id) = self.end_parts(h);
            match graph {
                Some(graph) => EdgeEnd::External(graph, id),
                None => EdgeEnd::Local(NodeIx(h)),
            }
        };
        (end(self.e_from[edge.index()]), end(self.e_to[edge.index()]))
    }

    /// Edges from a local node to an external end, in edge id order: edge, kind, graph id, node id.
    pub fn edges_out_external(
        &self,
        node: NodeIx,
    ) -> impl Iterator<Item = (EdgeIx, Kind<'_>, Id, Id)> {
        self.ext_out.of(node.0).iter().map(|(_, entry)| {
            let (graph, id) = self.ext[entry.other as usize];
            (
                EdgeIx(entry.edge),
                Kind(self.names.text(self.e_kind.get(entry.edge as usize))),
                graph,
                id,
            )
        })
    }

    /// Edges from an external end to a local node, in edge id order. Both-external edges have no adjacency.
    pub fn edges_in_external(
        &self,
        node: NodeIx,
    ) -> impl Iterator<Item = (EdgeIx, Kind<'_>, Id, Id)> {
        self.ext_in.of(node.0).iter().map(|(_, entry)| {
            let (graph, id) = self.ext[entry.other as usize];
            (
                EdgeIx(entry.edge),
                Kind(self.names.text(self.e_kind.get(entry.edge as usize))),
                graph,
                id,
            )
        })
    }
}
