//! `on_key: resolve` on node adds (ws003 s08, usage 202): an add whose entity key a node of its type in the written
//! graph holds resolves to that node and only appends or fills; without the opt-in nothing changes. Synthetic
//! fixtures over the fakes.

use std::sync::Arc;
use umx_core::codec::GraphFile;
use umx_core::evidence::EvidencePointer;
use umx_core::ports::{GraphDir, GraphFiles, ModelKey};
use umx_core::setup::GraphFilesSetup;
use umx_core::value::SetValue;
use umx_core::{Id, Node, Problem, Slot, Value};
use umx_fakes::{FakeClock, FakeEmbedder, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{
    BatchDone, BatchLine, BatchOp, BatchRequest, Ctx, NodeAddRequest, NodeEditRequest, Ports,
    Services,
};

const JO: &str = "email:jo@example.com";

struct World {
    fake: Arc<FakeGraphFiles>,
    svc: Services,
}

fn dir() -> GraphDir {
    GraphDir::parse("/work/g").unwrap()
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

fn world() -> World {
    world_on(
        Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap())),
        true,
    )
}

fn world_on(fake: Arc<FakeGraphFiles>, new: bool) -> World {
    let ports = Ports {
        sources: Arc::new(umx_fakes::FakeSourceReader::open(
            GraphDir::parse("/").unwrap(),
        )),
        files: fake.clone(),
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
    let embedder = Arc::new(FakeEmbedder::new(ModelKey::new("fake", 8).unwrap()));
    let mut svc = Services::new(ports).with_embedder(embedder);
    if new {
        svc.graph_new(&ctx(), "/work/g", "demo", None, false, None)
            .unwrap();
    } else {
        svc.graph_load(&ctx(), "/work/g", None).unwrap();
    }
    World { fake, svc }
}

fn disk(w: &World) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    (
        w.fake.read(&dir(), GraphFile::Nodes).unwrap(),
        w.fake.read(&dir(), GraphFile::Edges).unwrap(),
    )
}

/// A node add of `kind` titled `title` holding `keys` (comma separated), with on_key resolve when `on_key`.
fn add(kind: &str, title: &str, keys: &str, on_key: bool) -> NodeAddRequest {
    NodeAddRequest {
        kind: kind.into(),
        title: title.into(),
        sets: if keys.is_empty() {
            Vec::new()
        } else {
            vec![("entity_key".into(), SetValue::Raw(keys.into()))]
        },
        on_key_resolve: on_key,
        ..Default::default()
    }
}

fn with_sets(mut req: NodeAddRequest, sets: &[(&str, &str)]) -> NodeAddRequest {
    req.sets.extend(
        sets.iter()
            .map(|(f, v)| (f.to_string(), SetValue::Raw(v.to_string()))),
    );
    req
}

fn with_evidence(mut req: NodeAddRequest, pointers: &[&str]) -> NodeAddRequest {
    req.evidence = pointers
        .iter()
        .map(|p| EvidencePointer {
            pointer: p.to_string(),
            kind: None,
        })
        .collect();
    req
}

fn line(line: u32, r: Option<&str>, req: NodeAddRequest) -> BatchLine {
    BatchLine {
        line,
        op: BatchOp::NodeAdd {
            id: None,
            created: None,
            ref_name: r.map(Into::into),
            req,
            parent_ref: None,
        },
    }
}

fn request(lines: Vec<BatchLine>, dry_run: bool) -> BatchRequest {
    BatchRequest {
        lines,
        dry_run,
        ..Default::default()
    }
}

fn batch(w: &mut World, lines: Vec<BatchLine>, dry_run: bool) -> Result<BatchDone, Vec<Problem>> {
    w.svc
        .batch(&ctx(), request(lines, dry_run))
        .map(|d| d.value)
}

fn batch_notes(w: &mut World, req: BatchRequest) -> (BatchDone, Vec<Problem>) {
    let done = w.svc.batch(&ctx(), req).unwrap();
    (done.value, done.notes)
}

