//! Strength mode (PRD 002 search-10; the embeddings-expert's rule and named measurement): one key's strength cuts
//! from the rows its floors run cached, every text embedded as umx stores it (int8 rows, cosines as top_k scores
//! them), sending nothing: a text not in the cache fails the run.
//!
//! Corpora: BEIR SciFact, NFCorpus and ArguAna, the s03 fixture, and `tight`, a near-duplicate-heavy graph: the
//! [`TIGHT`] NFCorpus documents nearest (whole-node cosine) to the NFCorpus document with the most neighbours at or
//! above the key's similar_to floor. Each corpus is sampled at N = 300, 2,000 and 20,000 (capped at its size; one
//! seeded sample per N, the target always in it).
//!
//! On-topic: each of the corpus's own queries records its target (its relevant document with the best whole-node
//! cosine): whole-node cosine, robust z against the N nodes' whole-node cosines ([`Population`]), and whether it
//! passed the floor (its best row, whole node or field, as the vector leg cuts). Off-topic: a subject the graph does
//! not hold (the PRD's definition): another corpus's queries, kept only for the pairs in [`DISJOINT`] (spot-checked:
//! the 20 highest off-topic top hits show nothing on-topic; `--spot <file>` writes that evidence for every pair), and
//! the written off-topic list (embed-bakeoff/offtopic.jsonl, subjects no fixture node holds) where its recorded rows
//! exist. An off-topic query records its top hit: the best whole-node cosine among the nodes that passed the floor.
//!
//! The cuts, chosen from off-topic queries only (on-topic rates are reported, never targeted). A rule family gives
//! the cuts at a quantile level L of the kept off-topic queries' top hits (a query with no top hit past the floor
//! counts as the lowest value): `joint` a z line (a least-squares fit on ln N through each cell's z L-quantile,
//! raised until no cell lies above it) AND the pooled cosine L-quantile;
//! `cosine` the cosine alone; `z` the z line alone. Strong takes the loosest L (0.001 steps down from 0.999) at which
//! no cell's off-topic top hits are strong more than `--strong-rate` (1 in 100); fair the loosest at which none are
//! fair or strong more than 1 in 10. The shipped rule is `joint` (`--cosine-only` ships `cosine`); the others are the
//! z ablation, reported beside it.
//!
//! MIN_POPULATION: on a ladder of small N, a seeded bootstrap of each population ([`BOOT`] resamples) gives the spread
//! of a hit's z; the smallest N from which every larger rung's z moves (p90 of the 90% half-widths) less than half the
//! gap between the fair and strong z cuts, so a resample moves a label at most one step.

use super::*;
use umx_core::similarity::{Population, Strength, StrengthCuts, Use, strength};
use umx_core::vectors::TextHash;

/// The sampling seed: each (corpus, N) population is the first N of a splitmix64 shuffle seeded from it.
const SAMPLE_SEED: u64 = 1010;
/// The bootstrap seed.
const BOOT_SEED: u64 = 1011;
const LEVELS: [usize; 3] = [300, 2000, 20_000];
/// Off-topic top hits a fair cut may let be fair or strong: 1 in 10 (embeddings-expert).
const FAIR_RATE: f64 = 0.10;
const LADDER: [usize; 11] = [10, 15, 20, 30, 50, 75, 100, 150, 200, 300, 500];
const BOOT: usize = 25;
/// On-topic and off-topic queries per corpus in the bootstrap.
const BOOT_QUERIES: usize = 40;
/// Fixture queries embedded `--repeat` times live (the int8 reproducibility count).
const REPEAT_QUERIES: usize = 20;
/// The near-duplicate-heavy corpus's size.
const TIGHT: usize = 1000;
/// Off-topic top hits per pair in the spot check.
const SPOT: usize = 20;

/// The off-topic pairs kept, (corpus, query source, what the spot check showed), each after reading its 20 highest
/// off-topic top hits (`--spot`, embeddinggemma-2@256+p1, 2026-11-19): domains disjoint, nothing on-topic among them.
const DISJOINT: &[(&str, &str, &str)] = &[
    (
        "scifact",
        "fixture",
        "biomedical claims against food-plant, delivery, design and customer questions; the nearest share a word-level theme (allergy, bacteria, vision) but answer another question",
    ),
    (
        "scifact",
        "written",
        "8 of 45 past the floor; nearest are cell biology papers on a shared word (migration, thermogenesis, honey bee genomics against a waggle-dance question)",
    ),
    (
        "nfcorpus",
        "written",
        "4 of 45 past the floor; nearest share one word (whales and tuna, herding farms and Campylobacter)",
    ),
    (
        "arguana",
        "fixture",
        "debates against operational questions; nearest are unrelated arguments, one food-labelling policy debate against an allergen label review task",
    ),
    (
        "fixture",
        "scifact",
        "scientific claims against the four fixture domains; nearest are design-studio notes matched on a word (fidelity, warm-ups)",
    ),
    (
        "fixture",
        "written",
        "3 of 45 past the floor; nearest share one word (workshop, scoring)",
    ),
    ("tight", "written", "0 of 45 past the floor"),
];

/// The pairs dropped after the same spot check: some of their top hits are on topic.
const OVERLAPPING: &[(&str, &str, &str)] = &[
    (
        "scifact",
        "nfcorpus",
        "both biomedical and nutrition science",
    ),
    (
        "scifact",
        "arguana",
        "health debates (organ donation, vegetarianism, obesity, HIV) meet their papers",
    ),
    (
        "nfcorpus",
        "scifact",
        "both biomedical and nutrition science",
    ),
    (
        "nfcorpus",
        "arguana",
        "diet debates (vegetarianism, GM food, fat tax) meet nutrition papers",
    ),
    (
        "nfcorpus",
        "fixture",
        "food-safety questions meet food microbiology and pesticide papers",
    ),
    (
        "arguana",
        "scifact",
        "claims on policy subjects (one-child policy, open access, obesity) meet debates on them",
    ),
    (
        "arguana",
        "nfcorpus",
        "nutrition topics (trans fats, junk food, smoking) meet debates on them",
    ),
    (
        "arguana",
        "written",
        "sport and history questions (chess, cricket, the Roman Empire) meet debates on the same subjects",
    ),
    (
        "fixture",
        "nfcorpus",
        "food topics (shelf life, sweeteners, supplier risk) meet the food-plant notes",
    ),
    (
        "fixture",
        "arguana",
        "labelling and whistleblowing debates meet the food-plant notes on them",
    ),
    (
        "tight",
        "scifact",
        "biomedical claims meet the nutrition cluster",
    ),
    (
        "tight",
        "arguana",
        "diet debates meet the nutrition cluster",
    ),
    (
        "tight",
        "fixture",
        "food-safety questions (pesticides, additives, organic produce) meet the nutrition cluster",
    ),
];

/// The fixture's written off-topic list's query source name.
const WRITTEN: &str = "written";

/// One corpus: its documents' whole-node rows and every row, a short text of each (for the spot check only), and
/// its on-topic queries.
struct Corpus {
    name: String,
    ids: Vec<String>,
    whole: Vec<Row>,
    rows: Vec<Vec<Row>>,
    texts: Vec<String>,
    /// Its own queries with a target: id, row, target index, exact (equals a title), kind (fixture only).
    on: Vec<(String, Row, usize, bool, String)>,
    documents: usize,
    held: usize,
    source: Value,
}

/// Removes a scratch folder when dropped, so an error never leaves it behind.
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// One query source: (id, text, row).
struct Source {
    name: String,
    queries: Vec<(String, String, Row)>,
}

#[derive(Clone)]
struct Rec {
    corpus: String,
    n: usize,
    on: bool,
    source: String,
    query: String,
    kind: String,
    exact: bool,
    /// On-topic: the target's whole-node cosine and z, and whether its best row passed the floor.
    score: f32,
    z: Option<f64>,
    passes_floor: bool,
    /// The best whole-node cosine among the nodes that passed the floor, and its z: the hit whose label is the
    /// query's best (labels rise with the cosine within one population).
    top: Option<(f32, Option<f64>)>,
    pop: Population,
}

/// One label's cuts: z >= `z + z_per_ln * ln N` (unless `z_part` is off) and cosine >= `score`.
#[derive(Clone, Copy, Debug)]
struct Cut {
    z: f64,
    z_per_ln: f64,
    score: f64,
    z_part: bool,
}

impl Cut {
    fn passes_z(&self, z: Option<f64>, n: usize) -> bool {
        !self.z_part || z.is_some_and(|z| z >= self.z + self.z_per_ln * (n as f64).ln())
    }
    fn passes(&self, s: f32, z: Option<f64>, n: usize) -> bool {
        f64::from(s) >= self.score && self.passes_z(z, n)
    }
    fn json(&self) -> Value {
        json!({ "z": self.z, "z_per_ln": self.z_per_ln, "score": self.score, "z_part": self.z_part })
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Family {
    Joint,
    Cosine,
    Z,
}

impl Family {
    fn name(self) -> &'static str {
        match self {
            Family::Joint => "joint",
            Family::Cosine => "cosine_only",
            Family::Z => "z_only",
        }
    }
}

fn seed_of(set: &str, n: usize, base: u64) -> u64 {
    set.bytes()
        .fold(base ^ (n as u64).wrapping_mul(0x9E37_79B9), |h, b| {
            h.wrapping_mul(0x100_0000_01B3) ^ u64::from(b)
        })
}

/// The first `n` of a seeded shuffle of `0..total`.
fn sample(total: usize, n: usize, seed: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..total).collect();
    let mut rng = Rng(seed);
    for i in (1..order.len()).rev() {
        order.swap(i, rng.below(i + 1));
    }
    order.truncate(n);
    order
}

/// Nearest-rank quantile.
fn quantile(v: &[f64], p: f64) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let i = ((p * s.len() as f64).ceil() as usize).clamp(1, s.len()) - 1;
    Some(s[i])
}

fn q(v: &[f64], p: f64) -> Value {
    quantile(v, p).map_or(Value::Null, |x| json!(round4(x)))
}

/// The z and cosine an unlabelled off-topic query counts as in the quantiles (below any real value).
const NO_Z: f64 = -1000.0;
const NO_COSINE: f64 = -1.0;

/// A line `a + b ln N` through the points by least squares, raised until no point lies above it; with no points, a
/// line no z falls below.
fn envelope(points: &[(f64, f64)]) -> (f64, f64) {
    if points.is_empty() {
        return (NO_Z, 0.0);
    }
    let n = points.len() as f64;
    let mx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let my = points.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
    let sxy: f64 = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let b = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let a = my - b * mx;
    let lift = points
        .iter()
        .map(|p| p.1 - (a + b * p.0))
        .fold(0.0f64, f64::max);
    (a + lift, b)
}

