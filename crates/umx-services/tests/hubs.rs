//! PRD 003 hubs over the fakes: declarations, auto-joins on set, fill, add, edit and batch, the `auto_added_by` mark,
//! edges umx never removes, a missing hub node, and spines that never read a hub edge.

use serde_json::json;
use std::sync::Arc;
use umx_core::codec::{self, GraphFile};
use umx_core::ports::{GraphDir, GraphFiles};
use umx_core::value::SetValue;
use umx_core::{Id, Problem};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{
    BatchLine, BatchOp, BatchRequest, Ctx, EdgeListRequest, HubSetRequest, NodeAddRequest,
    NodeEditRequest, Ports, Services,
};

struct World {
    files: Arc<FakeGraphFiles>,
    svc: Services,
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

fn demo() -> World {
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
        registry: Arc::new(FakeLocalState::new().with_absent_ok(true)),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    };
    let mut svc = Services::new(ports);
    svc.graph_new(&ctx(), "/work/g", "demo", None, false, None)
        .unwrap();
    World { files, svc }
}

fn codes(w: &[Problem]) -> Vec<&str> {
    w.iter().map(|p| p.code).collect()
}

fn add(w: &mut World, kind: &str, title: &str, raised_in: Option<Id>) -> umx_services::NodeAdded {
    w.svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: kind.into(),
                title: title.into(),
                raised_in: raised_in.map(Id::to_text),
                ..Default::default()
            },
        )
        .unwrap()
        .value
}

fn set(name: &str, node: Option<Id>, type_hub: Option<&str>, via: Option<&str>) -> HubSetRequest {
    HubSetRequest {
        name: name.into(),
        node: node.map(Id::to_text),
        type_hub: type_hub.map(Into::into),
        via: via.map(Into::into),
        pattern: None,
    }
}

fn register(w: &mut World) -> Id {
    let id = add(w, "topic", "Risk register", None).node.id;
    let mut req = set("risk_register", Some(id), None, None);
    req.pattern = Some(json!({"type": ["risk"]}));
    w.svc.hub_set(&ctx(), req).unwrap();
    id
}

fn graph_json(w: &World) -> Vec<u8> {
    w.files
        .read(&GraphDir::parse("/work/g").unwrap(), GraphFile::Graph)
        .unwrap()
        .unwrap()
}

fn edges_json(w: &World) -> String {
    String::from_utf8(
        w.files
            .read(&GraphDir::parse("/work/g").unwrap(), GraphFile::Edges)
            .unwrap()
            .unwrap_or_default(),
    )
    .unwrap()
}

/// The edges `edge list --where auto_added_by=<hub>` lists, by (from, relation, to).
fn auto(w: &World, hub: &str) -> Vec<(Id, String, Id)> {
    let page = w
        .svc
        .edge_list(
            &ctx(),
            EdgeListRequest {
                wheres: vec![format!("auto_added_by={hub}")],
                ..Default::default()
            },
        )
        .unwrap()
        .value;
    let mut out: Vec<_> = page
        .rows
        .iter()
        .map(|r| (r.edge.from, r.edge.kind.clone(), r.edge.to))
        .collect();
    out.sort();
    out
}

fn retype(w: &mut World, node: Id, kind: &str) -> umx_services::Done<umx_services::NodeEdited> {
    w.svc
        .node_edit(
            &ctx(),
            &node.to_text(),
            NodeEditRequest {
                kind: Some(kind.into()),
                ..Default::default()
            },
        )
        .unwrap()
}

