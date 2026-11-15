//! similar_to lists in memory (workshop 008 section 9; ws004 s09). For each node handle, up to k fixed slots of
//! (other handle, score in ten-thousandths), 8 bytes each, sorted best first (higher score, then lower handle, which
//! is lower id); plus the reverse list (who holds each node) as packed arrays rebuilt after each batch, like the
//! adjacency. Scores come from the whole-node block's int8 rows and inverse lengths only
//! (`umx_core::similar::score_units`), so a rebuild from committed shards and lists kept current one node at a time
//! agree to the last digit.
//!
//! Everything here changes only under the writer's `&mut Graph`; nothing is computed on a read. Deciding when to
//! refresh and writing the files is the similarity service's (umx-services), not this crate's.

use crate::Graph;
use crate::view::NodeIx;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::mem::size_of;
use umx_core::Id;
use umx_core::similar::score_units;
use umx_core::similarity::dot;

/// One similar_to edge from a list: in `similar_out(a)`, `other` is in a's top k with `score` (best first); in
/// `similar_in(b)`, `other` is a node whose top k holds b, with the score in that node's list (in id order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimilarEdge {
    pub other: NodeIx,
    /// Ten-thousandths (`umx_core::similar::format_units` writes it).
    pub score: i32,
}

/// Best first: the higher score, then the lower other handle (handle order is id order).
#[inline]
fn better(a: SimilarEdge, b: SimilarEdge) -> bool {
    a.score > b.score || (a.score == b.score && a.other < b.other)
}

#[derive(Clone, Default)]
pub(crate) struct Similar {
    /// The vector space the lists are computed in; empty when the graph holds no lists.
    space: String,
    k: usize,
    floor: i32,
    /// Row `h` is `slots[h * k..h * k + len[h]]`.
    slots: Vec<SimilarEdge>,
    len: Vec<u8>,
    /// Reverse list: node `h`'s holders are `rev[rev_at[h]..rev_at[h + 1]]`.
    rev_at: Vec<u32>,
    rev: Vec<SimilarEdge>,
    /// Lists that lost an entry to a removed node (now top k-1, not top k): recompute before they are trusted.
    stale: Vec<u32>,
}

impl Similar {
    fn rows(&self) -> usize {
        self.len.len()
    }

    fn list(&self, h: usize) -> &[SimilarEdge] {
        if h >= self.rows() {
            return &[];
        }
        &self.slots[h * self.k..h * self.k + self.len[h] as usize]
    }

    /// Makes sure rows `0..rows` exist (a node appended after the lists were made).
    fn ensure_rows(&mut self, rows: usize) {
        if rows > self.rows() {
            self.slots.resize(
                rows * self.k,
                SimilarEdge {
                    other: NodeIx(0),
                    score: 0,
                },
            );
            self.len.resize(rows, 0);
        }
    }

    fn set_list(&mut self, h: usize, list: &[SimilarEdge]) {
        debug_assert!(list.len() <= self.k);
        debug_assert!(list.windows(2).all(|w| better(w[0], w[1])));
        self.ensure_rows(h + 1);
        self.slots[h * self.k..h * self.k + list.len()].copy_from_slice(list);
        self.len[h] = list.len() as u8;
    }

    /// Puts `e` into row `h` in order if it belongs in a top k: true if the list changed. `e.other` is not in it.
    fn offer(&mut self, h: usize, e: SimilarEdge) -> bool {
        self.ensure_rows(h + 1);
        let (k, len) = (self.k, self.len[h] as usize);
        // A full list whose worst entry is at least as good: most candidates stop here.
        if len == k && (k == 0 || !better(e, self.slots[h * k + k - 1])) {
            return false;
        }
        let row = &mut self.slots[h * k..h * k + k];
        let at = row[..len].partition_point(|x| better(*x, e));
        if at == k {
            return false;
        }
        let end = (len + 1).min(k);
        row.copy_within(at..end - 1, at + 1);
        row[at] = e;
        self.len[h] = end as u8;
        true
    }

