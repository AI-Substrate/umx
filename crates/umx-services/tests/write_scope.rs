//! Write resolution never reads provenance (architect FU2, plan 188 item 3). A services caller can hand any
//! write a `Ctx` whose `include_provenance` is true (the CLI refuses `--provenance` on writes, a direct caller does not
//! have to). Every write that resolves a node must still answer node_not_found for a provenance node and change
//! nothing. One row per entry point: a new write verb that resolves a node gets a row here.

use std::sync::Arc;
use umx_core::codec::GraphFile;
use umx_core::ports::{GraphDir, GraphFiles};
use umx_core::provenance::{Actor, ActorInput, Tier};
use umx_core::{Id, Problem};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{
    BatchLine, BatchOp, BatchRequest, Ctx, EdgeEvidence, NodeAddRequest, NodeEditRequest,
    NodeNoteRequest, NodeStatusRequest, Ports, Services,
};

fn ctx() -> Ctx {
    Ctx::new("tester")
}

fn writer() -> Actor {
    Actor::try_from(ActorInput {
        tier: Tier::Pij,
        seat: Some("pij-alpha".into()),
        role: Some("coder".into()),
        model: None,
        effort: None,
        harness: Some("copilot".into()),
        harness_session: Some("h-1111111111111111".into()),
        user: Some("tester".into()),
    })
    .unwrap()
}

/// A write given (services, ctx, the provenance node, a content node), answering its problem codes, or none when
/// it succeeded.
type Write = fn(&mut Services, &Ctx, &str, &str) -> Vec<&'static str>;

fn codes<T>(r: Result<T, Problem>) -> Vec<&'static str> {
    r.map_or_else(|p| vec![p.code], |_| Vec::new())
}

fn batch(svc: &mut Services, c: &Ctx, op: BatchOp, raised_in: Option<&str>) -> Vec<&'static str> {
    let req = BatchRequest {
        lines: vec![BatchLine { line: 1, op }],
        raised_in: raised_in.map(str::to_string),
        ..Default::default()
    };
    svc.batch(c, req)
        .map_or_else(|ps| ps.iter().map(|p| p.code).collect(), |_| Vec::new())
}

fn node_add(t: &str, parent: bool) -> NodeAddRequest {
    NodeAddRequest {
        kind: "problem".into(),
        title: "Child".into(),
        parent: parent.then(|| t.to_string()),
        raised_in: (!parent).then(|| t.to_string()),
        ..Default::default()
    }
}

/// A status move `--in` the provenance node (ws008 TM6): the anchor resolves as an edge end does.
fn status_in(t: &str) -> NodeStatusRequest {
    NodeStatusRequest {
        status: "open".into(),
        reason: Some("why".into()),
        label: None,
        anchor: Some(t.into()),
        evidence: None,
        evidence_kind: None,
    }
}

