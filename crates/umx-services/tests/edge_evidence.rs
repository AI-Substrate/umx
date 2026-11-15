//! ws006 S5b (req-124) as the second lander after ws007 P2 and plan 107's write-time secret scan: each new edge
//! evidence pointer records the writing session as `by_session`, exactly as node evidence does, and a pointer that
//! looks like a secret is warned by `edge add` and refused by a batch, as node evidence is.

use serde_json::{Value, json};
use std::sync::Arc;
use umx_core::codec::{self, GraphFile};
use umx_core::ports::{GraphDir, GraphFiles};
use umx_core::provenance::{Actor, ActorInput, Tier};
use umx_core::{Edge, Id};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{
    BatchLine, BatchOp, BatchRequest, Ctx, EdgeEvidence, NodeAddRequest, Ports, Services,
};

struct World {
    files: Arc<FakeGraphFiles>,
    svc: Services,
}

fn world() -> World {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let ports = Ports {
        sources: Arc::new(umx_fakes::FakeSourceReader::open(
            GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: Arc::new(FakeGraphFiles::new(GraphDir::parse("/cache").unwrap())),
        vector_cache_root: GraphDir::parse("/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    };
    let mut svc = Services::new(ports);
    svc.graph_new(&Ctx::new("tester"), "/work/g", "demo", None, false, None)
        .unwrap();
    World { files, svc }
}

/// A writer whose harness session is `session`: each one records its own session node.
fn writer(session: &str) -> Ctx {
    Ctx::new("tester").actor(
        Actor::try_from(ActorInput {
            tier: Tier::Pij,
            seat: Some("pij-alpha-seat".into()),
            role: Some("coder".into()),
            model: Some("openai/test-model".into()),
            effort: Some("high".into()),
            harness: Some("copilot".into()),
            harness_session: Some(session.into()),
            user: Some("tester".into()),
        })
        .unwrap(),
    )
}

fn node(w: &mut World, title: &str) -> String {
    w.svc
        .node_add(
            &Ctx::new("tester"),
            NodeAddRequest {
                kind: "problem".into(),
                title: title.into(),
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id
        .to_text()
}

fn evidence(pointers: &[&str]) -> EdgeEvidence {
    EdgeEvidence {
        pointers: pointers.iter().map(|p| (*p).to_string()).collect(),
        kind: None,
    }
}

fn edges_file(w: &World) -> Vec<u8> {
    w.files
        .read(&GraphDir::parse("/work/g").unwrap(), GraphFile::Edges)
        .unwrap()
        .unwrap()
}

fn on_disk(w: &World, id: Id) -> Edge {
    codec::decode_edges(&edges_file(w))
        .unwrap()
        .records
        .into_iter()
        .find(|e| e.id == id)
        .unwrap()
}

fn batch_edge(from: &str, to: &str, pointers: &[&str]) -> BatchRequest {
    BatchRequest {
        lines: vec![BatchLine {
            line: 1,
            op: BatchOp::EdgeAdd {
                id: None,
                from: Some(from.into()),
                from_ref: None,
                relation: "blocks".into(),
                to: Some(to.into()),
                to_ref: None,
                sets: Vec::new(),
                evidence: evidence(pointers),
            },
        }],
        ..Default::default()
    }
}

/// P2 on edges: a new edge's pointers carry its author session; pointers appended later, by edge add or a batch,
/// carry the appending session (also the edge's new `edited_by_session`), and earlier pointers keep theirs.
#[test]
fn edge_evidence_records_the_session_that_added_each_pointer() {
    let mut w = world();
    let (a, b) = (node(&mut w, "One"), node(&mut w, "Two"));
    let id = w
        .svc
        .edge_add_with(
            &writer("h-0123456789abcdef"),
            &a,
            "blocks",
            &b,
            &[],
            &evidence(&["docs/a.md#L3"]),
        )
        .unwrap()
        .value
        .edge
        .id;
    let e = on_disk(&w, id);
    let author = e
        .authored_by_session
        .expect("P2 records the author")
        .to_text();
    assert_eq!(e.evidence[0]["by_session"], author);
    assert_eq!(e.edited_by_session, None, "creation is not an edit");

    let done = w
        .svc
        .edge_add_with(
            &writer("h-fedcba9876543210"),
            &a,
            "blocks",
            &b,
            &[],
            &evidence(&["docs/a.md#L3", "docs/b.md#L1"]),
        )
        .unwrap();
    assert_eq!(done.notes[0].code, "evidence_appended");
    let e = on_disk(&w, id);
    let appender = e.edited_by_session.expect("an append is an edit").to_text();
    assert_ne!(appender, author);
    assert_eq!(
        e.evidence
            .iter()
            .map(|p| p["by_session"].clone())
            .collect::<Vec<Value>>(),
        [json!(author), json!(appender)],
        "the repeated pointer keeps its adder"
    );

    w.svc
        .batch(
            &writer("h-00000000000000aa"),
            batch_edge(&a, &b, &["docs/c.md#L2"]),
        )
        .unwrap();
    let e = on_disk(&w, id);
    let batcher = e.edited_by_session.unwrap().to_text();
    assert!(batcher != author && batcher != appender);
    assert_eq!(e.evidence.len(), 3);
    assert_eq!(e.evidence[1]["by_session"], appender);
    assert_eq!(e.evidence[2]["by_session"], batcher);
}

/// Plan 107's write scan on edges: a pointer that looks like a secret is stored with `secret_suspected` (field
/// `evidence`) by edge add, and refused as a batch line (`batch_line_invalid`, naming the field and rule, never the
/// text) by a batch, new edge or append, writing nothing.
#[test]
fn a_secret_shaped_edge_pointer_is_warned_by_edge_add_and_refused_by_a_batch() {
    let mut w = world();
    let (a, b, c) = (
        node(&mut w, "One"),
        node(&mut w, "Two"),
        node(&mut w, "Three"),
    );
    let token = format!("ghp_{}", "a".repeat(36))
        .replace('g', "%67")
        .replace('_', "%5F");
    let secret = format!("a.md#quote={token}");

    let added = w
        .svc
        .edge_add_with(
            &Ctx::new("tester"),
            &a,
            "blocks",
            &b,
            &[],
            &evidence(&[&secret]),
        )
        .unwrap();
    let warned: Vec<&str> = added.notes.iter().map(|p| p.code).collect();
    assert_eq!(warned, ["secret_suspected"]);
    assert!(
        !format!("{:?}", added.notes).contains("aaaaaaaa"),
        "never echoed"
    );

    let before = edges_file(&w);
    // A new edge, then an append to a-b (a pointer that edge does not hold yet, so the scan sees it as new).
    let other = format!("b.md#quote={token}");
    for (from, to) in [(&a, &c), (&a, &b)] {
        let refused = w
            .svc
            .batch(
                &Ctx::new("tester"),
                batch_edge(from, to, &[&other, "docs/ok.md#L1"]),
            )
            .unwrap_err();
        let line = refused
            .iter()
            .find(|p| p.code == "batch_line_invalid")
            .unwrap_or_else(|| panic!("{refused:?}"));
        let said = format!("{line:?}");
        assert!(said.contains("evidence matches the secret rule"), "{said}");
        assert!(!format!("{refused:?}").contains("aaaaaaaa"), "never echoed");
        assert_eq!(edges_file(&w), before, "nothing written");
    }
}
