//! Many writes as one (req-88; the ws003 s07 storage design, sections 2 and 3). Inside a batch a new edge's columns
//! are pushed without rebuilding adjacency, which is rebuilt once when the batch commits or rolls back. Every change
//! records its undo, so a rollback brings the graph back to its state before the batch: the image byte for byte, the
//! columns, the title text and the word index. Nothing reads adjacency inside a batch except [`Batch::find_edge`],
//! which checks each answer against the live columns (the caller keeps the batch's own edges).
//!
//! A removed edge (F75) is a tombstone until [`Batch::prepare`]: its row stays, so no edge handle moves and the stale
//! adjacency's entries stay valid all batch. `prepare` drops the tombstoned rows in the same one merge that places
//! out-of-order edges, before the graph is encoded, and the commit then builds adjacency once.
//!
//! Past its first few, a batch's node edit appends its new line to the image and leaves the old one as dead bytes
//! (review D1), as a fill does; `prepare` compacts once. A rollback that never compacted cuts the appended tails off,
//! newest first; after any compaction it puts every replaced line back from its bytes instead. While the image has
//! dead bytes, a new title or field name list of another length goes past its column's end too, instead of shifting
//! every row after it (s26); the compaction, or the end of a rollback that never compacted, settles the columns.

use crate::{EdgeRef, Graph, ImageFull};
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::ToString;
use alloc::vec::Vec;
use core::mem::size_of;
use umx_core::ui::Touched;
use umx_core::{Edge, Id, Node};

/// One change's undo is one `u32` in the log: its kind (eight kinds) in the top three bits and an index into that kind's table
/// below, so the log costs 4 bytes per change plus the ids and the replaced lines' bytes (storage design s3).
const KIND_SHIFT: u32 = 29;
const NODE: u32 = 0;
const LINE: u32 = 1;
const EDGE: u32 = 2;
const EDGE_WAS: u32 = 3;
const BULK: u32 = 4;
const BULK_EDGES: u32 = 5;
const FILL: u32 = 6;
const EDGE_GONE: u32 = 7;

/// A batch's first node edits splice while its image has no dead bytes (s18). One splice moves the tail of the image
/// in place (about 1 ms at 120 MB); a compaction copies the whole image into a fresh allocation (about 8 to 14 ms), so
/// appending pays only once a batch edits more than a few lines, and a single edit stays one splice.
const SPLICED_EDITS: usize = 8;

/// Where a row's line was before the batch appended its new one (a fill or an edit): undone newest first, by cutting
/// the tail off, while the batch has not compacted.
struct Tail {
    id: Id,
    old_at: u32,
    before: u32,
    after: u32,
    dead_before: u32,
}

/// What a committed batch changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub nodes_added: usize,
    pub nodes_replaced: usize,
    pub edges_added: usize,
    pub edges_replaced: usize,
    pub edges_removed: usize,
    /// Existing nodes whose provenance membership changed; refresh their lists only after commit.
    pub provenance_changed: Vec<Id>,
}

/// The undo log: changes in order, and per kind what puts each back.
#[derive(Default)]
struct Log {
    order: Vec<u32>,
    /// Nodes the batch added: drop them.
    nodes: Vec<Id>,
    /// Nodes the batch replaced, and each one's old line, exactly sized: put the line back byte for byte. An edit
    /// that appended its line also has its tail (None for a splice, which comes only after a compaction).
    lines: Vec<Id>,
    line_bytes: Vec<Box<[u8]>>,
    line_tails: Vec<Option<Tail>>,
    /// Edges the batch added: remove them.
    edges: Vec<Id>,
    /// Edges the batch replaced, as they were (never made by `umx batch`, which only adds new ids).
    edges_was: Vec<Edge>,
    bulks: Vec<Vec<Id>>,
    fills: usize,
    bulk_edges: Vec<Vec<Id>>,
    filled: Vec<Tail>,
    /// Edges the batch removed (tombstoned): clear the tombstone.
    gone: Vec<Id>,
}

impl Log {
    fn push(&mut self, kind: u32, index: usize) {
        debug_assert!(index < 1 << KIND_SHIFT, "a batch of over 2^29 changes");
        self.order.push((kind << KIND_SHIFT) | index as u32);
    }
}

/// A batch of writes to one graph. Commit it, or roll it back; dropped without either, it rolls back, so memory is
/// never left half applied.
pub struct Batch<'g> {
    g: &'g mut Graph,
    log: Log,
    /// The graph's one-slot undo line as it was before the batch, put back by a rollback.
    undo: Option<(Id, Vec<u8>)>,
    /// Whether an edge changed, so adjacency needs its one rebuild.
    edges: bool,
    provenance_changed: Vec<Id>,
    pending_edges: BTreeMap<Id, Edge>,
    pending_replacements: usize,
    /// The edges removed and not yet dropped from the columns, by id (ids never move; handles may on the rare add).
    gone: BTreeSet<Id>,
    /// Out-of-order edges the batch added and then removed before `prepare` placed them: never in the graph.
    unborn: BTreeSet<Id>,
    /// The records `prepare` dropped, so a rollback after it (a failed write) puts them back in one pass.
    dropped: Vec<Edge>,
    compacted: bool,
    peak_dead: usize,
    open: bool,
}

impl Graph {
    /// Starts a batch of writes (see [`Batch`]).
    pub fn batch(&mut self) -> Batch<'_> {
        debug_assert!(
            self.batch_placeholders.is_empty(),
            "stale batch placeholders"
        );
        let undo = self.undo.take();
        Batch {
            g: self,
            log: Log::default(),
            undo,
            edges: false,
            provenance_changed: Vec::new(),
            pending_edges: BTreeMap::new(),
            pending_replacements: 0,
            gone: BTreeSet::new(),
            unborn: BTreeSet::new(),
            dropped: Vec::new(),
            compacted: false,
            peak_dead: 0,
            open: true,
        }
    }

    /// The largest node or edge id the graph holds, so a dry run can number its new rows above every id without
    /// drawing real ones.
    pub fn last_id(&self) -> Option<Id> {
        self.ids
            .last()
            .copied()
            .into_iter()
            .chain(self.e_ids.last().copied())
            .chain(self.kept_nodes.iter().map(|k| k.0))
            .chain(self.kept_edges.iter().map(|k| k.id))
            .max()
    }

    /// Whether any live, tombstoned or kept node/edge record holds this identity.
    pub fn holds_id(&self, id: Id) -> bool {
        self.handle(id).is_some()
            || self.edge_handle(id).is_some()
            || self.kept_nodes.iter().any(|k| k.0 == id)
            || self.kept_edges.iter().any(|k| k.id == id)
    }
}

