//! ws004 s08 end to end through the daemon: the secret hold on the embed worker's real path (req-49), with allows
//! on disk across a restart, and the 5 s query-embed bound on `umx search` (req-53).

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

fn op(op: Op) -> Request {
    Request::op(op, None, None, build())
}

/// How long a test waits for the daemon to get somewhere: a deadline, not a wait (a passing test returns as soon as it
/// gets there). A gate runs these tests niced (cargo-slot) on a loaded machine, where 20 s once was not enough for a
/// search to reach its instance (load about 36, the gate, F112). No wait here proves anything by how long it
/// took; the s17 tests in lib.rs share it.
pub(crate) const SETTLE: Duration = Duration::from_secs(120);

fn eventually(what: &str, check: impl Fn() -> bool) {
    let until = Instant::now() + SETTLE;
    while !check() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A fake API key, built at run time so no key-shaped literal sits in the source.
fn fake_key(seed: &str) -> String {
    let mut k = String::from("AKIA");
    while k.len() < 20 {
        k.push_str(seed);
    }
    k.truncate(20);
    k
}

/// An embedder that records every text, with one session (a lock held while it works, as llama.cpp's is) and
/// optional delays: `per_text` for each document text, `query` for each query call. A query call can also be held
/// until the test releases it ([`Spy::hold_queries`]), so a query is in flight for as long as a test needs.
struct Spy {
    key: &'static str,
    locality: Locality,
    seen: Mutex<Vec<String>>,
    session: Mutex<()>,
    per_text: Duration,
    query: Duration,
    queries: AtomicUsize,
    holding: Mutex<bool>,
    released: Condvar,
}

impl Spy {
    fn new(locality: Locality, per_text: Duration, query: Duration) -> Arc<Spy> {
        Spy::keyed("spy@4", locality, per_text, query)
    }

    fn keyed(
        key: &'static str,
        locality: Locality,
        per_text: Duration,
        query: Duration,
    ) -> Arc<Spy> {
        Arc::new(Spy {
            key,
            locality,
            seen: Mutex::new(Vec::new()),
            session: Mutex::new(()),
            per_text,
            query,
            queries: AtomicUsize::new(0),
            holding: Mutex::new(false),
            released: Condvar::new(),
        })
    }

    /// From now on a query call waits, its session held, until [`Spy::release_queries`].
    fn hold_queries(&self) {
        *self.holding.lock().unwrap() = true;
    }

    fn release_queries(&self) {
        *self.holding.lock().unwrap() = false;
        self.released.notify_all();
    }

    fn saw(&self, needle: &str) -> bool {
        self.seen.lock().unwrap().iter().any(|t| t.contains(needle))
    }
}

impl Embedder for Spy {
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
        let _session = self.session.lock().unwrap();
        self.seen.lock().unwrap().extend(texts.iter().cloned());
        match kind {
            TextKind::Query => {
                self.queries.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(self.query);
                // A backstop, so a test that fails while holding never strands the call. Longer than any wait: one
                // that ended with SETTLE would free the query just as a broken guard's wait timed out, and the test
                // would fail somewhere else. So a run that fails while it holds a query can take up to 2 x SETTLE
                // to free it (the project manager).
                let holding = self.holding.lock().unwrap();
                let _ = self
                    .released
                    .wait_timeout_while(holding, SETTLE * 2, |held| *held)
                    .unwrap();
            }
            TextKind::Document => std::thread::sleep(self.per_text * texts.len() as u32),
        }
        Ok(Embedded {
            vectors: texts.iter().map(|_| vec![0.6, 0.8, 0.0, 0.0]).collect(),
            truncated: Vec::new(),
            tokens: None,
        })
    }
    fn key(&self) -> ModelKey {
        ModelKey::parse(self.key).unwrap()
    }
    fn concurrency_ceiling(&self) -> usize {
        1
    }
    fn max_input_tokens(&self) -> usize {
        512
    }
    fn locality(&self) -> Locality {
        self.locality
    }
    fn ready(&self) -> Result<(), EmbedError> {
        Ok(())
    }
    fn credential(&self) -> Option<Credential> {
        (self.locality == Locality::Hosted).then(|| Credential {
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
        .join(format!("umx-s08-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn daemon(root: &std::path::Path, spy: &Arc<Spy>) -> Daemon {
    let config = Config::at(root.join("home"), Some(true)).roots([root.to_str().unwrap()]);
    let (services, settings) = umx_compose::compose_with_config(umx_compose::Mode::Real, &config);
    Daemon::with_services(
        &config,
        build(),
        services.with_embedder(spy.clone()),
        settings,
        umx_compose::Mode::Real,
        umx_compose::Embedders::from_instances(Vec::new()),
    )
}

fn ok(env: Envelope) -> Envelope {
    assert!(env.errors.is_empty(), "{}", env.to_json());
    env
}

fn add(d: &Daemon, title: &str, note: &str) -> Envelope {
    ok(d.handle(&op(Op::NodeAdd {
        raised_in: None,
        kind: "problem".into(),
        title: title.into(),
        status: None,
        note: Some(note.into()),
        sets: Vec::new(),
        parent: None,
        evidence: Vec::new(),
        evidence_kind: None,
        locator: None,
    })))
}

/// The nodes of `alias` whose vectors are still to be written, held ones aside: the daemon's own record of the work
/// left to send (usize::MAX while it is not loaded).
fn pending(d: &Daemon, alias: &str) -> usize {
    d.services
        .read()
        .unwrap()
        .loaded(alias)
        .and_then(|l| l.pending_count())
        .unwrap_or(usize::MAX)
}

fn shards(dir: &std::path::Path) -> usize {
    fs::read_dir(dir.join("embeddings/spy@4"))
        .map(|d| {
            d.flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n.ends_with(".tsv") && !n.starts_with(".umx-tmp-")
                })
                .count()
        })
        .unwrap_or(0)
}

/// req-49 done_when: ingest a field holding a fake API key; the command warns and the hosted embedder never sees
/// it. Then the allow sends it (the instrument can see a positive), and after a restart the allow (on disk) still
/// holds while the other key is still held.
#[test]
fn a_fake_api_key_warns_and_never_reaches_the_hosted_embedder_across_a_restart() {
    let root = root("hold");
    let (k1, k2) = (fake_key("Q7Z3X9K2"), fake_key("M4N8P1R6"));
    let spy = Spy::new(Locality::Hosted, Duration::ZERO, Duration::ZERO);
    let d = daemon(&root, &spy);
    let g = root.join("g");
    ok(d.handle(&op(Op::GraphNew {
        path: g.display().to_string(),
        alias: "demo".into(),
        title: None,
        empty: false,
        embedding_model: None,
        secured: false,
        no_graphfile: false,
        graphfile: false,
    })));
    let one = add(
        &d,
        "Export one fails",
        &format!("it uses {k1} for the bucket"),
    );
    let warned: Vec<&str> = one.warnings.iter().map(|w| w.code()).collect();
    assert_eq!(warned, ["secret_suspected"], "{}", one.to_json());
    let json = one.to_json();
    assert!(
        json.contains("note") && json.contains("aws_access_key"),
        "{json}"
    );
    assert!(
        !serde_json::to_string(&one.warnings)
            .unwrap()
            .contains(&k1[..8]),
        "never in a warning"
    );
    let id = one.data["node"]["id"].as_str().unwrap().to_string();
    add(
        &d,
        "Export two fails",
        &format!("it uses {k2} for the bucket"),
    );
    // Nothing left to send, not a sleep: a node the hold missed stays pending until its key's vector is written, so
    // a broken hold is seen sending the key before this wait ends.
    eventually("both nodes' clean texts and nothing left to send", || {
        spy.saw("Export one fails") && spy.saw("Export two fails") && pending(&d, "demo") == 0
    });
    assert!(
        !spy.saw(&k1) && !spy.saw(&k2),
        "no key reached the hosted embedder"
    );

    let allowed = ok(d.handle(&op(Op::EmbeddingsAllow {
        node: id.clone(),
        field: "note".into(),
    })));
    assert_eq!(allowed.data["allowed"], true, "{}", allowed.to_json());
    eventually("the allowed note to be sent", || spy.saw(&k1));
    let file = root.join("home/allows/secret-allows.json");
    let text = fs::read_to_string(&file).unwrap();
    assert!(!text.contains(&k1) && !text.contains(&k1[4..12]), "{text}");
    assert_eq!(fs::metadata(&file).unwrap().mode() & 0o777, 0o600);

    // Restart: a new daemon over the same home; node one's vectors are gone so it is embedded again.
    // Stop the old daemon for real first (R120-4): its writer applies everything queued before the stop and then
    // exits, so no later vector write can land in the folder being removed.
    let stopped = ok(d.handle(&Request::new("daemon stop", Vec::new(), build())));
    assert_eq!(stopped.data["stopped"], true);
    drop(d);
    fs::remove_dir_all(g.join("embeddings")).unwrap();
    let spy = Spy::new(Locality::Hosted, Duration::ZERO, Duration::ZERO);
    let d = daemon(&root, &spy);
    ok(d.handle(&op(Op::GraphLoad {
        path: g.display().to_string(),
        as_alias: None,
        by_alias: None,
        graphfile: false,
        set: None,
    })));
    eventually(
        "the allow, node two's clean texts and nothing left to send",
        || spy.saw(&k1) && spy.saw("Export two fails") && pending(&d, "demo") == 0,
    );
    assert!(!spy.saw(&k2), "the hold held across the restart");
    let _ = fs::remove_dir_all(&root);
}

/// req-53: a query embed that takes longer than 5 s is given up at the bound; the search answers with word matches
/// and `similarity_unavailable`. Without the bound it would wait the embedder's 8 s.
#[test]
fn a_query_embed_past_five_seconds_falls_back_to_words_at_the_bound() {
    let root = root("bound");
    let spy = Spy::new(Locality::Hosted, Duration::ZERO, Duration::from_secs(8));
    let d = daemon(&root, &spy);
    let g = root.join("g");
    ok(d.handle(&op(Op::GraphNew {
        path: g.display().to_string(),
        alias: "demo".into(),
        title: None,
        empty: false,
        embedding_model: None,
        secured: false,
        no_graphfile: false,
        graphfile: false,
    })));
    add(&d, "Approval delays", "invoices wait for sign off");
    eventually("the node's vectors", || shards(&g) > 0);
    let started = Instant::now();
    let env = d.handle(&op(Op::Search {
        text: "approval delays".into(),
        within: None,
        limit: None,
        along: None,
    }));
    let took = started.elapsed();
    assert!(env.errors.is_empty(), "{}", env.to_json());
    assert_eq!(spy.queries.load(Ordering::SeqCst), 1, "the query was asked");
    assert!(
        took >= Duration::from_secs(5) && took < Duration::from_millis(6500),
        "bounded at 5 s: {took:?}"
    );
    let warning = env
        .warnings
        .iter()
        .find(|w| w.code() == "similarity_unavailable")
        .unwrap_or_else(|| panic!("{}", env.to_json()));
    assert!(
        warning.message().contains("took over 5 s"),
        "{}",
        warning.message()
    );
    assert!(
        !warning.message().contains("loading"),
        "a hosted instance has no model to load"
    );
    assert_eq!(env.data["rows"][0]["legs"], "word", "{}", env.to_json());
    let _ = fs::remove_dir_all(&root);
}

/// embeddings-expert, s08: a query that arrives while the worker is ingesting does not wait behind the backlog. The
/// worker sends no new batch to the instance while the query waits, so it waits behind one batch at most.
#[test]
fn a_query_during_ingest_finishes_inside_the_bound() {
    let root = root("ingest");
    // A batch of 16 texts takes 0.8 s; the whole ingest about 15 s.
    let spy = Spy::new(Locality::Local, Duration::from_millis(50), Duration::ZERO);
    let d = daemon(&root, &spy);
    let g = root.join("g");
    ok(d.handle(&op(Op::GraphNew {
        path: g.display().to_string(),
        alias: "demo".into(),
        title: None,
        empty: false,
        embedding_model: None,
        secured: false,
        no_graphfile: false,
        graphfile: false,
    })));
    for i in 0..100 {
        add(
            &d,
            &format!("Approval delay {i}"),
            "invoices wait for sign off",
        );
    }
    eventually("the first vectors", || shards(&g) > 0);
    let started = Instant::now();
    let env = ok(d.handle(&op(Op::Search {
        text: "approval delay".into(),
        within: None,
        limit: None,
        along: None,
    })));
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(3),
        "waited behind one batch at most: {took:?}"
    );
    assert!(
        !env.warnings
            .iter()
            .any(|w| w.code() == "similarity_unavailable"),
        "{}",
        env.to_json()
    );
    assert!(
        spy.seen.lock().unwrap().len() < 280,
        "the ingest was still running when the query went"
    );
    let _ = fs::remove_dir_all(&root);
}

/// R120-3: a query stuck on instance a holds only a's background batches; instance b's go on meanwhile. a's query is
/// held until the end, so b embedding at all proves it was not held behind a, with no clock (F112).
#[test]
fn a_query_in_flight_on_one_instance_never_holds_another_instances_batches() {
    let root = root("two");
    let a = Spy::keyed("aaa@4", Locality::Local, Duration::ZERO, Duration::ZERO);
    let b = Spy::keyed("bbb@4", Locality::Local, Duration::ZERO, Duration::ZERO);
    let config = Config::at(root.join("home"), Some(true)).roots([root.to_str().unwrap()]);
    let (mut services, settings) =
        umx_compose::compose_with_config(umx_compose::Mode::Real, &config);
    services.set_instances(
        vec![
            umx_services::embedding::Instance {
                name: "a".into(),
                embedder: a.clone(),
            },
            umx_services::embedding::Instance {
                name: "b".into(),
                embedder: b.clone(),
            },
        ],
        Some("a"),
    );
    let d = Daemon::with_services(
        &config,
        build(),
        services,
        settings,
        umx_compose::Mode::Real,
        umx_compose::Embedders::from_instances(Vec::new()),
    );
    for (alias, key) in [("ga", "aaa@4"), ("gb", "bbb@4")] {
        ok(d.handle(&op(Op::GraphNew {
            path: root.join(alias).display().to_string(),
            alias: alias.into(),
            title: None,
            empty: false,
            embedding_model: Some(key.into()),
            secured: false,
            no_graphfile: false,
            graphfile: false,
        })));
    }
    let add_to = |alias: &str, title: &str| {
        let mut req = op(Op::NodeAdd {
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
        });
        req.graph = Some(alias.into());
        ok(d.handle(&req));
    };
    add_to("ga", "Approval delays");
    // Written, not just embedded (F112): a search of a graph with no vectors yet embeds no query at all.
    eventually("ga's first vectors", || pending(&d, "ga") == 0);
    a.hold_queries();
    std::thread::scope(|s| {
        let mut search = op(Op::Search {
            text: "approval".into(),
            within: None,
            limit: None,
            along: None,
        });
        search.graph = Some("ga".into());
        let d = &d;
        let stuck = s.spawn(move || d.handle(&search));
        eventually("the query to reach a", || {
            a.queries.load(Ordering::SeqCst) == 1
        });
        // A batch for a now waits behind the query; one for b must not.
        add_to("ga", "Second for a");
        add_to("gb", "First for b");
        eventually("b to embed while a's query is in flight", || {
            b.saw("First for b")
        });
        assert_eq!(a.queries.load(Ordering::SeqCst), 1, "a's query still held");
        assert!(
            !a.saw("Second for a"),
            "a got no new batch while its query was in flight"
        );
        a.release_queries();
        let _ = stuck.join();
    });
    let _ = fs::remove_dir_all(&root);
}
