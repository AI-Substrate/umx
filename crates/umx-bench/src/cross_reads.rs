//! Paired local and cross-graph reads on the same rich node and edge populations.

use crate::{external, fixture};
use serde_json::{Value, json};
use std::hint::black_box;
use std::time::Instant;
use umx_compose::{Config, Mode, compose};
use umx_core::{Edge, Id, codec};
use umx_services::{Ctx, CypherRequest, Direction, PathRequest, WalkRequest};

fn percentiles(samples: &[f64]) -> (f64, f64) {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    (
        sorted[sorted.len() / 2],
        sorted[(sorted.len() * 95).div_ceil(100).saturating_sub(1)],
    )
}

fn distribution(
    runs: usize,
    expected: usize,
    mut read: impl FnMut() -> Result<usize, String>,
) -> Result<Value, String> {
    let rows = read()?;
    if rows != expected {
        return Err(format!(
            "cross-read benchmark expected {expected} rows, got {rows}"
        ));
    }
    let mut samples = Vec::new();
    for _ in 0..runs {
        let start = Instant::now();
        assert_eq!(black_box(read()?), rows);
        samples.push(start.elapsed().as_secs_f64() * 1e6);
    }
    let (p50, p95) = percentiles(&samples);
    Ok(json!({
        "samples_us": samples, "p50_us": p50, "p95_us": p95, "rows": rows,
    }))
}

fn hop(id: Id, from: Id, to: Id, graph: Option<Id>) -> Edge {
    Edge {
        id,
        kind: "bench_hop".into(),
        from_graph: None,
        from,
        to_graph: graph,
        to,
        created: String::new(),
        created_by: "umx-bench".into(),
        authored_by_session: None,
        edited_by_session: None,
        evidence: Vec::new(),
        fields: Default::default(),
    }
}