    /// Rebuilds the reverse list from every row.
    fn rebuild_reverse(&mut self) {
        let n = self.rows();
        let mut counts = vec![0u32; n + 1];
        for h in 0..n {
            for e in self.list(h) {
                counts[e.other.index() + 1] += 1;
            }
        }
        for i in 0..n {
            counts[i + 1] += counts[i];
        }
        let mut rev = vec![
            SimilarEdge {
                other: NodeIx(0),
                score: 0
            };
            counts[n] as usize
        ];
        let mut at = counts.clone();
        for h in 0..n {
            for e in self.list(h) {
                let slot = &mut at[e.other.index()];
                rev[*slot as usize] = SimilarEdge {
                    other: NodeIx(h as u32),
                    score: e.score,
                };
                *slot += 1;
            }
        }
        // Rows are visited in handle order, so each node's holders are in handle (id) order already.
        self.rev_at = counts;
        self.rev = rev;
    }

    /// Node handles were renumbered: `map[old]` is the new handle, `u32::MAX` for a node that is gone. Rows move,
    /// and so do the handles inside them; a list that held a removed node drops it and is marked stale.
    pub(crate) fn remap(&mut self, map: &[u32]) {
        if self.space.is_empty() {
            return;
        }
        let rows = map.iter().filter(|&&m| m != u32::MAX).count();
        let mut moved = Similar {
            space: core::mem::take(&mut self.space),
            k: self.k,
            floor: self.floor,
            slots: Vec::with_capacity(rows * self.k),
            len: Vec::with_capacity(rows),
            ..Similar::default()
        };
        moved.ensure_rows(rows);
        let mut stale: Vec<u32> = Vec::new();
        for (old, &new) in map.iter().enumerate() {
            if new == u32::MAX {
                continue;
            }
            let mut kept: Vec<SimilarEdge> = Vec::new();
            for e in self.list(old) {
                match map.get(e.other.index()).copied() {
                    Some(o) if o != u32::MAX => kept.push(SimilarEdge {
                        other: NodeIx(o),
                        score: e.score,
                    }),
                    _ => stale.push(new),
                }
            }
            moved.set_list(new as usize, &kept);
        }
        for s in &self.stale {
            if let Some(&n) = map.get(*s as usize).filter(|&&n| n != u32::MAX) {
                stale.push(n);
            }
        }
        stale.sort_unstable();
        stale.dedup();
        moved.stale = stale;
        moved.rebuild_reverse();
        *self = moved;
    }

    /// A node was inserted at handle `h` of `nodes` (the count before).
    pub(crate) fn inserted(&mut self, h: usize, nodes: usize) {
        if !self.space.is_empty() {
            let map: Vec<u32> = (0..nodes as u32)
                .map(|old| old + u32::from(old as usize >= h))
                .collect();
            self.remap(&map);
        }
    }

    /// Node `h` of `nodes` (the count before) was removed.
    pub(crate) fn removed(&mut self, h: usize, nodes: usize) {
        if !self.space.is_empty() {
            let map: Vec<u32> = (0..nodes as u32)
                .map(|old| match (old as usize).cmp(&h) {
                    core::cmp::Ordering::Less => old,
                    core::cmp::Ordering::Equal => u32::MAX,
                    core::cmp::Ordering::Greater => old - 1,
                })
                .collect();
            self.remap(&map);
        }
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.space.capacity()
            + self.slots.capacity() * size_of::<SimilarEdge>()
            + self.len.capacity()
            + self.rev_at.capacity() * size_of::<u32>()
            + self.rev.capacity() * size_of::<SimilarEdge>()
            + self.stale.capacity() * size_of::<u32>()
    }
}

impl core::fmt::Debug for Similar {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Similar")
            .field("space", &self.space)
            .field("k", &self.k)
            .field("edges", &self.rev.len())
            .finish()
    }
}

