//! The entity key family (ws008 E1, req-127): each node's `entity_key` list indexed as `<type>\0<key>`, so the
//! nodes of one type sharing a key are found by one lookup. Built in the load pass and kept on every row change,
//! like the provenance keys. A key repeated across nodes of the same type in one graph is kept; the lowest handle
//! wins and the rest are reported (`duplicate_key`, the req-102 rule).

use crate::{Graph, NodeRef, UniqueIndex};
use alloc::string::String;
use alloc::vec::Vec;
use umx_core::entity::{index_key, stored_keys};
use umx_core::{Id, Value};

#[derive(Clone, Debug)]
pub(crate) struct EntityKeys {
    keys: UniqueIndex<&'static str>,
    /// One bit per row: set while the row has entries in `keys`. A row is only removed when its bit says it is
    /// there, so a keyless row's load, rederive or write never touches `keys` (S208-1): `UniqueIndex::remove` finds a
    /// row by binary search since ws003 s19, and the bit saves that search for every keyless row.
    bits: Vec<u64>,
    /// Tests only: how many times a row was taken out of `keys`.
    #[cfg(test)]
    pub removes: usize,
    /// Tests only: how many type names `entity_key_types` has looked a key up under.
    #[cfg(test)]
    pub probes: core::cell::Cell<usize>,
}

impl Default for EntityKeys {
    fn default() -> Self {
        Self {
            keys: UniqueIndex::new(umx_core::entity::ENTITY_KEY),
            bits: Vec::new(),
            #[cfg(test)]
            removes: 0,
            #[cfg(test)]
            probes: core::cell::Cell::new(0),
        }
    }
}

impl EntityKeys {
    fn has(&self, h: u32) -> bool {
        self.bits
            .get(h as usize / 64)
            .is_some_and(|w| w & (1 << (h % 64)) != 0)
    }

    fn mark(&mut self, h: u32, on: bool) {
        let word = h as usize / 64;
        if word >= self.bits.len() {
            if !on {
                return;
            }
            self.bits.resize(word + 1, 0);
        }
        let bit = 1u64 << (h % 64);
        self.bits[word] = (self.bits[word] & !bit) | if on { bit } else { 0 };
    }

    fn take_out(&mut self, h: u32) {
        if self.has(h) {
            self.keys.remove(h);
            self.mark(h, false);
            #[cfg(test)]
            {
                self.removes += 1;
            }
        }
    }

    fn put(&mut self, h: u32, kind: &str, value: Option<&Value>, building: bool) {
        let Some(value) = value else { return };
        let keys = stored_keys(value);
        for key in &keys {
            let key = index_key(kind, key);
            let added = if building {
                self.keys.push_unsorted(&key, h)
            } else {
                self.keys.insert(&key, h)
            };
            added.expect("key bytes fit in the already-bounded node image");
        }
        if !keys.is_empty() {
            self.mark(h, true);
        }
    }

    /// A new last row's keys (None: the field is absent). A new row has no entries yet, so nothing is removed. While
    /// `building` (the load pass), keys are collected unsorted and sorted once by `finish_build` (ws003 s19).
    pub fn push(&mut self, h: u32, kind: &str, value: Option<&Value>, building: bool) {
        self.put(h, kind, value, building);
    }

    /// Sets row `h`'s keys from its `entity_key` value (None: the field is absent).
    pub fn set(&mut self, h: u32, kind: &str, value: Option<&Value>) {
        self.take_out(h);
        self.put(h, kind, value, false);
    }

    pub fn begin_build(&mut self) {
        self.keys.begin_build();
    }

    pub fn finish_build(&mut self) {
        self.keys.finish_build();
    }

    pub fn clear(&mut self) {
        self.keys.clear();
        self.bits.clear();
    }

    #[cfg(test)]
    pub(crate) fn check(&self) {
        self.keys.check();
    }

    pub fn pop(&mut self, h: u32) {
        self.take_out(h);
        self.bits.truncate((h as usize).div_ceil(64));
    }

