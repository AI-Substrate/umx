//! floor-calibration (workstream 004 s26, embeddings-expert ruling 2026-11-11): one embedding key's search and
//! similar_to floors from its noise distributions, checked on labels. Every text is embedded exactly as umx stores
//! it: nodes go into a real graph through umx's own services (in a scratch folder), the embed loop is umx's
//! (planner, batches, pacer, apply), and cosines are over the int8 rows umx keeps, scored as umx scores them.
//!
//! - search floor F_q: p99 of query-to-random-document cosine (200 random documents per query, none in its qrels),
//!   each document's best row (whole node and fields), as the vector leg compares it;
//! - similar_to floor F_d: p99 of document-to-random-document cosine of whole-node rows (200 per document);
//! - acceptance (s26b): umx search's hybrid nDCG@10 on every BEIR task within 0.01 of the floor at 0 AND the s03
//!   fixture's umx search recall@10 within 0.02 of the floor at 0, in the same run; else F_q is lowered by 0.01 steps
//!   to the first value where both hold, never below the query null's p50 (stopping there records floor_gate_unmet).
//!
//! Datasets: BEIR SciFact, NFCorpus and ArguAna, fetched at run time into `<cache>/beir`, checked against pinned
//! sha256s (the s03 values) and never committed; the s03 synthetic fixture is gated on recall@10. Provider vectors
//! are cached in `<cache>/floor-calibration/<key>.vec` (never committed), so a rerun sends nothing it sent before.
//! The result is data: `<out>/floors.json` (one entry per key, merged) and `<out>/RESULT.md`, rendered from it.
//!
//! Usage: floor-calibration --embedder <instance> --cache <dir> --out <dir> --commit <sha>
//!        [--config-home <umx home holding config.toml>] [--tasks scifact,nfcorpus,arguana,fixture]
//!        [--tokens-per-min 1500000] [--max-queries N] [--max-docs N] [--dry-run]
//!
//! Probe mode counts vector rows only, so it can run on graphs whose text must not leave the machine: each graph
//! folder under `--probe-root` is loaded in process (load it from a copy), each `alias | phrase` line of
//! `--probes` is searched at every `--floors` value, and only counts are printed (no titles, ids or text).
//!        floor-calibration --embedder <instance> --cache <dir> --probe-root <dir> --probes <file> --floors 0.5,0.3
//!
//! Strength mode (PRD 002 search-10, embeddings-expert) measures each key's strength cuts from the cached rows of a
//! full floors run, sending nothing (a text not in the cache fails the run), and merges them into floors.json beside
//! the key's floors; `--repeat N --offtopic-rows <file>` (a local instance only) also embeds 20 fixture queries N
//! times live, counting int8 differences, and records the fixture's off-topic query rows for the services tests:
//!        floor-calibration --strength --embedder <instance> --cache <dir> --out <dir> --commit <sha> [--raw <dir>]
//!        [--strong-rate 0.01] [--cosine-only] [--spot <file>] [--offtopic-rows <file> [--send-offtopic]]
//! `--render --out <dir>` writes RESULT.md again from floors.json, nothing else.
//! `--deployment-check` (with `--strength`) sends 10 s03 fixture queries and 10 fixture whole-node texts once and
//! compares them with the floors run's cached vectors (a deployment that changed the model behind it shows here).
//! `--send-offtopic` is the only way a hosted instance is sent anything in strength mode: the written off-topic list.
//!
//! Record mode writes the s03 fixture's vectors, every row, as production makes them, for hybrid_recall (s26b):
//!        floor-calibration --embedder <instance> --cache <fresh dir> --record <out.tsv>

use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use umx_compose::{Config, Mode, compose, compose_config};
use umx_core::Id;
use umx_core::ports::{Credential, EmbedError, Embedded, Embedder, Locality, ModelKey, TextKind};
use umx_core::similarity::{Floors, dot, inverse_length};
use umx_core::vectors::{quantise, whole_node_field};
use umx_services::{
    BatchLine, BatchOp, BatchRequest, Ctx, NodeAddRequest, Pacer, Planned, Services, Step,
    WORD_MAX_DF_SHARE, batches, estimate_tokens, step,
};

const BEIR: &str = "https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets/";
/// sha256 of each BEIR zip as first fetched on 2026-11-10 (s03, docs/project/benchmarks/embed-bakeoff/RESULT.md).
const BEIR_SHA256: &[(&str, &str)] = &[
    (
        "scifact",
        "536e14446a0ba56ed1398ab1055f39fe852686ecad24a6306c80c490fa8e0165",
    ),
    (
        "nfcorpus",
        "efe5be03f8c5b86a5870102d0599d227c8c6e2484328e68c6522560385671b0b",
    ),
    (
        "arguana",
        "cfdf79adce27a401b3cd3ea267903134dbfab2c6afeb95d7fe5724a00bf7557b",
    ),
];
const NULL_SAMPLES: usize = 200;
/// The most documents whose document-to-document null is sampled per task.
const NULL_DOCS: usize = 3000;
/// How far below the no-floor run nDCG@10 may fall on any task (the expert's acceptance).
const NDCG_SLACK: f64 = 0.01;
const LOWER_STEP: f64 = 0.01;
/// How far below the no-floor run the s03 fixture's umx search recall@10 may fall (embeddings-expert, s26b).
const FIXTURE_SLACK: f64 = 0.02;
/// Hits asked of each search: ArguAna drops the query's own document (as BEIR does), so ask for more than 10.
const SEARCH_LIMIT: usize = 20;
const BATCH_NODES: usize = 2000;

struct Args {
    embedder: String,
    config_home: Option<PathBuf>,
    cache: PathBuf,
    out: PathBuf,
    commit: String,
    tasks: Vec<String>,
    tokens_per_min: u64,
    max_queries: Option<usize>,
    max_docs: Option<usize>,
    dry_run: bool,
    probe_root: Option<PathBuf>,
    probes: Option<PathBuf>,
    floors: Vec<f64>,
    record: Option<PathBuf>,
    strength: bool,
    raw: Option<PathBuf>,
    repeat: usize,
    offtopic_rows: Option<PathBuf>,
    strong_rate: f64,
    cosine_only: bool,
    spot: Option<PathBuf>,
    send_offtopic: bool,
    deployment_check: bool,
    render: bool,
}

const USAGE: &str = "floor-calibration --embedder <instance> --cache <dir> --out <dir> --commit <sha> \
[--config-home <dir>] [--tasks scifact,nfcorpus,arguana,fixture] [--tokens-per-min 1500000] [--max-queries N] \
[--max-docs N] [--dry-run]";

