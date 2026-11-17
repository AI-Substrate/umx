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
    texts: AtomicUsize,
    /// Each background call's texts, in order.
    sizes: Mutex<Vec<usize>>,
    /// The most inputs a call takes: a longer one is refused, as a provider would (ws004).
    max_batch: usize,
    /// A call holding a text with this in it is refused for good (a bad input).
    poison: Option<&'static str>,
    /// Report what a call counted (bytes / 4), as Azure does.
    count_tokens: bool,
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
            texts: AtomicUsize::new(0),
            sizes: Mutex::new(Vec::new()),
            max_batch: 16,
            poison: None,
            count_tokens: false,
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
            self.texts.fetch_add(texts.len(), Ordering::SeqCst);
            self.sizes.lock().unwrap().push(texts.len());
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
        if texts.len() > self.max_batch {
            return Err(EmbedError::BadResponse(format!(
                "{} inputs; the provider takes {}",
                texts.len(),
                self.max_batch
            )));
        }
        if let Some(bad) = self.poison
            && texts.iter().any(|t| t.contains(bad))
        {
            return Err(EmbedError::BadResponse("a bad input".into()));
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
            tokens: self
                .count_tokens
                .then(|| texts.iter().map(|t| (t.len() / 4).max(1) as u64).sum()),
        })
    }
    fn key(&self) -> ModelKey {
        ModelKey::parse(self.key).unwrap()
    }
    fn concurrency_ceiling(&self) -> usize {
        self.ceiling
    }
    fn max_batch_inputs(&self) -> usize {
        self.max_batch
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
            no_std: false,
        })));
    }
    d
}