/// rules-16: declaring a named hub with a pattern joins every match in one write and lists them; the same
/// declaration again writes nothing; a type hub with a pattern is refused; graph.json takes format 9 only then.
#[test]
fn hub_set_backfills_matches_in_one_write_and_refuses_a_patterned_type_hub() {
    let mut w = demo();
    let before = graph_json(&w);
    let risks: Vec<Id> = (0..3)
        .map(|i| add(&mut w, "risk", &format!("Risk {i}"), None).node.id)
        .collect();
    add(&mut w, "question", "Not a risk", None);
    let hub = add(&mut w, "topic", "Risk register", None).node.id;
    assert_eq!(codec::decode_graph(&before).unwrap().umx < 9, true);
    let mut req = set("risk_register", Some(hub), None, None);
    req.pattern = Some(json!({"type": ["risk"]}));
    let done = w.svc.hub_set(&ctx(), req.clone()).unwrap();
    let mut joined: Vec<Id> = done.value.joined.iter().map(|j| j.node).collect();
    joined.sort();
    assert_eq!(joined, risks);
    assert_eq!(done.value.joined_total, 3);
    assert_eq!(done.value.format, 9);
    assert!(codes(&done.notes).contains(&"hub_joined"));
    assert_eq!(
        auto(&w, "risk_register"),
        risks
            .iter()
            .map(|r| (*r, "part_of".to_string(), hub))
            .collect::<Vec<_>>()
    );
    let stored = codec::decode_graph(&graph_json(&w)).unwrap();
    assert_eq!(stored.umx, 9);
    assert_eq!(stored.hubs.len(), 1);
    let (g, e) = (graph_json(&w), edges_json(&w));
    let again = w.svc.hub_set(&ctx(), req).unwrap().value;
    assert!(again.unchanged);
    assert_eq!((graph_json(&w), edges_json(&w)), (g, e));
    let mut bad = set("meetings", None, Some("meeting"), Some("raised_in"));
    bad.pattern = Some(json!({"type": ["risk"]}));
    assert_eq!(w.svc.hub_set(&ctx(), bad).unwrap_err().code, "hub_invalid");
    assert_eq!(
        w.svc
            .hub_set(&ctx(), set("meetings", None, Some("meeting"), None))
            .unwrap_err()
            .code,
        "hub_invalid"
    );
}

/// rules-15 and rules-17: a named risk register and a type hub over meetings; a risk raised in a meeting is linked to
/// it by the writer and added to the register by umx in the same write, and shows under both.
#[test]
fn named_and_type_hubs_list_and_show_and_a_risk_joins_both() {
    let mut w = demo();
    let hub = register(&mut w);
    w.svc
        .hub_set(
            &ctx(),
            set("meetings", None, Some("meeting"), Some("raised_in")),
        )
        .unwrap();
    let m1 = add(&mut w, "meeting", "Monday sync", None).node.id;
    let m2 = add(&mut w, "meeting", "Tuesday sync", None).node.id;
    let done = w
        .svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "risk".into(),
                title: "Supplier may slip".into(),
                raised_in: Some(m1.to_text()),
                ..Default::default()
            },
        )
        .unwrap();
    let risk = done.value.node.id;
    assert!(done.value.raised_in_edge.is_some());
    assert_eq!(done.value.hub_edges.len(), 1);
    assert_eq!(
        (done.value.hub_edges[0].node, done.value.hub_edges[0].to),
        (risk, hub)
    );
    assert!(codes(&done.notes).contains(&"hub_joined"));
    let list = w.svc.hub_list(&ctx()).unwrap().value;
    let names: Vec<_> = list.rows.iter().map(|r| r.hub.name.as_str()).collect();
    assert_eq!(names, ["meetings", "risk_register"]);
    assert_eq!((list.rows[0].hub_nodes, list.rows[0].members), (2, 1));
    assert_eq!((list.rows[1].hub_nodes, list.rows[1].members), (1, 1));
    assert_eq!(
        list.rows[1].add.as_str(),
        format!("umx edge add <node> part_of {} --graph demo", hub.to_text())
    );
    let meetings = w.svc.hub_show(&ctx(), "meetings", 20, 0).unwrap().value;
    let rows: Vec<_> = meetings
        .nodes
        .iter()
        .map(|n| (n.node, n.members.clone()))
        .collect();
    assert_eq!(rows, [(m1, vec![risk]), (m2, vec![])]);
    let register = w
        .svc
        .hub_show(&ctx(), "risk_register", 20, 0)
        .unwrap()
        .value;
    assert_eq!(register.nodes[0].members, [risk]);
    // A hand-added member: the ordinary edge, with no mark.
    let q = add(&mut w, "question", "Who owns it?", None).node.id;
    w.svc
        .edge_add(&ctx(), &q.to_text(), "part_of", &hub.to_text(), &[])
        .unwrap();
    assert_eq!(auto(&w, "risk_register"), [(risk, "part_of".into(), hub)]);
    assert_eq!(
        w.svc
            .hub_show(&ctx(), "risk_register", 20, 0)
            .unwrap()
            .value
            .nodes[0]
            .members
            .len(),
        2
    );
}

