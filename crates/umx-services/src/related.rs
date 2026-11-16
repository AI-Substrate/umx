//! Related nodes on write (PRD 001 req-123, D80, ws004 s28): once node add or batch has written its nodes, each new
//! node's whole-node text is embedded once and up to [`RELATED_K`] existing nodes of the same graph whose stored
//! whole-node vectors score at or above the model's floor come back as `related`, with their scores. Separate from the
//! word-based `near_duplicate`, and never called a duplicate.
//!
//! Like search, three steps, so the embed holds no lock and never holds the writer: [`Services::related_plan`] under
//! the shared lock, the embed with none ([`RelatedPlan::embed`], or the daemon's bounded query lane), and
//! [`Services::related_finish`] under the shared lock again. It never fails the write: whatever stops it for a node is
//! one `related_skipped` hint per reason, saying why and how to turn it on.

use crate::hybrid::QueryFailed;
use crate::{Services, problems};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use umx_core::ports::{Embedder, GraphDir, Locality, ModelKey, TextKind};
use umx_core::similarity::{Floor, FloorSource, Floors, Use, dot, inverse_length};
use umx_core::vectors::{quantise, whole_node_field};
use umx_core::{Command, Id, Policy, Problem, ValidName, secrets};
use umx_graph::NodeIx;

/// At most this many related nodes per new node (req-123).
pub const RELATED_K: usize = 3;
/// At most this many new nodes are checked per call, in one embed request, by an instance that embeds about 8 texts
/// a second or more (Metal, CUDA, a hosted one): about 2 s of its work (embeddings-expert, ws004 s28). The rest get a
/// `related_skipped` hint.
pub const RELATED_CAP: usize = 16;
/// The cap for a local model on the CPU (the F32 path, about 3.6 texts a second): about 2 s of its work.
pub const RELATED_CAP_CPU: usize = 4;

/// How a new node's text is embedded for the check: as a query, as search embeds one. For a key with no query
/// prompt the two kinds give the same vector.
pub fn related_kind() -> TextKind {
    TextKind::Query
}

/// Which floor cuts related scores for `key` (embeddings-expert, ws004 s28): a key with a query prompt (`+pN`) embeds
/// the new node as a query against stored documents, which the search floor is calibrated on; a key with none gives
/// the same vector either way, so its scores are document to document and the similar_to floor applies (the search
/// floor would let about 40% of random nodes through on text-embedding-3-large).
pub fn related_use(key: &ModelKey) -> Use {
    if key.prompt_version().is_some() {
        Use::Search
    } else {
        Use::Similar
    }
}

/// The floor a related score must reach for `key`: the machine config's floor for [`related_use`], else the
/// calibrated one.
pub fn related_floor(floors: &Floors, key: &ModelKey) -> Floor {
    floors.get(key, related_use(key))
}

/// Why the check did not run for a node, in the order hints list them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Skip {
    /// `--no-related` on this call.
    OffFlag,
    /// The graph's `related` setting is off.
    OffGraph,
    /// The machine's `related` setting is off.
    OffMachine,
    /// No embedder instance is configured.
    NoEmbedder,
    /// No configured instance may embed this graph here (its embedder setting, its folder's config, or its model).
    NotAdmitted,
    /// The model has no calibrated search floor and the machine config sets none.
    NoFloor,
    /// The graph holds no other node's vector yet.
    NoVectors,
    /// The node's text looks like it holds a secret and the instance is hosted (req-49): never sent.
    Secret,
    /// Past [`RELATED_CAP`] new nodes in one call.
    OverCap,
    /// The embed took longer than its bound.
    TimedOut,
    /// The embedder failed.
    EmbedFailed,
}

impl Skip {
    /// Every reason, in hint order.
    pub const ALL: [Skip; 11] = [
        Skip::OffFlag,
        Skip::OffGraph,
        Skip::OffMachine,
        Skip::NoEmbedder,
        Skip::NotAdmitted,
        Skip::NoFloor,
        Skip::NoVectors,
        Skip::Secret,
        Skip::OverCap,
        Skip::TimedOut,
        Skip::EmbedFailed,
    ];