/// A copy of one space's whole-node block, taken under a short read lock, so every node's top k can be computed with
/// no lock held (architect, ws004 s09) and applied only if the blocks did not change meanwhile.
#[derive(Clone)]
pub struct SimilarSnapshot {
    space: String,
    generation: u64,
    dims: usize,
    values: Vec<i8>,
    inv: Vec<f32>,
    present: Vec<u64>,
}

/// Every node's computed top k, ready to apply.
pub struct SimilarLists {
    space: String,
    generation: u64,
    k: usize,
    floor: i32,
    slots: Vec<SimilarEdge>,
    len: Vec<u8>,
}

impl SimilarLists {
    /// How many similar_to edges were computed.
    pub fn edges(&self) -> usize {
        self.len.iter().map(|&l| l as usize).sum()
    }
}

impl SimilarSnapshot {
    #[inline]
    fn has(&self, h: usize) -> bool {
        h < self.inv.len() && self.present[h / 64] & (1 << (h % 64)) != 0
    }

    fn row(&self, h: usize) -> &[i8] {
        &self.values[h * self.dims..(h + 1) * self.dims]
    }

    /// Every node's top `k` at or above `floor` (ten-thousandths). Each pair's int8 dot product is taken once and
    /// scored for both lists, each in its own order (its own inverse length first). Pure: needs no graph and no lock.
    pub fn compute(&self, k: usize, floor: i32) -> SimilarLists {
        let n = self.inv.len();
        let mut lists = Similar {
            space: self.space.clone(),
            k,
            floor,
            ..Similar::default()
        };
        lists.ensure_rows(n);
        if k > 0 {
            // The lowest score a list still takes (ten-thousandths): its floor until full, then its worst entry.
            let mut need = vec![floor; n];
            let present: Vec<usize> = (0..n).filter(|&h| self.has(h)).collect();
            for (i, &a) in present.iter().enumerate() {
                let (row_a, inv_a) = (self.row(a), self.inv[a]);
                for &b in &present[i + 1..] {
                    let d = dot(row_a, self.row(b));
                    // An f32 estimate first: a pair it puts more than a unit below what both lists need cannot enter
                    // either (its error is far below a unit), so the exact score is only taken near the cut.
                    let rough = d as f32 * inv_a * self.inv[b] * 10_000.0;
                    if rough < need[a].min(need[b]) as f32 - 1.0 {
                        continue;
                    }
                    for (x, y, inv_x, inv_y) in
                        [(a, b, inv_a, self.inv[b]), (b, a, self.inv[b], inv_a)]
                    {
                        let score = score_units(d, inv_x, inv_y);
                        if score >= need[x]
                            && lists.offer(
                                x,
                                SimilarEdge {
                                    other: NodeIx(y as u32),
                                    score,
                                },
                            )
                            && lists.len[x] as usize == k
                        {
                            need[x] = lists.slots[x * k + k - 1].score;
                        }
                    }
                }
            }
        }
        SimilarLists {
            space: self.space.clone(),
            generation: self.generation,
            k,
            floor,
            slots: lists.slots,
            len: lists.len,
        }
    }
}

/// Loads a graph's stored similar_to lines into its lists (see `Graph::load_similar`). Lines arrive in file order,
/// sorted by (node, other); each list is sorted by score when finished or dropped.
pub struct SimilarLoad<'g> {
    g: &'g mut Graph,
    done: bool,
}

