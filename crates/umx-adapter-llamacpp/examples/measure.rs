//! Speed and memory of the adapter in its default config (1 session x 4 threads, batch 16, width 256, Metal on
//! macOS), the way the s03 bake-off and the GPU spike measured it: 125 texts of 300 tokens cut from the synthetic
//! fixture (a JSON list of strings), warm-up first, then 50 queries one at a time. Run under `/usr/bin/time -l` for
//! peak RSS:
//!
//!     cargo run --release -p umx-adapter-llamacpp --example measure -- <models dir> <corpus.json> <queries.jsonl>
//!         [gpu|cpu] [batch size] [number of texts]

use std::path::PathBuf;
use std::time::Instant;

use umx_adapter_llamacpp::{Execution, LlamaConfig, LlamaEmbedder, ModelSpec};
use umx_core::ports::{Embedder, TextKind};

/// User + system CPU seconds of this process so far, from ps.
fn cpu_s() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .split(':')
        .fold(0.0, |acc, part| {
            acc * 60.0 + part.parse::<f64>().unwrap_or(0.0)
        })
}

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .unwrap_or(0.0)
        / 1024.0
}

fn pct(xs: &[f64], p: f64) -> f64 {
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((p / 100.0) * (v.len() - 1) as f64).round() as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (models, corpus, queries) = (&args[0], &args[1], &args[2]);
    let mut config = LlamaConfig::new(ModelSpec::embeddinggemma_2(), PathBuf::from(models));
    if args.get(3).map(String::as_str) == Some("cpu") {
        config.execution = Execution::Cpu;
    }
    if let Some(b) = args.get(4) {
        config.batch_size = b.parse().unwrap();
    }

    let t = Instant::now();
    LlamaEmbedder::new(config.clone()).unwrap().ready().unwrap();
    let first_load = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let e = LlamaEmbedder::new(config.clone()).unwrap();
    e.ready().unwrap();
    let warm_load = t.elapsed().as_secs_f64();
    let rss_after_load = rss_mb();

    let mut docs: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string(corpus).unwrap()).unwrap();
    if let Some(n) = args.get(5) {
        docs.truncate(n.parse().unwrap());
    }
    e.embed(&docs[..docs.len().min(16)], TextKind::Document)
        .unwrap();
    let (t, c0) = (Instant::now(), cpu_s());
    let out = e.embed(&docs, TextKind::Document).unwrap();
    let docs_per_s = docs.len() as f64 / t.elapsed().as_secs_f64();
    let cpu_s_per_1000_docs = (cpu_s() - c0) * 1000.0 / docs.len() as f64;
    let rss_after_run = rss_mb();

    let qs: Vec<String> = std::fs::read_to_string(queries)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(if docs.len() < 50 { 5 } else { 50 })
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["text"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    e.embed(&qs[..3], TextKind::Query).unwrap();
    let lat: Vec<f64> = qs
        .iter()
        .map(|q| {
            let t = Instant::now();
            e.embed(std::slice::from_ref(q), TextKind::Query).unwrap();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();

    println!(
        "{}",
        serde_json::json!({
            "key": e.key().to_string(), "execution": format!("{:?}", config.execution), "sessions": config.sessions,
            "threads": config.threads, "batch": config.batch_size, "corpus_texts": docs.len(),
            "truncated": out.truncated.len(), "first_load_s": first_load, "warm_load_s": warm_load,
            "docs_per_s": docs_per_s, "cpu_s_per_1000_docs": cpu_s_per_1000_docs,
            "query_ms": {"p50": pct(&lat, 50.0), "p95": pct(&lat, 95.0), "n": lat.len()},
            "rss_after_load_mb": rss_after_load, "rss_after_run_mb": rss_after_run,
        })
    );
}
