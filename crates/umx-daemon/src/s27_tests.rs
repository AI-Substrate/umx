//! ws004 s27 through the daemon: `umx embeddings probe` builds an instance added or changed after start from the
//! config as it is now (a fake stands in for compose's builder), and `umx embeddings status` reports each graph's
//! vectors and instance without calling the provider.

use super::*;
use std::path::PathBuf;
use umx_core::ports::{Embedder, Locality, ModelKey};
use umx_fakes::FakeEmbedder;
use umx_protocol::Status;

fn build() -> Build {
    Build {
        version: "t".into(),
        commit: "t".into(),
        built: "t".into(),
    }
}

fn ask(d: &Daemon, op: Op, graph: Option<&str>) -> Envelope {
    let request = Request::op(op, graph.map(str::to_owned), None, build());
    d.answer(request.to_json().as_bytes())
}

fn ok(env: Envelope) -> Envelope {
    assert!(env.errors.is_empty(), "{}", env.to_json());
    env
}

fn root(name: &str) -> PathBuf {
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("umx-s27-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("home")).unwrap();
    root
}

fn fake(key: &str) -> Arc<FakeEmbedder> {
    Arc::new(FakeEmbedder::new(ModelKey::parse(key).unwrap()))
}

fn hosted(key: &str) -> Arc<FakeEmbedder> {
    Arc::new(FakeEmbedder::new(ModelKey::parse(key).unwrap()).with_locality(Locality::Hosted))
}

/// A fake API key, built at run time so no key-shaped literal sits in the source.
fn fake_key(seed: &str) -> String {
    let mut k = String::from("AKIA");
    while k.len() < 20 {
        k.push_str(seed);
    }
    k.truncate(20);
    k
}

fn new_graph(d: &Daemon, root: &Path, alias: &str, key: &str) {
    ok(ask(
        d,
        Op::GraphNew {
            path: root.join(alias).display().to_string(),
            alias: alias.into(),
            title: None,
            empty: false,
            embedding_model: Some(key.into()),
            secured: false,
            no_graphfile: false,
            graphfile: false,
            no_std: false,
        },
        None,
    ));
}

fn add(d: &Daemon, alias: &str, title: &str, note: Option<String>) {
    ok(ask(
        d,
        Op::NodeAdd {
            on_key: None,
            related: None,
            raised_in: None,
            kind: "problem".into(),
            title: title.into(),
            status: None,
            note,
            sets: Vec::new(),
            parent: None,
            evidence: Vec::new(),
            evidence_kind: None,
            locator: None,
        },
        Some(alias),
    ));
}

fn row(env: &Envelope, alias: &str) -> Value {
    env.data["graphs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["graph"] == alias)
        .cloned()
        .unwrap_or_else(|| panic!("{alias} not in {}", env.to_json()))
}

/// Polls status until `alias`'s row passes `done`.
fn status_until(d: &Daemon, alias: &str, what: &str, done: impl Fn(&Value) -> bool) -> Value {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let env = ok(ask(d, Op::EmbeddingsStatus, None));
        let r = row(&env, alias);
        if done(&r) {
            return r;
        }
        assert!(Instant::now() < until, "{what}: {}", env.to_json());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn instance(name: &str, e: &Arc<FakeEmbedder>) -> umx_compose::Instance {
    umx_compose::Instance {
        name: name.into(),
        kind: "local".into(),
        built: Ok(Arc::clone(e) as Arc<dyn Embedder>),
        local: None,
        tokens_per_minute: None,
        host: None,
    }
}

/// A local instance's machine config lines.
fn lines(name: &str, model: &str, dims: u32) -> String {
    format!(
        "embedders.{name}.kind = \"local\"\nembedders.{name}.model = \"{model}\"\nembedders.{name}.dims = {dims}\n"
    )
}

/// A daemon whose machine config is `config` at start, with `running` built then (also the default), and whose
/// one-off builder returns `now` for its name.
fn daemon(
    root: &Path,
    config: &str,
    running: Option<(&str, &Arc<FakeEmbedder>)>,
    now: Vec<(&'static str, Arc<FakeEmbedder>)>,
) -> Daemon {
    let file = root.join("home/config.toml");
    fs::write(&file, config).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    let cfg = Config::at(root.join("home"), Some(true)).roots([root.to_str().unwrap()]);
    let (mut services, settings) = umx_compose::compose_with_config(umx_compose::Mode::Real, &cfg);
    if let Some((name, e)) = running {
        services.set_instances(
            vec![umx_services::embedding::Instance {
                name: name.into(),
                embedder: Arc::clone(e) as Arc<dyn Embedder>,
            }],
            Some(name),
        );
    }
    let embedders = umx_compose::Embedders::from_instances(
        running.map(|(n, e)| instance(n, e)).into_iter().collect(),
    );
    let mut d = Daemon::with_services(
        &cfg,
        build(),
        services,
        settings,
        umx_compose::Mode::Real,
        embedders,
    );
    d.build_now = Arc::new(move |name: &str| {
        now.iter()
            .find(|(n, _)| *n == name)
            .map(|(n, e)| instance(n, e))
    });
    d
}

fn set(d: &Daemon, key: &str, value: &str) {
    ok(ask(
        d,
        Op::ConfigSet {
            key: key.into(),
            value: value.into(),
            repo: false,
            cwd: None,
        },
        None,
    ));
}

fn probe(d: &Daemon, embedder: &str) -> Envelope {
    ask(
        d,
        Op::EmbeddingsProbe {
            embedder: Some(embedder.into()),
        },
        None,
    )
}

fn not_in_use(env: &Envelope) -> Option<&Issue> {
    env.warnings
        .iter()
        .find(|w| w.code() == "embedder_not_in_use")
}

#[test]
fn a_probe_builds_an_instance_added_after_start_without_a_restart() {
    let root = root("added");
    let later = fake("later-model@8");
    let d = daemon(&root, "", None, vec![("later", Arc::clone(&later))]);
    for (k, v) in [("kind", "local"), ("model", "later-model"), ("dims", "8")] {
        set(&d, &format!("embedders.later.{k}"), v);
    }
    let env = ok(probe(&d, "later"));
    assert_eq!(env.data["embedder"], "later");
    assert_eq!(env.data["key"], "later-model@8");
    let w = not_in_use(&env).unwrap_or_else(|| panic!("{}", env.to_json()));
    assert!(w.message().contains("added"), "{}", w.message());
    assert_eq!(w.next(), "umx daemon restart");
    // Only umx's fixed sentence went out, and the running daemon still has no embedder.
    assert_eq!(later.texts_embedded(), 1);
    assert!(d.embedders.get("later").is_none());
}

#[test]
fn a_probe_of_a_changed_instance_uses_the_config_now_and_an_unchanged_one_the_running() {
    let root = root("changed");
    let running = fake("a-model@4");
    let changed = fake("a-model@16");
    let d = daemon(
        &root,
        &format!("embedder.active = \"a\"\n{}", lines("a", "a-model", 4)),
        Some(("a", &running)),
        vec![("a", Arc::clone(&changed))],
    );
    let env = ok(probe(&d, "a"));
    assert_eq!(env.status, Status::Ok, "{}", env.to_json());
    assert_eq!(env.data["key"], "a-model@4");
    assert_eq!((running.texts_embedded(), changed.texts_embedded()), (1, 0));

    set(&d, "embedders.a.dims", "16");
    let env = ok(probe(&d, "a"));
    assert_eq!(env.data["key"], "a-model@16");
    let w = not_in_use(&env).unwrap_or_else(|| panic!("{}", env.to_json()));
    assert!(w.message().contains("changed"), "{}", w.message());
    assert_eq!((running.texts_embedded(), changed.texts_embedded()), (1, 1));
}

#[test]
fn status_reports_each_graph_with_no_provider_call() {
    let root = root("status");
    let a = fake("a-model@4");
    let d = daemon(
        &root,
        &format!("embedder.active = \"a\"\n{}", lines("a", "a-model", 4)),
        Some(("a", &a)),
        Vec::new(),
    );
    for (alias, key) in [("mine", "a-model@4"), ("other", "b-model@4")] {
        new_graph(&d, &root, alias, key);
        for title in ["Approval delays", "Vendor onboarding"] {
            add(&d, alias, title, None);
        }
    }
    status_until(&d, "mine", "mine never embedded", |r| {
        r["vectors_pending"] == 0
    });
    let calls = a.calls();
    let env = ok(ask(&d, Op::EmbeddingsStatus, None));
    assert_eq!(a.calls(), calls, "status called the provider");
    let mine = row(&env, "mine");
    assert_eq!(
        (
            &mine["embedder"],
            &mine["locality"],
            &mine["key"],
            &mine["state"]
        ),
        (
            &json!("a"),
            &json!("local"),
            &json!("a-model@4"),
            &json!("current")
        )
    );
    assert_eq!(
        (&mine["nodes"], &mine["vectors_held"], &mine["vectors_done"]),
        (&json!(2), &json!(0), &json!(2))
    );
    assert_eq!(mine["check"], "umx embeddings probe --graph mine");
    let other = row(&env, "other");
    assert_eq!(
        (&other["embedder"], &other["state"]),
        (&json!(null), &json!("no_embedder"))
    );
}

/// R177-1: positive held and pending counts, each reported as such. The node whose note holds a key-shaped text is
/// held from the hosted instance by the secret hold (not pending); a plain node added while the instance is offline
/// stays pending.
#[test]
fn status_counts_pending_and_held_nodes() {
    let root = root("held");
    let h = hosted("h-model@4");
    let d = daemon(
        &root,
        &format!("embedder.active = \"h\"\n{}", lines("h", "h-model", 4)),
        Some(("h", &h)),
        Vec::new(),
    );
    new_graph(&d, &root, "mine", "h-model@4");
    add(
        &d,
        "mine",
        "Export fails",
        Some(format!("it uses {} for the bucket", fake_key("Q7Z3X9K2"))),
    );
    let r = status_until(&d, "mine", "the held node never showed", |r| {
        r["vectors_pending"] == 0 && r["vectors_held"] == 1
    });
    assert_eq!(
        (&r["vectors_done"], &r["state"], &r["locality"]),
        (&json!(0), &json!("held"), &json!("hosted"))
    );
    h.set_offline(true);
    add(&d, "mine", "Approval delays", None);
    let r = status_until(&d, "mine", "the pending node never showed", |r| {
        r["vectors_pending"] == 1
    });
    assert_eq!(
        (&r["vectors_held"], &r["vectors_done"], &r["state"]),
        (&json!(1), &json!(0), &json!("embedding"))
    );
}

/// R177-1: a graph whose shards could not be read at load (`vectors_unreadable`) has unknown counts, and status says
/// `unknown`, never `current`. The shard folder holds a folder where a shard file should be (the services fixture
/// a_shard_scan_failure_is_not_a_clean_successful_load uses the same failure).
#[test]
fn status_says_unknown_when_the_shards_could_not_be_read() {
    let root = root("unknown");
    let a = fake("a-model@4");
    a.set_offline(true);
    let d = daemon(
        &root,
        &format!("embedder.active = \"a\"\n{}", lines("a", "a-model", 4)),
        Some(("a", &a)),
        Vec::new(),
    );
    new_graph(&d, &root, "mine", "a-model@4");
    add(&d, "mine", "Approval delays", None);
    // The instrument: before the failure the same graph reports a known count.
    let before = status_until(&d, "mine", "pending never showed", |r| {
        r["vectors_pending"] == 1
    });
    assert_eq!(before["state"], "embedding");
    ok(ask(
        &d,
        Op::GraphUnload {
            alias: "mine".into(),
        },
        None,
    ));
    fs::create_dir_all(root.join("mine/embeddings/a-model@4/00.tsv")).unwrap();
    let loaded = ask(
        &d,
        Op::GraphLoad {
            path: root.join("mine").display().to_string(),
            as_alias: None,
            by_alias: None,
            graphfile: false,
            set: None,
        },
        None,
    );
    assert!(
        loaded
            .warnings
            .iter()
            .any(|w| w.code() == "vectors_unreadable"),
        "{}",
        loaded.to_json()
    );
    let env = ok(ask(&d, Op::EmbeddingsStatus, None));
    let r = row(&env, "mine");
    assert_eq!(
        (
            &r["embedder"],
            &r["vectors_pending"],
            &r["vectors_done"],
            &r["state"]
        ),
        (&json!("a"), &json!(null), &json!(null), &json!("unknown"))
    );
}
