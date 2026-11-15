//! Paired bare/stamped fixtures use the same binary and the normal services/fs write measurements.

use crate::{Args, RemoveOnDrop, alloc, batch, fixture, ms, report, storage, work_dir, writes};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;
use umx_core::codec;
use umx_core::provenance::{Actor, ActorInput, Tier};
use umx_core::{Edge, Id, Node};
use umx_services::{Ctx, NodeAddRequest};

fn stamped(mut fx: fixture::Fixture, sessions: usize) -> fixture::Fixture {
    if sessions == 0 {
        return fx;
    }
    let mut nodes = codec::decode_nodes(&fx.nodes)
        .expect("fixture nodes")
        .records;
    let mut edges = codec::decode_edges(&fx.edges)
        .expect("fixture edges")
        .records;
    let mut next = nodes
        .iter()
        .map(|n| n.id)
        .chain(edges.iter().map(|e| e.id))
        .max()
        .unwrap()
        .0;
    let mut id = || {
        next += 1;
        Id(next)
    };
    let template = nodes[0].clone();
    let agent = id();
    let session_ids: Vec<_> = (0..sessions).map(|_| id()).collect();
    for (i, node) in nodes.iter_mut().enumerate() {
        node.authored_by_session = Some(session_ids[i % sessions]);
        node.edited_by_session = Some(session_ids[(i + 1) % sessions]);
    }
    for (i, edge) in edges.iter_mut().enumerate() {
        edge.authored_by_session = Some(session_ids[i % sessions]);
    }
    let record = |id, kind: &str, key: String| Node {
        id,
        kind: kind.into(),
        title: key.clone(),
        status: "open".into(),
        note: String::new(),
        evidence: Vec::new(),
        status_history: Vec::new(),
        created: template.created.clone(),
        updated: template.updated.clone(),
        created_by: "umx".into(),
        locator: None,
        authored_by_session: None,
        edited_by_session: None,
        fields: BTreeMap::from([("prov_key".into(), json!(key))]),
    };
    nodes.push(record(agent, "agent", "agent/unknown".into()));
    for (i, &session) in session_ids.iter().enumerate() {
        let key = if i == 0 {
            "session/unknown/unknown".into()
        } else {
            format!("session/copilot/h-{i:016x}")
        };
        let mut node = record(session, "session", key);
        node.fields.insert("effort".into(), Value::Null);
        nodes.push(node);
        edges.push(Edge {
            id: id(),
            kind: "session_of".into(),
            from_graph: None,
            from: session,
            to_graph: None,
            to: agent,
            created: template.created.clone(),
            created_by: "umx".into(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: BTreeMap::new(),
        });
    }
    fx.nodes = codec::encode_nodes(&nodes);
    fx.edges = codec::encode_edges(&edges);
    fx.node_count = nodes.len();
    fx.edge_count = edges.len();
    fx
}

fn samples(values: Vec<f64>) -> Value {
    let mut sorted = values.clone();
    sorted.sort_by(f64::total_cmp);
    json!({
        "p50": sorted[(sorted.len() - 1) / 2],
        "p95": sorted[((sorted.len() - 1) as f64 * 0.95).round() as usize],
        "samples": values,
    })
}

fn duplicate_stamp_lines(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        out.extend_from_slice(line);
        if line
            .windows(b"\"authored_by_session\"".len())
            .any(|w| w == b"\"authored_by_session\"")
        {
            out.extend_from_slice(line);
        }
    }
    out
}

