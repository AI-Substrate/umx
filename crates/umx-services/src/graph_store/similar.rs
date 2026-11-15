//! Stored similar_to lists of a loaded graph (workshop 008 section 9; ws004 s09): read them at load when
//! `_meta.tsv` says they match the graph's vectors and settings, keep them current after each batch of applied
//! vectors, and write the shards and `_meta.tsv` they change in one set. A secured graph's files go to the vector
//! cache, like its shards. Only ever this graph's own nodes: nothing between graphs is stored.

use super::{GraphStore, alias_name, problems};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::ports::{Change, FileRef, FilesError, GraphStamps, ModelKey, Written};
use umx_core::similar::{self, Meta, Settings, SimilarLine, SimilarShardLine};
use umx_core::vectors::{ShardFile, ShardKind, WHOLE_NODE_VERSION, shard_of};
use umx_core::{Id, Policy, Problem};
use umx_graph::{Graph, NodeIx, SimilarLists, SimilarSnapshot};

/// What a load found for a graph's stored lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimilarLoaded {
    /// `_meta.tsv` matches: the stored lists are in memory.
    Fresh,
    /// The files do not match the vectors or settings (or are missing, or unreadable): the lists must be rebuilt.
    Stale,
}

/// Step 1 of a similarity rebuild, taken under a short read lock: a copy of one load of a graph's whole-node block and
/// the settings its lists use. Opaque, so the daemon's threads carry it without naming a umx-graph type (architect F1).
pub struct SimilarityJob {
    instance: u64,
    space: String,
    snapshot: SimilarSnapshot,
    k: usize,
    floor: i32,
}

impl SimilarityJob {
    /// Step 2, with no lock held: every node's top k. Pure; O(n^2 x dims).
    pub fn compute(self) -> SimilarityResult {
        SimilarityResult {
            lists: self.snapshot.compute(self.k, self.floor),
            instance: self.instance,
            space: self.space,
            k: self.k,
            floor: self.floor,
        }
    }
}

/// Step 2's lists, for step 3 to apply under the writer; only ever to the load they were computed from.
pub struct SimilarityResult {
    instance: u64,
    space: String,
    k: usize,
    floor: i32,
    lists: SimilarLists,
}

/// [`similar::digest`] over `graph`'s whole-node vectors in `space` (provenance left out): what `_meta.tsv` records,
/// and what `umx embeddings status`' `vectors_digest` is made from (ws004 s34b). It reads the text hash each vector
/// line holds, never the node's current text, so an edit moves it only once its vector is applied.
fn whole_digest(graph: &Graph, space: &str) -> [u8; 32] {
    let block = graph.whole_block(space);
    similar::digest(
        graph
            .nodes()
            .filter(|n| !graph.is_provenance(n.ix()))
            .filter_map(|n| Some((n.id(), block?.hash(n.ix().index())?))),
    )
}

/// The `_meta.tsv` line for `graph`'s current whole-node vectors in `key` and `settings`, and the edges its lists hold.
fn expected_meta(graph: &Graph, key: &ModelKey, settings: Settings) -> Meta {
    let digest = whole_digest(graph, &key.to_string());
    Meta {
        key: key.clone(),
        k: settings.k,
        floor: settings.floor,
        text_form: WHOLE_NODE_VERSION,
        digest,
        edges: graph
            .nodes()
            .map(|n| graph.similar_out(n.ix()).len() as u64)
            .sum(),
    }
}

/// The lines of `node`'s list as the files write them.
fn lines_of(graph: &Graph, node: NodeIx) -> impl Iterator<Item = SimilarShardLine> + '_ {
    let id = graph.node_at(node).id();
    graph.similar_out(node).iter().map(move |e| {
        SimilarShardLine::Edge(SimilarLine {
            node: id,
            other: graph.node_at(e.other).id(),
            score: e.score,
        })
    })
}

