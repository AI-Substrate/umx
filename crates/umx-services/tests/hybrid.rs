//! Hybrid search over the fakes (ws004 s11; req-30, req-37, req-53; workshop 008 section 11): exact matches lead,
//! word and vector legs fused by reciprocal rank, every degraded state said, and no query text sent where a graph's
//! policy forbids it.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use umx_core::ports::{
    Change, EmbedError, Embedded, Embedder, FileRef, GraphDir, GraphFiles, Locality, ModelKey,
    TextKind,
};
use umx_core::setup::GraphFilesSetup;
use umx_core::similarity::{Floor, FloorSource, Floors};
use umx_core::{GraphEmbedding, Id, Policy};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{
    Ctx, HybridResult, NodeAddRequest, Pacer, Planned, Ports, Services, Step, batches, step,
};

/// A test embedder by concept: each known word adds to one axis, so a paraphrase ("feline") lands where its
/// meaning is ("cat") though they share no word. Unknown words add nothing: text of only those has no direction.
struct Concepts {
    key: ModelKey,
    locality: Locality,
    calls: AtomicUsize,
    offline: AtomicBool,
    /// Fails the test if called: proves a query was never sent.
    forbidden: bool,
}

impl Concepts {
    fn new(locality: Locality) -> Concepts {
        Concepts {
            key: concepts_key(),
            locality,
            calls: AtomicUsize::new(0),
            offline: AtomicBool::new(false),
            forbidden: false,
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

/// Not a `fake*` model, so the default floor (0.50) applies.
fn concepts_key() -> ModelKey {
    ModelKey::new("concepts", 8).unwrap()
}

fn vector_of(text: &str) -> Vec<f32> {
    let mut v = vec![0.0f32; 8];
    for w in text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        let axis = match w {
            "cat" | "cats" | "feline" | "kitten" => 0,
            "dog" | "dogs" | "canine" | "puppy" | "walking" => 1,
            "approval" | "sign" | "off" => 2,
            "delay" | "delays" | "slow" | "late" | "queue" => 3,
            "food" => 4,
            _ => continue,
        };
        v[axis] += 1.0;
    }
    v
}

impl Embedder for Concepts {
    fn embed(&self, texts: &[String], _: TextKind) -> Result<Embedded, EmbedError> {
        assert!(
            !self.forbidden,
            "the query was sent to a forbidden embedder"
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.offline.load(Ordering::SeqCst) {
            return Err(EmbedError::Unavailable {
                reason: "offline".into(),
                next: String::new(),
            });
        }
        Ok(Embedded {
            vectors: texts.iter().map(|t| vector_of(t)).collect(),
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
        512
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
        "test.concepts"
    }
}

struct World {
    files: Arc<FakeGraphFiles>,
    embedder: Arc<Concepts>,
    svc: Services,
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

fn world_with(embedder: Concepts) -> World {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    world_on(files, embedder)
}

/// A world over existing fake files (a restart with another embedder).
fn world_on(files: Arc<FakeGraphFiles>, embedder: Concepts) -> World {
    let embedder = Arc::new(embedder);
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
    // "concepts" is not calibrated: these tests cut at 0.5 through the machine config's search_floor (ws004 s26).
    svc.set_floors(config_floors());
    World {
        files,
        embedder,
        svc,
    }
}

fn config_floors() -> Floors {
    let mut floors = Floors::default();
    floors.set(&concepts_key(), Some(0.5), None);
    floors
}

fn graph(w: &mut World, alias: &str, embedding: Option<GraphEmbedding>) {
    w.svc
        .graph_new(
            &ctx(),
            &format!("/work/{alias}"),
            alias,
            None,
            false,
            embedding,
        )
        .unwrap();
}

fn section(key: Option<ModelKey>, policy: Policy) -> GraphEmbedding {
    GraphEmbedding {
        key,
        text_form: 1,
        policy,
        similarity: None,
        extra: Default::default(),
    }
}

fn add(w: &mut World, alias: &str, title: &str, note: &str) -> Id {
    let mut c = ctx();
    c.graph = Some(alias.into());
    w.svc
        .node_add(
            &c,
            NodeAddRequest {
                kind: "problem".into(),
                title: title.into(),
                note: Some(note.into()),
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id
}

/// Embeds every queued node through `w.embedder`.
fn run(w: &mut World) {
    let embedder = w.embedder.clone();
    let mut pacer = Pacer::default();
    loop {
        let jobs = w.svc.take_embed_jobs();
        if jobs.is_empty() {
            return;
        }
        let planned: Vec<Planned> = jobs
            .iter()
            .filter_map(|j| w.svc.embed_plan(j).unwrap())
            .collect();
        for batch in batches(planned) {
            match step(&*embedder, &batch, &mut pacer, 0) {
                Step::Done { vectors, .. } => {
                    w.svc.embed_apply(&vectors).unwrap();
                }
                other => panic!("{other:?}"),
            }
        }
    }
}

fn search(w: &World, graph: Option<&str>, text: &str) -> (HybridResult, Vec<String>) {
    let mut c = ctx();
    c.graph = graph.map(str::to_string);
    let done = w.svc.search(&c, text, None, 10).unwrap();
    let codes = done.notes.iter().map(|p| p.code.to_string()).collect();
    (done.value, codes)
}

/// The demo graph: an open graph with vectors for every node.
fn demo() -> (World, [Id; 4]) {
    let mut w = world_with(Concepts::new(Locality::Local));
    graph(&mut w, "demo", Some(section(None, Policy::OPEN)));
    let cat = add(&mut w, "demo", "Cat food shortage", "");
    let approval = add(&mut w, "demo", "Approval queue", "sign-off is slow");
    let dog = add(&mut w, "demo", "Dog walking", "");
    let words = add(
        &mut w,
        "demo",
        "Budget",
        "the approval delay hits every vendor",
    );
    run(&mut w);
    (w, [cat, approval, dog, words])
}

#[test]
fn vector_candidates_ignore_provenance_and_reserved_fields_but_keep_user_agents() {
    let (w, [cat, approval, dog, _]) = demo();
    let mut graph = w.svc.loaded_graph("demo").unwrap().clone();
    let mut provenance = graph.node(cat).unwrap().to_node();
    provenance.kind = "agent".into();
    provenance
        .fields
        .insert("prov_key".into(), serde_json::json!("agent/pij/pij-seat"));
    graph.insert_node(provenance).unwrap();
    let mut user_agent = graph.node(dog).unwrap().to_node();
    user_agent.kind = "agent".into();
    graph.insert_node(user_agent).unwrap();
    for field in umx_core::ontology::PROVENANCE_KEYS {
        graph.set_vector(
            &concepts_key().to_string(),
            approval,
            Some(field),
            &[127, 0, 0, 0, 0, 0, 0, 0],
            umx_core::vectors::TextHash::of("cat"),
        );
    }
    let cats = umx_services::vector_candidates(
        &graph,
        &concepts_key().to_string(),
        &[127, 0, 0, 0, 0, 0, 0, 0],
        None,
        0.9,
    );
    assert!(cats.is_empty(), "{cats:?}");
    let dogs = umx_services::vector_candidates(
        &graph,
        &concepts_key().to_string(),
        &[0, 127, 0, 0, 0, 0, 0, 0],
        None,
        0.9,
    );
    assert_eq!(dogs.len(), 1);
    assert_eq!(dogs[0].0, dog);
    assert!(
        graph
            .vector(
                &concepts_key().to_string(),
                graph.node_ix(cat).unwrap(),
                None
            )
            .is_some()
    );
}

/// req-30 done_when: an id is found by exact match, a paraphrase by vector, and each result names its match.
#[test]
fn each_hit_names_its_match_an_id_exactly_and_a_paraphrase_by_vector() {
    let (w, [cat, approval, dog, words]) = demo();

    let (by_id, _) = search(&w, None, &approval.to_text().to_lowercase());
    assert_eq!(by_id.hits[0].node.id, approval);
    assert_eq!(by_id.hits[0].exact.as_deref(), Some("id"));

    // "feline" shares no word with "Cat food shortage": only the vector leg finds it.
    let (para, warnings) = search(&w, None, "feline");
    assert!(warnings.is_empty(), "{warnings:?}");
    let first = &para.hits[0];
    assert_eq!((first.node.id, first.legs()), (cat, "vector"));
    let v = first.vector.as_ref().unwrap();
    assert_eq!(v.rank, 1);
    assert!(v.cosine >= 0.5, "{}", v.cosine);
    assert!(first.word.is_none());
    assert_eq!(
        para.floors,
        vec![(
            concepts_key(),
            Floor {
                value: 0.5,
                source: FloorSource::Config,
                run: None
            }
        )]
    );

    // Words and meaning both: the word leg names its field and BM25, the vector leg its field and cosine.
    let (both, _) = search(&w, None, "approval delay");
    let hit = both.hits.iter().find(|h| h.node.id == words).unwrap();
    assert_eq!(hit.legs(), "both");
    assert_eq!(hit.word.as_ref().unwrap().field, "note");
    assert!(both.hits.iter().any(|h| h.node.id == approval));

    // An exact title is rank 1, whatever the legs say.
    let (title, _) = search(&w, None, "dog WALKING");
    assert_eq!(title.hits[0].node.id, dog);
    assert_eq!(title.hits[0].exact.as_deref(), Some("title"));
}

/// The floor: a row below it never takes part (0.50 here, from the machine config). With no config floor an
/// uncalibrated model has none, so the same row takes part (ws004 s26).
#[test]
fn rows_below_the_floor_are_not_vector_hits() {
    let (mut w, [cat, ..]) = demo();
    // "cat dog approval delay" is 0.5 from each of four axes; "Cat food shortage" is cat + food: cosine 0.35.
    let (r, _) = search(&w, None, "cat dog approval delay");
    let cat_hit = r.hits.iter().find(|h| h.node.id == cat).unwrap();
    assert!(cat_hit.vector.is_none(), "{cat_hit:?}");
    assert_eq!(cat_hit.legs(), "word");

    w.svc.set_floors(Floors::default());
    let (r, _) = search(&w, None, "cat dog approval delay");
    assert_eq!(r.floors, vec![(concepts_key(), Floor::NONE)]);
    let cat_hit = r.hits.iter().find(|h| h.node.id == cat).unwrap();
    assert_eq!(cat_hit.legs(), "both", "{cat_hit:?}");
}

/// req-53: with the embedder offline, word matches come back marked `embedder_unavailable`, never "nothing similar".
#[test]
fn an_offline_embedder_gives_word_matches_marked_unavailable() {
    let (w, [_, approval, ..]) = demo();
    w.embedder.offline.store(true, Ordering::SeqCst);
    let (r, warnings) = search(&w, None, "approval");
    assert_eq!(warnings, vec!["similarity_unavailable"]);
    assert_eq!(r.hits[0].node.id, approval);
    assert!(r.hits.iter().all(|h| h.vector.is_none()));
}

/// A query with no direction (no word the model knows) is word-only and marked, not an empty vector result.
#[test]
fn a_query_with_no_direction_is_word_only_and_marked() {
    let (w, _) = demo();
    let (r, warnings) = search(&w, None, "budget");
    assert_eq!(warnings, vec!["similarity_unavailable"]);
    assert_eq!(r.hits[0].exact.as_deref(), Some("title"));
    assert!(r.hits.iter().all(|h| h.vector.is_none()));
}

/// Security's gate (architect s11): a secured graph's query never goes to an embedder that is not local. A named
/// secured graph is searched by words, warned, and the embedder is not called at all when no graph may use it.
#[test]
fn a_secured_graph_never_sends_its_query_to_a_hosted_embedder() {
    // Embedded by a local embedder (into the private cache), so the secured graph has vectors to search.
    let mut w = world_with(Concepts::new(Locality::Local));
    graph(&mut w, "vault", Some(section(None, Policy::SECURED)));
    let id = add(&mut w, "vault", "Approval delay", "");
    run(&mut w);
    // Restart with a hosted embedder that fails the test if called.
    let mut hosted = Concepts::new(Locality::Hosted);
    hosted.forbidden = true;
    let mut w = world_on(w.files.clone(), hosted);
    w.svc.graph_load(&ctx(), "/work/vault", None).unwrap();
    let space = concepts_key().to_string();
    let g = w.svc.loaded_graph("vault").unwrap();
    assert!(
        g.whole_block(&space).is_some_and(|b| b.count() == 1),
        "the vault has vectors"
    );
    let (r, warnings) = search(&w, Some("vault"), "approval");
    assert_eq!(r.hits[0].node.id, id);
    assert!(r.hits.iter().all(|h| h.vector.is_none()));
    // One warning says why and how to fix it, not two (s32).
    assert_eq!(warnings, ["similarity_no_embedder"]);
    assert_eq!(w.embedder.calls(), 0);
}

/// An open graph and a named secured one with a hosted embedder: the query is embedded once, for the open graph,
/// and only the open graph's nodes are vector hits.
#[test]
fn with_a_hosted_embedder_only_open_graphs_get_a_vector_leg() {
    let mut w = world_with(Concepts::new(Locality::Hosted));
    graph(&mut w, "demo", None);
    graph(&mut w, "vault", Some(section(None, Policy::SECURED)));
    add(&mut w, "demo", "Cat food shortage", "");
    add(&mut w, "vault", "Kitten care", "");
    run(&mut w);
    let calls = w.embedder.calls();
    let (r, warnings) = search(&w, Some("demo,vault"), "feline");
    assert_eq!(w.embedder.calls(), calls + 1);
    assert_eq!(warnings, ["similarity_no_embedder"]);
    assert!(r.hits.iter().all(|h| h.alias == "demo"), "{:?}", r.hits);
}

/// req-37: a graph recording another model is named (embedder_mismatch, both keys) and searched by words only;
/// vectors are compared only within one space.
#[test]
fn a_graph_in_another_models_space_is_named_and_searched_by_words_only() {
    let (mut w, [cat, ..]) = demo();
    let other = ModelKey::new("other", 8).unwrap();
    graph(
        &mut w,
        "elsewhere",
        Some(section(Some(other), Policy::OPEN)),
    );
    let far = add(&mut w, "elsewhere", "Cat food shortage", "");
    let (r, warnings) = search(&w, Some("demo,elsewhere"), "feline cat");
    assert!(
        warnings.contains(&"embedder_mismatch".to_string()),
        "{warnings:?}"
    );
    let hit = |id: Id| r.hits.iter().find(|h| h.node.id == id).unwrap();
    assert!(hit(cat).vector.is_some());
    assert!(hit(far).vector.is_none());
    assert_eq!(hit(far).legs(), "word");
}

/// Architect: a graph whose space changes between plan and finish (a reembed landed) loses its vector leg with a
/// warning; a query from one space is never scored against rows of another.
#[test]
fn a_model_change_between_plan_and_finish_drops_that_vector_leg() {
    let (mut w, _) = demo();
    let plan = w.svc.search_plan(&ctx(), "feline", None, 10).unwrap();
    let queries = plan.embed();
    // graph.json now records another model; reload it, as a pull would.
    let dir = GraphDir::parse("/work/demo").unwrap();
    let bytes = w
        .files
        .read(&dir, umx_core::codec::GraphFile::Graph)
        .unwrap()
        .unwrap();
    let mut meta: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    meta["embedding"]["key"] = serde_json::json!("other@8");
    let bytes = serde_json::to_vec(&meta).unwrap();
    w.files
        .write_set(
            &dir,
            &[Change {
                file: FileRef::Graph(umx_core::codec::GraphFile::Graph),
                bytes: Some(&bytes),
            }],
        )
        .unwrap();
    w.svc.graph_load(&ctx(), "/work/demo", None).unwrap();
    let done = w.svc.search_finish(plan, queries).unwrap();
    assert!(done.value.hits.iter().all(|h| h.vector.is_none()));
    assert!(
        done.notes
            .iter()
            .any(|p| p.code == "similarity_unavailable"),
        "{:?}",
        done.notes
    );
}

/// Nodes whose vectors are not made yet are counted (similarity_incomplete); the rest are searched.
#[test]
fn pending_vectors_are_counted_as_similarity_incomplete() {
    let (mut w, [cat, ..]) = demo();
    add(&mut w, "demo", "Kitten care", "");
    add(&mut w, "demo", "Puppy care", "");
    let (r, warnings) = search(&w, None, "feline");
    assert_eq!(warnings, vec!["similarity_incomplete"]);
    assert_eq!(r.hits[0].node.id, cat);
    let done = w.svc.search(&ctx(), "feline", None, 10).unwrap();
    let message = format!("{:?}", done.notes[0]);
    assert!(message.contains('2'), "{message}");
}

/// Without an embedder every search is word-only and says so.
#[test]
fn no_embedder_is_word_only_and_marked() {
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
    let mut svc = Services::new(ports);
    svc.graph_new(&ctx(), "/work/demo", "demo", None, false, None)
        .unwrap();
    let done = svc.search(&ctx(), "anything", None, 10).unwrap();
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, vec!["similarity_unavailable"]);
}

/// Only a hosted instance, and embedder.active none: what a machine with no local instance has.
fn hosted_only_no_default() -> World {
    let mut hosted = Concepts::new(Locality::Hosted);
    hosted.forbidden = true;
    let mut w = world_with(hosted);
    let instance = umx_services::embedding::Instance {
        name: "az".into(),
        embedder: w.embedder.clone(),
    };
    w.svc.set_instances(vec![instance], None);
    w
}

/// s32 (field report): a secured graph recording no model, on a machine whose only instance is hosted and with no
/// default, was left out of the vector leg with no warning, so a search with no word match came back empty and
/// silent. It is searched by words and says why, with the owner's fix; the hosted instance is never asked.
#[test]
fn a_graph_no_instance_admits_is_searched_by_words_and_says_why() {
    let mut w = hosted_only_no_default();
    graph(&mut w, "vault", Some(section(None, Policy::SECURED)));
    let id = add(&mut w, "vault", "Approval delay", "");
    let (r, warnings) = search(&w, Some("vault"), "approval");
    assert_eq!(r.hits.iter().map(|h| h.node.id).collect::<Vec<_>>(), [id]);
    assert_eq!(warnings, ["similarity_no_embedder"]);
    let (r, warnings) = search(&w, Some("vault"), "vendor onboarding");
    assert!(r.hits.is_empty());
    assert_eq!(
        warnings,
        ["similarity_no_embedder"],
        "never an empty, silent reply"
    );
    let done = w
        .svc
        .search(&ctx().graph("vault"), "vendor", None, 10)
        .unwrap();
    let shown = format!("{:?}", done.notes[0]);
    assert!(
        shown.contains("vault") && shown.contains("local"),
        "{shown}"
    );
    assert!(
        shown.contains("graph set vault --embedder trusted"),
        "{shown}"
    );
    // --all with named graphs opted in reads it the same way; without, it is only counted (d-20).
    let all = |include_named| {
        let c = ctx().scope(umx_services::Scope::All { include_named });
        let done = w.svc.search(&c, "approval", None, 10).unwrap();
        let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
        (done.value.hits.len(), codes, format!("{:?}", done.notes))
    };
    let (hits, codes, _) = all(true);
    assert_eq!(hits, 1);
    assert_eq!(codes, ["secured_graphs_read", "similarity_no_embedder"]);
    let (hits, codes, shown) = all(false);
    assert_eq!(hits, 0);
    assert_eq!(codes, ["secured_graphs_skipped"]);
    assert!(!shown.contains("vault"), "{shown}");
    assert_eq!(w.embedder.calls(), 0);
}

/// Embeddings note 1: an open-visibility graph whose embedder setting is `local` is read by searches that
/// name no graph, plain `--all` included. On a machine with only hosted instances, each such search says why it used
/// words only. The graph's hits name it already, so the warning may too (d-20 hides only graphs an unnamed read skips).
#[test]
fn an_unnamed_search_over_an_open_local_graph_is_never_silent() {
    let mut w = hosted_only_no_default();
    let open_local = Policy {
        visibility: umx_core::Visibility::Open,
        embedder: umx_core::EmbedderRule::Local,
    };
    graph(&mut w, "notes", Some(section(None, open_local)));
    add(&mut w, "notes", "Approval delay", "");
    for c in [
        ctx(),
        ctx().scope(umx_services::Scope::All {
            include_named: false,
        }),
    ] {
        for (text, hits) in [("approval", 1), ("vendor onboarding", 0)] {
            let done = w.svc.search(&c, text, None, 10).unwrap();
            assert_eq!(done.value.hits.len(), hits, "{text}");
            let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
            assert_eq!(codes, ["similarity_no_embedder"], "{text}");
        }
    }
    assert_eq!(w.embedder.calls(), 0);
}

/// An open graph recording no model, with no default to embed it, has no vector leg and says why.
#[test]
fn an_open_graph_with_no_model_and_no_default_says_why() {
    let mut w = hosted_only_no_default();
    graph(&mut w, "demo", None);
    add(&mut w, "demo", "Approval queue", "");
    let done = w.svc.search(&ctx(), "vendor", None, 10).unwrap();
    assert!(done.value.hits.is_empty());
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, ["similarity_unavailable"]);
    assert!(
        format!("{:?}", done.notes[0]).contains("no default embedder"),
        "{:?}",
        done.notes
    );
    assert_eq!(w.embedder.calls(), 0);
}

#[test]
fn all_graph_scope_keeps_one_primary_and_cannot_be_saved() {
    use umx_core::codec::GraphFile;
    let (mut w, _) = demo();
    let from = GraphDir::parse("/work/demo").unwrap();
    let copy = GraphDir::parse("/work/copy").unwrap();
    for file in [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges] {
        let bytes = w.files.read(&from, file).unwrap().unwrap();
        w.files.write(&copy, file, &bytes).unwrap();
    }
    w.svc
        .graph_load(&ctx(), "/work/copy", Some("copy"))
        .unwrap();
    for include_named in [false, true] {
        let c = ctx().scope(umx_services::Scope::All { include_named });
        let found = w.svc.search(&c, "dog walking", None, 10).unwrap();
        assert!(found.value.hits.iter().any(|h| h.alias == "demo"));
        assert!(found.value.hits.iter().all(|h| h.alias != "copy"));
        assert_eq!(
            w.svc.scope_save(&c, "all_saved", false).unwrap_err().code,
            "usage_invalid"
        );
    }
    w.svc
        .graph_set(&ctx(), "demo", &[("policy".into(), "secured".into())])
        .unwrap();
    let c = ctx().scope(umx_services::Scope::All {
        include_named: true,
    });
    let search = w.svc.search(&c, "dog walking", None, 10).unwrap();
    let query = w
        .svc
        .query(
            &c,
            &umx_services::CypherRequest {
                text: "MATCH (n) RETURN n.graph AS graph".into(),
                limit: None,
                offset: None,
                params: Vec::new(),
                along: None,
            },
        )
        .unwrap();
    for warnings in [&search.notes, &query.notes] {
        let read = warnings
            .iter()
            .find(|p| p.code == "secured_graphs_read")
            .unwrap();
        assert_eq!(read.candidates.len(), 1);
        assert_eq!(read.candidates[0].id, "demo");
        let skipped = warnings
            .iter()
            .find(|p| p.code == "secured_graphs_skipped")
            .unwrap();
        assert_eq!(skipped.slot("n").as_deref(), Some("1"));
        assert!(!format!("{warnings:?}").contains("copy"));
    }
    w.svc
        .scope_save(&ctx().graph("demo"), "only_demo", false)
        .unwrap();
    graph(
        &mut w,
        "other",
        Some(section(Some(concepts_key()), Policy::SECURED)),
    );
    add(&mut w, "other", "Dog walking", "");
    let saved = w.svc.scope_show("only_demo").unwrap();
    assert_eq!(saved.value.members.len(), 1);
    let found = w
        .svc
        .search(
            &ctx().scope(umx_services::Scope::Saved("only_demo".into())),
            "dog walking",
            None,
            10,
        )
        .unwrap();
    assert!(found.value.hits.iter().all(|h| h.alias == "demo"));
}

#[test]
fn all_graph_search_embeds_once_for_five_graphs_on_one_instance() {
    let mut w = world_with(Concepts::new(Locality::Hosted));
    for i in 0..5 {
        let alias = format!("g{i}");
        graph(
            &mut w,
            &alias,
            Some(section(Some(concepts_key()), Policy::OPEN)),
        );
        add(&mut w, &alias, "Cat care", "");
    }
    run(&mut w);
    let before = w.embedder.calls();
    let found = w
        .svc
        .search(
            &ctx().scope(umx_services::Scope::All {
                include_named: false,
            }),
            "feline",
            None,
            10,
        )
        .unwrap();
    assert_eq!(found.value.hits.len(), 5);
    assert_eq!(w.embedder.calls() - before, 1);
}

#[test]
fn all_graph_search_never_asks_a_hosted_instance_for_a_local_only_graph() {
    let mut embedder = Concepts::new(Locality::Hosted);
    embedder.forbidden = true;
    let mut w = world_with(embedder);
    graph(
        &mut w,
        "vault",
        Some(section(Some(concepts_key()), Policy::SECURED)),
    );
    add(&mut w, "vault", "Dog walking", "");
    let found = w
        .svc
        .search(
            &ctx().scope(umx_services::Scope::All {
                include_named: true,
            }),
            "dog walking",
            None,
            10,
        )
        .unwrap();
    assert_eq!(w.embedder.calls(), 0);
    assert_eq!(found.value.hits[0].alias, "vault");
    assert!(found.value.hits[0].vector.is_none());
    assert!(
        found
            .notes
            .iter()
            .any(|p| p.code == "similarity_no_embedder")
    );
    assert!(found.notes.iter().any(|p| p.code == "secured_graphs_read"));
}

#[test]
fn all_graph_search_keeps_visibility_rechecks_and_drops_unloaded_graphs() {
    for include_named in [false, true] {
        let (mut w, [_, _, dog, _]) = demo();
        let c = ctx()
            .scope(umx_services::Scope::All { include_named })
            .default_graph("not_the_scope");
        let plan = w.svc.search_plan(&c, "dog walking", None, 10).unwrap();
        let queries = plan.embed();
        w.svc
            .graph_set(&ctx(), "demo", &[("policy".into(), "secured".into())])
            .unwrap();
        let done = w.svc.search_finish(plan, queries).unwrap();
        if include_named {
            assert!(done.value.hits.iter().any(|h| h.node.id == dog));
            let warning = done
                .notes
                .iter()
                .find(|p| p.code == "secured_graphs_read")
                .unwrap();
            assert_eq!(warning.candidates[0].id, "demo");
        } else {
            assert!(done.value.hits.is_empty());
            assert_eq!(
                done.notes.iter().map(|p| p.code).collect::<Vec<_>>(),
                ["secured_graphs_skipped"]
            );
        }
        let (mut w, _) = demo();
        let plan = w.svc.search_plan(&c, "dog walking", None, 10).unwrap();
        let queries = plan.embed();
        w.svc.graph_unload(&ctx(), "demo").unwrap();
        let done = w.svc.search_finish(plan, queries).unwrap();
        assert!(done.value.hits.is_empty());
        assert!(done.notes.iter().all(|p| p.code != "secured_graphs_read"));
    }
}

#[test]
fn all_graph_search_without_opt_in_matches_the_unnamed_scope() {
    let (mut w, [_, _, dog, _]) = demo();
    graph(
        &mut w,
        "vault",
        Some(section(Some(concepts_key()), Policy::SECURED)),
    );
    add(&mut w, "vault", "Private dog walking", "");
    let plain = w.svc.search(&ctx(), "dog walking", None, 10).unwrap();
    let all = w
        .svc
        .search(
            &ctx()
                .scope(umx_services::Scope::All {
                    include_named: false,
                })
                .default_graph("vault"),
            "dog walking",
            None,
            10,
        )
        .unwrap();
    assert_eq!(plain, all);
    assert!(
        all.value
            .hits
            .iter()
            .any(|h| h.node.id == dog && h.alias == "demo")
    );
    assert!(all.value.hits.iter().all(|h| h.alias != "vault"));
    let skipped = all
        .notes
        .iter()
        .find(|p| p.code == "secured_graphs_skipped")
        .unwrap();
    assert_eq!(skipped.slot("n").as_deref(), Some("1"));
    assert!(skipped.candidates.is_empty());
}

/// R99-1 (reviewer, security; d-20): an unnamed search whose graph is secured while the query is being embedded
/// (no lock held) returns nothing of it, and counts it in secured_graphs_skipped. Named searches are unchanged.
#[test]
fn a_graph_secured_while_the_query_embeds_leaves_an_unnamed_search() {
    let (mut w, [_, _, dog, _]) = demo();
    let (before, _) = search(&w, None, "dog walking");
    assert_eq!(before.hits[0].node.id, dog);
    let plan = w.svc.search_plan(&ctx(), "dog walking", None, 10).unwrap();
    let queries = plan.embed();
    w.svc
        .graph_set(&ctx(), "demo", &[("policy".into(), "secured".into())])
        .unwrap();
    assert_eq!(w.svc.effective_policy("demo"), Some(Policy::SECURED));
    let done = w.svc.search_finish(plan, queries).unwrap();
    assert!(done.value.hits.is_empty(), "{:?}", done.value.hits);
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, vec!["secured_graphs_skipped"]);
    // Named, the secured graph is searched.
    let (named, _) = search(&w, Some("demo"), "dog walking");
    assert_eq!(named.hits[0].node.id, dog);
}

/// R99-2: a graph searched by words alone keeps every word hit at full credit, even when another graph in the same
/// search has a vector leg and the query has no informative term.
#[test]
fn a_word_only_graph_keeps_its_word_hits_beside_a_graph_with_vectors() {
    let (mut w, _) = demo();
    let other = ModelKey::new("other", 8).unwrap();
    graph(
        &mut w,
        "elsewhere",
        Some(section(Some(other), Policy::OPEN)),
    );
    let far = add(&mut w, "elsewhere", "Approval elsewhere", "");
    let (alone, _) = search(&w, Some("elsewhere"), "approval");
    assert_eq!(alone.hits[0].node.id, far);
    // "approval" is in 3 of the 5 nodes searched: not informative, so demo's word leg gives fusion nothing.
    let (both, warnings) = search(&w, Some("demo,elsewhere"), "approval");
    assert!(
        warnings.contains(&"embedder_mismatch".to_string()),
        "{warnings:?}"
    );
    let hit = both
        .hits
        .iter()
        .find(|h| h.node.id == far)
        .expect("the word-only graph's hit");
    let word = hit.word.as_ref().unwrap();
    assert_eq!((hit.legs(), word.coverage), ("word", 1.0));
    assert_eq!(both.word_leg, Some("no informative terms"));
}

/// R99-3: a node whose text changed keeps its stored (stale) vector searchable, and is counted as incomplete until
/// the fresh one lands.
#[test]
fn a_changed_node_with_a_stale_vector_is_counted_incomplete() {
    let (mut w, [cat, ..]) = demo();
    w.svc
        .node_edit(
            &ctx(),
            &cat.to_text(),
            umx_services::NodeEditRequest {
                note: Some("now also about kittens".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(w.svc.graph_list()[0].vectors_pending, Some(1));
    let (r, warnings) = search(&w, None, "feline");
    assert_eq!(r.hits[0].node.id, cat, "the stale vector still searches");
    assert_eq!(warnings, vec!["similarity_incomplete"]);
}

/// R99-4: built-in short columns (type, status, created, updated, created_by) match exactly, case ignored.
#[test]
fn built_in_columns_are_exact_match_columns() {
    let (w, ids) = demo();
    let (open, _) = search(&w, None, "OPEN");
    assert_eq!(open.hits.len(), 4);
    assert!(
        open.hits
            .iter()
            .all(|h| h.exact.as_deref() == Some("status"))
    );
    let order: Vec<Id> = open.hits.iter().map(|h| h.node.id).collect();
    let mut sorted = ids.to_vec();
    sorted.sort();
    assert_eq!(order, sorted, "exact hits in id order");
    let (kind, _) = search(&w, None, "problem");
    assert!(kind.hits.iter().all(|h| h.exact.as_deref() == Some("type")));
    let (by, _) = search(&w, None, "tester");
    assert_eq!(by.hits[0].exact.as_deref(), Some("created_by"));
}

/// Security (repo narrowing, folder rule): a graph whose folder's config names another embedder, or none,
/// gets no vector leg, and its query is never sent.
#[test]
fn a_folder_narrowed_to_another_embedder_never_sends_its_query() {
    let (w, [cat, ..]) = demo();
    let mut hosted = Concepts::new(Locality::Hosted);
    hosted.forbidden = true;
    let mut w = world_on(w.files.clone(), hosted);
    w.svc.graph_load(&ctx(), "/work/demo", None).unwrap();
    let plan = w
        .svc
        .search_plan_allowing(&ctx(), "cat", None, 10, &|_, _| false)
        .unwrap();
    let queries = plan.embed();
    let done = w.svc.search_finish(plan, queries).unwrap();
    assert_eq!(w.embedder.calls(), 0);
    assert_eq!(done.value.hits[0].node.id, cat);
    assert!(done.value.hits.iter().all(|h| h.vector.is_none()));
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, vec!["similarity_unavailable"]);
}

/// R99-1 leftover (reviewer round 2, security): a graph an unnamed search drops at finish is only counted; no
/// warning planned for it (here its embedder_mismatch, naming it and its model) is shown.
#[test]
fn a_graph_secured_during_the_embed_leaves_no_warning_that_names_it() {
    let (mut w, [_, _, dog, _]) = demo();
    let other = ModelKey::new("other", 8).unwrap();
    graph(
        &mut w,
        "elsewhere",
        Some(section(Some(other), Policy::OPEN)),
    );
    add(&mut w, "elsewhere", "Dog walking elsewhere", "");
    let plan = w.svc.search_plan(&ctx(), "dog walking", None, 10).unwrap();
    let queries = plan.embed();
    w.svc
        .graph_set(&ctx(), "elsewhere", &[("policy".into(), "secured".into())])
        .unwrap();
    let done = w.svc.search_finish(plan, queries).unwrap();
    assert!(done.value.hits.iter().all(|h| h.alias == "demo"));
    assert_eq!(done.value.hits[0].node.id, dog);
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, vec!["secured_graphs_skipped"]);
    assert!(!format!("{:?}", done.notes).contains("elsewhere"));
}

/// Security (repo-config rule): a graph whose repo config is refused or does not parse counts as naming no embedder, so
/// its query never goes to a hosted one. The daemon's folder rule is `repo_layer_ok` and the active name; this drives
/// the same check through the plan.
#[test]
fn a_broken_repo_config_sends_no_query() {
    use umx_core::ports::{ConfigFile, ConfigFiles};
    let (w, _) = demo();
    let mut hosted = Concepts::new(Locality::Hosted);
    hosted.forbidden = true;
    let mut w = world_on(w.files.clone(), hosted);
    w.svc.graph_load(&ctx(), "/work/demo", None).unwrap();
    let files = Arc::new(umx_fakes::FakeConfigFiles::new(
        GraphDir::parse("/home").unwrap(),
        GraphDir::parse("/").unwrap(),
    ));
    let repo = GraphDir::parse("/work/demo").unwrap();
    files.add_repo(repo.clone());
    let config = umx_services::ConfigServices::new(
        files.clone(),
        Arc::new(umx_fakes::FakeSecrets::new(Default::default())),
        Default::default(),
        GraphDir::parse("/home/secrets.env").unwrap(),
        umx_core::config::resolve(&Default::default()),
    );
    assert!(config.repo_layer_ok(&repo), "no repo file: fine");
    files
        .write(
            &ConfigFile::Repo(repo.clone()),
            b"embedder.active = [not toml",
        )
        .unwrap();
    assert!(!config.repo_layer_ok(&repo));
    let plan = w
        .svc
        .search_plan_allowing(&ctx(), "cat", None, 10, &|d, _| config.repo_layer_ok(d))
        .unwrap();
    let queries = plan.embed();
    let done = w.svc.search_finish(plan, queries).unwrap();
    assert_eq!(w.embedder.calls(), 0);
    assert!(done.value.hits.iter().all(|h| h.vector.is_none()));
}

/// s17 item 8: a search over two graphs on two instances embeds the query once per space, each with the instance its
/// graphs embed with. The secured graph's leg uses the local instance; the hosted one is asked only for the open
/// graph's space, and each graph gets vector hits.
#[test]
fn each_graph_vector_leg_uses_its_own_instance() {
    use umx_services::embedding::Instance;
    let key = |name: &str| ModelKey::new(name, 8).unwrap();
    let mut hosted = Concepts::new(Locality::Hosted);
    hosted.key = key("concepts-a");
    let mut local = Concepts::new(Locality::Local);
    local.key = key("concepts-b");
    let (hosted, local) = (Arc::new(hosted), Arc::new(local));
    let mut w = world_with(Concepts::new(Locality::Local));
    w.svc.set_instances(
        vec![
            Instance {
                name: "a".into(),
                embedder: hosted.clone(),
            },
            Instance {
                name: "b".into(),
                embedder: local.clone(),
            },
        ],
        Some("a"),
    );
    graph(
        &mut w,
        "open",
        Some(section(Some(key("concepts-a")), Policy::OPEN)),
    );
    graph(
        &mut w,
        "vault",
        Some(section(Some(key("concepts-b")), Policy::SECURED)),
    );
    let open_id = add(&mut w, "open", "Cat food shortage", "");
    let vault_id = add(&mut w, "vault", "Approval queue", "");
    let mut pacer = Pacer::default();
    loop {
        let jobs = w.svc.take_embed_jobs();
        if jobs.is_empty() {
            break;
        }
        for j in jobs {
            let Some(planned) = w.svc.embed_plan(&j).unwrap() else {
                continue;
            };
            let instance = w
                .svc
                .embedding()
                .embedder_for(w.svc.graph(), &j.alias)
                .unwrap();
            match step(&*instance.embedder, &[planned], &mut pacer, 0) {
                Step::Done { vectors, .. } => {
                    w.svc.embed_apply(&vectors).unwrap();
                }
                other => panic!("{other:?}"),
            }
        }
    }
    let (hosted_docs, local_docs) = (hosted.calls(), local.calls());
    assert!(hosted_docs > 0 && local_docs > 0);
    let (r, warnings) = search(&w, Some("open,vault"), "feline approval");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        hosted.calls(),
        hosted_docs + 1,
        "one query for the open graph's space"
    );
    assert_eq!(
        local.calls(),
        local_docs + 1,
        "one query for the vault's space, locally"
    );
    for id in [open_id, vault_id] {
        assert!(
            r.hits.iter().any(|h| h.node.id == id && h.vector.is_some()),
            "{id} is a vector hit"
        );
    }
}

/// Security F1 (d-21 with d-20): a copy an unnamed search left out is named in copies_skipped only if it is
/// still open when the search finishes; one secured meanwhile is not named.
#[test]
fn a_copy_secured_while_the_search_runs_is_not_named() {
    use umx_core::codec::GraphFile;
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
    let mut svc = Services::new(ports);
    svc.graph_new(&ctx(), "/work/demo", "demo", None, false, None)
        .unwrap();
    let mut c = ctx();
    c.graph = Some("demo".into());
    svc.node_add(
        &c,
        NodeAddRequest {
            kind: "problem".into(),
            title: "dog walking".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let (from, to) = (
        GraphDir::parse("/work/demo").unwrap(),
        GraphDir::parse("/work/demo_b").unwrap(),
    );
    for file in [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges] {
        let bytes = files.raw_for_tests(&from, file).unwrap_or_default();
        files.tamper_for_tests(&to, file, &bytes);
    }
    svc.graph_load(&ctx(), "/work/demo_b", Some("demo_b"))
        .unwrap();
    let codes = |w: &[umx_core::Problem]| w.iter().map(|p| p.code).collect::<Vec<_>>();
    let done = svc.search(&ctx(), "dog", None, 10).unwrap();
    assert!(
        codes(&done.notes).contains(&"copies_skipped"),
        "{:?}",
        done.notes
    );
    let plan = svc.search_plan(&ctx(), "dog", None, 10).unwrap();
    let queries = plan.embed();
    svc.graph_set(&ctx(), "demo_b", &[("policy".into(), "secured".into())])
        .unwrap();
    let done = svc.search_finish(plan, queries).unwrap();
    assert!(
        !codes(&done.notes).contains(&"copies_skipped"),
        "{:?}",
        done.notes
    );
    assert!(!format!("{:?}", done.notes).contains("demo_b"));
}

/// Reviewer R132-2: both copies of an unnamed search secured between plan and finish (securing one records the
/// graph id, so both are): nothing found, both loads counted in secured_graphs_skipped, neither named.
#[test]
fn copies_secured_while_the_search_runs_are_counted_not_named() {
    use umx_core::codec::GraphFile;
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
    let mut svc = Services::new(ports);
    svc.graph_new(&ctx(), "/work/demo", "demo", None, false, None)
        .unwrap();
    let mut c = ctx();
    c.graph = Some("demo".into());
    svc.node_add(
        &c,
        NodeAddRequest {
            kind: "problem".into(),
            title: "dog walking".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let (from, to) = (
        GraphDir::parse("/work/demo").unwrap(),
        GraphDir::parse("/work/demo_b").unwrap(),
    );
    for file in [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges] {
        let bytes = files.raw_for_tests(&from, file).unwrap_or_default();
        files.tamper_for_tests(&to, file, &bytes);
    }
    svc.graph_load(&ctx(), "/work/demo_b", Some("demo_b"))
        .unwrap();
    let plan = svc.search_plan(&ctx(), "dog", None, 10).unwrap();
    let queries = plan.embed();
    svc.graph_set(&ctx(), "demo", &[("policy".into(), "secured".into())])
        .unwrap();
    let done = svc.search_finish(plan, queries).unwrap();
    assert!(done.value.hits.is_empty());
    assert!(
        !format!("{:?}", done.notes).contains("demo"),
        "{:?}",
        done.notes
    );
    let secured = done
        .notes
        .iter()
        .find(|p| p.code == "secured_graphs_skipped")
        .expect("counted");
    assert_eq!(secured.slot("n").as_deref(), Some("2"), "{secured:?}");
}

/// ws004 s21 (req-92, architect): a capped prefix expansion is decided over every graph searched and reported, and
/// the fused scores it gives are the same to the bit whatever order the graphs come in. Each hit says how its words
/// matched.
#[test]
fn a_capped_prefix_is_reported_and_fuses_the_same_whatever_the_graph_order() {
    let word = |i: u8| {
        format!(
            "plank{}{}",
            (b'a' + i / 26) as char,
            (b'a' + i % 26) as char
        )
    };
    let mut w = world_with(Concepts::new(Locality::Local));
    graph(&mut w, "left", None);
    graph(&mut w, "right", None);
    for i in 0..70u8 {
        let alias = if i % 2 == 0 { "left" } else { "right" };
        // Titles of different lengths, so scores differ and ranks do not hang on ids alone.
        let filler = "fix ".repeat(usize::from(i % 5));
        add(&mut w, alias, &format!("{filler}{}", word(i)), "");
    }
    add(&mut w, "right", "Pre-approve listed vendors", "");
    // (alias, title, fused bits, word score bits, (query, term, form) per word)
    type Row = (
        String,
        String,
        u64,
        u64,
        Vec<(String, String, &'static str)>,
    );
    let rows = |graphs: &str, text: &str| {
        let mut c = ctx();
        c.graph = Some(graphs.into());
        let done = w.svc.search(&c, text, None, 100).unwrap();
        let warnings: Vec<String> = done.notes.iter().map(|p| p.code.to_string()).collect();
        let rows: Vec<Row> = done
            .value
            .hits
            .iter()
            .map(|h| {
                let m = h.word.as_ref().expect("a word hit");
                let terms = m
                    .terms
                    .iter()
                    .map(|t| (t.query.clone(), t.term.clone(), t.form))
                    .collect();
                (
                    h.alias.clone(),
                    h.node.title.clone(),
                    h.fused.to_bits(),
                    m.score.to_bits(),
                    terms,
                )
            })
            .collect();
        (rows, warnings)
    };
    let (a, warnings) = rows("left,right", "plank");
    assert!(
        warnings.contains(&"word_prefix_capped".to_string()),
        "{warnings:?}"
    );
    assert_eq!(a.len(), umx_graph::PREFIX_CAP);
    assert_eq!(a, rows("right,left", "plank").0);
    assert!(a.iter().all(|r| r.4[0].2 == "prefix"));
    // A tester's query: the plural matches by stem and says so.
    let (v, warnings) = rows("left,right", "vendor");
    assert!(!warnings.contains(&"word_prefix_capped".to_string()));
    assert_eq!(v.len(), 1);
    assert_eq!(
        v[0].4,
        vec![("vendor".to_string(), "vendors".to_string(), "stem")]
    );
}

/// R190-B2: floor_uncalibrated is decided per use from the floor each will use, so one override never hides the other
/// use: none set names both, a search floor alone leaves similar_to named, a similar floor alone leaves search, and
/// both (or a search floor with graph.json's own similar_to floor) say nothing. "concepts" is not calibrated.
#[test]
fn floor_uncalibrated_names_each_use_left_without_a_floor() {
    let (w, _) = demo();
    let files = w.files.clone();
    let load = |floors: Floors| {
        let mut w = world_on(files.clone(), Concepts::new(Locality::Local));
        w.svc.set_floors(floors);
        let done = w.svc.graph_load(&ctx(), "/work/demo", None).unwrap();
        done.notes
            .iter()
            .filter(|p| p.code == "floor_uncalibrated")
            .map(|p| p.slot("uses").unwrap())
            .collect::<Vec<_>>()
    };
    let with = |search: Option<f32>, similar: Option<f32>| {
        let mut f = Floors::default();
        f.set(&concepts_key(), search, similar);
        f
    };
    assert_eq!(load(Floors::default()), ["search and similar_to"]);
    assert_eq!(load(with(Some(0.5), None)), ["similar_to"]);
    assert_eq!(load(with(None, Some(0.5))), ["search"]);
    assert!(load(with(Some(0.5), Some(0.5))).is_empty());

    // graph.json's own similar_to floor counts for similar_to.
    let dir = GraphDir::parse("/work/demo").unwrap();
    let bytes = files
        .read(&dir, umx_core::codec::GraphFile::Graph)
        .unwrap()
        .unwrap();
    let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v["embedding"]["similarity"] = serde_json::json!({ "floor": 0.5 });
    files
        .write(
            &dir,
            umx_core::codec::GraphFile::Graph,
            v.to_string().as_bytes(),
        )
        .unwrap();
    assert!(load(with(Some(0.5), None)).is_empty());
    assert_eq!(load(Floors::default()), ["search"]);
}
