//! ws004 s25a: writes never wait on the embed backlog. A graph whose every node is pending (as after a restart
//! with a new embedder) embeds through a slow hosted fake; meanwhile a batch of writes returns in its normal time.

use super::*;
use std::sync::atomic::AtomicUsize;
use umx_core::ports::{Credential, EmbedError, Embedded, Embedder, Locality, ModelKey, TextKind};
use umx_protocol::{BatchLine, BatchOp, Op};

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

/// A hosted fake that waits for `open`, then takes `delay` per call, like a remote provider.
struct Slow {
    open: Mutex<bool>,
    opened: Condvar,
    delay: Duration,
    calls: AtomicUsize,
}

impl Embedder for Slow {
    fn embed(&self, texts: &[String], _: TextKind) -> Result<Embedded, EmbedError> {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.opened.wait(open).unwrap();
        }
        drop(open);
        std::thread::sleep(self.delay);
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Embedded {
            vectors: texts
                .iter()
                .map(|t| {
                    let a = (t.len() % 7) as f32 + 1.0;
                    let n = (a * a + 1.0).sqrt();
                    vec![a / n, 1.0 / n, 0.0, 0.0]
                })
                .collect(),
            truncated: Vec::new(),
            tokens: None,
        })
    }
    fn key(&self) -> ModelKey {
        ModelKey::parse("slow@4").unwrap()
    }
    fn concurrency_ceiling(&self) -> usize {
        32
    }
    fn max_input_tokens(&self) -> usize {
        8191
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
            from: "slow".into(),
        })
    }
    fn adapter_id(&self) -> &'static str {
        "fake.embedder"
    }
}

fn node(i: usize) -> BatchLine {
    BatchLine {
        line: i as u32 + 1,
        op: BatchOp::NodeAdd {
            on_key: None,
            raised_in: None,
            id: None,
            created: None,
            ref_name: None,
            kind: "problem".into(),
            title: format!("synthetic concept number {i} about widgets"),
            status: None,
            note: Some(format!(
                "a synthetic note {i} with a few words so each node has two texts to embed"
            )),
            sets: Vec::new(),
            parent: None,
            parent_ref: None,
            evidence: Vec::new(),
            evidence_kind: None,
        },
    }
}

fn batch(from: usize, n: usize) -> Request {
    op(Op::Batch {
        related: None,
        raised_in: None,
        echo_sentences: false,
        lines: (from..from + n).map(node).collect(),
        dry_run: false,
        refs: Default::default(),
        refs_graph: None,
        reason: None,
    })
}

fn p95(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[(times.len() * 95).div_ceil(100) - 1]
}

