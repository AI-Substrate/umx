//! `umx similarity pairs` (ws004 s31, req-136): each node's top k among the nodes of the OTHER graphs in scope,
//! computed per call from the graphs' whole-node int8 rows and never written.
//!
//! - Scope is search's (d-20, d-21): `graphs_in_scope`, so an unnamed read covers open graphs only (secured ones are
//!   counted, never named) and one copy per graph id. Copies named on purpose never pair a node with itself.
//! - One vector space: the `--model` key, else the first named graph's, else the one most graphs are in (ties to the
//!   lowest alias). A graph's space is the one its vectors are in: graph.json's recorded model, else its embedder's
//!   (`EmbeddingService::block_key`). Graphs in another space, or none, are left out and named in a warning.
//! - Scores are the stored similar_to lists' (`score_units`, the node's own inverse length first); ties go to the
//!   lower other id, then the lower other alias. `--min` defaults to the machine's similar floor for the key.
//! - Three steps, so a write never waits on a long read (s25a): [`Services::pairs_plan`] and one
//!   [`Services::pairs_copy`] per graph each under a short read hold; [`PairsJob::part`] with no lock (the daemon may
//!   run parts on threads; [`PairsJob::merge`] gives the same lists for any number of parts); then
//!   [`Services::pairs_finish`] under a short read hold, which re-checks scope and says when to run again.
//! - Caps ([`MAX_PAIR_DOTS`], [`MAX_PAIR_BYTES`]) are checked before anything is copied, and again as rows are copied.

use crate::{Ctx, Done, Scope, problems};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use umx_core::ports::ModelKey;
use umx_core::similar::{
    MAX_PAIR_BYTES, MAX_PAIR_DOTS, PAIRS_DEFAULT_K, PAIRS_MAX_K, SCALE, Settings, parse_units,
    score_units,
};
use umx_core::similarity::{FloorSource, dot};
use umx_core::{Id, Problem};

/// Rows a page holds when `--limit` is not given, and the most it may hold. The most is one call's every row for the
/// graph view (ws004 s34a, F194): 24k nodes at k 10, so a client reads them all at once instead of computing every
/// pair again for each page.
pub const PAIRS_DEFAULT_LIMIT: usize = 1_000;
pub const PAIRS_MAX_LIMIT: usize = 250_000;
/// The wall-clock backstop of one call (under the CLI's 120 s wait for a read).
pub const PAIRS_BACKSTOP_MS: u64 = 60_000;

/// Bytes copied per row besides its dims: an inverse length (4) and an id (16).
const ROW_EXTRA: u64 = 4 + 16;

/// Which fields each row of `umx similarity pairs` carries (ws004 s34b, F194): every field, or only the graph, id,
/// other graph, other id and score (`--rows ids`), for a client that reads ids and scores only (the graph view).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PairRows {
    #[default]
    Full,
    /// The finish only checks each node is still there: no type or title is resolved under the hold.
    Ids,
}

/// What `umx similarity pairs` was asked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PairsAsk {
    pub k: Option<usize>,
    pub min: Option<String>,
    pub model: Option<String>,
    pub same_graph: bool,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub fields: PairRows,
}

/// Why the run's key is the one it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelSource {
    /// `--model`.
    Flag,
    /// The first graph named (in `--graph` or scope order) that records a key.
    NamedFirst,
    /// The key most graphs in scope record (an unnamed read or `--all`); ties to the lowest alias.
    MostGraphs,
}

impl ModelSource {
    pub fn word(self) -> &'static str {
        match self {
            ModelSource::Flag => "flag",
            ModelSource::NamedFirst => "named-first",
            ModelSource::MostGraphs => "most-graphs",
        }
    }
}

/// The score floor a call cut at, in ten-thousandths, and where it came from (`flag` when `--min` set it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairsMin {
    pub units: i32,
    pub source: &'static str,
    pub calibrated: bool,
    pub run: Option<&'static str>,
}

