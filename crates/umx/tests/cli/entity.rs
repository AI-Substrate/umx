//! The cli tests: entity keys, links, denials and the entity reads.
//! A module of cli.rs, whose `Home` and helpers it uses, in its own file so no file nears the seed gate's
//! size cap.

use super::*;

#[test]
fn identity_is_resolved_once_per_write_or_batch_and_never_for_content_reads() {
    let mut home = Home::new("actor-capture");
    let path = home.graph();
    let bin = home.path.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let pij = bin.join("pij-rs");
    fs::write(&pij, r#"#!/bin/sh
printf '%s\n' "$*" >> "$(dirname "$0")/identity-calls"
case "$2" in
  whoami) printf '%s\n' '{"ok":true,"data":{"id":"pij-test-writer","harness":"copilot","session":"CapturedSession_01","role":"coder"}}' ;;
  state) printf '%s\n' '{"ok":true,"data":{"sessionStatus":{"status":{"model":{"value":"safe-model"}}}}}' ;;
  *) exit 64 ;;
esac
"#).unwrap();
    fs::set_permissions(&pij, fs::Permissions::from_mode(0o700)).unwrap();
    let (added, exit) = home.umx(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Captured writer",
    ]);
    assert_eq!(exit, 0, "{}", added.to_json());
    assert_eq!(added.data["actor_tier"], "pij");
    let id = added.data["node"]["id"].as_str().unwrap();
    let calls = || fs::read_to_string(bin.join("identity-calls")).unwrap();
    assert_eq!(
        calls().lines().collect::<Vec<_>>(),
        ["--json whoami", "--json state pij-test-writer"]
    );
    for args in [
        vec!["node", "get", id],
        vec!["node", "list"],
        vec!["node", "find", "Captured"],
        vec!["search", "Captured"],
        vec!["resolve", id],
    ] {
        let (_, exit) = home.umx(&args);
        assert_eq!(exit, 0);
    }
    assert_eq!(
        calls().lines().count(),
        2,
        "reads must not start identity tools"
    );
    let file = home.ops_file(
        "{\"op\":\"node add\",\"type\":\"problem\",\"title\":\"Batch one\"}\n\
         {\"op\":\"node add\",\"type\":\"option\",\"title\":\"Batch two\"}\n",
    );
    let (batch, exit) = home.umx(&["batch", "--file", &file]);
    assert_eq!(exit, 0, "{}", batch.to_json());
    assert_eq!(batch.data["actor_tier"], "pij");
    assert_eq!(
        calls().lines().collect::<Vec<_>>(),
        [
            "--json whoami",
            "--json state pij-test-writer",
            "--json whoami",
            "--json state pij-test-writer"
        ]
    );
    let target = batch.data["rows"][0]["id"].as_str().unwrap();
    let (edge, exit) = home.umx(&["edge", "add", id, "supports", target]);
    assert_eq!(exit, 0, "{}", edge.to_json());
    let edge_id = edge_id(&edge);
    let (edited, exit) = home.umx(&[
        "edge",
        "edit",
        &edge_id,
        "--reverse",
        "--by",
        "not-the-actor",
    ]);
    assert_eq!(exit, 0, "{}", edited.to_json());
    assert_eq!(edited.data["actor_tier"], "pij");
    assert_eq!(edited.data["edge"]["updated_by"], "pij-test-writer");
    assert_eq!(calls().lines().count(), 8, "one whoami and state per write");
    let nodes = fs::read_to_string(Path::new(&path).join("nodes.jsonl")).unwrap();
    let records = umx_core::codec::decode_nodes(nodes.as_bytes())
        .unwrap()
        .records;
    let session = records.iter().find(|n| n.kind == "session").unwrap();
    assert_eq!(
        session.fields["prov_key"],
        json!(format!(
            "session/copilot/{}",
            umx_core::provenance::hash_session_id("CapturedSession_01").unwrap()
        ))
    );
    assert_eq!(
        edited.data["edge"]["edited_by_session"],
        session.id.to_text()
    );
    assert_eq!(
        edited.data["edge"]["authored_by_session"],
        edge.data["edge"]["authored_by_session"]
    );
    let edges =
        umx_core::codec::decode_edges(&fs::read(Path::new(&path).join("edges.jsonl")).unwrap())
            .unwrap()
            .records;
    let linked = edges
        .iter()
        .find(|e| e.from == session.id && e.kind == "session_of")
        .unwrap();
    let agent = records.iter().find(|n| n.id == linked.to).unwrap();
    assert_eq!(agent.fields["prov_key"], "agent/pij/pij-test-writer");
    for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
        assert!(
            !fs::read_to_string(Path::new(&path).join(file))
                .unwrap()
                .contains("CapturedSession_01")
        );
    }
    assert!(!calls().contains("CapturedSession_01"));
}

#[test]
#[cfg(feature = "test-driver")]
fn the_shipped_binary_captures_identity_with_production_budgets() {
    let mut home = Home::new("actor-shipped");
    home.binary = env!("CARGO_BIN_EXE_umx");
    home.graph();
    let pij = home.path.join("bin/pij-rs");
    fs::create_dir_all(pij.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_umx-fake-pij"), &pij).unwrap();
    let added = home.ok(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Fast identity",
    ]);
    assert_eq!(added.data["actor_tier"], "pij", "{}", added.to_json());
}

/// Plan 188 item 6 (field report): a pij capture that overruns its budget falls back a tier and the write succeeds; the
/// helper it leaves is cleaned up without the old interim "tool cleanup pending" line on stderr.
#[test]
#[cfg(feature = "test-driver")]
fn a_slow_identity_helper_falls_back_and_prints_no_cleanup_line() {
    let mut home = Home::new("actor-slow");
    home.binary = env!("CARGO_BIN_EXE_umx");
    home.graph();
    let pij = home.path.join("bin/pij-rs");
    fs::create_dir_all(pij.parent().unwrap()).unwrap();
    fs::write(&pij, "#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
    fs::set_permissions(&pij, fs::Permissions::from_mode(0o700)).unwrap();
    let output = home
        .command(&[
            "node",
            "add",
            "--type",
            "problem",
            "--title",
            "Slow identity",
        ])
        .bounded_output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    let reply = Envelope::from_json(&output.stdout).unwrap();
    assert_ne!(reply.data["actor_tier"], "pij", "{}", reply.to_json());
    assert!(!stderr.contains("tool cleanup pending"), "{stderr}");
}

/// F113: `check --graph a,b,c` (or a saved scope) checks each graph with the whole list named, so links into
/// named-visibility graphs resolve; the answer does not depend on the order given; a single check still shows
/// graph_not_loaded with a link_targets_unnamed count; a listed graph that is not loaded is refused.
#[test]
fn check_names_every_graph_in_a_list_so_links_between_them_resolve() {
    let mut home = Home::new("check-list");
    home.linked_graphs();
    let kinds = |env: &Envelope| -> Vec<String> {
        let rows = env.data["rows"].as_array().unwrap();
        rows.iter()
            .map(|r| r["kind"].as_str().unwrap().to_owned())
            .collect()
    };

    let (single, _) = home.umx(&["check", "--graph", "demo"]);
    assert_eq!(
        kinds(&single),
        ["graph_not_loaded", "graph_not_loaded"],
        "{}",
        single.to_json()
    );
    assert!(single.data.get("graphs").is_none());

    let want = vec![
        ("d1".to_owned(), 0),
        ("d2".to_owned(), 0),
        ("demo".to_owned(), 0),
    ];
    let (listed, exit) = home.umx(&["check", "--graph", "demo,d1,d2"]);
    assert_eq!(exit, 0, "{}", listed.to_json());
    assert!(kinds(&listed).is_empty(), "{}", listed.to_json());
    assert_eq!(check_totals(&listed), want);
    assert!(
        !has(&listed, Code::LinkTargetsUnnamed),
        "{}",
        listed.to_json()
    );
    let (reversed, _) = home.umx(&["check", "--graph", "d2,d1,demo"]);
    assert_eq!(check_totals(&reversed), want);
    assert_eq!(reversed.data["rows"], listed.data["rows"]);

    home.ok(&["scope", "save", "linked", "--graph", "demo,d1,d2"]);
    let (scoped, _) = home.umx(&["check", "--scope", "linked"]);
    assert_eq!(check_totals(&scoped), want, "{}", scoped.to_json());
    // Graph-addressing-expert N3: a saved scope's warnings come through, as for node list.
    assert!(
        has(&scoped, Code::ScopeIncludesSecured),
        "{}",
        scoped.to_json()
    );

    let (partial, _) = home.umx(&["check", "--graph", "demo,d1"]);
    assert_eq!(
        kinds(&partial),
        ["graph_not_loaded"],
        "{}",
        partial.to_json()
    );
    let columns = partial.data["columns"].as_array().unwrap();
    assert!(columns.iter().any(|c| c == "graph"));
    assert_eq!(partial.data["rows"][0]["graph"], "demo");

    let (missing, exit) = home.umx(&["check", "--graph", "demo,nope"]);
    assert_eq!(
        (exit, missing.errors[0].code()),
        (1, "graph_not_loaded"),
        "{}",
        missing.to_json()
    );
}

/// F113: doctor checks every loaded graph with all of them named, so links between loaded graphs are not problems,
/// while a link into a graph that is not loaded still counts, with the explicit single-graph check as its next.
#[test]
fn doctor_reads_links_between_loaded_graphs() {
    let mut home = Home::new("doc-linked");
    home.linked_graphs_and_a_gone_one();
    let (env, _) = home.umx(&["doctor"]);
    let rows = env.data["rows"].as_array().unwrap();
    let outcome = |subject: &str| {
        let row = rows
            .iter()
            .find(|r| r["check"] == "graph check" && r["subject"] == subject);
        row.unwrap_or_else(|| panic!("no graph check for {subject}: {}", env.to_json()))["outcome"]
            .clone()
    };
    assert_eq!(outcome("demo"), "ok", "{}", env.to_json());
    assert_eq!(outcome("d1"), "ok", "{}", env.to_json());
    assert_eq!(outcome("d2"), "degraded", "{}", env.to_json());
    let found = env
        .warnings
        .iter()
        .find(|i| i.code() == "doctor_graph_problems")
        .unwrap();
    assert_eq!(
        found.message(),
        "Graph d2 has 1 problems in its files or the files it cites."
    );
    assert_eq!(found.next(), "umx check --graph d2");
}

/// F113 (review note): two loaded copies of one graph (design and design_b), both hidden, are each in
/// doctor's one named check, so demo's link into that graph resolves, while a single check of demo leaves it unread.
#[test]
fn doctor_reads_links_into_two_loaded_copies_of_one_graph() {
    let mut home = Home::new("doc-copies");
    home.design();
    home.design_copy("design_b");
    let demo = home.path.join("g").display().to_string();
    home.ok(&["graph", "new", &demo, "--alias", "demo"]);
    let a = home
        .ok(&[
            "node", "add", "--type", "problem", "--title", "Author", "--graph", "demo",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let address = "umx:design/01JDCYPHER00000000000000C1";
    home.ok(&["edge", "add", &a, "blocks", address, "--graph", "demo"]);
    for alias in ["design", "design_b"] {
        home.ok(&["graph", "set", alias, "--visibility", "named"]);
    }
    let (single, _) = home.umx(&["check", "--graph", "demo"]);
    assert_eq!(
        single.data["rows"][0]["kind"],
        "graph_not_loaded",
        "{}",
        single.to_json()
    );
    let (env, _) = home.umx(&["doctor"]);
    let rows = env.data["rows"].as_array().unwrap();
    let demo_row = rows
        .iter()
        .find(|r| r["check"] == "graph check" && r["subject"] == "demo")
        .unwrap_or_else(|| panic!("{}", env.to_json()));
    assert_eq!(demo_row["outcome"], "ok", "{}", env.to_json());
    for alias in ["design", "design_b"] {
        assert!(
            rows.iter()
                .any(|r| r["check"] == "graph check" && r["subject"] == alias),
            "{alias} not checked: {}",
            env.to_json()
        );
    }
}

/// Round 1 (B1, B2): below the skill's root doctor follows no link and opens only regular files, and its
/// caps bound the whole walk. A skill part moved outside and linked back would fingerprint the same if followed,
/// so ok here would mean the link was followed; empty folders change no file, so ok would mean they went uncounted;
/// a FIFO would block a read. Each is a bounded doctor_skill_out_of_date (Home::umx fails a run over 60 s).
#[test]
fn doctor_follows_no_link_inside_the_skill_and_stays_bounded() {
    // Each case plants into (the installed skill, a folder outside it).
    type Plant = fn(&Path, &Path);
    let cases: [(&str, Plant); 4] = [
        ("doc-skill-dirlink", |skill, outside| {
            fs::rename(skill.join("references"), outside.join("references")).unwrap();
            std::os::unix::fs::symlink(outside.join("references"), skill.join("references"))
                .unwrap();
        }),
        ("doc-skill-filelink", |skill, outside| {
            fs::rename(skill.join("SKILL.md"), outside.join("SKILL.md")).unwrap();
            std::os::unix::fs::symlink(outside.join("SKILL.md"), skill.join("SKILL.md")).unwrap();
        }),
        ("doc-skill-fifo", |skill, _| {
            let made = Command::new("mkfifo")
                .arg(skill.join("references/pipe"))
                .bounded_status()
                .unwrap();
            assert!(made.success());
        }),
        ("doc-skill-dirs", |skill, _| {
            for i in 0..600 {
                fs::create_dir(skill.join(format!("d{i}"))).unwrap();
            }
        }),
    ];
    for (name, plant) in cases {
        let home = Home::new(name);
        let skill = home.skill_copy(&home.path.join(".agents/skills/umx"));
        let outside = home.path.join("outside");
        fs::create_dir_all(&outside).unwrap();
        plant(&skill, &outside);
        let (env, exit) = home.umx(&["doctor"]);
        let rows = env.data["rows"].as_array().unwrap();
        let row = rows.iter().find(|r| r["check"] == "skill").unwrap();
        assert_eq!(
            (row["outcome"].as_str(), row["code"].as_str()),
            (Some("degraded"), Some("doctor_skill_out_of_date")),
            "{name}: {}",
            env.to_json()
        );
        assert!(exit >= 5, "{name}");
    }
}

/// req-73, api key: the key named by api_key_env is read from the environment at start; nothing runs az, and only
/// the env var NAME is ever shown. With the variable unset, the instance is not built and the error names it and
/// `az login`.
#[test]
fn probe_with_an_api_key_uses_the_named_env_var_and_never_shows_it() {
    let extra =
        "embedders.az.auth = \"api_key\"\nembedders.az.api_key_env = \"UMX_TEST_AZURE_KEY\"\n";
    let mut home = Home::new("api-key");
    home.az_active(extra);
    home.env
        .push(("UMX_TEST_AZURE_KEY".into(), "key-DO-NOT-LEAK".into()));
    home.start();
    let (env, _) = home.umx(&["embeddings", "probe"]);
    let found = issue(&env, Code::EmbedderUnavailable);
    assert!(
        found.message().contains("could not reach"),
        "{}",
        found.message()
    );
    let all = printed(&home, &env);
    assert!(!all.contains("DO-NOT-LEAK"), "{all}");
    let status = home.ok(&["daemon", "status"]);
    assert_eq!(
        status.data["embedder"]["credential"],
        json!({ "kind": "api_key", "from": "UMX_TEST_AZURE_KEY" })
    );

    let mut unset = Home::new("api-key-unset");
    unset.az_active(extra);
    let ready = unset.start();
    assert_eq!(
        ready.status,
        Status::Ok,
        "a broken instance never stops the daemon"
    );
    let (env, _) = unset.umx(&["embeddings", "probe"]);
    let found = issue(&env, Code::EmbedderInvalid);
    assert!(
        found.message().contains("UMX_TEST_AZURE_KEY"),
        "{}",
        found.message()
    );
    assert!(found.message().contains("az login"), "{}", found.message());
    assert_eq!(found.next(), "umx config check");
}

#[test]
fn doctor_never_shows_an_api_key() {
    let mut home = Home::new("doc-key");
    home.start();
    home.az();
    home.ok(&["config", "set", "embedders.az.auth", "api_key"]);
    home.ok(&["config", "set", "embedders.az.api_key_env", "UMX_TEST_KEY"]);
    home.ok(&["config", "set", "embedder.active", "az"]);
    let secret = "sk-not-a-real-key-0123456789abcdef";
    let (env, _) = home.umx_with(&["doctor"], &[("UMX_TEST_KEY", secret)]);
    assert!(!env.to_json().contains(secret), "{}", env.to_json());
    // Present is all doctor can say about a key: it never calls the endpoint (security F38).
    let row = env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["check"] == "embedder")
        .unwrap()
        .clone();
    assert_eq!(
        row["subject"], "text-embedding-3-small@256 (api key present, not verified)",
        "{row}"
    );
    let human = home
        .command(&["doctor", "--human"])
        .env("UMX_TEST_KEY", secret)
        .bounded_output()
        .unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&human.stdout),
        String::from_utf8_lossy(&human.stderr)
    );
    assert!(!all.contains(secret), "{all}");
}