/// graph-storage-expert's bar (s25a): a client write's p95 while 2,000 nodes drain stays within 1.5x of idle. The
/// cause it guards (measured 2026-11-11): every Apply wrote its vector shards and then every similar_to shard the
/// new vectors touched, under the write lock, in the writer's one FIFO; client writes waited behind it.
#[test]
#[ignore = "timing bench on a shared machine: run alone with --ignored --exact (s25a evidence)"]
fn a_batch_of_writes_returns_in_its_normal_time_during_a_full_embed_backlog() {
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("umx-s25a-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let config = Config::at(root.join("home"), Some(true)).roots([root.to_str().unwrap()]);
    let slow = Arc::new(Slow {
        open: Mutex::new(false),
        opened: Condvar::new(),
        delay: Duration::from_millis(50),
        calls: AtomicUsize::new(0),
    });
    let (services, settings) = umx_compose::compose_with_config(umx_compose::Mode::Real, &config);
    let services = services.with_embedder(slow.clone());
    let d = Daemon::with_services(
        &config,
        build(),
        services,
        settings,
        umx_compose::Mode::Real,
        umx_compose::Embedders::from_instances(Vec::new()),
    );
    ok(d.handle(&op(Op::GraphNew {
        path: root.join("g").display().to_string(),
        alias: "g".into(),
        title: None,
        empty: false,
        embedding_model: None,
        secured: false,
        no_graphfile: false,
        graphfile: false,
    })));
    // similar_to lists on, so every embedded node moves lists: the costly case.
    ok(d.handle(&op(Op::SimilarityRebuild)));
    let (nodes, samples, size) = (2000, 20, 5);
    let mut from = 0;
    while from < nodes {
        ok(d.handle(&batch(from, 250)));
        from += 250;
    }
    let timed = |d: &Daemon, from: &mut usize| {
        let started = Instant::now();
        ok(d.handle(&batch(*from, size)));
        *from += size;
        started.elapsed()
    };
    // Idle: the provider is stalled, nothing is applied.
    let idle: Vec<Duration> = (0..samples).map(|_| timed(&d, &mut from)).collect();
    // Every node is pending; the provider now answers and the backlog drains.
    *slow.open.lock().unwrap() = true;
    slow.opened.notify_all();
    let until = Instant::now() + Duration::from_secs(20);
    while slow.calls.load(Ordering::SeqCst) < 4 {
        assert!(Instant::now() < until, "the provider was never called");
        std::thread::sleep(Duration::from_millis(10));
    }
    let calls_before = slow.calls.load(Ordering::SeqCst);
    LONGEST_UNIT_HOLD_US.store(0, Ordering::SeqCst);
    let draining: Vec<Duration> = (0..samples)
        .map(|_| {
            std::thread::sleep(Duration::from_millis(50));
            timed(&d, &mut from)
        })
        .collect();
    let calls = slow.calls.load(Ordering::SeqCst) - calls_before;
    let pending = d
        .services
        .read()
        .unwrap()
        .loaded("g")
        .and_then(|l| l.pending_count())
        .unwrap_or(0);
    let longest = Duration::from_micros(LONGEST_UNIT_HOLD_US.load(Ordering::SeqCst));
    let (idle, draining) = (p95(idle), p95(draining));
    eprintln!(
        "s25a: {nodes} pending nodes, provider {:?} a call, {samples} {size}-op batches each: p95 {idle:?} idle, \
         {draining:?} while draining ({:.2}x); {calls} provider calls meanwhile, {pending} nodes still pending; \
         longest single unit hold {longest:?}",
        slow.delay,
        draining.as_secs_f64() / idle.as_secs_f64(),
    );
    let _ = fs::remove_dir_all(&root);
    assert!(
        calls > 0 && pending > 0,
        "the writes were timed during the drain"
    );
    assert!(
        draining.as_secs_f64() <= idle.as_secs_f64() * 1.5,
        "a write waited on the embed backlog: p95 {draining:?} against {idle:?} idle"
    );
}

/// R180-3: with client jobs and Apply units waiting at every turn and the backlog never quiet, owed similar_to
/// writes still get a step at least every FLUSH_EVERY + 1 turns once their debt is SIMILAR_FLUSH_MAX_AGE_MS old,
/// counted from when the debt began, and Apply units keep their turn. A deterministic clock: 10 ms a turn.
#[test]
fn overdue_similar_writes_get_a_turn_while_the_writer_never_runs_dry() {
    let mut s = Schedule::default();
    s.owed_now(0);
    let mut picks = Vec::new();
    let mut now = 0;
    while now < 2 * SIMILAR_FLUSH_MAX_AGE_MS {
        picks.push((now, s.pick(now, true, true, true, false)));
        now += 10;
    }
    let first_flush = picks
        .iter()
        .find(|(_, p)| *p == Pick::Flush)
        .map(|(t, _)| *t);
    assert_eq!(
        first_flush,
        Some(SIMILAR_FLUSH_MAX_AGE_MS),
        "the age counts from when the debt began, and none goes before"
    );
    let late: Vec<Pick> = picks
        .iter()
        .filter(|(t, _)| *t >= SIMILAR_FLUSH_MAX_AGE_MS)
        .map(|(_, p)| *p)
        .collect();
    for window in late.windows(FLUSH_EVERY + 1) {
        assert!(window.contains(&Pick::Flush), "{window:?}");
    }
    let all: Vec<Pick> = picks.iter().map(|(_, p)| *p).collect();
    // Overdue flush steps take turns too, so units come at least every 2 * (WRITES_BEFORE_APPLY + 1) turns.
    for window in all.windows(2 * (WRITES_BEFORE_APPLY + 1)) {
        assert!(window.contains(&Pick::Unit), "{window:?}");
    }
    assert!(all.contains(&Pick::Client));
}

/// R180-3: the debt's age starts at its first sight even with no owed_now, and resets once nothing is owed; idle
/// and quiet flushes at once, idle and not quiet waits.
#[test]
fn the_schedule_flushes_when_quiet_and_waits_otherwise() {
    let mut s = Schedule::default();
    assert_eq!(s.pick(0, false, false, true, false), Pick::Wait);
    assert_eq!(s.pick(5, false, false, true, true), Pick::Flush);
    assert_eq!(s.pick(10, false, false, false, false), Pick::Wait);
    assert_eq!(s.pick(10, true, false, false, false), Pick::Client);
    // A new debt seen at 20 is overdue at 20 + the age, not before.
    assert_eq!(s.pick(20, false, false, true, false), Pick::Wait);
    assert_eq!(
        s.pick(20 + SIMILAR_FLUSH_MAX_AGE_MS - 1, false, false, true, false),
        Pick::Wait
    );
    assert_eq!(
        s.pick(20 + SIMILAR_FLUSH_MAX_AGE_MS, false, false, true, false),
        Pick::Flush
    );
}
