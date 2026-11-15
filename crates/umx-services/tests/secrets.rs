//! The secret hold (req-49, SEC-10, ws004 s08) over the fakes: a field whose text looks like a credential warns on
//! the write and never reaches a hosted embedder until it is redacted or allowed; local embedders get everything;
//! the allow covers one exact text and lives in its own state file.

use std::sync::{Arc, Mutex};
use umx_core::Id;
use umx_core::ports::{
    EmbedError, Embedded, Embedder, GraphDir, LocalState, Locality, ModelKey, TextKind,
};
use umx_core::value::SetValue;
use umx_fakes::{FakeClock, FakeEmbedder, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{
    Ctx, NodeAddRequest, NodeEditRequest, Planned, Ports, QueryFailed, Services, embed,
};

/// A fake API key, built at run time so no key-shaped literal sits in the source.
fn fake_key(seed: &str) -> String {
    let mut k = String::from("AKIA");
    while k.len() < 20 {
        k.push_str(seed);
    }
    k.truncate(20);
    k
}

/// A hosted (or local) embedder that remembers every text it was sent.
struct Spy {
    inner: FakeEmbedder,
    seen: Mutex<Vec<String>>,
}

impl Spy {
    fn new(locality: Locality) -> Arc<Spy> {
        Arc::new(Spy {
            inner: FakeEmbedder::new(ModelKey::new("fake", 8).unwrap()).with_locality(locality),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn saw(&self, needle: &str) -> bool {
        self.seen.lock().unwrap().iter().any(|t| t.contains(needle))
    }

    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

impl Embedder for Spy {
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
        self.seen.lock().unwrap().extend(texts.iter().cloned());
        self.inner.embed(texts, kind)
    }
    fn key(&self) -> ModelKey {
        self.inner.key()
    }
    fn concurrency_ceiling(&self) -> usize {
        4
    }
    fn max_input_tokens(&self) -> usize {
        512
    }
    fn locality(&self) -> Locality {
        self.inner.locality()
    }
    fn ready(&self) -> Result<(), EmbedError> {
        Ok(())
    }
    fn credential(&self) -> Option<umx_core::ports::Credential> {
        self.inner.credential()
    }
    fn adapter_id(&self) -> &'static str {
        "fake.embedder"
    }
}

struct World {
    files: Arc<FakeGraphFiles>,
    allows: Arc<FakeLocalState>,
    spy: Arc<Spy>,
    svc: Services,
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

fn services(w: &World) -> Services {
    Services::new(Ports {
        sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
            umx_core::ports::GraphDir::parse("/").unwrap(),
        )),
        files: w.files.clone(),
        vector_cache: w.files.clone(),
        vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        allows: w.allows.clone(),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    })
    .with_embedder(w.spy.clone())
}

fn world(locality: Locality) -> World {
    let mut w = World {
        files: Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap())),
        allows: Arc::new(FakeLocalState::new()),
        spy: Spy::new(locality),
        svc: Services::new(Ports {
            sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
                umx_core::ports::GraphDir::parse("/").unwrap(),
            )),
            files: Arc::new(FakeGraphFiles::new(GraphDir::parse("/x").unwrap())),
            vector_cache: Arc::new(FakeGraphFiles::new(GraphDir::parse("/y").unwrap())),
            vector_cache_root: GraphDir::parse("/y").unwrap(),
            clock: Arc::new(FakeClock::default()),
            ids: Arc::new(FakeIds::new()),
            state: Arc::new(FakeLocalState::new()),
            scopes: Arc::new(FakeLocalState::new()),
            registry: Arc::new(FakeLocalState::new()),
            allows: Arc::new(FakeLocalState::new()),
            lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
        }),
    };
    w.svc = services(&w);
    w.svc
        .graph_new(&ctx(), "/work/g", "demo", None, false, None)
        .unwrap();
    w.svc
        .ontology_add_field(&ctx(), "problem", "impact:string", None)
        .unwrap();
    w
}

/// Adds a node; returns its id and the write's warning codes.
fn add(w: &mut World, title: &str, note: &str, impact: &str) -> (Id, Vec<String>) {
    let done = w
        .svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "problem".into(),
                title: title.into(),
                note: Some(note.into()),
                sets: vec![("impact".into(), SetValue::Str(impact.into()))],
                ..Default::default()
            },
        )
        .unwrap();
    let codes = done.notes.iter().map(|p| p.code.to_string()).collect();
    (done.value.node.id, codes)
}