fn known_actor_write_cost(fx: &fixture::Fixture, actor: &Actor) -> Value {
    let (mut g, _) = umx_graph::Graph::load_nodes(fx.nodes.clone()).expect("microbench nodes");
    g.load_edges(&fx.edges).expect("microbench edges");
    let mut next = g.last_id().expect("fixture ids").0;
    let mut draw = |_: &umx_graph::Graph| {
        next += 1;
        Id(next)
    };
    let mut b = g.batch();
    let (session, warnings) =
        umx_services::upsert_provenance_session(&mut b, actor, &mut draw, "2026-11-11T00:00:00Z")
            .expect("warm the actual upsert");
    assert!(warnings.is_empty());
    b.prepare().expect("prepare warmup");
    b.commit();
    let counts = (g.node_count(), g.edge_count());
    let mut template = g
        .nodes()
        .find(|n| !g.is_provenance(n.ix()))
        .expect("content node")
        .to_node();
    template.id = Id(g.last_id().unwrap().0 + 1);
    template.title = "Algorithm cost sample".into();
    template.record_author(session);
    let mut cached = Vec::new();
    let mut upsert = Vec::new();
    for i in 0..1000 {
        for with_upsert in if i % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let node = template.clone();
            let t = Instant::now();
            let mut b = g.batch();
            if with_upsert {
                let (found, warnings) = umx_services::upsert_provenance_session(
                    &mut b,
                    actor,
                    &mut |_| panic!("known actor must mint no metadata"),
                    "2026-11-11T00:00:00Z",
                )
                .expect("known actor upsert");
                assert_eq!(found, session);
                assert!(warnings.is_empty());
            }
            b.insert_node(node).expect("microbench content write");
            b.prepare().expect("prepare microbench");
            let us = t.elapsed().as_secs_f64() * 1e6;
            if with_upsert {
                upsert.push(us);
            } else {
                cached.push(us);
            }
            drop(b);
        }
    }
    assert_eq!((g.node_count(), g.edge_count()), counts);
    let cached = samples(cached);
    let upsert = samples(upsert);
    json!({
        "boundary": "graph apply and prepare only; no file IO or capture; rollback untimed",
        "baseline": "content stamps kept, session already resolved; diagnostic only, no production opt-out",
        "tier": actor.fields().tier.as_str(), "writes_per_arm": 1000, "new_metadata_per_write": 0,
        "cached_session_us": cached, "with_actual_upsert_us": upsert,
        "p50_upsert_delta_us": upsert["p50"].as_f64().unwrap() - cached["p50"].as_f64().unwrap(),
    })
}

fn churn(work: &std::path::Path) -> Result<Value, String> {
    let graph = work.join("churn");
    let config = umx_compose::Config::at(work.join("churn-home"), None)
        .roots([work.to_str().ok_or("work path")?]);
    let mut svc = umx_compose::compose(umx_compose::Mode::Real, &config);
    svc.graph_new(
        &Ctx::new("bench"),
        graph.to_str().ok_or("graph path")?,
        "churn",
        None,
        false,
        None,
    )
    .map_err(|p| format!("churn new: {}", p.code))?;
    let t = Instant::now();
    for i in 0..1000 {
        let actor = Actor::try_from(ActorInput {
            tier: Tier::Harness,
            harness: Some("copilot".into()),
            harness_session: Some(format!("h-{:016x}", i / 10)),
            seat: None,
            role: None,
            model: None,
            effort: None,
            user: Some("bench-writer".into()),
        })
        .expect("synthetic capture");
        svc.node_add(
            &Ctx::new("bench").graph("churn").actor(actor),
            NodeAddRequest {
                kind: "problem".into(),
                title: format!("Churn write {i}"),
                ..Default::default()
            },
        )
        .map_err(|p| format!("churn add: {}", p.code))?;
    }
    let elapsed = ms(t.elapsed());
    let g = &svc.loaded("churn").ok_or("churn unloaded")?.graph;
    let p = g.provenance_stats();
    Ok(json!({
        "writes": 1000, "writes_per_session": 10, "starting_content_nodes": 0,
        "content_nodes_added": g.node_count() - p.provenance_nodes,
        "provenance_nodes_added": p.provenance_nodes, "elapsed_ms": elapsed,
        "interner_ids": {"authored": p.authored_ids, "edited": p.edited_ids, "edge_authored": p.edge_authored_ids},
        "interner_bytes": {"authored": p.authored_bytes, "edited": p.edited_bytes, "edge_authored": p.edge_authored_bytes},
        "unique_index_bytes": p.unique_index_bytes, "provenance_heap_bytes": g.memory_bytes().provenance,
    }))
}

fn warm_service_graph(work: &std::path::Path, graph: &std::path::Path) -> Result<Value, String> {
    let config = umx_compose::Config::at(work.join("warmup-home"), None)
        .roots([work.to_str().ok_or("work path")?]);
    let mut svc = umx_compose::compose(umx_compose::Mode::Real, &config);
    let ctx = Ctx::new("umx-bench");
    svc.graph_load(&ctx, graph.to_str().ok_or("graph path")?, None)
        .map_err(|p| format!("warmup load: {}", p.code))?;
    let before = svc
        .loaded("bench")
        .ok_or("warmup graph unloaded")?
        .graph
        .provenance_count();
    let t = Instant::now();
    svc.node_add(
        &ctx,
        NodeAddRequest {
            kind: "problem".into(),
            title: "Steady service warmup".into(),
            ..Default::default()
        },
    )
    .map_err(|p| format!("warmup add: {}", p.code))?;
    let elapsed = ms(t.elapsed());
    let after = svc
        .loaded("bench")
        .ok_or("warmup graph unloaded")?
        .graph
        .provenance_count();
    Ok(
        json!({"unmeasured_content_writes": 1, "elapsed_ms": elapsed,
            "provenance_nodes_before": before, "provenance_nodes_after": after}),
    )
}