impl GraphStore {
    /// `alias`'s load instance and vectors' generation (ws004 s34b): while both are the same its vectors are too
    /// (the generation only grows within one load; a load gets a new instance). None when it is not loaded.
    pub fn vectors_stamp(&self, alias: &str) -> Option<(u64, u64)> {
        let loaded = self.graphs.get(alias)?;
        Some((loaded.instance, loaded.graph.vectors_generation()))
    }

    /// `umx embeddings status`' `vectors_digest` for `alias`'s vectors in `key` ([`similar::vectors_digest`]):
    /// `key` and [`whole_digest`] over them, the value `_meta.tsv` records. None when the graph is not loaded.
    /// O(nodes): callers keep it by [`Self::vectors_stamp`].
    pub fn vectors_digest(&self, alias: &str, key: &ModelKey) -> Option<[u8; 32]> {
        let loaded = self.graphs.get(alias)?;
        Some(similar::vectors_digest(
            key,
            &whole_digest(&loaded.graph, &key.to_string()),
        ))
    }

    pub(super) fn provenance_changed(&mut self, alias: &str, nodes: &[Id]) -> Vec<Problem> {
        if nodes.is_empty() {
            return Vec::new();
        }
        let Some(policy) = self.effective_policy(alias) else {
            return Vec::new();
        };
        match self.update_similar(alias, policy, nodes, true) {
            Ok((_, warnings)) => warnings,
            // Content is committed; a derived-file failure is warned and remains owed.
            Err(cause) => alloc::vec![cause],
        }
    }

    /// Reads `alias`'s stored lists for `key` into its graph if `_meta.tsv` matches the vectors it holds and
    /// `settings`, and the shards give exactly the edges it counts; otherwise leaves empty lists in place and says
    /// they are stale. A file that cannot be read, a missing shard, or a line that is damaged or refused (R102-3)
    /// makes them stale too: the rebuild writes them again.
    pub fn load_similar(
        &mut self,
        alias: &str,
        policy: Policy,
        key: &ModelKey,
        settings: Settings,
    ) -> SimilarLoaded {
        let space = key.to_string();
        let Some(loaded) = self.graphs.get(alias) else {
            return SimilarLoaded::Stale;
        };
        let expected = expected_meta(&loaded.graph, key, settings);
        let Ok((files, dir)) = self.shard_home(loaded, policy) else {
            return self.empty_similar(alias, &space, settings, true);
        };
        let stored = files
            .read_ref(&dir, &FileRef::SimilarMeta(key.clone()))
            .ok()
            .flatten();
        let has_vectors = loaded.graph.whole_block(&space).is_some();
        // The lists are not loaded yet, so the edge count is checked after reading them.
        let edges = match stored.as_deref().map(Meta::decode) {
            // No vectors and no files: nothing to store, nothing stale.
            None if !has_vectors => return self.empty_similar(alias, &space, settings, false),
            // The digest covers the whole-node texts, so lists from a carried older-version line still match: a
            // form bump that changes no text keeps them (s22). The next write of the lists records the current form.
            Some(Some(m))
                if Meta {
                    edges: m.edges,
                    text_form: m.text_form,
                    ..expected
                } == m =>
            {
                m.edges
            }
            _ => return self.empty_similar(alias, &space, settings, true),
        };
        let Ok(shards) = files.list_shards(&dir, key, ShardKind::Similar) else {
            return self.empty_similar(alias, &space, settings, true);
        };
        let graph = &mut self.graphs.get_mut(alias).expect("loaded above").graph;
        let mut load = graph.load_similar(&space, settings.k, settings.floor);
        let mut taken = 0u64;
        for shard in shards {
            let Ok(bytes) = files.read_ref(&dir, &FileRef::Shard(shard)) else {
                drop(load);
                return self.empty_similar(alias, &space, settings, true);
            };
            for line in similar::decode_shard(&bytes.unwrap_or_default()) {
                if let SimilarShardLine::Edge(e) = line {
                    taken += u64::from(load.put(e.node, e.other, e.score));
                }
            }
        }
        load.finish();
        if taken != edges {
            return self.empty_similar(alias, &space, settings, true);
        }
        self.set_similar_stale(alias, false);
        SimilarLoaded::Fresh
    }

