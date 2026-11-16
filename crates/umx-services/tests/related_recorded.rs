//! Related nodes on write (PRD 001 req-123 done_when; ws004 s28, embeddings-expert ruling) on recorded real vectors:
//! a synthetic fixture shaped like the five reported pairs (paraphrased titles and notes) plus a random control pair,
//! on a key with a query prompt (embeddinggemma-2@256+p1: search floor) and one without (text-embedding-3-large@256:
//! similar_to floor), each at its calibrated floor from umx_core::similarity::CALIBRATED. Pairs 1, 2 and 4 must come
//! back as related and the control must not; pairs 3 and 5 (same topic, another claim) are printed, not asserted.
//! The rows were made by `umx-bench`'s `related-record` through umx's own services and adapters (provenance beside
//! them); a text with no recorded row fails the test, so a changed text form cannot pass on stale vectors.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use umx_core::ports::{EmbedError, Embedded, Embedder, GraphDir, Locality, ModelKey, TextKind};
use umx_core::vectors::{TextHash, base64_decode};
use umx_core::{GraphEmbedding, Id, Policy};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::related::RelatedRequest;
use umx_services::{Ctx, NodeAddRequest, Pacer, Planned, Ports, Services, Step, batches, step};

fn fixture_dir() -> String {
    format!("{}/tests/fixtures/related", env!("CARGO_MANIFEST_DIR"))
}

/// Serves each text's recorded row by kind and hash; a text with none is noted and embeds as zeros.
struct Recorded {
    key: ModelKey,
    locality: Locality,
    rows: BTreeMap<(TextKind, TextHash), Vec<f32>>,
    misses: Mutex<Vec<String>>,
}

impl Recorded {
    fn load(key: ModelKey, locality: Locality) -> Recorded {
        let tsv = std::fs::read_to_string(format!(
            "{}/{}.rows.tsv",
            fixture_dir(),
            key.to_string().replace('@', "_")
        ))
        .expect("the recorded rows are committed");
        let mut rows = BTreeMap::new();
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
            let v = base64_decode(b64)
                .unwrap()
                .into_iter()
                .map(|b| f32::from(b as i8))
                .collect();
            rows.insert((kind, TextHash::parse(hash).unwrap()), v);
        }
        Recorded {
            key,
            locality,
            rows,
            misses: Mutex::new(Vec::new()),
        }
    }
}

impl Embedder for Recorded {
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
        let dims = self.key.dims();
        Ok(Embedded {
            vectors: texts
                .iter()
                .map(|t| {
                    self.rows
                        .get(&(kind, TextHash::of(t)))
                        .cloned()
                        .unwrap_or_else(|| {
                            self.misses.lock().unwrap().push(t.clone());
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
        2048
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
        "test.recorded"
    }
}

fn services(embedder: Arc<Recorded>) -> Services {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let ports = Ports {
        sources: Arc::new(umx_fakes::FakeSourceReader::open(
            GraphDir::parse("/").unwrap(),
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
    // The default floors: the calibrated table, nothing set in a config.
    Services::new(ports).with_embedder(embedder)
}

/// What each new node of the fixture got: its name to the names of its related nodes, with scores.
fn run(key: ModelKey, locality: Locality) -> BTreeMap<String, Vec<(String, f32)>> {
    let embedder = Arc::new(Recorded::load(key, locality));
    let mut svc = services(embedder.clone());
    let nodes: Vec<serde_json::Value> =
        std::fs::read_to_string(format!("{}/nodes.jsonl", fixture_dir()))
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
    let mut ctx = Ctx::new("related-record");
    svc.graph_new(
        &ctx,
        "/work/g",
        "fixture",
        None,
        false,
        Some(GraphEmbedding::new(None, Policy::OPEN)),
    )
    .unwrap();
    ctx.graph = Some("fixture".into());
    let mut names: BTreeMap<Id, String> = BTreeMap::new();
    let add = |svc: &mut Services, names: &mut BTreeMap<Id, String>, n: &serde_json::Value| -> Id {
        let id = svc
            .node_add(
                &ctx,
                NodeAddRequest {
                    kind: "problem".into(),
                    title: n["title"].as_str().unwrap().into(),
                    note: n["note"].as_str().map(Into::into),
                    ..Default::default()
                },
            )
            .unwrap()
            .value
            .node
            .id;
        names.insert(id, n["name"].as_str().unwrap().to_string());
        id
    };
    for n in nodes.iter().filter(|n| n["role"] == "existing") {
        add(&mut svc, &mut names, n);
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
            match step(&*embedder, &batch, &mut pacer, 0) {
                Step::Done { vectors, .. } => {
                    svc.embed_apply(&vectors).unwrap();
                }
                other => panic!("{other:?}"),
            }
        }
    }
    let mut out = BTreeMap::new();
    for n in nodes.iter().filter(|n| n["role"] == "new") {
        let id = add(&mut svc, &mut names, n);
        let r = svc.related(
            &RelatedRequest {
                alias: "fixture",
                added: &[id],
                flag: None,
                machine: true,
                verb: "node add",
            },
            &|_, _| true,
        );
        assert!(r.hints.is_empty(), "{:?}", r.hints);
        let floor = r.floor.expect("checked");
        assert!(floor.source.word() == "calibrated", "{floor:?}");
        let rows = r.rows[&id]
            .iter()
            .map(|row| (names[&row.id].clone(), row.score))
            .collect();
        out.insert(n["name"].as_str().unwrap().to_string(), rows);
    }
    let misses = embedder.misses.lock().unwrap().clone();
    assert!(
        misses.is_empty(),
        "texts with no recorded row (re-record with related-record): {misses:?}"
    );
    out
}

fn expect(got: &BTreeMap<String, Vec<(String, f32)>>, key: &str) {
    eprintln!("{key}: {got:?}");
    let names = |n: &str| {
        got[n]
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>()
    };
    for (pair, partner) in [
        ("pair1", "pay-late"),
        ("pair2", "onboard-slow"),
        ("pair4", "ci-disk"),
    ] {
        assert!(
            names(pair).contains(&partner.to_string()),
            "{key}: {pair} must list {partner}: {:?}",
            got[pair]
        );
    }
    // The control is about something no existing node is about: it lists nothing, and the random node it is paired
    // with (the picnic) is nobody's related node.
    assert!(
        got["control"].is_empty(),
        "{key}: the control must list nothing: {:?}",
        got["control"]
    );
    for (name, rows) in got {
        assert!(
            rows.iter().all(|(n, _)| n != "picnic"),
            "{key}: {name} lists the random control node: {rows:?}"
        );
    }
}

#[test]
fn a_prompt_key_finds_pairs_1_2_and_4_and_not_the_control_at_its_search_floor() {
    let key = ModelKey::new("embeddinggemma-2", 256)
        .unwrap()
        .with_prompt_version(1);
    let got = run(key.clone(), Locality::Local);
    expect(&got, &key.to_string());
}

#[test]
fn a_key_without_a_prompt_finds_pairs_1_2_and_4_and_not_the_control_at_its_similar_floor() {
    let key = ModelKey::new("text-embedding-3-large", 256).unwrap();
    let got = run(key.clone(), Locality::Hosted);
    expect(&got, &key.to_string());
}
