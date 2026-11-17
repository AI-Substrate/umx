//! ws004 s31: `umx similarity pairs` through the daemon. The reply's shape for the graph view, `--all` taking the
//! one scope gate, and a write that never waits on the compute (it holds no lock).

use super::*;
use std::sync::mpsc;
use umx_core::ports::{EmbedError, Embedded, Embedder, Locality, ModelKey, TextKind};
use umx_protocol::{Op, PairRows};

fn build() -> Build {
    Build {
        version: "t".into(),
        commit: "t".into(),
        built: "t".into(),
    }
}

fn ok(env: Envelope) -> Envelope {
    assert!(env.errors.is_empty(), "{}", env.to_json());
    env
}

/// A local fake: a 4-dim vector from the text's length, at once.
struct Quick;

impl Embedder for Quick {
    fn embed(&self, texts: &[String], _: TextKind) -> Result<Embedded, EmbedError> {
        Ok(Embedded {
            vectors: texts
                .iter()
                .map(|t| {
                    let a = (t.len() % 7) as f32 + 1.0;
                    let b = (t.len() % 3) as f32;
                    let n = (a * a + b * b + 1.0).sqrt();
                    vec![a / n, b / n, 1.0 / n, 0.0]
                })
                .collect(),
            truncated: Vec::new(),
            tokens: None,
        })
    }
    fn key(&self) -> ModelKey {
        ModelKey::parse("quick@4").unwrap()
    }
    fn concurrency_ceiling(&self) -> usize {
        1
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

fn pairs(graphs: Option<&str>) -> Request {
    pairs_rows(graphs, PairRows::Full)
}

fn pairs_rows(graphs: Option<&str>, rows: PairRows) -> Request {
    Request::op(
        Op::SimilarityPairs {
            k: Some(2),
            min: None,
            model: None,
            same_graph: false,
            limit: None,
            offset: None,
            rows,
        },
        graphs.map(str::to_owned),
        None,
        build(),
    )
}

fn node_add(graph: &str, title: &str) -> Request {
    Request::op(
        Op::NodeAdd {
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
        Some(graph.into()),
        None,
        build(),
    )
}

/// Two open graphs, a and b, of `n` embedded nodes each.
fn daemon(name: &str, n: usize) -> (Daemon, std::path::PathBuf) {
    daemon_with(name, n, &[("a", false), ("b", false)])
}

/// `graphs` (alias, secured) of `n` embedded nodes each; a secured graph is read only when named.
fn daemon_with(name: &str, n: usize, graphs: &[(&str, bool)]) -> (Daemon, std::path::PathBuf) {
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("umx-s31-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let config = Config::at(root.join("home"), Some(true)).roots([root.to_str().unwrap()]);
    let (services, settings) = umx_compose::compose_with_config(umx_compose::Mode::Real, &config);
    let d = Daemon::with_services(
        &config,
        build(),
        services.with_embedder(Arc::new(Quick)),
        settings,
        umx_compose::Mode::Real,
        umx_compose::Embedders::from_instances(Vec::new()),
    );
    for &(g, secured) in graphs {
        ok(d.handle(&Request::op(
            Op::GraphNew {
                path: root.join(g).display().to_string(),
                alias: g.into(),
                title: None,
                empty: false,
                embedding_model: None,
                secured,
                no_graphfile: false,
                graphfile: false,
            },
            None,
            None,
            build(),
        )));
        for i in 0..n {
            ok(d.handle(&node_add(g, &format!("concept {i}{}", "x".repeat(i % 5)))));
        }
    }
    let until = Instant::now() + Duration::from_secs(30);
    while graphs.iter().any(|(g, _)| {
        d.services
            .read()
            .unwrap()
            .loaded(g)
            .and_then(|l| l.pending_count())
            .unwrap_or(1)
            > 0
    }) {
        assert!(Instant::now() < until, "the nodes were never embedded");
        std::thread::sleep(Duration::from_millis(20));
    }
    (d, root)
}

#[test]
fn the_reply_is_the_list_shape_the_view_reads() {
    let (d, root) = daemon("shape", 4);
    // As a client reads it: the rows are written as JSON beside the envelope (ws004 s34a).
    let env = Envelope::from_json(ok(d.handle(&pairs(Some("a,b")))).to_json().as_bytes()).unwrap();
    assert_eq!(env.command, "similarity pairs");
    let data = &env.data;
    let rows = data["rows"].as_array().unwrap();
    assert!(!rows.is_empty());
    for row in rows {
        assert_ne!(row["graph"], row["other_graph"]);
        assert!(row["score"].is_f64() && row["rank"].is_u64());
        assert!(row["address"].as_str().unwrap().starts_with("umx:"));
    }
    assert_eq!(data["total"], rows.len());
    assert_eq!(data["truncated"], false);
    assert_eq!(data["model"], "quick@4");
    assert_eq!(data["model_source"], "named-first");
    assert_eq!(data["k"], 2);
    assert_eq!(data["min"]["source"], "none");
    assert_eq!(data["graphs"], json!(["a", "b"]));
    assert_eq!(data["nodes"], 8);
    assert!(
        env.warnings
            .iter()
            .any(|w| w.code() == "floor_uncalibrated")
    );
    // --all takes the one scope gate; with --graph it is refused there.
    let mut all = pairs(None);
    all.all = true;
    ok(d.handle(&all));
    all.graph = Some("a".into());
    let refused = d.handle(&all);
    assert_eq!(refused.errors[0].code(), "usage_invalid");
    let _ = fs::remove_dir_all(&root);
}

/// The five fields of every row, as JSON text, in reply order.
fn five(env: &Envelope) -> Vec<String> {
    env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let pick: serde_json::Map<String, Value> =
                ["graph", "id", "other_graph", "other_id", "score"]
                    .iter()
                    .map(|c| ((*c).to_string(), r[*c].clone()))
                    .collect();
            serde_json::to_string(&pick).unwrap()
        })
        .collect()
}

/// ws004 s34b: `--rows ids` through the daemon answers the full read's rows, in its order, with only the five
/// fields, and the same list data around them.
#[test]
fn lean_rows_are_the_full_rows_five_fields_in_order() {
    let (d, root) = daemon("lean", 4);
    let read = |rows| {
        Envelope::from_json(
            ok(d.handle(&pairs_rows(Some("a,b"), rows)))
                .to_json()
                .as_bytes(),
        )
        .unwrap()
    };
    let (full, lean) = (read(PairRows::Full), read(PairRows::Ids));
    assert!(!five(&full).is_empty());
    assert_eq!(five(&lean), five(&full));
    for row in lean.data["rows"].as_array().unwrap() {
        assert_eq!(row.as_object().unwrap().len(), 5, "{row}");
    }
    assert_eq!(
        lean.data["columns"],
        json!(["graph", "id", "other_graph", "other_id", "score"])
    );
    for key in ["total", "truncated", "model", "k", "graphs", "nodes"] {
        assert_eq!(lean.data[key], full.data[key], "{key}");
    }
    let _ = fs::remove_dir_all(&root);
}

/// Security on s34b: `--rows ids` takes the same scope as full rows. A secured graph left unnamed adds no lean row
/// and is only counted; named, it is read (the control that the test could see it). `embeddings status` gives it
/// no `vectors_digest`.
#[test]
fn a_secured_unnamed_graph_gives_no_lean_row_and_no_digest() {
    let (d, root) = daemon_with("secured", 3, &[("a", false), ("b", false), ("c", true)]);
    let names_c = |env: &Envelope| {
        env.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["graph"] == "c" || r["other_graph"] == "c")
    };
    let unnamed = Envelope::from_json(
        ok(d.handle(&pairs_rows(None, PairRows::Ids)))
            .to_json()
            .as_bytes(),
    )
    .unwrap();
    assert!(!five(&unnamed).is_empty());
    assert!(!names_c(&unnamed), "{}", unnamed.to_json());
    assert!(
        !unnamed.to_json().contains("\"c\""),
        "{}",
        unnamed.to_json()
    );
    assert!(
        unnamed
            .hints
            .iter()
            .any(|w| w.code() == "secured_graphs_skipped")
    );
    let named = Envelope::from_json(
        ok(d.handle(&pairs_rows(Some("a,b,c"), PairRows::Ids)))
            .to_json()
            .as_bytes(),
    )
    .unwrap();
    assert!(names_c(&named), "{}", named.to_json());
    // ws004 s36 (F206): the unnamed read lists only a and b as compared; named, c's digest is given (its rows are).
    assert_eq!(compared_graphs(&unnamed), ["a", "b"]);
    assert_eq!(compared_graphs(&named), ["a", "b", "c"]);
    assert!(
        named.data["compared"][2]["vectors_digest"]
            .as_str()
            .is_some_and(|h| h.len() == 64)
    );
    let status = ok(d.handle(&Request::op(Op::EmbeddingsStatus, None, None, build())));
    let digest = |alias: &str| {
        status.data["graphs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["graph"] == alias)
            .unwrap()["vectors_digest"]
            .clone()
    };
    assert!(
        digest("a").as_str().is_some_and(|h| h.len() == 64),
        "{}",
        status.to_json()
    );
    assert_eq!(digest("c"), Value::Null);
    let _ = fs::remove_dir_all(&root);
}

/// Each graph's `vectors_digest` from `embeddings status`.
fn digests(d: &Daemon) -> std::collections::BTreeMap<String, Value> {
    let status = ok(d.handle(&Request::op(Op::EmbeddingsStatus, None, None, build())));
    status.data["graphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| {
            (
                g["graph"].as_str().unwrap().to_string(),
                g["vectors_digest"].clone(),
            )
        })
        .collect()
}

/// ws004 s34b: the daemon keeps each digest until its graph's vectors change. A repeated status reads the same;
/// a new vector in a moves a's digest through the memo, and b's stays.
#[test]
fn the_status_digest_follows_new_vectors_through_the_memo() {
    let (d, root) = daemon("memo", 2);
    let first = digests(&d);
    assert!(
        first["a"].is_string() && first["b"].is_string(),
        "{first:?}"
    );
    assert_eq!(digests(&d), first);
    ok(d.handle(&node_add("a", "a new concept")));
    let until = Instant::now() + Duration::from_secs(30);
    while d
        .services
        .read()
        .unwrap()
        .loaded("a")
        .and_then(|l| l.pending_count())
        .unwrap_or(1)
        > 0
    {
        assert!(Instant::now() < until, "the node was never embedded");
        std::thread::sleep(Duration::from_millis(20));
    }
    let after = digests(&d);
    assert_ne!(
        after["a"], first["a"],
        "a's new vector did not move its digest"
    );
    assert_eq!(after["b"], first["b"]);
    let _ = fs::remove_dir_all(&root);
}

/// The graphs a pairs reply lists in `compared`, in order.
fn compared_graphs(env: &Envelope) -> Vec<String> {
    env.data["compared"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["graph"].as_str().unwrap().to_string())
        .collect()
}

/// A pairs reply's `compared` as alias -> digest, read as a client reads it.
fn compared(d: &Daemon, rows: PairRows) -> std::collections::BTreeMap<String, Value> {
    let env = Envelope::from_json(
        ok(d.handle(&pairs_rows(Some("a,b"), rows)))
            .to_json()
            .as_bytes(),
    )
    .unwrap();
    env.data["compared"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["graph"].as_str().unwrap().to_string(),
                c["vectors_digest"].clone(),
            )
        })
        .collect()
}