fn add(d: &Daemon, alias: &str, from: usize, n: usize) {
    let mut req = op(Op::Batch {
        related: None,
        raised_in: None,
        idempotency_key: None,
        echo_sentences: false,
        lines: (from..from + n)
            .map(|i| BatchLine {
                line: (i - from) as u32 + 1,
                op: BatchOp::NodeAdd {
                    on_key: None,
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
/// the ceiling at once, the query still goes at once, not behind a background call. The calls are full ones of 64
/// texts (ws004): a larger call never takes the query's slot.
#[test]
fn a_query_gets_its_slot_during_a_full_background_load() {
    let root = root("query");
    let mut provider = Provider::new("aaa@4", 4, Duration::ZERO);
    provider.max_batch = 64;
    let p = Arc::new(provider);
    let d = daemon(&root, &[("a", &p)]);
    // Some vectors first, so the search runs its vector leg (and so a query embed).
    add(&d, "ga", 0, 8);
    eventually("the first vectors", SETTLE, || pending(&d, "ga") == 0);
    p.delay_ms.store(1_500, Ordering::SeqCst);
    add(&d, "ga", 8, 200);
    eventually("every background slot busy", SETTLE, || {
        p.in_flight.load(Ordering::SeqCst) == embed_worker::slots(&*p)
    });
    let running = sizes(&p);
    let in_flight = &running[running.len() - embed_worker::slots(&*p)..];
    assert!(
        in_flight.iter().all(|&n| n > 16),
        "full calls in flight: {running:?}"
    );
    let mut search = op(Op::Search {
        text: "concept".into(),
        within: None,
        limit: None,
        along: None,
        also: Vec::new(),
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
        !env.hints
            .iter()
            .chain(&env.warnings)
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

/// The texts of `p`'s background calls, in order.
fn sizes(p: &Provider) -> Vec<usize> {
    p.sizes.lock().unwrap().clone()
}

/// Graphs `r0`.. of `aaa@4` beside the daemon's own, each with `nodes` nodes (three texts each: title, note and the
/// whole node), added one graph at a time.
fn graphs(d: &Daemon, root: &std::path::Path, count: usize, nodes: usize) -> Vec<String> {
    let aliases: Vec<String> = (0..count).map(|i| format!("r{i}")).collect();
    for a in &aliases {
        ok(d.handle(&op(Op::GraphNew {
            path: root.join(a).display().to_string(),
            alias: a.clone(),
            title: None,
            empty: false,
            embedding_model: Some("aaa@4".into()),
            secured: false,
            no_graphfile: false,
            graphfile: false,
            no_std: false,
        })));
    }
    for a in &aliases {
        add(d, a, 0, nodes);
    }
    aliases
}

/// ws004: with work plentiful, calls are cut at the instance's own limit (`max_batch_inputs`), not at 16 and not
/// at whatever few nodes a round held: 3 graphs of 200 nodes (1,800 texts) at 64 a call go in about 29 full calls,
/// where 16 a call or 8 nodes a round made 113 or more. At most one partial call per graph's burst of work and the
/// last, since a round that filled its calls puts its partial one back while more waits.
#[test]
fn background_calls_fill_to_the_instances_batch_limit() {
    let root = root("fill");
    let mut provider = Provider::new("aaa@4", 4, Duration::ZERO);
    provider.max_batch = 64;
    let p = Arc::new(provider);
    let d = daemon(&root, &[("a", &p)]);
    let aliases = graphs(&d, &root, 3, 200);
    eventually("every vector", SETTLE, || {
        aliases.iter().all(|a| pending(&d, a) == 0)
    });
    let sizes = sizes(&p);
    let texts: usize = sizes.iter().sum();
    assert_eq!(texts, 1_800, "{sizes:?}");
    // A literal, not max_batch_inputs(): the guard must not read the rule it checks. A full call may stop 2 short:
    // the next node's 3 texts would not fit.
    let full = sizes.iter().filter(|&&n| n >= 62).count();
    assert!(
        sizes.len() <= 1_800 / 62 + 4 && full >= sizes.len() - 4,
        "{} calls, {full} full: {sizes:?}",
        sizes.len()
    );
    let _ = fs::remove_dir_all(&root);
}

/// ws004: full calls keep the pace's 429 rule: the first 429 halves the instance's rate once, its batch comes back
/// whole and is embedded, and no call ever goes over the provider's limit.
#[test]
fn a_429_on_a_full_call_still_halves_the_rate_and_its_batch_comes_back() {
    let root = root("429-full");
    let mut provider = Provider::new("aaa@4", 4, Duration::ZERO);
    provider.max_batch = 64;
    provider.limited = AtomicUsize::new(1);
    let p = Arc::new(provider);
    let pace = 1_000_000;
    let d = daemon_paced(
        &root,
        &[("a", &p)],
        [("a".to_string(), pace)].into_iter().collect(),
    );
    let aliases = graphs(&d, &root, 1, 200);
    eventually("every vector", SETTLE, || pending(&d, &aliases[0]) == 0);
    let stats = d.drain.stats();
    let a = &stats["a"];
    assert_eq!(a.rate_limited, 1);
    assert_eq!(a.rate, Some(pace / 2), "one 429 halves the rate once");
    let sizes = sizes(&p);
    assert!(sizes.iter().all(|&n| n <= 64), "{sizes:?}");
    assert!(sizes[0] > 16, "the refused call was a full one: {sizes:?}");
    // The refused call counts in `sizes` too: its texts came back and were sent again.
    assert_eq!(sizes.iter().sum::<usize>(), 600 + sizes[0], "{sizes:?}");
    let _ = fs::remove_dir_all(&root);
}

/// ws004: one bad input in a full call costs a few calls, not one per node: the batch is halved down to the bad node,
/// which alone is dropped. 300 nodes at 256 texts a call: about 4 calls, plus 2 per halving (8 levels), where trying
/// each node alone made about 85 more.
#[test]
fn a_bad_input_in_a_full_call_is_found_by_halving() {
    let root = root("halving");
    let mut provider = Provider::new("aaa@4", 4, Duration::ZERO);
    provider.max_batch = 256;
    provider.poison = Some("concept 7 of r0");
    let p = Arc::new(provider);
    let d = daemon(&root, &[("a", &p)]);
    let aliases = graphs(&d, &root, 1, 300);
    // Once only the bad node is left, at most its own last try is still to come.
    eventually("every vector but the bad node's", SETTLE, || {
        pending(&d, &aliases[0]) == 1
    });
    let calls = sizes(&p).len() + 1;
    assert!(calls <= 4 + 2 + 2 * 8, "{calls} calls: {:?}", sizes(&p));
    let _ = fs::remove_dir_all(&root);
}

/// ws004: an Apply of a full call holds many nodes, but a writer's unit still holds at most [`MERGED_UNIT_MAX`] of
/// one shard file's, in order, so the longest lock hold stays bounded.
#[test]
fn an_applys_units_hold_at_most_the_merged_unit_max_of_one_shard() {
    let one_shard: Vec<u128> = (0..40u128).map(|n| n << 10).collect();
    let shard = umx_core::vectors::shard_of(umx_core::Id(one_shard[0]));
    assert!(
        one_shard
            .iter()
            .all(|&n| umx_core::vectors::shard_of(umx_core::Id(n)) == shard),
        "the nodes share a shard"
    );
    let vectors = tagged("g", &shard, &one_shard, 1, 0.0).vectors;
    let got = units(vectors, 1);
    let lens: Vec<usize> = got.iter().map(|u| u.vectors.len()).collect();
    assert_eq!(lens, [16, 16, 8]);
    let order: Vec<u128> = got
        .iter()
        .flat_map(|u| u.vectors.iter().map(|v| v.job.node.0))
        .collect();
    assert_eq!(order, one_shard);
}

/// The batching bench (run alone: `--ignored --exact --nocapture`), small on purpose: 3 graphs of 200 nodes (three
/// texts each), a fake provider of ceiling 32 at 400 ms a call (about Azure's) taking 2,048 texts a call, paced at
/// 1.5M tokens a minute, each note 3,000 bytes and the provider counting bytes / 4, as Azure does. A search every 2 s
/// meanwhile shows the query lane is still served.
#[test]
#[ignore = "timing bench: run alone with --ignored --exact --nocapture (ws004 evidence)"]
fn ws004_batching_bench() {
    let (graphs_n, nodes, delay, limit, pace, note) = (3, 200, 400, 2048, 1_500_000u64, 3000usize);
    let root = root("ws004-bench");
    let mut provider = Provider::new("aaa@4", 32, Duration::from_millis(delay));
    provider.max_batch = limit;
    provider.count_tokens = true;
    let p = Arc::new(provider);
    let d = daemon_paced(
        &root,
        &[("a", &p)],
        [("a".to_string(), pace)].into_iter().collect(),
    );
    let filler = "word ".repeat(note / 5);
    let aliases: Vec<String> = (0..graphs_n).map(|i| format!("r{i}")).collect();
    for a in &aliases {
        ok(d.handle(&op(Op::GraphNew {
            path: root.join(a).display().to_string(),
            alias: a.clone(),
            title: None,
            empty: false,
            embedding_model: Some("aaa@4".into()),
            secured: false,
            no_graphfile: false,
            graphfile: false,
            no_std: false,
        })));
    }
    let started = Instant::now();
    for a in &aliases {
        let mut req = op(Op::Batch {
            related: None,
            raised_in: None,
            echo_sentences: false,
            lines: (0..nodes)
                .map(|i| BatchLine {
                    line: i as u32 + 1,
                    op: BatchOp::NodeAdd {
                        on_key: None,
                        raised_in: None,
                        id: None,
                        created: None,
                        ref_name: None,
                        kind: "problem".into(),
                        title: format!("concept {i} of {a}"),
                        status: None,
                        note: Some(format!("note {i} {filler}")),
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
            idempotency_key: None,
        });
        req.graph = Some(a.clone());
        ok(d.handle(&req));
    }
    // A search every 2 s meanwhile: its query embed must not wait behind the background calls.
    let mut searches = Vec::new();
    let mut next = Instant::now();
    while !aliases.iter().all(|a| pending(&d, a) == 0) {
        if Instant::now() >= next {
            let mut search = op(Op::Search {
                text: "concept".into(),
                within: None,
                limit: None,
                along: None,
                also: Vec::new(),
            });
            search.graph = Some(aliases[0].clone());
            let t = Instant::now();
            ok(d.handle(&search));
            searches.push(t.elapsed().as_millis());
            next = Instant::now() + Duration::from_secs(2);
        }
        std::thread::sleep(Duration::from_millis(20));
        assert!(started.elapsed() < Duration::from_secs(900));
    }
    let secs = started.elapsed().as_secs_f64();
    let sizes = sizes(&p);
    let texts: usize = sizes.iter().sum();
    let stats = d.drain.stats();
    let counted = stats["a"].tokens_counted;
    eprintln!(
        "ws004 bench: {graphs_n} x {nodes} nodes, note {note} B, delay {delay} ms, limit {limit}, pace {pace}/min: \
         {texts} texts in {} calls = {:.1} a call (largest {}), {secs:.1} s, {:.0} counted tokens/min, most in flight \
         {}, search ms {searches:?}",
        sizes.len(),
        texts as f64 / sizes.len() as f64,
        sizes.iter().max().unwrap_or(&0),
        counted as f64 * 60.0 / secs,
        p.most.load(Ordering::SeqCst),
    );
    let _ = fs::remove_dir_all(&root);
}
