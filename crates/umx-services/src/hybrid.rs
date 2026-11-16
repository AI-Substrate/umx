//! Hybrid search (req-30, req-37, req-53; workshop 008 section 11): exact matches, the BM25F word leg (s10) and the
//! vector leg over each graph's blocks (s13), fused by reciprocal rank. It runs in three steps so a hosted query embed
//! never holds the writer: [`Services::search_plan`] under the shared lock decides which graphs get a vector leg,
//! [`SearchPlan::embed`] embeds the query with no lock held, and [`Services::search_finish`] scores under the shared
//! lock again, checking each graph's key once more.
//!
//! Deterministic (architect): the constants are named, sums run in a fixed order (word leg then vector leg), and ties
//! go to the graph alias and then the lower handle, as `umx search` has ordered since s10.

use crate::graph_store::GRAPH_WIDE;
use crate::{Ctx, Done, Outcome, Services, problems};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use umx_core::ports::{EmbedError, Embedder, GraphDir, Locality, ModelKey, TextKind};
use umx_core::secrets;
use umx_core::similarity::{self, inverse_length, top_k};
use umx_core::vectors::quantise;
use umx_core::{Id, Node, Policy, Problem, ValidName, Value};
use umx_graph::{Graph, NodeIx, NodeSet, WordQuery, WordStats};

/// The reciprocal rank constant: a hit at rank r in a leg adds `1 / (RRF_K + r)` (embeddings-expert, 008 section 11).
pub const RRF_K: f64 = 60.0;
/// Each leg gives at most this many hits, the best in scope, to the fusion.
pub const LEG_DEPTH: usize = 100;
/// A query term is informative when at most this share of the nodes searched hold it (embeddings-expert, s11). A word
/// hit's fusion credit is scaled by the share of the query's informative terms it holds (its coverage): BM25 ranks a
/// hit that matches one middling word first all the same, and RRF credits rank, not score. Model-free.
pub const WORD_MAX_DF_SHARE: f64 = 0.25;
/// The field a hit from a whole-node vector reports.
pub const WHOLE_NODE: &str = "@node";

/// Where the word leg ranked a hit.
#[derive(Clone, Debug, PartialEq)]
pub struct WordMatch {
    pub rank: usize,
    pub score: f64,
    pub field: String,
    /// The share of the query's informative terms the hit holds, 0 to 1; its word credit is scaled by it. 1 when no
    /// vector leg took part (word-only results keep every hit, req-53).
    pub coverage: f64,
    /// Each query word the hit holds and how it matched (req-92).
    pub terms: Vec<MatchedTerm>,
}

/// A query word a word hit holds: the term it met in the node and how, `exact`, `stem` (another form of the word,
/// by its Porter2 stem) or `prefix` (a word starting with it), req-92.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchedTerm {
    pub query: String,
    pub term: String,
    pub form: &'static str,
}

/// Where the vector leg ranked a hit: its best cosine over its whole-node and field vectors, and the field that won.
#[derive(Clone, Debug, PartialEq)]
pub struct VectorMatch {
    pub rank: usize,
    pub cosine: f32,
    pub field: String,
}

/// One result of a hybrid search.
#[derive(Clone, Debug, PartialEq)]
pub struct HybridHit {
    pub alias: String,
    pub policy: Policy,
    pub node: Node,
    /// The field whose whole value equals the query, case ignored (`title`, `id` or an exact-match column): such hits
    /// lead the results.
    pub exact: Option<String>,
    pub word: Option<WordMatch>,
    pub vector: Option<VectorMatch>,
    /// The reciprocal rank sum over the legs that found it.
    pub fused: f64,
}

impl HybridHit {
    /// PRD 002 search-07: the field a hit's text comes from and its cut ([`umx_core::cut::cut`], at most
    /// [`umx_core::cut::CUT_CHARS`] characters): the word leg's field, cut around the words it matched; else the best
    /// vector's field (not the whole-node vector, which is no one field); else the note from its start. The first of
    /// those that holds text wins; None when none does. Read from the hit's own node: no other node is read.
    pub fn text(&self) -> Option<(String, String)> {
        let word = self.word.as_ref().map(|w| {
            let terms: Vec<&str> = w.terms.iter().map(|t| t.term.as_str()).collect();
            (w.field.as_str(), terms)
        });
        let vector = self
            .vector
            .as_ref()
            .map(|v| v.field.as_str())
            .filter(|f| !f.starts_with('@'));
        let tried = word
            .into_iter()
            .chain(vector.map(|f| (f, Vec::new())))
            .chain([("note", Vec::new())]);
        for (field, words) in tried {
            if let Some(Value::String(text)) = self.node.field(field)
                && !text.trim().is_empty()
            {
                return Some((field.to_string(), umx_core::cut::cut(&text, &words)));
            }
        }
        None
    }

    /// Which legs found it: `word`, `vector`, `both`, or `exact` when only the exact match did.
    pub fn legs(&self) -> &'static str {
        match (&self.word, &self.vector) {
            (Some(_), Some(_)) => "both",
            (Some(_), None) => "word",
            (None, Some(_)) => "vector",
            (None, None) => "exact",
        }
    }
}

