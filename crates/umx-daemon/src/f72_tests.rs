//! ws002 F72 end to end through the daemon: graphs the registry remembers come back when a daemon starts, and a
//! named graph that unloaded when idle loads on demand (req-15), once however many requests miss it together.

use super::*;
use std::sync::atomic::AtomicUsize;
use umx_core::ports::{Credential, EmbedError, Embedded, Embedder, Locality, ModelKey, TextKind};
use umx_protocol::Op;

fn build() -> Build {
    Build {
        version: "t".into(),
        commit: "t".into(),
        built: "t".into(),
    }
}

fn op(op: Op, graph: Option<&str>) -> Request {
    Request::op(op, graph.map(str::to_owned), None, build())
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A hosted embedder that records every text it is sent.
#[derive(Default)]
struct Hosted {
    seen: Mutex<Vec<String>>,
    calls: AtomicUsize,
}

impl Hosted {
    fn saw(&self, needle: &str) -> bool {
        self.seen.lock().unwrap().iter().any(|t| t.contains(needle))
    }
}

impl Embedder for Hosted {
    fn embed(&self, texts: &[String], _kind: TextKind) -> Result<Embedded, EmbedError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().extend(texts.iter().cloned());
        Ok(Embedded {
            vectors: texts.iter().map(|_| vec![0.6, 0.8, 0.0, 0.0]).collect(),
            truncated: Vec::new(),
            tokens: None,
        })
    }
    fn key(&self) -> ModelKey {
        ModelKey::parse("spy@4").unwrap()
    }
    fn concurrency_ceiling(&self) -> usize {
        1
    }
    fn max_input_tokens(&self) -> usize {
        512
    }
    fn locality(&self) -> Locality {
        Locality::Hosted
    }
    fn ready(&self) -> Result<(), EmbedError> {
        Ok(())
    }
    fn credential(&self) -> Option<Credential> {
        Some(Credential {
            kind: "fake",
            from: "spy".into(),
        })
    }
    fn adapter_id(&self) -> &'static str {
        "fake.embedder"
    }
}

fn root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("umx-f72-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn daemon(root: &std::path::Path, idle_ms: u64, hosted: &Arc<Hosted>) -> Daemon {
    let config = Config::at(root.join("home"), Some(true))
        .roots([root.to_str().unwrap()])
        .idle_unload_flag(idle_ms);
    let (services, settings) = umx_compose::compose_with_config(umx_compose::Mode::Real, &config);
    let services = services.with_embedder(hosted.clone());
    Daemon::with_services(
        &config,
        build(),
        services,
        settings,
        umx_compose::Mode::Real,
        umx_compose::Embedders::from_instances(Vec::new()),
    )
}

fn ok(env: Envelope) -> Envelope {
    assert!(env.errors.is_empty(), "{}", env.to_json());
    env
}

fn new_graph(d: &Daemon, path: &std::path::Path, alias: &str, secured: bool) {
    ok(d.handle(&op(
        Op::GraphNew {
            path: path.display().to_string(),
            alias: alias.into(),
            title: None,
            empty: false,
            embedding_model: None,
            secured,
            no_graphfile: false,
            graphfile: false,
            no_std: false,
        },
        None,
    )));
}

fn add(d: &Daemon, graph: &str, title: &str) -> Envelope {
    d.handle(&op(
        Op::NodeAdd {
            tags: Default::default(),
            on_key: None,
            related: None,
            raised_in: None,
            kind: "problem".into(),
            title: title.into(),
            status: None,
            note: None,
            sets: Vec::new(),
            parent: None,
            evidence: Vec::new(),
            evidence_kind: None,
            locator: None,
        },
        Some(graph),
    ))
}

fn list(d: &Daemon, graph: &str) -> Envelope {
    d.handle(&op(
        Op::NodeList {
            no_tags: Default::default(),
            tags: Default::default(),
            kind: None,
            status: None,
            wheres: Vec::new(),
            within: None,
            columns: None,
            limit: None,
            offset: None,
            along: None,
        },
        Some(graph),
    ))
}

