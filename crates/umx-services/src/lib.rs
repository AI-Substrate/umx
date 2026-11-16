//! Use cases over ports and umx-graph. Pure: no std (blueprint 001, d-3).
//!
//! Split by capability (d-12): `GraphStore` owns the loaded graphs and is the one write path to graph files;
//! `EmbeddingService` holds only the embedder and reaches graphs through the store's pub fns; `Services` is the
//! facade over both. Services return values and warnings; they never format output. Reads take `&self`, writes
//! `&mut self`; the daemon serialises writes through its one writer (d-7). Every write reaches the files before it returns, and a
//! failed write leaves memory and files as they were.
#![no_std]

extern crate alloc;

pub mod config;
pub mod cypher;
pub mod embedding;
mod graph_store;
mod graphfile;
mod hybrid;
mod pairs;
pub mod problems;
mod query;
mod service_agent;
mod traversal;
pub mod ui;

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use umx_core::ports::{Embedder, GraphDir, ServiceManager};
use umx_core::provenance::Actor;
use umx_core::value::SetValue;
use umx_core::{Command, GraphEmbedding, Id, Node, Ontology, Problem, ValidName};

pub use config::{
    ConfigImported, ConfigServices, ConfigSet, ConfigShown, ConfigUnset, RootAdded, RootRemoved,
    RootRow, Shown, Startup,
};
pub use cypher::{CypherRequest, QUERY_BACKSTOP_MS, QUERY_PERMITS, QueryPermit, QueryRows};
pub use embedding::{
    Applied, EmbedJob, EmbedNote, EmbedQueue, EmbeddingService, NoteKind, Pacer, Planned, Scan,
    Step, Ticket, Vectors, batch_tokens, batches, embed, estimate_tokens, settle, step,
};
pub use graph_store::ResourceShown;
pub use graph_store::{AlongInfo, AlongRequest, AlongSource, along_one_graph, reserved_spine_name};
pub use graph_store::{
    BatchDone, BatchLine, BatchOp, BatchOutcome, BatchRequest, BatchRow, BatchSentence, CheckRow,
    Checked, EdgeAdded, EdgeChanged, EdgeEditRequest, EdgeEvidence, EdgeListRequest, EdgeRow,
    FieldsAdded, GraphInfo, GraphLoaded, GraphNew, GraphSet, GraphStore, GraphUnloaded, Guide,
    GuideEntry, GuideExample, GuideField, GuideType, HistoryRow, IncomingIndex, ListedNode, Loaded,
    NodeAddRequest, NodeAdded, NodeEditRequest, NodeEdited, NodeListRequest, NodeNoteRequest,
    NodeNoted, NodeStatusRequest, OntologyAdded, OntologyRemoved, PolicyConfirmed, PolicyView,
    Ports, ReadEdge, ResolveStatus, Resolved, RoleSet, SecretAllowed, StatusMoved, Target,
};
pub use graph_store::{DeclaredSpine, SpineRemoved, SpineSet, SpinesShown};
pub use graph_store::{
    EntityDenied, EntityLinked, EntityMatch, EntityMember, EntityProposed, EntityShown,
    EntityUnlinked,
};
pub use graph_store::{EvidenceAdded, EvidenceListed, EvidenceRemoved, EvidenceRow, Redacted};
pub use graph_store::{FolderState, GraphForgotten, IndexRow, Moved, PrimarySet, Renamed};
pub use graph_store::{GraphKey, edge_secret_flags};
pub use graph_store::{
    GraphfileAdded, GraphfileListing, GraphfileMode, GraphfileRow, InTheWay, InTheWayKind,
    ScanRequest, ScanRow, Scanned, SpineCandidate, SpineHint, SpineHints, SpineLine, SpineProposal,
    SpineSource, Unplaced, WhyNot,
};
pub use graph_store::{LINT_PAGE, Lint, LintKind, LintRequest, LintRow, RangePair};
/// The title scores `node add` passes to `Graph::near_duplicates` (ws004 s30); umx-bench's ruler times the same call.
pub use graph_store::{NEAR_DUPLICATE, NEAR_DUPLICATE_OTHER_TYPE};
pub use graph_store::{NodeWho, StampState, WhoListed, WhoNode, WhoRecord, WhoRow, WhoStamp};
pub use graph_store::{NotReloaded, ReloadReport, Reloaded};
pub use graph_store::{Scope, ScopeInfo, ScopeMemberInfo, SelectedRow, SourceRoots, TITLE_CHARS};
/// A rebuild's snapshot and lists cross the daemon's threads (ws004 s09) as services types: the daemon names no
/// umx-graph type (architect F1).
pub use graph_store::{SimilarityJob, SimilarityResult};
/// Timing only for umx-bench; not a production entry point.
#[cfg(feature = "bench")]
pub use graph_store::{bench_encode_full_edges, upsert_provenance_session};
pub use graphfile::{GraphfileLoad, GraphfileLoadedEntry, GraphfileNotLoaded};
pub use hybrid::{
    Admitted, Fused, HybridHit, HybridResult, LEG_DEPTH, MatchedTerm, QueryFailed, QueryRunner,
    QueryVectors, RRF_K, Ranked, SearchPlan, VectorMatch, WHOLE_NODE, WORD_MAX_DF_SHARE, WordMatch,
    WordRanked, fuse, vector_candidates,
};
pub use pairs::{
    ModelSource, PAIRS_BACKSTOP_MS, PAIRS_DEFAULT_LIMIT, PAIRS_MAX_LIMIT, Pair, PairNode, PairRow,
    PairRows, PairsAsk, PairsFound, PairsJob, PairsMin, PairsPart, PairsResult, Skipped,
};
pub use query::{
    Direction, EdgeCell, HIT_LINKS, HitLink, HitLinks, NeighbourRow, NeighboursRequest, NodeCell,
    PathRequest, PathRow, QueryService, WalkRequest, WalkRow, Way,
};
pub use service_agent::ServiceAgent;

/// One loaded graph's row of `umx embeddings status`.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddingsStatusRow {
    pub alias: String,
    /// The vector space graph.json records; None until one is chosen.
    pub key: Option<umx_core::ports::ModelKey>,
    pub policy: PolicyView,
    pub nodes: usize,
    /// None when the graph has no model space yet and no embedder serves it.
    pub pending: Option<usize>,
    pub held: Option<usize>,
    /// Nodes with every vector current: nodes less pending and held.
    pub done: Option<usize>,
    /// The instance that embeds it now; None when none may (no embedder, the folder rule, a secured graph and only
    /// hosted instances, or no instance makes its recorded key).
    pub instance: Option<String>,
    pub locality: Option<umx_core::ports::Locality>,
    /// What the graph's `vectors_digest` is made from ([`Services::vectors_digest`]; ws004 s34b). None when the graph
    /// has no vector space, or when an unnamed read would not show it (security: a graph that is not open gets no
    /// digest).
    pub vectors_stamp: Option<VectorsStamp>,
}

/// What a graph's `vectors_digest` is a function of while it holds (ws004 s34b): the space its vectors are in
/// (`EmbeddingService::block_key`, the space `similarity pairs` compares), its load instance and its vectors'
/// generation. A caller keeps the digest for as long as the stamp is equal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorsStamp {
    pub key: umx_core::ports::ModelKey,
    pub instance: u64,
    pub generation: u64,
}

/// Digests by graph alias, each with the stamp it was made at ([`Services::vectors_digest_kept`]; ws004 s34b, s36).
/// A caller that keeps one behind a mutex takes it inside the services read hold, never before it: the lock order
/// is the services read hold, then the memo's mutex, everywhere.
pub type DigestMemo = alloc::collections::BTreeMap<String, (VectorsStamp, [u8; 32])>;

/// Who is asking (`by`, recorded on writes) and about which graph: `graph` is an explicit `--graph`,
/// `default_graph` is `UMX_GRAPH`. `retry` is the command rebuilt from its parsed form (names, ids and flags;
/// free text as placeholders), used for the `graph_ambiguous` next.
#[derive(Clone, Debug)]
pub struct Ctx {
    pub graph: Option<String>,
    pub scope: Option<Scope>,
    pub default_graph: Option<String>,
    pub by: String,
    pub retry: Option<Command>,
    pub actor: Actor,
    pub include_provenance: bool,
    /// The former aliases this request named and the live aliases umx used instead (ws006 S23), as (former, live):
    /// the daemon gives one `alias_redirected` hint for each. Shared by every clone of this context.
    pub redirected: Redirected,
}

/// See [`Ctx::redirected`].
pub type Redirected = alloc::rc::Rc<core::cell::RefCell<Vec<(String, String)>>>;

impl Ctx {
    pub fn new(by: impl Into<String>) -> Ctx {
        Ctx {
            graph: None,
            scope: None,
            default_graph: None,
            by: by.into(),
            retry: None,
            actor: Actor::unknown(),
            include_provenance: false,
            redirected: Redirected::default(),
        }
    }

    /// Notes that `former` was read as `live` (once per pair).
    pub fn redirect(&self, former: &str, live: &str) {
        let mut seen = self.redirected.borrow_mut();
        if !seen.iter().any(|(f, l)| f == former && l == live) {
            seen.push((former.to_string(), live.to_string()));
        }
    }

    pub fn graph(mut self, alias: impl Into<String>) -> Ctx {
        self.graph = Some(alias.into());
        self
    }

    pub fn actor(mut self, actor: Actor) -> Ctx {
        self.actor = actor;
        self
    }

    pub fn with_provenance(mut self, include: bool) -> Ctx {
        self.include_provenance = include;
        self
    }

    pub(crate) fn read_scope<'g>(&self, graph: &'g umx_graph::Graph) -> graph_store::ReadScope<'g> {
        graph_store::ReadScope::new(graph).with_provenance(self.include_provenance)
    }

    pub(crate) fn actor_display(&self) -> &str {
        let actor = self.actor.fields();
        actor
            .seat
            .as_deref()
            .or(actor.user.as_deref())
            .unwrap_or("unknown")
    }

    pub fn scope(mut self, scope: Scope) -> Ctx {
        self.scope = Some(scope);
        self
    }

    pub fn default_graph(mut self, alias: impl Into<String>) -> Ctx {
        self.default_graph = Some(alias.into());
        self
    }

    pub fn retry(mut self, cmd: Command) -> Ctx {
        self.retry = Some(cmd);
        self
    }

    /// `cmd --graph <alias>` when the command named a graph explicitly, so a fix runs against the same graph; a
    /// `--graph a,b` list stays a list (F73) when every part is an alias, up to the 64 names a list may carry.
    pub(crate) fn same_graph(&self, cmd: Command) -> Command {
        let graphs: Option<Vec<ValidName>> = self
            .graph
            .as_deref()
            .map(|g| g.split(',').map(ValidName::alias).collect())
            .unwrap_or(None);
        match graphs {
            Some(gs) if (1..=64).contains(&gs.len()) => cmd.word("--graph").names(&gs, ","),
            _ => cmd,
        }
    }
}

/// A successful result and its notes: the non-fatal problems it raised. The daemon puts each note in the reply's
/// warnings or hints by its catalogue kind (ws002 X1, architect topic:hints-envelope); services keep one list.
#[derive(Clone, Debug, PartialEq)]
pub struct Done<T> {
    pub value: T,
    pub notes: Vec<Problem>,
}

impl<T> Done<T> {
    fn ok(value: T) -> Done<T> {
        Done {
            value,
            notes: Vec::new(),
        }
    }
}

pub type Outcome<T> = Result<Done<T>, Problem>;

