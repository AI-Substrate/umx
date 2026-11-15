//! Our own in-memory graph (blueprint 001, d-1; workshop 008 sections 2.1 to 2.4): nodes and edges as columns in id
//! order, addressed by dense `u32` handles, names interned, times as whole seconds, and packed adjacency per
//! direction rebuilt inside every write that adds or removes an edge. Node lines are kept as the bytes of
//! nodes.jsonl (the image): the hot columns and the scalar field columns are derived from each line, and the note,
//! evidence and history are read from the line only when asked. Every result comes back in a stable order (id order
//! unless scored). Pure: no std (blueprint 001, d-3).
#![no_std]

extern crate alloc;

mod batch;
mod cols;
pub mod columns;
#[cfg(test)]
mod difftest;
mod encode;
mod entity;
mod evidence_kinds;
mod external;
mod fields;
mod filter;
mod find;
mod json;
#[cfg(test)]
mod keys_difftest;
mod line;
mod load;
mod locators;
mod nodes;
mod provenance;
mod similar;
mod stem;
mod unique;
mod vectors;
mod view;
mod words;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use cols::{Csr, Entry, Fields, NameCol, Names, Times};
use core::mem::{size_of, size_of_val};
use external::{EXTERNAL, ExternalAdj};
use fields::FieldCol;
use line::FieldVal;
use umx_core::codec::{self, GraphFile, KeptLine};
use umx_core::id::{ID_LEN, normalise_id_text};
use umx_core::ontology::PART_OF;
use umx_core::ontology::TextFields;
use umx_core::{Edge, Id, Node};

pub use batch::{Applied, Batch};
pub use entity::DuplicateEntityKey;
pub use filter::{Op, Where, WhereError, parse_where};
pub use find::{find_keeps, near_duplicate_keeps, title_score};
pub use load::{
    Dangling, EdgesLoad, EdgesMark, GraphIdNode, InvalidProvenance, NodesLoad, Reversed, edge_lines,
};
pub use locators::DuplicateLocator;
pub use provenance::{DuplicateProvKey, ProvenanceEdge, ProvenanceStats};
pub use similar::{SimilarEdge, SimilarLists, SimilarLoad, SimilarSnapshot};
pub use unique::{IndexFull, UniqueIndex};
pub use vectors::{FieldBlock, Put, VectorLoad, WholeBlock};
pub use view::{EdgeEnd, EdgeIx, EdgeRef, Kind, NodeIx, NodeRef, NodeSet, Step};
pub use words::{
    Form, PREFIX_CAP, PREFIX_MIN, PREFIX_WEIGHT, STEM_WEIGHT, Variant, WordQuery, WordStats,
    WordTerm, words,
};

/// One word search hit: its score, the field that earned most of it, the marked query words it holds (an exact or
/// stem match 1, a prefix match [`PREFIX_WEIGHT`]) and how each query word it holds matched.
#[derive(Debug)]
pub struct WordHit<'g> {
    pub node: NodeRef<'g>,
    pub score: f64,
    pub field: &'g str,
    pub held: f64,
    pub terms: Vec<WordTerm>,
}

/// Similarity edges are followed only when a query names them (workshop 008, section 9).
const SIMILAR_TO: &str = "similar_to";

/// Shortest id prefix accepted when naming a node (workshop 007 rule 7); one number with batch refs' id check.
pub const MIN_PREFIX: usize = umx_core::id::MIN_PREFIX;

#[derive(Clone, Debug, Default)]
pub struct Graph {
    names: Names,
    // Nodes, one row per handle, in id order.
    ids: Vec<Id>,
    n_kind: NameCol,
    n_status: NameCol,
    n_by: NameCol,
    n_created: Times,
    n_updated: Times,
    /// Every title end to end; node h's is `titles[title_end[h - 1]..title_end[h]]`, unless `moved` holds it.
    titles: String,
    title_end: Vec<u32>,
    /// Every node's words by field, for word search (req-68); kept by every write that touches a row.
    words: words::WordIndex,
    /// The custom fields the word index holds besides title and note, per node type: those the ontology declares as
    /// text.
    text_fields: TextFields,
    /// The bytes of nodes.jsonl as the next write lays them out (s05): node h's line starts at `line_at[h]` and
    /// ends at the next `\n`. A line umx did not change keeps its bytes. Lines not in use (earlier lines of a
    /// repeated id, req-67) are in it too, listed in `kept_nodes`.
    image: Vec<u8>,
    /// Old placeholder lines, dead until the batch compacts its append-on-fill image.
    image_dead: usize,
    /// Transient batch state; empty outside a batch and never encoded.
    batch_placeholders: BTreeSet<Id>,
    line_at: Vec<u32>,
    /// The image lines that are not records: (id, start), in image order. Never read; written back by every
    /// encode, so never erased.
    kept_nodes: Vec<(Id, u32)>,
    /// Each node's custom field names, in name order: node h's are `nf[nf_end[h - 1]..nf_end[h]]`, unless `moved`
    /// holds it.
    nf: NameCol,
    nf_end: Vec<u32>,
    /// The rows whose title or field names a batch put past the row-ordered part of `titles` and `nf` instead of
    /// shifting every row after them (s26). Empty outside a batch; `settle_columns` lays the columns out again.
    moved: BTreeMap<u32, nodes::Moved>,
    /// The sparse column of each custom field name.
    f_cols: BTreeMap<u32, FieldCol>,
    /// The line the last replace took out, so putting the old record back (an undo) restores its exact bytes.
    undo: Option<(Id, Vec<u8>)>,
    // Edges, one row per handle, in id order. The top bit in an end indexes `ext`, never a node. Add any new edge-side
    // field to `Graph::reset_edges` (load.rs) too, so a reset still leaves the graph as `load_nodes` left it.
    e_ids: Vec<Id>,
    e_kind: NameCol,
    e_by: NameCol,
    e_from: Vec<u32>,
    e_to: Vec<u32>,
    ext: Vec<(Id, Id)>,
    ext_dirty: bool,
    ext_out: ExternalAdj,
    ext_in: ExternalAdj,
    e_created: Times,
    /// Custom fields and unknown keys of the edges that have any, by edge id.
    e_fields: BTreeMap<Id, Fields>,
    /// The `evidence` slot of the edges that have one (req-124), as the canonical bytes of the JSON array, by edge
    /// id: cold and sparse, decoded on read (graph-storage-expert's layout for ws006 S5b).
    e_evidence: BTreeMap<Id, Box<[u8]>>,
    out: Csr,
    inc: Csr,
    /// Edge file lines that are not edges of the graph: earlier lines of repeated ids (req-67) and dangling edges
    /// (req-76). Never read or queried; written back by every encode, so never erased.
    kept_edges: Vec<KeptLine>,
    /// Int8 vector blocks per vector space and field (workshop 008 section 2.5). Never written to the graph files.
    vectors: vectors::Vectors,
    /// Stored similar_to lists per node handle, and who holds each node (workshop 008 section 9).
    similar: similar::Similar,
    provenance: provenance::Provenance,
    /// Resource rows' locator kind and format, and the one locator index (ws006 S3).
    locators: locators::Locators,
    /// Each node's `entity_key` list by type and key (ws008 E1).
    entities: entity::EntityKeys,
    /// Each evidence kind's rows, in handle order (ws003 s22), and each row's kinds (s29).
    ev_kinds: evidence_kinds::EvidenceKinds,
    /// Tests only: a smaller image limit than `nodes::LIMIT`, to reach the refusal without 4 GiB of lines.
    #[cfg(test)]
    limit: Option<usize>,
    /// Tests only: a smaller column limit than `nodes::COLUMN_LIMIT`, to reach a batch's early settle without 4 GiB
    /// of titles.
    #[cfg(test)]
    column_limit: Option<usize>,
    /// Tests only: early column settles with dead image bytes and without (`settle_before_cap`).
    #[cfg(test)]
    early_settles: [usize; 2],
    /// Tests only: how many times adjacency was built, so a batch can be shown to build it once.
    #[cfg(test)]
    adjacency_builds: usize,
    #[cfg(test)]
    node_rebuilds: usize,
    #[cfg(test)]
    edge_rebuilds: usize,
}

/// One name's use in a graph ([`Graph::census`]): how many nodes (or edges) have it, and the lowest id among them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tally<'a> {
    pub name: &'a str,
    pub count: usize,
    pub first: Id,
}

/// What [`Graph::census`] returns: node types, statuses and relations in use, each in name order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Census<'a> {
    pub types: Vec<Tally<'a>>,
    pub statuses: Vec<Tally<'a>>,
    pub relations: Vec<Tally<'a>>,
}

/// A node write refused because nodes.jsonl's image would pass what umx holds (u32 offsets: 4 GiB). Nothing
/// changed. `size` is the bytes the image would have had, `limit` the most it may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageFull {
    pub size: u64,
    pub limit: u64,
}

/// The result of naming a node by id or id prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    Found(Id),
    /// Several nodes share the prefix (all of them, in id order).
    Ambiguous(Vec<Id>),
    /// Nothing matched; up to three ids sharing the longest leading text with what was given.
    NotFound(Vec<Id>),
}

#[derive(Clone, Debug, Default)]
pub struct NodeFilter {
    pub kind: Option<String>,
    pub status: Option<String>,
    pub wheres: Vec<Where>,
    pub within: Option<Id>,
}

/// s06's normalised title arena, rebuilt for the bench's comparison only (see [`Graph::scan_titles`]).
#[cfg(feature = "bench")]
pub struct ScanTitles {
    text: String,
    end: Vec<u32>,
}

#[cfg(feature = "bench")]
impl ScanTitles {
    /// Its heap bytes: what s06 kept per graph.
    pub fn heap_bytes(&self) -> usize {
        self.text.capacity() + self.end.capacity() * size_of::<u32>()
    }
}

