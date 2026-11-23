//! The graph store service (blueprint 001, d-12): loaded graphs, their files and every write to them.

mod adopt_std;
mod along;
mod batch;
mod casefold;
mod copies;
mod edge;
mod entity;
mod evidence_edit;
mod evidence_redact;
mod external;
#[cfg(test)]
mod external_ends_tests;
mod graph;
mod graphfile;
mod guide;
mod ignore;
mod incoming;
mod index;
mod lint;
mod node;
mod node_more;
mod ontology;
mod policy;
mod provenance;
mod raised_in;
mod registry;
mod rename;
mod resolve;
mod resources;
mod scopes;
mod search;
mod secrets;
mod settings;
mod similar;
mod sources;
mod spine;
mod spine_decl;
pub(crate) mod tags;
mod time_check;
mod ui;
mod unload;
mod vectors;
mod who;

pub use adopt_std::StdAdopted;
pub use along::{
    AlongInfo, AlongRequest, AlongSource, SpineBy, SpinePlace, one_graph as along_one_graph,
    reserved_spine_name,
};
pub(crate) use along::{GRAPH_WIDE, Kept, Why};
pub use evidence_edit::{EvidenceAdded, EvidenceListed, EvidenceRemoved, EvidenceRow, Redacted};
pub(crate) use external::ReadEnd;
pub use incoming::IncomingIndex;
#[cfg(feature = "bench")]
pub use provenance::upsert_session as upsert_provenance_session;
pub(crate) use search::Searched;
pub use similar::{SimilarLoaded, SimilarityJob, SimilarityResult};
pub use sources::SourceRoots;
pub use ui::{SelectedRow, TITLE_CHARS};
pub use who::{NodeWho, StampState, WhoListed, WhoNode, WhoRecord, WhoRow, WhoStamp};

use crate::{Ctx, Done, Outcome, Page, page, problems};
use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use umx_core::codec::{self, GraphFile};
use umx_core::ports::{
    Change, Clock, FileRef, FileStamp, FilesError, GraphDir, GraphFiles, GraphStamps, IdGenerator,
    LocalState, SourceReader, Written,
};
use umx_core::problem::Candidate;
use umx_core::ui::{Fed, Touched};
use umx_core::{GraphMeta, Id, IdText, Problem, ValidName};
use umx_graph::{Graph, Lookup, NodeRef};

pub(crate) use batch::add_late_warnings;
pub use batch::{
    BatchDone, BatchLine, BatchOp, BatchOutcome, BatchRequest, BatchRow, BatchSentence,
    Idempotency, Keyed,
};
pub use copies::PrimarySet;
pub use edge::{
    EdgeAdded, EdgeChanged, EdgeEditRequest, EdgeEvidence, EdgeListRequest, EdgeRow, ReadEdge,
};
pub use entity::{
    EntityDenied, EntityLinked, EntityMatch, EntityMember, EntityProposed, EntityShown,
    EntityUnlinked,
};
pub(crate) use graph::second;
pub use graph::{CheckRow, Checked, GraphInfo, GraphLoaded, GraphNew, MAX_LOAD_WARNINGS};
pub(crate) use graphfile::under;
pub use graphfile::{
    GraphfileAdded, GraphfileListing, GraphfileMode, GraphfileRow, ScanRequest, ScanRow, Scanned,
};
pub use guide::{Guide, GuideEntry, GuideExample, GuideField, GuideType};
pub(crate) use index::Known;
pub use index::{FolderState, GraphForgotten, IndexRow};
pub use lint::{LINT_PAGE, Lint, LintKind, LintRequest, LintRow, RangePair};
pub use node::{
    ListedNode, NEAR_DUPLICATE, NEAR_DUPLICATE_OTHER_TYPE, NodeAddRequest, NodeAdded,
    NodeListRequest,
};
pub(crate) use node::{key_refused, where_parts};
pub use node_more::{
    HistoryRow, NodeEditRequest, NodeEdited, NodeNoteRequest, NodeNoted, NodeStatusRequest,
    StatusMoved,
};
pub use ontology::{FieldsAdded, OntologyAdded, OntologyRemoved, RoleSet};
pub use policy::{PolicyConfirmed, PolicyMemo, PolicyView};
pub use registry::{NotReloaded, ReloadReport, Reloaded};
pub use rename::{Moved, Renamed};
pub(crate) use resolve::check_evidence;
pub use resolve::{ResolveStatus, Resolved, Target};
pub use resources::ResourceShown;
pub(crate) use scopes::SavedRead;
pub use scopes::{Scope, ScopeInfo, ScopeMemberInfo};
pub(crate) use search::ReadScope;
pub use secrets::{GraphKey, SecretAllowed, edge_secret_flags};
pub use settings::GraphSet;
pub use spine::{
    InTheWay, InTheWayKind, SpineCandidate, SpineHint, SpineHints, SpineLine, SpineProposal,
    SpineSource, Unplaced, WhyNot,
};
pub use spine_decl::{DeclaredSpine, SpineRemoved, SpineSet, SpinesShown};
pub use unload::GraphUnloaded;

/// The ports a `GraphStore` runs over.
#[derive(Clone)]
pub struct Ports {
    pub files: Arc<dyn GraphFiles>,
    /// Where a secured graph's vector shards go (req-46): a second files port with its own root, outside every
    /// graph folder, so they are never committed. Atomicity is per instance: a set never spans the graph folder
    /// and the cache (vectors arrive in their own write, after the node write, via the embed worker).
    pub vector_cache: Arc<dyn GraphFiles>,
    /// The cache's root folder; a graph's shards go in `<root>/<graph id>/<folder key>/embeddings/...`.
    pub vector_cache_root: GraphDir,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
    /// This machine's record of each graph's confirmed embedding policy.
    pub state: Arc<dyn LocalState>,
    /// This machine's secret allows (req-49, `$UMX_HOME/allows/secret-allows.json`): a LocalState of its own, in its
    /// own folder, so it can never break the policy record.
    pub allows: Arc<dyn LocalState>,
    /// This machine's lint terms (F139, `$UMX_HOME/lint/lint-terms.json`): a LocalState of its own, in its own folder;
    /// absent means no term scan.
    pub lint_terms: Arc<dyn LocalState>,
    pub scopes: Arc<dyn LocalState>,
    /// This machine's graph registry (req-56's first slice, F72, `$UMX_HOME/registry/graphs.toml`): the graphs to
    /// reload when the daemon starts. A LocalState of its own, in its own folder.
    pub registry: Arc<dyn LocalState>,
    /// Confines `root:` evidence sources to the roots (req-47, ws006 S1).
    pub sources: Arc<dyn SourceReader>,
}

