//! ws004 s25: background embedding runs up to each instance's concurrency ceiling at once, less one slot kept for
//! its queries; a 429 pauses only its instance; a policy change mid-drain stops further batches; the writer merges
//! waiting units for one shard file.

use super::*;
use std::sync::atomic::AtomicUsize;
use umx_core::ports::{EmbedError, Embedded, Embedder, Locality, ModelKey, TextKind};
use umx_protocol::{BatchLine, BatchOp, SettingArg};

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

fn ok(env: Envelope) -> Envelope {
    assert!(env.errors.is_empty(), "{}", env.to_json());
    env
}

/// How long a test waits for the daemon to get somewhere: a deadline, not a wait (a passing test returns as soon as it
/// gets there). A gate runs these tests niced (cargo-slot) on a loaded machine, where 10 s once was not enough for b's
/// 40 nodes (load average 52, the gate). No test here proves anything by how long it waited.
const SETTLE: Duration = Duration::from_secs(120);

fn eventually(what: &str, within: Duration, check: impl Fn() -> bool) {
    let until = Instant::now() + within;
    while !check() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("umx-s25-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

/// A provider that takes `ceiling` calls at once (more wait, as a deployment's own limit would make them), counts
/// how many it has in flight, and can answer the first `limited` background calls with a 429.
struct Provider {
    key: &'static str,
    ceiling: usize,
    locality: Locality,
    /// Each background call's time, in ms (changeable mid-test).
    delay_ms: std::sync::atomic::AtomicU64,
    query_delay: Duration,
    queries: AtomicUsize,
    t0: Instant,
    taking: Mutex<usize>,
    freed: Condvar,
    in_flight: AtomicUsize,
    most: AtomicUsize,
    calls: AtomicUsize,
    limited: AtomicUsize,
    retry_after_ms: u64,
    /// (ms since t0 when the call started, refused with a 429).
    log: Mutex<Vec<(u64, bool)>>,
}

impl Provider {
    fn new(key: &'static str, ceiling: usize, delay: Duration) -> Provider {
        Provider {
            key,
            ceiling,
            locality: Locality::Local,
            delay_ms: std::sync::atomic::AtomicU64::new(delay.as_millis() as u64),
            query_delay: Duration::from_millis(5),
            queries: AtomicUsize::new(0),
            t0: Instant::now(),
            taking: Mutex::new(0),
            freed: Condvar::new(),
            in_flight: AtomicUsize::new(0),
            most: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            limited: AtomicUsize::new(0),
            retry_after_ms: 0,
            log: Mutex::new(Vec::new()),
        }
    }
}

impl Embedder for Provider {
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
        let mut taking = self.taking.lock().unwrap();
        while *taking >= self.ceiling {
            taking = self.freed.wait(taking).unwrap();
        }
        *taking += 1;
        drop(taking);
        let at = self.t0.elapsed().as_millis() as u64;
        let background = kind == TextKind::Document;
        if background {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(now, Ordering::SeqCst);
        }
        let refused = background
            && self
                .limited
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok();
        if background {
            self.log.lock().unwrap().push((at, refused));
        }
        if !background {
            self.queries.fetch_add(1, Ordering::SeqCst);
        }
        if !refused {
            std::thread::sleep(if background {
                Duration::from_millis(self.delay_ms.load(Ordering::SeqCst))
            } else {
                self.query_delay
            });
        }
        if background {
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
        }
        *self.taking.lock().unwrap() -= 1;
        self.freed.notify_all();
        if refused {
            return Err(EmbedError::RateLimited {
                retry_after_ms: Some(self.retry_after_ms),
            });
        }
        let dims = ModelKey::parse(self.key).unwrap().dims();
        Ok(Embedded {
            vectors: texts
                .iter()
                .map(|t| {
                    let mut v = vec![0.0; dims];
                    v[t.len() % dims] = 1.0;
                    v
                })
                .collect(),
            truncated: Vec::new(),
            tokens: None,
        })
    }
    fn key(&self) -> ModelKey {
        ModelKey::parse(self.key).unwrap()
    }
    fn concurrency_ceiling(&self) -> usize {
        self.ceiling
    }
    fn max_input_tokens(&self) -> usize {
        8191
    }
    fn locality(&self) -> Locality {
        self.locality
    }
    fn ready(&self) -> Result<(), EmbedError> {
        Ok(())
    }
    fn credential(&self) -> Option<umx_core::ports::Credential> {
        None
    }
    fn adapter_id(&self) -> &'static str {
        "fake.embedder"
    }
}