/// What a hybrid search found, with the floor each vector space was cut at and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct HybridResult {
    pub hits: Vec<HybridHit>,
    /// How many matches fusion ranked before `--limit` cut the list (PRD 002 search-13): more than `hits` when it
    /// cut. Every exact match is counted, but the word and vector legs each give fusion at most their best
    /// [`LEG_DEPTH`], so this is not every node that holds the words.
    pub found: usize,
    pub floors: Vec<(ModelKey, similarity::Floor)>,
    /// Why the word leg gave fusion nothing, when it did not: `no informative terms` (every query word is in more
    /// than [`WORD_MAX_DF_SHARE`] of the nodes searched).
    pub word_leg: Option<&'static str>,
    /// `--graph` named copies of one graph (d-21): each hit's `graph` is its copy, shown as `copy` too.
    pub copies: bool,
    /// The spine range the legs were narrowed to (`--along`, D71 step 12), with what it left out of the graph.
    pub along: Option<crate::AlongInfo>,
}

/// Why a vector space got no query vector.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryFailed {
    /// The provider failed.
    Embed(EmbedError),
    /// The embed took longer than its bound (req-53, 5 s in the daemon). `warming`: the instance has not answered
    /// a query yet, so a local model may still be loading.
    TimedOut { warming: bool },
    /// The query looks like it holds a secret and the instance is not local, so it was not asked (req-49).
    Secret,
    /// A graph whose leg uses the instance no longer admits it (its embedder setting changed since the
    /// plan, or its folder's config), so it was not asked (D53).
    NotAdmitted,
}

/// The query vectors of one search, one per embedder instance (by name) a vector leg uses. Two instances of one
/// model are kept apart, so each graph's leg reads only the vector of the instance its own setting admits (D53).
pub type QueryVectors = BTreeMap<String, Result<Vec<f32>, QueryFailed>>;

/// Whether graph `alias`'s leg may still use instance `name`, asked right before the instance is sent the query.
pub type Admitted<'a> = &'a dyn Fn(&str, &str) -> bool;

/// How one query embed is run: the daemon bounds it in time (req-53); [`SearchPlan::embed`] calls it directly.
pub type QueryRunner<'a> = &'a dyn Fn(&Arc<dyn Embedder>, &str) -> Result<Vec<f32>, QueryFailed>;

/// A search decided under the shared lock: the graphs, which of them get a vector leg (and in which space), and the
/// warnings so far. Holds no borrow of the services.
pub struct SearchPlan {
    text: String,
    include_provenance: bool,
    within: Option<String>,
    limit: usize,
    aliases: Vec<String>,
    /// Whether the request named its graphs; an unnamed plan's graphs are checked again for an open policy at finish.
    named: bool,
    /// None is the ordinary selector; Some records a per-call all-graph request and its named-graph opt-in.
    all: Option<bool>,
    /// Secured graphs an unnamed search left out so far.
    skipped: usize,
    /// Whether `--graph` named copies of one graph (d-21): hits then say their copy.
    copies: bool,
    /// Other copies an unnamed search left out, named at finish if still open (d-21).
    copies_skipped: Vec<String>,
    saved: Option<crate::graph_store::SavedRead>,
    legs: BTreeMap<String, ModelKey>,
    /// The instance each leg uses, by graph alias (s17: each graph's own instance; D53: never another of its model).
    routes: BTreeMap<String, String>,
    /// Each instance a leg uses, by name.
    embedders: BTreeMap<String, Arc<dyn Embedder>>,
    /// Warnings so far, each with the graph it names (None for the whole search): one about a graph an unnamed
    /// search drops at finish is never shown, since that graph is only counted (d-20).
    warnings: Vec<(Option<String>, Problem)>,
    /// `--along` (D71 step 12): the spine range the legs are narrowed to, before fusion. One graph only.
    along: Option<crate::AlongRequest>,
}

impl SearchPlan {
    /// The query embedded once per instance a vector leg uses, holding no lock, with no re-check before each send
    /// ([`Self::embed_with`] has one; finish re-checks every leg either way). No graph with a leg (no embedder, every
    /// graph secured with a hosted embedder, a mismatch everywhere) calls nothing. Each instance is asked only for the
    /// graphs that chose it (s17, D53).
    pub fn embed(&self) -> QueryVectors {
        self.embed_with(&|_, _| true, &ask)
    }

    /// [`Self::embed`] through `run`, which may bound each call. Right before an instance is asked, `admitted` must
    /// hold for every graph whose leg uses it, else it is not asked ([`QueryFailed::NotAdmitted`]). A query that looks
    /// like it holds a secret is never sent to an instance that is not local (req-49, security s08): that instance
    /// fails with [`QueryFailed::Secret`].
    pub fn embed_with(&self, admitted: Admitted<'_>, run: QueryRunner<'_>) -> QueryVectors {
        let mut out = QueryVectors::new();
        let secret = secrets::scan(&self.text).is_some();
        for (name, embedder) in &self.embedders {
            let mut legs = self.routes.iter().filter(|(_, n)| *n == name).peekable();
            if legs.peek().is_none() {
                continue;
            }
            let vector = if !legs.all(|(alias, _)| admitted(alias, name)) {
                Err(QueryFailed::NotAdmitted)
            } else if secret && embedder.locality() != Locality::Local {
                Err(QueryFailed::Secret)
            } else {
                run(embedder, &self.text)
            };
            out.insert(name.clone(), vector);
        }
        out
    }

    /// Narrows the search to a spine range ([`crate::AlongRequest`]). One graph only: refused when the request named
    /// a list, a scope, `--all` or `--provenance`, or when no graph was named and several are searched.
    pub fn along(
        mut self,
        ctx: &Ctx,
        along: Option<crate::AlongRequest>,
    ) -> Result<SearchPlan, Problem> {
        if along.is_some() {
            crate::graph_store::along_one_graph(ctx)?;
            if self.aliases.len() > 1 {
                return Err(problems::usage_invalid(
                    "--along reads one graph; name it with --graph",
                    "time",
                ));
            }
        }
        self.along = along;
        Ok(self)
    }
}

