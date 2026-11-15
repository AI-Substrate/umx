//! The repo Graphfile in services (ws002 s17, D66, req-128): graph new adds to it, graph scan lists and changes it,
//! graph list and load --graphfile read it. Over the fakes; the CLI's end-to-end test runs it on disk.

use std::sync::Arc;
use umx_core::codec::GraphFile;
use umx_core::graphfile::{Entry, Graphfile};
use umx_core::ports::{ConfigFile, ConfigFiles, ConfigFilesError, GraphDir, Swapped};
use umx_core::{GraphEmbedding, Id, Policy};
use umx_fakes::{FakeClock, FakeConfigFiles, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{Ctx, GraphfileMode, Ports, ScanRequest, Services};

struct Machine {
    files: Arc<FakeGraphFiles>,
    config: Arc<FakeConfigFiles>,
}

const REPO: &str = "/work/repo";

impl Machine {
    fn new() -> Machine {
        let config = Arc::new(FakeConfigFiles::new(dir("/home"), dir("/work")));
        config.add_repo(dir(REPO));
        Machine {
            files: Arc::new(FakeGraphFiles::new(dir("/work"))),
            config,
        }
    }

    fn services_with(&self, config: Option<Arc<dyn ConfigFiles>>) -> Services {
        let svc = Services::new(Ports {
            files: self.files.clone(),
            vector_cache: Arc::new(FakeGraphFiles::new(dir("/cache"))),
            vector_cache_root: dir("/cache"),
            clock: Arc::new(FakeClock::default()),
            ids: Arc::new(FakeIds::new()),
            state: Arc::new(FakeLocalState::new()),
            scopes: Arc::new(FakeLocalState::new()),
            registry: Arc::new(FakeLocalState::new().with_absent_ok(true)),
            sources: Arc::new(umx_fakes::FakeSourceReader::open(dir("/"))),
            allows: Arc::new(FakeLocalState::new()),
            lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
        });
        match config {
            Some(c) => svc.with_config_files(c),
            None => svc,
        }
    }

    fn services(&self) -> Services {
        self.services_with(Some(self.config.clone()))
    }

    fn graphfile(&self) -> Option<String> {
        self.config
            .read(&ConfigFile::Graphfile(dir(REPO)))
            .unwrap()
            .map(|b| String::from_utf8(b).unwrap())
    }

    fn seed(&self, gf: &Graphfile) {
        self.config
            .seed(ConfigFile::Graphfile(dir(REPO)), &gf.encode().unwrap());
    }

    /// A graph's files at `path` with `id` and `alias`, as a clone or a copy would leave them.
    fn graph_at(&self, svc: &mut Services, path: &str, alias: &str) -> Id {
        let made = svc
            .graph_new(&ctx(), path, alias, None, true, None)
            .unwrap();
        let id = made.value.graph.id;
        svc.graph_unload(&ctx(), alias).unwrap();
        id
    }
}

fn dir(p: &str) -> GraphDir {
    GraphDir::parse(p).unwrap()
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

fn codes(warnings: &[umx_core::Problem]) -> Vec<&'static str> {
    warnings.iter().map(|w| w.code).collect()
}

fn new_graph(
    svc: &mut Services,
    path: &str,
    alias: &str,
    mode: GraphfileMode,
) -> umx_services::Done<umx_services::GraphNew> {
    svc.graph_new_with(&ctx(), path, alias, None, true, None, mode)
        .unwrap()
}

fn secured() -> Option<GraphEmbedding> {
    Some(GraphEmbedding::new(None, Policy::SECURED))
}

fn scan(svc: &Services, req: ScanRequest) -> umx_services::Done<umx_services::Scanned> {
    svc.graph_scan(&ctx(), REPO, &req).unwrap()
}

fn states(scanned: &umx_services::Scanned) -> Vec<(String, &'static str)> {
    scanned
        .rows
        .iter()
        .map(|r| (r.path.clone(), r.state))
        .collect()
}

#[test]
fn graph_new_adds_its_line_with_an_undo_and_the_file_is_canonical() {
    let m = Machine::new();
    let mut svc = m.services();
    let b = new_graph(&mut svc, "/work/repo/docs/b", "b", GraphfileMode::Add);
    assert!(b.notes.is_empty(), "{:?}", b.notes);
    let added = b.value.graphfile.clone().expect("a line was added");
    assert_eq!((added.path.as_str(), added.alias.as_str()), ("docs/b", "b"));
    assert_eq!(added.undo.as_str(), "umx graph scan /work/repo --drop b");
    assert_eq!(added.file.as_str(), "/work/repo/Graphfile");
    new_graph(&mut svc, "/work/repo/docs/a", "a", GraphfileMode::Add);
    new_graph(&mut svc, "/work/repo", "top", GraphfileMode::Add);
    let text = m.graphfile().unwrap();
    let gf = Graphfile::decode(text.as_bytes()).unwrap();
    let paths: Vec<&str> = gf.entries().iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, [".", "docs/a", "docs/b"], "sorted, the top as .");
    assert!(Graphfile::is_canonical(text.as_bytes()), "{text}");
    // The undo removes exactly that line.
    let dropped = scan(
        &svc,
        ScanRequest {
            drop: Some("b".into()),
            ..ScanRequest::default()
        },
    );
    assert!(dropped.value.dropped && dropped.value.written);
    let gf = Graphfile::decode(m.graphfile().unwrap().as_bytes()).unwrap();
    assert!(gf.entries().iter().all(|e| e.alias != "b"));
}

