//! Vectors in memory (workshop 008 section 2.5, blueprint d-15). A vector space is `model@dims`; it holds one
//! contiguous `i8` block per field. The whole-node block's row is the node handle, with a presence bit per node (a
//! node whose vector is pending or failed keeps its row zeroed and its bit clear). A field block holds only the rows
//! that exist, sorted by handle, each with its `u32` handle. Every row carries its inverse length, so an int8 dot
//! product becomes a cosine (umx_core::similarity). No `Vec` per vector, no map from id to row, no f32 copy.
//!
//! Blocks change only under the writer's `&mut Graph`: loading committed shards, applying a finished embed job, and
//! the writes that renumber handles. Nothing here is built or grown on a read.

use crate::Graph;
use crate::view::NodeIx;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::mem::size_of;
use umx_core::Id;
use umx_core::similarity::inverse_length;
use umx_core::vectors::TextHash;

/// What became of one vector handed to the graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Put {
    Stored,
    /// No node has this id: the vector is not held (the shard line stays on disk; services warn `vector_orphan`).
    Orphan,
    /// Its length is not the space's dimensions: not held.
    WrongDims,
    /// Its block's size would overflow: refused before anything is allocated (security).
    TooLarge,
}

/// The values in `rows` rows of `dims`, or `None` when that overflows.
fn block_len(rows: usize, dims: usize) -> Option<usize> {
    rows.checked_mul(dims)
}

/// Every vector space of a graph, by name.
#[derive(Clone, Default)]
pub(crate) struct Vectors {
    spaces: Vec<Space>,
    /// Bumped by every change to any block, so work computed from a copy of the blocks (a similarity rebuild, ws004
    /// s09) can tell whether they moved while it ran.
    generation: u64,
}

#[derive(Clone)]
struct Space {
    name: String,
    dims: usize,
    whole: Whole,
    /// By field name.
    fields: Vec<Field>,
}

/// Rows `0..inv.len()` by node handle; a handle at or past the end has no vector. Each row also keeps the text hash of
/// the whole-node vector it holds (graph-storage-expert, ws004 s09): `_meta.tsv`'s digest reads it, and so does
/// "pending" (a row absent, or its hash not the node's current text's).
#[derive(Clone, Default)]
struct Whole {
    values: Vec<i8>,
    inv: Vec<f32>,
    present: Vec<u64>,
    hashes: Vec<[u8; 16]>,
}

#[derive(Clone)]
struct Field {
    name: String,
    /// Strictly increasing.
    handles: Vec<u32>,
    values: Vec<i8>,
    inv: Vec<f32>,
}

/// Grows `v` to `len`, leaving an eighth spare when it must reallocate, so a run of single appends (new nodes'
/// vectors arriving one job at a time) copies the block a bounded number of times without doubling it.
fn grow<T: Clone>(v: &mut Vec<T>, len: usize, fill: T) {
    if v.capacity() < len {
        v.reserve_exact(len.saturating_add(len / 8) - v.len());
    }
    v.resize(len, fill);
}

impl Whole {
    /// `rows` zeroed rows, or `None` (nothing allocated) when the size overflows.
    fn zeroed(rows: usize, dims: usize) -> Option<Whole> {
        Some(Whole {
            values: vec![0; block_len(rows, dims)?],
            inv: vec![0.0; rows],
            present: vec![0; rows.div_ceil(64)],
            hashes: vec![[0; 16]; rows],
        })
    }

    fn rows(&self) -> usize {
        self.inv.len()
    }

    #[inline]
    fn has(&self, h: usize) -> bool {
        h < self.rows() && self.present[h / 64] & (1 << (h % 64)) != 0
    }

