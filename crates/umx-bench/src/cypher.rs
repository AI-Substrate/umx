//! Cypher rows (workstream 005 s02): umx-query's `run` over the synthetic graph, built as the storage keys build
//! it. Two query sets: the design fixture's queries (crates/umx-cypher/fixtures/design, design ids mapped to fixed
//! nodes of this graph) and workshop 006's query set (fixtures/cypher-006-queries.json, its `{X}`-style anchors
//! picked from this graph). Per query: the median and minimum of `runs` runs, the row count as a check, and the
//! peak bytes allocated while it ran. A query the executor refuses is recorded with its message, not timed.
//! Timed at the API, without services; main.rs's `query_q5_ms` times q5 through services (s04, architect).
//!
//! `similar` (workstream 005 s05): queries that name similar_to, over lists injected after the other rows ran
//! ([`SIMILAR_K`] entries per node, as the default settings make), reported apart so the rows above stay comparable
//! with a base that has no similar_to in Cypher.

use crate::alloc;
use crate::fixture::Fixture;
use crate::storage::{build, per_op};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use umx_graph::{Graph, NodeIx};
use umx_query::{GraphView, RunOptions, run};

const DESIGN: &str = include_str!("../../umx-cypher/fixtures/design/queries.json");
const WORKSHOP_006: &str = include_str!("../fixtures/cypher-006-queries.json");

/// The design fixture's ids are `01JDCYPHER` + zeros + a short name (fixtures/design/README.md).
const DESIGN_ID_PREFIX: &str = "01JDCYPHER";

/// The node with the most edges of `rel` arriving (`incoming`) or leaving, lowest handle on a tie.
fn busiest(g: &Graph, rel: &str, incoming: bool) -> NodeIx {
    g.nodes()
        .map(|n| {
            let steps: Box<dyn Iterator<Item = umx_graph::Step<'_>>> = if incoming {
                Box::new(g.edges_in(n.ix()))
            } else {
                Box::new(g.edges_out(n.ix()))
            };
            (steps.filter(|s| s.kind.as_str() == rel).count(), n.ix())
        })
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
        .map(|(_, ix)| ix)
        .expect("a non-empty graph")
}

/// The queries of one set with their anchors replaced by ids of this graph.
fn anchored(g: &Graph, set: &str) -> Vec<(String, String, String)> {
    let id = |ix: NodeIx| g.node_at(ix).id().to_text();
    let n = g.node_count();
    let anchors: BTreeMap<&str, String> = [
        ("{X}", id(busiest(g, "blocks", true))),
        ("{X2}", id(busiest(g, "depends_on", false))),
        ("{A}", id(busiest(g, "blocks", false))),
        (
            "{B}",
            g.node_at(g.nodes().nth(n / 2).expect("nodes").ix())
                .id()
                .to_text(),
        ),
    ]
    .into_iter()
    .collect();
    let text = if set == "design" {
        DESIGN
    } else {
        WORKSHOP_006
    };
    let all: Value = serde_json::from_str(text).expect("query set json");
    let mut design_ids: Vec<String> = Vec::new();
    all.as_array()
        .expect("a query list")
        .iter()
        .map(|q| {
            let mut cypher = q["cypher"].as_str().expect("cypher").to_string();
            for (k, v) in &anchors {
                cypher = cypher.replace(k, v);
            }
            // Each distinct design id becomes the node at a fixed position, in order of first use.
            while let Some(at) = cypher.find(DESIGN_ID_PREFIX) {
                let old = cypher[at..at + 26].to_string();
                let k = match design_ids.iter().position(|d| *d == old) {
                    Some(k) => k,
                    None => {
                        design_ids.push(old.clone());
                        design_ids.len() - 1
                    }
                };
                let new = id(g.nodes().nth((k * 7919 + 1) % n).expect("nodes").ix());
                cypher = cypher.replace(&old, &new);
            }
            let variant = q["variant"].as_str().unwrap_or("").to_string();
            (q["id"].as_str().expect("id").to_string(), variant, cypher)
        })
        .collect()
}

