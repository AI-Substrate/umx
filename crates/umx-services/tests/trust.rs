//! D53 (req-91, plan 071) and D62 (plan 099): a graph's two settings, visibility and embedder. Every instance here
//! is defined in the (machine) config, so every one is trusted: `trusted` admits all of them and `local` only the
//! local ones. Every instance is a spy that records the texts it was sent, so "never reached" is a count of zero,
//! and each test has a positive control that reaches the instance the rule allows.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use umx_core::codec::GraphFile;
use umx_core::ports::{
    EmbedError, Embedded, Embedder, GraphDir, GraphFiles, Locality, ModelKey, TextKind,
};
use umx_core::problem::Host;
use umx_core::setup::GraphFilesSetup;
use umx_core::{EmbedderRule, GraphEmbedding, Policy, Problem, Visibility};
use umx_fakes::{FakeClock, FakeEmbedder, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::embedding::{Instance, embed};
use umx_services::{Ctx, EmbedJob, NodeAddRequest, Ports, QueryFailed, Services};

const NAMED_TRUSTED: Policy = Policy {
    visibility: Visibility::Named,
    embedder: EmbedderRule::Trusted,
};

/// An embedder that remembers every text it was sent.
struct Spy {
    inner: FakeEmbedder,
    seen: Mutex<Vec<String>>,
    forbidden: AtomicBool,
}

impl Spy {
    fn new(model: &str, locality: Locality) -> Arc<Spy> {
        Arc::new(Spy {
            inner: FakeEmbedder::new(ModelKey::parse(model).unwrap()).with_locality(locality),
            seen: Mutex::new(Vec::new()),
            forbidden: AtomicBool::new(false),
        })
    }

    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    fn saw(&self, needle: &str) -> bool {
        self.seen.lock().unwrap().iter().any(|t| t.contains(needle))
    }

    /// How many times exactly `text` was sent (a query is sent alone).
    fn times(&self, text: &str) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|t| *t == text)
            .count()
    }
}

impl Embedder for Spy {
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
        assert!(
            !self.forbidden.load(Ordering::SeqCst),
            "an untrusted instance was asked"
        );
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
        None
    }
    fn adapter_id(&self) -> &'static str {
        "fake.embedder"
    }
}

/// `az` and `rogue` are hosted and make the same model, `rogue` is the default; `nomic` is local. With
/// [`World::with_twin`], `twin` is local and makes the hosted model too.
struct World {
    files: Arc<FakeGraphFiles>,
    state: Arc<FakeLocalState>,
    az: Arc<Spy>,
    rogue: Arc<Spy>,
    nomic: Arc<Spy>,
    twin: Option<Arc<Spy>>,
    svc: Services,
}

const HOSTED: &str = "hosted-m@8";
const LOCAL: &str = "local-m@8";

impl World {
    fn new() -> World {
        let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
        let state = Arc::new(FakeLocalState::new());
        let (az, rogue, nomic) = (
            Spy::new(HOSTED, Locality::Hosted),
            Spy::new(HOSTED, Locality::Hosted),
            Spy::new(LOCAL, Locality::Local),
        );
        let mut w = World {
            svc: Services::new(ports(&files, &state)),
            files,
            state,
            az,
            rogue,
            nomic,
            twin: None,
        };
        w.restart("rogue");
        w
    }

    /// With `twin`, a local instance of the hosted model, so two graphs of one model can use two instances.
    fn with_twin() -> World {
        let mut w = World::new();
        w.twin = Some(Spy::new(HOSTED, Locality::Local));
        w.restart("rogue");
        w
    }

    fn twin(&self) -> &Spy {
        self.twin.as_deref().expect("with_twin")
    }