fn codes(ps: &[Problem]) -> Vec<&str> {
    ps.iter().map(|p| p.code).collect()
}

fn node(w: &World, id: Id) -> Node {
    w.svc
        .loaded_graph("demo")
        .unwrap()
        .node(id)
        .unwrap()
        .to_node()
}

/// The content nodes of `kind` (provenance nodes left out).
fn count(w: &World, kind: &str) -> usize {
    let g = w.svc.loaded_graph("demo").unwrap();
    g.nodes()
        .filter(|n| !g.is_provenance(n.ix()) && n.kind() == kind)
        .count()
}

/// A node's evidence without the writing session (provenance, masked by the differential).
fn pointers(n: &Node) -> Vec<Value> {
    n.evidence
        .iter()
        .map(|e| {
            let mut e = e.clone();
            if let Value::Object(o) = &mut e {
                o.remove("by_session");
            }
            e
        })
        .collect()
}

/// A node's kind, title, status, note, evidence and fields.
type Content = (
    String,
    String,
    String,
    String,
    Vec<Value>,
    Vec<(String, Value)>,
);

/// A node's content with ids, times and provenance masked: what the differential compares.
fn content(n: &Node) -> Content {
    let fields = n
        .fields
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    (
        n.kind.clone(),
        n.title.clone(),
        n.status.clone(),
        n.note.clone(),
        pointers(n),
        fields,
    )
}

/// The differential: create, then resolve, then enrich through on_key leaves the node as the explicit verbs (node
/// add, then evidence and fields by node edit and node note) do, and the second add names the first's node.
#[test]
fn create_then_resolve_then_enrich_matches_the_explicit_verbs() {
    let mut w = world();
    let first = batch(
        &mut w,
        vec![line(
            1,
            Some("jo"),
            with_evidence(
                with_sets(add("person", "Jo Smith", JO, true), &[("role", "analyst")]),
                &["notes/a.md#L1"],
            ),
        )],
        false,
    )
    .unwrap();
    assert!(!first.rows[0].existed);
    let id = first.rows[0].id.unwrap();
    let second = batch(
        &mut w,
        vec![line(
            1,
            Some("jo_again"),
            with_evidence(
                with_sets(
                    add("person", "Jo Smith", &format!("{JO},upn:jo@corp"), true),
                    &[("role", "analyst"), ("team", "blue")],
                ),
                &["notes/a.md#L1", "notes/b.md#L2"],
            ),
        )],
        false,
    )
    .unwrap();
    let row = &second.rows[0];
    assert!(row.existed, "the second add resolves");
    assert_eq!(row.id, Some(id));
    assert_eq!(row.changed, ["entity_key", "team", "evidence"]);
    assert_eq!(second.refs, [("jo_again".to_string(), Some(id))]);
    assert_eq!(count(&w, "person"), 1);

    let mut x = world();
    let plain = x
        .svc
        .node_add(
            &ctx(),
            with_evidence(
                with_sets(add("person", "Jo Smith", JO, false), &[("role", "analyst")]),
                &["notes/a.md#L1"],
            ),
        )
        .unwrap()
        .value
        .node;
    x.svc
        .node_edit(
            &ctx(),
            &plain.id.to_text(),
            NodeEditRequest {
                sets: vec![
                    (
                        "entity_key".into(),
                        SetValue::Raw(format!("{JO},upn:jo@corp")),
                    ),
                    ("team".into(), SetValue::Raw("blue".into())),
                ],
                ..Default::default()
            },
        )
        .unwrap();
    x.svc
        .node_evidence_add(&ctx(), &plain.id.to_text(), "notes/b.md#L2", None)
        .unwrap();
    assert_eq!(content(&node(&w, id)), content(&node(&x, plain.id)));
    let resolved = node(&w, id);
    let sources: Vec<&str> = resolved
        .evidence
        .iter()
        .map(|e| e["source"].as_str().unwrap())
        .collect();
    assert_eq!(sources, ["notes/a.md", "notes/b.md"]);
}