/// Heap bytes the graph holds, by part (req-54). `hot` is what every read touches; `image` is the nodes file held in
/// memory; `field_columns` the scalar field columns; `cold` is edge custom fields, odd times, kept lines' places and
/// bytes, and the undo line; `vectors` is the vector blocks; `similarity` the similar_to lists and their reverse.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryBytes {
    pub node_columns: usize,
    pub edge_columns: usize,
    pub adjacency: usize,
    pub field_columns: usize,
    pub image: usize,
    pub cold: usize,
    /// The word index (req-68); read only by word search, so not part of `hot`.
    pub word_index: usize,
    pub vectors: usize,
    pub similarity: usize,
    /// External endpoint table and sparse external adjacency.
    pub external: usize,
    pub provenance: usize,
    /// Resource rows' kind and format column and the locator index (ws006 S3).
    pub locators: usize,
    /// The entity key index (ws008 E1).
    pub entity_keys: usize,
    /// The evidence kinds column (ws003 s22): the kind texts and each kind's rows.
    pub evidence_kinds: usize,
    /// The evidence kinds forward column (ws003 s29): each row's kind numbers.
    pub evidence_kinds_forward: usize,
    /// The typed time column: each date value's span and precision (D71 step 10).
    pub date_columns: usize,
}

impl MemoryBytes {
    pub fn hot(&self) -> usize {
        self.node_columns + self.edge_columns + self.adjacency
    }

    pub fn total(&self) -> usize {
        self.hot()
            + self.field_columns
            + self.image
            + self.cold
            + self.word_index
            + self.vectors
            + self.similarity
            + self.external
            + self.provenance
            + self.locators
            + self.entity_keys
            + self.evidence_kinds
            + self.evidence_kinds_forward
            + self.date_columns
    }
}

/// The handles of `numbers` whose row `fits` and whose value passes `test`, picked a block at a time without a branch
/// per holder.
#[inline]
fn pick(
    numbers: impl Iterator<Item = (u32, f64)>,
    fits: impl Fn(u32) -> bool,
    test: impl Fn(f64) -> bool,
) -> Vec<u32> {
    let mut picked = Vec::new();
    let mut block = [0u32; 256];
    let mut n = 0;
    for (h, x) in numbers {
        block[n] = h;
        n += usize::from(fits(h) & test(x));
        if n == block.len() {
            picked.extend_from_slice(&block);
            n = 0;
        }
    }
    picked.extend_from_slice(&block[..n]);
    picked
}

/// Keeps the last of each run of equal ids, sorted by id; a no-op on records already sorted without repeats.
fn last_per_id<T>(mut records: Vec<T>, id: impl Fn(&T) -> Id) -> Vec<T> {
    if records.windows(2).all(|w| id(&w[0]) < id(&w[1])) {
        return records;
    }
    records.sort_by_key(&id);
    let mut out = Vec::with_capacity(records.len());
    let mut it = records.into_iter().peekable();
    while let Some(r) = it.next() {
        if it.peek().is_none_or(|next| id(next) != id(&r)) {
            out.push(r);
        }
    }
    out
}

impl Graph {
    pub fn new() -> Graph {
        Graph::default()
    }

    /// A graph from records, in any order; for a repeated id the later record wins. The nodes go through the load
    /// path as the lines umx writes for them. An edge with a local end not among `nodes` is not an edge of the
    /// graph (req-76): it is kept as its canonical line and written back by `encode_edges`. External ends remain
    /// unresolved. Services load from the
    /// files instead (`load_nodes`, `load_edges`).
    pub fn from_records(nodes: Vec<Node>, edges: Vec<Edge>) -> Graph {
        Graph::from_records_indexing(nodes, edges, TextFields::default())
    }

    /// `from_records`, with the custom fields word search indexes besides title and note (`load_nodes_indexing`).
    pub fn from_records_indexing(
        nodes: Vec<Node>,
        edges: Vec<Edge>,
        text_fields: TextFields,
    ) -> Graph {
        let nodes = last_per_id(nodes, |n| n.id);
        let (mut g, _) = Graph::load_nodes_indexing(codec::encode_nodes(&nodes), text_fields)
            .expect("records umx encodes load");
        drop(nodes);
        let edges = last_per_id(edges, |e| e.id);
        g.e_ids.reserve_exact(edges.len());
        g.e_from.reserve_exact(edges.len());
        g.e_to.reserve_exact(edges.len());
        for e in edges {
            match g.edge_handles(e.from_graph, e.from, e.to_graph, e.to) {
                Some((f, t)) => g.push_edge(e, f, t),
                _ => {
                    let mut line = codec::encode_edges([&e]);
                    line.pop();
                    g.kept_edges.push(KeptLine { id: e.id, line });
                }
            }
        }
        g.rebuild_adjacency();
        g.shrink();
        g
    }

    /// Adds file lines that are not records (repeated ids' earlier lines, dangling edges' exact lines). A node line
    /// goes into the image just before the line in use for its id. They are written back by every encode and
    /// survive every write. A node line that would take the image past 4 GiB is refused (`ImageFull`); the lines
    /// before it stay.
    pub fn keep_lines(
        &mut self,
        nodes: Vec<KeptLine>,
        edges: Vec<KeptLine>,
    ) -> Result<(), ImageFull> {
        for k in nodes {
            self.keep_node_line(k)?;
        }
        self.kept_edges.extend(edges);
        Ok(())
    }

    /// How many kept lines the graph holds (nodes plus edges).
    pub fn kept_line_count(&self) -> usize {
        self.kept_nodes.len() + self.kept_edges.len()
    }

    /// The kept edge lines' exact bytes, without their newline, in the order they were kept: what [`Self::encode_edges`]
    /// writes besides the edges (s20: services checks their external ends without scanning the whole file).
    pub fn kept_edge_lines(&self) -> impl Iterator<Item = &[u8]> {
        self.kept_edges.iter().map(|k| k.line.as_slice())
    }

    /// The kept node lines' exact bytes (earlier lines of a repeated id), without their newline, in image order: what
    /// [`Self::encode_nodes`] writes besides the nodes (F183: an evidence redact refuses while one holds the pointer).
    pub fn kept_node_lines(&self) -> impl Iterator<Item = &[u8]> {
        self.kept_nodes.iter().map(|(_, at)| {
            let start = *at as usize;
            &self.image[start..json::newline(&self.image, start)]
        })
    }

    /// Drops the one-slot undo line (the line the last replace took out), so its bytes are no longer held: an evidence
    /// redact calls it once its write is in, since that line is the one it scrubbed (F196, storage review). A later put
    /// back of that old record is written in umx's own form instead of its old bytes.
    pub fn forget_undo(&mut self) {
        self.undo = None;
    }

    /// The one-slot undo line, if one is held. For tests: a redact's own check that it left no old bytes behind.
    #[doc(hidden)]
    pub fn undo_line(&self) -> Option<&[u8]> {
        self.undo.as_ref().map(|(_, line)| line.as_slice())
    }

    /// The lines (1-based) `id` is on in `file` as the next encode lays it out, if `file` holds kept lines for it
    /// (a repeated id): every kept line, then the line in use.
    pub fn repeat_lines(&self, file: GraphFile, id: Id) -> Option<Vec<usize>> {
        let (copies, kept_before, before) = match file {
            GraphFile::Nodes => (
                self.kept_nodes.iter().filter(|k| k.0 == id).count(),
                self.kept_nodes.iter().filter(|k| k.0 < id).count(),
                self.ids.partition_point(|x| *x < id),
            ),
            GraphFile::Edges => (
                self.kept_edges.iter().filter(|k| k.id == id).count(),
                self.kept_edges.iter().filter(|k| k.id < id).count(),
                self.e_ids.partition_point(|x| *x < id),
            ),
            GraphFile::Graph | GraphFile::GitIgnore => return None,
        };
        if copies == 0 {
            return None;
        }
        let first = before + kept_before + 1;
        Some((first..=first + copies).collect())
    }

    #[inline]
    fn handle(&self, id: Id) -> Option<u32> {
        self.ids.binary_search(&id).ok().map(|h| h as u32)
    }

    #[inline]
    fn edge_handle(&self, id: Id) -> Option<usize> {
        self.e_ids.binary_search(&id).ok()
    }

    #[inline]
    fn title(&self, h: usize) -> &str {
        &self.titles[self.title_range(h)]
    }

    fn push_edge(&mut self, e: Edge, from: u32, to: u32) {
        e.assert_provenance_fields();
        self.push_edge_parts(line::HotEdge::of(e), from, to);
    }

    fn push_edge_parts(&mut self, e: line::HotEdge<'_>, from: u32, to: u32) {
        let k = self.names.intern(&e.kind);
        self.names.edge_use(k, true);
        let b = self.names.intern(&e.by);
        self.e_ids.push(e.id);
        self.e_kind.push(k);
        self.e_by.push(b);
        self.e_from.push(from);
        self.e_to.push(to);
        self.e_created.push(e.id, &e.created);
        self.provenance.edge_authored.push(e.authored);
        self.provenance.edge_edited.set(e.id, e.edited);
        debug_assert!(
            e.evidence.is_empty() || !e.fields.contains_key(umx_core::model::EVIDENCE),
            "an edge holds evidence in its slot or as a custom field, never both"
        );
        if !e.evidence.is_empty() {
            self.e_evidence
                .insert(e.id, encode::evidence_bytes(&e.evidence));
        }
        if !e.fields.is_empty() {
            self.e_fields.insert(e.id, cols::fields_from(e.fields));
        }
    }

    fn set_edge(&mut self, h: usize, e: Edge, from: u32, to: u32) {
        e.assert_provenance_fields();
        self.ext_dirty |= (self.e_from[h] | self.e_to[h]) & EXTERNAL != 0;
        self.names.edge_use(self.e_kind.get(h), false);
        let k = self.names.intern(&e.kind);
        self.names.edge_use(k, true);
        let b = self.names.intern(&e.created_by);
        self.e_kind.set(h, k);
        self.e_by.set(h, b);
        self.e_from[h] = from;
        self.e_to[h] = to;
        self.e_created.set(h, e.id, &e.created);
        self.provenance.edge_authored.set(h, e.authored_by_session);
        self.provenance.edge_edited.set(e.id, e.edited_by_session);
        debug_assert!(
            e.evidence.is_empty() || !e.fields.contains_key(umx_core::model::EVIDENCE),
            "an edge holds evidence in its slot or as a custom field, never both"
        );
        self.e_evidence.remove(&e.id);
        if !e.evidence.is_empty() {
            self.e_evidence
                .insert(e.id, encode::evidence_bytes(&e.evidence));
        }
        self.e_fields.remove(&e.id);
        if !e.fields.is_empty() {
            self.e_fields.insert(e.id, cols::fields_from(e.fields));
        }
    }

