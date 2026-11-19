//! Vector shards of a loaded graph (workshop 008 section 2.5): read, list and write them through the files port.
//! The graph store stays the one write path to a graph folder (d-7); the embedding service calls these.

use super::{GraphStore, Loaded, alias_name, problems};
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use umx_core::codec::GraphFile;
use umx_core::ports::{
    Change, FileRef, FilesError, GraphDir, GraphFiles, GraphStamps, ModelKey, Written,
};
use umx_core::vectors::{self, ShardFile, ShardLine, TextHash, VectorLine};
use umx_core::{GraphEmbedding, Id, Policy, Problem};

impl GraphStore {
    /// Sets which nodes of `alias` have pending vectors (None: the graph has no model space).
    pub fn set_vectors_pending(&mut self, alias: &str, pending: Option<BTreeSet<Id>>) {
        if let Some(l) = self.graphs.get_mut(alias) {
            l.vectors_pending = pending;
        }
    }

    /// Sets which pending nodes of `alias` are outdated (see [`Loaded::vectors_outdated`]), from a scan.
    pub fn set_vectors_outdated(&mut self, alias: &str, outdated: BTreeSet<Id>) {
        if let Some(l) = self.graphs.get_mut(alias) {
            l.vectors_outdated = outdated;
            l.settle_text_form();
        }
    }

    /// Marks nodes of `alias` as holding a held text or not (see [`super::Loaded::vectors_held`]); `clear` empties the
    /// set first (a whole recount).
    pub fn mark_vectors_held(&mut self, alias: &str, clear: bool, nodes: &[(Id, bool)]) {
        let Some(l) = self.graphs.get_mut(alias) else {
            return;
        };
        if clear {
            l.vectors_held.clear();
        }
        for (id, held) in nodes {
            if *held {
                l.vectors_held.insert(*id);
            } else {
                l.vectors_held.remove(id);
            }
        }
    }

    /// Marks nodes of `alias` pending or not; no change when the graph has no model space.
    pub fn mark_vectors_pending(&mut self, alias: &str, nodes: &[(Id, bool)]) {
        let Some(set) = self
            .graphs
            .get_mut(alias)
            .and_then(|l| l.vectors_pending.as_mut())
        else {
            return;
        };
        for (id, pending) in nodes {
            if *pending {
                set.insert(*id);
            } else {
                set.remove(id);
            }
        }
        let l = self.graphs.get_mut(alias).expect("found above");
        for (id, pending) in nodes {
            if !*pending {
                l.vectors_outdated.remove(id);
            }
        }
        l.settle_text_form();
    }

    /// Records `key` as the model of the graph's vectors in graph.json (`umx embeddings reembed`, explicit only;
    /// D32). The policy is kept as it is. A failed write leaves graph.json and memory as they were.
    pub fn set_embedding_key(
        &mut self,
        alias: &str,
        key: ModelKey,
    ) -> Result<Vec<Problem>, Problem> {
        let files = Arc::clone(&self.ports.files);
        if !self.graphs.contains_key(alias) {
            return Err(self.not_loaded(alias));
        }
        let l = self.graphs.get_mut(alias).expect("checked above");
        let before = l.meta.clone();
        match &mut l.meta.embedding {
            Some(e) => e.key = Some(key),
            None => l.meta.embedding = Some(GraphEmbedding::new(Some(key), Policy::OPEN)),
        }
        // A reembed remakes every vector the form changed (req-97), so it records the current form with the key.
        if let Some(e) = l.meta.embedding.as_mut() {
            e.text_form = e.text_form.max(vectors::WHOLE_NODE_VERSION);
        }
        super::persist(
            files.as_ref(),
            alias,
            l,
            &[GraphFile::Graph],
            umx_core::ui::Touched::default(),
        )
        .inspect_err(|_| {
            l.meta = before;
        })
    }

