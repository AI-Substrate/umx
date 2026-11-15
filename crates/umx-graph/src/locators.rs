//! Resource nodes in memory (ws006 S3; workshop 010 9a): a resource row's locator kind and format in a sparse column
//! over its handle, and the one locator index, from a locator's key (kind, then normalised source) to its handle.
//! `hash`, `length` and `seq_max` stay cold in the line. A duplicate locator after a merge loads, is listed, and
//! nothing is erased; a lookup returns the lower id.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;
use core::mem::size_of;
use umx_core::locator::LOCATOR_FIELD;
use umx_core::{Format, Id, Locator, LocatorKind};

use crate::{Graph, NodeIx, NodeRef, UniqueIndex};

#[derive(Clone, Debug)]
pub(crate) struct Locators {
    /// Kind and format per resource row, keyed by handle.
    cols: BTreeMap<u32, (LocatorKind, Option<Format>)>,
    index: UniqueIndex<&'static str>,
    /// Rows whose stored source is not in normal form (a hand edit): keyed as it normalises, warned at load.
    unnormal: BTreeSet<u32>,
}

impl Default for Locators {
    fn default() -> Self {
        Self {
            cols: BTreeMap::new(),
            index: UniqueIndex::new("locator"),
            unnormal: BTreeSet::new(),
        }
    }
}

impl Locators {
    /// Sets row `h`'s locator (none for a plain node), replacing what the row had.
    pub fn set(&mut self, h: u32, locator: Option<&Locator>) {
        if self.cols.remove(&h).is_some() {
            self.index.remove(h);
        }
        self.unnormal.remove(&h);
        self.push(h, locator, false);
    }

    /// A row with no locator yet gets `locator`. While `building` (the load pass, between `begin_build` and
    /// `finish_build`), its key is collected unsorted and sorted once at the end (ws003 s19, review B2).
    pub fn push(&mut self, h: u32, locator: Option<&Locator>, building: bool) {
        let Some(l) = locator else { return };
        self.cols.insert(h, (l.kind, l.format));
        let key = l.key();
        let added = if building {
            self.index.push_unsorted(&key, h)
        } else {
            self.index.insert(&key, h)
        };
        added.expect("key bytes fit in the already-bounded node image");
        if !l.is_normalised() {
            self.unnormal.insert(h);
        }
    }

    pub fn begin_build(&mut self) {
        self.index.begin_build();
    }

    pub fn finish_build(&mut self) {
        self.index.finish_build();
    }

    /// Removes the last row.
    pub fn pop(&mut self, h: u32) {
        self.set(h, None);
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn heap_bytes(&self) -> usize {
        crate::cols::btree_bytes::<u32, (LocatorKind, Option<Format>)>(self.cols.len())
            + self.index.heap_bytes()
            + self.unnormal.len() * size_of::<u32>()
    }

    pub fn shrink(&mut self) {
        self.index.shrink();
    }

    #[cfg(test)]
    pub(crate) fn check(&self) {
        self.index.check();
    }
}

/// Two resource nodes with one locator (both branches of a merge added the same source): `node` is kept beside
/// `in_use`, the lower id, which lookups return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DuplicateLocator {
    pub node: Id,
    pub in_use: Id,
}

impl Graph {
    /// Whether any node has a locator: the graph then needs `FORMAT_LOCATORS` (the max rule, never lowered).
    pub fn has_locators(&self) -> bool {
        !self.locators.cols.is_empty()
    }

    /// The resource node holding `locator` (by its key: kind and normalised source), the lower id of duplicates.
    pub fn by_locator(&self, locator: &Locator) -> Option<NodeRef<'_>> {
        self.locators
            .index
            .first(&locator.key())
            .map(|h| NodeRef { g: self, h })
    }

    /// A row's locator kind and format, or None for a plain node.
    pub fn locator_kind(&self, node: NodeIx) -> Option<(LocatorKind, Option<Format>)> {
        assert!(
            node.index() < self.ids.len(),
            "a node handle from another borrow"
        );
        self.locators.cols.get(&node.0).copied()
    }

    /// Every resource node, in handle (id) order.
    pub fn resources(&self) -> impl Iterator<Item = NodeRef<'_>> + '_ {
        self.locators.cols.keys().map(|&h| NodeRef { g: self, h })
    }

    /// Each resource node that shares its locator with a lower id (`duplicate_locator`).
    pub fn duplicate_locators(&self) -> impl Iterator<Item = DuplicateLocator> + '_ {
        self.locators
            .index
            .duplicates()
            .map(|(h, winner)| DuplicateLocator {
                node: self.ids[h as usize],
                in_use: self.ids[winner as usize],
            })
    }

    /// Nodes holding a kept custom field named `locator`, a value that is not a locator (one from before the name was
    /// reserved): loaded and written back as it is, warned `locator_field_kept` (graph-storage-expert, S241-1).
    /// In id order.
    pub fn kept_locator_fields(&self) -> Vec<Id> {
        let Some(col) = self
            .names
            .get(LOCATOR_FIELD)
            .and_then(|f| self.f_cols.get(&f))
        else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        col.each(|h, _| rows.push(h));
        rows.sort_unstable();
        rows.into_iter().map(|h| self.ids[h as usize]).collect()
    }

    /// Resource nodes whose stored source is not in normal form (a hand edit), in id order.
    pub fn unnormalised_locators(&self) -> impl Iterator<Item = Id> + '_ {
        self.locators.unnormal.iter().map(|&h| self.ids[h as usize])
    }
}