/// A graph the daemon holds in memory.
pub struct Loaded {
    pub dir: GraphDir,
    pub meta: GraphMeta,
    /// The ontology umx acts on (ws008 T2, req-98): `meta.ontology` with std v1 first for a std graph
    /// ([`umx_core::Ontology::effective`]). Built at load and again by every graph.json write that stands
    /// ([`persist`]); every reader uses it ([`Loaded::ontology`]), while writes change and write `meta.ontology`.
    ontology: umx_core::Ontology,
    pub graph: Graph,
    /// Own-graph endpoint keys were normalized on load; the next write also flushes the edge file.
    edges_normalized: bool,
    /// The embedding policy umx acts on for this graph (graph.json and this machine's record).
    pub policy: PolicyView,
    /// Nodes without a current vector in the graph's own space (req-75), kept by the services as loads, writes and
    /// embeds change it; None when the graph has no space (no recorded key and no embedder).
    pub vectors_pending: Option<alloc::collections::BTreeSet<Id>>,
    /// Nodes with a text the secret hold keeps from this graph's hosted embedder (req-49): counted as `vectors_held`,
    /// not pending, so pending reaches 0 once everything that may be sent is embedded (embeddings-expert).
    pub vectors_held: alloc::collections::BTreeSet<Id>,
    /// The pending nodes whose committed whole-node vector was made from text that included a field now data (req-97,
    /// `text_form_outdated`), from the last scan; a node leaves it once its vectors are current. While it is empty and
    /// the space is known, graph.json's text form is the current one in memory, written at the next graph.json write.
    pub vectors_outdated: alloc::collections::BTreeSet<Id>,
    /// The stored similar_to lists did not match the vectors or settings at load and await a rebuild (workshop 008
    /// section 9): similar_to results carry `similar_to_incomplete` until it lands.
    pub similar_stale: bool,
    /// A similar_to write failed after the lists in memory moved on (R102-2): the files are owed in full, and the next
    /// write of the lists writes every shard and `_meta.tsv`, not only the lists its batch changed.
    pub similar_unwritten: bool,
    /// Shards whose lists moved in memory while their files were deferred (ws004 s25a): each is rewritten whole, a
    /// shard per step, and by any earlier write of the lists. Queries read memory, so they stay current.
    pub similar_owed: alloc::collections::BTreeSet<String>,
    /// `_meta.tsv` is owed: written last, once no shard is owed, so the files on disk never claim lists they lack.
    pub similar_meta_owed: bool,
    /// This cycle of owed writes has removed `_meta.tsv` (R180-1): until the last step writes it again, a load finds
    /// none and rebuilds, so list shards from two states can never pass as fresh, even when a text comes back.
    pub similar_meta_removed: bool,
    /// A step of owed writes failed: none is retried until new lists are owed, an unload, or a stop.
    pub similar_flush_paused: bool,
    /// This load of the graph, unique in the process (R102-1): work computed from one load (a similarity rebuild) is
    /// never applied to a later load of the same folder, whose counters may coincide.
    pub instance: u64,
    /// When this folder was first loaded, for the primary copy (d-21): unlike `instance`, a reload of the same folder
    /// keeps it, so reloading the primary never makes another copy primary (graph-addressing-expert B1).
    pub order: u64,
    /// Clock ms of the last command that used this graph, for idle unload (req-03).
    pub last_used: AtomicU64,
    /// This load's revision and the changes the views have not been handed yet (ws009 S5a).
    pub feed: Feed,
    /// graph.json, nodes.jsonl and edges.jsonl as this load read them or its last write left them (F144). Every write
    /// of the folder expects them, so a checkout, a pull or a hand edit since is refused, never written over.
    stamps: GraphStamps,
}

/// A loaded graph's revision (ws009 S5a, architect L2): 0 at each load, one more for each write set of its content
/// files that succeeded, and only there, in [`persist_parts`]. A rollback, a failed write, an in-memory rebuild and
/// the vector and similar_to files never move it. Not persisted.
#[derive(Debug, Default)]
pub struct Feed {
    rev: u64,
    /// Each commit's revision and what it touched, until the store hands them on ([`GraphStore::take_changes`]).
    /// At most [`umx_core::ui::CHANGE_LOG`]: older ones are dropped, and the change log sees the gap as overflow.
    pending: alloc::collections::VecDeque<(u64, Touched)>,
}

impl Feed {
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// The one commit: the revision moves on and what the write touched is kept for the views, with provenance
    /// nodes and their local edges left out of every list. An id the graph no longer holds cannot be judged and is
    /// kept: a write that removes ids commits while they are still in the graph (`mark_symmetric`), or removes only
    /// what its own read scope showed (`edge delete`, single or batch).
    fn commit(&mut self, graph: &Graph, mut touched: Touched) {
        let shown = |id: &Id| graph.node(*id).is_none_or(|n| !graph.is_provenance(n.ix()));
        let scope = ReadScope::new(graph);
        let shown_edge = |id: &Id| graph.edge(*id).is_none_or(|edge| scope.stored_edge(edge));
        for list in [
            &mut touched.nodes.added,
            &mut touched.nodes.edited,
            &mut touched.nodes.removed,
        ] {
            list.retain(shown);
        }
        for list in [
            &mut touched.edges.added,
            &mut touched.edges.edited,
            &mut touched.edges.removed,
        ] {
            list.retain(shown_edge);
        }
        self.rev += 1;
        if self.pending.len() == umx_core::ui::CHANGE_LOG {
            self.pending.pop_front();
        }
        self.pending.push_back((self.rev, touched));
    }
}

/// The next [`Loaded::instance`].
static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);

/// The next [`Loaded::order`].
static NEXT_ORDER: AtomicU64 = AtomicU64::new(1);

