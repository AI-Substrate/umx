use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::mem::size_of;
use umx_core::Id;
use umx_core::provenance::{ProvKey, is_provenance_key};

use crate::{Graph, NodeIx, NodeRef, UniqueIndex};

/// A separate interner per session column; zero is absent, even when Id(0) is present.
#[derive(Clone, Debug, Default)]
pub(crate) struct Sessions {
    values: Vec<u32>,
    ids: Vec<Id>,
    codes: BTreeMap<Id, u32>,
}

impl Sessions {
    fn code(&mut self, value: Option<Id>) -> u32 {
        let Some(id) = value else { return 0 };
        *self.codes.entry(id).or_insert_with(|| {
            self.ids.push(id);
            self.ids.len() as u32
        })
    }

    pub fn push(&mut self, value: Option<Id>) {
        let code = self.code(value);
        self.values.push(code);
    }

    pub fn set(&mut self, h: usize, value: Option<Id>) {
        let code = self.code(value);
        self.values[h] = code;
    }

    pub fn get(&self, h: usize) -> Option<Id> {
        self.values[h].checked_sub(1).map(|i| self.ids[i as usize])
    }

    pub fn remove(&mut self, h: usize) {
        self.values.remove(h);
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn interned(&self) -> usize {
        self.ids.len()
    }

    fn heap_bytes(&self) -> usize {
        self.values.capacity() * size_of::<u32>()
            + self.ids.capacity() * size_of::<Id>()
            + crate::cols::btree_bytes::<Id, u32>(self.codes.len())
    }

    fn shrink(&mut self) {
        self.values.shrink_to_fit();
        self.ids.shrink_to_fit();
    }
}

/// Edge edited_by_session (the project manager B): only edited edges carry it, so it is sparse by edge id with its own
/// interner. Keyed by id, so no handle shift touches it. Empty maps allocate nothing.
#[derive(Clone, Debug, Default)]
pub(crate) struct EdgeSessions {
    by_edge: BTreeMap<Id, u32>,
    ids: Vec<Id>,
    codes: BTreeMap<Id, u32>,
}

impl EdgeSessions {
    pub fn set(&mut self, edge: Id, value: Option<Id>) {
        let Some(session) = value else {
            self.by_edge.remove(&edge);
            return;
        };
        let ids = &mut self.ids;
        let code = *self.codes.entry(session).or_insert_with(|| {
            ids.push(session);
            ids.len() as u32 - 1
        });
        self.by_edge.insert(edge, code);
    }

    pub fn get(&self, edge: Id) -> Option<Id> {
        self.by_edge.get(&edge).map(|&c| self.ids[c as usize])
    }

    pub fn remove(&mut self, edge: Id) {
        self.by_edge.remove(&edge);
    }

    fn edges(&self) -> usize {
        self.by_edge.len()
    }

    fn interned(&self) -> usize {
        self.ids.len()
    }

    fn heap_bytes(&self) -> usize {
        crate::cols::btree_bytes::<Id, u32>(self.by_edge.len())
            + self.ids.capacity() * size_of::<Id>()
            + crate::cols::btree_bytes::<Id, u32>(self.codes.len())
    }

    fn shrink(&mut self) {
        self.ids.shrink_to_fit();
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Provenance {
    pub authored: Sessions,
    pub edited: Sessions,
    pub edge_authored: Sessions,
    pub edge_edited: EdgeSessions,
    pub keys: UniqueIndex<&'static str>,
    bits: Vec<u64>,
}

impl Default for Provenance {
    fn default() -> Self {
        Self {
            authored: Sessions::default(),
            edited: Sessions::default(),
            edge_authored: Sessions::default(),
            edge_edited: EdgeSessions::default(),
            keys: UniqueIndex::new("prov_key"),
            bits: Vec::new(),
        }
    }
}

impl Provenance {
    pub(crate) fn handles(&self) -> impl Iterator<Item = u32> + '_ {
        self.bits.iter().enumerate().flat_map(|(index, &bits)| {
            let mut bits = bits;
            core::iter::from_fn(move || {
                if bits == 0 {
                    return None;
                }
                let bit = bits.trailing_zeros();
                bits &= bits - 1;
                Some(index as u32 * 64 + bit)
            })
        })
    }

    pub fn count(&self) -> usize {
        self.bits
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    pub fn has(&self, h: u32) -> bool {
        self.bits
            .get(h as usize / 64)
            .is_some_and(|w| w & (1 << (h % 64)) != 0)
    }

    pub fn set(&mut self, h: u32, kind: &str, key: Option<&str>, building: bool) -> bool {
        self.bits
            .resize((h as usize + 1).div_ceil(64).max(self.bits.len()), 0);
        let valid = is_provenance_key(kind, key);
        let was = self.has(h);
        let bit = 1u64 << (h % 64);
        let word = &mut self.bits[h as usize / 64];
        *word = (*word & !bit) | if valid { bit } else { 0 };
        if was {
            self.keys.remove(h);
        }
        if valid {
            let key = key.expect("valid key").as_bytes();
            let added = if building {
                self.keys.push_unsorted(key, h)
            } else {
                self.keys.insert(key, h)
            };
            added.expect("key bytes fit in the already-bounded node image");
        }
        was != valid
    }

    pub fn clear_nodes(&mut self) {
        self.authored.clear();
        self.edited.clear();
        self.keys.clear();
        self.bits.clear();
    }

    pub fn pop(&mut self, h: u32) {
        self.authored.remove(h as usize);
        self.edited.remove(h as usize);
        self.keys.remove(h);
        self.bits[h as usize / 64] &= !(1 << (h % 64));
        self.bits.truncate((h as usize).div_ceil(64));
    }

    pub fn heap_bytes(&self) -> usize {
        self.authored.heap_bytes()
            + self.edited.heap_bytes()
            + self.edge_authored.heap_bytes()
            + self.edge_edited.heap_bytes()
            + self.keys.heap_bytes()
            + self.bits.capacity() * size_of::<u64>()
    }

    pub fn shrink(&mut self) {
        self.authored.shrink();
        self.edited.shrink();
        self.edge_authored.shrink();
        self.edge_edited.shrink();
        self.keys.shrink();
        self.bits.shrink_to_fit();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DuplicateProvKey {
    pub node: Id,
    pub in_use: Id,
}

/// Heap parts sum to `MemoryBytes::provenance`; interned ids remain until reload, even after their last use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProvenanceStats {
    pub unique_index_bytes: usize,
    pub unique_index_entries: usize,
    pub unique_index_dead_bytes: usize,
    pub bitset_bytes: usize,
    pub provenance_nodes: usize,
    pub authored_bytes: usize,
    pub authored_ids: usize,
    pub edited_bytes: usize,
    pub edited_ids: usize,
    pub edge_authored_bytes: usize,
    pub edge_authored_ids: usize,
    pub edge_edited_bytes: usize,
    pub edge_edited_ids: usize,
    /// Edges carrying edited_by_session.
    pub edge_edited_edges: usize,
}

impl DuplicateProvKey {
    pub fn code(self) -> &'static str {
        "duplicate_prov_key"
    }
}

/// A read-only node-to-session edge. It has no stored edge id.
#[derive(Clone, Copy, Debug)]
pub struct ProvenanceEdge<'a> {
    pub from: NodeRef<'a>,
    pub to: NodeRef<'a>,
    pub kind: &'static str,
}

impl Graph {
    pub fn provenance_node(&self, key: &str) -> Option<NodeRef<'_>> {
        ProvKey::parse(key).ok()?;
        self.provenance
            .keys
            .first(key.as_bytes())
            .map(|h| NodeRef { g: self, h })
    }

