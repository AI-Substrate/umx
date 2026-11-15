//! This machine's graph registry (req-56's first slice, ws002 F72): graphs come back when the daemon starts, in load
//! order, through the explicit load path; on-demand loads of remembered graphs (req-15).

use std::sync::Arc;
use std::sync::Mutex;
use umx_core::codec::GraphFile;
use umx_core::ports::{GraphDir, LocalState, LocalStateError, StateWritten};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{Ctx, Ports, Services};

const MINUTE: u64 = 60_000;

/// What survives a daemon restart: the graph files, the policy record and the registry.
struct Machine {
    files: Arc<FakeGraphFiles>,
    ids: Arc<FakeIds>,
    state: Arc<FakeLocalState>,
    registry: Arc<FakeLocalState>,
    clock: Arc<FakeClock>,
}

impl Machine {
    fn new() -> Machine {
        Machine {
            files: Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap())),
            ids: Arc::new(FakeIds::new()),
            state: Arc::new(FakeLocalState::new()),
            registry: Arc::new(FakeLocalState::new().with_absent_ok(true)),
            clock: Arc::new(FakeClock::default()),
        }
    }

    /// A daemon's services over this machine, as after a start.
    fn daemon(&self) -> Services {
        self.daemon_with(self.registry.clone())
    }

    /// [`Machine::daemon`] with `registry` as its registry port.
    fn daemon_with(&self, registry: Arc<dyn LocalState>) -> Services {
        Services::new(Ports {
            files: self.files.clone(),
            vector_cache: Arc::new(FakeGraphFiles::new(GraphDir::parse("/cache").unwrap())),
            vector_cache_root: GraphDir::parse("/cache").unwrap(),
            clock: self.clock.clone(),
            ids: self.ids.clone(),
            state: self.state.clone(),
            scopes: Arc::new(FakeLocalState::new()),
            registry,
            sources: Arc::new(umx_fakes::FakeSourceReader::open(
                GraphDir::parse("/").unwrap(),
            )),
            allows: Arc::new(FakeLocalState::new()),
            lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
        })
    }

    /// Copies one folder's graph files into another, as a person would.
    fn copy(&self, from: &str, to: &str) {
        let (from, to) = (dir(from), dir(to));
        for file in [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges] {
            let bytes = self.files.raw_for_tests(&from, file).unwrap_or_default();
            self.files.tamper_for_tests(&to, file, &bytes);
        }
    }

    fn registry_text(&self) -> String {
        String::from_utf8(self.registry.bytes().unwrap_or_default()).unwrap()
    }
}

fn dir(path: &str) -> GraphDir {
    GraphDir::parse(path).unwrap()
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

fn codes(warnings: &[umx_core::Problem]) -> Vec<&'static str> {
    warnings.iter().map(|w| w.code).collect()
}

fn aliases(svc: &Services) -> Vec<String> {
    svc.graph_list().into_iter().map(|g| g.alias).collect()
}

#[test]
fn graphs_come_back_in_load_order_and_an_unloaded_one_stays_away() {
    let m = Machine::new();
    let mut first = m.daemon();
    first
        .graph_new(&ctx(), "/work/a", "alpha", None, false, None)
        .unwrap();
    m.copy("/work/a", "/work/b");
    first.graph_load(&ctx(), "/work/b", Some("beta")).unwrap();
    first
        .graph_new(&ctx(), "/work/c", "gamma", None, false, None)
        .unwrap();
    first
        .graph_new(&ctx(), "/work/d", "delta", None, false, None)
        .unwrap();
    first.graph_unload(&ctx(), "delta").unwrap();
    // A reload of a folder already loaded keeps its row's place (d-21).
    first.graph_load(&ctx(), "/work/a", None).unwrap();
    drop(first);

    let mut second = m.daemon();
    let report = second.reload_remembered(&ctx(), None).clone();
    assert_eq!(
        report
            .reloaded
            .iter()
            .map(|r| r.alias.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "beta", "gamma"],
        "in load order, so alpha stays the primary copy"
    );
    assert!(report.not_reloaded.is_empty() && report.unreadable.is_none());
    assert_eq!(aliases(&second), ["alpha", "beta", "gamma"]);
    // The registry holds ids, aliases, folders and the loaded flag, and since s16 a display-only policy hint per row
    // (security, plan 098); never a policy record, and nothing reads the hint back as a policy.
    let text = m.registry_text();
    assert!(text.contains("umx_registry = 1"), "{text}");
    assert!(
        text.lines()
            .filter(|l| l.contains("policy"))
            .all(|l| l == "policy = \"open\""),
        "{text}"
    );
    // delta was unloaded on purpose: it does not come back, and nothing loads it on demand.
    assert!(second.graph().remembered_loaded("delta").is_none());
    let refused = second.load_registered(&ctx(), "delta").unwrap_err();
    assert_eq!(refused.code, "graph_not_loaded");
    assert_eq!(refused.subject.as_deref(), Some("delta"));
}

