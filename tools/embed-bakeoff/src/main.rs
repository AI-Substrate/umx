//! Workstream 004 s03: the Rust path for the embedding bake-off. ONNX Runtime (ort) over the
//! onnx-community exports, our own tokenisation, padding and pooling: the path s04 would ship.
//! Commands: check, quality (old 300-node fixture), parity (against beir_eval.py vectors), speed.
//! See README.md for the runs.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use ort::session::{Session, SessionInputValue};
use ort::value::{Tensor, ValueType};
use serde::Deserialize;
use serde_json::{Value, json};
use tokenizers::{Tokenizer, TruncationParams};

/// 2048 for every model (expert ruling); `--max-length` overrides it.
static MAX_LENGTH: AtomicUsize = AtomicUsize::new(2048);
/// Texts per run call in speed runs; the shipped config is 16 (`--batch` overrides it).
static BATCH: AtomicUsize = AtomicUsize::new(16);
const THREADS: usize = 4;
const CUT: usize = 256;

#[derive(Clone, Copy, PartialEq)]
enum Pool {
    /// The graph already pools and returns a 2-D output.
    InGraph,
    Mean,
    LastToken,
}

struct Spec {
    name: &'static str,
    repo: &'static str,
    commit: &'static str,
    output: &'static str,
    pool: Pool,
    query: fn(&str) -> String,
    doc: fn(&str, &str) -> String,
}

const WEB: &str =
    "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery:";

/// Prompts and pooling from each model card (see beir_eval.py, which uses the same forms).
fn spec(name: &str) -> Spec {
    match name {
        "qwen3-embedding-0.6b" => Spec {
            name: "qwen3-embedding-0.6b",
            repo: "onnx-community/Qwen3-Embedding-0.6B-ONNX",
            commit: "c25a394dd583836952667c12f008335071b3f43d",
            output: "last_hidden_state",
            pool: Pool::LastToken,
            query: |q| format!("{WEB}{q}"),
            doc: |t, x| format!("{t} {x}").trim().to_string(),
        },
        "harrier-oss-v1-0.6b" => Spec {
            name: "harrier-oss-v1-0.6b",
            repo: "onnx-community/harrier-oss-v1-0.6b-ONNX",
            commit: "e4daffa011e666dcd2ff2a3c6c05084090ac314d",
            output: "sentence_embedding",
            pool: Pool::InGraph,
            query: |q| format!("{WEB} {q}"),
            doc: |t, x| format!("{t} {x}").trim().to_string(),
        },
        "voyage-4-nano" => Spec {
            name: "voyage-4-nano",
            repo: "onnx-community/voyage-4-nano-ONNX",
            commit: "fb6f32cceb63b6eac92e1fdc08e00d03c41cdf5a",
            output: "last_hidden_state",
            pool: Pool::Mean,
            query: |q| format!("Represent the query for retrieving supporting documents: {q}"),
            doc: |t, x| {
                format!(
                    "Represent the document for retrieval: {}",
                    format!("{t} {x}").trim()
                )
            },
        },
        "embeddinggemma-2" => Spec {
            name: "embeddinggemma-2",
            repo: "onnx-community/embeddinggemma-2-ONNX",
            commit: "daa72c51243991dfcaf9f9137d2c573d8f7790c0",
            output: "sentence_embedding",
            pool: Pool::InGraph,
            query: |q| format!("task: search result | query: {q}"),
            doc: |t, x| {
                let t = if t.trim().is_empty() {
                    "none"
                } else {
                    t.trim()
                };
                format!("title: {t} | text: {x}")
            },
        },
        other => die(&format!(
            "unknown model {other:?}; use qwen3-embedding-0.6b, harrier-oss-v1-0.6b, voyage-4-nano or embeddinggemma-2"
        )),
    }
}

fn die(msg: &str) -> ! {
    eprintln!("embed-bakeoff: {msg}");
    std::process::exit(2)
}

fn model_dir(s: &Spec) -> PathBuf {
    let home = std::env::var_os("HOME").unwrap_or_else(|| die("HOME is not set"));
    Path::new(&home)
        .join(".cache/umx/models")
        .join(s.repo.replace('/', "--"))
        .join(s.commit)
}

struct Model {
    spec: Spec,
    session: Session,
    tok: Tokenizer,
}