    /// Every provenance node holding `key`, lowest handle first: [`Self::provenance_node`]'s, then any duplicate a
    /// merge left (duplicate_prov_key keeps both). Identity is the key, so a read joining on it takes them all (plan
    /// 188). None for a key that does not parse.
    pub fn provenance_holders<'g>(
        &'g self,
        key: &str,
    ) -> impl Iterator<Item = NodeRef<'g>> + use<'g> {
        let parsed = ProvKey::parse(key).is_ok();
        self.provenance
            .keys
            .holders(key.as_bytes())
            .filter(move |_| parsed)
            .map(move |h| NodeRef { g: self, h })
    }

    pub fn is_provenance(&self, node: NodeIx) -> bool {
        assert!(
            node.index() < self.ids.len(),
            "a node handle from another borrow"
        );
        self.provenance.has(node.0)
    }

    pub fn provenance_count(&self) -> usize {
        self.provenance.count()
    }

    pub fn provenance_stats(&self) -> ProvenanceStats {
        let p = &self.provenance;
        ProvenanceStats {
            unique_index_bytes: p.keys.heap_bytes(),
            unique_index_entries: p.keys.live_len(),
            unique_index_dead_bytes: p.keys.dead_bytes(),
            bitset_bytes: p.bits.capacity() * size_of::<u64>(),
            provenance_nodes: p.count(),
            authored_bytes: p.authored.heap_bytes(),
            authored_ids: p.authored.interned(),
            edited_bytes: p.edited.heap_bytes(),
            edited_ids: p.edited.interned(),
            edge_authored_bytes: p.edge_authored.heap_bytes(),
            edge_authored_ids: p.edge_authored.interned(),
            edge_edited_bytes: p.edge_edited.heap_bytes(),
            edge_edited_ids: p.edge_edited.interned(),
            edge_edited_edges: p.edge_edited.edges(),
        }
    }

    pub fn duplicate_prov_keys(&self) -> impl Iterator<Item = DuplicateProvKey> + '_ {
        self.provenance
            .keys
            .duplicates()
            .map(|(h, winner)| DuplicateProvKey {
                node: self.ids[h as usize],
                in_use: self.ids[winner as usize],
            })
    }

    fn session(&self, id: Option<Id>) -> Option<NodeRef<'_>> {
        let node = self.node(id?)?;
        (node.kind() == "session" && self.is_provenance(node.ix())).then_some(node)
    }

    /// Explicit derived views only. Ordinary adjacency and stored-edge iterators are unchanged in P1.
    pub fn provenance_edges(&self) -> impl Iterator<Item = ProvenanceEdge<'_>> {
        self.nodes()
            .flat_map(|node| self.node_provenance_edges(node.ix()))
    }

    pub fn node_provenance_edges(&self, ix: NodeIx) -> impl Iterator<Item = ProvenanceEdge<'_>> {
        let from = self.node_at(ix);
        [
            ("authored_by", self.provenance.authored.get(ix.index())),
            ("edited_by", self.provenance.edited.get(ix.index())),
        ]
        .into_iter()
        .filter_map(move |(kind, id)| {
            Some(ProvenanceEdge {
                from,
                to: self.session(id)?,
                kind,
            })
        })
    }
}