/// One query embed, unbounded.
fn ask(embedder: &Arc<dyn Embedder>, text: &str) -> Result<Vec<f32>, QueryFailed> {
    embedder
        .embed(&[text.to_string()], TextKind::Query)
        .map(|mut e| e.vectors.pop().unwrap_or_default())
        .map_err(QueryFailed::Embed)
}

/// The best rows of one graph's vector blocks in `space` for `query` (int8): each node's best cosine over its
/// whole-node and field vectors, at or above `floor`, best first then by handle, at most [`LEG_DEPTH`]. Exact for the
/// top [`LEG_DEPTH`]: a node outside every block's top [`LEG_DEPTH`] has that many nodes beating it.
pub fn vector_candidates(
    graph: &Graph,
    space: &str,
    query: &[i8],
    scope: Option<&NodeSet<'_>>,
    floor: f32,
) -> Vec<(Id, f32, String)> {
    // Each block's best rows: (field, [(handle, cosine)]).
    let mut blocks: Vec<(String, Vec<(NodeIx, f32)>)> = Vec::new();
    if let Some(w) = graph.whole_block(space)
        && w.dims() == query.len()
    {
        let rows = top_k(query, w.values(), w.inverse_lengths(), LEG_DEPTH, |i| {
            w.has(i)
                && !graph.is_provenance(w.node(i as u32))
                && scope.is_none_or(|s| s.contains_ix(w.node(i as u32)))
        });
        let rows = rows.into_iter().map(|h| (w.node(h.row), h.score)).collect();
        blocks.push((WHOLE_NODE.to_string(), rows));
    }
    for b in graph.field_blocks(space) {
        if b.dims() != query.len() || umx_core::ontology::PROVENANCE_KEYS.contains(&b.field()) {
            continue;
        }
        let rows = top_k(query, b.values(), b.inverse_lengths(), LEG_DEPTH, |i| {
            !graph.is_provenance(b.node(i as u32))
                && scope.is_none_or(|s| s.contains_ix(b.node(i as u32)))
        });
        let rows = rows.into_iter().map(|h| (b.node(h.row), h.score)).collect();
        blocks.push((b.field().to_string(), rows));
    }
    // Each node's best row; a tie goes to the field name first in order, so the result never depends on block order.
    let mut best: BTreeMap<NodeIx, (f32, &str)> = BTreeMap::new();
    for (field, rows) in &blocks {
        for &(node, score) in rows {
            if score < floor {
                continue;
            }
            let better = best
                .get(&node)
                .is_none_or(|&(s, f)| score > s || (score == s && field.as_str() < f));
            if better {
                best.insert(node, (score, field.as_str()));
            }
        }
    }
    let mut out: Vec<(NodeIx, f32, &str)> = best.into_iter().map(|(n, (s, f))| (n, s, f)).collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    out.truncate(LEG_DEPTH);
    out.into_iter()
        .map(|(n, s, f)| (graph.node_at(n).id(), s, f.to_string()))
        .collect()
}

/// One leg's ranked hits across graphs: (alias, node, score, field), best first. Within a graph id order is handle
/// order.
pub type Ranked = Vec<(String, Id, f64, String)>;

/// The word leg's ranked hits: (alias, node, BM25, field, coverage, matched words), best first.
pub type WordRanked = Vec<(String, Id, f64, String, f64, Vec<MatchedTerm>)>;

/// Ranks every graph's candidates together: score, then alias, then id (handle); at most [`LEG_DEPTH`].
fn rank(mut all: Ranked) -> Ranked {
    all.sort_by(|a, b| {
        b.2.total_cmp(&a.2)
            .then_with(|| a.0.cmp(&b.0))
            .then(a.1.cmp(&b.1))
    });
    all.truncate(LEG_DEPTH);
    all
}

/// [`rank`] for the word leg.
fn rank_words(mut all: WordRanked) -> WordRanked {
    all.sort_by(|a, b| {
        b.2.total_cmp(&a.2)
            .then_with(|| a.0.cmp(&b.0))
            .then(a.1.cmp(&b.1))
    });
    all.truncate(LEG_DEPTH);
    all
}

/// A fused entry before its node is read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fused {
    pub exact: Option<String>,
    pub word: Option<WordMatch>,
    pub vector: Option<VectorMatch>,
    pub fused: f64,
}