    fn empty_similar(
        &mut self,
        alias: &str,
        space: &str,
        settings: Settings,
        stale: bool,
    ) -> SimilarLoaded {
        if let Some(l) = self.graphs.get_mut(alias) {
            l.graph
                .load_similar(space, settings.k, settings.floor)
                .finish();
        }
        self.set_similar_stale(alias, stale);
        if stale {
            SimilarLoaded::Stale
        } else {
            SimilarLoaded::Fresh
        }
    }

    fn set_similar_stale(&mut self, alias: &str, stale: bool) {
        if let Some(l) = self.graphs.get_mut(alias) {
            l.similar_stale = stale;
        }
    }

    /// Whether `alias`'s similar_to lists are waiting for a rebuild (results meanwhile are incomplete).
    pub fn similar_stale(&self, alias: &str) -> bool {
        self.graphs.get(alias).is_some_and(|l| l.similar_stale)
    }

    /// After a batch of applied vectors: refreshes the lists for every node whose whole-node vector changed (and any
    /// list marked stale), then writes the shards of every list that changed and `_meta.tsv`, as one set. Nothing
    /// when the graph holds no lists or they await a rebuild (the rebuild writes everything). With `defer`, only
    /// memory moves and the changed lists are owed ([`Self::flush_similar_step`] writes them, ws004 s25a): until
    /// then the old `_meta.tsv` no longer matches the vectors, so a crash meanwhile loads the lists as stale.
    pub fn update_similar(
        &mut self,
        alias: &str,
        policy: Policy,
        changed_whole: &[Id],
        defer: bool,
    ) -> Result<(usize, Vec<Problem>), Problem> {
        let Some(loaded) = self.graphs.get_mut(alias) else {
            return Ok((0, Vec::new()));
        };
        if loaded.similar_stale || loaded.graph.similar_space().is_none() {
            return Ok((0, Vec::new()));
        }
        let graph = &mut loaded.graph;
        // Lists marked stale and every list that loses ground are recomputed once for the batch, not per node.
        let nodes: Vec<NodeIx> = changed_whole
            .iter()
            .filter_map(|id| graph.node_ix(*id))
            .collect();
        let changed: BTreeSet<NodeIx> = graph.refresh_similar_batch(&nodes).into_iter().collect();
        graph.finish_similar();
        if defer {
            let shards: Vec<String> = changed
                .iter()
                .map(|&ix| shard_of(graph.node_at(ix).id()))
                .collect();
            loaded.similar_owed.extend(shards);
            loaded.similar_meta_owed = true;
            loaded.similar_flush_paused = false;
            return Ok((0, Vec::new()));
        }
        if loaded.similar_unwritten {
            // An earlier write failed after the lists moved on: write them all, never `_meta.tsv` alone.
            return self.write_similar(alias, policy, None);
        }
        let ids: Vec<Id> = changed.iter().map(|&ix| graph.node_at(ix).id()).collect();
        self.write_similar(alias, policy, Some(&ids))
    }

    /// A copy of `alias`'s whole-node block and the settings its lists use, for a rebuild that computes with no
    /// lock held. None when the graph holds no lists or no vectors.
    pub fn similar_snapshot(&self, alias: &str) -> Option<SimilarityJob> {
        let loaded = self.graphs.get(alias)?;
        let graph = &loaded.graph;
        let space = graph.similar_space()?;
        let (k, floor) = graph.similar_settings();
        Some(SimilarityJob {
            instance: loaded.instance,
            space: space.to_string(),
            snapshot: graph.similar_snapshot(space)?,
            k,
            floor,
        })
    }