impl SimilarLoad<'_> {
    /// One stored edge. False, and nothing held, when either node is not in the graph, it names itself, the list
    /// already holds that other node, or the list is full (k from the settings): such a file is stale, which the
    /// `_meta.tsv` check catches.
    pub fn put(&mut self, node: Id, other: Id, score: i32) -> bool {
        let (Some(a), Some(b)) = (self.g.handle(node), self.g.handle(other)) else {
            return false;
        };
        if self.g.provenance.has(a) || self.g.provenance.has(b) {
            return false;
        }
        let s = &mut self.g.similar;
        let (a, k) = (a as usize, s.k);
        if a == b as usize || s.list(a).iter().any(|e| e.other.0 == b) {
            return false;
        }
        s.ensure_rows(a + 1);
        let len = s.len[a] as usize;
        if len == k {
            return false;
        }
        s.slots[a * k + len] = SimilarEdge {
            other: NodeIx(b),
            score,
        };
        s.len[a] += 1;
        true
    }

    /// Sorts every list best first and builds the reverse list.
    pub fn finish(mut self) {
        self.settle();
    }

    fn settle(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        let s = &mut self.g.similar;
        s.ensure_rows(self.g.ids.len());
        let k = s.k;
        for h in 0..s.rows() {
            let len = s.len[h] as usize;
            s.slots[h * k..h * k + len]
                .sort_unstable_by(|a, b| b.score.cmp(&a.score).then(a.other.cmp(&b.other)));
        }
        s.rebuild_reverse();
    }
}

impl Drop for SimilarLoad<'_> {
    fn drop(&mut self) {
        self.settle();
    }
}

impl Graph {
    /// The vector space the stored similar_to lists are computed in, if the graph holds lists.
    pub fn similar_space(&self) -> Option<&str> {
        (!self.similar.space.is_empty()).then_some(self.similar.space.as_str())
    }

    /// The k and floor (ten-thousandths) the lists were made with.
    pub fn similar_settings(&self) -> (usize, i32) {
        (self.similar.k, self.similar.floor)
    }

    /// A node's top k: the nodes in its list, best first (higher score, then lower id). Empty with no lists.
    #[inline]
    pub fn similar_out(&self, node: NodeIx) -> &[SimilarEdge] {
        if self.is_provenance(node) {
            return &[];
        }
        self.similar.list(node.index())
    }

    /// The nodes whose top k holds `node`, in id order, each with the score in its own list. Empty with no lists.
    #[inline]
    pub fn similar_in(&self, node: NodeIx) -> &[SimilarEdge] {
        if self.is_provenance(node) {
            return &[];
        }
        let h = node.index();
        if h + 1 >= self.similar.rev_at.len() {
            return &[];
        }
        &self.similar.rev[self.similar.rev_at[h] as usize..self.similar.rev_at[h + 1] as usize]
    }