/// The batch's second line with the key resolves to the node its first line adds, in the dry run and the real one;
/// without the opt-in the second line adds a second node, as before.
#[test]
fn a_second_line_with_the_key_resolves_to_the_first_lines_node() {
    let lines = || {
        vec![
            line(1, Some("a"), add("person", "Jo", JO, true)),
            line(
                2,
                Some("b"),
                with_evidence(add("person", "Jo", JO, true), &["notes/a.md#L3"]),
            ),
        ]
    };
    let mut w = world();
    let before = disk(&w);
    let dry = batch(&mut w, lines(), true).unwrap();
    assert_eq!(
        dry.rows.iter().map(|r| r.existed).collect::<Vec<_>>(),
        [false, true]
    );
    assert_eq!(dry.rows[1].changed, ["evidence"]);
    assert!(
        dry.rows.iter().all(|r| r.id.is_none()),
        "a drawn id stays null"
    );
    assert_eq!(disk(&w), before);
    let done = batch(&mut w, lines(), false).unwrap();
    assert_eq!(done.rows[1].id, done.rows[0].id);
    assert_eq!(done.refs[0].1, done.refs[1].1);
    assert_eq!(count(&w, "person"), 1);

    let mut plain = world();
    let (done, notes) = batch_notes(
        &mut plain,
        request(
            vec![
                line(1, None, add("person", "Jo", JO, false)),
                line(2, None, add("person", "Jo", JO, false)),
            ],
            false,
        ),
    );
    assert_eq!(count(&plain, "person"), 2);
    assert_ne!(done.rows[0].id, done.rows[1].id);
    assert!(codes(&notes).contains(&"duplicate_key"));
}

/// A key two nodes hold, or keys held one by each of two nodes, refuses the add, names both by address, never shows
/// a key and writes nothing (graph-addressing).
#[test]
fn keys_naming_two_nodes_refuse_and_name_both_by_address() {
    let mut w = world();
    let a = w
        .svc
        .node_add(&ctx(), add("person", "Jo", JO, false))
        .unwrap()
        .value
        .node
        .id;
    let b = w
        .svc
        .node_add(&ctx(), add("person", "Jo S", JO, false))
        .unwrap()
        .value
        .node
        .id;
    let c = w
        .svc
        .node_add(&ctx(), add("person", "Sam", "upn:sam@corp", false))
        .unwrap()
        .value
        .node
        .id;
    let d = w
        .svc
        .node_add(&ctx(), add("person", "Dee", "email:dee@example.com", false))
        .unwrap()
        .value
        .node
        .id;
    let before = disk(&w);
    let errors = batch(
        &mut w,
        vec![line(1, None, add("person", "Jo", JO, true))],
        false,
    )
    .unwrap_err();
    assert_eq!(codes(&errors), ["batch_failed", "on_key_ambiguous"]);
    let slot = |p: &Problem, name: &str| {
        p.slots
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, s)| s.clone())
            .unwrap()
    };
    assert_eq!(
        (slot(&errors[1], "node"), slot(&errors[1], "other")),
        (Slot::Id(a), Slot::Id(b))
    );
    // The fix names the other node by its address and keeps it in a failed batch (no key value).
    let Slot::Command(fix) = slot(&errors[1], "fix") else {
        panic!("a command")
    };
    assert_eq!(
        fix.as_str(),
        format!("umx entity key umx:demo/{b} remove <key>")
    );
    assert!(!format!("{:?}", errors[1]).contains("jo@example.com"));
    let single = w
        .svc
        .node_add(
            &ctx(),
            add("person", "Sam", "upn:sam@corp,email:dee@example.com", true),
        )
        .unwrap_err();
    assert_eq!(single.code, "on_key_ambiguous");
    assert_eq!(
        (slot(&single, "node"), slot(&single, "other")),
        (Slot::Id(c), Slot::Id(d))
    );
    assert!(!format!("{single:?}").contains("sam@corp"));
    assert_eq!(disk(&w), before);
}