#[test]
fn a_folder_holding_another_graph_is_not_loaded_under_the_old_alias() {
    let m = Machine::new();
    let mut first = m.daemon();
    first
        .graph_new(&ctx(), "/work/a", "alpha", None, false, None)
        .unwrap();
    first
        .graph_new(&ctx(), "/work/other", "other", None, false, None)
        .unwrap();
    first
        .graph_new(&ctx(), "/work/c", "gamma", None, false, None)
        .unwrap();
    drop(first);
    m.copy("/work/other", "/work/a");

    let mut second = m.daemon();
    let report = second.reload_remembered(&ctx(), None).clone();
    assert_eq!(report.not_reloaded.len(), 1);
    assert_eq!(
        (
            report.not_reloaded[0].alias.as_str(),
            report.not_reloaded[0].code
        ),
        ("alpha", "graph_id_changed")
    );
    assert_eq!(aliases(&second), ["gamma", "other"], "the rest reload");
    // Named again, the on-demand load refuses with the load's own error, and graph_not_loaded says why.
    let refused = second.load_registered(&ctx(), "alpha").unwrap_err();
    assert_eq!(refused.code, "graph_id_changed");
    let not = second.graph().not_loaded("alpha");
    let why = not.slot("why").unwrap();
    assert!(
        why.contains("did not reload") && why.contains("another graph"),
        "{why}"
    );
    let fix = not.slot("fix").unwrap();
    assert!(fix.contains("graph load /work/a --as alpha"), "{fix}");
    // Unloading the remembered alias forgets it.
    second.graph_unload(&ctx(), "alpha").unwrap();
    assert!(second.graph().remembered_loaded("alpha").is_none());
    assert!(second.graph().reload_report().not_reloaded.is_empty());
}

#[test]
fn an_unusable_registry_reloads_nothing_and_is_never_rewritten() {
    let m = Machine::new();
    m.registry.set_bytes(b"umx_registry = 2\nfuture = true\n");
    let mut svc = m.daemon();
    let report = svc.reload_remembered(&ctx(), None).clone();
    assert!(report.unreadable.unwrap().contains("newer"));
    let made = svc
        .graph_new(&ctx(), "/work/a", "alpha", None, false, None)
        .unwrap();
    assert_eq!(codes(&made.notes), ["registry_unreadable"]);
    assert_eq!(m.registry_text(), "umx_registry = 2\nfuture = true\n");
    m.registry.refuse(Some("open to others"));
    let report = svc.reload_remembered(&ctx(), None).clone();
    assert_eq!(report.unreadable.as_deref(), Some("open to others"));
}