fn args(argv: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut a = Args {
        embedder: String::new(),
        config_home: None,
        cache: PathBuf::new(),
        out: PathBuf::new(),
        commit: String::new(),
        tasks: ["scifact", "nfcorpus", "arguana", "fixture"]
            .map(String::from)
            .to_vec(),
        tokens_per_min: 1_500_000,
        max_queries: None,
        max_docs: None,
        dry_run: false,
        probe_root: None,
        probes: None,
        floors: Vec::new(),
        record: None,
        strength: false,
        raw: None,
        repeat: 0,
        offtopic_rows: None,
        strong_rate: 0.01,
        cosine_only: false,
        spot: None,
        send_offtopic: false,
        deployment_check: false,
        render: false,
    };
    let mut it = argv.into_iter();
    while let Some(flag) = it.next() {
        if flag == "--dry-run" {
            a.dry_run = true;
            continue;
        }
        if flag == "--strength" {
            a.strength = true;
            continue;
        }
        if flag == "--render" {
            a.render = true;
            continue;
        }
        if flag == "--deployment-check" {
            a.deployment_check = true;
            continue;
        }
        if flag == "--send-offtopic" {
            a.send_offtopic = true;
            continue;
        }
        if flag == "--cosine-only" {
            a.cosine_only = true;
            continue;
        }
        let value = it.next().ok_or(format!("{flag} needs a value. {USAGE}"))?;
        let number = || {
            value
                .parse::<usize>()
                .map_err(|_| format!("{flag} takes a number"))
        };
        match flag.as_str() {
            "--embedder" => a.embedder = value,
            "--config-home" => a.config_home = Some(PathBuf::from(&value)),
            "--cache" => a.cache = PathBuf::from(&value),
            "--out" => a.out = PathBuf::from(&value),
            "--commit" => a.commit = value,
            "--tasks" => a.tasks = value.split(',').map(String::from).collect(),
            "--tokens-per-min" => a.tokens_per_min = number()? as u64,
            "--max-queries" => a.max_queries = Some(number()?),
            "--max-docs" => a.max_docs = Some(number()?),
            "--probe-root" => a.probe_root = Some(PathBuf::from(&value)),
            "--probes" => a.probes = Some(PathBuf::from(&value)),
            "--record" => a.record = Some(PathBuf::from(&value)),
            "--raw" => a.raw = Some(PathBuf::from(&value)),
            "--repeat" => a.repeat = number()?,
            "--spot" => a.spot = Some(PathBuf::from(&value)),
            "--offtopic-rows" => a.offtopic_rows = Some(PathBuf::from(&value)),
            "--strong-rate" => {
                a.strong_rate = value
                    .parse::<f64>()
                    .ok()
                    .filter(|r| *r > 0.0 && *r < 1.0)
                    .ok_or("--strong-rate takes a share between 0 and 1")?
            }
            "--floors" => {
                a.floors = value
                    .split(',')
                    .map(|f| {
                        f.trim()
                            .parse::<f64>()
                            .map_err(|_| "--floors takes numbers".to_string())
                    })
                    .collect::<Result<_, _>>()?
            }
            _ => return Err(format!("unknown flag {flag}. {USAGE}")),
        }
    }
    if a.render {
        return if a.out.as_os_str().is_empty() {
            Err(format!("--render needs --out. {USAGE}"))
        } else {
            Ok(a)
        };
    }
    if a.record.is_some() {
        // A record always embeds every fixture text and writes the file; it has no dry run, so refuse the pair before
        // any provider is built (reviewer R206-B1: a hosted instance would otherwise be sent 980 texts).
        if a.dry_run {
            return Err(format!(
                "--record has no --dry-run: it always embeds the fixture's 980 texts and writes the file. {USAGE}"
            ));
        }
        return if a.embedder.is_empty() || a.cache.as_os_str().is_empty() {
            Err(format!("--record needs --embedder and --cache. {USAGE}"))
        } else {
            Ok(a)
        };
    }
    if a.probes.is_some() {
        return if a.embedder.is_empty()
            || a.cache.as_os_str().is_empty()
            || a.probe_root.is_none()
            || a.floors.is_empty()
        {
            Err(format!(
                "probe mode needs --embedder, --cache, --probe-root and --floors. {USAGE}"
            ))
        } else {
            Ok(a)
        };
    }
    if a.strength && a.dry_run {
        return Err(format!(
            "--strength has no --dry-run: it sends nothing. {USAGE}"
        ));
    }
    if a.deployment_check && a.repeat > strength::MAX_CHECK_PASSES {
        return Err(format!(
            "--deployment-check --repeat {}: at most {} passes, each resends the pinned texts. {USAGE}",
            a.repeat,
            strength::MAX_CHECK_PASSES
        ));
    }
    if a.send_offtopic && (!a.strength || a.offtopic_rows.is_none()) {
        return Err(format!(
            "--send-offtopic is only for --strength with --offtopic-rows. {USAGE}"
        ));
    }
    if a.embedder.is_empty() || a.cache.as_os_str().is_empty() || a.out.as_os_str().is_empty() {
        return Err(format!(
            "--embedder, --cache and --out are required. {USAGE}"
        ));
    }
    if a.commit.is_empty() && !a.dry_run {
        return Err("--commit is required (the umx commit the run measures)".into());
    }
    Ok(a)
}

/// Cached vectors by (text kind, sha256 of the text).
type Known = HashMap<(u8, [u8; 32]), Vec<f32>>;

/// A provider wrapped with a vector cache by (kind, sha256 of the text): what it returns is what umx would have
/// stored, and a rerun sends only what it has not sent before. Counts what it sends.
struct Cached {
    inner: Arc<dyn Embedder>,
    file: Mutex<fs::File>,
    known: Mutex<Known>,
    sent: Mutex<(usize, usize)>,
    /// Strength mode: a text not in the cache fails the embed instead of being sent (nothing leaves the machine).
    offline: std::sync::atomic::AtomicBool,
    /// Every cached record an embed read, by (kind, sha256): what a run used of the cache.
    read: Mutex<BTreeSet<(u8, [u8; 32])>>,
}

fn kind_byte(kind: TextKind) -> u8 {
    match kind {
        TextKind::Document => 0,
        TextKind::Query => 1,
    }
}

fn sha256_bytes(text: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(text).into()
}

impl Cached {
    /// Opens (or starts) the cache at `path`: records of kind (1 byte), sha256 (32), dims (u16 LE), f32 LE values. A
    /// torn last record (a run stopped mid-write) is cut off.
    fn open(inner: Arc<dyn Embedder>, path: &Path) -> std::io::Result<Cached> {
        let dims = inner.key().dims();
        let mut known = HashMap::new();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        if let Ok(mut f) = fs::File::open(path) {
            let mut bytes = Vec::new();
            f.read_to_end(&mut bytes)?;
            let record = 1 + 32 + 2 + 4 * dims;
            let whole = bytes.len() / record * record;
            for r in bytes[..whole].chunks_exact(record) {
                if u16::from_le_bytes([r[33], r[34]]) as usize != dims {
                    continue;
                }
                let v = r[35..]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes(*c))
                    .collect();
                known.insert((r[0], r[1..33].try_into().unwrap()), v);
            }
            if whole != bytes.len() {
                fs::OpenOptions::new()
                    .write(true)
                    .open(path)?
                    .set_len(whole as u64)?;
            }
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Cached {
            inner,
            file: Mutex::new(file),
            known: Mutex::new(known),
            sent: Mutex::new((0, 0)),
            offline: std::sync::atomic::AtomicBool::new(false),
            read: Mutex::new(BTreeSet::new()),
        })
    }

    fn has(&self, text: &str, kind: TextKind) -> bool {
        self.known
            .lock()
            .unwrap()
            .contains_key(&(kind_byte(kind), sha256_bytes(text.as_bytes())))
    }
}