fn cold_session_writes(
    work: &std::path::Path,
    graph: &std::path::Path,
    runs: usize,
) -> Result<Value, String> {
    let actor = |session: &str| {
        Actor::try_from(ActorInput {
            tier: Tier::Pij,
            seat: Some("pij-bench-writer".into()),
            role: Some("coder".into()),
            model: Some("safe-model".into()),
            effort: Some("high".into()),
            harness: Some("copilot".into()),
            harness_session: Some(session.into()),
            user: Some("bench-writer".into()),
        })
        .expect("synthetic cold-session actor")
    };
    let base = work.join("cold-base");
    writes::copy_graph(graph, &base)?;
    let config = umx_compose::Config::at(work.join("cold-seed-home"), None)
        .roots([work.to_str().ok_or("work path")?]);
    let mut seed = umx_compose::compose(umx_compose::Mode::Real, &config);
    seed.graph_load(&Ctx::new("bench"), base.to_str().ok_or("cold path")?, None)
        .map_err(|p| format!("cold seed load: {}", p.code))?;
    let t = Instant::now();
    seed.node_add(
        &Ctx::new("bench").actor(actor("h-fffffffffffffffe")),
        NodeAddRequest {
            kind: "problem".into(),
            title: "Known cold-session agent".into(),
            ..Default::default()
        },
    )
    .map_err(|p| format!("cold seed add: {}", p.code))?;
    let seed_ms = ms(t.elapsed());
    drop(seed);
    let mut times = Vec::new();
    for i in 0..runs {
        let copy = work.join(format!("cold-{i}"));
        writes::copy_graph(&base, &copy)?;
        let config = umx_compose::Config::at(work.join(format!("cold-home-{i}")), None)
            .roots([work.to_str().ok_or("work path")?]);
        let mut svc = umx_compose::compose(umx_compose::Mode::Real, &config);
        svc.graph_load(
            &Ctx::new("bench"),
            copy.to_str().ok_or("cold copy path")?,
            None,
        )
        .map_err(|p| format!("cold load: {}", p.code))?;
        let g = &svc.loaded("bench").ok_or("cold graph unloaded")?.graph;
        let (nodes, edges) = (g.provenance_count(), g.edge_count());
        let t = Instant::now();
        svc.node_add(
            &Ctx::new("bench").actor(actor("h-ffffffffffffffff")),
            NodeAddRequest {
                kind: "problem".into(),
                title: "First write by a new session".into(),
                ..Default::default()
            },
        )
        .map_err(|p| format!("cold add: {}", p.code))?;
        times.push(ms(t.elapsed()));
        let g = &svc.loaded("bench").ok_or("cold graph unloaded")?.graph;
        assert_eq!(
            g.provenance_count(),
            nodes + 1,
            "only the new session is minted"
        );
        assert_eq!(
            g.edge_count(),
            edges + 1,
            "one session_of edge forces adjacency rebuild"
        );
    }
    Ok(json!({
        "first_new_session_write_ms": samples(times),
        "unmeasured_agent_seed_ms": seed_ms,
        "agent_harness_model_platform_already_known": true,
        "new_provenance_nodes_per_write": 1, "new_provenance_edges_per_write": 1,
        "has_acceptance_band": false,
    }))
}