#[test]
fn a_bad_row_is_skipped_and_the_others_reload() {
    let m = Machine::new();
    let mut first = m.daemon();
    first
        .graph_new(&ctx(), "/work/a", "alpha", None, false, None)
        .unwrap();
    drop(first);
    let text = m.registry_text().replacen(
        "umx_registry = 1\n",
        "umx_registry = 1\n\n[[graphs]]\nalias = 'No Good'\nfolder = 'relative'\nloaded = true\n",
        1,
    );
    m.registry.set_bytes(text.as_bytes());
    let mut second = m.daemon();
    let report = second.reload_remembered(&ctx(), None).clone();
    assert_eq!(report.skipped_rows, [1]);
    assert_eq!(aliases(&second), ["alpha"]);
}

#[test]
fn idle_unload_keeps_the_row_and_a_named_graph_loads_on_demand_once() {
    let m = Machine::new();
    let mut svc = m.daemon();
    svc.graph_new(&ctx(), "/work/a", "alpha", None, false, None)
        .unwrap();
    svc.graph_new(&ctx(), "/work/b", "beta", None, false, None)
        .unwrap();
    svc.set_idle_unload_ms(MINUTE);
    m.clock.advance(MINUTE);
    assert_eq!(svc.unload_idle(), ["alpha", "beta"]);
    let not = svc.graph().not_loaded("alpha");
    assert_eq!(
        not.slot("why").as_deref(),
        Some("it unloaded after being idle")
    );
    assert!(svc.graph().remembered_loaded("alpha").is_some());
    let done = svc.load_registered(&ctx(), "alpha").unwrap();
    assert!(done.value.is_some());
    assert_eq!(codes(&done.notes)[0], "graph_loaded_on_demand");
    // A second miss on the same alias finds it loaded and loads nothing.
    let again = svc.load_registered(&ctx(), "alpha").unwrap();
    assert!(again.value.is_none() && again.notes.is_empty());
    // Its load order is now the latest: its row moved after beta's.
    let text = m.registry_text();
    assert!(
        text.find("/work/b").unwrap() < text.find("/work/a").unwrap(),
        "{text}"
    );
}

#[test]
fn a_full_or_unwritable_registry_warns_and_the_load_stands() {
    let m = Machine::new();
    let mut rows = String::from("umx_registry = 1\n");
    for n in 0..umx_core::registry::MAX_ROWS {
        rows += &format!(
            "[[graphs]]\nid = '{}'\nalias = 'g{n}'\nfolder = '/work/g{n}'\nloaded = true\n",
            umx_core::Id(n as u128 + 1000)
        );
    }
    m.registry.set_bytes(rows.as_bytes());
    let mut svc = m.daemon();
    let made = svc
        .graph_new(&ctx(), "/work/a", "alpha", None, false, None)
        .unwrap();
    assert_eq!(codes(&made.notes), ["registry_not_saved"]);
    assert_eq!(m.registry_text(), rows, "unchanged");
    m.registry.set_bytes(b"umx_registry = 1\n");
    m.registry.fail_writes(Some("disk full"));
    let made = svc
        .graph_new(&ctx(), "/work/b", "beta", None, false, None)
        .unwrap();
    assert_eq!(codes(&made.notes), ["registry_not_saved"]);
    assert_eq!(aliases(&svc), ["alpha", "beta"]);
}

/// Review R156-B2: an alias reused after an explicit unload still loads on demand once the new graph is idle.
#[test]
fn a_reused_alias_loads_its_new_folder_on_demand() {
    let m = Machine::new();
    let mut svc = m.daemon();
    svc.graph_new(&ctx(), "/work/old", "demo", None, false, None)
        .unwrap();
    svc.graph_unload(&ctx(), "demo").unwrap();
    svc.graph_new(&ctx(), "/work/new", "demo", None, false, None)
        .unwrap();
    svc.set_idle_unload_ms(MINUTE);
    m.clock.advance(MINUTE);
    assert_eq!(svc.unload_idle(), ["demo"]);
    let row = svc.graph().remembered_loaded("demo").unwrap();
    assert_eq!(row.folder, "/work/new");
    let done = svc.load_registered(&ctx(), "demo").unwrap();
    assert_eq!(done.value.unwrap().graph.path, "/work/new");
    // Reused while the old graph was idle, not unloaded: the alias still names one folder.
    m.clock.advance(MINUTE);
    assert_eq!(svc.unload_idle(), ["demo"]);
    svc.graph_new(&ctx(), "/work/newer", "demo", None, false, None)
        .unwrap();
    m.clock.advance(MINUTE);
    svc.unload_idle();
    let row = svc.graph().remembered_loaded("demo").unwrap();
    assert_eq!(row.folder, "/work/newer");
}

