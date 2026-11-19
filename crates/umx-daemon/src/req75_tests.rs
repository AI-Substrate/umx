//! req-75 and req-37 end to end (ws004 s06): a graph embedded on a teammate's machine with model A is pulled onto a
//! machine configured only for model B. Two real daemons over the real fs ports and config files, each with its own
//! home and one composed instance (A, then B); the graph reaches the second as a pull would leave it (its folder's
//! files copied, a fresh daemon loading them). Every request goes in as wire JSON through `answer`, as the socket's.
//!
//! The umx binary cannot compose a working fake embedder (the config takes only https endpoints and the Azure adapter
//! trusts only the platform's roots; the stub build fakes the graph files too), so this runs in the daemon's crate.
//! The CLI's parsing of these verbs is covered in crates/umx/tests/cli.rs.

use super::*;
use std::collections::BTreeSet;
use std::path::PathBuf;
use umx_core::ports::{Credential, EmbedError, Embedded, Embedder, Locality, ModelKey, TextKind};

const KEY_A: &str = "teammate-model@16";
const KEY_B: &str = "machine-model@32";

/// An embedder whose vectors are bags of words (each word adds 1 to a bucket hashed with a per-model seed, so texts
/// that share words point the same way within one model), and which records every text it is sent.
struct Recorder {
    key: ModelKey,
    seed: u64,
    seen: Mutex<Vec<String>>,
}

impl Recorder {
    fn new(key: &str, seed: u64) -> Arc<Recorder> {
        Arc::new(Recorder {
            key: ModelKey::parse(key).unwrap(),
            seed,
            seen: Mutex::new(Vec::new()),
        })
    }

