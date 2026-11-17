//! ws004 s28 (req-123, D80) end to end through the daemon: node add and batch reply with their related nodes after the
//! write, the floor that cut them, and a `related_skipped` hint whenever the check does not run; the call's flag, the
//! machine's `related` setting and a dry run each send nothing.

use super::*;
use std::sync::atomic::AtomicUsize;
use umx_core::ports::{Credential, EmbedError, Embedded, Embedder, Locality, ModelKey, TextKind};
use umx_core::similarity::Floors;
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

const SETTLE: Duration = Duration::from_secs(120);

/// One axis per concept word, so a paraphrase lands where its meaning is; counts query calls.
struct Concepts {
    queries: AtomicUsize,
}

fn vector_of(text: &str) -> Vec<f32> {
    let mut v = vec![0.0f32; 4];
    for w in text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        match w {
            "cat" | "kitten" | "feline" => v[0] += 1.0,
            "dog" | "puppy" => v[1] += 1.0,
            "approval" | "delay" => v[2] += 1.0,
            _ => {}
        }
    }
    v
}

impl Embedder for Concepts {
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
        if kind == TextKind::Query {
            self.queries.fetch_add(1, Ordering::SeqCst);
        }
        Ok(Embedded {
            vectors: texts.iter().map(|t| vector_of(t)).collect(),
            truncated: Vec::new(),
            tokens: None,
        })
    }
    fn key(&self) -> ModelKey {
        ModelKey::parse("concepts@4").unwrap()
    }
    fn concurrency_ceiling(&self) -> usize {
        1
    }
    fn max_input_tokens(&self) -> usize {
        512
    }
    fn locality(&self) -> Locality {
        Locality::Local
    }
    fn ready(&self) -> Result<(), EmbedError> {
        Ok(())
    }
    fn credential(&self) -> Option<Credential> {
        None
    }
    fn adapter_id(&self) -> &'static str {
        "test.concepts"
    }
}