    fn remove_edge_row(&mut self, h: usize) {
        self.ext_dirty |= (self.e_from[h] | self.e_to[h]) & EXTERNAL != 0;
        let id = self.e_ids.remove(h);
        self.names.edge_use(self.e_kind.get(h), false);
        self.e_kind.remove(h);
        self.e_by.remove(h);
        self.e_from.remove(h);
        self.e_to.remove(h);
        self.e_created.remove(h, id);
        self.provenance.edge_authored.remove(h);
        self.provenance.edge_edited.remove(id);
        self.e_fields.remove(&id);
        self.e_evidence.remove(&id);
    }

    fn edge_record(&self, h: usize) -> Edge {
        let id = self.e_ids[h];
        let (from_graph, from) = self.end_parts(self.e_from[h]);
        let (to_graph, to) = self.end_parts(self.e_to[h]);
        Edge {
            id,
            kind: self.names.text(self.e_kind.get(h)).into(),
            from_graph,
            from,
            to_graph,
            to,
            created: self.e_created.get(h, id),
            created_by: self.names.text(self.e_by.get(h)).into(),
            authored_by_session: self.provenance.edge_authored.get(h),
            edited_by_session: self.provenance.edge_edited.get(id),
            evidence: self.edge_evidence(id),
            fields: self
                .e_fields
                .get(&id)
                .map_or(BTreeMap::new(), |f| cols::fields_to(f)),
        }
    }