    /// Records the current whole-node text form in graph.json (`umx embeddings reembed` with the same key, req-97):
    /// the reembed queues every node it changes. No section, or already current: nothing is written. A failed write
    /// leaves graph.json and memory as they were.
    pub fn set_text_form(&mut self, alias: &str) -> Result<Vec<Problem>, Problem> {
        let files = Arc::clone(&self.ports.files);
        let l = self
            .graphs
            .get_mut(alias)
            .ok_or_else(|| problems::graph_not_loaded(alias))?;
        let Some(e) = l.meta.embedding.as_ref() else {
            return Ok(Vec::new());
        };
        if e.text_form >= vectors::WHOLE_NODE_VERSION {
            return Ok(Vec::new());
        }
        let before = l.meta.clone();
        if let Some(e) = l.meta.embedding.as_mut() {
            e.text_form = vectors::WHOLE_NODE_VERSION;
        }
        super::persist(
            files.as_ref(),
            alias,
            l,
            &[GraphFile::Graph],
            umx_core::ui::Touched::default(),
        )
        .inspect_err(|_| {
            l.meta = before;
        })
    }

    /// The files port and folder a graph's shards live in: its own folder, or for a secured graph the local
    /// cache at `<cache root>/<graph id>/<folder key>`, keyed per copy (graph-storage-expert: two worktree copies
    /// must not overwrite each other's lines). The id is a typed `Id`, so its text is 26 Crockford characters;
    /// the folder key is 16 hex digits of the SHA-256 of the resolved folder. Neither can leave the cache root.
    /// `policy` is the graph's effective policy, read once by the caller for its whole operation (architect).
    pub(super) fn shard_home(
        &self,
        loaded: &Loaded,
        policy: Policy,
    ) -> Result<(Arc<dyn GraphFiles>, GraphDir), Problem> {
        if policy == Policy::OPEN {
            return Ok((Arc::clone(&self.ports.files), loaded.dir.clone()));
        }
        let id = loaded.meta.id.to_text();
        let key = TextHash::of(loaded.dir.as_str()).to_string();
        let dir = self
            .ports
            .vector_cache_root
            .join(&format!("{id}/{}", &key[..16]))
            .map_err(|_| {
                problems::shard_read_failed(&FilesError::Invalid("vector cache folder"))
            })?;
        Ok((Arc::clone(&self.ports.vector_cache), dir))
    }

    /// Runs `write`, a set of `alias`'s shards in `home`, checked against the graph's content files (F144). In the
    /// graph's own folder `write` gets the load's stamps, which the files port checks at the commit. In the cache
    /// (a secured graph) the set holds none of the graph's files, so `write` gets no stamps (expect None, F144,
    /// architect): the store's looks at the graph's folder before the set and again after it are the guard, and
    /// either finding a change refuses with `graph_files_changed`, naming the graph's files only. The cache set may
    /// then stand, but nothing it carries is kept in memory; and a vector line is keyed by its text's hash, so after
    /// a load one made from text the graph no longer holds is not current: the node is pending until it is embedded
    /// again (meanwhile req-75's search may still use the stale vector, as for any edit).
    pub(super) fn write_checked<T>(
        &self,
        alias: &str,
        loaded: &Loaded,
        home: &GraphDir,
        write: impl FnOnce(Option<&GraphStamps>) -> Result<T, Problem>,
    ) -> Result<T, Problem> {
        if *home == loaded.dir {
            return write(Some(&loaded.stamps));
        }
        self.files_unchanged(alias)?;
        let out = write(None)?;
        self.files_unchanged(alias)?;
        Ok(out)
    }

    /// Whether a loaded node is content that may be embedded. Provenance never supplies document text.
    pub fn embeddable_node(&self, alias: &str, id: Id) -> bool {
        self.graphs.get(alias).is_some_and(|l| {
            l.graph
                .node_ix(id)
                .is_some_and(|ix| !l.graph.is_provenance(ix))
        })
    }

    /// The embedded texts of a content node (field name, text).
    pub fn node_texts(&self, alias: &str, id: Id) -> Option<Vec<(String, String)>> {
        let loaded = self.graphs.get(alias)?;
        let node = loaded.graph.node(id)?;
        if loaded.graph.is_provenance(node.ix()) {
            return None;
        }
        Some(vectors::node_texts(&node.to_node(), loaded.ontology()))
    }