/// Queries that must stop at the budget or the row cap (security's F2): the time and peak memory a refusal
/// takes are the figures that matter.
const BUDGET_PROBES: [(&str, &str); 2] = [
    ("product-3", "MATCH (a), (b), (c) RETURN count(*) AS n"),
    (
        "walk-untyped-1-10",
        "MATCH (a)-[*1..10]-(b) RETURN count(*) AS n",
    ),
];

/// Dates compared by instant (ws008 TM2): every node's `created` text sorted, filtered against a month and
/// folded by min and max, so the cost of reading each text as a date shows against the rows above.
const DATE_QUERIES: [(&str, &str); 3] = [
    (
        "order-by-created",
        "MATCH (n) RETURN n.id AS id, n.created AS created ORDER BY created",
    ),
    (
        "where-created-month",
        "MATCH (n) WHERE n.created < \"2026-11\" RETURN count(*) AS n",
    ),
    (
        "min-max-created",
        "MATCH (n) RETURN min(n.created) AS first, max(n.created) AS last",
    ),
];

/// Who cites a source: every node's evidence sources decoded from its line (a cold read, as `n.note`), by
/// membership and by prefix, next to a plain note read for scale. On the plain fixture no node has evidence.
const EVIDENCE_QUERIES: [(&str, &str); 3] = [
    (
        "sources-in",
        "MATCH (n) WHERE 'transcripts/call-007.ndjson' IN n.evidence_sources RETURN count(*) AS hits",
    ),
    (
        "sources-prefix",
        "MATCH (n) UNWIND n.evidence_sources AS s WITH n, s WHERE s STARTS WITH 'transcripts/call-00' \
         RETURN count(DISTINCT n) AS hits",
    ),
    (
        "note-read",
        "MATCH (n) WHERE n.note STARTS WITH 'zz' RETURN count(*) AS hits",
    ),
];

/// Entries per similar_to list in the injected lists: the default k.
const SIMILAR_K: usize = 10;

/// Queries over the similar_to lists: req-65's unbound score filter, as a count and as rows, and a bound walk of 2.
const SIMILAR_QUERIES: [(&str, &str); 3] = [
    (
        "unbound-score-count",
        "MATCH (a)-[s:similar_to]->(b) WHERE s.score > 0.8 RETURN count(*) AS n",
    ),
    (
        "unbound-score-rows",
        "MATCH (a)-[s:similar_to]->(b) WHERE s.score > 0.8 RETURN b",
    ),
    (
        "bound-walk-1-2",
        "MATCH (a {id: '{B}'})-[:similar_to*1..2]->(b) RETURN count(*) AS n",
    ),
];

/// Every node a full list: [`SIMILAR_K`] distinct others picked by a fixed stride, scores falling from 0.95 by
/// 0.03 a place (so the first five pass `> 0.8`). Deterministic, no vectors needed.
fn inject_similar(g: &mut Graph) {
    let ids: Vec<umx_core::Id> = g.nodes().map(|n| n.id()).collect();
    let n = ids.len();
    let mut load = g.load_similar("bench-model@256", SIMILAR_K, 0);
    for (i, &a) in ids.iter().enumerate() {
        for j in 0..SIMILAR_K.min(n.saturating_sub(1)) {
            let other = ids[(i + 1 + (j * 7919 + i * 31) % (n - 1)) % n];
            load.put(a, other, 9500 - 300 * j as i32);
        }
    }
    load.finish();
}