/// The `[[graphs]]` block of `alias` in the registry text.
fn block<'a>(text: &'a str, alias: &str) -> &'a str {
    let at = format!("alias = \"{alias}\"");
    text.split("[[graphs]]")
        .find(|b| b.contains(&at))
        .unwrap_or_else(|| panic!("no {alias} in {text}"))
}

/// Review R193-B1: graph forget changes only the index, so a write that does not replace it is an error and the
/// file and the reload report stay as they were. A replace whose folder sync fails still forgets, with a warning.
#[test]
fn a_forget_whose_write_fails_forgets_nothing() {
    let m = Machine::new();
    let mut first = m.daemon();
    first
        .graph_new(&ctx(), "/work/b", "beta", None, false, None)
        .unwrap();
    drop(first);
    m.files
        .tamper_for_tests(&dir("/work/b"), GraphFile::Graph, b"not json");
    let mut second = m.daemon();
    let report = second.reload_remembered(&ctx(), None).clone();
    assert_eq!(report.not_reloaded.len(), 1, "{report:?}");

    let before = m.registry.bytes();
    m.registry.fail_writes(Some("disk full"));
    let refused = second.graph_forget(&ctx(), "beta").unwrap_err();
    assert_eq!(refused.code, "graph_forget_not_saved");
    assert_eq!(m.registry.bytes(), before, "nothing was forgotten");
    assert_eq!(
        second.graph().reload_report().not_reloaded,
        report.not_reloaded
    );

    m.registry.fail_writes(None);
    m.registry.set_not_synced(true);
    let done = second.graph_forget(&ctx(), "beta").unwrap();
    assert_eq!(done.value.rows, 1);
    assert_eq!(codes(&done.notes), ["local_state_not_durable"]);
    assert!(second.graph().reload_report().not_reloaded.is_empty());
    assert!(!m.registry_text().contains("beta"));
}

/// Review R193-B1 (F80): at start, a no-primary mark that cannot be cleared is reported, never fatal, and stays in
/// the file for the next start.
#[test]
fn a_no_primary_mark_that_cannot_be_cleared_at_start_is_reported() {
    let m = Machine::new();
    let mut first = m.daemon();
    first
        .graph_new(&ctx(), "/work/a", "alpha", None, false, None)
        .unwrap();
    for (to, alias) in [("/work/b", "beta"), ("/work/c", "gamma")] {
        m.copy("/work/a", to);
        first.graph_load(&ctx(), to, Some(alias)).unwrap();
    }
    first.graph_unload(&ctx(), "alpha").unwrap();
    drop(first);
    assert!(
        m.registry_text().contains("no_primary"),
        "{}",
        m.registry_text()
    );
    m.files
        .tamper_for_tests(&dir("/work/c"), GraphFile::Graph, b"not json");
    let before = m.registry.bytes();
    m.registry.fail_writes(Some("disk full"));
    let mut second = m.daemon();
    let report = second.reload_remembered(&ctx(), None).clone();
    assert_eq!(report.primary_restored, ["beta"]);
    assert_eq!(codes(&report.warnings), ["registry_update_not_saved"]);
    assert_eq!(
        m.registry.bytes(),
        before,
        "the mark stays for the next start"
    );
}