/// Reciprocal rank fusion (008 section 11, rules 1, 2 and 4; embeddings-expert's coverage, s11), pure: `exact` hits
/// lead in (alias, handle) order; the rest by `coverage / (RRF_K + word rank) + 1 / (RRF_K + vector rank)` (word
/// then vector), then the better single-leg rank, then alias, then handle. Every key in any input is in the output
/// once.
pub fn fuse(
    exact: &[(String, Id, String)],
    word: &WordRanked,
    vector: &Ranked,
) -> Vec<((String, Id), Fused)> {
    let mut by: BTreeMap<(String, Id), Fused> = BTreeMap::new();
    for (alias, h, field) in exact {
        by.entry((alias.clone(), *h)).or_default().exact = Some(field.clone());
    }
    for (i, (alias, h, score, field, coverage, terms)) in word.iter().enumerate() {
        by.entry((alias.clone(), *h)).or_default().word = Some(WordMatch {
            rank: i + 1,
            score: *score,
            field: field.clone(),
            coverage: *coverage,
            terms: terms.clone(),
        });
    }
    for (i, (alias, h, score, field)) in vector.iter().enumerate() {
        by.entry((alias.clone(), *h)).or_default().vector = Some(VectorMatch {
            rank: i + 1,
            cosine: *score as f32,
            field: field.clone(),
        });
    }
    let mut out: Vec<((String, Id), Fused)> = by
        .into_iter()
        .map(|(k, mut f)| {
            let leg = |r: usize| 1.0 / (RRF_K + r as f64);
            f.fused = f.word.as_ref().map_or(0.0, |w| w.coverage * leg(w.rank))
                + f.vector.as_ref().map_or(0.0, |v| leg(v.rank));
            (k, f)
        })
        .collect();
    let best = |f: &Fused| {
        let w = f.word.as_ref().map_or(usize::MAX, |w| w.rank);
        let v = f.vector.as_ref().map_or(usize::MAX, |v| v.rank);
        w.min(v)
    };
    out.sort_by(|(ka, a), (kb, b)| {
        b.exact
            .is_some()
            .cmp(&a.exact.is_some())
            .then_with(|| {
                if a.exact.is_some() && b.exact.is_some() {
                    core::cmp::Ordering::Equal
                } else {
                    b.fused.total_cmp(&a.fused).then(best(a).cmp(&best(b)))
                }
            })
            .then_with(|| ka.cmp(kb))
    });
    out
}