/// Security, s17: a secured graph leaves the Graphfile byte for byte as it was, unless --graphfile.
#[test]
fn a_secured_graph_leaves_the_graphfile_untouched_unless_asked() {
    let m = Machine::new();
    let mut svc = m.services();
    new_graph(&mut svc, "/work/repo/open", "open", GraphfileMode::Add);
    let before = m.graphfile().unwrap();
    let made = svc
        .graph_new_with(
            &ctx(),
            "/work/repo/client",
            "client",
            None,
            true,
            secured(),
            GraphfileMode::Add,
        )
        .unwrap();
    assert_eq!(codes(&made.notes), ["graphfile_skipped_secured"]);
    assert!(made.value.graphfile.is_none());
    assert_eq!(m.graphfile().unwrap(), before, "byte for byte");
    let asked = svc
        .graph_new_with(
            &ctx(),
            "/work/repo/client2",
            "client2",
            None,
            true,
            secured(),
            GraphfileMode::Include,
        )
        .unwrap();
    assert!(asked.notes.is_empty(), "{:?}", asked.notes);
    assert!(m.graphfile().unwrap().contains("alias = \"client2\""));
}

#[test]
fn no_graphfile_outside_a_repo_or_when_told_and_none_unwired() {
    let m = Machine::new();
    let mut svc = m.services();
    let skipped = new_graph(
        &mut svc,
        "/work/repo/private",
        "private",
        GraphfileMode::Skip,
    );
    assert!(skipped.notes.is_empty() && skipped.value.graphfile.is_none());
    assert_eq!(m.graphfile(), None, "--no-graphfile writes nothing");
    let quiet = new_graph(&mut svc, "/work/quiet", "quiet", GraphfileMode::Add);
    assert!(
        quiet.notes.is_empty(),
        "the default stays quiet outside a repo: {:?}",
        quiet.notes
    );
    let outside = new_graph(&mut svc, "/work/loose", "loose", GraphfileMode::Include);
    assert_eq!(codes(&outside.notes), ["graphfile_no_repo"]);
    assert_eq!(
        outside.notes[0].slot("fix").as_deref(),
        Some("umx graph new /work/loose --alias loose --no-graphfile")
    );
    // Architect, s17: no wiring is never a quiet no-op; nothing is made.
    let mut unwired = m.services_with(None);
    let refused = unwired
        .graph_new_with(
            &ctx(),
            "/work/repo/x",
            "x",
            None,
            true,
            None,
            GraphfileMode::Add,
        )
        .unwrap_err();
    assert_eq!(refused.code, "internal");
    assert!(unwired.graph_list().is_empty());
    assert_eq!(
        unwired
            .graph_scan(&ctx(), REPO, &ScanRequest::default())
            .unwrap_err()
            .code,
        "internal"
    );
}