/// Review R193-B2: the start-up reload refreshes each reloaded row's index details: this load's time, and under
/// the settled policy, so a graph secured while the daemon was down loses its stored counts and model. Load order and
/// the rest of the file stay.
#[test]
fn a_restart_refreshes_and_redacts_each_reloaded_rows_details() {
    let m = Machine::new();
    m.clock.set(10 * MINUTE);
    let mut first = m.daemon();
    for (path, alias) in [("/work/a", "alpha"), ("/work/s", "sec")] {
        first
            .graph_new(&ctx(), path, alias, None, false, None)
            .unwrap();
    }
    let t1 = umx_core::time::iso_from_ms(10 * MINUTE);
    first
        .graph_set(&ctx(), "sec", &[("policy".into(), "secured".into())])
        .unwrap();
    drop(first);
    let text = m.registry_text();
    assert!(
        block(&text, "sec").contains("nodes = 0"),
        "stale before: {text}"
    );
    assert!(block(&text, "alpha").contains(&t1), "{text}");

    m.clock.set(20 * MINUTE);
    let mut second = m.daemon();
    let report = second.reload_remembered(&ctx(), None).clone();
    assert!(report.warnings.is_empty(), "{report:?}");
    let text = m.registry_text();
    let t2 = umx_core::time::iso_from_ms(20 * MINUTE);
    assert!(block(&text, "alpha").contains(&t2), "{text}");
    let sec = block(&text, "sec");
    for key in ["nodes", "edges", "model", "last_loaded"] {
        assert!(
            !sec.contains(key),
            "{key} stored for a secured graph: {sec}"
        );
    }
    assert!(sec.contains("policy = \"secured\""), "{sec}");
    assert!(
        text.find("alias = \"alpha\"") < text.find("alias = \"sec\""),
        "load order kept: {text}"
    );
}

/// Review R213-B1 (F99): a saved scope's on-demand plan is decided again, for the whole scope, when it is applied.
/// A member unloaded on purpose between the plan and the load refuses the scope with scope_graph_not_loaded, and no
/// member loads; the registry's loaded flags are untouched.
#[test]
fn a_scope_member_unloaded_after_the_plan_loads_nothing() {
    let m = Machine::new();
    let mut svc = m.daemon();
    svc.set_idle_unload_ms(MINUTE);
    for (path, alias) in [("/work/a", "alpha"), ("/work/b", "beta")] {
        svc.graph_new(&ctx(), path, alias, None, false, None)
            .unwrap();
    }
    svc.scope_save(&ctx().graph("alpha,beta"), "both", false)
        .unwrap();
    m.clock.advance(2 * MINUTE);
    assert_eq!(svc.unload_idle(), ["alpha", "beta"]);
    let plan = svc.graph().scope_load_plan("both").unwrap();
    let planned: Vec<&str> = plan.iter().map(|r| r.alias.as_str()).collect();
    assert_eq!(
        planned,
        ["alpha", "beta"],
        "both were eligible when planned"
    );

    // Another request unloads the later member on purpose before the load runs.
    svc.graph_unload(&ctx(), "beta").unwrap();
    let before = m.registry_text();
    let refused = svc.load_scope(&ctx(), "both").unwrap_err();
    assert_eq!(refused.code, "scope_graph_not_loaded");
    assert!(aliases(&svc).is_empty(), "no member loaded");
    assert_eq!(m.registry_text(), before, "loaded flags unchanged");
}

/// A registry that reads `reads` times and then refuses: the file went bad after the start-up reload read it.
struct RefusesAfter {
    inner: Arc<FakeLocalState>,
    reads: Mutex<usize>,
}

impl LocalState for RefusesAfter {
    fn adapter_id(&self) -> &'static str {
        self.inner.adapter_id()
    }
    fn path(&self) -> GraphDir {
        self.inner.path()
    }
    fn read(&self) -> Result<Option<Vec<u8>>, LocalStateError> {
        let mut reads = self.reads.lock().unwrap();
        if *reads == 0 {
            return Err(LocalStateError::Refused {
                file: self.path(),
                reason: "it is open to others",
            });
        }
        *reads -= 1;
        self.inner.read()
    }
    fn write(&self, bytes: &[u8]) -> Result<StateWritten, LocalStateError> {
        self.inner.write(bytes)
    }
}