/// One graph the call compares.
#[derive(Clone, Debug)]
struct Part {
    alias: String,
    instance: u64,
    /// Rows of this graph in the job's packed arrays.
    start: usize,
    end: usize,
    /// Its `vectors_digest`, taken in [`Services::pairs_copy`]'s hold with the rows (ws004 s36, F206).
    digest: [u8; 32],
}

/// Steps 1 and 2 of `umx similarity pairs` (see the module doc). Opaque, so the daemon carries it with no umx-graph
/// type (architect F1).
pub struct PairsJob {
    named: bool,
    key: Option<ModelKey>,
    model_source: ModelSource,
    k: usize,
    min: PairsMin,
    same_graph: bool,
    limit: usize,
    offset: usize,
    fields: PairRows,
    graphs: Vec<Part>,
    /// Graphs in scope left out: another space, or none of their nodes has a vector in this one yet.
    skipped_model: Vec<Skipped>,
    secured_skipped: usize,
    twice: Vec<String>,
    copies_skipped: Vec<String>,
    warnings: Vec<Problem>,
    dims: usize,
    values: Vec<i8>,
    inv: Vec<f32>,
    ids: Vec<Id>,
    /// Each row's graph, as its index in `graphs` (scope order).
    graph_of: Vec<u16>,
    /// Each graph's place in alias order, for ties.
    alias_rank: Vec<u16>,
}

/// One candidate in a node's list: the other row and its score.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    other: u32,
    score: i32,
}

/// Fixed k slots per row, best first.
struct Lists {
    k: usize,
    slots: Vec<Entry>,
    len: Vec<u8>,
}

/// One part's lists ([`PairsJob::part`]); [`PairsJob::merge`] joins them.
pub struct PairsPart {
    lists: Lists,
}

/// Every node's top k, from [`PairsJob::merge`], for [`Services::pairs_finish`].
pub struct PairsFound {
    job: PairsJob,
    lists: Lists,
    /// Every (row, rank - 1), best first: sorted here, with no lock held.
    order: Vec<(u32, u8)>,
}

/// A node the reply names: its graph (an index into [`PairsResult::graphs`]), id, type and title, and whether umx's
/// secret check flags that title (F228). Each node is resolved once, however many rows name it, so the finish hold
/// stays short at 250,000 rows (ws004 s34a). With [`PairRows::Ids`] the type and title are left empty, never read.
#[derive(Clone, Debug, PartialEq)]
pub struct PairNode {
    pub graph: u16,
    pub id: Id,
    pub kind: String,
    pub title: String,
    pub title_flagged: bool,
}

/// One row of the reply: a node, one of its top k in another graph (or its own with `--same-graph`), the score in
/// the node's list and the rank there (1 is best). `node` and `other` index [`PairsResult::names`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair {
    pub node: u32,
    pub other: u32,
    /// Ten-thousandths.
    pub score: i32,
    pub rank: u8,
}

/// A row with its names, borrowed from the result ([`PairsResult::rows`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PairRow<'a> {
    pub graph: &'a str,
    pub policy: &'static str,
    pub id: Id,
    pub kind: &'a str,
    pub title: &'a str,
    pub other_graph: &'a str,
    pub other_policy: &'static str,
    pub other_id: Id,
    pub other_kind: &'a str,
    pub other_title: &'a str,
    /// Ten-thousandths.
    pub score: i32,
    pub rank: usize,
    /// F228: whether umx's secret check flags each node's title (`field:title`, `field:other_title` in the reply).
    pub title_flagged: bool,
    pub other_title_flagged: bool,
}

/// A graph in scope that a call did not compare.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub graph: String,
    /// Its vector space, if it has one.
    pub model: Option<ModelKey>,
    /// `another model`, or `no vectors yet` (in the chosen space, or in any).
    pub reason: &'static str,
}