#[test]
fn scan_reports_each_state_and_add_writes_new_and_moved_only() {
    let m = Machine::new();
    let mut svc = m.services();
    let listed = m.graph_at(&mut svc, "/work/repo/listed", "listed");
    let moved = m.graph_at(&mut svc, "/work/repo/now_here", "moved");
    let other = m.graph_at(&mut svc, "/work/repo/mismatch", "other");
    let dup = m.graph_at(&mut svc, "/work/repo/dup_a", "dup");
    for file in [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges] {
        let bytes = m
            .files
            .raw_for_tests(&dir("/work/repo/dup_a"), file)
            .unwrap();
        m.files
            .tamper_for_tests(&dir("/work/repo/dup_b"), file, &bytes);
    }
    m.graph_at(&mut svc, "/work/repo/fresh", "fresh");
    svc.graph_new_with(
        &ctx(),
        "/work/repo/hidden",
        "hidden",
        None,
        true,
        secured(),
        GraphfileMode::Skip,
    )
    .unwrap();
    m.files.add_repo_for_tests(dir("/work/repo/sub"));
    m.graph_at(&mut svc, "/work/repo/sub/theirs", "theirs");
    let mut gf = Graphfile::new();
    gf.add(Entry::new("listed".into(), "listed".into(), listed))
        .unwrap();
    gf.add(Entry::new("was_here".into(), "moved".into(), moved))
        .unwrap();
    gf.add(Entry::new("mismatch".into(), "mismatch".into(), Id(77)))
        .unwrap();
    gf.add(Entry::new("gone".into(), "gone".into(), Id(78)))
        .unwrap();
    gf.add(Entry::new("dup_a".into(), "dup".into(), dup))
        .unwrap();
    m.seed(&gf);
    let before = m.graphfile().unwrap();

    let dry = scan(&svc, ScanRequest::default());
    assert_eq!(
        states(&dry.value),
        [
            ("dup_a".to_string(), "listed"),
            ("dup_b".into(), "duplicate_id"),
            ("fresh".into(), "new"),
            ("gone".into(), "missing"),
            ("hidden".into(), "new"),
            ("listed".into(), "listed"),
            ("mismatch".into(), "id_mismatch"),
            ("now_here".into(), "moved"),
            ("sub".into(), "nested_repo"),
        ]
    );
    let row = |p: &str| dry.value.rows.iter().find(|r| r.path == p).unwrap();
    assert_eq!(row("now_here").from.as_deref(), Some("was_here"));
    assert_eq!(row("now_here").unique, Some(true), "a complete scan knows");
    assert_eq!(row("mismatch").found_id, Some(other));
    assert!(row("hidden").secured && !row("fresh").secured);
    assert_eq!(
        row("sub")
            .next
            .as_ref()
            .map(|c| c.as_str().to_string())
            .as_deref(),
        Some("umx graph scan /work/repo/sub")
    );
    assert_eq!(m.graphfile().unwrap(), before, "a dry scan writes nothing");

    let add = scan(
        &svc,
        ScanRequest {
            add: true,
            ..ScanRequest::default()
        },
    );
    assert_eq!(codes(&add.notes), ["graphfile_skipped_secured"]);
    assert_eq!(
        (add.value.added, add.value.moved, add.value.written),
        (1, 1, true)
    );
    let gf = Graphfile::decode(m.graphfile().unwrap().as_bytes()).unwrap();
    let paths: Vec<&str> = gf.entries().iter().map(|e| e.path.as_str()).collect();
    assert_eq!(
        paths,
        ["dup_a", "fresh", "gone", "listed", "mismatch", "now_here"],
        "missing is kept"
    );
    let again = scan(
        &svc,
        ScanRequest {
            add: true,
            ..ScanRequest::default()
        },
    );
    assert!(!again.value.written, "nothing new: no write");
}

#[test]
fn a_hand_edited_graphfile_is_rewritten_only_when_asked_and_drop_of_an_unknown_alias_changes_nothing()
 {
    let m = Machine::new();
    let mut svc = m.services();
    new_graph(&mut svc, "/work/repo/a", "a", GraphfileMode::Add);
    let hand = m
        .graphfile()
        .unwrap()
        .replace("graphfile = 1\n", "graphfile = 1 # ours\n");
    m.config
        .seed(ConfigFile::Graphfile(dir(REPO)), hand.as_bytes());
    let made = new_graph(&mut svc, "/work/repo/b", "b", GraphfileMode::Add);
    assert_eq!(codes(&made.notes), ["graphfile_not_canonical"]);
    assert_eq!(
        made.notes[0].slot("fix").as_deref(),
        Some("umx graph scan /work/repo --add --rewrite")
    );
    assert_eq!(m.graphfile().unwrap(), hand, "left as it is");
    let refused = svc
        .graph_scan(
            &ctx(),
            REPO,
            &ScanRequest {
                drop: Some("nope".into()),
                rewrite: true,
                ..ScanRequest::default()
            },
        )
        .unwrap_err();
    assert_eq!(refused.code, "graphfile_unknown");
    assert_eq!(
        m.graphfile().unwrap(),
        hand,
        "an unknown --drop changes nothing"
    );
    let rewrite = scan(
        &svc,
        ScanRequest {
            add: true,
            rewrite: true,
            ..ScanRequest::default()
        },
    );
    assert!(rewrite.value.written && rewrite.value.added == 1);
    assert!(Graphfile::is_canonical(m.graphfile().unwrap().as_bytes()));
}