    /// The daemon starting again over the same files and record; `default` is embedder.active.
    fn restart(&mut self, default: &str) {
        let aliases: Vec<String> = self.svc.graph().aliases().map(String::from).collect();
        let paths: Vec<(String, String)> = aliases
            .iter()
            .map(|a| {
                (
                    a.clone(),
                    self.svc.loaded(a).unwrap().dir.as_str().to_owned(),
                )
            })
            .collect();
        let mut svc = Services::new(ports(&self.files, &self.state));
        let mut instances = vec![
            Instance {
                name: "az".into(),
                embedder: self.az.clone(),
            },
            Instance {
                name: "nomic".into(),
                embedder: self.nomic.clone(),
            },
            Instance {
                name: "rogue".into(),
                embedder: self.rogue.clone(),
            },
        ];
        if let Some(twin) = &self.twin {
            instances.push(Instance {
                name: "twin".into(),
                embedder: twin.clone(),
            });
        }
        svc.set_instances(instances, Some(default));
        // The hosts compose parses from each hosted instance's endpoint (D83, security W2): host[:port] only.
        svc.set_hosts(
            [
                ("az", "https://u:secret@az.example/p?api-key=x"),
                ("rogue", "https://rogue.example:8443/openai"),
            ]
            .into_iter()
            .map(|(n, e)| (n.to_string(), Host::from_endpoint(e).unwrap()))
            .collect(),
        );
        for (alias, path) in paths {
            svc.graph_load(&ctx(), &path, Some(&alias)).unwrap();
        }
        self.svc = svc;
    }

    fn graph(&mut self, alias: &str, model: &str, policy: Policy) -> Vec<&'static str> {
        let embedding = GraphEmbedding::new(Some(ModelKey::parse(model).unwrap()), policy);
        let done = self
            .svc
            .graph_new(
                &ctx(),
                &format!("/work/{alias}"),
                alias,
                None,
                false,
                Some(embedding),
            )
            .unwrap();
        done.notes.iter().map(|p| p.code).collect()
    }

    fn add(&mut self, alias: &str, title: &str, note: &str) {
        self.svc
            .node_add(
                &ctx().graph(alias),
                NodeAddRequest {
                    kind: "problem".into(),
                    title: title.into(),
                    note: Some(note.into()),
                    ..Default::default()
                },
            )
            .unwrap();
    }

    /// The daemon's embed worker in one step: each job goes to its graph's instance, re-checked right before it is
    /// sent (`still_sendable`), then applied.
    fn run(&mut self) {
        loop {
            let jobs: Vec<EmbedJob> = self.svc.take_embed_jobs();
            if jobs.is_empty() {
                return;
            }
            for job in jobs {
                let Some(planned) = self.svc.embed_plan(&job).unwrap() else {
                    continue;
                };
                let Some(i) = self
                    .svc
                    .embedding()
                    .embedder_for(self.svc.graph(), &job.alias)
                else {
                    continue;
                };
                let batch = self.svc.embedding().still_sendable(
                    self.svc.graph(),
                    &i.embedder,
                    vec![planned],
                );
                if batch.is_empty() {
                    continue;
                }
                let (vectors, _) = embed(&*i.embedder, &batch).unwrap();
                self.svc.embed_apply(&vectors).unwrap();
            }
        }
    }

    fn instance(&self, alias: &str) -> Option<String> {
        self.svc
            .embedding()
            .embedder_for(self.svc.graph(), alias)
            .map(|i| i.name)
    }

    /// graph.json rewritten as a pull or a hand edit would, then loaded again.
    fn edit_graph_json(
        &mut self,
        alias: &str,
        edit: impl Fn(&mut serde_json::Value),
    ) -> Vec<&'static str> {
        let dir = GraphDir::parse(&format!("/work/{alias}")).unwrap();
        let bytes = self.files.read(&dir, GraphFile::Graph).unwrap().unwrap();
        let mut doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        edit(&mut doc["embedding"]);
        let mut out = serde_json::to_vec_pretty(&doc).unwrap();
        out.push(b'\n');
        self.files.write(&dir, GraphFile::Graph, &out).unwrap();
        self.svc.graph_unload(&ctx(), alias).unwrap();
        let done = self
            .svc
            .graph_load(&ctx(), &format!("/work/{alias}"), Some(alias))
            .unwrap();
        done.notes.iter().map(|p| p.code).collect()
    }
}