    fn count(&self) -> usize {
        self.present.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Sets row `h`. A block touched for the first time is sized to `nodes` rows at once (its rows are node handles,
    /// so its size is known); one that must grow past that later keeps an eighth spare. False, with nothing changed
    /// or allocated, when the block's size would overflow.
    #[must_use]
    fn set(&mut self, h: usize, row: &[i8], hash: TextHash, nodes: usize) -> bool {
        let dims = row.len();
        if self.rows() == 0 && h < nodes {
            let Some(zeroed) = Whole::zeroed(nodes, dims) else {
                return false;
            };
            *self = zeroed;
        }
        if self.rows() <= h {
            let Some(len) = h.checked_add(1).and_then(|r| block_len(r, dims)) else {
                return false;
            };
            grow(&mut self.values, len, 0);
            grow(&mut self.inv, h + 1, 0.0);
            grow(&mut self.present, (h + 1).div_ceil(64), 0);
            grow(&mut self.hashes, h + 1, [0; 16]);
        }
        self.values[h * dims..(h + 1) * dims].copy_from_slice(row);
        self.inv[h] = inverse_length(row);
        self.present[h / 64] |= 1 << (h % 64);
        self.hashes[h] = hash.0;
        true
    }

    fn clear(&mut self, h: usize, dims: usize) -> bool {
        if !self.has(h) {
            return false;
        }
        self.values[h * dims..(h + 1) * dims].fill(0);
        self.inv[h] = 0.0;
        self.present[h / 64] &= !(1 << (h % 64));
        self.hashes[h] = [0; 16];
        true
    }

    /// Keeps rows `0..rows`.
    fn truncate(&mut self, rows: usize, dims: usize) {
        if rows >= self.rows() {
            return;
        }
        self.values.truncate(rows * dims);
        self.inv.truncate(rows);
        self.hashes.truncate(rows);
        self.present.truncate(rows.div_ceil(64));
        if let Some(last) = self.present.last_mut()
            && !rows.is_multiple_of(64)
        {
            *last &= (1 << (rows % 64)) - 1;
        }
    }

    fn heap_bytes(&self) -> usize {
        self.values.capacity()
            + self.inv.capacity() * size_of::<f32>()
            + self.present.capacity() * size_of::<u64>()
            + self.hashes.capacity() * 16
    }
}

impl Field {
    fn new(name: &str) -> Field {
        Field {
            name: name.to_string(),
            handles: Vec::new(),
            values: Vec::new(),
            inv: Vec::new(),
        }
    }

    fn set(&mut self, h: u32, row: &[i8]) {
        let dims = row.len();
        match self.handles.binary_search(&h) {
            Ok(r) => {
                self.values[r * dims..(r + 1) * dims].copy_from_slice(row);
                self.inv[r] = inverse_length(row);
            }
            Err(r) => {
                self.handles.insert(r, h);
                self.inv.insert(r, inverse_length(row));
                self.values.splice(r * dims..r * dims, row.iter().copied());
            }
        }
    }

    fn remove(&mut self, h: u32, dims: usize) -> bool {
        let Ok(r) = self.handles.binary_search(&h) else {
            return false;
        };
        self.handles.remove(r);
        self.inv.remove(r);
        self.values.drain(r * dims..(r + 1) * dims);
        true
    }

    /// Moves row `from` to row `to` (handle, inverse length, values).
    fn move_row(&mut self, from: usize, to: usize, dims: usize) {
        self.handles[to] = self.handles[from];
        self.inv[to] = self.inv[from];
        self.values
            .copy_within(from * dims..(from + 1) * dims, to * dims);
    }

    fn truncate(&mut self, rows: usize, dims: usize) {
        self.handles.truncate(rows);
        self.inv.truncate(rows);
        self.values.truncate(rows * dims);
    }

    /// After a load appended rows in file order: sorts them by handle in place, the first of any repeated handle
    /// kept (req-36, as `umx embeddings repair` keeps the first). Extra memory is 8 bytes per row for the order, never a second block.
    fn settle(&mut self, dims: usize) {
        if self.handles.windows(2).all(|w| w[0] < w[1]) {
            return;
        }
        let n = self.handles.len();
        // (handle, row) sorted: equal handles in file order, so the first of each run is the one to keep.
        let mut order: Vec<u64> = self
            .handles
            .iter()
            .enumerate()
            .map(|(r, &h)| u64::from(h) << 32 | r as u64)
            .collect();
        order.sort_unstable();
        let mut dropped = Vec::new();
        let mut kept = 0;
        let mut last = None;
        for i in 0..n {
            let (handle, row) = (order[i] >> 32, order[i] & 0xffff_ffff);
            if last == Some(handle) {
                dropped.push(row);
            } else {
                order[kept] = row;
                kept += 1;
            }
            last = Some(handle);
        }
        order.truncate(kept);
        order.extend(dropped);
        // A full gather order now: new row j is old row order[j]. Apply it by cycles with one spare row.
        const DONE: u64 = u64::MAX;
        let mut spare = vec![0i8; dims];
        for start in 0..n {
            if order[start] == DONE {
                continue;
            }
            let (spare_h, spare_inv) = (self.handles[start], self.inv[start]);
            spare.copy_from_slice(&self.values[start * dims..(start + 1) * dims]);
            let mut j = start;
            loop {
                let from = order[j] as usize;
                order[j] = DONE;
                if from == start {
                    self.handles[j] = spare_h;
                    self.inv[j] = spare_inv;
                    self.values[j * dims..(j + 1) * dims].copy_from_slice(&spare);
                    break;
                }
                self.move_row(from, j, dims);
                j = from;
            }
        }
        self.truncate(kept, dims);
    }

    fn shrink(&mut self) {
        self.handles.shrink_to_fit();
        self.inv.shrink_to_fit();
        self.values.shrink_to_fit();
    }

    fn heap_bytes(&self) -> usize {
        self.name.capacity()
            + self.handles.capacity() * size_of::<u32>()
            + self.values.capacity()
            + self.inv.capacity() * size_of::<f32>()
    }
}

impl Space {
    fn new(name: &str, dims: usize) -> Space {
        Space {
            name: name.to_string(),
            dims,
            whole: Whole::default(),
            fields: Vec::new(),
        }
    }

    fn field_mut(&mut self, name: &str) -> &mut Field {
        let i = match self.fields.binary_search_by(|f| f.name.as_str().cmp(name)) {
            Ok(i) => i,
            Err(i) => {
                self.fields.insert(i, Field::new(name));
                i
            }
        };
        &mut self.fields[i]
    }

    fn is_empty(&self) -> bool {
        self.whole.count() == 0 && self.fields.iter().all(|f| f.handles.is_empty())
    }

    fn heap_bytes(&self) -> usize {
        self.name.capacity()
            + self.whole.heap_bytes()
            + self.fields.capacity() * size_of::<Field>()
            + self.fields.iter().map(Field::heap_bytes).sum::<usize>()
    }
}

impl Vectors {
    pub(crate) fn membership_changed(&mut self) {
        self.generation += 1;
    }

    fn find(&self, space: &str) -> Result<usize, usize> {
        self.spaces.binary_search_by(|s| s.name.as_str().cmp(space))
    }

    fn space(&self, space: &str) -> Option<&Space> {
        self.find(space).ok().map(|i| &self.spaces[i])
    }

    /// Node handles are renumbered (a rebuild): `map[old]` is the new handle, `u32::MAX` for a node that is gone.
    /// The map keeps order, so field blocks stay sorted.
    pub(crate) fn remap(&mut self, map: &[u32]) {
        self.generation += 1;
        for s in &mut self.spaces {
            let dims = s.dims;
            let moved = |old: usize| map.get(old).copied().filter(|&n| n != u32::MAX);
            let held = (0..s.whole.rows()).filter(|&h| s.whole.has(h));
            let rows = held
                .clone()
                .filter_map(moved)
                .max()
                .map_or(0, |m| m as usize + 1);
            // At most as many rows as the block held already, so the size cannot overflow.
            let mut whole =
                Whole::zeroed(rows, dims).expect("no larger than the block it replaces");
            for old in held {
                if let Some(new) = moved(old) {
                    let row = &s.whole.values[old * dims..(old + 1) * dims];
                    let set = whole.set(new as usize, row, TextHash(s.whole.hashes[old]), rows);
                    debug_assert!(set);
                }
            }
            s.whole = whole;
            for f in &mut s.fields {
                let mut kept = 0;
                for r in 0..f.handles.len() {
                    let new = map.get(f.handles[r] as usize).copied().unwrap_or(u32::MAX);
                    if new != u32::MAX {
                        f.move_row(r, kept, dims);
                        f.handles[kept] = new;
                        kept += 1;
                    }
                }
                f.truncate(kept, dims);
            }
            s.fields.retain(|f| !f.handles.is_empty());
        }
        self.drop_empty();
    }

    /// A node was inserted at handle `h` of `nodes` (the count before): rows from `h` on move up one.
    pub(crate) fn inserted(&mut self, h: usize, nodes: usize) {
        if !self.spaces.is_empty() {
            let map: Vec<u32> = (0..nodes as u32)
                .map(|old| old + u32::from(old as usize >= h))
                .collect();
            self.remap(&map);
        }
    }

    /// Node `h` of `nodes` (the count before), not the last, was removed: its rows go, later rows move down one.
    pub(crate) fn removed(&mut self, h: usize, nodes: usize) {
        if !self.spaces.is_empty() {
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

    /// The last node (handle `h`) was removed.
    pub(crate) fn drop_last(&mut self, h: u32) {
        self.generation += 1;
        for s in &mut self.spaces {
            s.whole.truncate(h as usize, s.dims);
            for f in &mut s.fields {
                if f.handles.last() == Some(&h) {
                    let rows = f.handles.len() - 1;
                    f.truncate(rows, s.dims);
                }
            }
            s.fields.retain(|f| !f.handles.is_empty());
        }
        self.drop_empty();
    }

    /// Drops spaces with no vector; a graph left with none holds no vector memory at all.
    fn drop_empty(&mut self) {
        self.spaces.retain(|s| !s.is_empty());
        if self.spaces.is_empty() {
            self.spaces = Vec::new();
        }
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.spaces.capacity() * size_of::<Space>()
            + self.spaces.iter().map(Space::heap_bytes).sum::<usize>()
    }
}

impl core::fmt::Debug for Vectors {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut l = f.debug_list();
        for s in &self.spaces {
            l.entry(&(
                &s.name,
                s.whole.count(),
                s.fields
                    .iter()
                    .map(|f| (f.name.as_str(), f.handles.len()))
                    .collect::<Vec<_>>(),
            ));
        }
        l.finish()
    }
}

/// The whole-node block of a vector space: row `i` is the node with handle index `i`. Rows past `rows()` and rows
/// whose bit is clear have no vector. Good only for the `&Graph` borrow it came from.
#[derive(Clone, Copy)]
pub struct WholeBlock<'a> {
    dims: usize,
    block: &'a Whole,
}

impl<'a> WholeBlock<'a> {
    pub fn dims(self) -> usize {
        self.dims
    }

    /// Every row end to end, `rows() * dims()` values.
    pub fn values(self) -> &'a [i8] {
        &self.block.values
    }

    /// One per row; 0 for a row with no vector.
    pub fn inverse_lengths(self) -> &'a [f32] {
        &self.block.inv
    }

