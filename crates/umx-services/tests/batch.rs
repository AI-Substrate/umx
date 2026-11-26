//! `umx batch` over the fakes (req-88, ws003 s07): all or nothing on disk, in memory and in the embed queue; one
//! write_set per batch; refs; the single verbs' checks; dry runs that draw no ids. The write counter and the id
//! counter wrap the fakes here, so the fakes themselves are unchanged.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use umx_core::batch::{
    MAX_BATCH_ERRORS, MAX_BATCH_OPS, MAX_BATCH_WARNINGS, MAX_ECHO_SENTENCE_CHARS,
    MAX_ECHO_SENTENCES,
};
use umx_core::codec::GraphFile;
use umx_core::evidence::{EvidenceKind, EvidencePointer};
use umx_core::ports::{
    Change, Committed, CrashPoint, FileRef, FileStamp, FilesError, GraphDir, GraphFiles,
    GraphStamps, IdGenerator, ModelKey,
};
use umx_core::setup::GraphFilesSetup;
use umx_core::value::SetValue;
use umx_core::vectors::{ShardFile, ShardKind};
use umx_core::{Id, Problem};
use umx_fakes::{FakeClock, FakeEmbedder, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{
    BatchDone, BatchLine, BatchOp, BatchRequest, Ctx, EdgeEditRequest, EdgeListRequest,
    NodeAddRequest, NodeEditRequest, NodeNoteRequest, NodeStatusRequest, Ports, Services,
};

/// The fake files, counting write_sets and the files in each.
struct Counted {
    inner: Arc<FakeGraphFiles>,
    sets: Mutex<Vec<Vec<String>>>,
    /// Plan 205: every vector shard read fails while set, as an unreadable shard does.
    fail_shards: std::sync::atomic::AtomicBool,
}

impl GraphFiles for Counted {
    fn relocate(
        &self,
        from: &umx_core::ports::GraphDir,
        to: &umx_core::ports::GraphDir,
    ) -> Result<(), umx_core::ports::FilesError> {
        self.inner.relocate(from, to)
    }
    fn read_ref(&self, dir: &GraphDir, file: &FileRef) -> Result<Option<Vec<u8>>, FilesError> {
        if self.fail_shards.load(Ordering::SeqCst) && matches!(file, FileRef::Shard(_)) {
            return Err(FilesError::Io("read shard: planted failure".into()));
        }
        self.inner.read_ref(dir, file)
    }
    fn write_set_checked_also(
        &self,
        dir: &GraphDir,
        changes: &[Change<'_>],
        expect: Option<&GraphStamps>,
        also: &[(FileRef, FileStamp)],
    ) -> Result<Committed, FilesError> {
        let names = changes.iter().map(|c| c.file.path()).collect();
        self.sets.lock().unwrap().push(names);
        self.inner
            .write_set_checked_also(dir, changes, expect, also)
    }
    fn read_ref_stamped(
        &self,
        dir: &GraphDir,
        file: &FileRef,
    ) -> Result<(Option<Vec<u8>>, FileStamp), FilesError> {
        self.inner.read_ref_stamped(dir, file)
    }
    fn stamps(&self, dir: &GraphDir) -> Result<GraphStamps, FilesError> {
        self.inner.stamps(dir)
    }
    fn list(&self, dir: &GraphDir) -> Result<Vec<GraphFile>, FilesError> {
        self.inner.list(dir)
    }
    fn list_rule_sets(
        &self,
        dir: &GraphDir,
    ) -> Result<umx_core::ports::RuleSetListing, FilesError> {
        self.inner.list_rule_sets(dir)
    }
    fn stamp_ref(&self, dir: &GraphDir, file: &FileRef) -> Result<FileStamp, FilesError> {
        self.inner.stamp_ref(dir, file)
    }
    fn resolve(&self, dir: &GraphDir) -> Result<GraphDir, FilesError> {
        self.inner.resolve(dir)
    }
    fn list_shards(
        &self,
        dir: &GraphDir,
        key: &ModelKey,
        kind: ShardKind,
    ) -> Result<Vec<ShardFile>, FilesError> {
        self.inner.list_shards(dir, key, kind)
    }
    fn list_vector_spaces(&self, dir: &GraphDir) -> Result<Vec<ModelKey>, FilesError> {
        self.inner.list_vector_spaces(dir)
    }
    fn find_graphs(
        &self,
        root: &GraphDir,
        limits: umx_core::ports::FindLimits,
    ) -> Result<umx_core::ports::Found, FilesError> {
        self.inner.find_graphs(root, limits)
    }
    fn peek_graph(
        &self,
        dir: &GraphDir,
    ) -> Result<Option<umx_core::ports::FoundGraph>, FilesError> {
        self.inner.peek_graph(dir)
    }
    fn adapter_id(&self) -> &'static str {
        self.inner.adapter_id()
    }
}

/// The fake ids, counting draws.
struct CountedIds {
    inner: FakeIds,
    drawn: AtomicUsize,
}

impl IdGenerator for CountedIds {
    fn next_id(&self) -> Id {
        self.drawn.fetch_add(1, Ordering::SeqCst);
        self.inner.next_id()
    }
    fn adapter_id(&self) -> &'static str {
        self.inner.adapter_id()
    }
}

struct World {
    fake: Arc<FakeGraphFiles>,
    files: Arc<Counted>,
    ids: Arc<CountedIds>,
    svc: Services,
}