/// What `umx similarity pairs` answers.
#[derive(Clone, Debug, PartialEq)]
pub struct PairsResult {
    /// The page, best first.
    pub pairs: Vec<Pair>,
    /// The nodes the page names, each once.
    pub names: Vec<PairNode>,
    /// Rows found in all; `pairs` is the page `offset..offset + limit` of them.
    pub total: usize,
    pub truncated: bool,
    pub model: Option<ModelKey>,
    pub model_source: ModelSource,
    pub k: usize,
    pub min: PairsMin,
    pub graphs: Vec<String>,
    /// Each compared graph's policy word, in `graphs` order.
    pub policies: Vec<&'static str>,
    /// Each compared graph's `vectors_digest`, in `graphs` order: the digest of the vectors its rows were copied
    /// from, taken in the same read hold (ws004 s36, F206), so an edit and its undo inside one call cannot hide.
    pub digests: Vec<[u8; 32]>,
    /// Graphs in scope left out, each still readable by this call when it finished (security R298 B1).
    pub skipped_model: Vec<Skipped>,
    pub same_graph: bool,
    /// Nodes compared (with a vector in the key, provenance left out).
    pub nodes: usize,
    /// The fields each row carries, as asked.
    pub fields: PairRows,
}

impl PairsResult {
    /// The page's rows with their names.
    pub fn rows(&self) -> Vec<PairRow<'_>> {
        self.pairs.iter().map(|p| self.row(p)).collect()
    }

    pub fn row(&self, p: &Pair) -> PairRow<'_> {
        let (a, b) = (&self.names[p.node as usize], &self.names[p.other as usize]);
        PairRow {
            graph: &self.graphs[a.graph as usize],
            policy: self.policies[a.graph as usize],
            id: a.id,
            kind: &a.kind,
            title: &a.title,
            other_graph: &self.graphs[b.graph as usize],
            other_policy: self.policies[b.graph as usize],
            other_id: b.id,
            other_kind: &b.kind,
            other_title: &b.title,
            score: p.score,
            rank: p.rank.into(),
            title_flagged: a.title_flagged,
            other_title_flagged: b.title_flagged,
        }
    }
}

impl Lists {
    fn new(k: usize, rows: usize) -> Lists {
        Lists {
            k,
            slots: vec![Entry { other: 0, score: 0 }; rows * k],
            len: vec![0; rows],
        }
    }

    fn list(&self, h: usize) -> &[Entry] {
        &self.slots[h * self.k..h * self.k + self.len[h] as usize]
    }

    /// The worst score row `h` still takes: `floor` until full, then its last entry's.
    fn need(&self, h: usize, floor: i32) -> i32 {
        if (self.len[h] as usize) < self.k {
            floor
        } else {
            self.slots[h * self.k + self.k - 1].score
        }
    }

    /// Puts `e` into row `h` in order if it belongs in a top k.
    fn offer(&mut self, h: usize, e: Entry, better: &impl Fn(Entry, Entry) -> bool) {
        let (k, len) = (self.k, self.len[h] as usize);
        if len == k && !better(e, self.slots[h * k + k - 1]) {
            return;
        }
        let row = &mut self.slots[h * k..h * k + k];
        let at = row[..len].partition_point(|x| better(*x, e));
        let end = (len + 1).min(k);
        row.copy_within(at..end - 1, at + 1);
        row[at] = e;
        self.len[h] = end as u8;
    }
}

impl PairsJob {
    /// How many graphs [`Services::pairs_copy`] must copy, one per read hold.
    pub fn graphs(&self) -> usize {
        self.graphs.len()
    }

    fn rows(&self) -> usize {
        self.ids.len()
    }