    /// The edges holding a custom `evidence` field that is not an evidence list (kept as written, never read as
    /// evidence; services warn `evidence_field_kept`), in id order. Only edges with custom fields are looked at.
    pub fn edges_with_invalid_evidence(&self) -> impl Iterator<Item = Id> + '_ {
        self.e_fields
            .iter()
            .filter(|(_, f)| cols::field_in(f, umx_core::model::EVIDENCE).is_some())
            .map(|(id, _)| *id)
    }

    /// Edge `id`'s evidence slot, decoded; empty when it has none.
    pub(crate) fn edge_evidence(&self, id: Id) -> Vec<umx_core::Value> {
        self.e_evidence
            .get(&id)
            .map_or_else(Vec::new, |b| umx_core::model::decode_evidence(b))
    }

    /// Both adjacency arrays, rebuilt in full from the edge columns (workshop 008 section 2.3). Every write that adds
    /// or removes an edge calls it before returning. Public only with the `bench` feature, so the bench can time it.
    #[cfg(feature = "bench")]
    pub fn rebuild_adjacency(&mut self) {
        self.build_adjacency();
    }

    #[cfg(not(feature = "bench"))]
    pub(crate) fn rebuild_adjacency(&mut self) {
        self.build_adjacency();
    }

    fn build_adjacency(&mut self) {
        #[cfg(test)]
        {
            self.adjacency_builds += 1;
        }
        let n = self.ids.len();
        self.out.build(n, &self.e_from, &self.e_to);
        self.inc.build(n, &self.e_to, &self.e_from);
        self.build_external();
    }

    /// s06's title find, kept for the bench only (ws004 s15): every title's normalised text in one arena, and a scan
    /// that scores all of them. The branch's `find` must not be slower than this, and gives the same results.
    #[cfg(feature = "bench")]
    pub fn scan_titles(&self) -> ScanTitles {
        let mut t = ScanTitles {
            text: String::new(),
            end: Vec::with_capacity(self.ids.len()),
        };
        for h in 0..self.ids.len() {
            find::normalise_into(self.title(h), &mut t.text);
            t.end.push(t.text.len() as u32);
        }
        t.text.shrink_to_fit();
        t
    }

    /// s06's scan over `titles` (see [`Graph::scan_titles`]): (id, score), best first then by id.
    #[cfg(feature = "bench")]
    pub fn find_scan(&self, titles: &ScanTitles, text: &str, limit: usize) -> Vec<(Id, f64)> {
        let Some(q) = find::Query::new(text) else {
            return Vec::new();
        };
        let mut buf = Vec::new();
        let mut scored: Vec<(usize, f64)> = (0..self.ids.len())
            .map(|h| {
                let start = if h == 0 {
                    0
                } else {
                    titles.end[h - 1] as usize
                };
                let t = &titles.text[start..titles.end[h] as usize];
                (h, find::score(&q, t, &mut buf))
            })
            .filter(|(_, s)| *s >= find::MIN_SCORE)
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        scored.truncate(limit);
        scored.into_iter().map(|(h, s)| (self.ids[h], s)).collect()
    }

    /// How many titles `find` would score for `text` (its candidates) and the distinct terms in the word index:
    /// what the term-table scan costs, for the bench.
    #[cfg(feature = "bench")]
    pub fn title_candidates(&self, text: &str) -> (usize, usize) {
        let Some(q) = find::Query::new(text) else {
            return (0, self.words.term_count());
        };
        (
            self.words.title_candidates(&q.0, find::MIN_SCORE).len(),
            self.words.term_count(),
        )
    }
    /// [`Graph::title_candidates`]'s count through s15's term scan (every term's bigrams made, sorted and merged), the
    /// base ws004 s18 is timed against.
    #[cfg(feature = "bench")]
    pub fn title_candidates_s15(&self, text: &str) -> usize {
        self.title_candidates_s15_at(text, find::MIN_SCORE)
    }

    /// [`Graph::title_candidates_s15`] at threshold `min` (0.85 is the near-duplicate check's), bench only.
    #[cfg(feature = "bench")]
    pub fn title_candidates_s15_at(&self, text: &str, min: f64) -> usize {
        find::Query::new(text).map_or(0, |q| {
            let m = min - 0.001;
            let (walk, r_lo) = self.words.title_terms_scan(&q.0, m / (2.0 - m));
            self.words.gather(&q.0, m, walk, r_lo).len()
        })
    }

    /// The word index's term-side bigram figures (ws004 s18): (heap bytes, bigrams held), bench only.
    #[cfg(feature = "bench")]
    pub fn term_grams_bytes(&self) -> (usize, usize) {
        self.words.term_grams_bytes()
    }

    /// The word index's distinct terms and how many of them are title words (s21), bench only.
    #[cfg(feature = "bench")]
    pub fn word_terms(&self) -> (usize, usize) {
        (self.words.term_count(), self.words.title_term_count())
    }

    /// How many candidates the near-duplicate check (at 0.85) keeps for `text` (bench only).
    /// ws004 s21: the heap bytes of the word index's term order and stem classes, and the distinct terms.
    #[cfg(feature = "bench")]
    pub fn word_order_and_stem_bytes(&self) -> (usize, usize) {
        self.words.order_and_stem_bytes()
    }

    #[cfg(feature = "bench")]
    pub fn title_candidates_near(&self, text: &str) -> usize {
        find::Query::new(text).map_or(0, |q| self.words.title_candidates(&q.0, 0.85).len())
    }

    /// The word index rebuilt in full from the image, as load builds it. Public so the bench can time the build.
    #[doc(hidden)]
    pub fn rebuild_words(&mut self) {
        self.words = words::WordIndex::new();
        self.words.reserve_exact(self.ids.len());
        self.words.begin_build();
        for h in 0..self.ids.len() {
            self.index_row(h);
        }
        self.words.finish_build();
        self.words.shrink();
    }

    /// The word index's part of a write to node `id`: its words out, then in again. Public so the bench can time
    /// the index's own update apart from the image splice the write also does.
    #[doc(hidden)]
    pub fn reindex_words(&mut self, id: Id) {
        if let Some(h) = self.handle(id) {
            self.unindex_row(h as usize);
            self.index_row(h as usize);
        }
    }

    /// The custom fields word search indexes besides title and note. A change (an ontology write that declares a
    /// text field or changes one's kind) reindexes every node inside that write, never later on a read.
    pub fn set_text_fields(&mut self, text_fields: TextFields) {
        if text_fields != self.text_fields {
            self.text_fields = text_fields;
            self.rebuild_words();
        }
    }

    /// This graph's figures for a word query, to be summed (`WordStats::add`) with the other graphs searched before
    /// any of them is scored.
    pub fn word_stats(&self, q: &WordQuery) -> WordStats {
        let mut stats = self.words.stats(q);
        stats.nodes -= self.provenance_count() as u64;
        stats
    }

    /// Nodes matching `q`'s words by BM25F, best first then by handle (id order), at most `limit`, each with its score
    /// and the field that earned most of it. `stats` are the figures of every graph searched, summed; `scope` limits
    /// the nodes scored (the same bitset lists use), never the figures.
    pub fn word_search(
        &self,
        q: &WordQuery,
        stats: &WordStats,
        scope: Option<&NodeSet<'_>>,
        limit: usize,
    ) -> Vec<(NodeRef<'_>, f64, &str)> {
        self.search_words(q, stats, scope, limit, None)
            .into_iter()
            .map(|w| (w.node, w.score, w.field))
            .collect()
    }

    /// [`Self::word_search`], each hit also with the query words `marks` flags (one flag per word of `q`, in its
    /// order) that it holds, an exact or stem match counting 1 and a prefix match [`words::PREFIX_WEIGHT`], and how
    /// each word it holds matched. Scores are unchanged. Hybrid search weights a word hit's credit by that share of
    /// the query's informative terms (embeddings-expert, s11, s21).
    pub fn word_search_marked(
        &self,
        q: &WordQuery,
        stats: &WordStats,
        scope: Option<&NodeSet<'_>>,
        limit: usize,
        marks: &[bool],
    ) -> Vec<WordHit<'_>> {
        self.search_words(q, stats, scope, limit, Some(marks))
    }

    fn search_words(
        &self,
        q: &WordQuery,
        stats: &WordStats,
        scope: Option<&NodeSet<'_>>,
        limit: usize,
        marks: Option<&[bool]>,
    ) -> Vec<WordHit<'_>> {
        let within = |h: u32| scope.is_none_or(|s| s.has(h));
        let plans = stats.plans(q);
        self.words
            .search(
                &plans,
                stats,
                scope.map(|_| &within as &dyn Fn(u32) -> bool),
                limit,
                marks,
            )
            .into_iter()
            .map(|(h, score, held)| {
                let (field, terms) = self.words.explain(q, &plans, stats, h);
                WordHit {
                    node: NodeRef { g: self, h },
                    score,
                    field,
                    held,
                    terms,
                }
            })
            .collect()
    }

    /// The word index as an image that does not depend on how it was built, for tests that compare it with a fresh
    /// load after writes.
    #[doc(hidden)]
    pub fn word_index_image(&self) -> String {
        self.words.image(&self.ids)
    }

    /// The rare case for edges (workshop 008 section 2.1): an edge id below the last renumbers edge handles, so the
    /// edge columns are built again from records, as a load builds them. Kept lines come through unchanged.
    fn rebuild_edges(&mut self, add: Edge) {
        self.merge_edge_records(core::iter::once(add), &BTreeSet::new())
            .expect("validated edge ends");
        self.rebuild_adjacency();
    }

    fn merge_edge_records(
        &mut self,
        added: impl IntoIterator<Item = Edge>,
        removed: &BTreeSet<Id>,
    ) -> Result<(), Id> {
        let added: Vec<Edge> = added.into_iter().collect();
        for e in &added {
            if (e.from_graph.is_none() && self.handle(e.from).is_none())
                || (e.to_graph.is_none() && self.handle(e.to).is_none())
            {
                return Err(e.id);
            }
        }
        #[cfg(test)]
        {
            self.edge_rebuilds += 1;
        }
        let mut edges: Vec<Edge> = (0..self.e_ids.len())
            .filter(|&h| !removed.contains(&self.e_ids[h]))
            .map(|h| self.edge_record(h))
            .collect();
        edges.extend(added);
        edges.sort_by_key(|e| e.id);
        for h in (0..self.e_ids.len()).rev() {
            self.remove_edge_row(h);
        }
        self.ext.clear();
        self.ext_dirty = false;
        for e in edges {
            let (f, t) = self
                .edge_handles(e.from_graph, e.from, e.to_graph, e.to)
                .expect("edge ends");
            self.push_edge(e, f, t);
        }
        Ok(())
    }

    fn shrink(&mut self) {
        self.names.shrink();
        self.ids.shrink_to_fit();
        self.n_kind.shrink();
        self.n_status.shrink();
        self.n_by.shrink();
        self.n_created.shrink();
        self.n_updated.shrink();
        self.titles.shrink_to_fit();
        self.title_end.shrink_to_fit();
        self.words.shrink();
        self.image.shrink_to_fit();
        self.line_at.shrink_to_fit();
        self.kept_nodes.shrink_to_fit();
        self.nf.shrink();
        self.nf_end.shrink_to_fit();
        self.f_cols.values_mut().for_each(FieldCol::shrink);
        self.e_ids.shrink_to_fit();
        self.e_kind.shrink();
        self.e_by.shrink();
        self.e_from.shrink_to_fit();
        self.e_to.shrink_to_fit();
        self.e_created.shrink();
        self.out.shrink();
        self.inc.shrink();
        self.ext.shrink_to_fit();
        self.ext_out.shrink();
        self.ext_in.shrink();
        self.provenance.shrink();
        self.locators.shrink();
        self.entities.shrink();
        self.ev_kinds.shrink();
    }

    #[inline]
    pub fn node(&self, id: Id) -> Option<NodeRef<'_>> {
        self.handle(id).map(|h| NodeRef { g: self, h })
    }

    #[inline]
    pub fn edge(&self, id: Id) -> Option<EdgeRef<'_>> {
        self.edge_handle(id).map(|h| EdgeRef {
            g: self,
            h: h as u32,
        })
    }

    #[inline]
    pub fn node_ix(&self, id: Id) -> Option<NodeIx> {
        self.handle(id).map(NodeIx)
    }

    /// One more than the largest node handle's `index()`, for sizing a bitset or a visit stamp per node. Good only
    /// for the `&Graph` borrow it came from, like a handle.
    #[inline]
    pub fn node_bound(&self) -> usize {
        self.ids.len()
    }

    /// The node behind a handle. Panics if the handle did not come from this graph in the current borrow. A
    /// dangling edge is not an edge of the graph (req-76), so every `Step.other` and edge end resolves.
    #[inline]
    pub fn node_at(&self, ix: NodeIx) -> NodeRef<'_> {
        assert!(
            ix.index() < self.ids.len(),
            "a node handle from another borrow"
        );
        NodeRef { g: self, h: ix.0 }
    }

    /// The edge behind a handle. Panics if the handle did not come from this graph in the current borrow.
    #[inline]
    pub fn edge_at(&self, ix: EdgeIx) -> EdgeRef<'_> {
        assert!(
            ix.index() < self.e_ids.len(),
            "an edge handle from another borrow"
        );
        EdgeRef { g: self, h: ix.0 }
    }

    /// Every node, in id order.
    #[inline]
    pub fn nodes(&self) -> impl Iterator<Item = NodeRef<'_>> {
        (0..self.ids.len() as u32).map(move |h| NodeRef { g: self, h })
    }

    /// Every stored edge, in id order, including external edges. Use `edge_ends` before resolving its ends locally.
    #[inline]
    pub fn edges(&self) -> impl Iterator<Item = EdgeRef<'_>> {
        (0..self.e_ids.len() as u32).map(move |h| EdgeRef { g: self, h })
    }

    /// The relation or node type named `name`, if any edge or node in the graph uses it.
    pub fn kind_of(&self, name: &str) -> Option<Kind<'_>> {
        let n = self.names.get(name)?;
        (self.names.used_by_edge(n) || self.names.used_by_node(n)).then(|| Kind(self.names.text(n)))
    }

    pub fn kind_name<'a>(&'a self, kind: Kind<'a>) -> &'a str {
        kind.as_str()
    }

    /// The distinct relations the edges use, in name order. `similar_to` is left out (workshop 008, section 9).
    pub fn relation_kinds(&self) -> Vec<Kind<'_>> {
        self.names
            .in_name_order()
            .filter(|&n| self.names.used_by_edge(n))
            .map(|n| self.names.text(n))
            .filter(|k| *k != SIMILAR_TO)
            .map(Kind)
            .collect()
    }

    /// The distinct node types present, in name order.
    pub fn node_kinds(&self) -> Vec<Kind<'_>> {
        self.names
            .in_name_order()
            .filter(|&n| {
                let placeholders = if self.names.text(n).is_empty() {
                    self.batch_placeholders.len()
                } else {
                    0
                };
                self.names.node_uses(n) > placeholders
            })
            .map(|n| Kind(self.names.text(n)))
            .collect()
    }

    /// Every node type, status and relation in use, each with how many records use it and the lowest-id one, in name
    /// order (`umx graph guide`, plan 079). One pass over the name columns: O(nodes + edges), never a pass per name.
    /// Names no record uses (interned before a write took them out) and the `similar_to` relation are left out.
    pub fn census(&self) -> Census<'_> {
        self.census_of(false)
    }

    /// [`Graph::census`] without provenance (ws007 P3 default hiding): nodes with the provenance bit, and edges with a
    /// local end on one, are left out of every count and never chosen as an example. Same single pass.
    pub fn content_census(&self) -> Census<'_> {
        self.census_of(true)
    }

    /// Nodes and edges `content_census` counts: (nodes, edges) without provenance.
    pub fn content_counts(&self) -> (usize, usize) {
        let nodes = self.ids.len() - self.provenance_count();
        let edges = (0..self.e_ids.len())
            .filter(|&h| !self.edge_touches_provenance(h))
            .count();
        (nodes, edges)
    }

    /// Whether edge `h` has a local end on a provenance node (an external end never is one here).
    fn edge_touches_provenance(&self, h: usize) -> bool {
        [self.e_from[h], self.e_to[h]]
            .into_iter()
            .any(|end| end & EXTERNAL == 0 && self.provenance.has(end))
    }

    fn census_of(&self, content_only: bool) -> Census<'_> {
        let mut nodes = vec![(0u32, 0u32); self.names.len()];
        let mut statuses = vec![(0u32, 0u32); self.names.len()];
        let mut edges = vec![(0u32, 0u32); self.names.len()];
        let tally =
            |col: &NameCol, rows: usize, skip: &dyn Fn(usize) -> bool, out: &mut [(u32, u32)]| {
                // Handles are in id order, so a name's first handle is its lowest id.
                for h in (0..rows).rev() {
                    if skip(h) {
                        continue;
                    }
                    let t = &mut out[col.get(h) as usize];
                    *t = (t.0 + 1, h as u32);
                }
            };
        let node_skip = |h: usize| content_only && self.provenance.has(h as u32);
        let edge_skip = |h: usize| content_only && self.edge_touches_provenance(h);
        tally(&self.n_kind, self.ids.len(), &node_skip, &mut nodes);
        tally(&self.n_status, self.ids.len(), &node_skip, &mut statuses);
        tally(&self.e_kind, self.e_ids.len(), &edge_skip, &mut edges);
        let list = |counts: &[(u32, u32)], ids: &[Id]| -> Vec<Tally<'_>> {
            self.names
                .in_name_order()
                .filter(|&n| counts[n as usize].0 > 0)
                .map(|n| Tally {
                    name: self.names.text(n),
                    count: counts[n as usize].0 as usize,
                    first: ids[counts[n as usize].1 as usize],
                })
                .collect()
        };
        // similar_to is reserved only as a relation; a node type or status may use the name.
        let mut relations = list(&edges, &self.e_ids);
        relations.retain(|t| t.name != SIMILAR_TO);
        Census {
            types: list(&nodes, &self.ids),
            statuses: list(&statuses, &self.ids),
            relations,
        }
    }

    pub fn node_count(&self) -> usize {
        self.ids.len()
    }

    pub fn edge_count(&self) -> usize {
        self.e_ids.len()
    }

    /// Inserts or replaces a node; returns the one it replaced. The node's line is written into the image in the form
    /// umx writes, and its columns are derived from that line. A record equal to the one held changes nothing (the
    /// line keeps its bytes). A new id larger than every other (the normal case: ids are time ordered) is appended; a
    /// smaller one derives every row again from the image, the load path. A write that would take the image past
    /// 4 GiB is refused (`ImageFull`) and changes nothing.
    pub fn insert_node(&mut self, node: Node) -> Result<Option<Node>, ImageFull> {
        match self.ids.binary_search(&node.id) {
            Ok(h) => self.replace_node(h, node).map(Some),
            Err(h) => {
                debug_assert!(
                    self.kept_nodes
                        .binary_search_by_key(&node.id, |line| line.0)
                        .is_err(),
                    "a kept-only id cannot be inserted"
                );
                self.add_node(h, &node).map(|()| None)
            }
        }
    }

    /// Removes a node that no edge touches (used to undo an add; remove its edges first). Returns `None`, and
    /// changes nothing, when the node is absent or an edge still touches it. Earlier lines of a repeated id stay in
    /// the image.
    pub fn remove_node(&mut self, id: Id) -> Option<Node> {
        let h = self.handle(id)?;
        if !self.out.of(h).is_empty()
            || !self.inc.of(h).is_empty()
            || !self.ext_out.of(h).is_empty()
            || !self.ext_in.of(h).is_empty()
        {
            return None;
        }
        Some(self.drop_node(h as usize))
    }

    /// Inserts or replaces an edge and rebuilds adjacency. Local ends must exist; external ends remain unresolved.
    /// A dangling local end is refused (`None`, nothing changes), so a dangling edge only ever comes from a load.
    pub fn insert_edge(&mut self, edge: Edge) -> Option<Edge> {
        let Some((from, to)) =
            self.edge_handles(edge.from_graph, edge.from, edge.to_graph, edge.to)
        else {
            debug_assert!(false, "insert_edge: a dangling edge {:?}", edge.id);
            return None;
        };
        let old = match self.e_ids.binary_search(&edge.id) {
            Ok(h) => {
                let old = self.edge_record(h);
                self.set_edge(h, edge, from, to);
                Some(old)
            }
            Err(h) if h == self.e_ids.len() => {
                self.push_edge(edge, from, to);
                None
            }
            Err(_) => {
                self.rebuild_edges(edge);
                return None;
            }
        };
        self.rebuild_adjacency();
        old
    }

    /// Removes an edge of the graph and rebuilds adjacency. A kept (dangling) edge line is not an edge of the graph.
    pub fn remove_edge(&mut self, id: Id) -> Option<Edge> {
        let h = self.edge_handle(id)?;
        let old = self.edge_record(h);
        self.remove_edge_row(h);
        self.rebuild_adjacency();
        Some(old)
    }

    #[inline]
    fn step(&self, e: &Entry) -> Step<'_> {
        Step {
            edge: EdgeIx(e.edge),
            other: NodeIx(e.other),
            kind: Kind(self.names.text(self.e_kind.get(e.edge as usize))),
        }
    }

    /// Edges leaving `node`, in edge id order. Parallel edges are listed separately; a self-loop once here and once
    /// in `edges_in`.
    #[inline]
    pub fn edges_out(&self, node: NodeIx) -> impl Iterator<Item = Step<'_>> {
        self.out.of(node.0).iter().map(|e| self.step(e))
    }

    /// Edges arriving at `node`, in edge id order. Same rules as `edges_out`.
    #[inline]
    pub fn edges_in(&self, node: NodeIx) -> impl Iterator<Item = Step<'_>> {
        self.inc.of(node.0).iter().map(|e| self.step(e))
    }

    pub fn find_edge(&self, from: Id, kind: &str, to: Id) -> Option<EdgeRef<'_>> {
        let (f, t, k) = (self.handle(from)?, self.handle(to)?, self.names.get(kind)?);
        self.out
            .of(f)
            .iter()
            .find(|e| e.other == t && self.e_kind.get(e.edge as usize) == k)
            .map(|e| EdgeRef { g: self, h: e.edge })
    }

    /// Names a node by full id or a unique prefix of at least `MIN_PREFIX` characters (any case).
    pub fn lookup(&self, text: &str) -> Lookup {
        self.lookup_where(text, |_| true)
    }

    /// Resolves within a view: excluded nodes contribute neither ambiguity nor suggestions.
    pub fn lookup_where(&self, text: &str, visible: impl Fn(NodeIx) -> bool) -> Lookup {
        let Some(text) = normalise_id_text(text.trim()) else {
            return Lookup::NotFound(self.nearest("", &visible));
        };
        if text.len() == ID_LEN {
            return match text.parse::<Id>() {
                Ok(id) if self.node_ix(id).is_some_and(&visible) => Lookup::Found(id),
                _ => Lookup::NotFound(self.nearest(&text, &visible)),
            };
        }
        if text.len() < MIN_PREFIX || text.len() > ID_LEN {
            return Lookup::NotFound(self.nearest(&text, &visible));
        }
        // Text order is id order, so a prefix is a contiguous id range.
        let pad = |c: char| format!("{text}{}", c.to_string().repeat(ID_LEN - text.len()));
        let (Ok(lo), Ok(hi)) = (pad('0').parse::<Id>(), pad('Z').parse::<Id>()) else {
            return Lookup::NotFound(self.nearest(&text, &visible));
        };
        let (a, b) = (
            self.ids.partition_point(|id| *id < lo),
            self.ids.partition_point(|id| *id <= hi),
        );
        let hits: Vec<Id> = self.ids[a..b]
            .iter()
            .enumerate()
            .filter(|(h, _)| visible(NodeIx((a + h) as u32)))
            .map(|(_, id)| *id)
            .collect();
        match hits.len() {
            0 => Lookup::NotFound(self.nearest(&text, &visible)),
            1 => Lookup::Found(hits[0]),
            _ => Lookup::Ambiguous(hits),
        }
    }

    fn nearest(&self, text: &str, visible: &impl Fn(NodeIx) -> bool) -> Vec<Id> {
        let shared = |id: &Id| {
            id.to_text()
                .chars()
                .zip(text.chars())
                .take_while(|(a, b)| a == b)
                .count()
        };
        let mut scored: Vec<(usize, Id)> = self
            .ids
            .iter()
            .enumerate()
            .filter(|(h, _)| visible(NodeIx(*h as u32)))
            .map(|(_, id)| (shared(id), *id))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored
            .into_iter()
            .filter(|(s, _)| *s > 0)
            .take(3)
            .map(|(_, id)| id)
            .collect()
    }

    /// Every node under `root` by part_of (children point at their parent), not including `root`: a walk over the
    /// incoming adjacency into a bitset.
    pub fn within(&self, root: Id) -> NodeSet<'_> {
        self.within_where(root, |_| true)
    }

    /// A subtree in a view; excluded nodes are not visited or crossed.
    pub fn within_where(&self, root: Id, visible: impl Fn(NodeIx) -> bool) -> NodeSet<'_> {
        let mut set = NodeSet::empty(&self.ids);
        let (Some(r), Some(part_of)) = (self.handle(root), self.names.get(PART_OF)) else {
            return set;
        };
        if !visible(NodeIx(r)) {
            return set;
        }
        let mut stack = vec![r];
        while let Some(parent) = stack.pop() {
            for e in self.inc.of(parent) {
                if e.other != r
                    && visible(NodeIx(e.other))
                    && self.e_kind.get(e.edge as usize) == part_of
                    && set.insert(e.other)
                {
                    stack.push(e.other);
                }
            }
        }
        set
    }

    /// The live nodes `keep` admits, as a set (D71 step 12: a spine range narrows search's legs and the query view).
    /// One pass over the handles.
    pub fn node_set_where(&self, keep: impl Fn(NodeIx) -> bool) -> NodeSet<'_> {
        let mut set = NodeSet::empty(&self.ids);
        for n in self.nodes() {
            if keep(n.ix()) {
                set.insert(n.ix().0);
            }
        }
        set
    }

    /// The number of a name the filter asks for: `Ok(None)` when not asked, `Err(())` when no record could match.
    fn wanted(&self, name: Option<&str>) -> Result<Option<u32>, ()> {
        name.map(|n| self.names.get(n).ok_or(())).transpose()
    }

    /// Nodes matching every filter, in id order. A `--where` on a custom field is driven by that field's column, so
    /// nodes without the field are never looked at; failing that, `evidence_kinds=<text>` is driven by the evidence
    /// kinds' inverted index.
    pub fn list_nodes(&self, filter: &NodeFilter) -> Vec<NodeRef<'_>> {
        let (Ok(kind), Ok(status)) = (
            self.wanted(filter.kind.as_deref()),
            self.wanted(filter.status.as_deref()),
        ) else {
            return Vec::new();
        };
        let within = filter.within.map(|root| self.within(root));
        let keep = |h: usize| {
            kind.is_none_or(|k| self.n_kind.get(h) == k)
                && status.is_none_or(|s| self.n_status.get(h) == s)
                && within.as_ref().is_none_or(|w| w.has(h as u32))
        };
        let Some(driver) = filter
            .wheres
            .iter()
            .position(|w| !view::is_standard(&w.field))
        else {
            if let Some(driver) = filter
                .wheres
                .iter()
                .position(|w| w.field == "evidence_kinds" && w.op == Op::Eq && w.value.is_string())
            {
                // A node matches when one of its kinds passes the where's own test, so the candidates are the rows
                // of every kind that passes it (a date-like text can equal another text).
                let w = &filter.wheres[driver];
                let lists: Vec<&[u32]> = self
                    .ev_kinds
                    .each()
                    .filter(|(k, _)| w.test(&umx_core::Value::String((*k).into())))
                    .map(|(_, rows)| rows)
                    .collect();
                let hs: alloc::borrow::Cow<'_, [u32]> = match lists[..] {
                    [] => return Vec::new(),
                    [rows] => rows.into(),
                    _ => {
                        let mut all = lists.concat();
                        all.sort_unstable();
                        all.dedup();
                        all.into()
                    }
                };
                return hs
                    .iter()
                    .filter(|&&h| keep(h as usize))
                    .map(|&h| NodeRef { g: self, h })
                    .filter(|n| {
                        filter
                            .wheres
                            .iter()
                            .enumerate()
                            .all(|(j, w)| j == driver || w.matches(*n))
                    })
                    .collect();
            }
            return (0..self.ids.len())
                .filter(|&h| keep(h))
                .map(|h| NodeRef {
                    g: self,
                    h: h as u32,
                })
                .filter(|n| filter.wheres.iter().all(|w| w.matches(*n)))
                .collect();
        };
        let w = &filter.wheres[driver];
        let Some(col) = self.names.get(&w.field).and_then(|f| self.f_cols.get(&f)) else {
            return Vec::new();
        };
        let mut hs = Vec::new();
        let mut rest = |h: u32, v: FieldVal| {
            if keep(h as usize) && w.test(&self.field_value(h as usize, &w.field, v)) {
                hs.push(h);
            }
        };
        match w.number() {
            Some(want) => {
                // The hot case (`--type problem --where likelihood>=3`): numbers against a number, a tight loop
                // with no branch per holder (a filter that matches at random mispredicts).
                let narrow = match (&self.n_kind, kind, &status, &within) {
                    // Names are shared by types, statuses, authors, relations and fields, so a wanted name's number
                    // can pass u16 while no node's type does: then no node is of that type (R1).
                    (NameCol::Narrow(v), Some(k), None, None) => match u16::try_from(k) {
                        Ok(k) => Some((&v[..], k)),
                        Err(_) => return Vec::new(),
                    },
                    _ => None,
                };
                let fits = |h: u32| match narrow {
                    Some((v, k)) => v[h as usize] == k,
                    None => keep(h as usize),
                };
                let nums = col.numbers().map(|(h, n)| (h, n.as_f64()));
                let mut picked = match w.op {
                    Op::Eq => pick(nums, fits, |x| x == want),
                    Op::Ne => pick(nums, fits, |x| x != want),
                    Op::Lt => pick(nums, fits, |x| x < want),
                    Op::Le => pick(nums, fits, |x| x <= want),
                    Op::Gt => pick(nums, fits, |x| x > want),
                    Op::Ge => pick(nums, fits, |x| x >= want),
                };
                col.each_but_numbers(&mut rest);
                picked.append(&mut hs);
                hs = picked;
            }
            None => col.each(&mut rest),
        }
        // Each kind's list is in handle order; only a field holding values of several kinds needs the sort.
        if !hs.is_sorted() {
            hs.sort_unstable();
        }
        hs.into_iter()
            .map(|h| NodeRef { g: self, h })
            .filter(|n| {
                filter
                    .wheres
                    .iter()
                    .enumerate()
                    .all(|(j, w)| j == driver || w.matches(*n))
            })
            .collect()
    }

    /// Edges matching every given end and type, in id order.
    pub fn list_edges(
        &self,
        from: Option<Id>,
        to: Option<Id>,
        kind: Option<&str>,
    ) -> Vec<EdgeRef<'_>> {
        let Ok(kind) = self.wanted(kind) else {
            return Vec::new();
        };
        let end = |id: Option<Id>| match id {
            None => Ok(None),
            Some(id) => self.handle(id).map(Some).ok_or(()),
        };
        let (Ok(from), Ok(to)) = (end(from), end(to)) else {
            return Vec::new();
        };
        let keep = |e: u32| {
            (self.e_from[e as usize] | self.e_to[e as usize]) & EXTERNAL == 0
                && to.is_none_or(|t| self.e_to[e as usize] == t)
                && kind.is_none_or(|k| self.e_kind.get(e as usize) == k)
        };
        let candidates: Box<dyn Iterator<Item = u32>> = match (from, to) {
            (Some(f), _) => Box::new(self.out.of(f).iter().map(|e| e.edge)),
            (None, Some(t)) => Box::new(self.inc.of(t).iter().map(|e| e.edge)),
            (None, None) => Box::new(0..self.e_ids.len() as u32),
        };
        candidates
            .filter(|&e| keep(e))
            .map(|h| EdgeRef { g: self, h })
            .collect()
    }

    /// Nodes whose title approximately matches `text` (typos, word order) or holds every word of it (ws004 s24),
    /// best first, then by id. A query of at most three words found on whole-title Dice alone needs each word close
    /// to a title word below [`find::FUZZY_SURE`] (ws004 s30, [`find_keeps`]).
    pub fn find(&self, text: &str, kind: Option<&str>, limit: usize) -> Vec<(NodeRef<'_>, f64)> {
        self.find_in(text, kind, limit, find::MIN_SCORE, Mode::Find, &[])
    }

    /// Title find including provenance, which is never in the word index; the same keep-rule (ws004 s30).
    pub fn find_with_provenance(
        &self,
        text: &str,
        kind: Option<&str>,
        limit: usize,
    ) -> Vec<(NodeRef<'_>, f64)> {
        let provenance: Vec<_> = self.provenance.handles().collect();
        self.find_in(text, kind, limit, find::MIN_SCORE, Mode::Find, &provenance)
    }

    /// The near-duplicate check (`node add`, `umx batch`): the best `limit` titles near `title`, best first then by
    /// id. With `kind`, a node of that type is near at `min_score` and a node of any other type only at
    /// `other_type_min` (a cross-type match needs near-identical wording, ws004 s30); without, every node is near at
    /// `min_score`. Whole-title Dice, never s24's containment lift, and kept only if the words the titles do not share
    /// are within a typo ([`near_duplicate_keeps`]). Candidates come from the bound at `min_score`, so pass
    /// `other_type_min >= min_score`.
    pub fn near_duplicates(
        &self,
        title: &str,
        kind: Option<&str>,
        min_score: f64,
        other_type_min: f64,
        limit: usize,
    ) -> Vec<(NodeRef<'_>, f64)> {
        debug_assert!(other_type_min >= min_score);
        // A type the graph does not hold yet: every node is of another type.
        let same = kind.map(|k| self.names.get(k));
        self.find_in(
            title,
            None,
            limit,
            min_score,
            Mode::Near {
                same,
                other_type_min,
            },
            &[],
        )
    }

    fn find_in(
        &self,
        text: &str,
        kind: Option<&str>,
        limit: usize,
        min_score: f64,
        mode: Mode,
        provenance: &[u32],
    ) -> Vec<(NodeRef<'_>, f64)> {
        let Ok(kind) = self.wanted(kind) else {
            return Vec::new();
        };
        let Some(q) = find::Query::new(text) else {
            return Vec::new();
        };
        let contain = matches!(mode, Mode::Find);
        let short = contain && q.words().count() <= find::SHORT_QUERY_WORDS;
        // Candidates from the word index (ws004 s15): every title whose score could reach `min_score`, by a bound;
        // with `contain`, also every title holding each query word (ws004 s24). Both ascending, merged once each.
        let bound = self.words.title_candidates(&q.0, min_score);
        let contained = if contain {
            self.words.title_containing(q.words())
        } else {
            Vec::new()
        };
        let (mut text_buf, mut buf, mut words) =
            (String::new(), Vec::new(), find::Words::default());
        let mut score = |h: u32, holds_all: bool, text_buf: &mut String| -> f64 {
            if !holds_all {
                return find::score_title(&q, self.title(h as usize), text_buf, &mut buf);
            }
            let t = self.words.title_grams(h);
            let d = if t == u16::MAX {
                find::score_title(&q, self.title(h as usize), text_buf, &mut buf)
            } else {
                find::contained_dice(q.0.len(), usize::from(t))
            };
            find::contained_score(d)
        };
        // ws004 s30's keep-rules, on a hit that already reached `min_score`: they only drop hits, so the bound stays
        // complete, and they run before the best `limit` are chosen.
        let mut keeps = |h: u32, s: f64, holds_all: bool, text_buf: &mut String| -> bool {
            let mut normalised = || {
                text_buf.clear();
                find::normalise_into(self.title(h as usize), text_buf);
            };
            match mode {
                Mode::Find => {
                    if holds_all || !short || s >= find::FUZZY_SURE {
                        return true;
                    }
                    words.covered(&q.1, self.title(h as usize))
                }
                Mode::Near {
                    same,
                    other_type_min,
                } => {
                    let same_type = same.is_none_or(|k| k == Some(self.n_kind.get(h as usize)));
                    if !same_type && s < other_type_min {
                        return false;
                    }
                    normalised();
                    words.leftovers_close(&q.1, text_buf)
                }
            }
        };
        let mut scored: Vec<(usize, f64)> =
            Vec::with_capacity(bound.len() + contained.len() + provenance.len());
        let (mut i, mut j) = (0, 0);
        loop {
            let (h, holds_all) = match (bound.get(i), contained.get(j)) {
                (Some(&a), Some(&b)) if a == b => {
                    i += 1;
                    j += 1;
                    (a, true)
                }
                (Some(&a), Some(&b)) if a < b => {
                    i += 1;
                    (a, false)
                }
                (Some(&a), None) => {
                    i += 1;
                    (a, false)
                }
                (_, Some(&b)) => {
                    j += 1;
                    (b, true)
                }
                (None, None) => break,
            };
            if kind.is_some_and(|k| self.n_kind.get(h as usize) != k) {
                continue;
            }
            let s = score(h, holds_all, &mut text_buf);
            if s >= min_score && keeps(h, s, holds_all, &mut text_buf) {
                scored.push((h as usize, s));
            }
        }
        for &h in provenance {
            debug_assert!(bound.binary_search(&h).is_err() && contained.binary_search(&h).is_err());
            if kind.is_some_and(|k| self.n_kind.get(h as usize) != k) {
                continue;
            }
            let title = self.title(h as usize);
            let d = find::score_title(&q, title, &mut text_buf, &mut buf);
            text_buf.clear();
            find::normalise_into(title, &mut text_buf);
            let holds_all = q.words().all(|word| {
                text_buf.split(' ').filter(|other| *other == word).count()
                    >= q.words().filter(|other| *other == word).count()
            });
            let s = if holds_all {
                find::contained_score(d)
            } else {
                d
            };
            if s >= min_score && keeps(h, s, holds_all, &mut text_buf) {
                scored.push((h as usize, s));
            }
        }
        // Handle order is id order. A word in most titles makes most of them candidates (ws004 s24), so the best
        // `limit` are selected before sorting; the order is total, so they are the ones a full sort keeps.
        let order = |a: &(usize, f64), b: &(usize, f64)| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0));
        if limit < scored.len() {
            if limit > 0 {
                scored.select_nth_unstable_by(limit - 1, order);
            }
            scored.truncate(limit);
        }
        scored.sort_by(order);
        scored
            .into_iter()
            .map(|(h, s)| {
                (
                    NodeRef {
                        g: self,
                        h: h as u32,
                    },
                    s,
                )
            })
            .collect()
    }

    /// The bytes of nodes.jsonl for the graph as it is now, kept lines included: the image itself, borrowed, so a
    /// write never copies it (review D2).
    pub fn nodes_image(&self) -> &[u8] {
        debug_assert!(
            self.batch_placeholders.is_empty(),
            "unfilled batch placeholders"
        );
        assert_eq!(self.image_dead, 0, "compact the batch before encoding");
        &self.image
    }

    /// [`Graph::nodes_image`] as an owned copy.
    pub fn encode_nodes(&self) -> Vec<u8> {
        self.nodes_image().to_vec()
    }

    /// The bytes of edges.jsonl for the graph as it is now, kept lines included.
    pub fn encode_edges(&self) -> Vec<u8> {
        encode::lines(&self.e_ids, &self.kept_edges, 190, |e, out| {
            encode::edge_line(self, e, out)
        })
    }

    /// Heap bytes held, by part, counted from the columns' capacities (req-54).
    pub fn memory_bytes(&self) -> MemoryBytes {
        let edge_fields = cols::btree_bytes::<Id, Fields>(self.e_fields.len())
            + self
                .e_fields
                .values()
                .map(|f| cols::fields_bytes(f))
                .sum::<usize>()
            + cols::btree_bytes::<Id, Box<[u8]>>(self.e_evidence.len())
            + self.e_evidence.values().map(|b| b.len()).sum::<usize>();
        let field_columns = cols::btree_bytes::<u32, FieldCol>(self.f_cols.len())
            + self
                .f_cols
                .values()
                .map(FieldCol::heap_bytes)
                .sum::<usize>();
        MemoryBytes {
            node_columns: self.names.heap_bytes()
                + self.ids.capacity() * size_of::<Id>()
                + self.n_kind.heap_bytes()
                + self.n_status.heap_bytes()
                + self.n_by.heap_bytes()
                + self.n_created.column_bytes()
                + self.n_updated.column_bytes()
                + self.titles.capacity()
                + (self.title_end.capacity() + self.line_at.capacity() + self.nf_end.capacity())
                    * size_of::<u32>()
                + self.nf.heap_bytes()
                + cols::btree_bytes::<u32, nodes::Moved>(self.moved.len()),
            edge_columns: self.e_ids.capacity() * size_of::<Id>()
                + self.e_kind.heap_bytes()
                + self.e_by.heap_bytes()
                + (self.e_from.capacity() + self.e_to.capacity()) * size_of::<u32>()
                + self.e_created.column_bytes(),
            adjacency: self.out.heap_bytes() + self.inc.heap_bytes(),
            field_columns,
            image: self.image.capacity(),
            cold: edge_fields
                + self.n_created.exception_bytes()
                + self.n_updated.exception_bytes()
                + self.e_created.exception_bytes()
                + self.kept_nodes.capacity() * size_of::<(Id, u32)>()
                + size_of_val(&self.kept_edges[..])
                + self
                    .kept_edges
                    .iter()
                    .map(|l| l.line.capacity())
                    .sum::<usize>()
                + self.undo.as_ref().map_or(0, |u| u.1.capacity()),
            word_index: self.words.heap_bytes() + self.text_fields.heap_bytes(),
            vectors: self.vectors.heap_bytes(),
            similarity: self.similar.heap_bytes(),
            external: self.ext.capacity() * size_of::<(Id, Id)>()
                + self.ext_out.heap_bytes()
                + self.ext_in.heap_bytes(),
            provenance: self.provenance.heap_bytes(),
            locators: self.locators.heap_bytes(),
            entity_keys: self.entities.heap_bytes(),
            evidence_kinds: self.ev_kinds.heap_bytes(),
            evidence_kinds_forward: self.ev_kinds.forward_bytes(),
            date_columns: self.f_cols.values().map(FieldCol::date_bytes).sum(),
        }
    }
}