    /// The hint's `<reason>` words.
    pub fn words(self) -> &'static str {
        match self {
            Skip::OffFlag => {
                "--no-related turned it off for this call; give --related on the next one"
            }
            Skip::OffGraph => "the graph's related setting is off",
            Skip::OffMachine => "this machine's related setting is off",
            Skip::NoEmbedder => "no embedder is configured",
            Skip::NotAdmitted => "no configured embedder may embed this graph here",
            Skip::NoFloor => {
                "the embedder's model has no calibrated floor; set one in the config (umx config show lists the instances)"
            }
            Skip::NoVectors => "no other node of the graph has a vector yet",
            Skip::Secret => "its text looks like it holds a secret and the embedder is hosted",
            Skip::OverCap => {
                "a call checks only as many new nodes as the embedder embeds in about 2 s"
            }
            Skip::TimedOut => "the embedder did not answer in time",
            Skip::EmbedFailed => "the embedder failed",
        }
    }

    fn of(failed: &QueryFailed) -> Skip {
        match failed {
            QueryFailed::TimedOut { .. } => Skip::TimedOut,
            QueryFailed::Secret => Skip::Secret,
            QueryFailed::NotAdmitted => Skip::NotAdmitted,
            QueryFailed::Embed(_) => Skip::EmbedFailed,
        }
    }
}

/// Whether the check runs, by the one precedence (D80): the call's flag, else the graph's setting, else the
/// machine's (on unless set off).
pub fn switch(flag: Option<bool>, graph: Option<bool>, machine: bool) -> Result<(), Skip> {
    match (flag, graph) {
        (Some(true), _) => Ok(()),
        (Some(false), _) => Err(Skip::OffFlag),
        (None, Some(false)) => Err(Skip::OffGraph),
        (None, Some(true)) => Ok(()),
        (None, None) if machine => Ok(()),
        (None, None) => Err(Skip::OffMachine),
    }
}

/// What one call asks: the graph written, the nodes it added (in order), the call's flag, the machine's setting and
/// the verb (`node add` or `batch`) the fixes name.
#[derive(Clone, Debug)]
pub struct RelatedRequest<'a> {
    pub alias: &'a str,
    pub added: &'a [Id],
    pub flag: Option<bool>,
    pub machine: bool,
    pub verb: &'static str,
}

/// One existing node close in meaning to a new one.
#[derive(Clone, Debug, PartialEq)]
pub struct RelatedRow {
    pub id: Id,
    pub title: String,
    pub score: f32,
    /// F228: umx's secret check flags the title.
    pub title_flagged: bool,
}

/// The check's result for one call: each checked node's rows (possibly none), the floor that cut them (when any node
/// was checked), and the hints for the rest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Related {
    pub alias: String,
    pub rows: BTreeMap<Id, Vec<RelatedRow>>,
    pub floor: Option<RelatedFloor>,
    pub hints: Vec<Problem>,
}

/// The floor a related check used, for the reply: its value, where it came from and which use it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RelatedFloor {
    pub value: f32,
    pub source: FloorSource,
    pub use_: Use,
}

impl RelatedFloor {
    /// `search` or `similar_to`, as the floors are named elsewhere.
    pub fn use_word(&self) -> &'static str {
        match self.use_ {
            Use::Search => "search",
            Use::Similar => "similar_to",
        }
    }
}

/// Step one's result when there is something to embed.
pub struct RelatedPlan {
    alias: String,
    verb: &'static str,
    instance: String,
    embedder: Arc<dyn Embedder>,
    key: ModelKey,
    floor: RelatedFloor,
    checked: Vec<Id>,
    texts: Vec<String>,
    added: BTreeSet<Id>,
    skipped: BTreeMap<Skip, usize>,
}

impl RelatedPlan {
    pub fn embedder(&self) -> &Arc<dyn Embedder> {
        &self.embedder
    }

    pub fn instance(&self) -> &str {
        &self.instance
    }

    /// The whole-node texts to embed, one per checked node, in order.
    pub fn texts(&self) -> &[String] {
        &self.texts
    }

    /// One embed request for every checked node, unbounded (the daemon bounds it through its query lane).
    pub fn embed(&self) -> Result<Vec<Vec<f32>>, QueryFailed> {
        self.embedder
            .embed(&self.texts, related_kind())
            .map(|e| e.vectors)
            .map_err(QueryFailed::Embed)
    }
}