    /// Best first: the higher score, then the lower other id, then the lower other alias.
    fn better(&self) -> impl Fn(Entry, Entry) -> bool + '_ {
        move |a: Entry, b: Entry| {
            if a.score != b.score {
                return a.score > b.score;
            }
            let (ia, ib) = (self.ids[a.other as usize], self.ids[b.other as usize]);
            if ia != ib {
                return ia < ib;
            }
            self.alias_rank[self.graph_of[a.other as usize] as usize]
                < self.alias_rank[self.graph_of[b.other as usize] as usize]
        }
    }

    /// The first row each row is compared with (every later row is too): the next graph's first row, or with
    /// `--same-graph` the next row. Each pair is taken once, by its lower row, and scored for both ends.
    fn first_candidate(&self, a: usize) -> usize {
        if self.same_graph {
            a + 1
        } else {
            self.graphs[self.graph_of[a] as usize].end
        }
    }

    /// Step 2 for the rows `a` with `a % parts == part`, with no lock held; None when `cancel` said stop (checked
    /// every few rows). Pure.
    pub fn part(&self, part: usize, parts: usize, cancel: &dyn Fn() -> bool) -> Option<PairsPart> {
        let n = self.rows();
        let mut lists = Lists::new(self.k, n);
        let better = self.better();
        let floor = self.min.units;
        let dims = self.dims;
        let row = |h: usize| &self.values[h * dims..(h + 1) * dims];
        for (step, a) in (part..n).step_by(parts.max(1)).enumerate() {
            if step % 16 == 0 && cancel() {
                return None;
            }
            let (row_a, inv_a, id_a) = (row(a), self.inv[a], self.ids[a]);
            for b in self.first_candidate(a)..n {
                // A copy's node is the same node: never its own pair (d-21).
                if self.ids[b] == id_a {
                    continue;
                }
                let inv_b = self.inv[b];
                let d = dot(row_a, row(b));
                // An f32 estimate first, as the stored lists' rebuild: a pair more than a unit below what both
                // lists need cannot enter either, so the exact score is only taken near the cut.
                let rough = d as f32 * inv_a * inv_b * SCALE as f32;
                if rough < lists.need(a, floor).min(lists.need(b, floor)) as f32 - 1.0 {
                    continue;
                }
                for (x, y, inv_x, inv_y) in [(a, b, inv_a, inv_b), (b, a, inv_b, inv_a)] {
                    let score = score_units(d, inv_x, inv_y);
                    if score >= floor {
                        lists.offer(
                            x,
                            Entry {
                                other: y as u32,
                                score,
                            },
                            &better,
                        );
                    }
                }
            }
        }
        Some(PairsPart { lists })
    }

    /// Joins the parts' lists into every node's top k: the same lists, in the same order, for any number of parts.
    pub fn merge(self, parts: Vec<PairsPart>) -> PairsFound {
        let n = self.rows();
        let mut lists = Lists::new(self.k, n);
        {
            let better = self.better();
            for p in &parts {
                for h in 0..n {
                    for &e in p.lists.list(h) {
                        lists.offer(h, e, &better);
                    }
                }
            }
        }
        let mut order: Vec<(u32, u8)> = (0..n)
            .flat_map(|h| (0..lists.len[h]).map(move |r| (h as u32, r)))
            .collect();
        // Best first; then the node's graph (scope order), its id and the rank.
        order.sort_by(|&(ha, ra), &(hb, rb)| {
            let (ha, hb) = (ha as usize, hb as usize);
            let (ea, eb) = (lists.list(ha)[ra as usize], lists.list(hb)[rb as usize]);
            eb.score
                .cmp(&ea.score)
                .then(self.graph_of[ha].cmp(&self.graph_of[hb]))
                .then(self.ids[ha].cmp(&self.ids[hb]))
                .then(ra.cmp(&rb))
        });
        PairsFound {
            job: self,
            lists,
            order,
        }
    }

    fn check_caps(&self, per_graph: &[usize]) -> Result<(), Problem> {
        caps(per_graph, self.dims, self.same_graph)
    }
}

/// Both caps ([`MAX_PAIR_DOTS`], [`MAX_PAIR_BYTES`]) for graphs of `per_graph` rows each.
fn caps(per_graph: &[usize], dims: usize, same_graph: bool) -> Result<(), Problem> {
    let n: u64 = per_graph.iter().map(|&r| r as u64).sum();
    let bytes = n.saturating_mul(dims as u64 + ROW_EXTRA);
    let dots = if same_graph {
        n.saturating_mul(n.saturating_sub(1)) / 2
    } else {
        let squares: u64 = per_graph
            .iter()
            .map(|&r| (r as u64).saturating_mul(r as u64))
            .sum();
        n.saturating_mul(n).saturating_sub(squares) / 2
    };
    if dots > MAX_PAIR_DOTS {
        return Err(problems::similarity_pairs_too_large(
            n as usize,
            dims,
            dots,
            MAX_PAIR_DOTS,
            "pair comparisons",
        ));
    }
    if bytes > MAX_PAIR_BYTES {
        return Err(problems::similarity_pairs_too_large(
            n as usize,
            dims,
            bytes,
            MAX_PAIR_BYTES,
            "copied bytes",
        ));
    }
    Ok(())
}