impl Model {
    fn load(name: &str, threads: usize) -> Model {
        let spec = spec(name);
        let dir = model_dir(&spec);
        let onnx = dir.join("onnx/model.onnx");
        if !onnx.exists() {
            die(&format!(
                "{}: missing; run tools/embed-bakeoff/fetch.sh first",
                onnx.display()
            ));
        }
        // From the file, so ORT finds the external weight files beside it.
        let session = Session::builder()
            .map_err(|e| e.to_string())
            .and_then(|b| b.with_intra_threads(threads).map_err(|e| e.to_string()))
            .and_then(|mut b| b.commit_from_file(&onnx).map_err(|e| e.to_string()))
            .unwrap_or_else(|e| die(&format!("load {}: {e}", spec.name)));
        let mut tok = Tokenizer::from_file(dir.join("tokenizer.json"))
            .unwrap_or_else(|e| die(&format!("tokenizer {}: {e}", spec.name)));
        tok.with_padding(None);
        tok.with_truncation(Some(TruncationParams {
            max_length: MAX_LENGTH.load(Ordering::Relaxed),
            ..Default::default()
        }))
        .unwrap_or_else(|e| die(&format!("truncation: {e}")));
        Model { spec, session, tok }
    }

    fn token_len(&self, text: &str) -> usize {
        let mut t = self.tok.clone();
        t.with_truncation(None).ok();
        t.encode(text, true)
            .unwrap_or_else(|e| die(&format!("tokenize: {e}")))
            .len()
    }

    /// Native-width, L2-normalised vectors, one per text, in order. Right padding; pooling and
    /// last-token lookup use the attention mask.
    fn embed(&mut self, texts: &[String]) -> Vec<Vec<f32>> {
        let encs = self
            .tok
            .encode_batch(texts.to_vec(), true)
            .unwrap_or_else(|e| die(&format!("tokenize: {e}")));
        let (b, l) = (encs.len(), encs.iter().map(|e| e.len()).max().unwrap_or(0));
        let (mut ids, mut mask, mut pos) =
            (vec![0i64; b * l], vec![0i64; b * l], vec![0i64; b * l]);
        for (r, e) in encs.iter().enumerate() {
            for (c, &id) in e.get_ids().iter().enumerate() {
                ids[r * l + c] = id as i64;
                mask[r * l + c] = 1;
                pos[r * l + c] = c as i64;
            }
        }
        let shape = [b as i64, l as i64];
        let mut inputs: Vec<(String, SessionInputValue)> = Vec::new();
        for input in self.session.inputs() {
            let name = input.name().to_string();
            let v: SessionInputValue = match name.as_str() {
                "input_ids" => tensor_i64(shape, ids.clone()),
                "attention_mask" => tensor_i64(shape, mask.clone()),
                "position_ids" => tensor_i64(shape, pos.clone()),
                "token_type_ids" => tensor_i64(shape, vec![0; b * l]),
                // Empty KV cache (past length 0) and empty media features (text only).
                n if n.starts_with("past_key_values") || n.ends_with("_features") => {
                    let ValueType::Tensor { shape: s, .. } = input.dtype() else {
                        die(&format!("input {n} is not a tensor"))
                    };
                    let dims: Vec<i64> = s
                        .iter()
                        .enumerate()
                        .map(|(i, &d)| match (n.starts_with("past"), i, d) {
                            (true, 0, _) => b as i64,
                            (true, 2, _) => 0,
                            (false, 0, _) => 0,
                            (_, _, d) => d,
                        })
                        .collect();
                    Tensor::from_array((dims, Vec::<f32>::new()))
                        .unwrap_or_else(|e| die(&format!("{n}: {e}")))
                        .into()
                }
                n => die(&format!("{}: no rule for input {n}", self.spec.name)),
            };
            inputs.push((name, v));
        }
        let out = self
            .session
            .run(inputs)
            .unwrap_or_else(|e| die(&format!("run {}: {e}", self.spec.name)));
        let (s, data) = out[self.spec.output]
            .try_extract_tensor::<f32>()
            .unwrap_or_else(|e| die(&format!("output {}: {e}", self.spec.output)));
        let dims: Vec<usize> = s.iter().map(|&d| d as usize).collect();
        (0..b)
            .map(|r| {
                let v: Vec<f32> = match (self.spec.pool, dims.len()) {
                    (Pool::InGraph, 2) => data[r * dims[1]..(r + 1) * dims[1]].to_vec(),
                    (Pool::Mean, 3) => {
                        let d = dims[2];
                        let mut acc = vec![0f32; d];
                        let n = encs[r].len();
                        for c in 0..n {
                            let row = &data[(r * l + c) * d..(r * l + c + 1) * d];
                            acc.iter_mut().zip(row).for_each(|(a, x)| *a += x);
                        }
                        acc.iter_mut().for_each(|a| *a /= n as f32);
                        acc
                    }
                    (Pool::LastToken, 3) => {
                        let d = dims[2];
                        let c = encs[r].len() - 1;
                        data[(r * l + c) * d..(r * l + c + 1) * d].to_vec()
                    }
                    _ => die(&format!("unexpected output shape {dims:?}")),
                };
                normalise(v)
            })
            .collect()
    }
}