    /// Lists that lost an entry when a node went (now top k-1): the service recomputes them.
    pub fn similar_stale(&self) -> impl Iterator<Item = NodeIx> + '_ {
        self.similar.stale.iter().map(|&h| NodeIx(h))
    }

    /// Starts loading stored lines for `space`, replacing any lists the graph held: every list empty, k and floor
    /// as the settings say. Put each line, then `finish`.
    pub fn load_similar(&mut self, space: &str, k: usize, floor: i32) -> SimilarLoad<'_> {
        self.similar = Similar {
            space: space.to_string(),
            k,
            floor,
            ..Similar::default()
        };
        let n = self.ids.len();
        self.similar.ensure_rows(n);
        SimilarLoad {
            g: self,
            done: false,
        }
    }

    /// Drops every list (the graph's similarity is turned off, or about to be rebuilt from nothing).
    pub fn clear_similar(&mut self) {
        self.similar = Similar::default();
    }

    /// A copy of `space`'s whole-node block for [`SimilarSnapshot::compute`], with the generation it was taken at.
    pub fn similar_snapshot(&self, space: &str) -> Option<SimilarSnapshot> {
        let block = self.whole_block(space)?;
        Some(SimilarSnapshot {
            space: space.to_string(),
            generation: self.vectors_generation(),
            dims: block.dims(),
            values: block.values().to_vec(),
            inv: block.inverse_lengths().to_vec(),
            present: (0..block.rows())
                .collect::<Vec<_>>()
                .chunks(64)
                .map(|c| {
                    c.iter().fold(0u64, |w, &r| {
                        w | (u64::from(block.has(r) && !self.provenance.has(r as u32)) << (r % 64))
                    })
                })
                .collect(),
        })
    }

    /// Replaces every list with `lists` (and the space, k and floor they were made with), if the blocks are still at
    /// the generation the snapshot was taken at; false, changing nothing, if they moved (the caller computes again).
    /// The generation only orders changes within one graph value: a caller that may load the graph again meanwhile
    /// must also tell loads apart (umx-services does, R102-1). Lists with more rows than the graph has nodes are
    /// refused too, never applied.
    pub fn apply_similar(&mut self, lists: SimilarLists) -> bool {
        if lists.generation != self.vectors_generation() || lists.len.len() > self.ids.len() {
            return false;
        }
        self.similar = Similar {
            space: lists.space,
            k: lists.k,
            floor: lists.floor,
            slots: lists.slots,
            len: lists.len,
            ..Similar::default()
        };
        self.similar.ensure_rows(self.ids.len());
        self.similar.rebuild_reverse();
        true
    }

    /// Node `a`'s top k from a full scan of the whole-node block, without changing anything.
    fn similar_scan(&self, a: usize) -> Vec<SimilarEdge> {
        let s = &self.similar;
        let Some(block) = self.whole_block(&s.space) else {
            return Vec::new();
        };
        if !block.has(a) || self.provenance.has(a as u32) {
            return Vec::new();
        }
        let (dims, values, inv) = (block.dims(), block.values(), block.inverse_lengths());
        let row = |h: usize| &values[h * dims..(h + 1) * dims];
        let mut top = Similar {
            k: s.k,
            ..Similar::default()
        };
        for b in
            (0..block.rows()).filter(|&b| b != a && block.has(b) && !self.provenance.has(b as u32))
        {
            let score = score_units(dot(row(a), row(b)), inv[a], inv[b]);
            if score >= s.floor {
                top.offer(
                    0,
                    SimilarEdge {
                        other: NodeIx(b as u32),
                        score,
                    },
                );
            }
        }
        top.list(0).to_vec()
    }

    /// Brings the lists up to date after `node`'s whole-node vector arrived, changed or went: its own list from one
    /// scan; from the same scores, each list it now enters; a list that held it at a lower score than before is
    /// recomputed in full, as is every list marked stale. Returns every node whose list changed, in handle order. The
    /// reverse list is rebuilt by [`Graph::finish_similar`], once per batch.
    pub fn refresh_similar(&mut self, node: NodeIx) -> Vec<NodeIx> {
        self.refresh_similar_batch(&[node])
    }

    /// [`Graph::refresh_similar`] for every node of a batch whose whole-node vector arrived, changed or went, with
    /// every list that must be recomputed in full (it held one of them at a lower score than before, or is marked
    /// stale) recomputed once for the whole batch, not once per node (graph-storage-expert). Exact, because
    /// every vector of the batch is in place before it runs: a list put aside for a full scan is scanned against the
    /// final vectors, and every other list only gains entries or sees scores rise, which offers keep exact.
    pub fn refresh_similar_batch(&mut self, nodes: &[NodeIx]) -> Vec<NodeIx> {
        if self.similar.space.is_empty() {
            return Vec::new();
        }
        self.similar.ensure_rows(self.ids.len());
        let mut changed: Vec<usize> = Vec::new();
        let mut recompute: Vec<usize> = core::mem::take(&mut self.similar.stale)
            .into_iter()
            .map(|h| h as usize)
            .collect();
        for &node in nodes {
            self.refresh_one(node, &mut changed, &mut recompute);
        }
        recompute.sort_unstable();
        recompute.dedup();
        for b in recompute {
            let list = self.similar_scan(b);
            if list != self.similar.list(b) {
                self.similar.set_list(b, &list);
                changed.push(b);
            }
        }
        changed.sort_unstable();
        changed.dedup();
        changed.into_iter().map(|h| NodeIx(h as u32)).collect()
    }

    /// One node's part of [`Graph::refresh_similar_batch`]: its own list and the lists it enters or moves up in;
    /// lists that held it at a lower score go in `recompute`.
    fn refresh_one(&mut self, node: NodeIx, changed: &mut Vec<usize>, recompute: &mut Vec<usize>) {
        let a = node.index();
        let n = self.ids.len();
        let k = self.similar.k;
        // One pass: each other node's dot product with `a`, scored for `a`'s list and for the other's.
        let (k_, floor) = (k, self.similar.floor);
        let mut own = Similar {
            k: k_,
            ..Similar::default()
        };
        let entries: Vec<(usize, Option<i32>)> = match self.whole_block(&self.similar.space) {
            Some(block) if block.has(a) && !self.provenance.has(a as u32) => {
                let (dims, values, inv) = (block.dims(), block.values(), block.inverse_lengths());
                let row = |h: usize| &values[h * dims..(h + 1) * dims];
                let mut entries = Vec::with_capacity(block.count());
                for b in (0..block.rows())
                    .filter(|&b| b != a && block.has(b) && !self.provenance.has(b as u32))
                {
                    let d = dot(row(a), row(b));
                    let ab = score_units(d, inv[a], inv[b]);
                    if ab >= floor {
                        own.offer(
                            0,
                            SimilarEdge {
                                other: NodeIx(b as u32),
                                score: ab,
                            },
                        );
                    }
                    entries.push((b, Some(score_units(d, inv[b], inv[a]))));
                }
                entries
            }
            // No vector: `a` has no list and leaves every list that holds it.
            _ => (0..n).map(|b| (b, None)).collect(),
        };
        if own.list(0) != self.similar.list(a) {
            self.similar.set_list(a, own.list(0));
            changed.push(a);
        }
        for (b, score) in entries {
            let held = self
                .similar
                .list(b)
                .iter()
                .position(|e| e.other.index() == a);
            match (held, score) {
                (Some(i), Some(s)) if s >= self.similar.list(b)[i].score => {
                    // Better than before: still in the top k; move it up.
                    let mut list = self.similar.list(b).to_vec();
                    list.remove(i);
                    let mut fresh = Similar {
                        k,
                        ..Similar::default()
                    };
                    fresh.set_list(0, &list);
                    fresh.offer(
                        0,
                        SimilarEdge {
                            other: node,
                            score: s,
                        },
                    );
                    if fresh.list(0) != self.similar.list(b) {
                        self.similar.set_list(b, fresh.list(0));
                        changed.push(b);
                    }
                }
                (Some(_), _) => recompute.push(b),
                (None, Some(s)) if s >= self.similar.floor => {
                    if self.similar.offer(
                        b,
                        SimilarEdge {
                            other: node,
                            score: s,
                        },
                    ) {
                        changed.push(b);
                    }
                }
                (None, _) => {}
            }
        }
    }

    /// Recomputes the lists marked stale (they lost an entry when a node went); returns those that changed.
    pub fn refresh_stale_similar(&mut self) -> Vec<NodeIx> {
        let stale = core::mem::take(&mut self.similar.stale);
        let mut changed = Vec::new();
        for h in stale {
            let list = self.similar_scan(h as usize);
            if list != self.similar.list(h as usize) {
                self.similar.set_list(h as usize, &list);
                changed.push(NodeIx(h));
            }
        }
        changed
    }

    /// Rebuilds the reverse list after a batch of [`Graph::refresh_similar`] calls.
    pub fn finish_similar(&mut self) {
        if !self.similar.space.is_empty() {
            let n = self.ids.len();
            self.similar.ensure_rows(n);
            self.similar.rebuild_reverse();
        }
    }
}
