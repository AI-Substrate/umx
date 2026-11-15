//! Hybrid recall@10 on the s03 synthetic fixture (300 nodes, 80 labelled queries): reported, not gated (workstream
//! s11 row; embeddings-expert). Two modes: the bag-of-words fake embedder (no IDF, floor 0.0: it measures the fake,
//! not the fusion), and recorded real vectors (embeddinggemma-2@256+p1 int8, every row production embeds, committed
//! beside the fixture; its calibrated search floor from umx_core::similarity::CALIBRATED, ws004 s26/s26b), on which
//! hybrid >= max(word, vector) is the check. Run it with
//! `tools/cargo-slot test -p umx-services --test hybrid_recall -- --ignored --nocapture`.
use std::collections::BTreeMap;
use std::sync::Arc;
use umx_core::Id;
use umx_core::ports::{EmbedError, Embedded, Embedder, GraphDir, Locality, ModelKey, TextKind};
use umx_core::vectors::{TextHash, base64_decode};
use umx_fakes::{FakeClock, FakeEmbedder, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{
    Ctx, HybridHit, NodeAddRequest, Pacer, Planned, Ports, Services, Step, batches, step,
};

fn fixture_path(name: &str) -> String {
    format!(
        "{}/../../docs/project/benchmarks/embed-bakeoff/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn fixture(name: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(fixture_path(name))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Recorded vectors by text (s26b): every text the production path embeds for the s03 fixture (whole node, title and
/// note per node, and each query), made by umx's llama adapter and stored int8 by umx's TextHash of the text, beside
/// the fixture with their provenance. A text with no recorded row (a changed text form) embeds as zeros and is
/// counted, so the tests insist on none.
struct Recorded {
    key: ModelKey,
    by_hash: BTreeMap<(TextKind, TextHash), Vec<f32>>,
    /// Texts asked for with no recorded row, with their kind.
    misses: std::sync::Mutex<Vec<(TextKind, String)>>,
}

impl Recorded {
    /// None when the vectors file is not committed yet.
    fn load() -> Option<Recorded> {
        let tsv = std::fs::read_to_string(fixture_path("vectors/embeddinggemma-2@256+p1.rows.tsv"))
            .ok()?;
        let mut by_hash = BTreeMap::new();
        for line in tsv.lines().filter(|l| !l.is_empty()) {
            let mut parts = line.split('\t');
            let (kind, hash, b64) = (
                parts.next().unwrap(),
                parts.next().unwrap(),
                parts.next().unwrap(),
            );
            let kind = match kind {
                "document" => TextKind::Document,
                "query" => TextKind::Query,
                other => panic!("unknown row kind {other}"),
            };
            let hash = TextHash::parse(hash).expect("a text hash");
            let v = base64_decode(b64)
                .expect("base64")
                .into_iter()
                .map(|b| f32::from(b as i8))
                .collect();
            by_hash.insert((kind, hash), v);
        }
        Some(Recorded {
            key: ModelKey::new("embeddinggemma-2", 256)
                .unwrap()
                .with_prompt_version(1),
            by_hash,
            misses: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// Texts asked for with no recorded row, other than a node title searched whole as a query (the exact-title
    /// check: an exact match ranks first whatever the vector leg says, so those searches are not recorded).
    fn misses(&self) -> Vec<(TextKind, String)> {
        let titles: std::collections::BTreeSet<String> = fixture("nodes.jsonl")
            .iter()
            .map(|n| n["title"].as_str().unwrap().to_string())
            .collect();
        self.misses
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, t)| !(*k == TextKind::Query && titles.contains(t)))
            .cloned()
            .collect()
    }
}

impl Embedder for Recorded {
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
        let dims = self.key.dims();
        Ok(Embedded {
            vectors: texts
                .iter()
                .map(|t| {
                    self.by_hash
                        .get(&(kind, TextHash::of(t)))
                        .cloned()
                        .unwrap_or_else(|| {
                            self.misses.lock().unwrap().push((kind, t.clone()));
                            vec![0.0; dims]
                        })
                })
                .collect(),
            truncated: Vec::new(),
            tokens: None,
        })
    }
    fn key(&self) -> ModelKey {
        self.key.clone()
    }
    fn concurrency_ceiling(&self) -> usize {
        1
    }
    fn max_input_tokens(&self) -> usize {
        8192
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
        "test.recorded"
    }
}

fn recall(top: &[Id], relevant: &[Id]) -> f64 {
    relevant.iter().filter(|r| top.contains(r)).count() as f64 / relevant.len() as f64
}

/// recall@10 for word, vector and hybrid over every query, and per query kind; how many of 50 titles come back
/// first when searched whole.
/// The figures of one run: overall (word, vector, hybrid), per kind the same, and exact titles first out of 50.
struct Figures {
    overall: (f64, f64, f64),
    /// Per query kind: word, vector and hybrid recall@10, and the kind's query count.
    kinds: BTreeMap<String, (f64, f64, f64, usize)>,
    exact_first: usize,
}

/// How far a query kind's hybrid recall may fall below its best leg: 0.05, or one query when the kind has fewer than 20
/// (embeddings-expert: at n = 10 a 0.05 bound allows nothing but a perfect score, so the bound is the kind's
/// granularity, not a loosening). Recall is a share of n queries, so a drop of exactly the slack can carry float noise.
fn kind_slack(n: usize) -> f64 {
    0.05f64.max(1.0 / n as f64)
}

fn report(label: &str, embedder: Arc<dyn Embedder>, share: f64) -> Figures {
    report_at(label, embedder, share, None)
}

/// [`report`] with the search floor set to `search_floor` through the machine-config override, or the model's
/// calibrated floor when None.
fn report_at(
    label: &str,
    embedder: Arc<dyn Embedder>,
    share: f64,
    search_floor: Option<f32>,
) -> Figures {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let ports = Ports {
        sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
            umx_core::ports::GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: files.clone(),
        vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    };
    let mut svc = Services::new(ports).with_embedder(embedder.clone());
    if let Some(f) = search_floor {
        let mut floors = umx_core::similarity::Floors::default();
        floors.set(&embedder.key(), Some(f), None);
        svc.set_floors(floors);
    }
    let ctx = Ctx::new("bench");
    svc.graph_new(&ctx, "/work/g", "fixture", None, false, None)
        .unwrap();
    let mut ids: BTreeMap<String, Id> = BTreeMap::new();
    for n in fixture("nodes.jsonl") {
        let added = svc
            .node_add(
                &ctx,
                NodeAddRequest {
                    kind: n["type"].as_str().unwrap().into(),
                    title: n["title"].as_str().unwrap().into(),
                    note: Some(n["note"].as_str().unwrap().into()),
                    ..Default::default()
                },
            )
            .unwrap();
        ids.insert(n["id"].as_str().unwrap().into(), added.value.node.id);
    }
    let mut pacer = Pacer::default();
    loop {
        let jobs = svc.take_embed_jobs();
        if jobs.is_empty() {
            break;
        }
        let planned: Vec<Planned> = jobs
            .iter()
            .filter_map(|j| svc.embed_plan(j).unwrap())
            .collect();
        for batch in batches(planned) {
            let Step::Done { vectors, .. } = step(&*embedder, &batch, &mut pacer, 0) else {
                panic!("the test embedders do not fail");
            };
            svc.embed_apply(&vectors).unwrap();
        }
    }
    let queries = fixture("queries.jsonl");
    let (mut word, mut vector, mut hybrid) = (0.0, 0.0, 0.0);
    let mut kinds: BTreeMap<String, (usize, f64, f64, f64)> = BTreeMap::new();
    for q in &queries {
        let relevant: Vec<Id> = q["relevant"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| ids[r.as_str().unwrap()])
            .collect();
        let plan = svc
            .search_plan(&ctx, q["text"].as_str().unwrap(), None, 1000)
            .unwrap();
        let queries = plan.embed();
        let hits = svc
            .search_finish_with(plan, queries, share)
            .unwrap()
            .value
            .hits;
        let by = |rank: fn(&HybridHit) -> Option<usize>| {
            let mut ranked: Vec<(usize, Id)> = hits
                .iter()
                .filter_map(|h| rank(h).map(|r| (r, h.node.id)))
                .collect();
            ranked.sort();
            ranked
                .into_iter()
                .take(10)
                .map(|(_, id)| id)
                .collect::<Vec<_>>()
        };
        let w = recall(&by(|h| h.word.as_ref().map(|w| w.rank)), &relevant);
        let v = recall(&by(|h| h.vector.as_ref().map(|v| v.rank)), &relevant);
        let top: Vec<Id> = hits.iter().take(10).map(|h| h.node.id).collect();
        let f = recall(&top, &relevant);
        (word, vector, hybrid) = (word + w, vector + v, hybrid + f);
        let k = kinds.entry(q["kind"].as_str().unwrap().into()).or_default();
        *k = (k.0 + 1, k.1 + w, k.2 + v, k.3 + f);
    }
    // Every node's own title, searched whole, must come back first (an exact title match is rank 1).
    let mut exact_first = 0;
    for id in ids.values().take(50) {
        let title = svc
            .loaded_graph("fixture")
            .unwrap()
            .node(*id)
            .unwrap()
            .title()
            .to_string();
        let hits = svc.search(&ctx, &title, None, 10).unwrap().value.hits;
        if hits[0].node.id == *id {
            exact_first += 1;
        }
    }
    let n = queries.len() as f64;
    let (word, vector, hybrid) = (word / n, vector / n, hybrid / n);
    eprintln!(
        "s03 fixture, {label}, word share {share}, {} queries: recall@10 word {word:.3}, vector {vector:.3}, hybrid {hybrid:.3}; exact title rank 1: {exact_first}/50",
        queries.len(),
    );
    let kinds: BTreeMap<String, (f64, f64, f64, usize)> = kinds
        .into_iter()
        .map(|(kind, (count, w, v, f))| {
            let n = count as f64;
            eprintln!(
                "  {kind} ({n}): word {:.3}, vector {:.3}, hybrid {:.3}",
                w / n,
                v / n,
                f / n
            );
            (kind, (w / n, v / n, f / n, count))
        })
        .collect();
    Figures {
        overall: (word, vector, hybrid),
        kinds,
        exact_first,
    }
}

#[test]
#[ignore = "a report: run with --ignored --nocapture"]
fn hybrid_recall_at_10_on_the_s03_fixture() {
    report(
        "fake@256 (bag of words, floor 0.0)",
        Arc::new(FakeEmbedder::new(ModelKey::new("fake", 256).unwrap())),
        umx_services::WORD_MAX_DF_SHARE,
    );
    let Some(recorded) = Recorded::load() else {
        eprintln!("recorded vectors not committed: that mode is skipped");
        return;
    };
    let recorded = Arc::new(recorded);
    let real = report(
        "recorded embeddinggemma-2@256+p1 (calibrated search floor)",
        recorded.clone(),
        umx_services::WORD_MAX_DF_SHARE,
    );
    // The embeddings-expert's acceptance on this mode, reported, not gated.
    let (w, v, h) = real.overall;
    let say = |ok: bool| if ok { "holds" } else { "does NOT hold" };
    eprintln!(
        "  check hybrid >= max(word, vector) - 0.02: {}",
        say(h >= w.max(v) - 0.02)
    );
    let worst = real
        .kinds
        .iter()
        .map(|(k, (w, v, h, n))| (k.clone(), h - (w.max(*v) - kind_slack(*n))))
        .fold((String::new(), f64::INFINITY), |a, b| {
            if b.1 < a.1 { b } else { a }
        });
    eprintln!(
        "  check no kind more than max(0.05, 1/n) below its best leg: {} (least margin {} {:+.3})",
        say(worst.1 >= -1e-9),
        worst.0,
        worst.1
    );
    eprintln!(
        "  check exact title rank 1 in 50: {}",
        say(real.exact_first == 50)
    );
    report(
        "recorded embeddinggemma-2@256+p1 (calibrated search floor), comparison only",
        recorded,
        0.10,
    );
}

/// The embeddings-expert's bounds on the committed real vectors (every production row, s26b), checked in CI with no
/// model: every fixture text has a recorded row; at the calibrated search floor hybrid recall@10 is within 0.02 of the
/// better leg and of its own floor-0 value, at least 0.95, vector at least 0.94; no query kind more than max(0.05, 1/n)
/// below its best leg (n its query count); every exact title rank 1.
#[test]
fn hybrid_holds_its_recall_bounds_on_the_recorded_real_vectors() {
    let recorded =
        Arc::new(Recorded::load().expect("the recorded vectors are committed beside the fixture"));
    let at_0 = report_at(
        "recorded embeddinggemma-2@256+p1 (floor 0)",
        recorded.clone(),
        umx_services::WORD_MAX_DF_SHARE,
        Some(0.0),
    );
    let f = report(
        "recorded embeddinggemma-2@256+p1 (calibrated search floor)",
        recorded.clone(),
        umx_services::WORD_MAX_DF_SHARE,
    );
    assert_eq!(
        recorded.misses(),
        vec![],
        "every fixture text has a recorded row (a changed text form needs a new recording)"
    );
    let (w, v, h) = f.overall;
    assert!(
        h >= w.max(v) - 0.02,
        "hybrid {h:.3}, word {w:.3}, vector {v:.3}"
    );
    // Absolute bounds at the calibrated search floor (embeddings-expert, s26b): the bound above is relative, so a
    // floor that cuts the vector leg (0.7401 gave vector 0.756, hybrid 0.825 on the s03 whole-node rows) passed it.
    assert!(
        h >= 0.95,
        "hybrid recall@10 {h:.3} < 0.95 at the calibrated floor"
    );
    assert!(
        v >= 0.94,
        "vector recall@10 {v:.3} < 0.94 at the calibrated floor"
    );
    assert!(
        h >= at_0.overall.2 - 0.02,
        "hybrid recall@10 {h:.3} more than 0.02 below floor 0's {:.3}",
        at_0.overall.2
    );
    for (kind, (w, v, h, n)) in &f.kinds {
        assert!(
            *h >= w.max(*v) - kind_slack(*n) - 1e-9,
            "{kind} ({n} queries): hybrid {h:.3}, word {w:.3}, vector {v:.3}"
        );
    }
    assert_eq!(f.exact_first, 50);
}

/// The recorded rows were made with umx's current model and text form (embeddings-expert, s26b): the pinned model file
/// and its sha256, the whole-node text version and the field form all equal what umx uses now, and the rows file is
/// the one recorded. A model or form change fails here until the rows are recorded again (umx-bench
/// `floor-calibration --record`), so CI never measures stale vectors.
#[test]
fn the_recorded_rows_match_the_current_model_and_text_form() {
    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture_path("vectors/embeddinggemma-2@256+p1.rows.json"))
            .expect("the rows' provenance is committed beside them"),
    )
    .unwrap();
    let source = umx_core::models::pinned("embeddinggemma-2").expect("a pinned model");
    let file = &source.files[0];
    assert_eq!(meta["key"], "embeddinggemma-2@256+p1");
    assert_eq!(meta["repo"], source.repo.as_str());
    assert_eq!(
        meta["commit"],
        source.commit.as_str(),
        "the model's commit moved"
    );
    assert_eq!(meta["file"], file.path.as_str());
    assert_eq!(
        meta["file_sha256"],
        file.sha256.as_str(),
        "the model file moved"
    );
    assert_eq!(
        meta["whole_node_version"],
        umx_core::vectors::WHOLE_NODE_VERSION,
        "the whole-node text form changed"
    );
    // The rows were recorded under form 1; form 2 (D56) differs only in leaving data fields out, so they stay valid
    // while the nodes carry nothing but a title and a note (ws004 s22).
    for n in fixture("nodes.jsonl") {
        let mut keys: Vec<&str> = n.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["id", "note", "title", "type"], "{n}");
    }
    assert_eq!(
        meta["field_form"],
        umx_core::vectors::FIELD_FORM,
        "the field text form changed"
    );
    let rows = std::fs::read(fixture_path("vectors/embeddinggemma-2@256+p1.rows.tsv")).unwrap();
    assert_eq!(
        meta["rows_sha256"],
        umx_core::models::sha256_hex(&rows).as_str(),
        "the rows file is not the one recorded"
    );
    assert_eq!(meta["rows"], rows.iter().filter(|&&b| b == b'\n').count());
}