/// Rounds a cut up at the 4th decimal, so the stored value never sits below what was measured.
fn up4(x: f64) -> f64 {
    (x * 1e4).ceil() / 1e4
}

fn snippet(title: &str, body: &str) -> String {
    let t = title_of(title, body);
    let b: String = body.chars().take(160).collect();
    format!("{t} :: {b}")
}

fn load(
    name: &str,
    a: &Args,
    cached: &Arc<Cached>,
    scratch: &Path,
    kinds: &BTreeMap<String, String>,
) -> Result<(Corpus, Source), String> {
    let task = if name == "fixture" {
        fixture()?
    } else {
        beir(&a.cache, name)?
    };
    let b = build_with(
        &task,
        &scratch.join(name),
        cached,
        a.tokens_per_min,
        false,
        false,
    )?;
    // A node held as a suspected secret (req-49) has no row, here as in production; a failed one would be a text
    // missing from the cache.
    if b.failed > 0 {
        return Err(format!(
            "{name}: {} nodes failed: every row must come from the cache",
            b.failed
        ));
    }
    let (mut ids, mut whole, mut rows, mut texts) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut at: HashMap<String, usize> = HashMap::new();
    for (d, rs) in b.rows.iter().enumerate() {
        if let Some((_, r)) = rs.iter().find(|(w, _)| *w) {
            at.insert(task.docs[d].0.clone(), whole.len());
            ids.push(task.docs[d].0.clone());
            whole.push(r.clone());
            rows.push(rs.iter().map(|(_, r)| r.clone()).collect());
            texts.push(snippet(&task.docs[d].1, &task.docs[d].2));
        }
    }
    let titles: BTreeSet<String> = task
        .docs
        .iter()
        .map(|(_, t, body)| title_of(t, body).to_lowercase())
        .collect();
    let (mut on, mut all) = (Vec::new(), Vec::new());
    // Each query embedded as umx search embeds it (SearchPlan::embed: Query texts).
    for (id, text) in &task.queries {
        let got = cached
            .embed(std::slice::from_ref(text), TextKind::Query)
            .map_err(|e| format!("query {id}: {e:?}"))?;
        let Some(row) = got.vectors.first().map(|v| Row::of(v)) else {
            continue;
        };
        all.push((id.clone(), text.clone(), row.clone()));
        let rel: Vec<usize> = task
            .qrels
            .get(id)
            .into_iter()
            .flat_map(|r| r.keys())
            .filter(|d| !(task.ignore_own && **d == *id))
            .filter_map(|d| at.get(d).copied())
            .collect();
        // The target: the relevant document with the best whole-node cosine, the lower index on a tie.
        if let Some(&t) = rel.iter().max_by(|&&x, &&y| {
            row.cosine(&whole[x])
                .total_cmp(&row.cosine(&whole[y]))
                .then(y.cmp(&x))
        }) {
            let exact = titles.contains(&text.trim().to_lowercase());
            let kind = kinds.get(id).cloned().unwrap_or_default();
            on.push((id.clone(), row, t, exact, kind));
        }
    }
    Ok((
        Corpus {
            name: name.into(),
            ids,
            whole,
            rows,
            texts,
            on,
            documents: task.docs.len(),
            held: b.held,
            source: task.source.clone(),
        },
        Source {
            name: name.into(),
            queries: all,
        },
    ))
}

/// The written off-topic list with its recorded rows for `key`, when they are committed.
fn written(key: &ModelKey) -> Result<Option<Source>, String> {
    let dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/project/benchmarks/embed-bakeoff");
    let Ok(tsv) = fs::read_to_string(dir.join(format!("vectors/{}", offtopic_file(key)))) else {
        return Ok(None);
    };
    let rows: HashMap<String, Vec<u8>> = tsv
        .lines()
        .filter_map(|l| {
            let mut p = l.split('\t');
            (p.next()? == "query").then_some(())?;
            let h = p.next()?.to_string();
            Some((h, umx_core::vectors::base64_decode(p.next()?)?))
        })
        .collect();
    let mut queries = Vec::new();
    for q in json_lines(&dir.join("offtopic.jsonl"))? {
        let (id, t) = (text(&q, "id"), text(&q, "text"));
        let row = rows
            .get(&TextHash::of(&t).to_string())
            .ok_or(format!("written off-topic {id} has no recorded row"))?;
        let v: Vec<i8> = row.iter().map(|&b| b as i8).collect();
        let inv = inverse_length(&v);
        queries.push((id, t, Row { v, inv }));
    }
    Ok(Some(Source {
        name: WRITTEN.into(),
        queries,
    }))
}

/// The written off-topic rows file's name for `key`: the key with `@` written `_` (as the related fixtures are named),
/// so a path never reads as an email address.
fn offtopic_file(key: &ModelKey) -> String {
    format!("{}.offtopic.rows.tsv", key.to_string().replace('@', "_"))
}

/// The committed written off-topic rows file for `key` and its sha256, when it exists.
fn written_rows(key: &ModelKey) -> Value {
    let file = format!(
        "docs/project/benchmarks/embed-bakeoff/vectors/{}",
        offtopic_file(key)
    );
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(&file);
    fs::read(&path).map_or(
        Value::Null,
        |b| json!({ "file": "docs/project/benchmarks/embed-bakeoff/vectors/<model>_<dims>[+p<n>].offtopic.rows.tsv", "sha256": umx_core::models::sha256_hex(&b) }),
    )
}

/// The near-duplicate-heavy corpus: the [`TIGHT`] documents of `base` nearest the one with the most neighbours at or
/// above `similar`, with the base's queries whose target lies among them.
fn tight(base: &Corpus, similar: f32, search: f32) -> (Corpus, Value) {
    let n = base.whole.len();
    let mut best = (0usize, 0usize);
    for i in 0..n {
        let c = (0..n)
            .filter(|&j| j != i && base.whole[i].cosine(&base.whole[j]) >= similar)
            .count();
        if c > best.1 {
            best = (i, c);
        }
    }
    let seed = best.0;
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&x, &y| {
        base.whole[seed]
            .cosine(&base.whole[y])
            .total_cmp(&base.whole[seed].cosine(&base.whole[x]))
            .then(x.cmp(&y))
    });
    order.truncate(TIGHT.min(n));
    let index: HashMap<usize, usize> = order.iter().enumerate().map(|(k, &i)| (i, k)).collect();
    let share = |docs: &[usize], cut: f32| -> f64 {
        let (mut above, mut pairs) = (0usize, 0usize);
        for (x, &i) in docs.iter().enumerate() {
            for &j in &docs[x + 1..] {
                pairs += 1;
                above += usize::from(base.whole[i].cosine(&base.whole[j]) >= cut);
            }
        }
        above as f64 / pairs.max(1) as f64
    };
    let all: Vec<usize> = (0..n).collect();
    let about = json!({
        "base": base.name, "documents": order.len(), "seed": base.ids[seed], "seed_neighbours_at_similar_floor": best.1,
        "pairs_at_search_floor": round4(share(&order, search)), "base_pairs_at_search_floor": round4(share(&all, search)),
        "pairs_at_similar_floor": round4(share(&order, similar)), "base_pairs_at_similar_floor": round4(share(&all, similar)),
        "chosen": "the documents nearest (whole-node cosine) to the base document with the most neighbours at or above the similar_to floor",
    });
    let on = base
        .on
        .iter()
        .filter_map(|(id, row, t, exact, kind)| {
            index
                .get(t)
                .map(|&k| (id.clone(), row.clone(), k, *exact, kind.clone()))
        })
        .collect();
    (
        Corpus {
            name: "tight".into(),
            ids: order.iter().map(|&i| base.ids[i].clone()).collect(),
            whole: order.iter().map(|&i| base.whole[i].clone()).collect(),
            rows: order.iter().map(|&i| base.rows[i].clone()).collect(),
            texts: order.iter().map(|&i| base.texts[i].clone()).collect(),
            on,
            documents: order.len(),
            held: 0,
            source: json!({ "from": base.name }),
        },
        about,
    )
}

/// The candidate off-topic sources of a corpus: every other corpus's queries, and the written list.
fn candidates(corpus: &str) -> Vec<&'static str> {
    let own = if corpus == "tight" {
        "nfcorpus"
    } else {
        corpus
    };
    ["scifact", "nfcorpus", "arguana", "fixture", WRITTEN]
        .into_iter()
        .filter(|s| *s != own)
        .collect()
}

fn kept(corpus: &str, source: &str) -> bool {
    DISJOINT
        .iter()
        .any(|(c, s, _)| *c == corpus && *s == source)
}