#[test]
fn an_invalid_or_newer_or_missing_graphfile_is_not_used() {
    let m = Machine::new();
    let mut svc = m.services();
    assert_eq!(
        svc.graph_list_graphfile(REPO).unwrap_err().code,
        "graphfile_missing"
    );
    let id = m.graph_at(&mut svc, "/work/repo/a", "a");
    let twice = format!(
        "graphfile = 1\n\n[[graph]]\npath = \"a\"\nalias = \"a\"\nid = \"{t}\"\n\n[[graph]]\npath = \"a\"\nalias = \"b\"\nid = \"{t}\"\n",
        t = id.to_text()
    );
    m.config
        .seed(ConfigFile::Graphfile(dir(REPO)), twice.as_bytes());
    let invalid = svc.graph_list_graphfile(REPO).unwrap_err();
    assert_eq!(invalid.code, "graphfile_invalid");
    assert_eq!(
        invalid.slot("why").as_deref(),
        Some("lines 3 and 8: a path is listed twice")
    );
    assert_eq!(
        svc.graph_load_graphfile(&ctx(), REPO, None)
            .unwrap_err()
            .code,
        "graphfile_invalid"
    );
    assert!(svc.graph_list().is_empty(), "nothing from it is used");
    let scanned = scan(&svc, ScanRequest::default());
    assert_eq!(codes(&scanned.notes), ["graphfile_unusable"]);
    assert_eq!(
        states(&scanned.value),
        [("a".to_string(), "new")],
        "scan still lists"
    );
    let made = new_graph(&mut svc, "/work/repo/c", "c", GraphfileMode::Add);
    assert_eq!(codes(&made.notes), ["graphfile_unusable"]);
    assert_eq!(m.graphfile().unwrap(), twice, "never rewritten");
    m.config
        .seed(ConfigFile::Graphfile(dir(REPO)), b"graphfile = 2\n");
    assert_eq!(
        svc.graph_list_graphfile(REPO).unwrap_err().code,
        "graphfile_newer"
    );
}

#[test]
fn load_all_loads_each_entry_and_one_clash_or_mismatch_never_stops_the_others() {
    let m = Machine::new();
    // Made by another daemon (its own index), so this one has never loaded them.
    let mut maker = m.services();
    let a = m.graph_at(&mut maker, "/work/repo/a", "a");
    let b = m.graph_at(&mut maker, "/work/repo/b", "b");
    let c = m.graph_at(&mut maker, "/work/repo/c", "c");
    m.graph_at(&mut maker, "/work/elsewhere", "taken");
    let mut svc = m.services();
    svc.graph_load(&ctx(), "/work/elsewhere", None).unwrap();
    let mut gf = Graphfile::new();
    gf.add(Entry::new("a".into(), "a".into(), a)).unwrap();
    gf.add(Entry::new("b".into(), "taken".into(), b)).unwrap();
    gf.add(Entry::new("c".into(), "c".into(), Id(99))).unwrap();
    gf.add(Entry::new("d".into(), "d".into(), Id(98))).unwrap();
    assert!(gf.put_set("ac", &["a".into(), "c".into()]));
    m.seed(&gf);

    assert_eq!(
        svc.graph_load_graphfile(&ctx(), REPO, Some("nope"))
            .unwrap_err()
            .code,
        "graphfile_unknown"
    );
    let set = svc.graph_load_graphfile(&ctx(), REPO, Some("ac")).unwrap();
    let loaded: Vec<&str> = set.value.loaded.iter().map(|l| l.alias.as_str()).collect();
    assert_eq!(loaded, ["a"]);
    assert!(
        set.value.loaded[0].new_to_machine,
        "a was never loaded here"
    );
    assert_eq!(codes(&set.notes), ["graphfile_id_mismatch"]);
    let _ = c;

    let all = svc.graph_load_graphfile(&ctx(), REPO, None).unwrap();
    let loaded: Vec<(&str, bool)> = all
        .value
        .loaded
        .iter()
        .map(|l| (l.alias.as_str(), l.already))
        .collect();
    assert_eq!(
        loaded,
        [("a", true)],
        "the same folder again is not a clash"
    );
    let not: Vec<(&str, &str)> = all
        .value
        .not_loaded
        .iter()
        .map(|n| (n.alias.as_str(), n.code))
        .collect();
    assert_eq!(
        not,
        [
            ("taken", "alias_in_use"),
            ("c", "graphfile_id_mismatch"),
            ("d", "graph_not_found")
        ]
    );
    let clash = all
        .notes
        .iter()
        .find(|w| {
            w.code == "graphfile_entry_skipped" && w.slot("why").as_deref() == Some("alias_in_use")
        })
        .expect("the clash is one warning");
    assert_eq!(
        clash.slot("fix").as_deref(),
        Some("umx graph load /work/repo/b --as taken_2")
    );
    let listing = svc.graph_list_graphfile(REPO).unwrap();
    let rows: Vec<(&str, &str)> = listing
        .rows
        .iter()
        .map(|r| (r.alias.as_str(), r.state))
        .collect();
    assert_eq!(
        rows,
        [
            ("a", "loaded"),
            ("taken", "not_loaded"),
            ("c", "not_loaded"),
            ("d", "missing")
        ]
    );
    assert_eq!(listing.sets["ac"], ["a", "c"]);
}