fn ports(files: &Arc<FakeGraphFiles>, state: &Arc<FakeLocalState>) -> Ports {
    Ports {
        files: files.clone(),
        vector_cache: files.clone(),
        vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: state.clone(),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        sources: Arc::new(umx_fakes::FakeSourceReader::open(
            GraphDir::parse("/").unwrap(),
        )),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    }
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

/// A fake key built at run time, so no key-shaped literal sits in the source.
fn fake_key() -> String {
    let mut k = String::from("AKIA");
    while k.len() < 20 {
        k.push('Q');
    }
    k
}

#[test]
fn a_secured_graph_never_reaches_a_hosted_instance_trusted_or_not() {
    let mut w = World::new();
    // A hosted (so trusted) instance is the default here, so only the rule keeps it away.
    w.restart("az");
    let warned = w.graph("vault", HOSTED, Policy::SECURED);
    assert_eq!(warned, ["embedder_not_local_for_secured"]);
    w.graph("open", HOSTED, Policy::OPEN);
    w.add("vault", "Client plan", "Kept here");
    w.add("open", "Roadmap", "Public");
    w.run();
    assert_eq!(w.instance("vault"), None);
    assert!(!w.az.saw("Client plan") && !w.rogue.saw("Client plan"));
    assert_eq!(w.svc.embed_query("vault", "plan").unwrap(), None);
    // Positive control: an open graph of the same model reaches it.
    assert!(w.az.saw("Roadmap"));
}

#[test]
fn a_hosted_instance_in_the_machine_config_serves_a_named_trusted_graph_with_no_confirmation() {
    let mut w = World::new();
    w.restart("az");
    let warned = w.graph("acme", HOSTED, NAMED_TRUSTED);
    assert!(warned.is_empty(), "{warned:?}");
    w.add("acme", "Tenant plan", "Ours only");
    w.run();
    assert_eq!(w.instance("acme").as_deref(), Some("az"));
    assert!(w.az.saw("Tenant plan"), "documents go to it");
    let before = w.az.count();
    assert!(w.svc.embed_query("acme", "tenant").unwrap().is_some());
    assert_eq!(w.az.count(), before + 1, "the query too");
    assert_eq!(w.rogue.count(), 0, "only the instance it routes to");
}

#[test]
fn an_open_and_a_trusted_graph_share_the_trusted_instance_and_a_local_graph_never_reaches_it() {
    let mut w = World::new();
    w.restart("az");
    w.graph("open", HOSTED, Policy::OPEN);
    w.graph("acme", HOSTED, NAMED_TRUSTED);
    let local_only = Policy {
        visibility: Visibility::Open,
        embedder: EmbedderRule::Local,
    };
    w.graph("mine", HOSTED, local_only);
    w.add("open", "Open one", "a");
    w.add("acme", "Trusted one", "b");
    w.add("mine", "Local one", "c");
    w.run();
    assert!(w.az.saw("Open one") && w.az.saw("Trusted one"));
    assert!(!w.az.saw("Local one") && !w.rogue.saw("Local one"));
}

#[test]
fn a_trusted_graph_without_a_model_takes_the_default_hosted_instance() {
    let mut w = World::new();
    let embedding = GraphEmbedding::new(None, NAMED_TRUSTED);
    w.svc
        .graph_new(&ctx(), "/work/acme", "acme", None, false, Some(embedding))
        .unwrap();
    assert_eq!(w.instance("acme").as_deref(), Some("rogue"));
}

#[test]
fn d53_trust_records_in_the_policy_record_load_and_admit_nothing_more() {
    let mut w = World::new();
    w.state.set_bytes(
        br#"{
  "umx_state": 1,
  "graphs": [],
  "trusted": [
    { "instance": "ghost", "target": "0123", "confirmed_at": "2026-11-11T00:00:00Z" },
    { "instance": "az", "target": "0123", "confirmed_at": "2026-11-11T00:00:00Z" }
  ]
}"#,
    );
    let warned = w.graph("vault", HOSTED, Policy::SECURED);
    assert_eq!(warned, ["embedder_not_local_for_secured"]);
    assert_eq!(w.instance("vault"), None);
    for name in ["az", "rogue", "ghost"] {
        assert!(
            !w.svc.embedding().admits_now(w.svc.graph(), "vault", name),
            "{name}"
        );
    }
    assert!(
        !w.svc.embedding().trusted("ghost"),
        "never an instance the config does not define"
    );
    w.add("vault", "Client plan", "Kept here");
    w.run();
    assert_eq!((w.az.count(), w.rogue.count()), (0, 0));
    // The next write drops them.
    let record = String::from_utf8(w.state.bytes().unwrap()).unwrap();
    assert!(
        !record.contains("trusted") && !record.contains("ghost"),
        "{record}"
    );
}