    /// Puts a rebuild's lists in place and writes every similar_to file of the graph's space: `Ok(None)` when the
    /// graph was loaded again, or its vectors or list settings changed, while it computed (compute again), else how
    /// many files it wrote and any warnings. When the graph's files changed on disk since the load (F144) the
    /// rebuild publishes nothing: refused with `graph_files_changed` before its lists go in (even when they would
    /// write no file), and when the change lands while its files are written, its lists are taken out again and
    /// the graph's lists stay stale (results incomplete, no write owed) until a load.
    pub fn apply_similar_rebuild(
        &mut self,
        alias: &str,
        policy: Policy,
        result: SimilarityResult,
    ) -> Result<Option<(usize, Vec<Problem>)>, Problem> {
        let Some(loaded) = self.graphs.get(alias) else {
            return Ok(None);
        };
        let g = &loaded.graph;
        let same = loaded.instance == result.instance
            && g.similar_space() == Some(result.space.as_str())
            && g.similar_settings() == (result.k, result.floor);
        if !same {
            return Ok(None);
        }
        self.files_unchanged(alias)?;
        let Some(loaded) = self.graphs.get_mut(alias) else {
            return Ok(None);
        };
        if !loaded.graph.apply_similar(result.lists) {
            return Ok(None);
        }
        loaded.similar_stale = false;
        let written = self.write_similar(alias, policy, None);
        if let Err(p) = &written
            && p.code == "graph_files_changed"
            && let Some(l) = self.graphs.get_mut(alias)
            && let Some(space) = l.graph.similar_space().map(String::from)
        {
            let (k, floor) = l.graph.similar_settings();
            l.graph.load_similar(&space, k, floor).finish();
            l.similar_stale = true;
            l.similar_unwritten = false;
        }
        written.map(Some)
    }

    /// Writes the lists of `nodes` (`None`: every node, a rebuild) into their shards, keeping each shard's other
    /// lines (other nodes' lists, lines naming nodes not in the graph, lines umx cannot read), with `_meta.tsv`, as
    /// one set. A shard left with no lines is removed. Returns how many files changed. After any failure the files
    /// are owed in full (R102-2): the lists in memory have moved on, and a later batch may change none of them.
    /// Owed shards go in the same set, so `_meta.tsv` is never written while a list it describes lags (ws004 s25a).
    fn write_similar(
        &mut self,
        alias: &str,
        policy: Policy,
        nodes: Option<&[Id]>,
    ) -> Result<(usize, Vec<Problem>), Problem> {
        let merged: Option<Vec<Id>> = nodes.map(|ids| {
            let mut all: BTreeSet<Id> = self
                .graphs
                .get(alias)
                .map(|l| nodes_in_shards(&l.graph, &l.similar_owed))
                .unwrap_or_default();
            all.extend(ids.iter().copied());
            all.into_iter().collect()
        });
        let written = self.write_similar_files(alias, policy, merged.as_deref(), true);
        if let Some(l) = self.graphs.get_mut(alias) {
            l.similar_unwritten = written.is_err();
            if written.is_ok() {
                l.similar_owed.clear();
                l.similar_meta_owed = false;
                l.similar_meta_removed = false;
            }
        }
        written
    }

    /// Whether any loaded graph owes similar_to files it will try to write now (ws004 s25a).
    pub fn similar_owed(&self) -> bool {
        self.graphs.values().any(owes)
    }

    /// Lets every graph whose owed writes failed try again (a stop or restart writes what it can).
    pub fn unpause_similar(&mut self) {
        for l in self.graphs.values_mut() {
            l.similar_flush_paused = false;
        }
    }