/// The text a custom field's value matches exactly: a string, number or bool as written. Lists and objects never.
fn rendered(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

impl Services {
    /// Step one, under the shared lock: the graphs a search covers (SEC-5, d-20: an unnamed search skips secured
    /// graphs) and which get a vector leg. A graph gets none, always with a warning, when there is no embedder, when
    /// its embedder setting admits none of the instances (`similarity_no_embedder`: its query text is never sent;
    /// architect, security gate), when no default serves a graph that records no model, when its vectors are
    /// in another model's space than the embedder makes (`embedder_mismatch`, naming both keys; req-37), or when it
    /// holds no vectors in that space. Its word and exact legs always run.
    pub fn search_plan(
        &self,
        ctx: &Ctx,
        text: &str,
        within: Option<&str>,
        limit: usize,
    ) -> Result<SearchPlan, Problem> {
        self.search_plan_allowing(ctx, text, within, limit, &|_, _| true)
    }

    /// [`Self::search_plan`] where `allows(folder, instance)` says whether a graph's folder may use an instance: a repo
    /// layer that is unreadable or narrows to another instance or to none (req-72, the daemon's one folder rule)
    /// leaves that graph without a vector leg, and its query text is never sent (security, s11, s17). Each graph's
    /// leg uses the instance it embeds with: its recorded model, allowed by its policy (s17).
    pub fn search_plan_allowing(
        &self,
        ctx: &Ctx,
        text: &str,
        within: Option<&str>,
        limit: usize,
        allows: &dyn Fn(&GraphDir, &str) -> bool,
    ) -> Result<SearchPlan, Problem> {
        let store = &self.graph;
        let all = match &ctx.scope {
            Some(crate::Scope::All { include_named }) => Some(*include_named),
            _ => None,
        };
        let searched = store.search_graphs(ctx, within)?;
        if let Some(w) = within
            && !searched.aliases.is_empty()
        {
            store.search_root(ctx, &searched.aliases, w)?;
        }
        let mut warnings = Vec::new();
        // d-21: the other copies an unnamed search left out are named; named copies are unioned, with a warning.
        if !searched.twice.is_empty() {
            warnings.push((None, problems::graph_twice(&searched.twice)));
        }
        let mut legs = BTreeMap::new();
        let mut routes = BTreeMap::new();
        let mut embedders = BTreeMap::new();
        if self.embedding.instances().is_empty() {
            if !searched.aliases.is_empty() {
                warnings.push((
                    None,
                    problems::similarity_unavailable(None, "no embedder is configured"),
                ));
            }
        } else {
            for alias in &searched.aliases {
                let graph = ValidName::alias(alias);
                let policy = store.effective_policy(alias).unwrap_or(Policy::SECURED);
                // No instance here is one the graph's setting admits (a `local` graph with only hosted instances):
                // one warning says why and gives the owner's fix (s32), whether or not it records a model. Never
                // silent; a graph with no showable name is not named. The folder rule is not consulted here: with
                // no admitted instance it cannot change the outcome, and a folder that narrows an admitted one
                // warns below.
                if !self.embedding.admits_any(policy.embedder) {
                    warnings.push((
                        Some(alias.clone()),
                        problems::similarity_no_embedder(graph.as_ref(), policy.embedder),
                    ));
                    continue;
                }
                let loaded = store.loaded(alias).expect("searched");
                // The folder rule filters first, then the tie-break (R106-3).
                let usable = |n: &str| allows(&loaded.dir, n);
                let routed = self
                    .embedding
                    .instance_for_where(store, alias, policy, &usable);
                let Some(block) = self.embedding.block_key(store, alias) else {
                    // An `any` graph that records no model while embedder.active is none: nothing embeds it.
                    warnings.push((
                        Some(alias.clone()),
                        problems::similarity_unavailable(
                            graph.as_ref(),
                            "it records no model and no default embedder is set",
                        ),
                    ));
                    continue;
                };
                let Some(instance) = routed else {
                    if all == Some(true) && policy.embedder == umx_core::EmbedderRule::Local {
                        warnings.push((
                            Some(alias.clone()),
                            problems::similarity_unavailable(
                                graph.as_ref(),
                                "no local embedder is available for this graph",
                            ),
                        ));
                        continue;
                    }
                    if self.embedding.instance_for(store, alias, policy).is_some() {
                        warnings.push((
                            Some(alias.clone()),
                            problems::similarity_unavailable(
                                graph.as_ref(),
                                "its folder's config names another embedder",
                            ),
                        ));
                        continue;
                    }
                    // An instance makes the space but the graph's embedder setting admits none of them, or none makes it.
                    let makes = self
                        .embedding
                        .instances()
                        .iter()
                        .any(|i| i.embedder.key() == block);
                    if let Some(g) = &graph {
                        let problem = if makes {
                            self.embedding.not_admitted(g)
                        } else {
                            let default = self.embedding.key().unwrap_or_else(|| block.clone());
                            problems::embedder_mismatch(g, &block, &default)
                        };
                        warnings.push((Some(alias.clone()), problem));
                    }
                    continue;
                };
                if block != instance.embedder.key() {
                    if let Some(g) = &graph {
                        warnings.push((
                            Some(alias.clone()),
                            problems::embedder_mismatch(g, &block, &instance.embedder.key()),
                        ));
                    }
                    continue;
                }
                if !has_rows(&loaded.graph, &block) {
                    warnings.push((
                        Some(alias.clone()),
                        problems::similarity_unavailable(graph.as_ref(), "it has no vectors yet"),
                    ));
                    continue;
                }
                embedders.insert(instance.name.clone(), instance.embedder.clone());
                routes.insert(alias.clone(), instance.name.clone());
                legs.insert(alias.clone(), block);
            }
        }
        Ok(SearchPlan {
            text: text.to_string(),
            include_provenance: ctx.include_provenance,
            within: within.map(str::to_string),
            limit,
            aliases: searched.aliases,
            named: searched.named,
            all,
            skipped: searched.skipped,
            copies: !searched.twice.is_empty(),
            copies_skipped: searched.copies_skipped,
            saved: searched.saved,
            legs,
            routes,
            embedders,
            warnings,
            along: None,
        })
    }

    /// Step three, under the shared lock: the legs scored and fused. An unloaded graph leaves an all-mode plan;
    /// ordinary selectors return `graph_not_loaded`. A changed vector space loses its vector leg with a warning,
    /// so a query from one space is never scored against rows of another (architect).
    pub fn search_finish(&self, plan: SearchPlan, queries: QueryVectors) -> Outcome<HybridResult> {
        self.search_finish_with(plan, queries, WORD_MAX_DF_SHARE)
    }

    /// [`Self::search_finish`] with another informative-term share, for reports that compare shares.
    #[doc(hidden)]
    pub fn search_finish_with(
        &self,
        plan: SearchPlan,
        queries: QueryVectors,
        word_max_df_share: f64,
    ) -> Outcome<HybridResult> {
        let store = &self.graph;
        let SearchPlan {
            text,
            include_provenance,
            within,
            limit,
            mut aliases,
            named,
            all,
            mut skipped,
            copies,
            copies_skipped,
            saved,
            mut legs,
            routes,
            warnings: planned,
            along,
            ..
        } = plan;
        if all.is_some() {
            aliases.retain(|a| store.loaded(a).is_some());
            legs.retain(|a, _| aliases.contains(a));
        } else {
            for a in &aliases {
                if store.loaded(a).is_none() {
                    return Err(store.not_loaded(a));
                }
            }
        }
        // An unnamed search covers open graphs only (d-20): a graph secured (or unreadable) while the query was being
        // embedded leaves every leg and the word figures, and is counted, never named.
        if !named {
            let before = aliases.len();
            aliases.retain(|a| store.shown_unnamed(a));
            legs.retain(|a, _| aliases.contains(a));
            skipped += before - aliases.len();
        }
        // Copies are named only while still open at finish, as query does; one secured meanwhile is counted with the
        // secured graphs, never named (security F1, reviewer R132-2).
        let copies_skipped: Vec<String> = copies_skipped
            .into_iter()
            .filter(|c| all.is_none() || store.loaded(c).is_some())
            .collect();
        let left = copies_skipped.len();
        let others: Vec<String> = copies_skipped
            .into_iter()
            .filter(|c| store.shown_unnamed(c))
            .collect();
        skipped += left - others.len();
        let mut warnings: Vec<Problem> = Vec::new();
        if skipped > 0 {
            warnings.push(problems::secured_graphs_skipped(skipped));
        }
        if !others.is_empty() {
            warnings.push(problems::copies_skipped(&others));
        }
        if all == Some(true) {
            warnings.extend(store.named_graphs_read(&aliases));
        }
        if let Some(saved) = &saved {
            warnings.extend(store.saved_warnings(saved, &aliases)?);
        }
        warnings.extend(store.stale_notes(&aliases));
        warnings.extend(
            planned
                .into_iter()
                .filter(|(alias, _)| alias.as_ref().is_none_or(|a| aliases.contains(a)))
                .map(|(_, w)| w),
        );
        let ctx = Ctx::new("reader").with_provenance(include_provenance);
        let root = match &within {
            Some(w) if !aliases.is_empty() => Some(store.search_root(&ctx, &aliases, w)?),
            _ => None,
        };
        // --along (D71 step 12, graph-storage): the range narrows every leg before fusion, so out-of-range hits never
        // crowd in-range ones out of a leg's LEG_DEPTH. Built at finish, under the lock the legs read under.
        let mut kept = match (&along, aliases.as_slice()) {
            (Some(a), [only]) => Some((only.clone(), store.along(&ctx, only, a)?)),
            _ => None,
        };
        // Keys again, and the query vectors.
        let mut failed: BTreeSet<&'static str> = BTreeSet::new();
        // By instance: a leg reads only the vector of the instance it was planned with (D53).
        let mut query: BTreeMap<String, Vec<i8>> = BTreeMap::new();
        legs.retain(|alias, key| {
            if self.embedding.block_key(store, alias).as_ref() != Some(key) {
                warnings.push(problems::similarity_unavailable(
                    ValidName::alias(alias).as_ref(),
                    "its model changed during the search",
                ));
                return false;
            }
            let Some(name) = routes.get(alias) else {
                return false;
            };
            if !self.embedding.admits_now(store, alias, name) {
                warnings.push(problems::similarity_unavailable(
                    ValidName::alias(alias).as_ref(),
                    "its embedder setting changed during the search",
                ));
                return false;
            }
            match queries.get(name) {
                Some(Ok(v)) => {
                    let q = quantise(v);
                    if inverse_length(&q) == 0.0 || q.len() != key.dims() {
                        failed.insert("the query vector has no direction");
                        return false;
                    }
                    query.insert(name.clone(), q);
                    true
                }
                Some(Err(why)) => {
                    failed.insert(match why {
                        QueryFailed::Embed(_) => "the embedder failed",
                        QueryFailed::TimedOut { warming: false } => "the query embed took over 5 s",
                        QueryFailed::TimedOut { warming: true } => {
                            "the query embed took over 5 s (the local model may still be loading so search again)"
                        }
                        QueryFailed::Secret => {
                            "the query looks like it holds a secret so no hosted embedder was asked"
                        }
                        QueryFailed::NotAdmitted => {
                            "a graph's embedder setting changed during the search"
                        }
                    });
                    false
                }
                None => {
                    failed.insert("the query was not embedded");
                    false
                }
            }
        });
        for cause in failed {
            warnings.push(problems::similarity_unavailable(None, cause));
        }
        // Word leg: figures summed over every graph searched (s10); each hit's credit scaled by its coverage.
        let mut word = WordRanked::new();
        let mut word_leg = None;
        if let Some(q) = WordQuery::new(&text) {
            let mut stats = WordStats::default();
            for a in &aliases {
                stats.add(&store.loaded(a).expect("checked").graph.word_stats(&q));
            }
            for (w, n) in stats.capped(&q) {
                warnings.push(problems::word_prefix_capped(w, n, umx_graph::PREFIX_CAP));
            }
            let most = word_max_df_share * stats.nodes as f64;
            // A word's df here is its exact and stem matches' summed (s21): a common class is not informative.
            let informative: Vec<bool> = (0..q.terms().len())
                .map(|i| stats.informative_df(i) as f64 <= most)
                .collect();
            // Coverage weighs a word hit only in a graph that also has a vector leg: a graph searched by words alone
            // keeps every word hit at full credit (req-53), whatever the other graphs have.
            let wanted = informative.iter().filter(|&&i| i).count();
            for a in &aliases {
                let gated =
                    legs.contains_key(a) && routes.get(a).is_some_and(|n| query.contains_key(n));
                if gated && wanted == 0 {
                    word_leg = Some("no informative terms");
                    continue;
                }
                let g = &store.loaded(a).expect("checked").graph;
                let Some(scope) = scope_of(&root, a, g, include_provenance) else {
                    continue;
                };
                let scope = narrow(scope, kept.as_ref(), a, g);
                let hits =
                    g.word_search_marked(&q, &stats, scope.as_ref(), LEG_DEPTH, &informative);
                for hit in hits {
                    let coverage = if gated { hit.held / wanted as f64 } else { 1.0 };
                    let terms = hit
                        .terms
                        .into_iter()
                        .map(|t| MatchedTerm {
                            query: t.query,
                            term: t.term,
                            form: t.form.as_str(),
                        })
                        .collect();
                    word.push((
                        a.clone(),
                        hit.node.id(),
                        hit.score,
                        hit.field.to_string(),
                        coverage,
                        terms,
                    ));
                }
            }
        }
        let word = rank_words(word);
        // Vector leg: graphs compared only within one space (req-37).
        let mut vector = Ranked::new();
        let mut floors = Vec::new();
        let (mut pending, mut held) = (0usize, 0usize);
        let spaces: BTreeSet<&ModelKey> = legs.values().collect();
        for key in spaces {
            let cut = store.floors().get(key, similarity::Use::Search);
            let floor = cut.value;
            floors.push((key.clone(), cut));
            let space = key.to_string();
            let mut in_space = Ranked::new();
            for (alias, _) in legs.iter().filter(|(_, k)| *k == key) {
                let q = &query[&routes[alias]];
                let g = &store.loaded(alias).expect("checked").graph;
                let Some(scope) = scope_of(&root, alias, g, include_provenance) else {
                    continue;
                };
                let scope = narrow(scope, kept.as_ref(), alias, g);
                let l = store.loaded(alias).expect("checked");
                let (p, h) = pending_in(
                    g,
                    &space,
                    scope.as_ref(),
                    l.vectors_pending.as_ref(),
                    &l.vectors_held,
                );
                pending += p;
                held += h;
                for (h, cosine, field) in vector_candidates(g, &space, q, scope.as_ref(), floor) {
                    in_space.push((alias.clone(), h, f64::from(cosine), field));
                }
            }
            vector.extend(rank(in_space));
        }
        if pending > 0 {
            warnings.push(problems::similarity_incomplete(pending, held));
        }
        // Exact leg. Lower-casing can change a text's byte length only outside ASCII, so an ASCII text of another
        // length is skipped without allocating; an id is compared only when the query is as long as one.
        let needle = text.trim().to_lowercase();
        let could = |t: &str| t.len() == needle.len() || !t.is_ascii() || !needle.is_ascii();
        let same = |t: &str| could(t) && t.to_lowercase() == needle;
        let id_like = needle.len() == umx_core::id::ID_LEN;
        let time_like = needle.len() >= 10 && needle.as_bytes()[4] == b'-';
        let mut exact = Vec::new();
        for a in &aliases {
            let l = store.loaded(a).expect("checked");
            let Some(scope) = scope_of(&root, a, &l.graph, include_provenance) else {
                continue;
            };
            let scope = narrow(scope, kept.as_ref(), a, &l.graph);
            let text_fields = l.meta.ontology.text_fields();
            for n in ctx.read_scope(&l.graph).nodes() {
                if scope.as_ref().is_some_and(|s| !s.contains_ix(n.ix())) {
                    continue;
                }
                // Title, id, then the built-in short columns (type, status, created, updated, created_by), then
                // custom fields in name order: the first that equals the query is the one reported.
                // Times are rendered only for a query that could be one (they allocate).
                let builtin = || {
                    [("type", n.kind()), ("status", n.status())]
                        .into_iter()
                        .find(|(_, v)| same(v))
                        .map(|(f, _)| f)
                        .or_else(|| (time_like && same(&n.created())).then_some("created"))
                        .or_else(|| (time_like && same(&n.updated())).then_some("updated"))
                        .or_else(|| same(n.created_by()).then_some("created_by"))
                };
                let field = if same(n.title()) {
                    Some("title".to_string())
                } else if id_like && n.id().to_text().to_lowercase() == needle {
                    Some("id".to_string())
                } else if let Some(f) = builtin() {
                    Some(f.to_string())
                } else {
                    n.custom_field_names()
                        .filter(|f| !text_fields.is_text(n.kind(), f))
                        .find(|f| {
                            n.custom_field(f)
                                .and_then(|v| rendered(&v))
                                .is_some_and(|r| same(&r))
                        })
                        .map(str::to_string)
                };
                if let Some(field) = field {
                    exact.push((a.clone(), n.id(), field));
                }
            }
        }
        let fused = fuse(&exact, &word, &vector);
        let found = fused.len();
        let mut hits = Vec::new();
        for ((alias, id), f) in fused.into_iter().take(limit) {
            let l = store.loaded(&alias).expect("checked");
            let node = l.graph.node(id).expect("a hit is a node").to_node();
            hits.push(HybridHit {
                policy: store.effective_policy(&alias).unwrap_or(Policy::SECURED),
                alias,
                node,
                exact: f.exact,
                word: f.word,
                vector: f.vector,
                fused: f.fused,
            });
        }
        let along = kept.take().map(|(alias, mut k)| {
            k.count_graph(&store.loaded(&alias).expect("checked").graph);
            k.finish(GRAPH_WIDE, GRAPH_WIDE, &mut warnings)
        });
        Ok(Done {
            value: HybridResult {
                hits,
                found,
                floors,
                word_leg,
                copies,
                along,
            },
            notes: warnings,
        })
    }

    /// All three steps in one call, holding `&self` throughout (tests, the bench, callers without a writer).
    pub fn search(
        &self,
        ctx: &Ctx,
        text: &str,
        within: Option<&str>,
        limit: usize,
    ) -> Outcome<HybridResult> {
        self.search_along(ctx, text, within, limit, None)
    }

    /// [`Self::search`] narrowed to a spine range (`--along`, D71 step 12).
    pub fn search_along(
        &self,
        ctx: &Ctx,
        text: &str,
        within: Option<&str>,
        limit: usize,
        along: Option<crate::AlongRequest>,
    ) -> Outcome<HybridResult> {
        if along.is_some() {
            crate::graph_store::along_one_graph(ctx)?;
        }
        let plan = self
            .search_plan(ctx, text, within, limit)?
            .along(ctx, along)?;
        let queries = plan.embed_with(
            &|alias, name| self.embedding.admits_now(&self.graph, alias, name),
            &ask,
        );
        self.search_finish(plan, queries)
    }
}

/// The scope of `alias` in a search rooted at `root`: None when the root is in another graph (that graph only adds
/// to the word figures), else the subtree, or everything when there is no root.
fn scope_of<'g>(
    root: &Option<(String, Id)>,
    alias: &str,
    g: &'g Graph,
    include_provenance: bool,
) -> Option<Option<NodeSet<'g>>> {
    match root {
        Some((ra, _)) if ra != alias => None,
        Some((_, r)) => Some(Some(
            crate::graph_store::ReadScope::new(g)
                .with_provenance(include_provenance)
                .within(*r),
        )),
        None => Some(None),
    }
}