    pub fn heap_bytes(&self) -> usize {
        self.keys.heap_bytes() + self.bits.capacity() * core::mem::size_of::<u64>()
    }

    pub fn shrink(&mut self) {
        self.keys.shrink();
        self.bits.shrink_to_fit();
    }
}

/// A node whose entity key is held by a lower-id node of the same type in the same graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicateEntityKey {
    pub node: Id,
    pub in_use: Id,
    /// The key in stored form, `scheme:value`.
    pub key: String,
}

impl Graph {
    /// The node of type `kind` holding `key` (in stored form), the lowest id when several do.
    pub fn entity_node(&self, kind: &str, key: &str) -> Option<NodeRef<'_>> {
        self.entities
            .keys
            .first(&index_key(kind, key))
            .map(|h| NodeRef { g: self, h })
    }

    /// The node of type `kind` holding `key` (stored form), the lowest id when several do, and whether another node of
    /// that type holds it too (ws008 E2b: a key with two holders in one graph is not used to join there).
    pub fn entity_holders(&self, kind: &str, key: &str) -> (Option<NodeRef<'_>>, bool) {
        let (first, more) = self.entities.keys.first_two(&index_key(kind, key));
        (first.map(|h| NodeRef { g: self, h }), more)
    }

    /// Each node type in use here with a node holding `key` (stored form), and that node (the lowest id), in name
    /// order (ws008 E2b G5, graph-storage-expert). Probes the types nodes hold, declared or not, never the ontology's
    /// list.
    pub fn entity_key_types<'a>(
        &'a self,
        key: &'a str,
    ) -> impl Iterator<Item = (&'a str, NodeRef<'a>)> + 'a {
        self.names
            .in_name_order()
            .filter(|&n| self.names.used_by_node(n))
            .filter_map(move |n| {
                let kind = self.names.text(n);
                #[cfg(test)]
                self.entities.probes.set(self.entities.probes.get() + 1);
                self.entities
                    .keys
                    .first(&index_key(kind, key))
                    .map(|h| (kind, NodeRef { g: self, h }))
            })
    }

    /// Every repeated key, each losing node with the lower-id node in use, in type, key then id order.
    pub fn duplicate_entity_keys(&self) -> impl Iterator<Item = DuplicateEntityKey> + '_ {
        self.entities
            .keys
            .duplicates_with_keys()
            .map(|(h, winner, key)| DuplicateEntityKey {
                node: self.ids[h as usize],
                in_use: self.ids[winner as usize],
                key: key_text(key),
            })
    }

    /// Each node whose stored `entity_key` is not all in stored form, or holds entries that cannot be used, in id
    /// order. Only rows holding the field are read.
    pub fn entity_key_findings(&self) -> Vec<(Id, umx_core::entity::StoredFindings)> {
        let Some(col) = self
            .names
            .get(umx_core::entity::ENTITY_KEY)
            .and_then(|f| self.f_cols.get(&f))
        else {
            return Vec::new();
        };
        (0..self.ids.len() as u32)
            .filter(|h| col.get(*h).is_some())
            .filter_map(|h| {
                let value = NodeRef { g: self, h }.field(umx_core::entity::ENTITY_KEY)?;
                let f = umx_core::entity::stored_findings(&value);
                (f.not_canonical || f.unusable).then_some((self.ids[h as usize], f))
            })
            .collect()
    }

    /// The repeated keys that involve `id`, as winner or loser.
    pub fn duplicate_entity_keys_of(&self, id: Id) -> Vec<DuplicateEntityKey> {
        self.duplicate_entity_keys()
            .filter(|d| d.node == id || d.in_use == id)
            .collect()
    }
}

/// The `scheme:value` part of an index key.
fn key_text(key: &[u8]) -> String {
    let at = key.iter().position(|b| *b == 0).map_or(0, |i| i + 1);
    String::from_utf8_lossy(&key[at..]).into_owned()
}

#[cfg(test)]
mod tests {
    use crate::Graph;
    use alloc::string::String;
    use alloc::vec;
    use serde_json::json;
    use umx_core::{Id, Node};