/// `similarity` is documented in the embeddings topic, so its usage errors point there.
const NOUN: &str = "embeddings";

fn usage(what: &'static str) -> Problem {
    problems::usage_invalid(what, NOUN)
}

impl crate::Services {
    /// Step 1, under a short read hold: the graphs in scope, the key, the floor and the caps. Copies nothing.
    pub fn pairs_plan(&self, ctx: &Ctx, ask: &PairsAsk) -> Result<PairsJob, Problem> {
        let store = &self.graph;
        let k = ask.k.unwrap_or(PAIRS_DEFAULT_K);
        if k == 0 || k > PAIRS_MAX_K {
            return Err(usage("--k must be from 1 to 10"));
        }
        let limit = ask.limit.unwrap_or(PAIRS_DEFAULT_LIMIT);
        if limit == 0 || limit > PAIRS_MAX_LIMIT {
            return Err(usage("--limit must be from 1 to 250000"));
        }
        let flag_key = match &ask.model {
            Some(text) => Some(
                ModelKey::parse(text)
                    .map_err(|_| usage("--model is not a model key (model@dims)"))?,
            ),
            None => None,
        };
        let flag_min = match &ask.min {
            Some(text) => Some(
                parse_units(text)
                    .filter(|u| (-10_000..=10_000).contains(u))
                    .ok_or_else(|| {
                        usage("--min is a score from -1.0000 to 1.0000 with 4 decimals")
                    })?,
            ),
            None => None,
        };
        let scope = store.graphs_in_scope(ctx, None, NOUN)?;
        let all = matches!(ctx.scope, Some(Scope::All { .. }));
        let named = scope.named && !all;
        // Each graph's space and how many of its nodes have a whole-node vector there. Only graphs with vectors
        // vote for the space (embeddings-expert R298): an empty graph never decides it.
        let mut spaces: Vec<(String, Option<ModelKey>, usize)> = Vec::new();
        for alias in &scope.aliases {
            let loaded = store.loaded(alias).ok_or_else(|| store.not_loaded(alias))?;
            let space = self.embedding.block_key(store, alias);
            let rows = space
                .as_ref()
                .and_then(|k| loaded.graph.whole_block(&k.to_string()))
                .map_or(0, |b| b.count());
            spaces.push((alias.clone(), space, rows));
        }
        let voters: Vec<(String, Option<ModelKey>)> = spaces
            .iter()
            .filter(|(_, _, rows)| *rows > 0)
            .map(|(a, k, _)| (a.clone(), k.clone()))
            .collect();
        let (key, model_source) = match flag_key {
            Some(key) => (Some(key), ModelSource::Flag),
            None if named => (
                voters.iter().find_map(|(_, k)| k.clone()),
                ModelSource::NamedFirst,
            ),
            None => (most_graphs(&voters), ModelSource::MostGraphs),
        };
        if key.is_none() {
            return Err(usage(
                "no graph in scope has vectors yet (embed one first, or name a model with --model)",
            ));
        }
        let mut graphs = Vec::new();
        let mut skipped_model = Vec::new();
        let mut per_graph = Vec::new();
        for (alias, space, rows) in spaces {
            if space == key && rows > 0 {
                let loaded = store
                    .loaded(&alias)
                    .ok_or_else(|| store.not_loaded(&alias))?;
                per_graph.push(rows);
                graphs.push(Part {
                    alias,
                    instance: loaded.instance,
                    start: 0,
                    end: 0,
                    digest: [0; 32],
                });
            } else {
                let reason = if space.is_some() && space != key && rows > 0 {
                    "another model"
                } else {
                    "no vectors yet"
                };
                skipped_model.push(Skipped {
                    graph: alias,
                    model: space,
                    reason,
                });
            }
        }
        // similarity_model_mismatch is built at finish, from the graphs still readable then (security R298 B1).
        let mut warnings = Vec::new();
        let min = match (flag_min, &key) {
            (Some(units), _) => PairsMin {
                units,
                source: "flag",
                calibrated: false,
                run: None,
            },
            // The stored lists' default floor for the key, without any one graph's graph.json floor: a pair spans two.
            (None, Some(key)) => {
                let (floor, _) = Settings::of(key, None, store.floors());
                if floor.source == FloorSource::None {
                    warnings.push(problems::floor_uncalibrated(key, "similarity pairs"));
                }
                PairsMin {
                    units: floor.floor,
                    source: floor.source.word(),
                    calibrated: floor.calibrated(),
                    run: floor.run,
                }
            }
            (None, None) => PairsMin {
                units: 0,
                source: FloorSource::None.word(),
                calibrated: false,
                run: None,
            },
        };
        let mut by_alias: Vec<usize> = (0..graphs.len()).collect();
        by_alias.sort_by(|&a, &b| graphs[a].alias.cmp(&graphs[b].alias));
        let mut alias_rank = vec![0u16; graphs.len()];
        for (rank, &g) in by_alias.iter().enumerate() {
            alias_rank[g] = rank as u16;
        }
        let job = PairsJob {
            named: scope.named,
            dims: key.as_ref().map_or(0, ModelKey::dims),
            key,
            model_source,
            k,
            min,
            same_graph: ask.same_graph,
            limit,
            offset: ask.offset.unwrap_or(0),
            fields: ask.fields,
            graphs,
            skipped_model,
            secured_skipped: scope.skipped,
            twice: scope.twice,
            copies_skipped: scope.copies_skipped,
            warnings,
            values: Vec::new(),
            inv: Vec::new(),
            ids: Vec::new(),
            graph_of: Vec::new(),
            alias_rank,
        };
        job.check_caps(&per_graph)?;
        Ok(job)
    }

