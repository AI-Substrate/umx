//! The parity set's vectors (the s03 reference's 20 documents and 20 queries, at width 256) from one backend, so two
//! backends can be compared directly (CUDA against Metal and the CPU, ws004 s19). Prints JSON: the backend, the key,
//! and the document and query vectors in reference order.
//!
//!     cargo run --release -p umx-adapter-llamacpp --example vectors -- <models dir> [gpu|cpu] > vectors-<backend>.json

use std::path::PathBuf;

use umx_adapter_llamacpp::{Execution, LlamaConfig, LlamaEmbedder, ModelSpec};
use umx_core::ports::{Embedder, TextKind};

const QUERY_PREFIX: &str = "task: search result | query: ";

fn strings(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut config = LlamaConfig::new(ModelSpec::embeddinggemma_2(), PathBuf::from(&args[0]));
    if args.get(1).map(String::as_str) == Some("cpu") {
        config.execution = Execution::Cpu;
    }
    let reference: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/parity-ref-embeddinggemma-2.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let docs = strings(&reference["docs"]);
    let queries: Vec<String> = strings(&reference["queries"])
        .iter()
        .map(|q| q.strip_prefix(QUERY_PREFIX).unwrap().to_string())
        .collect();
    let e = LlamaEmbedder::new(config).unwrap();
    let docs = e.embed(&docs, TextKind::Document).unwrap().vectors;
    let queries = e.embed(&queries, TextKind::Query).unwrap().vectors;
    println!(
        "{}",
        serde_json::json!({
            "backend": e.backend().as_str(), "key": e.key().to_string(), "docs": docs, "queries": queries,
        })
    );
}
