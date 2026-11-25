//! Usage 220: a cited local file is hashed when the reference is added; `check --sources` says which changed; refresh
//! accepts the file as it is now. Over the fake source reader in its closed mode, so a file nobody wrote is missing.

use serde_json::Value;
use std::sync::{Arc, Mutex};
use umx_core::evidence::EvidencePointer;
use umx_core::ports::GraphDir;
use umx_core::source_hash::{Transcript, digest};
use umx_core::{Id, Problem};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState, FakeSourceReader};
use umx_services::{
    BatchLine, BatchOp, BatchRequest, Ctx, NodeAddRequest, Ports, RefreshPick, Services,
    SourceFinding, SourceRoots,
};

const TRANSCRIPT: &[u8] = b"{\"seq\":1,\"t\":\"hi\"}\n{\"seq\":42,\"t\":\"bye\"}\n";

struct World {
    sources: Arc<FakeSourceReader>,
    roots: Arc<Roots>,
    svc: Services,
}

/// The machine's `root:` folders, which a test can take away (a teammate's machine without the root).
#[derive(Default)]
struct Roots(Mutex<Option<GraphDir>>);

impl SourceRoots for Roots {
    fn root_folder(&self, name: &str) -> Option<GraphDir> {
        (name == "acme")
            .then(|| self.0.lock().unwrap().clone())
            .flatten()
    }
    fn repo_top(&self, _: &GraphDir) -> Option<GraphDir> {
        None
    }
}

fn path(p: &str) -> GraphDir {
    GraphDir::parse(&format!("/work/g/{p}")).unwrap()
}

fn world() -> World {
    let sources = Arc::new(FakeSourceReader::new(GraphDir::parse("/work").unwrap()));
    sources.add_folder(GraphDir::parse("/work/g").unwrap());
    sources.write_file(path("calls/3-nov.ndjson"), TRANSCRIPT);
    sources.write_file(path("docs/brief.md"), b"# Brief\n");
    sources.write_file(GraphDir::parse("/work/acme/a.md").unwrap(), b"acme\n");
    let roots = Arc::new(Roots::default());
    *roots.0.lock().unwrap() = Some(GraphDir::parse("/work/acme").unwrap());
    let mut svc = Services::new(Ports {
        files: Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap())),
        vector_cache: Arc::new(FakeGraphFiles::new(GraphDir::parse("/cache").unwrap())),
        vector_cache_root: GraphDir::parse("/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new().with_absent_ok(true)),
        sources: sources.clone(),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    })
    .with_source_roots(roots.clone());
    svc.graph_new(&ctx(), "/work/g", "demo", None, false, None)
        .unwrap();
    World {
        sources,
        roots,
        svc,
    }
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

/// The source codes among `w` (a document type is off this graph's vocabulary, which is not the point here).
fn codes(w: &[Problem]) -> Vec<&str> {
    w.iter()
        .map(|p| p.code)
        .filter(|c| c.starts_with("source_"))
        .collect()
}

fn add(w: &mut World, evidence: &[&str], locator: Option<&str>) -> (Id, Vec<Problem>) {
    let done = w
        .svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "document".into(),
                title: "Brief".into(),
                evidence: evidence.iter().map(|p| EvidencePointer::from(*p)).collect(),
                locator: locator.map(Into::into),
                ..Default::default()
            },
        )
        .unwrap();
    (done.value.node.id, done.notes)
}

fn evidence(w: &World, id: Id) -> Vec<Value> {
    let (_, n) = w.svc.node_get(&ctx(), &id.to_text()).unwrap().value;
    n.evidence
}

fn sources_check(w: &World) -> (umx_services::SourcesChecked, Vec<Problem>) {
    let done = w.svc.check(&ctx().graph("demo"), None, true).unwrap();
    (done.value.sources.unwrap(), done.notes)
}

#[test]
fn a_cited_file_and_transcript_are_hashed_when_added() {
    let mut w = world();
    let (id, warnings) = add(
        &mut w,
        &["calls/3-nov.ndjson#seq=1-2", "docs/brief.md#L1"],
        Some("calls/3-nov.ndjson"),
    );
    assert!(codes(&warnings).is_empty(), "{:?}", codes(&warnings));
    let want = digest(TRANSCRIPT, Some(Transcript::Ndjson));
    let ev = evidence(&w, id);
    assert_eq!(ev[0]["hash"], Value::String(want.hash.clone()));
    assert_eq!(ev[0]["hash_state"], "hashed");
    assert_eq!(ev[0]["length"], TRANSCRIPT.len());
    assert_eq!(ev[0]["seq_max"], 42);
    assert_eq!(
        ev[1]["hash"],
        Value::String(digest(b"# Brief\n", None).hash)
    );
    assert!(ev[1].get("seq_max").is_none());
    let shown = w.svc.resource_show(&ctx(), &id.to_text()).unwrap().value;
    let l = shown.node.locator.unwrap();
    assert_eq!(
        (l.hash, l.length, l.seq_max, l.hash_state.as_deref()),
        (Some(want.hash), Some(want.length), Some(42), Some("hashed"))
    );
    assert_eq!(
        w.sources.digests(),
        2,
        "the transcript once for both its citations"
    );
}