    /// One graph's rows, under its own short read hold: only rows holding a vector, packed, provenance left out, and
    /// the graph's `vectors_digest` of those vectors, through `memo` (the one source `embeddings status` uses; ws004
    /// s36). `Ok(false)` when the graph was unloaded or loaded again since the plan (plan again).
    pub fn pairs_copy(
        &self,
        job: &mut PairsJob,
        graph: usize,
        memo: &mut crate::DigestMemo,
    ) -> Result<bool, Problem> {
        let Some(loaded) = self.graph.loaded(&job.graphs[graph].alias) else {
            return Ok(false);
        };
        if loaded.instance != job.graphs[graph].instance {
            return Ok(false);
        }
        let start = job.ids.len();
        let key = job.key.as_ref().map(ToString::to_string);
        if let Some(block) = key.as_deref().and_then(|k| loaded.graph.whole_block(k)) {
            let g = &loaded.graph;
            let dims = block.dims();
            if dims != job.dims {
                return Ok(false);
            }
            // Check before copying what this graph adds: it may have grown since the plan.
            let mut per_graph: Vec<usize> = job.graphs[..graph]
                .iter()
                .map(|p| p.end - p.start)
                .collect();
            per_graph.push(block.count());
            job.check_caps(&per_graph)?;
            let (values, inv) = (block.values(), block.inverse_lengths());
            for r in (0..block.rows()).filter(|&r| block.has(r)) {
                let ix = block.node(r as u32);
                if g.is_provenance(ix) {
                    continue;
                }
                job.values
                    .extend_from_slice(&values[r * dims..(r + 1) * dims]);
                job.inv.push(inv[r]);
                job.ids.push(g.node_at(ix).id());
                job.graph_of.push(graph as u16);
            }
        }
        // The digest of what was just copied: the same graph, space and generation, inside this hold.
        let stamp = job.key.clone().map(|key| crate::VectorsStamp {
            key,
            instance: loaded.instance,
            generation: loaded.graph.vectors_generation(),
        });
        let Some(digest) =
            stamp.and_then(|s| self.vectors_digest_kept(&job.graphs[graph].alias, &s, memo))
        else {
            return Ok(false);
        };
        job.graphs[graph].start = start;
        job.graphs[graph].end = job.ids.len();
        job.graphs[graph].digest = digest;
        Ok(true)
    }