/// A daemon over `instances` (the first is the default), with one graph per instance keyed to its model.
fn daemon(root: &std::path::Path, instances: &[(&str, &Arc<Provider>)]) -> Daemon {
    daemon_paced(root, instances, std::collections::BTreeMap::new())
}

/// [`daemon`], with instances paced in tokens a minute.
fn daemon_paced(
    root: &std::path::Path,
    instances: &[(&str, &Arc<Provider>)],
    pace: std::collections::BTreeMap<String, u64>,
) -> Daemon {
    let config = Config::at(root.join("home"), Some(true)).roots([root.to_str().unwrap()]);
    let (mut services, settings) =
        umx_compose::compose_with_config(umx_compose::Mode::Real, &config);
    services.set_instances(
        instances
            .iter()
            .map(|(name, p)| umx_services::embedding::Instance {
                name: (*name).into(),
                embedder: Arc::clone(*p) as Arc<dyn Embedder>,
            })
            .collect(),
        Some(instances[0].0),
    );
    services.set_pace(pace);
    let d = Daemon::with_services(
        &config,
        build(),
        services,
        settings,
        umx_compose::Mode::Real,
        umx_compose::Embedders::from_instances(Vec::new()),
    );
    for (name, p) in instances {
        ok(d.handle(&op(Op::GraphNew {
            path: root.join(name).display().to_string(),
            alias: format!("g{name}"),
            title: None,
            empty: false,
            embedding_model: Some(p.key.into()),
            secured: false,
            no_graphfile: false,
            graphfile: false,
        })));
    }
    d
}

fn add(d: &Daemon, alias: &str, from: usize, n: usize) {
    let mut req = op(Op::Batch {
        raised_in: None,
        echo_sentences: false,
        lines: (from..from + n)
            .map(|i| BatchLine {
                line: (i - from) as u32 + 1,
                op: BatchOp::NodeAdd {
                    raised_in: None,
                    id: None,
                    created: None,
                    ref_name: None,
                    kind: "problem".into(),
                    title: format!("concept {i} of {alias}"),
                    status: None,
                    note: Some(format!("note {i} with a few words")),
                    sets: Vec::new(),
                    parent: None,
                    parent_ref: None,
                    evidence: Vec::new(),
                    evidence_kind: None,
                },
            })
            .collect(),
        dry_run: false,
        refs: Default::default(),
        refs_graph: None,
        reason: None,
    });
    req.graph = Some(alias.into());
    ok(d.handle(&req));
}

fn pending(d: &Daemon, alias: &str) -> usize {
    d.services
        .read()
        .unwrap()
        .loaded(alias)
        .and_then(|l| l.pending_count())
        .unwrap_or(usize::MAX)
}

/// The gain, bounded: an instance runs up to its slots (ceiling less one) of background calls at once, never more.
#[test]
fn background_calls_fill_the_instances_slots_and_never_more() {
    let root = root("slots");
    let p = Arc::new(Provider::new("aaa@4", 4, Duration::from_millis(40)));
    let d = daemon(&root, &[("a", &p)]);
    add(&d, "ga", 0, 160);
    eventually("every vector", SETTLE, || pending(&d, "ga") == 0);
    let most = p.most.load(Ordering::SeqCst);
    // A literal, not slots(): the guard must not read the rule it checks.
    assert_eq!(
        most, 3,
        "{most} background calls at once; slots are the ceiling (4) less one for queries"
    );
    let _ = fs::remove_dir_all(&root);
}