#[test]
fn held_secrets_never_reach_a_trusted_instance() {
    let mut w = World::new();
    w.restart("az");
    w.graph("acme", HOSTED, NAMED_TRUSTED);
    let key = fake_key();
    w.add("acme", "Deploy notes", &format!("the key is {key}"));
    w.run();
    assert!(
        w.az.saw("Deploy notes"),
        "the rest of the node still embeds"
    );
    assert!(
        !w.az.saw(&key),
        "the held field never goes to the trusted instance"
    );
    let before = w.az.count();
    assert_eq!(
        w.svc.embed_query("acme", &format!("find {key}")).unwrap(),
        None
    );
    assert_eq!(w.az.count(), before, "nor does a query holding a secret");
}

#[test]
fn a_loosening_by_pull_or_hand_edit_never_takes_effect_until_a_umx_call_confirms_it() {
    let mut w = World::new();
    // local to trusted
    w.graph("vault", HOSTED, Policy::SECURED);
    let warned = w.edit_graph_json("vault", |e| {
        e["visibility"] = "named".into();
        e["embedder"] = "trusted".into();
    });
    assert!(
        warned.contains(&"policy_downgrade_unconfirmed"),
        "{warned:?}"
    );
    assert_eq!(w.svc.effective_policy("vault"), Some(Policy::SECURED));
    w.add("vault", "Still local", "x");
    w.run();
    assert!(!w.az.saw("Still local") && !w.rogue.saw("Still local"));
    // trusted to any
    w.graph("acme", HOSTED, NAMED_TRUSTED);
    let warned = w.edit_graph_json("acme", |e| {
        let o = e.as_object_mut().unwrap();
        o.remove("visibility");
        o.remove("embedder");
        o.insert("policy".into(), "open".into());
    });
    assert!(
        warned.contains(&"policy_downgrade_unconfirmed"),
        "{warned:?}"
    );
    assert_eq!(w.svc.effective_policy("acme"), Some(NAMED_TRUSTED));
    assert!(!w.svc.graph().shown_unnamed("acme"), "still named");
    // One umx call by any caller confirms it, and warns: then the file's pair is what umx acts on.
    let done = w.svc.confirm_policy(&ctx(), "acme").unwrap();
    assert_eq!(codes(&done.notes), ["policy_lowered"]);
    assert_eq!(w.svc.effective_policy("acme"), Some(Policy::OPEN));
}

fn codes(notes: &[Problem]) -> Vec<&'static str> {
    notes.iter().map(|p| p.code).collect()
}

fn lowered(notes: &[Problem]) -> Vec<&Problem> {
    notes
        .iter()
        .filter(|p| p.code == "policy_lowered")
        .collect()
}

fn set(w: &mut World, alias: &str, name: &str, value: &str) -> Vec<Problem> {
    w.svc
        .graph_set(&ctx(), alias, &[(name.into(), value.into())])
        .unwrap()
        .notes
}

