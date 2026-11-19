//! External endpoint load and allocator measurements, without services or cross-graph traversal.

use crate::{alloc, fixture};
use serde_json::{Value, json};
use std::hint::black_box;
use std::time::Instant;
use umx_core::{Edge, Id, codec};
use umx_graph::Graph;

fn build(fx: &fixture::Fixture, edges: &[u8]) -> Graph {
    let (mut graph, _) = Graph::load_nodes(fx.nodes.clone()).expect("fixture nodes load");
    let report = graph.load_edges(edges).expect("fixture edges load");
    assert!(report.dangling.is_empty() && report.repeats.is_empty());
    graph
}

fn samples(runs: usize, mut load: impl FnMut() -> Graph) -> Vec<f64> {
    (0..runs)
        .map(|_| {
            let start = Instant::now();
            let graph = black_box(load());
            let elapsed = start.elapsed().as_secs_f64() * 1e3;
            drop(graph);
            elapsed
        })
        .collect()
}

fn median(samples: &[f64]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

fn edge_bytes(records: &mut [Edge], count: usize, both: bool) -> Vec<u8> {
    for (i, edge) in records.iter_mut().enumerate() {
        edge.from_graph = (i < count && (both || i % 2 == 0)).then_some(Id(90));
        edge.to_graph = (i < count && (both || i % 2 != 0)).then_some(Id(91));
    }
    codec::encode_edges(records.iter())
}

pub fn measure(runs: usize, fx: &fixture::Fixture) -> Value {
    let mut records = codec::decode_edges(&fx.edges)
        .expect("fixture edges decode")
        .records;
    let mut counts = vec![0, 1_000, 10_000];
    if fx.node_count >= 100_000 {
        counts.push(100_000);
    }
    counts.retain(|count| *count <= records.len());
    // Warm both parsing and the allocator before paired samples. Each pair alternates its order.
    drop(build(fx, &fx.edges));
    let mut rows = Vec::new();
    for count in counts {
        let edges = edge_bytes(&mut records, count, false);
        drop(build(fx, &edges));
        let mut local_samples = Vec::new();
        let mut external_samples = Vec::new();
        for run in 0..runs {
            let mut local = || local_samples.extend(samples(1, || build(fx, &fx.edges)));
            let mut external = || external_samples.extend(samples(1, || build(fx, &edges)));
            if run % 2 == 0 {
                local();
                external();
            } else {
                external();
                local();
            }
        }
        let before = alloc::live();
        let graph = build(fx, &edges);
        let counted = alloc::live_since(before);
        let memory = graph.memory_bytes();
        let external_adjacency: usize = graph
            .nodes()
            .map(|n| {
                graph.edges_out_external(n.ix()).count() + graph.edges_in_external(n.ix()).count()
            })
            .sum();
        assert_eq!(external_adjacency, count);
        assert_eq!(graph.edge_count(), fx.edge_count);
        let total_memory = memory.total();
        let external_memory = memory.external;
        drop(graph);
        rows.push(json!({
            "external_ends": count,
            "external_edges": count,
            "local_load_samples_ms": local_samples,
            "external_load_samples_ms": external_samples,
            "local_load_median_ms": median(&local_samples),
            "external_load_median_ms": median(&external_samples),
            "load_ratio": median(&external_samples) / median(&local_samples),
            "external_bytes": external_memory,
            "bytes_per_external_edge": if count == 0 { 0.0 } else { external_memory as f64 / count as f64 },
            "allocator_graph_bytes": counted,
            "memory_graph_bytes": total_memory,
            "memory_bytes_over_alloc": total_memory as f64 / counted as f64,
        }));
    }
    json!({ "rows": rows })
}

pub(crate) fn write_fixture(
    root: &std::path::Path,
    alias: &str,
    id: Id,
    fx: &fixture::Fixture,
    edges: &[u8],
) -> Result<std::path::PathBuf, String> {
    let path = root.join(alias);
    std::fs::create_dir(&path).map_err(|e| format!("make {}: {e}", path.display()))?;
    let mut meta = codec::decode_graph(&fx.graph).map_err(|e| format!("fixture metadata: {e}"))?;
    meta.id = id;
    meta.alias = alias.into();
    let records = codec::decode_edges(edges)
        .map_err(|e| e.to_string())?
        .records;
    let metadata = codec::encode_graph_with_edges(&meta, &records).map_err(|e| e.to_string())?;
    for (name, bytes) in [
        ("graph.json", metadata),
        ("nodes.jsonl", fx.nodes.clone()),
        ("edges.jsonl", edges.to_vec()),
    ] {
        std::fs::write(path.join(name), bytes).map_err(|e| format!("write {name}: {e}"))?;
    }
    Ok(path)
}

fn read_samples(
    runs: usize,
    mut read: impl FnMut() -> Result<usize, String>,
) -> Result<Value, String> {
    let mut samples = Vec::new();
    let mut rows = None;
    for _ in 0..runs {
        let start = Instant::now();
        let count = read()?;
        samples.push(start.elapsed().as_secs_f64() * 1e3);
        if let Some(expected) = rows {
            assert_eq!(count, expected);
        }
        rows = Some(count);
    }
    Ok(json!({"samples_ms": samples, "median_ms": median(&samples), "rows": rows}))
}

pub fn measure_services(runs: usize, fx: &fixture::Fixture) -> Result<Value, String> {
    use umx_compose::{Config, Mode, compose};
    use umx_services::{Ctx, Direction, EdgeListRequest, IncomingIndex, NeighboursRequest};
    let mut records = codec::decode_edges(&fx.edges)
        .map_err(|e| e.to_string())?
        .records;
    let mut counts = vec![1_000, 10_000];
    if fx.node_count >= 100_000 {
        counts.push(100_000);
    }
    let mut index_rows = Vec::new();
    let available = records.len();
    for count in counts.into_iter().filter(|count| *count <= available) {
        let edges = edge_bytes(&mut records, count, false);
        let graph = build(fx, &edges);
        let mut samples = Vec::new();
        for _ in 0..runs {
            let start = Instant::now();
            let mut index = IncomingIndex::default();
            index.add_graph(1, &graph);
            samples.push(start.elapsed().as_secs_f64() * 1e3);
            assert_eq!(index.len(), count);
        }
        let before = alloc::live();
        let mut index = IncomingIndex::default();
        index.add_graph(1, &graph);
        let memory = alloc::live_since(before);
        index_rows.push(
            json!({"external_entries": count, "samples_ms": samples, "median_ms": median(&samples),
            "allocator_bytes": memory, "bytes_per_entry": memory as f64 / count as f64}),
        );
    }
    let work = crate::work_dir()?;
    let _cleanup = crate::RemoveOnDrop(work.clone());
    let local = write_fixture(&work, "local", Id(91), fx, &fx.edges)?;
    let external = write_fixture(&work, "external", Id(92), fx, &fx.edges)?;
    let far = write_fixture(&work, "far", Id(90), fx, &fx.edges)?;
    let config = Config::at(work.join("home"), None)
        .roots([work.to_str().ok_or("work path is not UTF-8")?]);
    let mut svc = compose(Mode::Real, &config);
    for path in [&local, &external, &far] {
        svc.graph_load(
            &Ctx::new("bench"),
            path.to_str().ok_or("fixture path is not UTF-8")?,
            None,
        )
        .map_err(|p| format!("load fixture: {}", p.code))?;
    }
    let nodes = codec::decode_nodes(&fx.nodes)
        .map_err(|e| e.to_string())?
        .records;
    let from = nodes.last().ok_or("fixture needs nodes")?.id.to_text();
    let runs = runs.min(20).min(nodes.len().saturating_sub(1)).max(1);
    let mut local_samples = Vec::new();
    let mut external_samples = Vec::new();
    let mut first_external = None;
    for (sample, node) in nodes.iter().take(runs + 1).enumerate() {
        let to = node.id.to_text();
        let order = if sample % 2 == 0 {
            [("local", false), ("external", true)]
        } else {
            [("external", true), ("local", false)]
        };
        for (alias, external_end) in order {
            let to = if external_end {
                format!("umx:far/{to}")
            } else {
                to.clone()
            };
            let start = Instant::now();
            let done = svc
                .edge_add(
                    &Ctx::new("bench").graph(alias),
                    &from,
                    "bench_link",
                    &to,
                    &[],
                )
                .map_err(|p| format!("edge add: {}", p.code))?;
            let elapsed = start.elapsed().as_secs_f64() * 1e3;
            assert!(!done.value.existed);
            if sample == 0 {
                if external_end {
                    first_external = Some(elapsed);
                }
            } else if external_end {
                external_samples.push(elapsed);
            } else {
                local_samples.push(elapsed);
            }
        }
    }
    // A separate source puts 1,000 edges into one node in the far graph.
    let target = nodes[0].id;
    for edge in &mut records {
        edge.from_graph = None;
        edge.to_graph = None;
    }
    for edge in records.iter_mut().take(1_000) {
        edge.to_graph = Some(Id(90));
        edge.to = target;
    }
    let edges = codec::encode_edges(&records);
    let hot = write_fixture(&work, "hot", Id(93), fx, &edges)?;
    svc.graph_load(
        &Ctx::new("bench"),
        hot.to_str().ok_or("hot path is not UTF-8")?,
        None,
    )
    .map_err(|p| format!("load hot source: {}", p.code))?;
    let ctx = Ctx::new("bench").graph("far");
    let incoming_list = read_samples(runs, || {
        let page = svc
            .edge_list(
                &ctx,
                EdgeListRequest {
                    to: Some(target.to_text()),
                    limit: usize::MAX,
                    ..Default::default()
                },
            )
            .map_err(|p| p.code.to_string())?
            .value;
        assert_eq!(
            page.rows.iter().filter(|row| row.graph == Id(93)).count(),
            1_000
        );
        Ok(page.rows.len())
    })?;
    let incoming_neighbours = read_samples(runs, || {
        let page = svc
            .neighbours(
                &ctx,
                NeighboursRequest {
                    no_tags: Default::default(),
                    entities: Default::default(),
                    tags: Default::default(),
                    node: target.to_text(),
                    relation: None,
                    direction: Direction::Both,
                    limit: usize::MAX,
                    offset: 0,
                },
            )
            .map_err(|p| p.code.to_string())?
            .value;
        assert_eq!(
            page.rows.iter().filter(|row| row.graph == Id(93)).count(),
            1_000
        );
        Ok(page.rows.len())
    })?;
    Ok(
        json!({"index": index_rows, "local_add_samples_ms": local_samples, "external_add_samples_ms": external_samples,
        "local_add_median_ms": median(&local_samples), "external_add_median_ms": median(&external_samples),
        "external_over_local_add": median(&external_samples) / median(&local_samples),
        "first_external_add_with_format_upgrade_ms": first_external,
        "incoming_1000_edge_list": incoming_list, "incoming_1000_neighbours": incoming_neighbours}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_memory_matches_allocator_for_one_and_both_ends() {
        let fx = fixture::generate(67, 1_000, 3, true);
        let mut records = codec::decode_edges(&fx.edges).unwrap().records;
        for both in [false, true] {
            let edges = edge_bytes(&mut records, 1_000, both);
            let before = alloc::thread_net();
            let graph = build(&fx, &edges);
            let counted = alloc::thread_net() - before;
            let memory = graph.memory_bytes();
            assert!(memory.external <= 64 * 1_000, "{memory:?}");
            let ratio = memory.total() as f64 / counted as f64;
            assert!(
                (0.95..=1.05).contains(&ratio),
                "{memory:?}, allocator {counted}, ratio {ratio}"
            );
        }
    }

    #[test]
    fn incoming_index_memory_per_entry_is_within_its_allocator_cap() {
        let fx = fixture::generate(68, 1_000, 3, true);
        let mut records = codec::decode_edges(&fx.edges).unwrap().records;
        let edges = edge_bytes(&mut records, 1_000, false);
        let graph = build(&fx, &edges);
        let before = alloc::thread_net();
        let mut index = umx_services::IncomingIndex::default();
        index.add_graph(1, &graph);
        let counted = alloc::thread_net() - before;
        assert_eq!(index.len(), 1_000);
        assert!(
            counted > 0 && counted <= 96 * 1_000,
            "{counted} allocator bytes"
        );
    }
}