impl Services {
    /// Step one, under the shared lock: which added nodes are checked and with which instance, or the finished
    /// result (hints only) when nothing is to be embedded. `allows(folder, instance)` is the daemon's folder rule.
    pub fn related_plan(
        &self,
        req: &RelatedRequest<'_>,
        allows: &dyn Fn(&GraphDir, &str) -> bool,
    ) -> Result<RelatedPlan, Related> {
        let store = &self.graph;
        let mut done = Related {
            alias: req.alias.to_string(),
            ..Related::default()
        };
        let Some(loaded) = store.loaded(req.alias) else {
            return Err(done);
        };
        // Only a node with a whole-node text is checked: a provenance record or a node with no text has nothing to
        // compare, so it is left out silently.
        let whole = whole_node_field();
        let mut eligible: Vec<(Id, String)> = Vec::new();
        for &id in req.added {
            if eligible.iter().any(|(e, _)| *e == id) {
                continue;
            }
            if let Some(text) = store
                .node_texts(req.alias, id)
                .and_then(|t| t.into_iter().find(|(f, _)| *f == whole))
            {
                eligible.push((id, text.1));
            }
        }
        if eligible.is_empty() {
            return Err(done);
        }
        let all = eligible.len();
        let graph_setting = loaded.meta.embedding.as_ref().and_then(|e| e.related);
        let skip_all = |done: &mut Related, why: Skip, extra: Option<(&str, Use)>| {
            done.hints.push(hint(req.alias, req.verb, why, all, extra));
        };
        if let Err(why) = switch(req.flag, graph_setting, req.machine) {
            skip_all(&mut done, why, None);
            return Err(done);
        }
        if self.embedding.instances().is_empty() {
            skip_all(&mut done, Skip::NoEmbedder, None);
            return Err(done);
        }
        let policy = store.effective_policy(req.alias).unwrap_or(Policy::SECURED);
        let instance = self
            .embedding
            .admits_any(policy.embedder)
            .then(|| {
                self.embedding
                    .instance_for_where(store, req.alias, policy, &|n| allows(&loaded.dir, n))
            })
            .flatten();
        let block = self.embedding.block_key(store, req.alias);
        let (Some(instance), Some(key)) = (instance, block) else {
            skip_all(&mut done, Skip::NotAdmitted, None);
            return Err(done);
        };
        if key != instance.embedder.key() {
            skip_all(&mut done, Skip::NotAdmitted, None);
            return Err(done);
        }
        let floor = related_floor(store.floors(), &key);
        if floor.source == FloorSource::None {
            skip_all(
                &mut done,
                Skip::NoFloor,
                Some((instance.name.as_str(), related_use(&key))),
            );
            return Err(done);
        }
        let cap = self.embedding.related_cap(&instance.name);
        let added: BTreeSet<Id> = eligible.iter().map(|(id, _)| *id).collect();
        let graph = &loaded.graph;
        let added_ix: BTreeSet<NodeIx> = added.iter().filter_map(|id| graph.node_ix(*id)).collect();
        let space = key.to_string();
        let any_other = graph.whole_block(&space).is_some_and(|w| {
            (0..w.rows()).any(|i| {
                let n = w.node(i as u32);
                w.has(i) && !graph.is_provenance(n) && !added_ix.contains(&n)
            })
        });
        if !any_other {
            skip_all(&mut done, Skip::NoVectors, None);
            return Err(done);
        }
        let hosted = instance.embedder.locality() != Locality::Local;
        let mut skipped: BTreeMap<Skip, usize> = BTreeMap::new();
        let (mut checked, mut texts) = (Vec::new(), Vec::new());
        for (id, text) in eligible {
            // req-49: a text that looks like it holds a secret never goes to a hosted embedder.
            if hosted && secrets::scan(&text).is_some() {
                *skipped.entry(Skip::Secret).or_default() += 1;
            } else if checked.len() == cap {
                *skipped.entry(Skip::OverCap).or_default() += 1;
            } else {
                checked.push(id);
                texts.push(text);
            }
        }
        if checked.is_empty() {
            done.hints = skipped
                .iter()
                .map(|(why, n)| hint(req.alias, req.verb, *why, *n, None))
                .collect();
            return Err(done);
        }
        Ok(RelatedPlan {
            alias: req.alias.to_string(),
            verb: req.verb,
            instance: instance.name.clone(),
            embedder: instance.embedder.clone(),
            floor: RelatedFloor {
                value: floor.value,
                source: floor.source,
                use_: related_use(&key),
            },
            key,
            checked,
            texts,
            added,
            skipped,
        })
    }