/// Unloads every loaded graph the way idle unload does (the registry keeps each as loaded, so a named miss loads it on
/// demand), through the services' own idle path, then leaves idle unload off. Tests start their daemon with idle
/// unload off, so no timer can unload a graph between their steps however loaded the machine is (F109): the only
/// idle unloads are the ones a test asks for.
fn idle_out(d: &Daemon) {
    let mut s = d.services.write().unwrap();
    s.set_idle_unload_ms(1);
    // The services' own signal: due once every graph's last use is at least 1 ms old.
    let until = Instant::now() + Duration::from_secs(20);
    while s.idle_wait_ms().is_some_and(|ms| ms > 0) {
        assert!(Instant::now() < until, "idle unload never fell due");
        std::thread::sleep(Duration::from_millis(1));
    }
    let unloaded = s.unload_idle();
    s.set_idle_unload_ms(0);
    assert!(!unloaded.is_empty(), "nothing was loaded to idle out");
    assert!(s.graph_list().is_empty(), "every graph idled out");
}

fn stop(d: Daemon) {
    ok(d.handle(&Request::new("daemon stop", Vec::new(), build())));
}

/// The nodes of `alias` whose vectors are still to be written, held ones aside (the daemon's own record; usize::MAX
/// while it is not loaded), as in s08_tests.
fn pending(d: &Daemon, alias: &str) -> usize {
    d.services
        .read()
        .unwrap()
        .loaded(alias)
        .and_then(|l| l.pending_count())
        .unwrap_or(usize::MAX)
}

/// Security F72 condition 6: a remembered secured graph reloaded at start queues its embeds through the normal
/// worker, which never sends its text to a hosted embedder. An open graph reloaded beside it is embedded by the same
/// spy, so the instrument can see a positive.
#[test]
fn a_reloaded_secured_graph_never_reaches_a_hosted_embedder() {
    let root = root("secured");
    let hosted = Arc::new(Hosted::default());
    let d = daemon(&root, 0, &hosted);
    let (vault, open) = (root.join("vault"), root.join("open"));
    new_graph(&d, &vault, "vault", true);
    new_graph(&d, &open, "open", false);
    ok(add(&d, "vault", "Vault launch codes plan"));
    ok(add(&d, "open", "Open roadmap item"));
    // Written, not just embedded (ws012 s3b, as F112): seen by the embedder is not yet on disk.
    eventually("the open graph's node to be embedded and written", || {
        hosted.saw("Open roadmap item") && pending(&d, "open") == 0
    });
    stop(d);
    // The open graph's vectors go, so the restarted daemon must embed it again.
    fs::remove_dir_all(open.join("embeddings")).unwrap();

    let hosted = Arc::new(Hosted::default());
    let d = daemon(&root, 0, &hosted);
    // The daemon serves while the writer reloads.
    assert!(d.reloading.wait(Duration::from_secs(20)), "the reload ends");
    let status = d.status();
    let reloaded: Vec<&str> = status["registry"]["reloaded"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["alias"].as_str().unwrap())
        .collect();
    assert_eq!(reloaded, ["vault", "open"], "{status}");
    // Open written and the worker settled: every reloaded job was planned and then sent or dropped, so a vault text
    // that went would already be seen. No clock (ws012 s3b). Vault stays pending: no instance may take it.
    eventually("the open graph written and the worker settled", || {
        hosted.saw("Open roadmap item") && pending(&d, "open") == 0 && d.drain.settled()
    });
    assert!(
        !hosted.saw("Vault"),
        "a secured graph's text never reaches a hosted embedder"
    );
    assert!(pending(&d, "vault") > 0, "the vault node is still to embed");
    stop(d);
    let _ = fs::remove_dir_all(&root);
}