/// s17: the daemon keeps every built instance, and a graph probe uses the instance of the graph's recorded model,
/// not embedder.active (the default). The endpoint is a closed local port, so the probe stops after signing in,
/// naming the instance it used.
#[test]
fn a_graph_probe_uses_the_instance_of_the_graph_model() {
    let mut home = Home::new("per-graph");
    home.az_active(
        "embedders.small.kind = \"azure_openai\"\n\
         embedders.small.endpoint = \"https://127.0.0.1:9\"\n\
         embedders.small.deployment = \"emb2\"\n\
         embedders.small.model = \"text-embedding-3-large\"\n\
         embedders.small.dims = 256\n",
    );
    home.start();
    let path = home.path.join("g").display().to_string();
    let (env, _) = home.umx(&[
        "graph",
        "new",
        &path,
        "--alias",
        "demo",
        "--embedding-model",
        "text-embedding-3-large@256",
    ]);
    assert!(
        env.warnings.is_empty(),
        "the graph's model is built: {}",
        env.to_json()
    );
    let (env, _) = home.umx(&["embeddings", "probe", "--graph", "demo"]);
    let found = issue(&env, Code::EmbedderUnavailable);
    assert!(
        found.message().starts_with("Embedder small "),
        "{}",
        found.message()
    );
    let (env, _) = home.umx(&["embeddings", "probe"]);
    let found = issue(&env, Code::EmbedderUnavailable);
    assert!(
        found.message().starts_with("Embedder az "),
        "{}",
        found.message()
    );
}

/// ws003 s09 (usage 205): through the CLI and the daemon, a keyed batch sent again returns the receipt its first run
/// recorded (the receipt fields equal), writes nothing (the folder byte for byte), and that reply works as a
/// `--refs-from` receipt.
#[test]
fn batch_idempotency_key_replays_the_first_receipt_byte_for_byte() {
    let mut home = Home::new("batch-idempotency-replay");
    let path = home.graph();
    let file = home.ops_file(
        "{\"op\":\"node add\",\"ref\":\"q\",\"type\":\"question\",\"title\":\"Storage choice\"}\n\
         {\"op\":\"node add\",\"ref\":\"a\",\"type\":\"option\",\"title\":\"Files\"}\n\
         {\"op\":\"edge add\",\"from_ref\":\"a\",\"relation\":\"answers\",\"to_ref\":\"q\"}\n",
    );
    let args = ["batch", "--idempotency-key", "window-08", "--file", &file];
    let first = home.ok(&args);
    assert_eq!(first.data["replayed"], false);
    assert_eq!(
        first.data["counts"]["node add"],
        json!({"lines": 2, "existed": 0})
    );
    let before = folder_bytes(&path);
    assert!(before[3].is_some(), "the receipt is in the graph folder");
    let (receipt, again) = home.receipt("again", &args);
    assert_eq!(again.data["replayed"], true);
    for field in [
        "graph_id",
        "applied",
        "refs",
        "counts",
        "idempotency_key",
        "recorded_at",
    ] {
        assert_eq!(again.data[field], first.data[field], "{field}");
    }
    assert_eq!(again.data["rows"], json!([]));
    assert!(again.hints.iter().any(|h| h.code() == "batch_replayed"));
    assert_eq!(folder_bytes(&path), before, "a replay writes nothing");
    let links = home.ops_file("{\"op\":\"node note\",\"node_ref\":\"a\",\"append\":\"chosen\"}\n");
    home.ok(&["batch", "--refs-from", &receipt, "--file", &links]);
    let human = home
        .command(&[
            "batch",
            "--idempotency-key",
            "window-08",
            "--file",
            &file,
            "--human",
        ])
        .bounded_output()
        .unwrap();
    let text = String::from_utf8(human.stdout).unwrap();
    assert!(text.contains("replayed: recorded"), "{text}");
    let (env, exit) = home.umx(&["batch", "--idempotency-key", "a b", "--file", &file]);
    assert_eq!(exit, 2, "{}", env.to_json());
    assert!(
        !env.to_json().contains("a b"),
        "the usage error names the rule, not the key"
    );
}

/// ws003 s09: two keyed batches sent at once go through the daemon's one writer in turn: one runs, the other
/// replays it, and the nodes are added once.
#[test]
fn concurrent_batches_with_one_key_run_once() {
    let mut home = Home::new("batch-idempotency-concurrent");
    home.graph();
    let file = home
        .ops_file("{\"op\":\"node add\",\"ref\":\"q\",\"type\":\"question\",\"title\":\"Once\"}\n");
    let args = ["batch", "--idempotency-key", "together", "--file", &file];
    let children: Vec<_> = (0..2)
        .map(|_| home.command(&args).spawn_captured().unwrap())
        .collect();
    let replies: Vec<Envelope> = children
        .into_iter()
        .map(|c| Envelope::from_json(&c.finish(LIMIT).stdout).unwrap())
        .collect();
    let replayed: Vec<bool> = replies.iter().map(|r| r.data["replayed"] == true).collect();
    assert_eq!(replayed.iter().filter(|r| **r).count(), 1, "{replayed:?}");
    assert_eq!(replies[0].data["refs"], replies[1].data["refs"]);
    let list = home.ok(&["node", "list", "--type", "question"]);
    assert_eq!(list.data["rows"].as_array().unwrap().len(), 1);
}

#[test]
fn a_batch_port_keeps_explicit_ids_and_created_and_accepts_the_file_relation_key() {
    let mut home = Home::new("batch-port");
    let path = home.graph();
    let a = "01JC8XA4H7B3KT9M2NZ5QW6RDE";
    let b = "01JC8XA4H7B3KT9M2NZ5QW6RDF";
    let e = "01JC8XA4H7B3KT9M2NZ5QW6RDD";
    let text = format!(
        "{{\"op\":\"node add\",\"id\":\"{a}\",\"created\":\"2024-11-11T00:00:00Z\",\"ref\":\"p1\",\"type\":\"problem\",\"title\":\"Ported problem\"}}\n\
         {{\"op\":\"node add\",\"id\":\"{b}\",\"type\":\"option\",\"title\":\"Ported option\"}}\n\
         {{\"op\":\"edge add\",\"id\":\"{e}\",\"from\":\"{b}\",\"type\":\"supports\",\"to_ref\":\"p1\"}}\n"
    );
    let file = home.ops_file(&text);
    let dry = big(&home, &["batch", "--file", &file, "--dry-run"]);
    assert_eq!(dry.data["rows"][0]["id"], a);
    assert_eq!(dry.data["refs"]["p1"], a);
    let done = big(&home, &["batch", "--file", &file]);
    assert_eq!(done.data["rows"][2]["id"], e);
    let (got, exit) = home.umx(&["node", "get", a]);
    assert_eq!(exit, 0, "{}", got.to_json());
    assert_eq!(got.data["node"]["created"], "2024-11-11T00:00:00Z");
    let edge_file = fs::read_to_string(Path::new(&path).join("edges.jsonl")).unwrap();
    let record = edge_file
        .lines()
        .map(|line| umx_protocol::parse_json(line).unwrap())
        .find(|record| record["id"] == e)
        .expect("the acknowledged edge is persisted");
    assert_eq!(record["type"], "supports");
    assert!(record.get("relation").is_none());
    let file = home.ops_file(&format!(
        "{{\"op\":\"edge add\",\"from\":\"{a}\",\"relation\":\"supports\",\"type\":\"supports\",\"to\":\"{b}\"}}\n"
    ));
    let (env, exit) = home.umx(&["batch", "--file", &file]);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert_eq!(env.errors[1].code(), "batch_line_invalid");
    assert_eq!(env.errors[1].line(), Some(1));
    let file = home.ops_file(
        "{\"op\":\"node add\",\"id\":\"legacy-42\",\"type\":\"problem\",\"title\":\"Bad\"}\n",
    );
    let (env, exit) = home.umx(&["batch", "--file", &file]);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert_eq!(env.errors[1].code(), "id_invalid");
    assert!(env.errors[1].message().contains("source_id"));
}

/// D62: there is nothing to trust by hand any more, so a trusted_endpoint is not a key, and a graph's two settings
/// show in its rows; loosening one by graph set takes effect in that call, warned (D83).
#[test]
fn trusted_endpoint_is_not_a_key_and_graph_rows_show_both_settings() {
    let mut home = Home::new("trust");
    home.az_active("");
    home.secured_graph();
    let (env, exit) = home.umx(&[
        "config",
        "set",
        "embedders.az.trusted_endpoint",
        "https://127.0.0.1:9",
    ]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::ConfigKeyUnknown);
    let config = fs::read_to_string(home.path.join("config.toml")).unwrap();
    assert!(!config.contains("trusted_endpoint"), "{config}");
    let row = |home: &Home| home.ok(&["graph", "list"]).data["rows"][0].clone();
    let before = row(&home);
    assert_eq!(
        (
            &before["effective_policy"],
            &before["effective_visibility"],
            &before["effective_embedder"]
        ),
        (&json!("secured"), &json!("named"), &json!("local"))
    );
    let set = home.ok(&["graph", "set", "demo", "--embedder", "trusted"]);
    let lowered = issue(&set, Code::PolicyLowered);
    assert!(
        lowered.message().contains("az (hosted at "),
        "{}",
        lowered.message()
    );
    let after = row(&home);
    assert_eq!(after["embedding"]["embedder"], "trusted");
    assert_eq!(
        after["embedding"]["policy"], "secured",
        "as the file says it"
    );
    assert_eq!(
        after["effective_embedder"], "trusted",
        "lowered in one call"
    );
    let file = fs::read_to_string(home.path.join("g/graph.json")).unwrap();
    assert!(
        file.contains("\"policy\": \"secured\",\n    \"visibility\": \"named\",\n    \"embedder\": \"trusted\""),
        "{file}"
    );
}

/// ws008 E1 (req-102): the same key on nodes of the same type in two graphs is found by one --where across them;
/// an email key is compared lowercase whichever way it arrives; `entity key` adds and removes in stored form.
#[test]
fn an_entity_key_is_found_across_graphs_in_stored_form() {
    let mut home = Home::new("e1-keys");
    home.start();
    for alias in ["crm", "notes"] {
        let path = home.path.join(alias).display().to_string();
        home.ok(&["graph", "new", &path, "--alias", alias]);
    }
    let add = |graph: &str, kind: &str, title: &str, key: &str| {
        let set = format!("entity_key={key}");
        let env = home.ok(&[
            "node", "add", "--graph", graph, "--type", kind, "--title", title, "--set", &set,
        ]);
        env.data["node"]["id"].as_str().unwrap().to_owned()
    };
    let jo_crm = add("crm", "person", "Jo Smith", "email:Jo.Smith@Example.COM");
    let jo_notes = add("notes", "person", "J. Smith", "upn:jsmith@corp.example");
    add("notes", "person", "Someone else", "email:other@example.com");
    let stored = home.ok(&["node", "get", &format!("umx:crm/{jo_crm}")]);
    assert_eq!(
        stored.data["node"]["entity_key"],
        json!(["email:jo.smith@example.com"]),
        "stored lowercase"
    );

    // entity key add puts the crm key on the notes node, in stored form; adding it again changes nothing.
    let notes = format!("umx:notes/{jo_notes}");
    let added = home.ok(&["entity", "key", &notes, "add", "email:JO.SMITH@example.com"]);
    assert_eq!(added.data["changed"], json!(["entity_key"]));
    assert_eq!(
        added.data["node"]["entity_key"],
        json!(["upn:jsmith@corp.example", "email:jo.smith@example.com"])
    );
    let again = home.ok(&["entity", "key", &notes, "add", "email:jo.smith@example.com"]);
    assert_eq!(again.data["changed"], json!([]));

    // One --where over both graphs finds both nodes, whatever case the value is given in.
    let ids = |args: &[&str]| -> Vec<String> {
        let env = home.ok(args);
        let mut v: Vec<String> = env.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                r["address"]
                    .as_str()
                    .or(r["id"].as_str())
                    .unwrap()
                    .to_owned()
            })
            .collect();
        v.sort();
        v
    };
    let mut both = vec![format!("umx:crm/{jo_crm}"), notes.clone()];
    both.sort();
    for value in ["email:jo.smith@example.com", "email:Jo.Smith@EXAMPLE.com"] {
        let w = format!("entity_key={value}");
        assert_eq!(
            ids(&["node", "list", "--graph", "crm,notes", "--where", &w]),
            both,
            "{value}"
        );
    }
    // req-102's done_when: the same through a saved scope.
    home.ok(&["scope", "save", "people", "--graph", "crm,notes"]);
    let w = "entity_key=email:JO.smith@example.com";
    assert_eq!(
        ids(&["node", "list", "--scope", "people", "--where", w]),
        both
    );
    // Other list fields: one value without a comma is an element; with commas, the whole list as before.
    let tagged = home.ok(&[
        "node",
        "add",
        "--graph",
        "notes",
        "--type",
        "topic",
        "--title",
        "t",
        "--set-json",
        r#"labels=["red","blue"]"#,
    ]);
    let tagged = tagged.data["node"]["id"].as_str().unwrap().to_owned();
    // A custom list field (a std graph's `tags` is the sorted std tag list, req-117).
    for (w, hit) in [
        ("labels=red,blue", true),
        ("labels=red", true),
        ("labels=blue,red", false),
        ("labels=green", false),
    ] {
        let found = ids(&["node", "list", "--graph", "notes", "--where", w]);
        assert_eq!(found == [tagged.clone()], hit, "{w}: {found:?}");
    }
    // != is the complement on a list: the other notes person only.
    let ne = ids(&[
        "node",
        "list",
        "--graph",
        "notes",
        "--where",
        "entity_key!=email:jo.smith@example.com",
    ]);
    assert_eq!(ne.len(), 1, "{ne:?}");

    // remove takes it out again; removing the last key removes the field.
    let removed = home.ok(&[
        "entity",
        "key",
        &notes,
        "remove",
        "email:jo.smith@example.com",
    ]);
    assert_eq!(
        removed.data["node"]["entity_key"],
        json!(["upn:jsmith@corp.example"])
    );
    let w = "entity_key=email:jo.smith@example.com";
    assert_eq!(
        ids(&["node", "list", "--graph", "crm,notes", "--where", w]),
        [format!("umx:crm/{jo_crm}")]
    );
    let last = home.ok(&["entity", "key", &notes, "remove", "upn:jsmith@corp.example"]);
    assert!(
        last.data["node"].get("entity_key").is_none(),
        "{}",
        last.to_json()
    );
}

