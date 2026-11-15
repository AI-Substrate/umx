//! `umx batch` measurements (workstream 003 s07, req-88; graph-storage-expert's acceptance table). Two parts:
//! through the composed services and the real fs adapter on a copy of the graph folder (the path a daemon runs),
//! and umx-graph's batch scope on its own (apply, commit and rollback, no near-duplicate check and no disk).

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;
use umx_compose::{Config, Mode, compose};
use umx_core::{Edge, Id, Node};
use umx_graph::Graph;
use umx_services::{BatchLine, BatchOp, BatchRequest, Ctx, NodeAddRequest};

use crate::fixture::Fixture;
use crate::storage;

/// Ops per measured batch (req-88's done_when).
const OPS: usize = 1000;

fn ms(t: Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 1e6).round() / 1e3
}

fn median(mut v: Vec<f64>) -> Value {
    v.sort_by(f64::total_cmp);
    json!({ "median": v[v.len() / 2], "min": v[0], "runs": v.len() })
}

fn add_line(line: usize, title: String, parent: Option<&str>) -> BatchLine {
    BatchLine {
        line: line as u32,
        op: BatchOp::NodeAdd {
            id: None,
            created: None,
            ref_name: None,
            req: NodeAddRequest {
                kind: "problem".into(),
                title,
                parent: parent.map(str::to_owned),
                ..Default::default()
            },
            parent_ref: None,
        },
    }
}

fn copy_graph(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir(to).map_err(|e| format!("make {}: {e}", to.display()))?;
    for name in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
        std::fs::copy(from.join(name), to.join(name)).map_err(|e| format!("copy {name}: {e}"))?;
    }
    Ok(())
}

/// Through the composed services on a copy of the graph: a dry run and a real run of 1,000 adds (near-duplicate
/// check on, as the verb runs it), 1,000 adds each with a parent edge, and one-op batches against `node add`.
/// Each batch call holds the daemon's writer for its whole time, so these are also the longest writer holds.
pub fn services(work: &Path, graph: &Path, parent: &str) -> Result<Value, String> {
    let copy = work.join("gb");
    copy_graph(graph, &copy)?;
    let config = Config::at(work.join("batch-home"), None).roots([work.to_str().unwrap()]);
    let mut svc = compose(Mode::Real, &config);
    let ctx = Ctx::new("umx-bench");
    svc.graph_load(&ctx, copy.to_str().unwrap(), None)
        .map_err(|p| format!("load the copy: {}", p.code))?;
    let lines = |tag: &str, parent: Option<&str>| -> Vec<BatchLine> {
        (0..OPS)
            .map(|i| add_line(i + 1, format!("Bench batch {tag} {i}"), parent))
            .collect()
    };
    let run = |svc: &mut umx_services::Services,
               lines: Vec<BatchLine>,
               dry_run: bool|
     -> Result<(f64, usize), String> {
        let t = Instant::now();
        let done = svc
            .batch(
                &ctx,
                BatchRequest {
                    raised_in: None,
                    echo_sentences: false,
                    lines,
                    dry_run,
                    refs: Default::default(),
                    refs_graph: None,
                    reason: None,
                },
            )
            .map_err(|e| format!("batch: {}", e[0].code))?;
        Ok((ms(t), done.value.rows.len()))
    };
    let (dry_ms, dry_rows) = run(&mut svc, lines("dry", None), true)?;
    let (adds_ms, rows) = run(&mut svc, lines("adds", None), false)?;
    let (adds_edges_ms, _) = run(&mut svc, lines("child", Some(parent)), false)?;
    // A one-op batch against the single verb, interleaved so load hits both alike.
    let (mut one, mut single) = (Vec::new(), Vec::new());
    for i in 0..20 {
        let (t, _) = run(
            &mut svc,
            vec![add_line(1, format!("Bench one {i}"), None)],
            false,
        )?;
        one.push(t);
        let t = Instant::now();
        svc.node_add(
            &ctx,
            NodeAddRequest {
                kind: "problem".into(),
                title: format!("Bench single {i}"),
                ..Default::default()
            },
        )
        .map_err(|p| format!("node add: {}", p.code))?;
        single.push(ms(t));
    }
    one.sort_by(f64::total_cmp);
    single.sort_by(f64::total_cmp);
    Ok(json!({
        "ops": OPS,
        "dry_run_adds_ms": dry_ms, "dry_run_rows": dry_rows,
        "adds_ms": adds_ms, "adds_rows": rows,
        "adds_parent_edges_ms": adds_edges_ms,
        "one_op_batch_p50_ms": one[one.len() / 2],
        "node_add_p50_ms": single[single.len() / 2],
    }))
}

