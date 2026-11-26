//! F144 (plan 148): a loaded graph whose content files changed on disk since the load (a checkout, a pull, a hand
//! edit) refuses every write with graph_files_changed, writing nothing and leaving memory as it was, and its reads
//! warn stale_files. The check is made twice, up front by the store and at the commit by the files port; the plants
//! below take each away to show the other still holds, and both away to show the repro then loses data.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use umx_core::codec::GraphFile;
use umx_core::ports::{
    Change, Committed, FileRef, FileStamp, FilesError, GraphDir, GraphFiles, GraphStamps, ModelKey,
};
use umx_core::vectors::{ShardFile, ShardKind};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState, FakeSourceReader};
use umx_services::{Ctx, NodeAddRequest, NodeListRequest, Ports, Services};

/// The fake files with either check taken away (a broken build the plants stand for).
struct Planted {
    inner: Arc<FakeGraphFiles>,
    /// The store's up-front look fails, which it reads as nothing moved.
    no_up_front: bool,
    /// Every set is passed on without the stamps it carries.
    no_commit_check: bool,
    fail_refresh: Option<Arc<AtomicBool>>,
}

impl GraphFiles for Planted {
    fn relocate(
        &self,
        from: &umx_core::ports::GraphDir,
        to: &umx_core::ports::GraphDir,
    ) -> Result<(), umx_core::ports::FilesError> {
        self.inner.relocate(from, to)
    }
    fn read_ref(&self, dir: &GraphDir, file: &FileRef) -> Result<Option<Vec<u8>>, FilesError> {
        self.inner.read_ref(dir, file)
    }
    fn write_set_checked_also(
        &self,
        dir: &GraphDir,
        changes: &[Change<'_>],
        expect: Option<&GraphStamps>,
        also: &[(FileRef, FileStamp)],
    ) -> Result<Committed, FilesError> {
        let expect = expect.filter(|_| !self.no_commit_check);
        let committed = self
            .inner
            .write_set_checked_also(dir, changes, expect, also)?;
        if changes
            .iter()
            .any(|change| matches!(change.file, FileRef::RuleSet(_)))
            && let Some(fail) = &self.fail_refresh
        {
            fail.store(true, Ordering::SeqCst);
        }
        Ok(committed)
    }
    fn read_ref_stamped(
        &self,
        dir: &GraphDir,
        file: &FileRef,
    ) -> Result<(Option<Vec<u8>>, FileStamp), FilesError> {
        self.inner.read_ref_stamped(dir, file)
    }
    fn stamps(&self, dir: &GraphDir) -> Result<GraphStamps, FilesError> {
        match self.no_up_front {
            true => Err(FilesError::io("planted: no up-front look")),
            false => self.inner.stamps(dir),
        }
    }
    fn list(&self, dir: &GraphDir) -> Result<Vec<GraphFile>, FilesError> {
        self.inner.list(dir)
    }
    fn list_rule_sets(
        &self,
        dir: &GraphDir,
    ) -> Result<umx_core::ports::RuleSetListing, FilesError> {
        if self
            .fail_refresh
            .as_ref()
            .is_some_and(|fail| fail.swap(false, Ordering::SeqCst))
        {
            return Err(FilesError::RecoveryConflict(vec![GraphFile::Graph.into()]));
        }
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
        "test.planted"
    }
}

fn services(files: Arc<dyn GraphFiles>) -> Services {
    Services::new(Ports {
        sources: Arc::new(FakeSourceReader::open(GraphDir::parse("/").unwrap())),
        files,
        vector_cache: Arc::new(FakeGraphFiles::new(GraphDir::parse("/cache").unwrap())),
        vector_cache_root: GraphDir::parse("/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new().with_absent_ok(true)),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    })
}

fn ctx() -> Ctx {
    Ctx::new("tester").graph("demo")
}

fn add(svc: &mut Services, title: &str) -> umx_services::Outcome<umx_services::NodeAdded> {
    svc.node_add(
        &ctx(),
        NodeAddRequest {
            kind: "problem".into(),
            title: title.into(),
            ..Default::default()
        },
    )
}

fn listed(svc: &Services) -> (usize, Vec<&'static str>) {
    let done = svc.node_list(&ctx(), NodeListRequest::default()).unwrap();
    (
        done.value.total.unwrap(),
        done.notes.iter().map(|p| p.code).collect(),
    )
}

const CHECKED_OUT: &[u8] = b"";

/// The F144 repro over the planted port: load on one branch, a checkout replaces nodes.jsonl, then a node add.
/// Returns the add's error code, if it failed, and whether nodes.jsonl still holds the checked-out bytes.
fn repro(no_up_front: bool, no_commit_check: bool) -> (Option<&'static str>, bool) {
    let fake = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let mut svc = services(Arc::new(Planted {
        inner: fake.clone(),
        no_up_front,
        no_commit_check,
        fail_refresh: None,
    }));
    svc.graph_new(&Ctx::new("tester"), "/work/g", "demo", None, false, None)
        .unwrap();
    add(&mut svc, "Only on branch e1").unwrap();
    let dir = GraphDir::parse("/work/g").unwrap();
    fake.tamper_for_tests(&dir, GraphFile::Nodes, CHECKED_OUT);
    let code = add(&mut svc, "Written after the checkout")
        .err()
        .map(|p| p.code);
    let kept = fake.raw_for_tests(&dir, GraphFile::Nodes).as_deref() == Some(CHECKED_OUT);
    (code, kept)
}

#[test]
fn the_f144_repro_is_refused_and_writes_nothing() {
    assert_eq!(repro(false, false), (Some("graph_files_changed"), true));
}

#[test]
fn rule_sets_committed_changes_survive_refresh_failure_and_writes_require_reload() {
    use umx_core::ports::RuleSetFile;
    let fake = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let mut svc = services(Arc::new(Planted {
        inner: fake.clone(),
        no_up_front: false,
        no_commit_check: false,
        fail_refresh: Some(Arc::new(AtomicBool::new(false))),
    }));
    svc.graph_new(&Ctx::new("tester"), "/work/g", "demo", None, false, None)
        .unwrap();
    let dir = GraphDir::parse("/work/g").unwrap();
    let file = FileRef::RuleSet(RuleSetFile::new("kit").unwrap());
    let changed = svc.rule_set_add(&ctx(), "kit", b"{}").unwrap();
    assert!(!changed.value.unchanged);
    assert!(
        changed
            .notes
            .iter()
            .any(|problem| problem.code == "rule_sets_refresh_failed")
    );
    assert_eq!(
        fake.read_ref(&dir, &file).unwrap().as_deref(),
        Some(&b"{}"[..])
    );
    let refused = add(&mut svc, "Blocked until reload").unwrap_err();
    assert_eq!(refused.code, "graph_files_changed");
    let next = refused.slot("fix").unwrap();
    assert!(next.contains("umx graph unload demo"));
    assert!(next.contains("umx graph load /work/g --as demo"));
    svc.graph_load(&ctx(), "/work/g", Some("demo")).unwrap();
    add(&mut svc, "Reloaded").unwrap();
    let removed = svc.rule_set_remove(&ctx(), "kit").unwrap();
    assert!(!removed.value.unchanged);
    assert!(
        removed
            .notes
            .iter()
            .any(|problem| problem.code == "rule_sets_refresh_failed")
    );
    assert_eq!(fake.read_ref(&dir, &file).unwrap(), None);
    let refused = add(&mut svc, "Last copy is gone but snapshot failed").unwrap_err();
    assert_eq!(refused.code, "graph_files_changed");
    assert!(
        refused
            .slot("fix")
            .unwrap()
            .contains("umx graph load /work/g --as demo")
    );
    svc.graph_load(&ctx(), "/work/g", Some("demo")).unwrap();
    add(&mut svc, "Reloaded after removal").unwrap();
}

#[test]
fn plant_without_the_up_front_check_the_commit_check_still_refuses() {
    assert_eq!(repro(true, false), (Some("graph_files_changed"), true));
}

#[test]
fn plant_without_either_check_the_repro_loses_the_checked_out_file() {
    assert_eq!(
        repro(true, true),
        (None, false),
        "with both checks gone the write must go through and replace the file, or the repro proves nothing"
    );
}

/// The up-front check refuses before anything moves: memory keeps its nodes, the files keep their bytes, every
/// kind of write is refused, a read warns and still answers from memory, another graph is untouched, and a load
/// of the folder again clears it all.
#[test]
fn a_changed_graph_refuses_every_write_warns_on_reads_and_a_load_clears_it() {
    let fake = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let mut svc = services(fake.clone());
    let new = |svc: &mut Services, path: &str, alias: &str| {
        svc.graph_new(&Ctx::new("tester"), path, alias, None, false, None)
            .unwrap();
    };
    new(&mut svc, "/work/g", "demo");
    new(&mut svc, "/work/other", "other");
    let a = add(&mut svc, "First").unwrap().value.node.id.to_string();
    let dir = GraphDir::parse("/work/g").unwrap();
    fake.touch_for_tests(&dir, GraphFile::Edges);
    let disk =
        [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges].map(|f| fake.raw_for_tests(&dir, f));

    let refused = |r: Result<(), umx_core::Problem>| {
        let p = r.unwrap_err();
        assert_eq!(p.code, "graph_files_changed");
        assert_eq!(p.slot("files").as_deref(), Some("edges.jsonl"));
    };
    refused(add(&mut svc, "Second").map(|_| ()));
    refused(
        svc.edge_add(&ctx(), &a, "supports", &a, Default::default())
            .map(|_| ()),
    );
    refused(
        svc.graph_set(&ctx(), "demo", &[("title".into(), "New".into())])
            .map(|_| ()),
    );
    refused(
        svc.node_status(&ctx(), &a, "resolved", None, None)
            .map(|_| ()),
    );
    assert_eq!(
        [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges].map(|f| fake.raw_for_tests(&dir, f)),
        disk,
        "nothing written"
    );
    assert_eq!(listed(&svc), (1, vec!["stale_files"]), "memory as it was");
    // Another graph is not this one's files.
    svc.node_add(
        &Ctx::new("tester").graph("other"),
        NodeAddRequest {
            kind: "problem".into(),
            title: "Elsewhere".into(),
            ..Default::default()
        },
    )
    .unwrap();

    svc.graph_load(&Ctx::new("tester"), "/work/g", Some("demo"))
        .unwrap();
    assert_eq!(listed(&svc), (1, vec![]));
    add(&mut svc, "Second").unwrap();
}

/// ws008 E2 (graph-storage-expert): `entity unlink` on a graph whose files changed refuses with graph_files_changed
/// before any work, as `edge delete` does. The commit check is taken away, so only the up-front check can refuse: the
/// link stays in memory and the changed file keeps its bytes.
#[test]
fn entity_unlink_on_a_changed_graph_is_refused_up_front() {
    let fake = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let mut svc = services(Arc::new(Planted {
        inner: fake.clone(),
        no_up_front: false,
        no_commit_check: true,
        fail_refresh: None,
    }));
    svc.graph_new(&Ctx::new("tester"), "/work/g", "demo", None, false, None)
        .unwrap();
    let a = add(&mut svc, "Jo").unwrap().value.node.id.to_string();
    let b = add(&mut svc, "Jo again").unwrap().value.node.id.to_string();
    svc.entity_link(&ctx(), &a, &b, false).unwrap();
    let dir = GraphDir::parse("/work/g").unwrap();
    fake.tamper_for_tests(&dir, GraphFile::Edges, CHECKED_OUT);
    let refused = svc.entity_unlink(&ctx(), &a, &b).unwrap_err();
    assert_eq!(refused.code, "graph_files_changed");
    assert_eq!(refused.slot("files").as_deref(), Some("edges.jsonl"));
    assert_eq!(
        fake.raw_for_tests(&dir, GraphFile::Edges).as_deref(),
        Some(CHECKED_OUT),
        "nothing written"
    );
    let shown = svc.entity_show(&ctx(), &a).unwrap();
    assert_eq!(shown.value.members.len(), 2, "the link is still in memory");
}