impl Embedder for Cached {
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
        let k = kind_byte(kind);
        let hashes: Vec<[u8; 32]> = texts.iter().map(|t| sha256_bytes(t.as_bytes())).collect();
        let missing: Vec<usize> = {
            let known = self.known.lock().unwrap();
            (0..texts.len())
                .filter(|&i| !known.contains_key(&(k, hashes[i])))
                .collect()
        };
        let mut truncated = Vec::new();
        if !missing.is_empty() && self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(EmbedError::Config(format!(
                "{} texts are not in the vector cache and this run sends nothing",
                missing.len()
            )));
        }
        {
            let mut read = self.read.lock().unwrap();
            let gone: BTreeSet<usize> = missing.iter().copied().collect();
            for (i, h) in hashes.iter().enumerate() {
                if !gone.contains(&i) {
                    read.insert((k, *h));
                }
            }
        }
        if !missing.is_empty() {
            let send: Vec<String> = missing.iter().map(|&i| texts[i].clone()).collect();
            let got = self.inner.embed(&send, kind)?;
            {
                let mut sent = self.sent.lock().unwrap();
                sent.0 += send.len();
                sent.1 += send.iter().map(|t| estimate_tokens(t)).sum::<usize>();
            }
            truncated = got.truncated.iter().map(|&j| missing[j]).collect();
            let mut file = self.file.lock().unwrap();
            let mut known = self.known.lock().unwrap();
            for (j, v) in got.vectors.into_iter().enumerate() {
                let i = missing[j];
                let mut rec = Vec::with_capacity(35 + 4 * v.len());
                rec.push(k);
                rec.extend_from_slice(&hashes[i]);
                rec.extend_from_slice(&(v.len() as u16).to_le_bytes());
                for x in &v {
                    rec.extend_from_slice(&x.to_le_bytes());
                }
                file.write_all(&rec)
                    .map_err(|e| EmbedError::Config(format!("vector cache write failed: {e}")))?;
                known.insert((k, hashes[i]), v);
            }
        }
        let known = self.known.lock().unwrap();
        Ok(Embedded {
            vectors: hashes.iter().map(|h| known[&(k, *h)].clone()).collect(),
            truncated,
            tokens: None,
        })
    }
    fn key(&self) -> ModelKey {
        self.inner.key()
    }
    fn concurrency_ceiling(&self) -> usize {
        self.inner.concurrency_ceiling()
    }
    fn max_batch_inputs(&self) -> usize {
        self.inner.max_batch_inputs()
    }
    fn max_input_tokens(&self) -> usize {
        self.inner.max_input_tokens()
    }
    fn locality(&self) -> Locality {
        self.inner.locality()
    }
    fn ready(&self) -> Result<(), EmbedError> {
        self.inner.ready()
    }
    fn credential(&self) -> Option<Credential> {
        self.inner.credential()
    }
    fn adapter_id(&self) -> &'static str {
        self.inner.adapter_id()
    }
}