#[test]
fn a_remote_source_is_never_read() {
    let mut w = world();
    let (id, warnings) = add(
        &mut w,
        &[
            "https://example.com/memo#L1",
            "umx:demo/01JC8XA4H7Q1V9K2M3N4P5R6S7#L1",
        ],
        Some("https://example.com/pricing"),
    );
    assert!(!codes(&warnings).contains(&"source_not_hashed"));
    for p in evidence(&w, id) {
        assert_eq!(
            (&p["hash"], &p["hash_state"]),
            (&Value::Null, &Value::from("remote"))
        );
    }
    let l = w
        .svc
        .resource_show(&ctx(), &id.to_text())
        .unwrap()
        .value
        .node
        .locator
        .unwrap();
    assert_eq!((l.hash, l.hash_state.as_deref()), (None, Some("remote")));
    assert_eq!(w.sources.digests(), 0);
}

#[test]
fn a_missing_file_warns_and_the_write_goes_ahead() {
    let mut w = world();
    let (id, warnings) = add(&mut w, &["calls/none.vtt#seq=1-2"], Some("docs/gone.md"));
    assert_eq!(codes(&warnings), ["source_not_hashed", "source_not_hashed"]);
    let warning = warnings
        .iter()
        .find(|p| p.code == "source_not_hashed")
        .unwrap();
    let fix = warning
        .slots
        .iter()
        .find(|(k, _)| *k == "fix")
        .map(|(_, v)| format!("{v:?}"));
    assert!(fix.unwrap().contains("--sources"));
    let ev = evidence(&w, id);
    assert_eq!(
        (&ev[0]["hash"], &ev[0]["hash_state"]),
        (&Value::Null, &Value::from("missing"))
    );
}

#[test]
fn the_same_pointer_added_again_keeps_its_stored_hash() {
    let mut w = world();
    let (id, _) = add(&mut w, &["docs/brief.md#L1"], None);
    let before = evidence(&w, id);
    w.sources
        .write_file(path("docs/brief.md"), b"# Brief, edited\n");
    let again = w
        .svc
        .node_evidence_add(&ctx(), &id.to_text(), "docs/brief.md#L1", None)
        .unwrap();
    assert!(!again.value.added);
    assert_eq!(evidence(&w, id), before);
}

#[test]
fn check_reports_changed_and_missing_sources_and_refresh_accepts_them() {
    let mut w = world();
    let (id, _) = add(
        &mut w,
        &["docs/brief.md#L1", "calls/3-nov.ndjson#seq=1-2"],
        Some("docs/brief.md"),
    );
    let (clean, hints) = sources_check(&w);
    assert!(clean.rows.is_empty() && hints.is_empty());
    assert_eq!((clean.checked, clean.unchecked), (3, 0));

    w.sources
        .write_file(path("docs/brief.md"), b"# Brief, edited\n");
    w.sources.remove_file(&path("calls/3-nov.ndjson"));
    let (found, hints) = sources_check(&w);
    assert_eq!(codes(&hints), ["source_changed", "source_missing"]);
    let rows: Vec<_> = found
        .rows
        .iter()
        .map(|r| (r.kind, r.id, r.place.as_str(), r.old_length, r.new_length))
        .collect();
    assert_eq!(
        rows,
        [
            (SourceFinding::Changed, id, "locator", Some(8), Some(16)),
            (SourceFinding::Changed, id, "evidence:1", Some(8), Some(16)),
            (
                SourceFinding::Missing,
                id,
                "evidence:2",
                Some(TRANSCRIPT.len() as u64),
                None
            ),
        ]
    );
    assert_eq!((found.changed, found.missing), (2, 1));

    let refreshed = w
        .svc
        .node_evidence_refresh(&ctx(), &id.to_text(), Some(RefreshPick::At(1)))
        .unwrap()
        .value;
    assert_eq!(refreshed.rows.len(), 1);
    assert!(refreshed.rows[0].changed);
    let r = w.svc.resource_refresh(&ctx(), &id.to_text()).unwrap().value;
    assert!(r.changed);
    assert_eq!(r.node.locator.unwrap().length, Some(16));
    let again = w.svc.resource_refresh(&ctx(), &id.to_text()).unwrap().value;
    assert!(!again.changed, "nothing to accept twice");

    let (left, _) = sources_check(&w);
    let kinds: Vec<_> = left
        .rows
        .iter()
        .map(|r| (r.kind, r.place.as_str()))
        .collect();
    assert_eq!(kinds, [(SourceFinding::Missing, "evidence:2")]);
}