    /// One step of the owed similar_to writes, at most one file (ws004 s25a; R180-1, R180-2), in this order:
    /// files owed in full become every shard on disk and every shard a list lives in (no file); `_meta.tsv` is
    /// removed; each owed shard is rewritten whole; `_meta.tsv` is written last. `only` limits it to one graph.
    /// None when nothing is owed. A failed step keeps its debt and pauses the graph (no retry loop on a failing
    /// folder) until new lists are owed, an unload or a stop.
    pub fn flush_similar_step(&mut self, only: Option<&str>) -> Option<Vec<Problem>> {
        let alias = self
            .graphs
            .iter()
            .filter(|(a, l)| only.is_none_or(|o| o == a.as_str()) && owes(l))
            .map(|(a, _)| a.clone())
            .next()?;
        let policy = self.effective_policy(&alias);
        let l = self.graphs.get_mut(&alias)?;
        let Some(policy) = policy.filter(|_| !l.similar_stale && l.graph.similar_space().is_some())
        else {
            // No lists to write, or they await a rebuild, which writes everything.
            l.similar_owed.clear();
            l.similar_meta_owed = false;
            l.similar_unwritten = false;
            return Some(Vec::new());
        };
        let failed = |store: &mut GraphStore, p: Problem| {
            if let Some(l) = store.graphs.get_mut(&alias) {
                l.similar_flush_paused = true;
            }
            Some(alloc::vec![p])
        };
        if l.similar_unwritten {
            return match self.similar_shard_names(&alias, policy) {
                Ok(all) => {
                    let l = self.graphs.get_mut(&alias)?;
                    l.similar_owed.extend(all);
                    l.similar_meta_owed = true;
                    l.similar_unwritten = false;
                    Some(Vec::new())
                }
                Err(p) => failed(self, p),
            };
        }
        if !l.similar_meta_removed {
            return match self.remove_similar_meta(&alias, policy) {
                Ok((_, w)) => {
                    self.graphs.get_mut(&alias)?.similar_meta_removed = true;
                    Some(w)
                }
                Err(p) => failed(self, p),
            };
        }
        if let Some(shard) = l.similar_owed.first().cloned() {
            let one = BTreeSet::from([shard.clone()]);
            let ids: Vec<Id> = nodes_in_shards(&l.graph, &one).into_iter().collect();
            return match self.write_similar_files(&alias, policy, Some(&ids), false) {
                Ok((_, w)) => {
                    self.graphs.get_mut(&alias)?.similar_owed.remove(&shard);
                    Some(w)
                }
                Err(p) => failed(self, p),
            };
        }
        match self.write_similar_files(&alias, policy, Some(&[]), true) {
            Ok((_, w)) => {
                let l = self.graphs.get_mut(&alias)?;
                l.similar_meta_owed = false;
                l.similar_meta_removed = false;
                Some(w)
            }
            Err(p) => failed(self, p),
        }
    }

    /// Writes every similar_to file `alias` owes, a step at a time, and returns what the steps found (R180-4).
    pub(super) fn flush_similar_of(&mut self, alias: &str) -> Vec<Problem> {
        if let Some(l) = self.graphs.get_mut(alias) {
            l.similar_flush_paused = false;
        }
        let mut problems = Vec::new();
        while let Some(found) = self.flush_similar_step(Some(alias)) {
            problems.extend(found);
        }
        problems
    }

    /// What writing owed files found while graphs unloaded themselves, since the last call (R180-4).
    pub fn take_unload_problems(&mut self) -> Vec<Problem> {
        core::mem::take(&mut self.unload_problems)
    }

    pub(super) fn note_unload_problems(&mut self, problems: Vec<Problem>) {
        self.unload_problems.extend(problems);
    }

    /// Every similar_to shard of `alias`: those on disk and those a list in memory lives in.
    fn similar_shard_names(
        &self,
        alias: &str,
        policy: Policy,
    ) -> Result<BTreeSet<String>, Problem> {
        let Some(loaded) = self.graphs.get(alias) else {
            return Ok(BTreeSet::new());
        };
        let graph = &loaded.graph;
        let Some(key) = graph.similar_space().and_then(|s| ModelKey::parse(s).ok()) else {
            return Ok(BTreeSet::new());
        };
        let (files, dir) = self.shard_home(loaded, policy)?;
        let mut names: BTreeSet<String> = files
            .list_shards(&dir, &key, ShardKind::Similar)
            .map_err(|e| problems::files_error(&e, problems::shard_read_failed))?
            .into_iter()
            .map(|s| s.shard().to_string())
            .collect();
        names.extend(
            graph
                .nodes()
                .filter(|n| !graph.similar_out(n.ix()).is_empty())
                .map(|n| shard_of(n.id())),
        );
        Ok(names)
    }

