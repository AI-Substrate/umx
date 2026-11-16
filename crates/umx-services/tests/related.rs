//! Related nodes on write (PRD 001 req-123, D80, ws004 s28) over a concept embedder: the rows, what is never listed,
//! the off switch and its precedence (flag, graph, machine), and each reason the check is skipped with its hint.
//! Plumbing only: the floors here are set in the config for an uncalibrated test key, never tuned; the real-model
//! fixture is in related_recorded.rs.
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use umx_core::ports::{EmbedError, Embedded, Embedder, GraphDir, Locality, ModelKey, TextKind};
use umx_core::similarity::Floors;
use umx_core::{GraphEmbedding, Id, Policy, Problem};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::related::{Related, RelatedRequest};
use umx_services::{
    BatchLine, BatchOp, BatchRequest, Ctx, NodeAddRequest, Pacer, Planned, Ports, QueryFailed,
    Services, Step, batches, step,
};

/// Each known word adds to one axis, so a paraphrase lands where its meaning is though it shares no word.
struct Concepts {
    key: ModelKey,
    locality: Locality,
    calls: AtomicUsize,
    texts: AtomicUsize,
    offline: AtomicBool,
}

impl Concepts {
    fn new(key: ModelKey, locality: Locality) -> Concepts {
        Concepts {
            key,
            locality,
            calls: AtomicUsize::new(0),
            texts: AtomicUsize::new(0),
            offline: AtomicBool::new(false),
        }
    }
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
            "dog" | "dogs" | "canine" | "puppy" => 1,
            "approval" | "sign" | "off" => 2,
            "delay" | "slow" | "late" => 3,
            _ => continue,
        };
        v[axis] += 1.0;
    }
    v
}