    /// Step 3, under a short read hold: re-checks scope (a graph unloaded, loaded again or no longer readable since
    /// step 1 means `Ok(None)`: run again), then the page of rows, best first, with each node's type and title
    /// ([`PairRows::Full`]) or only checked to be there ([`PairRows::Ids`]). A node removed meanwhile is left out.
    pub fn pairs_finish(
        &self,
        ctx: &Ctx,
        found: PairsFound,
    ) -> Result<Option<Done<PairsResult>>, Problem> {
        let store = &self.graph;
        let job = &found.job;
        let now = store.graphs_in_scope(ctx, None, NOUN)?;
        for p in &job.graphs {
            let same = store
                .loaded(&p.alias)
                .is_some_and(|l| l.instance == p.instance);
            if !same
                || !now.aliases.contains(&p.alias)
                || (!job.named && !store.shown_unnamed(&p.alias))
            {
                return Ok(None);
            }
        }
        let total = found.order.len();
        let policies: Vec<&'static str> = job
            .graphs
            .iter()
            .map(|p| {
                store
                    .effective_policy(&p.alias)
                    .map_or("secured", |p| p.name())
            })
            .collect();
        let loaded: Vec<_> = job.graphs.iter().map(|p| store.loaded(&p.alias)).collect();
        // Each job row's place in `names`, resolved the first time a row names it: UNSEEN, GONE (removed since the
        // copy; its rows are left out) or the index.
        const UNSEEN: u32 = u32::MAX;
        const GONE: u32 = u32::MAX - 1;
        let mut slot = vec![UNSEEN; job.rows()];
        let mut names: Vec<PairNode> = Vec::new();
        let allows = core::cell::OnceCell::new();
        let mut name = |row: usize| -> Option<u32> {
            if slot[row] == UNSEEN {
                let graph = job.graph_of[row];
                let l = loaded[graph as usize];
                slot[row] = match l.and_then(|l| l.graph.node(job.ids[row]).map(|n| (l, n))) {
                    Some((l, n)) => {
                        let (kind, title) = match job.fields {
                            PairRows::Full => (n.kind().to_string(), n.title().to_string()),
                            PairRows::Ids => (String::new(), String::new()),
                        };
                        // F228: as node get flags the title; an ids row shows none.
                        let title_flagged =
                            store.title_flagged(&allows, l.meta.id, job.ids[row], &title);
                        names.push(PairNode {
                            graph,
                            id: job.ids[row],
                            kind,
                            title,
                            title_flagged,
                        });
                        (names.len() - 1) as u32
                    }
                    None => GONE,
                };
            }
            (slot[row] != GONE).then_some(slot[row])
        };
        let page = found.order.iter().skip(job.offset).take(job.limit);
        let mut pairs = Vec::with_capacity(page.len());
        for &(h, r) in page {
            let e = found.lists.list(h as usize)[r as usize];
            if let (Some(node), Some(other)) = (name(h as usize), name(e.other as usize)) {
                pairs.push(Pair {
                    node,
                    other,
                    score: e.score,
                    rank: r + 1,
                });
            }
        }
        let mut warnings = job.warnings.clone();
        // A left-out graph is named only while an unnamed read may still show it; one secured (or unloaded) since
        // the plan is counted with the secured graphs, never named (security R298 B1).
        let (skipped, hidden): (Vec<Skipped>, Vec<Skipped>) =
            job.skipped_model.iter().cloned().partition(|s| {
                job.named || (store.loaded(&s.graph).is_some() && store.shown_unnamed(&s.graph))
            });
        if let (Some(key), false) = (&job.key, skipped.is_empty()) {
            let left: Vec<String> = skipped.iter().map(|s| s.graph.clone()).collect();
            warnings.push(problems::similarity_model_mismatch(key, &left));
        }
        // d-21: other copies are named only while still open; one secured meanwhile is counted, never named.
        let others: Vec<String> = job
            .copies_skipped
            .iter()
            .filter(|c| store.shown_unnamed(c))
            .cloned()
            .collect();
        let secured =
            job.secured_skipped + (job.copies_skipped.len() - others.len()) + hidden.len();
        if secured > 0 {
            warnings.push(problems::secured_graphs_skipped(secured));
        }
        if !job.twice.is_empty() {
            warnings.push(problems::graph_twice(&job.twice));
        }
        Ok(Some(Done {
            value: PairsResult {
                truncated: total > job.offset.saturating_add(job.limit),
                total,
                pairs,
                names,
                model: job.key.clone(),
                model_source: job.model_source,
                k: job.k,
                min: job.min,
                graphs: job.graphs.iter().map(|p| p.alias.clone()).collect(),
                policies,
                digests: job.graphs.iter().map(|p| p.digest).collect(),
                skipped_model: skipped,
                same_graph: job.same_graph,
                nodes: job.rows(),
                fields: job.fields,
            },
            notes: warnings,
        }))
    }