fn tensor_i64(shape: [i64; 2], v: Vec<i64>) -> SessionInputValue<'static> {
    Tensor::from_array((shape, v))
        .unwrap_or_else(|e| die(&format!("tensor: {e}")))
        .into()
}

fn normalise(mut x: Vec<f32>) -> Vec<f32> {
    let n = x.iter().map(|a| a * a).sum::<f32>().sqrt().max(1e-12);
    x.iter_mut().for_each(|a| *a /= n);
    x
}

fn cut(v: &[f32], dims: usize) -> Vec<f32> {
    normalise(v[..dims.min(v.len())].to_vec())
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn embed_all(m: &mut Model, texts: &[String], batch: usize) -> Vec<Vec<f32>> {
    texts.chunks(batch).flat_map(|c| m.embed(c)).collect()
}

#[derive(Deserialize)]
struct Node {
    id: String,
    title: String,
    note: String,
}

#[derive(Deserialize, Clone)]
struct Query {
    text: String,
    kind: String,
    relevant: Vec<String>,
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/project/benchmarks/embed-bakeoff")
}

fn jsonl<T: for<'de> Deserialize<'de>>(f: &str) -> Vec<T> {
    let p = fixture_dir().join(f);
    fs::read_to_string(&p)
        .unwrap_or_else(|e| die(&format!("{}: {e}", p.display())))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .map(|(i, l)| {
            serde_json::from_str(l).unwrap_or_else(|e| die(&format!("{f}:{}: {e}", i + 1)))
        })
        .collect()
}

/// Whole-node Markdown key-value text (as in the first runs), in the model's document form.
fn node_doc(s: &Spec, n: &Node) -> String {
    (s.doc)(&n.title, &format!("title: {}\nnote: {}", n.title, n.note))
}

fn pct(xs: &[f64], p: f64) -> f64 {
    if xs.is_empty() {
        return f64::NAN;
    }
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((p / 100.0) * (v.len() - 1) as f64).round() as usize]
}

fn peak_rss_mb() -> f64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    // macOS reports bytes, Linux kilobytes.
    let bytes = if cfg!(target_os = "macos") {
        ru.ru_maxrss as f64
    } else {
        ru.ru_maxrss as f64 * 1024.0
    };
    bytes / 1_048_576.0
}

/// Resident set size now (ps), as against getrusage's lifetime peak.
fn rss_now_mb() -> f64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<f64>()
                .unwrap_or(f64::NAN)
        })
        .unwrap_or(f64::NAN)
        / 1024.0
}

fn load_avg() -> f64 {
    let mut l = [0f64; 3];
    unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
    l[0]
}

fn metrics(docs: &[Vec<f32>], ids: &[String], queries: &[Query], qv: &[Vec<f32>]) -> Value {
    let (mut r10, mut mrr) = (0.0, 0.0);
    let mut by_kind: BTreeMap<String, (f64, f64, usize)> = BTreeMap::new();
    for (q, v) in queries.iter().zip(qv) {
        let rel: HashSet<&str> = q.relevant.iter().map(String::as_str).collect();
        let mut scored: Vec<(f32, &str)> = docs
            .iter()
            .zip(ids)
            .map(|(d, id)| (dot(v, d), id.as_str()))
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        let h10 = scored[..10]
            .iter()
            .filter(|(_, id)| rel.contains(id))
            .count() as f64
            / rel.len() as f64;
        let rr = match scored.iter().position(|(_, id)| rel.contains(id)) {
            Some(p) if p < 10 => 1.0 / (p + 1) as f64,
            _ => 0.0,
        };
        r10 += h10;
        mrr += rr;
        let e = by_kind.entry(q.kind.clone()).or_default();
        e.0 += h10;
        e.1 += rr;
        e.2 += 1;
    }
    let n = queries.len() as f64;
    json!({"recall10": r10 / n, "mrr10": mrr / n,
        "by_kind": by_kind.iter().map(|(k, (r, m, c))| (k.clone(), json!({"n": c, "recall10": r / *c as f64, "mrr10": m / *c as f64}))).collect::<BTreeMap<_, _>>()})
}