/// D83: an agent (nothing typed, no terminal) lowers secured to named+trusted in one call. The record holds the
/// downgrade with its time, the warning names what loosened and each newly admitted instance with its host, and the
/// daemon then embeds the graph with the newly admitted instance by itself (today's behaviour, pinned here).
#[test]
fn an_agent_lowers_a_secured_graph_in_one_call_and_its_nodes_then_embed_by_themselves() {
    let mut w = World::new();
    w.graph("vault", HOSTED, Policy::SECURED);
    w.add("vault", "Vendor onboarding", "slow");
    w.run();
    assert_eq!(
        (w.az.count(), w.rogue.count()),
        (0, 0),
        "secured: nothing sent"
    );
    let notes = set(&mut w, "vault", "embedder", "trusted");
    let warned = lowered(&notes);
    assert_eq!(warned.len(), 1, "{notes:?}");
    let p = warned[0];
    assert_eq!(
        (
            p.slot("from").as_deref(),
            p.slot("to").as_deref(),
            p.slot("loosened").as_deref()
        ),
        (
            Some("secured"),
            Some("named+trusted"),
            Some("embedder rule local to trusted")
        )
    );
    let receivers = p.slot("receivers").unwrap();
    assert_eq!(
        receivers,
        "az (hosted at az.example) and rogue (hosted at rogue.example:8443)"
    );
    assert!(!format!("{p:?}").contains("secret") && !format!("{p:?}").contains("api-key"));
    assert_eq!(p.slot("at").as_deref(), Some("2026-11-09T00:00:00Z"));
    assert_eq!(
        p.slot("store").as_deref(),
        Some(""),
        "not open: the cache keeps its vectors"
    );
    assert_eq!(w.svc.effective_policy("vault"), Some(NAMED_TRUSTED));
    let shown = &w.svc.graph().loaded("vault").unwrap().policy;
    assert_eq!(shown.downgraded_at.as_deref(), Some("2026-11-09T00:00:00Z"));
    let record = String::from_utf8(w.state.bytes().unwrap()).unwrap();
    assert!(record.contains("secured to named+trusted"), "{record}");
    // No further command: the queued node goes to the newly admitted instance the graph routes to.
    w.run();
    assert!(
        w.rogue.saw("Vendor onboarding"),
        "embedded after the one-call downgrade"
    );
    assert!(w.rogue.count() > 0);
}

/// Security A1: only a `graph set` that itself lowers graph.json lowers the record; a pulled downgrade stays pending
/// through any other setting.
#[test]
fn a_graph_set_that_does_not_lower_leaves_a_pulled_downgrade_pending() {
    let mut w = World::new();
    w.graph("vault", HOSTED, Policy::SECURED);
    w.edit_graph_json("vault", |e| e["policy"] = "open".into());
    let notes = set(&mut w, "vault", "title", "Renamed");
    assert!(
        codes(&notes).contains(&"policy_downgrade_unconfirmed"),
        "{notes:?}"
    );
    assert!(lowered(&notes).is_empty(), "{notes:?}");
    assert_eq!(w.svc.effective_policy("vault"), Some(Policy::SECURED));
    w.add("vault", "Still local", "x");
    w.run();
    assert_eq!((w.az.count(), w.rogue.count()), (0, 0));
}

/// Security A2: the warning compares effective policies, so a visibility a pull lowered is named too when this call
/// lowers the embedder rule and the record drops on both.
#[test]
fn the_warning_names_every_loosening_of_the_effective_policy() {
    let mut w = World::new();
    w.graph("vault", HOSTED, Policy::SECURED);
    w.edit_graph_json("vault", |e| {
        e["visibility"] = "open".into();
        e["embedder"] = "local".into();
    });
    assert_eq!(w.svc.effective_policy("vault"), Some(Policy::SECURED));
    let notes = set(&mut w, "vault", "embedder", "trusted");
    let warned = lowered(&notes);
    assert_eq!(warned.len(), 1, "{notes:?}");
    assert_eq!(
        warned[0].slot("loosened").as_deref(),
        Some("visibility named to open and embedder rule local to trusted")
    );
    assert_eq!(warned[0].slot("from").as_deref(), Some("secured"));
    assert!(w.svc.graph().shown_unnamed("vault"));
}

/// Security A3: graph.json is written, but the record cannot take the lower policy: the graph stays at its recorded
/// policy, the reply says so, and no instance it did not admit before gets its text.
#[test]
fn a_record_that_cannot_take_the_lowering_keeps_the_graph_where_it_was() {
    let mut w = World::new();
    w.graph("vault", HOSTED, Policy::SECURED);
    w.add("vault", "Vendor onboarding", "slow");
    w.state
        .fail_writes(Some("write graph-policies.json: permission denied"));
    let notes = set(&mut w, "vault", "embedder", "trusted");
    assert_eq!(
        codes(&notes),
        [
            "policy_not_lowered",
            "policy_downgrade_unconfirmed",
            "embedder_not_local_for_secured"
        ]
    );
    assert_eq!(w.svc.effective_policy("vault"), Some(Policy::SECURED));
    w.run();
    assert_eq!((w.az.count(), w.rogue.count()), (0, 0));
    // Once the record takes writes, one umx call lowers it.
    w.state.fail_writes(None);
    let done = w.svc.confirm_policy(&ctx(), "vault").unwrap();
    assert_eq!(lowered(&done.notes).len(), 1, "{:?}", done.notes);
    assert_eq!(w.svc.effective_policy("vault"), Some(NAMED_TRUSTED));
}