    fn sent(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    fn saw(&self, needle: &str) -> bool {
        self.seen.lock().unwrap().iter().any(|t| t.contains(needle))
    }

    fn vector(&self, text: &str) -> Vec<f32> {
        let dims = self.key.dims();
        let mut v = vec![0.0f32; dims];
        for word in text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
        {
            let mut h = 0xcbf2_9ce4_8422_2325u64 ^ self.seed;
            for b in word.to_lowercase().bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
            v[(h % dims as u64) as usize] += 1.0;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1.0);
        v.iter_mut().for_each(|x| *x /= norm);
        v
    }
}

impl Embedder for Recorder {
    fn embed(&self, texts: &[String], _kind: TextKind) -> Result<Embedded, EmbedError> {
        self.seen.lock().unwrap().extend(texts.iter().cloned());
        Ok(Embedded {
            vectors: texts.iter().map(|t| self.vector(t)).collect(),
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
        Locality::Local
    }
    fn ready(&self) -> Result<(), EmbedError> {
        Ok(())
    }
    fn credential(&self) -> Option<Credential> {
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

/// One machine: its own home and roots, and a daemon whose only instance is `instance`, also the default.
fn machine(home: &Path, instance: &str, embedder: &Arc<Recorder>) -> Daemon {
    fs::create_dir_all(home).unwrap();
    let config = Config::at(home.join("home"), Some(true)).roots([home.to_str().unwrap()]);
    let (mut services, settings) =
        umx_compose::compose_with_config(umx_compose::Mode::Real, &config);
    services.set_instances(
        vec![umx_services::embedding::Instance {
            name: instance.into(),
            embedder: embedder.clone(),
        }],
        Some(instance),
    );
    Daemon::with_services(
        &config,
        build(),
        services,
        settings,
        umx_compose::Mode::Real,
        umx_compose::Embedders::from_instances(vec![umx_compose::Instance {
            name: instance.into(),
            kind: "local".into(),
            built: Ok(embedder.clone()),
            local: None,
            tokens_per_minute: None,
            host: None,
        }]),
    )
}

/// The request as the CLI sends it: wire JSON, read back by the daemon's own parser.
fn ask(d: &Daemon, op: Op, graph: Option<&str>) -> Envelope {
    let request = Request::op(op, graph.map(str::to_owned), None, build());
    d.answer(request.to_json().as_bytes())
}

fn ok(env: Envelope) -> Envelope {
    assert!(env.errors.is_empty(), "{}", env.to_json());
    env
}

fn add(d: &Daemon, graph: &str, title: &str, note: &str) -> String {
    let env = ok(ask(
        d,
        Op::NodeAdd {
            on_key: None,
            related: None,
            raised_in: None,
            kind: "problem".into(),
            title: title.into(),
            status: None,
            note: Some(note.into()),
            sets: Vec::new(),
            parent: None,
            evidence: Vec::new(),
            evidence_kind: None,
            locator: None,
        },
        Some(graph),
    ));
    env.data["node"]["id"].as_str().unwrap().to_owned()
}

fn new_graph(d: &Daemon, path: &Path, alias: &str, key: &str) {
    ok(ask(
        d,
        Op::GraphNew {
            path: path.display().to_string(),
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

fn pending(d: &Daemon, alias: &str) -> Value {
    let env = ok(ask(
        d,
        Op::GraphList {
            all: false,
            graphfile: None,
        },
        None,
    ));
    env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["alias"] == alias)
        .unwrap_or_else(|| panic!("{alias} not listed: {}", env.to_json()))["vectors_pending"]
        .clone()
}

/// The node's similar_to list: the model it was computed in, and its rows' scores.
fn similar(d: &Daemon, alias: &str, node: &str) -> (Value, Vec<Value>) {
    let env = ok(ask(
        d,
        Op::Neighbours {
            node: node.into(),
            relation: Some("similar_to".into()),
            direction: None,
            limit: None,
            offset: None,
        },
        Some(alias),
    ));
    let scores = env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["score"].clone())
        .collect();
    (env.data["similarity"]["model"].clone(), scores)
}

fn search(d: &Daemon, text: &str, graphs: &str) -> Envelope {
    ok(ask(
        d,
        Op::Search {
            text: text.into(),
            within: None,
            limit: None,
            along: None,
        },
        Some(graphs),
    ))
}

fn reembed(d: &Daemon, alias: &str, dry_run: bool) -> Envelope {
    ok(ask(
        d,
        Op::EmbeddingsReembed {
            dry_run,
            model: Some(KEY_B.into()),
        },
        Some(alias),
    ))
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(30);
    while !check() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn mismatches(env: &Envelope) -> Vec<&Issue> {
    env.warnings
        .iter()
        .filter(|i| i.code() == "embedder_mismatch")
        .collect()
}

/// Rows the vector leg found, by graph.
fn meaning_rows(env: &Envelope, graph: &str) -> usize {
    env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["graph"] == graph && matches!(r["legs"].as_str(), Some("vector" | "both")))
        .count()
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Every file under `dir` with its bytes.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let bytes = fs::read(&p).unwrap();
                out.push((p.strip_prefix(dir).unwrap().to_path_buf(), bytes));
            }
        }
    }
    out.sort();
    out
}

/// A model's vector lines in the graph folder: (node id, field) each.
fn vector_lines(graph: &Path, key: &str) -> Vec<(String, String)> {
    let Ok(entries) = fs::read_dir(graph.join("embeddings").join(key)) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for p in entries.map(|e| e.unwrap().path()) {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if name.ends_with(".tsv") && !name.starts_with(".umx-tmp-") {
            for line in fs::read_to_string(&p).unwrap().lines() {
                let mut cols = line.split('\t');
                if let (Some(id), Some(field)) = (cols.next(), cols.next()) {
                    lines.push((id.to_owned(), field.to_owned()));
                }
            }
        }
    }
    lines.sort();
    lines
}

fn recorded_key(graph: &Path) -> Value {
    let text = fs::read_to_string(graph.join("graph.json")).unwrap();
    umx_protocol::parse_json(&text).unwrap()["embedding"]["key"].clone()
}

#[test]
fn a_graph_pulled_from_another_model_keeps_working_and_reembeds_only_when_asked() {
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("umx-req75-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    // The teammate's machine, configured for model A: every node gets A vectors and a similar_to list, on disk.
    let a = Recorder::new(KEY_A, 1);
    let theirs = root.join("teammate");
    let team_dir = theirs.join("g");
    let teammate = machine(&theirs, "team", &a);
    new_graph(&teammate, &team_dir, "team", KEY_A);
    let team_nodes = [
        (
            "Approval delay in vendor onboarding",
            "vendor approval waits on legal review",
        ),
        (
            "Vendor onboarding checklist",
            "the checklist covers vendor approval and legal review",
        ),
        (
            "Legal review backlog",
            "legal review queue grows when vendor approval stalls",
        ),
        (
            "Badge printer jams",
            "the lobby badge printer jams every morning",
        ),
    ];
    let ids: Vec<String> = team_nodes
        .iter()
        .map(|(t, n)| add(&teammate, "team", t, n))
        .collect();
    eventually("the teammate's vectors", || pending(&teammate, "team") == 0);
    eventually("the teammate's similar_to lists", || {
        !similar(&teammate, "team", &ids[0]).1.is_empty()
    });
    ok(teammate.answer(
        Request::new("daemon stop", Vec::new(), build())
            .to_json()
            .as_bytes(),
    ));
    assert_eq!(recorded_key(&team_dir), KEY_A);
    assert!(!vector_lines(&team_dir, KEY_A).is_empty());

    // This machine, configured only for model B. The graph arrives as a pull leaves it: its files, nothing else.
    let b = Recorder::new(KEY_B, 2);
    let ours = root.join("mine");
    let pulled = ours.join("g");
    copy_dir(&team_dir, &pulled);
    let a_folder = pulled.join("embeddings").join(KEY_A);
    let a_before = snapshot(&a_folder);
    let mine = machine(&ours, "mine", &b);

    // `graph load` warns embedder_mismatch naming both models and the two ways out.
    let load = ok(ask(
        &mine,
        Op::GraphLoad {
            path: pulled.display().to_string(),
            as_alias: None,
            by_alias: None,
            graphfile: false,
            set: None,
        },
        None,
    ));
    let m = mismatches(&load);
    assert_eq!(m.len(), 1, "{}", load.to_json());
    let text = m[0].message();
    assert!(
        text.starts_with("Graph team ") && text.contains(KEY_A) && text.contains(KEY_B),
        "{text}"
    );
    assert!(
        text.contains("configure an embedder for teammate-model@16 (umx docs get config)"),
        "{text}"
    );
    assert!(
        text.contains("re-embed the graph with machine-model@32"),
        "{text}"
    );
    assert_eq!(m[0].next(), "umx embeddings reembed --graph team --dry-run");
    assert_eq!(
        load.data["graph"]["vectors_pending"],
        0,
        "the A vectors are current: {}",
        load.to_json()
    );

    // Similarity on the committed vectors still works: similar_to needs no embedder. Search has no A instance to
    // embed its query with, so it names the mismatch, finds by words and sends nothing.
    let (model, scores) = similar(&mine, "team", &ids[0]);
    assert_eq!(model, KEY_A);
    assert!(
        !scores.is_empty() && scores.iter().all(Value::is_number),
        "{scores:?}"
    );
    let found = search(&mine, "vendor approval", "team");
    let m = mismatches(&found);
    assert_eq!(m.len(), 1, "{}", found.to_json());
    assert!(m[0].message().contains(KEY_A) && m[0].message().contains(KEY_B));
    assert!(
        !found.data["rows"].as_array().unwrap().is_empty(),
        "{}",
        found.to_json()
    );
    assert_eq!(meaning_rows(&found, "team"), 0, "{}", found.to_json());
    assert_eq!(b.sent(), 0, "nothing goes to B");

    // req-37: beside it a graph of this machine's model. A search across both names the mismatch and the graph left
    // out of meaning, and the other graph still has meaning.
    let local_dir = ours.join("m");
    new_graph(&mine, &local_dir, "local", KEY_B);
    add(
        &mine,
        "local",
        "Vendor approval policy",
        "vendor approval needs two signatures",
    );
    eventually("this machine's vectors", || pending(&mine, "local") == 0);
    let both = search(&mine, "vendor approval", "team,local");
    let m = mismatches(&both);
    assert_eq!(m.len(), 1, "{}", both.to_json());
    assert!(
        m[0].message().starts_with("Graph team "),
        "{}",
        m[0].message()
    );
    assert!(m[0].message().contains(KEY_A) && m[0].message().contains(KEY_B));
    assert!(meaning_rows(&both, "local") > 0, "{}", both.to_json());
    assert_eq!(meaning_rows(&both, "team"), 0, "{}", both.to_json());

    // A node added to the pulled graph stays pending, never made with B. A later B node landing shows the worker ran
    // past it.
    add(
        &mine,
        "team",
        "Shredder pickup missed",
        "the shredder pickup was missed on friday",
    );
    add(
        &mine,
        "local",
        "Signature collection",
        "collect two signatures for vendor approval",
    );
    eventually("the later B node", || pending(&mine, "local") == 0);
    assert_eq!(pending(&mine, "team"), 1);
    for title in team_nodes
        .iter()
        .map(|(t, _)| *t)
        .chain(["Shredder pickup missed"])
    {
        assert!(!b.saw(title), "B was sent {title:?}");
    }
    assert!(
        vector_lines(&pulled, KEY_B).is_empty(),
        "no B vector in the pulled graph"
    );
    assert_eq!(snapshot(&a_folder), a_before, "the A vectors are untouched");

    // --dry-run prints the vector count and the cost, calls no embedder and changes nothing.
    let sent = b.sent();
    let dry = reembed(&mine, "team", true);
    let d = &dry.data;
    assert_eq!(
        (d["from"].as_str(), d["to"].as_str()),
        (Some(KEY_A), Some(KEY_B)),
        "{}",
        dry.to_json()
    );
    assert_eq!(
        (d["nodes"].as_u64(), d["dry_run"].as_bool()),
        (Some(5), Some(true)),
        "{}",
        dry.to_json()
    );
    let texts = d["texts"].as_u64().unwrap();
    assert!(
        texts >= 15 && d["estimated_tokens"].as_u64().unwrap() > 0,
        "{}",
        dry.to_json()
    );
    assert_eq!(b.sent(), sent, "the dry run called an embedder");
    assert_eq!(recorded_key(&pulled), KEY_A);
    assert_eq!(pending(&mine, "team"), 1);

    // Without it: graph.json records B and every vector is made again under B; A's folder stays as it was.
    let apply = reembed(&mine, "team", false);
    assert_eq!(
        apply.data["queued"].as_u64(),
        Some(5),
        "{}",
        apply.to_json()
    );
    assert_eq!(recorded_key(&pulled), KEY_B);
    eventually("the reembed", || pending(&mine, "team") == 0);
    let b_lines = vector_lines(&pulled, KEY_B);
    assert_eq!(
        b_lines.len() as u64,
        texts,
        "every vector the dry run counted: {b_lines:?}"
    );
    let b_nodes: BTreeSet<&str> = b_lines.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(b_nodes.len(), 5, "{b_lines:?}");
    assert_eq!(snapshot(&a_folder), a_before, "models are never mixed");
    assert!(
        b.saw("Badge printer jams"),
        "the instrument sees a positive"
    );

    // Afterwards search and similar_to work under B and nothing is pending.
    let found = search(&mine, "vendor approval", "team");
    assert!(mismatches(&found).is_empty(), "{}", found.to_json());
    assert!(meaning_rows(&found, "team") > 0, "{}", found.to_json());
    eventually("similar_to lists under B", || {
        let (model, scores) = similar(&mine, "team", &ids[0]);
        model == KEY_B && !scores.is_empty() && pulled.join("similar").join(KEY_B).is_dir()
    });
    assert_eq!(pending(&mine, "team"), 0);
    let _ = fs::remove_dir_all(&root);
}