/// One page of a list.
#[derive(Clone, Debug, PartialEq)]
pub struct Page<T> {
    pub alias: String,
    pub rows: Vec<T>,
    /// Every row the request matches, or `None` when counting them would cost a full scan (a lazy walk that
    /// stopped early; workshop 007's list shape). `truncated` is always exact.
    pub total: Option<usize>,
    pub truncated: bool,
    /// Present on an explicitly scoped list; aliases line up with the page's rows.
    pub graphs: Option<PageGraphs>,
    /// The spine range the rows were kept by (`--along`, D71 step 12), with what it left out.
    pub along: Option<graph_store::AlongInfo>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageGraphs {
    pub rows: Vec<String>,
    pub copies: bool,
}

impl<T> Page<T> {
    /// The same page with each row turned into what the response carries.
    pub(crate) fn map<U>(self, f: impl FnMut(T) -> U) -> Page<U> {
        Page {
            alias: self.alias,
            rows: self.rows.into_iter().map(f).collect(),
            total: self.total,
            truncated: self.truncated,
            graphs: self.graphs,
            along: self.along,
        }
    }
}

fn page<T: Clone>(alias: &str, all: Vec<T>, limit: usize, offset: usize) -> Page<T> {
    let total = all.len();
    let rows: Vec<T> = all.into_iter().skip(offset).take(limit).collect();
    let truncated = offset + rows.len() < total;
    Page {
        alias: alias.to_string(),
        rows,
        total: Some(total),
        truncated,
        graphs: None,
        along: None,
    }
}

/// A page taken from rows produced lazily: it stops after `offset + limit + 1` rows, so `truncated` is exact and
/// `total` is known only when nothing is left.
fn lazy_page<T>(
    alias: &str,
    mut all: impl Iterator<Item = T>,
    limit: usize,
    offset: usize,
) -> Page<T> {
    let skipped = all.by_ref().take(offset).count();
    let rows: Vec<T> = all.by_ref().take(limit).collect();
    let truncated = all.next().is_some();
    Page {
        alias: alias.to_string(),
        total: (!truncated).then_some(skipped + rows.len()),
        rows,
        truncated,
        graphs: None,
        along: None,
    }
}

/// The display address of a node or edge (req-14). Uses the full id so it never becomes ambiguous.
pub fn address(alias: &str, id: Id) -> String {
    alloc::format!("umx:{alias}/{id}")
}

/// The service facade the daemon and compose hold (d-12): the graph store, the embedding service and, later, the
/// config service. Each verb delegates to the service that owns it; see that service for what it does.
pub struct Services {
    graph: GraphStore,
    embedding: EmbeddingService,
    service: ServiceAgent,
    /// Fetches pinned models for `umx models fetch` (req-74); never used by embed.
    model_fetcher: Option<Arc<dyn umx_core::ports::ModelFetcher>>,
    /// Nodes whose vectors may need work since the last `take_embed_jobs`; empty while there is no embedder.
    pending: Vec<EmbedJob>,
    /// Graphs whose stored similar_to lists did not match at load, to rebuild off the writer (workshop 008 section 9).
    similarity_rebuilds: Vec<String>,
    /// At most [`QUERY_PERMITS`] Cypher queries at once (graph-storage-expert F2).
    query_permits: cypher::QueryPermits,
}

/// What `umx similarity rebuild` did to one graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimilarityRebuilt {
    pub alias: String,
    /// The vector space the lists are in.
    pub key: umx_core::ports::ModelKey,
    pub k: usize,
    /// Ten-thousandths.
    pub floor: i32,
    /// Whether the lists' floor is the calibrated one, where it comes from and its calibration run (ws004 s26).
    pub floor_calibrated: bool,
    pub floor_source: umx_core::similarity::FloorSource,
    pub floor_run: Option<&'static str>,
    /// similar_to edges in the graph after the rebuild.
    pub edges: usize,
    /// similar_to files written (shards and `_meta.tsv`); 0 when they already matched.
    pub files_written: usize,
}

/// What `umx embeddings reembed` costs, or did, for one graph. No text, only counts and model keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reembed {
    pub alias: String,
    /// The model graph.json recorded before.
    pub from: Option<umx_core::ports::ModelKey>,
    /// The configured embedder's model, which the graph's vectors are made with.
    pub to: umx_core::ports::ModelKey,
    /// Whether the embedder keeps text on this machine.
    pub local: bool,
    /// Nodes and texts still to embed in `to`, and their estimated tokens (two bytes a token).
    pub nodes: usize,
    pub texts: usize,
    pub estimated_tokens: usize,
    pub dry_run: bool,
    /// Nodes queued for the embed worker (0 for a dry run).
    pub queued: usize,
}

/// A reembed worked out under the shared lock, for the writer to apply: the cost, and the nodes to queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReembedPlan {
    pub cost: Reembed,
    pub nodes: Vec<Id>,
}

/// What `umx embeddings repair` did to one graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repaired {
    pub alias: String,
    /// Nodes queued to embed (a missing, stale or conflicting vector).
    pub queued: usize,
    /// Nodes whose repeated or leftover lines were removed without embedding.
    pub tidied: usize,
    pub shards_written: usize,
    /// Lines for nodes not in the graph, kept.
    pub orphans: usize,
    /// Lines umx cannot read, kept.
    pub kept_lines: usize,
}

impl Services {
    /// (port id, adapter id) for each wired port, read from the adapters these services run over (E-1). A port
    /// with nothing wired (the embedder in real mode until a real adapter is configured) has no row.
    pub fn adapters(&self) -> Vec<(&'static str, &'static str)> {
        let mut rows = self.graph.adapters().to_vec();
        rows.extend(self.embedding.adapter_id().map(|id| ("embedder", id)));
        rows.extend(self.service.adapter_id().map(|id| ("service-manager", id)));
        rows.extend(
            self.model_fetcher
                .as_ref()
                .map(|f| ("model-fetcher", f.adapter_id())),
        );
        rows.extend(self.graph.config_files_id().map(|id| ("config-files", id)));
        rows
    }

    /// No embedder: embedding verbs report it unavailable until `with_embedder`.
    pub fn new(ports: Ports) -> Services {
        Services {
            graph: GraphStore::new(ports),
            embedding: EmbeddingService::new(None),
            service: ServiceAgent::new(None),
            model_fetcher: None,
            pending: Vec::new(),
            similarity_rebuilds: Vec::new(),
            query_permits: cypher::QueryPermits::new(QUERY_PERMITS),
        }
    }

    /// The daemon's per-user service (`umx daemon install`, req-86).
    /// The model fetcher `umx models fetch` uses (req-74).
    pub fn with_model_fetcher(
        mut self,
        fetcher: Arc<dyn umx_core::ports::ModelFetcher>,
    ) -> Services {
        self.model_fetcher = Some(fetcher);
        self
    }

    pub fn model_fetcher(&self) -> Option<&Arc<dyn umx_core::ports::ModelFetcher>> {
        self.model_fetcher.as_ref()
    }

    pub fn with_service_manager(mut self, manager: Arc<dyn ServiceManager>) -> Services {
        self.service = ServiceAgent::new(Some(manager));
        self
    }

    /// No service manager, and why (said by `umx daemon install` and doctor).
    pub fn without_service_manager(mut self, why: &'static str) -> Services {
        self.service = ServiceAgent::unavailable(why);
        self
    }

    pub fn service(&self) -> &ServiceAgent {
        &self.service
    }

    pub fn with_roots(mut self, roots: Vec<GraphDir>) -> Services {
        self.graph = self.graph.with_roots(roots);
        self
    }

    /// Where a repo's Graphfile is read and written (ws002 s17): compose wires the config files port here too.
    pub fn with_config_files(mut self, files: Arc<dyn umx_core::ports::ConfigFiles>) -> Services {
        self.graph = self.graph.with_config_files(files);
        self
    }

    /// Where `root:<name>` evidence sources point (req-47): the config service, in compose.
    pub fn with_source_roots(mut self, roots: Arc<dyn SourceRoots>) -> Services {
        self.graph = self.graph.with_source_roots(roots);
        self
    }

    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Services {
        self.embedding = EmbeddingService::new(Some(embedder));
        self
    }

    /// Swaps the wired embedder for another already-built one (or none) when `embedder.active` changes; never
    /// builds anything.
    pub fn set_embedder(&mut self, embedder: Option<Arc<dyn Embedder>>) {
        self.embedding = EmbeddingService::new(embedder);
    }

    /// Every built instance, `default` naming `embedder.active`'s (ws004 s17): each graph uses the one that makes
    /// its recorded space and that its policy allows. Never builds anything.
    pub fn set_instances(&mut self, instances: Vec<embedding::Instance>, default: Option<&str>) {
        self.embedding = EmbeddingService::with_instances(instances, default);
    }

    /// The machine config's floors by model key (`embedders.<name>.search_floor`, `similar_floor`), set when the
    /// daemon starts with the instances. A graph loaded after this uses them; lists made with another floor are stale.
    pub fn set_floors(&mut self, floors: umx_core::similarity::Floors) {
        self.graph.set_floors(floors);
    }

    /// Each instance's background pace in tokens a minute, by name (ws004 s25), fixed at start.
    pub fn set_pace(&mut self, pace: alloc::collections::BTreeMap<String, u64>) {
        let embedding = core::mem::replace(&mut self.embedding, EmbeddingService::new(None));
        self.embedding = embedding.with_pace(pace);
    }

    pub fn graph(&self) -> &GraphStore {
        &self.graph
    }

    pub fn embedding(&self) -> &EmbeddingService {
        &self.embedding
    }

    pub fn loaded(&self, alias: &str) -> Option<&Loaded> {
        self.graph.loaded(alias)
    }

    pub fn aliases(&self) -> impl Iterator<Item = &str> {
        self.graph.aliases()
    }

    /// Timing only for umx-bench: [`GraphStore::bench_write_edges`].
    #[cfg(feature = "bench")]
    pub fn bench_write_edges(&mut self, alias: &str, bytes: &[u8]) -> Result<(), Problem> {
        self.graph.bench_write_edges(alias, bytes)
    }

    pub fn graph_new(
        &mut self,
        ctx: &Ctx,
        path: &str,
        alias: &str,
        title: Option<&str>,
        empty: bool,
        embedding: Option<GraphEmbedding>,
    ) -> Outcome<GraphNew> {
        let mut done = self
            .graph
            .graph_new(ctx, path, alias, title, empty, embedding)?;
        let alias = done.value.graph.alias.clone();
        self.pending_from(&alias, None);
        self.load_similarity(&alias, &mut done.notes);
        self.settle_held(&alias, None);
        let loaded = self.graph.loaded(&alias);
        done.value.graph.vectors_pending = loaded.and_then(Loaded::pending_count);
        done.value.graph.vectors_held = loaded.and_then(Loaded::held_count);
        done.notes
            .extend(self.embedding.graph_warnings(&self.graph, &alias));
        Ok(done)
    }

    /// `graph new` with the repo Graphfile (ws002 s17): as [`Services::graph_new`] (which adds nothing to a
    /// Graphfile), then the new graph's line added per `mode`. With no config files port wired it refuses before
    /// making anything (architect, s17).
    #[allow(clippy::too_many_arguments)]
    pub fn graph_new_with(
        &mut self,
        ctx: &Ctx,
        path: &str,
        alias: &str,
        title: Option<&str>,
        empty: bool,
        embedding: Option<GraphEmbedding>,
        mode: GraphfileMode,
    ) -> Outcome<GraphNew> {
        if mode != GraphfileMode::Skip {
            self.graph.graphfile_wired()?;
        }
        let mut done = self.graph_new(ctx, path, alias, title, empty, embedding)?;
        let (added, warnings) = self.graph.graphfile_add_new(&done.value.graph.alias, mode);
        done.value.graphfile = added;
        done.notes.extend(warnings);
        Ok(done)
    }

    /// `graph scan` (ws002 s17): see [`GraphStore::graph_scan`].
    pub fn graph_scan(&self, ctx: &Ctx, folder: &str, req: &ScanRequest) -> Outcome<Scanned> {
        self.graph.graph_scan(ctx.retry.as_ref(), folder, req)
    }

    /// `graph list --graphfile` (ws002 s17).
    pub fn graph_list_graphfile(&self, folder: &str) -> Result<GraphfileListing, Problem> {
        self.graph.graph_list_graphfile(folder)
    }