/// Security A4: the record is lowered by graph id, so another loaded copy whose graph.json is lower too drops in the
/// same call; each copy that changed gets its own warning, by its own alias.
#[test]
fn every_loaded_copy_the_lowering_reaches_is_warned() {
    let mut w = World::new();
    w.graph("vault", HOSTED, Policy::SECURED);
    let from = GraphDir::parse("/work/vault").unwrap();
    let to = GraphDir::parse("/work/copy").unwrap();
    for f in [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges] {
        if let Some(bytes) = w.files.read(&from, f).unwrap() {
            w.files.write(&to, f, &bytes).unwrap();
        }
    }
    w.svc
        .graph_load(&ctx(), "/work/copy", Some("copy"))
        .unwrap();
    w.edit_graph_json("copy", |e| {
        e["visibility"] = "named".into();
        e["embedder"] = "trusted".into();
    });
    assert_eq!(w.svc.effective_policy("copy"), Some(Policy::SECURED));
    let notes = set(&mut w, "vault", "embedder", "trusted");
    let graphs: Vec<String> = lowered(&notes)
        .iter()
        .map(|p| p.slot("graph").unwrap())
        .collect();
    assert_eq!(graphs, ["copy", "vault"]);
    assert_eq!(w.svc.effective_policy("copy"), Some(NAMED_TRUSTED));
}

/// Security W3: the instances named are the ones the routing rule admits after the folder rule; one the folder
/// narrows away is not named (the control, with no narrowing, names it).
#[test]
fn the_warning_names_only_instances_the_graphs_folder_may_use() {
    for (narrowed, expected) in [
        (
            false,
            "az (hosted at az.example) and rogue (hosted at rogue.example:8443)",
        ),
        (true, "rogue (hosted at rogue.example:8443)"),
    ] {
        let mut w = World::new();
        if narrowed {
            w.svc.set_folder_rule(Arc::new(|_, name| name != "az"));
        }
        w.graph("vault", HOSTED, Policy::SECURED);
        let notes = set(&mut w, "vault", "embedder", "trusted");
        assert_eq!(
            lowered(&notes)[0].slot("receivers").as_deref(),
            Some(expected),
            "narrowed: {narrowed}"
        );
    }
}

/// Security W6: lowered to fully open, the warning says new vectors go in the graph folder itself; trusted to any
/// admits no new instance here (every one is trusted), and says so.
#[test]
fn a_lowering_to_open_says_where_new_vectors_go() {
    let mut w = World::new();
    w.graph("acme", HOSTED, NAMED_TRUSTED);
    let notes = set(&mut w, "acme", "policy", "open");
    let p = lowered(&notes)[0];
    assert_eq!(
        p.slot("loosened").as_deref(),
        Some("visibility named to open and embedder rule trusted to any")
    );
    assert_eq!(
        p.slot("receivers").as_deref(),
        Some("no embedder instance of this machine")
    );
    assert!(p.slot("store").unwrap().contains("graph folder itself"));
}

#[test]
fn unscoped_search_skips_named_graphs_and_only_counts_them() {
    let mut w = World::new();
    w.graph("acme", HOSTED, NAMED_TRUSTED);
    w.graph("open", HOSTED, Policy::OPEN);
    w.add("acme", "Quarterly budget", "named");
    w.add("open", "Quarterly roadmap", "open");
    let found = w.svc.search(&ctx(), "quarterly", None, 10).unwrap();
    let graphs: Vec<&str> = found.value.hits.iter().map(|h| h.alias.as_str()).collect();
    assert_eq!(graphs, ["open"]);
    let skipped: Vec<_> = found
        .notes
        .iter()
        .filter(|p| p.code == "secured_graphs_skipped")
        .collect();
    assert_eq!(skipped.len(), 1);
    let text = format!("{:?}", skipped[0]);
    assert!(!text.contains("acme"), "never named: {text}");
    // Named, it is read.
    let named = w
        .svc
        .search(&ctx().graph("acme"), "quarterly", None, 10)
        .unwrap();
    assert_eq!(named.value.hits.len(), 1);
}