    pub fn rows(self) -> usize {
        self.block.rows()
    }

    /// Whether row `row` (a node handle's index) holds a vector.
    #[inline]
    pub fn has(self, row: usize) -> bool {
        self.block.has(row)
    }

    /// The node of a row.
    #[inline]
    pub fn node(self, row: u32) -> NodeIx {
        debug_assert!((row as usize) < self.rows());
        NodeIx(row)
    }

    /// The text hash of the whole-node vector in row `row`, if it holds one.
    #[inline]
    pub fn hash(self, row: usize) -> Option<TextHash> {
        self.block
            .has(row)
            .then(|| TextHash(self.block.hashes[row]))
    }

    /// How many nodes have a vector.
    pub fn count(self) -> usize {
        self.block.count()
    }
}

/// One field's block of a vector space: only the rows that exist, sorted by node handle. Good only for the
/// `&Graph` borrow it came from.
#[derive(Clone, Copy)]
pub struct FieldBlock<'a> {
    dims: usize,
    block: &'a Field,
}

impl<'a> FieldBlock<'a> {
    pub fn field(self) -> &'a str {
        &self.block.name
    }

    pub fn dims(self) -> usize {
        self.dims
    }

    pub fn values(self) -> &'a [i8] {
        &self.block.values
    }

    pub fn inverse_lengths(self) -> &'a [f32] {
        &self.block.inv
    }

    pub fn rows(self) -> usize {
        self.block.handles.len()
    }

    /// The node of a row.
    #[inline]
    pub fn node(self, row: u32) -> NodeIx {
        NodeIx(self.block.handles[row as usize])
    }
}