/// Plan 072: identical new records, appended ids versus older ids in seeded random order.
/// The measured call is the whole synchronous services batch, hence the daemon's writer hold.
pub fn explicit_ids(work: &Path, graph: &Path, fx: &Fixture, runs: usize) -> Result<Value, String> {
    let base = storage::build(fx);
    let low = base
        .nodes()
        .next()
        .ok_or("batch id benchmark needs nodes")?
        .id()
        .0;
    let mut random: Vec<usize> = (0..OPS).collect();
    let mut rng = crate::fixture::Rng(72);
    for i in (1..random.len()).rev() {
        random.swap(i, rng.below(i + 1));
    }
    let ids: Vec<Id> = random
        .iter()
        .map(|&i| Id(low - (1u128 << 80) + i as u128))
        .collect();
    let mut measured = serde_json::Map::new();
    for (explicit, edges, future) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, false),
        (false, false, true),
    ] {
        let tag = match (explicit, edges, future) {
            (_, _, true) => "mixed_future",
            (false, false, false) => "appended",
            (true, false, false) => "random_explicit",
            (false, true, false) => "appended_with_edges",
            (true, true, false) => "random_explicit_with_edges",
        };
        let mut holds = Vec::new();
        let mut applies = Vec::new();
        let mut dead_bytes = Vec::new();
        for run in 0..runs.max(1) {
            let copy = work.join(format!("ids-{tag}-{run}"));
            copy_graph(graph, &copy)?;
            let config = Config::at(work.join(format!("home-{tag}-{run}")), None)
                .roots([work.to_str().ok_or("work path is not UTF-8")?]);
            let mut svc = compose(Mode::Real, &config);
            let ctx = Ctx::new("batch-id-bench");
            svc.graph_load(&ctx, copy.to_str().ok_or("copy path is not UTF-8")?, None)
                .map_err(|p| format!("load: {}", p.code))?;
            let mut lines = Vec::new();
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| format!("system clock: {e}"))?
                .as_millis();
            let root = base.nodes().next().ok_or("base node")?.id().to_text();
            for (i, id) in ids.iter().enumerate() {
                let mut line = add_line(lines.len() + 1, format!("Imported parcel {i:04x}"), None);
                if let BatchOp::NodeAdd {
                    id: slot, ref_name, ..
                } = &mut line.op
                {
                    if explicit {
                        *slot = Some(id.to_text());
                    }
                    if future && i == 0 {
                        *slot = Some(Id((now_ms + 59_000) << 80).to_text());
                    }
                    *ref_name = Some(format!("p{i}"));
                }
                lines.push(line);
                if edges {
                    lines.push(BatchLine {
                        line: lines.len() as u32 + 1,
                        op: BatchOp::EdgeAdd {
                            id: explicit.then(|| Id(id.0 - (1u128 << 80)).to_text()),
                            from: None,
                            from_ref: Some(format!("p{i}")),
                            relation: "supports".into(),
                            to: Some(root.clone()),
                            to_ref: None,
                            sets: Vec::new(),
                            evidence: Default::default(),
                        },
                    });
                }
            }
            let t = Instant::now();
            let done = svc
                .batch(
                    &ctx,
                    BatchRequest {
                        raised_in: None,
                        reason: None,
                        echo_sentences: false,
                        lines,
                        dry_run: false,
                        refs: Default::default(),
                        refs_graph: None,
                    },
                )
                .map_err(|ps| {
                    format!("{tag}: {:?}", ps.iter().map(|p| p.code).collect::<Vec<_>>())
                })?;
            holds.push(ms(t));
            if done.value.rows.len() != OPS * (1 + usize::from(edges))
                || svc
                    .loaded("bench")
                    .ok_or("bench unloaded")?
                    .graph
                    .node_count()
                    != fx.node_count + OPS
            {
                return Err("batch id benchmark row count differs".into());
            }

            let mut g = base.clone();
            let high = g.last_id().ok_or("base ids")?.0;
            let t = Instant::now();
            let mut b = g.batch();
            if explicit || future {
                let planned: Vec<Id> = if future {
                    done.value
                        .rows
                        .iter()
                        .map(|r| r.id.ok_or("planned node has no id"))
                        .collect::<Result<_, _>>()?
                } else {
                    ids.clone()
                };
                b.insert_placeholders(planned.clone(), "2026-11-11T00:00:00Z")
                    .map_err(|e| format!("placeholders: {e:?}"))?;
                for (i, id) in planned.iter().enumerate() {
                    b.fill(*id, node(*id, format!("Imported parcel {i:04x}")))
                        .map_err(|e| format!("fill: {e:?}"))?;
                    if edges {
                        b.insert_edge(edge(
                            Id(id.0 - (1u128 << 80)),
                            *id,
                            base.nodes().next().ok_or("base node")?.id(),
                        ));
                    }
                }
            } else {
                for i in 0..OPS {
                    b.insert_node(node(
                        Id(high + i as u128 * 2 + 1),
                        format!("Imported parcel {i:04x}"),
                    ))
                    .map_err(|e| format!("append: {e:?}"))?;
                    if edges {
                        b.insert_edge(edge(
                            Id(high + i as u128 * 2 + 2),
                            Id(high + i as u128 * 2 + 1),
                            base.nodes().next().ok_or("base node")?.id(),
                        ));
                    }
                }
            }
            dead_bytes.push(b.peak_dead_bytes());
            b.commit();
            applies.push(ms(t));
        }
        measured.insert(
            tag.into(),
            json!({
                "writer_hold_ms": holds, "writer_hold": median(holds.clone()),
                "graph_apply_ms": applies, "graph_apply": median(applies.clone()),
                "peak_dead_bytes": dead_bytes,
            }),
        );
    }
    let ratio = measured["random_explicit"]["writer_hold"]["median"]
        .as_f64()
        .ok_or("explicit median")?
        / measured["appended"]["writer_hold"]["median"]
            .as_f64()
            .ok_or("appended median")?;
    Ok(json!({ "ops": OPS, "seed": 72, "random_to_append": ratio, "measurements": measured }))
}