pub fn measure(runs: usize, fx: &fixture::Fixture) -> Result<Value, String> {
    let work = crate::work_dir()?;
    let _cleanup = crate::RemoveOnDrop(work.clone());
    let nodes = codec::decode_nodes(&fx.nodes)
        .map_err(|e| e.to_string())?
        .records;
    let original = codec::decode_edges(&fx.edges)
        .map_err(|e| e.to_string())?
        .records;
    let ids: Vec<_> = nodes.iter().take(4).map(|n| n.id).collect();
    if ids.len() < 4 {
        return Err("cross-read benchmark needs four nodes".into());
    }
    let next = original
        .iter()
        .map(|e| e.id.0)
        .max()
        .ok_or("fixture needs edges")?
        + 1;
    let mut local = original.clone();
    local.extend((0..3).map(|i| hop(Id(next + i as u128), ids[i], ids[i + 1], None)));
    let mut left = original.clone();
    left.extend([
        hop(Id(next), ids[0], ids[1], None),
        hop(Id(next + 1), ids[1], ids[2], Some(Id(93))),
    ]);
    let mut right = original;
    right.push(hop(Id(next + 2), ids[2], ids[3], None));
    let local = external::write_fixture(
        &work,
        "local",
        Id(91),
        fx,
        &codec::encode_edges(local.iter()),
    )?;
    let left =
        external::write_fixture(&work, "left", Id(92), fx, &codec::encode_edges(left.iter()))?;
    let right = external::write_fixture(
        &work,
        "right",
        Id(93),
        fx,
        &codec::encode_edges(right.iter()),
    )?;
    let config = Config::at(work.join("home"), None)
        .roots([work.to_str().ok_or("non-UTF8 benchmark path")?]);
    let mut svc = compose(Mode::Real, &config);
    for path in [&local, &left, &right] {
        svc.graph_load(
            &Ctx::new("bench"),
            path.to_str().ok_or("non-UTF8 path")?,
            None,
        )
        .map_err(|p| format!("load: {}", p.code))?;
    }
    let mut result = serde_json::Map::new();
    for (name, selection, start, end) in [
        (
            "local",
            "local",
            format!("umx:local/{}", ids[0]),
            format!("umx:local/{}", ids[3]),
        ),
        (
            "cross",
            "left,right",
            format!("umx:left/{}", ids[0]),
            format!("umx:right/{}", ids[3]),
        ),
        (
            "local_same_scope",
            "local,right",
            format!("umx:local/{}", ids[0]),
            format!("umx:local/{}", ids[3]),
        ),
    ] {
        let ctx = Ctx::new("bench").graph(selection);
        result.insert(
            format!("{name}_walk_depth_3"),
            distribution(runs, 3, || {
                svc.walk(
                    &ctx,
                    WalkRequest {
                        node: start.clone(),
                        depth: 3,
                        relation: Some("bench_hop".into()),
                        direction: Direction::Out,
                        within: None,
                        limit: 100,
                        offset: 0,
                    },
                )
                .map(|r| r.value.rows.len())
                .map_err(|p| p.code.into())
            })?,
        );
        result.insert(
            format!("{name}_path"),
            distribution(runs, 4, || {
                svc.path(
                    &ctx,
                    PathRequest {
                        from: start.clone(),
                        to: end.clone(),
                        max_depth: 3,
                        relation: Some("bench_hop".into()),
                        direction: Direction::Out,
                    },
                )
                .map(|r| r.value.rows.len())
                .map_err(|p| p.code.into())
            })?,
        );
        let at = if name.starts_with("local") {
            format!("umx:local/{}", ids[1])
        } else {
            format!("umx:left/{}", ids[1])
        };
        result.insert(
            format!("{name}_one_hop"),
            distribution(runs, 1, || {
                svc.walk(
                    &ctx,
                    WalkRequest {
                        node: at.clone(),
                        depth: 1,
                        relation: Some("bench_hop".into()),
                        direction: Direction::Out,
                        within: None,
                        limit: 100,
                        offset: 0,
                    },
                )
                .map(|r| r.value.rows.len())
                .map_err(|p| p.code.into())
            })?,
        );
        let req = CypherRequest {
            text: format!(
                "MATCH (a {{id: '{}'}})-[:bench_hop]->()-[:bench_hop]->(b) RETURN b.id AS id",
                ids[0]
            ),
            limit: None,
            offset: None,
            params: Vec::new(),
            along: None,
        };
        result.insert(
            format!("{name}_cypher_two_hops"),
            distribution(runs, 1, || {
                svc.query(&ctx, &req)
                    .map(|r| r.value.rows.len())
                    .map_err(|p| p.code.into())
            })?,
        );
    }
    let added_single_scope = result["cross_one_hop"]["p50_us"].as_f64().expect("number")
        - result["local_one_hop"]["p50_us"].as_f64().expect("number");
    result.insert(
        "external_hop_added_single_scope_p50_us".into(),
        json!(added_single_scope),
    );
    let local_ctx = Ctx::new("bench").graph("local,right");
    let cross_ctx = Ctx::new("bench").graph("left,right");
    let local_at = format!("umx:local/{}", ids[1]);
    let cross_at = format!("umx:left/{}", ids[1]);
    let one_hop = |cross| {
        svc.walk(
            if cross { &cross_ctx } else { &local_ctx },
            WalkRequest {
                node: if cross {
                    cross_at.clone()
                } else {
                    local_at.clone()
                },
                depth: 1,
                relation: Some("bench_hop".into()),
                direction: Direction::Out,
                within: None,
                limit: 100,
                offset: 0,
            },
        )
        .map(|r| r.value.rows.len())
        .map_err(|p| p.code.to_string())
    };
    assert_eq!((one_hop(false)?, one_hop(true)?), (1, 1));
    let mut pairs = Vec::new();
    let mut deltas = Vec::new();
    for run in 0..runs {
        let mut local = 0.0;
        let mut cross = 0.0;
        for crossing in if run % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let start = Instant::now();
            assert_eq!(black_box(one_hop(crossing)?), 1);
            let elapsed = start.elapsed().as_secs_f64() * 1e6;
            if crossing {
                cross = elapsed;
            } else {
                local = elapsed;
            }
        }
        deltas.push(cross - local);
        pairs.push(json!({"local_us": local, "cross_us": cross, "added_us": cross - local}));
    }
    let (p50, p95) = percentiles(&deltas);
    result.insert("external_hop_added_p50_us".into(), json!(p50));
    result.insert("external_hop_added_p95_us".into(), json!(p95));
    result.insert("one_hop_paired_samples".into(), json!(pairs));
    result.insert("topology".into(), json!({
        "nodes_per_graph": fx.node_count, "background_edges_per_graph": fx.edge_count,
        "loaded_graphs": 3, "cross_scope_graphs": 2, "local_single_scope_graphs": 1,
        "chain": "local: n0->n1->n2->n3; cross: left/n0->left/n1->right/n2->right/n3",
        "relation": "bench_hop", "one_hop": "n1 to n2", "scope": "local vs left,right",
        "timing": "complete services read including scope, policy and finish recheck; warmup before samples",
        "hop_control": "alternating paired reads with two selected graphs in both cases; local stays inside one graph. The single-selected-graph baseline is also reported separately.",
    }));
    Ok(Value::Object(result))
}