    fn node(id: u128, title: &str, keys: Option<serde_json::Value>) -> Node {
        typed(id, "person", title, keys)
    }

    fn typed(id: u128, kind: &str, title: &str, keys: Option<serde_json::Value>) -> Node {
        Node {
            id: Id(id),
            kind: kind.into(),
            title: title.into(),
            status: "open".into(),
            note: String::new(),
            evidence: vec![],
            status_history: vec![],
            created: "now".into(),
            updated: "now".into(),
            created_by: "t".into(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields: keys
                .map(|k| [("entity_key".into(), k)].into())
                .unwrap_or_default(),
        }
    }

    /// S208-1: a keyless row never takes a row out of the index (a full scan of it), on load, add or edit.
    #[test]
    fn a_keyless_row_never_touches_the_index() {
        let mut g = Graph::new();
        g.insert_node(node(1, "keyed", Some(json!(["upn:a"]))))
            .unwrap();
        for id in 2..50 {
            g.insert_node(node(id, "plain", None)).unwrap();
        }
        assert_eq!(g.entities.removes, 0, "adds remove nothing");
        g.insert_node(node(25, "plain, edited", None)).unwrap();
        assert_eq!(
            g.entities.removes, 0,
            "a keyless edit leaves the index untouched"
        );
        let (loaded, _) = Graph::load_nodes(g.encode_nodes()).unwrap();
        assert_eq!(loaded.entities.removes, 0, "a load removes nothing");
        g.insert_node(node(1, "keyed, edited", Some(json!(["upn:b"]))))
            .unwrap();
        assert_eq!(
            g.entities.removes, 1,
            "a keyed row's edit takes its old keys out once"
        );
        assert_eq!(
            g.entity_node("person", "upn:b").map(|n| n.id()),
            Some(Id(1))
        );
        assert!(g.entity_node("person", "upn:a").is_none());
        g.insert_node(node(1, "keyless now", None)).unwrap();
        assert_eq!(g.entities.removes, 2);
        g.insert_node(node(1, "still keyless", None)).unwrap();
        assert_eq!(g.entities.removes, 2, "its bit went with its keys");
    }

    /// ws008 E2b G5 (graph-storage-expert): the types a key is held under are the types nodes hold, an undeclared one
    /// included; a type no node holds any more is not probed.
    #[test]
    fn entity_key_types_probes_the_types_nodes_hold() {
        let mut g = Graph::new();
        g.insert_node(typed(
            1,
            "organisation",
            "Acme",
            Some(json!(["domain:x.com"])),
        ))
        .unwrap();
        g.insert_node(typed(
            2,
            "made_up_type",
            "Acme team",
            Some(json!(["domain:x.com"])),
        ))
        .unwrap();
        g.insert_node(typed(3, "person", "Jo", None)).unwrap();
        let types = |g: &Graph| -> alloc::vec::Vec<(String, Id)> {
            g.entity_key_types("domain:x.com")
                .map(|(k, n)| (k.into(), n.id()))
                .collect()
        };
        assert_eq!(
            types(&g),
            [
                ("made_up_type".into(), Id(2)),
                ("organisation".into(), Id(1))
            ]
        );
        g.insert_node(typed(
            2,
            "organisation",
            "Acme team",
            Some(json!(["domain:x.com"])),
        ))
        .unwrap();
        assert!(!g.names.used_by_node(g.names.get("made_up_type").unwrap()));
        g.entities.probes.set(0);
        assert_eq!(types(&g), [("organisation".into(), Id(1))]);
        assert_eq!(
            g.entities.probes.get(),
            2,
            "only the types nodes hold now (organisation, person), never made_up_type or a status"
        );
        assert!(g.entity_holders("organisation", "domain:x.com").1);
        assert_eq!(
            g.entity_holders("organisation", "domain:x.com")
                .0
                .map(|n| n.id()),
            Some(Id(1))
        );
        assert!(g.entity_holders("person", "domain:x.com").0.is_none());
    }
}
