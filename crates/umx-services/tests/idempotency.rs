//! Batch idempotency keys and receipts (ws003 s09, usage 205): a real keyed batch records its receipt in the graph
//! folder's `receipts.jsonl` in its own write set; the same key and input returns that receipt and writes nothing;
//! another input is refused; a dropped key runs as new and says so. Synthetic fixtures over the fakes.

use std::sync::Arc;
use umx_core::Id;
use umx_core::Problem;
use umx_core::codec::GraphFile;
use umx_core::ports::{Clock, CrashPoint, FileRef, GraphDir, IdGenerator, ModelKey};
use umx_core::receipts::{self, Receipt, Receipts};
use umx_core::value::SetValue;
use umx_fakes::{FakeClock, FakeEmbedder, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{
    BatchDone, BatchLine, BatchOp, BatchRequest, Ctx, Idempotency, NodeAddRequest, Ports, Services,
};

const DAY: u64 = 86_400_000;

struct World {
    fake: Arc<FakeGraphFiles>,
    clock: Arc<FakeClock>,
    ids: Arc<FakeIds>,
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
    let clock = Arc::new(FakeClock::default());
    let ids = Arc::new(FakeIds::new());
    let ports = Ports {
        sources: Arc::new(umx_fakes::FakeSourceReader::open(
            GraphDir::parse("/").unwrap(),
        )),
        files: fake.clone(),
        vector_cache: Arc::new(FakeGraphFiles::new(GraphDir::parse("/cache").unwrap())),
        vector_cache_root: GraphDir::parse("/cache").unwrap(),
        clock: clock.clone(),
        ids: ids.clone(),
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
    World {
        fake,
        clock,
        ids,
        svc,
    }
}

fn add(kind: &str, title: &str) -> NodeAddRequest {
    NodeAddRequest {
        kind: kind.into(),
        title: title.into(),
        ..Default::default()
    }
}

fn node(line: u32, r: Option<&str>, req: NodeAddRequest) -> BatchLine {
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

fn edge(line: u32, from_ref: &str, relation: &str, to_ref: &str) -> BatchLine {
    BatchLine {
        line,
        op: BatchOp::EdgeAdd {
            id: None,
            from: None,
            from_ref: Some(from_ref.into()),
            relation: relation.into(),
            to: None,
            to_ref: Some(to_ref.into()),
            sets: Vec::new(),
            evidence: Default::default(),
        },
    }
}

/// Two nodes with refs and an edge between them.
fn lines() -> Vec<BatchLine> {
    vec![
        node(1, Some("q"), add("problem", "Approval delays")),
        node(2, Some("o"), add("option", "Hire another approver")),
        node(3, None, add("option", "Automate the check")),
        edge(4, "o", "supports", "q"),
    ]
}

/// The digest the daemon would send for an input; tests name inputs by a word.
fn input(word: &str) -> String {
    receipts::input_digest(word.as_bytes())
}

fn keyed(lines: Vec<BatchLine>, key: &str, word: &str, dry_run: bool) -> BatchRequest {
    BatchRequest {
        lines,
        dry_run,
        idempotency: Some(Idempotency {
            key: key.into(),
            input: input(word),
        }),
        ..Default::default()
    }
}

fn run(w: &mut World, req: BatchRequest) -> Result<(BatchDone, Vec<Problem>), Vec<Problem>> {
    w.svc.batch(&ctx(), req).map(|d| (d.value, d.notes))
}

fn codes(ps: &[Problem]) -> Vec<&str> {
    ps.iter().map(|p| p.code).collect()
}

/// Every file of the folder this feature can touch, byte for byte.
fn folder(w: &World) -> [Option<Vec<u8>>; 4] {
    [
        w.fake.raw_for_tests(&dir(), GraphFile::Graph),
        w.fake.raw_for_tests(&dir(), GraphFile::Nodes),
        w.fake.raw_for_tests(&dir(), GraphFile::Edges),
        w.fake.raw_for_tests(&dir(), FileRef::Receipts),
    ]
}

fn receipts_file(w: &World) -> Option<Vec<u8>> {
    w.fake.raw_for_tests(&dir(), FileRef::Receipts)
}

/// The graph's id, from a dry run (which writes nothing).
fn graph_id(w: &mut World) -> Id {
    run(
        w,
        BatchRequest {
            lines: vec![node(1, None, add("problem", "probe"))],
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap()
    .0
    .graph_id
}

fn content_nodes(w: &World) -> usize {
    let g = w.svc.loaded_graph("demo").unwrap();
    g.nodes().filter(|n| !g.is_provenance(n.ix())).count()
}

#[test]
fn a_replay_returns_the_first_receipt_and_changes_nothing() {
    let mut w = world();
    let (first, notes) = run(&mut w, keyed(lines(), "window-08", "one", false)).unwrap();
    assert!(notes.iter().all(|p| p.code != "batch_replayed"));
    let k = first.keyed.clone().unwrap();
    assert_eq!((k.key.as_str(), k.replayed), ("window-08", false));
    assert_eq!(k.recorded_at, Some(w.clock.now_ms()));
    assert_eq!(first.refs.len(), 2);
    assert_eq!(first.counts["node add"].lines, 3);
    assert_eq!(first.counts["edge add"].lines, 1);
    let before = folder(&w);
    assert!(before[3].is_some(), "the receipt is written with the batch");
    let writes = w.fake.graph_writes();
    let nodes = content_nodes(&w);
    let next = w.ids.next_id();
    w.clock.advance(60_000);

    let (again, notes) = run(&mut w, keyed(lines(), "window-08", "one", false)).unwrap();
    assert_eq!(codes(&notes), ["batch_replayed"]);
    let k = again.keyed.clone().unwrap();
    assert!(k.replayed);
    // The receipt fields are the first run's.
    assert_eq!(again.graph_id, first.graph_id);
    assert_eq!(again.refs, first.refs);
    assert_eq!(again.counts, first.counts);
    assert_eq!(k.recorded_at, first.keyed.unwrap().recorded_at);
    assert!(!again.dry_run);
    assert!(again.rows.is_empty() && again.touched.is_empty());
    // Nothing changed: every file byte for byte, no write, no node, no id drawn.
    assert_eq!(folder(&w), before);
    assert_eq!(w.fake.graph_writes(), writes);
    assert_eq!(content_nodes(&w), nodes);
    assert_eq!(w.ids.next_id(), Id(next.0 + 1));
}

#[test]
fn the_receipt_file_holds_no_caller_text() {
    let mut w = world();
    run(&mut w, keyed(lines(), "client-name-key", "one", false)).unwrap();
    let text = String::from_utf8(receipts_file(&w).unwrap()).unwrap();
    for caller in [
        "client-name-key",
        "Approval",
        "approver",
        "\"q\"",
        "\"o\"",
        "supports",
    ] {
        assert!(!text.contains(caller), "{caller} in {text}");
    }
    assert!(text.contains(&receipts::key_digest("client-name-key")));
}

#[test]
fn the_same_key_with_another_input_is_refused_and_changes_nothing() {
    let mut w = world();
    run(&mut w, keyed(lines(), "k1", "one", false)).unwrap();
    let before = folder(&w);
    for dry_run in [false, true] {
        let e = run(&mut w, keyed(lines(), "k1", "two", dry_run)).unwrap_err();
        assert_eq!(codes(&e), ["idempotency_key_reused"], "dry run {dry_run}");
        assert_eq!(folder(&w), before);
    }
}

#[test]
fn a_key_is_checked_against_its_rule() {
    let mut w = world();
    for bad in ["", "a b", "a/b", &"k".repeat(129)] {
        let e = run(&mut w, keyed(lines(), bad, "one", false)).unwrap_err();
        assert_eq!(codes(&e), ["usage_invalid"], "{bad:?}");
    }
    assert_eq!(receipts_file(&w), None);
}

#[test]
fn a_dry_run_records_nothing_and_reports_a_replay() {
    let mut w = world();
    let (dry, _) = run(&mut w, keyed(lines(), "k1", "one", true)).unwrap();
    assert!(!dry.keyed.unwrap().replayed);
    assert_eq!(receipts_file(&w), None, "a dry run records nothing");
    let (first, _) = run(&mut w, keyed(lines(), "k1", "one", false)).unwrap();
    assert!(
        !first.keyed.unwrap().replayed,
        "the dry run left no receipt to replay"
    );
    let before = folder(&w);
    let (dry, notes) = run(&mut w, keyed(lines(), "k1", "one", true)).unwrap();
    assert!(dry.dry_run && dry.keyed.unwrap().replayed);
    assert_eq!(codes(&notes), ["batch_replayed"]);
    assert_eq!(dry.refs, first.refs);
    assert_eq!(folder(&w), before);
}

#[test]
fn a_failed_batch_records_nothing_so_a_retry_runs() {
    let mut w = world();
    let mut bad = lines();
    bad.push(edge(5, "o", "supports", "nobody"));
    assert!(run(&mut w, keyed(bad, "k1", "bad", false)).is_err());
    assert_eq!(receipts_file(&w), None);
    // A refused write records nothing either.
    w.fake.fail_writes(true);
    assert!(run(&mut w, keyed(lines(), "k1", "one", false)).is_err());
    w.fake.fail_writes(false);
    assert_eq!(receipts_file(&w), None);
    let (done, _) = run(&mut w, keyed(lines(), "k1", "one", false)).unwrap();
    assert!(!done.keyed.unwrap().replayed, "the retry ran for real");
    assert_eq!(content_nodes(&w), 3);
}

/// A receipts file whose oldest record, for `key`, is dropped by the bounds: KEEP_COUNT newer ones, all past
/// KEEP_AGE_MS at `now`.
fn file_with_dropped(graph: Id, key: &str, now: u64) -> Vec<u8> {
    let mut r = Receipts::new(graph);
    r.record(
        receipts::key_digest(key),
        0,
        input("one"),
        Receipt::default(),
    );
    for i in 0..receipts::KEEP_COUNT as u64 {
        r.record(
            receipts::key_digest(&format!("other{i}")),
            1 + i,
            input("x"),
            Receipt::default(),
        );
    }
    r.evict(now);
    assert!(matches!(
        r.find(&receipts::key_digest(key)),
        receipts::Lookup::Dropped { .. }
    ));
    r.encode()
}

#[test]
fn a_dropped_key_runs_as_new_and_the_reply_says_so() {
    let mut w = world();
    let now = 30 * DAY;
    w.clock.set(now);
    let g = graph_id(&mut w);
    w.fake
        .tamper_for_tests(&dir(), FileRef::Receipts, &file_with_dropped(g, "k1", now));
    let (dry, notes) = run(&mut w, keyed(lines(), "k1", "one", true)).unwrap();
    assert!(!dry.keyed.unwrap().replayed);
    assert_eq!(codes(&notes), ["idempotency_key_expired"]);
    let (done, notes) = run(&mut w, keyed(lines(), "k1", "one", false)).unwrap();
    assert!(
        !done.keyed.unwrap().replayed,
        "a dropped key replays nothing"
    );
    let expired: Vec<&Problem> = notes
        .iter()
        .filter(|p| p.code == "idempotency_key_expired")
        .collect();
    assert_eq!(expired.len(), 1);
    assert_eq!(content_nodes(&w), 3);
    // The new run recorded a live receipt again: a re-send now replays.
    let (again, notes) = run(&mut w, keyed(lines(), "k1", "one", false)).unwrap();
    assert!(again.keyed.unwrap().replayed);
    assert_eq!(codes(&notes), ["batch_replayed"]);
}

#[test]
fn a_receipt_naming_ids_the_graph_lost_is_not_replayed() {
    let mut w = world();
    run(&mut w, keyed(lines(), "k1", "one", false)).unwrap();
    let g = graph_id(&mut w);
    let mut forged = Receipts::new(g);
    forged.record(
        receipts::key_digest("k1"),
        1,
        input("one"),
        Receipt {
            refs: vec![
                "01JC8XA4H7B3KT9M2NZ5QW6RDE".parse().unwrap(),
                "01JC8XA4H7B3KT9M2NZ5QW6RDF".parse().unwrap(),
            ],
            counts: Default::default(),
        },
    );
    w.fake
        .tamper_for_tests(&dir(), FileRef::Receipts, &forged.encode());
    let before = folder(&w);
    let e = run(&mut w, keyed(lines(), "k1", "one", false)).unwrap_err();
    assert_eq!(codes(&e), ["idempotency_receipt_stale"]);
    assert_eq!(e[0].slot("missing").as_deref(), Some("2"));
    assert_eq!(folder(&w), before);
}

#[test]
fn an_unusable_receipts_file_refuses_keyed_batches_only() {
    let other = Receipts::new("01JC8XA4H7B3KT9M2NZ5QW6RDE".parse().unwrap()).encode();
    let cap = FileRef::Receipts.cap() as usize;
    for (what, bytes) in [
        ("not json", b"not a receipts file\n".to_vec()),
        (
            "newer",
            b"{\"umx_receipts\":2,\"graph_id\":\"x\"}\n".to_vec(),
        ),
        ("another graph", other),
        ("over the cap", vec![b' '; cap + 1]),
    ] {
        let mut w = world();
        w.fake.tamper_for_tests(&dir(), FileRef::Receipts, &bytes);
        let e = run(&mut w, keyed(lines(), "k1", "one", false)).unwrap_err();
        assert_eq!(codes(&e), ["receipts_unreadable"], "{what}");
        assert_eq!(receipts_file(&w).as_deref(), Some(&bytes[..]), "{what}");
        assert_eq!(content_nodes(&w), 0, "{what}");
        run(
            &mut w,
            BatchRequest {
                lines: lines(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            content_nodes(&w),
            3,
            "{what}: a batch without a key never reads it"
        );
        assert_eq!(receipts_file(&w).as_deref(), Some(&bytes[..]), "{what}");
    }
}

#[test]
fn a_receipts_file_changed_while_the_batch_ran_refuses_it() {
    let mut w = world();
    run(&mut w, keyed(lines(), "k0", "zero", false)).unwrap();
    let before = folder(&w);
    w.fake.change_after_next_marker_for_tests(
        &dir(),
        &dir(),
        FileRef::Receipts,
        b"pulled while the batch ran\n",
    );
    let e = run(&mut w, keyed(lines(), "k1", "one", false)).unwrap_err();
    assert_eq!(codes(&e), ["receipts_unreadable"]);
    assert!(
        e[0].slot("why").unwrap().contains("changed on disk"),
        "{:?}",
        e[0]
    );
    assert_eq!(
        folder(&w)[..3],
        before[..3],
        "the content files are as they were"
    );
    assert_eq!(content_nodes(&w), 3, "memory is as it was");
}

#[test]
fn a_keyed_batch_that_changed_nothing_records_its_receipt_alone() {
    let mut w = world();
    let keyed_add = || {
        let mut req = add("person", "Jo Smith");
        req.sets = vec![(
            "entity_key".into(),
            SetValue::Raw("email:jo@example.com".into()),
        )];
        req.on_key_resolve = true;
        vec![node(1, Some("jo"), req)]
    };
    let (first, _) = run(
        &mut w,
        BatchRequest {
            lines: keyed_add(),
            ..Default::default()
        },
    )
    .unwrap();
    let content = folder(&w);
    assert_eq!(content[3], None);
    let writes = w.fake.graph_writes();
    let (done, _) = run(&mut w, keyed(keyed_add(), "k1", "jo", false)).unwrap();
    assert!(done.rows[0].existed);
    assert_eq!(done.refs, first.refs, "the ref binds to the found node");
    let after = folder(&w);
    assert_eq!(after[..3], content[..3], "no content file was written");
    assert!(after[3].is_some(), "the receipt was");
    assert_eq!(
        w.fake.graph_writes(),
        writes + 1,
        "one set: receipts.jsonl alone"
    );
    let (again, _) = run(&mut w, keyed(keyed_add(), "k1", "jo", false)).unwrap();
    assert!(again.keyed.unwrap().replayed);
    assert_eq!(again.counts["node add"].existed, 1);
    assert_eq!(again.refs, first.refs);
}

#[test]
fn a_crash_leaves_the_batch_and_its_receipt_both_or_neither() {
    for point in [
        CrashPoint::AfterTemps,
        CrashPoint::AfterMarker,
        CrashPoint::AfterFirstRename,
    ] {
        let mut w = world();
        w.fake.crash_at_for_tests(Some(point));
        assert!(run(&mut w, keyed(lines(), "k1", "one", false)).is_err());
        w.fake.crash_at_for_tests(None);
        let mut after = world_on(Arc::new(w.fake.restart_for_tests()), false);
        let landed = content_nodes(&after) == 3;
        let receipt = receipts_file(&after).is_some();
        assert_eq!(landed, receipt, "{point:?}: both or neither");
        assert_eq!(landed, point != CrashPoint::AfterTemps, "{point:?}");
        let (done, _) = run(&mut after, keyed(lines(), "k1", "one", false)).unwrap();
        assert_eq!(done.keyed.unwrap().replayed, landed, "{point:?}");
        assert_eq!(content_nodes(&after), 3, "{point:?}: never twice");
    }
}

#[test]
fn a_replay_still_needs_the_graph_as_a_write_does() {
    let mut w = world();
    run(&mut w, keyed(lines(), "k1", "one", false)).unwrap();
    let nodes = w.fake.raw_for_tests(&dir(), GraphFile::Nodes).unwrap();
    w.fake
        .tamper_for_tests(&dir(), GraphFile::Nodes, &[&nodes[..], b"\n"].concat());
    let e = run(&mut w, keyed(lines(), "k1", "one", false)).unwrap_err();
    assert_eq!(codes(&e), ["graph_files_changed"]);
}

#[test]
fn keys_are_per_graph_and_follow_a_rename() {
    let mut w = world();
    let (first, _) = run(&mut w, keyed(lines(), "k1", "one", false)).unwrap();
    w.svc
        .graph_new(&ctx(), "/work/h", "other", None, false, None)
        .unwrap();
    let mut on_other = keyed(lines(), "k1", "two", false);
    on_other.lines.truncate(2);
    let other_ctx = Ctx {
        graph: Some("other".into()),
        ..ctx()
    };
    let done = w.svc.batch(&other_ctx, on_other).unwrap().value;
    assert!(
        !done.keyed.unwrap().replayed,
        "another graph's key is another key"
    );
    w.svc.graph_rename(&ctx(), "demo", "demo2").unwrap();
    let renamed = Ctx {
        graph: Some("demo2".into()),
        ..ctx()
    };
    let again = w
        .svc
        .batch(&renamed, keyed(lines(), "k1", "one", false))
        .unwrap()
        .value;
    let k = again.keyed.unwrap();
    assert!(k.replayed, "the receipt moved with the graph");
    assert_eq!(again.alias, "demo2");
    assert_eq!(again.refs, first.refs);
}

/// Review B1: a receipts file at the read cap decodes in about linear time. The smallest records (dropped keys)
/// fill 16 MiB with over 160,000 lines; checking each key against every earlier record would take billions of
/// comparisons (tens of seconds even optimised), where a key set takes well under a second. The bound is generous
/// for a loaded machine and far below the quadratic cost.
#[test]
fn a_receipts_file_at_the_read_cap_decodes_in_about_linear_time() {
    let graph: Id = "01JC8XA4H7B3KT9M2NZ5QW6RDE".parse().unwrap();
    let cap = FileRef::Receipts.cap() as usize;
    let mut file = Receipts::new(graph).encode();
    let mut n = 0u64;
    loop {
        let line = format!("{{\"key\":\"{n:064x}\",\"at\":{n},\"dropped\":{n}}}\n");
        if file.len() + line.len() > cap {
            break;
        }
        file.extend_from_slice(line.as_bytes());
        n += 1;
    }
    assert!(n > 160_000, "{n} records");
    let started = std::time::Instant::now();
    let read = Receipts::decode(&file, graph).unwrap();
    let took = started.elapsed();
    assert_eq!(read.records().len() as u64, n);
    let bound = if cfg!(debug_assertions) { 10 } else { 2 };
    assert!(
        took < std::time::Duration::from_secs(bound),
        "decoding {n} records took {took:?}"
    );
    // A repeated key is still refused, wherever it is.
    let mut repeated = file.clone();
    repeated.extend_from_slice(
        format!("{{\"key\":\"{:064x}\",\"at\":1,\"dropped\":1}}\n", 7).as_bytes(),
    );
    assert!(Receipts::decode(&repeated, graph).is_err());
}