/// Never across types: the same key on another type adds a node and warns entity_type_mismatch.
#[test]
fn a_key_held_under_another_type_adds_a_node_and_warns() {
    let mut w = world();
    let org = w
        .svc
        .node_add(
            &ctx(),
            add("organisation", "Example", "domain:example.com", false),
        )
        .unwrap()
        .value
        .node
        .id;
    let (done, notes) = batch_notes(
        &mut w,
        request(
            vec![line(
                1,
                None,
                add("team", "Example", "domain:example.com", true),
            )],
            false,
        ),
    );
    assert!(!done.rows[0].existed);
    assert_ne!(done.rows[0].id, Some(org));
    assert_eq!(count(&w, "team"), 1);
    assert!(codes(&notes).contains(&"entity_type_mismatch"), "{notes:?}");
    let single = w
        .svc
        .node_add(
            &ctx(),
            add("project", "Example", "domain:example.com", true),
        )
        .unwrap();
    assert!(!single.value.resolved);
    assert!(codes(&single.notes).contains(&"entity_type_mismatch"));
}

/// Held values are kept, named by the hint and never echoed; an absent field and an empty note are filled.
#[test]
fn held_values_are_kept_and_named_while_absent_ones_fill() {
    let mut w = world();
    let mut req = with_sets(add("person", "Jo Smith", JO, false), &[("role", "analyst")]);
    req.status = Some("open".into());
    let id = w.svc.node_add(&ctx(), req).unwrap().value.node.id;
    let before = node(&w, id);
    let mut again = with_sets(
        add("person", "Joanna Smith", JO, true),
        &[("role", "lead"), ("team", "blue")],
    );
    again.status = Some("closed".into());
    again.note = Some("Joined in March".into());
    let (done, notes) = batch_notes(&mut w, request(vec![line(1, None, again)], false));
    assert!(done.rows[0].existed);
    assert_eq!(done.rows[0].changed, ["note", "team"]);
    let after = node(&w, id);
    assert_eq!(
        (&after.title, &after.status, &after.fields["role"]),
        (&before.title, &before.status, &before.fields["role"])
    );
    assert_eq!(after.note, "Joined in March");
    let held = notes
        .iter()
        .find(|p| p.code == "on_key_field_held")
        .expect("the held hint");
    let text = format!("{held:?}");
    for word in ["title", "status", "role"] {
        assert!(text.contains(word), "{text}");
    }
    for value in ["Joanna", "closed", "lead"] {
        assert!(!text.contains(value), "{value} echoed: {text}");
    }
    // A held note is named too; the stored one stays.
    let mut third = add("person", "Jo Smith", JO, true);
    third.note = Some("Left in May".into());
    let single = w.svc.node_add(&ctx(), third).unwrap();
    assert!(single.value.resolved);
    assert!(single.value.changed.is_empty());
    assert_eq!(single.value.node.note, "Joined in March");
    assert!(codes(&single.notes).contains(&"on_key_field_held"));
}

/// A resolve with nothing new writes nothing, not even attribution.
#[test]
fn a_resolve_with_nothing_new_writes_nothing() {
    let mut w = world();
    let req = || with_evidence(add("person", "Jo", JO, true), &["notes/a.md#L1"]);
    let id = w.svc.node_add(&ctx(), req()).unwrap().value.node.id;
    let before = disk(&w);
    let single = w.svc.node_add(&ctx(), req()).unwrap();
    assert!(single.value.resolved);
    assert_eq!(single.value.node.id, id);
    assert!(single.value.changed.is_empty());
    let done = batch(&mut w, vec![line(1, None, req())], false).unwrap();
    assert!(done.rows[0].existed && done.rows[0].changed.is_empty());
    assert_eq!(disk(&w), before);
}