/// rules-17 and rules-20: an edit that makes a node match joins it; an edit of a node that already matched (its auto
/// edge deleted by hand) does not put the edge back; hub show lists it and hub fill restores it.
#[test]
fn edits_join_only_when_they_make_a_node_match_and_fill_restores_hand_deletes() {
    let mut w = demo();
    let hub = register(&mut w);
    let q = add(&mut w, "question", "Is the supplier late?", None)
        .node
        .id;
    let done = retype(&mut w, q, "risk");
    assert_eq!(done.value.hub_edges.len(), 1);
    assert!(codes(&done.notes).contains(&"hub_joined"));
    let edge = done.value.hub_edges[0].edge;
    w.svc.edge_delete(&ctx(), &edge.to_text()).unwrap();
    // Reads, a reload and check never add it back.
    w.svc.hub_list(&ctx()).unwrap();
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    w.svc.check(&ctx(), None, None).unwrap();
    assert!(auto(&w, "risk_register").is_empty());
    // An unrelated edit of a node that already matched leaves it out too.
    let edited = w
        .svc
        .node_edit(
            &ctx(),
            &q.to_text(),
            NodeEditRequest {
                title: Some("Is the supplier very late?".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(edited.value.hub_edges.is_empty());
    assert!(!codes(&edited.notes).contains(&"hub_joined"));
    let shown = w.svc.hub_show(&ctx(), "risk_register", 20, 0).unwrap();
    assert_eq!(shown.value.candidates, [q]);
    assert!(codes(&shown.notes).contains(&"hub_unjoined"));
    let filled = w.svc.hub_fill(&ctx(), "risk_register", None).unwrap().value;
    assert_eq!(filled.joined_total, 1);
    assert_eq!(auto(&w, "risk_register"), [(q, "part_of".into(), hub)]);
    // Filling again is safe: a member is skipped.
    assert_eq!(
        w.svc
            .hub_fill(&ctx(), "risk_register", None)
            .unwrap()
            .value
            .joined_total,
        0
    );
    // Retyped out of the pattern, the edge stays.
    retype(&mut w, q, "question");
    assert_eq!(auto(&w, "risk_register").len(), 1);
}

/// rules-18: the mark is umx's alone; edge lists filter on it.
#[test]
fn auto_added_by_is_refused_by_hand_and_filters_edge_lists() {
    let mut w = demo();
    let hub = register(&mut w);
    let a = add(&mut w, "question", "A", None).node.id;
    let set_mark = [(
        "auto_added_by".to_string(),
        SetValue::Raw("risk_register".into()),
    )];
    assert_eq!(
        w.svc
            .edge_add(&ctx(), &a.to_text(), "part_of", &hub.to_text(), &set_mark)
            .unwrap_err()
            .code,
        "reserved_name"
    );
    let e = w
        .svc
        .edge_add(&ctx(), &a.to_text(), "part_of", &hub.to_text(), &[])
        .unwrap()
        .value
        .edge
        .id;
    let refused = w.svc.edge_edit(
        &ctx(),
        &e.to_text(),
        &umx_services::EdgeEditRequest {
            sets: set_mark.to_vec(),
            ..Default::default()
        },
    );
    assert_eq!(refused.unwrap_err().code, "reserved_name");
    let r = add(&mut w, "risk", "R", None).node.id;
    assert_eq!(auto(&w, "risk_register"), [(r, "part_of".into(), hub)]);
    assert!(auto(&w, "other").is_empty());
    let bad = w.svc.edge_list(
        &ctx(),
        EdgeListRequest {
            wheres: vec!["auto_added_by!=x".into()],
            ..Default::default()
        },
    );
    assert_eq!(bad.unwrap_err().code, "where_invalid");
}

/// rules-19: removing (or moving) a hub keeps every edge umx added for it, and says how many remain.
#[test]
fn removing_or_moving_a_hub_keeps_its_auto_edges() {
    let mut w = demo();
    let hub = register(&mut w);
    add(&mut w, "risk", "R1", None);
    add(&mut w, "risk", "R2", None);
    let other = add(&mut w, "topic", "Another register", None).node.id;
    let mut moved = set("risk_register", Some(other), None, None);
    moved.pattern = Some(json!({"type": ["risk"]}));
    let done = w.svc.hub_set(&ctx(), moved).unwrap();
    assert!(codes(&done.notes).contains(&"hub_edges_remain"));
    assert_eq!(done.value.joined_total, 2);
    assert_eq!(auto(&w, "risk_register").len(), 4);
    let removed = w.svc.hub_remove(&ctx(), "risk_register").unwrap();
    assert_eq!(removed.value.remaining, 4);
    assert!(codes(&removed.notes).contains(&"hub_edges_remain"));
    assert_eq!(
        edges_json(&w)
            .matches("\"auto_added_by\":\"risk_register\"")
            .count(),
        4
    );
    assert!(
        codec::decode_graph(&graph_json(&w))
            .unwrap()
            .hubs
            .is_empty()
    );
    assert!(edges_json(&w).contains(&hub.to_text()));
    // Without the hub, a new risk joins nothing.
    assert!(add(&mut w, "risk", "R3", None).hub_edges.is_empty());
}

/// rules-21: a named hub whose node has gone adds nothing, says so, and the graph loads.
#[test]
fn a_hub_whose_node_is_gone_adds_nothing_and_the_graph_loads() {
    let mut w = demo();
    let gone = Id(424_242);
    let hub = register(&mut w);
    // Point the declaration at a node that is not in the graph, as a hand edit or a merge leaves it.
    let mut meta = codec::decode_graph(&graph_json(&w)).unwrap();
    meta.hubs[0].node = Some(gone);
    w.files.tamper_for_tests(
        &GraphDir::parse("/work/g").unwrap(),
        GraphFile::Graph,
        &codec::encode_graph(&meta),
    );
    let loaded = w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    assert!(codes(&loaded.notes).contains(&"hub_node_missing"));
    let listed = w.svc.hub_list(&ctx()).unwrap();
    assert!(listed.value.rows[0].missing);
    assert!(codes(&listed.notes).contains(&"hub_node_missing"));
    let r = add(&mut w, "risk", "After the register went", None);
    assert!(r.hub_edges.is_empty());
    assert!(!edges_json(&w).contains(&hub.to_text()));
    let filled = w.svc.hub_fill(&ctx(), "risk_register", None).unwrap();
    assert_eq!(filled.value.joined_total, 0);
    assert!(codes(&filled.notes).contains(&"hub_node_missing"));
}

fn node_line(line: u32, kind: &str, title: &str, parent: Option<Id>) -> BatchLine {
    BatchLine {
        line,
        op: BatchOp::NodeAdd {
            id: None,
            created: None,
            ref_name: None,
            req: NodeAddRequest {
                kind: kind.into(),
                title: title.into(),
                parent: parent.map(Id::to_text),
                ..Default::default()
            },
            parent_ref: None,
        },
    }
}

fn batch(
    w: &mut World,
    lines: Vec<BatchLine>,
    dry_run: bool,
) -> umx_services::Done<umx_services::BatchDone> {
    w.svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                lines,
                dry_run,
                refs: Default::default(),
                refs_graph: None,
                reason: None,
            },
        )
        .unwrap()
}

/// rules-17: batch node adds join, batch edits join only when they make the node match, one hint per hub; a dry run
/// says so and writes nothing.
#[test]
fn batch_adds_and_edits_join_hubs_in_the_same_write() {
    let mut w = demo();
    let hub = register(&mut w);
    let q = add(&mut w, "question", "Q", None).node.id;
    let lines = vec![
        node_line(1, "risk", "B1", None),
        node_line(2, "risk", "B2", Some(hub)),
        node_line(3, "question", "B3", None),
        BatchLine {
            line: 4,
            op: BatchOp::NodeEdit {
                node: Some(q.to_text()),
                node_ref: None,
                req: NodeEditRequest {
                    kind: Some("risk".into()),
                    ..Default::default()
                },
            },
        },
    ];
    let edges = edges_json(&w);
    let dry = batch(&mut w, lines.clone(), true);
    assert!(codes(&dry.notes).contains(&"hub_joined"));
    assert_eq!(edges_json(&w), edges);
    let done = batch(&mut w, lines, false);
    let rows = &done.value.rows;
    assert_eq!(rows[0].hub_edges.len(), 1);
    assert!(
        rows[1].hub_edges.is_empty(),
        "already a member through --parent"
    );
    assert!(rows[2].hub_edges.is_empty());
    assert_eq!(rows[3].hub_edges.len(), 1);
    assert_eq!(
        done.notes.iter().filter(|p| p.code == "hub_joined").count(),
        1
    );
    let b1 = rows[0].id.unwrap();
    let mut want = vec![(b1, "part_of".to_string(), hub), (q, "part_of".into(), hub)];
    want.sort();
    assert_eq!(auto(&w, "risk_register"), want);
}

/// rules-20: fill carries on after a node with `--after`, joining only later nodes.
#[test]
fn fill_after_a_node_joins_only_later_nodes() {
    let mut w = demo();
    let hub = add(&mut w, "topic", "Risk register", None).node.id;
    let risks: Vec<Id> = (0..3)
        .map(|i| add(&mut w, "risk", &format!("R{i}"), None).node.id)
        .collect();
    // Declared by hand (as a git merge brings it), so no set has backfilled it yet.
    let mut meta = codec::decode_graph(&graph_json(&w)).unwrap();
    meta.hubs = vec![umx_core::hubs::Hub {
        name: "risk_register".into(),
        node: Some(hub),
        kind: None,
        via: "part_of".into(),
        pattern: serde_json::from_value(json!({"type": ["risk"]})).unwrap(),
    }];
    meta.umx = 9;
    w.files.tamper_for_tests(
        &GraphDir::parse("/work/g").unwrap(),
        GraphFile::Graph,
        &codec::encode_graph(&meta),
    );
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    let filled = w
        .svc
        .hub_fill(&ctx(), "risk_register", Some(&risks[0].to_text()))
        .unwrap()
        .value;
    let mut joined: Vec<Id> = filled.joined.iter().map(|j| j.node).collect();
    joined.sort();
    assert_eq!(joined, risks[1..]);
    assert!(filled.after.is_none());
}

/// rules-33: an edge umx adds for a hub never moves a node on a spine, even a hub over the spine's anchor type, whose
/// auto part_of edges would otherwise give each anchor a parent.
#[test]
fn hub_edges_never_change_spine_places() {
    let mut w = demo();
    let mut segs = Vec::new();
    for s in 0..3u32 {
        let id = w
            .svc
            .node_add(
                &ctx(),
                NodeAddRequest {
                    kind: "segment".into(),
                    title: format!("Segment {s}"),
                    sets: vec![
                        (
                            "starts_on".into(),
                            SetValue::Raw(format!("2026-11-1{}T09:00:00Z", s + 1)),
                        ),
                        ("step".into(), SetValue::Raw(format!("{}", 3 - s))),
                    ],
                    ..Default::default()
                },
            )
            .unwrap()
            .value
            .node
            .id;
        segs.push(id);
    }
    let points: Vec<Id> = segs
        .iter()
        .map(|s| add(&mut w, "risk", "Point", Some(*s)).node.id)
        .collect();
    let decl = json!({
        "name": "day", "title": "The day", "main": true,
        "anchors": { "type": "segment", "inside": "part_of" },
        "order": ["step", "starts_on"], "place": ["raised_in"],
    });
    w.svc.graph_spine_set(&ctx(), "demo", &decl).unwrap();
    let all: Vec<Id> = segs.iter().chain(&points).copied().collect();
    let before = w.svc.spine_places("demo", "day", &all).unwrap();
    // Each segment joins one register, in a different order than the spine's: as parents they would regroup them.
    for (i, s) in segs.iter().enumerate() {
        let reg = add(&mut w, "topic", &format!("Register {i}"), None).node.id;
        let mut req = set(&format!("reg_{i}"), Some(reg), None, None);
        req.pattern = Some(json!({"type": ["segment"], "field": {"step": 3 - i as i64}}));
        let done = w.svc.hub_set(&ctx(), req).unwrap().value;
        assert_eq!(done.joined.iter().map(|j| j.node).collect::<Vec<_>>(), [*s]);
    }
    let mut req = set(
        "risk_register",
        Some(add(&mut w, "topic", "Risks", None).node.id),
        None,
        None,
    );
    req.pattern = Some(json!({"type": ["risk"]}));
    assert_eq!(w.svc.hub_set(&ctx(), req).unwrap().value.joined_total, 3);
    assert_eq!(w.svc.spine_places("demo", "day", &all).unwrap(), before);
}

/// rules-16 and rules-20: past the join limit a hub set is never refused; it joins the nodes it reached in one
/// write, warns `hub_scan_cut`, and its next (`hub fill --after`) carries on until every match is a member.
#[test]
fn a_large_backfill_is_cut_and_continued_with_after() {
    let mut w = demo();
    let lines: Vec<BatchLine> = (0..1001)
        .map(|i| node_line(i + 1, "risk", &format!("Risk number {i}"), None))
        .collect();
    batch(&mut w, lines, false);
    let hub = add(&mut w, "topic", "Risk register", None).node.id;
    let mut req = set("risk_register", Some(hub), None, None);
    req.pattern = Some(json!({"type": ["risk"]}));
    let done = w.svc.hub_set(&ctx(), req).unwrap();
    assert_eq!(done.value.joined_total, 1000);
    assert_eq!(done.value.joined.len(), 100);
    let after = done.value.after.expect("cut");
    let cut = done
        .notes
        .iter()
        .find(|p| p.code == "hub_scan_cut")
        .unwrap();
    assert!(
        format!("{cut:?}").contains(&after.to_text()),
        "the next names where to carry on"
    );
    let next = w
        .svc
        .hub_fill(&ctx(), "risk_register", Some(&after.to_text()))
        .unwrap();
    assert_eq!(next.value.joined_total, 1);
    assert!(next.value.after.is_none());
    assert!(!codes(&next.notes).contains(&"hub_scan_cut"));
    assert_eq!(
        edges_json(&w)
            .matches("\"auto_added_by\":\"risk_register\"")
            .count(),
        1001
    );
}