    /// All three steps on this thread, as one part (tests and benches; the daemon releases its lock between steps).
    /// Digests are hashed afresh: no memo outlives the call.
    pub fn similarity_pairs(&self, ctx: &Ctx, ask: &PairsAsk) -> crate::Outcome<PairsResult> {
        let mut job = self.pairs_plan(ctx, ask)?;
        let mut memo = crate::DigestMemo::new();
        for g in 0..job.graphs() {
            if !self.pairs_copy(&mut job, g, &mut memo)? {
                return Err(usage("graphs changed during the read (retry)"));
            }
        }
        let part = job.part(0, 1, &|| false).expect("never cancelled");
        self.pairs_finish(ctx, job.merge(alloc::vec![part]))?
            .ok_or_else(|| usage("graphs changed during the read (retry)"))
    }
}

/// The key most graphs record; ties go to the key of the lowest alias.
fn most_graphs(recorded: &[(String, Option<ModelKey>)]) -> Option<ModelKey> {
    let mut counts: BTreeMap<&ModelKey, (usize, &str)> = BTreeMap::new();
    for (alias, key) in recorded {
        if let Some(key) = key {
            let e = counts.entry(key).or_insert((0, alias.as_str()));
            e.0 += 1;
            if alias.as_str() < e.1 {
                e.1 = alias.as_str();
            }
        }
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.0.cmp(&b.1.0).then(b.1.1.cmp(a.1.1)))
        .map(|(k, _)| k.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(r: Result<(), Problem>) -> Option<&'static str> {
        r.err()
            .map(|p| match p.slots.iter().find(|(n, _)| *n == "unit") {
                Some((_, umx_core::Slot::Word(w))) => *w,
                _ => "?",
            })
    }

    #[test]
    fn caps_count_cross_pairs_and_copied_bytes() {
        // 2 x 44k cross-only is 1.94e9 pairs: in. 2 x 45k is 2.03e9: out.
        assert_eq!(unit(caps(&[44_000, 44_000], 256, false)), None);
        assert_eq!(
            unit(caps(&[45_000, 45_000], 256, false)),
            Some("pair comparisons")
        );
        // The same nodes with same-graph pairs count every pair.
        assert_eq!(
            unit(caps(&[32_000, 32_000], 256, true)),
            Some("pair comparisons")
        );
        assert_eq!(unit(caps(&[32_000, 32_000], 256, false)), None);
        // 3072 dims: 175k rows copy 541 MB, past 512 MiB, with few pairs (one big graph and a small one).
        assert_eq!(
            unit(caps(&[175_000, 10], 3072, false)),
            Some("copied bytes")
        );
        assert_eq!(unit(caps(&[170_000, 10], 3072, false)), None);
    }
}