fn quality(name: &str) -> Value {
    let nodes: Vec<Node> = jsonl("nodes.jsonl");
    let queries: Vec<Query> = jsonl("queries.jsonl");
    let mut m = Model::load(name, THREADS);
    let docs: Vec<String> = nodes.iter().map(|n| node_doc(&m.spec, n)).collect();
    let qs: Vec<String> = queries.iter().map(|q| (m.spec.query)(&q.text)).collect();
    let raw_d = embed_all(&mut m, &docs, 1);
    let raw_q = embed_all(&mut m, &qs, 1);
    let ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
    let native = raw_d[0].len();
    let mut by = serde_json::Map::new();
    for dims in [CUT, native] {
        let d: Vec<_> = raw_d.iter().map(|v| cut(v, dims)).collect();
        let q: Vec<_> = raw_q.iter().map(|v| cut(v, dims)).collect();
        by.insert(dims.to_string(), metrics(&d, &ids, &queries, &q));
    }
    json!({"model": m.spec.name, "repo": m.spec.repo, "commit": m.spec.commit, "native_dims": native,
        "truncated": docs.iter().filter(|d| m.token_len(d) > MAX_LENGTH.load(Ordering::Relaxed)).count(), "dims": by})
}

fn check(name: &str) -> Value {
    let mut m = Model::load(name, THREADS);
    let words: Vec<String> = ["cat", "kitten", "carburetor"]
        .iter()
        .map(|w| (m.spec.query)(w))
        .collect();
    let v: Vec<Vec<f32>> = embed_all(&mut m, &words, 1)
        .iter()
        .map(|x| cut(x, CUT))
        .collect();
    let sample = (m.spec.query)("how long may soup cool");
    let enc = m.tok.encode(sample.as_str(), true).unwrap();
    let toks = enc.get_tokens();
    // Batch invariance: texts of different lengths in one right-padded batch against one by one.
    let nodes: Vec<Node> = jsonl("nodes.jsonl");
    let texts: Vec<String> = [0, 5, 37, 120]
        .iter()
        .map(|&i| node_doc(&m.spec, &nodes[i]))
        .collect();
    let single = embed_all(&mut m, &texts, 1);
    let batched = embed_all(&mut m, &texts, texts.len());
    let min_cos = single
        .iter()
        .zip(&batched)
        .map(|(a, b)| dot(&cut(a, CUT), &cut(b, CUT)))
        .fold(1f32, f32::min);
    json!({"model": m.spec.name, "cos_cat_kitten": dot(&v[0], &v[1]), "cos_cat_carburetor": dot(&v[0], &v[2]),
        "pass": dot(&v[0], &v[1]) > dot(&v[0], &v[2]), "last_tokens": toks[toks.len().saturating_sub(3)..],
        "batch_invariance_min_cos_256": min_cos})
}

#[derive(Deserialize)]
struct Reference {
    docs: Vec<String>,
    queries: Vec<String>,
    doc_vectors_256: Vec<Vec<f32>>,
    query_vectors_256: Vec<Vec<f32>>,
}

/// Cosine at 256 between this path and beir_eval.py's sentence-transformers vectors, same texts.
fn parity(name: &str) -> Value {
    let p = fixture_dir().join(format!("results/parity-ref-{name}.json"));
    let r: Reference = serde_json::from_str(
        &fs::read_to_string(&p).unwrap_or_else(|e| die(&format!("{}: {e}", p.display()))),
    )
    .unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
    let batch = BATCH.load(Ordering::Relaxed);
    let mut m = Model::load(name, THREADS);
    let mut kind = |texts: &[String], refs: &[Vec<f32>]| {
        let cos: Vec<f64> = embed_all(&mut m, texts, batch)
            .iter()
            .zip(refs)
            .map(|(a, b)| dot(&cut(a, CUT), b) as f64)
            .collect();
        json!({"n": cos.len(), "min": cos.iter().cloned().fold(1.0, f64::min), "mean": cos.iter().sum::<f64>() / cos.len() as f64})
    };
    let d = kind(&r.docs, &r.doc_vectors_256);
    let q = kind(&r.queries, &r.query_vectors_256);
    let pass = d["min"].as_f64().unwrap() >= 0.999 && q["min"].as_f64().unwrap() >= 0.999;
    json!({"model": name, "batch": batch, "docs": d, "queries": q, "pass": pass})
}