/// A leg's scope narrowed to the `--along` range of `alias`, when the search has one there (D71 step 12).
fn narrow<'g>(
    scope: Option<NodeSet<'g>>,
    kept: Option<&(String, crate::graph_store::Kept)>,
    alias: &str,
    g: &'g Graph,
) -> Option<NodeSet<'g>> {
    match kept {
        Some((a, k)) if a == alias => Some(match scope {
            Some(s) => g.node_set_where(|ix| s.contains_ix(ix) && k.keeps(ix)),
            None => k.set(g),
        }),
        _ => scope,
    }
}

/// Whether `graph` holds any vector in `key`'s space.
fn has_rows(graph: &Graph, key: &ModelKey) -> bool {
    let space = key.to_string();
    graph.whole_block(&space).is_some_and(|w| {
        graph
            .nodes()
            .any(|n| !graph.is_provenance(n.ix()) && w.has(n.ix().index()))
    }) || graph.field_blocks(&space).any(|b| {
        !umx_core::ontology::PROVENANCE_KEYS.contains(&b.field())
            && (0..b.rows()).any(|i| !graph.is_provenance(b.node(i as u32)))
    })
}

/// Nodes in scope without a current vector: the services' pending set when known (a stored stale vector still
/// searches, but its node counts until the fresh one lands; reviewer R99-3), else those with no whole-node row.
/// Nodes searched with no whole-node vector yet, and how many of them the secret hold keeps (req-49).
fn pending_in(
    graph: &Graph,
    space: &str,
    scope: Option<&NodeSet<'_>>,
    pending: Option<&BTreeSet<Id>>,
    held: &BTreeSet<Id>,
) -> (usize, usize) {
    let whole = graph.whole_block(space);
    graph
        .nodes()
        .filter(|n| !graph.is_provenance(n.ix()))
        .filter(|n| scope.is_none_or(|s| s.contains_ix(n.ix())))
        .filter(|n| match pending {
            Some(set) => set.contains(&n.id()),
            None => whole.is_none_or(|w| !w.has(n.ix().index())),
        })
        .fold((0, 0), |(p, h), n| {
            (p + 1, h + usize::from(held.contains(&n.id())))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn hit(alias: &str, n: u128, score: f64) -> (String, Id, f64, String) {
        (alias.into(), Id(n), score, "title".into())
    }

    fn word(
        alias: &str,
        n: u128,
        score: f64,
        coverage: f64,
    ) -> (String, Id, f64, String, f64, Vec<MatchedTerm>) {
        (
            alias.into(),
            Id(n),
            score,
            "title".into(),
            coverage,
            Vec::new(),
        )
    }

    /// One fused ranking pinned bit for bit (architect): exact first, then 1/(60 + rank) summed word then vector,
    /// then the better single-leg rank, then alias and id.
    #[test]
    fn a_fused_ranking_is_pinned_bit_for_bit() {
        let word: WordRanked = vec![
            word("a", 1, 9.0, 1.0),
            word("a", 2, 8.0, 1.0),
            word("b", 1, 7.0, 1.0),
        ];
        let vector: Ranked = vec![hit("a", 2, 0.9), hit("b", 3, 0.8)];
        let exact = vec![("b".to_string(), Id(9), "title".to_string())];
        let out = fuse(&exact, &word, &vector);
        let got: Vec<(&str, u128, u64)> = out
            .iter()
            .map(|((a, id), f)| (a.as_str(), id.0, f.fused.to_bits()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("b", 9, 0),
                // 1/62 (word rank 2) + 1/61 (vector rank 1)
                ("a", 2, 0x3fa0_a6c9_2bff_7560),
                ("a", 1, 0x3f90_c971_4fbc_da3b),
                ("b", 3, 0x3f90_8421_0842_1084),
                ("b", 1, 0x3f90_4104_1041_0410),
            ]
        );
        assert_eq!(out[1].1.word.as_ref().unwrap().rank, 2);
        assert_eq!(out[1].1.vector.as_ref().unwrap().rank, 1);
    }

    #[test]
    fn equal_fused_scores_go_to_the_better_leg_rank_then_alias_then_id() {
        // b/1 is word rank 1 and a/5 vector rank 1: equal sums and equal best ranks, so alias decides.
        let word: WordRanked = vec![word("b", 1, 3.0, 1.0), word("a", 7, 2.0, 1.0)];
        let vector: Ranked = vec![hit("a", 5, 0.9), hit("c", 2, 0.8)];
        let order: Vec<(String, u128)> = fuse(&[], &word, &vector)
            .into_iter()
            .map(|((a, id), _)| (a, id.0))
            .collect();
        assert_eq!(
            order,
            vec![
                ("a".into(), 5),
                ("b".into(), 1),
                ("a".into(), 7),
                ("c".into(), 2)
            ]
        );
    }

    /// The embeddings-expert's worked example: a word rank 1 hit holding 1 of 6 informative terms, also vector rank
    /// 30, scores 1/6 x 1/61 + 1/90 and falls below a vector-only rank 1 (1/61).
    #[test]
    fn coverage_scales_word_credit_so_one_term_in_six_cannot_beat_vector_rank_one() {
        let words: WordRanked = vec![word("a", 1, 9.0, 1.0 / 6.0)];
        let mut vector: Ranked = vec![hit("a", 2, 0.9)];
        for n in 3..32 {
            vector.push(hit("a", n, 0.8));
        }
        vector[29] = hit("a", 1, 0.7);
        let out = fuse(&[], &words, &vector);
        assert_eq!(out[0].0.1, Id(2));
        let noise = out.iter().find(|(k, _)| k.1 == Id(1)).unwrap();
        assert!((noise.1.fused - (1.0 / 6.0 / 61.0 + 1.0 / 90.0)).abs() < 1e-15);
        assert!(noise.1.fused < 1.0 / 61.0);
    }
}