/// One task's labelled data: documents (id, title, text), queries (id, text) and graded qrels.
struct Task {
    name: String,
    docs: Vec<(String, String, String)>,
    queries: Vec<(String, String)>,
    qrels: HashMap<String, HashMap<String, u32>>,
    source: Value,
    /// BEIR's ArguAna rule: a query's own document never counts as a hit.
    ignore_own: bool,
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    let mut f = fs::File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let status = cmd.status().map_err(|e| format!("{cmd:?}: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or(format!("{cmd:?}: {status}"))
}

fn json_lines(p: &Path) -> Result<Vec<Value>, String> {
    let f = fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    BufReader::new(f)
        .lines()
        .filter(|l| l.as_ref().map_or(true, |l| !l.trim().is_empty()))
        .map(|l| serde_json::from_str(&l.map_err(|e| e.to_string())?).map_err(|e| e.to_string()))
        .collect()
}

fn text(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

/// A BEIR task from `<cache>/beir`, fetched with curl when absent, its zip checked against the pinned sha256 on
/// every use.
fn beir(cache: &Path, name: &str) -> Result<Task, String> {
    let pinned = BEIR_SHA256
        .iter()
        .find(|(t, _)| *t == name)
        .ok_or(format!("unknown task {name}"))?
        .1;
    let dir = cache.join("beir");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let zip = dir.join(format!("{name}.zip"));
    let url = format!("{BEIR}{name}.zip");
    if !zip.exists() {
        let part = dir.join(format!("{name}.zip.part"));
        run(Command::new("curl")
            .args(["-fsSL", "-o"])
            .arg(&part)
            .arg(&url))?;
        fs::rename(&part, &zip).map_err(|e| e.to_string())?;
    }
    let got = sha256_file(&zip).map_err(|e| e.to_string())?;
    if got != pinned {
        return Err(format!(
            "{}: sha256 {got} is not the pinned {pinned}; refusing it",
            zip.display()
        ));
    }
    let data = dir.join(name);
    if !data.join("corpus.jsonl").exists() {
        run(Command::new("unzip")
            .args(["-q", "-o"])
            .arg(&zip)
            .arg("-d")
            .arg(&dir))?;
    }
    let docs = json_lines(&data.join("corpus.jsonl"))?
        .iter()
        .map(|d| (text(d, "_id"), text(d, "title"), text(d, "text")))
        .collect();
    let all: HashMap<String, String> = json_lines(&data.join("queries.jsonl"))?
        .iter()
        .map(|q| (text(q, "_id"), text(q, "text")))
        .collect();
    let mut qrels: HashMap<String, HashMap<String, u32>> = HashMap::new();
    let tsv = fs::read_to_string(data.join("qrels/test.tsv")).map_err(|e| e.to_string())?;
    for line in tsv.lines().skip(1) {
        let mut p = line.split('\t');
        let (q, d, g) = (
            p.next(),
            p.next(),
            p.next().and_then(|g| g.trim().parse::<u32>().ok()),
        );
        if let (Some(q), Some(d), Some(g)) = (q, d, g)
            && g > 0
        {
            qrels.entry(q.into()).or_default().insert(d.into(), g);
        }
    }
    let mut ids: Vec<&String> = qrels.keys().filter(|q| all.contains_key(*q)).collect();
    ids.sort();
    let queries = ids
        .into_iter()
        .map(|q| (q.clone(), all[q].clone()))
        .collect();
    Ok(Task {
        name: name.into(),
        docs,
        queries,
        qrels,
        source: json!({ "url": url, "sha256": got }),
        ignore_own: name == "arguana",
    })
}

/// The s03 synthetic fixture (300 nodes, 80 queries), committed beside the bake-off.
fn fixture() -> Result<Task, String> {
    let dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/project/benchmarks/embed-bakeoff");
    let nodes = json_lines(&dir.join("nodes.jsonl"))?;
    let qs = json_lines(&dir.join("queries.jsonl"))?;
    Ok(Task {
        name: "fixture".into(),
        docs: nodes
            .iter()
            .map(|n| (text(n, "id"), text(n, "title"), text(n, "note")))
            .collect(),
        queries: qs
            .iter()
            .map(|q| (text(q, "id"), text(q, "text")))
            .collect(),
        qrels: qs
            .iter()
            .map(|q| {
                let rel = q["relevant"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|r| r.as_str())
                    .map(|r| (r.to_string(), 1))
                    .collect();
                (text(q, "id"), rel)
            })
            .collect(),
        source: json!({ "path": "docs/project/benchmarks/embed-bakeoff" }),
        ignore_own: false,
    })
}

/// A smoke run: the first `max_queries` queries and, with `max_docs`, their relevant documents plus the first others
/// up to that many. A smoke result is marked `smoke` and never feeds the table.
fn trim(task: &mut Task, max_queries: Option<usize>, max_docs: Option<usize>) {
    if let Some(n) = max_queries {
        task.queries.truncate(n);
    }
    if let Some(n) = max_docs {
        let wanted: BTreeSet<String> = task
            .queries
            .iter()
            .filter_map(|(q, _)| task.qrels.get(q))
            .flat_map(|r| r.keys().cloned())
            .collect();
        let mut kept = 0;
        task.docs.retain(|(d, _, _)| {
            if wanted.contains(d) {
                return true;
            }
            kept += 1;
            kept <= n
        });
    }
}

/// A node title from a document: its title, else its first 12 words; one line, at most 200 characters.
fn title_of(title: &str, body: &str) -> String {
    let (base, words) = if title.trim().is_empty() {
        (body, 12)
    } else {
        (title, 64)
    };
    let t: String = base
        .split_whitespace()
        .take(words)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect();
    if t.is_empty() { "untitled".into() } else { t }
}

/// An int8 row with its inverse length, as umx keeps it.
#[derive(Clone)]
struct Row {
    v: Vec<i8>,
    inv: f32,
}

impl Row {
    fn of(f: &[f32]) -> Row {
        let v = quantise(f);
        let inv = inverse_length(&v);
        Row { v, inv }
    }

    /// The cosine as the vector leg scores a row (umx_core::similarity::top_k): the dot times both inverse lengths.
    fn cosine(&self, other: &Row) -> f32 {
        dot(&self.v, &other.v) as f32 * self.inv * other.inv
    }
}

/// A seeded generator (splitmix64), so every run samples the same pairs.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn round4(x: f64) -> f64 {
    (x * 1e4).round() / 1e4
}

/// Nearest-rank percentiles.
fn percentiles(values: &mut [f32]) -> Value {
    values.sort_by(f32::total_cmp);
    let at = |p: f64| {
        if values.is_empty() {
            return Value::Null;
        }
        let i = ((p * values.len() as f64).ceil() as usize).clamp(1, values.len()) - 1;
        json!(round4(f64::from(values[i])))
    };
    json!({ "p50": at(0.50), "p95": at(0.95), "p99": at(0.99), "samples": values.len() })
}

/// One task loaded into a scratch graph and embedded.
struct Built {
    svc: Services,
    by_node: HashMap<Id, usize>,
    /// Per document: its rows, each marked whole-node or not, as umx stored them.
    rows: Vec<Vec<(bool, Row)>>,
    held: usize,
    failed: usize,
}

fn ctx(alias: &str) -> Ctx {
    let mut c = Ctx::new("floor-calibration");
    c.graph = Some(alias.into());
    c
}

fn build(
    task: &Task,
    work: &Path,
    embedder: &Arc<Cached>,
    rate: u64,
    dry_run: bool,
) -> Result<Built, String> {
    build_with(task, work, embedder, rate, dry_run, true)
}

/// [`build`]; with `apply` false the vectors are only collected into `rows`, never stored in the graph (strength
/// mode reads only the rows, and storing is most of the time).
fn build_with(
    task: &Task,
    work: &Path,
    embedder: &Arc<Cached>,
    rate: u64,
    dry_run: bool,
    apply: bool,
) -> Result<Built, String> {
    let root = work.join("graphs");
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let config = Config::at(work.join("home"), None).roots([root.to_str().unwrap()]);
    let mut svc = compose(Mode::Real, &config).with_embedder(embedder.clone() as Arc<dyn Embedder>);
    let alias = task.name.as_str();
    let path = root.join(alias);
    svc.graph_new(
        &ctx(alias),
        path.to_str().unwrap(),
        alias,
        None,
        false,
        None,
    )
    .map_err(|p| format!("graph new: {p:?}"))?;
    let mut by_node = HashMap::new();
    for (chunk_no, chunk) in task.docs.chunks(BATCH_NODES).enumerate() {
        let lines = chunk
            .iter()
            .enumerate()
            .map(|(i, (_, title, body))| BatchLine {
                line: i as u32 + 1,
                op: BatchOp::NodeAdd {
                    id: None,
                    created: None,
                    ref_name: None,
                    req: NodeAddRequest {
                        kind: "concept".into(),
                        title: title_of(title, body),
                        note: Some(body.clone()),
                        ..Default::default()
                    },
                    parent_ref: None,
                },
            })
            .collect();
        let done = svc
            .batch(
                &ctx(alias),
                BatchRequest {
                    raised_in: None,
                    idempotency: None,
                    reason: None,
                    echo_sentences: false,
                    lines,
                    dry_run: false,
                    refs: Default::default(),
                    refs_graph: None,
                },
            )
            .map_err(|p| format!("batch: {p:?}"))?;
        for row in done.value.rows {
            if let Some(id) = row.id {
                by_node.insert(id, chunk_no * BATCH_NODES + row.line as usize - 1);
            }
        }
    }
    let mut rows: Vec<Vec<(bool, Row)>> = vec![Vec::new(); task.docs.len()];
    let whole = whole_node_field();
    let (mut held, mut failed) = (0, 0);
    let mut pacer = Pacer::new(rate);
    let start = Instant::now();
    let now = || start.elapsed().as_millis() as u64;
    loop {
        let jobs = svc.take_embed_jobs();
        if jobs.is_empty() {
            break;
        }
        let planned: Vec<Planned> = jobs
            .iter()
            .filter_map(|j| svc.embed_plan(j).ok().flatten())
            .collect();
        if dry_run {
            let tokens: usize = planned
                .iter()
                .flat_map(|p| &p.texts)
                .filter(|w| !embedder.has(&w.text, TextKind::Document))
                .map(|w| estimate_tokens(&w.text))
                .sum();
            let texts: usize = planned.iter().map(|p| p.texts.len()).sum();
            eprintln!(
                "{}: {} nodes, {texts} texts, about {tokens} document tokens to send (cached excluded)",
                task.name,
                planned.len()
            );
            break;
        }
        let all = batches(planned);
        let total = all.len();
        for (n, batch) in all.into_iter().enumerate() {
            loop {
                std::thread::sleep(Duration::from_millis(pacer.wait_ms(now())));
                match step(&**embedder, &batch, &mut pacer, now()) {
                    Step::Done { vectors, notes } => {
                        held += notes.len();
                        for v in &vectors {
                            if let Some(&d) = by_node.get(&v.job.node) {
                                for (field, _, f) in &v.vectors {
                                    rows[d].push((*field == whole, Row::of(f)));
                                }
                            }
                        }
                        if apply {
                            svc.embed_apply(&vectors)
                                .map_err(|p| format!("apply: {p:?}"))?;
                        }
                        break;
                    }
                    Step::Retry { wait_ms, .. } => {
                        eprintln!("{}: provider asked to wait {wait_ms} ms", task.name);
                        std::thread::sleep(Duration::from_millis(wait_ms));
                    }
                    Step::Failed { jobs, error } => {
                        eprintln!("{}: {} nodes failed: {error:?}", task.name, jobs.len());
                        failed += jobs.len();
                        break;
                    }
                }
            }
            if n % 50 == 0 || n + 1 == total {
                eprintln!(
                    "{}: batch {}/{total}, {:.0} s",
                    task.name,
                    n + 1,
                    start.elapsed().as_secs_f64()
                );
            }
        }
    }
    Ok(Built {
        svc,
        by_node,
        rows,
        held,
        failed,
    })
}

struct QueryRow {
    id: String,
    text: String,
    vectors: umx_services::QueryVectors,
    row: Option<Row>,
}

/// Each query's vectors through umx's own query embed (the search plan's), once, and its int8 row.
fn embed_queries(task: &Task, b: &Built) -> Result<Vec<QueryRow>, String> {
    let mut out = Vec::new();
    for (i, (id, q)) in task.queries.iter().enumerate() {
        if !task.qrels.contains_key(id) {
            continue;
        }
        let plan = b
            .svc
            .search_plan(&ctx(&task.name), q, None, SEARCH_LIMIT)
            .map_err(|p| format!("search plan: {p:?}"))?;
        let vectors = plan.embed();
        // One embedder instance serves the scratch graph and the query is one phrasing, so it has at most one vector.
        let row = match vectors.values().next() {
            Some(Ok(v)) => v.first().map(|v| Row::of(v)),
            Some(Err(e)) => return Err(format!("query {id}: {e:?}")),
            None => None,
        };
        out.push(QueryRow {
            id: id.clone(),
            text: q.clone(),
            vectors,
            row,
        });
        if i % 250 == 0 {
            eprintln!("{}: query {}/{}", task.name, i + 1, task.queries.len());
        }
    }
    Ok(out)
}

/// umx search over every query with the search floor at `floor`: nDCG@10 (linear gain, as pytrec_eval and s03),
/// recall@10, vector rows in the top 10 per query, and the queries with at least one.
fn evaluate(task: &Task, b: &mut Built, key: &ModelKey, floor: f64, queries: &[QueryRow]) -> Value {
    let mut floors = Floors::default();
    floors.set(key, Some(floor as f32), None);
    b.svc.set_floors(floors);
    let (mut ndcg, mut recall, mut vrows, mut with_v) = (0.0, 0.0, 0.0, 0usize);
    for q in queries {
        let plan = b
            .svc
            .search_plan(&ctx(&task.name), &q.text, None, SEARCH_LIMIT)
            .expect("search plan");
        let hits = b
            .svc
            .search_finish_with(plan, q.vectors.clone(), WORD_MAX_DF_SHARE)
            .expect("search")
            .value
            .hits;
        let top: Vec<(&str, bool)> = hits
            .iter()
            .filter_map(|h| {
                b.by_node
                    .get(&h.node.id)
                    .map(|&d| (task.docs[d].0.as_str(), h.vector.is_some()))
            })
            .filter(|(d, _)| !(task.ignore_own && *d == q.id))
            .take(10)
            .collect();
        let rel = &task.qrels[&q.id];
        let dcg: f64 = top
            .iter()
            .enumerate()
            .map(|(i, (d, _))| f64::from(*rel.get(*d).unwrap_or(&0)) / ((i + 2) as f64).log2())
            .sum();
        let mut ideal: Vec<u32> = rel.values().copied().collect();
        ideal.sort_unstable_by(|a, b| b.cmp(a));
        let idcg: f64 = ideal
            .iter()
            .take(10)
            .enumerate()
            .map(|(i, g)| f64::from(*g) / ((i + 2) as f64).log2())
            .sum();
        ndcg += if idcg > 0.0 { dcg / idcg } else { 0.0 };
        recall +=
            top.iter().filter(|(d, _)| rel.contains_key(*d)).count() as f64 / rel.len() as f64;
        let v = top.iter().filter(|(_, v)| *v).count();
        vrows += v as f64;
        with_v += usize::from(v > 0);
    }
    let n = queries.len().max(1) as f64;
    json!({
        "floor": round4(floor), "ndcg10": round4(ndcg / n), "recall10": round4(recall / n),
        "vector_rows_per_query": round4(vrows / n), "queries_with_vector_row": with_v, "queries": queries.len(),
    })
}

/// The two null samples: query to random non-relevant document (that document's best row), and document to random
/// other document (whole-node rows, scored as similar_to scores them).
fn nulls(task: &Task, b: &Built, queries: &[QueryRow], seed: u64) -> (Vec<f32>, Vec<f32>) {
    let mut rng = Rng(seed);
    let n = task.docs.len();
    let mut qd = Vec::new();
    for q in queries {
        let Some(qr) = &q.row else { continue };
        let rel = &task.qrels[&q.id];
        let (mut taken, mut tries) = (0, 0);
        while taken < NULL_SAMPLES && tries < NULL_SAMPLES * 20 {
            tries += 1;
            let d = rng.below(n);
            let id = task.docs[d].0.as_str();
            if rel.contains_key(id) || (task.ignore_own && id == q.id) || b.rows[d].is_empty() {
                continue;
            }
            let best = b.rows[d]
                .iter()
                .map(|(_, r)| qr.cosine(r))
                .fold(f32::NEG_INFINITY, f32::max);
            qd.push(best);
            taken += 1;
        }
    }
    let whole: Vec<Option<&Row>> = b
        .rows
        .iter()
        .map(|rs| rs.iter().find(|(w, _)| *w).map(|(_, r)| r))
        .collect();
    let mut order: Vec<usize> = (0..n).filter(|&i| whole[i].is_some()).collect();
    for i in (1..order.len()).rev() {
        order.swap(i, rng.below(i + 1));
    }
    let mut dd = Vec::new();
    for &a in order.iter().take(NULL_DOCS) {
        let ra = whole[a].expect("kept");
        let (mut taken, mut tries) = (0, 0);
        while taken < NULL_SAMPLES && tries < NULL_SAMPLES * 20 {
            tries += 1;
            let o = rng.below(n);
            if o == a {
                continue;
            }
            if let Some(ro) = whole[o] {
                dd.push(
                    umx_core::similar::score_units(dot(&ra.v, &ro.v), ra.inv, ro.inv) as f32
                        / 10_000.0,
                );
                taken += 1;
            }
        }
    }
    (qd, dd)
}

fn today() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        / 86_400;
    // The civil date from days since 1970-01-01 (Howard Hinnant's days_from_civil, inverted).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn main() -> ExitCode {
    match calibrate() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("floor-calibration: {e}");
            ExitCode::FAILURE
        }
    }
}