const WRITES: &[(&str, Write)] = &[
    ("node add --parent", |s, c, t, _| {
        codes(s.node_add(c, node_add(t, true)))
    }),
    ("node add --raised-in", |s, c, t, _| {
        codes(s.node_add(c, node_add(t, false)))
    }),
    ("node edit", |s, c, t, _| {
        let req = NodeEditRequest {
            title: Some("Renamed".into()),
            ..Default::default()
        };
        codes(s.node_edit(c, t, req))
    }),
    ("entity key add", |s, c, t, _| {
        codes(s.entity_key(c, t, true, "email:someone@example.com"))
    }),
    ("node note", |s, c, t, _| {
        codes(s.node_note(c, t, "more", &[], None))
    }),
    ("node status", |s, c, t, _| {
        codes(s.node_status(c, t, "open", Some("why"), None))
    }),
    ("node status --in", |s, c, t, n| {
        codes(s.node_status_with(c, n, status_in(t)))
    }),
    ("node evidence add", |s, c, t, _| {
        codes(s.node_evidence_add(c, t, "docs/a.md#L1", None))
    }),
    ("node evidence remove", |s, c, t, _| {
        codes(s.node_evidence_remove(c, t, "docs/a.md#L1", Some("stale"), None))
    }),
    ("edge add, from", |s, c, t, n| {
        codes(s.edge_add(c, t, "supports", n, &[]))
    }),
    ("edge add, to", |s, c, t, n| {
        codes(s.edge_add(c, n, "supports", t, &[]))
    }),
    ("entity link", |s, c, t, n| {
        codes(s.entity_link(c, n, t, false))
    }),
    ("entity link --dry-run", |s, c, t, n| {
        codes(s.entity_link(c, t, n, true))
    }),
    ("entity deny", |s, c, t, n| {
        codes(s.entity_deny(c, n, t, Some("not the same")))
    }),
    ("entity unlink", |s, c, t, n| {
        codes(s.entity_unlink(c, t, n))
    }),
    ("entity undeny", |s, c, t, n| {
        codes(s.entity_undeny(c, n, t))
    }),
    ("embeddings allow --node", |s, c, t, _| {
        codes(s.embeddings_allow(c, t, "title"))
    }),
    ("batch --raised-in", |s, c, t, _| {
        let op = BatchOp::NodeAdd {
            id: None,
            created: None,
            ref_name: None,
            req: NodeAddRequest {
                kind: "problem".into(),
                title: "Batched".into(),
                ..Default::default()
            },
            parent_ref: None,
        };
        batch(s, c, op, Some(t))
    }),
    ("batch node add parent", |s, c, t, _| {
        let op = BatchOp::NodeAdd {
            id: None,
            created: None,
            ref_name: None,
            req: node_add(t, true),
            parent_ref: None,
        };
        batch(s, c, op, None)
    }),
    ("batch node add raised_in", |s, c, t, _| {
        let op = BatchOp::NodeAdd {
            id: None,
            created: None,
            ref_name: None,
            req: node_add(t, false),
            parent_ref: None,
        };
        batch(s, c, op, None)
    }),
    ("batch edge add", |s, c, t, n| {
        let op = BatchOp::EdgeAdd {
            id: None,
            from: Some(n.into()),
            from_ref: None,
            relation: "supports".into(),
            to: Some(t.into()),
            to_ref: None,
            sets: Vec::new(),
            evidence: EdgeEvidence::default(),
        };
        batch(s, c, op, None)
    }),
    ("batch node edit", |s, c, t, _| {
        let op = BatchOp::NodeEdit {
            node: Some(t.into()),
            node_ref: None,
            req: NodeEditRequest {
                title: Some("Renamed".into()),
                ..Default::default()
            },
        };
        batch(s, c, op, None)
    }),
    ("batch node note", |s, c, t, _| {
        let op = BatchOp::NodeNote {
            node: Some(t.into()),
            node_ref: None,
            req: NodeNoteRequest {
                append: "more".into(),
                evidence: Vec::new(),
            },
        };
        batch(s, c, op, None)
    }),
    ("batch node status", |s, c, t, _| {
        let op = BatchOp::NodeStatus {
            node: Some(t.into()),
            node_ref: None,
            req: NodeStatusRequest {
                status: "open".into(),
                reason: Some("why".into()),
                label: None,
                anchor: None,
                evidence: None,
                evidence_kind: None,
            },
        };
        batch(s, c, op, None)
    }),
    ("batch node status in", |s, c, t, n| {
        let op = BatchOp::NodeStatus {
            node: Some(n.into()),
            node_ref: None,
            req: status_in(t),
        };
        batch(s, c, op, None)
    }),
    ("batch evidence add", |s, c, t, _| {
        let op = BatchOp::EvidenceAdd {
            node: Some(t.into()),
            node_ref: None,
            pointer: "docs/a.md#L1".into(),
            evidence_kind: None,
        };
        batch(s, c, op, None)
    }),
    ("batch evidence remove", |s, c, t, _| {
        let op = BatchOp::EvidenceRemove {
            node: Some(t.into()),
            node_ref: None,
            pointer: "docs/a.md#L1".into(),
            reason: Some("stale".into()),
            at: None,
            skip_missing: false,
        };
        batch(s, c, op, None)
    }),
];

#[test]
fn every_node_resolving_write_refuses_a_provenance_node_even_when_the_ctx_opts_in() {
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
    svc.graph_new(&ctx(), "/work/g", "demo", None, false, None)
        .unwrap();
    let content = svc
        .node_add(
            &ctx().actor(writer()),
            NodeAddRequest {
                kind: "problem".into(),
                title: "Content".into(),
                evidence: vec!["docs/a.md#L1".into()],
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id
        .to_text();
    let graph = svc.loaded_graph("demo").unwrap();
    let session: Id = graph
        .provenance_node("session/copilot/h-1111111111111111")
        .unwrap()
        .id();
    let dir = GraphDir::parse("/work/g").unwrap();
    let files_now = || {
        [GraphFile::Nodes, GraphFile::Edges]
            .map(|f| files.read(&dir, f).unwrap().unwrap_or_default())
    };
    let before = files_now();
    // Positive control: the same id resolves for a read that opts in, so a write that read through the request's
    // scope would find it.
    assert!(
        svc.node_get(&ctx().with_provenance(true), &session.to_text())
            .is_ok()
    );
    let mut wrong = Vec::new();
    let target = session.to_text();
    for (name, write) in WRITES {
        let got = write(
            &mut svc,
            &ctx().with_provenance(true).actor(writer()),
            &target,
            &content,
        );
        if !got.contains(&"node_not_found") {
            wrong.push(format!("{name}: {got:?}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    assert!(
        files_now() == before,
        "a refused write changed the graph's files"
    );
}