/// The worker's path: plan, the send check right before sending, embed, apply.
fn run(w: &mut World) {
    loop {
        let jobs = w.svc.take_embed_jobs();
        if jobs.is_empty() {
            return;
        }
        let planned: Vec<Planned> = jobs
            .iter()
            .filter_map(|j| w.svc.embed_plan(j).unwrap())
            .collect();
        let embedder: Arc<dyn Embedder> = w.spy.clone();
        let batch = w
            .svc
            .embedding()
            .still_sendable(w.svc.graph(), &embedder, planned);
        let (vectors, _) = embed(&*embedder, &batch).unwrap();
        w.svc.embed_apply(&vectors).unwrap();
    }
}

#[test]
fn a_fake_api_key_warns_and_never_reaches_a_hosted_embedder_but_the_rest_does() {
    let mut w = world(Locality::Hosted);
    let key = fake_key("Q7Z3X9K2");
    let (id, codes) = add(
        &mut w,
        "Billing export fails",
        &format!("the export uses {key} to reach the bucket"),
        "finance blocked",
    );
    assert_eq!(codes, ["secret_suspected"]);
    run(&mut w);
    assert!(
        !w.spy.saw(&key),
        "the key never reached the hosted embedder"
    );
    // The note's own vector and the whole-node vector (which holds the note) are held; the clean fields went.
    assert!(w.spy.saw("Billing export fails"));
    assert!(w.spy.saw("finance blocked"));
    assert!(
        !w.spy.saw("to reach the bucket"),
        "no text holding the note was sent"
    );
    // Still pending: the held vectors do not exist.
    let pending = w
        .svc
        .graph()
        .loaded("demo")
        .unwrap()
        .vectors_pending
        .clone();
    assert!(pending.unwrap().contains(&id));
}

#[test]
fn a_local_embedder_gets_every_text() {
    let mut w = world(Locality::Local);
    let key = fake_key("Q7Z3X9K2");
    let (_, codes) = add(&mut w, "Billing", &format!("uses {key}"), "x");
    assert_eq!(codes, ["secret_suspected"], "the warning is the same");
    run(&mut w);
    assert!(w.spy.saw(&key));
}

#[test]
fn the_warning_names_node_field_and_rule_and_never_the_text() {
    let mut w = world(Locality::Hosted);
    let key = fake_key("Q7Z3X9K2");
    let done = w
        .svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "problem".into(),
                title: "t".into(),
                sets: vec![("impact".into(), SetValue::Str(format!("key {key}")))],
                ..Default::default()
            },
        )
        .unwrap();
    let p = &done.notes[0];
    let shown = format!("{p:?}");
    assert!(
        shown.contains("aws_access_key") && shown.contains("impact"),
        "{shown}"
    );
    assert!(
        !shown.contains(&key) && !shown.contains(&key[..8]),
        "{shown}"
    );
}