fn calibrate() -> Result<(), String> {
    calibrate_with(std::env::args().skip(1))
}

fn calibrate_with(argv: impl IntoIterator<Item = String>) -> Result<(), String> {
    let a = args(argv)?;
    if a.render {
        return strength::render_only(&a.out);
    }
    let env = Config::from_env();
    let mut source = match &a.config_home {
        Some(home) => Config::at(home.clone(), None).search_path(env.search_path.clone()),
        None => Config::from_env(),
    };
    source.models_dir = env.models_dir.clone();
    let settings = compose_config(Mode::Real, &source);
    let embedders = umx_compose::embedders(Mode::Real, &source, &settings);
    let instance = embedders.get(&a.embedder).ok_or(format!(
        "no embedder instance {} in {}",
        a.embedder,
        source.config_file().display()
    ))?;
    let inner = instance
        .built
        .clone()
        .map_err(|e| format!("instance {}: {e}", a.embedder))?;
    let key = inner.key();
    let resolved = settings.resolve(None);
    let deployment = resolved
        .value(&format!("embedders.{}.deployment", a.embedder))
        .and_then(|v| v.as_text())
        .map(String::from);
    let cached = Arc::new(
        Cached::open(
            inner,
            &a.cache.join("floor-calibration").join(format!("{key}.vec")),
        )
        .map_err(|e| format!("vector cache: {e}"))?,
    );
    if let Some(probes) = &a.probes {
        return probe(&a, probes, &cached, &key);
    }
    if a.deployment_check {
        if !a.strength {
            return Err(format!("--deployment-check goes with --strength. {USAGE}"));
        }
        return strength::deployment_check(&a, &cached, &key, deployment);
    }
    if a.strength {
        let cache_file = a.cache.join("floor-calibration").join(format!("{key}.vec"));
        return strength::run(
            &a,
            &cached,
            &key,
            &cache_file,
            instance.kind.as_str(),
            deployment,
            instance.local_backend().map(|(b, _)| b),
        );
    }
    if let Some(out) = &a.record {
        return record(
            &a,
            out,
            &cached,
            &key,
            instance.local_backend().map(|(b, _)| b),
        );
    }
    let run_id = format!(
        "fc-{}-{}",
        today(),
        a.commit.chars().take(7).collect::<String>()
    );
    eprintln!(
        "floor-calibration: {key} through instance {} ({}), run {run_id}",
        a.embedder, instance.kind
    );
    let scratch =
        std::env::temp_dir().join(format!("umx-floor-calibration-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let mut tasks = BTreeMap::new();
    let (mut all_qd, mut all_dd) = (Vec::new(), Vec::new());
    let mut built: Vec<(Task, Built, Vec<QueryRow>)> = Vec::new();
    for (i, name) in a.tasks.iter().enumerate() {
        let mut task = if name == "fixture" {
            fixture()?
        } else {
            beir(&a.cache, name)?
        };
        trim(&mut task, a.max_queries, a.max_docs);
        eprintln!(
            "{name}: {} documents, {} queries",
            task.docs.len(),
            task.queries.len()
        );
        let mut b = build(
            &task,
            &scratch.join(name),
            &cached,
            a.tokens_per_min,
            a.dry_run,
        )?;
        if a.dry_run {
            let q: usize = task
                .queries
                .iter()
                .filter(|(_, t)| !cached.has(t, TextKind::Query))
                .map(|(_, t)| estimate_tokens(t))
                .sum();
            eprintln!("{name}: about {q} query tokens to send");
            continue;
        }
        let queries = embed_queries(&task, &b)?;
        let (mut qd, mut dd) = nulls(&task, &b, &queries, 26 + i as u64);
        if name != "fixture" {
            all_qd.extend_from_slice(&qd);
            all_dd.extend_from_slice(&dd);
        }
        let at_0 = evaluate(&task, &mut b, &key, 0.0, &queries);
        eprintln!("{name}: at floor 0 {at_0}");
        tasks.insert(
            name.clone(),
            json!({
                "source": task.source, "documents": task.docs.len(), "queries": queries.len(),
                "held": b.held, "failed": b.failed,
                "null_query_doc": percentiles(&mut qd), "null_doc_doc": percentiles(&mut dd), "at_0": at_0,
            }),
        );
        built.push((task, b, queries));
    }
    let _ = fs::remove_dir_all(&scratch);
    if a.dry_run {
        eprintln!("dry run: nothing was sent");
        return Ok(());
    }
    let null_qd = percentiles(&mut all_qd);
    let null_dd = percentiles(&mut all_dd);
    let p99 = null_qd["p99"].as_f64().ok_or("no query samples")?;
    let similar = null_dd["p99"].as_f64().ok_or("no document samples")?;
    // Lower the search floor in 0.01 steps until every BEIR task's nDCG@10 is within NDCG_SLACK of its no-floor run
    // AND the s03 fixture's umx search recall@10 is within FIXTURE_SLACK of its no-floor run (embeddings-expert,
    // s26b). Never below the pooled query-to-document null p50: if the gates need that, stop there and record
    // floor_gate_unmet for the expert to rule.
    let p50 = null_qd["p50"].as_f64().ok_or("no query samples")?;
    let mut floor = p99;
    let mut lowered = Vec::new();
    let mut gate_unmet = false;
    loop {
        let mut failed: Vec<Value> = Vec::new();
        for (task, b, queries) in built.iter_mut() {
            let at = evaluate(task, b, &key, floor, queries);
            eprintln!("{}: at floor {floor} {at}", task.name);
            let (metric, slack) = if task.name == "fixture" {
                ("recall10", FIXTURE_SLACK)
            } else {
                ("ndcg10", NDCG_SLACK)
            };
            let drop =
                tasks[&task.name]["at_0"][metric].as_f64().unwrap() - at[metric].as_f64().unwrap();
            tasks.get_mut(&task.name).unwrap()["at_floor"] = at;
            if drop > slack + 1e-9 {
                failed.push(json!({ "task": task.name, "metric": metric, "drop": round4(drop) }));
            }
        }
        if failed.is_empty() {
            break;
        }
        lowered.push(json!({ "from": round4(floor), "failed": failed }));
        if floor <= p50 + 1e-9 {
            gate_unmet = true;
            break;
        }
        floor = round4((floor - LOWER_STEP).max(p50));
    }
    let (texts_sent, tokens_sent) = *cached.sent.lock().unwrap();
    let entry = json!({
        "run": run_id, "run_date": today(), "umx_commit": a.commit, "model": key.model(), "dims": key.dims(),
        "prompt_version": key.prompt_version(), "instance_kind": instance.kind, "adapter": cached.adapter_id(),
        "deployment": deployment, "smoke": a.max_queries.is_some() || a.max_docs.is_some(),
        "search_floor": round4(floor), "search_floor_p99": round4(p99), "search_floor_lowered": lowered,
        "floor_gate_unmet": gate_unmet, "search_floor_stop_p50": round4(p50),
        "similar_floor": round4(similar), "null_query_doc": null_qd, "null_doc_doc": null_dd,
        "texts_sent": texts_sent, "estimated_tokens_sent": tokens_sent, "tokens_per_min": a.tokens_per_min,
        "tasks": tasks,
    });
    write_result(&a.out, &key.to_string(), entry)
}

/// Record mode (s26b, embeddings-expert): the s03 fixture's every text (whole node, title and note per node, and each
/// query) embedded exactly as production stores it, through this instance and umx's own services, written as
/// `<document|query> TAB <umx TextHash of the text, hex> TAB <base64 of the int8 row>`, sorted. hybrid_recall serves these by
/// the hash of the text it is asked to embed, so a changed text form misses instead of matching the wrong row. Use a
/// fresh --cache so every row is made now, not read from an earlier run.
fn record(
    a: &Args,
    out: &Path,
    embedder: &Arc<Cached>,
    key: &ModelKey,
    backend: Option<&'static str>,
) -> Result<(), String> {
    let task = fixture()?;
    let scratch = std::env::temp_dir().join(format!("umx-floor-record-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let b = build(&task, &scratch, embedder, a.tokens_per_min, false)?;
    let queries = embed_queries(&task, &b)?;
    let _ = fs::remove_dir_all(&scratch);
    let known = embedder.known.lock().unwrap();
    let mut lines: Vec<String> = known
        .iter()
        .map(|((kind, hash), v)| {
            let row: Vec<u8> = quantise(v).into_iter().map(|x| x as u8).collect();
            format!(
                "{}\t{}\t{}",
                if *kind == 0 { "document" } else { "query" },
                // umx's TextHash: the first 16 bytes of the sha256.
                hash[..16]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>(),
                umx_core::vectors::base64_encode(&row)
            )
        })
        .collect();
    lines.sort();
    let (docs, qs) = (
        lines.iter().filter(|l| l.starts_with("document")).count(),
        lines.iter().filter(|l| l.starts_with("query")).count(),
    );
    fs::write(out, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    let (sent, _) = *embedder.sent.lock().unwrap();
    eprintln!(
        "record: {key} through {} (backend {}): {docs} document rows, {qs} query rows ({} queries), {sent} texts embedded now; wrote {}",
        embedder.adapter_id(),
        backend.unwrap_or("hosted"),
        queries.len(),
        out.display()
    );
    Ok(())
}

/// Probe mode: loads every graph folder under `--probe-root`, searches each `alias | phrase` line at every floor,
/// and prints counts only: the probe's line number, then at each floor the hits and how many have a vector leg.
fn probe(a: &Args, probes: &Path, embedder: &Arc<Cached>, key: &ModelKey) -> Result<(), String> {
    let root = a
        .probe_root
        .as_ref()
        .expect("checked")
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let home = std::env::temp_dir().join(format!("umx-floor-probe-{}", std::process::id()));
    let config = Config::at(home.clone(), None).roots([root.to_str().unwrap()]);
    let mut svc = compose(Mode::Real, &config).with_embedder(embedder.clone() as Arc<dyn Embedder>);
    let mut entries: Vec<PathBuf> = fs::read_dir(&root)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("graph.json").exists())
        .collect();
    entries.sort();
    for dir in &entries {
        let alias = dir.file_name().unwrap().to_string_lossy().to_string();
        svc.graph_load(
            &Ctx::new("floor-calibration"),
            dir.to_str().unwrap(),
            Some(&alias),
        )
        .map_err(|p| format!("load {alias}: {}", p.code))?;
    }
    let lines = fs::read_to_string(probes).map_err(|e| e.to_string())?;
    let mut with_row = vec![0usize; a.floors.len()];
    let mut total = 0;
    for (n, line) in lines.lines().enumerate() {
        let mut parts = line.split(" | ");
        let (Some(alias), Some(phrase)) = (parts.next(), parts.next()) else {
            continue;
        };
        let c = ctx(alias.trim());
        let plan = svc
            .search_plan(&c, phrase.trim(), None, 10)
            .map_err(|p| format!("line {}: {}", n + 1, p.code))?;
        let vectors = plan.embed();
        let mut counts = Vec::new();
        for (i, f) in a.floors.iter().enumerate() {
            let mut floors = Floors::default();
            floors.set(key, Some(*f as f32), None);
            svc.set_floors(floors);
            let plan = svc
                .search_plan(&c, phrase.trim(), None, 10)
                .map_err(|p| format!("line {}: {}", n + 1, p.code))?;
            let hits = svc
                .search_finish_with(plan, vectors.clone(), WORD_MAX_DF_SHARE)
                .map_err(|p| format!("line {}: {}", n + 1, p.code))?
                .value
                .hits;
            let v = hits.iter().filter(|h| h.vector.is_some()).count();
            with_row[i] += usize::from(v > 0);
            counts.push(format!("floor {f}: {} hits, {v} vector", hits.len()));
        }
        total += 1;
        println!("probe line {}: {}", n + 1, counts.join("; "));
    }
    for (f, w) in a.floors.iter().zip(&with_row) {
        println!("floor {f}: {w} of {total} probes have at least one vector row");
    }
    let _ = fs::remove_dir_all(&home);
    Ok(())
}

/// Merges `entry` into `<out>/floors.json` under `key` and renders RESULT.md from the whole file.
fn write_result(out: &Path, key: &str, entry: Value) -> Result<(), String> {
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let path = out.join("floors.json");
    let mut all: Value = fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({ "keys": {} }));
    all["about"] = json!(
        "Calibrated similarity floors (workstream 004 s26), written by umx-bench's floor-calibration \
         (crates/umx-bench/src/bin/floor-calibration/main.rs); umx_core::similarity::CALIBRATED is checked against it. \
         No vectors and no corpus text."
    );
    all["keys"][key] = entry;
    let body = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())? + "\n";
    fs::write(&path, body).map_err(|e| e.to_string())?;
    let result = out.join("RESULT.md");
    // Notes written by hand below the marker are kept.
    let notes = fs::read_to_string(&result)
        .ok()
        .and_then(|t| t.find(NOTES).map(|i| t[i..].to_string()))
        .unwrap_or_else(|| format!("{NOTES}\n"));
    fs::write(&result, render(&all) + "\n" + &notes).map_err(|e| e.to_string())?;
    eprintln!("floor-calibration: wrote {} and RESULT.md", path.display());
    Ok(())
}

/// Strength mode (PRD 002 search-10): the strength cuts from the cached rows; see the module.
mod strength;

const NOTES: &str = "<!-- notes: hand-written below this line -->";

fn render(all: &Value) -> String {
    let mut s = String::from(
        "<!-- Generated from floors.json by umx-bench's floor-calibration down to the notes marker. -->\n\
         # Similarity floor calibration (workstream 004 s26)\n\n\
         Method (embeddings-expert ruling 2026-11-11): per key, every text embedded as umx stores it (umx's own \
         adapters and embed loop, int8 rows), cosines as umx scores them. Search floor: p99 of query-to-random-document \
         cosine (200 random non-relevant documents per query, each document's best row). similar_to floor: p99 of \
         document-to-random-document cosine (whole-node rows, 200 per document, up to 3000 documents per task). Gate \
         (s26b): umx search's hybrid nDCG@10 on each BEIR task within 0.01 of the floor at 0 AND the s03 fixture's umx \
         search recall@10 within 0.02 of the floor at 0, else the search floor is lowered in 0.01 steps, never below \
         the query null's p50 (stopping there is recorded as floor_gate_unmet).\n\n## Floors\n\n\
         | key | search floor | search p99 | similar floor | run | times lowered | gate unmet |\n\
         | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    let empty = serde_json::Map::new();
    let keys = all["keys"].as_object().unwrap_or(&empty);
    for (k, e) in keys {
        s += &format!(
            "| `{k}` | {} | {} | {} | {}{} | {} | {} |\n",
            e["search_floor"],
            e["search_floor_p99"],
            e["similar_floor"],
            e["run"].as_str().unwrap_or(""),
            if e["smoke"] == json!(true) {
                " (smoke)"
            } else {
                ""
            },
            e["search_floor_lowered"].as_array().map_or(0, Vec::len),
            if e["floor_gate_unmet"] == json!(true) {
                "floor_gate_unmet"
            } else {
                "no"
            }
        );
    }
    s += "\n## Null distributions (pooled over the BEIR tasks)\n\n\
          | key | query-doc p50 / p95 / p99 | doc-doc p50 / p95 / p99 |\n| --- | --- | --- |\n";
    for (k, e) in keys {
        let p = |v: &Value| format!("{} / {} / {}", v["p50"], v["p95"], v["p99"]);
        s += &format!(
            "| `{k}` | {} | {} |\n",
            p(&e["null_query_doc"]),
            p(&e["null_doc_doc"])
        );
    }
    s += "\n## Labels: umx search at floor 0 and at the search floor\n\n\
          | key | task | nDCG@10 at 0 | at floor | recall@10 at 0 | at floor | vector rows/query at 0 | at floor | \
          queries with a vector row at 0 | at floor |\n\
          | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n";
    for (k, e) in keys {
        for (t, v) in e["tasks"].as_object().into_iter().flatten() {
            let (z, f) = (&v["at_0"], &v["at_floor"]);
            s += &format!(
                "| `{k}` | {t}{} | {} | {} | {} | {} | {} | {} | {} / {} | {} |\n",
                if t == "fixture" {
                    " (gated on recall@10)"
                } else {
                    ""
                },
                z["ndcg10"],
                f["ndcg10"],
                z["recall10"],
                f["recall10"],
                z["vector_rows_per_query"],
                f["vector_rows_per_query"],
                z["queries_with_vector_row"],
                z["queries"],
                f["queries_with_vector_row"]
            );
        }
    }
    s += "\n## Lowering steps (the gates that failed at each floor tried)\n\n| key | floor tried | failed |\n| --- | --- | --- |\n";
    for (k, e) in keys {
        for step in e["search_floor_lowered"].as_array().into_iter().flatten() {
            let failed: Vec<String> = step["failed"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| {
                    format!(
                        "{} {} -{}",
                        f["task"].as_str().unwrap_or(""),
                        f["metric"].as_str().unwrap_or(""),
                        f["drop"]
                    )
                })
                .collect();
            s += &format!("| `{k}` | {} | {} |\n", step["from"], failed.join(", "));
        }
    }
    s += "\n## Provenance\n\n| key | run date | umx commit | adapter | deployment | texts sent | est. tokens sent |\n\
          | --- | --- | --- | --- | --- | --- | --- |\n";
    for (k, e) in keys {
        s += &format!(
            "| `{k}` | {} | {} | {} | {} | {} | {} |\n",
            e["run_date"].as_str().unwrap_or(""),
            e["umx_commit"].as_str().unwrap_or(""),
            e["adapter"].as_str().unwrap_or(""),
            e["deployment"].as_str().unwrap_or("-"),
            e["texts_sent"],
            e["estimated_tokens_sent"]
        );
    }
    s += "\nDatasets (fetched at run time, never committed):\n\n";
    let mut seen = BTreeSet::new();
    for e in keys.values() {
        for (t, v) in e["tasks"].as_object().into_iter().flatten() {
            if let Some(sha) = v["source"]["sha256"].as_str()
                && seen.insert(t.clone())
            {
                s += &format!(
                    "- {t}: {} sha256 `{sha}`\n",
                    v["source"]["url"].as_str().unwrap_or("")
                );
            }
        }
    }
    s += &strength::render(all);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    /// R206-B1: --record with --dry-run is refused before any config is read, embedder built or file written.
    #[test]
    fn a_record_refuses_dry_run_before_any_provider_work() {
        let dir = std::env::temp_dir().join(format!("fc-record-dry-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let out = dir.join("rows.tsv");
        // The config home does not exist: had parsing passed, the run would fail later naming the missing instance,
        // never with the dry-run refusal, and the cache folder would have been made.
        let err = calibrate_with(argv(&[
            "--embedder",
            "azure",
            "--config-home",
            dir.join("no-home").to_str().unwrap(),
            "--cache",
            dir.join("cache").to_str().unwrap(),
            "--record",
            out.to_str().unwrap(),
            "--dry-run",
        ]))
        .unwrap_err();
        assert!(err.contains("--record has no --dry-run"), "{err}");
        assert!(!out.exists(), "no rows file written");
        assert!(
            !dir.exists(),
            "nothing made at all (no cache, no scratch graph)"
        );
        // --deployment-check --repeat is capped before any config, provider or send (security, reviewer N1).
        let check = |n: &str| {
            args(argv(&[
                "--strength",
                "--deployment-check",
                "--repeat",
                n,
                "--embedder",
                "large",
                "--cache",
                "/c",
                "--out",
                "/o",
                "--commit",
                "x",
            ]))
        };
        assert!(
            check("4")
                .err()
                .is_some_and(|e| e.contains("at most 3 passes"))
        );
        assert!(check("3").is_ok());
        // The same record without --dry-run parses (it is the embed that needs a real instance).
        assert!(
            args(argv(&[
                "--embedder",
                "azure",
                "--cache",
                "/c",
                "--record",
                "/o"
            ]))
            .is_ok()
        );
    }
}