impl Loaded {
    pub(crate) fn read_scope(&self) -> ReadScope<'_> {
        ReadScope::new(&self.graph)
    }

    /// The ontology umx acts on: the stored one with std v1 first for a std graph. Read it, never `meta.ontology`,
    /// except to change what graph.json holds.
    pub fn ontology(&self) -> &umx_core::Ontology {
        &self.ontology
    }

    /// Builds the effective ontology again from `meta.ontology`, and the word index's fields from it (req-68), and
    /// turns the tag index on or off with std (ws008 t2b): a graph that adopts std builds it here, once.
    fn refresh_ontology(&mut self) {
        self.ontology = self.meta.ontology.effective();
        self.graph.set_text_fields(self.ontology.text_fields());
        self.graph
            .set_std_tags(self.ontology.std_version().is_some());
    }

    /// Pending vectors not held by the secret hold (None: the graph has no model space).
    pub fn pending_count(&self) -> Option<usize> {
        self.vectors_pending.as_ref().map(|p| {
            p.iter()
                .filter(|id| !self.vectors_held.contains(id))
                .count()
        })
    }

    /// Pending vectors held by the secret hold (None: the graph has no model space).
    pub fn held_count(&self) -> Option<usize> {
        self.vectors_pending
            .as_ref()
            .map(|p| p.iter().filter(|id| self.vectors_held.contains(id)).count())
    }

    /// Records the current text form in memory once no node is known to be outdated (req-97): never a write, so a
    /// load changes no file; the next graph.json write carries it. Unknown pending (no space, or a failed scan)
    /// changes nothing.
    pub(crate) fn settle_text_form(&mut self) {
        if self.vectors_pending.is_none() || !self.vectors_outdated.is_empty() {
            return;
        }
        if let Some(e) = self.meta.embedding.as_mut()
            && e.text_form < umx_core::vectors::WHOLE_NODE_VERSION
        {
            e.text_form = umx_core::vectors::WHOLE_NODE_VERSION;
        }
    }

    /// `ontology` is `meta.ontology.effective()`, built once by the caller, whose load indexes words with it; a
    /// graph indexed otherwise (a new one) gets its fields here.
    pub fn new(
        dir: GraphDir,
        meta: GraphMeta,
        ontology: umx_core::Ontology,
        mut graph: Graph,
        now_ms: u64,
        stamps: GraphStamps,
    ) -> Loaded {
        debug_assert!(ontology == meta.ontology.effective());
        graph.set_text_fields(ontology.text_fields());
        graph.set_std_tags(ontology.std_version().is_some());
        Loaded {
            dir,
            policy: PolicyView::file_only(meta.file_policy()),
            ontology,
            meta,
            graph,
            edges_normalized: false,
            vectors_pending: None,
            vectors_held: alloc::collections::BTreeSet::new(),
            vectors_outdated: alloc::collections::BTreeSet::new(),
            similar_stale: false,
            similar_unwritten: false,
            similar_owed: alloc::collections::BTreeSet::new(),
            similar_meta_owed: false,
            similar_meta_removed: false,
            similar_flush_paused: false,
            instance: NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed),
            order: NEXT_ORDER.fetch_add(1, Ordering::Relaxed),
            last_used: AtomicU64::new(now_ms),
            feed: Feed::default(),
            stamps,
        }
    }

    /// The lines `id` is on in `file` as the next write lays it out, if it is a repeated id.
    fn repeat(&self, file: GraphFile, id: Id) -> Option<codec::Repeat> {
        let lines = self.graph.repeat_lines(file, id)?;
        Some(codec::Repeat { file, id, lines })
    }

    pub fn last_used_ms(&self) -> u64 {
        self.last_used.load(Ordering::Relaxed)
    }
}

/// The graph store (d-12): owns the loaded graphs and the graph-file ports, and is the one write path to graph
/// files. Its fields are private so other services reach it only through its pub fns.
pub struct GraphStore {
    ports: Ports,
    graphs: BTreeMap<String, Loaded>,
    /// Idle time after which a graph unloads itself; 0 means never.
    idle_ms: u64,
    /// The folders graphs may live in, as the composition configured them; named in `path_outside_roots`.
    roots: Vec<GraphDir>,
    /// Secured policies (graph id, folder) this daemon could not write to this machine's record yet. They count as
    /// recorded, and every later write of the record carries them (ws004 s06 A2, review R1).
    unsaved: Vec<(Id, String, umx_core::Policy)>,
    /// How many times this store has written the record: a request's [`PolicyMemo`] taken before a write reads it
    /// again.
    state_writes: u64,
    /// Graph ids loaded as two or more copies whose primary was unloaded: none is primary until one copy is left
    /// (d-21, see `copies`).
    demoted: alloc::collections::BTreeSet<Id>,
    /// What the start-up reload did (F72), and when this store started.
    reload: ReloadReport,
    incoming: IncomingIndex,
    /// The machine config's search and similar_to floors by model key (ws004 s26); the calibrated table otherwise.
    floors: umx_core::similarity::Floors,
    /// What writing owed similar_to files found when graphs unloaded themselves (R180-4), for the daemon's log.
    unload_problems: Vec<Problem>,
    /// Where named source roots are (the config service); None leaves every `root:<name>` unknown and root:repo
    /// unavailable.
    source_roots: Option<Arc<dyn SourceRoots>>,
    /// Graphs loaded, reloaded or unloaded since the views were last handed the changes (ws009 S5a).
    resets: alloc::collections::BTreeSet<String>,
    /// Where a repo's Graphfile is read and written (ws002 s17), wired by compose; None refuses every Graphfile verb
    /// with `internal` (architect, s17), never a quiet no-op.
    config_files: Option<Arc<dyn umx_core::ports::ConfigFiles>>,
}

/// Splits `umx:<alias>/<id>` into its alias and id text; other text is an id or prefix in the chosen graph.
pub(crate) fn split_address(text: &str) -> (Option<&str>, &str) {
    match text.strip_prefix("umx:") {
        Some(rest) => match rest.split_once('/') {
            Some((alias, id)) => (Some(alias), id),
            None => (Some(rest), ""),
        },
        None => (None, text),
    }
}

impl GraphStore {
    /// (port id, adapter id) for the graph store's ports, read from the wired adapters (E-1).
    pub fn adapters(&self) -> [(&'static str, &'static str); 10] {
        [
            ("clock", self.ports.clock.adapter_id()),
            ("graph-files", self.ports.files.adapter_id()),
            ("id-generator", self.ports.ids.adapter_id()),
            ("local-state", self.ports.state.adapter_id()),
            ("secret-allows", self.ports.allows.adapter_id()),
            ("lint-terms", self.ports.lint_terms.adapter_id()),
            ("scopes", self.ports.scopes.adapter_id()),
            ("registry", self.ports.registry.adapter_id()),
            ("source-reader", self.ports.sources.adapter_id()),
            ("vector-cache", self.ports.vector_cache.adapter_id()),
        ]
    }

    pub fn new(ports: Ports) -> GraphStore {
        let reload = ReloadReport {
            started_ms: ports.clock.now_ms(),
            ..ReloadReport::default()
        };
        GraphStore {
            ports,
            reload,
            graphs: BTreeMap::new(),
            idle_ms: 0,
            roots: Vec::new(),
            unsaved: Vec::new(),
            state_writes: 0,
            demoted: alloc::collections::BTreeSet::new(),
            incoming: IncomingIndex::default(),
            floors: umx_core::similarity::Floors::default(),
            unload_problems: Vec::new(),
            source_roots: None,
            resets: alloc::collections::BTreeSet::new(),
            config_files: None,
        }
    }