pub fn run(a: &Args) -> Result<PathBuf, String> {
    let load_average = Command::new("uptime").output().map_err(|e| e.to_string())?;
    if !load_average.status.success() {
        return Err("uptime failed".into());
    }
    let work = work_dir()?;
    let _cleanup = RemoveOnDrop(work.clone());
    let mut fx = stamped(
        fixture::generate(a.seed, a.nodes, a.edges_per_node, true),
        a.provenance_sessions,
    );
    if a.provenance_duplicate_lines {
        fx.nodes = duplicate_stamp_lines(&fx.nodes);
        fx.edges = duplicate_stamp_lines(&fx.edges);
    }
    let graph = work.join("g");
    std::fs::create_dir(&graph).map_err(|e| e.to_string())?;
    for (name, bytes) in [
        ("graph.json", &fx.graph),
        ("nodes.jsonl", &fx.nodes),
        ("edges.jsonl", &fx.edges),
    ] {
        std::fs::write(graph.join(name), bytes).map_err(|e| e.to_string())?;
    }
    let mut loads = Vec::new();
    let mut peaks = Vec::new();
    let mut heap = Value::Null;
    let mut encode_nodes = Vec::new();
    let mut encode_edges = Vec::new();
    for _ in 0..a.runs {
        let before = alloc::live();
        alloc::reset_peak();
        let t = Instant::now();
        let g = if a.provenance_duplicate_lines {
            let (mut g, nodes) =
                umx_graph::Graph::load_nodes(fx.nodes.clone()).expect("plant nodes load");
            let edges = g.load_edges(&fx.edges).expect("plant edges load");
            assert_eq!(nodes.repeats.len(), a.nodes);
            assert_eq!(edges.repeats.len(), fx.edge_count - a.provenance_sessions);
            g
        } else {
            storage::build(&fx)
        };
        loads.push(ms(t.elapsed()));
        peaks.push(alloc::peak().saturating_sub(before));
        let m = g.memory_bytes();
        let p = g.provenance_stats();
        heap = json!({
            "total": m.total(), "node_columns": m.node_columns, "edge_columns": m.edge_columns,
            "adjacency": m.adjacency, "field_columns": m.field_columns, "image": m.image,
            "cold": m.cold, "word_index": m.word_index, "provenance": m.provenance,
            "provenance_parts": {
                "unique_index_bytes": p.unique_index_bytes, "unique_index_entries": p.unique_index_entries,
                "unique_index_dead_bytes": p.unique_index_dead_bytes, "bitset_bytes": p.bitset_bytes,
                "provenance_nodes": p.provenance_nodes, "authored_bytes": p.authored_bytes,
                "authored_ids": p.authored_ids, "edited_bytes": p.edited_bytes, "edited_ids": p.edited_ids,
                "edge_authored_bytes": p.edge_authored_bytes, "edge_authored_ids": p.edge_authored_ids,
                "edge_edited_bytes": p.edge_edited_bytes, "edge_edited_ids": p.edge_edited_ids,
                "edge_edited_edges": p.edge_edited_edges,
            },
        });
        let t = Instant::now();
        let encoded = g.encode_nodes();
        encode_nodes.push(ms(t.elapsed()));
        assert_eq!(encoded, fx.nodes);
        let t = Instant::now();
        let encoded = g.encode_edges();
        encode_edges.push(ms(t.elapsed()));
        assert_eq!(encoded, fx.edges);
    }
    let service_warmup = warm_service_graph(&work, &graph)?;
    let cold = cold_session_writes(&work, &graph, a.runs)?;
    let parent = writes::first_node_id(&fx.nodes)?;
    let in_process = writes::in_process(a.adds, &work, &graph, &parent)?;
    let mut batches = Vec::new();
    let mut parent_batches = Vec::new();
    for i in 0..a.runs {
        let run = work.join(format!("batch-{i}"));
        std::fs::create_dir(&run).map_err(|e| e.to_string())?;
        let measured = batch::services(&run, &graph, &parent)?;
        batches.push(measured["adds_ms"].as_f64().ok_or("missing batch timing")?);
        parent_batches.push(
            measured["adds_parent_edges_ms"]
                .as_f64()
                .ok_or("missing parent batch timing")?,
        );
    }
    let cli = if a.cli {
        crate::through_cli(a, &work, graph.to_str().ok_or("graph path")?, &parent)?
    } else {
        Value::Null
    };
    let search_path = umx_compose::Config::from_env().search_path;
    let known_actor = umx_compose::client_actor(
        umx_compose::Mode::Real,
        &search_path,
        umx_compose::ClientBudgets::default(),
    );
    let algorithm = known_actor_write_cost(&fx, &known_actor);
    let mut capture = BTreeMap::<String, Vec<f64>>::new();
    for _ in 0..a.adds.min(20) {
        let t = Instant::now();
        let actor = umx_compose::client_actor(
            umx_compose::Mode::Real,
            &search_path,
            umx_compose::ClientBudgets::default(),
        );
        let elapsed = ms(t.elapsed());
        capture
            .entry(actor.fields().tier.as_str().into())
            .or_default()
            .push(elapsed);
    }
    let capture: BTreeMap<_, _> = capture
        .into_iter()
        .map(|(tier, times)| (tier, samples(times)))
        .collect();
    let churn = if a.provenance_sessions > 0 {
        churn(&work)?
    } else {
        Value::Null
    };
    report::write(
        &a.out,
        &json!({
            "umx_bench": 2, "commit": a.commit, "date": umx_core::time::iso_from_ms(
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?.as_millis() as u64),
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "conditions": a.conditions, "machine": report::machine(),
            "load_average_start": String::from_utf8_lossy(&load_average.stdout).trim(),
            "fixture": {"kind": "rich", "seed": a.seed, "nodes": fx.node_count, "edges": fx.edge_count,
                "content_nodes": a.nodes, "sessions": a.provenance_sessions,
                "duplicate_stamp_lines": a.provenance_duplicate_lines,
                "bytes": {"nodes": fx.nodes.len(), "edges": fx.edges.len()}},
            "in_process": {"load_ms": samples(loads), "load_peak_bytes": peaks, "heap": heap,
                "encode_nodes_ms": samples(encode_nodes), "encode_edges_ms": samples(encode_edges),
                "writes": in_process, "batch_1000_ms": samples(batches),
                "batch_1000_parent_ms": samples(parent_batches)},
            "service_warmup": service_warmup, "cli": cli, "capture_only_ms": capture,
            "known_actor_write": algorithm, "cold_session": cold, "churn": churn,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_warmup_makes_fresh_copies_start_with_their_actor_already_known() {
        let work = work_dir().unwrap();
        let _cleanup = RemoveOnDrop(work.clone());
        let graph = work.join("g");
        std::fs::create_dir(&graph).unwrap();
        let fx = fixture::generate(1, 10, 3, true);
        for (name, bytes) in [
            ("graph.json", &fx.graph),
            ("nodes.jsonl", &fx.nodes),
            ("edges.jsonl", &fx.edges),
        ] {
            std::fs::write(graph.join(name), bytes).unwrap();
        }
        let warmup = warm_service_graph(&work, &graph).unwrap();
        assert_eq!(warmup["provenance_nodes_before"], 0);
        assert_eq!(warmup["provenance_nodes_after"], 2);
        let config =
            umx_compose::Config::at(work.join("fresh-home"), None).roots([work.to_str().unwrap()]);
        let mut fresh = umx_compose::compose(umx_compose::Mode::Real, &config);
        let ctx = Ctx::new("umx-bench");
        fresh
            .graph_load(&ctx, graph.to_str().unwrap(), None)
            .unwrap();
        let edges = fresh.loaded("bench").unwrap().graph.edge_count();
        fresh
            .node_add(
                &ctx,
                NodeAddRequest {
                    kind: "problem".into(),
                    title: "First timed write".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let g = &fresh.loaded("bench").unwrap().graph;
        assert_eq!(g.provenance_count(), 2);
        assert_eq!(
            g.edge_count(),
            edges,
            "no first-write metadata edge or adjacency rebuild"
        );
        let cold = cold_session_writes(&work, &graph, 3).unwrap();
        assert_eq!(
            cold["first_new_session_write_ms"]["samples"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn duplicate_line_plant_preserves_live_records_and_exposes_the_extra_bytes() {
        let fx = stamped(fixture::generate(1, 30, 3, true), 10);
        let nodes = duplicate_stamp_lines(&fx.nodes);
        let edges = duplicate_stamp_lines(&fx.edges);
        assert!(nodes.len() > fx.nodes.len() && edges.len() > fx.edges.len());
        let (mut g, loaded) = umx_graph::Graph::load_nodes(nodes.clone()).unwrap();
        assert_eq!(loaded.repeats.len(), 30);
        let loaded = g.load_edges(&edges).unwrap();
        assert_eq!(loaded.repeats.len(), fx.edge_count - 10);
        assert_eq!(
            (g.node_count(), g.edge_count()),
            (fx.node_count, fx.edge_count)
        );
        assert_eq!(g.kept_line_count(), 30 + fx.edge_count - 10);
        assert_eq!(g.encode_nodes(), nodes);
        assert_eq!(g.encode_edges(), edges);
    }

    #[test]
    fn stamped_fixture_attributes_all_content_but_never_metadata() {
        let bare = fixture::generate(1, 30, 3, true);
        let fx = stamped(bare, 10);
        let g = storage::build(&fx);
        let mut content = 0;
        let mut metadata = 0;
        for node in g.nodes() {
            if g.is_provenance(node.ix()) {
                metadata += 1;
                assert_eq!(
                    (node.authored_by_session(), node.edited_by_session()),
                    (None, None)
                );
            } else {
                content += 1;
                assert!(node.authored_by_session().is_some() && node.edited_by_session().is_some());
            }
        }
        assert_eq!((content, metadata), (30, 11));
        for edge in g.edges() {
            assert_eq!(
                edge.authored_by_session().is_some(),
                edge.kind() != "session_of"
            );
        }
    }
}