/// ws004 s36 (F206): with full rows and with `--rows ids`, each compared graph's digest is `embeddings status`' for
/// an unchanged graph, whichever of the two reads the memo first.
#[test]
fn the_compared_digests_are_the_status_digests() {
    let (d, root) = daemon("compared", 3);
    let pairs_first = compared(&d, PairRows::Full);
    let status = digests(&d);
    assert!(status["a"].is_string() && status["b"].is_string());
    assert_eq!(pairs_first, status);
    assert_eq!(compared(&d, PairRows::Ids), status);
    let _ = fs::remove_dir_all(&root);
}

/// Security on s36: when the finish's re-check drops a run (a graph loaded again after the copy) and the call runs
/// again, `compared` is the second copy's, never the stale one's. The hook adds and embeds a node in a, then loads a
/// again, once; the reply's digest for a is the new one.
#[test]
fn a_rerun_never_gives_the_stale_copys_digest() {
    let (d, root) = daemon("rerun", 3);
    let before = digests(&d);
    let runs = Arc::new(AtomicUsize::new(0));
    let d = Arc::new(d);
    {
        let (runs, weak) = (runs.clone(), Arc::downgrade(&d));
        let path = root.join("a").display().to_string();
        *d.pairs_hook.lock().unwrap() = Some(Box::new(move || {
            if runs.fetch_add(1, Ordering::SeqCst) > 0 {
                return;
            }
            let d = weak.upgrade().unwrap();
            ok(d.handle(&node_add("a", "added after the copy")));
            let until = Instant::now() + Duration::from_secs(30);
            while d
                .services
                .read()
                .unwrap()
                .loaded("a")
                .and_then(|l| l.pending_count())
                .unwrap_or(1)
                > 0
            {
                assert!(Instant::now() < until, "the node was never embedded");
                std::thread::sleep(Duration::from_millis(20));
            }
            let op = |op| Request::op(op, None, None, build());
            ok(d.handle(&op(Op::GraphUnload { alias: "a".into() })));
            ok(d.handle(&op(Op::GraphLoad {
                path: path.clone(),
                as_alias: Some("a".into()),
                by_alias: None,
                graphfile: false,
                set: None,
            })));
        }));
    }
    let got = compared(&d, PairRows::Ids);
    assert_eq!(
        runs.load(Ordering::SeqCst),
        2,
        "the finish dropped the first run"
    );
    let now = digests(&d);
    assert_ne!(now["a"], before["a"], "a's new vector moved its digest");
    assert_eq!(got, now);
    *d.pairs_hook.lock().unwrap() = None;
    let _ = fs::remove_dir_all(&root);
}