/// part_of is the one parent: filled once, the same parent (by any spelling) adds nothing, another is held.
/// raised_in is many: each new anchor adds an edge, a repeated one does not. Compared by stored ends.
#[test]
fn parent_fills_once_and_raised_in_appends_by_stored_ends() {
    let mut w = world();
    let topic = |w: &mut World, t: &str| {
        w.svc
            .node_add(&ctx(), add("topic", t, "", false))
            .unwrap()
            .value
            .node
            .id
    };
    let (p1, p2) = (topic(&mut w, "Payments"), topic(&mut w, "Billing"));
    let (m1, m2) = (topic(&mut w, "Kickoff"), topic(&mut w, "Review"));
    let id = w
        .svc
        .node_add(&ctx(), add("person", "Jo", JO, true))
        .unwrap()
        .value
        .node
        .id;
    let mut with_parent = |parent: String| {
        let mut req = add("person", "Jo", JO, true);
        req.parent = Some(parent);
        w.svc.node_add(&ctx(), req).unwrap()
    };
    let filled = with_parent(p1.to_text());
    assert!(filled.value.resolved && filled.value.parent_edge.is_some());
    let same = with_parent(format!("umx:demo/{p1}"));
    assert!(same.value.parent_edge.is_none());
    assert!(!codes(&same.notes).contains(&"on_key_field_held"));
    let other = with_parent(p2.to_text());
    assert!(other.value.parent_edge.is_none());
    assert!(codes(&other.notes).contains(&"on_key_field_held"));

    let raised = |w: &mut World, anchor: Id| {
        let mut req = request(vec![line(1, None, add("person", "Jo", JO, true))], false);
        req.raised_in = Some(format!("umx:demo/{anchor}"));
        batch_notes(w, req).0.rows.remove(0)
    };
    assert!(raised(&mut w, m1).raised_in_edge.is_some());
    assert!(raised(&mut w, m1).raised_in_edge.is_none());
    assert!(raised(&mut w, m2).raised_in_edge.is_some());
    let g = w.svc.loaded_graph("demo").unwrap();
    let ix = g.node_ix(id).unwrap();
    let out: Vec<&str> = g.edges_out(ix).map(|s| s.kind.as_str()).collect();
    assert_eq!(
        (
            out.iter().filter(|k| **k == "part_of").count(),
            out.iter().filter(|k| **k == "raised_in").count()
        ),
        (1, 2)
    );
}

/// graph-storage: the key index is kept by the row functions, so a failed batch's rollback takes its key out and a
/// later add with the key creates again.
#[test]
fn a_rolled_back_add_leaves_no_key_behind() {
    let mut w = world();
    let before = disk(&w);
    let errors = batch(
        &mut w,
        vec![
            line(1, Some("a"), add("person", "Jo", JO, true)),
            line(2, Some("b"), add("person", "Jo", JO, true)),
            BatchLine {
                line: 3,
                op: BatchOp::NodeEdit {
                    node: None,
                    node_ref: Some("missing".into()),
                    req: NodeEditRequest::default(),
                },
            },
        ],
        false,
    )
    .unwrap_err();
    assert_eq!(codes(&errors), ["batch_failed", "batch_ref_unknown"]);
    assert_eq!(disk(&w), before);
    assert!(
        w.svc
            .loaded_graph("demo")
            .unwrap()
            .entity_node("person", JO)
            .is_none()
    );
    let done = batch(
        &mut w,
        vec![line(1, None, add("person", "Jo", JO, true))],
        false,
    )
    .unwrap();
    assert!(!done.rows[0].existed);
    let again = batch(
        &mut w,
        vec![line(1, None, add("person", "Jo", JO, true))],
        false,
    )
    .unwrap();
    assert!(again.rows[0].existed);
    assert_eq!(again.rows[0].id, done.rows[0].id);
}