    pub fn with_config_files(mut self, files: Arc<dyn umx_core::ports::ConfigFiles>) -> GraphStore {
        self.config_files = Some(files);
        self
    }

    pub(crate) fn config_files_id(&self) -> Option<&'static str> {
        self.config_files.as_ref().map(|f| f.adapter_id())
    }

    pub fn with_source_roots(mut self, roots: Arc<dyn SourceRoots>) -> GraphStore {
        self.source_roots = Some(roots);
        self
    }

    pub(crate) fn source_check(&self) -> sources::SourceCheck {
        sources::SourceCheck {
            reader: Arc::clone(&self.ports.sources),
            roots: self.source_roots.clone(),
            graph_roots: self.roots.clone(),
        }
    }

    /// The floors searches and similar_to lists use: the machine config's, else the calibrated table's (ws004 s26).
    pub fn floors(&self) -> &umx_core::similarity::Floors {
        &self.floors
    }

    pub fn set_floors(&mut self, floors: umx_core::similarity::Floors) {
        self.floors = floors;
    }

    /// The graph roots to name when a folder is outside them. The files port enforces them.
    pub fn with_roots(mut self, roots: Vec<GraphDir>) -> GraphStore {
        self.roots = roots;
        self
    }

    /// The clock port, for a read's time backstop.
    pub(crate) fn clock(&self) -> &dyn umx_core::ports::Clock {
        &*self.ports.clock
    }

    /// What changed since the last call, for the views' change log (ws009 S5a): each graph loaded, reloaded or
    /// unloaded, then each commit of the graphs loaded now, in order. A commit always follows its graph's load, and a
    /// reload or unload drops the commits of the load it replaced, so this order is the order they happened in.
    pub fn take_changes(&mut self) -> Vec<Fed> {
        let mut fed: Vec<Fed> = core::mem::take(&mut self.resets)
            .into_iter()
            .map(|graph| Fed::Reset { graph })
            .collect();
        for (alias, l) in &mut self.graphs {
            fed.extend(l.feed.pending.drain(..).map(|(rev, touched)| Fed::Wrote {
                graph: alias.clone(),
                rev,
                touched,
            }));
        }
        fed
    }

    pub fn loaded(&self, alias: &str) -> Option<&Loaded> {
        self.graphs.get(alias)
    }

    pub fn aliases(&self) -> impl Iterator<Item = &str> {
        self.graphs.keys().map(String::as_str)
    }

    /// Timing only for umx-bench (ws003 s25 phase 0): `bytes` written as `alias`'s edges.jsonl through the checked
    /// write set an edge write uses, with no encode. Not a production entry point.
    #[cfg(feature = "bench")]
    pub fn bench_write_edges(&mut self, alias: &str, bytes: &[u8]) -> Result<(), Problem> {
        let loaded = self
            .graphs
            .get_mut(alias)
            .ok_or_else(|| problems::internal("bench_alias"))?;
        let changes = [Change {
            file: GraphFile::Edges.into(),
            bytes: Some(bytes),
        }];
        let committed = self
            .ports
            .files
            .write_set_checked(&loaded.dir, &changes, Some(&loaded.stamps))
            .map_err(|_| problems::internal("bench_write"))?;
        loaded.stamps = committed.stamps;
        Ok(())
    }

    /// For read capabilities outside the store (blueprint d-12): the graph the node references select (as every
    /// verb selects it, workshop 007 rule 6) and each given reference resolved to a node id in it, in order.
    pub fn resolve_nodes(
        &self,
        ctx: &Ctx,
        refs: &[Option<&str>],
    ) -> Result<(String, &Loaded, Vec<Option<Id>>), Problem> {
        let alias = self.select(ctx, refs)?;
        let l = self.get(&alias);
        let ids = refs
            .iter()
            .map(|r| r.map(|r| resolve_node_in(l, ctx, &alias, r)).transpose())
            .collect::<Result<_, _>>()?;
        Ok((alias, l, ids))
    }

    /// The graph a command acts on (workshop 007 rule 6): an address names its own graph (an explicit `--graph`
    /// that disagrees is refused), else `--graph`, else `UMX_GRAPH`, else the only loaded graph. `refs` are the
    /// node references the command was given. Choosing a graph counts as using it, which resets its idle unload.
    /// For a single-graph read that named no graph (no `--graph`, `UMX_GRAPH` or address) and took `alias`: the
    /// other copies of its graph, as copies_skipped (d-21). None when there are none or the read named its graph.
    pub(crate) fn copies_note(
        &self,
        ctx: &Ctx,
        refs: &[Option<&str>],
        alias: &str,
    ) -> Option<Problem> {
        let named = ctx.graph.is_some()
            || ctx.default_graph.is_some()
            || refs.iter().flatten().any(|r| split_address(r).0.is_some());
        let l = self.graphs.get(alias)?;
        let open = |a: &str| {
            self.effective_policy(a)
                .is_some_and(|p| p.visibility == umx_core::Visibility::Open)
        };
        if !open(alias) {
            return None;
        }
        let others: Vec<String> = self
            .copies_of(l.meta.id)
            .into_iter()
            .filter(|a| *a != alias && open(a))
            .map(|a| a.to_string())
            .collect();
        (!named && !others.is_empty()).then(|| problems::copies_skipped(&others))
    }

    pub(crate) fn select(&self, ctx: &Ctx, refs: &[Option<&str>]) -> Result<String, Problem> {
        let alias = self.choose(ctx, refs)?;
        self.graphs[&alias]
            .last_used
            .store(self.ports.clock.now_ms(), Ordering::Relaxed);
        Ok(alias)
    }

    /// [`Self::select`] for a write: the graph, once its files are as this load left them (F144).
    pub(crate) fn select_write(&self, ctx: &Ctx, refs: &[Option<&str>]) -> Result<String, Problem> {
        let alias = self.select(ctx, refs)?;
        self.files_unchanged(&alias)?;
        Ok(alias)
    }

    /// The files of `alias` that changed on disk since this load read them or its last write left them (F144), by a
    /// fresh look. A look that fails finds nothing here: the write's own set meets that failure and reports it,
    /// and a read answers as before.
    fn files_moved(&self, alias: &str) -> Vec<GraphFile> {
        let l = self.get(alias);
        self.ports
            .files
            .stamps(&l.dir)
            .map(|now| l.stamps.changed(&now))
            .unwrap_or_default()
    }

    /// The up-front check every write makes under the writer before it resolves anything (F144): refused with
    /// `graph_files_changed` when the graph's files changed on disk since this load, so nothing in memory moves.
    /// The files port checks again at the commit, so a change that lands after this look is refused there.
    /// How many graphs are loaded, copies included.
    pub(crate) fn loaded_count(&self) -> usize {
        self.graphs.len()
    }

    pub(crate) fn files_unchanged(&self, alias: &str) -> Result<(), Problem> {
        let moved = self.files_moved(alias);
        if moved.is_empty() {
            return Ok(());
        }
        Err(problems::graph_files_changed(
            &alias_name(alias),
            &self.get(alias).dir,
            &moved.into_iter().map(FileRef::from).collect::<Vec<_>>(),
        ))
    }

    /// `stale_files` for a read of `alias` when its files changed on disk since this load (F144): the read still
    /// answers from memory.
    pub(crate) fn stale_note(&self, alias: &str) -> Option<Problem> {
        let moved = self.files_moved(alias);
        (!moved.is_empty())
            .then(|| problems::stale_files(&alias_name(alias), &self.get(alias).dir, &moved))
    }

    /// [`Self::stale_note`] for each graph a read used.
    pub(crate) fn stale_notes<'a>(
        &self,
        aliases: impl IntoIterator<Item = &'a String>,
    ) -> Vec<Problem> {
        aliases
            .into_iter()
            .filter_map(|a| self.stale_note(a))
            .collect()
    }

    fn choose(&self, ctx: &Ctx, refs: &[Option<&str>]) -> Result<String, Problem> {
        if ctx.scope.is_some() {
            return Err(problems::usage_invalid(
                "this command reads or writes one graph (use traversal for a scope)",
                "scope",
            ));
        }
        if ctx.graph.as_deref().is_some_and(|g| g.contains(',')) {
            return Err(problems::usage_invalid(
                "this command reads or writes one graph (use --graph with one name)",
                "scope",
            ));
        }
        // ws006 S23: a former alias reads as its graph's live alias, before any other rule.
        let raw_address = self.addressed(ctx, refs)?;
        let address_alias = match raw_address {
            Some(selector)
                if !self.graphs.contains_key(&selector) && selector.parse::<Id>().is_ok() =>
            {
                Some(self.read_selector_alias(ctx, &selector)?)
            }
            Some(selector) => Some(self.canonical(ctx, &selector)?),
            None => None,
        };
        let graph = ctx
            .graph
            .as_deref()
            .map(|g| self.canonical(ctx, g))
            .transpose()?;
        if let (Some(a), Some(g)) = (address_alias.as_deref(), graph.as_deref())
            && a != g
        {
            return Err(problems::graph_mismatch(a, g));
        }
        // The default is read only when it is the selector used (review R15): an address or --graph wins, and a
        // default they override is never looked up, refused or hinted.
        let default = match (&address_alias, &graph, ctx.default_graph.as_deref()) {
            (None, None, Some(g)) => Some(self.canonical(ctx, g)?),
            _ => None,
        };
        if let Some(alias) = address_alias
            .as_deref()
            .or(graph.as_deref())
            .or(default.as_deref())
        {
            return if self.graphs.contains_key(alias) {
                Ok(alias.to_string())
            } else {
                Err(self.not_loaded(alias))
            };
        }
        // Copies of one graph and nothing else: the unnamed scope (d-20, then d-21) decides, as for query and search.
        // One open primary is read; secured copies are neither read nor named here, and fall through to
        // graph_ambiguous as any several loads do (reviewer R132-1).
        if let Some(first) = self.graphs.values().next()
            && self.graphs.len() > 1
            && self.graphs.values().all(|l| l.meta.id == first.meta.id)
        {
            let scope = self.unnamed_scope()?;
            if let [only] = scope.aliases.as_slice() {
                return Ok(only.clone());
            }
        }
        let mut aliases = self.graphs.keys();
        match (aliases.next(), aliases.next()) {
            (None, _) => Err(problems::graph_none_loaded()),
            (Some(only), None) => Ok(only.clone()),
            (Some(first), Some(_)) => Err(problems::graph_ambiguous(
                ctx.retry.as_ref(),
                &alias_name(first),
                self.graphs
                    .keys()
                    .map(|a| Candidate {
                        id: a.clone(),
                        title: None,
                        score: None,
                    })
                    .collect(),
            )),
        }
    }

    fn get(&self, alias: &str) -> &Loaded {
        &self.graphs[alias]
    }

    fn now(&self) -> String {
        umx_core::time::iso_from_ms(self.ports.clock.now_ms())
    }
}