/// The compute holds no lock: a write made while it runs returns at once (s25a's bar). Planting the compute under
/// the read lock fails this test.
#[test]
fn a_write_never_waits_on_the_compute() {
    let (d, root) = daemon("write", 3);
    let (reached_tx, reached) = mpsc::channel::<()>();
    let (release, release_rx) = mpsc::channel::<()>();
    let (reached_tx, release_rx) = (Mutex::new(reached_tx), Mutex::new(release_rx));
    *d.pairs_hook.lock().unwrap() = Some(Box::new(move || {
        reached_tx.lock().unwrap().send(()).unwrap();
        let _ = release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(30));
    }));
    std::thread::scope(|s| {
        let d = &d;
        let compute = s.spawn(move || d.handle(&pairs(Some("a,b"))));
        reached
            .recv_timeout(Duration::from_secs(30))
            .expect("the compute began");
        let (done_tx, done) = mpsc::channel();
        s.spawn(move || {
            let _ = done_tx.send(d.handle(&node_add("a", "written meanwhile")));
        });
        let wrote = done.recv_timeout(Duration::from_secs(5));
        release.send(()).unwrap();
        ok(wrote.expect("the write waited on the compute"));
        ok(compute.join().unwrap());
    });
    let _ = fs::remove_dir_all(&root);
}

