//! ws008 E2 cost (the entity design, section 5): `umx entity show` at the top of the stated scale, 50 graphs of
//! 2,000 nodes, 10% of nodes keyed (every key in every graph, so each keyed entity has 50 members), and one chain of
//! `same_as` links through all 50 graphs. Prints medians; never gates (D8). Run it in release:
//! `cargo test -p umx-services --release --test entity_cost -- --ignored --nocapture`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;
use umx_core::codec::GraphFile;
use umx_core::ports::{GraphDir, IdGenerator};
use umx_core::setup::GraphFilesSetup;
use umx_core::{Edge, Id, Node};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{Ctx, Ports, Services};

const GRAPHS: usize = 50;
const NODES: usize = 2_000;

fn node(id: Id, i: usize) -> Node {
    let mut fields = BTreeMap::new();
    if i.is_multiple_of(10) {
        fields.insert(
            "entity_key".to_string(),
            serde_json::json!([format!("email:p{}@example.com", i / 10)]),
        );
    }
    Node {
        id,
        kind: "person".into(),
        title: format!("Person {i}"),
        status: "open".into(),
        note: String::new(),
        evidence: Vec::new(),
        status_history: Vec::new(),
        created: "2026-11-12T00:00:00Z".into(),
        updated: "2026-11-12T00:00:00Z".into(),
        created_by: "bench".into(),
        locator: None,
        authored_by_session: None,
        edited_by_session: None,
        fields,
    }
}

fn median(mut ms: Vec<f64>) -> f64 {
    ms.sort_by(f64::total_cmp);
    ms[ms.len() / 2]
}

#[test]
#[ignore = "release-only measurement"]
fn entity_show_at_fifty_graphs_of_two_thousand_nodes() {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let ids = Arc::new(FakeIds::new());
    let ports = Ports {
        sources: Arc::new(umx_fakes::FakeSourceReader::open(
            GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: Arc::new(FakeGraphFiles::new(GraphDir::parse("/cache").unwrap())),
        vector_cache_root: GraphDir::parse("/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: ids.clone(),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    };
    let mut svc = Services::new(ports);
    let ctx = Ctx::new("bench");
    let mut graph_ids = Vec::new();
    let mut node_ids: Vec<Vec<Id>> = Vec::new();
    for g in 0..GRAPHS {
        let path = format!("/work/g{g}");
        let made = svc
            .graph_new(&ctx, &path, &format!("g{g}"), None, false, None)
            .unwrap();
        graph_ids.push(made.value.graph.id);
        node_ids.push((0..NODES).map(|_| ids.next_id()).collect());
    }
    let started = Instant::now();
    for g in 0..GRAPHS {
        let dir = GraphDir::parse(&format!("/work/g{g}")).unwrap();
        let mut nodes = String::new();
        for (i, id) in node_ids[g].iter().enumerate() {
            nodes.push_str(&serde_json::to_string(&node(*id, i)).unwrap());
            nodes.push('\n');
        }
        // Node 1 of each graph links to node 1 of the next: one entity through all 50 graphs by same_as only.
        let mut edges = String::new();
        if g + 1 < GRAPHS {
            let edge = Edge {
                id: ids.next_id(),
                kind: "same_as".into(),
                from_graph: None,
                from: node_ids[g][1],
                to_graph: Some(graph_ids[g + 1]),
                to: node_ids[g + 1][1],
                created: "2026-11-12T00:00:00Z".into(),
                created_by: "bench".into(),
                authored_by_session: None,
                edited_by_session: None,
                evidence: Vec::new(),
                fields: BTreeMap::new(),
            };
            edges.push_str(&serde_json::to_string(&edge).unwrap());
            edges.push('\n');
        }
        files
            .write(&dir, GraphFile::Nodes, nodes.as_bytes())
            .unwrap();
        files
            .write(&dir, GraphFile::Edges, edges.as_bytes())
            .unwrap();
        svc.graph_load(&ctx, &format!("/work/g{g}"), None).unwrap();
    }
    let load_s = started.elapsed().as_secs_f64();
    let mut report = Vec::new();
    for (what, node, want) in [
        ("keyed entity, 50 members by key", 0, GRAPHS),
        ("linked entity, 50 members by same_as", 1, GRAPHS),
        ("unkeyed, unlinked node, 1 member", 2, 1),
    ] {
        let address = format!("umx:g0/{}", node_ids[0][node]);
        let mut ms = Vec::new();
        for _ in 0..21 {
            let t = Instant::now();
            let shown = svc.entity_show(&ctx, &address).unwrap().value;
            ms.push(t.elapsed().as_secs_f64() * 1e3);
            assert_eq!(shown.members.len(), want, "{what}");
            assert_eq!(shown.graphs.len(), GRAPHS);
        }
        report.push(format!("{what}: median {:.3} ms", median(ms)));
    }
    println!(
        "entity show over {GRAPHS} graphs x {NODES} nodes (10% keyed, a 50-graph same_as chain), loaded in {load_s:.1} s:\n{}",
        report.join("\n")
    );
}