#[test]
fn a_legacy_secured_graph_and_record_keep_todays_meaning() {
    let mut w = World::new();
    w.graph("vault", LOCAL, Policy::SECURED);
    let dir = GraphDir::parse("/work/vault").unwrap();
    let text = String::from_utf8(w.files.read(&dir, GraphFile::Graph).unwrap().unwrap()).unwrap();
    assert!(
        text.contains("\"policy\": \"secured\"") && !text.contains("\"embedder\""),
        "{text}"
    );
    let record = String::from_utf8(w.state.bytes().unwrap()).unwrap();
    assert!(record.contains("\"policy\": \"secured\"") && !record.contains("\"visibility\""));
    w.restart("rogue");
    assert_eq!(w.svc.effective_policy("vault"), Some(Policy::SECURED));
    assert_eq!(w.instance("vault").as_deref(), Some("nomic"));
}

const QUERY: &str = "tenant";

/// vault is secured (named + local) on `twin`, a local instance of the hosted model; z_open is open on `rogue`, the
/// hosted default of the same model. Both hold vectors, so both have a vector leg on one model and two instances.
fn two_graphs_one_model() -> World {
    let mut w = World::with_twin();
    w.graph("vault", HOSTED, Policy::SECURED);
    w.graph("z_open", HOSTED, Policy::OPEN);
    w.add("vault", "Tenant plan", "Ours only");
    w.add("z_open", "Tenant roadmap", "Public");
    w.run();
    assert_eq!(w.instance("vault").as_deref(), Some("twin"));
    assert_eq!(w.instance("z_open").as_deref(), Some("rogue"));
    w
}

#[test]
fn all_graph_opt_in_keeps_trusted_local_and_open_routing_separate() {
    let mut w = World::new();
    w.restart("az");
    w.graph("acme", HOSTED, NAMED_TRUSTED);
    w.graph("vault", HOSTED, Policy::SECURED);
    w.graph("public", HOSTED, Policy::OPEN);
    for alias in ["acme", "vault", "public"] {
        w.add(alias, "Tenant plan", "Synthetic");
    }
    w.run();
    w.rogue.forbidden.store(true, Ordering::SeqCst);
    let plan = w
        .svc
        .search_plan_allowing(
            &ctx().scope(umx_services::Scope::All {
                include_named: true,
            }),
            QUERY,
            None,
            10,
            &|dir, name| !(dir.as_str() == "/work/public" && name == "rogue"),
        )
        .unwrap();
    let queries = plan.embed();
    assert_eq!(
        queries.keys().map(String::as_str).collect::<Vec<_>>(),
        ["az"]
    );
    assert_eq!(
        w.az.times(QUERY),
        1,
        "one hosted query for the open and trusted graphs together"
    );
    assert_eq!(w.rogue.times(QUERY), 0);
    let found = w.svc.search_finish(plan, queries).unwrap();
    assert!(
        found
            .value
            .hits
            .iter()
            .any(|h| h.alias == "vault" && h.vector.is_none())
    );
    assert!(
        found
            .notes
            .iter()
            .any(|p| p.code == "similarity_unavailable" && format!("{p:?}").contains("vault"))
    );
    let read = found
        .notes
        .iter()
        .find(|p| p.code == "secured_graphs_read")
        .unwrap();
    assert_eq!(
        read.candidates
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["acme", "vault"]
    );
}