/// The `reason` slot of each `registry_unreadable` among `warnings`.
fn unreadable(warnings: &[umx_core::Problem]) -> Vec<String> {
    warnings
        .iter()
        .filter(|w| w.code == "registry_unreadable")
        .map(|w| w.slot("reason").unwrap_or_default())
        .collect()
}

/// F103 (review note): a registry read at the start of the reload but refused afterwards is reported for each
/// step it stops (the index details, the no-primary marks), never skipped silently; the graphs still reload.
#[test]
fn a_registry_that_refuses_after_the_reload_read_is_reported() {
    let m = Machine::new();
    let mut first = m.daemon();
    first
        .graph_new(&ctx(), "/work/a", "alpha", None, false, None)
        .unwrap();
    for (to, alias) in [("/work/b", "beta"), ("/work/c", "gamma")] {
        m.copy("/work/a", to);
        first.graph_load(&ctx(), to, Some(alias)).unwrap();
    }
    first.graph_unload(&ctx(), "alpha").unwrap();
    drop(first);
    m.clock.advance(MINUTE);

    let flaky = Arc::new(RefusesAfter {
        inner: m.registry.clone(),
        reads: Mutex::new(1),
    });
    let mut second = m.daemon_with(flaky);
    let report = second.reload_remembered(&ctx(), None).clone();
    let reloaded: Vec<&str> = report.reloaded.iter().map(|r| r.alias.as_str()).collect();
    assert_eq!(reloaded, ["beta", "gamma"]);
    let reasons = unreadable(&report.warnings);
    assert_eq!(reasons.len(), 2, "{reasons:?}");
    assert!(
        reasons[0].contains("details of the reloaded graphs"),
        "{reasons:?}"
    );
    assert!(reasons[1].contains("no-primary marks"), "{reasons:?}");
}

/// F103: an unload whose registry read fails says the graph's details were not recorded.
#[test]
fn an_unload_that_cannot_read_the_registry_says_so() {
    let m = Machine::new();
    let mut svc = m.daemon();
    svc.graph_new(&ctx(), "/work/a", "alpha", None, false, None)
        .unwrap();
    m.registry.refuse(Some("it is open to others"));
    let done = svc.graph_unload(&ctx(), "alpha").unwrap();
    let reasons = unreadable(&done.notes);
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("details of graph alpha at unload")),
        "{reasons:?}"
    );
}

/// The copy a read that names no graph takes, or the refusal's code.
fn unnamed_read(svc: &Services) -> Result<String, &'static str> {
    svc.node_list(&ctx(), umx_services::NodeListRequest::default())
        .map(|done| done.value.alias)
        .map_err(|p| p.code)
}

/// F98: `graph primary` ends a no-primary state, moves the primary among loaded copies, and a restart keeps the
/// choice (the registry row moves ahead of its siblings and the no-primary mark goes).
#[test]
fn graph_primary_picks_the_copy_unnamed_reads_take_and_a_restart_keeps_it() {
    let m = Machine::new();
    let mut first = m.daemon();
    first
        .graph_new(&ctx(), "/work/a", "alpha", None, false, None)
        .unwrap();
    for (to, alias) in [("/work/b", "beta"), ("/work/c", "gamma")] {
        m.copy("/work/a", to);
        first.graph_load(&ctx(), to, Some(alias)).unwrap();
    }
    assert_eq!(unnamed_read(&first), Ok("alpha".into()), "positive control");
    first.graph_unload(&ctx(), "alpha").unwrap();
    assert_eq!(unnamed_read(&first), Err("copy_ambiguous"));

    let done = first.graph_primary(&ctx(), "gamma").unwrap();
    assert!(!done.value.unchanged && done.notes.is_empty(), "{done:?}");
    assert_eq!(unnamed_read(&first), Ok("gamma".into()));
    assert!(
        !m.registry_text().contains("no_primary"),
        "{}",
        m.registry_text()
    );
    let again = first.graph_primary(&ctx(), "gamma").unwrap();
    assert!(again.value.unchanged, "already primary");
    assert_eq!(
        first.graph_primary(&ctx(), "alpha").unwrap_err().code,
        "graph_not_loaded"
    );
    drop(first);

    let mut second = m.daemon();
    second.reload_remembered(&ctx(), None);
    assert_eq!(
        unnamed_read(&second),
        Ok("gamma".into()),
        "kept across a restart"
    );
    // Moving it back to a copy loaded earlier, and that holds across a restart too.
    assert!(
        !second
            .graph_primary(&ctx(), "beta")
            .unwrap()
            .value
            .unchanged
    );
    assert_eq!(unnamed_read(&second), Ok("beta".into()));
    drop(second);
    let mut third = m.daemon();
    third.reload_remembered(&ctx(), None);
    assert_eq!(unnamed_read(&third), Ok("beta".into()));
    third.graph_unload(&ctx(), "gamma").unwrap();
    assert!(
        third.graph_primary(&ctx(), "beta").unwrap().value.unchanged,
        "the only copy left"
    );
}