/// A loaded graph's alias; aliases are checked when a graph is created or loaded.
pub(crate) fn alias_name(alias: &str) -> ValidName {
    ValidName::alias(alias).expect("loaded aliases are snake_case")
}

/// Names a node in a loaded graph by id, prefix or address.
pub(crate) fn resolve_node(loaded: &Loaded, alias: &str, text: &str) -> Result<Id, Problem> {
    resolve_in(&loaded.graph, alias, text)
}

pub(crate) fn resolve_node_in(
    loaded: &Loaded,
    ctx: &Ctx,
    alias: &str,
    text: &str,
) -> Result<Id, Problem> {
    resolve_in_scope(
        ctx.read_scope(&loaded.graph),
        &loaded.graph,
        alias,
        text,
        |_| true,
    )
}

pub(crate) fn resolve_read_node(
    loaded: &Loaded,
    ctx: &Ctx,
    alias: &str,
    text: &str,
) -> Result<Id, Problem> {
    let (selector, node) = split_address(text);
    if (selector.is_some() && node.is_empty()) || node.parse::<Id>().ok() == Some(loaded.meta.id) {
        Ok(loaded.meta.id)
    } else {
        resolve_node_in(loaded, ctx, alias, text)
    }
}

/// [`resolve_node`] over the graph itself, for a write that holds it (a batch).
fn resolve_in(graph: &Graph, alias: &str, text: &str) -> Result<Id, Problem> {
    resolve_in_with(graph, alias, text, |_| true)
}

fn resolve_in_with(
    graph: &Graph,
    alias: &str,
    text: &str,
    visible: impl Fn(Id) -> bool,
) -> Result<Id, Problem> {
    resolve_in_scope(ReadScope::new(graph), graph, alias, text, visible)
}

