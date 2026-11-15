//! A sorted unique-key family. Duplicates remain present; the lowest handle wins. Beside the key order, each live
//! entry's key range is held in handle order, so removing a row finds its entries without a scan (ws003 s19).

use alloc::vec::Vec;
use core::mem::size_of;

#[derive(Clone, Copy, Debug)]
struct Entry {
    start: u32,
    end: u32,
    handle: u32,
    live: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexFull;

/// A live entry's key range, held in (handle, start, end) order (an empty key can share its start with the next).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Held {
    handle: u32,
    start: u32,
    end: u32,
}

/// One arena per family, with byte-exact comparison. Handles must be in id order.
#[derive(Clone, Debug)]
pub struct UniqueIndex<K> {
    family: K,
    arena: Vec<u8>,
    entries: Vec<Entry>,
    /// One row per live entry, outside a build; rebuilt by `finish_build` and `compact`, as ranges move.
    by_handle: Vec<Held>,
    dead_bytes: usize,
    sorted: bool,
}

impl<K> UniqueIndex<K> {
    pub fn new(family: K) -> Self {
        Self {
            family,
            arena: Vec::new(),
            entries: Vec::new(),
            by_handle: Vec::new(),
            dead_bytes: 0,
            sorted: true,
        }
    }

    pub fn family(&self) -> &K {
        &self.family
    }

    fn key(&self, e: &Entry) -> &[u8] {
        &self.arena[e.start as usize..e.end as usize]
    }

    fn append(&mut self, key: &[u8], handle: u32) -> Result<Entry, IndexFull> {
        if self.arena.len().saturating_add(key.len()) > u32::MAX as usize {
            self.compact();
        }
        let end = self.arena.len().checked_add(key.len()).ok_or(IndexFull)?;
        let end = u32::try_from(end).map_err(|_| IndexFull)?;
        let start = self.arena.len() as u32;
        self.arena.extend_from_slice(key);
        Ok(Entry {
            start,
            end,
            handle,
            live: true,
        })
    }

    pub fn begin_build(&mut self) {
        self.clear();
        self.sorted = false;
    }

    /// Collect keys without moving earlier entries. Reads are refused until `finish_build`.
    pub fn push_unsorted(&mut self, key: &[u8], handle: u32) -> Result<(), IndexFull> {
        assert!(!self.sorted, "begin_build before collecting keys");
        let entry = self.append(key, handle)?;
        self.entries.push(entry);
        Ok(())
    }

    pub fn finish_build(&mut self) {
        let arena = &self.arena;
        self.entries.sort_unstable_by(|a, b| {
            arena[a.start as usize..a.end as usize]
                .cmp(&arena[b.start as usize..b.end as usize])
                .then_with(|| a.handle.cmp(&b.handle))
        });
        let before = self.entries.len();
        self.entries.dedup_by(|a, b| {
            a.handle == b.handle
                && arena[a.start as usize..a.end as usize]
                    == arena[b.start as usize..b.end as usize]
        });
        if self.entries.len() != before {
            self.compact();
        } else {
            self.index_handles();
        }
        self.sorted = true;
    }

    pub fn insert(&mut self, key: &[u8], handle: u32) -> Result<(), IndexFull> {
        assert!(self.sorted, "unique index is still being built");
        let mut at = self
            .entries
            .partition_point(|e| (self.key(e), e.handle) < (key, handle));
        if self.entries[at..]
            .iter()
            .take_while(|e| self.key(e) == key && e.handle == handle)
            .any(|e| e.live)
        {
            return Ok(());
        }
        let compacting = self.arena.len().saturating_add(key.len()) > u32::MAX as usize;
        let entry = self.append(key, handle)?;
        if compacting {
            at = self
                .entries
                .partition_point(|e| (self.key(e), e.handle) < (key, handle));
        }
        self.entries.insert(at, entry);
        let at = self
            .by_handle
            .partition_point(|b| (b.handle, b.start, b.end) < (handle, entry.start, entry.end));
        self.by_handle.insert(
            at,
            Held {
                handle,
                start: entry.start,
                end: entry.end,
            },
        );
        Ok(())
    }