/// A config files port whose swap always finds the file changed: another writer, every time.
struct Racing(Arc<FakeConfigFiles>);

impl ConfigFiles for Racing {
    fn adapter_id(&self) -> &'static str {
        "fake.config-files"
    }
    fn path(&self, file: &ConfigFile) -> GraphDir {
        self.0.path(file)
    }
    fn read(&self, file: &ConfigFile) -> Result<Option<Vec<u8>>, ConfigFilesError> {
        self.0.read(file)
    }
    fn write(&self, file: &ConfigFile, bytes: &[u8]) -> Result<(), ConfigFilesError> {
        self.0.write(file, bytes)
    }
    fn swap(
        &self,
        _: &ConfigFile,
        _: Option<&[u8]>,
        _: &[u8],
    ) -> Result<Swapped, ConfigFilesError> {
        Ok(Swapped::Changed)
    }
    fn repo_root(&self, dir: &GraphDir) -> Result<Option<GraphDir>, ConfigFilesError> {
        self.0.repo_root(dir)
    }
}

/// Security, s17: a file that changed under umx is tried once more, then left with `graphfile_changed`.
#[test]
fn a_graphfile_that_keeps_changing_is_left_with_a_warning() {
    let m = Machine::new();
    let mut svc = m.services_with(Some(Arc::new(Racing(m.config.clone()))));
    let made = new_graph(&mut svc, "/work/repo/a", "a", GraphfileMode::Add);
    assert_eq!(codes(&made.notes), ["graphfile_changed"]);
    assert_eq!(
        made.notes[0].slot("fix").as_deref(),
        Some("umx graph scan /work/repo --add")
    );
    assert_eq!(m.graphfile(), None);
}

#[test]
fn a_write_over_a_cap_is_refused_and_changes_nothing() {
    let m = Machine::new();
    let mut svc = m.services();
    let mut gf = Graphfile::new();
    for n in 0..umx_core::graphfile::MAX_ENTRIES {
        gf.add(Entry::new(
            format!("g{n}"),
            format!("g{n}"),
            Id(n as u128 + 1000),
        ))
        .unwrap();
    }
    m.seed(&gf);
    let before = m.graphfile().unwrap();
    let made = new_graph(
        &mut svc,
        "/work/repo/one_more",
        "one_more",
        GraphfileMode::Add,
    );
    assert_eq!(codes(&made.notes), ["graphfile_too_large"]);
    assert_eq!(m.graphfile().unwrap(), before);
}