    /// Loads a graph, then reads its vector shards once: with an embedder, nodes with a missing or stale vector are
    /// queued (a stale vector stays in the file, and searchable, until its fresh one is written); with or without
    /// one, lines for unknown nodes or lines umx cannot read are kept and warned. A graph whose vectors are all current
    /// queues nothing, so a cloned graph loads with zero embeds (req-36).
    pub fn graph_load(
        &mut self,
        ctx: &Ctx,
        path: &str,
        as_alias: Option<&str>,
    ) -> Outcome<GraphLoaded> {
        let mut done = self.load(ctx, path, as_alias, None)?;
        let (alias, replaced) = (done.value.graph.alias.clone(), done.value.replaced);
        // ws006 S23: a graph this machine knew at a folder that no longer holds it was moved by hand.
        if !replaced && let Some((from, stale, total)) = self.graph.heal_moved(&alias) {
            done.value.moved_from = Some(from);
            done.value.stale = stale;
            done.value.stale_total = total;
        }
        done.notes.extend(self.graph.remember(&alias, replaced));
        Ok(done)
    }

    /// Loads every graph this machine's registry lists as loaded, in load order, through the same path as `graph
    /// load` (F72; security: nothing in the registry stands in for a check). Run once when the daemon starts; a graph
    /// that fails stays unloaded and is reported, never fatal.
    /// `refused`: a code every row fails with, unloaded (`config_invalid` while an invalid config closes every folder).
    pub fn reload_remembered(
        &mut self,
        ctx: &Ctx,
        refused: Option<&'static str>,
    ) -> &graph_store::ReloadReport {
        for mut row in self.graph.reload_rows() {
            let result = match refused {
                Some(code) => Err(code),
                None => self
                    .load(ctx, &row.folder, Some(&row.alias), Some(row.id))
                    .map(|done| {
                        // ws006 S23 (review R6): a rename cut short before the registry loads under graph.json's
                        // alias; the row is recorded, and refreshed, under the alias it loaded as.
                        row.alias = done.value.graph.alias.clone();
                        done.notes.iter().map(|w| w.code).collect()
                    })
                    .map_err(|p| p.code),
            };
            self.graph.record_reload(&row, result);
        }
        // F80: while every folder is closed nothing came back, so the marks are kept for the next start.
        if refused.is_none() {
            self.graph.refresh_reloaded();
            self.graph.restore_no_primary();
        }
        self.graph.reload_report()
    }

    /// `graph load <path>`, or for a bare alias (`by_alias`), the graph this machine's index knows by it (req-116).
    /// The folder `path` names wins, so a folder that holds a graph loads as before. Only when it holds none (or is
    /// outside the roots) is the index asked: one folder with the alias loads through the same path, expecting its
    /// remembered graph id; two or more refuse with alias_ambiguous (graph-addressing-expert, plan 098); none gives
    /// the folder's own error.
    pub fn graph_load_by(
        &mut self,
        ctx: &Ctx,
        path: &str,
        as_alias: Option<&str>,
        by_alias: Option<&str>,
    ) -> Outcome<GraphLoaded> {
        let missed = match self.graph_load(ctx, path, as_alias) {
            Err(p) if matches!(p.code, "graph_not_found" | "path_outside_roots") => p,
            done => return done,
        };
        let Some(alias) = by_alias.and_then(ValidName::alias) else {
            return Err(missed);
        };
        let row = match self.graph.known(alias.as_str()) {
            graph_store::Known::None => {
                // ws006 S23 (review R17): a former alias of a loaded graph loads that graph's folder again, under its
                // live alias. The folder and the index come first, as before.
                let live = self.graph.canonical(ctx, alias.as_str())?;
                let Some(dir) = (live != alias.as_str())
                    .then(|| self.graph.loaded(&live).map(|l| l.dir.as_str().to_string()))
                    .flatten()
                else {
                    return Err(missed);
                };
                return self.graph_load(ctx, &dir, Some(as_alias.unwrap_or(&live)));
            }
            graph_store::Known::Many(rows) => return Err(self.graph.ambiguous(&alias, &rows)),
            graph_store::Known::One(row) => row,
        };
        let name = as_alias.unwrap_or(alias.as_str());
        let mut done = self.load(ctx, &row.folder, Some(name), Some(row.id))?;
        let (loaded, replaced) = (done.value.graph.alias.clone(), done.value.replaced);
        done.notes.extend(self.graph.remember(&loaded, replaced));
        Ok(done)
    }

    /// `graph forget <alias>` (req-116).
    pub fn graph_forget(&mut self, ctx: &Ctx, alias: &str) -> Outcome<GraphForgotten> {
        self.graph.graph_forget(ctx, alias)
    }

    /// `graph list --all` (req-116), each open graph's repository found through `config`'s files port.
    pub fn graph_index(&self, config: Option<&ConfigServices>) -> Done<Vec<IndexRow>> {
        self.graph
            .graph_index(&|dir| config.and_then(|c| c.repo_root(dir)))
    }

    pub fn graph_index_in(
        &self,
        ctx: &Ctx,
        config: Option<&ConfigServices>,
    ) -> Done<Vec<IndexRow>> {
        let mut done = self.graph_index(config);
        let counts = self.graph_list_in(ctx);
        for row in &mut done.value {
            if let Some(info) = counts.iter().find(|info| {
                row.loaded
                    .as_ref()
                    .is_some_and(|loaded| loaded.alias == info.alias)
            }) {
                row.nodes = Some(info.nodes as u64);
                row.edges = Some(info.edges as u64);
                row.loaded = Some(info.clone());
            }
        }
        done
    }

    /// Loads a graph this machine remembers as loaded but that is not in memory, by alias, through the same path as
    /// `graph load` (req-15, F72): for the daemon's on-demand load and for `umx resolve`. Ok(None) when the alias is
    /// already loaded (a second miss on one alias loads nothing). Errors: graph_not_loaded when the registry has no
    /// such loaded row (an explicitly unloaded one is never loaded here), or the load's own error.
    pub fn load_registered(&mut self, ctx: &Ctx, alias: &str) -> Outcome<Option<GraphLoaded>> {
        if self.graph.loaded(alias).is_some() {
            return Ok(Done::ok(None));
        }
        let Some(row) = self.graph.remembered_loaded(alias) else {
            return Err(self.graph.not_loaded(alias));
        };
        let done = self.load_row(ctx, &row)?;
        Ok(Done {
            value: Some(done.value),
            notes: done.notes,
        })
    }

    /// F118: loads every graph of a `--graph a,b` list that is not in memory but the registry remembers as loaded, for a
    /// list read that missed one. As F99 does for a scope, the plan is decided for the whole list first, under the
    /// caller's write lock: a listed graph unloaded on purpose (not remembered) refuses it with `graph_not_loaded`
    /// before any graph loads. `aliases` is the whole list, not the graphs that were missing when the read missed: one
    /// loaded then may have been unloaded on purpose since, and must refuse the plan too (review R257-B1). A list
    /// names its graphs, so secured ones follow the F72 rules as `--graph x` does. A load that fails partway through
    /// the plan returns its error and leaves the graphs loaded before it in memory, as [`Self::load_scope`] does:
    /// each is a whole, valid load the registry remembers, and the read is refused. Returns how many loaded.
    pub fn load_registered_all(&mut self, ctx: &Ctx, aliases: &[String]) -> Outcome<usize> {
        let mut rows = Vec::new();
        for alias in aliases {
            if self.graph.loaded(alias).is_some() {
                continue;
            }
            let Some(row) = self.graph.remembered_loaded(alias) else {
                return Err(self.graph.not_loaded(alias));
            };
            rows.push(row);
        }
        let mut warnings = Vec::new();
        for row in &rows {
            warnings.extend(self.load_row(ctx, row)?.notes);
        }
        Ok(Done {
            value: rows.len(),
            notes: warnings,
        })
    }

    /// F99: loads the members of saved scope `name` that the registry remembers as loaded, for a `--scope` read that
    /// missed them. The plan is decided for the whole scope first, by graph id, under the caller's write lock (the
    /// daemon's writer), so a member unloaded on purpose since the miss refuses the plan with `scope_graph_not_loaded`
    /// before any member loads (review R213-B1). Each load expects its row's graph id. Returns how many loaded.
    pub fn load_scope(&mut self, ctx: &Ctx, name: &str) -> Outcome<usize> {
        let plan = self.graph.scope_load_plan(name)?;
        let mut warnings = Vec::new();
        for row in &plan {
            warnings.extend(self.load_row(ctx, row)?.notes);
        }
        Ok(Done {
            value: plan.len(),
            notes: warnings,
        })
    }

    /// Loads a remembered registry row through the `graph load` path, expecting its graph id, and says it loaded on
    /// demand. Its load order is now the latest, so its row moves to the end (d-21's primary survives a restart).
    fn load_row(&mut self, ctx: &Ctx, row: &umx_core::registry::Row) -> Outcome<GraphLoaded> {
        let mut done = self.load(ctx, &row.folder, Some(&row.alias), Some(row.id))?;
        let alias = done.value.graph.alias.clone();
        done.notes.extend(self.graph.remember(&alias, false));
        if let Some(l) = self.graph.loaded(&alias) {
            let name = graph_store::alias_name(&alias);
            done.notes
                .insert(0, problems::graph_loaded_on_demand(&name, &l.dir));
        }
        Ok(done)
    }

    /// [`Self::graph_load`] without the registry; with `expect`, a folder holding another graph id is refused.
    fn load(
        &mut self,
        ctx: &Ctx,
        path: &str,
        as_alias: Option<&str>,
        expect: Option<umx_core::Id>,
    ) -> Outcome<GraphLoaded> {
        let mut done = self
            .graph
            .graph_load_expecting(ctx, path, as_alias, expect)?;
        let alias = done.value.graph.alias.clone();
        // A scan that fails is a warning, never a clean load (reviewer R60-2): the graph is usable, its vectors
        // are not known, so nothing is queued from them.
        let scanned = self.embedding.scan(&mut self.graph, &alias);
        if let Err(cause) = &scanned {
            done.notes.push(problems::vectors_unreadable(
                &graph_store::alias_name(&alias),
                cause,
            ));
        }
        match &scanned {
            Ok(scan) => self.pending_from(&alias, Some(scan)),
            // Unknown, not zero: the shards could not be read.
            Err(_) => self.graph.set_vectors_pending(&alias, None),
        }
        self.settle_held(&alias, None);
        let loaded = self.graph.loaded(&alias);
        done.value.graph.vectors_pending = loaded.and_then(Loaded::pending_count);
        done.value.graph.vectors_held = loaded.and_then(Loaded::held_count);
        if scanned.is_ok() {
            self.load_similarity(&alias, &mut done.notes);
        }
        if let Ok(scan) = scanned {
            self.queue(&alias, scan.to_embed.iter().chain(&scan.to_tidy).copied());
            let mut warnings =
                EmbeddingService::scan_warnings(&graph_store::alias_name(&alias), &scan);
            if !scan.outdated.is_empty() {
                done.notes.push(problems::text_form_outdated(
                    &graph_store::alias_name(&alias),
                    scan.outdated.len(),
                ));
            }
            let hidden = warnings
                .len()
                .saturating_sub(graph_store::MAX_LOAD_WARNINGS);
            warnings.truncate(graph_store::MAX_LOAD_WARNINGS);
            if hidden > 0 {
                warnings.push(problems::load_warnings_not_shown(
                    &graph_store::alias_name(&alias),
                    hidden,
                ));
            }
            done.notes.extend(warnings);
        }
        done.notes
            .extend(self.embedding.graph_warnings(&self.graph, &alias));
        Ok(done)
    }