fn dir() -> GraphDir {
    GraphDir::parse("/work/g").unwrap()
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

fn world() -> World {
    let fake = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    world_on(fake, true)
}

fn world_on(fake: Arc<FakeGraphFiles>, new: bool) -> World {
    let files = Arc::new(Counted {
        inner: fake.clone(),
        sets: Mutex::default(),
        fail_shards: Default::default(),
    });
    let ids = Arc::new(CountedIds {
        inner: FakeIds::new(),
        drawn: AtomicUsize::new(0),
    });
    let ports = Ports {
        sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
            umx_core::ports::GraphDir::parse("/").unwrap(),
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
    let embedder = Arc::new(FakeEmbedder::new(ModelKey::new("fake", 8).unwrap()));
    let mut svc = Services::new(ports).with_embedder(embedder);
    if new {
        svc.graph_new(&ctx(), "/work/g", "demo", None, false, None)
            .unwrap();
    } else {
        svc.graph_load(&ctx(), "/work/g", None).unwrap();
    }
    files.sets.lock().unwrap().clear();
    ids.drawn.store(0, Ordering::SeqCst);
    World {
        fake,
        files,
        ids,
        svc,
    }
}

fn disk(w: &World) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    (
        w.fake.read(&dir(), GraphFile::Nodes).unwrap(),
        w.fake.read(&dir(), GraphFile::Edges).unwrap(),
    )
}

fn memory(w: &World) -> (Vec<u8>, Vec<u8>, String) {
    let g = w.svc.loaded_graph("demo").unwrap();
    (g.encode_nodes(), g.encode_edges(), g.word_index_image())
}

fn add(kind: &str, title: &str) -> NodeAddRequest {
    NodeAddRequest {
        kind: kind.into(),
        title: title.into(),
        ..Default::default()
    }
}

fn node(line: u32, r: Option<&str>, req: NodeAddRequest, parent_ref: Option<&str>) -> BatchLine {
    BatchLine {
        line,
        op: BatchOp::NodeAdd {
            id: None,
            created: None,
            ref_name: r.map(Into::into),
            req,
            parent_ref: parent_ref.map(Into::into),
        },
    }
}

fn edge(
    line: u32,
    from: (Option<&str>, Option<&str>),
    rel: &str,
    to: (Option<&str>, Option<&str>),
) -> BatchLine {
    BatchLine {
        line,
        op: BatchOp::EdgeAdd {
            id: None,
            from: from.0.map(Into::into),
            from_ref: from.1.map(Into::into),
            relation: rel.into(),
            to: to.0.map(Into::into),
            to_ref: to.1.map(Into::into),
            sets: Vec::new(),
            evidence: Default::default(),
        },
    }
}

fn edge_sets(mut line: BatchLine, values: Vec<(String, SetValue)>) -> BatchLine {
    let BatchOp::EdgeAdd { sets, .. } = &mut line.op else {
        panic!("an edge add line");
    };
    *sets = values;
    line
}

fn edit(line: u32, node: (Option<&str>, Option<&str>), req: NodeEditRequest) -> BatchLine {
    BatchLine {
        line,
        op: BatchOp::NodeEdit {
            node: node.0.map(Into::into),
            node_ref: node.1.map(Into::into),
            req,
        },
    }
}

fn run(w: &mut World, lines: Vec<BatchLine>, dry_run: bool) -> Result<BatchDone, Vec<Problem>> {
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
        .map(|d| d.value)
}

fn echo(w: &mut World, lines: Vec<BatchLine>, dry_run: bool) -> Result<BatchDone, Vec<Problem>> {
    w.svc
        .batch(
            &ctx().graph("demo"),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                reason: None,
                lines,
                dry_run,
                echo_sentences: true,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .map(|done| done.value)
}

#[test]
fn batch_echo_captures_refs_explicit_ids_and_each_edge_ops_line_time_direction() {
    let mut w = world();
    let a = Id(1001).to_text();
    let b = Id(1000).to_text();
    let e = Id(1002).to_text();
    let lines = vec![
        with_id(node(1, Some("a"), add("problem", "Alpha"), None), &a),
        with_id(node(2, Some("b"), add("option", "Beta"), None), &b),
        with_id(
            edge(3, (None, Some("a")), "supports", (None, Some("b"))),
            &e,
        ),
        edge(4, (None, Some("a")), "supports", (None, Some("b"))),
        edit(
            5,
            (Some(&format!("umx:demo/{a}")), None),
            NodeEditRequest {
                title: Some("Revised Alpha".into()),
                ..Default::default()
            },
        ),
        BatchLine {
            line: 6,
            op: BatchOp::EdgeEdit {
                edge: e.clone(),
                req: EdgeEditRequest {
                    relation: None,
                    reverse: true,
                    ..Default::default()
                },
            },
        },
        BatchLine {
            line: 7,
            op: BatchOp::EdgeEdit {
                edge: e.clone(),
                req: EdgeEditRequest {
                    relation: None,
                    reverse: false,
                    ..Default::default()
                },
            },
        },
        BatchLine {
            line: 8,
            op: BatchOp::EdgeDelete { edge: e.clone() },
        },
    ];
    let before = (disk(&w), memory(&w));
    let dry = echo(&mut w, lines.clone(), true).unwrap();
    assert_eq!((disk(&w), memory(&w)), before);
    assert_eq!(w.ids.drawn.load(Ordering::SeqCst), 0);
    assert!(w.files.sets.lock().unwrap().is_empty());
    let real = echo(&mut w, lines, false).unwrap();
    assert_eq!(dry.sentences, real.sentences);
    let sentences = real.sentences.unwrap();
    assert_eq!(
        sentences.iter().map(|s| s.line).collect::<Vec<_>>(),
        [3, 4, 6, 7, 8]
    );
    assert!(sentences.iter().all(|s| s.id == Some(Id(1002))));
    assert_eq!(
        sentences
            .iter()
            .map(|s| s.sentence.as_str())
            .collect::<Vec<_>>(),
        [
            "Alpha supports Beta",
            "Alpha supports Beta",
            "Beta supports Revised Alpha",
            "Beta supports Revised Alpha",
            "Beta supports Revised Alpha",
        ]
    );
    assert_eq!(sentences[4].op, "edge delete");
    assert_eq!(real.sentences_omitted, 0);
}

#[test]
fn batch_echo_keeps_default_receipts_and_hides_unminted_dry_ids() {
    let mut w = world();
    let lines = vec![
        node(1, Some("a"), add("problem", "Alpha"), None),
        node(2, Some("b"), add("option", "Beta"), None),
        edge(3, (None, Some("a")), "depends_on", (None, Some("b"))),
    ];
    let before = (disk(&w), memory(&w));
    let dry = echo(&mut w, lines.clone(), true).unwrap();
    let off = run(&mut w, lines.clone(), true).unwrap();
    let mut without_echo = dry.clone();
    without_echo.sentences = None;
    assert_eq!(without_echo, off);
    assert_eq!((disk(&w), memory(&w)), before);
    assert_eq!(w.ids.drawn.load(Ordering::SeqCst), 0);
    let sentence = &dry.sentences.as_ref().unwrap()[0];
    assert_eq!(sentence.id, None);
    assert_eq!(sentence.sentence, "Alpha depends on Beta");
    let real = echo(&mut w, lines, false).unwrap();
    assert!(real.sentences.as_ref().unwrap()[0].id.is_some());
    assert_eq!(
        sentence.sentence,
        real.sentences.as_ref().unwrap()[0].sentence
    );
}

#[test]
fn batch_echo_external_titles_and_graph_aliases_are_open_only_like_unloaded() {
    let mut w = world();
    let local = w
        .svc
        .node_add(&ctx(), add("problem", "Local"))
        .unwrap()
        .value
        .node
        .id
        .to_text();
    let far = w
        .svc
        .graph_new(&ctx(), "/work/far", "far_alias_147", None, false, None)
        .unwrap()
        .value
        .graph
        .id;
    let remote = w
        .svc
        .node_add(
            &ctx().graph("far_alias_147"),
            add("problem", "Visible far title"),
        )
        .unwrap()
        .value
        .node
        .id;
    let lines = |named: bool, explicit: bool| {
        let target = if named {
            format!("umx:far_alias_147/{remote}")
        } else {
            format!("umx:{far}/{remote}")
        };
        let graph = if named {
            "umx:far_alias_147".into()
        } else {
            format!("umx:{far}")
        };
        [
            edge(1, (Some(&local), None), "supports", (Some(&target), None)),
            edge(2, (Some(&local), None), "supports", (Some(&graph), None)),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            if explicit {
                with_id(line, &Id(9000 + i as u128).to_text())
            } else {
                line
            }
        })
        .collect()
    };
    let open = echo(&mut w, lines(true, true), true)
        .unwrap()
        .sentences
        .unwrap();
    assert_eq!(open[0].sentence, "Local supports Visible far title");
    assert_eq!(open[1].sentence, "Local supports far_alias_147");
    w.svc
        .graph_set(
            &ctx(),
            "far_alias_147",
            &[("policy".into(), "secured".into())],
        )
        .unwrap();
    let mut secured = None;
    for dry in [true, false] {
        let got = echo(&mut w, lines(true, true), dry)
            .unwrap()
            .sentences
            .unwrap();
        assert_eq!(
            got[0].sentence,
            format!("Local supports umx:{far}/{remote}")
        );
        assert_eq!(got[1].sentence, format!("Local supports umx:{far}/{far}"));
        for sentence in &got {
            assert!(!sentence.sentence.contains("Visible far title"));
            assert!(!sentence.sentence.contains("far_alias_147"));
        }
        secured = Some(got);
    }
    w.svc.graph_unload(&ctx(), "far_alias_147").unwrap();
    for dry in [true, false] {
        assert_eq!(
            echo(&mut w, lines(false, false), dry).unwrap().sentences,
            secured
        );
    }
}

#[test]
fn batch_echo_secured_graph_alias_plant_is_hidden_even_when_the_input_names_it() {
    let mut w = world();
    let local = w
        .svc
        .node_add(&ctx(), add("problem", "Local"))
        .unwrap()
        .value
        .node
        .id
        .to_text();
    let far = w
        .svc
        .graph_new(&ctx(), "/work/alias", "alias_plant_147", None, false, None)
        .unwrap()
        .value
        .graph
        .id;
    w.svc
        .graph_set(
            &ctx(),
            "alias_plant_147",
            &[("policy".into(), "secured".into())],
        )
        .unwrap();
    for dry in [true, false] {
        let got = echo(
            &mut w,
            vec![edge(
                1,
                (Some(&local), None),
                "supports",
                (Some("umx:alias_plant_147"), None),
            )],
            dry,
        )
        .unwrap();
        let text = &got.sentences.as_ref().unwrap()[0].sentence;
        assert_eq!(text, &format!("Local supports umx:{far}/{far}"));
        assert!(!text.contains("alias_plant_147"), "{text}");
    }
}

#[test]
fn batch_echo_caps_records_and_sentence_characters_without_writing_a_dry_run() {
    for count in [0, 100, 101, MAX_BATCH_OPS - 2] {
        let mut w = world();
        let mut lines = vec![
            node(1, Some("a"), add("problem", "Alpha"), None),
            node(2, Some("b"), add("option", "Beta"), None),
        ];
        lines.extend((0..count).map(|i| {
            edge(
                i as u32 + 3,
                (None, Some("a")),
                "supports",
                (None, Some("b")),
            )
        }));
        let before = (disk(&w), memory(&w));
        let got = echo(&mut w, lines, true).unwrap();
        assert_eq!(
            got.sentences.as_ref().unwrap().len(),
            count.min(MAX_ECHO_SENTENCES)
        );
        assert_eq!(
            got.sentences_omitted,
            count.saturating_sub(MAX_ECHO_SENTENCES)
        );
        assert!(
            got.sentences
                .unwrap()
                .iter()
                .all(|s| s.sentence == "Alpha supports Beta")
        );
        assert_eq!((disk(&w), memory(&w)), before);
        assert_eq!(w.ids.drawn.load(Ordering::SeqCst), 0);
        assert!(w.files.sets.lock().unwrap().is_empty());
    }
    let mut w = world();
    let got = echo(
        &mut w,
        vec![
            node(1, Some("a"), add("problem", &"左".repeat(2000)), None),
            node(2, Some("b"), add("option", &"右".repeat(2000)), None),
            edge(3, (None, Some("a")), "supports", (None, Some("b"))),
        ],
        true,
    )
    .unwrap();
    let text = &got.sentences.as_ref().unwrap()[0].sentence;
    assert!(text.chars().count() <= MAX_ECHO_SENTENCE_CHARS);
    assert!(text.starts_with('左') && text.ends_with("..."));
    assert!(text.contains("... supports 右"));
}

#[test]
fn batch_echo_keeps_atomic_rollback_when_a_later_line_fails() {
    let mut w = world();
    let before = (disk(&w), memory(&w));
    let errors = echo(
        &mut w,
        vec![
            node(1, Some("a"), add("problem", "Alpha"), None),
            node(2, Some("b"), add("option", "Beta"), None),
            edge(3, (None, Some("a")), "supports", (None, Some("b"))),
            node(4, Some("a"), add("problem", "Repeated ref"), None),
        ],
        false,
    )
    .unwrap_err();
    assert_eq!(
        codes(&errors),
        [("batch_failed", None), ("batch_ref_repeated", Some(4))]
    );
    assert_eq!((disk(&w), memory(&w)), before);
    assert!(w.files.sets.lock().unwrap().is_empty());
    assert!(w.svc.take_embed_jobs().is_empty());
}

fn note_line(
    line: u32,
    node: (Option<&str>, Option<&str>),
    append: &str,
    evidence: Vec<EvidencePointer>,
) -> BatchLine {
    BatchLine {
        line,
        op: BatchOp::NodeNote {
            node: node.0.map(Into::into),
            node_ref: node.1.map(Into::into),
            req: NodeNoteRequest {
                append: append.into(),
                evidence,
            },
        },
    }
}

fn status_line(
    line: u32,
    node: (Option<&str>, Option<&str>),
    status: &str,
    reason: Option<&str>,
    label: Option<&str>,
) -> BatchLine {
    BatchLine {
        line,
        op: BatchOp::NodeStatus {
            node: node.0.map(Into::into),
            node_ref: node.1.map(Into::into),
            req: NodeStatusRequest {
                anchor: None,
                evidence: None,
                evidence_kind: None,
                status: status.into(),
                reason: reason.map(Into::into),
                label: label.map(Into::into),
            },
        },
    }
}

#[test]
fn batch_notes_and_status_match_singles_and_do_not_mutate_dry_runs() {
    let mut batch = world();
    let mut single = world();
    let mut req = add("problem", "Review");
    req.note = Some("First paragraph".into());
    let id = batch
        .svc
        .node_add(&ctx(), req.clone())
        .unwrap()
        .value
        .node
        .id;
    let other = single.svc.node_add(&ctx(), req).unwrap().value.node.id;
    assert_eq!(id, other);
    batch.svc.take_embed_jobs();
    single.svc.take_embed_jobs();
    batch.files.sets.lock().unwrap().clear();
    batch.ids.drawn.store(0, Ordering::SeqCst);
    let note = NodeNoteRequest {
        append: "Second paragraph".into(),
        evidence: vec![
            EvidencePointer {
                pointer: "notes.md#L1-L2".into(),
                kind: Some(EvidenceKind::Document),
            },
            EvidencePointer {
                pointer: "notes.md#quote=An inference".into(),
                kind: Some(EvidenceKind::Analyst),
            },
        ],
    };
    let address = format!("umx:demo/{id}");
    let lines = vec![
        note_line(
            1,
            (Some(&id.to_text()), None),
            &note.append,
            note.evidence.clone(),
        ),
        status_line(2, (Some(&address), None), "agreed", Some("reviewed"), None),
    ];
    let before = (disk(&batch), memory(&batch));
    let dry = run(&mut batch, lines.clone(), true).unwrap();
    assert_eq!(dry.rows[0].changed, ["note", "evidence"]);
    assert_eq!(dry.rows[1].changed, ["status", "status_history"]);
    assert_eq!((disk(&batch), memory(&batch)), before);
    assert!(batch.files.sets.lock().unwrap().is_empty());
    assert_eq!(batch.ids.drawn.load(Ordering::SeqCst), 0);
    assert!(batch.svc.take_embed_jobs().is_empty());

    run(&mut batch, lines, false).unwrap();
    single
        .svc
        .node_note_request(&ctx(), &other.to_text(), note)
        .unwrap();
    single
        .svc
        .node_status(&ctx(), &other.to_text(), "agreed", Some("reviewed"), None)
        .unwrap();
    let actual = batch.svc.node_get(&ctx(), &id.to_text()).unwrap().value.1;
    assert_eq!(
        actual,
        single
            .svc
            .node_get(&ctx(), &other.to_text())
            .unwrap()
            .value
            .1
    );
    assert_eq!(actual.note, "First paragraph\n\nSecond paragraph");
    assert_eq!(actual.evidence[0]["kind"], "document");
    assert_eq!(actual.evidence[1]["kind"], "analyst");
    assert_eq!(actual.status_history[0]["by"], "tester");
    assert_eq!(actual.status_history[0]["label"], "resolved");
    assert_eq!(batch.svc.take_embed_jobs().len(), 1);
}

#[test]
fn batch_notes_refuse_virtual_graph_fragments_before_mutation_as_singles_do() {
    let mut w = world();
    let owner = w.svc.graph_list()[0].id;
    let id = w
        .svc
        .node_add(&ctx(), add("problem", "Review"))
        .unwrap()
        .value
        .node
        .id;
    w.svc.take_embed_jobs();
    w.files.sets.lock().unwrap().clear();
    let before = (disk(&w), memory(&w));
    let mut req = NodeNoteRequest {
        append: "Reviewed".into(),
        evidence: vec![EvidencePointer {
            pointer: format!("umx:demo/{owner}#L1"),
            kind: Some(EvidenceKind::Analyst),
        }],
    };
    let error = w
        .svc
        .node_note_request(&ctx(), &id.to_text(), req.clone())
        .unwrap_err();
    assert_eq!(error.code, "evidence_pointer_invalid");
    for dry in [true, false] {
        let errors = run(
            &mut w,
            vec![note_line(
                7,
                (Some(&id.to_text()), None),
                &req.append,
                req.evidence.clone(),
            )],
            dry,
        )
        .unwrap_err();
        assert_eq!(
            codes(&errors),
            [
                ("batch_failed", None),
                ("evidence_pointer_invalid", Some(7))
            ]
        );
        assert_eq!((disk(&w), memory(&w)), before);
        assert!(w.files.sets.lock().unwrap().is_empty());
        assert!(w.svc.take_embed_jobs().is_empty());
    }

    req.evidence[0].pointer = format!("umx:demo/{id}#L1");
    run(
        &mut w,
        vec![note_line(
            7,
            (Some(&id.to_text()), None),
            &req.append,
            req.evidence,
        )],
        false,
    )
    .unwrap();
    let node = w.svc.node_get(&ctx(), &id.to_text()).unwrap().value.1;
    assert_eq!(node.note, "Reviewed");
    assert_eq!(node.evidence[0]["kind"], "analyst");
}

#[test]
fn batch_note_status_refs_limits_and_rollback_preserve_the_graph() {
    let mut w = world();
    let lines = vec![
        node(1, Some("p"), add("problem", "Review"), None),
        note_line(2, (None, Some("p")), "Appended", Vec::new()),
        status_line(3, (None, Some("p")), "agreed", Some("reviewed"), None),
    ];
    let done = run(&mut w, lines, false).unwrap();
    let id = done.refs[0].1.unwrap();
    let got = w.svc.node_get(&ctx(), &id.to_text()).unwrap().value.1;
    assert_eq!(
        (got.note.as_str(), got.status.as_str()),
        ("Appended", "agreed")
    );
    w.svc.take_embed_jobs();
    let before = (disk(&w), memory(&w));
    let pointer = EvidencePointer {
        pointer: "src/aws_secret_key.rs#L1".into(),
        kind: None,
    };
    for dry in [true, false] {
        let errors = run(
            &mut w,
            vec![
                note_line(
                    1,
                    (Some(&id.to_text()), None),
                    "Would roll back",
                    vec![pointer.clone()],
                ),
                status_line(
                    2,
                    (Some(&id.to_text()), None),
                    "open",
                    Some("reviewed"),
                    Some("not_a_label"),
                ),
            ],
            dry,
        )
        .unwrap_err();
        assert_eq!(
            codes(&errors),
            [("batch_failed", None), ("usage_invalid", Some(2))]
        );
        assert_eq!((disk(&w), memory(&w)), before);
        assert!(w.svc.take_embed_jobs().is_empty());
        let errors = run(
            &mut w,
            vec![note_line(
                3,
                (Some(&id.to_text()), None),
                "Over cap",
                vec![pointer.clone(); 33],
            )],
            dry,
        )
        .unwrap_err();
        assert_eq!(
            codes(&errors),
            [("batch_failed", None), ("batch_line_invalid", Some(3))]
        );
    }
    let errors = run(
        &mut w,
        vec![
            note_line(1, (None, Some("later")), "Forward ref", Vec::new()),
            node(2, Some("later"), add("problem", "Later"), None),
        ],
        false,
    )
    .unwrap_err();
    assert_eq!(errors[1].code, "batch_ref_unknown");
    assert_eq!((disk(&w), memory(&w)), before);

    w.fake.fail_writes(true);
    assert!(
        run(
            &mut w,
            vec![
                note_line(
                    1,
                    (Some(&id.to_text()), None),
                    "Lost",
                    vec![pointer.clone()]
                ),
                status_line(
                    2,
                    (Some(&id.to_text()), None),
                    "open",
                    Some("reviewed"),
                    None
                ),
            ],
            false,
        )
        .is_err()
    );
    assert_eq!((disk(&w), memory(&w)), before);
    assert!(w.svc.take_embed_jobs().is_empty());
    w.fake.fail_writes(false);
    run(
        &mut w,
        vec![note_line(
            1,
            (Some(&id.to_text()), None),
            "Benign path",
            vec![pointer],
        )],
        false,
    )
    .unwrap();
}

#[test]
fn batch_write_scan_refuses_each_surface_in_dry_and_real_runs_without_echoing_values() {
    let token = format!("ghp_{}", "a".repeat(36));
    for surface in [
        "note", "reason", "quote", "section", "query", "edge", "add", "edit",
    ] {
        for dry in [true, false] {
            let mut w = world();
            let a = w
                .svc
                .node_add(&ctx(), add("problem", "A"))
                .unwrap()
                .value
                .node
                .id
                .to_text();
            let b = w
                .svc
                .node_add(&ctx(), add("problem", "B"))
                .unwrap()
                .value
                .node
                .id
                .to_text();
            w.svc.take_embed_jobs();
            w.files.sets.lock().unwrap().clear();
            let before = (disk(&w), memory(&w));
            let line = match surface {
                "note" => note_line(7, (Some(&a), None), &token, Vec::new()),
                "reason" => status_line(7, (Some(&a), None), "agreed", Some(&token), None),
                "quote" | "section" | "query" => {
                    let pointer = match surface {
                        "quote" => format!("notes.md#quote={}", token.replace('_', "%5F")),
                        "section" => {
                            format!("notes.md#section=Overview/{}", token.replace('_', "%5F"))
                        }
                        _ => format!("https://example.test/a?token={token}#L1"),
                    };
                    note_line(
                        7,
                        (Some(&a), None),
                        "Safe append",
                        vec![EvidencePointer {
                            pointer,
                            kind: None,
                        }],
                    )
                }
                "edge" => {
                    let mut line = edge(7, (Some(&a), None), "supports", (Some(&b), None));
                    if let BatchOp::EdgeAdd { sets, .. } = &mut line.op {
                        sets.push(("access_token".into(), SetValue::Str(token.clone())));
                    }
                    line
                }
                "add" => {
                    let mut req = add("problem", "Tainted");
                    req.note = Some(token.clone());
                    node(7, Some("tainted"), req, None)
                }
                "edit" => edit(
                    7,
                    (Some(&a), None),
                    NodeEditRequest {
                        note: Some(token.clone()),
                        ..Default::default()
                    },
                ),
                _ => unreachable!(),
            };
            let errors = run(&mut w, vec![line], dry).unwrap_err();
            assert_eq!(
                codes(&errors),
                [("batch_failed", None), ("batch_line_invalid", Some(7))],
                "{surface}"
            );
            assert!(errors[1].slot("reason").unwrap().contains("secret rule"));
            let rendered = format!("{errors:?}");
            assert!(
                !rendered.contains(&token) && !rendered.contains("ghp_"),
                "{rendered}"
            );
            assert_eq!((disk(&w), memory(&w)), before);
            assert!(w.files.sets.lock().unwrap().is_empty());
            assert!(w.svc.take_embed_jobs().is_empty());
        }
    }
    let mut w = world();
    let mut req = add("problem", "Tainted");
    req.note = Some(token);
    let before = (disk(&w), memory(&w));
    for dry in [true, false] {
        let errors = run(
            &mut w,
            vec![
                node(1, Some("tainted"), req.clone(), None),
                edit(
                    2,
                    (None, Some("tainted")),
                    NodeEditRequest {
                        note: Some("Removed later".into()),
                        ..Default::default()
                    },
                ),
            ],
            dry,
        )
        .unwrap_err();
        assert_eq!(errors[1].code, "batch_line_invalid");
        assert_eq!(errors[1].line, Some(1));
        assert_eq!((disk(&w), memory(&w)), before);
    }
}

#[test]
fn edge_edit_rechecks_string_fields_and_refuses_both_batch_modes() {
    let mut w = world();
    let token = format!("ghp_{}", "a".repeat(36));
    let a = w
        .svc
        .node_add(&ctx(), add("problem", "A"))
        .unwrap()
        .value
        .node
        .id
        .to_text();
    let b = w
        .svc
        .node_add(&ctx(), add("problem", "B"))
        .unwrap()
        .value
        .node
        .id
        .to_text();
    let added = w
        .svc
        .edge_add(
            &ctx(),
            &a,
            "supports",
            &b,
            &[("access_token".into(), SetValue::Str(token.clone()))],
        )
        .unwrap();
    assert!(added.notes.iter().any(|p| p.code == "secret_suspected"));
    let edge = added.value.edge.id.to_text();
    w.svc.take_embed_jobs();
    w.files.sets.lock().unwrap().clear();
    let before = (disk(&w), memory(&w));
    let req = EdgeEditRequest {
        relation: Some("blocks".into()),
        reverse: false,
        ..Default::default()
    };
    for dry in [true, false] {
        let errors = run(
            &mut w,
            vec![
                note_line(1, (Some(&a), None), "Must roll back", Vec::new()),
                BatchLine {
                    line: 2,
                    op: BatchOp::EdgeEdit {
                        edge: edge.clone(),
                        req: req.clone(),
                    },
                },
            ],
            dry,
        )
        .unwrap_err();
        assert_eq!(
            codes(&errors),
            [("batch_failed", None), ("batch_line_invalid", Some(2))]
        );
        assert!(errors[1].slot("reason").unwrap().contains("access_token"));
        assert!(!format!("{errors:?}").contains(&token));
        assert_eq!((disk(&w), memory(&w)), before);
        assert!(w.files.sets.lock().unwrap().is_empty());
        assert!(w.svc.take_embed_jobs().is_empty());
    }
    let single = w.svc.edge_edit(&ctx(), &edge, &req).unwrap();
    assert_eq!(single.value.edge.kind, "blocks");
    assert_eq!(
        single.value.edge.fields["access_token"].as_str(),
        Some(token.as_str())
    );
    assert!(single.notes.iter().any(|p| p.code == "secret_suspected"));
}

#[test]
fn a_153_op_mixed_batch_writes_each_file_once_and_queues_each_node_once() {
    let mut w = world();
    let anchor = w
        .svc
        .node_add(&ctx(), add("problem", "Anchor"))
        .unwrap()
        .value
        .node
        .id
        .to_text();
    w.svc.take_embed_jobs();
    w.files.sets.lock().unwrap().clear();
    w.ids.drawn.store(0, Ordering::SeqCst);
    let mut lines = Vec::new();
    for i in 0..31 {
        let reference = format!("n{i}");
        let line = lines.len() as u32 + 1;
        lines.push(node(
            line,
            Some(&reference),
            add("problem", &format!("Review {i}")),
            None,
        ));
        if i < 30 {
            lines.push(edge(
                line + 1,
                (None, Some(&reference)),
                "supports",
                (Some(&anchor), None),
            ));
            lines.push(edit(
                line + 2,
                (None, Some(&reference)),
                NodeEditRequest {
                    note: Some("Replaced".into()),
                    ..Default::default()
                },
            ));
        }
        let line = lines.len() as u32 + 1;
        lines.push(note_line(
            line,
            (None, Some(&reference)),
            "Appended",
            Vec::new(),
        ));
        lines.push(status_line(
            line + 1,
            (None, Some(&reference)),
            "agreed",
            Some("reviewed"),
            None,
        ));
    }
    assert_eq!(lines.len(), 153);
    let before = (disk(&w), memory(&w));
    run(&mut w, lines.clone(), true).unwrap();
    assert_eq!((disk(&w), memory(&w)), before);
    assert!(w.files.sets.lock().unwrap().is_empty());
    assert_eq!(w.ids.drawn.load(Ordering::SeqCst), 0);
    let done = run(&mut w, lines, false).unwrap();
    assert_eq!(done.rows.len(), 153);
    assert_eq!(
        *w.files.sets.lock().unwrap(),
        vec![vec!["nodes.jsonl".to_owned(), "edges.jsonl".to_owned()]]
    );
    assert_eq!(w.svc.take_embed_jobs().len(), 31);
}

fn codes(ps: &[Problem]) -> Vec<(&str, Option<u32>)> {
    ps.iter().map(|p| (p.code, p.line)).collect()
}

mod provenance {
    use super::*;
    use umx_core::provenance::{Actor, ActorInput, Tier};

    fn writer(session: &str, role: &str) -> Ctx {
        ctx().actor(
            Actor::try_from(ActorInput {
                tier: Tier::Pij,
                seat: Some("pij-alpha-seat".into()),
                role: Some(role.into()),
                model: Some("openai/test-model".into()),
                effort: Some("high".into()),
                harness: Some("copilot".into()),
                harness_session: Some(session.into()),
                user: Some("tester".into()),
            })
            .unwrap(),
        )
    }

    #[test]
    fn batch_notes_and_statuses_stamp_only_the_new_evidence_and_last_editor() {
        use umx_services::{NodeNoteRequest, NodeStatusRequest};
        let mut w = world();
        let author = writer("h-0123456789abcdef", "coder");
        let mut req = add("problem", "Attributed batch target");
        req.evidence = vec!["docs/old.md#L1".into()];
        let old = w.svc.node_add(&author, req).unwrap().value.node;
        let editor = writer("h-fedcba9876543210", "reviewer");
        let lines = vec![
            BatchLine {
                line: 1,
                op: BatchOp::NodeNote {
                    node: Some(old.id.to_text()),
                    node_ref: None,
                    req: NodeNoteRequest::from_legacy(
                        "More detail",
                        &["docs/old.md#L1".into(), "docs/new.md#L2".into()],
                        None,
                    )
                    .unwrap(),
                },
            },
            BatchLine {
                line: 2,
                op: BatchOp::NodeStatus {
                    node: Some(old.id.to_text()),
                    node_ref: None,
                    req: NodeStatusRequest {
                        anchor: None,
                        evidence: None,
                        evidence_kind: None,
                        status: "resolved".into(),
                        reason: Some("Evidence checked".into()),
                        label: None,
                    },
                },
            },
        ];
        let before = (disk(&w), memory(&w));
        w.svc
            .batch(
                &editor,
                BatchRequest {
                    idempotency: None,
                    raised_in: None,
                    reason: None,
                    echo_sentences: false,
                    lines: lines.clone(),
                    dry_run: true,
                    refs: Default::default(),
                    refs_graph: None,
                },
            )
            .unwrap();
        assert_eq!((disk(&w), memory(&w)), before);
        w.fake.fail_writes_to(Some(GraphFile::Edges));
        assert!(
            w.svc
                .batch(
                    &editor,
                    BatchRequest {
                        idempotency: None,
                        raised_in: None,
                        reason: None,
                        echo_sentences: false,
                        lines: lines.clone(),
                        dry_run: false,
                        refs: Default::default(),
                        refs_graph: None,
                    }
                )
                .is_err()
        );
        w.fake.fail_writes_to(None);
        assert_eq!((disk(&w), memory(&w)), before);
        w.svc
            .batch(
                &editor,
                BatchRequest {
                    idempotency: None,
                    raised_in: None,
                    reason: None,
                    echo_sentences: false,
                    lines,
                    dry_run: false,
                    refs: Default::default(),
                    refs_graph: None,
                },
            )
            .unwrap();
        let node = w
            .svc
            .loaded_graph("demo")
            .unwrap()
            .node(old.id)
            .unwrap()
            .to_node();
        assert_eq!(node.authored_by_session, old.authored_by_session);
        let session = node.edited_by_session.unwrap();
        assert_ne!(Some(session), old.authored_by_session);
        assert_eq!(node.evidence.len(), 2);
        assert_eq!(node.evidence[0], old.evidence[0]);
        assert_eq!(node.evidence[1]["by_session"], session.to_text());
        assert_eq!(node.status, "resolved");
    }

    #[test]
    fn edge_edits_use_the_actor_display_and_atomically_link_the_editing_session() {
        use umx_services::EdgeEditRequest;
        for batch in [false, true] {
            let mut w = world();
            let author = writer("h-0123456789abcdef", "coder");
            let a = w
                .svc
                .node_add(&author, add("problem", "Source"))
                .unwrap()
                .value
                .node
                .id;
            let b = w
                .svc
                .node_add(&author, add("problem", "Target"))
                .unwrap()
                .value
                .node
                .id;
            let original = w
                .svc
                .edge_add(&author, &a.to_text(), "supports", &b.to_text(), &[])
                .unwrap()
                .value
                .edge;
            let apply = |w: &mut World, editor: &Ctx, req: EdgeEditRequest| {
                if batch {
                    w.svc
                        .batch(
                            editor,
                            BatchRequest {
                                idempotency: None,
                                raised_in: None,
                                reason: None,
                                echo_sentences: false,
                                lines: vec![BatchLine {
                                    line: 1,
                                    op: BatchOp::EdgeEdit {
                                        edge: original.id.to_text(),
                                        req,
                                    },
                                }],
                                dry_run: false,
                                refs: Default::default(),
                                refs_graph: None,
                            },
                        )
                        .map(|_| ())
                        .map_err(|ps| ps.into_iter().map(|p| p.code).collect::<Vec<_>>())
                } else {
                    w.svc
                        .edge_edit(editor, &original.id.to_text(), &req)
                        .map(|_| ())
                        .map_err(|p| vec![p.code])
                }
            };
            let mut editor = writer("h-fedcba9876543210", "reviewer");
            editor.by = "forged-by-label".into();
            let before = (disk(&w), memory(&w));
            let writes = w.files.sets.lock().unwrap().len();
            apply(&mut w, &editor, EdgeEditRequest::default()).unwrap();
            assert_eq!(
                (disk(&w), memory(&w)),
                before,
                "a no-op must not mint the new session"
            );
            assert_eq!(w.files.sets.lock().unwrap().len(), writes);
            w.fake.fail_writes_to(Some(GraphFile::Nodes));
            assert!(
                apply(
                    &mut w,
                    &editor,
                    EdgeEditRequest {
                        reverse: true,
                        ..Default::default()
                    }
                )
                .is_err()
            );
            w.fake.fail_writes_to(None);
            assert_eq!(
                (disk(&w), memory(&w)),
                before,
                "failed session and edge writes roll back together"
            );
            apply(
                &mut w,
                &editor,
                EdgeEditRequest {
                    reverse: true,
                    ..Default::default()
                },
            )
            .unwrap();
            let g = w.svc.loaded_graph("demo").unwrap();
            let edited = g.edge(original.id).unwrap().to_edge();
            assert_eq!(edited.fields["updated_by"], "pij-alpha-seat");
            assert_eq!(edited.created_by, original.created_by);
            assert_eq!(edited.authored_by_session, original.authored_by_session);
            let session = edited.edited_by_session.unwrap();
            assert!(!edited.fields.contains_key("edited_by_session"));
            assert_ne!(Some(session), original.authored_by_session);
            let session = g.node(session).unwrap();
            let agent = g
                .edges_out(session.ix())
                .find(|e| e.kind.as_str() == "session_of")
                .unwrap();
            assert_eq!(
                g.node_at(agent.other).field("prov_key"),
                Some(serde_json::json!("agent/pij/pij-alpha-seat"))
            );
            let rows = w
                .svc
                .query(
                    &ctx(),
                    &umx_services::CypherRequest {
                        text: format!(
                            "MATCH ()-[r]->() WHERE r.id = '{}' RETURN r.edited_by_session",
                            original.id
                        ),
                        limit: None,
                        offset: None,
                        params: vec![],
                        along: None,
                    },
                )
                .unwrap()
                .value
                .rows;
            assert_eq!(
                rows,
                [vec![umx_services::cypher::Cell::Str(
                    session.id().to_text()
                )]]
            );
            let reloaded = world_on(w.fake.clone(), false);
            assert_eq!(
                reloaded
                    .svc
                    .loaded_graph("demo")
                    .unwrap()
                    .edge(original.id)
                    .unwrap()
                    .to_edge(),
                edited
            );

            let mut harness: ActorInput = editor.actor.into();
            harness.tier = Tier::Harness;
            harness.seat = None;
            harness.harness_session = Some("h-aaaaaaaaaaaaaaaa".into());
            let mut editor = ctx().actor(Actor::try_from(harness).unwrap());
            editor.by = "another-label".into();
            apply(
                &mut w,
                &editor,
                EdgeEditRequest {
                    reverse: true,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                w.svc
                    .loaded_graph("demo")
                    .unwrap()
                    .edge(original.id)
                    .unwrap()
                    .field("updated_by"),
                Some(serde_json::json!("tester"))
            );
        }
    }

    #[test]
    fn metadata_minting_never_consumes_an_explicit_content_identity() {
        let mut w = world();
        let id = Id(w.svc.graph_list()[0].id.0 + 1);
        let line = with_id(
            node(1, Some("content"), add("problem", "Explicit content"), None),
            &id.to_text(),
        );
        let done = run(&mut w, vec![line], false).unwrap();
        assert_eq!(done.refs[0].1, Some(id));
        let content = w.svc.node_get(&ctx(), &id.to_text()).unwrap().value.1;
        assert_eq!(content.title, "Explicit content");
        let session = w
            .svc
            .loaded_graph("demo")
            .unwrap()
            .provenance_node("session/unknown/unknown")
            .unwrap()
            .id();
        assert_ne!(session, id);
        assert_eq!(content.authored_by_session, Some(session));
    }

    #[test]
    fn writes_share_idempotent_unstamped_metadata_and_preserve_original_authorship() {
        let mut w = world();
        let first = writer("h-0123456789abcdef", "coder");
        let mut request = add("problem", "First content");
        request.evidence = vec!["docs/first.md#L1".into()];
        let added = w.svc.node_add(&first, request).unwrap().value.node;
        let authored = added.authored_by_session.unwrap();
        assert_eq!(added.edited_by_session, Some(authored));
        assert_eq!(added.created_by, "tester");
        assert_eq!(added.evidence[0]["by_session"], authored.to_text());
        assert_eq!(
            w.files.sets.lock().unwrap().as_slice(),
            [vec!["nodes.jsonl".to_string(), "edges.jsonl".to_string()]]
        );
        let g = w.svc.loaded_graph("demo").unwrap();
        assert_eq!(g.nodes().filter(|n| g.is_provenance(n.ix())).count(), 5);
        assert!(
            g.nodes()
                .filter(|n| g.is_provenance(n.ix()))
                .all(|n| { n.authored_by_session().is_none() && n.edited_by_session().is_none() })
        );
        assert!(g.edges().all(|e| e.authored_by_session().is_none()));
        assert_eq!(
            g.provenance_node("session/copilot/h-0123456789abcdef")
                .unwrap()
                .field("model"),
            Some(serde_json::json!("openai/test-model"))
        );
        let mut child = add("problem", "Second content");
        child.parent = Some(added.id.to_text());
        let child = w.svc.node_add(&first, child).unwrap().value;
        assert_eq!(
            child.parent_edge.unwrap().edge.authored_by_session,
            Some(authored)
        );
        let g = w.svc.loaded_graph("demo").unwrap();
        assert_eq!(g.nodes().filter(|n| g.is_provenance(n.ix())).count(), 5);
        let second = writer("h-fedcba9876543210", "reviewer");
        let edited = w
            .svc
            .node_edit(
                &second,
                &added.id.to_text(),
                NodeEditRequest {
                    title: Some("Edited content".into()),
                    ..Default::default()
                },
            )
            .unwrap()
            .value
            .node;
        assert_eq!(edited.authored_by_session, Some(authored));
        assert_ne!(edited.edited_by_session, Some(authored));
        let g = w.svc.loaded_graph("demo").unwrap();
        assert_eq!(
            g.provenance_node("agent/pij/pij-alpha-seat")
                .unwrap()
                .field("role"),
            Some(serde_json::json!("reviewer"))
        );
        let before = memory(&w);
        let writes = w.files.sets.lock().unwrap().len();
        let unused = writer("h-aaaaaaaaaaaaaaaa", "unused");
        let unchanged = w
            .svc
            .node_edit(
                &unused,
                &added.id.to_text(),
                NodeEditRequest {
                    title: Some("Edited content".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(unchanged.value.changed.is_empty());
        assert_eq!(memory(&w), before);
        assert_eq!(w.files.sets.lock().unwrap().len(), writes);
        assert!(
            w.svc
                .loaded_graph("demo")
                .unwrap()
                .provenance_node("session/copilot/h-aaaaaaaaaaaaaaaa")
                .is_none()
        );
    }

    #[test]
    fn failed_writes_failed_batches_and_dry_runs_leave_no_provenance_or_content() {
        let mut w = world();
        let ctx = writer("h-0123456789abcdef", "coder");
        let before = (disk(&w), memory(&w));
        w.fake.fail_writes_to(Some(GraphFile::Edges));
        assert!(w.svc.node_add(&ctx, add("problem", "Unwritten")).is_err());
        w.fake.fail_writes_to(None);
        assert_eq!((disk(&w), memory(&w)), before);
        for dry_run in [false, true] {
            let lines = if dry_run {
                vec![node(1, Some("a"), add("problem", "Dry content"), None)]
            } else {
                vec![
                    node(1, Some("a"), add("problem", "Rolled back"), None),
                    edge(2, (None, Some("missing")), "supports", (None, Some("a"))),
                ]
            };
            let result = w.svc.batch(
                &ctx,
                BatchRequest {
                    idempotency: None,
                    raised_in: None,
                    reason: None,
                    lines,
                    dry_run,
                    echo_sentences: false,
                    refs: Default::default(),
                    refs_graph: None,
                },
            );
            assert_eq!(result.is_ok(), dry_run);
            assert_eq!((disk(&w), memory(&w)), before);
        }
        assert!(w.svc.take_embed_jobs().is_empty());
    }

    #[test]
    fn first_attributed_write_after_restart_never_mints_the_owner_graph_id() {
        for batch in [false, true] {
            let original = world();
            let owner = original.svc.loaded("demo").unwrap().meta.id;
            assert!(
                original
                    .svc
                    .loaded_graph("demo")
                    .unwrap()
                    .nodes()
                    .next()
                    .is_none()
            );
            let mut w = world_on(original.fake.clone(), false);
            let content = if batch {
                run(
                    &mut w,
                    vec![node(
                        1,
                        Some("content"),
                        add("problem", "First content"),
                        None,
                    )],
                    false,
                )
                .unwrap()
                .refs[0]
                    .1
                    .unwrap()
            } else {
                w.svc
                    .node_add(&ctx(), add("problem", "First content"))
                    .unwrap()
                    .value
                    .node
                    .id
            };
            let reloaded = world_on(w.fake.clone(), false);
            let graph = reloaded.svc.loaded_graph("demo").unwrap();
            assert!(!graph.holds_id(owner), "batch={batch}");
            let session = graph
                .provenance_node("session/unknown/unknown")
                .unwrap()
                .id();
            assert_ne!(content, owner);
            assert_ne!(session, owner);
            assert_eq!(
                graph.node(content).unwrap().authored_by_session(),
                Some(session)
            );
        }
    }

    #[test]
    fn change_feed_hides_provenance_nodes_and_edges_for_single_and_batch_writes() {
        use umx_core::ui::{Fed, Touched};
        for batch in [false, true] {
            let mut w = world();
            w.svc.take_changes();
            let (parent, child, edge) = if batch {
                let done = run(
                    &mut w,
                    vec![
                        node(1, Some("parent"), add("problem", "Parent"), None),
                        node(2, Some("child"), add("problem", "Child"), Some("parent")),
                    ],
                    false,
                )
                .unwrap();
                (
                    done.refs[0].1.unwrap(),
                    done.refs[1].1.unwrap(),
                    done.rows[1].parent_edge.unwrap(),
                )
            } else {
                let parent = w
                    .svc
                    .node_add(&ctx(), add("problem", "Parent"))
                    .unwrap()
                    .value
                    .node
                    .id;
                let mut request = add("problem", "Child");
                request.parent = Some(parent.to_text());
                let child = w.svc.node_add(&ctx(), request).unwrap().value;
                (parent, child.node.id, child.parent_edge.unwrap().edge.id)
            };
            assert!(w.svc.loaded_graph("demo").unwrap().edges().count() > 1);
            let changes = w.svc.take_changes();
            assert_eq!(changes.len(), if batch { 1 } else { 2 });
            let mut touched = Touched::default();
            for change in changes {
                let Fed::Wrote {
                    graph,
                    touched: next,
                    ..
                } = change
                else {
                    panic!("content writes do not reset the feed");
                };
                assert_eq!(graph, "demo");
                touched.then(&next);
            }
            let mut expected = Touched::default();
            expected.nodes.added.extend([parent, child]);
            expected.edges.added.insert(edge);
            assert_eq!(touched, expected, "batch={batch}");
        }
    }

    #[test]
    fn batches_echo_and_stamp_content_and_parent_edges_but_not_the_provenance_graph() {
        let mut w = world();
        let mut child = add("problem", "Child");
        child.evidence = vec!["docs/child.md#L1".into()];
        let done = w
            .svc
            .batch(
                &writer("h-0123456789abcdef", "coder"),
                BatchRequest {
                    idempotency: None,
                    raised_in: None,
                    reason: None,
                    echo_sentences: true,
                    lines: vec![
                        node(1, Some("a"), add("problem", "Parent"), None),
                        node(2, Some("b"), child, Some("a")),
                        edge(3, (None, Some("b")), "supports", (None, Some("a"))),
                    ],
                    dry_run: false,
                    refs: Default::default(),
                    refs_graph: None,
                },
            )
            .unwrap()
            .value;
        let sentences = done.sentences.as_ref().unwrap();
        assert_eq!(sentences.len(), 1);
        assert_eq!(sentences[0].line, 3);
        assert_eq!(sentences[0].op, "edge add");
        assert_eq!(sentences[0].id, done.rows[2].id);
        assert_eq!(sentences[0].sentence, "Child supports Parent");
        assert_eq!(done.sentences_omitted, 0);
        let g = w.svc.loaded_graph("demo").unwrap();
        let session = g
            .provenance_node("session/copilot/h-0123456789abcdef")
            .unwrap()
            .id();
        for (_, id) in done.refs {
            let n = g.node(id.unwrap()).unwrap();
            assert_eq!(n.authored_by_session(), Some(session));
            assert_eq!(n.edited_by_session(), Some(session));
            for pointer in n.to_node().evidence {
                assert_eq!(pointer["by_session"], session.to_text());
            }
        }
        for e in g.edges() {
            assert_eq!(
                e.authored_by_session(),
                (!umx_core::ontology::PROVENANCE_RELATIONS.contains(&e.kind())).then_some(session)
            );
        }
        assert_eq!(w.files.sets.lock().unwrap().len(), 1);
        assert_eq!(w.svc.take_embed_jobs().len(), 2);
    }

    #[test]
    fn a_conflicting_session_keeps_its_first_agent_and_warns_without_new_metadata() {
        let mut w = world();
        let first = writer("h-0123456789abcdef", "coder");
        let added = w
            .svc
            .node_add(&first, add("problem", "First"))
            .unwrap()
            .value
            .node;
        let mut input: ActorInput = first.actor.clone().into();
        input.seat = Some("pij-beta-seat".into());
        let second = ctx().actor(Actor::try_from(input).unwrap());
        let before = w
            .svc
            .loaded_graph("demo")
            .unwrap()
            .nodes()
            .filter(|n| w.svc.loaded_graph("demo").unwrap().is_provenance(n.ix()))
            .count();
        let done = w.svc.node_add(&second, add("problem", "Second")).unwrap();
        assert!(
            done.notes
                .iter()
                .any(|w| w.code == "session_agent_conflict")
        );
        assert_eq!(
            done.value.node.authored_by_session,
            added.authored_by_session
        );
        let g = w.svc.loaded_graph("demo").unwrap();
        assert_eq!(
            g.nodes().filter(|n| g.is_provenance(n.ix())).count(),
            before
        );
        assert!(g.provenance_node("agent/pij/pij-beta-seat").is_none());
    }

    #[test]
    fn session_snapshots_keep_known_facts_through_gaps_and_accept_new_known_values() {
        let mut w = world();
        let first = writer("h-0123456789abcdef", "coder");
        let added = w
            .svc
            .node_add(&first, add("problem", "First capture"))
            .unwrap()
            .value
            .node;
        let session = added.authored_by_session.unwrap();
        for value in [None, Some("unknown"), Some("UNKNOWN")] {
            let mut missing: ActorInput = first.actor.clone().into();
            missing.role = value.map(|_| "unknown".into());
            missing.model = value.map(str::to_owned);
            missing.effort = value.map(|_| "unknown".into());
            w.svc
                .node_add(
                    &ctx().actor(Actor::try_from(missing).unwrap()),
                    add("problem", "Capture gap"),
                )
                .unwrap();
            let g = w.svc.loaded_graph("demo").unwrap();
            let kept = g.node(session).unwrap();
            assert_eq!(kept.field("role"), Some(serde_json::json!("coder")));
            assert_eq!(
                kept.field("model"),
                Some(serde_json::json!("openai/test-model"))
            );
            assert_eq!(kept.field("effort"), Some(serde_json::json!("high")));
            assert_eq!(
                g.provenance_node("agent/pij/pij-alpha-seat")
                    .unwrap()
                    .field("role"),
                Some(serde_json::json!("coder"))
            );
            assert!(g.provenance_node("model/unknown").is_none());
        }
        let mut newer: ActorInput = first.actor.clone().into();
        newer.role = Some("reviewer".into());
        newer.model = Some("openai/next-model".into());
        newer.effort = Some("low".into());
        w.svc
            .node_add(
                &ctx().actor(Actor::try_from(newer).unwrap()),
                add("problem", "New known capture"),
            )
            .unwrap();
        let g = w.svc.loaded_graph("demo").unwrap();
        let latest = g.node(session).unwrap();
        assert_eq!(latest.field("role"), Some(serde_json::json!("reviewer")));
        assert_eq!(
            latest.field("model"),
            Some(serde_json::json!("openai/next-model"))
        );
        assert_eq!(latest.field("effort"), Some(serde_json::json!("low")));
        assert_eq!(
            g.provenance_node("agent/pij/pij-alpha-seat")
                .unwrap()
                .field("role"),
            Some(serde_json::json!("reviewer"))
        );
        assert!(g.provenance_node("model/openai-test-model").is_some());
        assert!(g.provenance_node("model/openai-next-model").is_some());
    }

    #[test]
    fn an_edit_keeps_bad_authorship_replaces_bad_editor_and_tags_only_new_evidence() {
        let mut w = world();
        let mut request = add("problem", "Older content");
        request.evidence = vec!["docs/old.md#L1".into()];
        let id = w.svc.node_add(&ctx(), request).unwrap().value.node.id;
        let mut records: Vec<_> = w
            .svc
            .loaded_graph("demo")
            .unwrap()
            .nodes()
            .map(|n| n.to_node())
            .collect();
        let old = records.iter_mut().find(|n| n.id == id).unwrap();
        old.authored_by_session = None;
        old.edited_by_session = None;
        old.fields
            .insert("authored_by_session".into(), serde_json::json!("not-an-id"));
        old.fields
            .insert("edited_by_session".into(), serde_json::Value::Null);
        old.evidence[0]
            .as_object_mut()
            .unwrap()
            .remove("by_session");
        w.fake
            .write_set(
                &dir(),
                &[Change {
                    file: GraphFile::Nodes.into(),
                    bytes: Some(&umx_core::codec::encode_nodes(&records)),
                }],
            )
            .unwrap();
        w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
        let writer = writer("h-0123456789abcdef", "coder");
        let done = w
            .svc
            .node_note(
                &writer,
                &id.to_text(),
                "More evidence",
                &["docs/old.md#L1".into(), "docs/new.md#L2".into()],
                None,
            )
            .unwrap()
            .value;
        assert_eq!(done.evidence_added, 1);
        assert_eq!(done.node.authored_by_session, None);
        assert_eq!(done.node.fields["authored_by_session"], "not-an-id");
        assert!(!done.node.fields.contains_key("edited_by_session"));
        assert!(done.node.evidence[0].get("by_session").is_none());
        assert_eq!(
            done.node.evidence[1]["by_session"],
            done.node.edited_by_session.unwrap().to_text()
        );
        let encoded = serde_json::to_string(&done.node).unwrap();
        assert_eq!(encoded.matches("\"authored_by_session\":").count(), 1);
        assert_eq!(encoded.matches("\"edited_by_session\":").count(), 1);
        let again = w
            .svc
            .node_note(
                &writer,
                &id.to_text(),
                "Already cited",
                &["docs/new.md#L2".into()],
                None,
            )
            .unwrap();
        assert_eq!(again.value.evidence_added, 0);
    }

    #[test]
    fn reserved_types_stay_content_and_reserved_relations_and_fields_are_refused() {
        let mut w = world();
        let user = w.svc.node_add(&ctx(), add("agent", "User agent")).unwrap();
        assert!(user.notes.iter().any(|w| w.code == "reserved_type"));
        let a = user.value.node.id;
        let b = w
            .svc
            .node_add(&ctx(), add("problem", "Other content"))
            .unwrap()
            .value
            .node
            .id;
        assert_eq!(w.svc.node_get(&ctx(), &a.to_text()).unwrap().value.1.id, a);
        for relation in umx_core::ontology::PROVENANCE_RELATIONS {
            assert_eq!(
                w.svc
                    .edge_add(&ctx(), &a.to_text(), relation, &b.to_text(), &[])
                    .unwrap_err()
                    .code,
                "reserved_relation"
            );
        }
        for relation in ["authored_by", "edited_by"] {
            assert_eq!(
                w.svc
                    .edge_add(&ctx(), &a.to_text(), relation, &b.to_text(), &[])
                    .unwrap_err()
                    .code,
                "derived_relation"
            );
        }
        let created = w
            .svc
            .edge_add(&ctx(), &a.to_text(), "supports", &b.to_text(), &[])
            .unwrap()
            .value
            .edge;
        assert!(created.authored_by_session.is_some());
        for field in umx_core::ontology::PROVENANCE_KEYS {
            assert_eq!(
                w.svc
                    .edge_add(
                        &ctx(),
                        &a.to_text(),
                        "supports",
                        &b.to_text(),
                        &[(field.to_string(), SetValue::Str("invalid".into()))]
                    )
                    .unwrap_err()
                    .code,
                "reserved_name"
            );
        }
    }

    #[test]
    fn duplicate_provenance_records_load_warn_and_keep_the_lower_id() {
        let mut w = world();
        w.svc.node_add(&ctx(), add("problem", "Content")).unwrap();
        let g = w.svc.loaded_graph("demo").unwrap();
        let original = g.provenance_node("agent/unknown").unwrap().id();
        let mut duplicate = g.node(original).unwrap().to_node();
        duplicate.id = Id(g.last_id().unwrap().0 + 1);
        let duplicate_id = duplicate.id;
        let mut records: Vec<_> = g.nodes().map(|n| n.to_node()).collect();
        records.push(duplicate);
        w.fake
            .write_set(
                &dir(),
                &[Change {
                    file: GraphFile::Nodes.into(),
                    bytes: Some(&umx_core::codec::encode_nodes(&records)),
                }],
            )
            .unwrap();
        let loaded = w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
        assert!(loaded.notes.iter().any(|w| w.code == "duplicate_prov_key"));
        let g = w.svc.loaded_graph("demo").unwrap();
        assert_eq!(g.provenance_node("agent/unknown").unwrap().id(), original);
        assert!(g.node(duplicate_id).is_some());
        assert!(
            w.svc
                .check(&ctx(), None, None)
                .unwrap()
                .value
                .page
                .rows
                .iter()
                .any(|r| r.kind == "duplicate_prov_key")
        );
    }
}

fn with_id(mut line: BatchLine, id: &str) -> BatchLine {
    match &mut line.op {
        BatchOp::NodeAdd { id: slot, .. } | BatchOp::EdgeAdd { id: slot, .. } => {
            *slot = Some(id.into());
        }
        _ => panic!("an add"),
    }
    line
}

#[test]
fn ports_keep_ids_created_refs_and_line_order_including_dry_runs_and_failed_writes() {
    let mut w = world();
    let x = w
        .svc
        .node_add(&ctx(), add("problem", "Base source"))
        .unwrap()
        .value
        .node
        .id;
    let y = w
        .svc
        .node_add(&ctx(), add("problem", "Base destination"))
        .unwrap()
        .value
        .node
        .id;
    w.svc
        .edge_add(&ctx(), &x.to_text(), "supports", &y.to_text(), &[])
        .unwrap();
    w.svc.take_embed_jobs();
    w.ids.drawn.store(0, Ordering::SeqCst);
    w.files.sets.lock().unwrap().clear();
    let (a, b, e) = (Id(30), Id(10), Id(5));
    let a_text = a.to_text();
    let mut first = with_id(
        node(1, Some("p1"), add("problem", "Invoice dispute queue"), None),
        &a_text,
    );
    if let BatchOp::NodeAdd { created, .. } = &mut first.op {
        *created = Some("2024-11-11T10:00:00+10:00".into());
    }
    let lines = vec![
        first,
        with_id(
            node(
                2,
                Some("p2"),
                add("problem", "Invoice dispute queue!"),
                None,
            ),
            &b.to_text(),
        ),
        with_id(
            edge(3, (Some(&a_text), None), "supports", (None, Some("p2"))),
            &e.to_text(),
        ),
        edit(
            4,
            (Some(&a_text), None),
            NodeEditRequest {
                note: Some("Kept".into()),
                ..Default::default()
            },
        ),
    ];
    let before = (disk(&w), memory(&w));
    let dry = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                reason: None,
                lines: lines.clone(),
                dry_run: true,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap();
    assert_eq!(
        dry.value.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        [Some(a), Some(b), Some(e), Some(a)]
    );
    assert_eq!(
        dry.value.refs,
        [("p1".into(), Some(a)), ("p2".into(), Some(b))]
    );
    let near: Vec<_> = dry
        .notes
        .iter()
        .filter(|p| p.code == "near_duplicate")
        .collect();
    assert_eq!(near.len(), 1);
    assert_eq!(
        near[0].line,
        Some(2),
        "future explicit nodes never influence earlier lines"
    );
    assert_eq!(near[0].candidates[0].id, a_text);
    assert_eq!(w.ids.drawn.load(Ordering::SeqCst), 0);
    assert_eq!((disk(&w), memory(&w)), before);
    w.fake.fail_writes(true);
    assert!(run(&mut w, lines.clone(), false).is_err());
    assert_eq!((disk(&w), memory(&w)), before);
    w.fake.fail_writes(false);
    run(&mut w, lines, false).unwrap();
    let graph = w.svc.loaded_graph("demo").unwrap();
    let n = graph.node(a).unwrap().to_node();
    assert_eq!(n.created, "2024-11-11T00:00:00Z");
    assert_eq!(n.updated, "2026-11-09T00:00:00Z");
    assert_eq!(n.note, "Kept");
    assert_eq!(graph.edge(e).unwrap().from(), a);
    assert_eq!(graph.edge(e).unwrap().to(), b);
}

#[test]
fn edge_edits_and_deletes_keep_explicit_ids_and_reach_edges_added_out_of_order() {
    // F75 with an earlier fix: a batch with explicit ids can retype, reverse and delete the out-of-order edges it adds,
    // and delete an old edge, all or nothing: a dry run and a failed write change nothing.
    let mut w = world();
    let x = w
        .svc
        .node_add(&ctx(), add("problem", "Source"))
        .unwrap()
        .value
        .node
        .id;
    let y = w
        .svc
        .node_add(&ctx(), add("problem", "Target"))
        .unwrap()
        .value
        .node
        .id;
    let (x, y) = (x.to_text(), y.to_text());
    let old = w
        .svc
        .edge_add(&ctx(), &x, "supports", &y, &[])
        .unwrap()
        .value
        .edge
        .id;
    w.svc.take_embed_jobs();
    w.ids.drawn.store(0, Ordering::SeqCst);
    w.files.sets.lock().unwrap().clear();
    let (a, b, e5, e6) = (Id(30), Id(10), Id(5), Id(6));
    let line = |line, op| BatchLine { line, op };
    let lines = vec![
        with_id(
            node(1, Some("p1"), add("problem", "Alpha"), None),
            &a.to_text(),
        ),
        with_id(
            node(2, Some("p2"), add("decision", "Bravo"), None),
            &b.to_text(),
        ),
        with_id(
            edge(3, (None, Some("p1")), "supports", (None, Some("p2"))),
            &e5.to_text(),
        ),
        with_id(
            edge(4, (Some(&x), None), "blocks", (Some(&y), None)),
            &e6.to_text(),
        ),
        line(
            5,
            BatchOp::EdgeEdit {
                edge: e5.to_text(),
                req: EdgeEditRequest {
                    relation: Some("blocks".into()),
                    reverse: true,
                    ..Default::default()
                },
            },
        ),
        line(6, BatchOp::EdgeDelete { edge: e6.to_text() }),
        line(
            7,
            BatchOp::EdgeDelete {
                edge: old.to_text(),
            },
        ),
    ];
    let before = (disk(&w), memory(&w));
    let dry = run(&mut w, lines.clone(), true).unwrap();
    assert_eq!(
        dry.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        [
            Some(a),
            Some(b),
            Some(e5),
            Some(e6),
            Some(e5),
            Some(e6),
            Some(old)
        ]
    );
    assert_eq!(dry.rows[4].changed, ["type", "direction"]);
    assert_eq!((disk(&w), memory(&w)), before);
    w.fake.fail_writes(true);
    assert!(run(&mut w, lines.clone(), false).is_err());
    assert_eq!((disk(&w), memory(&w)), before);
    w.fake.fail_writes(false);
    run(&mut w, lines, false).unwrap();
    assert_eq!(w.ids.drawn.load(Ordering::SeqCst), 0);
    let graph = w.svc.loaded_graph("demo").unwrap();
    let kept = graph.edge(e5).unwrap().to_edge();
    assert_eq!((kept.from, kept.kind.as_str(), kept.to), (b, "blocks", a));
    assert_eq!(kept.fields["updated_by"], "unknown");
    assert!(graph.edge(e6).is_none() && graph.edge(old).is_none());
    assert_eq!(
        graph
            .edges()
            .filter(|e| e.authored_by_session().is_some())
            .count(),
        1
    );
    let file = w.fake.read(&dir(), GraphFile::Edges).unwrap().unwrap();
    assert_eq!(
        umx_core::codec::decode_edges(&file)
            .unwrap()
            .records
            .into_iter()
            .filter(|e| e.authored_by_session.is_some())
            .count(),
        1
    );
}

#[test]
fn a_future_explicit_id_uses_one_bulk_plan_without_changing_generated_identities() {
    let mut w = world();
    let future = Id(u128::from(umx_fakes::FAKE_EPOCH_MS + 59_000) << 80);
    let mut lines = vec![with_id(
        node(1, Some("p0"), add("problem", "Future source"), None),
        &future.to_text(),
    )];
    for i in 0..1000 {
        lines.push(node(
            i + 2,
            None,
            add("problem", &format!("Imported parcel {i:04x}")),
            Some("p0"),
        ));
    }
    let before = (disk(&w), memory(&w));
    let dry = run(&mut w, lines.clone(), true).unwrap();
    assert_eq!(dry.rows[0].id, Some(future));
    assert!(dry.rows[1..].iter().all(|r| r.id.is_none()));
    assert_eq!(w.ids.drawn.load(Ordering::SeqCst), 0);
    assert_eq!((disk(&w), memory(&w)), before);
    let done = run(&mut w, lines, false).unwrap();
    assert_eq!(done.rows[0].id, Some(future));
    let prefix = u128::from(umx_fakes::FAKE_EPOCH_MS) << 80;
    for (i, row) in done.rows[1..].iter().enumerate() {
        assert_eq!(row.id, Some(Id(prefix | (i as u128 * 2 + 2))));
        assert_eq!(row.parent_edge, Some(Id(prefix | (i as u128 * 2 + 3))));
    }
    let g = w.svc.loaded_graph("demo").unwrap();
    assert_eq!(
        (
            g.nodes().filter(|n| !g.is_provenance(n.ix())).count(),
            g.edges().filter(|e| e.kind() == "part_of").count()
        ),
        (1001, 1000)
    );
    g.check_rows();
}

#[test]
fn explicit_ids_preflight_every_identity_and_the_clock_boundary_before_drawing_or_writing() {
    use umx_fakes::FAKE_EPOCH_MS;
    let mut w = world();
    let existing = w
        .svc
        .node_add(&ctx(), add("problem", "Existing"))
        .unwrap()
        .value
        .node
        .id;
    w.ids.drawn.store(0, Ordering::SeqCst);
    w.files.sets.lock().unwrap().clear();
    let before = (disk(&w), memory(&w));
    let future = Id(u128::from(FAKE_EPOCH_MS + 60_001) << 80);
    let errors = run(
        &mut w,
        vec![
            with_id(
                node(1, None, add("problem", "UUID"), None),
                "550e8400-e29b-41d4-a716-446655440000",
            ),
            with_id(node(2, None, add("problem", "Prefix"), None), "01JC8X"),
            with_id(
                node(3, None, add("problem", "Existing"), None),
                &existing.to_text(),
            ),
            with_id(
                node(4, None, add("problem", "Future"), None),
                &future.to_text(),
            ),
            with_id(
                node(5, None, add("problem", "First"), None),
                &Id(22).to_text(),
            ),
            with_id(
                edge(
                    6,
                    (Some(&existing.to_text()), None),
                    "supports",
                    (Some(&existing.to_text()), None),
                ),
                &Id(22).to_text(),
            ),
        ],
        false,
    )
    .unwrap_err();
    assert_eq!(
        codes(&errors),
        [
            ("batch_failed", None),
            ("id_invalid", Some(1)),
            ("id_invalid", Some(2)),
            ("id_taken", Some(3)),
            ("id_in_future", Some(4)),
            ("id_taken", Some(6)),
        ]
    );
    assert_eq!((disk(&w), memory(&w)), before);
    assert_eq!(w.ids.drawn.load(Ordering::SeqCst), 0);
    assert!(w.files.sets.lock().unwrap().is_empty());
    let boundary = Id(u128::from(FAKE_EPOCH_MS + 60_000) << 80);
    run(
        &mut w,
        vec![with_id(
            node(1, None, add("problem", "Boundary"), None),
            &boundary.to_text(),
        )],
        true,
    )
    .unwrap();
}

#[test]
fn explicit_node_ids_reserve_only_their_own_graph_id_before_drawing_or_writing() {
    let mut w = world();
    let owner = w.svc.graph_list()[0].id;
    assert!(!w.svc.loaded_graph("demo").unwrap().holds_id(owner));
    let before = (disk(&w), memory(&w));
    for dry_run in [false, true] {
        let errors = run(
            &mut w,
            vec![with_id(
                node(1, None, add("problem", "Reserved graph identity"), None),
                &owner.to_text(),
            )],
            dry_run,
        )
        .unwrap_err();
        assert_eq!(
            codes(&errors),
            [("batch_failed", None), ("id_taken", Some(1))]
        );
        assert_eq!(
            errors[1].slot("reason").as_deref(),
            Some("the graph id (reserved for its virtual node)")
        );
        assert_eq!((disk(&w), memory(&w)), before);
        assert_eq!(w.ids.drawn.load(Ordering::SeqCst), 0);
        assert!(w.files.sets.lock().unwrap().is_empty());
    }

    let other = w
        .svc
        .graph_new(&ctx(), "/work/other", "other", None, false, None)
        .unwrap()
        .value
        .graph
        .id;
    let done = w
        .svc
        .batch(
            &ctx().graph("demo"),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                reason: None,
                lines: vec![with_id(
                    node(1, None, add("problem", "Another graph identity"), None),
                    &other.to_text(),
                )],
                dry_run: false,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap();
    assert_eq!(done.value.rows[0].id, Some(other));
    assert_eq!(
        w.svc
            .node_get(&ctx().graph("demo"), &other.to_text())
            .unwrap()
            .value
            .1
            .kind,
        "problem"
    );
    assert_eq!(
        w.svc.node_get(&ctx(), "umx:other").unwrap().value.1.kind,
        "graph"
    );
}

#[test]
fn explicit_nodes_are_unavailable_before_their_line_and_do_not_infer_future_fields() {
    let mut w = world();
    let text = Id(10).to_text();
    let before = (disk(&w), memory(&w));
    let errors = run(
        &mut w,
        vec![
            edit(
                1,
                (Some(&text), None),
                NodeEditRequest {
                    title: Some("Too soon".into()),
                    ..Default::default()
                },
            ),
            with_id(node(2, None, add("problem", "Later"), None), &text),
        ],
        false,
    )
    .unwrap_err();
    assert_eq!(
        codes(&errors),
        [("batch_failed", None), ("node_not_found", Some(1))]
    );
    assert_eq!((disk(&w), memory(&w)), before);
    let mut first = add("problem", "First");
    first
        .sets
        .push(("port_field".into(), SetValue::Str("a".into())));
    let mut second = add("problem", "Second");
    second
        .sets
        .push(("port_field".into(), SetValue::Json(serde_json::json!(true))));
    let done = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                reason: None,
                lines: vec![
                    with_id(node(1, None, first, None), &Id(30).to_text()),
                    with_id(node(2, None, second, None), &text),
                ],
                dry_run: false,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap();
    let mismatch: Vec<_> = done
        .notes
        .iter()
        .filter(|p| p.code == "field_kind_mismatch")
        .map(|p| p.line)
        .collect();
    assert_eq!(mismatch, [Some(2)]);
    for bad in [
        "2026-11-09",
        "2026-02-30T00:00:00Z",
        "2026-11-09T00:00:00.badZ",
        "123456789é",
    ] {
        let mut line = node(9, None, add("problem", "Bad created"), None);
        if let BatchOp::NodeAdd { created, .. } = &mut line.op {
            *created = Some(bad.into());
        }
        assert_eq!(
            codes(&run(&mut w, vec![line], false).unwrap_err()),
            [("batch_failed", None), ("batch_line_invalid", Some(9))]
        );
    }
}

#[test]
fn held_tombstones_repeated_lines_and_dangling_edge_ids_are_never_reused() {
    let mut w = world();
    let n = w
        .svc
        .node_add(&ctx(), add("problem", "Original"))
        .unwrap()
        .value
        .node;
    let mut tombstone = n.clone();
    tombstone.status = "deleted".into();
    let nodes = umx_core::codec::encode_nodes([&n, &tombstone]);
    w.fake.write(&dir(), GraphFile::Nodes, &nodes).unwrap();
    let edge_id = Id(80);
    let edges = format!(
        "{{\"id\":\"{edge_id}\",\"type\":\"supports\",\"from\":\"{}\",\"to\":\"{}\",\"created\":\"2026-11-09T00:00:00Z\",\"created_by\":\"hand\"}}\n",
        Id(81),
        Id(82),
    );
    w.fake
        .write(&dir(), GraphFile::Edges, edges.as_bytes())
        .unwrap();
    let mut w = world_on(w.fake, false);
    let before = (disk(&w), memory(&w));
    let errors = run(
        &mut w,
        vec![
            with_id(
                node(1, None, add("problem", "Reuse"), None),
                &n.id.to_text(),
            ),
            with_id(
                node(2, None, add("problem", "Reuse edge id"), None),
                &edge_id.to_text(),
            ),
        ],
        false,
    )
    .unwrap_err();
    assert_eq!(
        codes(&errors),
        [
            ("batch_failed", None),
            ("id_taken", Some(1)),
            ("id_taken", Some(2))
        ]
    );
    assert_eq!((disk(&w), memory(&w)), before);
}

#[test]
fn unnamed_secured_graphs_cannot_be_probed_by_explicit_ids() {
    let setup = |source_id| {
        let mut w = world();
        run(
            &mut w,
            vec![with_id(
                node(1, None, add("problem", "Source"), None),
                &Id(source_id).to_text(),
            )],
            false,
        )
        .unwrap();
        w.svc
            .graph_set(&ctx(), "demo", &[("policy".into(), "secured".into())])
            .unwrap();
        w.svc
            .graph_new(&ctx(), "/work/target", "target", None, false, None)
            .unwrap();
        w
    };
    let request = || BatchRequest {
        idempotency: None,
        raised_in: None,
        echo_sentences: false,
        reason: None,
        lines: vec![with_id(
            node(1, Some("estate"), add("problem", "Imported"), None),
            &Id(17).to_text(),
        )],
        dry_run: true,
        refs: Default::default(),
        refs_graph: None,
    };
    let mut held = setup(17);
    let mut absent = setup(18);
    let target = ctx().graph("target");
    let held_reply = held.svc.batch(&target, request()).unwrap();
    let absent_reply = absent.svc.batch(&target, request()).unwrap();
    assert_eq!(
        held_reply, absent_reply,
        "no warning or count may reveal the secured node"
    );
    assert!(
        held_reply
            .notes
            .iter()
            .all(|p| p.code != "id_in_other_graph")
    );

    let mut named = request();
    named.lines.push(edge(
        2,
        (None, Some("estate")),
        "supports",
        (Some(&format!("umx:demo/{}", Id(17))), None),
    ));
    let named_reply = held.svc.batch(&target, named).unwrap();
    assert!(
        named_reply
            .notes
            .iter()
            .any(|p| p.code == "id_in_other_graph")
    );

    held.svc
        .graph_set(&ctx(), "demo", &[("policy".into(), "open".into())])
        .unwrap();
    assert_eq!(
        held.svc.effective_policy("demo"),
        Some(umx_core::Policy::OPEN)
    );
    let open_reply = held.svc.batch(&target, request()).unwrap();
    assert_eq!(
        open_reply
            .notes
            .iter()
            .filter(|p| p.code == "id_in_other_graph")
            .count(),
        1
    );
}

#[test]
fn bad_ref_errors_identify_the_first_character_position_or_limit_without_echoing_text() {
    let mut w = world();
    for (name, character, position, rule) in [
        (
            "brd_exA",
            "A",
            "7",
            "a ref holds only lowercase a-z 0-9 _ and -",
        ),
        ("9abc", "9", "1", "a ref starts with a lowercase letter"),
        (
            "a.b",
            ".",
            "2",
            "a ref holds only lowercase a-z 0-9 _ and -",
        ),
        (
            "a\u{202e}private",
            "U+202E",
            "2",
            "a ref holds only lowercase a-z 0-9 _ and -",
        ),
    ] {
        let errors = run(
            &mut w,
            vec![node(4, Some(name), add("problem", "T"), None)],
            false,
        )
        .unwrap_err();
        let p = &errors[1];
        assert_eq!((p.code, p.line), ("batch_line_invalid", Some(4)));
        let reason = p.slot("reason").unwrap();
        assert!(
            reason.contains(character)
                && reason.contains(&format!("position {position}"))
                && reason.contains(rule),
            "{reason}"
        );
        assert!(
            !reason.contains(name) && !reason.contains('\u{202e}'),
            "{reason}"
        );
    }
    let name = "a".repeat(33);
    let errors = run(
        &mut w,
        vec![node(5, Some(&name), add("problem", "T"), None)],
        false,
    )
    .unwrap_err();
    assert!(errors[1].slot("reason").unwrap().contains("limit of 32"));
}

#[test]
fn ordinary_words_are_batch_refs_in_their_separate_fields() {
    let mut w = world();
    let done = run(
        &mut w,
        vec![
            node(1, Some("estate"), add("problem", "Estate plan"), None),
            node(
                2,
                Some("contract"),
                add("option", "Contract terms"),
                Some("estate"),
            ),
            node(
                3,
                Some("workshop"),
                add("question", "Workshop decision"),
                None,
            ),
            edge(
                4,
                (None, Some("contract")),
                "supports",
                (None, Some("workshop")),
            ),
            edit(
                5,
                (None, Some("estate")),
                NodeEditRequest {
                    note: Some("Discussed".into()),
                    ..Default::default()
                },
            ),
        ],
        false,
    )
    .unwrap();
    assert_eq!(
        done.refs
            .iter()
            .map(|(r, _)| r.as_str())
            .collect::<Vec<_>>(),
        ["estate", "contract", "workshop"]
    );
    assert_eq!(
        w.svc
            .loaded_graph("demo")
            .unwrap()
            .node(done.refs[0].1.unwrap())
            .unwrap()
            .to_node()
            .note,
        "Discussed"
    );
    let errors = run(
        &mut w,
        vec![edit(
            7,
            (None, Some("deadbeef")),
            NodeEditRequest {
                title: Some("Unknown ref".into()),
                ..Default::default()
            },
        )],
        false,
    )
    .unwrap_err();
    assert_eq!(
        codes(&errors),
        [("batch_failed", None), ("batch_ref_unknown", Some(7))]
    );
}

#[test]
fn copied_node_ids_warn_only_across_different_loaded_graph_identities() {
    let mut w = world();
    let id = Id(17);
    run(
        &mut w,
        vec![with_id(
            node(1, None, add("problem", "Source"), None),
            &id.to_text(),
        )],
        false,
    )
    .unwrap();
    let copy_dir = GraphDir::parse("/work/copy").unwrap();
    for file in [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges] {
        let bytes = w.fake.read(&dir(), file).unwrap().unwrap();
        w.fake.write(&copy_dir, file, &bytes).unwrap();
    }
    w.svc
        .graph_load(&ctx(), "/work/copy", Some("copy"))
        .unwrap();
    w.svc
        .graph_new(&ctx(), "/work/target", "target", None, false, None)
        .unwrap();
    let mut target = ctx();
    target.graph = Some("target".into());
    let done = w
        .svc
        .batch(
            &target,
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                reason: None,
                lines: vec![with_id(
                    node(1, None, add("problem", "Imported"), None),
                    &id.to_text(),
                )],
                dry_run: true,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap();
    assert_eq!(
        done.notes
            .iter()
            .filter(|p| p.code == "id_in_other_graph")
            .count(),
        2
    );
    w.svc.graph_unload(&ctx(), "demo").unwrap();
    // Same graph id: a copy import is not another graph identity.
    let mut copy = ctx();
    copy.graph = Some("copy".into());
    let fresh = Id(18);
    w.svc
        .batch(
            &copy,
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                reason: None,
                lines: vec![with_id(
                    node(1, None, add("problem", "Fresh"), None),
                    &fresh.to_text(),
                )],
                dry_run: false,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap();
    // Reload the source with the copy's new files, giving two copies that hold the same fresh id.
    for file in [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges] {
        let mut bytes = w.fake.read(&copy_dir, file).unwrap().unwrap();
        if file == GraphFile::Nodes {
            let mut n = w
                .svc
                .loaded_graph("copy")
                .unwrap()
                .node(fresh)
                .unwrap()
                .to_node();
            n.id = Id(19);
            bytes.extend(umx_core::codec::encode_nodes([&n]));
        }
        w.fake.write(&dir(), file, &bytes).unwrap();
    }
    w.svc.graph_load(&ctx(), "/work/g", Some("demo")).unwrap();
    let done = w
        .svc
        .batch(
            &copy,
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                reason: None,
                lines: vec![with_id(
                    node(1, None, add("problem", "Different fresh"), None),
                    &Id(19).to_text(),
                )],
                dry_run: true,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap();
    assert!(done.notes.iter().all(|p| p.code != "id_in_other_graph"));
}

/// 1,000 lines: a question per ten, each answer under its question by parent_ref and answering it by an edge.
fn thousand() -> Vec<BatchLine> {
    let mut lines = Vec::new();
    let mut n = 0u32;
    while lines.len() < 1000 {
        n += 1;
        let q = format!("q{n}");
        lines.push(node(
            lines.len() as u32 + 1,
            Some(&q),
            add("question", &format!("Question number {n}")),
            None,
        ));
        for a in 0..4 {
            if lines.len() >= 1000 {
                break;
            }
            let r = format!("a{n}_{a}");
            lines.push(node(
                lines.len() as u32 + 1,
                Some(&r),
                add("option", &format!("Answer {a} to question {n}")),
                Some(&q),
            ));
            if lines.len() < 1000 {
                lines.push(edge(
                    lines.len() as u32 + 1,
                    (None, Some(&r)),
                    "answers",
                    (None, Some(&q)),
                ));
            }
        }
    }
    lines
}

#[test]
fn a_thousand_line_batch_writes_each_file_once_and_maps_every_ref() {
    let mut w = world();
    let done = run(&mut w, thousand(), false).unwrap();
    let sets = w.files.sets.lock().unwrap().clone();
    assert_eq!(
        sets,
        vec![vec!["nodes.jsonl".to_owned(), "edges.jsonl".to_owned()]]
    );
    let added = done.rows.iter().filter(|r| r.op == "node add").count();
    assert_eq!(done.refs.len(), added);
    assert!(done.refs.iter().all(|(_, id)| id.is_some()));
    let graphs = w.svc.graph_list();
    assert_eq!(graphs[0].nodes, added);
    // Each answer has its parent edge and its answers edge.
    assert_eq!(
        graphs[0].edges,
        done.rows.iter().filter(|r| r.parent_edge.is_some()).count() * 2
    );
    // Embedding is queued once per touched node, after the write.
    assert_eq!(w.svc.take_embed_jobs().len(), added);
}

#[test]
fn a_bad_line_900_changes_nothing_and_every_bad_line_is_named() {
    let mut w = world();
    let before = (disk(&w), memory(&w));
    let mut lines = thousand();
    lines[899] = node(900, None, add("Not Snake", "bad"), None);
    lines[949] = edge(950, (None, Some("nope")), "answers", (None, Some("q1")));
    let errors = run(&mut w, lines, false).unwrap_err();
    assert_eq!(
        codes(&errors),
        [
            ("batch_failed", None),
            ("name_invalid", Some(900)),
            ("batch_ref_unknown", Some(950))
        ]
    );
    assert_eq!(errors[0].slot("failed").as_deref(), Some("2"));
    assert_eq!((disk(&w), memory(&w)), before);
    assert!(w.files.sets.lock().unwrap().is_empty());
    assert!(w.svc.take_embed_jobs().is_empty());
}

#[test]
fn a_failed_write_rolls_memory_back_and_queues_nothing() {
    let mut w = world();
    let first = run(
        &mut w,
        vec![node(1, Some("p"), add("problem", "Kept"), None)],
        false,
    )
    .unwrap()
    .refs[0]
        .1
        .unwrap();
    w.svc.take_embed_jobs();
    let before = (disk(&w), memory(&w));
    w.fake.fail_writes(true);
    let text = first.to_text();
    let errors = run(
        &mut w,
        vec![
            node(1, Some("n"), add("risk", "Lost"), None),
            edit(
                2,
                (Some(&text), None),
                NodeEditRequest {
                    title: Some("Kept renamed".into()),
                    ..Default::default()
                },
            ),
            edge(3, (None, Some("n")), "blocks", (Some(&text), None)),
        ],
        false,
    )
    .unwrap_err();
    assert_eq!(codes(&errors), [("write_failed", None)]);
    w.fake.fail_writes(false);
    assert_eq!((disk(&w), memory(&w)), before);
    assert!(w.svc.take_embed_jobs().is_empty());
    w.svc.loaded_graph("demo").unwrap().check_rows();
}

#[test]
fn a_crash_at_any_point_of_the_one_write_leaves_all_or_none_on_reopen() {
    for (point, stands) in [
        (CrashPoint::AfterTemps, false),
        (CrashPoint::AfterMarker, true),
        (CrashPoint::AfterFirstRename, true),
    ] {
        let mut w = world();
        w.fake.crash_at_for_tests(Some(point));
        assert!(
            run(&mut w, thousand()[..30].to_vec(), false).is_err(),
            "{point:?}"
        );
        let reopened = world_on(Arc::new(w.fake.restart_for_tests()), false);
        let g = reopened.svc.loaded_graph("demo").unwrap();
        let (nodes, edges) = (g.node_count(), g.edge_count());
        if stands {
            assert!(nodes > 0 && edges > 0, "{point:?}: {nodes} {edges}");
        } else {
            assert_eq!((nodes, edges), (0, 0), "{point:?}");
        }
    }
}

#[test]
fn the_same_ops_as_single_verbs_and_as_one_batch_give_the_same_files() {
    let mut singles = world();
    let mut batched = world();
    let mut ids = std::collections::BTreeMap::<String, String>::new();
    for l in thousand()[..40].iter().cloned() {
        match l.op {
            BatchOp::NodeAdd {
                ref_name,
                mut req,
                parent_ref,
                ..
            } => {
                req.parent = parent_ref.map(|r| ids[&r].clone());
                let n = singles.svc.node_add(&ctx(), req).unwrap().value.node.id;
                ids.insert(ref_name.unwrap(), n.to_text());
            }
            BatchOp::EdgeAdd {
                from_ref,
                relation,
                to_ref,
                ..
            } => {
                let (f, t) = (&ids[&from_ref.unwrap()], &ids[&to_ref.unwrap()]);
                singles.svc.edge_add(&ctx(), f, &relation, t, &[]).unwrap();
            }
            BatchOp::NodeEdit { .. }
            | BatchOp::NodeNote { .. }
            | BatchOp::NodeStatus { .. }
            | BatchOp::EdgeDelete { .. }
            | BatchOp::EdgeEdit { .. }
            | BatchOp::EvidenceAdd { .. }
            | BatchOp::EvidenceRemove { .. } => {
                unreachable!()
            }
        }
    }
    run(&mut batched, thousand()[..40].to_vec(), false).unwrap();
    // The fakes draw the same ids in the same order and the clock stands still, so the files are equal outright.
    assert_eq!(disk(&batched), disk(&singles));
    assert_eq!(memory(&batched), memory(&singles));

    // F75: edge edits and deletes, as single verbs and as one batch, give the same files too (one code path).
    let edges: Vec<String> = singles
        .svc
        .edge_list(&ctx(), EdgeListRequest::default())
        .unwrap()
        .value
        .rows
        .iter()
        .map(|r| r.edge.id.expect("stored edge").to_text())
        .collect();
    let edit = |relation: Option<&str>, reverse| EdgeEditRequest {
        relation: relation.map(str::to_owned),
        reverse,
        ..Default::default()
    };
    let fixes = [
        (0, Some(edit(Some("supports"), false))),
        (1, Some(edit(None, true))),
        (2, None),
        (3, Some(edit(Some("depends_on"), true))),
        (4, None),
    ];
    let mut lines = Vec::new();
    for (i, (at, fix)) in fixes.into_iter().enumerate() {
        let edge = edges[at].clone();
        let op = match fix {
            Some(req) => {
                singles.svc.edge_edit(&ctx(), &edge, &req).unwrap();
                BatchOp::EdgeEdit { edge, req }
            }
            None => {
                singles.svc.edge_delete(&ctx(), &edge).unwrap();
                BatchOp::EdgeDelete { edge }
            }
        };
        lines.push(BatchLine {
            line: i as u32 + 1,
            op,
        });
    }
    let done = run(&mut batched, lines, false).unwrap();
    assert_eq!(done.rows.len(), 5);
    assert_eq!(disk(&batched), disk(&singles));
    assert_eq!(memory(&batched), memory(&singles));
}

#[test]
fn a_dry_run_writes_nothing_draws_no_id_and_reports_what_the_real_run_does() {
    let mut w = world();
    let existing = w
        .svc
        .node_add(&ctx(), add("question", "Which store do we use"))
        .unwrap()
        .value
        .node
        .id
        .to_text();
    w.svc.take_embed_jobs();
    w.files.sets.lock().unwrap().clear();
    w.ids.drawn.store(0, Ordering::SeqCst);
    let before = (disk(&w), memory(&w));
    let lines = vec![
        node(
            1,
            Some("q"),
            add("question", "Which store do we use?"),
            None,
        ),
        node(3, Some("a"), add("answer", "Files"), Some("q")),
        edge(4, (None, Some("a")), "answers", (Some(&existing), None)),
        edit(
            5,
            (None, Some("q")),
            NodeEditRequest {
                note: Some("why".into()),
                ..Default::default()
            },
        ),
        node(6, None, add("question", "Which store do we use??"), None),
        node(7, None, add("risk", "Datastore choice for the graph"), None),
        node(
            8,
            None,
            add("risk", "Datastore choice for the graph!"),
            None,
        ),
    ];
    let dry = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                reason: None,
                lines: lines.clone(),
                dry_run: true,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap();
    assert_eq!((disk(&w), memory(&w)), before);
    assert!(w.files.sets.lock().unwrap().is_empty());
    assert_eq!(w.ids.drawn.load(Ordering::SeqCst), 0);
    assert!(w.svc.take_embed_jobs().is_empty());
    assert!(dry.value.dry_run);
    assert!(dry.value.rows.iter().all(|r| r.id.is_none()));
    assert!(dry.value.refs.iter().all(|(_, id)| id.is_none()));

    let real = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                reason: None,
                lines,
                dry_run: false,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap();
    let shape = |d: &umx_services::Done<BatchDone>| {
        let rows: Vec<_> = d
            .value
            .rows
            .iter()
            .map(|r| (r.line, r.op, r.ref_name.clone(), r.changed.clone()))
            .collect();
        let warnings: Vec<_> = d.notes.iter().map(|p| (p.code, p.line)).collect();
        (rows, warnings)
    };
    assert_eq!(shape(&dry), shape(&real));
    // Near duplicates against the graph and against an earlier line, each at its line; in the dry run a candidate
    // the batch made is named by its line, never by an id that will not exist. A best match an earlier line adds is
    // near_duplicate_line, naming that line (ws004 s30), in the dry run and the real run alike.
    let near: Vec<_> = dry
        .notes
        .iter()
        .filter(|p| p.code.starts_with("near_duplicate"))
        .collect();
    assert_eq!(
        near.iter().map(|p| (p.code, p.line)).collect::<Vec<_>>(),
        [
            ("near_duplicate", Some(1)),
            ("near_duplicate", Some(6)),
            ("near_duplicate_line", Some(8))
        ]
    );
    assert_eq!(near[0].candidates[0].id, existing);
    assert!(near[1].candidates.iter().any(|c| c.id == "line:1"));
    assert_eq!(near[2].slot("candidate line").as_deref(), Some("7"));
    assert_eq!(near[2].slot("candidate type").as_deref(), Some("risk"));
    assert_eq!(
        near[2].slot("fix").as_deref(),
        Some("umx batch --file <file> --dry-run")
    );
    assert_eq!(near[2].candidates[0].id, "line:7");
    assert!(
        near.iter()
            .flat_map(|p| &p.candidates)
            .all(|c| c.title.is_none())
    );
    assert!(near.iter().all(|p| p.subject.is_none()));
    let real_near: Vec<_> = real
        .notes
        .iter()
        .filter(|p| p.code.starts_with("near_duplicate"))
        .collect();
    assert_eq!(real_near[2].code, "near_duplicate_line");
    let first = real_near[2].candidates[0].id.parse::<Id>().unwrap();
    assert_eq!(
        real_near[2].slot("fix"),
        Some(format!("umx node get umx:demo/{first}"))
    );
    assert!(real_near.iter().all(|p| p.subject.is_some()));
    assert_eq!(w.files.sets.lock().unwrap().len(), 1);
}

/// R107-1 on the batch path (ws004 s15): a title with its words joined warns `near_duplicate` against a graph node
/// at 0.885 (the reviewer's split-word probe) and against an earlier line, as s06's scan did.
#[test]
fn joined_words_warn_near_duplicate_in_a_batch() {
    let mut w = world();
    let existing = w
        .svc
        .node_add(&ctx(), add("problem", "customer approval workflow"))
        .unwrap()
        .value
        .node
        .id
        .to_text();
    assert!(
        umx_graph::title_score("customerapprovalworkflow", "customer approval workflow") >= 0.85
    );
    assert!(umx_graph::title_score("invoicedisputequeue", "invoice dispute queue") >= 0.85);
    for dry_run in [true, false] {
        let lines = vec![
            node(1, None, add("problem", "customerapprovalworkflow"), None),
            node(2, None, add("problem", "invoice dispute queue"), None),
            node(3, None, add("problem", "invoicedisputequeue"), None),
        ];
        let done = w
            .svc
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
            .unwrap();
        let near: Vec<_> = done
            .notes
            .iter()
            .filter(|p| p.code.starts_with("near_duplicate"))
            .map(|p| (p.line, p.candidates[0].id.clone()))
            .collect();
        let line_2 = if dry_run {
            "line:2".to_owned()
        } else {
            done.value.rows[1].id.unwrap().to_text()
        };
        assert_eq!(
            near,
            [(Some(1), existing.clone()), (Some(3), line_2)],
            "dry run {dry_run}"
        );
    }
}

#[test]
fn refs_are_defined_once_on_an_earlier_line_and_never_reach_a_real_node() {
    let mut w = world();
    // A real node titled like a ref: the ref is a separate namespace, so it is never found.
    w.svc.node_add(&ctx(), add("problem", "q1")).unwrap();
    let before = (disk(&w), memory(&w));
    let errors = run(
        &mut w,
        vec![
            edge(1, (None, Some("q1")), "answers", (None, Some("later"))),
            node(2, Some("later"), add("question", "Later"), None),
            node(3, Some("later"), add("question", "Again"), None),
            node(4, Some("brk_1"), add("Bad Type", "Broken"), None),
            edge(5, (None, Some("brk_1")), "answers", (None, Some("later"))),
            node(6, Some("bad.ref"), add("question", "Bad ref"), None),
            node(7, Some("Upper"), add("question", "Bad ref"), None),
            edge(
                8,
                (Some("01JC8X"), Some("later")),
                "answers",
                (None, Some("later")),
            ),
            edge(9, (None, None), "answers", (None, Some("later"))),
        ],
        false,
    )
    .unwrap_err();
    assert_eq!(
        codes(&errors),
        [
            ("batch_failed", None),
            ("batch_ref_unknown", Some(1)),
            ("batch_ref_repeated", Some(3)),
            ("name_invalid", Some(4)),
            ("batch_ref_unknown", Some(5)),
            ("batch_line_invalid", Some(6)),
            ("batch_line_invalid", Some(7)),
            ("batch_line_invalid", Some(8)),
            ("batch_line_invalid", Some(9)),
        ]
    );
    assert_eq!(errors[1].subject.as_deref(), Some("q1"));
    assert_eq!(
        errors[1].slot("why").as_deref(),
        Some("no earlier line defines")
    );
    assert_eq!(
        errors[4].slot("why").as_deref(),
        Some("was defined by a line that failed")
    );
    assert!(
        errors[5]
            .slot("reason")
            .unwrap()
            .contains("a ref holds only lowercase a-z 0-9 _ and -")
    );
    // The single verb's retry is no fix inside a batch: the next is the batch's dry run.
    assert_eq!(
        errors[3].slot("fix").as_deref(),
        Some("umx batch --file <file> --dry-run")
    );
    assert!(errors[7].slot("reason").unwrap().contains("not both"));
    assert_eq!(
        errors[8].slot("reason").as_deref(),
        Some("give from or from_ref")
    );
    assert_eq!((disk(&w), memory(&w)), before);
}

#[test]
fn errors_stop_at_the_cap_with_the_total_and_too_many_ops_is_refused_unread() {
    let mut w = world();
    let lines: Vec<BatchLine> = (1..=150)
        .map(|i| node(i, None, add("Bad", "x"), None))
        .collect();
    let errors = run(&mut w, lines, false).unwrap_err();
    assert_eq!(errors.len(), MAX_BATCH_ERRORS + 2);
    assert_eq!(errors[0].slot("failed").as_deref(), Some("150"));
    let last = errors.last().unwrap();
    assert_eq!(
        (
            last.code,
            last.slot("failed").as_deref(),
            last.slot("shown").as_deref()
        ),
        ("batch_errors_truncated", Some("150"), Some("100"))
    );

    let lines: Vec<BatchLine> = (1..=MAX_BATCH_OPS as u32 + 1)
        .map(|i| node(i, None, add("problem", "x"), None))
        .collect();
    let errors = run(&mut w, lines, false).unwrap_err();
    assert_eq!(codes(&errors), [("batch_too_large", None)]);
    assert_eq!(w.ids.drawn.load(Ordering::SeqCst), 0);
    assert!(w.files.sets.lock().unwrap().is_empty());
}

/// Every rejection the single verbs make is made by a batch too, with the same code (one code path, security s07).
#[test]
fn the_single_verbs_rejections_are_the_batchs_rejections() {
    let mut w = world();
    let real = w
        .svc
        .node_add(&ctx(), add("problem", "Real"))
        .unwrap()
        .value
        .node
        .id
        .to_text();
    let set = |f: &str, v: &str| vec![(f.to_owned(), SetValue::Raw(v.to_owned()))];
    let node_adds = vec![
        add("Bad Type", "t"),
        NodeAddRequest {
            status: Some("Not ok".into()),
            ..add("problem", "t")
        },
        NodeAddRequest {
            sets: set("id", "x"),
            ..add("problem", "t")
        },
        NodeAddRequest {
            sets: set("Bad Field", "x"),
            ..add("problem", "t")
        },
        NodeAddRequest {
            parent: Some("01ZZZZZZZZ".into()),
            ..add("problem", "t")
        },
        NodeAddRequest {
            evidence: vec!["not a pointer".into()],
            ..add("problem", "t")
        },
        NodeAddRequest {
            evidence: vec!["https://example.com/x".into()],
            evidence_kind: Some("telepathy".into()),
            ..add("problem", "t")
        },
    ];
    let edits = vec![
        NodeEditRequest {
            kind: Some("Bad Type".into()),
            ..Default::default()
        },
        NodeEditRequest {
            sets: set("status", "done"),
            ..Default::default()
        },
        NodeEditRequest {
            unsets: vec!["created".into()],
            ..Default::default()
        },
        NodeEditRequest {
            sets: set("Bad Field", "x"),
            ..Default::default()
        },
    ];
    type EdgeCase<'a> = (&'a str, &'a str, &'a str, Vec<(String, SetValue)>);
    let edges: Vec<EdgeCase> = vec![
        (real.as_str(), "Bad Rel", real.as_str(), vec![]),
        // similar_to is made only by `umx similarity rebuild` (ws004 s09).
        (real.as_str(), "similar_to", real.as_str(), vec![]),
        (real.as_str(), "blocks", "01ZZZZZZZZ", vec![]),
        (real.as_str(), "blocks", real.as_str(), set("from", "x")),
        (
            real.as_str(),
            "blocks",
            real.as_str(),
            set("Bad Field", "x"),
        ),
    ];
    let mut cases = 0;
    for req in node_adds {
        let single = w.svc.node_add(&ctx(), req.clone()).unwrap_err().code;
        let batch = run(&mut w, vec![node(7, None, req, None)], false).unwrap_err();
        assert_eq!(codes(&batch[1..]), [(single, Some(7))]);
        cases += 1;
    }
    for req in edits {
        let single = w
            .svc
            .node_edit(&ctx(), &real, req.clone())
            .unwrap_err()
            .code;
        let batch = run(&mut w, vec![edit(7, (Some(&real), None), req)], false).unwrap_err();
        assert_eq!(codes(&batch[1..]), [(single, Some(7))]);
        cases += 1;
    }
    for (from, rel, to, sets) in edges {
        let single = w
            .svc
            .edge_add(&ctx(), from, rel, to, &sets)
            .unwrap_err()
            .code;
        let mut l = edge(7, (Some(from), None), rel, (Some(to), None));
        if let BatchOp::EdgeAdd { sets: s, .. } = &mut l.op {
            *s = sets;
        }
        let batch = run(&mut w, vec![l], false).unwrap_err();
        assert_eq!(codes(&batch[1..]), [(single, Some(7))]);
        cases += 1;
    }
    assert_eq!(cases, 16);
    assert!(
        w.files.sets.lock().unwrap().len() == 1,
        "only the setup's add wrote"
    );
}

#[test]
fn edge_add_existing_fields_match_earlier_refs_and_persisted_edges_without_writes() {
    let mut w = world();
    let values = || vec![("why".into(), SetValue::Str("same_value".into()))];
    let first = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                reason: None,
                echo_sentences: false,
                dry_run: false,
                lines: vec![
                    node(1, Some("a"), add("problem", "A"), None),
                    node(2, Some("b"), add("decision", "B"), None),
                    edge_sets(
                        edge(3, (None, Some("a")), "blocks", (None, Some("b"))),
                        values(),
                    ),
                    edge_sets(
                        edge(4, (None, Some("a")), "blocks", (None, Some("b"))),
                        values(),
                    ),
                ],
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap();
    assert!(!first.value.rows[2].existed);
    assert!(first.value.rows[3].existed);
    assert_eq!(first.value.rows[2].id, first.value.rows[3].id);
    assert!(
        first
            .notes
            .iter()
            .any(|p| p.code == "edge_exists" && p.line == Some(4))
    );
    let (a, b) = (
        first.value.rows[0].id.unwrap().to_text(),
        first.value.rows[1].id.unwrap().to_text(),
    );
    let before = (disk(&w), memory(&w));
    w.files.sets.lock().unwrap().clear();
    w.ids.drawn.store(0, Ordering::SeqCst);
    for dry_run in [true, false] {
        let done = w
            .svc
            .batch(
                &ctx(),
                BatchRequest {
                    idempotency: None,
                    raised_in: None,
                    reason: None,
                    echo_sentences: false,
                    dry_run,
                    lines: vec![edge_sets(
                        edge(7, (Some(&a), None), "blocks", (Some(&b), None)),
                        values(),
                    )],
                    refs: Default::default(),
                    refs_graph: None,
                },
            )
            .unwrap();
        assert!(done.value.rows[0].existed);
        assert_eq!(done.value.rows[0].id, first.value.rows[2].id);
        assert_eq!(codes(&done.notes), [("edge_exists", Some(7))]);
        assert_eq!((disk(&w), memory(&w)), before);
    }
    assert!(w.files.sets.lock().unwrap().is_empty());
    assert_eq!(w.ids.drawn.load(Ordering::SeqCst), 0);
}

#[test]
fn edge_add_existing_refuses_changed_or_missing_fields_at_the_same_line_and_rolls_back() {
    let mut w = world();
    let first = run(
        &mut w,
        vec![
            node(1, Some("a"), add("problem", "A"), None),
            node(2, Some("b"), add("decision", "B"), None),
            edge_sets(
                edge(3, (None, Some("a")), "blocks", (None, Some("b"))),
                vec![("why".into(), SetValue::Str("stored_private_marker".into()))],
            ),
        ],
        false,
    )
    .unwrap();
    let (a, b) = (
        first.rows[0].id.unwrap().to_text(),
        first.rows[1].id.unwrap().to_text(),
    );
    let before = (disk(&w), memory(&w));
    w.files.sets.lock().unwrap().clear();
    for field in ["why", "missing"] {
        let mut dry_errors = None;
        for dry_run in [true, false] {
            let errors = run(
                &mut w,
                vec![
                    edit(
                        2,
                        (Some(&a), None),
                        NodeEditRequest {
                            title: Some("Earlier edit must roll back".into()),
                            ..Default::default()
                        },
                    ),
                    edge(4, (Some(&b), None), "blocks", (Some(&a), None)),
                    edge_sets(
                        edge(7, (Some(&a), None), "blocks", (Some(&b), None)),
                        vec![(
                            field.into(),
                            SetValue::Str("requested_private_marker".into()),
                        )],
                    ),
                ],
                dry_run,
            )
            .unwrap_err();
            assert_eq!(
                codes(&errors),
                [("batch_failed", None), ("edge_exists_differs", Some(7))]
            );
            assert_eq!(
                errors[1].slot("id"),
                first.rows[2].id.map(|id| id.to_text())
            );
            assert_eq!(errors[1].slot("fields").as_deref(), Some(field));
            // plan 205: the next is the in-place edit F134 added, naming the first field; never its value.
            assert_eq!(
                errors[1].slot("fix"),
                Some(format!(
                    "umx edge edit {} --set {field}=<value>",
                    first.rows[2].id.unwrap().to_text()
                ))
            );
            assert!(!format!("{errors:?}").contains("private_marker"));
            if dry_run {
                dry_errors = Some(errors);
            } else {
                assert_eq!(Some(errors), dry_errors);
            }
            assert_eq!((disk(&w), memory(&w)), before);
        }
    }
    assert!(w.files.sets.lock().unwrap().is_empty());
}

#[test]
fn edge_add_existing_compares_pending_out_of_order_fields_in_dry_and_real_runs() {
    let mut w = world();
    let before = (disk(&w), memory(&w));
    for explicit in [None, Some("00000000000000000000000001")] {
        let mut dry_errors = None;
        for dry_run in [true, false] {
            let mut first = edge_sets(
                edge(3, (None, Some("a")), "blocks", (None, Some("b"))),
                vec![("why".into(), SetValue::Str("stored_private_marker".into()))],
            );
            if let BatchOp::EdgeAdd { id, .. } = &mut first.op {
                *id = explicit.map(Into::into);
            }
            let errors = run(
                &mut w,
                vec![
                    node(1, Some("a"), add("problem", "A"), None),
                    node(2, Some("b"), add("decision", "B"), None),
                    first,
                    edge_sets(
                        edge(4, (None, Some("a")), "blocks", (None, Some("b"))),
                        vec![(
                            "why".into(),
                            SetValue::Str("requested_private_marker".into()),
                        )],
                    ),
                ],
                dry_run,
            )
            .unwrap_err();
            assert_eq!(
                codes(&errors),
                [("batch_failed", None), ("edge_exists_differs", Some(4))]
            );
            assert_eq!(errors[1].slot("fields").as_deref(), Some("why"));
            if explicit.is_none() {
                assert_eq!(errors[1].slot("id").as_deref(), Some("<id>"));
                assert!(errors[1].subject.is_none());
            }
            assert!(!format!("{errors:?}").contains("private_marker"));
            if dry_run {
                dry_errors = Some(errors);
            } else {
                assert_eq!(Some(errors), dry_errors);
            }
            assert_eq!((disk(&w), memory(&w)), before);
        }
    }
    assert!(w.files.sets.lock().unwrap().is_empty());
}

#[test]
fn edge_add_existing_keeps_missing_selectors_distinct_from_stored_passage_selectors() {
    for selector in ["from_selector", "to_selector"] {
        let mut w = world();
        let first = run(
            &mut w,
            vec![
                node(1, Some("a"), add("problem", "A"), None),
                node(2, Some("b"), add("decision", "B"), None),
                edge(3, (None, Some("a")), "blocks", (None, Some("b"))),
            ],
            false,
        )
        .unwrap();
        let (a, b) = (
            first.rows[0].id.unwrap().to_text(),
            first.rows[1].id.unwrap().to_text(),
        );
        w.svc.graph_unload(&ctx(), "demo").unwrap();
        let bytes = w.fake.read(&dir(), GraphFile::Edges).unwrap().unwrap();
        let passage_id = first.rows[2].id.unwrap().to_text();
        let mut image = Vec::new();
        for line in bytes.split_inclusive(|byte| *byte == b'\n') {
            let mut passage: serde_json::Value = serde_json::from_slice(line).unwrap();
            if passage["id"] == passage_id {
                passage[selector] = serde_json::json!("L1-L4");
                image.extend_from_slice(&serde_json::to_vec(&passage).unwrap());
                image.push(b'\n');
            } else {
                image.extend_from_slice(line);
            }
        }
        w.fake.tamper_for_tests(&dir(), GraphFile::Edges, &image);
        w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
        let whole = run(
            &mut w,
            vec![edge(7, (Some(&a), None), "blocks", (Some(&b), None))],
            false,
        )
        .unwrap();
        assert!(!whole.rows[0].existed);
        assert_ne!(whole.rows[0].id, first.rows[2].id);
        let again = w.svc.edge_add(&ctx(), &a, "blocks", &b, &[]).unwrap();
        assert!(again.value.existed);
        assert_eq!(Some(again.value.edge.id), whole.rows[0].id);
        let again = run(
            &mut w,
            vec![edge(8, (Some(&a), None), "blocks", (Some(&b), None))],
            false,
        )
        .unwrap();
        assert!(again.rows[0].existed);
        assert_eq!(again.rows[0].id, whole.rows[0].id);
        assert_eq!(
            w.svc
                .loaded_graph("demo")
                .unwrap()
                .edges()
                .filter(|edge| edge.kind() == "blocks")
                .count(),
            2
        );
    }
}

#[test]
fn an_existing_edge_is_reported_not_added_whether_the_graph_or_an_earlier_line_made_it() {
    let mut w = world();
    let a = w
        .svc
        .node_add(&ctx(), add("problem", "A"))
        .unwrap()
        .value
        .node
        .id
        .to_text();
    let b = w
        .svc
        .node_add(&ctx(), add("problem", "B"))
        .unwrap()
        .value
        .node
        .id
        .to_text();
    w.svc.edge_add(&ctx(), &a, "blocks", &b, &[]).unwrap();
    let done = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                reason: None,
                lines: vec![
                    edge(1, (Some(&a), None), "blocks", (Some(&b), None)),
                    edge(2, (Some(&b), None), "blocks", (Some(&a), None)),
                    edge(3, (Some(&b), None), "blocks", (Some(&a), None)),
                ],
                dry_run: false,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap();
    let existed: Vec<bool> = done.value.rows.iter().map(|r| r.existed).collect();
    assert_eq!(existed, [true, false, true]);
    assert_eq!(done.value.rows[1].id, done.value.rows[2].id);
    let warned: Vec<_> = done.notes.iter().map(|p| (p.code, p.line)).collect();
    assert_eq!(warned, [("edge_exists", Some(1)), ("edge_exists", Some(3))]);
    assert_eq!(w.svc.graph_list()[0].edges, 2);
}

/// Review R103-3: a line refused by a single verb's check points at the batch, never at a single verb run against
/// the graph with the line's values (here `use_status_verb` on a node that already exists).
#[test]
fn a_line_error_never_suggests_a_single_verb_with_the_lines_values() {
    let mut w = world();
    let real = w
        .svc
        .node_add(&ctx(), add("problem", "Real"))
        .unwrap()
        .value
        .node
        .id
        .to_text();
    let errors = run(
        &mut w,
        vec![edit(
            3,
            (Some(&real), None),
            NodeEditRequest {
                sets: vec![("status".into(), SetValue::Raw("status_from_batch".into()))],
                ..Default::default()
            },
        )],
        false,
    )
    .unwrap_err();
    assert_eq!(codes(&errors[1..]), [("use_status_verb", Some(3))]);
    assert_eq!(
        errors[1].slot("fix").as_deref(),
        Some("umx batch --file <file> --dry-run")
    );
    for p in &errors {
        for (_, slot) in &p.slots {
            assert!(!slot.to_string().contains("status_from_batch"), "{p:?}");
        }
    }
}

/// Review R103-1: warnings stop at 100 with the total, and a batch whose reply could not fit the client's frame is
/// refused before anything is written, dry run or not.
#[test]
fn warnings_are_capped_and_an_oversized_reply_is_refused_before_writing() {
    let mut w = world();
    let lines: Vec<BatchLine> = (1..=150)
        .map(|i| node(i, None, add("widget", &format!("Widget number {i}")), None))
        .collect();
    let done = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                reason: None,
                lines,
                dry_run: false,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap();
    assert_eq!(done.notes.len(), 101);
    let last = done.notes.last().unwrap();
    // 150 off-vocabulary types plus the near duplicates the similar titles raise.
    assert_eq!(
        (last.code, last.slot("shown").as_deref()),
        ("batch_warnings_truncated", Some("100"))
    );
    assert!(last.slot("warned").unwrap().parse::<u32>().unwrap() >= 150);

    // Each line changes 100 long field names, so `changed` alone is about 12.6 MB: over the reply bound.
    let target = w
        .svc
        .node_add(&ctx(), add("problem", "Target"))
        .unwrap()
        .value
        .node
        .id
        .to_text();
    let fields: Vec<String> = (0..100)
        .map(|f| format!("f{f:02}_{}", "x".repeat(58)))
        .collect();
    let lines = |n: u32| -> Vec<BatchLine> {
        (1..=n)
            .map(|i| {
                let sets = fields
                    .iter()
                    .map(|f| (f.clone(), SetValue::Raw(i.to_string())))
                    .collect();
                edit(
                    i,
                    (Some(&target), None),
                    NodeEditRequest {
                        sets,
                        ..Default::default()
                    },
                )
            })
            .collect()
    };
    w.files.sets.lock().unwrap().clear();
    let before = (disk(&w), memory(&w));
    for dry_run in [true, false] {
        let errors = w
            .svc
            .batch(
                &ctx(),
                BatchRequest {
                    idempotency: None,
                    raised_in: None,
                    echo_sentences: false,
                    reason: None,
                    lines: lines(2000),
                    dry_run,
                    refs: Default::default(),
                    refs_graph: None,
                },
            )
            .unwrap_err();
        assert_eq!(codes(&errors), [("batch_too_large", None)]);
        assert_eq!(errors[0].slot("what").as_deref(), Some("reply bytes"));
        assert_eq!((disk(&w), memory(&w)), before);
    }
    assert!(w.files.sets.lock().unwrap().is_empty());
    // The positive control: the same lines, fewer of them, apply.
    run(&mut w, lines(100), false).unwrap();
}

/// D49 through a batch, as through `edge add`: a symmetric relation is stored once with the lower id as `from`, and
/// either direction of one already there, in the graph or from an earlier line, is that edge.
#[test]
fn a_symmetric_edge_is_stored_once_whichever_way_a_line_gives_it() {
    let mut w = world();
    w.svc
        .ontology_add_relation(&ctx(), "pairs_with", true, None)
        .unwrap();
    let a = w
        .svc
        .node_add(&ctx(), add("problem", "A"))
        .unwrap()
        .value
        .node
        .id;
    let b = w
        .svc
        .node_add(&ctx(), add("problem", "B"))
        .unwrap()
        .value
        .node
        .id;
    let c = w
        .svc
        .node_add(&ctx(), add("problem", "C"))
        .unwrap()
        .value
        .node
        .id;
    let (at, bt, ct) = (a.to_text(), b.to_text(), c.to_text());
    // Stored by the single verb given high to low: the batch's reverse finds it.
    w.svc.edge_add(&ctx(), &bt, "pairs_with", &at, &[]).unwrap();
    let done = run(
        &mut w,
        vec![
            edge(1, (Some(&at), None), "pairs_with", (Some(&bt), None)),
            edge(2, (Some(&ct), None), "pairs_with", (Some(&at), None)),
            edge(3, (Some(&at), None), "pairs_with", (Some(&ct), None)),
        ],
        false,
    )
    .unwrap();
    let existed: Vec<bool> = done.rows.iter().map(|r| r.existed).collect();
    assert_eq!(existed, [true, false, true]);
    assert_eq!(done.rows[1].id, done.rows[2].id);
    let g = w.svc.loaded_graph("demo").unwrap();
    assert_eq!(w.svc.graph_list()[0].edges, 2);
    let made = g.edge(done.rows[1].id.unwrap()).unwrap();
    assert_eq!((made.from(), made.to()), (a, c), "the lower id is from");
}

#[test]
fn a_batch_edge_line_checks_its_fields_against_the_relation_as_edge_add_does() {
    // F92: a declared relation field types the value (field_kind_mismatch when it does not fit), an undeclared one
    // warns off_vocabulary_edge_field, each at its line; the stored edges are the single verb's.
    let mut w = world();
    let id = |w: &mut World, title: &str| {
        w.svc
            .node_add(&ctx(), add("problem", title))
            .unwrap()
            .value
            .node
            .id
            .to_text()
    };
    let (a, b, c) = (id(&mut w, "Alpha"), id(&mut w, "Beta"), id(&mut w, "Gamma"));
    w.svc
        .ontology_add_relation_field(&ctx(), "supports", "confidence:float")
        .unwrap();
    let with = |line, from: &str, to: &str, sets: &[(&str, &str)]| {
        let mut l = edge(line, (Some(from), None), "supports", (Some(to), None));
        if let BatchOp::EdgeAdd { sets: s, .. } = &mut l.op {
            *s = sets
                .iter()
                .map(|(f, v)| ((*f).to_owned(), SetValue::Raw((*v).to_owned())))
                .collect();
        }
        l
    };
    let done = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                reason: None,
                lines: vec![
                    with(3, &a, &b, &[("confidence", "0.7")]),
                    with(4, &b, &c, &[("confidence", "high"), ("why", "synthetic")]),
                ],
                dry_run: false,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap();
    let warned: Vec<_> = done.notes.iter().map(|p| (p.code, p.line)).collect();
    assert_eq!(
        warned,
        [
            ("field_kind_mismatch", Some(4)),
            ("off_vocabulary_edge_field", Some(4))
        ]
    );
    let off = &done.notes[1];
    assert_eq!(
        (
            off.slot("relation").as_deref(),
            off.slot("field").as_deref()
        ),
        (Some("supports"), Some("why"))
    );
    let edges = String::from_utf8(
        w.files
            .inner
            .read(&dir(), GraphFile::Edges)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert!(edges.contains("\"confidence\":0.7"), "{edges}");
    assert!(edges.contains("\"confidence\":\"high\""), "{edges}");

    // F92 at F75's edge edit: retyping an edge warns for each custom field the new relation does not declare.
    let edge_id = |w: &World, from: &str| {
        w.svc
            .edge_list(
                &ctx(),
                EdgeListRequest {
                    from: Some(from.to_owned()),
                    ..Default::default()
                },
            )
            .unwrap()
            .value
            .rows[0]
            .edge
            .id
            .expect("stored edge")
            .to_text()
    };
    let (ab, bc) = (edge_id(&w, &a), edge_id(&w, &b));
    let retype = |line, edge: String| BatchLine {
        line,
        op: BatchOp::EdgeEdit {
            edge,
            req: EdgeEditRequest {
                relation: Some("blocks".into()),
                reverse: false,
                ..Default::default()
            },
        },
    };
    let done = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                reason: None,
                lines: vec![retype(5, ab), retype(6, bc)],
                dry_run: false,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap();
    let warned: Vec<_> = done
        .notes
        .iter()
        .map(|p| (p.code, p.line, p.slot("field")))
        .collect();
    let at = |line, field: &str| {
        (
            "off_vocabulary_edge_field",
            Some(line),
            Some(field.to_owned()),
        )
    };
    assert_eq!(
        warned,
        [at(5, "confidence"), at(6, "confidence"), at(6, "why")]
    );
    assert!(
        done.notes
            .iter()
            .all(|p| p.slot("relation").as_deref() == Some("blocks"))
    );
}

/// R208-2 (ws008 E1): duplicate_key warnings are counted and capped with every other batch warning, before the reply
/// bound, and a dry run reports the same ones.
#[test]
fn duplicate_key_warnings_share_the_batch_cap_and_a_dry_run_reports_the_same() {
    let mut w = world();
    let keyed = |title: &str| {
        let mut req = add("problem", title);
        req.sets
            .push(("entity_key".into(), SetValue::Raw("email:a@x.com".into())));
        req
    };
    let winner = w
        .svc
        .node_add(&ctx(), keyed("Winner"))
        .unwrap()
        .value
        .node
        .id;
    let copies: Vec<BatchLine> = (0..101)
        .map(|i| node(i + 1, None, keyed(&format!("Copy number {i}")), None))
        .collect();
    let added = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                reason: None,
                lines: copies,
                dry_run: false,
                echo_sentences: false,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap();
    assert!(added.notes.len() <= MAX_BATCH_WARNINGS + 1);
    let codes = |ps: &[Problem]| ps.iter().map(|p| p.code).collect::<Vec<_>>();
    // One edit of the winner: every copy shares its key, so 101 pairs, over the cap.
    let edit_winner = || {
        vec![edit(
            1,
            (Some(&winner.to_text()), None),
            NodeEditRequest {
                title: Some("Winner, renamed".into()),
                ..Default::default()
            },
        )]
    };
    let dry = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                reason: None,
                lines: edit_winner(),
                dry_run: true,
                echo_sentences: false,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap()
        .notes;
    let real = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                reason: None,
                lines: edit_winner(),
                dry_run: false,
                echo_sentences: false,
                refs: Default::default(),
                refs_graph: None,
            },
        )
        .unwrap()
        .notes;
    for ws in [&dry, &real] {
        assert_eq!(ws.len(), MAX_BATCH_WARNINGS + 1, "{:?}", codes(ws));
        assert_eq!(
            ws.iter().filter(|p| p.code == "duplicate_key").count(),
            MAX_BATCH_WARNINGS
        );
        assert_eq!(ws.last().unwrap().code, "batch_warnings_truncated");
    }
    assert_eq!(
        codes(&dry),
        codes(&real),
        "a dry run reports what the run will"
    );
}

/// ws003 s24: refs an earlier applied batch bound, imported with `--refs-from` (the CLI reads the receipts).
mod refs_from {
    use super::*;
    use umx_core::batch::MAX_IMPORTED_REFS;

    fn import(
        w: &mut World,
        lines: Vec<BatchLine>,
        dry_run: bool,
        refs: &[(&str, &str)],
        graph: Option<Id>,
    ) -> Result<BatchDone, Vec<Problem>> {
        w.svc
            .batch(
                &ctx(),
                BatchRequest {
                    idempotency: None,
                    raised_in: None,
                    reason: None,
                    lines,
                    dry_run,
                    echo_sentences: false,
                    refs: refs
                        .iter()
                        .map(|(n, id)| (n.to_string(), id.to_string()))
                        .collect(),
                    refs_graph: graph.map(Id::to_text),
                },
            )
            .map(|d| d.value)
    }

    /// A first applied batch binding p (a problem) and o (an option): its refs and graph id.
    fn first(w: &mut World) -> (String, String, Id) {
        let done = run(
            w,
            vec![
                node(1, Some("p"), add("problem", "Parent"), None),
                node(2, Some("o"), add("option", "Option"), None),
            ],
            false,
        )
        .unwrap();
        let id = |i: usize| done.refs[i].1.unwrap().to_text();
        (id(0), id(1), done.graph_id)
    }

    #[test]
    fn imported_refs_work_in_every_ref_field_and_receipts_list_only_their_own_refs() {
        let mut w = world();
        let (p, o, graph) = first(&mut w);
        let lines = || {
            vec![
                node(1, Some("c"), add("problem", "Child"), Some("p")),
                edge(2, (None, Some("o")), "supports", (None, Some("p"))),
                edit(
                    3,
                    (None, Some("p")),
                    NodeEditRequest {
                        title: Some("Parent, renamed".into()),
                        ..Default::default()
                    },
                ),
                note_line(4, (None, Some("o")), "More on the option", Vec::new()),
                status_line(5, (None, Some("p")), "agreed", Some("reviewed"), None),
            ]
        };
        let refs = [("p", p.as_str()), ("o", o.as_str())];
        let before = (disk(&w), memory(&w));
        let dry = import(&mut w, lines(), true, &refs, Some(graph)).unwrap();
        assert_eq!((disk(&w), memory(&w)), before, "a dry run writes nothing");
        assert_eq!(dry.refs, [("c".to_owned(), None)]);
        let done = import(&mut w, lines(), false, &refs, Some(graph)).unwrap();
        assert_eq!(done.graph_id, graph);
        assert_eq!(done.refs.len(), 1, "imported refs are never listed again");
        assert_eq!(done.refs[0].0, "c");
        let ids: Vec<Option<String>> = done.rows.iter().map(|r| r.id.map(Id::to_text)).collect();
        assert_eq!(
            ids[2..],
            [Some(p.clone()), Some(o.clone()), Some(p.clone())]
        );
        let parent = w.svc.node_get(&ctx(), &p).unwrap().value.1;
        assert_eq!(parent.title, "Parent, renamed");
        assert_eq!(parent.status, "agreed");
        let g = w.svc.loaded_graph("demo").unwrap();
        let ends = |id: Id| {
            let e = g.edge(id).unwrap().to_edge();
            (e.kind, e.from.to_text(), e.to.to_text())
        };
        assert_eq!(
            ends(done.rows[1].id.unwrap()),
            ("supports".to_owned(), o.clone(), p.clone()),
            "from_ref o and to_ref p"
        );
        let child = done.rows[0].id.unwrap().to_text();
        assert_eq!(
            ends(done.rows[0].parent_edge.unwrap()),
            ("part_of".to_owned(), child, p.clone()),
            "parent_ref p"
        );
    }

    #[test]
    fn the_receipts_graph_id_must_be_the_batchs_and_an_old_receipt_without_one_is_accepted() {
        let mut w = world();
        let (p, _, graph) = first(&mut w);
        let line = || {
            vec![edit(
                1,
                (None, Some("p")),
                NodeEditRequest {
                    title: Some("Renamed".into()),
                    ..Default::default()
                },
            )]
        };
        let refs = [("p", p.as_str())];
        let before = (disk(&w), memory(&w));
        for dry_run in [true, false] {
            let errors = import(&mut w, line(), dry_run, &refs, Some(Id(graph.0 + 1))).unwrap_err();
            assert_eq!(codes(&errors), [("batch_refs_other_graph", None)]);
        }
        assert_eq!((disk(&w), memory(&w)), before);
        let mut raw = BatchRequest {
            idempotency: None,
            raised_in: None,
            reason: None,
            lines: line(),
            dry_run: false,
            echo_sentences: false,
            refs: [("p".to_owned(), p.clone())].into(),
            refs_graph: Some(graph.to_text().to_lowercase()),
        };
        let errors = w.svc.batch(&ctx(), raw.clone()).unwrap_err();
        assert_eq!(codes(&errors), [("usage_invalid", None)]);
        raw.refs_graph = None;
        assert!(
            w.svc.batch(&ctx(), raw).is_ok(),
            "a receipt from before s24 has no graph_id"
        );
        assert!(import(&mut w, line(), false, &refs, Some(graph)).is_ok());
    }

    #[test]
    fn a_ref_both_imported_and_defined_is_refused_at_its_line_never_shadowed() {
        let mut w = world();
        let (p, _, graph) = first(&mut w);
        let before = (disk(&w), memory(&w));
        for dry_run in [true, false] {
            let errors = import(
                &mut w,
                vec![
                    node(1, Some("x"), add("problem", "Fine"), None),
                    node(2, Some("p"), add("problem", "Shadow"), None),
                    edit(
                        3,
                        (None, Some("p")),
                        NodeEditRequest {
                            title: Some("Renamed".into()),
                            ..Default::default()
                        },
                    ),
                ],
                dry_run,
                &[("p", p.as_str())],
                Some(graph),
            )
            .unwrap_err();
            assert_eq!(
                codes(&errors),
                [("batch_failed", None), ("batch_ref_conflict", Some(2))]
            );
            assert_eq!(errors[1].subject.as_deref(), Some("p"));
        }
        assert_eq!((disk(&w), memory(&w)), before);
    }

    #[test]
    fn missing_deleted_and_provenance_ids_fail_as_unknown_refs_and_nothing_changes() {
        let mut w = world();
        let (p, o, graph) = first(&mut w);
        // umx has no node delete: a node is gone when its line leaves nodes.jsonl (by hand, or a revert) after the
        // receipt that named it was written.
        let nodes = w.fake.read(&dir(), GraphFile::Nodes).unwrap().unwrap();
        let kept: Vec<u8> = String::from_utf8(nodes)
            .unwrap()
            .lines()
            .filter(|l| !l.contains(&o))
            .flat_map(|l| format!("{l}\n").into_bytes())
            .collect();
        w.fake.write(&dir(), GraphFile::Nodes, &kept).unwrap();
        let mut w = world_on(w.fake, false);
        assert!(w.svc.node_get(&ctx(), &o).is_err(), "o is deleted");
        let session = w
            .svc
            .loaded_graph("demo")
            .unwrap()
            .provenance_node("session/unknown/unknown")
            .unwrap()
            .id()
            .to_text();
        let missing = Id(graph.0 ^ 0xffff).to_text();
        let refs = [
            ("gone", missing.as_str()),
            ("dead", o.as_str()),
            ("who", session.as_str()),
        ];
        let before = (disk(&w), memory(&w));
        for dry_run in [true, false] {
            let errors = import(
                &mut w,
                vec![
                    edit(
                        1,
                        (None, Some("gone")),
                        NodeEditRequest {
                            title: Some("A".into()),
                            ..Default::default()
                        },
                    ),
                    note_line(2, (None, Some("dead")), "B", Vec::new()),
                    edge(3, (None, Some("who")), "supports", (Some(&p), None)),
                ],
                dry_run,
                &refs,
                Some(graph),
            )
            .unwrap_err();
            assert_eq!(
                codes(&errors),
                [
                    ("batch_failed", None),
                    ("batch_ref_unknown", Some(1)),
                    ("batch_ref_unknown", Some(2)),
                    ("batch_ref_unknown", Some(3)),
                ],
                "{errors:?}"
            );
            let subjects: Vec<_> = errors[1..].iter().map(|p| p.subject.as_deref()).collect();
            assert_eq!(subjects, [Some("gone"), Some("dead"), Some("who")]);
            assert!(errors[1..].iter().all(|p| p.candidates.is_empty()));
        }
        assert_eq!((disk(&w), memory(&w)), before);
    }

    #[test]
    fn imports_are_capped_and_every_name_and_id_is_checked_before_any_line() {
        let mut w = world();
        let (p, _, graph) = first(&mut w);
        let line = || {
            vec![edit(
                1,
                (None, Some("p")),
                NodeEditRequest {
                    title: Some("Renamed".into()),
                    ..Default::default()
                },
            )]
        };
        // One more than the cap is refused before any line; exactly the cap (p among them) runs.
        let mut many: Vec<(String, String)> = (1..MAX_IMPORTED_REFS)
            .map(|i| (format!("r{i}"), p.clone()))
            .collect();
        many.push(("p".to_owned(), p.clone()));
        let at_cap: Vec<(&str, &str)> =
            many.iter().map(|(n, i)| (n.as_str(), i.as_str())).collect();
        let mut over = at_cap.clone();
        over.push(("r0", p.as_str()));
        let errors = import(&mut w, line(), true, &over, Some(graph)).unwrap_err();
        assert_eq!(codes(&errors), [("batch_too_large", None)]);
        assert!(import(&mut w, line(), true, &at_cap, Some(graph)).is_ok());
        let lower = p.to_lowercase();
        for bad in [
            [("P", p.as_str())],
            [("p", lower.as_str())],
            [("p", &p[..20])],
        ] {
            let errors = import(&mut w, line(), true, &bad, Some(graph)).unwrap_err();
            assert_eq!(codes(&errors), [("usage_invalid", None)], "{bad:?}");
        }
    }
}

/// Review D1 through the service (s18): seeded rounds of real, dry and failing batches that mix node edits with
/// node adds, edge adds and edge deletes. A dry or failing batch leaves disk and memory byte for byte as they were and
/// writes nothing. A real batch writes nodes.jsonl and edges.jsonl exactly as a second graph gets them from the same
/// adds and deletes in a batch followed by the edits as single `node edit`s (the splice path), and as memory holds.
#[test]
fn random_batches_write_the_bytes_single_edits_write_and_dry_or_failed_ones_change_nothing() {
    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }
    const TITLES: [&str; 4] = [
        "Approval delay",
        "Risk register review",
        "A much longer title about the approval of the risk register",
        "x",
    ];
    for seed in 1..=40u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let (mut a, mut b) = (world(), world());
        let mut ids = Vec::new();
        for i in 0..8 {
            let req = add("problem", &format!("Row {i} approval"));
            let id = a.svc.node_add(&ctx(), req.clone()).unwrap().value.node.id;
            assert_eq!(b.svc.node_add(&ctx(), req).unwrap().value.node.id, id);
            ids.push(id.to_text());
        }
        let mut used = std::collections::BTreeSet::new();
        for round in 0..10u32 {
            let mut edits = Vec::new();
            let mut others = Vec::new();
            // Only the edges this test added: a node add's own edges (its session, say) are not for deleting.
            let edges: Vec<String> = a
                .svc
                .loaded_graph("demo")
                .unwrap()
                .edges()
                .filter(|e| matches!(e.kind(), "supports" | "blocks"))
                .map(|e| e.id().to_text())
                .collect();
            let mut deleted = std::collections::BTreeSet::new();
            for line in 1..=1 + rng.below(12) as u32 {
                match rng.below(6) {
                    0 => others.push(node(line, None, add("option", TITLES[rng.below(4)]), None)),
                    1 => {
                        let (from, to) = (rng.below(ids.len()), rng.below(ids.len()));
                        let rel = ["supports", "blocks"][rng.below(2)];
                        if used.insert((from, rel, to)) {
                            others.push(edge(
                                line,
                                (Some(&ids[from]), None),
                                rel,
                                (Some(&ids[to]), None),
                            ));
                        }
                    }
                    2 if !edges.is_empty() => {
                        let e = edges[rng.below(edges.len())].clone();
                        if deleted.insert(e.clone()) {
                            others.push(BatchLine {
                                line,
                                op: BatchOp::EdgeDelete { edge: e },
                            });
                        }
                    }
                    _ => {
                        let mut req = NodeEditRequest::default();
                        if rng.below(2) == 0 {
                            req.title = Some(TITLES[rng.below(4)].into());
                        } else {
                            req.note = Some("note ".repeat(rng.below(30)));
                        }
                        edits.push((ids[rng.below(ids.len())].clone(), req));
                    }
                }
            }
            let lines: Vec<BatchLine> = others
                .iter()
                .cloned()
                .chain(
                    edits
                        .iter()
                        .map(|(id, req)| edit(0, (Some(id), None), req.clone())),
                )
                .enumerate()
                .map(|(i, mut l)| {
                    l.line = i as u32 + 1;
                    l
                })
                .collect();
            let before = (disk(&a), memory(&a));
            a.files.sets.lock().unwrap().clear();
            match rng.below(3) {
                0 => {
                    run(&mut a, lines, true).unwrap();
                }
                1 => {
                    let mut lines = lines;
                    let missing = Id(u128::MAX >> 1).to_text();
                    lines.push(edit(
                        lines.len() as u32 + 1,
                        (Some(&missing), None),
                        NodeEditRequest {
                            title: Some("Never written".into()),
                            ..Default::default()
                        },
                    ));
                    // A failed batch has drawn its ids; the second graph draws them too, so the next ids agree.
                    run(&mut b, lines.clone(), false).unwrap_err();
                    run(&mut a, lines, false).unwrap_err();
                }
                _ => {
                    run(&mut a, lines, false).unwrap();
                    if !others.is_empty() {
                        run(&mut b, others, false).unwrap();
                    }
                    for (id, req) in edits {
                        b.svc.node_edit(&ctx(), &id, req).unwrap();
                    }
                    let (nodes, edges) = disk(&a);
                    assert!(
                        (nodes.clone(), edges.clone()) == disk(&b),
                        "seed {seed} round {round}: files differ from single edits"
                    );
                    let m = memory(&a);
                    assert!(
                        (nodes, edges) == (Some(m.0.clone()), Some(m.1.clone())),
                        "seed {seed} round {round}: disk differs from memory"
                    );
                    assert!(m == memory(&b), "seed {seed} round {round}: memory differs");
                    continue;
                }
            }
            assert!(
                (disk(&a), memory(&a)) == before,
                "seed {seed} round {round}: a dry or failed batch changed the graph"
            );
            assert!(a.files.sets.lock().unwrap().is_empty());
        }
    }
}

/// ws002 X1: hints follow the warnings, never count toward batch_warnings_truncated, and segment_without_parent is
/// judged on the batch's result: a segment a later line links part_of its meeting has none.
#[test]
fn hints_follow_the_warnings_and_never_count_toward_the_cap() {
    let mut w = world();
    let mut lines = vec![
        node(1, Some("lone"), add("segment", "Opening"), None),
        node(2, Some("meet"), add("meeting", "Kick off"), None),
        node(3, Some("linked"), add("segment", "Agenda"), None),
        edge(4, (None, Some("linked")), "part_of", (None, Some("meet"))),
        node(5, None, add("problem", "Approval delays"), None),
        // A near duplicate of line 5: a hint per line, inside the batch's own loop.
        node(6, None, add("problem", "Approval delay"), None),
    ];
    // Then more warnings than a batch lists: an undeclared type on every line.
    lines.extend(
        (7..=112).map(|i| node(i, None, add("widget", &format!("Widget number {i}")), None)),
    );
    let done = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                echo_sentences: false,
                reason: None,
                lines,
                dry_run: false,
                refs: Default::default(),
                refs_graph: None,
                raised_in: None,
            },
        )
        .unwrap();
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    let cap = codes
        .iter()
        .position(|c| *c == "batch_warnings_truncated")
        .expect("the cap marker");
    let hints = ["near_duplicate_line", "segment_without_parent"];
    assert_eq!(cap, MAX_BATCH_WARNINGS, "{codes:?}");
    assert!(codes[..cap].iter().all(|c| !hints.contains(c)), "{codes:?}");
    assert_eq!(
        codes[cap + 1..],
        hints,
        "hints come last, after the cap marker"
    );
    // 106 widgets, three undeclared segment and meeting types, three undated anchors: hints are not counted.
    let warned: usize = done.notes[cap].slot("warned").unwrap().parse().unwrap();
    assert_eq!(warned, 112, "{codes:?}");
    let lone = done.value.rows[0].id.unwrap();
    assert_eq!(done.notes.last().unwrap().slot("id"), Some(lone.to_text()));
}

/// A batch's reply notes (warnings then hints) with its result.
fn noted(
    w: &mut World,
    c: &Ctx,
    lines: Vec<BatchLine>,
    dry_run: bool,
) -> (BatchDone, Vec<Problem>) {
    let done = w
        .svc
        .batch(
            c,
            BatchRequest {
                idempotency: None,
                echo_sentences: false,
                reason: None,
                lines,
                dry_run,
                refs: Default::default(),
                refs_graph: None,
                raised_in: None,
            },
        )
        .unwrap();
    (done.value, done.notes)
}

/// review R1: a batch keeps every hint, so the daemon picks the first by catalogue order and counts the rest:
/// 101 reasonless status moves do not crowd out a near duplicate that comes after them.
#[test]
fn a_batch_keeps_every_hint_for_the_reply_to_choose_from() {
    let mut w = world();
    let mut lines: Vec<BatchLine> = (1..=101)
        .map(|i| {
            node(
                i,
                Some(&format!("n{i}")),
                add("problem", &format!("Item number {i}")),
                None,
            )
        })
        .collect();
    lines.extend((1..=101).map(|i| {
        status_line(
            101 + i,
            (None, Some(&format!("n{i}"))),
            "agreed",
            None,
            None,
        )
    }));
    lines.push(node(203, None, add("problem", "Approval delays"), None));
    lines.push(node(204, None, add("problem", "Approval delay"), None));
    let (_, notes) = noted(&mut w, &ctx(), lines, false);
    let count = |code: &str| notes.iter().filter(|p| p.code == code).count();
    assert_eq!(count("reason_missing"), 101);
    assert_eq!(count("near_duplicate_line"), 1);
    assert_eq!(
        count("batch_warnings_truncated"),
        0,
        "hints never count toward the warning cap"
    );
}

/// review R2, R3: an edge the batch adds with an out-of-order id (held back until the write) still parents its
/// segment, dry run or not; a segment left without one is hinted with the line that touched it.
#[test]
fn a_batch_segment_hint_sees_held_back_edges_and_names_its_line() {
    let mut w = world();
    let (a, b) = (
        w.svc
            .node_add(&ctx(), add("problem", "One"))
            .unwrap()
            .value
            .node
            .id
            .to_text(),
        w.svc
            .node_add(&ctx(), add("problem", "Two"))
            .unwrap()
            .value
            .node
            .id
            .to_text(),
    );
    w.svc.edge_add(&ctx(), &a, "supports", &b, &[]).unwrap();
    let held = BatchLine {
        line: 4,
        op: BatchOp::EdgeAdd {
            id: Some("00000000000000000000000005".into()),
            from: None,
            from_ref: Some("s".into()),
            relation: "part_of".into(),
            to: None,
            to_ref: Some("m".into()),
            sets: Vec::new(),
            evidence: Default::default(),
        },
    };
    for dry_run in [true, false] {
        let lines = vec![
            node(1, Some("lone"), add("segment", "Closing"), None),
            node(2, Some("s"), add("segment", "Opening"), None),
            node(3, Some("m"), add("meeting", "Kick off"), None),
            held.clone(),
        ];
        let (_, notes) = noted(&mut w, &ctx(), lines, dry_run);
        let hinted: Vec<Option<u32>> = notes
            .iter()
            .filter(|p| p.code == "segment_without_parent")
            .map(|p| p.line)
            .collect();
        assert_eq!(hinted, [Some(1)], "dry run {dry_run}");
    }
}

/// review R4: a segment whose part_of edge goes to another graph has a parent; an edit gives no hint.
/// review R5: an edge stored here from another graph's node with the same id parents that node, not this one.
#[test]
fn a_segment_parent_in_another_graph_counts_and_one_from_another_graph_does_not() {
    let mut w = world();
    let beta = w
        .svc
        .graph_new(&ctx(), "/work/beta", "beta", None, false, None)
        .unwrap()
        .value
        .graph
        .id;
    let meeting = w
        .svc
        .node_add(&ctx().graph("beta"), add("meeting", "Kick off"))
        .unwrap()
        .value
        .node
        .id;
    let segment = w
        .svc
        .node_add(&ctx().graph("demo"), add("segment", "Opening"))
        .unwrap()
        .value
        .node
        .id;
    w.svc
        .edge_add(
            &ctx().graph("demo"),
            &segment.to_text(),
            "part_of",
            &format!("umx:{beta}/{meeting}"),
            &[],
        )
        .unwrap();
    let edited = w
        .svc
        .node_edit(
            &ctx().graph("demo"),
            &segment.to_text(),
            umx_services::NodeEditRequest {
                title: Some("Opening remarks".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        !edited
            .notes
            .iter()
            .any(|p| p.code == "segment_without_parent"),
        "{:?}",
        edited.notes
    );

    // R5: demo stores umx:<other graph>/<lone's id> part_of a local meeting; that parents the other graph's node.
    let lone = w
        .svc
        .node_add(&ctx().graph("demo"), add("segment", "Closing"))
        .unwrap()
        .value
        .node
        .id;
    let local = w
        .svc
        .node_add(&ctx().graph("demo"), add("meeting", "Wrap up"))
        .unwrap()
        .value
        .node
        .id;
    let elsewhere = "01JC8XA4H7Q1V9K2M3N4P5R6S7";
    w.svc
        .edge_add(
            &ctx().graph("demo"),
            &format!("umx:{elsewhere}/{lone}"),
            "part_of",
            &local.to_text(),
            &[],
        )
        .unwrap();
    let edit_lone = |title: &str| {
        edit(
            1,
            (Some(&lone.to_text()), None),
            umx_services::NodeEditRequest {
                title: Some(title.into()),
                ..Default::default()
            },
        )
    };
    let (_, notes) = noted(
        &mut w,
        &ctx().graph("demo"),
        vec![edit_lone("Closing words")],
        false,
    );
    assert!(
        notes.iter().any(|p| p.code == "segment_without_parent"),
        "{notes:?}"
    );
}

/// ws003 s27: each node add line's near-duplicate note (its code, and its candidates' ids, scores and order) is a
/// scan's of every title the graph and the batch's earlier lines hold, in batch order. The scan scores with the public
/// scorer and keep-rule only, so it shares no candidate step with the word index's grouped walk. Titles are natural
/// words, 64-hex tokens, both mixed, one or two letters, and typos of earlier titles, on two types.
#[test]
fn near_duplicate_notes_equal_a_scan_of_every_title_in_batch_order() {
    const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
    const HEX: &[u8] = b"0123456789abcdef";
    let mut seed = 0x5327_u64;
    let mut below = |n: usize| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((seed >> 33) % n as u64) as usize
    };
    fn word(
        below: &mut dyn FnMut(usize) -> usize,
        alphabet: &[u8],
        lo: usize,
        hi: usize,
    ) -> String {
        (0..lo + below(hi - lo + 1))
            .map(|_| alphabet[below(alphabet.len())] as char)
            .collect()
    }
    let vocab: Vec<String> = (0..40).map(|_| word(&mut below, LETTERS, 3, 9)).collect();
    let mut w = world();
    // (id, type, title) of every node the batches added.
    let mut held: Vec<(Id, &str, String)> = Vec::new();
    let (mut hits, mut from_line, mut hex_hits) = (0, 0, 0);
    for round in 0..8 {
        let mut lines = Vec::new();
        let mut asked: Vec<(&str, String)> = Vec::new();
        for i in 0..60 {
            let kind = ["problem", "risk"][below(2)];
            let mut title = match below(6) {
                0 => word(&mut below, HEX, 64, 64),
                1 => {
                    let w = vocab[below(vocab.len())].clone();
                    format!("{w} {}", word(&mut below, HEX, 8, 16))
                }
                2 => word(&mut below, LETTERS, 1, 2),
                _ => (0..1 + below(4))
                    .map(|_| vocab[below(vocab.len())].clone())
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            // A typo of a title held or asked earlier (an earlier line half the time): a near duplicate of the
            // graph or of an earlier line.
            let lines_only = !asked.is_empty() && below(2) == 0;
            let earlier: Vec<&String> = held
                .iter()
                .filter(|_| !lines_only)
                .map(|h| &h.2)
                .chain(asked.iter().map(|a| &a.1))
                .collect();
            if !earlier.is_empty() && below(3) != 0 {
                let old = earlier[below(earlier.len())];
                let mut c: Vec<char> = old.chars().collect();
                let at = below(c.len());
                // A 64-hex token keeps its length: other long hex is a secret by the scan's rule.
                let hex = old.split(' ').any(|w| w.len() > 40);
                match below(3) {
                    _ if hex => c[at] = HEX[below(16)] as char,
                    0 => c[at] = LETTERS[below(26)] as char,
                    1 if c.len() > 1 => {
                        c.remove(at);
                    }
                    _ => c.insert(at, HEX[below(16)] as char),
                }
                title = c.into_iter().collect();
            }
            lines.push(node(i + 1, None, add(kind, &title), None));
            asked.push((kind, title));
        }
        let done = w
            .svc
            .batch(
                &ctx(),
                BatchRequest {
                    idempotency: None,
                    raised_in: None,
                    echo_sentences: false,
                    lines,
                    dry_run: false,
                    refs: Default::default(),
                    refs_graph: None,
                    reason: None,
                },
            )
            .unwrap();
        let ids: Vec<Id> = done.value.rows.iter().map(|r| r.id.unwrap()).collect();
        let before = held.len();
        // (line, code, candidates as (id, score)) of each near-duplicate note.
        type Note<'a> = (u32, &'a str, Vec<(Id, f64)>);
        let mut want: Vec<Note> = Vec::new();
        for (i, (kind, title)) in asked.iter().enumerate() {
            let mut near: Vec<(Id, f64)> = held
                .iter()
                .filter_map(|(id, k, t)| {
                    let s = umx_graph::title_score(title, t);
                    (s >= 0.85
                        && (k == kind || s >= 0.95)
                        && umx_graph::near_duplicate_keeps(title, t))
                    .then_some((*id, s))
                })
                .collect();
            near.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            near.truncate(3);
            held.push((ids[i], kind, title.clone()));
            let Some(&(best, _)) = near.first() else {
                continue;
            };
            let code = if held[..before].iter().any(|h| h.0 == best) {
                "near_duplicate"
            } else {
                from_line += 1;
                "near_duplicate_line"
            };
            hits += near.len();
            hex_hits += usize::from(title.len() >= 64);
            want.push((i as u32 + 1, code, near));
        }
        // Warnings in line order, then hints in line order.
        want.sort_by_key(|(line, code, _)| (umx_services::problems::is_hint(code), *line));
        let got: Vec<Note> = done
            .notes
            .iter()
            .filter(|p| p.code.starts_with("near_duplicate"))
            .map(|p| {
                let cands = p
                    .candidates
                    .iter()
                    .map(|c| (c.id.parse().unwrap(), c.score.unwrap()))
                    .collect();
                (p.line.unwrap(), p.code, cands)
            })
            .collect();
        assert_eq!(got, want, "round {round}");
    }
    assert!(
        hits > 60 && from_line > 20 && hex_hits >= 4,
        "{hits} candidates, {from_line} best on an earlier line, {hex_hits} hex titles near"
    );
}

/// A batch's reply with its notes (warnings then hints), for plan 205's checks.
fn run_notes(w: &mut World, lines: Vec<BatchLine>) -> (BatchDone, Vec<Problem>) {
    let done = w
        .svc
        .batch(
            &ctx(),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                lines,
                dry_run: false,
                refs: Default::default(),
                refs_graph: None,
                reason: None,
            },
        )
        .unwrap();
    (done.value, done.notes)
}

fn evidence_of(w: &World, id: Id) -> Vec<umx_core::Value> {
    let g = w.svc.loaded_graph("demo").unwrap();
    g.node(id).unwrap().to_node().evidence
}

#[test]
fn node_add_evidence_entries_store_what_a_node_note_stores_with_its_checks() {
    // Plan 205 item 6: `{pointer, kind}` entries on a node add, as a node note takes them.
    let entries = || {
        vec![
            EvidencePointer {
                pointer: "notes.md#L1-L4".into(),
                kind: Some(EvidenceKind::Analyst),
            },
            "calls/3-nov.vtt#seq=120-140".into(),
        ]
    };
    let mut w = world();
    let mut req = add("problem", "Added with evidence");
    req.evidence = entries();
    let added = run(&mut w, vec![node(1, None, req, None)], false).unwrap();
    let noted = run(
        &mut w,
        vec![
            node(1, Some("plain"), add("problem", "Noted later"), None),
            note_line(2, (None, Some("plain")), "Reviewed", entries()),
        ],
        false,
    )
    .unwrap();
    let (a, b) = (added.rows[0].id.unwrap(), noted.rows[0].id.unwrap());
    let strip = |v: Vec<umx_core::Value>| -> Vec<umx_core::Value> {
        v.into_iter()
            .map(|mut e| {
                e.as_object_mut().unwrap().remove("by_session");
                e
            })
            .collect()
    };
    assert_eq!(strip(evidence_of(&w, a)), strip(evidence_of(&w, b)));
    assert_eq!(evidence_of(&w, a)[0]["kind"], "analyst");
    // evidence_kind is every kindless entry's kind; an entry's own kind beside it refuses the line.
    let mut req = add("problem", "Room evidence");
    req.evidence = vec!["notes.md#L9".into()];
    req.evidence_kind = Some("room".into());
    let room = run(&mut w, vec![node(1, None, req, None)], false).unwrap();
    assert_eq!(evidence_of(&w, room.rows[0].id.unwrap())[0]["kind"], "room");
    let before = (disk(&w), memory(&w));
    for dry_run in [true, false] {
        let mut both = add("problem", "Both kinds");
        both.evidence = entries();
        both.evidence_kind = Some("room".into());
        let errors = run(&mut w, vec![node(3, None, both, None)], dry_run).unwrap_err();
        assert_eq!(
            codes(&errors),
            [("batch_failed", None), ("batch_line_invalid", Some(3))]
        );
        assert_eq!(
            errors[1].slot("reason").as_deref(),
            Some("give a kind on each evidence entry or evidence_kind but not both")
        );
        // A pointer that does not parse, or that holds a secret, is refused as a string pointer is.
        let token = format!("ghp_{}", "a".repeat(36));
        for pointer in [
            "not a pointer".to_string(),
            format!("https://example.test/a?token={token}#L1"),
        ] {
            let refusals: Vec<Vec<Problem>> = [Some(EvidenceKind::Chat), None]
                .into_iter()
                .map(|kind| {
                    let mut req = add("problem", "Bad pointer");
                    req.evidence = vec![EvidencePointer {
                        pointer: pointer.clone(),
                        kind,
                    }];
                    run(&mut w, vec![node(4, None, req, None)], dry_run).unwrap_err()
                })
                .collect();
            assert_eq!(codes(&refusals[0]), codes(&refusals[1]), "{pointer}");
            assert_eq!(
                refusals[0][1].slot("reason"),
                refusals[1][1].slot("reason"),
                "{pointer}"
            );
            assert!(!format!("{refusals:?}").contains(&token));
        }
    }
    assert_eq!((disk(&w), memory(&w)), before);
}

#[test]
fn a_ref_in_a_plain_node_field_fails_with_a_hint_to_use_its_ref_field() {
    // Plan 205 item 3: node_not_found on `node: q1` when q1 is a ref an earlier line bound.
    let mut w = world();
    let errors = run(
        &mut w,
        vec![
            node(1, Some("q1"), add("question", "Which store?"), None),
            edit(
                2,
                (Some("q1"), None),
                NodeEditRequest {
                    title: Some("Which file store?".into()),
                    ..Default::default()
                },
            ),
            edge(3, (Some("q1"), None), "answers", (None, Some("q1"))),
            edit(
                4,
                (Some("zz9"), None),
                NodeEditRequest {
                    title: Some("Not a ref either".into()),
                    ..Default::default()
                },
            ),
        ],
        true,
    )
    .unwrap_err();
    let hinted: Vec<&Problem> = errors
        .iter()
        .filter(|p| p.code == "batch_use_ref_field")
        .collect();
    let named: Vec<(Option<u32>, String)> = hinted
        .iter()
        .map(|p| {
            let (field, ref_field) = (p.slot("field").unwrap(), p.slot("ref_field").unwrap());
            (p.line, format!("{field} -> {ref_field}"))
        })
        .collect();
    assert_eq!(
        named,
        [
            (Some(2), "node -> node_ref".to_string()),
            (Some(3), "from -> from_ref".to_string()),
        ]
    );
    // The hints follow every error: batch_failed still leads, and the ref itself is never named.
    assert_eq!(errors[0].code, "batch_failed");
    let first_hint = errors
        .iter()
        .position(|p| p.code == "batch_use_ref_field")
        .unwrap();
    assert!(
        errors[first_hint..]
            .iter()
            .all(|p| p.code == "batch_use_ref_field")
    );
    assert!(!format!("{hinted:?}").contains("\"q1\""));
}

#[test]
fn a_field_set_twice_on_one_line_warns_duplicate_field_once_per_field() {
    // Plan 205 item 4: last-wins drops data, so it warns (a warning, not a hint).
    let mut w = world();
    let mut req = add("problem", "Twice");
    req.sets = vec![
        ("owner".into(), SetValue::Str("first".into())),
        ("owner".into(), SetValue::Str("second".into())),
        ("cost".into(), SetValue::Json(umx_core::Value::from(1))),
        ("owner".into(), SetValue::Str("third".into())),
        ("cost".into(), SetValue::Json(umx_core::Value::from(2))),
        ("area".into(), SetValue::Str("once".into())),
    ];
    let (done, notes) = run_notes(&mut w, vec![node(5, None, req, None)]);
    let dupes: Vec<(Option<u32>, Option<String>)> = notes
        .iter()
        .filter(|p| p.code == "duplicate_field")
        .map(|p| (p.line, p.slot("field")))
        .collect();
    assert_eq!(
        dupes,
        [
            (Some(5), Some("owner".into())),
            (Some(5), Some("cost".into()))
        ]
    );
    let g = w.svc.loaded_graph("demo").unwrap();
    let node = g.node(done.rows[0].id.unwrap()).unwrap().to_node();
    assert_eq!(node.fields["owner"], "third", "still last wins");
}

#[test]
fn a_bad_ref_names_its_character_and_the_lowercase_rule_readably() {
    // Plan 205 item 5: `_` stays readable in the rule (it was shown as `?`).
    let mut w = world();
    for (name, want) in [
        (
            "Q1",
            "ref character Q at position 1 is not allowed - a ref starts with a lowercase letter a-z",
        ),
        (
            "brd_exA",
            "ref character A at position 7 is not allowed - a ref holds only lowercase a-z 0-9 _ and -",
        ),
    ] {
        let errors = run(
            &mut w,
            vec![node(1, Some(name), add("problem", "Bad ref"), None)],
            true,
        )
        .unwrap_err();
        assert_eq!(errors[1].slot("reason").as_deref(), Some(want), "{name}");
    }
}

#[test]
fn warnings_after_the_write_join_the_batch_warning_cap_and_its_count() {
    // Plan 205 item 8: one vector_shard_unreadable per touched node once bypassed the 100 cap.
    let mut w = world();
    let key = ModelKey::new("fake", 8).unwrap();
    w.svc
        .graph_new(
            &ctx(),
            "/work/emb",
            "emb",
            None,
            false,
            Some(umx_core::GraphEmbedding::new(
                Some(key),
                umx_core::Policy::OPEN,
            )),
        )
        .unwrap();
    w.files.fail_shards.store(true, Ordering::SeqCst);
    let n = MAX_BATCH_WARNINGS + 50;
    let lines = (1..=n as u32)
        .map(|i| {
            node(
                i,
                None,
                add("problem", &format!("Distinct topic {i:04} zq{i}")),
                None,
            )
        })
        .collect();
    let done = w
        .svc
        .batch(
            &ctx().graph("emb"),
            BatchRequest {
                idempotency: None,
                raised_in: None,
                echo_sentences: false,
                lines,
                dry_run: false,
                refs: Default::default(),
                refs_graph: None,
                reason: None,
            },
        )
        .unwrap();
    let warnings: Vec<&Problem> = done
        .notes
        .iter()
        .filter(|p| !umx_services::problems::is_hint(p.code))
        .collect();
    let unreadable = warnings
        .iter()
        .filter(|p| p.code == "vector_shard_unreadable")
        .count();
    let last = warnings.last().unwrap();
    assert!(unreadable > 0, "the planted failure reached the recheck");
    assert_eq!(warnings.len(), MAX_BATCH_WARNINGS + 1);
    assert_eq!(last.code, "batch_warnings_truncated");
    assert!(
        last.slot("warned").unwrap().parse::<usize>().unwrap() >= n,
        "every unreadable shard is counted"
    );
    assert_eq!(
        done.value.warned,
        last.slot("warned").unwrap().parse::<usize>().unwrap()
    );
}