/// R289-1: a new graph whose alias the Graphfile already has gets a suffix that keeps it a valid alias, even at the
/// 64-character cap, so the file umx writes is one it can read; the reply shows the alias written
/// (graph-addressing-expert).
#[test]
fn a_clashing_alias_at_the_cap_gets_a_suffix_that_stays_valid_and_the_reply_names_it() {
    let m = Machine::new();
    let mut svc = m.services();
    let (long, longer) = ("a".repeat(63), "c".repeat(64));
    let mut gf = Graphfile::new();
    gf.add(Entry::new("held_a".into(), long.clone(), Id(5)))
        .unwrap();
    gf.add(Entry::new("held_c".into(), longer.clone(), Id(6)))
        .unwrap();
    m.seed(&gf);
    m.graph_at(&mut svc, "/work/repo/again_a", &long);
    m.graph_at(&mut svc, "/work/repo/again_c", &longer);
    let add = scan(
        &svc,
        ScanRequest {
            add: true,
            ..ScanRequest::default()
        },
    );
    assert!(add.notes.is_empty(), "{:?}", add.notes);
    assert_eq!((add.value.added, add.value.written), (2, true));
    let gf = Graphfile::decode(m.graphfile().unwrap().as_bytes()).expect("umx reads what it wrote");
    let alias = |p: &str| gf.by_path(p).unwrap().alias.clone();
    let (a2, c2) = (
        format!("{}_2", "a".repeat(62)),
        format!("{}_2", "c".repeat(62)),
    );
    assert_eq!(
        (alias("again_a"), alias("again_c")),
        (a2.clone(), c2.clone())
    );
    assert_eq!((alias("held_a"), alias("held_c")), (long, longer), "kept");
    let row = |p: &str| {
        add.value
            .rows
            .iter()
            .find(|r| r.path == p)
            .unwrap()
            .alias
            .clone()
    };
    assert_eq!((row("again_a"), row("again_c")), (Some(a2), Some(c2)));
}

/// R289-2: with no folder named, scan covers the whole repo from its top wherever the folder given is; a named
/// folder narrows it.
#[test]
fn a_bare_scan_from_a_subfolder_covers_the_whole_repo() {
    let m = Machine::new();
    let mut svc = m.services();
    m.graph_at(&mut svc, "/work/repo/top_level", "top_level");
    m.graph_at(&mut svc, "/work/repo/docs/inner", "inner");
    let at = |whole_repo| {
        let req = ScanRequest {
            whole_repo,
            ..ScanRequest::default()
        };
        let done = svc.graph_scan(&ctx(), "/work/repo/docs", &req).unwrap();
        states(&done.value)
    };
    assert_eq!(
        at(true),
        [
            ("docs/inner".to_string(), "new"),
            ("top_level".into(), "new")
        ]
    );
    assert_eq!(at(false), [("docs/inner".to_string(), "new")]);
}

/// R289-3: a scan that stopped at a cap, or could not read a graph.json, proves nothing about what it did not see.
/// An entry is missing or moved only when its own folder holds no graph, so --add never points a live graph's entry
/// at a shallow copy.
#[test]
fn an_incomplete_scan_never_moves_a_live_entry_to_a_copy() {
    let m = Machine::new();
    let mut svc = m.services();
    let deep = m.graph_at(&mut svc, "/work/repo/d1/d2/deep", "deep");
    let broken = m.graph_at(&mut svc, "/work/repo/broken", "broken");
    for (from, to) in [("d1/d2/deep", "deep_copy"), ("broken", "broken_copy")] {
        for file in [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges] {
            let bytes = m
                .files
                .raw_for_tests(&dir(&format!("{REPO}/{from}")), file)
                .unwrap();
            m.files
                .tamper_for_tests(&dir(&format!("{REPO}/{to}")), file, &bytes);
        }
    }
    m.files
        .tamper_for_tests(&dir("/work/repo/broken"), GraphFile::Graph, b"not json");
    let mut gf = Graphfile::new();
    gf.add(Entry::new("d1/d2/deep".into(), "deep".into(), deep))
        .unwrap();
    gf.add(Entry::new("broken".into(), "broken".into(), broken))
        .unwrap();
    m.seed(&gf);
    let before = m.graphfile().unwrap();
    let shallow = ScanRequest {
        add: true,
        depth: Some(1),
        ..ScanRequest::default()
    };
    let add = scan(&svc, shallow);
    assert!(
        add.value.truncated && add.value.unreadable == 1,
        "{:?}",
        add.value
    );
    assert_eq!(
        states(&add.value),
        [
            ("broken".to_string(), "unreadable"),
            ("broken_copy".into(), "duplicate_id"),
            ("d1/d2/deep".into(), "listed"),
            ("deep_copy".into(), "duplicate_id"),
        ]
    );
    assert!(!add.value.written && add.value.moved == 0);
    assert_eq!(m.graphfile().unwrap(), before, "byte for byte");
}