impl Embedder for Concepts {
    fn embed(&self, texts: &[String], _: TextKind) -> Result<Embedded, EmbedError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.texts.fetch_add(texts.len(), Ordering::SeqCst);
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

/// No query prompt: the similar_to floor applies (embeddings-expert, s28).
fn plain_key() -> ModelKey {
    ModelKey::new("concepts", 8).unwrap()
}

/// A query prompt: the search floor applies.
fn prompt_key() -> ModelKey {
    ModelKey::new("concepts", 8).unwrap().with_prompt_version(1)
}

struct World {
    embedder: Arc<Concepts>,
    svc: Services,
}

fn ctx(alias: &str) -> Ctx {
    let mut c = Ctx::new("tester");
    c.graph = Some(alias.into());
    c
}

fn ports(files: &Arc<FakeGraphFiles>) -> Ports {
    Ports {
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
    }
}

/// A world whose embedder makes `key`; `floor` is set in the config for the use `key` takes (None: no floor).
fn world(key: ModelKey, locality: Locality, floor: Option<f32>) -> World {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let embedder = Arc::new(Concepts::new(key.clone(), locality));
    let mut svc = Services::new(ports(&files)).with_embedder(embedder.clone());
    let mut floors = Floors::default();
    match (floor, key.prompt_version()) {
        (Some(f), Some(_)) => floors.set(&key, Some(f), None),
        (Some(f), None) => floors.set(&key, None, Some(f)),
        (None, _) => {}
    }
    svc.set_floors(floors);
    World { embedder, svc }
}

fn graph(w: &mut World, policy: Policy) {
    w.svc
        .graph_new(
            &Ctx::new("tester"),
            "/work/g",
            "g",
            None,
            false,
            Some(GraphEmbedding::new(None, policy)),
        )
        .unwrap();
}

fn add(w: &mut World, title: &str, note: &str) -> Id {
    w.svc
        .node_add(
            &ctx("g"),
            NodeAddRequest {
                kind: "problem".into(),
                title: title.into(),
                note: (!note.is_empty()).then(|| note.into()),
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id
}

/// Embeds every queued node, as the daemon's worker does.
fn embed_all(w: &mut World) {
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

/// The demo graph: cat, dog and approval nodes, all embedded.
fn demo(key: ModelKey) -> (World, [Id; 3]) {
    let mut w = world(key, Locality::Local, Some(0.5));
    graph(&mut w, Policy::OPEN);
    let cat = add(&mut w, "Cat food", "");
    let dog = add(&mut w, "Dog walking", "");
    let approval = add(&mut w, "Approval queue", "sign off is slow");
    embed_all(&mut w);
    (w, [cat, dog, approval])
}

fn check(w: &World, added: &[Id], flag: Option<bool>, machine: bool) -> Related {
    w.svc.related(
        &RelatedRequest {
            alias: "g",
            added,
            flag,
            machine,
            verb: "node add",
        },
        &|_, _| true,
    )
}

fn calls(w: &World) -> usize {
    w.embedder.calls.load(Ordering::SeqCst)
}

/// The one hint's reason words and fix, failing unless there is exactly one `related_skipped` hint.
fn skipped(r: &Related) -> (String, String, String) {
    let [p]: &[Problem; 1] = r.hints.as_slice().try_into().expect("one hint");
    assert_eq!(p.code, "related_skipped");
    (
        p.slot("reason").unwrap(),
        p.slot("fix").unwrap(),
        p.slot("count").unwrap(),
    )
}

fn set(w: &mut World, value: &str) {
    w.svc
        .graph_set(
            &Ctx::new("tester"),
            "g",
            &[("related".to_string(), value.to_string())],
        )
        .unwrap();
}

#[test]
fn a_new_node_lists_the_existing_nodes_close_in_meaning_with_scores_and_the_floor() {
    let (mut w, [cat, ..]) = demo(plain_key());
    let kitten = add(&mut w, "Feline kitten", "");
    let before = calls(&w);
    let r = check(&w, &[kitten], None, true);
    assert!(r.hints.is_empty(), "{:?}", r.hints);
    let rows = &r.rows[&kitten];
    assert_eq!(rows.len(), 1, "only the cat node is close: {rows:?}");
    assert_eq!(rows[0].id, cat);
    assert_eq!(rows[0].title, "Cat food");
    assert!(rows[0].score > 0.99, "{}", rows[0].score);
    // No prompt: the similar_to floor, from the config here.
    let floor = r.floor.expect("a floor");
    assert_eq!(
        (floor.value, floor.source.word(), floor.use_word()),
        (0.5, "config", "similar_to")
    );
    assert_eq!(calls(&w), before + 1, "one embed call for the check");
}

#[test]
fn a_prompt_key_cuts_at_its_search_floor() {
    let (mut w, [cat, ..]) = demo(prompt_key());
    let kitten = add(&mut w, "Kitten", "");
    let r = check(&w, &[kitten], None, true);
    assert_eq!(r.rows[&kitten][0].id, cat);
    assert_eq!(r.floor.unwrap().use_word(), "search");
}

#[test]
fn the_node_itself_and_the_nodes_the_same_call_added_are_never_listed() {
    let (mut w, [cat, ..]) = demo(plain_key());
    let req = BatchRequest {
        lines: ["Kitten", "Feline"]
            .into_iter()
            .enumerate()
            .map(|(i, t)| BatchLine {
                line: i as u32 + 1,
                op: BatchOp::NodeAdd {
                    id: None,
                    created: None,
                    ref_name: None,
                    req: NodeAddRequest {
                        kind: "problem".into(),
                        title: t.into(),
                        ..Default::default()
                    },
                    parent_ref: None,
                },
            })
            .collect(),
        ..Default::default()
    };
    let done = w.svc.batch(&ctx("g"), req).unwrap().value;
    let added: Vec<Id> = done.rows.iter().filter_map(|r| r.id).collect();
    // Embed the batch's own nodes too: stored vectors of the same call still never count.
    embed_all(&mut w);
    let before = calls(&w);
    let r = check(&w, &added, None, true);
    for id in &added {
        let ids: Vec<Id> = r.rows[id].iter().map(|r| r.id).collect();
        assert_eq!(ids, [cat], "{id}");
    }
    assert_eq!(calls(&w), before + 1, "one embed request for both nodes");
}

#[test]
fn at_most_three_rows_best_first_and_ties_go_to_the_lower_id() {
    let mut w = world(plain_key(), Locality::Local, Some(0.5));
    graph(&mut w, Policy::OPEN);
    let ids: Vec<Id> = (0..5).map(|i| add(&mut w, "Cat", &"x".repeat(i))).collect();
    let near = add(&mut w, "Cat dog", "");
    embed_all(&mut w);
    let new = add(&mut w, "Cat", "");
    let r = check(&w, &[new], None, true);
    let got: Vec<Id> = r.rows[&new].iter().map(|r| r.id).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(got, sorted[..3], "five equal scores: the three lowest ids");
    assert!(!got.contains(&near));
}

#[test]
fn nothing_under_the_floor_is_listed() {
    let (mut w, _) = demo(plain_key());
    let other = add(&mut w, "Puppy", "");
    let r = check(&w, &[other], None, true);
    let dog_only: Vec<String> = r.rows[&other].iter().map(|r| r.title.clone()).collect();
    assert_eq!(dog_only, ["Dog walking"]);
    let unrelated = add(&mut w, "Late", "");
    let r = check(&w, &[unrelated], None, true);
    assert!(
        r.rows[&unrelated].iter().all(|row| row.score >= 0.5),
        "{:?}",
        r.rows
    );
}

#[test]
fn no_related_on_the_call_sends_nothing_and_says_how_to_turn_it_on() {
    let (mut w, _) = demo(plain_key());
    let before = calls(&w);
    let kitten = add(&mut w, "Kitten", "");
    let r = check(&w, &[kitten], Some(false), true);
    assert_eq!(calls(&w), before, "no embed call");
    assert!(r.rows.is_empty());
    let (reason, fix, count) = skipped(&r);
    assert_eq!(
        reason,
        "--no-related turned it off for this call; give --related on the next one"
    );
    assert_eq!(
        fix,
        "umx node add --type <type> --title <title> --graph g --related"
    );
    assert_eq!(count, "1");
}

#[test]
fn the_graph_setting_turns_it_off_and_the_flag_turns_it_back_on() {
    let (mut w, [cat, ..]) = demo(plain_key());
    set(&mut w, "off");
    let kitten = add(&mut w, "Kitten", "");
    let before = calls(&w);
    let r = check(&w, &[kitten], None, true);
    assert_eq!(calls(&w), before);
    let (reason, fix, _) = skipped(&r);
    assert_eq!(reason, "the graph's related setting is off");
    assert_eq!(fix, "umx graph set g --related on");
    let r = check(&w, &[kitten], Some(true), true);
    assert_eq!(r.rows[&kitten][0].id, cat);
}

#[test]
fn the_machine_setting_turns_it_off_and_the_graph_or_the_flag_turns_it_back_on() {
    let (mut w, [cat, ..]) = demo(plain_key());
    let kitten = add(&mut w, "Kitten", "");
    let before = calls(&w);
    let r = check(&w, &[kitten], None, false);
    assert_eq!(calls(&w), before);
    let (reason, fix, _) = skipped(&r);
    assert_eq!(reason, "this machine's related setting is off");
    assert_eq!(fix, "umx config set related on");
    assert_eq!(
        check(&w, &[kitten], Some(true), false).rows[&kitten][0].id,
        cat
    );
    set(&mut w, "on");
    assert_eq!(check(&w, &[kitten], None, false).rows[&kitten][0].id, cat);
}

#[test]
fn with_no_embedder_the_reply_says_the_check_was_skipped() {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let mut svc = Services::new(ports(&files));
    svc.graph_new(&Ctx::new("tester"), "/work/g", "g", None, false, None)
        .unwrap();
    let id = svc
        .node_add(
            &ctx("g"),
            NodeAddRequest {
                kind: "problem".into(),
                title: "Kitten".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id;
    let r = svc.related(
        &RelatedRequest {
            alias: "g",
            added: &[id],
            flag: None,
            machine: true,
            verb: "node add",
        },
        &|_, _| true,
    );
    let (reason, fix, _) = skipped(&r);
    assert_eq!(reason, "no embedder is configured");
    assert_eq!(fix, "umx embeddings status");
}

#[test]
fn with_no_floor_it_is_skipped_and_the_fix_names_the_floor_for_the_key() {
    for (key, leaf) in [
        (plain_key(), "similar_floor"),
        (prompt_key(), "search_floor"),
    ] {
        let mut w = world(key, Locality::Local, None);
        graph(&mut w, Policy::OPEN);
        add(&mut w, "Cat", "");
        embed_all(&mut w);
        let before = calls(&w);
        let kitten = add(&mut w, "Kitten", "");
        let r = check(&w, &[kitten], None, true);
        assert_eq!(calls(&w), before);
        let (reason, fix, _) = skipped(&r);
        assert_eq!(
            reason,
            "the embedder's model has no calibrated floor; set one in the config (umx config show lists the instances)"
        );
        // This instance has no name (one wired embedder), so the fix names the key with an `<instance>` placeholder;
        // a named instance's fix names it (the golden list, tests/fixtures/related/fixes.txt).
        assert_eq!(
            fix,
            format!("umx config set embedders.<instance>.{leaf} <cosine>")
        );
    }
}

#[test]
fn with_no_other_vectors_yet_it_is_skipped() {
    let mut w = world(plain_key(), Locality::Local, Some(0.5));
    graph(&mut w, Policy::OPEN);
    add(&mut w, "Cat", "");
    let kitten = add(&mut w, "Kitten", "");
    let r = check(&w, &[kitten], None, true);
    assert_eq!(skipped(&r).0, "no other node of the graph has a vector yet");
    assert_eq!(calls(&w), 0);
}

#[test]
fn a_secured_graph_never_sends_to_a_hosted_embedder() {
    let mut w = world(plain_key(), Locality::Hosted, Some(0.5));
    graph(&mut w, Policy::SECURED);
    let kitten = add(&mut w, "Kitten", "");
    let r = check(&w, &[kitten], None, true);
    assert_eq!(
        skipped(&r).0,
        "no configured embedder may embed this graph here"
    );
    assert_eq!(calls(&w), 0);
}

#[test]
fn a_text_that_looks_like_a_secret_never_goes_to_a_hosted_embedder() {
    let mut w = world(plain_key(), Locality::Hosted, Some(0.5));
    graph(&mut w, Policy::OPEN);
    add(&mut w, "Cat", "");
    embed_all(&mut w);
    let before = calls(&w);
    // Built at run time, so no secret-shaped literal sits in the source.
    let note = format!("pass{}=hunter2-correct-horse-battery-staple", "word");
    let secret = add(&mut w, "Kitten", &note);
    let r = check(&w, &[secret], None, true);
    assert_eq!(
        skipped(&r).0,
        "its text looks like it holds a secret and the embedder is hosted"
    );
    assert_eq!(calls(&w), before);
}

#[test]
fn past_the_cap_the_rest_are_counted_in_one_hint_and_one_request_is_sent() {
    let (mut w, _) = demo(plain_key());
    w.svc.set_related_caps(BTreeMap::from([(String::new(), 2)]));
    let added: Vec<Id> = ["Kitten", "Puppy", "Feline", "Canine"]
        .into_iter()
        .map(|t| add(&mut w, t, ""))
        .collect();
    let (calls_before, texts_before) = (calls(&w), w.embedder.texts.load(Ordering::SeqCst));
    let r = check(&w, &added, None, true);
    assert_eq!(calls(&w), calls_before + 1);
    assert_eq!(w.embedder.texts.load(Ordering::SeqCst), texts_before + 2);
    assert_eq!(r.rows.keys().copied().collect::<Vec<_>>(), {
        let mut first = added[..2].to_vec();
        first.sort();
        first
    });
    let (reason, fix, count) = skipped(&r);
    assert_eq!(
        reason,
        "a call checks only as many new nodes as the embedder embeds in about 2 s"
    );
    assert_eq!(fix, "umx search <title> --graph g");
    assert_eq!(count, "2");
}

#[test]
fn a_time_out_or_a_failed_embed_gives_its_hint_and_never_rows() {
    let (mut w, _) = demo(plain_key());
    let kitten = add(&mut w, "Kitten", "");
    let req = RelatedRequest {
        alias: "g",
        added: &[kitten],
        flag: None,
        machine: true,
        verb: "batch",
    };
    let plan = w.svc.related_plan(&req, &|_, _| true).ok().unwrap();
    let r = w.svc.related_finish(
        plan,
        Err(QueryFailed::TimedOut { warming: false }),
        &|_, _| true,
    );
    assert!(r.rows.is_empty() && r.floor.is_none());
    assert_eq!(skipped(&r).0, "the embedder did not answer in time");
    w.embedder.offline.store(true, Ordering::SeqCst);
    let r = check(&w, &[kitten], None, true);
    assert_eq!(skipped(&r).0, "the embedder failed");
}

#[test]
fn an_instance_no_longer_admitted_at_finish_scores_nothing() {
    let (mut w, _) = demo(plain_key());
    let kitten = add(&mut w, "Kitten", "");
    let req = RelatedRequest {
        alias: "g",
        added: &[kitten],
        flag: None,
        machine: true,
        verb: "node add",
    };
    let plan = w.svc.related_plan(&req, &|_, _| true).ok().unwrap();
    let vectors = plan.embed();
    // The folder's config narrowed to another instance between plan and finish.
    let r = w.svc.related_finish(plan, vectors, &|_, _| false);
    assert!(r.rows.is_empty());
    assert_eq!(
        skipped(&r).0,
        "no configured embedder may embed this graph here"
    );
}
