//! Write-path measurements (workstream 003 s03 follow-ups, plan 028). `node add` writes nodes.jsonl alone, a
//! one-file set with no marker; with `--parent` it writes nodes.jsonl and edges.jsonl as one two-file set (temps,
//! marker, renames, recovery hashes). Kept apart from main.rs so the storage steps rebase cheaply.

use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use umx_compose::{Config, Mode, compose};
use umx_core::ports::{EmbedError, Embedded, Embedder, Locality, ModelKey, TextKind};
use umx_services::{Ctx, NodeAddRequest};

/// A local model that is never called (ws004 s06 B, architect F1/F2). With it composed, a write also checks the
/// node's pending vectors and a reembed plans every node, so their cost shows; `embed` failing is a bench bug.
struct NoCalls;

impl Embedder for NoCalls {
    fn credential(&self) -> Option<umx_core::ports::Credential> {
        None
    }
    fn embed(&self, _texts: &[String], _kind: TextKind) -> Result<Embedded, EmbedError> {
        Err(EmbedError::Config("the bench never embeds".into()))
    }
    fn key(&self) -> ModelKey {
        ModelKey::new("bench", 8).expect("a valid key")
    }
    fn concurrency_ceiling(&self) -> usize {
        1
    }
    fn max_input_tokens(&self) -> usize {
        512
    }
    fn locality(&self) -> Locality {
        Locality::Local
    }
    fn ready(&self) -> Result<(), EmbedError> {
        Ok(())
    }
    fn adapter_id(&self) -> &'static str {
        "bench.no-calls"
    }
}

/// In-process adds are capped: at 100k rich each two-file add rewrites about 150 MB.
const IN_PROCESS_ADDS: usize = 20;

/// The first node's id, read from the first line of nodes.jsonl (the fixture writes nodes in id order).
pub fn first_node_id(nodes_jsonl: &[u8]) -> Result<String, String> {
    let line = nodes_jsonl
        .split(|b| *b == b'\n')
        .next()
        .ok_or("the fixture has no nodes")?;
    let v: Value = serde_json::from_slice(line).map_err(|e| format!("first node line: {e}"))?;
    v["id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "first node line has no id".to_owned())
}

fn ms(t: Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 1e6).round() / 1e3
}

/// p50, p95 and max of `samples` in ms.
fn stats(mut samples: Vec<f64>) -> Value {
    let raw = samples.clone();
    samples.sort_by(f64::total_cmp);
    let at = |p: f64| samples[((samples.len() - 1) as f64 * p).round() as usize];
    json!({ "p50": at(0.5), "p95": at(0.95), "max": samples[samples.len() - 1], "n": samples.len(), "samples": raw })
}

/// Copies the three graph files to `to`, so the adds below never change the graph the other parts measure.
pub(super) fn copy_graph(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir(to).map_err(|e| format!("make {}: {e}", to.display()))?;
    for name in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
        std::fs::copy(from.join(name), to.join(name)).map_err(|e| format!("copy {name}: {e}"))?;
    }
    Ok(())
}

/// In process, through the composed services and the real fs adapter: `node add` without and with a parent,
/// interleaved so machine load hits both alike. Runs on a copy of the graph folder.
pub fn in_process(adds: usize, work: &Path, graph: &Path, parent: &str) -> Result<Value, String> {
    let copy = work.join("gw");
    copy_graph(graph, &copy)?;
    let config = Config::at(work.join("writes-home"), None).roots([work.to_str().unwrap()]);
    let mut svc = compose(Mode::Real, &config);
    let ctx = Ctx::new("umx-bench");
    svc.graph_load(&ctx, copy.to_str().unwrap(), None)
        .map_err(|p| format!("load the copy: {}", p.code))?;
    let n = adds.clamp(1, IN_PROCESS_ADDS);
    let (mut one, mut two) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for i in 0..n {
        let add = |title: String, parent: Option<String>| NodeAddRequest {
            kind: "problem".into(),
            title,
            parent,
            ..Default::default()
        };
        let t = Instant::now();
        svc.node_add(&ctx, add(format!("Bench write {i}"), None))
            .map_err(|p| format!("node add: {}", p.code))?;
        one.push(ms(t));
        let t = Instant::now();
        svc.node_add(
            &ctx,
            add(format!("Bench child {i}"), Some(parent.to_owned())),
        )
        .map_err(|p| format!("node add --parent: {}", p.code))?;
        two.push(ms(t));
    }
    // The same adds with a model composed: each also reads the node's shard to count its pending vectors (F2).
    let modelled = work.join("gm");
    copy_graph(graph, &modelled)?;
    let mut with_model = compose(Mode::Real, &config).with_embedder(Arc::new(NoCalls));
    with_model
        .graph_load(&ctx, modelled.to_str().unwrap(), None)
        .map_err(|p| format!("load the modelled copy: {}", p.code))?;
    let mut three = Vec::with_capacity(n);
    for i in 0..n {
        let t = Instant::now();
        with_model
            .node_add(
                &ctx,
                NodeAddRequest {
                    kind: "problem".into(),
                    title: format!("Bench modelled {i}"),
                    ..Default::default()
                },
            )
            .map_err(|p| format!("node add with a model: {}", p.code))?;
        three.push(ms(t));
    }
    // `embeddings reembed --dry-run`: plans every node and reads every shard of the model; calls nothing (F1).
    let mut dry = Vec::with_capacity(3);
    let mut cost = Value::Null;
    for _ in 0..3 {
        let t = Instant::now();
        let plan = with_model
            .reembed_plan(&ctx, None)
            .map_err(|p| format!("reembed --dry-run: {}", p.code))?;
        dry.push(ms(t));
        cost = json!({ "nodes": plan.cost.nodes, "texts": plan.cost.texts, "estimated_tokens": plan.cost.estimated_tokens });
    }
    Ok(json!({
        "add_ms": stats(one), "add_parent_ms": stats(two), "add_with_model_ms": stats(three),
        "reembed_dry_run_ms": stats(dry), "reembed_dry_run_cost": cost,
    }))
}

/// Through the CLI: `node add --parent`, timed by `run`, which runs one umx command and returns its wall time.
pub fn cli_parent_adds(
    adds: usize,
    parent: &str,
    mut run: impl FnMut(&[&str]) -> Result<f64, String>,
) -> Result<Value, String> {
    let mut took = Vec::with_capacity(adds.max(1));
    for i in 0..adds.max(1) {
        let title = format!("Bench child {i}");
        took.push(run(&[
            "node", "add", "--type", "problem", "--title", &title, "--parent", parent,
        ])?);
    }
    Ok(stats(took))
}