#[test]
fn an_allow_sends_that_exact_text_and_a_change_holds_it_again() {
    let mut w = world(Locality::Hosted);
    let key = fake_key("Q7Z3X9K2");
    let (id, _) = add(&mut w, "Billing", &format!("uses {key}"), "x");
    run(&mut w);
    assert!(!w.spy.saw(&key));
    let node = id.to_text();
    // A field that holds no secret: nothing to allow.
    let none = w.svc.embeddings_allow(&ctx(), &node, "impact").unwrap();
    assert!(!none.value.allowed);
    let done = w.svc.embeddings_allow(&ctx(), &node, "note").unwrap();
    assert!(done.value.allowed);
    assert_eq!(done.value.rule, Some("aws_access_key"));
    run(&mut w);
    assert!(
        w.spy.saw(&key),
        "allowed: it went (the instrument can see a positive)"
    );
    // A new key in the same field is a new text: held again, and warned again.
    let other = fake_key("M4N8P1R6");
    let edit = w
        .svc
        .node_edit(
            &ctx(),
            &node,
            NodeEditRequest {
                note: Some(format!("uses {other}")),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(edit.notes.iter().any(|p| p.code == "secret_suspected"));
    run(&mut w);
    assert!(!w.spy.saw(&other));
}

#[test]
fn allows_and_holds_survive_a_restart() {
    let mut w = world(Locality::Hosted);
    let (k1, k2) = (fake_key("Q7Z3X9K2"), fake_key("M4N8P1R6"));
    let (a, _) = add(&mut w, "One", &format!("uses {k1}"), "x");
    add(&mut w, "Two", &format!("uses {k2}"), "y");
    w.svc
        .embeddings_allow(&ctx(), &a.to_text(), "note")
        .unwrap();
    // A restart: new services over the same files and the same allows file; the graph loads again.
    w.spy = Spy::new(Locality::Hosted);
    w.svc = services(&w);
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    run(&mut w);
    assert!(w.spy.saw(&k1), "the allow was kept");
    assert!(!w.spy.saw(&k2), "the hold was not");
}

#[test]
fn a_text_edited_between_plan_and_send_is_judged_on_what_is_sent() {
    let mut w = world(Locality::Hosted);
    let key = fake_key("Q7Z3X9K2");
    let (id, _) = add(&mut w, "Billing", &format!("uses {key}"), "x");
    let jobs = w.svc.take_embed_jobs();
    let planned: Vec<Planned> = jobs
        .iter()
        .filter_map(|j| w.svc.embed_plan(j).unwrap())
        .collect();
    // Redacted after the plan: the planned texts still hold the key, so they are still held.
    w.svc
        .node_edit(
            &ctx(),
            &id.to_text(),
            NodeEditRequest {
                note: Some("uses the vault".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let embedder: Arc<dyn Embedder> = w.spy.clone();
    let batch = w
        .svc
        .embedding()
        .still_sendable(w.svc.graph(), &embedder, planned);
    assert!(
        batch
            .iter()
            .flat_map(|p| &p.texts)
            .all(|t| !t.text.contains(&key))
    );
}

#[test]
fn an_unreadable_allows_file_holds_everything() {
    let mut w = world(Locality::Hosted);
    let key = fake_key("Q7Z3X9K2");
    let (id, _) = add(&mut w, "Billing", &format!("uses {key}"), "x");
    w.svc
        .embeddings_allow(&ctx(), &id.to_text(), "note")
        .unwrap();
    w.allows.write(b"not json").unwrap();
    // Re-queue the node (an edit elsewhere on it) and run.
    w.svc
        .node_edit(
            &ctx(),
            &id.to_text(),
            NodeEditRequest {
                title: Some("Billing again".into()),
                ..Default::default()
            },
        )
        .unwrap();
    run(&mut w);
    assert!(!w.spy.saw(&key));
    // And a new allow is refused rather than overwriting a file umx cannot read.
    let refused = w
        .svc
        .embeddings_allow(&ctx(), &id.to_text(), "note")
        .unwrap_err();
    assert_eq!(refused.code, "secret_allow_refused");
}

#[test]
fn allows_are_pruned_and_capped() {
    let mut w = world(Locality::Hosted);
    let mut ids = Vec::new();
    for i in 0..257 {
        let (id, _) = add(
            &mut w,
            &format!("n{i}"),
            &format!("password: Zx9{i}Qw7Er5Ty3"),
            "x",
        );
        ids.push(id);
    }
    for id in &ids[..256] {
        assert!(
            w.svc
                .embeddings_allow(&ctx(), &id.to_text(), "note")
                .unwrap()
                .value
                .allowed
        );
    }
    let full = w
        .svc
        .embeddings_allow(&ctx(), &ids[256].to_text(), "note")
        .unwrap_err();
    assert_eq!(full.code, "secret_allow_refused");
    // Redacting one frees its entry on the next write.
    w.svc
        .node_edit(
            &ctx(),
            &ids[0].to_text(),
            NodeEditRequest {
                note: Some("in the vault".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        w.svc
            .embeddings_allow(&ctx(), &ids[256].to_text(), "note")
            .unwrap()
            .value
            .allowed
    );
}

#[test]
fn a_query_holding_a_secret_never_reaches_a_hosted_embedder() {
    let mut w = world(Locality::Hosted);
    add(&mut w, "Billing export", "clean", "x");
    run(&mut w);
    let before = w.spy.count();
    let key = fake_key("Q7Z3X9K2");
    let text = format!("billing {key}");
    assert_eq!(w.svc.embed_query("demo", &text).unwrap(), None);
    let plan = w.svc.search_plan(&ctx(), &text, None, 10).unwrap();
    let queries = plan.embed();
    assert!(
        queries.values().all(|q| *q == Err(QueryFailed::Secret)) && !queries.is_empty(),
        "{queries:?}"
    );
    assert_eq!(w.spy.count(), before, "nothing was sent");
    let found = w.svc.search_finish(plan, queries).unwrap();
    assert!(
        found
            .notes
            .iter()
            .any(|p| p.code == "similarity_unavailable"),
        "{:?}",
        found.notes
    );
    // A clean query does go (positive control).
    assert!(
        w.svc
            .embed_query("demo", "billing export")
            .unwrap()
            .is_some()
    );
}

/// d-20 (SEC-5) still holds when the query embed times out (req-53): an unnamed search skips the secured graph with a
/// count only, and its word matches come back with `similarity_unavailable`.
#[test]
fn a_timed_out_query_embed_keeps_skipping_unnamed_secured_graphs_with_a_count() {
    let mut w = world(Locality::Local);
    add(&mut w, "Billing export", "clean", "x");
    let secured = umx_core::GraphEmbedding::new(None, umx_core::Policy::SECURED);
    w.svc
        .graph_new(&ctx(), "/work/v", "vault", None, false, Some(secured))
        .unwrap();
    let mut in_vault = ctx();
    in_vault.graph = Some("vault".into());
    w.svc
        .node_add(
            &in_vault,
            NodeAddRequest {
                kind: "problem".into(),
                title: "Billing vault plan".into(),
                ..Default::default()
            },
        )
        .unwrap();
    run(&mut w);
    let plan = w.svc.search_plan(&ctx(), "billing", None, 10).unwrap();
    let queries = plan.embed_with(&|_, _| true, &|_, _| {
        Err(QueryFailed::TimedOut { warming: false })
    });
    let found = w.svc.search_finish(plan, queries).unwrap();
    let codes: Vec<&str> = found.notes.iter().map(|p| p.code).collect();
    assert!(codes.contains(&"secured_graphs_skipped"), "{codes:?}");
    assert!(codes.contains(&"similarity_unavailable"), "{codes:?}");
    assert!(found.value.hits.iter().all(|h| h.alias == "demo"));
    assert!(!found.value.hits.is_empty());
    assert!(!format!("{:?}", found.notes).contains("vault"));
}

/// R120-1: allowing one field never releases another flagged text through the whole-node text, even when the
/// other text contains the allowed one (a longer secret that starts with it, or the allowed field's own form).
#[test]
fn an_allowed_field_never_lets_another_secret_out_through_the_whole_node() {
    for note in [
        "password: Zx9Qw7Er5Ty3Longer9Q",
        "title: password: Zx9Qw7Er5Ty3Longer9Q",
    ] {
        let mut w = world(Locality::Hosted);
        let (id, _) = add(&mut w, "password: Zx9Qw7Er5Ty3", note, "x");
        let allowed = w
            .svc
            .embeddings_allow(&ctx(), &id.to_text(), "title")
            .unwrap();
        assert!(allowed.value.allowed);
        run(&mut w);
        assert!(w.spy.saw("Zx9Qw7Er5Ty3"), "the allowed title went");
        assert!(
            !w.spy.saw("Longer9Q"),
            "{note}: the note's longer secret never went"
        );
    }
}

/// R120-2: the allows file never grows past what its reader takes. With the longest field names, every one of 256
/// allows keeps it under the cap; the next is refused, saying how to make room, and every earlier allow still reads.
#[test]
fn the_allows_file_stays_under_the_reader_cap_by_bytes_as_well_as_count() {
    use umx_core::secret_allows::{MAX_ALLOWS_BYTES, SecretAllows};
    let mut w = world(Locality::Hosted);
    let long = "a_very_long_field_name_that_reaches_the_limit_of_sixty_four_char";
    assert_eq!(long.len(), 64);
    w.svc
        .ontology_add_field(&ctx(), "problem", &format!("{long}:string"), None)
        .unwrap();
    let mut granted = 0;
    let refused = loop {
        let done = w
            .svc
            .node_add(
                &ctx(),
                NodeAddRequest {
                    kind: "problem".into(),
                    title: format!("n{granted}"),
                    sets: vec![(
                        long.into(),
                        SetValue::Str(format!("password: Zx9{granted}Qw7Er5Ty3")),
                    )],
                    ..Default::default()
                },
            )
            .unwrap();
        match w
            .svc
            .embeddings_allow(&ctx(), &done.value.node.id.to_text(), long)
        {
            Ok(_) => granted += 1,
            Err(p) => break p,
        }
        let bytes = w.allows.read().unwrap().unwrap();
        assert!(
            bytes.len() <= MAX_ALLOWS_BYTES,
            "allow {granted}: {} bytes",
            bytes.len()
        );
        assert!(granted <= 256);
    };
    assert_eq!(refused.code, "secret_allow_refused");
    assert!(
        format!("{refused:?}").contains("remove allows"),
        "{refused:?}"
    );
    assert_eq!(granted, 256);
    let kept = SecretAllows::decode(&w.allows.read().unwrap().unwrap()).unwrap();
    assert_eq!(kept.allows.len(), granted);
}

/// embeddings-expert: held nodes are counted apart from pending ones, so pending reaches 0 once everything
/// that may be sent is embedded; a search names how many of the nodes it could not compare are held.
#[test]
fn held_nodes_are_counted_apart_from_pending_ones() {
    let mut w = world(Locality::Hosted);
    let key = fake_key("Q7Z3X9K2");
    let (id, _) = add(&mut w, "Billing export", &format!("uses {key}"), "x");
    add(&mut w, "Billing report", "clean", "y");
    run(&mut w);
    let info = &w.svc.graph_list()[0];
    assert_eq!(
        (info.vectors_pending, info.vectors_held),
        (Some(0), Some(1))
    );
    let plan = w.svc.search_plan(&ctx(), "billing", None, 10).unwrap();
    let queries = plan.embed();
    let found = w.svc.search_finish(plan, queries).unwrap();
    let incomplete = found
        .notes
        .iter()
        .find(|p| p.code == "similarity_incomplete")
        .unwrap();
    assert_eq!(incomplete.slot("n").as_deref(), Some("1"));
    assert_eq!(incomplete.slot("held").as_deref(), Some("1"));
    // Allowed and embedded: neither pending nor held.
    w.svc
        .embeddings_allow(&ctx(), &id.to_text(), "note")
        .unwrap();
    run(&mut w);
    let info = &w.svc.graph_list()[0];
    assert_eq!(
        (info.vectors_pending, info.vectors_held),
        (Some(0), Some(0))
    );
}

#[test]
fn a_secret_in_a_data_field_still_warns_at_write_is_never_sent_and_needs_no_allow() {
    let mut w = world(Locality::Hosted);
    w.svc
        .ontology_add_field_as(&ctx(), "problem", "token:string", None, Some("data"))
        .unwrap();
    let key = fake_key("D4T4F1E7");
    let done = w
        .svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "problem".into(),
                title: "Export".into(),
                sets: vec![("token".into(), SetValue::Str(key.clone()))],
                ..Default::default()
            },
        )
        .unwrap();
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert!(codes.contains(&"secret_suspected"), "{codes:?}");
    let id = done.value.node.id;
    run(&mut w);
    assert!(!w.spy.saw(&key), "a data field is never sent");
    assert!(w.spy.saw("Export"), "the rest of the node went");
    assert_eq!(
        w.svc.graph_list()[0].vectors_held,
        Some(0),
        "nothing is held"
    );
    let allow = w
        .svc
        .embeddings_allow(&ctx(), &id.to_text(), "token")
        .unwrap();
    assert!(!allow.value.allowed);
    assert_eq!(
        allow.value.why,
        Some("the field is data, so it is never embedded or sent")
    );
}

#[test]
fn a_field_made_data_between_plan_and_send_never_reaches_a_hosted_embedder() {
    // R189-1: the worker plans, releases the lock and checks each batch again right before sending.
    let mut w = world(Locality::Hosted);
    w.svc
        .ontology_add_field(&ctx(), "problem", "file:string", None)
        .unwrap();
    w.svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "problem".into(),
                title: "Export".into(),
                sets: vec![("file".into(), SetValue::Str("src/zzprivate.rs".into()))],
                ..Default::default()
            },
        )
        .unwrap();
    let jobs = w.svc.take_embed_jobs();
    let planned: Vec<Planned> = jobs
        .iter()
        .filter_map(|j| w.svc.embed_plan(j).unwrap())
        .collect();
    let texts = |b: &[Planned]| -> Vec<String> {
        b.iter()
            .flat_map(|p| &p.texts)
            .map(|t| t.text.clone())
            .collect()
    };
    let was = texts(&planned);
    assert!(
        was.iter()
            .any(|t| t == "title: none | text: src/zzprivate.rs"),
        "{was:?}"
    );
    assert!(
        was.iter()
            .any(|t| t.starts_with("title: Export") && t.contains("zzprivate")),
        "{was:?}"
    );
    w.svc
        .ontology_set_field(&ctx(), "problem", "file", "data")
        .unwrap();
    let embedder: Arc<dyn Embedder> = w.spy.clone();
    let batch = w
        .svc
        .embedding()
        .still_sendable(w.svc.graph(), &embedder, planned);
    let (vectors, _) = embed(&*embedder, &batch).unwrap();
    assert!(
        !w.spy.saw("zzprivate"),
        "neither the field nor the whole node"
    );
    assert!(w.spy.saw("Export"), "the title's own vector still went");
    w.svc.embed_apply(&vectors).unwrap();
    // The role change queued the node: its current whole-node text goes next, without the file.
    run(&mut w);
    assert!(!w.spy.saw("zzprivate"));
    assert!(w.spy.saw("title: Export | text: "));
}

/// F188: a read names every place of a shown node the write scan's own detector flags, in scan order (a field, a
/// data field, the locator, an entity key by its position in the stored array, an evidence pointer, a status reason
/// and a removal entry, by position), never the text. A column read flags only its columns; an embedded field this
/// machine allows is not flagged, as on a write; a node with nothing flagged gets no names.
#[test]
fn reads_name_every_flagged_place_and_honour_columns_and_allows() {
    let mut w = world(Locality::Hosted);
    let key = fake_key("Q7Z3X9K2");
    let (id, _) = add(&mut w, "Billing", &format!("uses {key}"), "nothing secret");
    let (clean, _) = add(&mut w, "Plain", "nothing here", "nothing secret");
    let (_, stored) = w.svc.node_get(&ctx(), &id.to_text()).unwrap().value;
    let (_, plain) = w.svc.node_get(&ctx(), &clean.to_text()).unwrap().value;

    let mut shown = stored.clone();
    shown
        .fields
        .insert("impact".into(), serde_json::json!(format!("costs {key}")));
    shown.fields.insert(
        "entity_key".into(),
        serde_json::json!(["email:ann@example.test", format!("aws:{key}")]),
    );
    shown.locator = Some(
        umx_core::Locator::parse(&format!("https://docs.example.test/a?token={key}")).unwrap(),
    );
    shown.evidence = vec![
        serde_json::json!({ "source": "docs/clean.md", "kind": "document" }),
        serde_json::json!({ "source": format!("https://x.example.test/a?token={key}"), "kind": "document" }),
    ];
    shown.status_history = vec![
        serde_json::json!({ "from": "open", "to": "closed", "reason": "done" }),
        serde_json::json!({ "from": "closed", "to": "open", "reason": format!("leaked {key}") }),
    ];
    shown.fields.insert(
        "evidence_removed".into(),
        serde_json::json!([{ "removed": { "source": "docs/old.md" }, "reason": format!("was {key}") }]),
    );
    let whole = w
        .svc
        .secret_flags(&[("demo", &shown), ("demo", &plain)], None);
    assert_eq!(
        whole[0],
        [
            "field:note",
            "field:impact",
            "key:2",
            "field:locator",
            "evidence:2",
            "status_reason:2",
            "evidence_removed:1",
        ]
    );
    assert!(whole[1].is_empty(), "nothing flagged, no names");
    assert!(
        !format!("{whole:?}").contains("AKIA"),
        "names never carry the text"
    );

    let columns: Vec<String> = ["id", "title", "entity_key"].map(String::from).to_vec();
    let picked = w.svc.secret_flags(&[("demo", &shown)], Some(&columns));
    assert_eq!(
        picked[0],
        ["key:2"],
        "an unreturned column is never flagged"
    );

    w.svc
        .embeddings_allow(&ctx(), &id.to_text(), "note")
        .unwrap();
    let allowed = w.svc.secret_flags(&[("demo", &stored)], None);
    assert!(allowed[0].is_empty(), "{allowed:?}");
}

/// R324-3: entity show reads this machine's allows once per read, however many members and proposals it shows with
/// a suspect title.
#[test]
fn entity_show_reads_the_allows_file_once_for_every_suspect_title() {
    let mut w = world(Locality::Hosted);
    let key = fake_key("Q7Z3X9K2");
    let ids: Vec<Id> = ["one", "two", "three"]
        .iter()
        .map(|t| add(&mut w, &format!("uses {key} {t}"), "nothing", "nothing").0)
        .collect();
    for pair in ids.windows(2) {
        w.svc
            .entity_link(&ctx(), &pair[0].to_text(), &pair[1].to_text(), false)
            .unwrap();
    }
    let before = w.allows.reads();
    let shown = w.svc.entity_show(&ctx(), &ids[0].to_text()).unwrap().value;
    assert_eq!(shown.members.len(), 3);
    assert!(
        shown
            .members
            .iter()
            .all(|m| m.secret_suspected == ["field:title"]),
        "{:?}",
        shown.members
    );
    assert_eq!(
        w.allows.reads() - before,
        1,
        "one allows read for the whole show"
    );
}

/// R324-4: a stored key in an older form (`aws: <token>`, canonical `aws:<token>`) that looks like a secret is not new
/// when an unrelated edit rewrites it canonically: the edit warns nothing about it, and a batch edit is not refused.
#[test]
fn an_unrelated_edit_never_counts_a_loaded_non_canonical_key_as_new() {
    use umx_core::ports::GraphFiles;
    let mut w = world(Locality::Hosted);
    let key = fake_key("Q7Z3X9K2");
    let (id, _) = add(&mut w, "Billing", "nothing", "nothing");
    let dir = GraphDir::parse("/work/g").unwrap();
    let bytes = w
        .files
        .read(&dir, umx_core::codec::GraphFile::Nodes)
        .unwrap()
        .unwrap();
    let mut lines: Vec<serde_json::Value> = String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    for line in &mut lines {
        if line["id"] == id.to_text() {
            line["entity_key"] = serde_json::json!([format!("aws: {key}")]);
        }
    }
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    w.files
        .tamper_for_tests(&dir, umx_core::codec::GraphFile::Nodes, text.as_bytes());
    w.svc.graph_unload(&ctx(), "demo").unwrap();
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    let edit = w
        .svc
        .node_edit(
            &ctx(),
            &id.to_text(),
            NodeEditRequest {
                title: Some("Billing two".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        edit.notes.iter().all(|p| p.code != "secret_suspected"),
        "{:?}",
        edit.notes
    );
    let batch = w.svc.batch(
        &ctx(),
        umx_services::BatchRequest {
            lines: vec![umx_services::BatchLine {
                line: 1,
                op: umx_services::BatchOp::NodeEdit {
                    node: Some(id.to_text()),
                    node_ref: None,
                    req: NodeEditRequest {
                        title: Some("Billing three".into()),
                        ..Default::default()
                    },
                },
            }],
            ..Default::default()
        },
    );
    assert!(batch.is_ok(), "{:?}", batch.err());
}

/// R324-5: a read clears an allowed field only while the field is still embedded: allow a content field, make it a
/// data field, and the read flags it again (vectors leave data fields out, so the allow no longer means anything).
#[test]
fn an_allow_stops_hiding_a_field_once_it_is_made_data() {
    let mut w = world(Locality::Hosted);
    let key = fake_key("Q7Z3X9K2");
    let (id, _) = add(&mut w, "Billing", "nothing", &format!("costs {key}"));
    w.svc
        .embeddings_allow(&ctx(), &id.to_text(), "impact")
        .unwrap();
    let flags = |w: &World| {
        let (_, n) = w.svc.node_get(&ctx(), &id.to_text()).unwrap().value;
        w.svc.secret_flags(&[("demo", &n)], None).remove(0)
    };
    assert!(flags(&w).is_empty(), "allowed while it is content");
    w.svc
        .ontology_set_field(&ctx(), "problem", "impact", "data")
        .unwrap();
    assert_eq!(flags(&w), ["field:impact"]);
}

/// s16 (security N1): node list reads its rows in place, so its flags come from what each row returns, not a decoded
/// node. For every column set they equal the flags the whole decoded node gives for the same columns: suspect text
/// in the title, the note, a declared and an undeclared string field, an entity key, an evidence pointer, a status
/// reason and the locator, an allowed field, and clean rows. The page's flags read the allows file at most once.
#[test]
fn listed_flags_equal_the_whole_nodes_for_every_column_set() {
    let mut w = world(Locality::Hosted);
    let key = fake_key("Q7Z3X9K2");
    let secret = format!("uses {key}");
    let mut node = |title: &str, note: &str, impact: &str, req: NodeAddRequest| {
        w.svc
            .node_add(
                &ctx(),
                NodeAddRequest {
                    kind: "problem".into(),
                    title: title.into(),
                    note: Some(note.into()),
                    sets: [
                        vec![("impact".to_string(), SetValue::Str(impact.into()))],
                        req.sets,
                    ]
                    .concat(),
                    ..req
                },
            )
            .unwrap()
            .value
            .node
            .id
    };
    let plain = NodeAddRequest::default;
    node(&secret, "clean", "clean", plain());
    node("Clean note", &secret, "clean", plain());
    let allowed = node("Allowed impact", "clean", &secret, plain());
    node("Flagged impact", "clean", &secret, plain());
    node(
        "Undeclared",
        "clean",
        "clean",
        NodeAddRequest {
            sets: vec![("owner".into(), SetValue::Str(secret.clone()))],
            ..plain()
        },
    );
    node(
        "Pointer",
        "clean",
        "clean",
        NodeAddRequest {
            evidence: vec![format!("https://x.example.test/a?token={key}#L1").into()],
            ..plain()
        },
    );
    node(
        "Resource",
        "clean",
        "clean",
        NodeAddRequest {
            locator: Some(format!("https://docs.example.test/a?token={key}")),
            ..plain()
        },
    );
    let moved = node("Moved", "clean", "clean", plain());
    let keyed = node("Keyed", "clean", "clean", plain());
    node("Clean", "clean", "clean", plain());
    w.svc
        .node_status(&ctx(), &moved.to_text(), "agreed", Some(&secret), None)
        .unwrap();
    w.svc
        .entity_key(&ctx(), &keyed.to_text(), true, &format!("aws:{key}"))
        .unwrap();
    w.svc
        .embeddings_allow(&ctx(), &allowed.to_text(), "impact")
        .unwrap();
    let page = w
        .svc
        .node_list(
            &ctx(),
            umx_services::NodeListRequest {
                limit: usize::MAX,
                ..Default::default()
            },
        )
        .unwrap()
        .value;
    let rows: Vec<(&str, umx_services::ListedNode<'_>)> =
        page.rows.iter().map(|n| ("demo", *n)).collect();
    let whole: Vec<umx_core::Node> = page.rows.iter().map(|n| n.to_node()).collect();
    let whole: Vec<(&str, &umx_core::Node)> = whole.iter().map(|n| ("demo", n)).collect();
    let base = ["id", "type", "title", "status", "updated"];
    let mut sets: Vec<Vec<&str>> = vec![
        base.to_vec(),
        [&base[..], &["entity_key", "impact", "owner", "prov_key"]].concat(),
    ];
    for c in [
        "note",
        "impact",
        "owner",
        "entity_key",
        "evidence",
        "status_history",
        "locator",
        "locator_source",
        "locator_kind",
        "evidence_removed",
        "secret_suspected",
    ] {
        sets.push(vec!["id", c]);
    }
    sets.push(vec!["id", "impact", "note", "entity_key"]);
    let mut seen = std::collections::BTreeSet::new();
    for set in sets {
        let columns: Vec<String> = set.iter().map(|c| c.to_string()).collect();
        let before = w.allows.reads();
        let listed = w.svc.listed_secret_flags(&rows, &columns);
        assert!(w.allows.reads() - before <= 1, "{set:?}: one allows read");
        assert_eq!(
            listed,
            w.svc.secret_flags(&whole, Some(&columns)),
            "{set:?}"
        );
        seen.extend(listed.into_iter().flatten());
    }
    for name in [
        "field:title",
        "field:note",
        "field:impact",
        "field:owner",
        "key:1",
        "evidence:1",
        "status_reason:1",
        "field:locator",
        "field:locator_source",
    ] {
        assert!(seen.contains(name), "{name} flagged somewhere: {seen:?}");
    }
}