/// Architect F72: several requests that miss one idle-unloaded graph at once load it once; each runs. Idle unload
/// stays off after the one idle-out, so a slow thread can never see the graph unload again (F109).
#[test]
fn concurrent_misses_on_an_idle_graph_load_it_once() {
    let root = root("idle");
    let hosted = Arc::new(Hosted::default());
    let d = daemon(&root, 0, &hosted);
    new_graph(&d, &root.join("g"), "demo", false);
    ok(add(&d, "demo", "Approval delays"));
    idle_out(&d);
    let replies: Vec<Envelope> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..4).map(|_| s.spawn(|| list(&d, "demo"))).collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut on_demand = 0;
    for r in &replies {
        let r = ok(r.clone());
        assert_eq!(r.data["total"], 1, "{}", r.to_json());
        on_demand += r
            .hints
            .iter()
            .filter(|w| w.code() == "graph_loaded_on_demand")
            .count();
    }
    assert_eq!(on_demand, 1, "one load, however many missed");
    stop(d);
    let _ = fs::remove_dir_all(&root);
}

/// Only the request's own graph_not_loaded error starts an on-demand load; the same code as a warning never does.
#[test]
fn only_a_graph_not_loaded_error_names_a_graph_to_load() {
    let missed = umx_services::problems::graph_not_loaded("demo");
    let issue = verbs::issue(&missed, "node list");
    let failed = Envelope::fail("node list", issue.clone());
    assert_eq!(missed_alias(&failed).as_deref(), Some("demo"));
    let warned = Envelope::ok("node add", json!({})).warn(issue);
    assert_eq!(missed_alias(&warned), None);
    let other = Envelope::fail("node list", internal("writer_stopped"));
    assert_eq!(missed_alias(&other), None);
}

/// Review R156-B1: request A misses an idle graph, request B loads it, then A reaches its on-demand step. A runs
/// again and succeeds without a second load (no graph_loaded_on_demand of its own); B's reply carried the one load.
/// No idle timer runs between B's load and A's rerun (F109), so the only load A could see is a second one.
#[test]
fn a_miss_whose_graph_another_request_loaded_runs_again() {
    let root = root("raced");
    let hosted = Arc::new(Hosted::default());
    let d = daemon(&root, 0, &hosted);
    new_graph(&d, &root.join("g"), "demo", false);
    ok(add(&d, "demo", "Approval delays"));
    idle_out(&d);
    let request = op(
        Op::NodeList {
            no_tags: Default::default(),
            tags: Default::default(),
            kind: None,
            status: None,
            wheres: Vec::new(),
            within: None,
            columns: None,
            limit: None,
            offset: None,
            along: None,
        },
        Some("demo"),
    );
    let missed = d.dispatch(&request);
    assert_eq!(missed_alias(&missed).as_deref(), Some("demo"));
    let b = ok(list(&d, "demo"));
    assert!(b.hints.iter().any(|w| w.code() == "graph_loaded_on_demand"));
    let a = ok(d.on_demand(&request, missed));
    assert_eq!(a.data["total"], 1, "{}", a.to_json());
    assert!(!a.hints.iter().any(|w| w.code() == "graph_loaded_on_demand"));
    stop(d);
    let _ = fs::remove_dir_all(&root);
}