fn root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("umx-s28-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

/// A daemon whose one embedder is `e`, with a similar_to floor of 0.5 in the config for its key (no query prompt:
/// the similar_to floor applies). Uncalibrated test key; the floor is not tuned.
fn daemon(root: &std::path::Path, e: &Arc<Concepts>) -> Daemon {
    let config = Config::at(root.join("home"), Some(true)).roots([root.to_str().unwrap()]);
    let (mut services, settings) =
        umx_compose::compose_with_config(umx_compose::Mode::Real, &config);
    let mut floors = Floors::default();
    floors.set(&e.key(), None, Some(0.5));
    services.set_floors(floors);
    Daemon::with_services(
        &config,
        build(),
        services.with_embedder(e.clone()),
        settings,
        umx_compose::Mode::Real,
        umx_compose::Embedders::from_instances(Vec::new()),
    )
}

fn ok(env: Envelope) -> Envelope {
    assert!(env.errors.is_empty(), "{}", env.to_json());
    env
}

fn add(d: &Daemon, title: &str, related: Option<bool>) -> Envelope {
    ok(d.handle(&op(Op::NodeAdd {
        related,
        on_key: None,
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
    })))
}

fn batch(d: &Daemon, titles: &[&str], dry_run: bool, related: Option<bool>) -> Envelope {
    let lines = titles
        .iter()
        .enumerate()
        .map(|(i, t)| BatchLine {
            line: i as u32 + 1,
            op: serde_json::from_value::<BatchOp>(
                json!({"op": "node add", "type": "problem", "title": t}),
            )
            .unwrap(),
        })
        .collect();
    ok(d.handle(&op(Op::Batch {
        idempotency_key: None,
        related,
        lines,
        dry_run,
        echo_sentences: false,
        refs: Default::default(),
        refs_graph: None,
        reason: None,
        raised_in: None,
    })))
}

/// The `related_skipped` hints' reasons and next commands.
fn skips(env: &Envelope) -> Vec<(String, String)> {
    let v = serde_json::to_value(env).unwrap();
    v["hints"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|h| h["code"] == "related_skipped")
        .map(|h| {
            (
                h["message"].as_str().unwrap().to_string(),
                h["next"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn titles(related: &Value) -> Vec<String> {
    related
        .as_array()
        .unwrap_or_else(|| panic!("related rows: {related}"))
        .iter()
        .map(|r| r["title"].as_str().unwrap().to_string())
        .collect()
}

/// A graph with a cat and a dog node, both embedded.
fn demo(name: &str) -> (Daemon, Arc<Concepts>) {
    let root = root(name);
    let e = Arc::new(Concepts {
        queries: AtomicUsize::new(0),
    });
    let d = daemon(&root, &e);
    ok(d.handle(&op(Op::GraphNew {
        path: root.join("g").display().to_string(),
        alias: "demo".into(),
        title: None,
        empty: false,
        embedding_model: None,
        secured: false,
        no_graphfile: false,
        graphfile: false,
    })));
    add(&d, "Cat food", Some(false));
    add(&d, "Dog walking", Some(false));
    let until = Instant::now() + SETTLE;
    while d
        .services
        .read()
        .unwrap()
        .loaded("demo")
        .and_then(|l| l.pending_count())
        != Some(0)
    {
        assert!(Instant::now() < until, "timed out waiting for vectors");
        std::thread::sleep(Duration::from_millis(20));
    }
    (d, e)
}

#[test]
fn node_add_replies_with_its_related_nodes_and_the_floor_after_the_write() {
    let (d, e) = demo("add");
    let before = e.queries.load(Ordering::SeqCst);
    let env = add(&d, "Feline kitten", None);
    assert_eq!(e.queries.load(Ordering::SeqCst), before + 1);
    assert_eq!(titles(&env.data["related"]), ["Cat food"]);
    let row = &env.data["related"][0];
    assert!(row["score"].as_f64().unwrap() > 0.99, "{row}");
    assert!(row["address"].as_str().unwrap().starts_with("umx:demo/"));
    assert_eq!(
        env.data["related_floor"],
        json!({"value": 0.5, "source": "config", "use": "similar_to"})
    );
    assert!(skips(&env).is_empty(), "{}", env.to_json());
}

#[test]
fn no_related_sends_nothing_and_the_hint_says_how_to_turn_it_on() {
    let (d, e) = demo("off");
    let before = e.queries.load(Ordering::SeqCst);
    let env = add(&d, "Feline kitten", Some(false));
    assert_eq!(e.queries.load(Ordering::SeqCst), before, "no embed call");
    assert_eq!(env.data["related"], Value::Null);
    assert_eq!(env.data["related_floor"], Value::Null);
    let skips = skips(&env);
    assert_eq!(skips.len(), 1, "{skips:?}");
    assert!(
        skips[0].0.contains("--no-related turned it off"),
        "{skips:?}"
    );
    assert_eq!(
        skips[0].1,
        "umx node add --type <type> --title <title> --graph demo --related"
    );
}

#[test]
fn the_machine_setting_turns_it_off_until_the_flag_turns_it_on() {
    let (d, e) = demo("machine");
    ok(d.handle(&op(Op::ConfigSet {
        key: "related".into(),
        value: "off".into(),
        repo: false,
        cwd: None,
    })));
    let before = e.queries.load(Ordering::SeqCst);
    let env = add(&d, "Kitten", None);
    assert_eq!(e.queries.load(Ordering::SeqCst), before);
    assert_eq!(skips(&env)[0].1, "umx config set related on");
    let env = add(&d, "Feline", Some(true));
    // Kitten may have its vector by now too; the cat node always.
    assert!(titles(&env.data["related"]).contains(&"Cat food".to_string()));
}

#[test]
fn an_applied_batch_lists_each_added_node_s_related_nodes_but_a_dry_run_checks_nothing() {
    let (d, e) = demo("batch");
    let before = e.queries.load(Ordering::SeqCst);
    let env = batch(&d, &["Kitten", "Puppy"], true, None);
    assert_eq!(
        e.queries.load(Ordering::SeqCst),
        before,
        "a dry run sends nothing"
    );
    assert!(skips(&env).is_empty());
    let env = batch(&d, &["Kitten", "Feline", "Puppy"], false, None);
    assert_eq!(e.queries.load(Ordering::SeqCst), before + 1, "one request");
    let rows = env.data["rows"].as_array().unwrap();
    // Kitten and Feline never list each other: the same call added both.
    assert_eq!(titles(&rows[0]["related"]), ["Cat food"]);
    assert_eq!(titles(&rows[1]["related"]), ["Cat food"]);
    assert_eq!(titles(&rows[2]["related"]), ["Dog walking"]);
    assert!(
        env.data["columns"]
            .as_array()
            .unwrap()
            .contains(&json!("related"))
    );
    assert_eq!(env.data["related_floor"]["use"], "similar_to");
    let env = batch(&d, &["Cat"], false, Some(false));
    assert_eq!(
        skips(&env)[0].1,
        "umx batch --file <file> --graph demo --related"
    );
}

/// A node add that `--on-key resolve` resolves to a node already there adds nothing new: no related check, no hint
/// (ws003 s08 with req-123).
#[test]
fn an_add_resolved_by_its_entity_key_is_not_checked() {
    let (d, e) = demo("on-key");
    let keyed = |title: &str, on_key: Option<umx_protocol::OnKey>| {
        ok(d.handle(&op(Op::NodeAdd {
            related: None,
            on_key,
            raised_in: None,
            kind: "problem".into(),
            title: title.into(),
            status: None,
            note: None,
            sets: vec![umx_protocol::SetArg {
                field: "entity_key".into(),
                value: umx_protocol::SetValue::Raw("ticket:cat-1".into()),
            }],
            parent: None,
            evidence: Vec::new(),
            evidence_kind: None,
            locator: None,
        })))
    };
    keyed("Kitten", None);
    let before = e.queries.load(Ordering::SeqCst);
    let env = keyed("Feline kitten", Some(umx_protocol::OnKey::Resolve));
    assert_eq!(env.data["resolved"], json!(true), "{}", env.to_json());
    assert_eq!(e.queries.load(Ordering::SeqCst), before, "no embed call");
    assert_eq!(env.data["related"], Value::Null);
    assert!(skips(&env).is_empty(), "{}", env.to_json());
}