impl Batch<'_> {
    /// The graph as the batch has changed it so far. Its title text and word index include the batch's nodes; its
    /// adjacency does not include the batch's edges until the batch commits.
    pub fn graph(&self) -> &Graph {
        self.g
    }

    pub fn changed_files(&self) -> impl Iterator<Item = umx_core::codec::GraphFile> {
        [
            (
                !self.log.nodes.is_empty()
                    || !self.log.lines.is_empty()
                    || !self.log.bulks.is_empty()
                    || !self.log.filled.is_empty(),
                umx_core::codec::GraphFile::Nodes,
            ),
            (
                self.edges || !self.pending_edges.is_empty(),
                umx_core::codec::GraphFile::Edges,
            ),
        ]
        .into_iter()
        .filter_map(|(changed, file)| changed.then_some(file))
    }

    /// Reserves new identities in one merge. Fill each at its own line before persisting.
    pub fn insert_placeholders(&mut self, mut ids: Vec<Id>, now: &str) -> Result<(), ImageFull> {
        if self.g.image_dead != 0 {
            self.compact();
        }
        ids.sort_unstable();
        assert!(
            ids.windows(2).all(|w| w[0] != w[1]),
            "repeated placeholder id"
        );
        assert!(
            ids.iter().all(|id| !self.g.holds_id(*id)),
            "held placeholder id"
        );
        if ids.is_empty() {
            return Ok(());
        }
        let nodes: Vec<Node> = ids
            .iter()
            .map(|&id| Node {
                id,
                kind: Default::default(),
                title: Default::default(),
                status: Default::default(),
                note: Default::default(),
                evidence: Default::default(),
                status_history: Default::default(),
                created: now.to_string(),
                updated: now.to_string(),
                created_by: Default::default(),
                locator: None,
                authored_by_session: None,
                edited_by_session: None,
                fields: Default::default(),
            })
            .collect();
        let lines: Vec<(Id, Vec<u8>)> = nodes
            .iter()
            .map(|n| (n.id, umx_core::codec::encode_nodes([n])))
            .collect();
        // Check the whole addition before even the append-only path changes anything.
        let growth = lines.iter().map(|(_, l)| l.len()).sum::<usize>();
        let size = self.g.image.len().saturating_add(growth);
        let limit = self.g.limit();
        if size > limit {
            return Err(ImageFull {
                size: size as u64,
                limit: limit as u64,
            });
        }
        if self.g.ids.last().is_none_or(|last| ids[0] > *last) {
            for n in nodes {
                self.g.add_node(self.g.ids.len(), &n)?;
            }
        } else {
            self.g.bulk_nodes(&lines, &BTreeSet::new())?;
        }
        self.g.batch_placeholders.extend(ids.iter().copied());
        self.log.push(BULK, self.log.bulks.len());
        self.log.bulks.push(ids);
        Ok(())
    }

    pub fn is_placeholder(&self, id: Id) -> bool {
        self.g.batch_placeholders.contains(&id)
    }

    pub fn unfilled(&self) -> usize {
        self.g.batch_placeholders.len()
    }

    pub fn fill(&mut self, id: Id, node: Node) -> Result<(), ImageFull> {
        assert_eq!(id, node.id, "filled another identity");
        assert!(
            self.g.batch_placeholders.contains(&id),
            "not an unfilled placeholder"
        );
        let h = self.g.handle(id).expect("a placeholder") as usize;
        let line = self.g.line_for(&node);
        let live = self.g.image.len() - self.g.image_dead;
        let size = live - self.g.line(h).len() + line.len();
        let limit = self.g.limit();
        if size > limit {
            return Err(ImageFull {
                size: size as u64,
                limit: limit as u64,
            });
        }
        if self.g.image.len().saturating_add(line.len() + 1) > limit {
            self.compact();
            let before = self.log.lines.len();
            self.insert_node(node)?;
            self.log.fills += self.log.lines.len() - before;
        } else {
            let undo = self.append(h, &line);
            self.log.push(FILL, self.log.filled.len());
            self.log.filled.push(undo);
        }
        self.g.batch_placeholders.remove(&id);
        Ok(())
    }

    fn compact(&mut self) {
        self.g.compact_image();
        self.compacted = true;
    }

    /// Appends row `h`'s new line (see `Graph::append_line`) and returns its undo. The caller checked the limit.
    fn append(&mut self, h: usize, line: &[u8]) -> Tail {
        let g = &mut *self.g;
        let tail = Tail {
            id: g.ids[h],
            old_at: g.line_at[h],
            before: g.image.len() as u32,
            after: (g.image.len() + line.len() + 1) as u32,
            dead_before: g.image_dead as u32,
        };
        g.append_line(h, line);
        self.peak_dead = self.peak_dead.max(g.image_dead);
        tail
    }

    /// A batch's edit of row `h` (review D1): the new line is appended and the old one is dead until `prepare`
    /// compacts, instead of a splice that moves every line after it. The refusal is `Graph::replace_node`'s, on the
    /// live bytes. A splice is left only for the batch's first [`SPLICED_EDITS`] edits on a clean image, when the
    /// line is unchanged, or when the appended line would not fit even compacted.
    fn replace(&mut self, h: usize, node: Node) -> Result<(Node, Option<Tail>), ImageFull> {
        if self.g.image_dead == 0 && self.log.lines.len() < SPLICED_EDITS {
            return self.g.replace_node(h, node).map(|old| (old, None));
        }
        let g = &mut *self.g;
        let old = g.node_record(h);
        if old == node {
            return Ok((old, None));
        }
        let line = g.line_for(&node);
        let size = g.image.len() - g.image_dead - g.line(h).len() + line.len();
        let limit = g.limit();
        if size > limit {
            return Err(ImageFull {
                size: size as u64,
                limit: limit as u64,
            });
        }
        if g.image.len().saturating_add(line.len() + 1) > limit {
            self.compact();
        }
        let g = &mut *self.g;
        if *line == *g.line(h) || g.image.len() + line.len() + 1 > limit {
            return g.replace_node(h, node).map(|old| (old, None));
        }
        let before = g.line(h).to_vec();
        let tail = self.append(h, &line);
        self.g.undo = Some((node.id, before));
        Ok((old, Some(tail)))
    }

    pub fn peak_dead_bytes(&self) -> usize {
        self.peak_dead
    }

    pub fn ready_to_persist(&self) -> bool {
        self.g.batch_placeholders.is_empty() && self.g.image_dead == 0
    }

    /// Materializes deferred edges and drops the removed edges' rows, in one merge, before the caller encodes the
    /// write set.
    pub fn prepare(&mut self) -> Result<(), Id> {
        self.compact();
        if self.pending_edges.is_empty() && self.gone.is_empty() {
            return Ok(());
        }
        let g = &mut *self.g;
        let dropped: Vec<Edge> = self
            .gone
            .iter()
            .map(|id| g.edge_record(g.edge_handle(*id).expect("a tombstoned row")))
            .collect();
        // Each tombstone gave up its name use; the merge takes one back for every row it removes.
        let uses = |g: &mut Graph, add: bool| {
            for e in &dropped {
                let h = g.edge_handle(e.id).expect("a tombstoned row");
                g.names.edge_use(g.e_kind.get(h), add);
            }
        };
        uses(g, true);
        let pending = core::mem::take(&mut self.pending_edges);
        let ids: Vec<Id> = pending.keys().copied().collect();
        if let Err(id) = g.merge_edge_records(pending.values().cloned(), &self.gone) {
            uses(g, false);
            self.pending_edges = pending;
            return Err(id);
        }
        if !ids.is_empty() {
            self.log.push(BULK_EDGES, self.log.bulk_edges.len());
            self.log.bulk_edges.push(ids);
        }
        self.gone.clear();
        self.dropped.extend(dropped);
        self.edges = true;
        Ok(())
    }

    /// An edge of the graph as the batch has changed it so far: None once the batch removed it. An out-of-order edge
    /// the batch added is not here until `prepare` places it (umx-services keeps the batch's own edges).
    pub fn edge(&self, id: Id) -> Option<EdgeRef<'_>> {
        self.g.edge(id).filter(|_| !self.gone.contains(&id))
    }

    /// The record of an edge as the batch has changed it so far, including an out-of-order edge it added and has not
    /// placed yet; None once the batch removed it.
    pub fn edge_record(&self, id: Id) -> Option<Edge> {
        self.pending_edges
            .get(&id)
            .cloned()
            .or_else(|| self.edge(id).map(|e| e.to_edge()))
    }

    /// `Graph::find_edge` as the batch has changed it so far, except the edges the batch added or moved, which the
    /// caller keeps. Adjacency is as it was before the batch (rows never move inside one), so each answer is checked
    /// against the live columns: a removed edge, or an edited edge at its old ends, never answers.
    pub fn find_edge(&self, from: Id, kind: &str, to: Id) -> Option<EdgeRef<'_>> {
        let g = &*self.g;
        let (f, t, k) = (g.handle(from)?, g.handle(to)?, g.names.get(kind)?);
        g.out
            .of(f)
            .iter()
            .map(|e| e.edge as usize)
            .find(|&h| {
                g.e_from[h] == f
                    && g.e_to[h] == t
                    && g.e_kind.get(h) == k
                    && !self.gone.contains(&g.e_ids[h])
            })
            .map(|h| EdgeRef { g, h: h as u32 })
    }

    /// Removes an edge as a tombstone (see the module docs), or drops an out-of-order edge the batch added and has
    /// not placed yet; returns it, or None, changing nothing, when no live edge has the id.
    pub fn remove_edge(&mut self, id: Id) -> Option<Edge> {
        if self.gone.contains(&id) {
            return None;
        }
        let old = match self.pending_edges.remove(&id) {
            Some(old) => {
                self.unborn.insert(id);
                old
            }
            None => {
                let g = &mut *self.g;
                let h = g.edge_handle(id)?;
                let old = g.edge_record(h);
                g.names.edge_use(g.e_kind.get(h), false);
                self.gone.insert(id);
                old
            }
        };
        self.edges = true;
        self.log.push(EDGE_GONE, self.log.gone.len());
        self.log.gone.push(id);
        Some(old)
    }

    /// `Graph::insert_node`, recording its undo.
    pub fn insert_node(&mut self, node: Node) -> Result<Option<Node>, ImageFull> {
        let added = self.g.handle(node.id).is_none();
        if self.g.image_dead != 0 && added {
            let line = self.g.line_for(&node);
            let middle = self.g.ids.last().is_some_and(|id| node.id < *id);
            if middle || self.g.image.len() + line.len() + 1 > self.g.limit() {
                self.compact();
            }
        }
        match self.g.ids.binary_search(&node.id) {
            Ok(h) => {
                let before: Box<[u8]> = self.g.line(h).into();
                let was_provenance = self.g.provenance.has(h as u32);
                let (old, tail) = self.replace(h, node)?;
                if was_provenance != self.g.provenance.has(h as u32) {
                    self.provenance_changed.push(old.id);
                }
                if self.g.line(h) != &*before {
                    self.log.push(LINE, self.log.lines.len());
                    self.log.lines.push(old.id);
                    self.log.line_bytes.push(before);
                    self.log.line_tails.push(tail);
                }
                Ok(Some(old))
            }
            Err(h) => {
                debug_assert!(
                    self.g
                        .kept_nodes
                        .binary_search_by_key(&node.id, |line| line.0)
                        .is_err(),
                    "a kept-only id cannot be inserted"
                );
                self.g.add_node(h, &node)?;
                self.log.push(NODE, self.log.nodes.len());
                self.log.nodes.push(node.id);
                Ok(None)
            }
        }
    }

    /// Adds an edge without rebuilding adjacency, or replaces one in place. Out-of-order ids are materialized by
    /// `prepare` or `commit`. Never an edge the batch removed.
    pub fn insert_edge(&mut self, edge: Edge) -> Option<Edge> {
        debug_assert!(!self.gone.contains(&edge.id), "an edge the batch removed");
        let g = &mut *self.g;
        let Some((from, to)) = g.edge_handles(edge.from_graph, edge.from, edge.to_graph, edge.to)
        else {
            debug_assert!(false, "Batch::insert_edge: a dangling edge {:?}", edge.id);
            return None;
        };
        if let Some(old) = self.pending_edges.get_mut(&edge.id) {
            self.pending_replacements += 1;
            return Some(core::mem::replace(old, edge));
        }
        let id = edge.id;
        let old = match g.e_ids.binary_search(&id) {
            Ok(h) => {
                self.edges = true;
                let old = g.edge_record(h);
                g.set_edge(h, edge, from, to);
                self.log.push(EDGE_WAS, self.log.edges_was.len());
                self.log.edges_was.push(old.clone());
                return Some(old);
            }
            Err(h) if h == g.e_ids.len() => {
                self.edges = true;
                g.push_edge(edge, from, to);
                None
            }
            Err(_) => {
                self.pending_edges.insert(id, edge);
                return None;
            }
        };
        self.log.push(EDGE, self.log.edges.len());
        self.log.edges.push(id);
        old
    }

    /// Heap bytes the undo log holds: one `u32` per change, the ids, and the replaced lines and records.
    pub fn undo_bytes(&self) -> usize {
        let l = &self.log;
        l.order.capacity() * size_of::<u32>()
            + (l.nodes.capacity() + l.lines.capacity() + l.edges.capacity()) * size_of::<Id>()
            + l.line_bytes.capacity() * size_of::<Box<[u8]>>()
            + l.line_bytes.iter().map(|b| b.len()).sum::<usize>()
            + l.edges_was.capacity() * size_of::<Edge>()
            + l.edges_was
                .iter()
                .map(|e| e.kind.capacity() + e.created.capacity() + e.created_by.capacity())
                .sum::<usize>()
            + l.bulks
                .iter()
                .map(|ids| ids.capacity() * size_of::<Id>())
                .sum::<usize>()
            + l.bulk_edges
                .iter()
                .map(|ids| ids.capacity() * size_of::<Id>())
                .sum::<usize>()
            + l.filled.capacity() * size_of::<Tail>()
            + l.line_tails.capacity() * size_of::<Option<Tail>>()
            + (l.gone.capacity() + self.gone.len()) * size_of::<Id>()
            + self.dropped.capacity() * size_of::<Edge>()
    }

    /// What the batch changed so far, read from its undo log (ws009 S5a, for the views' change feed): nodes added
    /// (placeholders and their fills included) or edited, and edges added (placed or still pending), edited or
    /// removed (F75). An edge the batch added and then removed is in none; a node or edge it added and then edited is
    /// added. A line rewritten to the same bytes is no edit.
    pub fn touched(&self) -> Touched {
        let l = &self.log;
        let mut t = Touched::default();
        t.nodes.added = l
            .nodes
            .iter()
            .chain(l.bulks.iter().flatten())
            .copied()
            .collect();
        t.nodes.edited = l
            .lines
            .iter()
            .filter(|id| !t.nodes.added.contains(id))
            .copied()
            .collect();
        let gone: BTreeSet<Id> = l.gone.iter().copied().collect();
        t.edges.added = l
            .edges
            .iter()
            .chain(l.bulk_edges.iter().flatten())
            .chain(self.pending_edges.keys())
            .filter(|id| !gone.contains(id))
            .copied()
            .collect();
        let born: BTreeSet<Id> = l
            .edges
            .iter()
            .chain(l.bulk_edges.iter().flatten())
            .copied()
            .collect();
        t.edges.removed = gone
            .iter()
            .filter(|id| !born.contains(id) && !self.unborn.contains(id))
            .copied()
            .collect();
        t.edges.edited = l
            .edges_was
            .iter()
            .map(|e| e.id)
            .filter(|id| !born.contains(id) && !gone.contains(id))
            .collect();
        t
    }

    /// Keeps every change: adjacency is rebuilt once if an edge changed, and the undo log is dropped.
    pub fn commit(mut self) -> Applied {
        assert!(
            self.g.batch_placeholders.is_empty(),
            "unfilled batch placeholders"
        );
        self.prepare().expect("a batch's edge ends exist");
        assert!(self.ready_to_persist(), "uncompacted batch image");
        self.open = false;
        if self.edges {
            self.g.build_adjacency();
        }
        self.provenance_changed.sort_unstable();
        self.provenance_changed.dedup();
        debug_assert!(self.g.batch_placeholders.is_empty());
        Applied {
            nodes_added: self.log.nodes.len() + self.log.bulks.iter().map(Vec::len).sum::<usize>(),
            nodes_replaced: self.log.lines.len() - self.log.fills,
            edges_added: self.log.edges.len()
                + self.log.bulk_edges.iter().map(Vec::len).sum::<usize>(),
            edges_replaced: self.log.edges_was.len() + self.pending_replacements,
            edges_removed: self.log.gone.len(),
            provenance_changed: core::mem::take(&mut self.provenance_changed),
        }
    }

    /// Undoes every change, newest first, then rebuilds adjacency once if an edge changed.
    pub fn rollback(mut self) {
        self.undo_all();
    }

    fn undo_all(&mut self) {
        self.open = false;
        // After prepare (a failed write): the dropped rows go back in one pass, before anything that looks an edge up
        // by id; their name uses come back with them, so their tombstones have nothing left to give back.
        if !self.dropped.is_empty() {
            let dropped = core::mem::take(&mut self.dropped);
            self.g
                .merge_edge_records(dropped, &BTreeSet::new())
                .expect("a dropped edge's ends exist");
            self.edges = true;
        }
        // Tombstones not yet dropped give back their name use.
        for id in core::mem::take(&mut self.gone) {
            let h = self.g.edge_handle(id).expect("a tombstoned row");
            self.g.names.edge_use(self.g.e_kind.get(h), true);
        }
        if self.compacted {
            self.undo_compacted();
            self.g.batch_placeholders.clear();
            debug_assert!(self.g.batch_placeholders.is_empty());
            return;
        }
        let g = &mut *self.g;
        let log = &mut self.log;
        let edges_changed = self.edges || !log.edges.is_empty() || !log.edges_was.is_empty();
        self.pending_edges.clear();
        while let Some(entry) = log.order.pop() {
            let i = (entry & ((1 << KIND_SHIFT) - 1)) as usize;
            match entry >> KIND_SHIFT {
                FILL => untail(g, &log.filled[i]),
                BULK_EDGES => {
                    let ids = log.bulk_edges[i].iter().copied().collect();
                    g.merge_edge_records(core::iter::empty(), &ids)
                        .expect("no edges added");
                }
                BULK => {
                    let ids = &log.bulks[i];
                    let removed: BTreeSet<Id> = ids.iter().copied().collect();
                    if g.ids.ends_with(ids) {
                        for _ in ids {
                            g.drop_node(g.ids.len() - 1);
                        }
                    } else {
                        g.bulk_nodes(&[], &removed)
                            .expect("removing added lines fits");
                    }
                }
                EDGE => {
                    let h = g
                        .edge_handle(log.edges[i])
                        .expect("an edge the batch added");
                    g.remove_edge_row(h);
                }
                EDGE_GONE => {}
                EDGE_WAS => {
                    let e = log.edges_was.pop().expect("an edge the batch replaced");
                    let h = g.edge_handle(e.id).expect("an edge the batch replaced");
                    let (from, to) = g
                        .edge_handles(e.from_graph, e.from, e.to_graph, e.to)
                        .expect("edge ends");
                    g.set_edge(h, e, from, to);
                }
                LINE => match &log.line_tails[i] {
                    Some(tail) => untail(g, tail),
                    None => {
                        let h = g.handle(log.lines[i]).expect("a node the batch replaced") as usize;
                        g.restore_line(h, &log.line_bytes[i]);
                    }
                },
                _ => {
                    // Its edges are gone already: an edge always comes after the nodes it joins.
                    let h = g.handle(log.nodes[i]).expect("a node the batch added") as usize;
                    g.drop_node(h);
                }
            }
        }
        g.settle_columns();
        if edges_changed {
            g.build_adjacency();
        }
        g.undo = self.undo.take();
        g.batch_placeholders.clear();
        debug_assert!(g.batch_placeholders.is_empty());
    }

    fn undo_compacted(&mut self) {
        self.g.compact_image();
        self.pending_edges.clear();
        let nodes: BTreeSet<Id> = self
            .log
            .nodes
            .iter()
            .copied()
            .chain(self.log.bulks.iter().flatten().copied())
            .collect();
        let edges: BTreeSet<Id> = self
            .log
            .edges
            .iter()
            .copied()
            .chain(self.log.bulk_edges.iter().flatten().copied())
            .collect();
        for edge in self
            .log
            .edges_was
            .iter()
            .rev()
            .filter(|e| !edges.contains(&e.id))
        {
            let h = self.g.edge_handle(edge.id).expect("a replaced edge");
            let (from, to) = self
                .g
                .edge_handles(edge.from_graph, edge.from, edge.to_graph, edge.to)
                .expect("original edge ends");
            self.g.set_edge(h, edge.clone(), from, to);
        }
        if !edges.is_empty() {
            self.g
                .merge_edge_records(core::iter::empty(), &edges)
                .expect("no edges added");
        }
        if !nodes.is_empty() {
            self.g
                .bulk_nodes(&[], &nodes)
                .expect("removing added rows fits");
        } else if self.edges {
            self.g.build_adjacency();
        }
        for entry in self.log.order.iter().rev() {
            if entry >> KIND_SHIFT == LINE {
                let i = (entry & ((1 << KIND_SHIFT) - 1)) as usize;
                let id = self.log.lines[i];
                if !nodes.contains(&id) {
                    let h = self.g.handle(id).expect("an original node") as usize;
                    self.g.restore_line(h, &self.log.line_bytes[i]);
                }
            }
        }
        self.g.undo = self.undo.take();
    }
}