/// Review R257-B1: a `--graph alpha,beta` read misses alpha (idled out, remembered) while beta is loaded. The daemon's
/// own miss selection chooses what to load; then beta is unloaded on purpose before the writer runs that load. The
/// writer must refuse the whole list before loading alpha (R213-B1), and the registry may change only by beta's
/// unload. Each step runs in order on this thread, so the interleaving is the same on every run.
#[test]
fn a_list_member_unloaded_on_purpose_after_the_miss_refuses_and_loads_nothing() {
    let root = root("list-race");
    let hosted = Arc::new(Hosted::default());
    let d = daemon(&root, 0, &hosted);
    new_graph(&d, &root.join("a"), "alpha", false);
    new_graph(&d, &root.join("b"), "beta", false);
    ok(add(&d, "alpha", "Approval delays"));
    ok(add(&d, "beta", "Approval delays"));
    idle_out(&d);
    // beta loads on demand; alpha stays idled out.
    ok(list(&d, "beta"));
    let request = op(
        Op::NodeList {
            no_tags: Default::default(),
            tags: Default::default(),
            kind: None,
            status: None,
            wheres: Vec::new(),
            within: None,
            columns: None,
            limit: None,
            offset: None,
            along: None,
        },
        Some("alpha,beta"),
    );
    let missed = d.dispatch(&request);
    assert_eq!(missed_alias(&missed).as_deref(), Some("alpha"));
    let miss = match d.miss_for(&request, missed) {
        ControlFlow::Continue(miss) => miss,
        ControlFlow::Break(env) => panic!("a list miss to load: {}", env.to_json()),
    };
    assert!(matches!(miss, Miss::Aliases(_)));

    let registry = root
        .join("home")
        .join(umx_core::registry::REGISTRY_DIR)
        .join(umx_core::registry::REGISTRY_FILE);
    let at_miss = fs::read(&registry).unwrap();
    ok(d.handle(&op(
        Op::GraphUnload {
            alias: "beta".into(),
        },
        None,
    )));
    let unloaded = fs::read(&registry).unwrap();
    assert_ne!(at_miss, unloaded, "the unload is recorded in the registry");

    let env = d.load_then_rerun(&request, miss);
    assert_eq!(
        env.errors.first().map(|e| e.code()),
        Some("graph_not_loaded"),
        "{}",
        env.to_json()
    );
    assert!(
        d.services.read().unwrap().graph_list().is_empty(),
        "alpha must not load for a refused list read"
    );
    assert_eq!(
        String::from_utf8_lossy(&fs::read(&registry).unwrap()),
        String::from_utf8_lossy(&unloaded),
        "the registry changed only by beta's unload"
    );
    stop(d);
    let _ = fs::remove_dir_all(&root);
}

/// A daemon serves before the remembered graphs are back. While the reload runs (held here at gates, as a
/// slow disk holds it), `daemon status` answers at once and says how many still wait; a request naming a graph still
/// waiting gets that graph loaded ahead of the others; one naming none waits for the reload; then all are back.
#[test]
fn a_start_serves_while_the_remembered_graphs_reload() {
    let root = root("boot");
    let hosted = Arc::new(Hosted::default());
    let d = daemon(&root, 0, &hosted);
    for alias in ["a", "b", "c"] {
        new_graph(&d, &root.join(alias), alias, false);
        ok(add(&d, alias, &format!("node in {alias}")));
    }
    stop(d);
    let (gate_a, gate_b) = (gate(&root, "a"), gate(&root, "b"));

    // Before the daemon served first, it reloaded every graph before it was built, so it never got past gate a.
    let (tx, rx) = channel();
    let (r, h) = (root.clone(), Arc::clone(&hosted));
    std::thread::spawn(move || tx.send(daemon(&r, 0, &h)).unwrap());
    let d = Arc::new(
        rx.recv_timeout(Duration::from_secs(5))
            .expect("the daemon is built while its graphs still wait"),
    );
    let asked = Instant::now();
    let status = ok(d.handle(&Request::new("daemon status", Vec::new(), build())));
    assert!(
        asked.elapsed() < Duration::from_secs(1),
        "status answered at once"
    );
    assert!(status.data["registry"]["reloading"].as_u64().unwrap() > 0);

    // A read naming no graph waits for the reload; one naming c (third in load order) does not.
    let unnamed = {
        let d = Arc::clone(&d);
        std::thread::spawn(move || {
            d.handle(&op(
                Op::NodeFind {
                    text: "node".into(),
                    kind: None,
                    limit: None,
                    within: None,
                    along: None,
                    tags: Vec::new(),
                    no_tags: Vec::new(),
                },
                None,
            ))
        })
    };
    let (d_c, sent) = (Arc::clone(&d), d.writer_jobs.load(Ordering::SeqCst));
    let named = std::thread::spawn(move || list(&d_c, "c"));
    eventually("the miss on c to reach the writer", || {
        d.writer_jobs.load(Ordering::SeqCst) > sent
    });
    open_gate(&gate_a);
    let named = named.join().unwrap();
    assert!(named.errors.is_empty(), "{}", named.to_json());
    assert!(named.to_json().contains("node in c"), "{}", named.to_json());
    let loaded = |alias: &str| d.services.read().unwrap().loaded(alias).is_some();
    assert!(
        loaded("a") && loaded("c") && !loaded("b"),
        "c loaded ahead of b"
    );
    assert!(
        !unnamed.is_finished(),
        "an unnamed request waits for the reload"
    );

    open_gate(&gate_b);
    let unnamed = unnamed.join().unwrap();
    assert!(
        !unnamed.to_json().contains("daemon_busy"),
        "{}",
        unnamed.to_json()
    );
    assert!(loaded("b"));
    let status = ok(d.handle(&Request::new("daemon status", Vec::new(), build())));
    let registry = &status.data["registry"];
    assert_eq!(registry["reloading"], 0);
    let mut back: Vec<&str> = registry["reloaded"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["alias"].as_str().unwrap())
        .collect();
    back.sort_unstable();
    assert_eq!(back, ["a", "b", "c"]);
    assert!(registry["not_reloaded"].as_array().unwrap().is_empty());
    ungate(&root);
    let _ = fs::remove_dir_all(&root);
}