#[test]
fn all_graph_legs_read_the_vector_of_their_own_instance() {
    let w = two_graphs_one_model();
    let plan = w
        .svc
        .search_plan(
            &ctx().scope(umx_services::Scope::All {
                include_named: true,
            }),
            QUERY,
            None,
            10,
        )
        .unwrap();
    let mut queries = plan.embed();
    assert_eq!(w.twin().times(QUERY), 1);
    assert_eq!(
        w.rogue.times(QUERY),
        1,
        "open graph's positive hosted control"
    );
    let graph = &w.svc.loaded("z_open").unwrap().graph;
    let id = graph
        .nodes()
        .find(|n| n.title() == "Tenant roadmap")
        .unwrap()
        .id();
    let exact: Vec<f32> = graph
        .vector(HOSTED, graph.node_ix(id).unwrap(), None)
        .unwrap()
        .iter()
        .map(|v| f32::from(*v))
        .collect();
    queries.insert("twin".into(), Ok(vec![vec![0.0; 8]]));
    queries.insert("rogue".into(), Ok(vec![exact]));
    let found = w.svc.search_finish(plan, queries).unwrap();
    assert!(
        found
            .value
            .hits
            .iter()
            .any(|h| h.alias == "vault" && h.vector.is_none())
    );
    assert!(
        found
            .value
            .hits
            .iter()
            .any(|h| h.alias == "z_open" && h.vector.is_some())
    );
}

#[test]
fn a_search_over_two_graphs_of_one_model_asks_each_graph_its_own_instance() {
    for graphs in ["vault,z_open", "z_open,vault"] {
        let w = two_graphs_one_model();
        let found = w.svc.search(&ctx().graph(graphs), QUERY, None, 10).unwrap();
        assert_eq!(
            w.twin().times(QUERY),
            1,
            "{graphs}: the secured graph's leg asks its local instance"
        );
        assert_eq!(
            w.rogue.times(QUERY),
            1,
            "{graphs}: the hosted instance is asked once, for z_open only"
        );
        assert!(
            found
                .notes
                .iter()
                .all(|p| p.code != "similarity_unavailable"),
            "{graphs}: both legs ran: {:?}",
            found.notes
        );
    }
}

#[test]
fn a_folder_that_forbids_a_graphs_instance_leaves_it_words_only_and_never_borrows_another() {
    let w = two_graphs_one_model();
    let forbid = |dir: &GraphDir, name: &str| !(dir.as_str() == "/work/vault" && name == "twin");
    let plan = w
        .svc
        .search_plan_allowing(&ctx().graph("vault,z_open"), QUERY, None, 10, &forbid)
        .unwrap();
    let queries = plan.embed();
    let found = w.svc.search_finish(plan, queries).unwrap();
    assert_eq!(w.twin().times(QUERY), 0);
    assert_eq!(w.rogue.times(QUERY), 1, "z_open's leg only");
    assert!(
        found
            .notes
            .iter()
            .any(|p| p.code == "similarity_unavailable" && format!("{p:?}").contains("vault")),
        "{:?}",
        found.notes
    );
}

#[test]
fn an_instance_a_graph_stops_admitting_after_the_plan_is_never_asked() {
    let mut w = two_graphs_one_model();
    let plan = w
        .svc
        .search_plan(&ctx().graph("z_open"), QUERY, None, 10)
        .unwrap();
    // Tightened between the plan and the send (takes effect at once).
    w.svc
        .graph_set(&ctx(), "z_open", &[("embedder".into(), "local".into())])
        .unwrap();
    let svc = &w.svc;
    let queries = plan.embed_with(
        &|alias, name| svc.embedding().admits_now(svc.graph(), alias, name),
        &|_, _| panic!("no instance may be asked"),
    );
    assert_eq!(queries.get("rogue"), Some(&Err(QueryFailed::NotAdmitted)));
    assert_eq!(w.rogue.times(QUERY) + w.twin().times(QUERY), 0);
}

#[test]
fn a_query_vector_from_an_instance_no_longer_admitted_is_never_read() {
    let mut w = two_graphs_one_model();
    let plan = w
        .svc
        .search_plan(&ctx().graph("z_open"), QUERY, None, 10)
        .unwrap();
    let queries = plan.embed();
    assert_eq!(w.rogue.times(QUERY), 1);
    // Tightened while the query was out.
    w.svc
        .graph_set(&ctx(), "z_open", &[("embedder".into(), "local".into())])
        .unwrap();
    let found = w.svc.search_finish(plan, queries).unwrap();
    assert!(
        found
            .notes
            .iter()
            .any(|p| p.code == "similarity_unavailable"
                && format!("{p:?}").contains("embedder setting changed")),
        "{:?}",
        found.notes
    );
}