/// graph-storage-expert, s17: a scan only looks. A graph folder whose write was cut short (temp files and a marker
/// left) is reported pending, whether the walk reaches it or only the entry's own folder is looked at, and nothing
/// in it is rolled forward, rolled back or removed; nothing is written.
#[test]
fn a_scan_reports_a_cut_short_write_as_pending_and_never_repairs_it() {
    let m = Machine::new();
    let mut svc = m.services();
    let deep = m.graph_at(&mut svc, "/work/repo/d1/d2/deep", "deep");
    m.graph_at(&mut svc, "/work/repo/loose", "loose");
    let mut gf = Graphfile::new();
    gf.add(Entry::new("d1/d2/deep".into(), "deep".into(), deep))
        .unwrap();
    m.seed(&gf);
    let folders = [dir("/work/repo/d1/d2/deep"), dir("/work/repo/loose")];
    for d in &folders {
        m.files.cut_short_for_tests(d);
    }
    let snapshot = || {
        let files = folders
            .iter()
            .map(|d| {
                let raw = [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges]
                    .map(|f| m.files.raw_for_tests(d, f));
                (m.files.leftovers_for_tests(d), raw)
            })
            .collect::<Vec<_>>();
        (files, m.graphfile())
    };
    let before = snapshot();
    assert!(
        before.0.iter().all(|(left, _)| !left.is_empty()),
        "{before:?}"
    );
    // Depth 1: the walk stops above `deep`, so its entry's own folder is looked at; the default depth walks to both.
    for depth in [Some(1), None] {
        let add = scan(
            &svc,
            ScanRequest {
                add: true,
                depth,
                ..ScanRequest::default()
            },
        );
        assert_eq!(
            states(&add.value),
            [
                ("d1/d2/deep".to_string(), "pending"),
                ("loose".into(), "pending")
            ],
            "depth {depth:?}"
        );
        assert!(!add.value.written, "depth {depth:?}");
        assert_eq!(snapshot(), before, "byte for byte, depth {depth:?}");
    }
}

/// Security, s17: a scan that stopped early cannot know a moved graph's id is at that one path only, so the row says
/// so (unique null) and --add leaves the entry's path as it is; a complete scan moves it.
#[test]
fn a_truncated_scan_does_not_know_a_move_is_unique_and_add_leaves_it() {
    let m = Machine::new();
    let mut svc = m.services();
    let moved = m.graph_at(&mut svc, "/work/repo/now_here", "moved");
    m.files.add_folder_for_tests(dir("/work/repo/a/b"));
    let mut gf = Graphfile::new();
    gf.add(Entry::new("was_here".into(), "moved".into(), moved))
        .unwrap();
    m.seed(&gf);
    let before = m.graphfile().unwrap();
    let shallow = scan(
        &svc,
        ScanRequest {
            add: true,
            depth: Some(1),
            ..ScanRequest::default()
        },
    );
    assert!(shallow.value.truncated);
    let row = shallow
        .value
        .rows
        .iter()
        .find(|r| r.path == "now_here")
        .unwrap();
    assert_eq!((row.state, row.unique), ("moved", None));
    assert!(!shallow.value.written && shallow.value.moved == 0);
    assert_eq!(m.graphfile().unwrap(), before, "byte for byte");
    let full = scan(
        &svc,
        ScanRequest {
            add: true,
            ..ScanRequest::default()
        },
    );
    let row = full
        .value
        .rows
        .iter()
        .find(|r| r.path == "now_here")
        .unwrap();
    assert_eq!((row.state, row.unique), ("moved", Some(true)));
    assert_eq!((full.value.moved, full.value.written), (1, true));
}