/// Texts of `n` tokens cut from the fixture's whole-node texts run together, in document form.
fn speed_corpus(m: &Model, n: usize, count: usize) -> Vec<String> {
    let nodes: Vec<Node> = jsonl("nodes.jsonl");
    let all = nodes
        .iter()
        .map(|x| format!("title: {}\nnote: {}", x.title, x.note))
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut t = m.tok.clone();
    t.with_truncation(None).ok();
    let ids = t.encode(all.as_str(), false).unwrap().get_ids().to_vec();
    let body = n - m.token_len(&(m.spec.doc)("", ""));
    ids.chunks(body)
        .filter(|c| c.len() == body)
        .take(count)
        .map(|c| (m.spec.doc)("", &t.decode(c, true).unwrap()))
        .collect()
}

fn speed(name: &str) -> Value {
    let batch = BATCH.load(Ordering::Relaxed);
    let t = Instant::now();
    let first = Model::load(name, THREADS);
    let first_load = t.elapsed().as_secs_f64();
    let corpus = speed_corpus(&first, 300, 160);
    let corpus_tokens =
        corpus.iter().map(|c| first.token_len(c)).sum::<usize>() as f64 / corpus.len() as f64;
    drop(first);
    let t = Instant::now();
    let mut m = Model::load(name, THREADS);
    let warm_load = t.elapsed().as_secs_f64();
    let rss_after_load = rss_now_mb();
    embed_all(&mut m, &corpus[..batch.min(corpus.len())], batch);
    let t = Instant::now();
    embed_all(&mut m, &corpus, batch);
    let docs_per_s = corpus.len() as f64 / t.elapsed().as_secs_f64();
    let rss_after_run = rss_now_mb();
    let queries: Vec<Query> = jsonl("queries.jsonl");
    let qs: Vec<String> = queries.iter().map(|q| (m.spec.query)(&q.text)).collect();
    embed_all(&mut m, &qs[..3], 1);
    let lat: Vec<f64> = qs
        .iter()
        .map(|q| {
            let t = Instant::now();
            m.embed(std::slice::from_ref(q));
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    json!({"model": m.spec.name, "sessions": 1, "threads": THREADS, "batch": batch,
        "first_load_s": first_load, "warm_load_s": warm_load, "corpus_texts": corpus.len(), "corpus_mean_tokens": corpus_tokens,
        "docs_per_s": docs_per_s, "query_ms": {"p50": pct(&lat, 50.0), "p95": pct(&lat, 95.0), "n": lat.len()},
        "rss_after_load_mb": rss_after_load, "rss_after_run_mb": rss_after_run, "peak_rss_mb": peak_rss_mb()})
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| {
        args.get(i)
            .map(String::as_str)
            .unwrap_or_else(|| die(USAGE))
    };
    for (flag, slot) in [("--max-length", &MAX_LENGTH), ("--batch", &BATCH)] {
        if let Some(i) = args.iter().position(|a| a == flag) {
            slot.store(
                arg(i + 1)
                    .parse()
                    .unwrap_or_else(|_| die(&format!("{flag} takes a number"))),
                Ordering::Relaxed,
            );
        }
    }
    let load_at_start = load_avg();
    let mut out = match arg(0) {
        "check" => check(arg(1)),
        "quality" => quality(arg(1)),
        "parity" => parity(arg(1)),
        "speed" => speed(arg(1)),
        _ => die(USAGE),
    };
    out["max_length"] = json!(MAX_LENGTH.load(Ordering::Relaxed));
    out["load_avg_1m_at_start"] = json!(load_at_start);
    out["load_avg_1m_at_end"] = json!(load_avg());
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}

const USAGE: &str =
    "usage: embed-bakeoff <check|quality|parity|speed> <model> [--batch N] [--max-length N]";