pub(super) fn run(
    a: &Args,
    cached: &Arc<Cached>,
    key: &ModelKey,
    cache_file: &Path,
    instance_kind: &str,
    deployment: Option<String>,
    backend: Option<&'static str>,
) -> Result<(), String> {
    let floors_json = a.out.join("floors.json");
    let all: Value = serde_json::from_str(
        &fs::read_to_string(&floors_json).map_err(|e| format!("{}: {e}", floors_json.display()))?,
    )
    .map_err(|e| e.to_string())?;
    let floors_run = all["keys"][key.to_string()]["run"]
        .as_str()
        .ok_or(format!("{key} has no floors run in floors.json"))?
        .to_string();
    let floor = umx_core::similarity::floor(key, Use::Search);
    let similar = umx_core::similarity::floor(key, Use::Similar).value;
    if !floor.calibrated() || floor.run != Some(floors_run.as_str()) {
        return Err(format!(
            "{key}: the calibrated floor's run {:?} is not floors.json's {floors_run}",
            floor.run
        ));
    }
    let fl = floor.value;
    let cache_sha =
        sha256_file(cache_file).map_err(|e| format!("{}: {e}", cache_file.display()))?;
    let records = cached.known.lock().unwrap().len();
    let mut offtopic = Value::Null;
    if let Some(out) = &a.offtopic_rows {
        offtopic = offtopic_rows(a, cached, key, out, backend)?;
    }
    let mut reproducibility = Value::Null;
    if a.repeat > 0 {
        if backend.is_none() {
            return Err("--repeat embeds live and is for a local instance only".into());
        }
        reproducibility = repeat(a, cached, key, backend)?;
    }
    cached
        .offline
        .store(true, std::sync::atomic::Ordering::Relaxed);
    eprintln!("strength: {key}, floors run {floors_run}, cache {records} records, sending nothing");
    let kinds: BTreeMap<String, String> = json_lines(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/project/benchmarks/embed-bakeoff/queries.jsonl"),
    )?
    .iter()
    .map(|q| (text(q, "id"), text(q, "kind")))
    .collect();
    // A fresh, unguessable scratch folder, removed however the run ends (security N1).
    let scratch = std::env::temp_dir().join(format!(
        "umx-strength-{}-{:016x}",
        std::process::id(),
        Rng(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64)
        .next()
    ));
    fs::create_dir(&scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
    let _scratch = RemoveOnDrop(scratch.clone());
    let (mut corpora, mut sources) = (Vec::new(), Vec::new());
    for name in ["scifact", "nfcorpus", "arguana", "fixture"] {
        let (c, s) = load(name, a, cached, &scratch, &kinds)?;
        eprintln!(
            "{name}: {} whole-node rows, {} on-topic queries",
            c.whole.len(),
            c.on.len()
        );
        corpora.push(c);
        sources.push(s);
    }
    let _ = fs::remove_dir_all(&scratch);
    let read = cached.read.lock().unwrap().len();
    let written = written(key)?;
    if let Some(w) = written {
        sources.push(w);
    }
    let (t, tight_about) = tight(&corpora[1], similar, fl);
    eprintln!("tight: {tight_about}");
    corpora.push(t);
    // Every record.
    let mut recs: Vec<Rec> = Vec::new();
    for c in &corpora {
        let total = c.whole.len();
        let mut levels: Vec<usize> = LEVELS.iter().map(|&l| l.min(total)).collect();
        levels.dedup();
        let samples: Vec<(usize, Vec<usize>)> = levels
            .iter()
            .map(|&n| (n, sample(total, n, seed_of(&c.name, n, SAMPLE_SEED))))
            .collect();
        let (mut whole, mut best) = (vec![0f32; total], vec![0f32; total]);
        let score = |row: &Row, whole: &mut Vec<f32>, best: &mut Vec<f32>| {
            for i in 0..total {
                whole[i] = row.cosine(&c.whole[i]);
                best[i] = c.rows[i]
                    .iter()
                    .map(|r| row.cosine(r))
                    .fold(whole[i], f32::max);
            }
        };
        let cell = |whole: &[f32], best: &[f32], s: &[usize], target: Option<usize>| {
            let mut ix: Vec<usize> = s.to_vec();
            if let Some(t) = target
                && !ix.contains(&t)
            {
                *ix.last_mut().unwrap() = t;
            }
            let mut pop: Vec<f32> = ix.iter().map(|&i| whole[i]).collect();
            let top = ix
                .iter()
                .filter(|&&i| best[i] >= fl)
                .map(|&i| whole[i])
                .fold(None, |m: Option<f32>, s| Some(m.map_or(s, |m| m.max(s))));
            let p = Population::of(&mut pop).expect("not empty");
            (p, top.map(|t| (t, p.z(t))))
        };
        for (id, row, target, exact, kind) in &c.on {
            score(row, &mut whole, &mut best);
            for (n, s) in &samples {
                let (pop, top) = cell(&whole, &best, s, Some(*target));
                recs.push(Rec {
                    corpus: c.name.clone(),
                    n: *n,
                    on: true,
                    source: c.name.clone(),
                    query: id.clone(),
                    kind: kind.clone(),
                    exact: *exact,
                    score: whole[*target],
                    z: pop.z(whole[*target]),
                    passes_floor: best[*target] >= fl,
                    top,
                    pop,
                });
            }
        }
        for src in sources
            .iter()
            .filter(|s| candidates(&c.name).contains(&s.name.as_str()))
        {
            for (id, _, row) in &src.queries {
                score(row, &mut whole, &mut best);
                for (n, s) in &samples {
                    let (pop, top) = cell(&whole, &best, s, None);
                    let (score, z) = top.unwrap_or((f32::NAN, None));
                    recs.push(Rec {
                        corpus: c.name.clone(),
                        n: *n,
                        on: false,
                        source: src.name.clone(),
                        query: id.clone(),
                        kind: String::new(),
                        exact: false,
                        score,
                        z,
                        passes_floor: top.is_some(),
                        top,
                        pop,
                    });
                }
            }
        }
        eprintln!("{}: cells {levels:?} scored", c.name);
    }
    if let Some(path) = &a.spot {
        return spot(path, &corpora, &sources, fl);
    }
    // The cells the cuts are chosen on: each (corpus, N) with a kept off-topic source.
    let mut cells: Vec<(String, usize)> = Vec::new();
    for r in recs.iter().filter(|r| !r.on && kept(&r.corpus, &r.source)) {
        if !cells.iter().any(|(c, n)| *c == r.corpus && *n == r.n) {
            cells.push((r.corpus.clone(), r.n));
        }
    }
    let off = |c: &str, n: usize| -> Vec<&Rec> {
        recs.iter()
            .filter(|r| !r.on && r.corpus == c && r.n == n && kept(&r.corpus, &r.source))
            .collect()
    };
    // An off-topic query whose top hit is below the floor (or has no z) is never labelled: it counts in the
    // quantiles as the lowest value, so a cell's quantile is over all its off-topic queries.
    let off_tops: Vec<f64> = recs
        .iter()
        .filter(|r| !r.on && kept(&r.corpus, &r.source))
        .map(|r| r.top.map_or(NO_COSINE, |t| f64::from(t.0)))
        .collect();
    let at = |family: Family, level: f64| -> Cut {
        let points: Vec<(f64, f64)> = cells
            .iter()
            .filter_map(|(c, n)| {
                let zs: Vec<f64> = off(c, *n)
                    .iter()
                    .map(|r| r.top.and_then(|t| t.1).unwrap_or(NO_Z))
                    .collect();
                // A cell whose quantile is unlabelled sets no point: nothing there needs a z cut.
                quantile(&zs, level)
                    .filter(|z| *z > NO_Z)
                    .map(|z| ((*n as f64).ln(), z))
            })
            .collect();
        let (za, zb) = envelope(&points);
        // Never below the search floor: a hit below it is never labelled, so a lower cut would say nothing more (and a
        // level reaching the unlabelled queries would give the sentinel).
        let score = up4(quantile(&off_tops, level).unwrap_or(1.0)).max(round4(f64::from(fl)));
        match family {
            Family::Joint => Cut {
                z: up4(za),
                z_per_ln: up4(zb),
                score,
                z_part: true,
            },
            Family::Cosine => Cut {
                z: 0.0,
                z_per_ln: 0.0,
                score,
                z_part: false,
            },
            Family::Z => Cut {
                z: up4(za),
                z_per_ln: up4(zb),
                score: 0.0,
                z_part: true,
            },
        }
    };
    // A label of a (cosine, z) in a population of n: 2 strong, 1 fair, 0 weak.
    let grade = |s: f32, z: Option<f64>, n: usize, strong: &Cut, fair: &Cut| -> u8 {
        if strong.passes(s, z, n) {
            2
        } else if fair.passes(s, z, n) {
            1
        } else {
            0
        }
    };
    let top_grade = |r: &Rec, strong: &Cut, fair: &Cut| -> u8 {
        r.top.map_or(0, |(s, z)| grade(s, z, r.n, strong, fair))
    };
    let worst = |strong: &Cut, fair: &Cut, at_least: u8| -> f64 {
        cells
            .iter()
            .map(|(c, n)| {
                let rs = off(c, *n);
                rs.iter()
                    .filter(|r| top_grade(r, strong, fair) >= at_least)
                    .count() as f64
                    / rs.len().max(1) as f64
            })
            .fold(0.0, f64::max)
    };
    let never = Cut {
        z: f64::INFINITY,
        z_per_ln: 0.0,
        score: f64::INFINITY,
        z_part: true,
    };
    // The loosest level (0.001 steps down from 0.999) at which the off-topic rate holds in every cell; None when even
    // 0.999 fails it (the rate cannot be met, so no cut is recorded for it).
    let loosest = |family: Family, ok: &dyn Fn(&Cut) -> bool| -> Option<(f64, Cut)> {
        let mut best = None;
        let mut level = 999;
        while level >= 300 {
            let l = f64::from(level) / 1000.0;
            let c = at(family, l);
            if ok(&c) {
                best = Some((l, c));
            } else {
                break;
            }
            level -= 1;
        }
        best
    };
    let choose = |family: Family, rate: f64| -> Option<((f64, Cut), (f64, Cut))> {
        let strong = loosest(family, &|c| worst(c, &never, 2) <= rate + 1e-12)?;
        let fair = loosest(family, &|c| worst(&strong.1, c, 1) <= FAIR_RATE + 1e-12)?;
        Some((strong, fair))
    };
    let share = |rs: &[&Rec], f: &dyn Fn(&Rec) -> bool| -> Value {
        json!(round4(
            rs.iter().filter(|r| f(r)).count() as f64 / rs.len().max(1) as f64
        ))
    };
    let on_grade = |r: &Rec, strong: &Cut, fair: &Cut| -> Option<u8> {
        r.passes_floor
            .then(|| grade(r.score, r.z, r.n, strong, fair))
    };
    // search-10's hint: fires when no hit is strong, fair or exact (the shipped rule, [`no_strong_match`] in
    // umx-services), and the strong-only variant.
    let fires = |r: &Rec, strong: &Cut, fair: &Cut, strong_only: bool| -> bool {
        !r.exact && top_grade(r, strong, fair) < if strong_only { 2 } else { 1 }
    };
    let corpus_cells: Vec<(String, usize)> = {
        let mut v: Vec<(String, usize)> = Vec::new();
        for r in &recs {
            if !v.iter().any(|(c, n)| *c == r.corpus && *n == r.n) {
                v.push((r.corpus.clone(), r.n));
            }
        }
        v
    };
    let evaluate = |strong: &Cut, fair: &Cut| -> Value {
        let mut on_cells = serde_json::Map::new();
        for (c, n) in &corpus_cells {
            let rs: Vec<&Rec> = recs
                .iter()
                .filter(|r| r.on && r.corpus == *c && r.n == *n)
                .collect();
            let offs = off(c, *n);
            on_cells.insert(
                format!("{c}@{n}"),
                json!({
                    "on_strong": share(&rs, &|r| on_grade(r, strong, fair) == Some(2)),
                    "on_fair_or_strong": share(&rs, &|r| on_grade(r, strong, fair) >= Some(1)),
                    "off_queries": offs.len(),
                    "off_strong": share(&offs, &|r| top_grade(r, strong, fair) == 2),
                    "off_fair_or_strong": share(&offs, &|r| top_grade(r, strong, fair) >= 1),
                }),
            );
        }
        let mut fixture = serde_json::Map::new();
        for kind in ["paraphrase", "keyword", "near_miss", "tail", "title"] {
            let rs: Vec<&Rec> = recs
                .iter()
                .filter(|r| r.on && r.corpus == "fixture" && r.kind == kind)
                .collect();
            fixture.insert(
                kind.into(),
                json!({
                    "queries": rs.len(),
                    "target_strong": share(&rs, &|r| on_grade(r, strong, fair) == Some(2)),
                    "target_fair_or_strong": share(&rs, &|r| on_grade(r, strong, fair) >= Some(1)),
                    "no_strong_match_fires": share(&rs, &|r| fires(r, strong, fair, false)),
                    "strong_only_rule_fires": share(&rs, &|r| fires(r, strong, fair, true)),
                }),
            );
        }
        for src in candidates("fixture") {
            let rs: Vec<&Rec> = recs
                .iter()
                .filter(|r| !r.on && r.corpus == "fixture" && r.source == src)
                .collect();
            if rs.is_empty() {
                continue;
            }
            fixture.insert(
                format!("off_topic_{src}"),
                json!({
                    "queries": rs.len(), "kept": kept("fixture", src),
                    "top_strong": share(&rs, &|r| top_grade(r, strong, fair) == 2),
                    "top_fair_or_strong": share(&rs, &|r| top_grade(r, strong, fair) >= 1),
                    "no_strong_match_fires": share(&rs, &|r| fires(r, strong, fair, false)),
                    "strong_only_rule_fires": share(&rs, &|r| fires(r, strong, fair, true)),
                }),
            );
        }
        json!({
            "strong": strong.json(), "fair": fair.json(),
            "worst_off_topic_strong": round4(worst(strong, fair, 2)),
            "worst_off_topic_fair_or_strong": round4(worst(strong, fair, 1)),
            "cells": on_cells, "fixture": fixture,
        })
    };
    // The z ablation and the rate tables: each family at 1, 2 and 5 in 100.
    let mut ablation = Vec::new();
    for family in [Family::Joint, Family::Cosine, Family::Z] {
        for rate in [0.01, 0.02, 0.05] {
            let Some(((sl, s), (fl_, f))) = choose(family, rate) else {
                ablation.push(json!({ "family": family.name(), "off_topic_strong_rate": rate, "rate_unmet": true }));
                continue;
            };
            let mut e = evaluate(&s, &f);
            e["family"] = json!(family.name());
            e["off_topic_strong_rate"] = json!(rate);
            e["strong_level"] = json!(sl);
            e["fair_level"] = json!(fl_);
            if family == Family::Joint {
                // Which part of the joint rule rejects each off-topic top hit that passed the floor but is not strong.
                let mut binds = serde_json::Map::new();
                for (c, n) in &cells {
                    let (mut zo, mut so, mut both) = (0, 0, 0);
                    for r in off(c, *n) {
                        let Some((t, z)) = r.top else { continue };
                        let (zf, sf) = (!s.passes_z(z, r.n), f64::from(t) < s.score);
                        match (zf, sf) {
                            (true, false) => zo += 1,
                            (false, true) => so += 1,
                            (true, true) => both += 1,
                            (false, false) => {}
                        }
                    }
                    binds.insert(
                        format!("{c}@{n}"),
                        json!({ "z_only": zo, "cosine_only": so, "both": both }),
                    );
                }
                e["rejected_by"] = Value::Object(binds);
            }
            ablation.push(e);
        }
    }
    let run_id = format!(
        "sc-{}-{}",
        today(),
        a.commit.chars().take(7).collect::<String>()
    );
    let shipped = if a.cosine_only {
        Family::Cosine
    } else {
        Family::Joint
    };
    let ((strong_level, sc), (fair_level, fc)) = choose(shipped, a.strong_rate).ok_or(format!(
        "{key}: no {} cut keeps the off-topic rates ({} strong, {FAIR_RATE} fair or strong) in every cell, even at the \
         0.999 level: nothing is recorded",
        shipped.name(),
        a.strong_rate
    ))?;
    // The cosine-only rule's strong cut at the same rate, for the z settling table (embeddings-expert).
    let cos_cut = choose(Family::Cosine, a.strong_rate)
        .map(|((_, c), _)| c)
        .ok_or("no cosine-only cut meets the strong rate")?;
    // The largest population measured (D4): labels stop above twice it, until a larger corpus is measured.
    let max_n_measured = cells.iter().map(|(_, n)| *n).max().unwrap_or(0);
    let mut cuts = StrengthCuts {
        strong_z: sc.z as f32,
        strong_z_per_ln: sc.z_per_ln as f32,
        strong_score: sc.score as f32,
        fair_z: fc.z as f32,
        fair_z_per_ln: fc.z_per_ln as f32,
        fair_score: fc.score as f32,
        min_population: 0,
        z_part: sc.z_part,
        max_n_measured,
        run: Box::leak(run_id.clone().into_boxed_str()),
    };
    let min = if cuts.z_part {
        min_population(&corpora, &sources, &cuts)
    } else {
        json!({ "min_population": 0, "why": "the cosine-only rule needs no population statistics" })
    };
    cuts.min_population = min["min_population"].as_u64().unwrap() as usize;
    // Check: umx_core's own rule gives the tool's labels (the shipped cuts, no population minimum).
    let core = StrengthCuts {
        min_population: 0,
        ..cuts
    };
    for r in recs.iter().filter(|r| r.on && r.passes_floor) {
        let want = match grade(r.score, r.z, r.n, &sc, &fc) {
            2 => Strength::Strong,
            1 => Strength::Fair,
            _ => Strength::Weak,
        };
        let got = strength(&r.pop, &core, r.score);
        if got.is_some() && got != Some(want) {
            return Err(format!(
                "umx_core labels {} {:?}, the tool {want:?}",
                r.query, got
            ));
        }
    }
    let mut pairs = serde_json::Map::new();
    for c in &corpora {
        for src in candidates(&c.name) {
            for (cn, n) in corpus_cells.iter().filter(|(cn, _)| *cn == c.name) {
                let rs: Vec<&Rec> = recs
                    .iter()
                    .filter(|r| !r.on && r.corpus == *cn && r.n == *n && r.source == src)
                    .collect();
                if rs.is_empty() {
                    continue;
                }
                let zs: Vec<f64> = rs.iter().filter_map(|r| r.top.and_then(|t| t.1)).collect();
                let ss: Vec<f64> = rs
                    .iter()
                    .filter_map(|r| r.top.map(|t| f64::from(t.0)))
                    .collect();
                pairs.insert(
                    format!("{cn}<-{src}@{n}"),
                    json!({
                        "kept": kept(cn, src), "queries": rs.len(),
                        "strong": share(&rs, &|r| top_grade(r, &sc, &fc) == 2),
                        "fair_or_strong": share(&rs, &|r| top_grade(r, &sc, &fc) >= 1),
                        "passed_floor": share(&rs, &|r| r.top.is_some()),
                        "top_z": { "p50": q(&zs, 0.5), "p99": q(&zs, 0.99) },
                        "top_cosine": { "p50": q(&ss, 0.5), "p99": q(&ss, 0.99) },
                    }),
                );
            }
        }
    }
    let disjoint: Vec<Value> = DISJOINT
        .iter()
        .map(|(c, s, why)| json!({ "corpus": c, "queries": s, "spot_check": why }))
        .collect();
    let dropped: Vec<Value> = OVERLAPPING
        .iter()
        .map(|(c, s, why)| json!({ "corpus": c, "queries": s, "spot_check": why }))
        .collect();
    if let Some(dir) = &a.raw {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("strength-{key}.jsonl"));
        let lines: Vec<String> = recs
            .iter()
            .map(|r| {
                json!({
                    "corpus": r.corpus, "n": r.n, "on_topic": r.on, "source": r.source, "query": r.query,
                    "kind": r.kind, "exact": r.exact, "score": round4(f64::from(r.score)), "z": r.z.map(round4),
                    "passes_floor": r.passes_floor, "top": r.top.map(|t| round4(f64::from(t.0))),
                    "top_z": r.top.and_then(|t| t.1).map(round4), "kept": kept(&r.corpus, &r.source),
                })
                .to_string()
            })
            .collect();
        fs::write(&path, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
        eprintln!("strength: raw records in {}", path.display());
    }
    let corpora_json: serde_json::Map<String, Value> = corpora
        .iter()
        .map(|c| {
            (
                c.name.clone(),
                json!({
                    "source": c.source, "documents": c.documents, "held": c.held, "whole_node_rows": c.whole.len(),
                    "on_topic_queries": c.on.len(),
                    "off_topic_kept": candidates(&c.name).into_iter().filter(|s| kept(&c.name, s)).collect::<Vec<_>>(),
                }),
            )
        })
        .collect();
    let shipped_eval = evaluate(&sc, &fc);
    let strength_json = json!({
        "run": run_id,
        "run_date": today(),
        "umx_commit": a.commit,
        "tool_at": "the PR head that adds or changes this tool (its commit is recorded after merge); umx_commit is the base it was built on, which does not hold the tool",
        "floors_run": floors_run,
        "tool": "umx-bench floor-calibration --strength (crates/umx-bench/src/bin/floor-calibration/strength.rs)",
        "command": format!(
            "floor-calibration --strength --embedder {} --cache <cache> --out docs/project/benchmarks/floor-calibration --commit {}{}{}",
            a.embedder, a.commit,
            if a.strong_rate == 0.01 { String::new() } else { format!(" --strong-rate {}", a.strong_rate) },
            if a.cosine_only { " --cosine-only" } else { "" },
        ),
        "instance_kind": instance_kind,
        "deployment": deployment,
        // Which deployment each set of rows came from: the floors run's for the BEIR and fixture rows (its cache),
        // and the written off-topic rows' own (their provenance note).
        "rows_from": {
            "beir_and_fixture": format!(
                "the floors run {floors_run} ({}), deployment {}",
                all["keys"][key.to_string()]["run_date"].as_str().unwrap_or(""),
                all["keys"][key.to_string()]["deployment"].as_str().unwrap_or("none (local)")
            ),
            "written_off_topic": "docs/project/benchmarks/embed-bakeoff/vectors/<model>_<dims>[+p<n>].offtopic.rows.provenance.md",
        },
        "cache": {
            "file": "<cache>/floor-calibration/<key>.vec",
            "sha256": cache_sha,
            "records": records,
            "records_read": read,
            "offline": true,
            "texts_sent": cached.sent.lock().unwrap().0,
            "regenerate": "floor-calibration --embedder <instance> --cache <cache> --out <dir> --commit <sha> (the floors run fills the cache); without the cache this tool cannot run again, and the committed cuts stay as they are",
        },
        "corpora": corpora_json,
        "tight": tight_about,
        "off_topic_pairs_kept": disjoint,
        "off_topic_pairs_dropped": dropped,
        "spot_check": "each candidate pair's 20 highest off-topic top hits read side by side (`--spot <file>`, a scratch file never committed: it holds corpus text); kept only when none is on topic",
        "written_off_topic": sources.iter().any(|s| s.name == WRITTEN),
        "written_off_topic_rows": written_rows(key),
        "sample_seed": SAMPLE_SEED,
        "bootstrap_seed": BOOT_SEED,
        "levels": LEVELS,
        "rule": "strong: z >= strong_z + strong_z_per_ln * ln N (when z_part) AND cosine >= strong_score; fair: the same with the fair cuts; else weak. z = (cosine - median) / MAD over every searched node's whole-node cosine. The cuts at a quantile level L of the kept off-topic top hits (the best whole-node cosine among nodes past the floor): the z line a least-squares fit on ln N through each (corpus, N) cell's L-quantile, raised over every cell, and the pooled cosine L-quantile; strong the loosest L (0.001 steps from 0.999) at which no cell's off-topic top hits are strong more than strong_rate, fair the loosest at which none are fair or strong more than 1 in 10. MIN_POPULATION: the smallest N from which every larger rung's p90 bootstrap 90% half-width of z is at most half the gap between the strong and fair z cuts at that N.",
        "strong_rate": a.strong_rate,
        "fair_rate": FAIR_RATE,
        "strong_level": strong_level,
        "fair_level": fair_level,
        "search_floor": round4(f64::from(fl)),
        // The cuts as measured (4 decimals, rounded up); CALIBRATED holds each as the nearest f32.
        "cuts": {
            "strong_z": sc.z, "strong_z_per_ln": sc.z_per_ln, "strong_score": sc.score,
            "fair_z": fc.z, "fair_z_per_ln": fc.z_per_ln, "fair_score": fc.score,
            "min_population": cuts.min_population, "z_part": cuts.z_part, "max_n_measured": max_n_measured,
        },
        "max_n_note": "labels stop (null, and the space does not count toward no_strong_match) when more than 2 x max_n_measured nodes are searched in the space, until a larger corpus is measured (follow-up: a 57k corpus such as FiQA sampled at 20k and 50k)",
        "z_settling": settling(&corpora, &sc, &cos_cut, fl),
        "shipped": shipped_eval,
        // The ablation at the stated rate only; every rate and the per-pair table go to `--raw` (policy file).
        "ablation": ablation.iter().filter(|e| e["off_topic_strong_rate"] == json!(0.01)).cloned().collect::<Vec<_>>(),
        "min_population": min,
        "reproducibility": reproducibility,
        "offtopic_rows": offtopic,
    });
    if let Some(dir) = &a.raw {
        let path = dir.join(format!("policy-{key}.json"));
        let tables = json!({ "pairs": pairs, "ablation": ablation,
            "shipped": strength_json["shipped"], "tight": strength_json["tight"], "cuts": strength_json["cuts"],
            "min_population": strength_json["min_population"] });
        fs::write(&path, serde_json::to_string_pretty(&tables).unwrap() + "\n")
            .map_err(|e| e.to_string())?;
        eprintln!("strength: tables in {}", path.display());
    }
    write_strength(&a.out, &key.to_string(), strength_json)
}

/// The embeddings expert's z settling measurement: for each on-topic query of the tight graph (N 300 and 1,000),
/// scifact (N 300) and the s03 fixture, how many hits (nodes past the floor in the population) are strong under the
/// joint rule and under the cosine-only rule at the same rate; the median and p90 per cell.
fn settling(corpora: &[Corpus], joint: &Cut, cosine: &Cut, floor: f32) -> Value {
    let mut out = serde_json::Map::new();
    for (name, want) in [
        ("tight", 300),
        ("tight", 1000),
        ("scifact", 300),
        ("fixture", 300),
    ] {
        let Some(c) = corpora.iter().find(|c| c.name == name) else {
            continue;
        };
        let total = c.whole.len();
        let n = want.min(total);
        let s = sample(total, n, seed_of(&c.name, n, SAMPLE_SEED));
        let (mut j, mut k) = (Vec::new(), Vec::new());
        for (_, row, target, _, _) in &c.on {
            let mut ix = s.clone();
            if !ix.contains(target) {
                *ix.last_mut().unwrap() = *target;
            }
            let whole: Vec<f32> = ix.iter().map(|&i| row.cosine(&c.whole[i])).collect();
            let past: Vec<bool> = ix
                .iter()
                .zip(&whole)
                .map(|(&i, &w)| c.rows[i].iter().map(|r| row.cosine(r)).fold(w, f32::max) >= floor)
                .collect();
            let p = Population::of(&mut whole.clone()).expect("not empty");
            let count = |cut: &Cut| {
                whole
                    .iter()
                    .zip(&past)
                    .filter(|(w, ok)| **ok && cut.passes(**w, p.z(**w), n))
                    .count() as f64
            };
            j.push(count(joint));
            k.push(count(cosine));
        }
        let stats = |v: &[f64]| json!({ "p50": q(v, 0.5), "p90": q(v, 0.9) });
        out.insert(
            format!("{name}@{n}"),
            json!({ "queries": c.on.len(), "joint": stats(&j), "cosine_only": stats(&k) }),
        );
    }
    out.insert("caveat".into(), json!(SETTLING_CAVEAT));
    Value::Object(out)
}

/// What the z settling table shows and does not (the prime's ruling to keep z).
const SETTLING_CAVEAT: &str = "the joint rule's median is 0 in these cells too, because strong is rare by design at the \
1-in-100 off-topic rate; the table shows z prevents a flood of strong hits in tight graphs, not that z makes strong \
common; cosine-only has higher on-topic strong rates and floods tight graphs";

/// `--spot <file>`: for every candidate off-topic pair, its [`SPOT`] highest off-topic top hits at the corpus's full
/// size, the query and the top document side by side, to judge whether the domains are disjoint. Written to a scratch
/// file for reading, never committed (it holds corpus text).
fn spot(path: &Path, corpora: &[Corpus], sources: &[Source], floor: f32) -> Result<(), String> {
    let mut out = String::new();
    for c in corpora {
        for src in sources
            .iter()
            .filter(|s| candidates(&c.name).contains(&s.name.as_str()))
        {
            let mut tops: Vec<(f32, usize, &str)> = Vec::new();
            for (_, text, row) in &src.queries {
                let best = (0..c.whole.len())
                    .filter(|&i| {
                        c.rows[i]
                            .iter()
                            .map(|r| row.cosine(r))
                            .fold(row.cosine(&c.whole[i]), f32::max)
                            >= floor
                    })
                    .map(|i| (row.cosine(&c.whole[i]), i))
                    .max_by(|x, y| x.0.total_cmp(&y.0).then(y.1.cmp(&x.1)));
                if let Some((s, i)) = best {
                    tops.push((s, i, text.as_str()));
                }
            }
            tops.sort_by(|x, y| y.0.total_cmp(&x.0));
            out += &format!(
                "\n=== corpus {} <- queries {} ({} of {} queries have a hit past the floor)\n",
                c.name,
                src.name,
                tops.len(),
                src.queries.len()
            );
            for (s, i, t) in tops.iter().take(SPOT) {
                let qt: String = t.chars().take(140).collect();
                out += &format!(
                    "{s:.4} | Q: {qt}\n       | D {}: {}\n",
                    c.ids[*i], c.texts[*i]
                );
            }
        }
    }
    fs::write(path, out).map_err(|e| e.to_string())?;
    eprintln!("strength: spot check in {}", path.display());
    Ok(())
}

/// The smallest rung of [`LADDER`] from which every larger rung's z moves (p90 of the bootstrap 90% half-widths) less
/// than half the gap between the fair and strong z cuts.
fn min_population(corpora: &[Corpus], sources: &[Source], cuts: &StrengthCuts) -> Value {
    let cuts = StrengthCuts {
        min_population: 0,
        ..*cuts
    };
    let mut rungs = Vec::new();
    for &n in &LADDER {
        let (mut flips, mut labels) = (0usize, 0usize);
        let mut halfwidths = Vec::new();
        for c in corpora {
            let total = c.whole.len();
            if n > total {
                continue;
            }
            let s = sample(total, n, seed_of(&c.name, n, SAMPLE_SEED));
            let mut rng = Rng(seed_of(&c.name, n, BOOT_SEED));
            let mut hits: Vec<(Vec<f32>, f32)> = Vec::new();
            for (_, row, target, _, _) in c.on.iter().take(BOOT_QUERIES) {
                let mut pop: Vec<f32> = s.iter().map(|&i| row.cosine(&c.whole[i])).collect();
                if !s.contains(target) {
                    *pop.last_mut().unwrap() = row.cosine(&c.whole[*target]);
                }
                hits.push((pop, row.cosine(&c.whole[*target])));
            }
            for src in sources.iter().filter(|x| kept(&c.name, &x.name)) {
                for (_, _, row) in src.queries.iter().take(BOOT_QUERIES / 2) {
                    let pop: Vec<f32> = s.iter().map(|&i| row.cosine(&c.whole[i])).collect();
                    let top = pop.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                    hits.push((pop, top));
                }
            }
            for (pop, st) in hits {
                let base = Population::of(&mut pop.clone()).unwrap();
                let want = strength(&base, &cuts, st);
                let mut zs = Vec::new();
                for _ in 0..BOOT {
                    let mut re: Vec<f32> = (0..n).map(|_| pop[rng.below(n)]).collect();
                    let p = Population::of(&mut re).unwrap();
                    labels += 1;
                    flips += usize::from(strength(&p, &cuts, st) != want);
                    if let Some(z) = p.z(st) {
                        zs.push(z);
                    }
                }
                if let (Some(lo), Some(hi)) = (quantile(&zs, 0.05), quantile(&zs, 0.95)) {
                    halfwidths.push((hi - lo) / 2.0);
                }
            }
        }
        let half_gap = (cuts.strong_z_at(n) - cuts.fair_z_at(n)) / 2.0;
        rungs.push(json!({
            "n": n, "labels": labels, "flip_rate": round4(flips as f64 / labels.max(1) as f64),
            "z_halfwidth_p50": q(&halfwidths, 0.5), "z_halfwidth_p90": q(&halfwidths, 0.9),
            "half_gap": round4(half_gap),
        }));
    }
    let mut min = *LADDER.last().unwrap();
    for r in rungs.iter().rev() {
        if r["z_halfwidth_p90"]
            .as_f64()
            .is_some_and(|w| w <= r["half_gap"].as_f64().unwrap())
        {
            min = r["n"].as_u64().unwrap() as usize;
        } else {
            break;
        }
    }
    json!({ "min_population": min, "resamples": BOOT, "queries_per_corpus": BOOT_QUERIES, "ladder": rungs })
}

/// The int8 reproducibility count (embeddings-expert Q4/Q6): the first [`REPEAT_QUERIES`] fixture queries embedded
/// `--repeat` times live through the instance, each run's int8 rows against the first run's and against the
/// committed recorded rows. With `--offtopic-rows`, the fixture's off-topic queries' rows are written too.
fn repeat(
    a: &Args,
    cached: &Arc<Cached>,
    key: &ModelKey,
    backend: Option<&'static str>,
) -> Result<Value, String> {
    let dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/project/benchmarks/embed-bakeoff");
    let texts: Vec<String> = json_lines(&dir.join("queries.jsonl"))?
        .iter()
        .take(REPEAT_QUERIES)
        .map(|q| text(q, "text"))
        .collect();
    let recorded: HashMap<String, Vec<u8>> =
        fs::read_to_string(dir.join(format!("vectors/{key}.rows.tsv")))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut p = l.split('\t');
                (p.next()? == "query").then_some(())?;
                let h = p.next()?.to_string();
                Some((h, umx_core::vectors::base64_decode(p.next()?)?))
            })
            .collect();
    // The fixture's recorded document rows: how far a changed query row moves a cosine against them.
    let docs: Vec<Row> = fs::read_to_string(dir.join(format!("vectors/{key}.rows.tsv")))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut p = l.split('\t');
            (p.next()? == "document").then_some(())?;
            let _ = p.next()?;
            let v: Vec<i8> = umx_core::vectors::base64_decode(p.next()?)?
                .into_iter()
                .map(|b| b as i8)
                .collect();
            let inv = inverse_length(&v);
            Some(Row { v, inv })
        })
        .collect();
    let inner = &cached.inner;
    let mut first: Vec<Vec<i8>> = Vec::new();
    let (mut rows_differ, mut components_differ) = (0usize, 0usize);
    let (mut rec_rows, mut rec_components, mut rec_compared) = (0usize, 0usize, 0usize);
    let (mut rec_step, mut rec_cosine) = (0i32, 0f32);
    for run in 0..a.repeat {
        let got = inner
            .embed(&texts, TextKind::Query)
            .map_err(|e| format!("repeat: {e:?}"))?;
        let rows: Vec<Vec<i8>> = got.vectors.iter().map(|v| quantise(v)).collect();
        if run == 0 {
            for (t, r) in texts.iter().zip(&rows) {
                if let Some(want) = recorded.get(&TextHash::of(t).to_string()) {
                    rec_compared += 1;
                    let d = want.iter().zip(r).filter(|(w, x)| **w as i8 != **x).count();
                    rec_components += d;
                    rec_rows += usize::from(d > 0);
                    for (w, x) in want.iter().zip(r) {
                        rec_step = rec_step.max((i32::from(*w as i8) - i32::from(*x)).abs());
                    }
                    let old: Vec<i8> = want.iter().map(|&b| b as i8).collect();
                    let (old, new) = (
                        Row {
                            inv: inverse_length(&old),
                            v: old,
                        },
                        Row {
                            inv: inverse_length(r),
                            v: r.clone(),
                        },
                    );
                    for doc in &docs {
                        rec_cosine = rec_cosine.max((old.cosine(doc) - new.cosine(doc)).abs());
                    }
                }
            }
            first = rows;
        } else {
            for (f, r) in first.iter().zip(&rows) {
                let d = f.iter().zip(r).filter(|(x, y)| x != y).count();
                components_differ += d;
                rows_differ += usize::from(d > 0);
            }
        }
    }
    Ok(json!({
        "backend": backend, "queries": texts.len(), "runs": a.repeat,
        "rows_compared": texts.len() * a.repeat.saturating_sub(1), "rows_differing": rows_differ,
        "int8_components_differing": components_differ,
        "against_recorded": { "rows_compared": rec_compared, "rows_differing": rec_rows,
            "int8_components_differing": rec_components, "largest_int8_step": rec_step,
            "largest_cosine_change": (f64::from(rec_cosine) * 1e6).ceil() / 1e6,
            "documents": docs.len(),
            "note": "the committed query rows were recorded on 2026-11-11 by another umx build on the same machine and backend" },
        "hosted": "not measured",
    }))
}