/// Two copies of one graph: alpha (primary, remembered) and a copy at /work/b loaded as beta while the registry
/// could not be written, so beta has no row. The registry works again when this returns.
fn beta_loaded_unsaved(m: &Machine, svc: &mut Services) {
    svc.graph_new(&ctx(), "/work/a", "alpha", None, false, None)
        .unwrap();
    m.copy("/work/a", "/work/b");
    m.registry.fail_writes(Some("disk full"));
    let loaded = svc.graph_load(&ctx(), "/work/b", Some("beta")).unwrap();
    assert_eq!(codes(&loaded.notes), ["registry_not_saved"]);
    m.registry.fail_writes(None);
}

/// The copy unnamed reads take after a restart, and the aliases that came back (sorted).
fn after_restart(m: &Machine) -> (Result<String, &'static str>, Vec<String>) {
    let mut svc = m.daemon();
    svc.reload_remembered(&ctx(), None);
    let mut back = aliases(&svc);
    back.sort();
    (unnamed_read(&svc), back)
}

/// Review R227-1: a copy loaded while the registry could not be written has no row; making it primary writes its
/// row, first among its graph's rows, so a restart brings it back as the primary.
#[test]
fn a_primary_whose_row_was_never_saved_gets_one() {
    let m = Machine::new();
    let mut svc = m.daemon();
    beta_loaded_unsaved(&m, &mut svc);
    assert!(!m.registry_text().contains("/work/b"), "no row: the setup");
    let done = svc.graph_primary(&ctx(), "beta").unwrap();
    assert!(!done.value.unchanged && done.notes.is_empty(), "{done:?}");
    drop(svc);
    assert_eq!(
        after_restart(&m),
        (Ok("beta".into()), vec!["alpha".into(), "beta".into()])
    );
}

/// Review R227-1: a copy unloaded, then loaded again while the registry could not be written, keeps a row that says
/// it is not loaded; making it primary marks the row loaded, so a restart brings it back as the primary.
#[test]
fn a_primary_whose_row_says_unloaded_is_marked_loaded() {
    let m = Machine::new();
    let mut svc = m.daemon();
    svc.graph_new(&ctx(), "/work/a", "alpha", None, false, None)
        .unwrap();
    m.copy("/work/a", "/work/b");
    svc.graph_load(&ctx(), "/work/b", Some("beta")).unwrap();
    svc.graph_unload(&ctx(), "beta").unwrap();
    m.registry.fail_writes(Some("disk full"));
    svc.graph_load(&ctx(), "/work/b", Some("beta")).unwrap();
    m.registry.fail_writes(None);
    let row = block(&m.registry_text(), "beta").to_string();
    assert!(row.contains("loaded = false"), "the setup: {row}");
    let done = svc.graph_primary(&ctx(), "beta").unwrap();
    assert!(done.notes.is_empty(), "{done:?}");
    drop(svc);
    assert_eq!(
        after_restart(&m),
        (Ok("beta".into()), vec!["alpha".into(), "beta".into()])
    );
}