/// A restart or stop begun while the compute runs cancels it: similarity_pairs_timed_out, no rows.
#[test]
fn a_restart_during_the_compute_cancels_it() {
    let (d, root) = daemon("cancel", 3);
    d.relaunching.store(true, Ordering::SeqCst);
    let env = d.handle(&pairs(Some("a,b")));
    d.relaunching.store(false, Ordering::SeqCst);
    assert_eq!(
        env.errors[0].code(),
        "similarity_pairs_timed_out",
        "{}",
        env.to_json()
    );
    assert!(env.errors[0].message().contains("stopping or restarting"));
    assert_eq!(env.data, Value::Null);
    let _ = fs::remove_dir_all(&root);
}

/// The bench's vector for node `i` of graph `g`: 256 int8 values from a seeded xorshift, so every run is the same.
fn seeded_row(g: usize, i: usize) -> Vec<i8> {
    let mut x = (0x9e37_79b9_7f4a_7c15u64 ^ ((g as u64) << 40) ^ i as u64) | 1;
    (0..256)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % 255) as i8
        })
        .collect()
}

/// Writes a whole-node vector for every node in `dir`'s nodes.jsonl as committed `wide@256` shard files, with the
/// product's shard encoder: a generated graph with vectors, made in seconds and never committed (graph-storage R298).
fn write_shards(dir: &Path, g: usize) -> usize {
    use umx_core::vectors::{
        ShardLine, TextHash, VectorLine, encode_shard, shard_of, whole_node_field,
    };
    let mut by_shard: std::collections::BTreeMap<String, Vec<ShardLine>> = Default::default();
    let nodes = fs::read_to_string(dir.join("nodes.jsonl")).unwrap();
    // Content nodes only: provenance nodes (agent, session) are never embedded.
    let mut ids: Vec<umx_core::Id> = nodes
        .lines()
        .map(|l| umx_protocol::parse_json(l).unwrap())
        .filter(|v: &Value| v["type"] == "problem")
        .map(|v| v["id"].as_str().unwrap().parse::<umx_core::Id>().unwrap())
        .collect();
    ids.sort();
    for (i, id) in ids.iter().enumerate() {
        by_shard
            .entry(shard_of(*id))
            .or_default()
            .push(ShardLine::Vector(VectorLine {
                node: *id,
                field: whole_node_field(),
                hash: TextHash::of("bench"),
                values: seeded_row(g, i),
            }));
    }
    let space = dir.join("embeddings/wide@256");
    fs::create_dir_all(&space).unwrap();
    for (shard, lines) in by_shard {
        fs::write(space.join(format!("{shard}.tsv")), encode_shard(&lines)).unwrap();
    }
    ids.len()
}