    /// Step three, under the shared lock: each checked node's vector scored against the graph's stored whole-node
    /// vectors in the plan's space: int8 dot as search scores, at or above the floor, best first, ties to the lower
    /// id, at most [`RELATED_K`], never the node itself or another node the same call added. The instance must
    /// still be admitted and the graph's space unchanged (re-checked here), else nothing is scored.
    pub fn related_finish(
        &self,
        plan: RelatedPlan,
        vectors: Result<Vec<Vec<f32>>, QueryFailed>,
        allows: &dyn Fn(&GraphDir, &str) -> bool,
    ) -> Related {
        let mut done = Related {
            alias: plan.alias.clone(),
            ..Related::default()
        };
        let mut skipped = plan.skipped.clone();
        let store = &self.graph;
        let still = store.loaded(&plan.alias).is_some_and(|l| {
            self.embedding
                .admits_now(store, &plan.alias, &plan.instance)
                && allows(&l.dir, &plan.instance)
                && self.embedding.block_key(store, &plan.alias).as_ref() == Some(&plan.key)
        });
        let vectors = match vectors {
            _ if !still => Err(Skip::NotAdmitted),
            Ok(v) if v.len() == plan.checked.len() => Ok(v),
            Ok(_) => Err(Skip::EmbedFailed),
            Err(failed) => Err(Skip::of(&failed)),
        };
        match vectors {
            Err(why) => *skipped.entry(why).or_default() += plan.checked.len(),
            Ok(vectors) => {
                let loaded = store.loaded(&plan.alias).expect("checked above");
                let graph = &loaded.graph;
                let added: BTreeSet<NodeIx> = plan
                    .added
                    .iter()
                    .filter_map(|id| graph.node_ix(*id))
                    .collect();
                let block = graph.whole_block(&plan.key.to_string());
                for (id, v) in plan.checked.iter().zip(vectors) {
                    let query = quantise(&v);
                    let Some(w) = block.filter(|w| w.dims() == query.len()) else {
                        *skipped.entry(Skip::EmbedFailed).or_default() += 1;
                        continue;
                    };
                    let q = inverse_length(&query);
                    let inv = w.inverse_lengths();
                    let mut hits: Vec<(f32, Id, NodeIx)> = w
                        .values()
                        .chunks_exact(query.len())
                        .enumerate()
                        .filter_map(|(i, row)| {
                            let n = w.node(i as u32);
                            if !w.has(i) || graph.is_provenance(n) || added.contains(&n) {
                                return None;
                            }
                            // The score search's top_k gives: the dot times both inverse lengths.
                            let score = dot(&query, row) as f32 * q * inv[i];
                            (q != 0.0 && score >= plan.floor.value)
                                .then(|| (score, graph.node_at(n).id(), n))
                        })
                        .collect();
                    hits.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
                    hits.truncate(RELATED_K);
                    let titles: Vec<(crate::GraphKey<'_>, Id, &str)> = hits
                        .iter()
                        .map(|(_, id, n)| {
                            (
                                crate::GraphKey::Alias(plan.alias.as_str()),
                                *id,
                                graph.node_at(*n).title(),
                            )
                        })
                        .collect();
                    let flagged = self.titles_flagged(&titles);
                    let rows = hits
                        .iter()
                        .zip(flagged)
                        .map(|((score, id, n), title_flagged)| RelatedRow {
                            id: *id,
                            title: graph.node_at(*n).title().to_string(),
                            score: *score,
                            title_flagged,
                        })
                        .collect();
                    done.rows.insert(*id, rows);
                }
                done.floor = (!done.rows.is_empty()).then_some(plan.floor);
            }
        }
        done.hints = skipped
            .iter()
            .map(|(why, n)| hint(&plan.alias, plan.verb, *why, *n, None))
            .collect();
        done
    }

    /// The three steps in one, embedding directly (no time bound): for callers without a query lane.
    pub fn related(
        &self,
        req: &RelatedRequest<'_>,
        allows: &dyn Fn(&GraphDir, &str) -> bool,
    ) -> Related {
        match self.related_plan(req, allows) {
            Err(done) => done,
            Ok(plan) => {
                let vectors = plan.embed();
                self.related_finish(plan, vectors, allows)
            }
        }
    }
}

/// The `related_skipped` hint for `count` nodes skipped for `why`, with a whole command, runnable as printed once its
/// placeholders are filled, that turns the check on or shows what is missing (reviewer N1). `floor` is the embedder
/// instance and the floor use for the no-floor fix, which names the config key to set: the instance's own name when it
/// has one, else the `<instance>` placeholder (`umx config show` lists the instances).
fn hint(
    alias: &str,
    verb: &'static str,
    why: Skip,
    count: usize,
    floor: Option<(&str, Use)>,
) -> Problem {
    problems::related_skipped(count, why.words(), fix(alias, verb, why, floor))
}

fn fix(alias: &str, verb: &'static str, why: Skip, floor: Option<(&str, Use)>) -> Command {
    let graph = ValidName::alias(alias);
    let on_graph = |c: Command| match &graph {
        Some(g) => c.word("--graph").name(g),
        None => c.word("--graph").word("<alias>"),
    };
    match why {
        Skip::OffFlag if verb == "batch" => {
            on_graph(Command::new("batch").word("--file").word("<file>")).word("--related")
        }
        Skip::OffFlag => on_graph(
            Command::new("node add")
                .word("--type")
                .word("<type>")
                .word("--title")
                .word("<title>"),
        )
        .word("--related"),
        Skip::OffGraph => match &graph {
            Some(g) => Command::new("graph set").name(g),
            None => Command::new("graph set").word("<alias>"),
        }
        .word("--related")
        .word("on"),
        Skip::OffMachine => Command::new("config set").word("related").word("on"),
        Skip::NoEmbedder | Skip::NotAdmitted | Skip::NoVectors | Skip::EmbedFailed => {
            Command::new("embeddings status")
        }
        Skip::NoFloor => {
            let (instance, use_) = floor.unwrap_or(("", Use::Search));
            let c = Command::new("config set");
            let c = match (instance.is_empty(), use_) {
                (false, Use::Search) => c.key(&alloc::format!("embedders.{instance}.search_floor")),
                (false, Use::Similar) => {
                    c.key(&alloc::format!("embedders.{instance}.similar_floor"))
                }
                (true, Use::Search) => c.word("embedders.<instance>.search_floor"),
                (true, Use::Similar) => c.word("embedders.<instance>.similar_floor"),
            };
            c.word("<cosine>")
        }
        Skip::Secret => Command::new("docs get").word("embeddings"),
        Skip::OverCap | Skip::TimedOut => on_graph(Command::new("search").word("<title>")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_wins_then_the_graph_then_the_machine() {
        assert_eq!(switch(Some(true), Some(false), false), Ok(()));
        assert_eq!(switch(Some(false), Some(true), true), Err(Skip::OffFlag));
        assert_eq!(switch(None, Some(false), true), Err(Skip::OffGraph));
        assert_eq!(switch(None, Some(true), false), Ok(()));
        assert_eq!(switch(None, None, false), Err(Skip::OffMachine));
        assert_eq!(switch(None, None, true), Ok(()));
    }

    #[test]
    fn a_prompt_key_uses_the_search_floor_and_a_key_without_one_the_similar_floor() {
        let gemma = ModelKey::parse("embeddinggemma-2@256+p1").unwrap();
        let floor = related_floor(&Floors::default(), &gemma);
        assert_eq!(floor, umx_core::similarity::floor(&gemma, Use::Search));
        assert!(floor.calibrated());
        let large = ModelKey::parse("text-embedding-3-large@256").unwrap();
        let floor = related_floor(&Floors::default(), &large);
        assert_eq!(floor, umx_core::similarity::floor(&large, Use::Similar));
        assert!(floor.calibrated());
    }

    /// Reviewer N1: every reason's fix is a whole command. The list is the golden file umx-cli's parser test reads
    /// (`tests/fixtures/related/fixes.txt`), so a fix that changes here must parse there too.
    #[test]
    fn every_reason_s_fix_is_a_whole_command_as_the_golden_file_lists_it() {
        let mut got = Vec::new();
        for why in Skip::ALL {
            for verb in ["node add", "batch"] {
                got.push(
                    fix("demo", verb, why, Some(("gemma", Use::Search)))
                        .as_str()
                        .to_string(),
                );
            }
        }
        got.push(
            fix(
                "demo",
                "node add",
                Skip::NoFloor,
                Some(("azure", Use::Similar)),
            )
            .as_str()
            .to_string(),
        );
        got.push(
            fix("demo", "node add", Skip::NoFloor, Some(("", Use::Search)))
                .as_str()
                .to_string(),
        );
        got.push(
            fix("demo", "node add", Skip::NoFloor, Some(("", Use::Similar)))
                .as_str()
                .to_string(),
        );
        got.sort();
        got.dedup();
        let golden: Vec<&str> = include_str!("../tests/fixtures/related/fixes.txt")
            .lines()
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(
            got,
            golden,
            "update tests/fixtures/related/fixes.txt to:\n{}",
            got.join("\n")
        );
    }
}