/// What [`Graph::find_in`] answers for.
#[derive(Clone, Copy)]
enum Mode {
    /// `node find`: s24's containment and s30's short-query word rule.
    Find,
    /// The near-duplicate check: `same` is the new node's stored type (`Some(None)` when the graph holds no such
    /// type yet), whose nodes are near at the call's threshold and every other node at `other_type_min`.
    Near {
        same: Option<Option<u32>>,
        other_type_min: f64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub fn node(id: u128, kind: &str, title: &str) -> Node {
        Node {
            id: Id(id),
            kind: kind.into(),
            title: title.into(),
            status: "open".into(),
            note: String::new(),
            evidence: vec![],
            status_history: vec![],
            created: String::new(),
            updated: String::new(),
            created_by: String::new(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields: Default::default(),
        }
    }

    /// F186 (cypher review): on a resource node, `locator`, `locator_of` and `locator_selector` have no value until
    /// S6, so they answer None without reading the record from its line; a part with a value still reads it.
    #[test]
    fn locator_names_with_no_value_never_read_the_record() {
        let mut brief = node(1, "document", "Brief");
        brief.locator = Some(umx_core::Locator::parse("docs/brief.md").unwrap());
        let g = Graph::from_records(vec![brief], Vec::new());
        let n = g.node(Id(1)).unwrap();
        crate::nodes::WATCHED.store(
            core::ptr::from_ref(&g) as usize,
            core::sync::atomic::Ordering::SeqCst,
        );
        let read = || crate::nodes::RECORDS_READ.load(core::sync::atomic::Ordering::SeqCst);
        let before = read();
        for name in ["locator", "locator_of", "locator_selector"] {
            assert_eq!(n.field(name), None, "{name}");
        }
        assert_eq!(
            read(),
            before,
            "a locator name with no value read the record"
        );
        assert_eq!(n.field("locator_source"), Some(json!("docs/brief.md")));
        assert_eq!(read(), before + 1, "a locator part with a value reads it");
    }

    /// F215: for every name in `LOCATOR_NAMES`, the stored read (`NodeRef::field`) answers what the record answers
    /// (`Node::field`), on a resource node with every part set, on one with only the parts parse gives, and on a
    /// plain node. A later step that adds a locator name, or answers one from a column or as null, cannot drift
    /// from the record without failing here.
    #[test]
    fn every_locator_name_reads_as_the_record_answers_it() {
        let mut full = node(1, "document", "Full");
        let mut locator = umx_core::Locator::parse("docs/full.md").unwrap();
        locator.hash = Some(alloc::format!("sha256:{}", "0".repeat(64)));
        locator.length = Some(120);
        locator.seq_max = Some(7);
        full.locator = Some(locator);
        let mut parsed = node(2, "document", "Parsed");
        parsed.locator = Some(umx_core::Locator::parse("https://example.com/a").unwrap());
        let plain = node(3, "problem", "Plain");
        let records = vec![full, parsed, plain];
        let g = Graph::from_records(records.clone(), Vec::new());
        for record in &records {
            let n = g.node(record.id).unwrap();
            for name in umx_core::locator::LOCATOR_NAMES {
                assert_eq!(n.field(name), record.field(name), "{} {name}", record.title);
            }
        }
    }

    /// Plan 079 (graph-storage-expert): census counts agree with the names' use counts after mixed writes, leave
    /// out names no record uses any more and the similar_to relation (not a similar_to type or status), and name
    /// the lowest id as the example.
    #[test]
    fn content_census_leaves_out_provenance_nodes_their_edges_and_examples() {
        let mut agent = node(1, "agent", "seat");
        agent
            .fields
            .insert("prov_key".into(), serde_json::json!("agent/pij/pij-seat"));
        let mut session = node(2, "session", "s");
        session.fields.insert(
            "prov_key".into(),
            serde_json::json!("session/pij/abcdefgh12"),
        );
        // A user node of a reserved type without a valid prov_key stays content.
        let lookalike = node(3, "agent", "not provenance");
        let edge = |id: u128, kind: &str, from: u128, to: u128| Edge {
            id: Id(id),
            kind: kind.into(),
            from_graph: None,
            from: Id(from),
            to_graph: None,
            to: Id(to),
            created: String::new(),
            created_by: String::new(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: Default::default(),
        };
        let g = Graph::from_records(
            vec![
                agent,
                session,
                lookalike,
                node(4, "risk", "r"),
                node(5, "risk", "r2"),
            ],
            vec![
                edge(10, "session_of", 2, 1),
                edge(11, "blocks", 4, 5),
                edge(12, "blocks", 3, 4),
                edge(13, "supports", 4, 2),
            ],
        );
        let shown = |ts: &[Tally<'_>]| -> Vec<(String, usize, u128)> {
            ts.iter()
                .map(|t| (t.name.to_string(), t.count, t.first.0))
                .collect()
        };
        let all = g.census();
        assert_eq!(
            shown(&all.types),
            [
                ("agent".into(), 2, 1),
                ("risk".into(), 2, 4),
                ("session".into(), 1, 2)
            ]
        );
        let c = g.content_census();
        assert_eq!(
            shown(&c.types),
            [("agent".into(), 1, 3), ("risk".into(), 2, 4)]
        );
        assert_eq!(shown(&c.statuses), [("open".into(), 3, 3)]);
        assert_eq!(shown(&c.relations), [("blocks".into(), 2, 11)]);
        assert_eq!(g.content_counts(), (3, 2));
        assert_eq!((g.node_count(), g.edge_count()), (5, 4));
    }

    #[test]
    fn census_counts_match_the_use_counts_after_mixed_writes() {
        let mut g = Graph::new();
        for (id, kind) in [(5, "risk"), (2, "problem"), (9, "problem"), (7, "gone")] {
            g.insert_node(node(id, kind, "t")).unwrap();
        }
        let mut parked = node(4, "problem", "p");
        parked.status = "parked".into();
        g.insert_node(parked).unwrap();
        let edge = |id: u128, kind: &str, from: u128, to: u128| Edge {
            id: Id(id),
            kind: kind.into(),
            from_graph: None,
            from: Id(from),
            to_graph: None,
            to: Id(to),
            created: String::new(),
            created_by: String::new(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: Default::default(),
        };
        g.insert_edge(edge(30, "blocks", 2, 5));
        g.insert_edge(edge(20, "blocks", 9, 5));
        g.insert_edge(edge(25, "supports", 4, 2));
        g.insert_edge(edge(26, SIMILAR_TO, 4, 9));
        g.remove_edge(Id(25));
        g.remove_node(Id(7));
        g.insert_node(node(9, "risk", "t")).unwrap();
        // Review B1: similar_to is reserved as a relation only, so a node type and status of that name are counted.
        let mut odd = node(11, SIMILAR_TO, "s");
        odd.status = SIMILAR_TO.into();
        g.insert_node(odd).unwrap();
        let c = g.census();
        let shown = |ts: &[Tally<'_>]| -> Vec<(String, usize, u128)> {
            ts.iter()
                .map(|t| (t.name.to_string(), t.count, t.first.0))
                .collect()
        };
        assert_eq!(
            shown(&c.types),
            [
                ("problem".into(), 2, 2),
                ("risk".into(), 2, 5),
                (SIMILAR_TO.into(), 1, 11)
            ]
        );
        assert_eq!(
            shown(&c.statuses),
            [
                ("open".into(), 3, 2),
                ("parked".into(), 1, 4),
                (SIMILAR_TO.into(), 1, 11)
            ]
        );
        assert_eq!(shown(&c.relations), [("blocks".into(), 2, 20)]);
        // Node 11 uses similar_to as both its type and its status, so its use count is 2 there.
        for t in c.types.iter().filter(|t| t.name != SIMILAR_TO) {
            let n = g.names.get(t.name).unwrap();
            assert_eq!(g.names.uses(n).0 as usize, t.count, "{}", t.name);
        }
        for t in &c.relations {
            let n = g.names.get(t.name).unwrap();
            assert_eq!(g.names.uses(n).1 as usize, t.count, "{}", t.name);
        }
        let used = |n: u32| {
            g.names.used_by_node(n) && g.names.text(n) != "open" && g.names.text(n) != "parked"
        };
        assert_eq!(
            g.names.in_name_order().filter(|&n| used(n)).count(),
            c.types.len()
        );
    }

    /// Security: a node write that would take the image past its limit is a refusal, never a panic, and leaves
    /// the graph as it was: an append, a replace that grows, an add below the last id, and a kept line.
    #[test]
    fn a_node_write_past_the_image_limit_is_refused_and_changes_nothing() {
        let mut g = Graph::new();
        g.insert_node(node(1, "problem", "a")).unwrap();
        g.insert_node(node(3, "problem", "c")).unwrap();
        let image = g.encode_nodes();
        let limit = image.len() + 8;
        g.limit = Some(limit);
        let long = "x".repeat(20);
        let refusals = [
            g.insert_node(node(4, "problem", "d")),
            g.insert_node(node(3, "problem", &long)),
            g.insert_node(node(2, "problem", "b")),
        ];
        for refused in refusals {
            let err = refused.unwrap_err();
            assert_eq!(err.limit, limit as u64);
            assert!(err.size > err.limit, "{err:?}");
        }
        let kept = KeptLine {
            id: Id(3),
            line: br#"{"id":"old"}"#.repeat(2).to_vec(),
        };
        assert!(g.keep_lines(vec![kept], Vec::new()).is_err());
        assert_eq!(g.encode_nodes(), image);
        assert_eq!(g.node_count(), 2);
        assert_eq!(g.kept_line_count(), 0);
        g.check_rows();
        // Up to the limit is fine, and a replace that shrinks always is.
        g.insert_node(node(3, "problem", "c-plus-8b")).unwrap();
        assert_eq!(g.encode_nodes().len(), limit);
        g.insert_node(node(3, "problem", "")).unwrap();
    }

    fn edge(id: u128, from: u128, kind: &str, to: u128) -> Edge {
        Edge {
            id: Id(id),
            kind: kind.into(),
            from_graph: None,
            from: Id(from),
            to_graph: None,
            to: Id(to),
            created: String::new(),
            created_by: String::new(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: Default::default(),
        }
    }

    fn sample() -> Graph {
        let mut risk = node(3, "risk", "Launch slips");
        risk.fields.insert("likelihood".into(), json!(3));
        Graph::from_records(
            vec![
                node(1, "problem", "Approval delays"),
                node(2, "decision", "Launch date"),
                risk,
            ],
            vec![
                edge(10, 1, "blocks", 2),
                edge(11, 3, "part_of", 1),
                edge(12, 2, "part_of", 3),
            ],
        )
    }

    #[test]
    fn edges_are_indexed_both_ways_and_unindexed_on_remove() {
        let mut g = sample();
        assert_eq!(g.list_edges(Some(Id(1)), None, None).len(), 1);
        assert_eq!(
            g.list_edges(None, Some(Id(2)), Some("blocks"))[0].id(),
            Id(10)
        );
        assert_eq!(
            g.find_edge(Id(1), "blocks", Id(2)).map(|e| e.id()),
            Some(Id(10))
        );
        g.remove_edge(Id(10));
        assert!(g.list_edges(None, Some(Id(2)), Some("blocks")).is_empty());
        assert_eq!(g.edge_count(), 2);
    }

    #[test]
    fn within_walks_part_of_down_the_hierarchy() {
        let g = sample();
        assert_eq!(
            g.within(Id(1)).iter().collect::<Vec<_>>(),
            vec![Id(2), Id(3)]
        );
        assert!(g.within(Id(2)).is_empty());
    }

    #[test]
    fn lookup_by_id_and_prefix() {
        let mut g = Graph::new();
        let a = Id(0x0194_0000_0000_0000_0000_0000_0000_0001);
        let b = Id(0x0194_0000_0000_0000_0000_0000_0000_0002);
        let c = Id(0x0195_0000_0000_0000_0000_0000_0000_0001);
        for id in [a, b, c] {
            g.insert_node(node(id.0, "problem", "x")).unwrap();
        }
        assert_eq!(g.lookup(&a.to_text()), Lookup::Found(a));
        assert_eq!(g.lookup(&a.to_text().to_lowercase()), Lookup::Found(a));
        assert_eq!(g.lookup(&c.to_text()[..8]), Lookup::Found(c));
        assert_eq!(g.lookup(&a.to_text()[..8]), Lookup::Ambiguous(vec![a, b]));
        assert!(matches!(g.lookup(&a.to_text()[..5]), Lookup::NotFound(_)));
        assert!(matches!(g.lookup("not-an-id!"), Lookup::NotFound(_)));
        let Lookup::NotFound(near) = g.lookup("06A0000000") else {
            panic!()
        };
        assert_eq!(near.len(), 3);
    }

    #[test]
    fn list_filters_and_where() {
        let g = sample();
        let by_type = NodeFilter {
            kind: Some("risk".into()),
            ..Default::default()
        };
        assert_eq!(g.list_nodes(&by_type)[0].id(), Id(3));
        let w = |s: &str, v| {
            let (f, op, _) = parse_where(s).unwrap();
            NodeFilter {
                wheres: vec![Where {
                    field: f,
                    op,
                    value: v,
                }],
                ..Default::default()
            }
        };
        assert_eq!(g.list_nodes(&w("likelihood>=3", json!(3))).len(), 1);
        assert_eq!(g.list_nodes(&w("likelihood>3", json!(3))).len(), 0);
        // A node without the field never matches, != included.
        assert_eq!(g.list_nodes(&w("likelihood!=4", json!(4))).len(), 1);
        assert_eq!(
            g.list_nodes(&w("title=Launch date", json!("Launch date")))
                .len(),
            1
        );
        let within = NodeFilter {
            within: Some(Id(1)),
            kind: Some("decision".into()),
            ..Default::default()
        };
        assert_eq!(g.list_nodes(&within)[0].id(), Id(2));
    }

    #[test]
    fn find_tolerates_typos_and_word_order() {
        let g = sample();
        let hits = g.find("aproval delays", None, 10);
        assert_eq!(hits[0].0.title(), "Approval delays");
        assert!(hits[0].1 >= 0.85, "{}", hits[0].1);
        assert_eq!(g.find("delays approval", None, 10)[0].1, 1.0);
        assert!(title_score("Approval delay", "Approval delays") >= 0.85);
        assert!(title_score("Launch date", "Approval delays") < 0.5);
        assert!(g.find("zzzz qqqq", None, 10).is_empty());
        assert!(
            g.find("approval delays", Some("decision"), 10)
                .iter()
                .all(|(n, _)| n.kind() == "decision")
        );
    }
}