pub fn measure(runs: usize, fx: &Fixture) -> Value {
    let mut g = build(fx);
    // The fixture's ontology is the starter one: its symmetric relations are the built-ins (D49).
    let ontology = umx_core::Ontology::starter();
    let symmetric = ontology.symmetric_kinds();
    let view = GraphView::one("bench", &g, &symmetric);
    let options = RunOptions::default();
    let b_anchor = g
        .node_at(g.nodes().nth(g.node_count() / 2).expect("nodes").ix())
        .id()
        .to_text();
    let mut out = Vec::new();
    let (
        mut slowest,
        mut slowest_id,
        mut max_peak,
        mut max_peak_id,
        mut sum,
        mut max_work,
        mut max_work_id,
    ) = (
        0.0_f64,
        String::new(),
        0_usize,
        String::new(),
        0.0_f64,
        0_u64,
        String::new(),
    );
    let mut queries: Vec<(String, String, String, String)> = Vec::new();
    for set in ["design", "workshop-006"] {
        for (id, variant, cypher) in anchored(&g, set) {
            queries.push((set.to_string(), id, variant, cypher));
        }
    }
    for (id, cypher) in BUDGET_PROBES {
        queries.push((
            "budget".to_string(),
            id.to_string(),
            "probe".to_string(),
            cypher.to_string(),
        ));
    }
    for (id, cypher) in DATE_QUERIES {
        queries.push((
            "dates".to_string(),
            id.to_string(),
            "tm2".to_string(),
            cypher.to_string(),
        ));
    }
    for (id, cypher) in EVIDENCE_QUERIES {
        queries.push((
            "evidence".to_string(),
            id.to_string(),
            "scan".to_string(),
            cypher.to_string(),
        ));
    }
    for (set, id, variant, cypher) in queries {
        let base = alloc::live();
        alloc::reset_peak();
        let started = std::time::Instant::now();
        let first = run(&view, &cypher, &options);
        let first_ms = (started.elapsed().as_secs_f64() * 1e6).round() / 1e3;
        let peak = alloc::peak().saturating_sub(base);
        let (total, work) = match first {
            Ok(r) => (r.total, r.work),
            Err(e) => {
                let slots: Vec<String> = e.slots.iter().map(|(n, v)| format!("{n}={v}")).collect();
                out.push(json!({
                    "set": set, "id": id, "variant": variant, "refused": e.code, "slots": slots,
                    "first_ms": first_ms, "peak_bytes": peak,
                }));
                continue;
            }
        };
        let (time, checked) = per_op(runs, 1, 1e3, || {
            run(&view, &cypher, &options).map(|r| r.total).ok()
        });
        assert_eq!(checked, Some(total), "{id} changed its row count");
        let median = time["median"].as_f64().unwrap_or(0.0);
        let name = format!("{set}/{id}/{variant}");
        sum += median;
        if median > slowest {
            (slowest, slowest_id) = (median, name.clone());
        }
        if peak > max_peak {
            (max_peak, max_peak_id) = (peak, name.clone());
        }
        if work > max_work {
            (max_work, max_work_id) = (work, name);
        }
        out.push(json!({
            "set": set, "id": id, "variant": variant, "rows": total, "work": work, "ms": time, "peak_bytes": peak,
        }));
    }
    drop(view);
    inject_similar(&mut g);
    let view = GraphView::one("bench", &g, &symmetric);
    let mut similar = Vec::new();
    for (id, cypher) in SIMILAR_QUERIES {
        let cypher = cypher.replace("{B}", &b_anchor);
        let base = alloc::live();
        alloc::reset_peak();
        let first = run(&view, &cypher, &options);
        let peak = alloc::peak().saturating_sub(base);
        let r = first.unwrap_or_else(|e| panic!("{id} refused: {}", e.code));
        let (time, checked) = per_op(runs, 1, 1e3, || {
            run(&view, &cypher, &options).map(|r| r.total).ok()
        });
        assert_eq!(checked, Some(r.total), "{id} changed its row count");
        similar.push(json!({
            "id": id, "rows": r.total, "work": r.work, "ms": time, "peak_bytes": peak,
        }));
    }
    eprintln!("umx-bench: cypher done");
    json!({
        "similar": { "k": SIMILAR_K, "queries": similar },
        "queries": out,
        "slowest_ms": slowest, "slowest": slowest_id,
        "max_peak_bytes": max_peak, "max_peak": max_peak_id,
        "max_work": max_work, "max_work_query": max_work_id,
        "sum_of_medians_ms": (sum * 1e3).round() / 1e3,
        "budget": { "max_work": umx_cypher_limits().0, "max_rows": umx_cypher_limits().1 },
    })
}

/// The default work budget and row cap the rows ran under.
fn umx_cypher_limits() -> (u64, usize) {
    let o = RunOptions::default();
    (o.max_work, o.max_rows)
}