    /// Every node id of a loaded graph, in id order.
    pub fn node_ids(&self, alias: &str) -> Vec<Id> {
        self.graphs
            .get(alias)
            .map(|l| {
                l.graph
                    .nodes()
                    .filter(|n| !l.graph.is_provenance(n.ix()))
                    .map(|n| n.id())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The shard files of one vector space in a loaded graph.
    pub fn list_shards(
        &self,
        alias: &str,
        policy: Policy,
        key: &ModelKey,
    ) -> Result<Vec<ShardFile>, Problem> {
        let Some(loaded) = self.graphs.get(alias) else {
            return Ok(Vec::new());
        };
        let (files, dir) = self.shard_home(loaded, policy)?;
        files
            .list_shards(&dir, key, vectors::ShardKind::Vectors)
            .map_err(|e| problems::shard_read_failed(&e))
    }

    /// The vector spaces a loaded graph's folder holds shards for.
    pub fn list_vector_spaces(
        &self,
        alias: &str,
        policy: Policy,
    ) -> Result<Vec<ModelKey>, Problem> {
        let Some(loaded) = self.graphs.get(alias) else {
            return Ok(Vec::new());
        };
        let (files, dir) = self.shard_home(loaded, policy)?;
        files
            .list_vector_spaces(&dir)
            .map_err(|e| problems::shard_read_failed(&e))
    }

    /// Reads every shard of every vector space of `alias` once, one shard at a time, handing each shard's lines (with
    /// their 1-based line numbers) to `see` with read access to the graph. The lines of `blocks` (the one space the
    /// graph holds in memory) that have its width also go into the graph's vector blocks, replacing what that space
    /// held (workshop 008 section 2.5, ws004 s13). Only one shard's lines are alive at a time, so a load holds little
    /// beside the blocks (acceptance section 2: at most 10%). If a shard cannot be read the space is left empty,
    /// never half loaded.
    pub fn read_vectors(
        &mut self,
        alias: &str,
        policy: Policy,
        blocks: Option<&ModelKey>,
        mut see: impl FnMut(&ModelKey, &ShardFile, Vec<(usize, ShardLine)>, ShardReading<'_>),
    ) -> Result<(), Problem> {
        let Some(loaded) = self.graphs.get(alias) else {
            return Ok(());
        };
        let (files, dir) = self.shard_home(loaded, policy)?;
        let failed = |e: FilesError| problems::shard_read_failed(&e);
        let spaces = files.list_vector_spaces(&dir).map_err(failed)?;
        let whole = vectors::whole_node_field();
        let Loaded {
            graph, ontology, ..
        } = self
            .graphs
            .get_mut(alias)
            .expect("the graph was loaded above");
        let mut access = match blocks {
            Some(key) => Access::Load(graph.load_vectors(&key.to_string(), key.dims())),
            None => Access::Read(graph),
        };
        let mut read = || -> Result<(), Problem> {
            for space in &spaces {
                let into = blocks == Some(space);
                for shard in files
                    .list_shards(&dir, space, vectors::ShardKind::Vectors)
                    .map_err(failed)?
                {
                    let bytes = files
                        .read_ref(&dir, &FileRef::Shard(shard.clone()))
                        .map_err(failed)?;
                    let lines = bytes
                        .map(|b| vectors::decode_shard_numbered(&b))
                        .unwrap_or_default();
                    if let (true, Access::Load(load)) = (into, &mut access) {
                        let width = |v: &&VectorLine| v.values.len() == space.dims();
                        let vectors: Vec<&VectorLine> = lines
                            .iter()
                            .filter_map(|(_, l)| l.vector())
                            .filter(width)
                            .collect();
                        for v in whole_preferred(&vectors, &whole) {
                            if let Some(field) = block_field(&v.field, &whole) {
                                load.put(v.node, field, &v.values, v.hash);
                            }
                        }
                    }
                    let reading = ShardReading {
                        graph: access.graph(),
                        ontology,
                    };
                    see(space, &shard, lines, reading);
                }
            }
            Ok(())
        };
        let result = read();
        match (access, &result) {
            (Access::Load(load), Ok(())) => drop(load.finish()),
            (Access::Load(load), Err(_)) => load.abandon(),
            (Access::Read(_), _) => {}
        }
        result
    }

    /// After `apply` wrote a shard (or found it unchanged): the graph's blocks take the shard's lines for `nodes`, and
    /// drop the vectors of fields those nodes had a line for `before` but no longer have. Blocks change only here and
    /// in `read_vectors`, under the writer. Returns the nodes whose whole-node vector changed (for similar_to).
    pub fn set_block_vectors(
        &mut self,
        alias: &str,
        key: &ModelKey,
        nodes: &[Id],
        before: &[ShardLine],
        after: &[ShardLine],
    ) -> Vec<Id> {
        let Some(loaded) = self.graphs.get_mut(alias) else {
            return Vec::new();
        };
        let (space, whole) = (key.to_string(), vectors::whole_node_field());
        let mut whole_changed = Vec::new();
        for &node in nodes {
            let whole_of = |lines| {
                block_lines(lines, node, key, &whole)
                    .into_iter()
                    .find(|(f, _)| f.is_none())
                    .map(|(_, v)| (v.hash, v.values.clone()))
            };
            if whole_of(before) != whole_of(after) {
                whole_changed.push(node);
            }
            let now = block_lines(after, node, key, &whole);
            for (field, _) in block_lines(before, node, key, &whole) {
                if !now.iter().any(|(f, _)| *f == field) {
                    loaded.graph.remove_vector(&space, node, field);
                }
            }
            for (field, v) in now {
                loaded
                    .graph
                    .set_vector(&space, node, field, &v.values, v.hash);
            }
        }
        whole_changed
    }

    /// A shard's lines with their physical line numbers; empty if the shard does not exist.
    pub fn read_shard_numbered(
        &self,
        alias: &str,
        policy: Policy,
        shard: &ShardFile,
    ) -> Result<Vec<(usize, ShardLine)>, Problem> {
        let Some(loaded) = self.graphs.get(alias) else {
            return Ok(Vec::new());
        };
        let (files, dir) = self.shard_home(loaded, policy)?;
        let bytes = files
            .read_ref(&dir, &FileRef::Shard(shard.clone()))
            .map_err(|e| problems::files_error(&e, problems::shard_read_failed))?;
        Ok(bytes
            .map(|b| vectors::decode_shard_numbered(&b))
            .unwrap_or_default())
    }

    /// A shard's lines in file order; empty if the shard does not exist.
    pub fn read_shard(
        &self,
        alias: &str,
        policy: Policy,
        shard: &ShardFile,
    ) -> Result<Vec<ShardLine>, Problem> {
        let Some(loaded) = self.graphs.get(alias) else {
            return Ok(Vec::new());
        };
        let (files, dir) = self.shard_home(loaded, policy)?;
        let bytes = files
            .read_ref(&dir, &FileRef::Shard(shard.clone()))
            .map_err(|e| problems::files_error(&e, problems::shard_read_failed))?;
        Ok(bytes.map(|b| vectors::decode_shard(&b)).unwrap_or_default())
    }

    /// Writes a shard whole and atomically (a one-change set through the files port, so it shares the graph
    /// files' recovery) if its bytes change; returns whether it wrote, and a warning if the write stands but was
    /// not synced. A failed write changed nothing in memory. The vectors were made from the graph in memory, so the
    /// write expects its content files as the load left them (F144): in the graph's folder the files port checks
    /// them at the commit; in the cache (a secured graph) the store looks before and after
    /// ([`Self::write_checked`]). On `graph_files_changed` the caller drops the vectors and does not retry.
    pub fn write_shard(
        &mut self,
        alias: &str,
        policy: Policy,
        shard: &ShardFile,
        lines: &[ShardLine],
        before: &[ShardLine],
    ) -> Result<(bool, Vec<Problem>), Problem> {
        let Some(loaded) = self.graphs.get(alias) else {
            return Ok((false, Vec::new()));
        };
        let bytes = vectors::encode_shard(lines);
        if bytes == vectors::encode_shard(before) {
            return Ok((false, Vec::new()));
        }
        let change = Change {
            file: FileRef::Shard(shard.clone()),
            bytes: Some(&bytes),
        };
        let (files, dir) = self.shard_home(loaded, policy)?;
        // F144, architect: in the cache `expect` is None, since the set holds none of the graph's files; the looks
        // at the graph's folder before and after the set (write_checked) are the guard.
        let written =
            self.write_checked(alias, loaded, &dir, |expect| {
                match files.write_set_checked(&dir, &[change], expect) {
                    Ok(c) => Ok(c.written),
                    Err(FilesError::RecoveryConflict(conflicts)) => Err(
                        problems::recovery_conflict(&dir, &conflicts, Some(&alias_name(alias))),
                    ),
                    Err(FilesError::Changed(changed)) => Err(problems::graph_files_changed(
                        &alias_name(alias),
                        &loaded.dir,
                        &changed,
                    )),
                    Err(e) => Err(problems::files_error(&e, |e| {
                        problems::shard_write_failed(shard, e)
                    })),
                }
            })?;
        match written {
            Written::Durable => Ok((true, Vec::new())),
            Written::NotSynced(e) => {
                Ok((true, alloc::vec![problems::shard_not_durable(shard, &e)]))
            }
        }
    }
}

/// Read access to a loaded graph while its shards are read: is a node there, and what are its embedded texts.
#[derive(Clone, Copy)]
pub struct ShardReading<'a> {
    graph: &'a umx_graph::Graph,
    ontology: &'a umx_core::Ontology,
}

impl ShardReading<'_> {
    pub fn has_node(&self, id: Id) -> bool {
        self.graph.node_ix(id).is_some()
    }

    /// A node's handle index (below the graph's node bound), for a bit per node.
    pub fn node_index(&self, id: Id) -> Option<usize> {
        self.graph.node_ix(id).map(|ix| ix.index())
    }

    /// The embedded texts of a node (field name, text), or None if it is not in the graph.
    pub fn node_texts(&self, id: Id) -> Option<Vec<(String, String)>> {
        let node = self.graph.node(id)?;
        if self.graph.is_provenance(node.ix()) {
            return None;
        }
        Some(vectors::node_texts(&node.to_node(), self.ontology))
    }

    /// The hashes of the whole-node texts a node had before a field became data: its form v1 text
    /// ([`vectors::whole_text_v1`]), its text under the role default before search-04
    /// ([`vectors::whole_text_before_data_default`]) and each text with some of its now-data fields put back
    /// ([`vectors::whole_texts_with_data`]), to tell outdated vectors from edited ones.
    pub fn whole_former(&self, id: Id) -> Vec<TextHash> {
        let Some(node) = self.graph.node(id) else {
            return Vec::new();
        };
        let node = node.to_node();
        let mut out: Vec<TextHash> = vectors::whole_text_v1(&node, self.ontology)
            .into_iter()
            .chain(vectors::whole_text_before_data_default(
                &node,
                self.ontology,
            ))
            .chain(vectors::whole_texts_with_data(&node, self.ontology))
            .map(|t| TextHash::of(&t))
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// The graph while shards are read: loading one space into its blocks, or only read.
enum Access<'g> {
    Load(umx_graph::VectorLoad<'g>),
    Read(&'g umx_graph::Graph),
}

impl Access<'_> {
    fn graph(&self) -> &umx_graph::Graph {
        match self {
            Access::Load(load) => load.graph(),
            Access::Read(g) => g,
        }
    }
}

/// The block field of a shard line's field: `Some(None)` for the current whole-node vector, `Some(Some(field))` for
/// a field vector, `None` for an older whole-node version (a leftover that `apply` drops, never searched).
fn block_field<'a>(field: &'a str, whole: &str) -> Option<Option<&'a str>> {
    if field == whole || vectors::whole_node_version(field).is_some() {
        Some(None)
    } else if field.starts_with('@') {
        None
    } else {
        Some(Some(field))
    }
}

/// `lines` with at most one whole-node line per node: the current version's, else the first older one, which is
/// still that node's vector until reembed (req-97, embeddings-expert). Other lines are kept, in order.
fn whole_preferred<'a>(lines: &[&'a VectorLine], whole: &str) -> Vec<&'a VectorLine> {
    let current: BTreeSet<Id> = lines
        .iter()
        .filter(|v| v.field == whole)
        .map(|v| v.node)
        .collect();
    let mut taken: BTreeSet<Id> = BTreeSet::new();
    lines
        .iter()
        .copied()
        .filter(|v| match vectors::whole_node_version(&v.field) {
            None => true,
            Some(_) if v.field == whole => true,
            Some(_) => !current.contains(&v.node) && taken.insert(v.node),
        })
        .collect()
}

/// The lines of `node` in `lines` that go into blocks, with their block field: the right width, and one whole-node
/// line ([`whole_preferred`]).
fn block_lines<'a>(
    lines: &'a [ShardLine],
    node: Id,
    key: &ModelKey,
    whole: &str,
) -> Vec<(Option<&'a str>, &'a VectorLine)> {
    let mine: Vec<&VectorLine> = lines
        .iter()
        .filter_map(ShardLine::vector)
        .filter(|v| v.node == node && v.values.len() == key.dims())
        .collect();
    whole_preferred(&mine, whole)
        .into_iter()
        .filter_map(|v| Some((block_field(&v.field, whole)?, v)))
        .collect()
}