/// F176: a graph whose creation was cut short holds only the marker and temp files, no graph.json yet. Scan
/// reports it pending (no alias or id), whether the walk meets it or only its entry's own folder is looked at; an
/// entry pointing there is never missing or moved to a copy, and nothing in the folder changes.
#[test]
fn a_creation_cut_short_is_pending_never_missing_or_moved() {
    let m = Machine::new();
    let mut svc = m.services();
    let made = m.graph_at(&mut svc, "/work/repo/copy", "made");
    let (fresh, deep) = (dir("/work/repo/fresh"), dir("/work/repo/d1/d2/made"));
    for d in [&fresh, &deep] {
        m.files.add_folder_for_tests(d.clone());
        m.files.cut_short_for_tests(d);
    }
    let mut gf = Graphfile::new();
    gf.add(Entry::new("d1/d2/made".into(), "made".into(), made))
        .unwrap();
    m.seed(&gf);
    let snapshot = || {
        let left = [&fresh, &deep].map(|d| m.files.leftovers_for_tests(d));
        let json = [&fresh, &deep].map(|d| m.files.raw_for_tests(d, GraphFile::Graph));
        (left, json, m.graphfile())
    };
    let before = snapshot();
    assert!(
        before.0.iter().all(|l| !l.is_empty()) && before.1 == [None, None],
        "{before:?}"
    );
    for depth in [Some(1), None] {
        let add = scan(
            &svc,
            ScanRequest {
                add: true,
                depth,
                ..ScanRequest::default()
            },
        );
        assert_eq!(
            states(&add.value),
            [
                ("copy".to_string(), "duplicate_id"),
                ("d1/d2/made".into(), "pending"),
                ("fresh".into(), "pending")
            ],
            "depth {depth:?}"
        );
        let fresh_row = add.value.rows.iter().find(|r| r.path == "fresh").unwrap();
        assert_eq!((&fresh_row.alias, fresh_row.id), (&None, None));
        assert!(!add.value.written, "depth {depth:?}");
        assert_eq!(snapshot(), before, "byte for byte, depth {depth:?}");
    }
}

/// F177: a move is unique only when the scan hid nothing. A pending or unreadable graph met only in an entry's own
/// folder (here a dot-folder the walk skips; each arm on its own, F178), or a folder the walk could not list, leaves
/// `unique` null, and --add does not move the entry.
#[test]
fn a_fallback_or_unlisted_unknown_keeps_a_move_from_being_unique() {
    for case in ["fallback pending", "fallback unreadable", "unlisted"] {
        let m = Machine::new();
        let mut svc = m.services();
        let moved = m.graph_at(&mut svc, "/work/repo/now_here", "moved");
        let mut gf = Graphfile::new();
        gf.add(Entry::new("was_here".into(), "moved".into(), moved))
            .unwrap();
        if case == "fallback pending" {
            let hidden = m.graph_at(&mut svc, "/work/repo/.hidden/p", "p");
            m.files.cut_short_for_tests(&dir("/work/repo/.hidden/p"));
            gf.add(Entry::new(".hidden/p".into(), "p".into(), hidden))
                .unwrap();
        } else if case == "fallback unreadable" {
            // F178 (review N310-2): a graph.json umx cannot read, in a folder only the entry's own look reaches.
            let hidden = m.graph_at(&mut svc, "/work/repo/.hidden/u", "u");
            m.files
                .tamper_for_tests(&dir("/work/repo/.hidden/u"), GraphFile::Graph, b"not json");
            gf.add(Entry::new(".hidden/u".into(), "u".into(), hidden))
                .unwrap();
        } else {
            m.files.add_folder_for_tests(dir("/work/repo/shut/x"));
            m.files.unlistable_for_tests(dir("/work/repo/shut"), true);
        }
        m.seed(&gf);
        let before = m.graphfile().unwrap();
        let add = scan(
            &svc,
            ScanRequest {
                add: true,
                ..ScanRequest::default()
            },
        );
        assert!(
            !add.value.truncated,
            "{case}: the walk itself was not cut short"
        );
        let row = add
            .value
            .rows
            .iter()
            .find(|r| r.path == "now_here")
            .unwrap();
        assert_eq!((row.state, row.unique), ("moved", None), "{case}");
        assert!(!add.value.written && add.value.moved == 0, "{case}");
        assert_eq!(m.graphfile().unwrap(), before, "{case}: byte for byte");
        if case == "unlisted" {
            assert_eq!(add.value.unreadable, 1, "{case}");
        }
        if case == "fallback unreadable" {
            assert_eq!(
                add.value.unreadable, 0,
                "{case}: the walk itself read everything it saw"
            );
            let own = add
                .value
                .rows
                .iter()
                .find(|r| r.path == ".hidden/u")
                .unwrap();
            assert_eq!(own.state, "unreadable", "{case}");
        }
    }
}