/// A kept line (an earlier line of a repeated id) and a provenance node never resolve; a key stored in another
/// spelling of its stored form does, and the write stores it canonical.
#[test]
fn kept_lines_and_provenance_nodes_never_resolve() {
    let mut w = world();
    let kept = w
        .svc
        .node_add(&ctx(), add("person", "Jo", JO, false))
        .unwrap()
        .value
        .node;
    let sam = w
        .svc
        .node_add(&ctx(), add("person", "Sam", "upn:sam@corp", false))
        .unwrap()
        .value
        .node;
    let (prov, prov_kind) = {
        let g = w.svc.loaded_graph("demo").unwrap();
        let n = g.nodes().find(|n| g.is_provenance(n.ix())).unwrap();
        (n.to_node(), n.kind().to_string())
    };
    let mut keyless = kept.clone();
    keyless.fields.remove("entity_key");
    let mut cased = sam.clone();
    cased
        .fields
        .insert("entity_key".into(), serde_json::json!(["upn:Sam@Corp"]));
    let mut prov_keyed = prov.clone();
    prov_keyed
        .fields
        .insert("entity_key".into(), serde_json::json!(["name:marker"]));
    let nodes = w.fake.read(&dir(), GraphFile::Nodes).unwrap().unwrap();
    let mut text = String::from_utf8(nodes).unwrap();
    let replace = |text: &mut String, n: &Node, with: &Node| {
        let old = String::from_utf8(umx_core::codec::encode_nodes([n])).unwrap();
        let new = String::from_utf8(umx_core::codec::encode_nodes([with])).unwrap();
        assert!(text.contains(&old));
        *text = text.replace(&old, &new);
    };
    replace(&mut text, &sam, &cased);
    replace(&mut text, &prov, &prov_keyed);
    text.push_str(&String::from_utf8(umx_core::codec::encode_nodes([&keyless])).unwrap());
    w.fake
        .write(&dir(), GraphFile::Nodes, text.as_bytes())
        .unwrap();
    let mut w = world_on(w.fake, false);
    let done = batch(
        &mut w,
        vec![
            line(1, None, add("person", "Jo", JO, true)),
            line(2, None, add(&prov_kind, "Marker", "name:marker", true)),
            line(3, None, add("person", "Sam", "upn:sam@corp", true)),
        ],
        false,
    )
    .unwrap();
    assert_eq!(
        done.rows.iter().map(|r| r.existed).collect::<Vec<_>>(),
        [false, false, true]
    );
    assert_ne!(done.rows[0].id, Some(kept.id));
    assert_ne!(done.rows[1].id, Some(prov.id));
    assert_eq!(done.rows[2].id, Some(sam.id));
}

