//! Hybrid search (req-30, req-37, req-53; workshop 008 section 11): exact matches, the BM25F word leg (s10) and the
//! vector leg over each graph's blocks (s13), fused by reciprocal rank. It runs in three steps so a hosted query embed
//! never holds the writer: [`Services::search_plan`] under the shared lock decides which graphs get a vector leg,
//! [`SearchPlan::embed`] embeds the query with no lock held, and [`Services::search_finish`] scores under the shared
//! lock again, checking each graph's key once more.
//!
//! Deterministic (architect): the constants are named, sums run in a fixed order (word leg then vector leg), and ties
//! go to the graph alias and then the lower handle, as `umx search` has ordered since s10.
//!
//! PRD 002 search-09: a search may carry up to 5 phrasings of one question (`--also`). Each runs its own exact, word
//! and vector legs over the same graphs, scope and floor; the phrasings are embedded in one call per instance, and
//! [`fuse_phrasings`] sums each node's credit over them, so agreement between phrasings ranks a node higher.

use crate::graph_store::GRAPH_WIDE;
use crate::{Ctx, Done, Outcome, Services, problems};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use umx_core::ports::{EmbedError, Embedder, GraphDir, Locality, ModelKey, TextKind};
use umx_core::secrets;
use umx_core::similarity::{self, inverse_length, top_k};
use umx_core::vectors::quantise;
use umx_core::{Command, Id, Node, Policy, Problem, ValidName, Value};
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
    /// The reciprocal rank sum over the legs that found it, summed over the phrasings.
    pub fused: f64,
    /// PRD 002 search-09: the 1-based positions of the phrasings that found it (1 is the main text, 2 to 5 each
    /// `--also` in order). `word` and `vector` are those of its best phrasing, the largest term in its sum.
    pub phrasings: Vec<usize>,
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
    /// How many phrasings were searched: 1, or more with `--also` (PRD 002 search-09).
    pub phrasings: usize,
}

/// Why a vector space got no query vector.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryFailed {
    /// The provider failed.
    Embed(EmbedError),
    /// The embed took longer than its bound (req-53, 5 s in the daemon). `warming`: the instance has not answered
    /// a query yet, so a local model may still be loading.
    TimedOut { warming: bool },
    /// The query (any of its phrasings) looks like it holds a secret and the instance is not local, so it was not
    /// asked (req-49; PRD 002 search-09: all or none).
    Secret,
    /// A graph whose leg uses the instance no longer admits it (its embedder setting changed since the
    /// plan, or its folder's config), so it was not asked (D53).
    NotAdmitted,
}

/// The query vectors of one search, by embedder instance (by name) a vector leg uses: one vector per phrasing, in
/// order. Two instances of one model are kept apart, so each graph's leg reads only the vectors of the instance its
/// own setting admits (D53).
pub type QueryVectors = BTreeMap<String, Result<Vec<Vec<f32>>, QueryFailed>>;

/// Whether graph `alias`'s leg may still use instance `name`, asked right before the instance is sent the query.
pub type Admitted<'a> = &'a dyn Fn(&str, &str) -> bool;

/// How one query embed is run, every phrasing in one call: the daemon bounds it in time (req-53);
/// [`SearchPlan::embed`] calls it directly.
pub type QueryRunner<'a> =
    &'a dyn Fn(&Arc<dyn Embedder>, &[String]) -> Result<Vec<Vec<f32>>, QueryFailed>;