/// R106-2 under concurrency: a 429 pauses only its instance for its Retry-After (no new call starts on it, while
/// the other instance's calls go on), and its refused jobs come back and are embedded. The pause is 5 s, not a
/// fraction of one: on a loaded gate (two earlier gates) one dispatcher round can take over a second, so a
/// shorter pause could pass with no round for b in it. Both providers count time from one start.
#[test]
fn a_429_pauses_only_its_instance_and_its_jobs_come_back() {
    let root = root("429");
    let mut a = Provider::new("aaa@4", 4, Duration::from_millis(20));
    a.limited = AtomicUsize::new(1);
    a.retry_after_ms = 5_000;
    let mut b = Provider::new("bbb@4", 4, Duration::from_millis(20));
    b.t0 = a.t0;
    let (a, b) = (Arc::new(a), Arc::new(b));
    let d = daemon(&root, &[("a", &a), ("b", &b)]);
    add(&d, "ga", 0, 60);
    add(&d, "gb", 0, 200);
    eventually("both graphs embedded", SETTLE, || {
        pending(&d, "ga") == 0 && pending(&d, "gb") == 0
    });
    let a_log = a.log.lock().unwrap().clone();
    let refused = a_log.iter().find(|(_, r)| *r).expect("a 429").0;
    // No call STARTS in the pause, except calls already sent when the 429 came: a's 3 slots (ceiling 4 less one)
    // hold the refused call and at most 2 others until the dispatcher reads the 429, however late a loaded machine
    // lets those 2 start. A literal, not slots(): the guard must not read the rule it checks.
    let started_in_pause: Vec<u64> = a_log
        .iter()
        .filter(|(t, r)| !*r && *t >= refused && *t < refused + a.retry_after_ms)
        .map(|(t, _)| *t)
        .collect();
    assert!(
        started_in_pause.len() <= 2,
        "a was paused: a refused at {refused} ms, calls started in its pause {started_in_pause:?}"
    );
    assert!(
        a_log.iter().any(|(t, _)| *t >= refused + a.retry_after_ms),
        "a's jobs came back after the wait"
    );
    let b_log = b.log.lock().unwrap().clone();
    let b_in_pause = b_log
        .iter()
        .filter(|(t, _)| *t > refused && *t < refused + a.retry_after_ms)
        .count();
    assert!(
        b_in_pause > 0,
        "b went on while a was paused: a refused at {refused} ms, a's calls {a_log:?}, b's calls {b_log:?}"
    );
    let _ = fs::remove_dir_all(&root);
}

/// A search's query embed has its own slot: with every background slot busy on a provider that takes no more than
/// the ceiling at once, the query still goes at once, not behind a background call.
#[test]
fn a_query_gets_its_slot_during_a_full_background_load() {
    let root = root("query");
    let p = Arc::new(Provider::new("aaa@4", 4, Duration::ZERO));
    let d = daemon(&root, &[("a", &p)]);
    // Some vectors first, so the search runs its vector leg (and so a query embed).
    add(&d, "ga", 0, 8);
    eventually("the first vectors", SETTLE, || pending(&d, "ga") == 0);
    p.delay_ms.store(1_500, Ordering::SeqCst);
    add(&d, "ga", 8, 200);
    eventually("every background slot busy", SETTLE, || {
        p.in_flight.load(Ordering::SeqCst) == embed_worker::slots(&*p)
    });
    let mut search = op(Op::Search {
        text: "concept".into(),
        within: None,
        limit: None,
        along: None,
    });
    search.graph = Some("ga".into());
    let started = Instant::now();
    let env = ok(d.handle(&search));
    let took = started.elapsed();
    assert!(
        took < Duration::from_millis(700),
        "the query waited behind background calls: {took:?}"
    );
    assert!(
        !env.warnings
            .iter()
            .any(|w| w.code() == "similarity_unavailable"),
        "{}",
        env.to_json()
    );
    assert_eq!(p.queries.load(Ordering::SeqCst), 1, "the query embed ran");
    let _ = fs::remove_dir_all(&root);
}

/// The send-time re-check holds under concurrency: once a graph is secured, its hosted instance gets no new batch,
/// not even one already planned and waiting for its pace. At most one call that passed the check just before the
/// policy changed may still start.
#[test]
fn a_graph_secured_mid_drain_sends_its_hosted_instance_no_further_batch() {
    let root = root("secured");
    let mut p = Provider::new("aaa@4", 8, Duration::from_millis(10));
    p.locality = Locality::Hosted;
    let p = Arc::new(p);
    // About 5 calls a second: planned batches wait in the dispatcher for their pace.
    let d = daemon_paced(&root, &[("a", &p)], [("a".to_string(), 60_000)].into());
    add(&d, "ga", 0, 400);
    eventually("the drain under way", SETTLE, || {
        p.calls.load(Ordering::SeqCst) >= 4
    });
    let mut set = op(Op::GraphSet {
        alias: "ga".into(),
        settings: vec![SettingArg {
            name: "policy".into(),
            value: "secured".into(),
        }],
    });
    set.graph = Some("ga".into());
    assert!(pending(&d, "ga") > 0, "the drain was still under way");
    ok(d.handle(&set));
    let at_secure = p.calls.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(1_500));
    let after = p.calls.load(Ordering::SeqCst);
    assert!(
        after - at_secure <= 1,
        "{} calls started after the graph was secured",
        after - at_secure
    );
    let _ = fs::remove_dir_all(&root);
}

