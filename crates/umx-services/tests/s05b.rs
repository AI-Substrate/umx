//! Services over the fakes: the s05b verbs (graph unload, idle unload, node edit | note | status | history).

use serde_json::json;
use std::sync::Arc;
use umx_core::codec::{self, GraphFile};
use umx_core::ports::{GraphDir, GraphFiles};
use umx_core::value::SetValue;
use umx_core::{Id, Node, Problem};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{Ctx, NodeAddRequest, NodeEditRequest, Ports, Services};

const MINUTE: u64 = 60_000;

struct World {
    files: Arc<FakeGraphFiles>,
    clock: Arc<FakeClock>,
    svc: Services,
}

fn world() -> World {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let clock = Arc::new(FakeClock::default());
    let ports = Ports {
        sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
            umx_core::ports::GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: std::sync::Arc::new(umx_fakes::FakeGraphFiles::new(
            umx_core::ports::GraphDir::parse("/cache").unwrap(),
        )),
        vector_cache_root: umx_core::ports::GraphDir::parse("/cache").unwrap(),
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

fn add(w: &mut World, title: &str) -> Id {
    w.svc
        .node_add(
            &ctx(),
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
}

fn codes(w: &[Problem]) -> Vec<&str> {
    w.iter().map(|p| p.code).collect()
}

fn on_disk(w: &World, id: Id) -> Node {
    let bytes = w
        .files
        .read(&GraphDir::parse("/work/g").unwrap(), GraphFile::Nodes)
        .unwrap()
        .unwrap();
    codec::decode_nodes(&bytes)
        .unwrap()
        .records
        .into_iter()
        .find(|n| n.id == id)
        .unwrap()
}

fn text(id: Id) -> String {
    id.to_text()
}

// ---------------------------------------------------------------- graph unload, idle unload (req-03)

#[test]
fn unload_drops_the_graph_and_an_unknown_alias_is_graph_not_loaded() {
    let mut w = world();
    let done = w.svc.graph_unload(&ctx(), "demo").unwrap();
    assert_eq!(done.value.alias, "demo");
    assert!(w.svc.graph_list().is_empty());
    let err = w.svc.graph_unload(&ctx(), "demo").unwrap_err();
    assert_eq!(err.code, "graph_not_loaded");
    // The files stay: the graph loads again as it was.
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    assert_eq!(w.svc.graph_list().len(), 1);
}

#[test]
fn an_idle_graph_unloads_one_idle_time_after_its_last_use() {
    let mut w = world();
    assert_eq!(
        w.svc.next_idle_unload_ms(),
        None,
        "idle unload is off by default"
    );
    w.svc.set_idle_unload_ms(MINUTE);
    let start = w.svc.loaded("demo").unwrap().last_used_ms();
    assert_eq!(w.svc.next_idle_unload_ms(), Some(start + MINUTE));
    assert_eq!(w.svc.graph_list()[0].unloads_in_s, Some(60));

    w.clock.advance(MINUTE - 1);
    assert!(w.svc.unload_idle().is_empty());
    let id = add(&mut w, "Approval delays"); // a request resets the deadline
    assert_eq!(w.svc.next_idle_unload_ms(), Some(start + 2 * MINUTE - 1));
    w.clock.advance(MINUTE - 1);
    assert!(w.svc.unload_idle().is_empty());
    assert_eq!(w.svc.graph_list()[0].unloads_in_s, Some(1));

    w.clock.advance(1);
    assert_eq!(w.svc.unload_idle(), vec!["demo".to_string()]);
    assert!(w.svc.graph_list().is_empty());
    assert_eq!(w.svc.next_idle_unload_ms(), None);
    assert_eq!(on_disk(&w, id).title, "Approval delays");
}

#[test]
fn listing_graphs_does_not_count_as_use() {
    let mut w = world();
    w.svc.set_idle_unload_ms(MINUTE);
    w.clock.advance(MINUTE);
    w.svc.graph_list();
    assert_eq!(w.svc.unload_idle(), vec!["demo".to_string()]);
}

#[test]
fn idle_unload_set_to_zero_never_unloads() {
    let mut w = world();
    w.svc.set_idle_unload_ms(0);
    w.clock.advance(1000 * MINUTE);
    assert!(w.svc.unload_idle().is_empty());
    assert_eq!(w.svc.graph_list()[0].unloads_in_s, None);
}

#[test]
fn a_huge_idle_time_with_a_nonzero_clock_never_overflows() {
    // Review B1: used + idle overflowed and panicked the daemon's writer.
    let mut w = world();
    w.clock.set(1_800_000_000_000);
    w.svc.set_idle_unload_ms(u64::MAX);
    w.svc.graph_unload(&ctx(), "demo").unwrap();
    w.svc
        .graph_new(&ctx(), "/work/h", "other", None, false, None)
        .unwrap();
    add(&mut w, "Approval delays");
    let listed = w.svc.graph_list();
    assert!(listed.iter().all(|g| g.unloads_in_s.is_some()));
    assert_eq!(w.svc.idle_wait_ms(), Some(u64::MAX - 1_800_000_000_000));
    assert!(w.svc.unload_idle().is_empty());
}

// ---------------------------------------------------------------- node status, history (req-12)

#[test]
fn a_status_move_without_a_reason_warns_and_both_moves_show_in_history() {
    let mut w = world();
    let id = add(&mut w, "Approval delays");
    let first = w
        .svc
        .node_status(&ctx(), &text(id), "agreed", None, None)
        .unwrap();
    assert_eq!(codes(&first.notes), ["reason_missing"]);
    assert_eq!(
        first.notes[0].slot("fix"),
        Some(format!(
            "umx node status umx:demo/{} agreed --reason <why>",
            text(id)
        ))
    );
    assert_eq!(first.notes[0].subject, Some(text(id)));
    assert_eq!(
        (
            first.value.change.from.as_str(),
            first.value.change.to.as_str()
        ),
        ("open", "agreed")
    );
    assert_eq!(first.value.change.label.as_deref(), Some("resolved"));

    w.clock.advance(1000);
    let second = w
        .svc
        .node_status(&ctx(), &text(id), "contested", Some("new data"), None)
        .unwrap();
    assert!(second.notes.is_empty());
    assert_eq!(second.value.change.label.as_deref(), Some("in_contention"));

    let n = on_disk(&w, id);
    assert_eq!(n.status, "contested");
    assert_eq!(n.status_history.len(), 2);
    assert_eq!(n.status_history[1]["reason"], json!("new data"));
    assert_eq!(n.updated, second.value.change.at);

    let rows = w
        .svc
        .node_history(&ctx(), &text(id), 100)
        .unwrap()
        .value
        .rows;
    let kinds: Vec<_> = rows
        .iter()
        .map(|r| (r.kind, r.from.as_deref(), r.to.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("created", None, "open"),
            ("status", Some("open"), "agreed"),
            ("status", Some("agreed"), "contested")
        ]
    );
    assert_eq!(rows[1].reason, None);
    assert_eq!(rows[2].reason.as_deref(), Some("new data"));
    assert!(rows.iter().all(|r| r.by == "tester" && !r.at.is_empty()));
    assert!(rows[1].at < rows[2].at);

    let limited = w.svc.node_history(&ctx(), &text(id), 2).unwrap().value;
    assert_eq!(
        (limited.rows.len(), limited.total, limited.truncated),
        (2, Some(3), true)
    );
    let newest: Vec<_> = limited.rows.iter().map(|r| r.to.as_str()).collect();
    assert_eq!(
        newest,
        ["agreed", "contested"],
        "the newest rows, oldest first"
    );
}

#[test]
fn a_given_label_wins_and_an_unknown_label_or_bad_status_is_refused() {
    let mut w = world();
    let id = add(&mut w, "Approval delays");
    let moved = w
        .svc
        .node_status(
            &ctx(),
            &text(id),
            "leaning",
            Some("why"),
            Some("back_on_table"),
        )
        .unwrap();
    assert_eq!(moved.value.change.label.as_deref(), Some("back_on_table"));

    let err = w
        .svc
        .node_status(&ctx(), &text(id), "agreed", Some("why"), Some("done"))
        .unwrap_err();
    assert_eq!(err.code, "usage_invalid");
    let err = w
        .svc
        .node_status(&ctx(), &text(id), "Agreed", Some("why"), None)
        .unwrap_err();
    assert_eq!(err.code, "name_invalid");
    assert_eq!(
        on_disk(&w, id).status_history.len(),
        1,
        "refusals change nothing"
    );

    let off = w
        .svc
        .node_status(&ctx(), &text(id), "parked", Some("why"), None)
        .unwrap();
    assert_eq!(codes(&off.notes), ["off_vocabulary_status"]);
    assert_eq!(off.value.change.label, None);
}

// ---------------------------------------------------------------- node note (req-58)

#[test]
fn note_appends_a_paragraph_and_evidence_in_one_write() {
    let mut w = world();
    let id = add(&mut w, "Approval delays");
    let done = w
        .svc
        .node_note(
            &ctx(),
            &text(id),
            "Seen in the 9 Nov call.",
            &["transcripts/012.ndjson#seq=120-140".into()],
            Some("room"),
        )
        .unwrap();
    assert_eq!(done.value.evidence_added, 1);
    w.svc
        .node_note(
            &ctx(),
            &text(id),
            "Confirmed by finance.",
            &[
                "transcripts/012.ndjson#seq=120-140".into(),
                "notes/fin.md#L3-L9".into(),
            ],
            None,
        )
        .unwrap();
    let n = on_disk(&w, id);
    assert_eq!(n.note, "Seen in the 9 Nov call.\n\nConfirmed by finance.");
    assert_eq!(
        n.evidence.len(),
        3,
        "same pointer with another kind is a new entry"
    );
    assert_eq!(
        n.evidence[0],
        json!({ "source": "transcripts/012.ndjson", "selector": { "seq": [120, 140] },
                "kind": "room", "fidelity": "lossy", "hash": EMPTY_SHA256, "hash_state": "hashed", "length": 0,
                "by_session": n.edited_by_session.unwrap().to_text() })
    );
    assert_eq!(n.evidence[2]["selector"], json!({ "lines": [3, 9] }));
    assert_eq!(n.evidence[2]["kind"], json!("document"));

    let again = w
        .svc
        .node_note(
            &ctx(),
            &text(id),
            "Again.",
            &["notes/fin.md#L3-L9".into()],
            None,
        )
        .unwrap();
    assert_eq!(
        again.value.evidence_added, 0,
        "an identical pointer is not added twice"
    );
}

#[test]
fn a_bad_pointer_or_kind_refuses_the_whole_note() {
    let mut w = world();
    let id = add(&mut w, "Approval delays");
    let before = on_disk(&w, id);
    let note = |w: &mut World, ev: &str, kind: Option<&str>| {
        w.svc
            .node_note(
                &ctx(),
                &text(id),
                "text",
                &["a.md#L1".into(), ev.into()],
                kind,
            )
            .unwrap_err()
            .code
    };
    assert_eq!(note(&mut w, "a.md", None), "evidence_pointer_invalid");
    assert_eq!(
        note(&mut w, "../outside.md#L1", None),
        "evidence_outside_roots"
    );
    assert_eq!(
        note(&mut w, "file:///work/g/a.md#L1", None),
        "evidence_outside_roots"
    );
    assert_eq!(
        note(&mut w, "/etc/passwd#L1", None),
        "evidence_outside_roots"
    );
    assert_eq!(note(&mut w, "a.md#L2", Some("rumour")), "usage_invalid");
    let empty = w
        .svc
        .node_note(&ctx(), &text(id), "  ", &[], None)
        .unwrap_err();
    assert_eq!(empty.code, "usage_invalid");
    assert_eq!(on_disk(&w, id), before);
}

#[test]
fn a_failed_write_leaves_the_node_as_it_was() {
    let mut w = world();
    let id = add(&mut w, "Approval delays");
    w.files.fail_writes(true);
    let err = w
        .svc
        .node_status(&ctx(), &text(id), "agreed", Some("why"), None)
        .unwrap_err();
    assert_eq!(err.code, "write_failed");
    w.files.fail_writes(false);
    let n = w.svc.node_get(&ctx(), &text(id)).unwrap().value.1;
    assert_eq!((n.status.as_str(), n.status_history.len()), ("open", 0));
}

// ---------------------------------------------------------------- node add --evidence (workshop 007 node add row)

fn add_with(w: &mut World, evidence: &[&str], kind: Option<&str>) -> Result<Node, Problem> {
    w.svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "problem".into(),
                title: "Approval delays".into(),
                evidence: evidence.iter().map(|e| (*e).into()).collect(),
                evidence_kind: kind.map(str::to_string),
                ..Default::default()
            },
        )
        .map(|d| d.value.node)
}

/// The open fake reads a file it was not told about as empty (usage 220).
const EMPTY_SHA256: &str =
    "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[test]
fn add_stores_evidence_in_its_one_write() {
    let mut w = world();
    let p = "transcripts/012.ndjson#seq=120-140";
    let n = add_with(&mut w, &[p, p, "notes/fin.md#L3-L9"], Some("room")).unwrap();
    let on = on_disk(&w, n.id);
    assert_eq!(on, n);
    assert_eq!(on.evidence.len(), 2, "a repeated pointer is stored once");
    assert_eq!(
        on.evidence[0],
        json!({ "source": "transcripts/012.ndjson", "selector": { "seq": [120, 140] },
                "kind": "room", "fidelity": "lossy", "hash": EMPTY_SHA256, "hash_state": "hashed", "length": 0,
                "by_session": on.authored_by_session.unwrap().to_text() })
    );
    let plain = add_with(&mut w, &["notes/fin.md#L3"], None).unwrap();
    assert_eq!(on_disk(&w, plain.id).evidence[0]["kind"], json!("document"));
}

#[test]
fn add_with_a_bad_pointer_or_kind_adds_no_node() {
    let mut w = world();
    let refused = |w: &mut World, ev: &str, kind: Option<&str>| {
        add_with(w, &["a.md#L1", ev], kind).unwrap_err().code
    };
    assert_eq!(refused(&mut w, "a.md", None), "evidence_pointer_invalid");
    assert_eq!(
        refused(&mut w, "../outside.md#L1", None),
        "evidence_outside_roots"
    );
    assert_eq!(
        refused(&mut w, "/etc/passwd#L1", None),
        "evidence_outside_roots"
    );
    assert_eq!(refused(&mut w, "a.md#L2", Some("rumour")), "usage_invalid");
    assert!(
        w.files
            .read(&GraphDir::parse("/work/g").unwrap(), GraphFile::Nodes)
            .unwrap()
            .is_none_or(|b| codec::decode_nodes(&b).unwrap().records.is_empty()),
        "no node was written"
    );
    assert_eq!(
        w.svc
            .node_find(&ctx(), "Approval", None, None, 10)
            .unwrap()
            .value
            .total,
        Some(0)
    );
}

// ---------------------------------------------------------------- node edit

#[test]
fn edit_changes_title_type_note_and_fields_and_reports_what_changed() {
    let mut w = world();
    let id = add(&mut w, "Approval delays");
    w.clock.advance(1000);
    let done = w
        .svc
        .node_edit(
            &ctx(),
            &text(id),
            NodeEditRequest {
                tag: Default::default(),
                untag: Default::default(),
                title: Some("Approval delays in Q4".into()),
                kind: Some("question".into()),
                note: Some("Rewritten.".into()),
                sets: vec![("likelihood".into(), SetValue::Raw("3".into()))],
                unsets: vec![],
                locator: None,
            },
        )
        .unwrap();
    assert_eq!(done.value.changed, ["type", "title", "note", "likelihood"]);
    assert_eq!(codes(&done.notes), ["off_vocabulary_field"]);
    let n = on_disk(&w, id);
    assert_eq!(
        (n.kind.as_str(), n.title.as_str()),
        ("question", "Approval delays in Q4")
    );
    assert_eq!(n.fields["likelihood"], json!(3));
    assert_ne!(n.updated, n.created);

    let unset = w
        .svc
        .node_edit(
            &ctx(),
            &text(id),
            NodeEditRequest {
                unsets: vec!["likelihood".into(), "absent".into()],
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(unset.value.changed, ["likelihood"]);
    assert!(!on_disk(&w, id).fields.contains_key("likelihood"));
}

#[test]
fn an_edit_that_changes_nothing_writes_nothing() {
    let mut w = world();
    let id = add(&mut w, "Approval delays");
    let before = on_disk(&w, id);
    w.clock.advance(1000);
    w.files.fail_writes(true); // any write would fail
    let done = w
        .svc
        .node_edit(
            &ctx(),
            &text(id),
            NodeEditRequest {
                title: Some("Approval delays".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(done.value.changed.is_empty());
    w.files.fail_writes(false);
    assert_eq!(on_disk(&w, id), before);
}

#[test]
fn edit_refuses_status_reserved_and_invalid_names() {
    let mut w = world();
    let id = add(&mut w, "Approval delays");
    let edit =
        |w: &mut World, req: NodeEditRequest| w.svc.node_edit(&ctx(), &text(id), req).unwrap_err();
    let status = edit(
        &mut w,
        NodeEditRequest {
            sets: vec![("status".into(), SetValue::Raw("agreed".into()))],
            ..Default::default()
        },
    );
    assert_eq!(status.code, "use_status_verb");
    assert_eq!(
        status.slot("fix"),
        Some(format!(
            "umx node status umx:demo/{} agreed --reason <why>",
            text(id)
        ))
    );
    let unset_status = edit(
        &mut w,
        NodeEditRequest {
            unsets: vec!["status".into()],
            ..Default::default()
        },
    );
    assert_eq!(unset_status.code, "use_status_verb");
    let reserved = edit(
        &mut w,
        NodeEditRequest {
            sets: vec![("created_by".into(), SetValue::Raw("x".into()))],
            ..Default::default()
        },
    );
    assert_eq!(reserved.code, "reserved_name");
    let invalid = edit(
        &mut w,
        NodeEditRequest {
            kind: Some("Risk".into()),
            ..Default::default()
        },
    );
    assert_eq!(invalid.code, "name_invalid");
    assert_eq!(
        invalid.slot("fix"),
        Some(format!("umx node edit umx:demo/{} --type risk", text(id)))
    );
}