/// A search decided under the shared lock: the graphs, which of them get a vector leg (and in which space), and the
/// warnings so far. Holds no borrow of the services.
pub struct SearchPlan {
    /// The phrasings: the main text, then each `--also` (PRD 002 search-09).
    texts: Vec<String>,
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
    /// `--tag` and `--no-tag` (ws008 t2b): every leg is narrowed to the nodes that pass, from each graph's tag index,
    /// before it ranks; a graph without std gives no hits and is counted.
    tags: Option<umx_graph::TagFilter>,
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
    /// fails with [`QueryFailed::Secret`]. Every phrasing goes in the one call, and is scanned: if any looks like a
    /// secret, no instance that is not local is asked at all (PRD 002 search-09).
    pub fn embed_with(&self, admitted: Admitted<'_>, run: QueryRunner<'_>) -> QueryVectors {
        let mut out = QueryVectors::new();
        let secret = self.texts.iter().any(|t| secrets::scan(t).is_some());
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
                run(embedder, &self.texts)
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

    /// `--tag` and `--no-tag` (ws008 t2b, req-117). A search of one graph that does not use std is refused
    /// (`tag_needs_std`); a list, a scope, `--all` or an unnamed search of several graphs skips those without std,
    /// with `tag_filter_not_std` at finish. A refused tag's next repeats the graph (a list's first std graph) and every
    /// tag; the words are a placeholder.
    pub fn tags(
        mut self,
        services: &Services,
        ctx: &Ctx,
        tags: &[String],
        no_tags: &[String],
    ) -> Result<SearchPlan, Problem> {
        use crate::graph_store::tags as t;
        if tags.is_empty() && no_tags.is_empty() {
            return Ok(self);
        }
        let is_std = |a: &String| {
            services
                .graph
                .loaded(a)
                .is_some_and(|l| t::is_std(l.ontology()))
        };
        let loaded = services.graph.loaded_count();
        if let [only] = self.aliases.as_slice()
            && !t::several(ctx, loaded)
            && !is_std(only)
        {
            return Err(problems::tag_needs_std(&crate::graph_store::alias_name(
                only,
            )));
        }
        let first = self.aliases.iter().find(|a| is_std(a)).map(String::as_str);
        let base = t::read_graph(ctx, Command::new("search").word("<words>"), first, loaded);
        self.tags = t::read_filter(tags, no_tags, base)?;
        Ok(self)
    }

    /// PRD 002 search-09: more phrasings of the question (`--also`), each searched alike over the same graphs,
    /// scope and floor, and fused into one list. The caller has checked them (at most 4, none empty or repeated:
    /// `umx_protocol::also_problem`).
    pub fn also(mut self, also: &[String]) -> SearchPlan {
        self.texts.extend(also.iter().cloned());
        self
    }
}

/// One query embed of every phrasing, unbounded.
fn ask(embedder: &Arc<dyn Embedder>, texts: &[String]) -> Result<Vec<Vec<f32>>, QueryFailed> {
    embedder
        .embed(texts, TextKind::Query)
        .map(|e| e.vectors)
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
    /// The 1-based positions of the phrasings that found it (PRD 002 search-09).
    pub phrasings: Vec<usize>,
}

/// One phrasing's legs as [`fuse`] takes them: its exact hits, its word leg and its vector leg, each best first.
pub type Legs<'a> = (&'a [(String, Id, String)], &'a WordRanked, &'a Ranked);

/// Reciprocal rank fusion (008 section 11, rules 1, 2 and 4; embeddings-expert's coverage, s11), pure: `exact` hits
/// lead in (alias, handle) order; the rest by `coverage / (RRF_K + word rank) + 1 / (RRF_K + vector rank)` (word
/// then vector), then the better single-leg rank, then alias, then handle. Every key in any input is in the output
/// once. One phrasing: [`fuse_phrasings`] with one entry.
pub fn fuse(
    exact: &[(String, Id, String)],
    word: &WordRanked,
    vector: &Ranked,
) -> Vec<((String, Id), Fused)> {
    fuse_phrasings(&[(exact, word, vector)])
}