fn quantile(mut times: Vec<Duration>, q: usize) -> Duration {
    times.sort();
    times[(times.len() * q).div_ceil(100).max(1) - 1]
}

#[test]
#[ignore = "bench on a shared machine: run alone with --ignored --exact --nocapture (s31 evidence)"]
fn pairs_bench_5x2000() {
    pairs_bench(5, 2_000, 3, None);
}

/// The largest two equal graphs under the pair cap (2 x 50k is refused before copying).
#[test]
#[ignore = "bench on a shared machine: run alone with --ignored --exact --nocapture (s31 evidence)"]
fn pairs_bench_2x44000() {
    pairs_bench(2, 44_000, 3, None);
}

/// F194 (ws004 s34a): the view's read, 12 graphs of 2k nodes at k 10, every row (240k) in one call, against one
/// 10k page. Adds the reply's bytes and the most the daemon holds above live for one call, counted by the
/// allocator.
#[test]
#[ignore = "bench on a shared machine: run alone with --ignored --exact --nocapture (s34a evidence)"]
fn pairs_bench_12x2000_every_row() {
    pairs_bench(12, 2_000, 10, Some(250_000));
}

/// `graphs` graphs (`g0`, `g1`, ...) of `nodes` nodes, each with a generated `wide@256` vector per node loaded and its
/// similar_to lists rebuilt; returns the daemon, its temp root, the aliases and the vectors written.
fn bench_daemon(graphs: usize, nodes: usize) -> (Daemon, std::path::PathBuf, Vec<String>, usize) {
    let spec = format!("{graphs}x{nodes}");
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("umx-s31-bench-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let config = Config::at(root.join("home"), Some(true)).roots([root.to_str().unwrap()]);
    let (services, settings) = umx_compose::compose_with_config(umx_compose::Mode::Real, &config);
    let d = Daemon::with_services(
        &config,
        build(),
        services,
        settings,
        umx_compose::Mode::Real,
        umx_compose::Embedders::from_instances(Vec::new()),
    );
    let aliases: Vec<String> = (0..graphs).map(|g| format!("g{g}")).collect();
    let started = Instant::now();
    for alias in &aliases {
        ok(d.handle(&Request::op(
            Op::GraphNew {
                path: root.join(alias).display().to_string(),
                alias: alias.clone(),
                title: None,
                empty: false,
                embedding_model: Some("wide@256".into()),
                secured: false,
                no_graphfile: false,
                graphfile: false,
            },
            None,
            None,
            build(),
        )));
        let mut from = 0;
        while from < nodes {
            let n = (nodes - from).min(10_000);
            let lines = (from..from + n)
                .map(|i| umx_protocol::BatchLine {
                    line: (i - from) as u32 + 1,
                    op: umx_protocol::BatchOp::NodeAdd {
                        on_key: None,
                        raised_in: None,
                        id: None,
                        created: None,
                        ref_name: None,
                        kind: "problem".into(),
                        title: format!("{alias} synthetic concept {i}"),
                        status: None,
                        note: None,
                        sets: Vec::new(),
                        parent: None,
                        parent_ref: None,
                        evidence: Vec::new(),
                        evidence_kind: None,
                    },
                })
                .collect();
            ok(d.handle(&Request::op(
                Op::Batch {
                    related: None,
                    raised_in: None,
                    idempotency_key: None,
                    reason: None,
                    lines,
                    dry_run: false,
                    echo_sentences: false,
                    refs: Default::default(),
                    refs_graph: None,
                },
                Some(alias.clone()),
                None,
                build(),
            )));
            from += n;
        }
    }
    // No embedder: the vectors are generated shard files, then a normal load reads them into the blocks.
    let mut vectors = 0;
    for (g, alias) in aliases.iter().enumerate() {
        let dir = root.join(alias);
        vectors += write_shards(&dir, g);
        ok(d.handle(&Request::op(
            Op::GraphLoad {
                path: dir.display().to_string(),
                as_alias: None,
                by_alias: None,
                graphfile: false,
                set: None,
            },
            None,
            None,
            build(),
        )));
    }
    // A load with vectors and no similar/ files rebuilds each graph's similar_to lists in the background, through
    // the writer: wait for that before timing any write, or the first write waits behind it.
    let until = Instant::now() + Duration::from_secs(1800);
    while aliases.iter().any(|g| {
        d.services
            .read()
            .unwrap()
            .loaded(g)
            .is_none_or(|l| l.similar_stale || l.similar_meta_owed || !l.similar_owed.is_empty())
    }) {
        assert!(
            Instant::now() < until,
            "the similar_to lists were never rebuilt"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    for i in 0..10 {
        ok(d.handle(&node_add("g0", &format!("warm {i}"))));
    }
    eprintln!(
        "s31 bench {spec}: built, {vectors} vectors written and loaded, lists rebuilt in {:?}",
        started.elapsed()
    );
    (d, root, aliases, vectors)
}

fn pairs_request(aliases: &[String], k: usize, limit: Option<usize>, rows: PairRows) -> Request {
    Request::op(
        Op::SimilarityPairs {
            k: Some(k),
            min: None,
            model: None,
            same_graph: false,
            limit,
            offset: None,
            rows,
        },
        Some(aliases.join(",")),
        None,
        build(),
    )
}

/// graph-storage-expert's bench (s31): `graphs` graphs of `nodes` nodes at 256 dims, top `k`, `limit` rows a call.
/// Reports each read hold (plan, each graph's copy, finish), the compute, the bytes copied, and a client write's p95
/// and max while it runs against the same writes with nothing else running. Bar: p95 within 1.25x and max within
/// 1.5x. With a limit it also reports, for that call with full rows, for it with `--rows ids` (s34b) and for one 10k
/// page, the time, the reply's bytes and the daemon's peak above live (s34a).
fn pairs_bench(graphs: usize, nodes: usize, k: usize, limit: Option<usize>) {
    let spec = format!("{graphs}x{nodes}");
    let (d, root, aliases, vectors) = bench_daemon(graphs, nodes);
    let request_for =
        |limit: Option<usize>, rows: PairRows| pairs_request(&aliases, k, limit, rows);
    let request = request_for(limit, PairRows::Full);
    let write = |i: usize| {
        let t = Instant::now();
        ok(d.handle(&node_add("g0", &format!("write {i}"))));
        t.elapsed()
    };
    let baseline: Vec<Duration> = (0..40)
        .map(|i| {
            std::thread::sleep(Duration::from_millis(20));
            write(i)
        })
        .collect();
    PAIRS_TIMES.lock().unwrap().clear();
    let running = std::sync::atomic::AtomicBool::new(true);
    let (env, total, during) = std::thread::scope(|s| {
        let (d, request, running) = (&d, &request, &running);
        // Calls back to back for at least 5 s, so enough writes land while one runs; total is the first call's.
        let pairs = s.spawn(move || {
            let t = Instant::now();
            let env = d.handle(request);
            let first = t.elapsed();
            while t.elapsed() < Duration::from_secs(5) && env.errors.is_empty() {
                d.handle(request);
            }
            running.store(false, Ordering::SeqCst);
            (env, first)
        });
        let mut during = Vec::new();
        let mut i = 1000;
        while running.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(20));
            during.push(write(i));
            i += 1;
        }
        let (env, total) = pairs.join().unwrap();
        (env, total, during)
    });
    let times = PAIRS_TIMES.lock().unwrap().clone();
    let of = |step: &str| -> Vec<Duration> {
        times
            .iter()
            .filter(|(s, _)| *s == step)
            .map(|(_, t)| *t)
            .collect()
    };
    let calls = of("compute").len().max(1);
    let copies = of("copy");
    let rows = env.data["nodes"].as_u64().unwrap_or(0);
    // The instrument can see what was written: every generated vector was compared.
    assert_eq!(rows as usize, vectors, "{}", env.to_json());
    eprintln!(
        "s31 bench {spec}: {:?} errors {:?}; total {total:?}; plan p50 {:?}; copy holds p50 {:?} max {:?} ({} copies); \
         compute {:?}; finish {:?}; {calls} calls; {rows} rows copied, {} bytes ({:.1} MiB); {} pair rows",
        env.status,
        env.errors.iter().map(|e| e.code()).collect::<Vec<_>>(),
        quantile(of("plan"), 50),
        copies
            .iter()
            .copied()
            .max()
            .map(|_| quantile(copies.clone(), 50)),
        copies.iter().max(),
        copies.len(),
        quantile(of("compute"), 50),
        quantile(of("finish"), 50),
        rows * (256 + 20),
        (rows * (256 + 20)) as f64 / (1 << 20) as f64,
        env.data["total"],
    );
    eprintln!(
        "s31 bench {spec}: finish holds max {:?} over {calls} calls",
        of("finish").iter().max()
    );
    let (b95, bmax) = (quantile(baseline.clone(), 95), quantile(baseline, 100));
    let (d95, dmax) = (quantile(during.clone(), 95), quantile(during.clone(), 100));
    eprintln!(
        "s31 bench {spec}: client writes p95 {b95:?} max {bmax:?} alone; p95 {d95:?} max {dmax:?} during ({} writes; \
         {:.2}x p95, {:.2}x max)",
        during.len(),
        d95.as_secs_f64() / b95.as_secs_f64(),
        dmax.as_secs_f64() / bmax.as_secs_f64(),
    );
    if let Some(limit) = limit {
        // One call at a time, nothing else running: its wall time, finish hold, reply bytes (as the daemon writes
        // them) and the most the daemon holds above live from the request to the written bytes. Full and lean rows
        // alternate, three runs each (s34b), so machine load hits both alike.
        let mut runs = Vec::new();
        for _ in 0..3 {
            runs.push(("every row", limit, PairRows::Full));
            runs.push(("every row --rows ids", limit, PairRows::Ids));
        }
        runs.push(("one 10k page", 10_000, PairRows::Full));
        let mut counted: Vec<(PairRows, usize)> = Vec::new();
        for (what, limit, fields) in runs {
            let request = request_for(Some(limit), fields);
            PAIRS_TIMES.lock().unwrap().clear();
            let t = Instant::now();
            let ((rows, bytes), peak) = crate::test_alloc::peak_above_live(|| {
                let env = ok(d.handle(&request));
                // What the serve path writes: three parts, the rows held once.
                let (head, rows, tail) = env.json_parts();
                let count = rows.matches("\"score\":").count();
                (count, head.len() + rows.len() + tail.len())
            });
            let took = t.elapsed();
            if limit > 10_000 {
                counted.push((fields, rows));
            }
            let finish: Vec<Duration> = PAIRS_TIMES
                .lock()
                .unwrap()
                .iter()
                .filter(|(s, _)| *s == "finish")
                .map(|(_, t)| *t)
                .collect();
            eprintln!(
                "s34 bench {spec} k {k} {what}: {rows} rows in {took:?}; finish hold {:?}; reply {bytes} bytes \
                 ({:.1} MiB, {:.0} B/row); daemon peak above live {peak} bytes ({:.1} MiB, {:.2}x the reply)",
                finish.first(),
                bytes as f64 / (1 << 20) as f64,
                bytes as f64 / rows.max(1) as f64,
                peak as f64 / (1 << 20) as f64,
                peak as f64 / bytes as f64,
            );
        }
        // Both forms answer the same rows.
        assert!(counted.windows(2).all(|w| w[0].1 == w[1].1), "{counted:?}");
    }
    let _ = fs::remove_dir_all(&root);
    if env.errors.is_empty() {
        assert!(
            during.len() >= 5,
            "too few writes to judge: the compute was too quick"
        );
        assert!(d95.as_secs_f64() <= b95.as_secs_f64() * 1.25, "p95 bar");
        assert!(dmax.as_secs_f64() <= bmax.as_secs_f64() * 1.5, "max bar");
    }
}

/// graph-storage-expert's ask on s34b: the first, uncached `embeddings status` over one graph of `nodes` nodes
/// hashes every vector line under the services read hold. Reports that hold (the digest alone, then the whole
/// status call, then a cached call) against a client write's max with nothing else running, the s25a bar.
fn status_digest_bench(nodes: usize) {
    let (d, root, aliases, vectors) = bench_daemon(1, nodes);
    let write = |i: usize| {
        let t = Instant::now();
        ok(d.handle(&node_add("g0", &format!("write {i}"))));
        t.elapsed()
    };
    let baseline: Vec<Duration> = (0..40)
        .map(|i| {
            std::thread::sleep(Duration::from_millis(20));
            write(i)
        })
        .collect();
    let mut digest_holds = Vec::new();
    for _ in 0..5 {
        let services = d.services.read().unwrap();
        let stamp = services
            .embeddings_status(&|_, _| true)
            .remove(0)
            .vectors_stamp
            .unwrap();
        let t = Instant::now();
        let digest = services.vectors_digest(&aliases[0], &stamp);
        digest_holds.push(t.elapsed());
        assert!(digest.is_some());
    }
    let status = Request::op(Op::EmbeddingsStatus, None, None, build());
    // The daemon's first status call hashes (its memo is empty); the next reads the memo.
    let t = Instant::now();
    let first = ok(d.handle(&status));
    let uncached = t.elapsed();
    let t = Instant::now();
    let again = ok(d.handle(&status));
    let cached = t.elapsed();
    assert_eq!(
        first.data["graphs"][0]["vectors_digest"],
        again.data["graphs"][0]["vectors_digest"]
    );
    assert!(first.data["graphs"][0]["vectors_digest"].is_string());
    // ws004 s36 (graph-storage): a pairs copy takes the graph's digest in its own hold. Cold, a fresh memo hashes
    // every vector line there; warm, the memo answers. Each run plans, then times one copy under its read hold.
    let ctx = umx_services::Ctx::new("bench").graph(&aliases[0]);
    let ask = umx_services::PairsAsk::default();
    let mut warm = umx_services::DigestMemo::new();
    let (mut cold_holds, mut warm_holds) = (Vec::new(), Vec::new());
    for _ in 0..5 {
        for (memo, holds) in [
            (&mut umx_services::DigestMemo::new(), &mut cold_holds),
            (&mut warm, &mut warm_holds),
        ] {
            let services = d.services.read().unwrap();
            let mut job = services.pairs_plan(&ctx, &ask).unwrap();
            let t = Instant::now();
            assert!(services.pairs_copy(&mut job, 0, memo).unwrap());
            holds.push(t.elapsed());
        }
    }
    eprintln!(
        "s34b bench 1x{nodes} ({vectors} vectors): vectors_digest hold {digest_holds:?}; status uncached {uncached:?}, \
         cached {cached:?}; client writes alone p95 {:?} max {:?}",
        quantile(baseline.clone(), 95),
        quantile(baseline.clone(), 100),
    );
    eprintln!(
        "s36 bench 1x{nodes}: pairs copy hold, cold memo {cold_holds:?} (max {:?}); warm memo {warm_holds:?} (max {:?}); \
         client writes alone max {:?}",
        cold_holds.iter().max(),
        warm_holds.iter().max(),
        quantile(baseline, 100),
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
#[ignore = "bench on a shared machine: run alone with --ignored --exact --nocapture (s34b evidence)"]
fn status_digest_bench_100k() {
    status_digest_bench(100_000);
}
