//! Services over the fakes: evidence maintenance (req-126, ws006 S1b): `node evidence list|add|remove`, the
//! `evidence_removed` trail and the batch `evidence add` and `evidence remove` lines.

use serde_json::{Value, json};
use std::sync::Arc;
use umx_core::codec::{self, GraphFile};
use umx_core::ports::{Change, GraphDir, GraphFiles};
use umx_core::provenance::{Actor, ActorInput, Tier};
use umx_core::setup::GraphFilesSetup;
use umx_core::{Id, Node, Problem};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{BatchLine, BatchOp, BatchRequest, Ctx, NodeAddRequest, Ports, Services};

struct World {
    files: Arc<FakeGraphFiles>,
    clock: Arc<FakeClock>,
    svc: Services,
}

fn world() -> World {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let clock = Arc::new(FakeClock::default());
    let ports = Ports {
        sources: Arc::new(umx_fakes::FakeSourceReader::open(
            GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: Arc::new(FakeGraphFiles::new(GraphDir::parse("/cache").unwrap())),
        vector_cache_root: GraphDir::parse("/cache").unwrap(),
        clock: clock.clone(),
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
    World { files, clock, svc }
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

/// A node holding `evidence`, stored by node add.
fn node(w: &mut World, evidence: &[&str]) -> String {
    w.svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "problem".into(),
                title: "Approval delays".into(),
                evidence: evidence.iter().map(|e| (*e).into()).collect(),
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id
        .to_text()
}

/// nodes.jsonl now, with the clock moved on a minute, so any later write changes a node's `updated` and the bytes:
/// "writes nothing" is then checked, not assumed.
fn snapshot(w: &World) -> Vec<u8> {
    w.clock.advance(60_000);
    nodes_file(w)
}

fn nodes_file(w: &World) -> Vec<u8> {
    w.files
        .read(&GraphDir::parse("/work/g").unwrap(), GraphFile::Nodes)
        .unwrap()
        .unwrap()
}

fn on_disk(w: &World, id: &str) -> Node {
    let id: Id = id.parse().unwrap();
    codec::decode_nodes(&nodes_file(w))
        .unwrap()
        .records
        .into_iter()
        .find(|n| n.id == id)
        .unwrap()
}

fn remove(
    w: &mut World,
    id: &str,
    pointer: &str,
    reason: Option<&str>,
    at: Option<usize>,
) -> Result<(Option<Value>, Vec<&'static str>), Problem> {
    w.svc
        .node_evidence_remove(&ctx(), id, pointer, reason, at)
        .map(|d| {
            (
                d.value.removed.map(|r| r.pointer),
                d.notes.iter().map(|p| p.code).collect(),
            )
        })
}

fn listed(w: &World, id: &str, removed: bool) -> Vec<(usize, String)> {
    w.svc
        .node_evidence_list(&ctx(), id, removed)
        .unwrap()
        .value
        .rows
        .into_iter()
        .map(|r| (r.at, r.text))
        .collect()
}

#[test]
fn list_gives_each_pointer_its_position_and_canonical_text() {
    let mut w = world();
    let id = node(
        &mut w,
        &[
            "calls/3.ndjson#seq=1-5",
            "docs/a.md#quote=we ship",
            "umx:other/01JC8XA4H7Q1V9K2M3N4P5R6S7#L3-L4?repo=https://example.invalid/x.git",
        ],
    );
    assert_eq!(
        listed(&w, &id, false),
        vec![
            (1, "calls/3.ndjson#seq=1-5".to_string()),
            (2, "docs/a.md#quote=we%20ship".to_string()),
            (
                3,
                "umx:other/01JC8XA4H7Q1V9K2M3N4P5R6S7#L3-L4?repo=https://example.invalid/x.git"
                    .to_string()
            ),
        ]
    );
    assert!(listed(&w, &id, true).is_empty());
}

#[test]
fn remove_needs_a_reason_and_moves_the_pointer_to_the_trail() {
    let mut w = world();
    let id = node(
        &mut w,
        &["calls/3.ndjson#seq=1-5", "docs/a.md#quote=we ship"],
    );
    let before = snapshot(&w);
    for reason in [None, Some("   "), Some(&*"x".repeat(501))] {
        let p = remove(&mut w, &id, "docs/a.md#quote=we ship", reason, None).unwrap_err();
        assert_eq!(p.code, "usage_invalid", "{reason:?}");
    }
    assert_eq!(nodes_file(&w), before, "a refused remove writes nothing");

    // The canonical spelling matches the raw text stored by node add; the kind is not compared.
    let (entry, warnings) = remove(
        &mut w,
        &id,
        "docs/a.md#quote=we%20ship",
        Some("stale"),
        None,
    )
    .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let entry = entry.unwrap();
    let n = on_disk(&w, &id);
    assert_eq!(n.evidence.len(), 1);
    assert_eq!(n.evidence[0]["source"], "calls/3.ndjson");
    let trail = n.fields["evidence_removed"].as_array().unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0], entry);
    // Nested, never flattened: the pointer verbatim under `removed`, then when, why and by whom.
    let pointer = &entry["removed"];
    assert_eq!(pointer["source"], "docs/a.md");
    assert_eq!(pointer["selector"], json!({ "quote": "we ship" }));
    assert_eq!(pointer["kind"], "document");
    assert_eq!(entry["reason"], "stale");
    assert_eq!(entry["by"], "tester");
    assert!(entry["by_session"].is_string(), "{entry}");
    assert!(entry["removed_at"].as_str().unwrap().contains('T'));
    assert_eq!(n.updated, entry["removed_at"].as_str().unwrap());
    let keys: Vec<&str> = entry
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.len(), 5, "{keys:?}");
    assert_eq!(
        listed(&w, &id, false),
        vec![(1, "calls/3.ndjson#seq=1-5".to_string())]
    );
    assert_eq!(
        listed(&w, &id, true),
        vec![(1, "docs/a.md#quote=we%20ship".to_string())]
    );
}

#[test]
fn removing_again_is_already_removed_and_writes_nothing() {
    let mut w = world();
    let id = node(&mut w, &["docs/a.md#L3"]);
    remove(&mut w, &id, "docs/a.md#L3", Some("stale"), None).unwrap();
    let before = snapshot(&w);
    let (entry, warnings) = remove(&mut w, &id, "docs/a.md#L3-L3", Some("stale"), None).unwrap();
    assert_eq!((entry, warnings), (None, vec!["already_removed"]));
    assert_eq!(nodes_file(&w), before);
    // Never in the trail: evidence_not_found, with the live pointers as candidates.
    let id = node(&mut w, &["docs/b.md#L1"]);
    let p = remove(&mut w, &id, "docs/b.md#L2", Some("stale"), None).unwrap_err();
    assert_eq!(p.code, "evidence_not_found");
    assert_eq!(p.candidates.len(), 1);
    assert_eq!(p.candidates[0].title.as_deref(), Some("docs/b.md#L1"));
}

#[test]
fn duplicates_are_ambiguous_until_at_picks_one() {
    let mut w = world();
    let id = node(&mut w, &["docs/a.md#L3", "docs/b.md#L1"]);
    w.svc
        .node_evidence_add(&ctx(), &id, "docs/a.md#L3", Some("room"))
        .unwrap();
    let before = snapshot(&w);
    let p = remove(&mut w, &id, "docs/a.md#L3", Some("stale"), None).unwrap_err();
    assert_eq!(p.code, "evidence_ambiguous");
    let positions: Vec<&str> = p.candidates.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(positions, vec!["1", "3"]);
    // --at must name a matching position.
    let p = remove(&mut w, &id, "docs/a.md#L3", Some("stale"), Some(2)).unwrap_err();
    assert_eq!(p.code, "usage_invalid");
    assert_eq!(nodes_file(&w), before);
    let (entry, _) = remove(&mut w, &id, "docs/a.md#L3", Some("dup"), Some(3)).unwrap();
    assert_eq!(entry.unwrap()["removed"]["kind"], "room");
    let left = on_disk(&w, &id).evidence;
    assert_eq!(left.len(), 2);
    assert_eq!(left[0]["kind"], "document");
}

#[test]
fn add_stores_one_pointer_and_a_repeat_writes_nothing() {
    let mut w = world();
    let id = node(&mut w, &[]);
    let added = w
        .svc
        .node_evidence_add(&ctx(), &id, "docs/a.md#section=Scope/Risks", None)
        .unwrap()
        .value;
    assert!(added.added);
    assert_eq!(added.row.at, 1);
    assert_eq!(added.row.text, "docs/a.md#section=Scope/Risks");
    let before = snapshot(&w);
    let again = w
        .svc
        .node_evidence_add(&ctx(), &id, "docs/a.md#section=Scope/Risks", None)
        .unwrap()
        .value;
    assert!(!again.added);
    assert_eq!(nodes_file(&w), before);
    let p = w
        .svc
        .node_evidence_add(&ctx(), &id, "../x.md#L1", None)
        .unwrap_err();
    assert_eq!(p.code, "evidence_outside_roots");
}

/// A graph with evidence but no removals keeps its bytes: no `evidence_removed` key appears, and reads and
/// no-op adds write nothing.
#[test]
fn no_removals_means_no_trail_key_and_the_same_bytes() {
    let mut w = world();
    let id = node(&mut w, &["docs/a.md#L3"]);
    let before = snapshot(&w);
    assert!(!String::from_utf8_lossy(&before).contains("evidence_removed"));
    listed(&w, &id, false);
    listed(&w, &id, true);
    w.svc
        .node_evidence_add(&ctx(), &id, "docs/a.md#L3", None)
        .unwrap();
    assert_eq!(nodes_file(&w), before);
}

/// The graph's own `evidence_removed` field (not a list of objects) is never overwritten.
#[test]
fn a_node_with_its_own_evidence_removed_field_refuses() {
    let mut w = world();
    let id = node(&mut w, &["docs/a.md#L3"]);
    let mut n = on_disk(&w, &id);
    n.fields.insert("evidence_removed".into(), json!("mine"));
    let dir = GraphDir::parse("/work/g").unwrap();
    w.files
        .write(&dir, GraphFile::Nodes, &codec::encode_nodes([&n]))
        .unwrap();
    w.svc.graph_unload(&ctx(), "demo").unwrap();
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    let before = snapshot(&w);
    let p = remove(&mut w, &id, "docs/a.md#L3", Some("stale"), None).unwrap_err();
    assert_eq!(p.code, "node_reserved_field_in_use");
    assert_eq!(nodes_file(&w), before);
    // Nor can anyone set it by hand.
    let p = w
        .svc
        .node_edit(
            &ctx(),
            &id,
            umx_services::NodeEditRequest {
                sets: vec![(
                    "evidence_removed".into(),
                    umx_core::value::SetValue::Raw("x".into()),
                )],
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(p.code, "reserved_name");
}

fn line(n: u32, op: BatchOp) -> BatchLine {
    BatchLine { line: n, op }
}

fn batch_remove(id: &str, pointer: &str, reason: Option<&str>, skip: bool) -> BatchOp {
    BatchOp::EvidenceRemove {
        node: Some(id.to_string()),
        node_ref: None,
        pointer: pointer.to_string(),
        reason: reason.map(str::to_string),
        at: None,
        skip_missing: skip,
    }
}

/// A tester's 60: one batch adds 60 pointers, a second removes them all with one reason (one line overrides it), and
/// a rerun of the same file is all already_removed warnings and writes nothing.
#[test]
fn a_batch_removes_sixty_pointers_with_one_reason() {
    let mut w = world();
    let id = node(&mut w, &[]);
    let pointers: Vec<String> = (1..=60)
        .map(|i| format!("calls/3.ndjson#seq={i}-{i}"))
        .collect();
    let adds = pointers
        .iter()
        .enumerate()
        .map(|(i, p)| {
            line(
                i as u32 + 1,
                BatchOp::EvidenceAdd {
                    node: Some(id.clone()),
                    node_ref: None,
                    pointer: p.clone(),
                    evidence_kind: None,
                },
            )
        })
        .collect();
    w.svc
        .batch(
            &ctx(),
            BatchRequest {
                lines: adds,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(on_disk(&w, &id).evidence.len(), 60);

    let removes = || -> Vec<BatchLine> {
        pointers
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let own = (i == 0).then_some("the first is a greeting");
                line(i as u32 + 1, batch_remove(&id, p, own, false))
            })
            .collect()
    };
    let done = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                lines: removes(),
                reason: Some("question lines are not evidence".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(done.notes.is_empty(), "{:?}", done.notes);
    let n = on_disk(&w, &id);
    assert!(n.evidence.is_empty());
    let trail = n.fields["evidence_removed"].as_array().unwrap();
    assert_eq!(trail.len(), 60);
    assert_eq!(trail[0]["reason"], "the first is a greeting");
    assert!(
        trail[1..]
            .iter()
            .all(|t| t["reason"] == "question lines are not evidence")
    );

    let before = snapshot(&w);
    let rerun = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                lines: removes(),
                reason: Some("question lines are not evidence".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(rerun.notes.len(), 60);
    assert!(rerun.notes.iter().all(|p| p.code == "already_removed"));
    assert_eq!(nodes_file(&w), before);
}

/// A remove line without a reason, in a batch without one, fails the batch; a pointer that matches nothing fails it
/// too unless the line says `"missing": "skip"`. A failed batch writes nothing.
#[test]
fn a_batch_needs_a_reason_and_fails_on_a_missing_pointer_unless_skipped() {
    let mut w = world();
    let id = node(&mut w, &["docs/a.md#L3"]);
    let before = snapshot(&w);
    let run = |w: &mut World, lines: Vec<BatchLine>, reason: Option<&str>| {
        w.svc.batch(
            &ctx(),
            BatchRequest {
                lines,
                reason: reason.map(str::to_string),
                ..Default::default()
            },
        )
    };
    let errors = run(
        &mut w,
        vec![line(1, batch_remove(&id, "docs/a.md#L3", None, false))],
        None,
    )
    .unwrap_err();
    assert_eq!(errors[1].code, "usage_invalid");
    let errors = run(
        &mut w,
        vec![
            line(1, batch_remove(&id, "docs/a.md#L3", None, false)),
            line(2, batch_remove(&id, "docs/gone.md#L1", None, false)),
        ],
        Some("stale"),
    )
    .unwrap_err();
    assert_eq!(errors[0].code, "batch_failed");
    assert_eq!(errors[1].code, "evidence_not_found");
    assert_eq!(errors[1].line, Some(2));
    assert_eq!(nodes_file(&w), before, "all or nothing");
    let done = run(
        &mut w,
        vec![
            line(1, batch_remove(&id, "docs/a.md#L3", None, false)),
            line(2, batch_remove(&id, "docs/gone.md#L1", None, true)),
        ],
        Some("stale"),
    )
    .unwrap();
    assert_eq!(done.notes[0].code, "evidence_missing_skipped");
    assert_eq!(done.notes[0].line, Some(2));
    assert!(on_disk(&w, &id).evidence.is_empty());
}

/// Matching is text only: a `umx:` pointer into a graph that is not loaded (or secured) is removed without looking
/// that graph up. A typed id prefix matches the full id stored on this node, under the same alias only (F2);
/// the `?repo=` hint is not part of the match either way (F1).
#[test]
fn umx_pointers_match_as_text_with_prefixes_and_without_the_hint() {
    let mut w = world();
    let full = "umx:secret_graph/01JC8XA4H7Q1V9K2M3N4P5R6S7#L1";
    let hinted =
        "umx:secret_graph/01JC8XB000Q1V9K2M3N4P5R6S7#L2?repo=https://example.invalid/x.git";
    let id = node(&mut w, &[full, hinted]);
    // Another alias with the same id is not this pointer.
    let p = remove(
        &mut w,
        &id,
        "umx:other_graph/01JC8XA4H7#L1",
        Some("stale"),
        None,
    )
    .unwrap_err();
    assert_eq!(p.code, "evidence_not_found");
    // A lower-case prefix, scheme in any case: matched as text.
    let (entry, warnings) = remove(
        &mut w,
        &id,
        "UMX:secret_graph/01jc8xa4h7#L1",
        Some("stale"),
        None,
    )
    .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        entry.unwrap()["removed"]["source"],
        "umx:secret_graph/01JC8XA4H7Q1V9K2M3N4P5R6S7"
    );
    // A prefix two pointers share is ambiguous, as any double match.
    let id = node(
        &mut w,
        &[full, "umx:secret_graph/01JC8XA4H7ZZZZZZZZZZZZZZZZ#L1"],
    );
    let p = remove(
        &mut w,
        &id,
        "umx:secret_graph/01JC8XA4H7#L1",
        Some("stale"),
        None,
    )
    .unwrap_err();
    assert_eq!(p.code, "evidence_ambiguous");
    // The stored hint is ignored, and a given hint too; the trail keeps the stored source as it was.
    let other = node(&mut w, &[hinted]);
    let (entry, _) = remove(
        &mut w,
        &other,
        "umx:secret_graph/01JC8XB000Q1V9K2M3N4P5R6S7#L2",
        Some("stale"),
        None,
    )
    .unwrap();
    assert_eq!(
        entry.unwrap()["removed"]["source"],
        "umx:secret_graph/01JC8XB000Q1V9K2M3N4P5R6S7?repo=https://example.invalid/x.git"
    );
    let (entry, warnings) = remove(
        &mut w,
        &id,
        "umx:secret_graph/01JC8XA4H7ZZ#L1?repo=https://elsewhere.invalid/y.git",
        Some("stale"),
        None,
    )
    .unwrap();
    assert!(warnings.is_empty() && entry.is_some(), "{warnings:?}");
}

/// Security N2: a reason may not hold anything that moves, hides or reorders text.
#[test]
fn a_reason_with_control_bidi_or_zero_width_characters_is_refused() {
    let mut w = world();
    let id = node(&mut w, &["docs/a.md#L3"]);
    for reason in [
        "line\nbreak",
        "tab\there",
        "rtl \u{202E}evil",
        "iso \u{2067}x",
        "zero\u{200B}width",
        "bom\u{FEFF}",
    ] {
        let p = remove(&mut w, &id, "docs/a.md#L3", Some(reason), None).unwrap_err();
        assert_eq!(p.code, "usage_invalid", "{reason:?}");
    }
    remove(
        &mut w,
        &id,
        "docs/a.md#L3",
        Some("stale: was a question (2026)"),
        None,
    )
    .unwrap();
}

/// Security N1: a removal reason that looks like a secret is named by the write scan (field and rule only,
/// never the text): the verb warns secret_suspected, as every node write does, and a batch line is refused.
#[test]
fn a_secret_looking_reason_is_caught_by_the_write_scan() {
    let mut w = world();
    let id = node(&mut w, &["docs/a.md#L3", "docs/b.md#L1"]);
    let token = format!("ghp_{}", "c".repeat(36));
    let (_, warnings) = remove(&mut w, &id, "docs/a.md#L3", Some(&token), None).unwrap();
    assert_eq!(warnings, vec!["secret_suspected"]);
    let before = snapshot(&w);
    let errors = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                lines: vec![line(1, batch_remove(&id, "docs/b.md#L1", None, false))],
                reason: Some(token.clone()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(
        errors.iter().any(|p| p.code == "batch_line_invalid"
            && format!("{p:?}").contains("evidence_removed_reason matches the secret rule")),
        "{errors:?}"
    );
    assert!(errors.iter().all(|p| !format!("{p:?}").contains(&token)));
    assert_eq!(nodes_file(&w), before, "a refused batch writes nothing");
}

/// A writer in its own harness session (ws007 P2), so the adding and removing sessions differ.
fn writer(session: &str) -> Ctx {
    ctx().actor(
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

fn trail_of(n: &Node) -> &Vec<Value> {
    n.fields["evidence_removed"].as_array().unwrap()
}

/// graph-addressing-expert ruling: the entry's `by_session` is the session that removed the pointer (the
/// write's own session), the pointer under `removed` is byte for byte what was stored (its `by_session`, the adder,
/// included), and a later removal stamps only its own entry.
#[test]
fn the_trail_names_the_removing_session_and_keeps_the_adders_pointer_verbatim() {
    let mut w = world();
    let id = node(&mut w, &["docs/a.md#L3", "docs/b.md#L1"]);
    let stored = on_disk(&w, &id).evidence.clone();
    let adder = stored[0]["by_session"]
        .as_str()
        .expect("P2 stamps the adding session")
        .to_string();
    let done = w
        .svc
        .node_evidence_remove(
            &writer("h-0123456789abcdef"),
            &id,
            "docs/a.md#L3",
            Some("stale"),
            None,
        )
        .unwrap();
    let n = on_disk(&w, &id);
    let first = n.edited_by_session.unwrap().to_text();
    assert_ne!(first, adder);
    let entry = &trail_of(&n)[0];
    assert_eq!(entry["by_session"], first);
    assert_eq!(
        serde_json::to_string(&entry["removed"]).unwrap(),
        serde_json::to_string(&stored[0]).unwrap(),
        "the adder's pointer, byte for byte"
    );
    assert_eq!(entry["removed"]["by_session"], adder);
    assert_eq!(
        done.value.removed.unwrap().pointer,
        *entry,
        "the reply is the entry as stored"
    );

    w.svc
        .node_evidence_remove(
            &writer("h-fedcba9876543210"),
            &id,
            "docs/b.md#L1",
            Some("stale too"),
            None,
        )
        .unwrap();
    let n = on_disk(&w, &id);
    let second = n.edited_by_session.unwrap().to_text();
    assert_ne!(second, first);
    let trail = trail_of(&n);
    assert_eq!(
        (&trail[0]["by_session"], &trail[1]["by_session"]),
        (&Value::from(first), &Value::from(second)),
        "an earlier entry keeps its remover"
    );
    assert_eq!(trail[1]["removed"], stored[1]);
}

/// A pointer stored before ws007 P2 has no `by_session`; removing it invents none, while the entry still names the
/// removing session.
#[test]
fn a_pointer_from_before_p2_gets_no_by_session_on_removal() {
    let mut w = world();
    let id = node(&mut w, &["docs/a.md#L3"]);
    let mut records = codec::decode_nodes(&nodes_file(&w)).unwrap().records;
    for pointer in records.iter_mut().flat_map(|n| n.evidence.iter_mut()) {
        pointer.as_object_mut().unwrap().remove("by_session");
    }
    w.files
        .write_set(
            &GraphDir::parse("/work/g").unwrap(),
            &[Change {
                file: GraphFile::Nodes.into(),
                bytes: Some(&codec::encode_nodes(&records)),
            }],
        )
        .unwrap();
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    let stored = on_disk(&w, &id).evidence[0].clone();
    assert!(stored.get("by_session").is_none(), "{stored}");
    remove(&mut w, &id, "docs/a.md#L3", Some("stale"), None).unwrap();
    let n = on_disk(&w, &id);
    let entry = &trail_of(&n)[0];
    assert_eq!(entry["removed"], stored);
    assert!(entry["removed"].get("by_session").is_none(), "{entry}");
    assert_eq!(entry["by_session"], n.edited_by_session.unwrap().to_text());
}

/// A batch `evidence remove` line stamps the batch's session on its entry, as the verb does.
#[test]
fn a_batch_remove_stamps_the_batch_session_on_the_trail() {
    let mut w = world();
    let id = node(&mut w, &["docs/a.md#L3", "docs/b.md#L1"]);
    let stored = on_disk(&w, &id).evidence[1].clone();
    w.svc
        .batch(
            &writer("h-00112233445566aa"),
            BatchRequest {
                lines: vec![line(1, batch_remove(&id, "docs/b.md#L1", None, false))],
                reason: Some("stale".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let n = on_disk(&w, &id);
    let session = n.edited_by_session.unwrap().to_text();
    assert_ne!(Some(session.as_str()), stored["by_session"].as_str());
    let entry = &trail_of(&n)[0];
    assert_eq!(entry["by_session"], session);
    assert_eq!(entry["removed"], stored);
}

/// F144 (graph-storage-expert): a list of a graph whose nodes.jsonl changed on disk since the load still lists (memory
/// as loaded) and warns stale_files, as every read does; a remove is refused graph_files_changed.
#[test]
fn a_list_after_nodes_jsonl_changed_on_disk_warns_stale_files() {
    let mut w = world();
    let id = node(&mut w, &["docs/a.md#L3"]);
    let fresh = w.svc.node_evidence_list(&ctx(), &id, false).unwrap();
    assert!(fresh.notes.is_empty(), "{:?}", fresh.notes);
    w.files
        .touch_for_tests(&GraphDir::parse("/work/g").unwrap(), GraphFile::Nodes);
    let done = w.svc.node_evidence_list(&ctx(), &id, false).unwrap();
    assert_eq!(
        done.notes.iter().map(|p| p.code).collect::<Vec<_>>(),
        ["stale_files"]
    );
    assert_eq!(done.value.rows.len(), 1);
    let refused = remove(&mut w, &id, "docs/a.md#L3", Some("stale"), None).unwrap_err();
    assert_eq!(refused.code, "graph_files_changed");
}

// F183 redact, review round 1 (R1-R7). Synthetic needles only.

fn redact(
    w: &mut World,
    id: &str,
    pointer: &str,
    reason: &str,
) -> Result<umx_services::Redacted, Problem> {
    w.svc
        .node_evidence_redact(&ctx(), id, pointer, Some(reason))
        .map(|d| d.value.redacted.unwrap())
}

fn redact_files(w: &World) -> Vec<Vec<u8>> {
    let dir = GraphDir::parse("/work/g").unwrap();
    [GraphFile::Nodes, GraphFile::Edges, GraphFile::Graph]
        .map(|f| w.files.read(&dir, f).unwrap().unwrap_or_default())
        .to_vec()
}

fn file_text(w: &World) -> String {
    redact_files(w)
        .iter()
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .collect()
}

/// Rewrites nodes.jsonl (or edges.jsonl) line by line, then loads the graph again, as a hand edit or merge would.
fn hand_edit(w: &mut World, file: GraphFile, f: impl Fn(&str) -> String) {
    let dir = GraphDir::parse("/work/g").unwrap();
    let text = String::from_utf8(w.files.read(&dir, file).unwrap().unwrap()).unwrap();
    let edited: String = text.lines().map(f).collect();
    w.files.write(&dir, file, edited.as_bytes()).unwrap();
    w.svc.graph_unload(&ctx(), "demo").unwrap();
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
}

fn blocked(p: &Problem) -> Vec<(String, String)> {
    assert_eq!(p.code, "evidence_redact_blocked", "{p:?}");
    p.candidates
        .iter()
        .map(|c| (c.id.clone(), c.title.clone().unwrap_or_default()))
        .collect()
}

/// R1: a lower-case id prefix names the stored full id, and every pointer with that identity is scrubbed.
#[test]
fn a_redact_scrubs_by_the_stored_identity_from_a_lowercase_prefix() {
    let mut w = world();
    let a = node(&mut w, &["umx:secret_graph/01JC8XA4H7Q1V9K2M3N4P5R6S7#L1"]);
    let b = node(&mut w, &["UMX:secret_graph/01jc8xa4h7q1v9k2m3n4p5r6s7#L2"]);
    let done = redact(
        &mut w,
        &a,
        "umx:secret_graph/01jc8xa4h7#L1",
        "it named a secured graph",
    )
    .unwrap();
    assert_eq!((done.pointers, done.nodes), (2, 2), "{b}");
    let text = file_text(&w).to_lowercase();
    assert!(!text.contains("01jc8xa4h7q1v9k2m3n4p5r6s7"), "{text}");
}

/// R1: http and https schemes fold in any case; the path keeps its case.
#[test]
fn a_redact_folds_mixed_case_url_schemes() {
    let mut w = world();
    let a = node(&mut w, &["https://example.invalid/Leak73#L1"]);
    node(
        &mut w,
        &["HTTPS://example.invalid/Leak73#quote=nothing private"],
    );
    node(&mut w, &["https://example.invalid/Other74#L1"]);
    let done = redact(
        &mut w,
        &a,
        "https://example.invalid/Leak73#L1",
        "a private page",
    )
    .unwrap();
    assert_eq!((done.pointers, done.nodes), (2, 2));
    let text = file_text(&w);
    assert!(!text.contains("Leak73"), "{text}");
    assert!(
        text.contains("example.invalid/Other74"),
        "another source stays"
    );
}

/// R1, R3: a prefix that names two stored identities is ambiguous, and the refusal lists no pointer.
#[test]
fn a_redact_of_an_ambiguous_prefix_is_refused_without_its_pointers() {
    let mut w = world();
    let a = node(
        &mut w,
        &[
            "umx:sg/01JC8XA4H7Q1V9K2M3N4P5R6S7#L1",
            "umx:sg/01JC8XA4H7ZZZZZZZZZZZZZZZZ#L1",
        ],
    );
    let before = redact_files(&w);
    let p = redact(&mut w, &a, "umx:sg/01JC8XA4H7#L1", "private").unwrap_err();
    assert_eq!(p.code, "evidence_ambiguous");
    assert!(p.candidates.is_empty(), "{p:?}");
    assert_eq!(redact_files(&w), before);
}

/// R3: a pointer the node does not hold is refused without listing the node's pointers.
#[test]
fn a_redact_that_names_nothing_lists_no_pointer() {
    let mut w = world();
    let a = node(&mut w, &["leak/src73.md#quote=privq73"]);
    let p = redact(&mut w, &a, "leak/src73.md#L999", "private").unwrap_err();
    assert_eq!(p.code, "evidence_not_found");
    let shown = format!("{p:?}");
    assert!(
        p.candidates.is_empty() && !shown.contains("privq73"),
        "{shown}"
    );
}

/// R2, R3: a custom field's key, a typed field and a kept trail entry's metadata all block; a custom field is placed
/// by ordinal, never named.
#[test]
fn a_redact_checks_keys_typed_fields_and_kept_metadata() {
    let mut w = world();
    let a = node(&mut w, &["leak/src73.md#quote=privq73"]);
    let keyed = node(&mut w, &[]);
    w.svc
        .node_edit(
            &ctx(),
            &keyed,
            umx_services::NodeEditRequest {
                sets: vec![(
                    "privq73".into(),
                    umx_core::value::SetValue::Json(json!(true)),
                )],
                ..Default::default()
            },
        )
        .unwrap();
    let author = w
        .svc
        .node_add(
            &Ctx::new("privq73"),
            NodeAddRequest {
                kind: "problem".into(),
                title: "Made by someone".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id
        .to_text();
    // A legacy trail entry whose `by` is the quote: it is redacted in place, but `by` would stay.
    let trailed = node(&mut w, &[]);
    hand_edit(&mut w, GraphFile::Nodes, |l| {
        let mut line: Value = serde_json::from_str(l).unwrap();
        if line["id"] == json!(trailed) {
            line["evidence_removed"] = json!([{
                "removed": {"source": "leak/src73.md", "selector": {"lines": [1, 1]}, "kind": "document"},
                "removed_at": "2026-01-01T00:00:00Z", "reason": "stale", "by": "privq73"
            }]);
        }
        format!("{line}\n")
    });
    let before = redact_files(&w);
    let p = redact(&mut w, &a, "leak/src73.md#quote=privq73", "private").unwrap_err();
    let places = blocked(&p);
    for (id, place) in [
        (keyed.as_str(), "field 1"),
        (author.as_str(), "created_by"),
        (trailed.as_str(), "evidence_removed:1"),
    ] {
        assert!(
            places.iter().any(|(i, p)| i == id && p == place),
            "{id} {place}: {places:?}"
        );
    }
    assert!(!format!("{p:?}").contains("privq73"), "{p:?}");
    assert_eq!(redact_files(&w), before);

    // Fail closed: a quote that is also the pointer's kind would stay in the redacted entry's kind.
    let mut w = world();
    let a = node(&mut w, &["docs/a.md#quote=document"]);
    let places = blocked(&redact(&mut w, &a, "docs/a.md#quote=document", "private").unwrap_err());
    assert!(
        places
            .iter()
            .any(|(i, p)| *i == a && p == "evidence_removed:1"),
        "{places:?}"
    );
}

/// R4: a kept repeated line is read as JSON strings, so an escaped spelling of a needle blocks (nodes and edges).
#[test]
fn a_redact_reads_escaped_needles_in_kept_lines() {
    let mut w = world();
    let a = node(&mut w, &["leak/src73.md#quote=PRIVQ73"]);
    let other = node(&mut w, &[]);
    w.svc.edge_add(&ctx(), &a, "supports", &other, &[]).unwrap();
    hand_edit(&mut w, GraphFile::Nodes, |l| {
        let line: Value = serde_json::from_str(l).unwrap();
        if line["id"] == json!(a) {
            let earlier = l
                .replace("leak/src73.md", "leak\\/src73.md")
                .replace("PRIVQ73", "\\u0050RIVQ73");
            assert_ne!(earlier, l);
            format!("{earlier}\n{l}\n")
        } else {
            format!("{l}\n")
        }
    });
    hand_edit(&mut w, GraphFile::Edges, |l| {
        let mut earlier: Value = serde_json::from_str(l).unwrap();
        earlier["memo"] = json!("x");
        let earlier = earlier
            .to_string()
            .replace("\"memo\":\"x\"", "\"memo\":\"\\u0050RIVQ73\"");
        format!("{earlier}\n{l}\n")
    });
    let before = redact_files(&w);
    let places =
        blocked(&redact(&mut w, &a, "leak/src73.md#quote=PRIVQ73", "private").unwrap_err());
    assert!(places.iter().any(|(i, _)| i == "nodes.jsonl"), "{places:?}");
    assert!(places.iter().any(|(i, _)| i == "edges.jsonl"), "{places:?}");
    assert_eq!(redact_files(&w), before);
}

/// R5: graph.json is read as it is on disk, so an earlier duplicate member the loaded metadata dropped still blocks.
#[test]
fn a_redact_reads_graph_json_as_written() {
    let mut w = world();
    let a = node(&mut w, &["leak/src73.md#quote=PRIVQ73"]);
    let dir = GraphDir::parse("/work/g").unwrap();
    let meta = String::from_utf8(w.files.read(&dir, GraphFile::Graph).unwrap().unwrap()).unwrap();
    let tampered = meta.replacen(
        "\"ontology\": {",
        "\"ontology\": {\"memo\": \"PRIVQ73\", \"memo\": \"safe\",",
        1,
    );
    assert_ne!(tampered, meta);
    w.files
        .write(&dir, GraphFile::Graph, tampered.as_bytes())
        .unwrap();
    w.svc.graph_unload(&ctx(), "demo").unwrap();
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    let places =
        blocked(&redact(&mut w, &a, "leak/src73.md#quote=PRIVQ73", "private").unwrap_err());
    assert_eq!(
        places,
        vec![("graph.json".to_string(), "the graph's metadata".to_string())]
    );
}

/// R6: a reason that holds a needle exactly is refused even where whole-string lower-casing would hide it.
#[test]
fn a_redact_refuses_a_reason_holding_a_unicode_needle() {
    let mut w = world();
    let a = node(&mut w, &["docs/a.md#quote=ΟΣ"]);
    for reason in ["ΟΣX leaked", "οσx leaked"] {
        let p = redact(&mut w, &a, "docs/a.md#quote=ΟΣ", reason).unwrap_err();
        assert_eq!(p.code, "usage_invalid", "{reason}");
    }
}

/// R7: an unrelated node with its own scalar `evidence_removed` does not stop the redact, and is kept as it is.
#[test]
fn a_redact_never_needs_a_trail_on_an_unrelated_node() {
    let mut w = world();
    let a = node(&mut w, &["leak/src73.md#L1"]);
    let b = node(&mut w, &[]);
    hand_edit(&mut w, GraphFile::Nodes, |l| {
        let mut line: Value = serde_json::from_str(l).unwrap();
        if line["id"] == json!(b) {
            line["evidence_removed"] = json!("mine");
        }
        format!("{line}\n")
    });
    let done = redact(&mut w, &a, "leak/src73.md#L1", "private").unwrap();
    assert_eq!((done.pointers, done.nodes), (1, 1));
    assert_eq!(on_disk(&w, &b).fields["evidence_removed"], json!("mine"));
}

/// F203 N2 (security): a node the redact leaves as it is keeps its line byte for byte, so a duplicate member
/// holding the needle (which the decoded record drops) still blocks, in either order, and nothing is written.
#[test]
fn a_redact_reads_an_unchanged_nodes_line_as_stored() {
    for members in [
        "\"memo\":\"PRIVQ73\",\"memo\":\"safe\",",
        "\"memo\":\"safe\",\"memo\":\"PRIVQ73\",",
    ] {
        let mut w = world();
        let a = node(&mut w, &["leak/src73.md#quote=PRIVQ73"]);
        let other = node(&mut w, &[]);
        hand_edit(&mut w, GraphFile::Nodes, |l| {
            let line: Value = serde_json::from_str(l).unwrap();
            if line["id"] == json!(other) {
                format!("{}\n", l.replacen('{', &format!("{{{members}"), 1))
            } else {
                format!("{l}\n")
            }
        });
        let before = redact_files(&w);
        let p = redact(&mut w, &a, "leak/src73.md#quote=PRIVQ73", "private").unwrap_err();
        let places = blocked(&p);
        assert!(
            !places.is_empty() && places.iter().all(|(i, _)| *i == other),
            "{members}: {places:?}"
        );
        assert!(!format!("{p:?}").contains("PRIVQ73"), "{p:?}");
        assert_eq!(redact_files(&w), before, "{members}");
    }
}

/// F203 N1 (security): the redact reads each record it writes once the write has added its attribution (the
/// session in `edited_by_session` and on the trail). Ids are sequential here, so a first world learns the session the
/// redact will get (a harness session of its own), and a second, identical world quotes it: only the attribution
/// holds it, and the redact refuses.
#[test]
fn a_redact_reads_its_records_with_their_new_attribution() {
    let run = |quote: &str| {
        let mut w = world();
        let pointer = format!("docs/a.md#quote={quote}");
        let a = node(&mut w, &[pointer.as_str()]);
        let b = node(&mut w, &[]);
        let edge = w
            .svc
            .edge_add_with(
                &ctx(),
                &a,
                "supports",
                &b,
                &[],
                &umx_services::EdgeEvidence {
                    pointers: vec![pointer.clone()],
                    kind: None,
                },
            )
            .unwrap()
            .value
            .edge
            .id
            .to_text();
        let before = redact_files(&w);
        let done =
            w.svc
                .node_evidence_redact(&writer("redact-session"), &a, &pointer, Some("private"));
        (w, a, edge, before, done)
    };
    let (w, a, _, _, done) = run("ZZPLACEHOLDER");
    assert_eq!(done.unwrap().value.redacted.unwrap().edges, 1);
    let session = on_disk(&w, &a).edited_by_session.unwrap().to_text();

    let (w, a, edge, before, done) = run(&session);
    let p = done.unwrap_err();
    let places = blocked(&p);
    for id in [&a, &edge] {
        assert!(
            places
                .iter()
                .any(|(i, p)| i == id && p == "edited_by_session"),
            "{id}: {places:?}"
        );
    }
    // The quote is the new session record's own id here, so that record is a candidate by its id; no place holds text.
    assert!(places.iter().all(|(_, p)| !p.contains(&session)), "{p:?}");
    assert_eq!(redact_files(&w), before);
}

/// F203 N3 (security): needles match under Unicode's full case folding, so `ß` meets `SS` and a final `ς`
/// meets `Σ`, where lower case alone keeps them apart.
#[test]
fn a_redact_refuses_a_reason_holding_a_case_folded_needle() {
    let mut w = world();
    let a = node(&mut w, &["docs/a.md#quote=Straße"]);
    let b = node(&mut w, &["docs/b.md#quote=ΟΣ"]);
    for (id, pointer, reason) in [
        (&a, "docs/a.md#quote=Straße", "STRASSE leaked"),
        (&a, "docs/a.md#quote=Straße", "strasse leaked"),
        (&b, "docs/b.md#quote=ΟΣ", "οςx leaked"),
    ] {
        let p = redact(&mut w, id, pointer, reason).unwrap_err();
        assert_eq!(p.code, "usage_invalid", "{reason}");
    }
}

/// F203 N4 (security): the pointer given names the stored one by identity, so an http or https scheme typed
/// in another case than the stored one still names it; the path keeps its case.
#[test]
fn a_redact_names_a_url_pointer_whatever_the_schemes_case() {
    let mut w = world();
    let a = node(&mut w, &["HTTPS://example.invalid/Leak73#L2"]);
    let b = node(&mut w, &["https://example.invalid/Leak74#L2"]);
    let done = redact(&mut w, &a, "https://example.invalid/Leak73#L2", "private").unwrap();
    assert_eq!((done.pointers, done.nodes), (1, 1));
    let p = redact(&mut w, &b, "https://example.invalid/leak74#L2", "private").unwrap_err();
    assert_eq!(p.code, "evidence_not_found");
    let done = redact(&mut w, &b, "Https://example.invalid/Leak74#L2", "private").unwrap();
    assert_eq!((done.pointers, done.nodes), (1, 1));
    assert!(!file_text(&w).contains("Leak7"), "{}", file_text(&w));
}

/// An actor claim with `tier` and the given role, user and harness session (and a seat and harness for pij).
fn actor_ctx(tier: Tier, role: Option<&str>, user: &str, session: Option<&str>) -> Ctx {
    let pij = matches!(tier, Tier::Pij);
    ctx().actor(
        Actor::try_from(ActorInput {
            tier,
            seat: pij.then(|| "pij-alpha-seat".into()),
            role: role.map(Into::into),
            model: None,
            effort: None,
            harness: pij.then(|| "copilot".into()),
            harness_session: session.map(Into::into),
            user: Some(user.into()),
        })
        .unwrap(),
    )
}

/// Review R1 (security's N1): the redact reads every record its write puts down, the session and
/// agent records it upserts included. A new CLI actor whose user is the quote, and a known actor whose role becomes
/// the quote (its session and agent records edited), each refuse with the writer by `by` safe, and nothing is written.
#[test]
fn a_redact_reads_the_provenance_records_its_write_puts_down() {
    let mut w = world();
    let a = node(&mut w, &["leak/src73.md#quote=privq73"]);
    let before = redact_files(&w);
    let new_actor = actor_ctx(Tier::Cli, None, "privq73", None);
    let p = w
        .svc
        .node_evidence_redact(
            &new_actor,
            &a,
            "leak/src73.md#quote=privq73",
            Some("private"),
        )
        .unwrap_err();
    let places = blocked(&p);
    assert!(places.iter().any(|(i, _)| *i != a), "{places:?}");
    assert!(!format!("{p:?}").contains("privq73"), "{p:?}");
    assert_eq!(redact_files(&w), before);

    let known = |role| actor_ctx(Tier::Pij, Some(role), "tester", Some("s-0123456789abcdef"));
    w.svc
        .node_add(
            &known("coder"),
            NodeAddRequest {
                kind: "problem".into(),
                title: "Written by a known actor".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let held: Vec<String> = codec::decode_nodes(&nodes_file(&w))
        .unwrap()
        .records
        .iter()
        .map(|n| n.id.to_text())
        .collect();
    let before = redact_files(&w);
    let places = blocked(
        &w.svc
            .node_evidence_redact(
                &known("privq73"),
                &a,
                "leak/src73.md#quote=privq73",
                Some("private"),
            )
            .unwrap_err(),
    );
    assert!(
        places.iter().any(|(i, _)| *i != a && held.contains(i)),
        "an edited provenance record: {places:?}"
    );
    assert_eq!(redact_files(&w), before);
}

/// S365-1 (storage review): umx-graph keeps no edge line bytes, so the redact always rewrites edges.jsonl from the
/// records. An edge line with a duplicate member holding the quote (the decoded record keeps the safe copy) is scrubbed
/// by a redact that changes no edge.
#[test]
fn a_redact_rewrites_edges_jsonl_even_when_no_edge_changes() {
    let mut w = world();
    let a = node(&mut w, &["leak/src73.md#quote=PRIVQ73"]);
    let other = node(&mut w, &[]);
    w.svc.edge_add(&ctx(), &a, "supports", &other, &[]).unwrap();
    hand_edit(&mut w, GraphFile::Edges, |l| {
        format!(
            "{}\n",
            l.replacen('{', "{\"memo\":\"PRIVQ73\",\"memo\":\"safe\",", 1)
        )
    });
    assert!(file_text(&w).contains("PRIVQ73"));
    let done = redact(&mut w, &a, "leak/src73.md#quote=PRIVQ73", "private").unwrap();
    assert_eq!((done.nodes, done.edges), (1, 0));
    assert!(!file_text(&w).contains("PRIVQ73"), "{}", file_text(&w));
}

/// F196 (storage review): once a redact's write is in, the graph holds no undo line, so the scrubbed line it replaced
/// is not kept in memory until the next write.
#[test]
fn a_redact_leaves_no_undo_line_behind() {
    let mut w = world();
    let a = node(&mut w, &["leak/src73.md#quote=PRIVQ73"]);
    redact(&mut w, &a, "leak/src73.md#quote=PRIVQ73", "private").unwrap();
    let undo = w.svc.loaded_graph("demo").unwrap().undo_line();
    assert!(undo.is_none(), "{:?}", undo.map(String::from_utf8_lossy));
}

/// F197: the redact passes over a line that cannot hold a needle without parsing it, and decodes only the nodes that
/// may hold the pointer. Neither shortcut misses one: a needle in another case in a node's note and in an edge's
/// field still blocks, and a `umx:` pointer whose stored id spells a Crockford alias (`O` for `0`) is still found and
/// scrubbed, though its text is not the identity's.
#[test]
fn a_redact_shortcut_never_misses_a_needle_or_a_holder() {
    let mut w = world();
    let a = node(&mut w, &["leak/src73.md#quote=privq73"]);
    let noted = node(&mut w, &[]);
    w.svc
        .node_edit(
            &ctx(),
            &noted,
            umx_services::NodeEditRequest {
                note: Some("Mentions PRIVQ73 in passing".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let edge = w
        .svc
        .edge_add(
            &ctx(),
            &a,
            "supports",
            &noted,
            &[(
                "memo".into(),
                umx_core::value::SetValue::Json(json!("Says Privq73")),
            )],
        )
        .unwrap()
        .value
        .edge
        .id
        .to_text();
    let before = redact_files(&w);
    let places =
        blocked(&redact(&mut w, &a, "leak/src73.md#quote=privq73", "private").unwrap_err());
    for (id, place) in [(noted.as_str(), "note"), (edge.as_str(), "field 1")] {
        assert!(
            places.iter().any(|(i, p)| i == id && p == place),
            "{id} {place}: {places:?}"
        );
    }
    assert_eq!(redact_files(&w), before);

    let mut w = world();
    let a = node(&mut w, &["umx:sg/01JC8XA4H7Q1V9K2M3N4P5R6S7#L1"]);
    let b = node(&mut w, &["umx:sg/O1JC8XA4H7Q1V9K2M3N4P5R6S7#L2"]);
    let done = redact(
        &mut w,
        &a,
        "umx:sg/01JC8XA4H7Q1V9K2M3N4P5R6S7#L1",
        "private",
    )
    .unwrap();
    assert_eq!((done.pointers, done.nodes), (2, 2), "{b}");
    let text = file_text(&w).to_lowercase();
    assert!(!text.contains("1jc8xa4h7q1v9k2m3n4p5r6s7"), "{text}");
}

/// F213, F218: the redact's prefilter passes over only an ASCII line with no escape. An active edge's field and an
/// active node's note that hold the needle only once their escapes are read, or only once a non-ASCII character is
/// folded (the Kelvin sign `K` folds to `k`), still block: those lines are parsed, not lower-cased. An unchanged edge
/// is read as umx encodes it, so its escape must be the encoder's own: the needle holds a quote (`privq"73`), which
/// every JSON line writes `\"` (F218: a hand `\u0050` on an edge became a plain `P`, read by the shortcut).
#[test]
fn a_redact_reads_escaped_and_non_ascii_active_lines() {
    // Only the edge holds the needle, first, so a shortcut that misses an escaped edge fails here.
    let mut w = world();
    let a = node(&mut w, &["leak/src73.md#quote=privq%2273"]);
    let other = node(&mut w, &[]);
    let memo = [(
        "memo".into(),
        umx_core::value::SetValue::Json(json!("Says PRIVQ\"73")),
    )];
    let edge = w
        .svc
        .edge_add(&ctx(), &a, "supports", &other, &memo)
        .unwrap();
    let edge = edge.value.edge.id.to_text();
    let text = file_text(&w);
    assert!(text.contains(r#""memo":"Says PRIVQ\"73""#), "{text}");
    let before = redact_files(&w);
    let refused = redact(&mut w, &a, "leak/src73.md#quote=privq%2273", "private");
    let places = blocked(&refused.expect_err("the escaped edge blocks"));
    assert!(
        places.iter().any(|(i, p)| *i == edge && p == "field 1"),
        "escaped edge: {places:?}"
    );
    assert_eq!(redact_files(&w), before);

    // A node's line is read as stored, so a hand `\u0050` stays and only the parse finds the needle.
    let mut w = world();
    let a = node(&mut w, &["leak/src72.md#quote=privq72"]);
    let noted = node(&mut w, &[]);
    w.svc
        .node_edit(
            &ctx(),
            &noted,
            umx_services::NodeEditRequest {
                note: Some("Mentions PRIVQ72 in passing".into()),
                ..Default::default()
            },
        )
        .unwrap();
    hand_edit(&mut w, GraphFile::Nodes, |l| {
        format!("{}\n", l.replace("PRIVQ72", "\\u0050RIVQ72"))
    });
    let text = file_text(&w);
    assert!(
        text.contains(r#""note":"Mentions \u0050RIVQ72 in passing""#),
        "{text}"
    );
    let before = redact_files(&w);
    let refused = redact(&mut w, &a, "leak/src72.md#quote=privq72", "private");
    let places = blocked(&refused.expect_err("the escaped note blocks"));
    assert!(
        places.iter().any(|(i, p)| *i == noted && p == "note"),
        "escaped note: {places:?}"
    );
    assert_eq!(redact_files(&w), before);

    let mut w = world();
    let a = node(&mut w, &["leak/src74.md#quote=kq74x"]);
    let noted = node(&mut w, &[]);
    w.svc
        .node_edit(
            &ctx(),
            &noted,
            umx_services::NodeEditRequest {
                note: Some("Mentions \u{212A}Q74X in passing".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let memo = [(
        "memo".into(),
        umx_core::value::SetValue::Json(json!("Says \u{212A}q74x")),
    )];
    let edge = w
        .svc
        .edge_add(&ctx(), &a, "supports", &noted, &memo)
        .unwrap();
    let edge = edge.value.edge.id.to_text();
    let text = file_text(&w);
    assert!(
        text.contains("Mentions \u{212A}Q74X") && text.contains("Says \u{212A}q74x"),
        "{text}"
    );
    let before = redact_files(&w);
    let places = blocked(&redact(&mut w, &a, "leak/src74.md#quote=kq74x", "private").unwrap_err());
    for (id, place) in [(noted.as_str(), "note"), (edge.as_str(), "field 1")] {
        assert!(
            places.iter().any(|(i, p)| i == id && p == place),
            "non-ASCII {id} {place}: {places:?}"
        );
    }
    assert_eq!(redact_files(&w), before);
}

/// A node with `title` and `note`, and no evidence.
fn noted(w: &mut World, title: &str, note: &str) -> String {
    w.svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "problem".into(),
                title: title.into(),
                note: Some(note.into()),
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id
        .to_text()
}

fn line_of(w: &World, id: &str) -> String {
    String::from_utf8(nodes_file(w))
        .unwrap()
        .lines()
        .find(|l| l.contains(id))
        .unwrap()
        .to_string()
}

const PROFILE: &str = "https://www.example.com/in/jo-bloggs";

/// A section heading is structure, not a secret: a common heading word elsewhere in the graph does not block, and a
/// pointer a plain remove left in the trail is scrubbed there; nothing else changes.
#[test]
fn a_redact_matches_the_source_not_a_common_section_heading() {
    let mut w = world();
    let pointer = format!("{PROFILE}#section=Experience");
    let a = node(&mut w, &[pointer.as_str()]);
    let other = noted(&mut w, "Experience at Acme", "Experience matters");
    remove(&mut w, &a, &pointer, Some("stale"), None).unwrap();
    assert!(
        file_text(&w).contains("jo-bloggs"),
        "a plain remove keeps it in the trail"
    );
    let other_before = line_of(&w, &other);
    let edges_before = redact_files(&w)[1].clone();

    let done = redact(&mut w, &a, &pointer, "a personal profile").unwrap();
    assert_eq!((done.pointers, done.nodes, done.edges), (1, 1, 0));
    assert!(!file_text(&w).contains("jo-bloggs"), "{}", file_text(&w));
    assert_eq!(
        line_of(&w, &other),
        other_before,
        "an unrelated node keeps its bytes"
    );
    assert_eq!(redact_files(&w)[1], edges_before);
    let node: Value = serde_json::from_str(&line_of(&w, &a)).unwrap();
    let trail = node["evidence_removed"].as_array().unwrap();
    assert_eq!(trail.len(), 1, "{trail:?}");
    assert_eq!(trail[0]["redacted"], json!(true));
    assert_eq!(trail[0]["reason"], json!("a personal profile"));
    assert!(trail[0].get("removed").is_none(), "{trail:?}");
    assert_eq!(node["title"], json!("Approval delays"));
    assert_eq!(node["evidence"], json!([]));
}

/// The source itself elsewhere still blocks; the refusal names the record and field, and its next opens that record.
#[test]
fn a_redact_names_where_the_source_is_held_elsewhere() {
    let mut w = world();
    let pointer = format!("{PROFILE}#section=Experience");
    let a = node(&mut w, &[pointer.as_str()]);
    let held = noted(&mut w, "Contact", &format!("see {PROFILE} for more"));
    let before = redact_files(&w);
    let p = redact(&mut w, &a, &pointer, "a personal profile").unwrap_err();
    assert_eq!(blocked(&p), [(held.clone(), "note".to_string())]);
    assert_eq!(
        p.slot("fix").as_deref(),
        Some(format!("umx node get umx:demo/{held}").as_str())
    );
    assert!(!format!("{p:?}").contains("jo-bloggs"), "{p:?}");
    assert_eq!(redact_files(&w), before);
}

/// A heading that looks like a secret is still a needle (fail closed on what matters).
#[test]
fn a_redact_still_checks_a_secret_shaped_section_heading() {
    let mut w = world();
    // Built at run time so no token-shaped literal is committed.
    let secret = format!("{}_{}", "ghp", "Y8kP2mQ7vT4nR1sW6xZ3cB9dF5gH0jL2aE7u");
    let pointer = format!("docs/keys.md#section={secret}");
    let a = node(&mut w, &[pointer.as_str()]);
    let held = noted(&mut w, "Keys", &format!("rotated {secret}"));
    let p = redact(&mut w, &a, &pointer, "it named a key").unwrap_err();
    assert_eq!(blocked(&p), [(held, "note".to_string())]);
}