/// [`fuse`] over several phrasings of one question (PRD 002 search-09; architect, embeddings-expert): a node exact
/// for any phrasing is exact (a tier, not a term, so phrasings cannot outweigh exact hits), reporting the field of
/// the first phrasing that matched it exactly. Each phrasing's term is its `coverage / (RRF_K + word rank) +
/// 1 / (RRF_K + vector rank)`, and a node's fused score sums the terms in phrasing order, so a node two phrasings
/// find at rank r scores 2 / (RRF_K + r) against 1 / (RRF_K + r) for one. No 1/P scale: one factor for every node
/// cannot change the order. The word and vector a node shows are those of its best phrasing (its largest term, the
/// earlier on a tie); ties in the sum go to the best single-leg rank over every phrasing, then alias, then handle.
/// With one phrasing this is [`fuse`]'s ranking, bit for bit.
pub fn fuse_phrasings(phrasings: &[Legs<'_>]) -> Vec<((String, Id), Fused)> {
    struct Found {
        exact: Option<String>,
        /// Per phrasing: found exactly, its word match, its vector match.
        each: Vec<(bool, Option<WordMatch>, Option<VectorMatch>)>,
    }
    let mut by: BTreeMap<(String, Id), Found> = BTreeMap::new();
    let new = || Found {
        exact: None,
        each: vec![(false, None, None); phrasings.len()],
    };
    for (p, (exact, word, vector)) in phrasings.iter().enumerate() {
        for (alias, h, field) in exact.iter() {
            let f = by.entry((alias.clone(), *h)).or_insert_with(new);
            f.exact.get_or_insert_with(|| field.clone());
            f.each[p].0 = true;
        }
        for (i, (alias, h, score, field, coverage, terms)) in word.iter().enumerate() {
            by.entry((alias.clone(), *h)).or_insert_with(new).each[p].1 = Some(WordMatch {
                rank: i + 1,
                score: *score,
                field: field.clone(),
                coverage: *coverage,
                terms: terms.clone(),
            });
        }
        for (i, (alias, h, score, field)) in vector.iter().enumerate() {
            by.entry((alias.clone(), *h)).or_insert_with(new).each[p].2 = Some(VectorMatch {
                rank: i + 1,
                cosine: *score as f32,
                field: field.clone(),
            });
        }
    }
    let leg = |r: usize| 1.0 / (RRF_K + r as f64);
    let mut out: Vec<((String, Id), Fused, usize)> = by
        .into_iter()
        .map(|(k, found)| {
            let (mut sum, mut best, mut rank) = (None::<f64>, None::<(f64, usize)>, usize::MAX);
            for (p, (_, word, vector)) in found.each.iter().enumerate() {
                if word.is_none() && vector.is_none() {
                    continue;
                }
                let term = word.as_ref().map_or(0.0, |w| w.coverage * leg(w.rank))
                    + vector.as_ref().map_or(0.0, |v| leg(v.rank));
                sum = Some(sum.map_or(term, |s| s + term));
                if best.is_none_or(|(t, _)| term > t) {
                    best = Some((term, p));
                }
                let w = word.as_ref().map_or(usize::MAX, |w| w.rank);
                let v = vector.as_ref().map_or(usize::MAX, |v| v.rank);
                rank = rank.min(w).min(v);
            }
            let phrasings = found
                .each
                .iter()
                .enumerate()
                .filter(|(_, (exact, word, vector))| *exact || word.is_some() || vector.is_some())
                .map(|(p, _)| p + 1)
                .collect();
            let (word, vector) = match best {
                Some((_, p)) => (found.each[p].1.clone(), found.each[p].2.clone()),
                None => (None, None),
            };
            let fused = Fused {
                exact: found.exact,
                word,
                vector,
                fused: sum.unwrap_or(0.0),
                phrasings,
            };
            (k, fused, rank)
        })
        .collect();
    out.sort_by(|(ka, a, ra), (kb, b, rb)| {
        b.exact
            .is_some()
            .cmp(&a.exact.is_some())
            .then_with(|| {
                if a.exact.is_some() && b.exact.is_some() {
                    core::cmp::Ordering::Equal
                } else {
                    b.fused.total_cmp(&a.fused).then(ra.cmp(rb))
                }
            })
            .then_with(|| ka.cmp(kb))
    });
    out.into_iter().map(|(k, f, _)| (k, f)).collect()
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
            texts: vec![text.to_string()],
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
            tags: None,
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
            texts,
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
            tags,
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
        // ws008 t2b: a graph without std has no std tags, so a tag filter finds nothing there; counted, never named.
        // By graph id: named copies of one graph are one graph skipped (d-21).
        let not_std = aliases
            .iter()
            .map(|a| store.loaded(a).expect("checked"))
            .filter(|l| tags.is_some() && !l.graph.std_tags())
            .map(|l| l.meta.id)
            .collect::<BTreeSet<Id>>()
            .len();
        if not_std > 0 {
            warnings.push(problems::tag_filter_not_std(not_std));
        }
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
        // By instance: a leg reads only the vectors of the instance it was planned with (D53), one per phrasing. An
        // instance that gave too few, or one with no direction, gives this search no vector leg at all (search-09:
        // all or none, as the secret rule).
        let mut query: BTreeMap<String, Vec<Vec<i8>>> = BTreeMap::new();
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
                Some(Ok(vs)) => {
                    let qs: Vec<Vec<i8>> = vs.iter().map(|v| quantise(v)).collect();
                    if qs.len() != texts.len()
                        || qs
                            .iter()
                            .any(|q| inverse_length(q) == 0.0 || q.len() != key.dims())
                    {
                        failed.insert("the query vector has no direction");
                        return false;
                    }
                    query.insert(name.clone(), qs);
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
        // Word leg, once per phrasing (search-09): figures summed over every graph searched (s10); each hit's credit
        // scaled by its coverage of that phrasing's informative terms.
        let mut words: Vec<WordRanked> = Vec::new();
        // Phrasings whose word leg gave fusion nothing, every word common; `word_leg` says so when all did.
        let mut uninformative = 0;
        // A capped prefix is named once, however many phrasings hold it.
        let mut capped: BTreeSet<String> = BTreeSet::new();
        for text in &texts {
            let mut word = WordRanked::new();
            let mut no_terms = false;
            if let Some(q) = WordQuery::new(text) {
                let mut stats = WordStats::default();
                for a in &aliases {
                    stats.add(&store.loaded(a).expect("checked").graph.word_stats(&q));
                }
                let mut named = Vec::new();
                for (w, n) in stats.capped(&q) {
                    if !capped.contains(w) {
                        warnings.push(problems::word_prefix_capped(w, n, umx_graph::PREFIX_CAP));
                        named.push(w.to_string());
                    }
                }
                capped.extend(named);
                let most = word_max_df_share * stats.nodes as f64;
                // A word's df here is its exact and stem matches' summed (s21): a common class is not informative.
                let informative: Vec<bool> = (0..q.terms().len())
                    .map(|i| stats.informative_df(i) as f64 <= most)
                    .collect();
                // Coverage weighs a word hit only in a graph that also has a vector leg: a graph searched by words
                // alone keeps every word hit at full credit (req-53), whatever the other graphs have.
                let wanted = informative.iter().filter(|&&i| i).count();
                for a in &aliases {
                    let gated = legs.contains_key(a)
                        && routes.get(a).is_some_and(|n| query.contains_key(n));
                    if gated && wanted == 0 {
                        no_terms = true;
                        continue;
                    }
                    let g = &store.loaded(a).expect("checked").graph;
                    let Some(scope) = scope_of(&root, a, g, include_provenance) else {
                        continue;
                    };
                    let scope = tagged(
                        narrow(scope, kept.as_ref(), a, g),
                        tags.as_ref(),
                        g,
                        include_provenance,
                    );
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
            uninformative += usize::from(no_terms);
            words.push(rank_words(word));
        }
        let word_leg = (uninformative == texts.len()).then_some("no informative terms");
        // Vector leg, once per phrasing: graphs compared only within one space (req-37), each phrasing at the same
        // floor.
        let mut vectors: Vec<Ranked> = vec![Ranked::new(); texts.len()];
        let mut floors = Vec::new();
        let (mut pending, mut held) = (0usize, 0usize);
        let spaces: BTreeSet<&ModelKey> = legs.values().collect();
        for key in spaces {
            let cut = store.floors().get(key, similarity::Use::Search);
            let floor = cut.value;
            floors.push((key.clone(), cut));
            let space = key.to_string();
            let mut in_space: Vec<Ranked> = vec![Ranked::new(); texts.len()];
            for (alias, _) in legs.iter().filter(|(_, k)| *k == key) {
                let qs = &query[&routes[alias]];
                let g = &store.loaded(alias).expect("checked").graph;
                let Some(scope) = scope_of(&root, alias, g, include_provenance) else {
                    continue;
                };
                let scope = tagged(
                    narrow(scope, kept.as_ref(), alias, g),
                    tags.as_ref(),
                    g,
                    include_provenance,
                );
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
                for (q, ranked) in qs.iter().zip(in_space.iter_mut()) {
                    for (h, cosine, field) in vector_candidates(g, &space, q, scope.as_ref(), floor)
                    {
                        ranked.push((alias.clone(), h, f64::from(cosine), field));
                    }
                }
            }
            for (vector, ranked) in vectors.iter_mut().zip(in_space) {
                vector.extend(rank(ranked));
            }
        }
        if pending > 0 {
            warnings.push(problems::similarity_incomplete(pending, held));
        }
        // Exact leg, for each phrasing in one pass over the nodes. Lower-casing can change a text's byte length only
        // outside ASCII, so an ASCII text of another length is skipped without allocating; an id is compared only when
        // the phrasing is as long as one.
        let needles: Vec<(String, bool, bool)> = texts
            .iter()
            .map(|t| {
                let needle = t.trim().to_lowercase();
                let id_like = needle.len() == umx_core::id::ID_LEN;
                let time_like = needle.len() >= 10 && needle.as_bytes()[4] == b'-';
                (needle, id_like, time_like)
            })
            .collect();
        let mut exact: Vec<Vec<(String, Id, String)>> = vec![Vec::new(); texts.len()];
        for a in &aliases {
            let l = store.loaded(a).expect("checked");
            let Some(scope) = scope_of(&root, a, &l.graph, include_provenance) else {
                continue;
            };
            let scope = tagged(
                narrow(scope, kept.as_ref(), a, &l.graph),
                tags.as_ref(),
                &l.graph,
                include_provenance,
            );
            let text_fields = l.ontology().text_fields();
            for n in ctx.read_scope(&l.graph).nodes() {
                if scope.as_ref().is_some_and(|s| !s.contains_ix(n.ix())) {
                    continue;
                }
                for ((needle, id_like, time_like), found) in needles.iter().zip(exact.iter_mut()) {
                    let could =
                        |t: &str| t.len() == needle.len() || !t.is_ascii() || !needle.is_ascii();
                    let same = |t: &str| could(t) && t.to_lowercase() == *needle;
                    // Title, id, then the built-in short columns (type, status, created, updated, created_by), then
                    // custom fields in name order: the first that equals the phrasing is the one reported.
                    // Times are rendered only for a phrasing that could be one (they allocate).
                    let builtin = || {
                        [("type", n.kind()), ("status", n.status())]
                            .into_iter()
                            .find(|(_, v)| same(v))
                            .map(|(f, _)| f)
                            .or_else(|| (*time_like && same(&n.created())).then_some("created"))
                            .or_else(|| (*time_like && same(&n.updated())).then_some("updated"))
                            .or_else(|| same(n.created_by()).then_some("created_by"))
                    };
                    let field = if same(n.title()) {
                        Some("title".to_string())
                    } else if *id_like && n.id().to_text().to_lowercase() == *needle {
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
                        found.push((a.clone(), n.id(), field));
                    }
                }
            }
        }
        let each: Vec<Legs<'_>> = exact
            .iter()
            .zip(&words)
            .zip(&vectors)
            .map(|((e, w), v)| (e.as_slice(), w, v))
            .collect();
        let fused = fuse_phrasings(&each);
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
                phrasings: f.phrasings,
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
                phrasings: texts.len(),
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
        self.search_phrasings(ctx, text, &[], within, limit, along)
    }

    /// [`Self::search_along`] with more phrasings of the question (`--also`, PRD 002 search-09), each searched alike
    /// and fused into one list; every phrasing goes to each instance in one call.
    pub fn search_phrasings(
        &self,
        ctx: &Ctx,
        text: &str,
        also: &[String],
        within: Option<&str>,
        limit: usize,
        along: Option<crate::AlongRequest>,
    ) -> Outcome<HybridResult> {
        self.search_tagged(ctx, text, also, within, limit, along, (&[], &[]))
    }

    /// [`Self::search_phrasings`] narrowed by `--tag` and `--no-tag` (ws008 t2b): see [`SearchPlan::tags`].
    #[allow(clippy::too_many_arguments)]
    pub fn search_tagged(
        &self,
        ctx: &Ctx,
        text: &str,
        also: &[String],
        within: Option<&str>,
        limit: usize,
        along: Option<crate::AlongRequest>,
        (tags, no_tags): (&[String], &[String]),
    ) -> Outcome<HybridResult> {
        if along.is_some() {
            crate::graph_store::along_one_graph(ctx)?;
        }
        let plan = self
            .search_plan(ctx, text, within, limit)?
            .along(ctx, along)?
            .tags(self, ctx, tags, no_tags)?
            .also(also);
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

/// A leg's scope narrowed to the nodes `tags` admits (ws008 t2b), from the graph's tag index: a set the legs search
/// within, so the tags cut before ranking and a page is never emptied after. A graph without std admits none.
fn tagged<'g>(
    scope: Option<NodeSet<'g>>,
    tags: Option<&umx_graph::TagFilter>,
    g: &'g Graph,
    include_provenance: bool,
) -> Option<NodeSet<'g>> {
    let Some(f) = tags else {
        return scope;
    };
    let set = g.tag_set(f);
    Some(match scope {
        Some(s) => s.and(&set),
        None if include_provenance => set,
        None => g.node_set_where(|ix| set.contains_ix(ix) && !g.is_provenance(ix)),
    })
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

    /// The fusion as it was before search-09, verbatim: the reference [`fuse_phrasings`] must equal bit for bit with
    /// one phrasing (embeddings-expert: with no --also the list is byte-identical).
    fn old_fuse(
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

    /// search-09 (embeddings-expert Q2a): with one phrasing the new fusion is the old one, bit for bit, on 2,000
    /// generated leg sets (ties, overlaps, exact hits, fractional and zero coverage); an extra phrasing that finds
    /// nothing changes nothing but the positions.
    #[test]
    fn one_phrasing_fuses_exactly_as_before() {
        let mut seed: u64 = 0x5eed_0909;
        let mut next = |n: u64| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) % n
        };
        let coverages = [1.0, 0.5, 1.0 / 3.0, 1.0 / 6.0, 0.0];
        for _ in 0..2000 {
            let mut pick = |most: u64| {
                let mut keys: Vec<(String, u128)> = Vec::new();
                for _ in 0..next(most) {
                    let k = (
                        ["a", "b", "c"][next(3) as usize].to_string(),
                        u128::from(next(30)),
                    );
                    if !keys.contains(&k) {
                        keys.push(k);
                    }
                }
                keys
            };
            let exact: Vec<(String, Id, String)> = pick(3)
                .into_iter()
                .map(|(a, n)| (a, Id(n), "title".to_string()))
                .collect();
            let words: Vec<(String, u128)> = pick(25);
            let vectors: Vec<(String, u128)> = pick(25);
            let word: WordRanked = words
                .iter()
                .map(|(a, n)| word(a, *n, 9.0, coverages[(*n % 5) as usize]))
                .collect();
            let vector: Ranked = vectors.iter().map(|(a, n)| hit(a, *n, 0.8)).collect();
            let old = old_fuse(&exact, &word, &vector);
            let new = fuse(&exact, &word, &vector);
            let empty = (Vec::new(), WordRanked::new(), Ranked::new());
            let padded = fuse_phrasings(&[
                (&exact, &word, &vector),
                (empty.0.as_slice(), &empty.1, &empty.2),
            ]);
            for got in [&new, &padded] {
                assert_eq!(got.len(), old.len());
                for ((ko, o), (kn, n)) in old.iter().zip(got.iter()) {
                    assert_eq!(ko, kn);
                    assert_eq!(o.fused.to_bits(), n.fused.to_bits(), "{ko:?}");
                    assert_eq!(
                        (&o.exact, &o.word, &o.vector),
                        (&n.exact, &n.word, &n.vector)
                    );
                    assert_eq!(n.phrasings, [1]);
                }
            }
        }
    }
}