fn node(id: Id, title: String) -> Node {
    Node {
        id,
        kind: "problem".into(),
        title,
        status: "open".into(),
        note: String::new(),
        evidence: Vec::new(),
        status_history: Vec::new(),
        created: "2026-11-10T00:00:00Z".into(),
        updated: "2026-11-10T00:00:00Z".into(),
        created_by: "umx-bench".into(),
        locator: None,
        authored_by_session: None,
        edited_by_session: None,
        fields: BTreeMap::new(),
    }
}

fn edge(id: Id, from: Id, to: Id) -> Edge {
    Edge {
        id,
        kind: "part_of".into(),
        from_graph: None,
        from,
        to_graph: None,
        to,
        created: "2026-11-10T00:00:00Z".into(),
        created_by: "umx-bench".into(),
        authored_by_session: None,
        edited_by_session: None,
        evidence: Vec::new(),
        fields: BTreeMap::new(),
    }
}

enum Op {
    Node(Node),
    Edge(Edge),
}

/// `adds` new nodes, each with a part_of edge to the first node when `edges`, then `edits` existing nodes
/// retitled, spread across the graph; ids above every id the graph holds.
fn ops(g: &Graph, adds: usize, edges: bool, edits: usize) -> Vec<Op> {
    let mut next = g.last_id().map_or(0, |i| i.0);
    let mut id = || {
        next += 1;
        Id(next)
    };
    let root = g.nodes().next().expect("a node").id();
    let mut out = Vec::new();
    for i in 0..adds {
        let n = id();
        out.push(Op::Node(node(n, format!("Batch apply {i}"))));
        if edges {
            out.push(Op::Edge(edge(id(), n, root)));
        }
    }
    let ids: Vec<Id> = g.nodes().map(|n| n.id()).collect();
    for i in 0..edits {
        let mut n = g
            .node(ids[i * ids.len() / edits.max(1)])
            .expect("a node")
            .to_node();
        n.title.push_str(" (edited)");
        out.push(Op::Node(n));
    }
    out
}

fn apply(b: &mut umx_graph::Batch<'_>, ops: Vec<Op>) {
    for op in ops {
        match op {
            Op::Node(n) => {
                b.insert_node(n).expect("fits");
            }
            Op::Edge(e) => {
                b.insert_edge(e);
            }
        }
    }
}

/// umx-graph's batch scope alone: apply and commit 1,000 adds with 1,000 edges, 1,000 edits, and a 1,000-op mixed
/// batch rolled back (its time against its apply, the undo log's bytes, and the graph equal afterwards).
pub fn graph(runs: usize, fx: &Fixture) -> Value {
    let base = storage::build(fx);
    let (mut adds, mut edits, mut mixed, mut back) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut undo_bytes, mut replaced_bytes, mut equal) = (0, 0, true);
    for _ in 0..runs.max(1) {
        let mut g = base.clone();
        let o = ops(&g, OPS, true, 0);
        let t = Instant::now();
        let mut b = g.batch();
        apply(&mut b, o);
        b.commit();
        adds.push(ms(t));

        let mut g = base.clone();
        let o = ops(&g, 0, false, OPS);
        let t = Instant::now();
        let mut b = g.batch();
        apply(&mut b, o);
        b.commit();
        edits.push(ms(t));

        let mut g = base.clone();
        let o = ops(&g, 350, true, 300);
        let before = (g.encode_nodes(), g.encode_edges());
        // The replaced lines' bytes (the fixture writes umx's canonical form, so encoding gives their length).
        replaced_bytes = o
            .iter()
            .filter_map(|op| match op {
                Op::Node(n) => g.node(n.id).map(|r| r.to_node()),
                Op::Edge(_) => None,
            })
            .map(|n| umx_core::codec::encode_nodes([&n]).len() - 1)
            .sum();
        let t = Instant::now();
        let mut b = g.batch();
        apply(&mut b, o);
        mixed.push(ms(t));
        undo_bytes = b.undo_bytes();
        let t = Instant::now();
        b.rollback();
        back.push(ms(t));
        equal &= (g.encode_nodes(), g.encode_edges()) == before;
    }
    json!({
        "ops": OPS,
        "apply_adds_edges_ms": median(adds),
        "apply_edits_ms": median(edits),
        "mixed_apply_ms": median(mixed),
        "rollback_ms": median(back),
        "rollback_equal": equal,
        "undo_bytes": undo_bytes,
        "undo_bound_bytes": replaced_bytes + 16 * OPS,
    })
}