    /// Removes `alias`'s `_meta.tsv`, alone (R180-1). Nothing when there is none.
    fn remove_similar_meta(
        &self,
        alias: &str,
        policy: Policy,
    ) -> Result<(usize, Vec<Problem>), Problem> {
        let Some(loaded) = self.graphs.get(alias) else {
            return Ok((0, Vec::new()));
        };
        let Some(key) = loaded
            .graph
            .similar_space()
            .and_then(|s| ModelKey::parse(s).ok())
        else {
            return Ok((0, Vec::new()));
        };
        let (files, dir) = self.shard_home(loaded, policy)?;
        let meta = FileRef::SimilarMeta(key);
        let there = files
            .read_ref(&dir, &meta)
            .map_err(|e| problems::files_error(&e, problems::shard_read_failed))?;
        if there.is_none() {
            return Ok((0, Vec::new()));
        }
        // F144, architect: in the cache `expect` is None, since the set holds none of the graph's files; the looks
        // at the graph's folder before and after the set (write_checked) are the guard.
        self.write_checked(alias, loaded, &dir, |expect| {
            commit(
                alias,
                files.as_ref(),
                &dir,
                &[Change {
                    file: meta,
                    bytes: None,
                }],
                (&loaded.dir, expect),
            )
        })
    }

    fn write_similar_files(
        &self,
        alias: &str,
        policy: Policy,
        nodes: Option<&[Id]>,
        with_meta: bool,
    ) -> Result<(usize, Vec<Problem>), Problem> {
        let Some(loaded) = self.graphs.get(alias) else {
            return Ok((0, Vec::new()));
        };
        let graph = &loaded.graph;
        let Some(key) = graph.similar_space().and_then(|s| ModelKey::parse(s).ok()) else {
            return Ok((0, Vec::new()));
        };
        let (k, floor) = graph.similar_settings();
        // Only k and the floor go into the files; where the floor came from is not recorded there.
        let settings = Settings {
            k,
            floor,
            source: umx_core::similarity::FloorSource::None,
            run: None,
        };
        let (files, dir) = self.shard_home(loaded, policy)?;
        let failed = |e: FilesError| problems::files_error(&e, problems::shard_read_failed);
        // The shards to write, each with the graph nodes whose lists it carries.
        let mut by_shard: BTreeMap<String, Vec<NodeIx>> = BTreeMap::new();
        match nodes {
            Some(ids) => {
                for id in ids {
                    if let Some(ix) = graph.node_ix(*id) {
                        by_shard.entry(shard_of(*id)).or_default().push(ix);
                    }
                }
            }
            None => {
                for shard in files
                    .list_shards(&dir, &key, ShardKind::Similar)
                    .map_err(failed)?
                {
                    by_shard.entry(shard.shard().to_string()).or_default();
                }
                for n in graph
                    .nodes()
                    .filter(|n| !graph.similar_out(n.ix()).is_empty())
                {
                    by_shard.entry(shard_of(n.id())).or_default().push(n.ix());
                }
            }
        }
        let mut changes: Vec<(FileRef, Option<Vec<u8>>)> = Vec::new();
        for (name, owned) in by_shard {
            let shard = ShardFile::of_kind(ShardKind::Similar, key.clone(), &name)
                .expect("a node id's shard name");
            let file = FileRef::Shard(shard);
            let before = files.read_ref(&dir, &file).map_err(failed)?;
            let rewritten: BTreeSet<Id> = owned.iter().map(|&ix| graph.node_at(ix).id()).collect();
            let mut lines: Vec<SimilarShardLine> =
                similar::decode_shard(before.as_deref().unwrap_or_default())
                    .into_iter()
                    .filter(|l| match l {
                        SimilarShardLine::Edge(e) => match nodes {
                            // A rebuild owns every graph node's lines; lines naming a node not in the graph stay.
                            None => graph.node_ix(e.node).is_none(),
                            Some(_) => !rewritten.contains(&e.node),
                        },
                        SimilarShardLine::Kept(_) => true,
                    })
                    .collect();
            for &ix in &owned {
                lines.extend(lines_of(graph, ix));
            }
            let bytes = similar::encode_shard(&lines);
            match (&before, bytes.is_empty()) {
                (Some(old), false) if *old == bytes => {}
                (None, true) => {}
                (_, true) => changes.push((file, None)),
                (_, false) => changes.push((file, Some(bytes))),
            }
        }
        let meta = FileRef::SimilarMeta(key.clone());
        let meta_bytes = expected_meta(graph, &key, settings).encode();
        if with_meta
            && files.read_ref(&dir, &meta).map_err(failed)?.as_deref() != Some(&meta_bytes[..])
        {
            changes.push((meta, Some(meta_bytes)));
        }
        if changes.is_empty() {
            return Ok((0, Vec::new()));
        }
        let set: Vec<Change<'_>> = changes
            .iter()
            .map(|(file, bytes)| Change {
                file: file.clone(),
                bytes: bytes.as_deref(),
            })
            .collect();
        // F144, architect: in the cache `expect` is None, since the set holds none of the graph's files; the looks
        // at the graph's folder before and after the set (write_checked) are the guard.
        self.write_checked(alias, loaded, &dir, |expect| {
            commit(alias, files.as_ref(), &dir, &set, (&loaded.dir, expect))
        })
    }
}