/// Review R227 note 2: a choice whose save failed holds until the daemon stops; repeating it once the registry can be
/// written answers unchanged and writes it, so a restart keeps it.
#[test]
fn repeating_a_primary_whose_save_failed_saves_it() {
    let m = Machine::new();
    let mut svc = m.daemon();
    svc.graph_new(&ctx(), "/work/a", "alpha", None, false, None)
        .unwrap();
    m.copy("/work/a", "/work/b");
    svc.graph_load(&ctx(), "/work/b", Some("beta")).unwrap();
    m.registry.fail_writes(Some("disk full"));
    let failed = svc.graph_primary(&ctx(), "beta").unwrap();
    assert_eq!(codes(&failed.notes), ["registry_update_not_saved"]);
    assert_eq!(unnamed_read(&svc), Ok("beta".into()), "it holds here");
    m.registry.fail_writes(None);
    let again = svc.graph_primary(&ctx(), "beta").unwrap();
    assert!(again.value.unchanged && again.notes.is_empty(), "{again:?}");
    drop(svc);
    assert_eq!(after_restart(&m).0, Ok("beta".into()));
}

/// F98 (security): a copy whose effective policy is not open may be made primary; the reply warns
/// `primary_is_secured` with a placeholder next, and names no other copy anywhere.
#[test]
fn a_secured_primary_is_warned_and_the_reply_names_no_other_copy() {
    let m = Machine::new();
    let mut svc = m.daemon();
    svc.graph_new(&ctx(), "/work/a", "alpha", None, false, None)
        .unwrap();
    m.copy("/work/a", "/work/b");
    svc.graph_load(&ctx(), "/work/b", Some("beta")).unwrap();
    let open = svc.graph_primary(&ctx(), "beta").unwrap();
    assert!(open.notes.is_empty(), "positive control: {open:?}");
    svc.graph_primary(&ctx(), "alpha").unwrap();
    svc.graph_set(&ctx(), "beta", &[("policy".into(), "secured".into())])
        .unwrap();
    let done = svc.graph_primary(&ctx(), "beta").unwrap();
    assert_eq!(codes(&done.notes), ["primary_is_secured"]);
    let shown = format!("{done:?}");
    assert!(!shown.contains("alpha"), "{shown}");
    assert!(
        shown.contains("umx graph primary <another copy>"),
        "{shown}"
    );
}

/// F98 refinement (the project manager, graph-addressing-expert, security): the warning follows effective visibility,
/// the only thing that makes unnamed reads skip a copy (D53). A copy whose embedder is trusted but whose visibility is
/// open is read unnamed, so making it primary warns nothing.
#[test]
fn an_open_visibility_primary_with_a_trusted_embedder_is_not_warned() {
    let m = Machine::new();
    let mut svc = m.daemon();
    svc.graph_new(&ctx(), "/work/a", "alpha", None, false, None)
        .unwrap();
    m.copy("/work/a", "/work/b");
    svc.graph_load(&ctx(), "/work/b", Some("beta")).unwrap();
    svc.graph_set(&ctx(), "beta", &[("embedder".into(), "trusted".into())])
        .unwrap();
    let done = svc.graph_primary(&ctx(), "beta").unwrap();
    assert!(!done.value.unchanged);
    assert!(done.notes.is_empty(), "{done:?}");
    assert_eq!(
        unnamed_read(&svc),
        Ok("beta".into()),
        "read unnamed, as no warning says"
    );
    // The same copy with named visibility is warned (the control).
    svc.graph_primary(&ctx(), "alpha").unwrap();
    svc.graph_set(&ctx(), "beta", &[("visibility".into(), "named".into())])
        .unwrap();
    let named = svc.graph_primary(&ctx(), "beta").unwrap();
    assert_eq!(codes(&named.notes), ["primary_is_secured"]);
}