/// `--offtopic-rows <file>`: the written off-topic list (embed-bakeoff/offtopic.jsonl, committed synthetic text) embedded
/// once through the instance as umx search embeds a query, int8 rows written to `out`. A hosted instance is sent them
/// only with `--send-offtopic`, and only these texts: the one call takes exactly the file's lines, and the offline
/// guard stays on for everything else (the cache is never asked to send).
fn offtopic_rows(
    a: &Args,
    cached: &Arc<Cached>,
    key: &ModelKey,
    out: &Path,
    backend: Option<&'static str>,
) -> Result<Value, String> {
    if backend.is_none() && !a.send_offtopic {
        return Err(format!(
            "{key} is hosted: --offtopic-rows sends the written off-topic list, so it needs --send-offtopic"
        ));
    }
    let dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/project/benchmarks/embed-bakeoff");
    // Pinned before any send (security): the committed list, byte for byte, and its 45 texts only.
    let file = dir.join("offtopic.jsonl");
    let sha = sha256_file(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    if sha != OFFTOPIC_SHA256 {
        return Err(format!(
            "offtopic.jsonl is not the committed list (sha256 {sha}); nothing sent"
        ));
    }
    let off: Vec<String> = json_lines(&file)?.iter().map(|q| text(q, "text")).collect();
    if off.len() != OFFTOPIC_TEXTS || off.iter().any(|t| t.is_empty()) {
        return Err(format!(
            "offtopic.jsonl holds {} texts, not {OFFTOPIC_TEXTS}; nothing sent",
            off.len()
        ));
    }
    let tokens: usize = off.iter().map(|t| estimate_tokens(t)).sum();
    let got = cached
        .inner
        .embed(&off, TextKind::Query)
        .map_err(|e| format!("off-topic rows: {e:?}"))?;
    if got.vectors.len() != off.len() {
        return Err(format!(
            "{} vectors for {} texts",
            got.vectors.len(),
            off.len()
        ));
    }
    let mut lines: Vec<String> = off
        .iter()
        .zip(&got.vectors)
        .map(|(t, v)| {
            let row: Vec<u8> = quantise(v).into_iter().map(|x| x as u8).collect();
            format!(
                "query\t{}\t{}",
                TextHash::of(t),
                umx_core::vectors::base64_encode(&row)
            )
        })
        .collect();
    lines.sort();
    let body = lines.join("\n") + "\n";
    fs::write(out, &body).map_err(|e| e.to_string())?;
    eprintln!(
        "strength: {key}: {} off-topic texts sent in one call (about {tokens} tokens); rows written",
        off.len()
    );
    Ok(json!({
        "texts_sent": off.len(), "estimated_tokens_sent": tokens, "calls": 1,
        "rows_sha256": umx_core::models::sha256_hex(body.as_bytes()),
        "instance_kind": if backend.is_some() { "local" } else { "hosted" },
    }))
}

/// Merges `strength` into `<out>/floors.json` under `key`, beside its floors, and renders RESULT.md again.
fn write_strength(out: &Path, key: &str, mut strength: Value) -> Result<(), String> {
    let path = out.join("floors.json");
    let mut all: Value =
        serde_json::from_str(&fs::read_to_string(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    // A key's deployment check outlives a rerun of its measurement; while it is failed, the cuts are withheld: kept
    // as `cuts_measured` for the record, `cuts` null, so the key labels nothing (embeddings expert's rule).
    let old = &all["keys"][key]["strength"]["deployment_check"];
    if strength["deployment_check"].is_null() && !old.is_null() {
        strength["deployment_check"] = old.clone();
    }
    withhold_if_failed(&mut strength);
    all["keys"][key]["strength"] = strength;
    let body = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())? + "\n";
    fs::write(&path, body).map_err(|e| e.to_string())?;
    let result = out.join("RESULT.md");
    let notes = fs::read_to_string(&result)
        .ok()
        .and_then(|t| t.find(NOTES).map(|i| t[i..].to_string()))
        .unwrap_or_else(|| format!("{NOTES}\n"));
    fs::write(&result, super::render(&all) + "\n" + &notes).map_err(|e| e.to_string())?;
    eprintln!("strength: wrote {} and RESULT.md", path.display());
    Ok(())
}

/// The deployment check's rule (embeddings expert, 2026-11-20): the cosine bar alone. Hosted services are
/// nondeterministic on GPUs, so int8 rows may differ by a few steps while the cosine stays at 0.9997 or above; a model
/// change moves every text to about 0.9 to 0.99.
const CHECK_RULE: &str = "pass when every cosine >= 0.999 (embeddings expert, 2026-11-20: the cosine bar alone; hosted \
int8 rows may differ by a few steps from GPU nondeterminism)";

fn check_passes(min_cosine: f64) -> bool {
    min_cosine >= 0.999
}

/// A record whose deployment check failed keeps its cuts only as `cuts_measured`: `cuts` null, so the key labels
/// nothing. A passed (or absent) check leaves the record as it is.
fn withhold_if_failed(strength: &mut Value) {
    let min = strength["deployment_check"]["min_cosine"].as_f64();
    if min.is_some_and(|m| !check_passes(m)) && !strength["cuts"].is_null() {
        strength["deployment_check"]["pass"] = json!(false);
        strength["cuts_measured"] = strength["cuts"].take();
        strength["withheld"] = json!(WITHHELD);
    }
}

/// Why a key's cuts are withheld after a failed deployment check.
const WITHHELD: &str = "the deployment check failed: today's deployment gives vectors with a cosine below 0.999 to \
the rows the cuts were measured on, so the cuts are not used (labels null, no no_strong_match) until the key is measured \
again on one deployment";

/// `--render`: RESULT.md again from floors.json, measuring and sending nothing.
pub(super) fn render_only(out: &Path) -> Result<(), String> {
    let all: Value = serde_json::from_str(
        &fs::read_to_string(out.join("floors.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let result = out.join("RESULT.md");
    let notes = fs::read_to_string(&result)
        .ok()
        .and_then(|t| t.find(NOTES).map(|i| t[i..].to_string()))
        .unwrap_or_else(|| format!("{NOTES}\n"));
    fs::write(&result, super::render(&all) + "\n" + &notes).map_err(|e| e.to_string())
}

/// The most passes `--deployment-check --repeat` sends (each resends the pinned texts to the instance).
pub(super) const MAX_CHECK_PASSES: usize = 3;

/// Texts of each kind the deployment check sends.
const DRIFT_TEXTS: usize = 10;
/// The committed written off-topic list: its sha256 and its texts, the only thing `--send-offtopic` sends.
const OFFTOPIC_SHA256: &str = "cc99b6ad389f8fb8737f2aa96b9790727ca4b37ffc1c14b0c4ebb2119e9a3a41";
const OFFTOPIC_TEXTS: usize = 45;
/// The s03 fixture files the deployment check reads (sha256), and the ids of the texts it sends: their only source.
const NODES_SHA256: &str = "34f651be773d7fe4929ae7c3cc056a663ca4891c9c30a8bc7877d5989ac33200";
const QUERIES_SHA256: &str = "598d9dab900307bba1af59b0e7100fa41f30797fb76f6b3f0a9c4aea5d107a87";
const DRIFT_NODES: [&str; DRIFT_TEXTS] = [
    "reg-001", "reg-002", "reg-003", "reg-004", "reg-005", "reg-006", "reg-007", "reg-008",
    "reg-009", "reg-010",
];
const DRIFT_QUERIES: [&str; DRIFT_TEXTS] = [
    "q-001", "q-002", "q-003", "q-004", "q-005", "q-006", "q-007", "q-008", "q-009", "q-010",
];

/// `--deployment-check` (embeddings expert, on s8's hosted keys): the floors run embedded through deployments that are
/// gone, so the first [`DRIFT_TEXTS`] s03 fixture queries and the whole-node texts of its first [`DRIFT_TEXTS`] nodes
/// (synthetic text only, never BEIR) are embedded once through the instance now and compared with the vectors that
/// run cached for the same texts: per text the cosine of the float vectors and of the int8 rows, how many int8
/// components differ and by at most how many steps. Pass: every cosine at least 0.999 (the int8 step count is
/// reported, not a criterion). Merged into the key's strength record as `deployment_check`; the written off-topic
/// rows' own provenance is docs/project/benchmarks/embed-bakeoff/vectors/<model>_<dims>.offtopic.rows.provenance.md.
pub(super) fn deployment_check(
    a: &Args,
    cached: &Arc<Cached>,
    key: &ModelKey,
    deployment: Option<String>,
) -> Result<(), String> {
    cached
        .offline
        .store(true, std::sync::atomic::Ordering::Relaxed);
    // Pinned before any send (security): the committed fixture files and the listed texts only.
    let dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/project/benchmarks/embed-bakeoff");
    for (name, want) in [
        ("nodes.jsonl", NODES_SHA256),
        ("queries.jsonl", QUERIES_SHA256),
    ] {
        let got = sha256_file(&dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
        if got != want {
            return Err(format!(
                "{name} is not the committed fixture (sha256 {got}); nothing sent"
            ));
        }
    }
    let mut task = fixture()?;
    task.docs
        .retain(|(id, _, _)| DRIFT_NODES.contains(&id.as_str()));
    task.queries
        .retain(|(id, _)| DRIFT_QUERIES.contains(&id.as_str()));
    if task.docs.len() != DRIFT_TEXTS || task.queries.len() != DRIFT_TEXTS {
        return Err("the pinned fixture ids are not all there; nothing sent".into());
    }
    let scratch = std::env::temp_dir().join(format!(
        "umx-drift-{}-{:016x}",
        std::process::id(),
        Rng(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64)
        .next()
    ));
    fs::create_dir(&scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
    let _scratch = RemoveOnDrop(scratch.clone());
    let root = scratch.join("graphs");
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let config = Config::at(scratch.join("home"), None).roots([root.to_str().unwrap()]);
    let mut svc = compose(Mode::Real, &config).with_embedder(cached.clone() as Arc<dyn Embedder>);
    svc.graph_new(
        &ctx("drift"),
        root.join("drift").to_str().unwrap(),
        "drift",
        None,
        false,
        None,
    )
    .map_err(|p| format!("graph new: {p:?}"))?;
    // The whole-node texts exactly as the floors run built them: the same nodes through umx's own planner.
    for (id, title, body) in task.docs.iter().take(DRIFT_TEXTS) {
        svc.node_add(
            &ctx("drift"),
            NodeAddRequest {
                kind: "concept".into(),
                title: title_of(title, body),
                note: Some(body.clone()),
                ..Default::default()
            },
        )
        .map_err(|p| format!("node {id}: {p:?}"))?;
    }
    let whole = whole_node_field();
    let docs: Vec<String> = svc
        .take_embed_jobs()
        .iter()
        .filter_map(|j| svc.embed_plan(j).ok().flatten())
        .flat_map(|p| p.texts)
        .filter(|w| w.field == whole)
        .map(|w| w.text)
        .collect();
    let queries: Vec<String> = task
        .queries
        .iter()
        .take(DRIFT_TEXTS)
        .map(|(_, q)| q.clone())
        .collect();
    // Only s03 fixture texts are sent, and only ones the floors run cached, so each has its old vector.
    assert!(docs.len() == DRIFT_TEXTS && queries.len() == DRIFT_TEXTS);
    let old = |t: &str, kind: TextKind| -> Result<Vec<f32>, String> {
        cached
            .known
            .lock()
            .unwrap()
            .get(&(kind_byte(kind), sha256_bytes(t.as_bytes())))
            .cloned()
            .ok_or_else(|| {
                format!("a {kind:?} text is not in the floors cache: not the floors run's text")
            })
    };
    let tokens: usize = docs
        .iter()
        .chain(&queries)
        .map(|t| estimate_tokens(t))
        .sum();
    // The texts in order (queries, then whole-node texts), their labels and old vectors.
    let mut labels = Vec::new();
    let mut olds: Vec<Vec<f32>> = Vec::new();
    for (kind, texts, label) in [
        (TextKind::Query, &queries, "query"),
        (TextKind::Document, &docs, "node"),
    ] {
        for (i, t) in texts.iter().enumerate() {
            labels.push(format!("{label}-{:02}", i + 1));
            olds.push(old(t, kind)?);
        }
    }
    // Each pass sends the same pinned texts once: one Query call, one Document call (`--repeat N` passes, default 1).
    let passes = a.repeat.max(1);
    // Every pass resends the pinned texts: refused above the cap before anything is sent (args checks it first).
    if passes > MAX_CHECK_PASSES {
        return Err(format!(
            "--repeat {passes}: --deployment-check sends at most {MAX_CHECK_PASSES} passes"
        ));
    }
    let mut runs: Vec<Vec<Vec<f32>>> = Vec::new();
    for _ in 0..passes {
        let mut run = Vec::new();
        for (kind, texts) in [(TextKind::Query, &queries), (TextKind::Document, &docs)] {
            let got = cached
                .inner
                .embed(texts, kind)
                .map_err(|e| format!("deployment check: {e:?}"))?;
            if got.vectors.len() != texts.len() {
                return Err("the instance returned another number of vectors".into());
            }
            run.extend(got.vectors);
        }
        runs.push(run);
    }
    let compare = |o: &[f32], n: &[f32]| -> Value {
        let dotf: f64 = o
            .iter()
            .zip(n)
            .map(|(x, y)| f64::from(*x) * f64::from(*y))
            .sum();
        let norm = |v: &[f32]| v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
        let (ro, rn) = (Row::of(o), Row::of(n));
        let differ = ro.v.iter().zip(&rn.v).filter(|(x, y)| x != y).count();
        let step =
            ro.v.iter()
                .zip(&rn.v)
                .map(|(x, y)| (i32::from(*x) - i32::from(*y)).abs())
                .max()
                .unwrap_or(0);
        json!({
            "cosine": (dotf / (norm(o) * norm(n)) * 1e6).floor() / 1e6,
            "int8_cosine": (f64::from(ro.cosine(&rn)) * 1e6).floor() / 1e6,
            "components_differing": differ, "max_int8_step": step,
        })
    };
    let summary = |rows: &[Value]| -> (f64, i64) {
        (
            rows.iter()
                .map(|r| r["cosine"].as_f64().unwrap())
                .fold(1.0, f64::min),
            rows.iter()
                .map(|r| r["max_int8_step"].as_i64().unwrap())
                .max()
                .unwrap_or(0),
        )
    };
    let path = a.out.join("floors.json");
    let all: Value = serde_json::from_str(&fs::read_to_string(&path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut st = all["keys"][key.to_string()]["strength"].clone();
    if st.is_null() {
        return Err(format!("{key} has no strength record to add the check to"));
    }
    let sent = labels.len() * passes;
    if passes == 1 {
        let rows: Vec<Value> = labels
            .iter()
            .zip(olds.iter().zip(&runs[0]))
            .map(|(l, (o, n))| {
                let mut r = compare(o, n);
                r["text"] = json!(l);
                r
            })
            .collect();
        let (min_cos, max_step) = summary(&rows);
        let pass = check_passes(min_cos);
        eprintln!(
            "deployment check: {key}: {sent} texts sent (about {tokens} tokens), min cosine {min_cos}, max int8 \
             step {max_step}: {}",
            if pass { "PASS" } else { "FAIL" }
        );
        st["deployment_check"] = json!({
            "date": today(), "deployment_now": deployment, "texts_sent": sent, "estimated_tokens_sent": tokens,
            "texts": "s03 fixture queries q-001 to q-010 (Query) and the whole-node texts of nodes reg-001 to reg-010 (Document); nodes.jsonl and queries.jsonl pinned by sha256",
            "against": "the vectors the floors run fc-2026-11-11-cdc1965 cached for the same texts, through the deployment it used",
            "rule": CHECK_RULE,
            "min_cosine": min_cos, "max_int8_step": max_step, "pass": pass, "per_text": rows,
        });
    } else {
        // Run to run (the first two passes) against each pass's drift from the floors run's vectors.
        let rows: Vec<Value> = (0..labels.len())
            .map(|i| {
                let mut r =
                    json!({ "text": labels[i], "a_vs_b": compare(&runs[0][i], &runs[1][i]) });
                for (p, run) in runs.iter().enumerate() {
                    r[format!("run{}_vs_floors_run", p + 1)] = compare(&olds[i], &run[i]);
                }
                r
            })
            .collect();
        let part = |k: &str| -> Vec<Value> { rows.iter().map(|r| r[k].clone()).collect() };
        let (ab_cos, ab_step) = summary(&part("a_vs_b"));
        let identical = part("a_vs_b")
            .iter()
            .all(|r| r["components_differing"] == json!(0));
        eprintln!(
            "deployment check: {key}: {passes} passes, {sent} texts sent (about {} tokens); run to run min cosine \
             {ab_cos}, max int8 step {ab_step}, identical {identical}",
            tokens * passes
        );
        st["deployment_check_repeat"] = json!({
            "date": today(), "deployment_now": deployment, "passes": passes, "texts_sent": sent,
            "estimated_tokens_sent": tokens * passes,
            "texts": "the same pinned s03 fixture texts as deployment_check",
            "run_to_run": { "min_cosine": ab_cos, "max_int8_step": ab_step, "bit_identical": identical },
            "per_text": rows,
        });
    }
    write_strength(&a.out, &key.to_string(), st)
}

/// The strength section of RESULT.md.
pub(super) fn render(all: &Value) -> String {
    let empty = serde_json::Map::new();
    let keys = all["keys"].as_object().unwrap_or(&empty);
    let measured: Vec<(&String, &Value)> = keys
        .iter()
        .filter_map(|(k, e)| e.get("strength").map(|s| (k, s)))
        .collect();
    if measured.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "\n## Strength cuts (PRD 002 search-10)\n\n\
         Method (embeddings-expert): `floor-calibration --strength`, from the rows of the key's floors run (its vector \
         cache, offline: a text not in it fails the run). Whole-node cosines; z = (cosine - median) / MAD over the N \
         nodes searched. Corpora: the three BEIR sets, the s03 fixture and `tight` (the 1,000 NFCorpus documents \
         nearest the one with most near neighbours), each at N = 300, 2,000 and 20,000 (capped at its size). Off-topic \
         queries are a subject the graph does not hold: another set's queries only for pairs whose domains are disjoint \
         (spot-checked), and the fixture's written off-topic list. Strong: no cell's off-topic top hits strong more \
         than 1 in 100; fair: none fair or strong more than 1 in 10. On-topic rates are reported, never targeted.\n\n\
         | key | run | z part | strong z (+ per ln N) | strong cosine | fair z (+ per ln N) | fair cosine | min population | max N measured (labels stop above 2x) |\n\
         | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for (k, st) in &measured {
        let withheld = st["cuts"].is_null();
        let c = if withheld {
            &st["cuts_measured"]
        } else {
            &st["cuts"]
        };
        if withheld {
            s += &format!(
                "| `{k}` | {} | withheld (deployment check failed; measured: strong z {} + {} ln N, cosine {}; fair z {} + {} ln N, cosine {}) | - | - | - | - | - | - |\n",
                st["run"].as_str().unwrap_or(""),
                c["strong_z"],
                c["strong_z_per_ln"],
                c["strong_score"],
                c["fair_z"],
                c["fair_z_per_ln"],
                c["fair_score"]
            );
            continue;
        }
        s += &format!(
            "| `{k}` | {} | {} | {} (+{}) | {} | {} (+{}) | {} | {} | {} |\n",
            st["run"].as_str().unwrap_or(""),
            c["z_part"],
            c["strong_z"],
            c["strong_z_per_ln"],
            c["strong_score"],
            c["fair_z"],
            c["fair_z_per_ln"],
            c["fair_score"],
            c["min_population"],
            c["max_n_measured"]
        );
    }
    s += "\n### Shipped cuts per corpus and N\n\n\
          On-topic: share of targets. Off-topic: how many of the cell's off-topic queries' top hits are strong, and fair or \
          strong (a cell of 45 queries cannot show 1 in 100: one is 2.2%, so none is allowed there).\n\n\
          | key | cell | on-topic strong | on-topic fair or strong | off-topic strong | off-topic fair or strong |\n\
          | --- | --- | --- | --- | --- | --- |\n";
    for (k, st) in &measured {
        for (cell, v) in st["shipped"]["cells"].as_object().into_iter().flatten() {
            let n = v["off_queries"].as_u64().unwrap_or(0);
            let of = |share: &Value| {
                let k = (share.as_f64().unwrap_or(0.0) * n as f64).round() as u64;
                format!("{k} of {n}")
            };
            let (off_s, off_f) = if n == 0 {
                (
                    "no off-topic queries".to_string(),
                    "no off-topic queries".to_string(),
                )
            } else {
                (of(&v["off_strong"]), of(&v["off_fair_or_strong"]))
            };
            s += &format!(
                "| `{k}` | {cell} | {} | {} | {off_s} | {off_f} |\n",
                v["on_strong"], v["on_fair_or_strong"]
            );
        }
    }
    s += "\n### z settling (embeddings-expert): strong hits per on-topic query, median / p90\n\n\
          | key | cell | queries | joint rule | cosine-only rule |\n| --- | --- | --- | --- | --- |\n";
    for (k, st) in &measured {
        for (cell, v) in st["z_settling"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(c, _)| *c != "caveat")
        {
            s += &format!(
                "| `{k}` | {cell} | {} | {} / {} | {} / {} |\n",
                v["queries"],
                v["joint"]["p50"],
                v["joint"]["p90"],
                v["cosine_only"]["p50"],
                v["cosine_only"]["p90"]
            );
        }
    }
    s += "\n### s03 fixture by query kind (shipped cuts; N = 300)\n\n\
          | key | kind | queries | target strong | target fair or strong | no_strong_match fires | a strong-only rule fires |\n\
          | --- | --- | --- | --- | --- | --- | --- |\n";
    for (k, st) in &measured {
        for (kind, v) in st["shipped"]["fixture"].as_object().into_iter().flatten() {
            s += &format!(
                "| `{k}` | {kind} | {} | {} | {} | {} | {} |\n",
                v["queries"],
                if v["target_strong"].is_null() {
                    &v["top_strong"]
                } else {
                    &v["target_strong"]
                },
                if v["target_fair_or_strong"].is_null() {
                    &v["top_fair_or_strong"]
                } else {
                    &v["target_fair_or_strong"]
                },
                v["no_strong_match_fires"],
                v["strong_only_rule_fires"]
            );
        }
    }
    s += "\n### z ablation (each family's cuts chosen alike; worst cell over the kept off-topic cells)\n\n\
          | key | family | off-topic strong rate | strong cuts | worst off-topic strong | worst off-topic fair or strong | fixture paraphrase strong |\n\
          | --- | --- | --- | --- | --- | --- | --- |\n";
    for (k, st) in &measured {
        for e in st["ablation"].as_array().into_iter().flatten() {
            if e["rate_unmet"] == json!(true) {
                s += &format!(
                    "| `{k}` | {} | {} | no cut meets the rate | - | - | - |\n",
                    e["family"].as_str().unwrap_or(""),
                    e["off_topic_strong_rate"]
                );
                continue;
            }
            let c = &e["strong"];
            s += &format!(
                "| `{k}` | {} | {} | z {} + {} ln N, cosine {}{} | {} | {} | {} |\n",
                e["family"].as_str().unwrap_or(""),
                e["off_topic_strong_rate"],
                c["z"],
                c["z_per_ln"],
                c["score"],
                if c["z_part"] == json!(false) {
                    " (no z)"
                } else {
                    ""
                },
                e["worst_off_topic_strong"],
                e["worst_off_topic_fair_or_strong"],
                e["fixture"]["paraphrase"]["target_strong"]
            );
        }
    }
    s += &format!("\nCaveat: {SETTLING_CAVEAT}.\n");
    let checked: Vec<_> = measured
        .iter()
        .filter(|(_, st)| !st["deployment_check"].is_null())
        .collect();
    if !checked.is_empty() {
        s += "\n### Deployment drift check (hosted keys)\n\n\
              The floors run's deployments are gone; the same s03 fixture texts embedded through today's deployment \
              against the floors run's cached vectors (10 queries, 10 whole-node texts). Pass: every cosine >= 0.999 (the \
              int8 step count is reported, not a criterion).\n\n\
              | key | date | texts | min cosine | max int8 step | result |\n| --- | --- | --- | --- | --- | --- |\n";
        for (k, st) in checked {
            let c = &st["deployment_check"];
            s += &format!(
                "| `{k}` | {} | {} | {} | {} | {} |\n",
                c["date"].as_str().unwrap_or(""),
                c["texts_sent"],
                c["min_cosine"],
                c["max_int8_step"],
                if c["pass"] == json!(true) {
                    "pass"
                } else {
                    "FAIL"
                }
            );
        }
    }
    s += "\n### MIN_POPULATION bootstrap (N: p90 z half-width / half the strong-fair gap)\n\n| key | rungs |\n| --- | --- |\n";
    for (k, st) in &measured {
        let rungs: Vec<String> = st["min_population"]["ladder"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| format!("{}: {} / {}", r["n"], r["z_halfwidth_p90"], r["half_gap"]))
            .collect();
        s += &format!("| `{k}` | {} |\n", rungs.join(", "));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every candidate off-topic pair was judged: kept or dropped, never both, never neither.
    #[test]
    fn every_candidate_pair_is_kept_or_dropped_once() {
        for corpus in ["scifact", "nfcorpus", "arguana", "fixture", "tight"] {
            for source in candidates(corpus) {
                let k = DISJOINT
                    .iter()
                    .filter(|(c, s, _)| *c == corpus && *s == source)
                    .count();
                let d = OVERLAPPING
                    .iter()
                    .filter(|(c, s, _)| *c == corpus && *s == source)
                    .count();
                assert_eq!(k + d, 1, "{corpus} <- {source}");
            }
        }
        assert_eq!(DISJOINT.len() + OVERLAPPING.len(), 5 * 4);
    }

    /// A deployment check below the cosine bar withholds the cuts; at or above it (whatever the int8 steps) it does
    /// not.
    #[test]
    fn a_failed_deployment_check_withholds_the_cuts() {
        let record = |min: f64, steps: i64| {
            json!({ "cuts": { "strong_score": 0.5 },
                "deployment_check": { "min_cosine": min, "max_int8_step": steps, "pass": true } })
        };
        let mut low = record(0.9989, 0);
        withhold_if_failed(&mut low);
        assert!(low["cuts"].is_null() && low["cuts_measured"]["strong_score"] == json!(0.5));
        assert_eq!(low["deployment_check"]["pass"], json!(false));
        for (min, steps) in [(0.999, 3), (0.999659, 3), (1.0, 0)] {
            let mut ok = record(min, steps);
            withhold_if_failed(&mut ok);
            assert_eq!(ok["cuts"]["strong_score"], json!(0.5), "{min}");
            assert!(ok["withheld"].is_null());
        }
    }
}