fn resolve_in_scope(
    scope: ReadScope<'_>,
    graph: &Graph,
    alias: &str,
    text: &str,
    visible: impl Fn(Id) -> bool,
) -> Result<Id, Problem> {
    let (_, id_text) = split_address(text);
    let candidates = |ids: &[Id]| -> Vec<Candidate> {
        ids.iter()
            .map(|id| Candidate {
                id: id.to_text(),
                title: graph.node(*id).map(|n| n.title().into()),
                score: None,
            })
            .collect()
    };
    let lookup = graph.lookup_where(id_text, |ix| {
        scope.contains_ix(ix) && visible(graph.node_at(ix).id())
    });
    match lookup {
        Lookup::Found(id) => Ok(id),
        Lookup::Ambiguous(ids) => {
            let prefix = IdText::new(id_text).expect("a prefix that matched is id text");
            Err(problems::id_ambiguous(&alias_name(alias), &prefix, &ids)
                .candidates(candidates(&ids)))
        }
        Lookup::NotFound(ids) => {
            Err(
                problems::node_not_found(id_text, &alias_name(alias), ids.first())
                    .candidates(candidates(&ids)),
            )
        }
    }
}

/// A graph file's bytes from memory; nodes.jsonl is the image, borrowed (review D2). `.gitignore` is a user file,
/// never encoded from memory: asking for it is a bug, reported as `internal` rather than a panic that would take the
/// daemon's writer down.
fn encode_parts<'g>(
    meta: &GraphMeta,
    graph: &'g Graph,
    file: GraphFile,
) -> Result<Cow<'g, [u8]>, Problem> {
    Ok(match file {
        GraphFile::Graph => Cow::Owned(encode_edge_meta(meta, graph)?),
        GraphFile::Nodes => Cow::Borrowed(graph.nodes_image()),
        GraphFile::Edges => Cow::Owned(encode_full_edges(meta, graph)?.edges),
        GraphFile::GitIgnore => return Err(problems::internal("encode_gitignore")),
    })
}

struct EncodedEdges {
    metadata: Vec<u8>,
    edges: Vec<u8>,
    umx: u32,
}

/// Validate the exact output image, including every retained line and every copy of a repeated id. The format and
/// the own-graph check come from the end columns and the kept lines (B3, s20); the file is scanned only when they
/// find an end naming this graph or a kept line that does not decode, so the error and its line are exact.
fn encode_full_edges(meta: &GraphMeta, graph: &Graph) -> Result<EncodedEdges, Problem> {
    let edges = graph.encode_edges();
    let Some(external) = graph::external_ends(graph, meta.id) else {
        return scan_full_edges(meta, edges);
    };
    let umx = meta.umx.max(meta.needed_format_for(external, false));
    let metadata = codec::encode_graph(&GraphMeta {
        umx,
        ..meta.clone()
    });
    Ok(EncodedEdges {
        metadata,
        edges,
        umx,
    })
}

/// [`encode_full_edges`] by scanning every line of `edges` for `_graph`: the test oracle, and the path that names
/// the exact line of an error.
fn scan_full_edges(meta: &GraphMeta, edges: Vec<u8>) -> Result<EncodedEdges, Problem> {
    let mut external = Vec::new();
    let mut line_numbers = Vec::new();
    for (line, bytes) in edges.split(|byte| *byte == b'\n').enumerate() {
        if graph::has_graph_key(bytes) {
            let decoded = codec::decode_edges(bytes).map_err(|error| {
                problems::graph_invalid(&codec::DecodeError {
                    line: line + 1,
                    ..error
                })
            })?;
            for edge in decoded
                .records
                .into_iter()
                .filter(umx_core::Edge::has_external_end)
            {
                external.push(edge);
                line_numbers.push(line + 1);
            }
        }
    }
    let umx = meta.umx.max(meta.needed_format_for_edges(&external));
    let metadata = codec::encode_graph_with_edges(meta, &external).map_err(|error| {
        let line = line_numbers
            .get(error.line.saturating_sub(1))
            .copied()
            .unwrap_or(error.line);
        problems::graph_invalid(&codec::DecodeError { line, ..error })
    })?;
    Ok(EncodedEdges {
        metadata,
        edges,
        umx,
    })
}

fn encode_edge_meta(meta: &GraphMeta, graph: &Graph) -> Result<Vec<u8>, Problem> {
    let external: Vec<_> = graph
        .edges()
        .filter(|edge| edge.from_graph().is_some() || edge.to_graph().is_some())
        .map(|edge| edge.to_edge())
        .collect();
    codec::encode_graph_with_edges(meta, &external).map_err(|error| problems::graph_invalid(&error))
}

/// Timing only for umx-bench (ws003 s25 phase 0): [`encode_full_edges`]'s edge bytes, graph.json bytes and format.
#[cfg(feature = "bench")]
pub fn bench_encode_full_edges(
    meta: &GraphMeta,
    graph: &Graph,
) -> Result<(Vec<u8>, Vec<u8>, u32), Problem> {
    encode_full_edges(meta, graph).map(|e| (e.edges, e.metadata, e.umx))
}

/// `meta` as the graph's nodes need it: `FORMAT_LOCATORS` once any node has a locator (ws006 S3, workshop 010; the max rule,
/// never lowered), written in the same set as the first locator line.
fn with_node_format(meta: &GraphMeta, graph: &Graph) -> GraphMeta {
    let mut meta = meta.clone();
    meta.umx = meta
        .umx
        .max(meta.needed_format_for(false, graph.has_locators()));
    meta
}

fn needed_format(meta: &GraphMeta, graph: &Graph) -> u32 {
    let external = graph
        .edges()
        .any(|e| e.from_graph().is_some() || e.to_graph().is_some());
    meta.umx
        .max(meta.needed_format_for(external, graph.has_locators()))
}

/// Writes `which` from memory as one set (workshop 008 section 4.1). An error means nothing changed on disk, so the
/// caller puts back the one record it changed (undo by record, section 4); no file is ever rewritten to undo. A set
/// that stands but did not finish on disk is kept and becomes a `write_not_durable` warning (ap-13): undoing it
/// would make memory and files disagree.
fn persist(
    files: &dyn GraphFiles,
    alias: &str,
    loaded: &mut Loaded,
    which: &[GraphFile],
    touched: Touched,
) -> Result<Vec<Problem>, Problem> {
    let written = persist_parts(
        files,
        alias,
        &loaded.dir,
        &loaded.meta,
        &loaded.graph,
        Commit {
            which,
            rewrite_edges: loaded.edges_normalized,
            feed: &mut loaded.feed,
            touched,
            stamps: &mut loaded.stamps,
            receipts: None,
        },
    )?;
    loaded.meta.umx = written.umx;
    loaded.edges_normalized = false;
    if which.contains(&GraphFile::Graph) {
        loaded.refresh_ontology();
    }
    Ok(written.warnings)
}