/// Loads one vector space's committed vectors into blocks (see `Graph::load_vectors`). Put each decoded shard line
/// as it is read; the blocks are sorted and trimmed when this is finished or dropped.
pub struct VectorLoad<'g> {
    g: &'g mut Graph,
    space: usize,
    /// Set once the blocks are sorted and trimmed: how many vectors the space holds.
    settled: Option<usize>,
}

impl VectorLoad<'_> {
    /// One vector: `field` is `None` for the whole-node vector. A repeated (node, field) keeps the first put (req-36:
    /// repair keeps the first line of a repeat too).
    /// `hash` is the line's text hash; the whole-node row keeps it.
    pub fn put(&mut self, id: Id, field: Option<&str>, values: &[i8], hash: TextHash) -> Put {
        let Some(h) = self.g.handle(id) else {
            return Put::Orphan;
        };
        let n = self.g.ids.len();
        let s = &mut self.g.vectors.spaces[self.space];
        if values.len() != s.dims {
            return Put::WrongDims;
        }
        match field {
            None => {
                // One row per node, zeroed (pending until a vector arrives), allocated once.
                if !s.whole.has(h as usize) && !s.whole.set(h as usize, values, hash, n) {
                    return Put::TooLarge;
                }
            }
            Some(name) => {
                let dims = s.dims;
                let f = s.field_mut(name);
                if f.handles.capacity() == 0 {
                    // At most one row per node: reserve that once rather than doubling a block while it fills.
                    let Some(len) = block_len(n, dims) else {
                        return Put::TooLarge;
                    };
                    f.handles.reserve_exact(n);
                    f.inv.reserve_exact(n);
                    f.values.reserve_exact(len);
                }
                f.handles.push(h);
                f.inv.push(inverse_length(values));
                f.values.extend_from_slice(values);
            }
        }
        Put::Stored
    }

    /// The graph being loaded into, read only: its nodes are all there while vectors arrive.
    pub fn graph(&self) -> &Graph {
        self.g
    }

    /// Gives up the load (a shard could not be read): the space is left with no vectors, never half loaded.
    pub fn abandon(mut self) {
        let s = &mut self.g.vectors.spaces[self.space];
        s.whole = Whole::default();
        s.fields.clear();
        self.settle();
    }

    /// Sorts and trims the blocks; returns how many vectors the space holds.
    pub fn finish(mut self) -> usize {
        self.settle()
    }

    fn settle(&mut self) -> usize {
        if let Some(count) = self.settled {
            return count;
        }
        let s = &mut self.g.vectors.spaces[self.space];
        let dims = s.dims;
        for f in &mut s.fields {
            f.settle(dims);
            f.shrink();
            assert!(f.values.len() == f.inv.len() * dims && f.handles.len() == f.inv.len());
        }
        assert_eq!(s.whole.values.len(), s.whole.inv.len() * dims);
        if s.whole.count() == 0 {
            s.whole = Whole::default();
        }
        let count = s.whole.count() + s.fields.iter().map(|f| f.handles.len()).sum::<usize>();
        if count == 0 {
            self.g.vectors.drop_empty();
        }
        self.settled = Some(count);
        count
    }
}