    /// The loaded graph `alias`, read only (its vector blocks included), or `None` when it is not loaded. Tests only:
    /// no edge reaches the graph (architect).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn loaded_graph(&self, alias: &str) -> Option<&umx_graph::Graph> {
        self.graph.loaded(alias).map(|l| &l.graph)
    }

    /// Virtual identity comes from the loaded graph's reserved id, never from stored fields.
    pub fn is_virtual_graph_node(&self, alias: &str, id: Id) -> bool {
        self.graph
            .loaded(alias)
            .is_some_and(|loaded| loaded.meta.id == id)
    }

    /// Reads `alias`'s stored similar_to lists for its block space (the key graph.json records, else the embedder's)
    /// with its settings; a setting out of range warns and uses the default; lists that do not match the vectors are
    /// queued for a rebuild and warn `similar_to_incomplete` (workshop 008 section 9).
    fn load_similarity(&mut self, alias: &str, warnings: &mut Vec<Problem>) {
        let (Some(key), Some(policy)) = (
            self.embedding.block_key(&self.graph, alias),
            self.graph.effective_policy(alias),
        ) else {
            return;
        };
        let raw = self
            .graph
            .loaded(alias)
            .and_then(|l| l.meta.embedding.as_ref())
            .and_then(|e| e.similarity.clone());
        let (settings, bad) =
            umx_core::similar::Settings::of(&key, raw.as_ref(), self.graph.floors());
        // Only a graph holding vectors is cut by a floor; one with none yet says nothing. Each use is judged by the
        // floor it will use: the machine config's, graph.json's for similar_to, else the calibrated table's.
        let holds_vectors = self.graph.loaded(alias).is_some_and(|l| {
            let space = key.to_string();
            l.graph.whole_block(&space).is_some() || l.graph.field_blocks(&space).next().is_some()
        });
        let search = self
            .graph
            .floors()
            .get(&key, umx_core::similarity::Use::Search)
            .source;
        if holds_vectors
            && let Some(uses) =
                umx_core::similarity::uses_without_floor(&key, search, settings.source)
        {
            warnings.push(problems::floor_uncalibrated(&key, uses));
        }
        let name = graph_store::alias_name(alias);
        for b in bad {
            let setting = match b {
                umx_core::similar::BadSetting::K => "k",
                umx_core::similar::BadSetting::Floor => "floor",
                umx_core::similar::BadSetting::Section => "similarity",
            };
            warnings.push(problems::similarity_setting_invalid(&name, setting));
        }
        if self.graph.load_similar(alias, policy, &key, settings)
            == graph_store::SimilarLoaded::Stale
        {
            warnings.push(problems::similar_to_incomplete(&name));
            if !self.similarity_rebuilds.iter().any(|a| a == alias) {
                self.similarity_rebuilds.push(alias.to_string());
            }
        }
    }

    /// Graphs whose similar_to lists await a rebuild since the last call, for the daemon to rebuild off the writer.
    pub fn take_similarity_rebuilds(&mut self) -> Vec<String> {
        core::mem::take(&mut self.similarity_rebuilds)
    }

    /// Whether `alias`'s similar_to lists await a rebuild (its similar_to results are incomplete meanwhile).
    pub fn similarity_stale(&self, alias: &str) -> bool {
        self.graph.similar_stale(alias)
    }

    /// Step 1 of a rebuild, under a short read lock: a copy of `alias`'s whole-node block and its list settings.
    pub fn similarity_snapshot(&self, alias: &str) -> Option<SimilarityJob> {
        self.graph.similar_snapshot(alias)
    }

    /// Step 3 of a rebuild, under the writer: puts the computed lists in place and writes the files. `Ok(None)` when
    /// the graph was loaded again, or its vectors or list settings changed, after the snapshot (take a new one).
    pub fn similarity_apply(
        &mut self,
        alias: &str,
        lists: SimilarityResult,
    ) -> Result<Option<(usize, Vec<Problem>)>, Problem> {
        let Some(policy) = self.graph.effective_policy(alias) else {
            return Ok(None);
        };
        self.graph.apply_similar_rebuild(alias, policy, lists)
    }

    /// Step 1 of `umx similarity rebuild` for the daemon, under the shared lock: the graph `ctx` selects and a copy of
    /// its whole-node block. `None` when its lists are not set up for its block space yet (the writer then runs
    /// [`Services::similarity_rebuild`], which sets them up) or it has no vectors.
    pub fn similarity_rebuild_start(
        &self,
        ctx: &Ctx,
    ) -> Result<(String, Option<SimilarityJob>), Problem> {
        let alias = self.graph.select(ctx, &[])?;
        let Some(key) = self.embedding.block_key(&self.graph, &alias) else {
            return Err(problems::embedder_not_configured());
        };
        let set_up = self
            .graph
            .loaded(&alias)
            .is_some_and(|l| l.graph.similar_space() == Some(&key.to_string()));
        let snapshot = if set_up {
            self.graph.similar_snapshot(&alias)
        } else {
            None
        };
        Ok((alias, snapshot))
    }

    /// The model, k and floor (ten-thousandths) of `alias`'s stored similar_to lists, for a similar_to result to
    /// carry (workshop 008 section 9). None when the graph holds no lists.
    pub fn similarity_info(&self, alias: &str) -> Option<(umx_core::ports::ModelKey, usize, i32)> {
        let graph = &self.graph.loaded(alias)?.graph;
        let key = umx_core::ports::ModelKey::parse(graph.similar_space()?).ok()?;
        let (k, floor) = graph.similar_settings();
        Some((key, k, floor))
    }

    /// Where the similar_to floor `floor` (ten-thousandths) of `alias`'s lists in `key` comes from, and its calibration
    /// run: graph.json's, the machine config's, the calibrated table's or none (ws004 s26). `calibrated` is true only
    /// when the lists were made with the calibrated floor (lists made with another are stale and being rebuilt).
    pub fn similar_floor(
        &self,
        alias: &str,
        key: &umx_core::ports::ModelKey,
        floor: i32,
    ) -> (
        bool,
        umx_core::similarity::FloorSource,
        Option<&'static str>,
    ) {
        let raw = self
            .graph
            .loaded(alias)
            .and_then(|l| l.meta.embedding.as_ref())
            .and_then(|e| e.similarity.as_ref());
        let (settings, _) = umx_core::similar::Settings::of(key, raw, self.graph.floors());
        (
            settings.calibrated() && settings.floor == floor,
            settings.source,
            settings.run,
        )
    }

    /// What a rebuild of `alias` left: its space, settings and edge count, with `files_written`.
    pub fn similarity_summary(
        &self,
        alias: &str,
        files_written: usize,
    ) -> Option<SimilarityRebuilt> {
        let graph = &self.graph.loaded(alias)?.graph;
        let key = umx_core::ports::ModelKey::parse(graph.similar_space()?).ok()?;
        let (k, floor) = graph.similar_settings();
        let (floor_calibrated, floor_source, floor_run) = self.similar_floor(alias, &key, floor);
        Some(SimilarityRebuilt {
            alias: alias.to_string(),
            key,
            k,
            floor,
            floor_calibrated,
            floor_source,
            floor_run,
            edges: graph.nodes().map(|n| graph.similar_out(n.ix()).len()).sum(),
            files_written,
        })
    }

    /// `umx similarity rebuild` in one call (tests, and a caller holding the writer throughout): recomputes every
    /// similar_to list of the selected graph from its committed vectors and writes the files, which then equal what
    /// the lists kept current would have written, byte for byte (req-65).
    pub fn similarity_rebuild(&mut self, ctx: &Ctx) -> Outcome<SimilarityRebuilt> {
        let alias = self.graph.select(ctx, &[])?;
        let Some(key) = self.embedding.block_key(&self.graph, &alias) else {
            return Err(problems::embedder_not_configured());
        };
        let mut warnings = Vec::new();
        if self
            .graph
            .loaded(&alias)
            .is_some_and(|l| l.graph.similar_space() != Some(&key.to_string()))
        {
            self.load_similarity(&alias, &mut warnings);
            warnings.retain(|p| p.code != "similar_to_incomplete");
        }
        self.similarity_rebuilds.retain(|a| *a != alias);
        let files_written = match self.graph.similar_snapshot(&alias) {
            Some(job) => match self.similarity_apply(&alias, job.compute())? {
                Some((written, w)) => {
                    warnings.extend(w);
                    written
                }
                None => 0,
            },
            None => 0,
        };
        let value = self
            .similarity_summary(&alias, files_written)
            .unwrap_or(SimilarityRebuilt {
                alias,
                key,
                k: 0,
                floor: 0,
                floor_calibrated: false,
                floor_source: umx_core::similarity::FloorSource::None,
                floor_run: None,
                edges: 0,
                files_written,
            });
        Ok(Done {
            value,
            notes: warnings,
        })
    }

    /// A query's vector for searching `alias`; see [`EmbeddingService::embed_query`].
    pub fn embed_query(
        &self,
        alias: &str,
        text: &str,
    ) -> Result<Option<Vec<f32>>, umx_core::ports::EmbedError> {
        self.embedding.embed_query(&self.graph, alias, text)
    }

    /// See [`GraphStore::effective_policy`].
    pub fn effective_policy(&self, alias: &str) -> Option<umx_core::Policy> {
        self.graph.effective_policy(alias)
    }

    pub fn confirm_policy(
        &mut self,
        ctx: &Ctx,
        alias: &str,
        typed: Option<&str>,
    ) -> Outcome<PolicyConfirmed> {
        let before = self.policies();
        let mut done = self.graph.confirm_policy(ctx, alias, typed)?;
        done.notes.extend(self.recount_changed(&before));
        done.notes
            .extend(self.embedding.graph_warnings(&self.graph, alias));
        Ok(done)
    }

    pub fn graph_list(&self) -> Vec<GraphInfo> {
        self.graph.graph_list()
    }

    pub fn graph_list_in(&self, ctx: &Ctx) -> Vec<GraphInfo> {
        self.graph.graph_list_in(ctx)
    }

    /// `umx embeddings status` (ws004 s27): per loaded graph, its recorded key, its vectors pending, held and done,
    /// and the instance it embeds with now (s17 routing; `usable` is the daemon's folder rule). Reads only what the
    /// daemon holds: no provider call, no credential read.
    pub fn embeddings_status(
        &self,
        usable: &dyn Fn(&GraphDir, &str) -> bool,
    ) -> Vec<EmbeddingsStatusRow> {
        self.graph
            .graph_list()
            .into_iter()
            .map(|g| {
                let instance = self.graph.loaded(&g.alias).and_then(|l| {
                    self.embedding
                        .embedder_for_where(&self.graph, &g.alias, &|n| usable(&l.dir, n))
                });
                let done = match (g.vectors_pending, g.vectors_held) {
                    (Some(p), held) => Some(g.nodes.saturating_sub(p + held.unwrap_or(0))),
                    (None, _) => None,
                };
                EmbeddingsStatusRow {
                    key: g.embedding.as_ref().and_then(|e| e.key.clone()),
                    nodes: g.nodes,
                    pending: g.vectors_pending,
                    held: g.vectors_held,
                    done,
                    vectors_stamp: self.vectors_stamp(&g.alias),
                    locality: instance.as_ref().map(|i| i.embedder.locality()),
                    instance: instance.map(|i| i.name),
                    policy: g.policy,
                    alias: g.alias,
                }
            })
            .collect()
    }

    /// `alias`'s [`VectorsStamp`], None when it has no vector space or is not open (security, ws004 s34b).
    fn vectors_stamp(&self, alias: &str) -> Option<VectorsStamp> {
        if !self.graph.shown_unnamed(alias) {
            return None;
        }
        let key = self.embedding.block_key(&self.graph, alias)?;
        let (instance, generation) = self.graph.vectors_stamp(alias)?;
        Some(VectorsStamp {
            key,
            instance,
            generation,
        })
    }

    /// `alias`'s `vectors_digest` at `stamp` (ws004 s34b, `umx_core::similar::vectors_digest`): a cache key that moves
    /// when its stored vectors' space, nodes or text hashes do, made from the vectors in memory, never from current
    /// text. O(nodes), so callers keep it by its stamp. None when the graph is not loaded.
    pub fn vectors_digest(&self, alias: &str, stamp: &VectorsStamp) -> Option<[u8; 32]> {
        self.graph.vectors_digest(alias, &stamp.key)
    }

    /// [`Self::vectors_digest`] through `memo`: the kept digest while `alias`'s entry was made at `stamp`, else hashed
    /// and kept. The one source of `embeddings status`' and `similarity pairs`' digests (ws004 s34b, s36); the caller
    /// holds the read hold its stamp was taken under, so the digest is of the vectors that stamp names.
    pub fn vectors_digest_kept(
        &self,
        alias: &str,
        stamp: &VectorsStamp,
        memo: &mut DigestMemo,
    ) -> Option<[u8; 32]> {
        match memo.get(alias) {
            Some((at, digest)) if at == stamp => Some(*digest),
            _ => {
                let digest = self.vectors_digest(alias, stamp)?;
                memo.insert(alias.to_string(), (stamp.clone(), digest));
                Some(digest)
            }
        }
    }

    /// `umx check`, with `--lint` (F139) when `lint` is given.
    pub fn check(&self, ctx: &Ctx, lint: Option<&LintRequest>) -> Outcome<Checked> {
        self.graph.check(ctx, lint)
    }

    pub fn graph_unload(&mut self, ctx: &Ctx, alias: &str) -> Outcome<GraphUnloaded> {
        self.graph.graph_unload(ctx, alias)
    }

    /// What a graph name a request gave means (ws006 S23): a loaded alias as it is, a former alias as its graph's
    /// live alias (recorded on `ctx` for the `alias_redirected` hint).
    pub fn canonical(&self, ctx: &Ctx, name: &str) -> Result<String, umx_core::Problem> {
        self.graph.canonical(ctx, name)
    }

    /// `umx graph rename <old> <new>` (ws006 S23, req-56).
    /// Work queued for the graph moves to its new alias, and work already taken under the old one (dropped when it
    /// comes back, as its alias is gone) is queued again: the nodes still pending and a similar_to rebuild still
    /// owed (review R8).
    pub fn graph_rename(&mut self, ctx: &Ctx, old: &str, new: &str) -> Outcome<Renamed> {
        let done = self.graph.graph_rename(ctx, old, new)?;
        let (from, to) = (done.value.from.as_str(), done.value.graph.as_str());
        for job in &mut self.pending {
            if job.alias == from {
                job.alias = to.to_string();
            }
        }
        self.similarity_rebuilds.retain(|a| a != from);
        if let Some(l) = self.graph.loaded(to) {
            let pending: Vec<Id> = l.vectors_pending.iter().flatten().copied().collect();
            if l.similar_stale && !self.similarity_rebuilds.iter().any(|a| a == to) {
                self.similarity_rebuilds.push(to.to_string());
            }
            self.queue(to, pending);
        }
        Ok(done)
    }

    /// `umx graph move <alias> <path>` (ws006 S23).
    pub fn graph_move(&mut self, ctx: &Ctx, alias: &str, to: &str) -> Outcome<Moved> {
        self.graph.graph_move(ctx, alias, to)
    }

    /// `umx graph primary <alias>` (F98).
    pub fn graph_primary(&mut self, ctx: &Ctx, alias: &str) -> Outcome<PrimarySet> {
        self.graph.graph_primary(ctx, alias)
    }

    pub fn graph_set(
        &mut self,
        ctx: &Ctx,
        alias: &str,
        given: &[(String, String)],
    ) -> Outcome<GraphSet> {
        let before = self.policies();
        let mut done = self.graph.graph_set(ctx, alias, given)?;
        done.notes.extend(self.recount_changed(&before));
        done.notes
            .extend(self.embedding.graph_warnings(&self.graph, alias));
        Ok(done)
    }

    pub fn set_idle_unload_ms(&mut self, ms: u64) {
        self.graph.set_idle_unload_ms(ms)
    }

    pub fn idle_unload_ms(&self) -> u64 {
        self.graph.idle_unload_ms()
    }

    pub fn next_idle_unload_ms(&self) -> Option<u64> {
        self.graph.next_idle_unload_ms()
    }

    pub fn idle_wait_ms(&self) -> Option<u64> {
        self.graph.idle_wait_ms()
    }

    pub fn unload_idle(&mut self) -> Vec<String> {
        self.graph.unload_idle()
    }

    pub fn node_add(&mut self, ctx: &Ctx, req: NodeAddRequest) -> Outcome<NodeAdded> {
        let mut done = self.graph.node_add(ctx, req)?;
        let w = self.touched(&done.value.alias, done.value.node.id);
        done.notes.extend(w);
        Ok(done)
    }

    /// `umx batch` (req-88): every line or none. The nodes it added or changed are queued for embedding once, after
    /// the write, and never for a dry run or a failed batch.
    pub fn batch(&mut self, ctx: &Ctx, req: BatchRequest) -> BatchOutcome {
        let mut done = self.graph.batch(ctx, req)?;
        let touched = core::mem::take(&mut done.value.touched);
        if !touched.is_empty() {
            let alias = done.value.alias.clone();
            self.queue(&alias, touched.iter().copied());
            self.settle_held(&alias, Some(&touched));
            // One vector_shard_unreadable per touched node: under the batch's warning cap (plan 205).
            let late = self.recheck(&alias, &touched);
            graph_store::add_late_warnings(&mut done, late);
        }
        Ok(done)
    }

    pub fn node_get(&self, ctx: &Ctx, node: &str) -> Outcome<(String, Node)> {
        self.graph.node_get(ctx, node)
    }

    /// F188: the secret-scan flags of each shown node (see [`GraphStore::secret_flags`]).
    pub fn secret_flags(
        &self,
        rows: &[(&str, &Node)],
        columns: Option<&[String]>,
    ) -> Vec<Vec<String>> {
        self.graph.secret_flags(rows, columns)
    }

    /// F228: which node titles a read shows on another record its secret check flags (see
    /// [`GraphStore::titles_flagged`]).
    pub fn titles_flagged(&self, titles: &[(GraphKey<'_>, Id, &str)]) -> Vec<bool> {
        self.graph.titles_flagged(titles)
    }

    /// F228: each edge row's flagged `from` and `to` titles (see [`GraphStore::edge_titles_flagged`]).
    pub fn edge_titles_flagged(&self, rows: &[EdgeRow]) -> Vec<[bool; 2]> {
        self.graph.edge_titles_flagged(rows)
    }

    /// F234: which problem candidates' titles the secret check flags (see
    /// [`GraphStore::candidate_titles_flagged`]).
    pub fn candidate_titles_flagged(&self, candidates: &[(&str, &str)]) -> Vec<bool> {
        self.graph.candidate_titles_flagged(candidates)
    }

    /// F188 over `node list` (security N1 on s16): the flags of each listed row for the columns it returns, as
    /// [`secret_flags`](Self::secret_flags) gives them for the whole node, without decoding a line for a row that
    /// shows only hot columns. One allows read per call at most, as there.
    pub fn listed_secret_flags(
        &self,
        rows: &[(&str, ListedNode<'_>)],
        columns: &[String],
    ) -> Vec<Vec<String>> {
        let nodes: Vec<Node> = rows.iter().map(|(_, n)| n.shown(columns)).collect();
        let shown: Vec<(&str, &Node)> = rows.iter().map(|(alias, _)| *alias).zip(&nodes).collect();
        self.graph.secret_flags(&shown, Some(columns))
    }

    /// `umx resource show <node>` (ws006 S3b).
    pub fn resource_show(&self, ctx: &Ctx, node: &str) -> Outcome<ResourceShown> {
        self.graph.resource_show(ctx, node)
    }

    /// `umx node list`. A `--where entity_key=<k>` lookup that names no graph while several are loaded covers every
    /// loaded open graph (ws008 E2b F9), as `entity show` does; every other list keeps its one-graph rule. A key
    /// lookup also warns `entity_type_mismatch` when the key is held under another type in a covered graph (G5).
    pub fn node_list(&self, ctx: &Ctx, req: NodeListRequest) -> Outcome<Page<ListedNode<'_>>> {
        if req.along.is_some() {
            graph_store::along_one_graph(ctx)?;
        }
        let within = req.within.clone();
        // Every `entity_key = <k>` equality, as the filter itself parses it (R9: spaces around `=`; R10: all of them).
        let mut keys: Vec<String> = req
            .wheres
            .iter()
            .filter_map(|w| umx_graph::parse_where(w).ok())
            .filter(|(field, op, _)| {
                field == umx_core::entity::ENTITY_KEY && *op == umx_graph::Op::Eq
            })
            .filter_map(|(_, _, raw)| umx_core::entity::normalise_key(&raw).ok())
            .collect();
        keys.sort();
        keys.dedup();
        let all_unnamed = !keys.is_empty()
            && ctx.graph.is_none()
            && ctx.scope.is_none()
            && ctx.default_graph.is_none()
            && within.is_none()
            && req.along.is_none()
            && self.graph.loaded_count() > 1;
        let mut done = self.graph.scoped_page(
            ctx,
            &[within.as_deref()],
            "node",
            req.limit,
            req.offset,
            all_unnamed,
            |single, limit, offset| {
                self.graph.node_list(
                    single,
                    NodeListRequest {
                        limit,
                        offset,
                        ..req.clone()
                    },
                )
            },
        )?;
        let aliases: Vec<&str> = done.value.alias.split(',').collect();
        for key in &keys {
            if let Some(w) = self
                .graph
                .key_type_mismatch(&aliases, key, req.kind.as_deref())
                && !done.notes.contains(&w)
            {
                done.notes.push(w);
            }
        }
        Ok(done)
    }

    /// A single-graph read's result with copies_skipped when it named no graph and its graph has other copies
    /// loaded (d-21): the same scope rule as query and search, through the store's one helper.
    fn noted<T>(
        &self,
        ctx: &Ctx,
        refs: &[Option<&str>],
        out: Outcome<Page<T>>,
    ) -> Outcome<Page<T>> {
        out.map(|mut done| {
            if let Some(note) = self.graph.copies_note(ctx, refs, &done.value.alias) {
                done.notes.push(note);
            }
            done
        })
    }

    pub fn node_find(
        &self,
        ctx: &Ctx,
        text: &str,
        kind: Option<&str>,
        within: Option<&str>,
        limit: usize,
    ) -> Outcome<Page<(Node, f64)>> {
        self.node_find_along(ctx, text, kind, within, limit, None)
    }

    /// [`Self::node_find`] keeping only the matches a spine places in range (`--along`, D71 step 12).
    pub fn node_find_along(
        &self,
        ctx: &Ctx,
        text: &str,
        kind: Option<&str>,
        within: Option<&str>,
        limit: usize,
        along: Option<&AlongRequest>,
    ) -> Outcome<Page<(Node, f64)>> {
        self.graph.node_find(ctx, text, kind, within, limit, along)
    }

    pub fn neighbours(&self, ctx: &Ctx, req: NeighboursRequest) -> Outcome<Page<NeighbourRow>> {
        let node = req.node.clone();
        self.noted(
            ctx,
            &[Some(&node)],
            QueryService.neighbours(&self.graph, ctx, req),
        )
    }

    /// `umx query` (req-27): see [`cypher`].
    pub fn query(&self, ctx: &Ctx, req: &CypherRequest) -> Outcome<QueryRows> {
        cypher::query(&self.graph, &self.query_permits, ctx, req)
    }

    /// Takes one query permit and holds it until the guard drops, as a running query does. Tests only.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn hold_query_permit(&self) -> Option<QueryPermit<'_>> {
        self.query_permits.try_acquire()
    }

    /// Query permits held now. Tests only.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn query_permits_held(&self) -> usize {
        self.query_permits.held()
    }

    pub fn walk(&self, ctx: &Ctx, req: WalkRequest) -> Outcome<Page<WalkRow>> {
        let (node, within) = (req.node.clone(), req.within.clone());
        self.noted(
            ctx,
            &[Some(&node), within.as_deref()],
            QueryService.walk(&self.graph, ctx, req),
        )
    }

    pub fn path(&self, ctx: &Ctx, req: PathRequest) -> Outcome<Page<PathRow>> {
        let (from, to) = (req.from.clone(), req.to.clone());
        self.noted(
            ctx,
            &[Some(&from), Some(&to)],
            QueryService.path(&self.graph, ctx, req),
        )
    }

    pub fn node_edit(
        &mut self,
        ctx: &Ctx,
        node: &str,
        req: NodeEditRequest,
    ) -> Outcome<NodeEdited> {
        let mut done = self.graph.node_edit(ctx, node, req)?;
        if !done.value.changed.is_empty() {
            let w = self.touched(&done.value.alias, done.value.node.id);
            done.notes.extend(w);
        }
        Ok(done)
    }

    /// `umx entity key <node> add|remove <key>` (ws008 E1): the key is checked and put in stored form, then the
    /// node's `entity_key` list changes through `node edit`, so it is written, stamped and warned the same way.
    /// Adding a key already there, or removing one that is not, changes nothing.
    pub fn entity_key(
        &mut self,
        ctx: &Ctx,
        node: &str,
        add: bool,
        key: &str,
    ) -> Outcome<NodeEdited> {
        let key = umx_core::entity::normalise_key(key).map_err(|r| graph_store::key_refused(&r))?;
        let (_, current) = self
            .graph
            .node_get(&ctx.clone().with_provenance(false), node)?
            .value;
        let mut keys = current
            .fields
            .get(umx_core::entity::ENTITY_KEY)
            .map(umx_core::entity::stored_keys)
            .unwrap_or_default();
        if add {
            if !keys.contains(&key) {
                keys.push(key);
            }
        } else {
            keys.retain(|k| *k != key);
        }
        let field = umx_core::entity::ENTITY_KEY.to_string();
        let mut req = NodeEditRequest::default();
        if keys.is_empty() {
            req.unsets.push(field);
        } else {
            let list = keys.into_iter().map(umx_core::Value::String).collect();
            req.sets
                .push((field, SetValue::Json(umx_core::Value::Array(list))));
        }
        self.node_edit(ctx, node, req)
    }

    pub fn node_note(
        &mut self,
        ctx: &Ctx,
        node: &str,
        append: &str,
        evidence: &[String],
        evidence_kind: Option<&str>,
    ) -> Outcome<NodeNoted> {
        let req = NodeNoteRequest::from_legacy(append, evidence, evidence_kind)?;
        self.node_note_request(ctx, node, req)
    }

    pub fn node_note_request(
        &mut self,
        ctx: &Ctx,
        node: &str,
        req: NodeNoteRequest,
    ) -> Outcome<NodeNoted> {
        let mut done = self.graph.node_note(ctx, node, req)?;
        let w = self.touched(&done.value.alias, done.value.node.id);
        done.notes.extend(w);
        Ok(done)
    }

    pub fn node_status(
        &mut self,
        ctx: &Ctx,
        node: &str,
        status: &str,
        reason: Option<&str>,
        label: Option<&str>,
    ) -> Outcome<StatusMoved> {
        self.graph.node_status(ctx, node, status, reason, label)
    }

    /// `umx node status` with `--in` and `--evidence` (ws008 TM6, req-150).
    pub fn node_status_with(
        &mut self,
        ctx: &Ctx,
        node: &str,
        req: NodeStatusRequest,
    ) -> Outcome<StatusMoved> {
        self.graph.node_status_with(ctx, node, req)
    }

    /// `umx resolve <address>`: what an address names in the loaded graphs (req-15 in part).
    pub fn resolve(&self, ctx: &Ctx, address: &str) -> Outcome<Resolved> {
        self.graph.resolve(ctx, address)
    }

    pub fn node_history(&self, ctx: &Ctx, node: &str, limit: usize) -> Outcome<Page<HistoryRow>> {
        self.graph.node_history(ctx, node, limit)
    }

    /// `umx node evidence <node> list` (req-126). Evidence is never embedded, so its verbs queue nothing.
    pub fn node_evidence_list(
        &self,
        ctx: &Ctx,
        node: &str,
        removed: bool,
    ) -> Outcome<EvidenceListed> {
        self.graph.node_evidence_list(ctx, node, removed)
    }

    pub fn node_evidence_add(
        &mut self,
        ctx: &Ctx,
        node: &str,
        pointer: &str,
        evidence_kind: Option<&str>,
    ) -> Outcome<EvidenceAdded> {
        self.graph
            .node_evidence_add(ctx, node, pointer, evidence_kind)
    }

    pub fn node_evidence_remove(
        &mut self,
        ctx: &Ctx,
        node: &str,
        pointer: &str,
        reason: Option<&str>,
        at: Option<usize>,
    ) -> Outcome<EvidenceRemoved> {
        self.graph
            .node_evidence_remove(ctx, node, pointer, reason, at)
    }

    pub fn node_evidence_redact(
        &mut self,
        ctx: &Ctx,
        node: &str,
        pointer: &str,
        reason: Option<&str>,
    ) -> Outcome<EvidenceRemoved> {
        self.graph.node_evidence_redact(ctx, node, pointer, reason)
    }

    pub fn edge_add(
        &mut self,
        ctx: &Ctx,
        from: &str,
        relation: &str,
        to: &str,
        sets: &[(String, SetValue)],
    ) -> Outcome<EdgeAdded> {
        self.graph.edge_add(ctx, from, relation, to, sets)
    }

    /// `umx edge add --evidence` (req-124): see [`GraphStore::edge_add_with`].
    pub fn edge_add_with(
        &mut self,
        ctx: &Ctx,
        from: &str,
        relation: &str,
        to: &str,
        sets: &[(String, SetValue)],
        evidence: &EdgeEvidence,
    ) -> Outcome<EdgeAdded> {
        self.graph
            .edge_add_with(ctx, from, relation, to, sets, evidence)
    }

    /// `umx edge show` (req-124).
    pub fn edge_show(&self, ctx: &Ctx, edge: &str) -> Outcome<EdgeRow> {
        self.graph.edge_show(ctx, edge)
    }

    pub fn edge_delete(&mut self, ctx: &Ctx, edge: &str) -> Outcome<EdgeChanged> {
        self.graph.edge_delete(ctx, edge)
    }

    /// `umx entity link <a> <b>` (ws008 E2): one `same_as` edge, in the graph the request names or `a`'s.
    pub fn entity_link(
        &mut self,
        ctx: &Ctx,
        a: &str,
        b: &str,
        dry_run: bool,
    ) -> Outcome<EntityLinked> {
        self.graph.entity_link(ctx, a, b, dry_run)
    }

    /// `umx entity unlink <a> <b>` (ws008 E2): removes the `same_as` edge between them, if one is stored.
    pub fn entity_deny(
        &mut self,
        ctx: &Ctx,
        a: &str,
        b: &str,
        reason: Option<&str>,
    ) -> Outcome<EntityDenied> {
        self.graph.entity_deny(ctx, a, b, reason)
    }

    pub fn entity_undeny(&mut self, ctx: &Ctx, a: &str, b: &str) -> Outcome<EntityUnlinked> {
        self.graph.entity_undeny(ctx, a, b)
    }

    pub fn entity_unlink(&mut self, ctx: &Ctx, a: &str, b: &str) -> Outcome<EntityUnlinked> {
        self.graph.entity_unlink(ctx, a, b)
    }

    /// `umx who <key|seat|session>` (ws007 s04, req-96).
    pub fn who(&self, ctx: &Ctx, who: &str, limit: usize) -> Outcome<WhoListed> {
        self.graph.who(ctx, who, limit)
    }

    /// `umx node who <node>` (ws007 s04, req-96).
    pub fn node_who(&self, ctx: &Ctx, node: &str) -> Outcome<NodeWho> {
        self.graph.node_who(ctx, node)
    }

    /// `umx entity show <node>` (ws008 E2): the node's entity over the graphs the read covers, built at read time.
    pub fn entity_show(&self, ctx: &Ctx, node: &str) -> Outcome<EntityShown> {
        self.graph.entity_show(ctx, node)
    }

    pub fn edge_edit(
        &mut self,
        ctx: &Ctx,
        edge: &str,
        req: &EdgeEditRequest,
    ) -> Outcome<EdgeChanged> {
        self.graph.edge_edit(ctx, edge, req)
    }

    pub fn edge_list(&self, ctx: &Ctx, req: EdgeListRequest) -> Outcome<Page<EdgeRow>> {
        if req.along.is_some() {
            graph_store::along_one_graph(ctx)?;
        }
        self.graph.scoped_page(
            ctx,
            &[req.from.as_deref(), req.to.as_deref()],
            "edge",
            req.limit,
            req.offset,
            false,
            |single, limit, offset| {
                self.graph.edge_list_in(
                    single,
                    EdgeListRequest {
                        limit,
                        offset,
                        ..req.clone()
                    },
                    ctx,
                )
            },
        )
    }

    pub fn scope_save(&mut self, ctx: &Ctx, name: &str, replace: bool) -> Outcome<ScopeInfo> {
        self.graph.scope_save(ctx, name, replace)
    }

    pub fn scope_list(&self) -> Outcome<Vec<ScopeInfo>> {
        self.graph.scope_list()
    }

    pub fn scope_show(&self, name: &str) -> Outcome<ScopeInfo> {
        self.graph.scope_show(name)
    }

    pub fn scope_delete(&mut self, name: &str) -> Outcome<String> {
        self.graph.scope_delete(name)
    }

    /// After a write to a node's text: queue it (when this embedder may embed the graph) and update whether its
    /// vectors are pending in the graph's own space (req-75). A shard that cannot be read leaves the count as it was.
    fn touched(&mut self, alias: &str, node: Id) -> Vec<Problem> {
        self.queue(alias, [node]);
        self.settle_held(alias, Some(&[node]));
        let mut warnings = Vec::new();
        warnings.extend(self.recheck(alias, &[node]));
        warnings
    }

    /// Updates whether `nodes` have pending vectors. A shard that cannot be read never leaves a node counted
    /// current: it is marked pending and warned (review R85-1).
    fn recheck(&mut self, alias: &str, nodes: &[Id]) -> Vec<Problem> {
        match self.embedding.pending_among(&self.graph, alias, nodes) {
            Ok(marks) => {
                self.graph.mark_vectors_pending(alias, &marks);
                Vec::new()
            }
            Err(cause) => {
                let marks: Vec<(Id, bool)> = nodes.iter().map(|id| (*id, true)).collect();
                self.graph.mark_vectors_pending(alias, &marks);
                let graph = graph_store::alias_name(alias);
                nodes
                    .iter()
                    .map(|id| problems::vector_shard_unreadable(&graph, *id, &cause))
                    .collect()
            }
        }
    }

    /// Effective policies of the loaded graphs, to see which ones a command changed.
    fn policies(&self) -> alloc::collections::BTreeMap<String, umx_core::Policy> {
        self.graph
            .graph_list()
            .into_iter()
            .map(|g| (g.alias, g.policy.effective))
            .collect()
    }

    /// Recounts pending vectors of each graph whose effective policy changed since `before`, and queues what the new
    /// store lacks: the policy picks the shard home (graph folder or this machine's cache), so the old count is for
    /// another store (review R85-2).
    fn recount_changed(
        &mut self,
        before: &alloc::collections::BTreeMap<String, umx_core::Policy>,
    ) -> Vec<Problem> {
        let mut warnings = Vec::new();
        for (alias, now) in self.policies() {
            if before.get(&alias) == Some(&now) {
                continue;
            }
            self.refill(&alias, &mut warnings);
        }
        warnings
    }

    /// Recounts and queues one graph as a load would, after what decides its store or instance changed.
    fn refill(&mut self, alias: &str, warnings: &mut Vec<Problem>) {
        match self.embedding.scan(&mut self.graph, alias) {
            Ok(scan) => {
                self.pending_from(alias, Some(&scan));
                self.settle_held(alias, None);
                self.load_similarity(alias, warnings);
                // The new store is filled as a load would fill it.
                self.queue(alias, scan.to_embed.iter().chain(&scan.to_tidy).copied());
            }
            Err(cause) => {
                self.graph.set_vectors_pending(alias, None);
                warnings.push(problems::vectors_unreadable(
                    &graph_store::alias_name(alias),
                    &cause,
                ));
            }
        }
    }

    /// Recounts which of `nodes` (None: every node) have a text the secret hold keeps from the graph's embedder, so
    /// `vectors_held` and `vectors_pending` stay apart (req-49). Only a hosted instance holds anything.
    fn settle_held(&mut self, alias: &str, nodes: Option<&[Id]>) {
        let hosted = self
            .embedding
            .embedder_for(&self.graph, alias)
            .is_some_and(|i| i.embedder.locality() != umx_core::ports::Locality::Local);
        if !hosted {
            self.graph.mark_vectors_held(alias, true, &[]);
            return;
        }
        let ids: Vec<Id> = match nodes {
            Some(n) => n.to_vec(),
            None => self.graph.node_ids(alias),
        };
        let allows = self.graph.secret_allows();
        let marks: Vec<(Id, bool)> = ids
            .iter()
            .map(|id| {
                (
                    *id,
                    embedding::holds_secret(&self.graph, &allows, alias, *id),
                )
            })
            .collect();
        self.graph.mark_vectors_held(alias, nodes.is_none(), &marks);
    }

    /// Queues `nodes` of `alias` for the embed worker, when there is an embedder.
    fn queue(&mut self, alias: &str, nodes: impl IntoIterator<Item = Id>) {
        if self.embedding.key_for(&self.graph, alias).is_none() {
            return;
        }
        self.pending.extend(
            nodes
                .into_iter()
                .filter(|id| self.graph.embeddable_node(alias, *id))
                .map(|node| EmbedJob {
                    alias: alias.to_string(),
                    node,
                }),
        );
    }

    /// The nodes queued for embedding since the last call. Writes only queue; the embedding itself runs off the
    /// writer (the daemon's embed worker), so an edit returns before its vectors exist (req-35).
    /// What changed in the graphs since the last call, for the views' change log (ws009 S5a).
    pub fn take_changes(&mut self) -> Vec<umx_core::ui::Fed> {
        self.graph.take_changes()
    }

    pub fn take_embed_jobs(&mut self) -> Vec<EmbedJob> {
        let mut seen = alloc::collections::BTreeSet::new();
        let mut jobs = core::mem::take(&mut self.pending);
        jobs.retain(|j| self.graph.embeddable_node(&j.alias, j.node) && seen.insert(j.clone()));
        jobs
    }

    /// What a queued node needs (shared access only; see [`EmbeddingService::plan`]).
    pub fn embed_plan(&self, job: &EmbedJob) -> Result<Option<Planned>, Problem> {
        self.embedding.plan(&self.graph, job)
    }

    /// [`Self::embed_plan`] for a round of jobs, each graph's policy read once for the round.
    pub fn embed_plan_all(&self, jobs: &[EmbedJob]) -> Vec<Result<Option<Planned>, Problem>> {
        self.embedding.plan_all(&self.graph, jobs)
    }

    /// Writes embedded vectors into their shards (the writer; see [`EmbeddingService::apply`]), then clears the
    /// pending mark of each node whose vectors are now current.
    pub fn embed_apply(&mut self, results: &[Vectors]) -> Result<(Applied, Vec<Problem>), Problem> {
        self.embed_apply_with(results, false)
    }

    /// [`Self::embed_apply`] for the daemon's embed backlog (ws004 s25a): the similar_to lists move in memory and
    /// their files are owed, written later a file at a time by [`Self::flush_similar_step`], so the writer's lock is
    /// held for one vector shard, never for every list file the new vectors touch.
    pub fn embed_apply_deferred(
        &mut self,
        results: &[Vectors],
    ) -> Result<(Applied, Vec<Problem>), Problem> {
        self.embed_apply_with(results, true)
    }

    fn embed_apply_with(
        &mut self,
        results: &[Vectors],
        defer_similar: bool,
    ) -> Result<(Applied, Vec<Problem>), Problem> {
        let (applied, mut warnings) =
            self.embedding
                .apply(&mut self.graph, results, defer_similar)?;
        let mut by_graph: alloc::collections::BTreeMap<&str, Vec<Id>> = Default::default();
        for r in results {
            by_graph.entry(&r.job.alias).or_default().push(r.job.node);
        }
        for (alias, nodes) in by_graph {
            warnings.extend(self.recheck(alias, &nodes));
        }
        Ok((applied, warnings))
    }

    /// Whether any loaded graph owes similar_to files from deferred applies (ws004 s25a).
    pub fn similar_owed(&self) -> bool {
        self.graph.similar_owed()
    }

    /// Writes at most one owed similar_to file (`_meta.tsv` removed first, then a shard, then `_meta.tsv` last);
    /// None when nothing is owed.
    pub fn flush_similar_step(&mut self) -> Option<Vec<Problem>> {
        self.graph.flush_similar_step(None)
    }

    /// Lets graphs whose owed similar_to writes failed try again (before a stop or restart).
    pub fn unpause_similar(&mut self) {
        self.graph.unpause_similar();
    }

    /// What writing owed similar_to files found while graphs unloaded themselves (R180-4), for the daemon's log.
    pub fn take_unload_problems(&mut self) -> Vec<Problem> {
        self.graph.take_unload_problems()
    }

    /// The pending set of a graph from a scan of its shards: None when it has no model space.
    fn pending_from(&mut self, alias: &str, scan: Option<&Scan>) {
        let pending = self.embedding.block_key(&self.graph, alias).map(|_| {
            scan.map(|s| s.pending.iter().copied().collect())
                .unwrap_or_default()
        });
        self.graph.set_vectors_pending(alias, pending);
        let outdated = scan
            .map(|s| s.outdated.iter().copied().collect())
            .unwrap_or_default();
        self.graph.set_vectors_outdated(alias, outdated);
    }

    /// `umx embeddings allow --node <id> --field <f>` (req-49): lets the field's current text, which the secret scan
    /// holds, go to a hosted embedder on this machine, and queues the node so it is embedded now.
    pub fn embeddings_allow(
        &mut self,
        ctx: &Ctx,
        node: &str,
        field: &str,
    ) -> Outcome<SecretAllowed> {
        let done = self.graph.allow_secret(ctx, node, field)?;
        if done.value.allowed {
            let (alias, id) = (done.value.alias.clone(), done.value.node);
            self.queue(&alias, [id]);
            self.settle_held(&alias, Some(&[id]));
        }
        Ok(done)
    }

    /// `umx embeddings reembed`: makes the selected graph's vectors with the configured embedder's model. The cost
    /// (nodes, texts and estimated tokens still to embed in that model, which model, local or hosted) is worked
    /// out from the shards without calling the embedder; `dry_run` stops there. Otherwise graph.json's recorded key
    /// becomes the embedder's (only here, never by itself; D32) and every node that needs it is queued for the
    /// embed worker, which paces and writes as for any edit. Vectors of the old model stay in their own folder,
    /// never mixed in. A secured graph is refused unless the embedder is local; `model`, when given, must be the
    /// configured embedder's key. The daemon runs [`Self::reembed_plan`] under the shared lock and only
    /// [`Self::reembed_apply`] on the writer (architect F1); this runs both, for callers that hold the services.
    pub fn embeddings_reembed(
        &mut self,
        ctx: &Ctx,
        dry_run: bool,
        model: Option<&umx_core::ports::ModelKey>,
    ) -> Outcome<Reembed> {
        let plan = self.reembed_plan(ctx, model)?;
        if dry_run {
            return Ok(Done::ok(plan.cost));
        }
        self.reembed_apply(plan)
    }

    /// The read half of a reembed: checks (embedder, `model`, secured graph and locality) and the cost, reading
    /// every shard of the target model once. Changes nothing; the plan's cost is the `--dry-run` answer.
    pub fn reembed_plan(
        &self,
        ctx: &Ctx,
        model: Option<&umx_core::ports::ModelKey>,
    ) -> Result<ReembedPlan, Problem> {
        let alias = self.graph.select(ctx, &[])?;
        let to = self.reembed_check(&alias, model)?;
        let embedder = self.embedding.embedder().expect("checked above");
        let from = self
            .graph
            .loaded(&alias)
            .and_then(|l| l.meta.embedding.as_ref())
            .and_then(|e| e.key.clone());
        let needs = self.embedding.needs(&self.graph, &alias, &to)?;
        let cost = Reembed {
            alias: alias.clone(),
            from,
            to,
            local: embedder.locality() == umx_core::ports::Locality::Local,
            nodes: needs.len(),
            texts: needs.iter().map(|p| p.texts.len()).sum(),
            estimated_tokens: needs
                .iter()
                .flat_map(|p| &p.texts)
                .map(|w| estimate_tokens(&w.text))
                .sum(),
            dry_run: true,
            queued: 0,
        };
        Ok(ReembedPlan {
            nodes: needs.iter().map(|p| p.job.node).collect(),
            cost,
        })
    }

    /// The writer half: checks again (the embedder, policy or graph may have changed since the plan; no shard is
    /// read), records the embedder's key in graph.json if it differs, and queues the planned nodes plus any that
    /// were changed since. Nodes added meanwhile are queued as well, from the pending set.
    pub fn reembed_apply(&mut self, plan: ReembedPlan) -> Outcome<Reembed> {
        let ReembedPlan { nodes, mut cost } = plan;
        let alias = cost.alias.clone();
        let to = self.reembed_check(&alias, Some(&cost.to))?;
        let mut warnings = Vec::new();
        if cost.from.as_ref() != Some(&to) {
            warnings.extend(self.graph.set_embedding_key(&alias, to)?);
            // The graph's block space is now `to` (s13): load its vectors already on disk into memory, count what is
            // pending and queue what is missing, as a load does. Only a real reembed that changes the key pays this
            // read on the writer; the cost and the plan are read under the shared lock.
            match self.embedding.scan(&mut self.graph, &alias) {
                Ok(scan) => {
                    self.pending_from(&alias, Some(&scan));
                    // The lists follow the new space: its stored ones if they match, else a rebuild.
                    self.load_similarity(&alias, &mut warnings);
                    // Outdated nodes too: a reembed is what remakes them (req-97).
                    let queued: alloc::collections::BTreeSet<Id> = scan
                        .to_embed
                        .iter()
                        .chain(&scan.to_tidy)
                        .chain(&scan.outdated)
                        .copied()
                        .collect();
                    if self.embedding.key_for(&self.graph, &alias).is_some() {
                        cost.queued = queued.len();
                        self.queue(&alias, queued);
                    }
                    cost.dry_run = false;
                    return Ok(Done {
                        value: cost,
                        notes: warnings,
                    });
                }
                Err(cause) => warnings.push(problems::vectors_unreadable(
                    &graph_store::alias_name(&alias),
                    &cause,
                )),
            }
        }
        // The same key: the plan holds every node whose text form changed, so the current form is recorded (req-97).
        warnings.extend(self.graph.set_text_form(&alias)?);
        let mut ids: alloc::collections::BTreeSet<Id> = nodes.into_iter().collect();
        if let Some(pending) = self
            .graph
            .loaded(&alias)
            .and_then(|l| l.vectors_pending.as_ref())
        {
            ids.extend(pending.iter().copied());
        }
        if self.embedding.key_for(&self.graph, &alias).is_some() {
            cost.queued = ids.len();
            self.queue(&alias, ids.iter().copied());
        }
        self.graph.set_vectors_pending(&alias, Some(ids));
        self.settle_held(&alias, None);
        cost.dry_run = false;
        Ok(Done {
            value: cost,
            notes: warnings,
        })
    }

    /// The reembed checks: an embedder, `model` (if given) is its key, and a secured graph only with a local one.
    /// The configured key, or why not.
    fn reembed_check(
        &self,
        alias: &str,
        model: Option<&umx_core::ports::ModelKey>,
    ) -> Result<umx_core::ports::ModelKey, Problem> {
        let Some(embedder) = self.embedding.embedder() else {
            return Err(problems::embedder_not_configured());
        };
        if self.graph.loaded(alias).is_none() {
            return Err(self.graph.not_loaded(alias));
        }
        let graph = graph_store::alias_name(alias);
        let to = embedder.key();
        if let Some(m) = model
            && *m != to
        {
            return Err(problems::reembed_model_not_configured(&graph, m, &to));
        }
        // The graph's embedder setting must admit the default (req-46, req-91), read fresh; not loaded fails closed.
        let rule = self
            .graph
            .effective_policy(alias)
            .unwrap_or(umx_core::Policy::SECURED)
            .embedder;
        let name = self.embedding.default_name().unwrap_or_default();
        if !rule.admits(embedder.locality(), self.embedding.trusted(&name)) {
            return Err(problems::secured_embed_refused(&graph));
        }
        Ok(to)
    }

    /// `umx embeddings repair`: reads the graph's shards once; removes repeated lines (keeping the first in sort
    /// order, req-36) and lines for fields a node no longer has; queues every node with a missing, stale or
    /// conflicting vector. Lines for unknown nodes and lines umx cannot read are kept and warned.
    pub fn embeddings_repair(&mut self, ctx: &Ctx) -> Outcome<Repaired> {
        if self.embedding.key().is_none() {
            return Err(problems::embedder_not_configured());
        }
        let alias = self.graph.select(ctx, &[])?;
        let scan = self.embedding.scan(&mut self.graph, &alias)?;
        let mut similar_warnings = Vec::new();
        self.load_similarity(&alias, &mut similar_warnings);
        let tidy: Vec<Vectors> = scan
            .to_tidy
            .iter()
            .map(|node| Vectors {
                job: EmbedJob {
                    alias: alias.clone(),
                    node: *node,
                },
                key: None,
                vectors: Vec::new(),
            })
            .collect();
        let (applied, mut warnings) = self.embedding.apply(&mut self.graph, &tidy, false)?;
        warnings.extend(similar_warnings);
        self.queue(&alias, scan.to_embed.iter().copied());
        warnings.extend(EmbeddingService::scan_warnings(
            &graph_store::alias_name(&alias),
            &scan,
        ));
        warnings.extend(self.embedding.graph_warnings(&self.graph, &alias));
        Ok(Done {
            value: Repaired {
                alias,
                queued: scan.to_embed.len(),
                tidied: scan.to_tidy.len(),
                shards_written: applied.shards_written,
                orphans: scan.orphans.len(),
                kept_lines: scan.kept.len(),
            },
            notes: warnings,
        })
    }

    pub fn ontology_show(&self, ctx: &Ctx) -> Outcome<(String, Ontology)> {
        self.graph.ontology_show(ctx)
    }

    pub fn ontology_add_type(
        &mut self,
        ctx: &Ctx,
        name: &str,
        fields: &[String],
        description: Option<&str>,
    ) -> Outcome<OntologyAdded> {
        self.graph.ontology_add_type(ctx, name, fields, description)
    }

    pub fn ontology_add_status(
        &mut self,
        ctx: &Ctx,
        name: &str,
        description: Option<&str>,
    ) -> Outcome<OntologyAdded> {
        self.graph.ontology_add_status(ctx, name, description)
    }

    pub fn ontology_set_type(
        &mut self,
        ctx: &Ctx,
        name: &str,
        columns: Option<&[String]>,
    ) -> Outcome<OntologyAdded> {
        self.graph.ontology_set_type(ctx, name, columns)
    }

    /// F96: a type's default `node list --type` columns; see [`GraphStore::node_type_columns`].
    pub fn node_type_columns(
        &self,
        ctx: &Ctx,
        kind: &str,
        within: Option<&str>,
    ) -> Option<Vec<String>> {
        self.graph.node_type_columns(ctx, kind, within)
    }

    pub fn ontology_add_relation(
        &mut self,
        ctx: &Ctx,
        name: &str,
        symmetric: bool,
        description: Option<&str>,
    ) -> Outcome<OntologyAdded> {
        self.graph
            .ontology_add_relation(ctx, name, symmetric, description)
    }

    pub fn ontology_add_field(
        &mut self,
        ctx: &Ctx,
        type_name: &str,
        spec: &str,
        description: Option<&str>,
    ) -> Outcome<OntologyAdded> {
        self.ontology_add_field_as(ctx, type_name, spec, description, None)
    }

    /// `ontology add field <type> <name>:<kind> [--description d] [--role data|content]` (D56): with no role, the
    /// kind's default.
    pub fn ontology_add_field_as(
        &mut self,
        ctx: &Ctx,
        type_name: &str,
        spec: &str,
        description: Option<&str>,
        role: Option<&str>,
    ) -> Outcome<OntologyAdded> {
        let mut done = self
            .graph
            .ontology_add_field(ctx, type_name, spec, description, role)?;
        // search-05: a field whose nodes already hold values (set before it was declared) is judged at once.
        let hints = self
            .graph
            .role_hints(&done.value.alias, Some((type_name, &done.value.name)));
        done.notes.extend(hints);
        Ok(done)
    }

    /// PRD 002 search-08: a search hit's place in its graph (see [`query::hit_links`]). Read only for the hits a
    /// search returns.
    pub fn hit_links(&self, alias: &str, id: Id) -> Option<HitLinks> {
        query::hit_links(&self.graph, alias, id)
    }

    /// search-05: the role hints of `alias` (see [`graph_store::GraphStore::role_hints`]).
    pub fn role_hints(&self, alias: &str, only: Option<(&str, &str)>) -> Vec<Problem> {
        self.graph.role_hints(alias, only)
    }

    /// search-05 for `umx embeddings status`, which lists every loaded graph: the role hints of an open graph only,
    /// so a graph read only when named is never read for a listing.
    pub fn role_hints_unnamed(&self, alias: &str) -> Vec<Problem> {
        if self.graph.shown_unnamed(alias) {
            self.graph.role_hints(alias, None)
        } else {
            Vec::new()
        }
    }

    /// `umx graph guide <alias>` (plan 079).
    pub fn graph_guide(&self, ctx: &Ctx, alias: &str) -> Outcome<Guide> {
        self.graph.graph_guide(ctx, alias)
    }

    /// `umx graph spine propose <graph> [--unplaced <candidate>]` (ws005 s07): reads only. `limit` is the page of
    /// unplaced ids; the daemon gives its list default when the caller names none.
    pub fn graph_spine_propose(
        &self,
        ctx: &Ctx,
        alias: &str,
        unplaced: Option<&str>,
        by_rule: Option<&str>,
        limit: usize,
        offset: Option<usize>,
    ) -> Outcome<SpineProposal> {
        self.graph
            .graph_spine_propose(ctx, alias, unplaced, by_rule, limit, offset)
    }

    /// `umx graph spine set <graph> --file <decl.json>` (ws008 TM5): stores one declared spine in graph.json.
    pub fn graph_spine_set(
        &mut self,
        ctx: &Ctx,
        alias: &str,
        decl: &umx_core::Value,
    ) -> Outcome<SpineSet> {
        self.graph.graph_spine_set(ctx, alias, decl)
    }

    /// `umx graph spine confirm <graph> <candidate>` (ws008 TM5): stores a proposed candidate's declaration unchanged.
    pub fn graph_spine_confirm(
        &mut self,
        ctx: &Ctx,
        alias: &str,
        candidate: &str,
    ) -> Outcome<SpineSet> {
        self.graph.graph_spine_confirm(ctx, alias, candidate)
    }

    /// `umx graph spine show <graph> [<name>]` (ws008 TM5): the declared spines, placed on the graph now. Reads only.
    /// `--by-rule <name>` or `--unplaced <name>` (D71 step 12) adds one page of that spine's ids.
    #[allow(clippy::too_many_arguments)]
    pub fn graph_spine_show(
        &self,
        ctx: &Ctx,
        alias: &str,
        name: Option<&str>,
        by_rule: bool,
        unplaced: bool,
        limit: usize,
        offset: Option<usize>,
    ) -> Outcome<SpinesShown> {
        self.graph
            .graph_spine_show(ctx, alias, name, by_rule, unplaced, limit, offset)
    }

    /// `umx graph spine remove <graph> <name>` (ws008 TM5): drops one declared spine.
    pub fn graph_spine_remove(
        &mut self,
        ctx: &Ctx,
        alias: &str,
        name: &str,
    ) -> Outcome<SpineRemoved> {
        self.graph.graph_spine_remove(ctx, alias, name)
    }

    /// `ontology set field <type> <field> --role data|content` (D56, req-97): the nodes whose embedded text the
    /// change alters are pending, with a `role_changed` warning naming how many, and queued so the worker remakes
    /// their vectors (whole node, plus the field's own for data to content; data lines go at that write).
    pub fn ontology_set_field(
        &mut self,
        ctx: &Ctx,
        type_name: &str,
        field: &str,
        role: &str,
    ) -> Outcome<RoleSet> {
        let mut done = self.graph.ontology_set_field(ctx, type_name, field, role)?;
        let v = &done.value;
        if v.affected.is_empty() {
            return Ok(done);
        }
        let (alias, affected) = (v.alias.clone(), v.affected.clone());
        let names = (ValidName::new(&v.type_name), ValidName::new(&v.field));
        if let (Some(t), Some(f)) = names {
            done.notes.push(problems::role_changed(
                &graph_store::alias_name(&alias),
                &t,
                &f,
                v.role,
                affected.len(),
            ));
        }
        let marks: Vec<(Id, bool)> = affected.iter().map(|id| (*id, true)).collect();
        self.graph.mark_vectors_pending(&alias, &marks);
        self.queue(&alias, affected.iter().copied());
        self.settle_held(&alias, Some(&affected));
        Ok(done)
    }

    /// F76: `ontology remove type`; refused while in use (see the store's doc).
    pub fn ontology_remove_type(&mut self, ctx: &Ctx, name: &str) -> Outcome<OntologyRemoved> {
        self.graph.ontology_remove_type(ctx, name)
    }

    pub fn ontology_remove_status(&mut self, ctx: &Ctx, name: &str) -> Outcome<OntologyRemoved> {
        self.graph.ontology_remove_status(ctx, name)
    }

    pub fn ontology_remove_relation(&mut self, ctx: &Ctx, name: &str) -> Outcome<OntologyRemoved> {
        self.graph.ontology_remove_relation(ctx, name)
    }

    pub fn ontology_remove_field(
        &mut self,
        ctx: &Ctx,
        type_name: &str,
        field: &str,
    ) -> Outcome<OntologyRemoved> {
        self.graph.ontology_remove_field(ctx, type_name, field)
    }

    pub fn ontology_remove_relation_field(
        &mut self,
        ctx: &Ctx,
        relation: &str,
        field: &str,
    ) -> Outcome<OntologyRemoved> {
        self.graph
            .ontology_remove_relation_field(ctx, relation, field)
    }

    /// F71: one field on several types, or on every declared type, in one write (see the store's doc).
    pub fn ontology_add_field_many(
        &mut self,
        ctx: &Ctx,
        types: &[String],
        all: bool,
        spec: &str,
        description: Option<&str>,
    ) -> Result<Done<FieldsAdded>, Vec<Problem>> {
        self.ontology_add_field_many_as(ctx, types, all, spec, description, None)
    }

    /// [`Self::ontology_add_field_many`] with `--role data|content` (D56): an identical add compares the role too.
    pub fn ontology_add_field_many_as(
        &mut self,
        ctx: &Ctx,
        types: &[String],
        all: bool,
        spec: &str,
        description: Option<&str>,
        role: Option<&str>,
    ) -> Result<Done<FieldsAdded>, Vec<Problem>> {
        let mut done =
            self.graph
                .ontology_add_field_many(ctx, types, all, spec, description, role)?;
        // search-05, as the single-type add.
        let v = &done.value;
        let hints: Vec<Problem> = v
            .types
            .iter()
            .chain(&v.unchanged)
            .flat_map(|t| self.graph.role_hints(&v.alias, Some((t, &v.name))))
            .collect();
        done.notes.extend(hints);
        Ok(done)
    }

    /// F92: a custom field on a relation's edges (see the store's doc).
    pub fn ontology_add_relation_field(
        &mut self,
        ctx: &Ctx,
        relation: &str,
        spec: &str,
    ) -> Outcome<OntologyAdded> {
        self.graph.ontology_add_relation_field(ctx, relation, spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fix_keeps_the_graph_list_it_was_given() {
        // F73: `--graph a,b` stays a list in a fix, instead of being dropped.
        let run = |graph: Option<&str>| {
            let mut ctx = Ctx::new("t");
            ctx.graph = graph.map(Into::into);
            ctx.same_graph(Command::new("search").word("<text>"))
                .as_str()
                .to_string()
        };
        assert_eq!(run(Some("alpha")), "umx search <text> --graph alpha");
        assert_eq!(
            run(Some("alpha,beta")),
            "umx search <text> --graph alpha,beta"
        );
        for dropped in [
            None,
            Some("alpha,Beta"),
            Some("alpha,,beta"),
            Some("alpha,"),
        ] {
            assert_eq!(run(dropped), "umx search <text>", "{dropped:?}");
        }
    }
}