/// Review B1, security N2: every holder of a key goes through the write scope before the key is judged. A
/// provenance node holding the key beside one content node of its type leaves that node to resolve to; beside two,
/// the refusal names the two content nodes, never the provenance one. Both entry paths.
#[test]
fn a_provenance_holder_is_skipped_before_a_key_is_judged() {
    let mut w = world();
    w.svc
        .node_add(&ctx(), add("topic", "Warm up", "", false))
        .unwrap();
    let (prov, kind) = {
        let g = w.svc.loaded_graph("demo").unwrap();
        let n = g.nodes().find(|n| g.is_provenance(n.ix())).unwrap();
        (n.to_node(), n.kind().to_string())
    };
    let c1 = w
        .svc
        .node_add(&ctx(), add(&kind, "Marker one", "name:marker", false))
        .unwrap()
        .value
        .node
        .id;
    assert!(prov.id < c1, "the provenance node is the lowest holder");
    let mut keyed = prov.clone();
    keyed
        .fields
        .insert("entity_key".into(), serde_json::json!(["name:marker"]));
    let nodes = w.fake.read(&dir(), GraphFile::Nodes).unwrap().unwrap();
    let old = String::from_utf8(umx_core::codec::encode_nodes([&prov])).unwrap();
    let new = String::from_utf8(umx_core::codec::encode_nodes([&keyed])).unwrap();
    let text = String::from_utf8(nodes).unwrap();
    assert!(text.contains(&old));
    w.fake
        .write(
            &dir(),
            GraphFile::Nodes,
            text.replace(&old, &new).as_bytes(),
        )
        .unwrap();
    let mut w = world_on(w.fake, false);
    let resolve = || add(&kind, "Marker one", "name:marker", true);
    let done = batch(&mut w, vec![line(1, None, resolve())], false).unwrap();
    assert!(done.rows[0].existed);
    assert_eq!(done.rows[0].id, Some(c1));
    let single = w.svc.node_add(&ctx(), resolve()).unwrap();
    assert!(single.value.resolved);
    assert_eq!(single.value.node.id, c1);

    let c2 = w
        .svc
        .node_add(&ctx(), add(&kind, "Marker two", "name:marker", false))
        .unwrap()
        .value
        .node
        .id;
    let named = |p: &Problem| {
        p.slots
            .iter()
            .filter_map(|(_, s)| match s {
                Slot::Id(id) => Some(*id),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let errors = batch(&mut w, vec![line(1, None, resolve())], false).unwrap_err();
    assert_eq!(codes(&errors), ["batch_failed", "on_key_ambiguous"]);
    assert_eq!(named(&errors[1]), [c1, c2]);
    let single = w.svc.node_add(&ctx(), resolve()).unwrap_err();
    assert_eq!(single.code, "on_key_ambiguous");
    assert_eq!(named(&single), [c1, c2]);
    for p in [&errors[1], &single] {
        assert!(!format!("{p:?}").contains(&prov.id.to_text()), "{p:?}");
    }
}

/// An explicit id with on_key, and on_key with no key, refuse at their line; the single verb refuses on_key with no
/// key and with a locator.
#[test]
fn on_key_refuses_an_explicit_id_a_missing_key_and_a_locator() {
    let mut w = world();
    let before = disk(&w);
    let mut explicit = line(1, None, add("person", "Jo", JO, true));
    if let BatchOp::NodeAdd { id, .. } = &mut explicit.op {
        *id = Some(Id(u128::from(umx_fakes::FAKE_EPOCH_MS) << 80 | 7).to_text());
    }
    let errors = batch(&mut w, vec![explicit], false).unwrap_err();
    assert_eq!(codes(&errors), ["batch_failed", "batch_line_invalid"]);
    assert_eq!(errors[1].line, Some(1));
    let errors = batch(
        &mut w,
        vec![line(1, None, add("person", "Sam", "", true))],
        false,
    )
    .unwrap_err();
    assert_eq!(codes(&errors), ["batch_failed", "batch_line_invalid"]);
    let no_key = w
        .svc
        .node_add(&ctx(), add("person", "Sam", "", true))
        .unwrap_err();
    assert_eq!(no_key.code, "usage_invalid");
    let mut located = add("resource", "Brief", "name:brief", true);
    located.locator = Some("docs/brief.md".into());
    assert_eq!(
        w.svc.node_add(&ctx(), located).unwrap_err().code,
        "usage_invalid"
    );
    assert_eq!(disk(&w), before);
}

/// graph-storage: in a preplanned batch (an id below the graph's highest), an on_key line holds no placeholder and,
/// when it creates, draws its node's and edges' ids at its line; a later line with the key resolves to it.
#[test]
fn a_preplanned_batch_creates_on_key_lines_at_their_line() {
    let mut w = world();
    let kickoff = w
        .svc
        .node_add(&ctx(), add("meeting", "Kickoff", "", false))
        .unwrap()
        .value
        .node
        .id;
    let future = Id(u128::from(umx_fakes::FAKE_EPOCH_MS + 59_000) << 80);
    let mut lines = vec![BatchLine {
        line: 1,
        op: BatchOp::NodeAdd {
            id: Some(future.to_text()),
            created: None,
            ref_name: Some("parent".into()),
            req: add("topic", "Imported", "id_no:topic", false),
            parent_ref: None,
        },
    }];
    for i in 0..20u32 {
        let mut req = add(
            "person",
            &format!("Person {i}"),
            &format!("id_no:{i}"),
            true,
        );
        req.evidence = Vec::new();
        lines.push(BatchLine {
            line: i + 2,
            op: BatchOp::NodeAdd {
                id: None,
                created: None,
                ref_name: None,
                req,
                parent_ref: Some("parent".into()),
            },
        });
    }
    lines.push(line(30, None, add("person", "Person 3", "id_no:3", true)));
    // The explicit-id line's node (a filled placeholder) resolves too, and takes a raised_in edge it lacks.
    let mut topic = add("topic", "Imported", "id_no:topic", true);
    topic.raised_in = Some(kickoff.to_text());
    lines.push(line(31, None, topic));
    let dry = batch(&mut w, lines.clone(), true).unwrap();
    assert!(dry.rows[21].existed && dry.rows[22].existed);
    let done = batch(&mut w, lines, false).unwrap();
    assert_eq!(done.rows[0].id, Some(future));
    assert!(
        done.rows[1..21]
            .iter()
            .all(|r| !r.existed && r.parent_edge.is_some())
    );
    assert!(done.rows[21].existed);
    assert_eq!(done.rows[21].id, done.rows[4].id);
    let last = done.rows.last().unwrap();
    assert!(last.existed && last.raised_in_edge.is_some());
    assert_eq!(last.id, Some(future));
    assert_eq!((count(&w, "person"), count(&w, "topic")), (20, 1));
    w.svc.loaded_graph("demo").unwrap().check_rows();
}

/// graph-storage's measurement: 100 creating on_key lines in a preplanned batch on a 100k-node graph (each is a
/// middle insert). Run alone in release: `cargo test -p umx-services --release --test on_key -- --ignored --exact
/// a_preplanned_batch_of_100_on_key_creates_at_100k --nocapture`.
#[test]
#[ignore = "timing bench: run alone in release with --ignored --exact --nocapture (ws003 s08 evidence)"]
fn a_preplanned_batch_of_100_on_key_creates_at_100k() {
    let mut w = world();
    for chunk in 0..10u32 {
        let lines = (0..10_000u32)
            .map(|i| {
                line(
                    i + 1,
                    None,
                    add("thing", &format!("Thing {chunk} {i}"), "", false),
                )
            })
            .collect();
        batch(&mut w, lines, false).unwrap();
    }
    let future = Id(u128::from(umx_fakes::FAKE_EPOCH_MS + 59_000) << 80);
    let mut lines = vec![BatchLine {
        line: 1,
        op: BatchOp::NodeAdd {
            id: Some(future.to_text()),
            created: None,
            ref_name: None,
            req: add("topic", "Imported", "", false),
            parent_ref: None,
        },
    }];
    lines.extend((0..100u32).map(|i| {
        line(
            i + 2,
            None,
            add("person", &format!("P {i}"), &format!("id_no:{i}"), true),
        )
    }));
    let timed = |w: &mut World, lines: Vec<BatchLine>| {
        let started = std::time::Instant::now();
        let done = batch(w, lines, false).unwrap();
        assert_eq!(done.rows.iter().filter(|r| !r.existed).count(), 101);
        started.elapsed()
    };
    let on_key = timed(&mut w, lines.clone());
    // The baseline: the same lines without on_key, preplanned into one bulk merge (a later future id).
    for l in &mut lines {
        if let BatchOp::NodeAdd { id, req, .. } = &mut l.op {
            req.on_key_resolve = false;
            req.sets.clear();
            if id.is_some() {
                *id = Some(Id(future.0 + 1).to_text());
            }
        }
    }
    let plain = timed(&mut w, lines);
    eprintln!("100 creating lines, preplanned, at 100k nodes: on_key {on_key:?}, plain {plain:?}");
}