impl Drop for VectorLoad<'_> {
    fn drop(&mut self) {
        self.settle();
    }
}

impl Graph {
    /// Starts loading the committed vectors of one vector space (`model@dims`, `dims` values each), replacing any
    /// the space held. Put every vector line, then `finish`. Only the writer calls this, under `&mut Graph`.
    pub fn load_vectors(&mut self, space: &str, dims: usize) -> VectorLoad<'_> {
        self.vectors.generation += 1;
        let i = match self.vectors.find(space) {
            Ok(i) => {
                self.vectors.spaces[i] = Space::new(space, dims);
                i
            }
            Err(i) => {
                self.vectors.spaces.insert(i, Space::new(space, dims));
                i
            }
        };
        VectorLoad {
            g: self,
            space: i,
            settled: None,
        }
    }

    /// Applies one finished vector (`field` `None` for the whole-node vector), replacing the one it had. A space
    /// seen for the first time takes its dimensions from this vector. `hash` is the text hash of what was embedded;
    /// the whole-node row keeps it.
    pub fn set_vector(
        &mut self,
        space: &str,
        id: Id,
        field: Option<&str>,
        values: &[i8],
        hash: TextHash,
    ) -> Put {
        let Some(h) = self.handle(id) else {
            return Put::Orphan;
        };
        self.vectors.generation += 1;
        let i = match self.vectors.find(space) {
            Ok(i) => i,
            Err(i) => {
                if values.is_empty() {
                    return Put::WrongDims;
                }
                self.vectors
                    .spaces
                    .insert(i, Space::new(space, values.len()));
                i
            }
        };
        let s = &mut self.vectors.spaces[i];
        if values.len() != s.dims {
            return Put::WrongDims;
        }
        match field {
            None if !s.whole.set(h as usize, values, hash, self.ids.len()) => return Put::TooLarge,
            None => {}
            Some(name) => s.field_mut(name).set(h, values),
        }
        Put::Stored
    }

    /// Drops one vector; true if there was one.
    pub fn remove_vector(&mut self, space: &str, id: Id, field: Option<&str>) -> bool {
        let (Some(h), Ok(i)) = (self.handle(id), self.vectors.find(space)) else {
            return false;
        };
        self.vectors.generation += 1;
        let s = &mut self.vectors.spaces[i];
        let removed = match field {
            None => s.whole.clear(h as usize, s.dims),
            Some(name) => {
                let dims = s.dims;
                match s.fields.iter_mut().position(|f| f.name == name) {
                    Some(f) => {
                        let removed = s.fields[f].remove(h, dims);
                        if s.fields[f].handles.is_empty() {
                            s.fields.remove(f);
                        }
                        removed
                    }
                    None => false,
                }
            }
        };
        if s.is_empty() {
            self.vectors.drop_empty();
        }
        removed
    }

    /// Changes on every block change (a load, an applied or removed vector, a renumbering), never on a read: a copy
    /// of the blocks and provenance membership is still current while the generation is the same.
    pub fn vectors_generation(&self) -> u64 {
        self.vectors.generation
    }

    /// The vector spaces the graph holds vectors for, in name order.
    pub fn vector_spaces(&self) -> impl Iterator<Item = &str> {
        self.vectors.spaces.iter().map(|s| s.name.as_str())
    }

    /// A space's whole-node block, if it holds any whole-node vector.
    pub fn whole_block(&self, space: &str) -> Option<WholeBlock<'_>> {
        let s = self.vectors.space(space)?;
        assert_eq!(
            s.whole.values.len(),
            s.whole.inv.len() * s.dims,
            "a whole-node block's shape"
        );
        (s.whole.rows() > 0).then_some(WholeBlock {
            dims: s.dims,
            block: &s.whole,
        })
    }

    /// A space's field blocks, in field name order.
    pub fn field_blocks(&self, space: &str) -> impl Iterator<Item = FieldBlock<'_>> {
        self.vectors.space(space).into_iter().flat_map(|s| {
            s.fields.iter().map(|f| FieldBlock {
                dims: s.dims,
                block: f,
            })
        })
    }

    /// One node's vector (`field` `None` for the whole-node vector).
    pub fn vector(&self, space: &str, node: NodeIx, field: Option<&str>) -> Option<&[i8]> {
        let s = self.vectors.space(space)?;
        let h = node.index();
        let (dims, row, values) = match field {
            None => (s.dims, s.whole.has(h).then_some(h)?, &s.whole.values),
            Some(name) => {
                let f = &s.fields[s
                    .fields
                    .binary_search_by(|f| f.name.as_str().cmp(name))
                    .ok()?];
                (s.dims, f.handles.binary_search(&node.0).ok()?, &f.values)
            }
        };
        Some(&values[row * dims..(row + 1) * dims])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Security: a block whose size overflows is refused before anything is allocated, in release too.
    #[test]
    fn an_overflowing_block_size_is_refused_and_never_allocated() {
        assert_eq!(block_len(usize::MAX / 2, 3), None);
        assert_eq!(block_len(1000, 256), Some(256_000));
        assert!(Whole::zeroed(usize::MAX / 2, 4).is_none());
        let mut whole = Whole::default();
        let h = TextHash([0; 16]);
        assert!(!whole.set(usize::MAX / 2, &[1, 2, 3, 4], h, usize::MAX));
        assert!(
            !whole.set(usize::MAX - 1, &[1, 2, 3, 4], h, 0),
            "growing past the end overflows too"
        );
        assert_eq!(
            (whole.rows(), whole.values.capacity()),
            (0, 0),
            "nothing allocated"
        );
        assert!(whole.set(2, &[1, 2, 3, 4], h, 3));
        assert_eq!(whole.rows(), 3);
    }
}
