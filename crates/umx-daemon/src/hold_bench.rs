//! ws004 s25 hold A/B (graph-storage-expert, S201-1): one 100 ms provider taking two calls at once (so one
//! background slot on s25), 1,000 nodes, similar_to lists on; every write-lock hold of an Apply unit or a flush step
//! is logged with the files it wrote. The same file builds on main, so the two binaries run interleaved.

use super::*;
use umx_core::ports::{EmbedError, Embedded, Embedder, Locality, ModelKey, TextKind};
use umx_protocol::{BatchLine, BatchOp};

/// 100 ms a call; s25 runs `ceiling - 1` of them at once, main one.
struct Slow(usize);

impl Embedder for Slow {
    fn embed(&self, texts: &[String], _kind: TextKind) -> Result<Embedded, EmbedError> {
        std::thread::sleep(Duration::from_millis(100));
        Ok(Embedded {
            vectors: texts
                .iter()
                .map(|t| {
                    let mut v = vec![0.0; 4];
                    v[t.len() % 4] = 1.0;
                    v
                })
                .collect(),
            ..Embedded::default()
        })
    }
    fn key(&self) -> ModelKey {
        ModelKey::parse("aaa@4").unwrap()
    }
    fn concurrency_ceiling(&self) -> usize {
        self.0
    }
    fn max_input_tokens(&self) -> usize {
        8191
    }
    fn locality(&self) -> Locality {
        Locality::Local
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

fn ok(env: Envelope) {
    assert!(env.errors.is_empty(), "{}", env.to_json());
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(900);
    while !check() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// count, max, p50, p95 and p99 of `us`, in ms.
fn stats(mut us: Vec<u64>) -> String {
    if us.is_empty() {
        return "none".into();
    }
    us.sort_unstable();
    let at =
        |q: f64| us[((q * us.len() as f64).ceil() as usize).clamp(1, us.len()) - 1] as f64 / 1000.0;
    format!(
        "n {} max {:.1} p50 {:.1} p95 {:.1} p99 {:.1} ms",
        us.len(),
        *us.last().unwrap() as f64 / 1000.0,
        at(0.50),
        at(0.95),
        at(0.99)
    )
}

/// One slot on s25, the same as main's one call at a time: the matched config of the A/B.
#[test]
#[ignore = "timing bench: run alone with --ignored --exact"]
fn hold_ab_bench() {
    run(2);
}

/// s25 only (architect A2): the same holds under 31 parallel calls.
#[test]
#[ignore = "timing bench: run alone with --ignored --exact"]
fn hold_bench_31_slots() {
    run(32);
}

fn run(ceiling: usize) {
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("umx-holdab-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let config = Config::at(root.join("home"), Some(true)).roots([root.to_str().unwrap()]);
    let (mut services, settings) =
        umx_compose::compose_with_config(umx_compose::Mode::Real, &config);
    services.set_instances(
        vec![umx_services::embedding::Instance {
            name: "a".into(),
            embedder: Arc::new(Slow(ceiling)) as Arc<dyn Embedder>,
        }],
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
    ok(d.handle(&op(Op::GraphNew {
        path: root.join("a").display().to_string(),
        alias: "ga".into(),
        title: None,
        empty: false,
        embedding_model: Some("aaa@4".into()),
        secured: false,
        no_graphfile: false,
        graphfile: false,
    })));
    ok(d.handle(&op(Op::SimilarityRebuild)));
    HOLDS.lock().unwrap().clear();
    let nodes = 1_000;
    let started = Instant::now();
    for from in (0..nodes).step_by(250) {
        let mut req = op(Op::Batch {
            raised_in: None,
            echo_sentences: false,
            lines: (from..from + 250)
                .map(|i| BatchLine {
                    line: (i - from) as u32 + 1,
                    op: BatchOp::NodeAdd {
                        on_key: None,
                        raised_in: None,
                        id: None,
                        created: None,
                        ref_name: None,
                        kind: "problem".into(),
                        title: format!("concept {i} of ga"),
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
        req.graph = Some("ga".into());
        ok(d.handle(&req));
    }
    let pending = || {
        d.services
            .read()
            .unwrap()
            .loaded("ga")
            .and_then(|l| l.pending_count())
            .unwrap_or(0)
    };
    eventually("every vector", || pending() == 0);
    let applied = started.elapsed().as_secs_f64();
    eventually("the similar_to files", || {
        !d.services.read().unwrap().similar_owed()
    });
    let holds = HOLDS.lock().unwrap().clone();
    let raw = std::env::temp_dir().join(format!("umx-holdab-raw-{}.tsv", std::process::id()));
    let mut lines = String::from("us\tkind\tfiles\n");
    for (us, files) in &holds {
        lines.push_str(&match files {
            Some(f) => format!("{us}\tapply\t{f}\n"),
            None => format!("{us}\tflush\t-\n"),
        });
    }
    fs::write(&raw, lines).unwrap();
    let pick = |keep: &dyn Fn(Option<usize>) -> bool| -> Vec<u64> {
        holds.iter().filter(|h| keep(h.1)).map(|h| h.0).collect()
    };
    let mut files = std::collections::BTreeMap::<usize, usize>::new();
    for f in holds.iter().filter_map(|h| h.1) {
        *files.entry(f).or_default() += 1;
    }
    eprintln!(
        "hold bench: ceiling {ceiling}, {nodes} nodes applied in {applied:.1} s = {:.0} nodes/s\n  apply holds: {}\n  \
         apply holds that wrote files: {}\n  apply holds per files written (files: holds): {files:?}\n  flush-step \
         holds: {}\n  raw log: {}",
        nodes as f64 / applied,
        stats(pick(&|f| f.is_some())),
        stats(pick(&|f| f.is_some_and(|f| f > 0))),
        stats(pick(&|f| f.is_none())),
        raw.display(),
    );
    let _ = fs::remove_dir_all(&root);
}