/// A unit of Apply `apply` with one vector for each of `nodes`, each tagged `tag` (its value).
fn tagged(alias: &str, shard: &str, nodes: &[u128], apply: u64, tag: f32) -> Unit {
    Unit {
        at: (alias.into(), shard.into()),
        vectors: nodes
            .iter()
            .map(|&node| umx_services::Vectors {
                job: umx_services::EmbedJob {
                    alias: alias.into(),
                    node: umx_core::Id(node),
                },
                key: None,
                vectors: vec![(
                    "title".into(),
                    umx_core::vectors::TextHash::of("t"),
                    vec![tag],
                )],
            })
            .collect(),
        applies: vec![apply],
        lasts: 0,
    }
}

fn unit(alias: &str, shard: &str, node: u128, apply: u64) -> Unit {
    tagged(alias, shard, &[node], apply, 0.0)
}

/// R201-2: an instance waiting on its pace keeps neither its ready batch nor its queued jobs in another's way. At 6
/// tokens a minute a's next call is over 20 minutes off (its first batch is over 120 estimated tokens), far beyond
/// [`SETTLE`]; b, not used before, embeds its whole graph meanwhile.
#[test]
fn a_paced_instance_never_keeps_another_instances_work_from_being_planned() {
    let root = root("paced");
    let a = Arc::new(Provider::new("aaa@4", 2, Duration::from_millis(10)));
    let b = Arc::new(Provider::new("bbb@4", 2, Duration::from_millis(10)));
    let d = daemon_paced(
        &root,
        &[("a", &a), ("b", &b)],
        [("a".to_string(), 6)].into(),
    );
    add(&d, "ga", 0, 40);
    // a's first vectors written (the daemon's own signal, not a sleep): its call is done and its pace is running, and
    // a's remaining jobs are ahead of b's in the queue, so a planner that let a's wait hide others would hide b.
    eventually("a's first vectors", SETTLE, || pending(&d, "ga") < 40);
    assert_eq!(a.calls.load(Ordering::SeqCst), 1, "a's one call so far");
    add(&d, "gb", 0, 40);
    eventually("b's whole graph", SETTLE, || pending(&d, "gb") == 0);
    assert_eq!(
        a.calls.load(Ordering::SeqCst),
        1,
        "a still waits on its pace"
    );
    assert!(pending(&d, "ga") > 0);
    let _ = fs::remove_dir_all(&root);
}

/// graph-storage-expert (s25 D2): a popped unit takes later units for the same graph and shard, in queue order (so a
/// node's later vectors win), up to the batch size, and leaves the rest where they were.
#[test]
fn a_taken_unit_merges_later_units_for_its_shard_in_order_up_to_the_cap() {
    let mut q: std::collections::VecDeque<Unit> = std::collections::VecDeque::new();
    // Apply 1 has units for shards 01 and 02; Applies 2, 3 and 4 one each.
    q.push_back(unit("g", "01", 1, 1));
    q.push_back(unit("g", "02", 2, 1));
    q.push_back(unit("g", "01", 3, 2));
    q.push_back(unit("h", "01", 4, 3));
    q.push_back(unit("g", "01", 1, 4));
    let first = take_unit(&mut q).unwrap();
    let nodes: Vec<u128> = first.vectors.iter().map(|v| v.job.node.0).collect();
    assert_eq!(nodes, [1, 3, 1], "same graph and shard, in queue order");
    assert_eq!(
        first.lasts, 2,
        "Applies 2 and 4 end here; 1 still waits on shard 02"
    );
    let rest: Vec<(String, String)> = q.iter().map(|u| u.at.clone()).collect();
    assert_eq!(
        rest,
        [("g".into(), "02".into()), ("h".into(), "01".into())],
        "others keep their order"
    );
    for i in 0..(2 * MERGED_UNIT_MAX) as u128 {
        q.push_back(unit("k", "05", i, 100 + i as u64));
    }
    q.retain(|u| u.at.0 == "k");
    let capped = take_unit(&mut q).unwrap();
    assert_eq!(capped.vectors.len(), MERGED_UNIT_MAX);
    assert_eq!(q.len(), MERGED_UNIT_MAX);
}