/// ws008 E1: a malformed key is refused and nothing is written; entity_key cannot be declared; a graph with no keys
/// keeps byte-identical files through a reload.
#[test]
fn a_malformed_entity_key_is_refused_and_writes_nothing() {
    let mut home = Home::new("e1-refuse");
    let path = home.graph();
    let id = home.add("person", "Jo");
    let files = |p: &str| -> Vec<Vec<u8>> {
        ["graph.json", "nodes.jsonl", "edges.jsonl"]
            .iter()
            .map(|f| fs::read(Path::new(p).join(f)).unwrap_or_default())
            .collect()
    };
    let before = files(&path);
    for (args, why) in [
        (
            vec![
                "node",
                "add",
                "--type",
                "person",
                "--title",
                "x",
                "--set",
                "entity_key=jo@x.com",
            ],
            "no scheme",
        ),
        (
            vec!["node", "edit", &id, "--set", "entity_key=E-Mail:jo@x.com"],
            "the scheme is not snake_case",
        ),
        (
            vec!["entity", "key", &id, "add", "id:not-a-umx-id"],
            "an id: key",
        ),
        (vec!["entity", "key", &id, "add", "email:"], "no value"),
    ] {
        let (env, exit) = home.umx(&args);
        assert_eq!(exit, 1, "{args:?}: {}", env.to_json());
        let found = issue(&env, Code::EntityKeyInvalid);
        assert!(
            found.message().contains(why),
            "{args:?}: {}",
            found.message()
        );
        assert!(
            !found.message().contains("jo@x.com"),
            "the key is never echoed"
        );
    }
    // The fixed scheme goes in next, the value never does.
    let (env, _) = home.umx(&["entity", "key", &id, "add", "Email:jo@x.com"]);
    assert_eq!(
        issue(&env, Code::EntityKeyInvalid).next(),
        "umx entity key <node> add email:<value>"
    );
    // Graph-addressing-expert F1 and security N1, N3: bounded and clean values, at most 32 keys.
    let long = format!("domain:{}", "x".repeat(257));
    let many = (0..33)
        .map(|i| format!("n:{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let many = format!("entity_key={many}");
    for (args, why) in [
        (vec!["entity", "key", &id, "add", &long], "over 256 bytes"),
        (
            vec!["entity", "key", &id, "add", "email:jo\u{200B}@x.com"],
            "zero-width",
        ),
        (
            vec!["entity", "key", &id, "add", "domain:x\u{202E}y"],
            "bidi",
        ),
        (
            vec!["entity", "key", &id, "add", "email:jo\tx@x.com"],
            "control character",
        ),
        (
            vec!["entity", "key", &id, "add", "upn:jo smith"],
            "whitespace inside",
        ),
        (vec!["node", "edit", &id, "--set", &many], "at most 32 keys"),
    ] {
        let (env, exit) = home.umx(&args);
        assert_eq!(exit, 1, "{why}: {}", env.to_json());
        let found = issue(&env, Code::EntityKeyInvalid);
        assert!(found.message().contains(why), "{why}: {}", found.message());
    }
    assert_eq!(files(&path), before, "a refused key writes nothing");
    let (env, exit) = home.umx(&["ontology", "add", "field", "person", "entity_key:list"]);
    assert_eq!((exit, env.errors[0].code()), (1, "reserved_name"));
    let (env, exit) = home.umx(&["entity", "key", &id, "change", "email:a@x.com"]);
    assert_eq!((exit, env.errors[0].code()), (2, "usage_invalid"));

    // Security N2: a key that looks like a secret is kept and warns, naming the field and rule, never the key.
    let secret = format!("aws:AKIA{}", "Q7Z3X9K2".repeat(2));
    let (env, exit) = home.umx(&["entity", "key", &id, "add", &secret]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let found = issue(&env, Code::SecretSuspected);
    assert!(
        found.message().contains("entity_key"),
        "{}",
        found.message()
    );
    assert!(
        !found.message().contains("AKIA") && !found.next().contains("AKIA"),
        "the key never appears in an issue"
    );
    home.ok(&["entity", "key", &id, "remove", &secret]);
    let before = files(&path);

    // No keys anywhere: a reload and a keyed read change no byte.
    home.ok(&["node", "list", "--where", "entity_key=email:jo@x.com"]);
    home.ok(&["graph", "unload", "demo"]);
    home.ok(&["graph", "load", &path]);
    assert_eq!(files(&path), before);
    assert!(!String::from_utf8_lossy(&before[1]).contains("entity_key"));
}

/// ws008 E1: two nodes of the same type in one graph with the same key warn duplicate_key, on the write that makes
/// it and on load; the same key on different types does not.
#[test]
fn a_shared_key_in_one_graph_warns_duplicate_key_only_for_one_type() {
    let mut home = Home::new("e1-dup");
    let path = home.graph();
    let add = |kind: &str, title: &str, key: &str| {
        let set = format!("entity_key={key}");
        home.ok(&[
            "node", "add", "--type", kind, "--title", title, "--set", &set,
        ])
    };
    let a = add("person", "a", "email:a@x.com");
    let org = add("organisation", "x", "email:a@x.com");
    assert!(
        !has(&org, Code::DuplicateKey),
        "another type: {}",
        org.to_json()
    );
    let b = add("person", "b", "email:A@X.com");
    let found = issue(&b, Code::DuplicateKey);
    let (a_id, b_id) = (
        a.data["node"]["id"].as_str().unwrap(),
        b.data["node"]["id"].as_str().unwrap(),
    );
    assert!(
        found.message().contains(b_id) && found.message().contains(a_id),
        "{}",
        found.message()
    );
    assert!(
        !found.message().contains("a@x.com"),
        "the key is never shown"
    );
    // A batch that makes one says so once.
    let batch = r#"{"op":"node add","type":"person","title":"c","sets":[{"field":"entity_key","value":["email:a@x.com"]}]}"#;
    let file = home.path.join("ops.jsonl");
    fs::write(&file, format!("{batch}\n")).unwrap();
    // A dry run reports it too: the warning is counted with the line warnings, before the reply bound.
    let dry = home.ok(&["batch", "--file", file.to_str().unwrap(), "--dry-run"]);
    let n = dry
        .warnings
        .iter()
        .filter(|w| w.code() == "duplicate_key")
        .count();
    assert_eq!(n, 1, "{}", dry.to_json());
    let ran = home.ok(&["batch", "--file", file.to_str().unwrap()]);
    let n = ran
        .warnings
        .iter()
        .filter(|w| w.code() == "duplicate_key")
        .count();
    assert_eq!(n, 1, "{}", ran.to_json());
    // On load: each repeat once.
    home.ok(&["graph", "unload", "demo"]);
    let (loaded, exit) = home.umx(&["graph", "load", &path]);
    assert_eq!(exit, 0, "{}", loaded.to_json());
    let n = loaded
        .warnings
        .iter()
        .filter(|w| w.code() == "duplicate_key")
        .count();
    assert_eq!(n, 2, "b and c each repeat a's key: {}", loaded.to_json());
}

/// ws008 E1 (graph-addressing-expert, review R1): a hand-edited key that is not in stored form is still found by
/// --where (both sides compare stored forms), warns on load, and is stored canonically on the node's next write; an
/// entry umx cannot use is kept in the file, warned, and left out.
#[test]
fn a_hand_edited_key_is_found_warned_and_made_canonical_on_the_next_write() {
    let mut home = Home::new("e1-canon");
    let path = home.graph();
    let id = home.ok(&[
        "node",
        "add",
        "--type",
        "person",
        "--title",
        "Ana",
        "--set",
        "entity_key=email:ana@x.com",
    ]);
    let id = id.data["node"]["id"].as_str().unwrap().to_owned();
    home.ok(&["graph", "unload", "demo"]);
    let file = Path::new(&path).join("nodes.jsonl");
    let text = fs::read_to_string(&file).unwrap();
    let edited = text.replace(
        r#"["email:ana@x.com"]"#,
        r#"["email:Ana@X.com","not a key"]"#,
    );
    assert_ne!(edited, text, "the hand edit applied");
    fs::write(&file, &edited).unwrap();
    let loaded = home.ok(&["graph", "load", &path]);
    issue(&loaded, Code::EntityKeyNotCanonical);
    issue(&loaded, Code::EntityKeyUnusable);
    assert!(
        !loaded.to_json().contains("Ana@X"),
        "the key is never shown in an issue"
    );
    let found = home.ok(&["node", "list", "--where", "entity_key=email:ana@x.com"]);
    assert_eq!(
        found.data["rows"][0]["id"],
        json!(id),
        "{}",
        found.to_json()
    );
    // A write of the node stores the key canonically and keeps the entry it cannot read.
    home.ok(&["node", "edit", &id, "--title", "Ana B"]);
    let now = fs::read_to_string(&file).unwrap();
    assert!(now.contains(r#"["email:ana@x.com","not a key"]"#), "{now}");
}

/// ws008 T6 (req-138): `instance_of` is a std relation. Any graph writes it without declaring it, across graphs, one
/// way; it never joins an entity (entity show follows same_as only), while a key the two share still does; it cannot be
/// made symmetric or removed.
#[test]
fn instance_of_relates_across_graphs_and_never_joins_an_entity() {
    let mut home = Home::new("t6-instance");
    let n = e2_people(&mut home, &["deploy", "catalog"], &[]);
    let (instance, product) = (n[0].clone(), n[1].clone());
    let (added, exit) = home.umx(&[
        "edge",
        "add",
        &instance,
        "instance_of",
        &product,
        "--graph",
        "deploy",
    ]);
    assert_eq!(exit, 0, "{}", added.to_json());
    assert!(
        added.warnings.is_empty(),
        "instance_of is std: {}",
        added.to_json()
    );
    assert_eq!(added.data["edge"]["type"], "instance_of");
    let shown = home.ok(&["entity", "show", &instance]);
    assert_eq!(
        e2_members(&shown),
        std::slice::from_ref(&instance),
        "{}",
        shown.to_json()
    );
    let back = home.ok(&["entity", "show", &product]);
    assert_eq!(
        e2_members(&back),
        std::slice::from_ref(&product),
        "{}",
        back.to_json()
    );

    // A key both hold joins them as ever; the instance_of edge adds nothing to the match.
    for node in [&instance, &product] {
        home.ok(&[
            "node",
            "edit",
            node,
            "--set",
            "entity_key=email:jo@example.com",
        ]);
    }
    let keyed = home.ok(&["entity", "show", &instance]);
    assert_eq!(
        e2_members(&keyed),
        [instance.clone(), product.clone()],
        "{}",
        keyed.to_json()
    );
    let by: Vec<&str> = keyed.data["members"][1]["matched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["by"].as_str().unwrap())
        .collect();
    assert_eq!(by, ["entity_key"], "{}", keyed.to_json());

    // Directed by what it means, and the graph's own to keep.
    let (env, exit) = home.umx(&[
        "ontology",
        "add",
        "relation",
        "instance_of",
        "--symmetric",
        "--graph",
        "deploy",
    ]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::SymmetricRefused);
    let (env, exit) = home.umx(&[
        "ontology",
        "remove",
        "relation",
        "instance_of",
        "--graph",
        "deploy",
    ]);
    assert_ne!(exit, 0, "{}", env.to_json());
}

/// ws008 E2: `entity link` stores one same_as edge (in the first node's graph, lower end first), `entity show` lists
/// both members across the two graphs and how each joined; a re-link either way is unchanged; a self-link is
/// refused and writes nothing; two types still link and warn; `entity unlink` removes the link and a second unlink
/// is unchanged.
#[test]
fn an_entity_link_joins_two_graphs_and_unlink_removes_it() {
    let mut home = Home::new("e2-link");
    let n = e2_people(&mut home, &["crm", "meetings"], &[]);
    let (a, b) = (n[0].clone(), n[1].clone());
    let linked = home.ok(&["entity", "link", &a, &b]);
    assert_eq!(linked.data["edge"]["graph"], "crm", "{}", linked.to_json());
    assert_eq!(linked.data["edge"]["type"], "same_as");
    assert_eq!(linked.data["unchanged"], false);
    assert!(
        linked.warnings.is_empty(),
        "same_as is std: {}",
        linked.to_json()
    );
    let edges = e2_same_as(&home, "crm");
    assert_eq!(edges.len(), 1, "{edges:?}");

    let shown = home.ok(&["entity", "show", &a]);
    assert_eq!(
        e2_members(&shown),
        [a.clone(), b.clone()],
        "{}",
        shown.to_json()
    );
    assert_eq!(shown.data["members"][0]["matched"][0]["by"], "asked");
    assert_eq!(shown.data["members"][1]["matched"][0]["by"], "same_as");
    assert_eq!(
        shown.data["members"][1]["matched"][0]["edge"],
        linked.data["edge"]["address"]
    );
    let mut graphs: Vec<&str> = shown.data["graphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g.as_str().unwrap())
        .collect();
    graphs.sort();
    assert_eq!(graphs, ["crm", "meetings"]);

    // Re-linking, in either order, is unchanged and writes nothing.
    for (x, y) in [(&a, &b), (&b, &a)] {
        let again = home.ok(&["entity", "link", x, y, "--graph", "crm"]);
        assert_eq!(again.data["unchanged"], true, "{}", again.to_json());
        assert_eq!(again.data["edge"]["id"], linked.data["edge"]["id"]);
    }
    assert_eq!(e2_same_as(&home, "crm"), edges);

    // A node linked to itself is refused, and nothing is written.
    let (env, exit) = home.umx(&["entity", "link", &a, &a]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::EntityLinkInvalid);
    assert_eq!(e2_same_as(&home, "crm"), edges);

    // Two types still link, and warn.
    let org = home.ok(&[
        "node",
        "add",
        "--graph",
        "meetings",
        "--type",
        "organisation",
        "--title",
        "X",
    ]);
    let org = org.data["node"]["address"].as_str().unwrap().to_owned();
    let odd = home.ok(&["entity", "link", &a, &org]);
    issue(&odd, Code::EntityTypeMismatch);
    home.ok(&["entity", "unlink", &a, &org]);

    // Unlink, given the other way round and with no --graph: b's graph holds nothing, a's does.
    let gone = home.ok(&["entity", "unlink", &b, &a]);
    assert_eq!(gone.data["deleted"], true, "{}", gone.to_json());
    assert_eq!(gone.data["edge"]["id"], linked.data["edge"]["id"]);
    assert_eq!(e2_members(&home.ok(&["entity", "show", &a])), [a.as_str()]);
    // An end that does not resolve is its error, as with --graph (review note), never "unchanged".
    let missing = "umx:meetings/01JC8XA4H7ZZZZZZZZZZZZZZZZ";
    let (env, exit) = home.umx(&["entity", "unlink", &a, missing]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::NodeNotFound);
    let again = home.ok(&["entity", "unlink", &a, &b]);
    assert_eq!(
        (
            again.data["deleted"].clone(),
            again.data["unchanged"].clone()
        ),
        (json!(false), json!(true))
    );
}

/// ws008 E2: a key shared within one type and a same_as link make one transitive entity, from either end; a
/// possibly_same_as partner is listed as proposed and never joined, and its own proposals are not followed.
#[test]
fn keys_and_links_make_one_entity_and_proposals_stay_apart() {
    let mut home = Home::new("e2-group");
    let key: &[&str] = &["--set", "entity_key=email:jo@x.com"];
    let n = e2_people(&mut home, &["g1", "g2", "g3", "g4", "g5"], &[key, key]);
    // g1 and g2 share a key; g2 links to g3 (stored in g3); g4 is proposed for g3; g5 is proposed for g4.
    home.ok(&["entity", "link", &n[1], &n[2], "--graph", "g3"]);
    home.ok(&[
        "edge",
        "add",
        &n[2],
        "possibly_same_as",
        &n[3],
        "--graph",
        "g3",
    ]);
    home.ok(&[
        "edge",
        "add",
        &n[3],
        "possibly_same_as",
        &n[4],
        "--graph",
        "g4",
    ]);
    for start in [&n[0], &n[2]] {
        let shown = home.ok(&["entity", "show", start]);
        let mut members = e2_members(&shown);
        members.sort();
        let mut want = vec![n[0].clone(), n[1].clone(), n[2].clone()];
        want.sort();
        assert_eq!(members, want, "from {start}: {}", shown.to_json());
        let proposed: Vec<&str> = shown.data["proposed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["address"].as_str().unwrap())
            .collect();
        assert_eq!(proposed, [n[3].as_str()], "{}", shown.to_json());
        assert_eq!(shown.data["proposed"][0]["with"], n[2].as_str());
    }
    let first = home.ok(&["entity", "show", &n[0]]);
    let by_key = &first.data["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["address"] == n[1].as_str())
        .unwrap()["matched"];
    assert_eq!(
        by_key[0],
        json!({"by": "entity_key", "key": "email:jo@x.com"})
    );
}

/// ws008 E2 with d-20: a link into a secured graph shows nothing from it to a read that does not name it (not its
/// member, not its graph in `graphs`, not its node id anywhere); naming it shows the member.
#[test]
fn entity_show_leaves_an_unnamed_secured_graph_out() {
    let mut home = Home::new("e2-secured");
    home.secured_graph();
    let secret = home.ok(&[
        "node", "add", "--graph", "demo", "--type", "person", "--title", "Jo",
    ]);
    let secret = secret.data["node"]["address"].as_str().unwrap().to_owned();
    let open = e2_people(&mut home, &["open"], &[]).remove(0);
    let linked = home.ok(&["entity", "link", &open, &secret]);
    issue(&linked, Code::LinkIntoSecured);
    let hidden = home.ok(&["entity", "show", &open]);
    assert_eq!(e2_members(&hidden), [open.as_str()], "{}", hidden.to_json());
    assert_eq!(hidden.data["graphs"], json!(["open"]));
    let id = secret.rsplit('/').next().unwrap();
    assert!(!hidden.to_json().contains(id), "{}", hidden.to_json());
    // Control: named, the secured member is shown.
    let named = home.ok(&["entity", "show", &open, "--graph", "open,demo"]);
    assert_eq!(
        e2_members(&named),
        [open.clone(), secret.clone()],
        "{}",
        named.to_json()
    );
}

/// ws008 E2 with d-21: two copies of one graph never group, even both named, and a link between them is refused.
#[test]
fn two_copies_of_one_graph_never_group_and_cannot_be_linked() {
    let mut home = Home::new("e2-copies");
    let key: &[&str] = &["--set", "entity_key=email:jo@x.com"];
    let n = e2_people(&mut home, &["g"], &[key]);
    let other = home.ok(&[
        "node", "add", "--graph", "g", "--type", "person", "--title", "Jo B",
    ]);
    let other = other.data["node"]["id"].as_str().unwrap().to_owned();
    let from = home.path.join("g");
    let to = home.path.join("g2");
    fs::create_dir_all(&to).unwrap();
    for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
        fs::copy(from.join(file), to.join(file)).unwrap();
    }
    home.ok(&["graph", "load", to.to_str().unwrap(), "--as", "g2"]);
    for flags in [&[][..], &["--graph", "g,g2"][..]] {
        let mut args = vec!["entity", "show", n[0].as_str()];
        args.extend_from_slice(flags);
        let shown = home.ok(&args);
        assert_eq!(
            e2_members(&shown),
            [n[0].clone()],
            "{flags:?}: {}",
            shown.to_json()
        );
        assert_eq!(shown.data["graphs"], json!(["g"]), "{}", shown.to_json());
    }
    let (env, exit) = home.umx(&["entity", "link", &n[0], &format!("umx:g2/{other}")]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::EntityLinkInvalid);
}

/// ws008 E2 (graph-addressing-expert F1): `UMX_GRAPH` names a third graph; `entity link` with two addresses still
/// stores the link in the first node's graph, and `entity unlink` still falls back to the second node's graph and
/// removes it.
#[test]
fn umx_graph_never_moves_an_entity_link_or_hides_one_from_unlink() {
    let mut home = Home::new("e2-umx-graph");
    let n = e2_people(&mut home, &["x", "z", "y"], &[]);
    let (a, b) = (n[0].clone(), n[1].clone());
    let y = [("UMX_GRAPH", "y")];
    let (linked, exit) = home.umx_with(&["entity", "link", &a, &b], &y);
    assert_eq!(exit, 0, "{}", linked.to_json());
    assert_eq!(linked.data["edge"]["graph"], "x", "{}", linked.to_json());
    home.ok(&["entity", "unlink", &a, &b]);
    home.ok(&["entity", "link", &a, &b, "--graph", "z"]);
    let (gone, exit) = home.umx_with(&["entity", "unlink", &a, &b], &y);
    assert_eq!(exit, 0, "{}", gone.to_json());
    assert_eq!(gone.data["deleted"], true, "{}", gone.to_json());
    assert!(e2_same_as(&home, "z").is_empty());
}

/// ws008 E2 (review, the project manager's ruling): a same_as pair is stored once. Linked in A's graph, linking B to A
/// from B's graph (by default, or with --graph naming it) is unchanged and writes no second copy.
#[test]
fn a_same_as_pair_is_stored_once_whatever_the_order_or_graph() {
    let mut home = Home::new("e2-once");
    let n = e2_people(&mut home, &["ga", "gb"], &[]);
    let (a, b) = (n[0].clone(), n[1].clone());
    let first = home.ok(&["entity", "link", &a, &b]);
    for args in [
        vec!["entity", "link", b.as_str(), a.as_str()],
        vec!["entity", "link", b.as_str(), a.as_str(), "--graph", "gb"],
    ] {
        let again = home.ok(&args);
        assert_eq!(
            again.data["unchanged"],
            true,
            "{args:?}: {}",
            again.to_json()
        );
        assert_eq!(again.data["edge"]["id"], first.data["edge"]["id"]);
    }
    let lines = |g: &str| e2_same_as(&home, g).len();
    assert_eq!(
        (lines("ga"), lines("gb")),
        (1, 0),
        "exactly one edge on disk"
    );
}

/// ws008 E2 (R250-4): the type check reads the copy the link names. gb2 is a copy of gb (same graph id and node ids)
/// whose copy of the node is an organisation; linking a person to it through gb2's address warns, and the stored end
/// is that graph id and node.
#[test]
fn the_type_check_reads_the_copy_the_link_names() {
    let mut home = Home::new("e2-copy-type");
    let n = e2_people(&mut home, &["ga", "gb"], &[]);
    let a = n[0].clone();
    let id = n[1].rsplit('/').next().unwrap().to_owned();
    let (from, to) = (home.path.join("gb"), home.path.join("gb2"));
    fs::create_dir_all(&to).unwrap();
    for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
        let text = fs::read_to_string(from.join(file)).unwrap();
        fs::write(
            to.join(file),
            text.replace(r#""type":"person""#, r#""type":"organisation""#),
        )
        .unwrap();
    }
    home.ok(&["graph", "load", to.to_str().unwrap(), "--as", "gb2"]);
    let linked = home.ok(&["entity", "link", &a, &format!("umx:gb2/{id}")]);
    issue(&linked, Code::EntityTypeMismatch);
    let graph: Value =
        umx_protocol::parse_json(&fs::read_to_string(from.join("graph.json")).unwrap()).unwrap();
    let e = &linked.data["edge"];
    let ends = [(&e["from_graph"], &e["from"]), (&e["to_graph"], &e["to"])];
    assert!(
        ends.contains(&(&graph["id"], &json!(id))),
        "the stored end is gb's graph id and the node: {}",
        linked.to_json()
    );
}

/// ws008 E2b G1 and G3, review T4: A keyed in three graphs and B keyed in the same three, then A in g1 linked
/// to B in g2. The link warns `entity_link_overlap` naming a member already in the other end's graph and is still
/// stored; the entity then holds two nodes of each graph, and `entity show` warns `entity_graph_twice` for each,
/// naming the two nodes and what joined them.
#[test]
fn a_link_that_merges_two_entities_warns_and_show_names_each_graph_held_twice() {
    let mut home = Home::new("e2b-t4");
    e2b_graphs(&mut home, &["g1", "g2", "g3"]);
    let a: Vec<String> = ["g1", "g2", "g3"]
        .iter()
        .map(|g| e2b_node(&home, g, "person", "Ann", "email:ann@example.com"))
        .collect();
    let b: Vec<String> = ["g1", "g2", "g3"]
        .iter()
        .map(|g| e2b_node(&home, g, "person", "Bob", "email:bob@example.com"))
        .collect();
    let quiet = home.ok(&["entity", "show", &a[0]]);
    assert!(
        issues(&quiet, Code::EntityGraphTwice).is_empty(),
        "one node per graph: {}",
        quiet.to_json()
    );
    let linked = home.ok(&["entity", "link", &a[0], &b[1]]);
    assert_eq!(linked.data["unchanged"], false, "{}", linked.to_json());
    let overlap = issues(&linked, Code::EntityLinkOverlap);
    assert_eq!(overlap.len(), 2, "one per side: {}", linked.to_json());
    let id = |addr: &str| addr.rsplit('/').next().unwrap().to_owned();
    assert!(
        overlap.iter().any(|i| i.message().contains(&id(&a[1]))),
        "names A's member in g2: {}",
        linked.to_json()
    );
    let shown = home.ok(&["entity", "show", &a[0]]);
    assert_eq!(e2_members(&shown).len(), 6, "{}", shown.to_json());
    let twice = issues(&shown, Code::EntityGraphTwice);
    assert_eq!(twice.len(), 3, "{}", shown.to_json());
    for (g, i) in ["g1", "g2", "g3"].iter().zip(&twice) {
        assert!(i.message().contains(g), "{}", i.message());
        assert!(
            i.message().contains("scheme email") || i.message().contains("same_as link"),
            "says what joined them: {}",
            i.message()
        );
    }
}

/// ws008 E2b G2, review T6: in g1, B1 (made first) holds both keys and A1 holds A's; in g2, A2 and B2. A key two
/// nodes of one type hold in g1 joins neither there, so A2's entity is A2 alone (not B1, and through it B2), and
/// `entity show` warns `entity_key_ignored` for g1.
#[test]
fn a_key_two_nodes_hold_in_one_graph_joins_neither_there() {
    let mut home = Home::new("e2b-t6");
    e2b_graphs(&mut home, &["g1", "g2"]);
    let (ka, kb) = ("email:ann@example.com", "email:bob@example.com");
    let b1 = e2b_node(&home, "g1", "person", "Bob", &format!("{ka},{kb}"));
    let a1 = e2b_node(&home, "g1", "person", "Ann", ka);
    let a2 = e2b_node(&home, "g2", "person", "Ann", ka);
    let b2 = e2b_node(&home, "g2", "person", "Bob", kb);
    let shown = home.ok(&["entity", "show", &a2]);
    assert_eq!(e2_members(&shown), [a2.as_str()], "{}", shown.to_json());
    let ignored = issue(&shown, Code::EntityKeyIgnored);
    assert!(ignored.message().contains("g1"), "{}", ignored.message());
    assert!(
        !shown.to_json().contains("ann@example.com")
            || shown.data["members"][0]["entity_key"][0] == ka
    );
    assert!(
        !shown.to_json().contains(&b1)
            && !shown.to_json().contains(&a1)
            && !shown.to_json().contains(&b2)
    );
    // From B2: B's key has one holder in each graph, so B1 joins. B1's key A is held twice in g1, so it joins nothing
    // anywhere (R2: key joins are symmetric), and A2 stays out.
    let bob = home.ok(&["entity", "show", &b2]);
    assert_eq!(
        e2_members(&bob),
        [b2.clone(), b1.clone()],
        "{}",
        bob.to_json()
    );
    assert!(
        issues(&bob, Code::EntityGraphTwice).is_empty(),
        "{}",
        bob.to_json()
    );
    let _ = a1;
}

/// ws008 E2b G4, review T5: a person linked to an organisation; `entity show` warns `entity_types_mixed`.
#[test]
fn an_entity_of_two_types_warns_on_show() {
    let mut home = Home::new("e2b-t5");
    e2b_graphs(&mut home, &["g1", "g2"]);
    let p = e2b_node(&home, "g1", "person", "Acme", "");
    let o = e2b_node(&home, "g2", "organisation", "Acme", "");
    let linked = home.ok(&["entity", "link", &p, &o]);
    issue(&linked, Code::EntityTypeMismatch);
    let shown = home.ok(&["entity", "show", &p]);
    let mixed = issue(&shown, Code::EntityTypesMixed);
    assert!(
        mixed.message().contains("person") && mixed.message().contains("organisation"),
        "{}",
        mixed.message()
    );
    let same = home.ok(&[
        "entity",
        "show",
        &e2b_node(&home, "g1", "person", "Solo", ""),
    ]);
    assert!(issues(&same, Code::EntityTypesMixed).is_empty());
}

/// ws008 E2b F4 (graph-addressing-expert): `entity` is the address of the member with the lowest (graph id, node id),
/// whatever order the graphs are listed in; `entity_id` is the same member by graph id; both give the same entity when
/// asked; `keys` is the shared key.
#[test]
fn the_entity_is_its_lowest_member_by_graph_id() {
    let mut home = Home::new("e2b-id");
    // Made z, y, x: z's graph id is the lowest, while the scope lists them x, y, z.
    e2b_graphs(&mut home, &["z", "y", "x"]);
    let key = "email:ann@example.com";
    let n: Vec<String> = ["z", "y", "x"]
        .iter()
        .map(|g| e2b_node(&home, g, "person", "Ann", key))
        .collect();
    let shown = home.ok(&["entity", "show", &n[2]]);
    assert_eq!(shown.data["entity"], json!(n[0]), "{}", shown.to_json());
    let gid = home.ok(&["graph", "list"]).data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["alias"] == "z")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let node = n[0].rsplit('/').next().unwrap();
    assert_eq!(shown.data["entity_id"], json!(format!("umx:{gid}/{node}")));
    assert_eq!(shown.data["keys"], json!([key]));
    let mut want = e2_members(&shown);
    want.sort();
    for asked in [&shown.data["entity"], &shown.data["entity_id"]] {
        let again = home.ok(&["entity", "show", asked.as_str().unwrap()]);
        let mut got = e2_members(&again);
        got.sort();
        assert_eq!(got, want, "{asked}");
        assert_eq!(again.data["entity"], shown.data["entity"]);
    }
}

/// ws008 E2b F4: a hidden bridge (a node in a secured graph the read does not name, made first so its graph id is the
/// lowest) is never the entity's lead.
#[test]
fn a_hidden_bridge_never_leads_the_entity() {
    let mut home = Home::new("e2b-bridge");
    home.secured_graph();
    let hidden = home.ok(&[
        "node", "add", "--graph", "demo", "--type", "person", "--title", "Jo",
    ]);
    let hidden = hidden.data["node"]["address"].as_str().unwrap().to_owned();
    e2b_graphs(&mut home, &["g1", "g2"]);
    let a = e2b_node(&home, "g1", "person", "Jo", "");
    let b = e2b_node(&home, "g2", "person", "Jo", "");
    home.ok(&["entity", "link", &a, &hidden]);
    home.ok(&["entity", "link", &b, &hidden]);
    let shown = home.ok(&["entity", "show", &a]);
    assert_eq!(
        e2_members(&shown),
        [a.clone(), b.clone()],
        "{}",
        shown.to_json()
    );
    let lead = shown.data["entity"].as_str().unwrap();
    assert!(lead == a || lead == b, "{}", shown.to_json());
    let id = hidden.rsplit('/').next().unwrap();
    assert!(!shown.to_json().contains(id), "{}", shown.to_json());
}

/// ws008 E2b F8 and F7: domain keys are caseless and url keys lowercase their scheme and host; a hand-written
/// mixed-case domain loads as not in stored form and is still found; a key that is the node's own title warns
/// `entity_key_is_title`, and another value does not.
#[test]
fn domain_and_url_keys_have_one_form_and_a_title_key_warns() {
    let mut home = Home::new("e2b-keys");
    let path = home.graph();
    let added = home.ok(&[
        "node",
        "add",
        "--type",
        "organisation",
        "--title",
        "Contoso",
        "--set",
        "entity_key=domain:Microsoft.com,url:HTTPS://Example.COM/Path",
    ]);
    assert_eq!(
        added.data["node"]["entity_key"],
        json!(["domain:microsoft.com", "url:https://example.com/Path"]),
        "{}",
        added.to_json()
    );
    assert!(issues(&added, Code::EntityKeyIsTitle).is_empty());
    let id = added.data["node"]["id"].as_str().unwrap().to_owned();
    let file = Path::new(&path).join("nodes.jsonl");
    let text = fs::read_to_string(&file).unwrap();
    fs::write(
        &file,
        text.replace("domain:microsoft.com", "domain:MicroSoft.com"),
    )
    .unwrap();
    let loaded = home.ok(&["graph", "load", &path]);
    issue(&loaded, Code::EntityKeyNotCanonical);
    let found = home.ok(&[
        "node",
        "list",
        "--where",
        "entity_key=domain:MICROSOFT.com.",
    ]);
    assert_eq!(
        found.data["rows"][0]["id"],
        json!(id),
        "{}",
        found.to_json()
    );

    let titled = home.ok(&[
        "node",
        "add",
        "--type",
        "person",
        "--title",
        "Jo  Smith",
        "--set",
        "entity_key=name:jo smith",
    ]);
    let warned = issue(&titled, Code::EntityKeyIsTitle);
    assert!(warned.message().contains("name"), "{}", warned.message());
    assert!(!warned.message().contains("jo smith"), "never the value");
    let other = home.ok(&[
        "node",
        "add",
        "--type",
        "person",
        "--title",
        "Jo Smith",
        "--set",
        "entity_key=email:jo@example.com",
    ]);
    assert!(issues(&other, Code::EntityKeyIsTitle).is_empty());
    let batch = home.ops_file(
        "{\"op\":\"node add\",\"type\":\"person\",\"title\":\"Ann Lee\",\"sets\":[{\"field\":\"entity_key\",\"value\":\"name:ann lee\"}]}\n",
    );
    let (env, exit) = home.umx(&["batch", "--file", &batch]);
    assert_eq!(exit, 0, "{}", env.to_json());
    issue(&env, Code::EntityKeyIsTitle);
}

/// ws008 E2b G5 (the project manager, curator field report): an organisation in g1 and a team in g2 share a domain key,
/// and an unnamed secured graph holds a third type with it. `entity show`, the key lookup and `umx check` each warn
/// `entity_type_mismatch` naming both types and g1 and g2, never the secured graph or its node; next retypes.
#[test]
fn the_same_key_on_two_types_is_reported_and_never_names_an_uncovered_graph() {
    let mut home = Home::new("e2b-types");
    home.secured_graph();
    let hidden = home.ok(&[
        "node",
        "add",
        "--graph",
        "demo",
        "--type",
        "club",
        "--title",
        "Acme club",
        "--set",
        "entity_key=domain:acme.example",
    ]);
    let hidden_id = hidden.data["node"]["id"].as_str().unwrap().to_owned();
    e2b_graphs(&mut home, &["g1", "g2"]);
    let org = e2b_node(&home, "g1", "organisation", "Acme", "domain:acme.example");
    e2b_node(&home, "g2", "team", "Acme team", "domain:acme.example");
    let check = |env: &Envelope, what: &str| {
        let i = issue(env, Code::EntityTypeMismatch);
        let text = format!("{} {}", i.message(), i.next());
        for want in ["organisation", "team", "g1", "g2"] {
            assert!(text.contains(want), "{what}: {want} in {text}");
        }
        assert!(
            i.next().contains("node edit") && i.next().contains("--type"),
            "{what}: {}",
            i.next()
        );
        let all = env.to_json();
        assert!(
            !all.contains("club") && !all.contains("demo") && !all.contains(&hidden_id),
            "{what}: {all}"
        );
    };
    check(&home.ok(&["entity", "show", &org]), "entity show");
    let listed = home.ok(&["node", "list", "--where", "entity_key=domain:acme.example"]);
    check(&listed, "node list");
    check(&home.ok(&["check", "--graph", "g1,g2"]), "check");
}

/// ws008 E2b F9: with two open graphs and a secured one loaded and none named, `node list --where entity_key=` lists
/// both open graphs' holders with their graphs; any other unnamed list still asks for a graph.
#[test]
fn a_key_lookup_covers_the_open_graphs_by_default() {
    let mut home = Home::new("e2b-f9");
    home.secured_graph();
    home.ok(&[
        "node",
        "add",
        "--graph",
        "demo",
        "--type",
        "person",
        "--title",
        "Jo",
        "--set",
        "entity_key=email:jo@example.com",
    ]);
    e2b_graphs(&mut home, &["g1", "g2"]);
    let a = e2b_node(&home, "g1", "person", "Jo", "email:jo@example.com");
    let b = e2b_node(&home, "g2", "person", "Jo", "email:jo@example.com");
    let listed = home.ok(&["node", "list", "--where", "entity_key=email:jo@example.com"]);
    let ids: Vec<&str> = listed.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    let id = |s: &str| s.rsplit('/').next().unwrap().to_owned();
    assert_eq!(ids, [id(&a), id(&b)], "{}", listed.to_json());
    assert!(
        !listed.to_json().contains("\"demo\""),
        "{}",
        listed.to_json()
    );
    let (plain, exit) = home.umx(&["node", "list", "--where", "status=open"]);
    assert_eq!(exit, 1, "{}", plain.to_json());
    assert_eq!(
        plain.errors[0].code(),
        "graph_ambiguous",
        "{}",
        plain.to_json()
    );
}

/// ws008 E2b F179: `entity link --dry-run` gives what a real link would (the graph, the sentence, its warnings, each
/// once) and writes nothing; an existing link dry-runs as unchanged; a refused link is refused the same way.
#[test]
fn an_entity_link_dry_run_checks_and_writes_nothing() {
    let mut home = Home::new("e2b-f179");
    e2b_graphs(&mut home, &["g1", "g2"]);
    let p = e2b_node(&home, "g1", "person", "Acme", "");
    let o = e2b_node(&home, "g2", "organisation", "Acme", "");
    // F187 (graph-storage-expert): every content file of both graphs, not only the edges.
    let edges = || -> Vec<String> {
        ["g1", "g2"]
            .iter()
            .flat_map(|g| {
                ["graph.json", "nodes.jsonl", "edges.jsonl"].map(|f| home.path.join(g).join(f))
            })
            .map(|p| fs::read_to_string(p).unwrap_or_default())
            .collect()
    };
    let before = edges();
    let dry = home.ok(&["entity", "link", &p, &o, "--dry-run"]);
    assert_eq!(
        (&dry.data["dry_run"], &dry.data["edge"], &dry.data["graph"]),
        (&json!(true), &Value::Null, &json!("g1")),
        "{}",
        dry.to_json()
    );
    assert!(
        dry.data["sentence"].as_str().unwrap().contains("same as"),
        "{}",
        dry.to_json()
    );
    assert_eq!(
        issues(&dry, Code::EntityTypeMismatch).len(),
        1,
        "{}",
        dry.to_json()
    );
    assert_eq!(edges(), before, "nothing written");
    assert_eq!(e2_members(&home.ok(&["entity", "show", &p])), [p.as_str()]);
    let real = home.ok(&["entity", "link", &p, &o]);
    assert_eq!(real.data["dry_run"], false);
    let again = home.ok(&["entity", "link", &o, &p, "--dry-run"]);
    assert_eq!(again.data["unchanged"], true, "{}", again.to_json());
    assert_eq!(again.data["edge"]["id"], real.data["edge"]["id"]);
    let (env, exit) = home.umx(&["entity", "link", &p, &p, "--dry-run"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::EntityLinkInvalid);
}

/// ws008 E2b F180: a batch `edge add` line of type same_as between two types warns `entity_type_mismatch`, in a dry
/// run and a real one, as `entity link` does; between two nodes of one type it does not.
#[test]
fn a_batch_same_as_line_between_two_types_warns() {
    let mut home = Home::new("e2b-f180");
    e2b_graphs(&mut home, &["g1", "g2"]);
    let p = e2b_node(&home, "g1", "person", "Acme", "");
    let o = e2b_node(&home, "g2", "organisation", "Acme", "");
    let q = e2b_node(&home, "g1", "person", "Acme too", "");
    let line = |a: &str, b: &str| {
        home.ops_file(&format!(
            "{{\"op\":\"edge add\",\"from\":\"{a}\",\"type\":\"same_as\",\"to\":\"{b}\"}}\n"
        ))
    };
    for dry in [true, false] {
        let file = line(&p, &o);
        let mut args = vec!["batch", "--graph", "g1", "--file", file.as_str()];
        if dry {
            args.push("--dry-run");
        }
        let env = home.ok(&args);
        let i = issue(&env, Code::EntityTypeMismatch);
        assert!(
            i.message().contains("person") && i.message().contains("organisation"),
            "{dry}: {}",
            i.message()
        );
    }
    let file = line(&p, &q);
    let same = home.ok(&["batch", "--graph", "g1", "--file", file.as_str()]);
    assert!(
        issues(&same, Code::EntityTypeMismatch).is_empty(),
        "{}",
        same.to_json()
    );
}

/// R2: the entity address names the same entity from every member, also when the lower graph id is the graph
/// where the doubly held key is NOT held twice (review T6 with the graph ids reversed). Key joins are symmetric: a
/// key two nodes of one type hold in a graph joins neither of them anywhere.
#[test]
fn the_entity_address_round_trips_from_every_member() {
    let mut home = Home::new("e2b-r2");
    // `low` is made first, so its graph id is the lower one.
    e2b_graphs(&mut home, &["low", "high"]);
    let (ka, kb) = ("email:ann@example.com", "email:bob@example.com");
    let a2 = e2b_node(&home, "low", "person", "Ann", ka);
    let b2 = e2b_node(&home, "low", "person", "Bob", kb);
    let b1 = e2b_node(&home, "high", "person", "Bob", &format!("{ka},{kb}"));
    let a1 = e2b_node(&home, "high", "person", "Ann", ka);
    let sorted = |env: &Envelope| {
        let mut m = e2_members(env);
        m.sort();
        m
    };
    for (asked, want) in [
        (&b2, vec![b2.clone(), b1.clone()]),
        (&b1, vec![b2.clone(), b1.clone()]),
        (&a2, vec![a2.clone()]),
        (&a1, vec![a1.clone()]),
    ] {
        let shown = home.ok(&["entity", "show", asked]);
        let mut want = want.clone();
        want.sort();
        assert_eq!(sorted(&shown), want, "from {asked}: {}", shown.to_json());
        for again in [&shown.data["entity"], &shown.data["entity_id"]] {
            let back = home.ok(&["entity", "show", again.as_str().unwrap()]);
            assert_eq!(
                sorted(&back),
                want,
                "{again} from {asked}: {}",
                back.to_json()
            );
            assert_eq!(back.data["entity"], shown.data["entity"]);
        }
    }
}

/// R3: two nodes of one graph hold the same key and are joined by a same_as link. The key is held twice there, so
/// it joined neither; G1 names the link as what joined them, and no member lists the key as a match.
#[test]
fn g1_names_the_link_when_a_doubly_held_key_did_not_join() {
    let mut home = Home::new("e2b-r3");
    e2b_graphs(&mut home, &["g1"]);
    let key = "email:jo@example.com";
    let p = e2b_node(&home, "g1", "person", "Jo", key);
    let q = e2b_node(&home, "g1", "person", "Jo S", key);
    let linked = home.ok(&["entity", "link", &p, &q]);
    let edge = linked.data["edge"]["id"].as_str().unwrap().to_owned();
    let shown = home.ok(&["entity", "show", &p]);
    let twice = issue(&shown, Code::EntityGraphTwice);
    assert!(
        twice.message().contains("same_as link") && twice.message().contains(&edge),
        "{}",
        twice.message()
    );
    assert!(!twice.message().contains("scheme"), "{}", twice.message());
    assert!(
        !shown.to_json().contains("\"by\":\"entity_key\""),
        "{}",
        shown.to_json()
    );
}

/// R4: the overlap check reads the links a named secured graph stores: A (open g1) is linked to C in the secured
/// graph, stored there; linking A to B in that secured graph, addressed by alias, warns `entity_link_overlap` naming C.
#[test]
fn the_overlap_check_sees_links_a_named_secured_graph_stores() {
    let mut home = Home::new("e2b-r4");
    home.secured_graph();
    let add = |home: &Home, title: &str| {
        home.ok(&[
            "node", "add", "--graph", "demo", "--type", "person", "--title", title,
        ])
        .data["node"]["address"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (b, c) = (add(&home, "Jo B"), add(&home, "Jo C"));
    e2b_graphs(&mut home, &["g1"]);
    let a = e2b_node(&home, "g1", "person", "Jo", "");
    home.ok(&["entity", "link", &a, &c, "--graph", "demo"]);
    let linked = home.ok(&["entity", "link", &a, &b]);
    let overlap = issue(&linked, Code::EntityLinkOverlap);
    let c_id = c.rsplit('/').next().unwrap();
    assert!(overlap.message().contains(c_id), "{}", overlap.message());
}

/// R5: copies of an unrelated graph with no primary do not refuse a link between two addressed nodes; the
/// overlap check only warns, so it is skipped when its scope cannot be built.
#[test]
fn unrelated_copies_with_no_primary_never_refuse_a_link() {
    let mut home = Home::new("e2b-r5");
    home.design();
    home.design_copy("design_b");
    home.design_copy("design_c");
    home.ok(&["graph", "unload", "design"]);
    e2b_graphs(&mut home, &["g1", "g2"]);
    let a = e2b_node(&home, "g1", "person", "Jo", "");
    let b = e2b_node(&home, "g2", "person", "Jo", "");
    let (env, exit) = home.umx(&["entity", "link", &a, &b]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(env.data["unchanged"], false);
}

/// R1 and graph-storage-expert's F187: `entity link --dry-run` naming no graph, with two loaded, is graph_ambiguous;
/// its next, run as printed, is still a dry run, and nothing in either graph's nodes, edges or graph.json changes.
#[test]
fn a_dry_run_link_stays_a_dry_run_through_its_retry() {
    let mut home = Home::new("e2b-r1");
    e2b_graphs(&mut home, &["g1", "g2"]);
    let a = e2b_node(&home, "g1", "person", "Jo", "");
    let b = e2b_node(&home, "g1", "person", "Jo S", "");
    let files = |home: &Home| -> Vec<String> {
        ["g1", "g2"]
            .iter()
            .flat_map(|g| {
                ["graph.json", "nodes.jsonl", "edges.jsonl"].map(|f| home.path.join(g).join(f))
            })
            .map(|p| fs::read_to_string(p).unwrap_or_default())
            .collect()
    };
    let before = files(&home);
    let bare = |addr: &str| addr.rsplit('/').next().unwrap().to_owned();
    let (env, exit) = home.umx(&["entity", "link", &bare(&a), &bare(&b), "--dry-run"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let next = issue(&env, Code::GraphAmbiguous).next().to_string();
    assert!(next.contains("--dry-run"), "{next}");
    let (env, exit) = run_as_printed(&home, &next);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(env.data["dry_run"], true, "{}", env.to_json());
    assert_eq!(files(&home), before, "nothing written");
}

/// R6: `umx check` reports G5 from the files it just read: a type changed on disk after the load is seen, and a
/// conflict fixed on disk is no longer reported.
#[test]
fn check_reports_key_types_from_the_files_it_reads() {
    let mut home = Home::new("e2b-r6");
    e2b_graphs(&mut home, &["g1", "g2"]);
    e2b_node(&home, "g1", "organisation", "Acme", "domain:acme.example");
    e2b_node(&home, "g2", "organisation", "Acme", "domain:acme.example");
    let nodes = home.path.join("g2/nodes.jsonl");
    let retype = |from: &str, to: &str| {
        let text = fs::read_to_string(&nodes).unwrap();
        fs::write(
            &nodes,
            text.replace(
                &format!("\"type\":\"{from}\""),
                &format!("\"type\":\"{to}\""),
            ),
        )
        .unwrap();
    };
    let clean = home.ok(&["check", "--graph", "g1,g2"]);
    assert!(
        issues(&clean, Code::EntityTypeMismatch).is_empty(),
        "{}",
        clean.to_json()
    );
    retype("organisation", "team");
    let changed = home.ok(&["check", "--graph", "g1,g2"]);
    let i = issue(&changed, Code::EntityTypeMismatch);
    assert!(i.message().contains("team"), "{}", i.message());
    retype("team", "organisation");
    let fixed = home.ok(&["check", "--graph", "g1,g2"]);
    assert!(
        issues(&fixed, Code::EntityTypeMismatch).is_empty(),
        "{}",
        fixed.to_json()
    );
}

/// R8, R9 and R10: a domain given with repeated root dots is found by the text it was given; a key lookup
/// written with spaces around `=` covers the open graphs; every key equality is checked for G5, not only the first.
#[test]
fn key_lookups_parse_as_the_filter_does_and_check_every_key() {
    let mut home = Home::new("e2b-r9");
    e2b_graphs(&mut home, &["g1", "g2"]);
    let given = "domain:Acme.EXAMPLE..";
    let o = e2b_node(
        &home,
        "g1",
        "organisation",
        "Acme",
        &format!("{given},email:ops@acme.example"),
    );
    e2b_node(&home, "g2", "team", "Ops", "email:ops@acme.example");
    let found = home.ok(&[
        "node",
        "list",
        "--graph",
        "g1",
        "--where",
        &format!("entity_key={given}"),
    ]);
    assert_eq!(
        found.data["rows"][0]["id"],
        json!(o.rsplit('/').next().unwrap()),
        "{}",
        found.to_json()
    );
    let spaced = home.ok(&[
        "node",
        "list",
        "--where",
        "entity_key = email:ops@acme.example",
    ]);
    assert_eq!(
        spaced.data["rows"].as_array().unwrap().len(),
        2,
        "{}",
        spaced.to_json()
    );
    let later = home.ok(&[
        "node",
        "list",
        "--graph",
        "g1,g2",
        "--type",
        "organisation",
        "--where",
        "entity_key=domain:acme.example",
        "--where",
        "entity_key=email:ops@acme.example",
    ]);
    let i = issue(&later, Code::EntityTypeMismatch);
    assert!(i.message().contains("team"), "{}", i.message());
}

/// ws008 E2c (req-137): `entity deny` stores one not_same_as edge with its reason and the writer's stamps; a key no
/// longer joins the pair, and `entity show` says which join it skipped and why; a repeat is unchanged and keeps the
/// first reason; `entity undeny` removes it and the key joins again.
#[test]
fn a_denial_splits_an_entity_until_it_is_removed() {
    let mut home = Home::new("e2c-deny");
    e2b_graphs(&mut home, &["g1", "g2"]);
    let key = "email:jo@example.com";
    let a = e2b_node(&home, "g1", "person", "Jo", key);
    let b = e2b_node(&home, "g2", "person", "Jo", key);
    assert_eq!(e2_members(&home.ok(&["entity", "show", &a])).len(), 2);
    let denied = home.ok(&[
        "entity",
        "deny",
        &a,
        &b,
        "--reason",
        "two different people named Jo",
    ]);
    let edge = &denied.data["edge"];
    assert_eq!(
        (
            &edge["type"],
            &edge["graph"],
            &edge["reason"],
            &denied.data["unchanged"]
        ),
        (
            &json!("not_same_as"),
            &json!("g1"),
            &json!("two different people named Jo"),
            &json!(false)
        ),
        "{}",
        denied.to_json()
    );
    assert!(
        edge["created_by"].is_string() && edge["authored_by_session"].is_string(),
        "{}",
        denied.to_json()
    );
    let shown = home.ok(&["entity", "show", &a]);
    assert_eq!(e2_members(&shown), [a.as_str()], "{}", shown.to_json());
    let skipped = issue(&shown, Code::EntityJoinDenied);
    let edge_id = edge["id"].as_str().unwrap();
    assert!(
        skipped.message().contains("scheme email") && skipped.message().contains(edge_id),
        "{}",
        skipped.message()
    );
    assert!(
        !skipped.message().contains("jo@example.com"),
        "never the key's value"
    );
    assert!(
        skipped.next().starts_with("umx entity undeny"),
        "{}",
        skipped.next()
    );
    let again = home.ok(&["entity", "deny", &b, &a, "--reason", "something else"]);
    assert_eq!(again.data["unchanged"], true, "{}", again.to_json());
    assert_eq!(
        again.data["edge"]["reason"],
        "two different people named Jo"
    );
    let gone = home.ok(&["entity", "undeny", &b, &a]);
    assert_eq!(gone.data["deleted"], true, "{}", gone.to_json());
    assert_eq!(e2_members(&home.ok(&["entity", "show", &a])).len(), 2);
    assert_eq!(
        home.ok(&["entity", "undeny", &a, &b]).data["unchanged"],
        true
    );
}

/// ws008 E2c (security): the reason is required and checked like evidence remove's, and a secret-looking reason
/// warns; `reason` is reserved on edges, so `--set` cannot forge one.
#[test]
fn a_denial_needs_a_clean_reason_and_reason_is_reserved() {
    let mut home = Home::new("e2c-reason");
    e2b_graphs(&mut home, &["g1"]);
    let (a, b) = (
        e2b_node(&home, "g1", "person", "Jo", ""),
        e2b_node(&home, "g1", "person", "Jo B", ""),
    );
    let long = "x".repeat(501);
    for args in [
        vec!["entity", "deny", a.as_str(), b.as_str()],
        vec!["entity", "deny", a.as_str(), b.as_str(), "--reason", "   "],
        vec![
            "entity",
            "deny",
            a.as_str(),
            b.as_str(),
            "--reason",
            long.as_str(),
        ],
        vec![
            "entity",
            "deny",
            a.as_str(),
            b.as_str(),
            "--reason",
            "bad\u{202E}text",
        ],
    ] {
        let (env, exit) = home.umx(&args);
        assert_ne!(exit, 0, "{}", env.to_json());
        assert_eq!(env.errors[0].code(), "usage_invalid", "{}", env.to_json());
    }
    let secret = home.ok(&[
        "entity",
        "deny",
        &a,
        &b,
        "--reason",
        "token ghp_0123456789abcdefghijklmnopqrstuvwxyzAB",
    ]);
    issue(&secret, Code::SecretSuspected);
    let c = e2b_node(&home, "g1", "person", "Jo C", "");
    let (env, exit) = home.umx(&["edge", "add", &a, "blocks", &c, "--set", "reason=forged"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert_eq!(env.errors[0].code(), "reserved_name", "{}", env.to_json());
}

/// ws008 E2c (graph-addressing-expert, security): what deny refuses, and the writes that would bypass it: a node with
/// itself, a graph holding neither node, a pair this graph links; `entity link` on a denied pair (next undeny); `edge
/// add`, `edge edit`, `edge delete` and a batch line of not_same_as (next entity deny or undeny). Nothing is written.
#[test]
fn deny_refusals_and_edge_writes_of_a_denial_are_refused() {
    let mut home = Home::new("e2c-refuse");
    e2b_graphs(&mut home, &["g1", "g2", "g3"]);
    let a = e2b_node(&home, "g1", "person", "Jo", "");
    let b = e2b_node(&home, "g2", "person", "Jo", "");
    let c = e2b_node(&home, "g1", "person", "Jo C", "");
    let refused = |home: &Home, args: &[&str], next: &str| {
        let (env, exit) = home.umx(args);
        assert_eq!(exit, 1, "{args:?}: {}", env.to_json());
        let i = issue(&env, Code::EntityLinkInvalid);
        assert!(i.next().starts_with(next), "{args:?}: {}", i.next());
    };
    let r = ["--reason", "not the same"];
    refused(
        &home,
        &[&["entity", "deny", &a, &a][..], &r].concat(),
        "umx docs get entity",
    );
    refused(
        &home,
        &[&["entity", "deny", &a, &b, "--graph", "g3"][..], &r].concat(),
        "umx entity deny",
    );
    home.ok(&["entity", "link", &a, &c]);
    refused(
        &home,
        &[&["entity", "deny", &a, &c][..], &r].concat(),
        "umx entity unlink",
    );
    home.ok(&[&["entity", "deny", &a, &b][..], &r].concat());
    refused(&home, &["entity", "link", &b, &a], "umx entity undeny");
    refused(
        &home,
        &["edge", "add", &a, "not_same_as", &c],
        "umx entity deny",
    );
    let denial = home
        .ok(&["edge", "list", "--graph", "g1", "--type", "not_same_as"])
        .data["rows"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    refused(
        &home,
        &["edge", "delete", &denial, "--graph", "g1"],
        "umx entity undeny",
    );
    refused(
        &home,
        &[
            "edge",
            "edit",
            &denial,
            "--relation",
            "same_as",
            "--graph",
            "g1",
        ],
        "umx entity undeny",
    );
    // R6 (review): a batch line keeps the deny or undeny fix; no batch line can write a denial.
    for (line, next) in [
        (
            json!({"op":"edge add","from":a,"type":"not_same_as","to":c}),
            "umx entity deny",
        ),
        (
            json!({"op":"edge edit","edge":denial,"relation":"same_as"}),
            "umx entity undeny",
        ),
        (
            json!({"op":"edge delete","edge":denial}),
            "umx entity undeny",
        ),
    ] {
        let ops = home.ops_file(&format!("{line}\n"));
        let (env, exit) = home.umx(&["batch", "--graph", "g1", "--file", &ops]);
        assert_eq!(exit, 1, "{line}: {}", env.to_json());
        let i = issue(&env, Code::EntityLinkInvalid);
        assert!(i.next().starts_with(next), "{line}: {}", i.next());
    }
    assert_eq!(
        home.ok(&["edge", "list", "--graph", "g1", "--type", "not_same_as"])
            .data["rows"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// ws008 E2c (security): a far node the writer cannot read is refused exactly as an unknown node. One address, the far
/// graph's id with a complete node id, gives the same bytes whether that graph is unloaded, loaded without the node,
/// or never existed. R2 (review): so does a node id prefix, which only a loaded graph can resolve.
#[test]
fn a_denial_of_an_unreadable_far_node_is_refused_as_unknown_byte_for_byte() {
    let mut home = Home::new("e2c-far");
    e2b_graphs(&mut home, &["g1", "g2"]);
    let a = e2b_node(&home, "g1", "person", "Jo", "");
    let b = e2b_node(&home, "g2", "person", "Jo", "");
    let gid = e2c_gid(&home, "g2");
    // A complete node id g2 does not hold.
    let ghost = "01JC8XA4H7B3KT9M2NZ5QW6RDE";
    let far = format!("umx:{gid}/{ghost}");
    let deny = |home: &Home, far: &str| {
        let (env, exit) = home.umx(&["entity", "deny", &a, far, "--reason", "not the same"]);
        assert_eq!(exit, 1, "{}", env.to_json());
        let v: Value = umx_protocol::parse_json(&env.to_json()).unwrap();
        v["errors"].to_string()
    };
    let loaded = deny(&home, &far);
    assert!(loaded.contains("node_not_found"), "{loaded}");
    let prefix = format!("umx:{gid}/{}", &ghost[..6]);
    let prefix_loaded = deny(&home, &prefix);
    assert!(prefix_loaded.contains("node_not_found"), "{prefix_loaded}");
    // Security N1: the same address with g2 loaded and secured gives the same bytes.
    home.ok(&["graph", "set", "g2", "--policy", "secured"]);
    assert_eq!(deny(&home, &far), loaded, "loaded and secured");
    let g2 = home.path.join("g2").display().to_string();
    home.ok(&["graph", "unload", "g2"]);
    assert_eq!(deny(&home, &far), loaded, "unloaded");
    assert_eq!(deny(&home, &prefix), prefix_loaded, "a prefix, unloaded");
    let never = format!("umx:01JC8XA4H7B3KT9M2NZ5QW6RDF/{ghost}");
    assert_eq!(
        deny(&home, &never).replace("01JC8XA4H7B3KT9M2NZ5QW6RDF", &gid),
        loaded,
        "a graph that never existed: only the address given differs"
    );
    home.ok(&["graph", "load", &g2]);
    // Control: a far node the writer can read is denied.
    home.ok(&["entity", "deny", &a, &b, "--reason", "not the same"]);
}

/// ws008 E2c: a denial replaces a possibly_same_as between the pair stored in the same graph, in one write; a
/// proposal stored in another graph is not touched, but a read drops it while the denial counts.
#[test]
fn a_denial_replaces_a_proposal_in_its_graph_and_drops_one_from_reads() {
    let mut home = Home::new("e2c-propose");
    e2b_graphs(&mut home, &["g1", "g2"]);
    let a = e2b_node(&home, "g1", "person", "Jo", "");
    let b = e2b_node(&home, "g2", "person", "Jo", "");
    let here = home.ok(&["edge", "add", &a, "possibly_same_as", &b, "--graph", "g1"]);
    home.ok(&["edge", "add", &a, "possibly_same_as", &b, "--graph", "g2"]);
    let denied = home.ok(&["entity", "deny", &a, &b, "--reason", "not the same"]);
    let replaced = format!("umx:g1/{}", here.data["edge"]["id"].as_str().unwrap());
    assert_eq!(
        denied.data["replaced"],
        json!(replaced),
        "{}",
        denied.to_json()
    );
    let kinds = |g: &str| -> Vec<String> {
        home.ok(&["edge", "list", "--graph", g]).data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["type"].as_str().unwrap().to_owned())
            .filter(|t| t.contains("same_as"))
            .collect()
    };
    assert_eq!(kinds("g1"), ["not_same_as"]);
    assert_eq!(kinds("g2"), ["possibly_same_as"]);
    let shown = home.ok(&["entity", "show", &a]);
    assert_eq!(shown.data["proposed"], json!([]), "{}", shown.to_json());
}

/// ws008 E2c (graph-addressing-expert's order): keys join by (type, key), holders by (graph id, node id), then links by
/// (storing graph id, edge id); a join that would put a denied pair together is skipped. Three graphs hold one key; A
/// and B are denied: A (lowest graph id) anchors the key, so A joins C and B stays apart, from whichever member is
/// asked.
#[test]
fn denials_split_in_one_fixed_order_from_every_member() {
    let mut home = Home::new("e2c-order");
    e2b_graphs(&mut home, &["g1", "g2", "g3"]);
    let key = "email:jo@example.com";
    let a = e2b_node(&home, "g1", "person", "Jo", key);
    let b = e2b_node(&home, "g2", "person", "Jo", key);
    let c = e2b_node(&home, "g3", "person", "Jo", key);
    home.ok(&["entity", "deny", &a, &b, "--reason", "not the same"]);
    let members = |asked: &str| {
        let mut m = e2_members(&home.ok(&["entity", "show", asked]));
        m.sort();
        m
    };
    let mut ac = vec![a.clone(), c.clone()];
    ac.sort();
    assert_eq!(members(&a), ac);
    assert_eq!(members(&c), ac);
    assert_eq!(members(&b), std::slice::from_ref(&b));
}

/// ws008 E2c (security): a denial counts only while its graph and both its nodes are covered. A denial of A and a node
/// C in an unnamed secured graph is ignored silently: A, C (a hidden bridge, by links) and B stay one entity, nothing
/// names the denial, and nothing names C. Named, the denial splits A off.
#[test]
fn a_denial_with_an_uncovered_node_is_ignored_and_never_named() {
    let mut home = Home::new("e2c-cover");
    home.secured_graph();
    let c = home
        .ok(&[
            "node", "add", "--graph", "demo", "--type", "person", "--title", "Jo",
        ])
        .data["node"]["address"]
        .as_str()
        .unwrap()
        .to_owned();
    e2b_graphs(&mut home, &["g1", "g2"]);
    let a = e2b_node(&home, "g1", "person", "Jo", "");
    let b = e2b_node(&home, "g2", "person", "Jo", "");
    // Both links stored in g2; the denial of A and C stored in g1.
    home.ok(&["entity", "link", &a, &c, "--graph", "g2"]);
    home.ok(&["entity", "link", &b, &c, "--graph", "g2"]);
    home.ok(&[
        "entity",
        "deny",
        &a,
        &c,
        "--reason",
        "not the same",
        "--graph",
        "g1",
    ]);
    let shown = home.ok(&["entity", "show", &a]);
    assert_eq!(
        e2_members(&shown),
        [a.clone(), b.clone()],
        "{}",
        shown.to_json()
    );
    assert!(
        issues(&shown, Code::EntityJoinDenied).is_empty(),
        "{}",
        shown.to_json()
    );
    assert!(
        !shown.to_json().contains(&e2c_id(&c)),
        "{}",
        shown.to_json()
    );
    let named = home.ok(&["entity", "show", &a, "--graph", "g1,g2,demo"]);
    assert_eq!(
        e2_members(&named),
        std::slice::from_ref(&a),
        "{}",
        named.to_json()
    );
    issue(&named, Code::EntityJoinDenied);
}

/// S323-1 (graph-storage-expert): a denial that would replace a proposal whose id edges.jsonl repeats is refused with
/// edge_repeated, as `edge delete` is, and nothing changes; the repeated line would bring the proposal back on load.
#[test]
fn a_denial_never_replaces_a_repeated_proposal() {
    let mut home = Home::new("e2c-repeat");
    e2b_graphs(&mut home, &["g1"]);
    let a = e2b_node(&home, "g1", "person", "Jo", "");
    let b = e2b_node(&home, "g1", "person", "Jo B", "");
    let id = home
        .ok(&["edge", "add", &a, "possibly_same_as", &b, "--graph", "g1"])
        .data["edge"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = home.path.join("g1");
    let edges = path.join("edges.jsonl");
    let text: String = fs::read_to_string(&edges)
        .unwrap()
        .lines()
        .map(|line| {
            if line.contains(&id) {
                format!("{line}\n{line}\n")
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    fs::write(&edges, text).unwrap();
    home.ok(&["graph", "load", path.to_str().unwrap()]);
    let before = fs::read_to_string(&edges).unwrap();
    let (env, exit) = home.umx(&["entity", "deny", &a, &b, "--reason", "not the same"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::EdgeRepeated);
    assert_eq!(
        fs::read_to_string(&edges).unwrap(),
        before,
        "nothing written"
    );
}

/// R1 (review): a stored denial is found from the ends the author resolved, so a bare end in the author's graph
/// plus an addressed far end, reversed and stored from the other graph, meets the denial the first graph stores: deny
/// is unchanged and keeps its reason, and link is refused.
#[test]
fn a_denial_is_found_from_a_bare_end_and_an_addressed_far_end_reversed() {
    let mut home = Home::new("e2c-r1");
    e2b_graphs(&mut home, &["g1", "g2"]);
    let a = e2b_node(&home, "g1", "person", "Jo", "");
    let b = e2b_node(&home, "g2", "person", "Jo", "");
    home.ok(&["entity", "deny", &a, &b, "--reason", "first reason"]);
    let again = home.ok(&[
        "entity",
        "deny",
        &e2c_id(&b),
        &a,
        "--graph",
        "g2",
        "--reason",
        "second reason",
    ]);
    assert_eq!(
        (
            &again.data["unchanged"],
            &again.data["edge"]["graph"],
            &again.data["edge"]["reason"]
        ),
        (&json!(true), &json!("g1"), &json!("first reason")),
        "{}",
        again.to_json()
    );
    let (env, exit) = home.umx(&["entity", "link", &e2c_id(&b), &a, "--graph", "g2"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let i = issue(&env, Code::EntityLinkInvalid);
    assert!(i.next().starts_with("umx entity undeny"), "{}", i.next());
}

/// R1 (review), as E2's links: the reversed bare-and-addressed link is unchanged, so g2 stores nothing.
#[test]
fn a_link_is_found_from_a_bare_end_and_an_addressed_far_end_reversed() {
    let mut home = Home::new("e2c-r1-link");
    e2b_graphs(&mut home, &["g1", "g2"]);
    let c = e2b_node(&home, "g1", "person", "Jo C", "");
    let d = e2b_node(&home, "g2", "person", "Jo C", "");
    home.ok(&["entity", "link", &c, &d]);
    let linked = home.ok(&["entity", "link", &e2c_id(&d), &c, "--graph", "g2"]);
    assert_eq!(
        (&linked.data["unchanged"], &linked.data["edge"]["graph"]),
        (&json!(true), &json!("g1")),
        "{}",
        linked.to_json()
    );
    for g in ["g1", "g2"] {
        let rows = home.ok(&["edge", "list", "--graph", g]).data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["type"].as_str().unwrap().contains("same_as"))
            .count();
        assert_eq!(rows, usize::from(g == "g1"), "{g}");
    }
}

/// R4 (review): a key held twice in a graph joins neither holder, as target as well as anchor. A and B in g1
/// share a domain C in g2 holds too; A's email reaches D in g3, and C and D share a name; A and D are denied. The
/// usable keys decide: C and D are one entity, and A stands alone.
#[test]
fn a_key_held_twice_in_a_graph_joins_no_target_under_a_denial() {
    let mut home = Home::new("e2c-r4");
    e2b_graphs(&mut home, &["g1", "g2", "g3"]);
    let a = e2b_node(
        &home,
        "g1",
        "person",
        "Jo",
        "domain:a.example,email:b@example.com",
    );
    e2b_node(&home, "g1", "person", "Jo B", "domain:a.example");
    let c = e2b_node(&home, "g2", "person", "Jo C", "domain:a.example,name:c");
    let d = e2b_node(&home, "g3", "person", "Jo D", "email:b@example.com,name:c");
    home.ok(&["entity", "deny", &a, &d, "--reason", "not the same"]);
    let members = |asked: &str| {
        let mut m = e2_members(&home.ok(&["entity", "show", asked]));
        m.sort();
        m
    };
    let mut cd = vec![c.clone(), d.clone()];
    cd.sort();
    assert_eq!(members(&c), cd);
    assert_eq!(members(&d), cd);
    assert_eq!(members(&a), std::slice::from_ref(&a));
}

/// R5 (review): a proposal into a group the entity is denied with is dropped, even when the walk never reached
/// that group. A is denied with B; B and C share a key; a possibly_same_as between A and C (stored in g3) is not
/// proposed. Without the denial, it is.
#[test]
fn a_proposal_into_a_denied_group_is_dropped() {
    let mut home = Home::new("e2c-r5");
    e2b_graphs(&mut home, &["g1", "g2", "g3"]);
    let key = "email:jo@example.com";
    let a = e2b_node(&home, "g1", "person", "Jo", "");
    let b = e2b_node(&home, "g2", "person", "Jo", key);
    let c = e2b_node(&home, "g3", "person", "Jo", key);
    home.ok(&["edge", "add", &a, "possibly_same_as", &c, "--graph", "g3"]);
    let proposed = |home: &Home| home.ok(&["entity", "show", &a]).data["proposed"].clone();
    assert_eq!(
        proposed(&home).as_array().unwrap().len(),
        1,
        "control: proposed before the denial"
    );
    home.ok(&["entity", "deny", &a, &b, "--reason", "not the same"]);
    assert_eq!(proposed(&home), json!([]));
}

/// ws008 TM4: `batch --raised-in <anchor>` gives every node the batch adds a raised_in edge to it; a node add line's
/// own `raised_in` names its anchor instead; edits and edge lines get none. Rows name each edge and its anchor; a dry
/// run names the anchors and writes nothing.
#[test]
fn a_batch_raised_in_an_anchor_links_every_node_it_adds() {
    let mut home = Home::new("tm4-batch");
    e2b_graphs(&mut home, &["g1", "g2"]);
    let segment = e2b_node(&home, "g1", "segment", "Morning", "");
    let meeting = e2b_node(&home, "g2", "meeting", "Kickoff", "");
    let existing = e2b_node(&home, "g1", "risk", "Old", "");
    let lines = [
        json!({"op":"node add","ref":"a","type":"risk","title":"A"}),
        json!({"op":"node add","ref":"b","type":"risk","title":"B","raised_in":meeting}),
        json!({"op":"node edit","node":existing,"title":"Older"}),
        json!({"op":"edge add","from_ref":"a","type":"blocks","to_ref":"b"}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let ops = home.ops_file(&text);
    let run = |dry: bool| {
        let mut args = vec![
            "batch",
            "--graph",
            "g1",
            "--file",
            ops.as_str(),
            "--raised-in",
            segment.as_str(),
        ];
        if dry {
            args.push("--dry-run");
        }
        home.ok(&args)
    };
    let dry = run(true);
    let rows = dry.data["rows"].as_array().unwrap();
    assert_eq!(rows[0]["raised_in"], json!(segment), "{}", dry.to_json());
    assert!(rows[0]["raised_in_edge"].is_null());
    assert!(
        rows[1]["raised_in"]
            .as_str()
            .unwrap()
            .ends_with(&e2c_id(&meeting))
    );
    assert!(rows[2]["raised_in"].is_null() && rows[3]["raised_in"].is_null());
    assert!(
        tm4_raised(&home, "g1").is_empty(),
        "a dry run writes nothing"
    );
    let done = run(false);
    let rows = done.data["rows"].as_array().unwrap();
    let id = |r: &Value| r["id"].as_str().unwrap().to_owned();
    assert!(rows[0]["raised_in_edge"].is_string() && rows[1]["raised_in_edge"].is_string());
    assert!(rows[2]["raised_in_edge"].is_null() && rows[3]["raised_in_edge"].is_null());
    let mut want = vec![
        (id(&rows[0]), e2c_id(&segment)),
        (id(&rows[1]), e2c_id(&meeting)),
    ];
    want.sort();
    assert_eq!(tm4_raised(&home, "g1"), want);
}

/// F139: `check --lint` finds orphans (an anchor alone is not one), an entity_key on a proposed node and a "lacks no"
/// title, one hint per kind with its next; a plain check has no lint and `--kind` needs `--lint`.
#[test]
fn check_lint_finds_orphans_proposed_keys_and_double_negatives() {
    let mut home = Home::new("f139-kinds");
    let path = home.graph();
    let alone = home.add("problem", "Alone");
    home.ok(&[
        "node",
        "add",
        "--type",
        "segment",
        "--title",
        "Morning",
        "--set",
        "starts_on=2026-11-12",
    ]);
    let (a, b) = (
        home.add("problem", "Plan lacks no owner"),
        home.add("decision", "Ship"),
    );
    home.ok(&["edge", "add", &a, "supports", &b]);
    let keyed = home.add("problem", "Keyed");
    home.ok(&["edge", "add", &keyed, "supports", &b]);
    home.ok(&["node", "edit", &keyed, "--set", "entity_key=sku:1"]);
    home.ok(&["node", "status", &keyed, "proposed", "--reason", "new"]);
    // A key on a node that is not proposed, and a proposed node with no key: neither is a finding.
    home.ok(&["node", "edit", &b, "--set", "entity_key=sku:2"]);
    let bare = home.add("problem", "Bare");
    home.ok(&["edge", "add", &bare, "supports", &b]);
    home.ok(&["node", "status", &bare, "proposed", "--reason", "new"]);
    // R377-2: proposed nodes whose stored entity_key holds no usable key (a hand edit) are not findings either.
    let unusable = [
        ("sku:7", "[null]"),
        ("sku:8", "[\"\"]"),
        ("sku:9", "\"sku:9\""),
    ];
    for (key, _) in unusable {
        let junk = home.add("problem", key);
        home.ok(&["edge", "add", &junk, "supports", &b]);
        home.ok(&["node", "edit", &junk, "--set", &format!("entity_key={key}")]);
        home.ok(&["node", "status", &junk, "proposed", "--reason", "new"]);
    }
    let nodes = Path::new(&path).join("nodes.jsonl");
    let mut text = fs::read_to_string(&nodes).unwrap();
    for (key, stored) in unusable {
        let from = format!("\"entity_key\":[\"{key}\"]");
        assert!(text.contains(&from), "{text}");
        text = text.replace(&from, &format!("\"entity_key\":{stored}"));
    }
    fs::write(&nodes, text).unwrap();
    let plain = home.ok(&["check"]);
    assert!(plain.data.get("lint").is_none(), "{}", plain.to_json());
    let (env, exit) = home.umx(&["check", "--lint"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(
        f139_rows(&env, "orphans"),
        vec![(f139_full(&home, &alone), Value::Null)]
    );
    assert_eq!(
        f139_rows(&env, "proposed_keys"),
        vec![(f139_full(&home, &keyed), json!("field:entity_key"))]
    );
    assert_eq!(
        f139_rows(&env, "double_negatives"),
        vec![(f139_full(&home, &a), json!("field:title"))]
    );
    let orphans = issue(&env, Code::LintOrphans);
    assert!(
        orphans.next().starts_with("umx node get umx:demo/"),
        "{}",
        orphans.next()
    );
    issue(&env, Code::LintProposedKeys);
    assert!(
        issue(&env, Code::LintDoubleNegatives)
            .next()
            .contains("--title <text>"),
        "{}",
        env.to_json()
    );
    assert_eq!(
        env.data["lint"]["skipped"],
        json!([
            {"kind": "sensitivity_label", "why": "umx has no sensitivity label field"},
            {"kind": "terms", "why": "no lint terms file on this machine"}
        ])
    );
    let (env, exit) = home.umx(&["check", "--kind", "orphans"]);
    assert_eq!(exit, 2, "{}", env.to_json());
    let (env, exit) = home.umx(&["check", "--lint", "--kind", "nonsense"]);
    assert_ne!(exit, 0, "{}", env.to_json());
    issue(&env, Code::UsageInvalid);
}

/// F139: a raised_in link whose node's evidence sits outside its anchor's, in the same source and unit, is named
/// with the nearest pair of pointers (pm-003: not their spans); one that overlaps is not; a node raised in two
/// anchors gives a row per link.
#[test]
fn check_lint_names_a_raised_in_link_whose_evidence_sits_elsewhere() {
    let mut home = Home::new("f139-apart");
    home.graph();
    let (segment, apart) = f139_apart(&home);
    let segment_at = |title: &str, seq: &str| {
        home.ok(&[
            "node",
            "add",
            "--type",
            "segment",
            "--title",
            title,
            "--set",
            "starts_on=2026-11-12T12:00:00Z",
            "--evidence",
            &format!("t.ndjson#seq={seq}"),
        ])
        .data["node"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (noon, evening) = (segment_at("Noon", "5-6"), segment_at("Evening", "60-61"));
    let twice = home
        .ok(&[
            "node",
            "add",
            "--type",
            "risk",
            "--title",
            "Twice",
            "--evidence",
            "t.ndjson#seq=1-2",
            "--evidence",
            "t.ndjson#seq=10-11",
            "--raised-in",
            &noon,
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    home.ok(&["edge", "add", &twice, "raised_in", &evening]);
    home.ok(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "Inside",
        "--evidence",
        "t.ndjson#seq=3-4",
        "--raised-in",
        &segment,
    ]);
    let (env, exit) = home.umx(&["check", "--lint", "--kind", "raised_in_evidence"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let rows = env.data["lint"]["rows"].as_array().unwrap();
    let mut got: Vec<(Value, Value, Value)> = rows
        .iter()
        .map(|r| (r["id"].clone(), r["anchor"].clone(), r["ranges"].clone()))
        .collect();
    got.sort_by_key(|r| r.2.to_string());
    let id = |n: &str| f139_full(&home, n);
    let ranges =
        |node: [u64; 2], anchor: [u64; 2]| json!({"unit": "seq", "node": node, "anchor": anchor});
    let mut want = vec![
        (id(&apart), id(&segment), ranges([40, 45], [1, 5])),
        (id(&twice), id(&noon), ranges([1, 2], [5, 6])),
        (id(&twice), id(&evening), ranges([10, 11], [60, 61])),
    ];
    want.sort_by_key(|r| r.2.to_string());
    assert_eq!(got, want, "{}", env.to_json());
    let first = &rows[0];
    let at = format!(
        "the first is {} to {}, nearest at seq {}-{} against {}-{}",
        first["id"].as_str().unwrap(),
        first["anchor"].as_str().unwrap(),
        first["ranges"]["node"][0],
        first["ranges"]["node"][1],
        first["ranges"]["anchor"][0],
        first["ranges"]["anchor"][1]
    );
    let hint = issue(&env, Code::LintRaisedInEvidence);
    assert!(hint.message().contains(&at), "{at}: {}", hint.message());
}

/// ws006 S23 round 2 (review R9): an entity end given by a former alias gets its graph's owner and denial checks: a
/// denial stored in beta still stops a link after beta is renamed and the link names it as beta.
#[test]
fn a_denial_holds_when_a_link_names_its_graph_by_a_former_alias() {
    let mut home = Home::new("s23-deny");
    let (_, x, y) = s23_pair(&mut home);
    let (ax, by) = (format!("umx:alpha/{x}"), format!("umx:beta/{y}"));
    home.ok(&[
        "entity",
        "deny",
        &ax,
        &by,
        "--reason",
        "two different things",
        "--graph",
        "beta",
    ]);
    home.ok(&["graph", "rename", "beta", "gamma"]);
    let (env, exit) = home.umx(&["entity", "link", &ax, &by, "--graph", "alpha"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert!(
        issue(&env, Code::EntityLinkInvalid)
            .message()
            .contains("denied"),
        "{}",
        env.to_json()
    );
}

#[test]
fn a_hub_hit_lists_three_links_and_counts_the_rest() {
    // PRD 002 search-08: a node with 500 stored links carries 3 and `links_more: 497`, read from its own edges.
    let mut home = Home::new("search-hub");
    home.graph();
    let hub = add_node(&home, "Quokka hub", &[]);
    let mut ops = String::new();
    for i in 0..500 {
        ops.push_str(&format!(
            "{{\"op\":\"node add\",\"ref\":\"n{i}\",\"type\":\"problem\",\"title\":\"Spoke {i}\"}}\n{{\"op\":\"edge add\",\"from_ref\":\"n{i}\",\"relation\":\"supports\",\"to\":\"{hub}\"}}\n"
        ));
    }
    let file = home.path.join("hub.jsonl");
    fs::write(&file, ops).unwrap();
    home.ok(&["batch", "--file", file.to_str().unwrap()]);
    let env = home.ok(&["search", "quokka"]);
    let row = hit(&env, "Quokka hub");
    assert_eq!(
        (row["links"].as_array().unwrap().len(), &row["links_more"]),
        (3, &json!(497)),
        "{}",
        env.to_json()
    );
    assert!(
        row["links"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["direction"] == "in")
    );
}

/// ws003 s08 (usage 202): `--on-key resolve` and a batch line's `on_key` resolve to the node of the type already
/// holding the key, through the CLI and the daemon, and say so; a reply without the flag is unchanged; any other
/// mode is a usage error.
#[test]
fn on_key_resolve_finds_the_node_holding_the_key() {
    let mut home = Home::new("s08-on-key");
    home.graph();
    let add = |home: &Home, extra: &[&str]| {
        let mut words = vec![
            "node",
            "add",
            "--type",
            "person",
            "--title",
            "Jo Smith",
            "--set",
            "entity_key=email:jo@example.com",
        ];
        words.extend(extra);
        home.ok(&words)
    };
    let first = add(&home, &["--on-key", "resolve"]);
    assert_eq!(first.data["resolved"], json!(false), "{}", first.to_json());
    let id = first.data["node"]["id"].as_str().unwrap().to_owned();
    let again = add(&home, &["--on-key", "resolve", "--set", "team=blue"]);
    assert_eq!(
        (
            &again.data["resolved"],
            &again.data["node"]["id"],
            &again.data["changed"]
        ),
        (&json!(true), &json!(id), &json!(["team"])),
        "{}",
        again.to_json()
    );
    let lines = [
        json!({"op":"node add","ref":"jo","type":"person","title":"Jo Smith","on_key":"resolve",
            "sets":[{"field":"entity_key","value":"email:jo@example.com"}]}),
        json!({"op":"node add","ref":"sam","type":"person","title":"Sam","on_key":"resolve",
            "sets":[{"field":"entity_key","value":"upn:sam@corp"}]}),
        json!({"op":"node add","ref":"sam_again","type":"person","title":"Sam","on_key":"resolve",
            "sets":[{"field":"entity_key","value":"upn:sam@corp"}]}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let ops = home.ops_file(&text);
    let done = home.ok(&["batch", "--file", ops.as_str()]);
    let rows = done.data["rows"].as_array().unwrap();
    let existed: Vec<&Value> = rows.iter().map(|r| &r["existed"]).collect();
    assert_eq!(existed, [&json!(true), &json!(false), &json!(true)]);
    assert_eq!(rows[0]["id"], json!(id));
    assert_eq!(rows[2]["id"], rows[1]["id"]);
    assert_eq!(done.data["refs"]["jo"], json!(id));
    let plain = add(&home, &[]);
    assert!(plain.data.get("resolved").is_none(), "{}", plain.to_json());
    let (env, exit) = home.umx(&[
        "node", "add", "--type", "person", "--title", "x", "--on-key", "create",
    ]);
    assert_eq!((exit, env.errors[0].code()), (2, "usage_invalid"));
}

#[test]
fn a_std_field_again_writes_nothing_and_another_kind_role_or_a_reserved_key_is_refused() {
    // req-100 clauses 3 and 4: a std field std already gives the type or relation, with std's kind and role, is
    // std's (unchanged, nothing written); another role is field_role_conflict; a reserved key is reserved_name.
    let mut home = Home::new("t2c-again");
    let plain = std_and_plain(&mut home);
    let demo = home.path.join("g").display().to_string();
    let file = |path: &str| fs::read_to_string(Path::new(path).join("graph.json")).unwrap();
    let before = file(&demo);
    for args in [
        &["ontology", "add", "field", "person", "job_title:string"][..],
        &["ontology", "add", "field", "person", "tags:list"][..],
        &["ontology", "add", "field", "meeting", "starts_on:date"][..],
        &["ontology", "add", "field", "std:person", "aliases:list"][..],
        &[
            "ontology",
            "add",
            "field",
            "--relation",
            "works_for",
            "starts_on:date",
        ][..],
    ] {
        let env = home.ok(&[args, &["--graph", "demo"][..]].concat());
        assert_eq!(
            env.data["unchanged"],
            json!(true),
            "{args:?}: {}",
            env.to_json()
        );
    }
    let env = home.ok(&[
        "ontology",
        "add",
        "field",
        "person,organisation",
        "aliases:list",
        "--graph",
        "demo",
    ]);
    assert_eq!(
        (&env.data["types"], &env.data["unchanged"]),
        (&json!([]), &json!(["person", "organisation"])),
        "{}",
        env.to_json()
    );
    let env = refused_with(
        &home,
        &[
            "ontology",
            "add",
            "field",
            "person",
            "job_title:string",
            "--role",
            "content",
            "--graph",
            "demo",
        ],
        Code::FieldRoleConflict,
    );
    let found = issue(&env, Code::FieldRoleConflict);
    assert_eq!(found.next(), "umx ontology show --graph demo");
    assert!(
        found.message().contains("ontology set field"),
        "{}",
        env.to_json()
    );
    for key in [
        "id:string",
        "title:string",
        "status:string",
        "note:string",
        "created:date",
        "prov_key:string",
        "locator:string",
        "entity_key:list",
        "std:string",
    ] {
        refused_with(
            &home,
            &["ontology", "add", "field", "person", key, "--graph", "demo"],
            Code::ReservedName,
        );
    }
    assert_eq!(file(&demo), before, "nothing written");
    // A graph without std declares the same field as its own, as before.
    let env = home.ok(&[
        "ontology",
        "add",
        "field",
        "person",
        "job_title:string",
        "--graph",
        "plain",
    ]);
    assert_eq!(env.data["unchanged"], json!(false));
    assert!(file(&plain).contains("job_title"));
}