    pub fn first(&self, key: &[u8]) -> Option<u32> {
        assert!(self.sorted, "unique index is still being built");
        let at = self.entries.partition_point(|e| self.key(e) < key);
        self.entries[at..]
            .iter()
            .take_while(|e| self.key(e) == key)
            .find(|e| e.live)
            .map(|e| e.handle)
    }

    /// The lowest live handle holding `key`, and whether a second live handle holds it too (ws008 E2b,
    /// graph-storage-expert). Dead entries stay in the key's run until compaction, so the run is walked to the second
    /// live entry.
    pub fn first_two(&self, key: &[u8]) -> (Option<u32>, bool) {
        assert!(self.sorted, "unique index is still being built");
        let at = self.entries.partition_point(|e| self.key(e) < key);
        let mut live = self.entries[at..]
            .iter()
            .take_while(|e| self.key(e) == key)
            .filter(|e| e.live);
        let first = live.next().map(|e| e.handle);
        (first, live.next().is_some())
    }

    /// Every live handle holding `key`, in index order (the lowest handle first): the one [`Self::first`] gives, then
    /// any a merge left beside it (plan 188, graph-storage-expert).
    pub fn holders<'s>(&'s self, key: &[u8]) -> impl Iterator<Item = u32> + use<'s, K> {
        assert!(self.sorted, "unique index is still being built");
        let at = self.entries.partition_point(|e| self.key(e) < key);
        let end = self.entries.partition_point(|e| self.key(e) <= key);
        self.entries[at..end]
            .iter()
            .filter(|e| e.live)
            .map(|e| e.handle)
    }

    /// Each losing handle and the winning handle once, in key then handle order.
    pub fn duplicates(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.duplicates_with_keys()
            .map(|(h, winner, _)| (h, winner))
    }

    /// As [`Self::duplicates`], with the shared key.
    pub fn duplicates_with_keys(&self) -> impl Iterator<Item = (u32, u32, &[u8])> + '_ {
        assert!(self.sorted, "unique index is still being built");
        let mut winner: Option<&Entry> = None;
        self.entries.iter().filter(|e| e.live).filter_map(move |e| {
            if let Some(first) = winner
                && self.key(first) == self.key(e)
            {
                Some((e.handle, first.handle, self.key(e)))
            } else {
                winner = Some(e);
                None
            }
        })
    }

    /// Marks `handle`'s live entries dead: its rows in `by_handle` give each key, and the entry is found by a binary
    /// search on (key, handle), the live one at that key's range (a dead copy of the same pair can sit beside it).
    pub fn remove(&mut self, handle: u32) {
        assert!(self.sorted, "unique index is still being built");
        let lo = self.by_handle.partition_point(|b| b.handle < handle);
        let hi = lo + self.by_handle[lo..].partition_point(|b| b.handle == handle);
        let (arena, entries) = (&self.arena, &mut self.entries);
        let of = |e: &Entry| &arena[e.start as usize..e.end as usize];
        for b in &self.by_handle[lo..hi] {
            let key = &arena[b.start as usize..b.end as usize];
            let at = entries.partition_point(|e| (of(e), e.handle) < (key, handle));
            let e = entries[at..]
                .iter_mut()
                .take_while(|e| e.handle == handle && of(e) == key)
                .find(|e| e.live && e.start == b.start)
                .expect("a held range is a live entry of its key and handle");
            e.live = false;
            self.dead_bytes += (b.end - b.start) as usize;
        }
        self.by_handle.drain(lo..hi);
        let live_bytes = self.arena.len() - self.dead_bytes;
        if self.dead_bytes > live_bytes || live_bytes == 0 {
            self.compact();
        }
    }

    /// `by_handle` from the live entries.
    fn index_handles(&mut self) {
        self.by_handle.clear();
        self.by_handle
            .extend(self.entries.iter().filter(|e| e.live).map(|e| Held {
                handle: e.handle,
                start: e.start,
                end: e.end,
            }));
        self.by_handle
            .sort_unstable_by_key(|b| (b.handle, b.start, b.end));
    }

    /// Whether `by_handle` holds exactly one row per live entry, and nothing else. A full sort: called only from
    /// `compact`'s `debug_assert!` and from tests, never in a release build.
    fn handles_match(&self) -> bool {
        let mut want: Vec<Held> = self
            .entries
            .iter()
            .filter(|e| e.live)
            .map(|e| Held {
                handle: e.handle,
                start: e.start,
                end: e.end,
            })
            .collect();
        want.sort_unstable_by_key(|b| (b.handle, b.start, b.end));
        want == self.by_handle
    }

    /// Panics unless `by_handle` matches the live entries (tests, after every step).
    #[cfg(test)]
    pub(crate) fn check(&self) {
        assert!(self.sorted, "unique index is still being built");
        assert!(self.handles_match(), "by_handle out of step");
    }

    fn compact(&mut self) {
        debug_assert!(
            !self.sorted || self.handles_match(),
            "by_handle out of step before compaction"
        );
        self.entries.retain(|e| e.live);
        let old = core::mem::take(&mut self.arena);
        let size = self
            .entries
            .iter()
            .map(|e| (e.end - e.start) as usize)
            .sum();
        self.arena = Vec::with_capacity(size);
        for e in &mut self.entries {
            let key = &old[e.start as usize..e.end as usize];
            e.start = self.arena.len() as u32;
            self.arena.extend_from_slice(key);
            e.end = self.arena.len() as u32;
        }
        self.dead_bytes = 0;
        self.index_handles();
    }

    pub fn clear(&mut self) {
        self.arena.clear();
        self.entries.clear();
        self.by_handle.clear();
        self.dead_bytes = 0;
        self.sorted = true;
    }

    pub fn live_len(&self) -> usize {
        self.entries.iter().filter(|e| e.live).count()
    }

    pub fn dead_bytes(&self) -> usize {
        self.dead_bytes
    }

    pub fn heap_bytes(&self) -> usize {
        self.arena.capacity()
            + self.entries.capacity() * size_of::<Entry>()
            + self.by_handle.capacity() * size_of::<Held>()
    }

    pub fn shrink(&mut self) {
        self.arena.shrink_to_fit();
        self.entries.shrink_to_fit();
        self.by_handle.shrink_to_fit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeSet;

    #[test]
    fn families_share_one_exact_byte_mechanism_and_keep_all_duplicates() {
        let mut index = UniqueIndex::new("person_key");
        for (key, handle) in [
            (b"Abc".as_slice(), 5),
            (b"abc", 9),
            (b"Abc", 2),
            (b"Abc", 7),
        ] {
            index.insert(key, handle).unwrap();
        }
        assert_eq!(index.first(b"Abc"), Some(2));
        assert_eq!(index.first(b"abc"), Some(9));
        assert_eq!(index.first(b"missing"), None);
        assert_eq!(index.duplicates().collect::<Vec<_>>(), [(5, 2), (7, 2)]);
        index.remove(2);
        assert_eq!(index.first(b"Abc"), Some(5));
        assert_eq!(index.duplicates().collect::<Vec<_>>(), [(7, 5)]);
        index.insert(b"Abc", 5).unwrap();
        assert_eq!(index.duplicates().count(), 1);
        assert_eq!(index.family(), &"person_key");
    }

    #[test]
    fn bulk_build_sorts_exact_keys_and_removal_compacts_only_after_dead_exceeds_live() {
        let mut index = UniqueIndex::new("prov_key");
        index.begin_build();
        for (key, h) in [(b"bbb".as_slice(), 5), (b"aaa", 3), (b"aaa", 1)] {
            index.push_unsorted(key, h).unwrap();
        }
        index.finish_build();
        assert_eq!(index.first(b"aaa"), Some(1));
        assert_eq!(index.duplicates().collect::<Vec<_>>(), [(3, 1)]);
        index.remove(1);
        assert_eq!(index.first(b"aaa"), Some(3));
        assert_eq!(index.duplicates().count(), 0);
        assert_eq!((index.dead_bytes, index.arena.len()), (3, 9));
        index.remove(3);
        assert_eq!((index.dead_bytes, index.arena.len()), (0, 3));
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.first(b"bbb"), Some(5));
        index.insert(b"aaa", 7).unwrap();
        assert_eq!(index.first(b"aaa"), Some(7));
    }

    /// ws008 E2b (graph-storage-expert): `first_two` counts live holders only, whatever dead entries the run still
    /// holds.
    #[test]
    fn first_two_sees_only_live_holders() {
        let mut index = UniqueIndex::new("entity_key");
        index.insert(b"k", 2).unwrap();
        index.insert(b"k", 5).unwrap();
        index.insert(b"z", 9).unwrap();
        index.insert(b"z", 1).unwrap();
        index.insert(b"z", 3).unwrap();
        assert_eq!(index.first_two(b"k"), (Some(2), true), "two live holders");
        assert_eq!(index.first_two(b"missing"), (None, false));
        index.remove(5);
        assert!(
            index.dead_bytes > 0,
            "the removed entry is still in the run"
        );
        assert_eq!(
            index.first_two(b"k"),
            (Some(2), false),
            "a dead entry is no holder"
        );
        index.insert(b"k", 5).unwrap();
        index.remove(5);
        index.insert(b"k", 5).unwrap();
        assert_eq!(index.first_two(b"k"), (Some(2), true));
        index.remove(2);
        assert_eq!(
            index.first_two(b"k"),
            (Some(5), false),
            "the lower holder removed: the higher one alone"
        );
        let mut one = UniqueIndex::new("entity_key");
        one.insert(b"k", 4).unwrap();
        one.insert(b"k", 6).unwrap();
        one.insert(b"q", 7).unwrap();
        one.remove(6);
        one.insert(b"k", 4).unwrap();
        assert_eq!(
            one.first_two(b"k"),
            (Some(4), false),
            "re-inserting the same handle is still one holder"
        );
    }

    #[test]
    #[should_panic(expected = "unique index is still being built")]
    fn lookup_cannot_observe_a_partially_built_index() {
        let mut index = UniqueIndex::new("prov_key");
        index.begin_build();
        index.push_unsorted(b"key", 1).unwrap();
        index.first(b"key");
    }

    /// Every read of `index` against the live (key, handle) pairs of `model`, and its handle order.
    fn agrees(index: &UniqueIndex<&str>, model: &BTreeSet<(Vec<u8>, u32)>, keys: &[&[u8]]) {
        index.check();
        for k in keys {
            let holders: Vec<u32> = model
                .iter()
                .filter(|(x, _)| x == k)
                .map(|(_, h)| *h)
                .collect();
            assert_eq!(index.first(k), holders.first().copied(), "{k:?}");
            assert_eq!(
                index.first_two(k),
                (holders.first().copied(), holders.len() > 1),
                "{k:?}"
            );
        }
        let mut dups = Vec::new();
        let mut winner: Option<&(Vec<u8>, u32)> = None;
        for pair in model {
            match winner {
                Some(w) if w.0 == pair.0 => dups.push((pair.1, w.1, pair.0.clone())),
                _ => winner = Some(pair),
            }
        }
        let got: Vec<_> = index
            .duplicates_with_keys()
            .map(|(h, w, k)| (h, w, k.to_vec()))
            .collect();
        assert_eq!(got, dups);
        assert_eq!(index.live_len(), model.len());
    }

    /// ws003 s19: seeded inserts and removes, several keys per handle and keys shared across handles, against a model
    /// of the live pairs, checked after every step through compactions; then a bulk build of the model's pairs, each
    /// pushed twice in reverse order, answers the same.
    #[test]
    fn removal_by_handle_matches_a_model_through_inserts_removes_and_compactions() {
        let keys: [&[u8]; 6] = [b"a", b"b", b"bb", b"c", b"", b"zz"];
        let mut compactions = 0;
        for seed in 1..=8u64 {
            let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let mut next = move || {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x
            };
            let mut index = UniqueIndex::new("k");
            let mut model = BTreeSet::new();
            for _ in 0..400 {
                let h = (next() % 12) as u32;
                let arena = index.arena.len();
                if next() % 3 == 0 {
                    index.remove(h);
                    model.retain(|(_, x)| *x != h);
                    compactions += usize::from(index.arena.len() < arena);
                } else {
                    let k = keys[(next() % 6) as usize];
                    index.insert(k, h).unwrap();
                    model.insert((k.to_vec(), h));
                }
                agrees(&index, &model, &keys);
            }
            let mut bulk = UniqueIndex::new("k");
            bulk.begin_build();
            for (k, h) in model.iter().rev().chain(&model) {
                bulk.push_unsorted(k, *h).unwrap();
            }
            bulk.finish_build();
            agrees(&bulk, &model, &keys);
        }
        assert!(compactions > 8, "{compactions}");
    }
}