/// R201-1: merging stops at the first unit for the shard that does not fit. Units of 8, 12 and 8 nodes, with node 99
/// in the second and third: taking the third with the first would write its vector for 99 before the second's.
#[test]
fn a_unit_that_does_not_fit_ends_the_merge_so_a_nodes_later_vector_still_wins() {
    let mut q: std::collections::VecDeque<Unit> = std::collections::VecDeque::new();
    let b: Vec<u128> = (10..21).chain([99]).collect();
    let c: Vec<u128> = (30..37).chain([99]).collect();
    q.push_back(tagged("g", "01", &(0..8).collect::<Vec<_>>(), 1, 1.0));
    q.push_back(tagged("g", "01", &b, 2, 2.0));
    q.push_back(tagged("g", "01", &c, 3, 3.0));
    let mut written: Vec<(u128, f32)> = Vec::new();
    let mut sizes = Vec::new();
    while let Some(u) = take_unit(&mut q) {
        sizes.push(u.vectors.len());
        written.extend(u.vectors.iter().map(|v| (v.job.node.0, v.vectors[0].2[0])));
    }
    let last_for_99 = written.iter().rev().find(|(n, _)| *n == 99).map(|w| w.1);
    assert_eq!(
        last_for_99,
        Some(3.0),
        "the later unit's vector is written last: {sizes:?}"
    );
    assert_eq!(sizes, [8, 12, 8]);
}

/// R201-3: Apply 1's only unit (shard z) merges with Apply 2's last unit (shard z) while Apply 2's shard-a unit still
/// waits: writing the merged unit finishes Apply 1 only, and Apply 2 ends with its shard-a unit.
#[test]
fn a_merged_unit_finishes_an_apply_only_when_none_of_its_units_still_waits() {
    let mut q: std::collections::VecDeque<Unit> = std::collections::VecDeque::new();
    q.push_back(unit("g", "z", 1, 1));
    q.push_back(unit("g", "a", 2, 2));
    q.push_back(unit("g", "z", 3, 2));
    let merged = take_unit(&mut q).unwrap();
    assert_eq!(merged.vectors.len(), 2);
    assert_eq!(merged.lasts, 1, "Apply 2's shard-a unit is still waiting");
    let rest = take_unit(&mut q).unwrap();
    assert_eq!(rest.at.1, "a");
    assert_eq!(rest.lasts, 1);
    assert!(q.is_empty());
}

/// The s25 bench (run alone: `--ignored --exact`): 1,000 nodes with similar_to lists on, through a fake provider at
/// 100 ms a call (the writer is the limit) and at 400 ms (about Azure's: the provider is), each at 1 slot (one call at
/// a time, as before s25) and at 31. Each run waits until its owed similar_to files are written before the next
/// starts, so one daemon's file writes never slow the next one's drain.
#[test]
#[ignore = "timing bench: run alone with --ignored --exact (s25 evidence)"]
fn s25_drain_rate_bench() {
    for (delay_ms, ceiling) in [(100, 2), (100, 32), (400, 2), (400, 32)] {
        let root = root(&format!("bench-{delay_ms}-{ceiling}"));
        let p = Arc::new(Provider::new(
            "aaa@4",
            ceiling,
            Duration::from_millis(delay_ms),
        ));
        let d = daemon(&root, &[("a", &p)]);
        ok(d.handle(&op(Op::SimilarityRebuild)));
        LONGEST_UNIT_HOLD_US.store(0, Ordering::SeqCst);
        let nodes = 1_000;
        let started = Instant::now();
        for from in (0..nodes).step_by(250) {
            add(&d, "ga", from, 250);
        }
        eventually("every vector", Duration::from_secs(900), || {
            pending(&d, "ga") == 0
        });
        let applied = started.elapsed().as_secs_f64();
        eventually("the similar_to files", Duration::from_secs(900), || {
            !d.services.read().unwrap().similar_owed()
        });
        let settled = started.elapsed().as_secs_f64();
        eprintln!(
            "s25 bench: provider {delay_ms} ms, ceiling {ceiling} ({} slots): {nodes} nodes applied in {applied:.1} s = \
             {:.0} nodes/s ({settled:.1} s with list files written), {:.1} provider calls/s, most in flight {}, \
             longest hold {:?}",
            embed_worker::slots(&*p),
            nodes as f64 / applied,
            p.calls.load(Ordering::SeqCst) as f64 / applied,
            p.most.load(Ordering::SeqCst),
            Duration::from_micros(LONGEST_UNIT_HOLD_US.load(Ordering::SeqCst)),
        );
        let _ = fs::remove_dir_all(&root);
    }
}