struct Persisted {
    warnings: Vec<Problem>,
    umx: u32,
}

/// What a write set commits: its files, and what it touched for the graph's [`Feed`].
struct Commit<'a> {
    which: &'a [GraphFile],
    /// Own-graph endpoint keys were normalized on load: the edge file is written too.
    rewrite_edges: bool,
    feed: &'a mut Feed,
    touched: Touched,
    /// The graph's stamps: the set expects them, and a set that stands replaces them (F144).
    stamps: &'a mut GraphStamps,
    /// A keyed batch's receipts file (ws003 s09), written in the same set so the receipt and the batch stand or fall
    /// together.
    receipts: Option<ReceiptsWrite<'a>>,
}

/// A keyed batch's `receipts.jsonl` (ws003 s09): its new bytes, and its stamp when the batch read it, which the set
/// checks as it checks the content files (graph-storage: a receipt lost between read and write runs a re-send twice).
#[derive(Clone, Copy)]
pub(crate) struct ReceiptsWrite<'a> {
    pub bytes: &'a [u8],
    pub read: FileStamp,
}

impl ReceiptsWrite<'_> {
    fn change(&self) -> Change<'_> {
        Change {
            file: FileRef::Receipts,
            bytes: Some(self.bytes),
        }
    }

    fn also(&self) -> [(FileRef, FileStamp); 1] {
        [(FileRef::Receipts, self.read)]
    }
}

/// A set's refusal because a file it checks changed: the receipts file alone is the keyed batch's own refusal (sent
/// again, it reads the file afresh); any content file is F144's.
fn changed_problem(alias: &str, dir: &GraphDir, changed: &[FileRef]) -> Problem {
    if changed.iter().all(|f| *f == FileRef::Receipts) {
        return problems::receipts_unreadable(
            &alias_name(alias),
            "it changed on disk while this batch ran; send the batch again",
        );
    }
    problems::graph_files_changed(&alias_name(alias), dir, changed)
}

/// A keyed batch that changed nothing still records its receipt (graph-storage Q4): a set of `receipts.jsonl` alone,
/// checked against the graph's stamps and the receipts file's own. It moves no feed, revision or embed queue.
fn persist_receipts(
    files: &dyn GraphFiles,
    alias: &str,
    dir: &GraphDir,
    stamps: &mut GraphStamps,
    receipts: &ReceiptsWrite<'_>,
) -> Result<Vec<Problem>, Problem> {
    let written =
        files.write_set_checked_also(dir, &[receipts.change()], Some(stamps), &receipts.also());
    match written {
        Ok(committed) => {
            *stamps = committed.stamps;
            Ok(match committed.written {
                Written::Durable => Vec::new(),
                Written::NotSynced(e) => alloc::vec![problems::receipts_not_durable(&e)],
            })
        }
        Err(FilesError::RecoveryConflict(conflicts)) => Err(problems::recovery_conflict(
            dir,
            &conflicts,
            Some(&alias_name(alias)),
        )),
        Err(FilesError::Changed(changed)) => Err(changed_problem(alias, dir, &changed)),
        Err(e) => Err(problems::files_error(&e, problems::receipts_write_failed)),
    }
}

/// [`persist`] from the parts of a loaded graph, for a write that holds its graph (a batch). Every write of a graph's
/// content files comes here, so here alone a write that stands moves the graph's revision on (ws009 S5a).
fn persist_parts(
    files: &dyn GraphFiles,
    alias: &str,
    dir: &GraphDir,
    meta: &GraphMeta,
    graph: &Graph,
    commit: Commit<'_>,
) -> Result<Persisted, Problem> {
    let Commit {
        which,
        rewrite_edges,
        feed,
        touched,
        stamps,
        receipts,
    } = commit;
    let had = meta.umx;
    let raised = with_node_format(meta, graph);
    let meta = &raised;
    let mut which = which.to_vec();
    if rewrite_edges && !which.contains(&GraphFile::Edges) {
        which.push(GraphFile::Edges);
    }
    let encoded = which
        .contains(&GraphFile::Edges)
        .then(|| encode_full_edges(meta, graph))
        .transpose()?;
    let umx = encoded
        .as_ref()
        .map_or_else(|| needed_format(meta, graph), |encoded| encoded.umx);
    if umx > had && !which.contains(&GraphFile::Graph) {
        which.push(GraphFile::Graph);
    }
    let (mut metadata, mut edges) = match encoded {
        Some(encoded) => (Some(encoded.metadata), Some(encoded.edges)),
        None => (None, None),
    };
    let bytes: Vec<Cow<'_, [u8]>> = which
        .iter()
        .map(|file| match file {
            GraphFile::Edges => edges
                .take()
                .map(Cow::Owned)
                .ok_or_else(|| problems::internal("edge_output")),
            GraphFile::Graph if metadata.is_some() => {
                Ok(Cow::Owned(metadata.take().expect("checked metadata")))
            }
            _ => encode_parts(meta, graph, *file),
        })
        .collect::<Result<_, _>>()?;
    let mut changes: Vec<Change<'_>> = which
        .iter()
        .zip(&bytes)
        .map(|(file, b)| Change {
            file: (*file).into(),
            bytes: Some(&**b),
        })
        .collect();
    let also = match &receipts {
        Some(r) => {
            changes.push(r.change());
            r.also().to_vec()
        }
        None => Vec::new(),
    };
    let written = files.write_set_checked_also(dir, &changes, Some(stamps), &also);
    if let Ok(committed) = &written {
        feed.commit(graph, touched);
        *stamps = committed.stamps;
    }
    match written.map(|c| c.written) {
        Ok(Written::Durable) => Ok(Persisted {
            warnings: Vec::new(),
            umx,
        }),
        Ok(Written::NotSynced(e)) => Ok(Persisted {
            warnings: alloc::vec![problems::write_not_durable(named(&which, &e), &e)],
            umx,
        }),
        Err(FilesError::RecoveryConflict(conflicts)) => Err(problems::recovery_conflict(
            dir,
            &conflicts,
            Some(&alias_name(alias)),
        )),
        Err(FilesError::Changed(changed)) => Err(changed_problem(alias, dir, &changed)),
        Err(e) => Err(problems::files_error(&e, |e| {
            problems::write_failed(named(&which, e), e)
        })),
    }
}

/// The file of a set that an adapter error names, else the first.
fn named(which: &[GraphFile], e: &FilesError) -> GraphFile {
    let text = e.to_string();
    which
        .iter()
        .copied()
        .find(|f| text.contains(f.file_name()))
        .unwrap_or(which[0])
}