/// Whether a graph owes similar_to files it will try to write now.
fn owes(l: &super::Loaded) -> bool {
    !l.similar_flush_paused
        && (l.similar_unwritten || l.similar_meta_owed || !l.similar_owed.is_empty())
}

/// The graph nodes whose lists live in `shards`: a shard rewritten whole rewrites each of their lines.
fn nodes_in_shards(graph: &Graph, shards: &BTreeSet<String>) -> BTreeSet<Id> {
    if shards.is_empty() {
        return BTreeSet::new();
    }
    graph
        .nodes()
        .map(|n| n.id())
        .filter(|id| shards.contains(&shard_of(*id)))
        .collect()
}

/// Writes one set of similar_to changes, in umx's words when it fails or is not synced. The lists were computed
/// from the graph in memory, so the set expects its content files as `graph` (folder, stamps) holds them (F144);
/// on `graph_files_changed` the lists stay owed in memory and the flush pauses, never retried in a loop.
fn commit(
    alias: &str,
    files: &dyn umx_core::ports::GraphFiles,
    dir: &umx_core::ports::GraphDir,
    set: &[Change<'_>],
    graph: (&umx_core::ports::GraphDir, Option<&GraphStamps>),
) -> Result<(usize, Vec<Problem>), Problem> {
    let first = set[0].file.clone();
    match files
        .write_set_checked(dir, set, graph.1)
        .map(|c| c.written)
    {
        Ok(Written::Durable) => Ok((set.len(), Vec::new())),
        Ok(Written::NotSynced(e)) => Ok((
            set.len(),
            alloc::vec![problems::similar_not_durable(&first, &e)],
        )),
        Err(FilesError::RecoveryConflict(conflicts)) => Err(problems::recovery_conflict(
            dir,
            &conflicts,
            Some(&alias_name(alias)),
        )),
        Err(FilesError::Changed(changed)) => Err(problems::graph_files_changed(
            &alias_name(alias),
            graph.0,
            &changed,
        )),
        Err(e) => Err(problems::files_error(&e, |e| {
            problems::similar_write_failed(&first, e)
        })),
    }
}