/// Holds the start-up reload before it reloads `alias`'s folder under `root`, until [`open_gate`].
fn gate(root: &std::path::Path, alias: &str) -> ReloadGate {
    let gate: ReloadGate = Arc::new((Mutex::new(false), Condvar::new()));
    let folder = root.join(alias).display().to_string();
    RELOAD_GATES
        .lock()
        .unwrap()
        .push((folder, Arc::clone(&gate)));
    gate
}

fn open_gate(gate: &ReloadGate) {
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
}

fn ungate(root: &std::path::Path) {
    RELOAD_GATES
        .lock()
        .unwrap()
        .retain(|(f, _)| !f.starts_with(root.to_str().unwrap()));
}

/// Graphs `aliases` (open, one node each) remembered under `root`, the daemon that made them stopped.
fn remembered(root: &std::path::Path, hosted: &Arc<Hosted>, aliases: &[&str]) {
    let d = daemon(root, 0, hosted);
    for alias in aliases {
        new_graph(&d, &root.join(alias), alias, false);
        ok(add(&d, alias, &format!("node in {alias}")));
    }
    stop(d);
}

/// `request` sent on its own thread, which must still be waiting a moment later: it waits for the reload.
fn waits(d: &Arc<Daemon>, request: Request) -> std::thread::JoinHandle<Envelope> {
    let d = Arc::clone(d);
    let waiting = std::thread::spawn(move || d.handle(&request));
    std::thread::sleep(Duration::from_millis(300));
    assert!(!waiting.is_finished(), "the request waits for the reload");
    waiting
}

/// Review of #72 (1): `graph load --as a` while remembered `a` still waits must not take its alias (the registry
/// would then drop `a` for good). It waits for the reload, then is refused, and `a` stays remembered.
#[test]
fn a_load_during_the_reload_never_takes_a_waiting_graphs_alias() {
    let root = root("boot-alias");
    let hosted = Arc::new(Hosted::default());
    remembered(&root, &hosted, &["first", "a"]);
    {
        let d = daemon(&root, 0, &hosted);
        new_graph(&d, &root.join("other"), "other", false);
        ok(d.handle(&op(
            Op::GraphUnload {
                alias: "other".into(),
            },
            None,
        )));
        stop(d);
    }
    let held = gate(&root, "first");
    let d = Arc::new(daemon(&root, 0, &hosted));
    let load = waits(
        &d,
        op(
            Op::GraphLoad {
                path: root.join("other").display().to_string(),
                as_alias: Some("a".into()),
                by_alias: None,
                graphfile: false,
                set: None,
            },
            None,
        ),
    );
    open_gate(&held);
    let load = load.join().unwrap();
    assert!(!load.errors.is_empty(), "{}", load.to_json());
    let s = d.services.read().unwrap();
    let a = s.loaded("a").expect("a came back");
    assert_eq!(a.dir.as_str(), root.join("a").display().to_string());
    assert!(s.graph().remembered_loaded("a").is_none(), "a is in memory");
    drop(s);
    let status = ok(d.handle(&Request::new("daemon status", Vec::new(), build())));
    let back = status.data["registry"]["reloaded"].to_string();
    assert!(back.contains("\"a\""), "{back}");
    ungate(&root);
    let _ = fs::remove_dir_all(&root);
}