/// Off-vocabulary count for `graph load`: nodes whose type, status or a field is not declared.
fn off_vocabulary(loaded: &Loaded) -> usize {
    let o = loaded.ontology();
    let on_vocab = |n: NodeRef| {
        o.type_decl(n.kind()).is_some_and(|t| {
            n.custom_field_names()
                .all(|f| t.fields.iter().any(|d| d.name == f))
        }) && o.has_status(n.status())
    };
    loaded
        .read_scope()
        .nodes()
        .filter(|n| !on_vocab(*n))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use umx_core::Ontology;
    use umx_core::model::FORMAT_VERSION;

    #[test]
    fn edge_persistence_rejects_an_own_graph_end_through_the_core_codec() {
        let meta = codec::decode_graph(include_bytes!(
            "../../tests/fixtures/own-graph-end/graph.json"
        ))
        .unwrap();
        let nodes = codec::decode_nodes(include_bytes!(
            "../../tests/fixtures/own-graph-end/nodes.jsonl"
        ))
        .unwrap()
        .records;
        let edges = codec::decode_edges(include_bytes!(
            "../../tests/fixtures/own-graph-end/edges.jsonl"
        ))
        .unwrap()
        .records;
        let graph = Graph::from_records(nodes, edges);
        let problem = encode_parts(&meta, &graph, GraphFile::Edges).unwrap_err();
        assert_eq!(problem.code, "graph_invalid");
        assert!(problem.slot("reason").unwrap().contains("names this graph"));
    }

    #[test]
    fn edge_persistence_validates_kept_lines_not_only_live_edges() {
        let meta = codec::decode_graph(include_bytes!(
            "../../tests/fixtures/own-graph-end/graph.json"
        ))
        .unwrap();
        let records = codec::decode_edges(include_bytes!(
            "../../tests/fixtures/own-graph-end/edges.jsonl"
        ))
        .unwrap()
        .records;
        let edge = &records[0];
        let mut line = codec::encode_edges([edge]);
        line.pop();
        let mut graph = Graph::new();
        graph
            .keep_lines(
                Vec::new(),
                alloc::vec![codec::KeptLine { id: edge.id, line }],
            )
            .unwrap();
        assert_eq!(graph.edge_count(), 0, "the only record is kept");
        let problem = encode_parts(&meta, &graph, GraphFile::Edges).unwrap_err();
        assert_eq!(problem.code, "graph_invalid");
        assert!(problem.slot("reason").unwrap().contains("names this graph"));
    }

    #[test]
    fn a_kept_external_record_also_requires_format_three() {
        let mut meta = codec::decode_graph(include_bytes!(
            "../../tests/fixtures/own-graph-end/graph.json"
        ))
        .unwrap();
        meta.umx = 1;
        let mut edge = codec::decode_edges(include_bytes!(
            "../../tests/fixtures/own-graph-end/edges.jsonl"
        ))
        .unwrap()
        .records
        .remove(0);
        edge.from_graph = Some(Id(90));
        edge.to_graph = Some(Id(91));
        let original = codec::encode_edges([&edge]);
        let mut line = original.clone();
        line.pop();
        let mut graph = Graph::new();
        graph
            .keep_lines(
                Vec::new(),
                alloc::vec![codec::KeptLine { id: edge.id, line }],
            )
            .unwrap();
        let encoded = encode_full_edges(&meta, &graph).unwrap();
        assert_eq!(encoded.umx, 3);
        assert_eq!(codec::decode_graph(&encoded.metadata).unwrap().umx, 3);
        assert_eq!(encoded.edges, original);
        assert_eq!(meta.umx, 1, "encoding does not commit metadata");
    }

    /// Security and graph-storage (plan 129): a provenance node or edge is left out of `removed` by the same
    /// filter as `added` and `edited`, when the write commits while it is still in the graph.
    #[test]
    fn the_change_feed_leaves_provenance_out_of_removed_too() {
        let node = |id: u128, kind: &str, key: Option<&str>| umx_core::Node {
            id: Id(id),
            kind: kind.into(),
            title: "title".into(),
            status: "open".into(),
            note: String::new(),
            evidence: Vec::new(),
            status_history: Vec::new(),
            created: "now".into(),
            updated: "now".into(),
            created_by: "t".into(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields: key
                .map(|k| [("prov_key".into(), umx_core::Value::String(k.into()))].into())
                .unwrap_or_default(),
        };
        let edge = |id: u128, from: u128, to: u128| umx_core::Edge {
            id: Id(id),
            kind: "supports".into(),
            from_graph: None,
            from: Id(from),
            to_graph: None,
            to: Id(to),
            created: "now".into(),
            created_by: "t".into(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: BTreeMap::new(),
        };
        let graph = Graph::from_records(
            alloc::vec![
                node(10, "problem", None),
                node(20, "problem", None),
                node(30, "session", Some("session/copilot/Abc_1234")),
            ],
            alloc::vec![edge(100, 10, 20), edge(101, 10, 30)],
        );
        assert!(graph.is_provenance(graph.node(Id(30)).unwrap().ix()));
        let mut touched = Touched::default();
        touched.nodes.removed.extend([Id(20), Id(30)]);
        touched.edges.removed.extend([Id(100), Id(101)]);
        let mut feed = Feed::default();
        feed.commit(&graph, touched);
        let (rev, fed) = feed.pending.pop_front().unwrap();
        assert_eq!(rev, 1);
        assert_eq!(fed.nodes.removed, [Id(20)].into());
        assert_eq!(fed.edges.removed, [Id(100)].into());
    }

    #[test]
    fn encoding_gitignore_is_an_internal_problem_not_a_panic() {
        let meta = GraphMeta {
            spines: Vec::new(),
            former_aliases: Vec::new(),
            former_paths: Vec::new(),
            umx: FORMAT_VERSION,
            id: Id(1),
            alias: "demo".into(),
            title: "demo".into(),
            created: "2026-11-10T00:00:00Z".into(),
            embedding: None,
            ontology: Ontology::default(),
        };
        let loaded = Loaded::new(
            GraphDir::parse("/g").unwrap(),
            meta,
            Ontology::default(),
            Graph::new(),
            0,
            GraphStamps::ABSENT,
        );
        let p = encode_parts(&loaded.meta, &loaded.graph, GraphFile::GitIgnore).unwrap_err();
        assert_eq!(
            (p.code, p.slot("code").as_deref()),
            ("internal", Some("encode_gitignore"))
        );
        assert!(encode_parts(&loaded.meta, &loaded.graph, GraphFile::Nodes).is_ok());
    }
}