/// Cuts the newest appended line off and points its row back at its old line (the batch never compacted).
fn untail(g: &mut Graph, tail: &Tail) {
    let h = g.handle(tail.id).expect("a row the batch appended to") as usize;
    assert_eq!(
        g.image.len(),
        tail.after as usize,
        "undo the appended tail first"
    );
    g.stash_words(h);
    g.line_at[h] = tail.old_at;
    g.image.truncate(tail.before as usize);
    g.image_dead = tail.dead_before as usize;
    g.derive_row(h);
}

impl Drop for Batch<'_> {
    fn drop(&mut self) {
        if self.open {
            self.undo_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Graph;
    use crate::columns::FieldId;
    use crate::tests::node;
    use alloc::collections::BTreeMap;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;
    use umx_core::{Edge, Id, Node};

    fn text(n: u128) -> String {
        Id(n).to_text()
    }

    fn edge(id: u128, from: u128, kind: &str, to: u128) -> Edge {
        Edge {
            id: Id(id),
            kind: kind.into(),
            from_graph: None,
            from: Id(from),
            to_graph: None,
            to: Id(to),
            created: "2026-11-10T00:00:00Z".into(),
            created_by: "t".into(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: BTreeMap::new(),
        }
    }

    /// A hand-edited graph: odd key order and spacing, an unknown key, odd times, a repeated id (its old line is
    /// kept) and a dangling edge line.
    fn hand_edited() -> Graph {
        let (a, b, c) = (text(10), text(20), text(30));
        let nodes = format!(
            "{}\n{}\n{}\n{}\n",
            format_args!(
                r#"{{ "title" : "Bravo risk",  "id":"{b}", "type":"risk","status":"open","note":"n","evidence":[],"status_history":[],"created":"2026-11-10T01:00:00.250Z","updated":"x","created_by":"hand", "colour":"red", "count": 3 }}"#
            ),
            format_args!(
                r#"{{"id":"{a}","type":"problem","title":"Alpha old","status":"open","note":"","evidence":[],"status_history":[],"created":"2026-11-10T01:00:00Z","updated":"2026-11-10T01:00:00Z","created_by":"hand"}}"#
            ),
            format_args!(
                r#"{{"id":"{c}","type":"problem","title":"Charlie approval delay","status":"open","note":"line","evidence":[],"status_history":[],"created":"","updated":"later","created_by":"hand"}}"#
            ),
            format_args!(
                r#"{{"id":"{a}","type":"problem","title":"Alpha","status":"done","note":"","evidence":[],"status_history":[],"created":"2026-11-10T01:00:00Z","updated":"2026-11-10T02:00:00Z","created_by":"hand"}}"#
            ),
        );
        let edges = format!(
            "{}\n{}\n",
            format_args!(
                r#"{{"to":"{}","id":"{}","type":"blocks","from":"{a}","created":"x","created_by":"hand"}}"#,
                text(9),
                text(12)
            ),
            format_args!(
                r#"{{ "id":"{}","type":"part_of","from":"{b}","to":"{a}","created":"2026-11-10T03:00:00+00:00","created_by":"hand","via":"merge" }}"#,
                text(11)
            ),
        );
        let (mut g, _) = Graph::load_nodes(nodes.into_bytes()).unwrap();
        g.load_edges(edges.as_bytes()).unwrap();
        g.check_rows();
        g
    }

    /// Each node with its edges out and in, by id.
    type Adjacency = Vec<(Id, Vec<Id>, Vec<Id>)>;
    /// Both files, the word index, adjacency and a find's hits.
    type State = (Vec<u8>, Vec<u8>, String, Adjacency, Vec<(Id, u64)>);

    /// Everything a reader can see, adjacency included.
    fn state(g: &Graph) -> State {
        let adjacency = g
            .nodes()
            .map(|n| {
                let ix = g.node_ix(n.id()).unwrap();
                let out = g.edges_out(ix).map(|s| g.edge_at(s.edge).id()).collect();
                let inc = g.edges_in(ix).map(|s| g.edge_at(s.edge).id()).collect();
                (n.id(), out, inc)
            })
            .collect();
        let found = g
            .find("alpha approval risk", None, 10)
            .into_iter()
            .map(|(n, s)| (n.id(), (s * 1e9) as u64))
            .collect();
        (
            g.encode_nodes(),
            g.encode_edges(),
            g.word_index_image(),
            adjacency,
            found,
        )
    }

    fn edited(g: &Graph, id: u128, title: &str) -> Node {
        let mut n = g.node(Id(id)).unwrap().to_node();
        n.title = title.into();
        n.fields.insert("colour".into(), "blue".into());
        n
    }

    #[test]
    fn placeholder_kinds_do_not_hide_a_real_empty_type_and_leave_no_batch_state() {
        for prepared in [false, true] {
            for commit in [false, true] {
                let mut g = hand_edited();
                g.insert_node(node(40, "", "Hand-edited empty type"))
                    .unwrap();
                assert!(g.node_kinds().iter().any(|k| k.as_str().is_empty()));
                {
                    let mut b = g.batch();
                    b.insert_placeholders(alloc::vec![Id(5)], "2026-11-11T00:00:00Z")
                        .unwrap();
                    assert!(b.graph().node_kinds().iter().any(|k| k.as_str().is_empty()));
                    if commit {
                        b.fill(Id(5), node(5, "problem", "Filled")).unwrap();
                    }
                    if prepared {
                        b.prepare().unwrap();
                    }
                    if commit {
                        b.commit();
                    } else {
                        b.rollback();
                    }
                }
                assert!(
                    g.batch_placeholders.is_empty(),
                    "batch state escaped rollback or commit"
                );
                assert!(g.node_kinds().iter().any(|k| k.as_str().is_empty()));
                let copy = g.clone();
                assert!(copy.batch_placeholders.is_empty());
                g.encode_nodes();
            }
        }
    }

    #[test]
    #[should_panic(expected = "bulk removal would erase a kept node line")]
    fn bulk_removal_cannot_erase_kept_records() {
        let mut g = hand_edited();
        g.bulk_nodes(&[], &[Id(10)].into_iter().collect()).unwrap();
    }

    #[test]
    fn bulk_placeholders_rebuild_once_hide_data_and_undo_every_handle_index() {
        use umx_core::vectors::TextHash;
        for prepared in [false, true] {
            let mut g = hand_edited();
            let mut vectors = g.load_vectors("test@2", 2);
            for id in [10, 20, 30] {
                vectors.put(Id(id), None, &[id as i8, 1], TextHash([3; 16]));
                vectors.put(Id(id), Some("title"), &[1, id as i8], TextHash([4; 16]));
            }
            vectors.finish();
            let mut similar = g.load_similar("test@2", 2, 0);
            assert!(similar.put(Id(10), Id(30), 9000));
            assert!(similar.put(Id(20), Id(10), 8000));
            similar.finish();
            let before = state(&g);
            let builds = g.node_rebuilds;
            let mut b = g.batch();
            let ids = alloc::vec![Id(25), Id(5), Id(15)];
            b.insert_placeholders(ids.clone(), "2026-11-11T00:00:00Z")
                .unwrap();
            assert_eq!(b.g.node_rebuilds - builds, 1);
            assert_eq!(b.unfilled(), 3);
            assert!(
                b.graph()
                    .node_kinds()
                    .iter()
                    .all(|k| !k.as_str().is_empty())
            );
            assert_eq!(b.graph().first_use("colour"), Some("red".into()));
            assert!(
                b.graph()
                    .near_duplicates("", None, 0.85, 0.85, 3)
                    .is_empty()
            );
            assert!(
                b.graph()
                    .find("approval", None, 10)
                    .iter()
                    .all(|(n, _)| !ids.contains(&n.id()))
            );
            for id in &ids {
                b.fill(*id, node(id.0, "problem", "Imported")).unwrap();
            }
            assert_eq!(b.unfilled(), 0);
            assert!(b.peak_dead_bytes() > 0);
            assert!(!b.ready_to_persist());
            if prepared {
                let mut expected = hand_edited();
                for id in &ids {
                    expected
                        .insert_node(node(id.0, "problem", "Imported"))
                        .unwrap();
                }
                b.prepare().unwrap();
                assert!(b.ready_to_persist());
                b.graph().check_rows();
                assert_eq!(b.graph().encode_nodes(), expected.encode_nodes());
            }
            for id in [10, 20, 30] {
                let ix = b.graph().node_ix(Id(id)).unwrap();
                assert_eq!(
                    b.graph().vector("test@2", ix, None),
                    Some([id as i8, 1].as_slice())
                );
                assert_eq!(
                    b.graph().vector("test@2", ix, Some("title")),
                    Some([1, id as i8].as_slice())
                );
            }
            let ix = b.graph().node_ix(Id(10)).unwrap();
            let other = b.graph().similar_out(ix)[0].other;
            assert_eq!(b.graph().node_at(other).id(), Id(30));
            b.insert_edge(edge(100, 5, "supports", 10));
            let changed = edited(b.graph(), 20, "Changed while the image is dirty");
            b.insert_node(changed).unwrap();
            b.insert_node(node(150, "option", "Appended after fills"))
                .unwrap();
            b.insert_edge(edge(151, 150, "supports", 5));
            b.rollback();
            assert_eq!(
                g.node_rebuilds - builds,
                2,
                "one merge and one undo, never per id"
            );
            g.check_rows();
            assert_eq!(state(&g), before);
            for id in [10, 20, 30] {
                assert_eq!(
                    g.vector("test@2", g.node_ix(Id(id)).unwrap(), None),
                    Some([id as i8, 1].as_slice())
                );
            }
            assert_eq!(
                g.node_at(g.similar_out(g.node_ix(Id(10)).unwrap())[0].other)
                    .id(),
                Id(30)
            );
        }
    }

    #[test]
    #[should_panic(expected = "compact the batch before encoding")]
    fn a_dirty_fill_image_cannot_be_encoded() {
        let mut g = hand_edited();
        let mut b = g.batch();
        b.insert_placeholders(alloc::vec![Id(5)], "2026-11-11T00:00:00Z")
            .unwrap();
        b.fill(Id(5), node(5, "problem", "Filled")).unwrap();
        b.graph().encode_nodes();
    }

    #[test]
    fn dead_bytes_never_refuse_a_live_image_that_fits() {
        for commit in [false, true] {
            let mut g = hand_edited();
            let before = state(&g);
            let a = node(5, "problem", "A filled node");
            let b_node = node(15, "problem", "Another filled node");
            let mut expected = g.clone();
            expected.insert_node(a.clone()).unwrap();
            expected.insert_node(b_node.clone()).unwrap();
            let mut b = g.batch();
            b.insert_placeholders(alloc::vec![Id(5), Id(15)], "2026-11-11T00:00:00Z")
                .unwrap();
            b.g.limit = Some(b.g.image.len() + umx_core::codec::encode_nodes([&a]).len() + 1);
            b.fill(Id(5), a).unwrap();
            assert!(b.g.image_dead > 0);
            b.fill(Id(15), b_node).unwrap();
            assert!(b.compacted, "physical space ran out, not live space");
            if commit {
                b.commit();
                g.check_rows();
                assert_eq!(state(&g), state(&expected));
            } else {
                b.rollback();
                g.check_rows();
                assert_eq!(state(&g), before);
            }
        }
    }

    #[test]
    #[should_panic(expected = "unfilled batch placeholders")]
    fn an_unfilled_placeholder_cannot_commit() {
        let mut g = hand_edited();
        let mut b = g.batch();
        b.insert_placeholders(alloc::vec![Id(5)], "2026-11-11T00:00:00Z")
            .unwrap();
        b.commit();
    }

    #[test]
    fn out_of_order_edges_merge_once_and_bulk_undo_after_prepare_is_exact() {
        let mut original = hand_edited();
        let before = state(&original);
        let edges: Vec<Edge> = [8, 2, 6, 4]
            .into_iter()
            .map(|id| edge(id, 10, "supports", 30))
            .collect();
        let mut expected = original.clone();
        for e in &edges {
            expected.insert_edge(e.clone());
        }
        let builds = original.edge_rebuilds;
        let adjacency = original.adjacency_builds;
        let mut b = original.batch();
        for e in edges.clone() {
            b.insert_edge(e);
        }
        assert_eq!(b.g.edge_rebuilds, builds);
        b.prepare().unwrap();
        assert_eq!(b.g.edge_rebuilds - builds, 1);
        assert_eq!(
            b.g.adjacency_builds, adjacency,
            "prepare changes columns, not adjacency"
        );
        assert_eq!(b.graph().encode_edges(), expected.encode_edges());
        b.rollback();
        assert_eq!(
            original.edge_rebuilds - builds,
            2,
            "one merge and one bulk removal"
        );
        assert_eq!(state(&original), before);
        let mut b = original.batch();
        for e in edges {
            b.insert_edge(e);
        }
        b.commit();
        assert_eq!(state(&original), state(&expected));
    }

    #[test]
    fn a_thousand_random_ids_take_one_node_rebuild_and_bulk_refusal_changes_nothing() {
        let mut g = Graph::from_records(
            (1..=10_000)
                .map(|i| node(i * 2, "problem", "Base"))
                .collect(),
            Vec::new(),
        );
        let builds = g.node_rebuilds;
        let mut b = g.batch();
        let ids: Vec<Id> = (0..1000).map(|i| Id(((i * 37) % 1000) * 2 + 1)).collect();
        b.insert_placeholders(ids.clone(), "2026-11-11T00:00:00Z")
            .unwrap();
        for id in ids {
            b.fill(id, node(id.0, "problem", "Imported")).unwrap();
        }
        b.commit();
        assert_eq!(g.node_rebuilds - builds, 1);
        assert_eq!(g.node_count(), 11_000);
        g.check_rows();
        let before = state(&g);
        g.limit = Some(g.image.len() + 1);
        let mut b = g.batch();
        assert!(
            b.insert_placeholders(alloc::vec![Id(30_000), Id(30_001)], "2026-11-11T00:00:00Z")
                .is_err()
        );
        b.rollback();
        assert_eq!(state(&g), before);
    }

    /// The ops of a mixed batch, in order: appends, edits of hand-written and new nodes, an out-of-order node and
    /// edge (the rare paths), and edges to old and new nodes.
    fn ops(g: &Graph) -> Vec<Result<Node, Edge>> {
        alloc::vec![
            Ok(node(100, "risk", "New risk approval")),
            Ok(node(101, "option", "Option one")),
            Err(edge(200, 100, "part_of", 10)),
            Ok(edited(g, 20, "Bravo risk renamed")),
            Ok(edited(g, 20, "Bravo risk renamed twice")),
            Err(edge(201, 101, "supports", 100)),
            Ok(node(102, "question", "Which approval?")),
            Ok({
                let mut n = node(101, "option", "Option one edited");
                n.note = "a note".into();
                n
            }),
            Ok(node(15, "problem", "Out of order")),
            Err(edge(13, 15, "blocks", 30)),
            Err(edge(202, 102, "answers", 15)),
        ]
    }

    #[test]
    fn a_rolled_back_batch_leaves_every_byte_row_and_index_as_it_was() {
        let mut g = hand_edited();
        let before = state(&g);
        let memory = g.memory_bytes();
        let ops = ops(&g);
        let mut b = g.batch();
        for op in ops {
            match op {
                Ok(n) => {
                    b.insert_node(n).unwrap();
                }
                Err(e) => {
                    b.insert_edge(e);
                }
            }
        }
        assert!(b.undo_bytes() > 0);
        b.rollback();
        g.check_rows();
        assert_eq!(state(&g), before);
        // Columns keep the capacity they grew to (as after any write), so only the hot parts' lengths match.
        assert_eq!(g.node_count(), 3);
        assert_eq!(g.edge_count(), 1);
        assert!(g.memory_bytes().image >= memory.image);
    }

    #[test]
    fn a_dropped_batch_rolls_back() {
        let mut g = hand_edited();
        let before = state(&g);
        {
            let mut b = g.batch();
            b.insert_node(node(100, "risk", "Dropped")).unwrap();
            b.insert_edge(edge(200, 100, "part_of", 10));
        }
        assert_eq!(state(&g), before);
    }

    #[test]
    fn a_committed_batch_equals_the_same_writes_one_by_one_with_one_adjacency_rebuild() {
        let mut one_by_one = hand_edited();
        let mut batched = hand_edited();
        for op in ops(&one_by_one) {
            match op {
                Ok(n) => {
                    one_by_one.insert_node(n).unwrap();
                }
                Err(e) => {
                    one_by_one.insert_edge(e);
                }
            }
        }
        let ops = ops(&batched);
        let mut b = batched.batch();
        for op in ops {
            match op {
                Ok(n) => {
                    b.insert_node(n).unwrap();
                }
                Err(e) => {
                    b.insert_edge(e);
                }
            }
        }
        let applied = b.commit();
        assert_eq!(
            (
                applied.nodes_added,
                applied.nodes_replaced,
                applied.edges_added
            ),
            (4, 3, 4)
        );
        batched.check_rows();
        assert_eq!(state(&batched), state(&one_by_one));

        // Appends only (the batch's normal case): adjacency is built exactly once, at commit.
        let mut g = hand_edited();
        let builds = g.adjacency_builds;
        let mut b = g.batch();
        for i in 0..50u128 {
            b.insert_node(node(1000 + i, "risk", "n")).unwrap();
            b.insert_edge(edge(2000 + i, 1000 + i, "part_of", 10));
        }
        b.commit();
        assert_eq!(g.adjacency_builds - builds, 1);
        let ix = g.node_ix(Id(10)).unwrap();
        assert_eq!(g.edges_in(ix).count(), 51);
    }

    #[test]
    fn touched_is_exact_for_adds_edits_deletes_and_placeholders() {
        for prepared in [false, true] {
            let mut g = hand_edited();
            g.insert_edge(edge(13, 30, "blocks", 10));
            let same = g.node(Id(20)).unwrap().to_node();
            let edited_30 = edited(&g, 30, "Charlie again");
            let mut b = g.batch();
            b.insert_node(node(100, "risk", "new")).unwrap();
            b.insert_node(node(100, "risk", "new, then edited"))
                .unwrap();
            b.insert_node(edited_30).unwrap();
            b.insert_node(same).unwrap();
            b.insert_placeholders(alloc::vec![Id(5)], "2026-11-11T00:00:00Z")
                .unwrap();
            b.fill(Id(5), node(5, "problem", "Imported")).unwrap();
            b.insert_edge(edge(200, 100, "part_of", 30));
            b.insert_edge(edge(3, 30, "blocks", 20));
            b.insert_edge(edge(4, 30, "blocks", 10));
            assert!(
                b.remove_edge(Id(4)).is_some(),
                "added out of order, then removed"
            );
            b.insert_edge(edge(201, 100, "blocks", 10));
            assert!(b.remove_edge(Id(201)).is_some(), "appended, then removed");
            b.insert_edge(edge(11, 20, "blocks", 30));
            assert!(b.remove_edge(Id(13)).is_some(), "a removal (F75)");
            if prepared {
                b.prepare().unwrap();
            }
            let t = b.touched();
            let ids = |v: &[u128]| {
                v.iter()
                    .map(|i| Id(*i))
                    .collect::<alloc::collections::BTreeSet<Id>>()
            };
            assert_eq!(t.nodes.added, ids(&[5, 100]), "prepared {prepared}");
            assert_eq!(t.nodes.edited, ids(&[30]), "the same bytes are no edit");
            assert!(t.nodes.removed.is_empty());
            assert_eq!(t.edges.added, ids(&[3, 200]), "prepared {prepared}");
            assert_eq!(t.edges.edited, ids(&[11]));
            assert_eq!(t.edges.removed, ids(&[13]));
            b.commit();
        }
    }

    #[test]
    fn a_batch_that_does_not_fit_is_refused_at_the_op_and_rolls_back_whole() {
        let mut g = hand_edited();
        let before = state(&g);
        g.limit = Some(g.encode_nodes().len() + 400);
        let mut b = g.batch();
        b.insert_node(node(100, "risk", "fits")).unwrap();
        b.insert_edge(edge(200, 100, "part_of", 10));
        let full = b.insert_node(node(101, "risk", &"x".repeat(500)));
        assert!(full.is_err());
        b.rollback();
        g.limit = None;
        assert_eq!(state(&g), before);
    }

    /// xorshift, seeded: the random batch is the same on every run.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    const KINDS: [&str; 4] = ["blocks", "supports", "answers", "depends_on"];
    const NODES: u128 = 60;

    /// 60 nodes and 600 edges, no two with the same (from, relation, to).
    fn wide() -> Graph {
        let nodes = (0..NODES)
            .map(|i| node(1_000_000 + i, "risk", "n"))
            .collect();
        let mut seen = alloc::collections::BTreeSet::new();
        let mut edges = Vec::new();
        let mut rng = Rng(7);
        while edges.len() < 600 {
            let (a, b, k) = (rng.below(60) as u128, rng.below(60) as u128, rng.below(4));
            if seen.insert((a, k, b)) {
                edges.push(edge(
                    edges.len() as u128 + 1,
                    1_000_000 + a,
                    KINDS[k],
                    1_000_000 + b,
                ));
            }
        }
        Graph::from_records(nodes, edges)
    }

    // Test ops only: boxing the edge would buy nothing.
    #[allow(clippy::large_enum_variant)]
    enum EdgeOp {
        /// A new edge, or an edge as edited (same id).
        Put(Edge),
        Remove(Id),
    }

    /// An edge's (from, relation, to).
    type Key = (Id, String, Id);

    /// `n` ops valid against `r` as each is applied to it one by one (adds above every id, edits and removes of live
    /// edges, never two edges with one (from, relation, to)), with each op's (from, relation, to) before and after.
    fn random_ops(r: &mut Graph, n: usize, seed: u64) -> Vec<(EdgeOp, Vec<Key>)> {
        let mut rng = Rng(seed);
        let mut next = 10_000u128;
        let mut out = Vec::new();
        while out.len() < n {
            let live: Vec<Edge> = r.edges().map(|e| e.to_edge()).collect();
            let pick = |rng: &mut Rng| live[rng.below(live.len())].clone();
            let node = |rng: &mut Rng| Id(1_000_000 + rng.below(NODES as usize) as u128);
            let (op, keys) = match rng.below(3) {
                0 => {
                    let (a, b) = (node(&mut rng), node(&mut rng));
                    let k = KINDS[rng.below(4)];
                    next += 1;
                    let e = edge(next, a.0, k, b.0);
                    (EdgeOp::Put(e), alloc::vec![(a, k.into(), b)])
                }
                1 => {
                    let mut e = pick(&mut rng);
                    let old = (e.from, e.kind.clone(), e.to);
                    if rng.below(2) == 0 {
                        e.kind = KINDS[rng.below(4)].into();
                    } else {
                        (e.from, e.to) = (e.to, e.from);
                    }
                    e.fields
                        .insert("updated".into(), "2026-11-11T00:00:00Z".into());
                    let new = (e.from, e.kind.clone(), e.to);
                    (EdgeOp::Put(e), alloc::vec![old, new])
                }
                _ => {
                    let e = pick(&mut rng);
                    (
                        EdgeOp::Remove(e.id),
                        alloc::vec![(e.from, e.kind.clone(), e.to)],
                    )
                }
            };
            if let EdgeOp::Put(e) = &op {
                match r.find_edge(e.from, &e.kind, e.to) {
                    Some(other) if other.id() != e.id => continue,
                    _ => {}
                }
                r.insert_edge(e.clone());
            } else if let EdgeOp::Remove(id) = &op {
                r.remove_edge(*id);
            }
            out.push((op, keys));
        }
        out
    }

    /// Applies one op to a batch, keeping the caller's overlay of moved edges as umx-services does.
    fn apply(b: &mut super::Batch<'_>, moved: &mut BTreeMap<(Id, String, Id), Id>, op: &EdgeOp) {
        let mut forget = |b: &super::Batch<'_>, id: Id| {
            if let Some(e) = b.edge(id) {
                let key = (e.from(), String::from(e.kind()), e.to());
                if moved.get(&key) == Some(&id) {
                    moved.remove(&key);
                }
            }
        };
        match op {
            EdgeOp::Put(e) => {
                forget(b, e.id);
                moved.insert((e.from, e.kind.clone(), e.to), e.id);
                b.insert_edge(e.clone());
            }
            EdgeOp::Remove(id) => {
                forget(b, *id);
                b.remove_edge(*id).expect("a live edge");
            }
        }
    }

    fn relations(g: &Graph) -> Vec<String> {
        g.relation_kinds()
            .iter()
            .map(|k| String::from(k.as_str()))
            .collect()
    }

    #[test]
    fn deletes_and_edits_in_a_batch_equal_the_same_writes_one_by_one() {
        // F75 (graph-storage-expert): 10k random adds, edits and removes; at every line the batch answers find_edge as
        // the graph written one by one does, and the committed file is byte for byte the same.
        let mut r = wide();
        let ops = random_ops(&mut r, 10_000, 99);
        let mut g = wide();
        let (builds, rebuilds) = (g.adjacency_builds, g.edge_rebuilds);
        let mut b = g.batch();
        let mut moved = BTreeMap::new();
        let mut check = wide();
        let mut probe = Rng(5);
        for (op, keys) in &ops {
            apply(&mut b, &mut moved, op);
            match op {
                EdgeOp::Put(e) => {
                    check.insert_edge(e.clone());
                }
                EdgeOp::Remove(id) => {
                    check.remove_edge(*id);
                }
            }
            let mut asks = keys.clone();
            for _ in 0..3 {
                let a = Id(1_000_000 + probe.below(NODES as usize) as u128);
                let z = Id(1_000_000 + probe.below(NODES as usize) as u128);
                asks.push((a, KINDS[probe.below(4)].into(), z));
            }
            for (a, k, z) in asks {
                let want = check.find_edge(a, &k, z).map(|e| e.id());
                let got = moved
                    .get(&(a, k.clone(), z))
                    .copied()
                    .or_else(|| b.find_edge(a, &k, z).map(|e| e.id()));
                assert_eq!(got, want, "{a:?} {k} {z:?}");
            }
        }
        let applied = b.commit();
        assert!(applied.edges_removed > 0 && applied.edges_replaced > 0);
        g.check_rows();
        assert_eq!(g.encode_edges(), r.encode_edges());
        assert_eq!(state(&g), state(&r));
        assert_eq!(relations(&g), relations(&r));
        assert_eq!(
            (g.adjacency_builds - builds, g.edge_rebuilds - rebuilds),
            (1, 1)
        );
    }

    #[test]
    fn a_batch_with_deletes_rolls_back_exactly_before_and_after_prepare() {
        for prepared in [false, true] {
            let mut g = wide();
            let before = (state(&g), relations(&g));
            let mut r = wide();
            let ops = random_ops(&mut r, 500, 3);
            let mut b = g.batch();
            let mut moved = BTreeMap::new();
            for (op, _) in &ops {
                apply(&mut b, &mut moved, op);
            }
            if prepared {
                // A failed write: prepare dropped the removed rows, then the batch rolls back.
                b.prepare().unwrap();
                assert_eq!(b.graph().encode_edges(), r.encode_edges());
            }
            b.rollback();
            g.check_rows();
            assert_eq!((state(&g), relations(&g)), before, "prepared {prepared}");
        }
    }

    /// 10 nodes and edges at the even ids 2 to 40, so an odd id is an out-of-order edge (explicit ids).
    fn gapped() -> Graph {
        let nodes = (0..10).map(|i| node(1_000_000 + i, "risk", "n")).collect();
        let edges = (1..=20u128)
            .map(|i| {
                edge(
                    2 * i,
                    1_000_000 + i % 10,
                    KINDS[(i / 10) as usize],
                    1_000_000 + (i + 3) % 10,
                )
            })
            .collect();
        Graph::from_records(nodes, edges)
    }

    #[test]
    fn deletes_and_edits_mix_with_out_of_order_edges_in_one_merge() {
        // F75 with an earlier fix: tombstones and out-of-order (explicit id) edges leave in the same one merge, and a
        // batch can edit or remove an edge it added out of order before prepare places it.
        let mut retyped = edge(7, 1_000_001, "answers", 1_000_002);
        retyped
            .fields
            .insert("updated".into(), "2026-11-11T00:00:00Z".into());
        let mut moved = gapped().edge(Id(10)).unwrap().to_edge();
        (moved.from, moved.to) = (moved.to, moved.from);
        let ops = [
            EdgeOp::Put(edge(7, 1_000_001, "blocks", 1_000_002)),
            EdgeOp::Put(edge(9, 1_000_003, "blocks", 1_000_004)),
            EdgeOp::Remove(Id(4)),
            EdgeOp::Remove(Id(9)),
            EdgeOp::Put(retyped.clone()),
            EdgeOp::Put(moved),
            EdgeOp::Remove(Id(10)),
            EdgeOp::Put(edge(41, 1_000_005, "blocks", 1_000_006)),
            EdgeOp::Remove(Id(41)),
        ];
        let mut want = gapped();
        for op in &ops {
            match op {
                EdgeOp::Put(e) => {
                    want.insert_edge(e.clone());
                }
                EdgeOp::Remove(id) => {
                    want.remove_edge(*id);
                }
            }
        }
        for end in ["commit", "rollback", "prepare then rollback"] {
            let mut g = gapped();
            let before = (state(&g), relations(&g));
            let (builds, rebuilds) = (g.adjacency_builds, g.edge_rebuilds);
            let mut b = g.batch();
            for op in &ops {
                match op {
                    EdgeOp::Put(e) => {
                        b.insert_edge(e.clone());
                    }
                    EdgeOp::Remove(id) => {
                        assert!(b.remove_edge(*id).is_some(), "{id:?}");
                    }
                }
                if let EdgeOp::Put(e) = op {
                    assert_eq!(b.edge_record(e.id).as_ref(), Some(e));
                }
            }
            assert_eq!(b.edge_record(Id(9)), None);
            assert!(
                b.edge(Id(4)).is_none()
                    && b.find_edge(Id(1_000_002), "blocks", Id(1_000_005))
                        .is_none()
            );
            match end {
                "commit" => {
                    let applied = b.commit();
                    assert_eq!(applied.edges_removed, 4);
                    g.check_rows();
                    assert_eq!(g.encode_edges(), want.encode_edges());
                    assert_eq!((state(&g), relations(&g)), (state(&want), relations(&want)));
                    assert_eq!(
                        (g.adjacency_builds - builds, g.edge_rebuilds - rebuilds),
                        (1, 1),
                        "one merge places edge 7 and drops 4 and 10; one adjacency build"
                    );
                }
                "rollback" => {
                    b.rollback();
                    g.check_rows();
                    assert_eq!((state(&g), relations(&g)), before);
                }
                _ => {
                    b.prepare().unwrap();
                    assert_eq!(b.graph().encode_edges(), want.encode_edges());
                    b.rollback();
                    g.check_rows();
                    assert_eq!((state(&g), relations(&g)), before);
                }
            }
        }
    }

    #[test]
    fn a_failed_write_after_prepare_restores_filled_placeholders_and_removed_edges_exactly() {
        // graph-storage-expert (R196): one batch fills placeholders (dead image bytes, so prepare compacts and a
        // rollback takes undo_compacted) and removes and edits edges (dropped rows present); a failed write after
        // prepare must leave every byte and index as it was.
        let mut g = gapped();
        let before = (state(&g), relations(&g));
        let mut b = g.batch();
        let ids = alloc::vec![Id(999_999), Id(1_000_100)];
        b.insert_placeholders(ids.clone(), "2026-11-11T00:00:00Z")
            .unwrap();
        for id in &ids {
            b.fill(*id, node(id.0, "risk", "Imported risk")).unwrap();
        }
        assert!(b.peak_dead_bytes() > 0, "the fills left dead image bytes");
        b.insert_edge(edge(3, 999_999, "blocks", 1_000_100));
        b.insert_edge(edge(43, 1_000_100, "blocks", 1_000_001));
        let mut moved = b.edge_record(Id(10)).unwrap();
        (moved.from, moved.to) = (moved.to, moved.from);
        b.insert_edge(moved);
        for id in [4, 10, 43] {
            assert!(b.remove_edge(Id(id)).is_some(), "{id}");
        }
        b.prepare().unwrap();
        assert!(b.ready_to_persist(), "prepare compacted the image");
        assert!(b.graph().edge(Id(3)).is_some());
        assert!(
            [4, 10, 43]
                .iter()
                .all(|id| b.graph().edge(Id(*id)).is_none())
        );
        b.rollback();
        g.check_rows();
        assert_eq!((state(&g), relations(&g)), before);
        assert!(ids.iter().all(|id| !g.holds_id(*id)));
    }

    /// One write of the random differential below.
    enum Op {
        Node(Node),
        /// Placeholders for these ids, each then filled with its node.
        Placeholders(Vec<Node>),
        Edge(Edge),
        Remove(Id),
    }

    const TITLES: [&str; 5] = [
        "Approval delay",
        "Alpha risk",
        "A much longer title about the approval of the risk register",
        "x",
        "Charlie option",
    ];

    /// One of [`TITLES`], a third of the time with a word of its own (ws003 s27: new title words for the groups).
    fn random_title(rng: &mut Rng) -> String {
        let base = TITLES[rng.below(5)];
        if rng.below(3) == 0 {
            format!("{base} f{:05x}", rng.below(1 << 20))
        } else {
            base.into()
        }
    }

    const EVIDENCE: [&str; 3] = ["chat", "room", "document"];

    /// D71 step 10: values of a `due` field for the typed time column. Every precision, before 1970 and the year
    /// edges 0001 and 9999 (0000 is no date since F211), an offset, padding, an escaped character (the decoded text
    /// is a date, the raw bytes are not), and text, a number, a bool and a list that are no date (kind changes).
    const DUES: [&str; 16] = [
        "2026-11-20",
        "2025-01",
        "2025",
        "0001",
        "0001-01-01T00:00:00Z",
        "9999-12-31",
        "9999-12-31T23:59:59Z",
        "0000",
        "1969-12-31",
        "1066-10",
        "2026-10-30T09:40:00+11:00",
        " 2026-12-01",
        "2026-12-02\n",
        "1969-07-20T20:17:40Z",
        "soon",
        "2026-13-01",
    ];

    /// One of [`DUES`], or a value of another kind.
    fn random_due(rng: &mut Rng) -> umx_core::Value {
        match rng.below(DUES.len() + 3) {
            i if i < DUES.len() => serde_json::json!(DUES[i]),
            i if i == DUES.len() => serde_json::json!(2025),
            i if i == DUES.len() + 1 => serde_json::json!(true),
            _ => serde_json::json!(["2025-01"]),
        }
    }

    /// D71 step 10: the typed time column holds, for every row and field, what `date_span` reads from the value's
    /// text: a string that reads as a date has its span and precision, anything else (another kind, text that is no
    /// date) none, and no date outlives the value it came from.
    fn dates_follow_the_text(g: &Graph, seed: u64) {
        let mut dated = 0;
        for h in 0..g.ids.len() {
            for f in g.field_names(h) {
                let name = g.names.text(f);
                let want = match g.custom_value(h, name) {
                    Some(umx_core::Value::String(s)) => umx_core::time::date_span(&s),
                    _ => None,
                };
                dated += usize::from(want.is_some());
                let got = crate::NodeRef { g, h: h as u32 }.date_at(FieldId(f));
                assert_eq!(
                    got,
                    want.map(|d| (d, d.precision())),
                    "seed {seed}: row {h} field {name}"
                );
            }
        }
        let mut held = 0;
        g.f_cols
            .values()
            .for_each(|c| c.each_date(|_, _, _| held += 1));
        assert_eq!(held, dated, "seed {seed}: a date outlived its value");
    }

    /// Every node's title, field names and values (with each value's date from the typed time column), and evidence
    /// kinds, by text.
    type Rows = Vec<(
        Id,
        String,
        Vec<(
            String,
            Option<umx_core::Value>,
            Option<(umx_core::time::DateSpan, umx_core::time::Precision)>,
        )>,
        Vec<String>,
    )>;
    /// The rows, then title finds and near duplicates (id, score), then node lists.
    type Reads = (Rows, Vec<Vec<(Id, u64)>>, Vec<Vec<Id>>);

    /// What a reader sees of the node columns (s26): each row's title, field names and values and evidence kinds, and
    /// the reads over them (title find, near duplicates, the node list by type, field and evidence kind, the evidence
    /// kind index), by text so two graphs compare whatever their name numbers.
    fn reads(g: &Graph) -> Reads {
        let rows = (0..g.ids.len())
            .map(|h| {
                let fields = g
                    .field_names(h)
                    .map(|f| {
                        let name = g.names.text(f);
                        let date = crate::NodeRef { g, h: h as u32 }.date_at(FieldId(f));
                        (name.into(), g.custom_value(h, name), date)
                    })
                    .collect();
                let kinds = crate::NodeRef { g, h: h as u32 }
                    .evidence_kinds()
                    .map(Into::into)
                    .collect();
                (g.ids[h], g.title(h).into(), fields, kinds)
            })
            .collect();
        let scored = |hits: Vec<(crate::NodeRef<'_>, f64)>| {
            hits.into_iter()
                .map(|(n, s)| (n.id(), (s * 1e9) as u64))
                .collect()
        };
        // ws003 s27: the near-duplicate threshold's candidates by group, equal to today's walk of every title word
        // here, and to a fresh load's where the caller compares.
        let grouped = |t: &str| -> Vec<(Id, u64)> {
            let q = crate::find::Query::new(t).expect("words");
            let got = g.words.title_candidates(&q.0, 0.85);
            assert_eq!(got, g.words.title_candidates_ungrouped(&q.0, 0.85), "{t:?}");
            got.into_iter().map(|h| (g.ids[h as usize], 0)).collect()
        };
        let finds = TITLES
            .iter()
            .flat_map(|t| {
                [
                    scored(g.find(t, None, 10)),
                    scored(g.near_duplicates(t, Some("problem"), 0.5, 0.6, 5)),
                    scored(g.near_duplicates(t, Some("problem"), 0.85, 0.95, 5)),
                    grouped(t),
                ]
            })
            .collect();
        let filter = |field: &str, value: &str| crate::NodeFilter {
            wheres: alloc::vec![crate::Where {
                field: field.into(),
                op: crate::Op::Eq,
                value: value.into(),
            }],
            ..Default::default()
        };
        let risks = crate::NodeFilter {
            kind: Some("risk".into()),
            ..Default::default()
        };
        let lists = [
            filter("evidence_kinds", "chat"),
            filter("colour", "red"),
            risks,
        ]
        .iter()
        .map(|f| g.list_nodes(f).iter().map(|n| n.id()).collect())
        .chain(
            EVIDENCE
                .iter()
                .map(|k| g.nodes_with_evidence_kind(k).map(|n| n.id()).collect()),
        )
        .collect();
        (rows, finds, lists)
    }

    /// A graph loaded from `g`'s rows' lines as they are now (mid-batch: the appended ones, not the dead ones).
    fn fresh(g: &Graph) -> Graph {
        let mut bytes = Vec::new();
        for h in 0..g.ids.len() {
            bytes.extend_from_slice(g.line(h));
            bytes.push(b'\n');
        }
        Graph::load_nodes(bytes).unwrap().0
    }

    /// What the generator remembers: each node's earlier versions, and the ids it removed (never drawn again).
    #[derive(Default)]
    struct Seen {
        versions: BTreeMap<Id, Vec<Node>>,
        removed: alloc::collections::BTreeSet<Id>,
    }

    /// An id no record holds or held: above every id, or (when `middle`) below the last one when there is a gap.
    fn free_id(want: &Graph, seen: &Seen, rng: &mut Rng, middle: bool) -> Id {
        let last = want
            .last_id()
            .max(seen.removed.last().copied())
            .map_or(0, |id| id.0);
        let free = |id: &Id| !want.holds_id(*id) && !seen.removed.contains(id);
        if middle {
            for _ in 0..20 {
                let id = Id(1 + rng.below(last as usize) as u128);
                if free(&id) {
                    return id;
                }
            }
        }
        Id(last + 1 + rng.below(3) as u128)
    }

    /// A random write, applied to `want` one write at a time (`Graph::insert_node` splices: the old batch edit). With
    /// `kinds` 4 every write is a node edit.
    fn random_op(want: &mut Graph, rng: &mut Rng, seen: &mut Seen, kinds: usize) -> Op {
        let ids: Vec<Id> = want.nodes().map(|n| n.id()).collect();
        let edges: Vec<Id> = want.edges().map(|e| e.id()).collect();
        let op = match rng.below(kinds) {
            4 | 5 => {
                let middle = rng.below(2) == 0;
                let id = free_id(want, seen, rng, middle);
                let mut n = node(id.0, "option", &random_title(rng));
                // Half the adds hold a `due`, so a rolled-back add at the end pops a dated row (D71 step 10).
                if rng.below(2) == 0 {
                    n.fields.insert("due".into(), random_due(rng));
                }
                Op::Node(n)
            }
            6 => {
                let mut nodes: Vec<Node> = Vec::new();
                for _ in 0..1 + rng.below(3) {
                    let middle = rng.below(2) == 0;
                    let id = free_id(want, seen, rng, middle);
                    if !nodes.iter().any(|n| n.id == id) {
                        nodes.push(node(id.0, "problem", &random_title(rng)));
                    }
                }
                Op::Placeholders(nodes)
            }
            7 => {
                let (from, to) = (ids[rng.below(ids.len())], ids[rng.below(ids.len())]);
                let kind = KINDS[rng.below(4)];
                if want.find_edge(from, kind, to).is_some() {
                    return random_op(want, rng, seen, kinds);
                }
                let middle = rng.below(3) == 0;
                let id = free_id(want, seen, rng, middle);
                Op::Edge(edge(id.0, from.0, kind, to.0))
            }
            8 | 9 if !edges.is_empty() => {
                let id = edges[rng.below(edges.len())];
                seen.removed.insert(id);
                Op::Remove(id)
            }
            _ => {
                let id = ids[rng.below(ids.len())];
                let mut n = want.node(id).unwrap().to_node();
                let seen = seen.versions.entry(id).or_default();
                // Each edit changes one thing (s26's word and field diffs): the title, the note's counts, a field's
                // presence, an earlier version, the evidence, a field's value or kind, one word of the note, or the
                // `due` field's value (the typed time column, D71 step 10: a date of any precision, text, another kind,
                // or gone).
                match rng.below(9) {
                    0 => n.title = random_title(rng),
                    1 => n.note = "note ".repeat(rng.below(40)),
                    2 => {
                        if n.fields.remove("colour").is_none() {
                            n.fields.insert("colour".into(), "red".into());
                        }
                    }
                    // An earlier version: the graph's undo slot puts back its exact bytes.
                    3 if !seen.is_empty() => n = seen[rng.below(seen.len())].clone(),
                    // The evidence kinds index (s22) follows the edited row.
                    4 => {
                        n.evidence = EVIDENCE[..rng.below(EVIDENCE.len() + 1)]
                            .iter()
                            .map(|kind| serde_json::json!({"source": "a.md", "kind": kind}))
                            .collect();
                    }
                    5 => {
                        let v = match rng.below(6) {
                            0 => serde_json::json!("red"),
                            1 => serde_json::json!("blue"),
                            2 => serde_json::json!(3),
                            3 => serde_json::json!(2.5),
                            4 => serde_json::json!(true),
                            _ => serde_json::json!(["red"]),
                        };
                        n.fields.insert("colour".into(), v);
                    }
                    6 => {
                        let word = ["alpha", "approval", "register", "zulu"][rng.below(4)];
                        n.note = match n.note.rsplit_once(' ') {
                            Some((rest, _)) if rng.below(2) == 0 => format!("{rest} {word}"),
                            _ => format!("{} {word}", n.note),
                        };
                    }
                    7 => {
                        if rng.below(4) == 0 {
                            n.fields.remove("due");
                        } else {
                            n.fields.insert("due".into(), random_due(rng));
                        }
                    }
                    _ => {}
                }
                seen.push(want.node(id).unwrap().to_node());
                Op::Node(n)
            }
        };
        match &op {
            Op::Node(n) => {
                want.insert_node(n.clone()).unwrap();
            }
            Op::Placeholders(nodes) => {
                for n in nodes {
                    want.insert_node(n.clone()).unwrap();
                }
            }
            Op::Edge(e) => {
                want.insert_edge(e.clone());
            }
            Op::Remove(id) => {
                want.remove_edge(*id).unwrap();
            }
        }
        op
    }

    fn apply_op(b: &mut super::Batch<'_>, op: &Op) -> Result<(), crate::ImageFull> {
        match op {
            Op::Node(n) => b.insert_node(n.clone()).map(drop),
            Op::Placeholders(nodes) => {
                b.insert_placeholders(
                    nodes.iter().map(|n| n.id).collect(),
                    "2026-11-11T00:00:00Z",
                )?;
                nodes.iter().try_for_each(|n| b.fill(n.id, n.clone()))
            }
            Op::Edge(e) => {
                b.insert_edge(e.clone());
                Ok(())
            }
            Op::Remove(id) => {
                b.remove_edge(*id).expect("a live edge");
                Ok(())
            }
        }
    }

    /// The memory parts a batch of edits and the same edits one by one must agree on. The capacities of the image, its
    /// line starts, the titles and the field names are left out (the titles' and names' lengths stay in): a compaction
    /// or a column settle allocates them exactly, while splices grow them. (Adds in the middle and placeholders rebuild
    /// the columns on one side only, so only edit-only batches compare these.) The word index's title-word groups are
    /// left out too (ws003 s27, graph-storage C2): when they group afresh follows each side's own history.
    fn parts(g: &Graph) -> crate::MemoryBytes {
        let m = g.memory_bytes();
        crate::MemoryBytes {
            image: 0,
            node_columns: m.node_columns
                - g.line_at.capacity() * size_of::<u32>()
                - g.titles.capacity()
                - g.nf.heap_bytes()
                + g.titles.len()
                + g.nf.len(),
            word_index: m.word_index - g.words.group_bytes(),
            ..m
        }
    }

    /// One case of the differential below: `len` and `end` are drawn from the seed when None.
    struct Case {
        seed: u64,
        kinds: usize,
        len: Option<usize>,
        fill_first: bool,
        end: Option<usize>,
    }

    /// Review D1, seeded differential: random batches of edits (spliced, then appended), adds at the end and in the
    /// middle, placeholders and fills, and edge adds and removes, some under a limit that compacts mid-batch or
    /// refuses an op; and edit batches of 1 to 20 lines around `SPLICED_EDITS`, with and without a fill first, at
    /// every end. A committed batch equals the same writes one by one (the splice path every batch edit took before):
    /// both files, the word index, adjacency, finds and the undo slot, and for edit-only batches the memory parts. A
    /// refused, rolled back (before or after prepare) or dropped batch leaves all of them as they were. After every op
    /// (s26), the node columns read as a fresh load of the batch's current lines reads them ([`reads`]), and the typed
    /// time column holds what `date_span` reads from each value's text ([`dates_follow_the_text`], D71 step 10).
    #[test]
    fn random_batches_equal_the_splice_path_and_roll_back_exactly() {
        // Built twice rather than cloned: a clone sizes every column exactly, so capacities would differ from the start.
        // ws003 s27: `extra` nodes with a title word of their own, so some batches' new title words pass the word
        // index's group tail and it groups afresh mid-batch (and a rollback empties places in the new groups).
        let build = |extra: u128| {
            let mut g = hand_edited();
            for i in 0..12u128 {
                let mut n = node(40 + i * 10, "risk", TITLES[i as usize % 5]);
                if i % 2 == 0 {
                    n.fields
                        .insert("due".into(), DUES[i as usize % DUES.len()].into());
                }
                g.insert_node(n).unwrap();
            }
            g.insert_node(edited(&g, 20, "Bravo before the batch"))
                .unwrap();
            for i in 0..extra {
                g.insert_node(node(2000 + i, "risk", &alloc::format!("Extra w{i:03}x")))
                    .unwrap();
            }
            g
        };
        let random = (1..=500u64).map(|seed| Case {
            seed,
            kinds: if seed > 400 { 4 } else { 10 },
            len: None,
            fill_first: false,
            end: None,
        });
        let threshold = (1..=20usize).flat_map(|len| {
            [false, true].into_iter().flat_map(move |fill_first| {
                (0..4).map(move |end| Case {
                    seed: 1000 + (len * 8 + usize::from(fill_first) * 4 + end) as u64,
                    kinds: 4,
                    len: Some(len),
                    fill_first,
                    end: Some(end),
                })
            })
        });
        let (mut ends, mut mixed, mut moved, mut regrouped) = ([0usize; 5], 0, 0, 0);
        let mut dates_seen = alloc::collections::BTreeSet::new();
        for case in random.chain(threshold) {
            let seed = case.seed;
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let extra = u128::from(seed % 8) * 13;
            let mut g = build(extra);
            let before = (state(&g), relations(&g), g.undo.clone(), parts(&g));
            let mut want = build(extra);
            // A batch takes the undo slot for its rollback, so its writes start without it, as the one by one ones do.
            want.undo = None;
            let mut seen = Seen::default();
            let mut ops = Vec::new();
            if case.fill_first {
                let id = free_id(&want, &seen, &mut rng, false);
                let n = node(id.0, "problem", "Filled first");
                want.insert_node(n.clone()).unwrap();
                ops.push(Op::Placeholders(alloc::vec![n]));
            }
            let len = case.len.unwrap_or_else(|| 1 + rng.below(40));
            ops.extend((0..len).map(|_| random_op(&mut want, &mut rng, &mut seen, case.kinds)));
            if case.end.is_none() && rng.below(3) == 0 {
                g.limit = Some(g.image.len() + 200 + rng.below(3000));
            }
            let end = case.end.unwrap_or_else(|| rng.below(4));
            let mut b = g.batch();
            let mut refused = false;
            for op in &ops {
                let groups = b.graph().words.group_count();
                refused = apply_op(&mut b, op).is_err();
                let now = b.graph();
                moved += usize::from(!now.moved.is_empty());
                regrouped += usize::from(now.words.group_count() != groups);
                assert_eq!(
                    reads(now),
                    reads(&fresh(now)),
                    "seed {seed}: columns mid-batch"
                );
                dates_follow_the_text(now, seed);
                now.f_cols.values().for_each(|c| {
                    c.each_date(|_, d, p| {
                        dates_seen.insert((d.start < 0, p as u8));
                    })
                });
                if refused {
                    break;
                }
            }
            let tails = &b.log.line_tails;
            if tails.iter().any(Option::is_none) && tails.iter().any(Option::is_some) {
                mixed += 1;
            }
            // The cost policy: splices only before any dead byte and only for the first edits; a splice after a fill
            // would still be correct (splice_image shifts past dead bytes) but pays for a compaction it cannot avoid.
            if case.len.is_some() && !b.compacted {
                let spliced = if case.fill_first {
                    0
                } else {
                    super::SPLICED_EDITS
                };
                assert!(
                    tails.iter().take(spliced).all(Option::is_none)
                        && tails.iter().skip(spliced).all(Option::is_some),
                    "seed {seed}: splices exactly the first {spliced} edits"
                );
            }
            let end = if refused { 4 } else { end };
            // A fill that ran out of physical room is written as an edit, which fills the undo slot (as before s18);
            // one by one it is an add.
            let filled_as_edit = b.log.fills > 0;
            ends[end] += 1;
            match end {
                0 => {
                    b.commit();
                }
                1 | 4 => b.rollback(),
                2 => {
                    b.prepare().unwrap();
                    b.rollback();
                }
                _ => drop(b),
            }
            g.limit = None;
            g.check_rows();
            dates_follow_the_text(&g, seed);
            assert_eq!(g.image_dead, 0, "seed {seed}");
            let after = (state(&g), relations(&g), g.undo.clone(), parts(&g));
            let want = if end == 0 {
                (
                    state(&want),
                    relations(&want),
                    want.undo.clone(),
                    parts(&want),
                )
            } else {
                before
            };
            let (s, w) = (&after.0, &want.0);
            assert!(s.0 == w.0, "seed {seed} end {end}: nodes.jsonl");
            assert!(s.1 == w.1, "seed {seed} end {end}: edges.jsonl");
            assert!(s.2 == w.2, "seed {seed} end {end}: word index");
            assert!(s.3 == w.3, "seed {seed} end {end}: adjacency");
            assert!(s.4 == w.4, "seed {seed} end {end}: finds");
            assert!(after.1 == want.1, "seed {seed} end {end}: relations");
            if !(end == 0 && filled_as_edit) {
                assert!(after.2 == want.2, "seed {seed} end {end}: undo slot");
            }
            if end == 0 && case.kinds == 4 && !case.fill_first {
                assert_eq!(after.3, want.3, "seed {seed}: memory parts");
            }
        }
        assert!(ends.iter().all(|n| *n > 0), "every end ran: {ends:?}");
        assert!(mixed > 0, "some batches spliced, then appended");
        assert!(moved > 0, "some batches moved titles or field names");
        assert!(regrouped > 0, "some batches grouped the title words afresh");
        // The typed time column held every precision on both sides of 1970 (a date-time before it since F211).
        use umx_core::time::Precision as P;
        for (before_1970, p) in [
            (false, P::Second),
            (false, P::Day),
            (false, P::Month),
            (false, P::Year),
            (true, P::Second),
            (true, P::Day),
            (true, P::Month),
            (true, P::Year),
        ] {
            assert!(
                dates_seen.contains(&(before_1970, p as u8)),
                "the column held a {p:?} date (before 1970: {before_1970}): {dates_seen:?}"
            );
        }
    }

    /// graph-storage (s18): the first edits splice, later ones append, a limit forces a compaction mid-batch, more
    /// edits append after it, and a rollback restores every line from its bytes (no tail from before the compaction is
    /// cut off).
    #[test]
    fn edits_across_a_mid_batch_compaction_roll_back_from_the_line_bytes() {
        let mut g = hand_edited();
        let before = (state(&g), g.undo.clone());
        let first = edited(&g, 20, "Bravo, appended before the compaction");
        let long = edited(&g, 30, &"Charlie, a longer title ".repeat(4));
        let long_len = umx_core::codec::encode_nodes([&long]).len();
        let mut b = g.batch();
        for i in 0..super::SPLICED_EDITS {
            b.insert_node(node(10, "problem", &format!("Spliced {i}")))
                .unwrap();
        }
        assert_eq!(b.g.image_dead, 0, "the first edits spliced");
        b.insert_node(first).unwrap();
        assert!(b.g.image_dead > 0 && !b.compacted, "the next edit appended");
        // The long line fits only once the dead bytes are gone.
        b.g.limit = Some(b.g.image.len() + long_len - 1);
        b.insert_node(long).unwrap();
        assert!(
            b.compacted,
            "the physical tail ran out, so the batch compacted"
        );
        assert!(b.g.image_dead > 0, "the edit after the compaction appended");
        b.insert_node(node(10, "problem", "A")).unwrap();
        let tails = &b.log.line_tails;
        assert_eq!(tails.len(), super::SPLICED_EDITS + 3);
        assert!(tails[..super::SPLICED_EDITS].iter().all(Option::is_none));
        assert!(tails[super::SPLICED_EDITS..].iter().all(Option::is_some));
        b.rollback();
        g.limit = None;
        g.check_rows();
        assert_eq!((state(&g), g.undo.clone()), before);
    }

    /// review and graph-storage: a rollback that never compacted puts every restored title and name list
    /// past the columns' end again while the image shrinks, so the moved tails could outgrow the image and their
    /// `u32` offsets. Under a small column limit the columns are laid out early instead of passing it: while edits
    /// append, on the rollback's untails (dead image bytes) and on its later spliced line restores (none). Every read
    /// equals a fresh load's after each edit and after the rollback.
    #[test]
    fn a_rollback_lays_the_columns_out_before_they_pass_their_limit() {
        let mut g = hand_edited();
        for i in 0..12u128 {
            g.insert_node(node(40 + i * 10, "risk", "Alpha risk"))
                .unwrap();
        }
        let before = (state(&g), g.undo.clone(), reads(&g));
        let ids: Vec<Id> = g.nodes().map(|n| n.id()).collect();
        let title = |k: usize| format!("Edited title number {k:02}");
        let live = |g: &Graph| (0..g.ids.len()).map(|h| g.title(h).len()).sum::<usize>();
        let growth: usize = ids
            .iter()
            .enumerate()
            .map(|(k, id)| title(k).len() - g.node(*id).unwrap().title().len().min(title(k).len()))
            .sum();
        // Room for every row's new title laid out, and a few tails past it.
        g.column_limit = Some(live(&g) + growth + 60);
        let mut b = g.batch();
        for (k, id) in ids.iter().enumerate() {
            let n = edited(b.graph(), id.0, &title(k));
            b.insert_node(n).unwrap();
            let now = b.graph();
            assert_eq!(reads(now), reads(&fresh(now)), "after edit {k}");
        }
        assert!(
            b.g.early_settles[0] > 0,
            "the edits laid the columns out early"
        );
        assert!(b.g.image_dead > 0 && !b.compacted);
        let tails = b.log.line_tails.iter().filter(|t| t.is_some()).count();
        assert!(
            tails > 0 && tails < b.log.lines.len(),
            "edits spliced, then appended"
        );
        // The restored titles go past the end again: a couple of them pass this limit each time.
        b.g.early_settles = [0, 0];
        b.g.column_limit = Some(live(b.g) + 25);
        b.rollback();
        assert!(
            g.early_settles[0] > 0 && g.early_settles[1] > 0,
            "the untails and the spliced restores both laid the columns out early: {:?}",
            g.early_settles
        );
        g.column_limit = None;
        g.check_rows();
        assert_eq!(reads(&g), reads(&fresh(&g)));
        assert_eq!((state(&g), g.undo.clone(), reads(&g)), before);
    }
}