/// Review of #72 (2) and (3): `UMX_GRAPH` with `--all`, and an entity read naming one graph, cover every loaded
/// graph, so they wait for the reload rather than answer from the graphs back so far.
#[test]
fn reads_that_cover_every_graph_wait_for_the_reload_even_when_a_graph_is_named() {
    let root = root("boot-cover");
    let hosted = Arc::new(Hosted::default());
    remembered(&root, &hosted, &["a", "b"]);
    let held = gate(&root, "b");
    let d = Arc::new(daemon(&root, 0, &hosted));
    eventually("a to come back", || {
        d.services.read().unwrap().loaded("a").is_some()
    });
    let mut all = op(
        Op::Query {
            text: "MATCH (n) RETURN n.title".into(),
            limit: None,
            offset: None,
            params: Vec::new(),
            along: None,
        },
        None,
    );
    all.all = true;
    all.default_graph = Some("a".into());
    let all = waits(&d, all);
    let id = list(&d, "a").data["rows"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let entity = waits(&d, op(Op::EntityShow { node: id }, Some("a")));
    open_gate(&held);
    let all = all.join().unwrap();
    assert!(all.to_json().contains("node in b"), "{}", all.to_json());
    let entity = entity.join().unwrap();
    assert!(
        !entity.to_json().contains("daemon_busy"),
        "{}",
        entity.to_json()
    );
    ungate(&root);
    let _ = fs::remove_dir_all(&root);
}

/// Review of #72 (4): a miss on a later copy of a graph still waiting loads the earlier waiting copy first, in
/// queue order, so the earliest copy stays primary (d-21).
#[test]
fn a_miss_on_a_waiting_copy_keeps_the_earliest_copy_primary() {
    let root = root("boot-copies");
    let hosted = Arc::new(Hosted::default());
    let d = daemon(&root, 0, &hosted);
    new_graph(&d, &root.join("first"), "first", false);
    new_graph(&d, &root.join("x1"), "x1", false);
    ok(add(&d, "x1", "node in x"));
    let copied = std::process::Command::new("cp")
        .arg("-R")
        .arg(root.join("x1"))
        .arg(root.join("x2"))
        .status()
        .unwrap();
    assert!(copied.success());
    ok(d.handle(&op(
        Op::GraphLoad {
            path: root.join("x2").display().to_string(),
            as_alias: Some("x2".into()),
            by_alias: None,
            graphfile: false,
            set: None,
        },
        None,
    )));
    stop(d);
    let held = gate(&root, "first");
    let d = Arc::new(daemon(&root, 0, &hosted));
    let sent = d.writer_jobs.load(Ordering::SeqCst);
    let named = {
        let d = Arc::clone(&d);
        std::thread::spawn(move || list(&d, "x2"))
    };
    eventually("the miss on x2 to reach the writer", || {
        d.writer_jobs.load(Ordering::SeqCst) > sent
    });
    open_gate(&held);
    let named = named.join().unwrap();
    assert!(named.errors.is_empty(), "{}", named.to_json());
    let s = d.services.read().unwrap();
    let order = |alias: &str| s.loaded(alias).expect("loaded").order;
    assert!(order("x1") < order("x2"), "the earlier copy loaded first");
    drop(s);
    ungate(&root);
    let _ = fs::remove_dir_all(&root);
}