#[test]
fn a_batch_reads_each_cited_file_once() {
    let mut w = world();
    let line = |n: u32| BatchLine {
        line: n,
        op: BatchOp::NodeAdd {
            id: None,
            created: None,
            ref_name: None,
            req: NodeAddRequest {
                kind: "problem".into(),
                title: format!("Point {n}"),
                evidence: vec![EvidencePointer::from(
                    format!("calls/3-nov.ndjson#seq={n}-{n}").as_str(),
                )],
                ..Default::default()
            },
            parent_ref: None,
        },
    };
    w.svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                lines: (1..=5).map(line).collect(),
                dry_run: false,
                refs: Default::default(),
                refs_graph: None,
                reason: None,
            },
        )
        .unwrap();
    assert_eq!(w.sources.digests(), 1);
}

#[test]
fn a_refresh_whose_root_is_not_usable_here_keeps_the_stored_hash() {
    let mut w = world();
    let (id, _) = add(&mut w, &["root:acme/a.md#L1"], Some("root:acme/a.md"));
    let before = evidence(&w, id);
    assert_eq!(before[0]["hash_state"], "hashed");
    let locator = w
        .svc
        .resource_show(&ctx(), &id.to_text())
        .unwrap()
        .value
        .node
        .locator;
    *w.roots.0.lock().unwrap() = None;

    let done = w
        .svc
        .node_evidence_refresh(&ctx(), &id.to_text(), Some(RefreshPick::All))
        .unwrap();
    assert_eq!(codes(&done.notes), ["source_not_refreshed"]);
    assert!(!done.value.rows[0].read && !done.value.rows[0].changed);
    assert!(done.value.hashed.is_empty());
    assert_eq!(evidence(&w, id), before, "the good hash is kept");

    let done = w.svc.resource_refresh(&ctx(), &id.to_text()).unwrap();
    assert_eq!(codes(&done.notes), ["source_not_refreshed"]);
    assert!(!done.value.changed);
    assert_eq!(done.value.node.locator, locator);
}

#[test]
fn a_bare_refresh_is_refused_and_all_lists_what_it_read() {
    let mut w = world();
    let (id, _) = add(
        &mut w,
        &[
            "docs/brief.md#L1",
            "docs/brief.md#L2",
            "https://example.com/a#L1",
        ],
        Some("docs/brief.md"),
    );
    let refused = w.svc.node_evidence_refresh(&ctx(), &id.to_text(), None);
    assert_eq!(refused.unwrap_err().code, "usage_invalid");
    let all = w
        .svc
        .node_evidence_refresh(&ctx(), &id.to_text(), Some(RefreshPick::All))
        .unwrap()
        .value;
    assert_eq!(all.rows.len(), 3);
    assert_eq!(
        all.hashed,
        ["docs/brief.md"],
        "each local source once; a URL is never read"
    );
    let r = w.svc.resource_refresh(&ctx(), &id.to_text()).unwrap().value;
    assert_eq!(r.hashed, ["docs/brief.md"]);
}

#[test]
fn a_refreshed_missing_state_is_accepted_until_it_changes() {
    let mut w = world();
    let (id, _) = add(&mut w, &["docs/brief.md#L1", "calls/none.md#L1"], None);
    let (found, _) = sources_check(&w);
    let rows: Vec<_> = found
        .rows
        .iter()
        .map(|r| (r.kind, r.place.as_str()))
        .collect();
    assert_eq!(
        rows,
        [(SourceFinding::Missing, "evidence:2")],
        "missing since it was cited"
    );

    w.sources.remove_file(&path("docs/brief.md"));
    for at in [1, 2] {
        w.svc
            .node_evidence_refresh(&ctx(), &id.to_text(), Some(RefreshPick::At(at)))
            .unwrap();
    }
    let ev = evidence(&w, id);
    assert_eq!(
        (&ev[0]["hash_state"], &ev[0]["hash_accepted"]),
        (&Value::from("missing"), &Value::from(true))
    );
    let (found, hints) = sources_check(&w);
    assert!(found.rows.is_empty() && hints.is_empty(), "accepted");

    w.sources.write_file(path("calls/none.md"), b"here now\n");
    let (found, _) = sources_check(&w);
    let rows: Vec<_> = found
        .rows
        .iter()
        .map(|r| (r.kind, r.place.as_str()))
        .collect();
    assert_eq!(
        rows,
        [(SourceFinding::Changed, "evidence:2")],
        "it changed again"
    );
}
