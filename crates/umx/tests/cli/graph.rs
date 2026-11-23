//! The cli tests: graph verbs: new, set, guide, list, rename, move, copies, scopes, spines and check.
//! A module of cli.rs, whose `Home` and helpers it uses, in its own file so no file nears the seed gate's
//! size cap.

use super::*;

/// F144 (review R1-5): the next of graph_files_changed and stale_files quotes a graph folder whose path holds a
/// space, so the line runs as printed: split as a shell splits it (single quotes), each `umx` command in turn
/// unloads and loads the folder again, and the write then goes through.
#[test]
fn the_files_changed_next_quotes_a_folder_with_a_space_and_runs_as_printed() {
    let mut home = Home::new("f144-space");
    home.graph();
    let path = home.path.join("my graph").display().to_string();
    home.ok(&["graph", "new", &path, "--alias", "spaced"]);
    let add = |home: &Home, title: &str| {
        home.umx(&[
            "node", "add", "--graph", "spaced", "--type", "finding", "--title", title,
        ])
    };
    assert_eq!(add(&home, "First").1, 0);
    let nodes = Path::new(&path).join("nodes.jsonl");
    let first = fs::read(&nodes).unwrap();
    assert_eq!(add(&home, "Second").1, 0);
    checkout(&path, "nodes.jsonl", &first);
    // The folder as the daemon resolved it (on macOS /tmp is /private/tmp).
    let resolved = fs::canonicalize(&path).unwrap().display().to_string();
    let want = format!("umx graph unload spaced && umx graph load '{resolved}' --as spaced");
    let listed = home.ok(&["node", "list", "--graph", "spaced"]);
    assert_eq!(issue(&listed, Code::StaleFiles).next(), want);
    let (refused, exit) = add(&home, "Third");
    assert_eq!(exit, 1, "{}", refused.to_json());
    let next = issue(&refused, Code::GraphFilesChanged).next().to_string();
    assert_eq!(next, want);
    for command in next.split(" && ") {
        let mut words: Vec<String> = Vec::new();
        for (i, part) in command.split('\'').enumerate() {
            match i % 2 {
                0 => words.extend(part.split_whitespace().map(String::from)),
                _ => words.push(part.to_string()),
            }
        }
        assert_eq!(words.remove(0), "umx");
        let args: Vec<&str> = words.iter().map(String::as_str).collect();
        home.ok(&args);
    }
    assert_eq!(add(&home, "Third").1, 0);
    let listed = home.ok(&["node", "list", "--graph", "spaced"]);
    assert_eq!(
        listed.data["total"], 2,
        "the checked-out node and the new one"
    );
}

#[test]
fn an_agent_can_seed_a_graph_using_only_what_agents_start_here_prints() {
    // req-39: the commands come from the printed text, not from this test.
    let mut home = Home::new("start-here");
    let text = home.human(&["agents-start-here"]);
    let printed: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("umx "))
        .map(|l| l.split(" #").next().unwrap().trim().to_owned())
        .collect();
    let find = |verb: &str| {
        printed
            .iter()
            .find(|l| l.starts_with(verb))
            .cloned()
            .unwrap()
    };
    let (new, add) = (find("umx graph new"), find("umx node add"));
    home.start();
    let graph = home.path.join("my-graph").display().to_string();
    let new = new.replace("/path/to/my-graph", &graph);
    let words = |line: &str| -> Vec<String> {
        // Split on spaces, keeping "quoted words" together.
        let mut out = Vec::new();
        for (i, part) in line.split('"').enumerate() {
            if i % 2 == 1 {
                out.push(part.to_owned());
            } else {
                out.extend(part.split_whitespace().map(str::to_owned));
            }
        }
        out.remove(0);
        out
    };
    for line in [new, add] {
        let w = words(&line);
        let (env, exit) = home.umx(&w.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(exit, 0, "{line}: {}", env.to_json());
    }
    assert_eq!(
        umx_core::codec::decode_nodes(&fs::read(home.path.join("my-graph/nodes.jsonl")).unwrap())
            .unwrap()
            .records
            .iter()
            .filter(|n| !umx_core::provenance::is_provenance(n))
            .count(),
        1
    );
}

#[test]
fn an_unused_graph_unloads_itself_and_a_read_pushes_the_deadline_back() {
    // req-03: --idle-unload; req-06: the daemon sleeps until the deadline rather than polling. A graph unloads one
    // idle time after its last use (umx-services tests/s05b.rs proves that rule on a fake clock); here, end to end, a
    // read moves the daemon's own last use, and the daemon unloads by itself. Judged on the daemon's clock, not on
    // when this test's commands happen to run, which a loaded gate stretches by seconds (F102).
    let mut home = Home::new("idle-unload");
    home.start_with(&["--idle-unload", "10s"]);
    let path = home.path.join("g").display().to_string();
    home.ok(&["graph", "new", &path, "--alias", "demo"]);
    let before = home.ok(&["graph", "list"]).data["rows"][0].clone();
    assert!(before["unloads_in_s"].as_u64().unwrap() <= 10, "{before}");
    // last_used is in whole seconds: let one pass before the read.
    std::thread::sleep(Duration::from_millis(1100));
    let (read, exit) = home.umx(&["node", "list"]);
    assert_eq!(
        exit,
        0,
        "the read came after demo unloaded: {}",
        read.to_json()
    );
    let after = home.ok(&["graph", "list"]).data["rows"][0].clone();
    assert!(
        after["last_used"].as_str().unwrap() > before["last_used"].as_str().unwrap(),
        "the node list moved demo's last use, and so its unload deadline: {before} then {after}"
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (now, _) = home.umx(&["graph", "list"]);
        if now.data["total"] == 0 {
            break;
        }
        assert!(Instant::now() < deadline, "demo never unloaded");
        std::thread::sleep(Duration::from_millis(250));
    }
    // Review B1: a value that would overflow the deadline is refused before the daemon starts.
    for bad in ["soon", "18446744073709551615ms", "721h"] {
        let (env, exit) = home.umx(&["daemon", "start", "--idle-unload", bad]);
        assert_eq!((exit, env.errors[0].code()), (2, "usage_invalid"), "{bad}");
        assert_eq!(env.errors[0].next(), "umx docs get daemon");
    }
    let (status, _) = home.umx(&["daemon", "status"]);
    assert_eq!(
        (&status.data["running"], &status.data["writer"]),
        (&json!(true), &json!("ok"))
    );
}

#[test]
fn two_spellings_of_one_folder_are_one_working_copy() {
    // Review B1: a symlink spelling (and on macOS /private/tmp for /tmp) must not make a second working copy.
    let mut home = Home::new("spell");
    home.start();
    fs::create_dir_all(home.path.join("real")).unwrap();
    std::os::unix::fs::symlink(home.path.join("real"), home.path.join("link")).unwrap();
    let real = home.path.join("real/g").display().to_string();
    let link = home.path.join("link/g").display().to_string();
    home.umx(&["graph", "new", &real, "--alias", "physical_a"]);
    home.umx(&[
        "node",
        "add",
        "--graph",
        "physical_a",
        "--type",
        "problem",
        "--title",
        "Seed",
    ]);
    let mut spellings = vec![link];
    if cfg!(target_os = "macos") {
        spellings.push(real.replacen("/tmp/", "/private/tmp/", 1));
    }
    for (i, spelling) in spellings.iter().enumerate() {
        let alias = format!("physical_{}", i + 2);
        let (loaded, exit) = home.umx(&["graph", "load", spelling, "--as", &alias]);
        assert_eq!(exit, 0, "{}", loaded.to_json());
        let (list, _) = home.umx(&["graph", "list"]);
        assert_eq!(list.data["total"], 1, "{}", list.to_json());
        let title = format!("Added through {alias}");
        let (added, exit) = home.umx(&[
            "node", "add", "--graph", &alias, "--type", "problem", "--title", &title,
        ]);
        assert_eq!(exit, 0, "{}", added.to_json());
    }
    // The earlier alias is gone, so a write through it cannot land on a stale copy.
    let (stale, exit) = home.umx(&[
        "node",
        "add",
        "--graph",
        "physical_a",
        "--type",
        "problem",
        "--title",
        "Lost?",
    ]);
    assert_eq!((exit, stale.errors[0].code()), (1, "graph_not_loaded"));
    // Every acknowledged write is in the file, checked independently of the daemon.
    let nodes = fs::read_to_string(home.path.join("real/g/nodes.jsonl")).unwrap();
    let content = nodes
        .lines()
        .map(|line| umx_protocol::parse_json(line).unwrap())
        .filter(|node| node["type"] == "problem")
        .count();
    assert_eq!(content, 1 + spellings.len(), "{nodes}");
    assert!(nodes.contains("Seed") && nodes.contains("Added through physical_2"));
}

#[test]
fn an_address_names_its_own_graph() {
    // Review B2.
    let mut home = Home::new("addr");
    home.graph();
    let beta = home.path.join("beta").display().to_string();
    home.umx(&["graph", "new", &beta, "--alias", "beta"]);
    let (b, _) = home.umx(&[
        "node", "add", "--graph", "beta", "--type", "problem", "--title", "In beta",
    ]);
    let id = b.data["node"]["id"].as_str().unwrap().to_owned();
    let address = format!("umx:beta/{id}");
    let alpha_default = [("UMX_GRAPH", "demo")];

    let (got, exit) = home.umx_env(&["node", "get", &address], &alpha_default);
    assert_eq!(
        (exit, got.data["node"]["graph"].as_str()),
        (0, Some("beta")),
        "{}",
        got.to_json()
    );
    let (child, exit) = home.umx_env(
        &[
            "node", "add", "--type", "option", "--title", "Child", "--parent", &address,
        ],
        &alpha_default,
    );
    assert_eq!(
        (exit, child.data["node"]["graph"].as_str()),
        (0, Some("beta")),
        "{}",
        child.to_json()
    );
    let (within, _) = home.umx_env(&["node", "list", "--within", &address], &alpha_default);
    assert_eq!(within.data["total"], 1, "{}", within.to_json());
    let (edges, exit) = home.umx_env(&["edge", "list", "--to", &address], &alpha_default);
    assert_eq!(
        (exit, edges.data["total"].as_u64()),
        (0, Some(1)),
        "{}",
        edges.to_json()
    );

    let (clash, exit) = home.umx(&["node", "get", "--graph", "demo", &address]);
    assert_eq!((exit, clash.errors[0].code()), (1, "graph_mismatch"));
    let (cross, _) = home.umx(&[
        "edge",
        "list",
        "--from",
        "umx:demo/01ABCDEF",
        "--to",
        &address,
    ]);
    assert_eq!(cross.errors[0].code(), "cross_graph_not_yet");
}

#[test]
fn a_failed_graph_new_leaves_nothing_and_can_be_retried() {
    // Review B4: a later-file failure.
    let mut home = Home::new("retry");
    home.start();
    let g = home.path.join("new");
    fs::create_dir_all(g.join("edges.jsonl")).unwrap();
    let path = g.display().to_string();
    let (failed, exit) = home.umx(&["graph", "new", &path, "--alias", "broken"]);
    assert_eq!(
        (exit, failed.errors[0].code()),
        (4, "write_failed"),
        "{}",
        failed.to_json()
    );
    assert!(
        !g.join("nodes.jsonl").exists() && !g.join("graph.json").exists(),
        "nothing changed"
    );
    fs::remove_dir(g.join("edges.jsonl")).unwrap();
    let (ok, exit) = home.umx(&["graph", "new", &path, "--alias", "broken"]);
    assert_eq!(exit, 0, "{}", ok.to_json());
    assert_eq!(home.lines("new/nodes.jsonl"), 0);
}

#[test]
fn graph_new_never_erases_records_it_did_not_write() {
    // Review round 2: records written by umx, then graph.json lost.
    let mut home = Home::new("orphan");
    let source = home.graph();
    home.add("problem", "Pre-existing record");
    let orphan = home.path.join("orphan");
    fs::create_dir_all(&orphan).unwrap();
    fs::copy(format!("{source}/nodes.jsonl"), orphan.join("nodes.jsonl")).unwrap();
    let before = fs::read(orphan.join("nodes.jsonl")).unwrap();
    let path = orphan.display().to_string();
    let (refused, exit) = home.umx(&["graph", "new", &path, "--alias", "orphan"]);
    assert_eq!(
        (exit, refused.errors[0].code()),
        (1, "graph_orphan_records"),
        "{}",
        refused.to_json()
    );
    assert_eq!(refused.errors[0].next(), "umx docs get graph");
    assert_eq!(
        fs::read(orphan.join("nodes.jsonl")).unwrap(),
        before,
        "bytes unchanged"
    );
    assert!(!orphan.join("graph.json").exists() && !orphan.join("edges.jsonl").exists());
    let (add, exit) = home.umx(&[
        "node", "add", "--graph", "orphan", "--type", "problem", "--title", "x",
    ]);
    assert_eq!((exit, add.errors[0].code()), (1, "graph_not_loaded"));
    assert_eq!(
        fs::read(orphan.join("nodes.jsonl")).unwrap(),
        before,
        "still unchanged"
    );
}

#[test]
fn a_repeated_id_from_a_merge_is_kept_reported_and_survives_the_next_write() {
    // req-67 done_when, through the binary.
    let mut home = Home::new("dup");
    let path = home.graph();
    let id = home.add("problem", "Approval delays");
    let file = Path::new(&path).join("nodes.jsonl");
    let before = fs::read_to_string(&file).unwrap();
    let original_lines = before.lines().count();
    let original_line = before
        .lines()
        .position(|line| umx_protocol::parse_json(line).unwrap()["id"] == id)
        .unwrap()
        + 1;
    let line = before.lines().nth(original_line - 1).unwrap();
    let changed = line.replace("Approval delays", "Approval delays, other branch");
    fs::write(&file, format!("{before}{changed}\n")).unwrap();

    let (env, exit) = home.umx(&["graph", "load", &path]);
    assert_eq!(exit, 0);
    let warned = issue(&env, Code::DuplicateId);
    assert!(warned.message().contains(&id), "{}", warned.message());
    assert_eq!(warned.next(), "umx check --graph demo");

    let (env, _) = home.umx(&["check", "--graph", "demo"]);
    assert_eq!(env.data["rows"][0]["kind"], "duplicate_id");
    assert_eq!(env.data["rows"][0]["subject"], json!(id));
    assert_eq!(
        env.data["rows"][0]["lines"],
        json!([original_line, original_lines + 1])
    );

    home.add("decision", "Unrelated");
    let (env, _) = home.umx(&["node", "edit", &id, "--title", "Edited in use"]);
    issue(&env, Code::DuplicateId);
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.contains("\"Approval delays\""), "{text}");
    assert!(text.contains("Edited in use"), "{text}");
    assert_eq!(text.lines().count(), original_lines + 2);
}

// ws004 s06 part A (D32): graph new records the graph's embedding model; load and list show it.
#[test]
fn graph_new_records_the_embedding_model_and_list_and_load_show_it() {
    let mut home = Home::new("embedding-model");
    home.start();
    let path = home.path.join("g").display().to_string();
    let (made, exit) = home.umx(&[
        "graph",
        "new",
        &path,
        "--alias",
        "demo",
        "--embedding-model",
        "qwen3-embedding-0.6b@256+p1",
    ]);
    assert_eq!(exit, 0, "{}", made.to_json());
    // A new graph records the current text form (2: data fields left out, D56) and the format it needs.
    let shown = json!({ "key": "qwen3-embedding-0.6b@256+p1", "text_form": 2, "policy": "open" });
    assert_eq!(made.data["graph"]["embedding"], shown);
    let file: umx_protocol::Value =
        umx_protocol::parse_json(&fs::read_to_string(Path::new(&path).join("graph.json")).unwrap())
            .unwrap();
    assert_eq!((&file["umx"], &file["embedding"]), (&json!(4), &shown));
    let (listed, _) = home.umx(&["graph", "list"]);
    assert_eq!(listed.data["rows"][0]["embedding"], shown);
    home.umx(&["graph", "unload", "demo"]);
    let (loaded, exit) = home.umx(&["graph", "load", &path]);
    assert_eq!((exit, &loaded.data["graph"]["embedding"]), (0, &shown));

    let plain = home.path.join("h").display().to_string();
    let (made, _) = home.umx(&["graph", "new", &plain, "--alias", "plain"]);
    assert!(made.data["graph"]["embedding"].is_null());

    let bad = home.path.join("b").display().to_string();
    for key in ["nomic-ai/nomic-embed@256", "m@0", "Model@8"] {
        let (env, exit) = home.umx(&[
            "graph",
            "new",
            &bad,
            "--alias",
            "bad",
            "--embedding-model",
            key,
        ]);
        assert_eq!((exit, env.errors[0].code()), (2, "usage_invalid"), "{key}");
        assert!(!Path::new(&bad).join("graph.json").exists(), "{key}");
    }
}

#[test]
fn graph_new_prints_how_to_change_the_graph_and_each_command_runs_as_printed() {
    // req-71. Two graphs loaded, so a next that forgot its graph would be refused as ambiguous.
    let mut home = Home::new("gnext");
    home.graph();
    let path = home.path.join("x").display().to_string();
    let out = home
        .command(&["graph", "new", &path, "--alias", "x", "--human"])
        .bounded_output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let (_, tail) = stdout
        .split_once("next:\n")
        .unwrap_or_else(|| panic!("no next: block in {stdout}"));
    let printed: Vec<&str> = tail.lines().map(str::trim).collect();
    assert_eq!(
        printed,
        [
            "umx ontology add type experiment --graph x",
            "umx ontology add relation mitigates --graph x",
            "umx ontology add status parked --graph x",
            "umx graph set x --title \"Launch planning\"",
            "umx graph set x --policy secured",
            "umx graph set x --purpose \"Decide how to launch in spring\"",
            "umx graph set x --conventions \"Titles are short noun phrases\"",
            "umx graph set x --description \"status:open=Not worked on yet\"",
        ],
        "the next: block must end the output"
    );
    for line in &printed {
        let (env, exit) = run_as_printed(&home, line);
        assert_eq!(
            (exit, env.status),
            (0, Status::Ok),
            "{line}: {}",
            env.to_json()
        );
    }
    let meta = graph_json(&path);
    assert_eq!(meta["title"], "Launch planning");
    let ontology = meta["ontology"].to_string();
    for name in ["\"experiment\"", "\"mitigates\"", "\"parked\""] {
        assert!(ontology.contains(name), "{name} not declared: {ontology}");
    }
    assert_eq!(
        graph_json(&home.path.join("g").display().to_string())["title"],
        "demo"
    );
}

#[test]
fn graph_new_envelope_matches_its_fixture() {
    let mut home = Home::new("gfix");
    home.start();
    let path = home.path.join("launch").display().to_string();
    let (env, exit) = home.umx(&["graph", "new", &path, "--alias", "launch"]);
    assert_eq!(exit, 0);
    let tier = env.data["actor_tier"]
        .as_str()
        .expect("a write records its actor tier");
    assert!(["pij", "harness", "cli", "unknown"].contains(&tier));
    let fixture = format!(
        "{}/../umx-protocol/fixtures/envelope/graph-new.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = fs::read_to_string(&fixture)
        .unwrap()
        .replace("<id>", env.data["graph"]["id"].as_str().unwrap())
        .replace("<path>", env.data["graph"]["path"].as_str().unwrap())
        .replace("<actor-tier>", tier);
    let want = Envelope::from_json(text.as_bytes()).unwrap();
    assert_eq!(env, want, "{fixture} differs from what umx prints");
}

#[test]
fn graph_set_checks_every_setting_before_changing_anything() {
    let mut home = Home::new("gset");
    let path = home.graph();
    let before = fs::read(format!("{path}/graph.json")).unwrap();
    let unchanged = |what: &str| {
        assert_eq!(
            fs::read(format!("{path}/graph.json")).unwrap(),
            before,
            "{what}"
        );
    };

    let (env, exit) = home.umx(&["graph", "set", "demo", "--title", "New", "--colour", "blue"]);
    assert_eq!(exit, 2);
    let found = issue(&env, Code::SettingUnknown);
    assert_eq!(
        found.message(),
        "colour is not a graph setting; the settings are title or policy or visibility or embedder or related or purpose or conventions or description."
    );
    assert_eq!(found.next(), "umx graph set demo --title <text>");
    unchanged("an unknown setting with a known one");

    // The settings an error lists are the ones the graph docs document.
    let (docs, _) = home.umx(&["docs", "get", "graph"]);
    let markdown = docs.data["markdown"].as_str().unwrap();
    let listed = found
        .message()
        .rsplit_once("are ")
        .unwrap()
        .1
        .trim_end_matches('.');
    for name in listed.split(" or ") {
        assert!(
            markdown.contains(&format!("--{name} ")),
            "docs get graph lacks --{name}"
        );
    }

    let (env, exit) = home.umx(&["graph", "set", "demo", "--title", "  "]);
    assert_eq!((exit, env.errors[0].code()), (2, "usage_invalid"));
    unchanged("an empty title");
    let (env, exit) = home.umx(&["graph", "set", "demo"]);
    assert_eq!((exit, env.errors[0].code()), (2, "usage_invalid"));
    let (env, exit) = home.umx(&["graph", "set", "nope", "--title", "x"]);
    assert_eq!((exit, env.errors[0].code()), (1, "graph_not_loaded"));
    let (env, exit) = home.umx(&["graph", "set", "demo", "--empty"]);
    assert_eq!((exit, env.errors[0].code()), (2, "usage_invalid"));
    unchanged("refusals");

    let (env, exit) = home.umx(&["graph", "set", "demo", "--title=Launch planning"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(
        env.data,
        json!({ "actor_tier": env.data["actor_tier"], "alias": "demo", "changed": ["title"], "title": "Launch planning" })
    );
    assert_eq!(graph_json(&path)["title"], "Launch planning");
    // The write survives an unload and load.
    home.umx(&["graph", "unload", "demo"]);
    let (env, _) = home.umx(&["graph", "load", &path]);
    assert_eq!(env.data["graph"]["alias"], "demo");
    assert_eq!(graph_json(&path)["title"], "Launch planning");
}

#[test]
fn spine_propose_lists_what_a_graph_supports_and_guide_and_check_show_the_best() {
    // ws005 s07 (plan 156): the verb, its hints run as printed, --unplaced, and the guide and check lines.
    let mut home = Home::new("spine");
    home.graph();
    // A fresh graph: no spine offered, still a success, and check's status and exit code are unchanged.
    let (env, exit) = home.umx(&["check"]);
    assert_eq!((exit, env.status), (0, Status::Ok), "{}", env.to_json());
    assert_eq!(env.data["spines"][0]["graph"], "demo");
    assert_eq!(env.data["spines"][0]["spine"]["best"], json!(null));
    assert_eq!(
        env.data["spines"][0]["spine"]["next"],
        "umx graph spine propose demo"
    );

    for args in [
        // The anchor type shares its name with the `recorded` built-in (review B3).
        &["ontology", "add", "type", "recorded"][..],
        &["ontology", "add", "field", "recorded", "step:int"],
    ] {
        assert_eq!(home.umx(args).1, 0, "{args:?}");
    }
    let segments: Vec<String> = (1..=3)
        .map(|step| {
            let (env, exit) = home.umx(&[
                "node",
                "add",
                "--type",
                "recorded",
                "--title",
                &format!("Part {step}"),
                "--set",
                &format!("step={step}"),
            ]);
            assert_eq!(exit, 0, "{}", env.to_json());
            env.data["node"]["id"].as_str().unwrap().to_owned()
        })
        .collect();
    for i in 0..9 {
        let point = home.add("problem", &format!("Point {i}"));
        assert_eq!(
            home.umx(&["edge", "add", &point, "raised_in", &segments[i % 3]])
                .1,
            0
        );
    }
    let stray = home.add("problem", "Not linked");

    let (env, exit) = home.umx(&["graph", "spine", "propose", "demo"]);
    assert_eq!((exit, env.status), (0, Status::Ok), "{}", env.to_json());
    let best = &env.data["candidates"][0];
    assert_eq!(
        (
            &best["name"],
            &best["source"],
            &best["clock"],
            &best["offered"]
        ),
        (
            &json!("type:recorded"),
            &json!("anchor_type"),
            &json!("order"),
            &json!(true)
        )
    );
    assert_eq!(best["decl"]["order"], json!(["step"]));
    assert_eq!(best["decl"]["place"], json!(["raised_in"]));
    assert_eq!(
        best["cover"],
        json!({ "total": 13, "by_field": 3, "by_link": 9, "by_evidence": 0, "by_rule": 0, "not_placed": 1 })
    );
    let hint = &env.data["hints"]["not_placed"];
    assert_eq!(
        (&hint["candidate"], &hint["count"]),
        (&json!("type:recorded"), &json!(1))
    );
    let (listed, exit) = run_as_printed(&home, hint["next"].as_str().unwrap());
    assert_eq!(exit, 0, "{}", listed.to_json());
    assert_eq!(listed.data["unplaced"]["ids"], json!([stray]));
    // The built-in of the same plain name is its own candidate: it places every node by `created`.
    let (env, exit) = home.umx(&[
        "graph",
        "spine",
        "propose",
        "demo",
        "--unplaced",
        "recorded",
    ]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(
        (&env.data["unplaced"]["total"], &env.data["unplaced"]["ids"]),
        (&json!(0), &json!([]))
    );
    // A page at a time: no node has a status move, so `status` leaves all 13 out.
    let (env, _) = home.umx(&[
        "graph",
        "spine",
        "propose",
        "demo",
        "--unplaced",
        "status",
        "--limit",
        "2",
        "--offset",
        "1",
    ]);
    let page = &env.data["unplaced"];
    assert_eq!((&page["total"], &page["offset"]), (&json!(13), &json!(1)));
    assert_eq!(page["ids"].as_array().map(Vec::len), Some(2));

    let (env, exit) = home.umx(&["graph", "spine", "propose", "demo", "--unplaced", "nothing"]);
    assert_eq!(exit, 2, "{}", env.to_json());
    let refused = issue(&env, Code::UsageInvalid);
    assert!(
        ["type:recorded", "recorded"]
            .iter()
            .all(|n| refused.candidates().iter().any(|c| c["id"] == *n)),
        "{}",
        env.to_json()
    );

    let (env, _) = home.umx(&["graph", "guide", "demo"]);
    assert_eq!(env.data["spine"]["best"]["candidate"], "type:recorded");
    assert_eq!(env.data["spine"]["best"]["cover"]["not_placed"], 1);
    let (env, exit) = home.umx(&["check"]);
    assert_eq!((exit, env.status), (0, Status::Ok), "{}", env.to_json());
    assert_eq!(
        env.data["spines"][0]["spine"]["best"]["candidate"],
        "type:recorded"
    );
    let text = home.human(&["graph", "spine", "propose", "demo"]);
    assert!(
        text.contains("type:recorded (anchor_type, order): 12 of 13 placed"),
        "{text}"
    );
    assert!(
        home.human(&["graph", "guide", "demo"])
            .contains("spine demo: type:recorded (order), 12 of 13 placed")
    );
}

#[test]
fn graph_guide_orients_an_agent_and_its_nexts_fill_it_in() {
    // Plan 079 ac-0001, ac-0002, ac-0005. Two graphs loaded, so a next that forgot its graph would be refused.
    let mut home = Home::new("guide");
    let path = home.graph();
    let other = home.path.join("other").display().to_string();
    home.umx(&["graph", "new", &other, "--alias", "other"]);

    // A fresh graph: nothing described, still a success (exit 0) with hints (ws002 X1), and every next runs as
    // printed.
    let (env, exit) = home.umx(&["graph", "guide", "demo"]);
    assert_eq!((exit, env.status), (0, Status::Ok), "{}", env.to_json());
    assert_eq!(env.data["purpose"], json!(null));
    assert_eq!(
        env.data["purpose_next"],
        "umx graph set demo --purpose <text>"
    );
    assert_eq!(
        env.data["conventions_next"],
        "umx graph set demo --conventions <text>"
    );
    assert_eq!(
        issue(&env, Code::PurposeMissing).next(),
        "umx graph set demo --purpose <text>"
    );
    assert_eq!(
        issue(&env, Code::DescriptionsMissing).message(),
        "31 types, statuses, relations or fields in demo have no description."
    );
    let problem = guide_entry(&env, "types", "problem");
    assert_eq!(
        (
            &problem["declared"],
            &problem["count"],
            &problem["example"],
            &problem["description"]
        ),
        (&json!(true), &json!(0), &json!(null), &json!(null))
    );
    assert_eq!(
        problem["next"],
        "umx graph set demo --description type:problem=<text>"
    );
    let filled = |line: &str| line.replace("<text>", "'Something in the way'");
    for line in [
        problem["next"].as_str().unwrap(),
        env.data["purpose_next"].as_str().unwrap(),
        guide_entry(&env, "relations", "blocks")["next"]
            .as_str()
            .unwrap(),
    ] {
        let (env, exit) = run_as_printed(&home, &filled(line));
        assert_eq!(
            (exit, env.status),
            (0, Status::Ok),
            "{line}: {}",
            env.to_json()
        );
    }
    home.umx(&["graph", "unload", "other"]);

    // Records, then the words that describe them: describing changes graph.json only.
    let one = home.add("problem", "One");
    let two = home.add("problem", "Two");
    let decision = home.add("decision", "Launch date");
    home.add("widget", "Odd one");
    home.umx(&["edge", "add", &two, "blocks", &decision]);
    home.umx(&["edge", "add", &one, "blocks", &decision]);
    let records = |f: &str| fs::read(format!("{path}/{f}")).unwrap();
    let (nodes, edges) = (records("nodes.jsonl"), records("edges.jsonl"));
    for args in [
        &[
            "ontology",
            "add",
            "type",
            "risk",
            "--field",
            "likelihood:float",
            "--description",
            "Something that may go wrong",
        ][..],
        &[
            "ontology",
            "add",
            "status",
            "parked",
            "--description",
            "Waiting on someone else",
        ],
        &[
            "ontology",
            "add",
            "relation",
            "mitigates",
            "--description",
            "Makes the other less likely",
        ],
        &[
            "ontology",
            "add",
            "field",
            "risk",
            "owner:string",
            "--description",
            "Who watches it",
        ],
        &[
            "graph",
            "set",
            "demo",
            "--description",
            "field:risk.likelihood=0 to 1",
            "--conventions",
            "Short titles.\nOne idea per node.",
        ],
    ] {
        let (env, exit) = home.umx(args);
        assert_eq!(
            (exit, env.status),
            (0, Status::Ok),
            "{args:?}: {}",
            env.to_json()
        );
    }
    assert_eq!(
        (records("nodes.jsonl"), records("edges.jsonl")),
        (nodes, edges)
    );

    let (env, exit) = home.umx(&["graph", "guide", "demo"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert!(
        env.warnings
            .iter()
            .all(|w| w.code() == "descriptions_missing")
    );
    assert_eq!(env.data["purpose"], "Something in the way");
    assert_eq!(env.data["purpose_next"], json!(null));
    assert_eq!(env.data["conventions"], "Short titles.\nOne idea per node.");
    assert_eq!(
        env.data["graph"],
        json!({ "alias": "demo", "title": "demo", "nodes": 4, "edges": 2 })
    );
    let problem = guide_entry(&env, "types", "problem");
    assert_eq!(
        (&problem["description"], &problem["count"], &problem["next"]),
        (&json!("Something in the way"), &json!(2), &json!(null))
    );
    assert_eq!(problem["example"], json!({ "id": one, "title": "One" }));
    let widget = guide_entry(&env, "types", "widget");
    assert_eq!(
        (&widget["declared"], &widget["count"]),
        (&json!(false), &json!(1))
    );
    assert_eq!(
        widget["next"],
        "umx ontology add type widget --description <text> --graph demo"
    );
    // risk is a std type: std's due_on first, which graph.json does not declare, so it asks for no description.
    let risk = guide_entry(&env, "types", "risk");
    assert_eq!(
        risk["fields"],
        json!([
            { "name": "due_on", "kind": "date", "description": null, "next": null, "std": true },
            { "name": "likelihood", "kind": "float", "description": "0 to 1", "next": null, "std": false },
            { "name": "owner", "kind": "string", "description": "Who watches it", "next": null, "std": false },
        ])
    );
    let blocks = guide_entry(&env, "relations", "blocks");
    assert_eq!(
        (&blocks["count"], &blocks["symmetric"]),
        (&json!(2), &json!(false))
    );
    let first_edge = blocks["example"]["sentence"].as_str().unwrap();
    assert!(
        first_edge == "Two blocks Launch date",
        "the lowest edge id is the first added: {first_edge}"
    );
    assert_eq!(
        guide_entry(&env, "relations", "conflicts_with")["symmetric"],
        true
    );
    assert_eq!(guide_entry(&env, "statuses", "open")["count"], 4);
    assert_eq!(
        guide_entry(&env, "statuses", "parked")["description"],
        "Waiting on someone else"
    );
    let meta = graph_json(&path);
    assert_eq!(meta["umx"], 1, "no format bump");
    assert_eq!(
        meta["ontology"]["status_descriptions"],
        json!({ "parked": "Waiting on someone else" })
    );

    // Human mode reads it, graph text cleaned.
    let (stdout, _) = output_with(&home, &["graph", "guide", "demo", "--human"], &[]);
    for want in [
        "Graph demo: demo (4 nodes, 2 edges)",
        "Purpose\n  Something in the way\n",
        "Conventions\n  Short titles.\n  One idea per node.\n",
        "problem (2): Something in the way\n",
        "widget (1, not declared):\n",
        "field likelihood (float): 0 to 1\n",
        "e.g. Two blocks Launch date\n",
    ] {
        assert!(stdout.contains(want), "{want:?} not in:\n{stdout}");
    }

    // Clearing: an empty text removes the key again.
    home.umx(&[
        "graph",
        "set",
        "demo",
        "--conventions",
        "",
        "--description",
        "status:parked=",
    ]);
    let meta = graph_json(&path);
    assert!(meta["ontology"].get("conventions").is_none(), "{meta}");
    assert!(
        meta["ontology"].get("status_descriptions").is_none(),
        "{meta}"
    );
}

#[test]
fn graph_set_names_the_alias_it_was_given_not_the_one_graph_json_stores() {
    // Review B2: a copy loaded with --as keeps graph.json's alias (demo). graph set's nexts and warnings name the
    // alias the session selected (beta), or following them would change the other graph.
    let mut home = Home::new("gsalias");
    let path = home.graph();
    let copy = home.path.join("copy");
    fs::create_dir_all(&copy).unwrap();
    for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
        let from = Path::new(&path).join(file);
        if from.exists() {
            fs::copy(from, copy.join(file)).unwrap();
        }
    }
    let copy = copy.display().to_string();
    home.ok(&["graph", "load", &copy, "--as", "beta"]);
    assert_eq!(graph_json(&copy)["alias"], "demo");
    let demo_before = fs::read(format!("{path}/graph.json")).unwrap();

    let (env, exit) = home.umx(&[
        "graph",
        "set",
        "beta",
        "--description",
        "field:risk.owner=Who",
    ]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let line = issue(&env, Code::DescriptionUndeclared).next().to_string();
    assert_eq!(
        line,
        "umx ontology add field risk owner:<kind> --description <text> --graph beta"
    );
    let line = line
        .replace("<kind>", "string")
        .replace("<text>", "'Who watches it'");
    assert_eq!(run_as_printed(&home, &line).1, 0, "{line}");
    assert!(
        graph_json(&copy)["ontology"]["types"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "risk"),
        "the repair changed beta"
    );
    let (env, _) = home.umx(&[
        "graph",
        "set",
        "beta",
        "--description",
        "status:odd=Unusual",
    ]);
    assert_eq!(
        issue(&env, Code::OffVocabularyStatus).next(),
        "umx ontology add status odd --graph beta"
    );
    assert_eq!(
        fs::read(format!("{path}/graph.json")).unwrap(),
        demo_before,
        "demo is untouched"
    );
    assert_eq!(
        graph_json(&copy)["alias"],
        "demo",
        "the stored alias is not rewritten"
    );
}

#[test]
fn graph_guide_reads_an_end_in_another_graph_as_its_address_never_as_a_local_node() {
    // Review B3: the edge's far end names a node of beta whose id is also a node of demo. The guide must not
    // read it as demo's node (wrong title), and must keep beta's identity in the sentence and the JSON.
    let mut home = Home::new("guide-far");
    let path = home.graph();
    let a = home.add("problem", "Author node");
    let decoy = home.add("problem", "Local decoy");
    let beta_path = home.path.join("beta").display().to_string();
    let beta = home.ok(&["graph", "new", &beta_path, "--alias", "beta"]);
    let beta_id = beta.data["graph"]["id"].as_str().unwrap().to_string();
    let b = home.ok(&[
        "node", "add", "--type", "problem", "--title", "Far node", "--graph", "beta",
    ]);
    let b = b.data["node"]["id"].as_str().unwrap().to_string();
    home.ok(&[
        "edge",
        "add",
        &a,
        "blocks",
        &format!("umx:beta/{b}"),
        "--graph",
        "demo",
    ]);
    home.ok(&["graph", "unload", "demo"]);
    let edges = Path::new(&path).join("edges.jsonl");
    let text = fs::read_to_string(&edges).unwrap();
    assert!(text.contains(&b), "{text}");
    fs::write(&edges, text.replace(&b, &decoy)).unwrap();
    home.umx(&["graph", "load", &path]);

    let env = home.ok(&["graph", "guide", "demo"]);
    let demo_id = graph_json(&path)["id"].clone();
    assert!(demo_id.is_string(), "{demo_id}");
    let example = &guide_entry(&env, "relations", "blocks")["example"];
    let address = format!("umx:{beta_id}/{decoy}");
    assert_eq!(
        (
            &example["from_graph"],
            &example["from"],
            &example["to_graph"],
            &example["to"]
        ),
        (&demo_id, &json!(a), &json!(beta_id), &json!(decoy))
    );
    assert_eq!(example["sentence"], format!("Author node blocks {address}"));
    let (stdout, _) = output_with(&home, &["graph", "guide", "demo", "--human"], &[]);
    assert!(stdout.contains(&address), "{stdout}");
    assert!(
        !stdout.contains("Local decoy blocks") && !stdout.contains("blocks Local decoy"),
        "{stdout}"
    );
}

/// F98: copy_ambiguous's next is `umx graph primary <copy>`, it runs as printed and ends the refusal; a secured
/// copy made primary warns `primary_is_secured`, and the reply names no other copy (security).
#[test]
fn graph_primary_ends_copy_ambiguous_and_names_only_the_copy_given() {
    let mut home = Home::new("gprimary");
    home.design();
    home.design_copy("design_b");
    home.design_copy("design_c");
    home.ok(&["node", "list"]);
    home.ok(&["graph", "unload", "design"]);
    let (env, exit) = home.umx(&["node", "list"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let next = issue(&env, Code::CopyAmbiguous).next().to_string();
    assert_eq!(next, "umx graph primary design_b");
    let (env, exit) = run_as_printed(&home, &next);
    assert_eq!((exit, env.status), (0, Status::Ok), "{}", env.to_json());
    assert_eq!(
        (&env.data["alias"], &env.data["unchanged"]),
        (&json!("design_b"), &json!(false))
    );
    home.ok(&["node", "list"]);
    assert_eq!(
        home.ok(&["graph", "primary", "design_b"]).data["unchanged"],
        true
    );

    home.ok(&["graph", "set", "design_c", "--policy", "secured"]);
    let (env, exit) = home.umx(&["graph", "primary", "design_c"]);
    assert_eq!(
        (exit, env.status),
        (0, Status::Warning),
        "{}",
        env.to_json()
    );
    assert_eq!(
        issue(&env, Code::PrimaryIsSecured).next(),
        "umx graph primary <another copy>"
    );
    let reply = env.to_json();
    assert!(!reply.contains("design_b"), "{reply}");
    assert_eq!(
        reply.matches("design").count(),
        reply.matches("design_c").count(),
        "no alias but design_c: {reply}"
    );
}

/// F113, security ruling a: a single check's rows and link_targets_unnamed count are the same whether the far
/// graph is not loaded or is loaded and hidden (named visibility), so neither says a hidden graph is loaded.
#[test]
fn a_single_check_is_the_same_for_an_unloaded_and_a_hidden_target() {
    let mut home = Home::new("check-hidden");
    home.linked_graphs();
    let (hidden, _) = home.umx(&["check", "--graph", "demo"]);
    home.ok(&["graph", "unload", "d1"]);
    home.ok(&["graph", "unload", "d2"]);
    let (unloaded, _) = home.umx(&["check", "--graph", "demo"]);
    assert_eq!(hidden.data, unloaded.data);
    let warned = issue(&hidden, Code::LinkTargetsUnnamed)
        .message()
        .to_owned();
    assert!(warned.starts_with("2 links in demo"), "{warned}");
    assert_eq!(warned, issue(&unloaded, Code::LinkTargetsUnnamed).message());
}

#[test]
fn raw_all_graph_selectors_are_checked_before_any_operation_runs() {
    let mut home = Home::new("raw-all-graph-reads");
    home.graph();
    for op in [
        umx_protocol::Op::Search {
            no_tags: Default::default(),
            tags: Default::default(),
            text: "words".into(),
            within: None,
            limit: None,
            along: None,
            also: Vec::new(),
        },
        umx_protocol::Op::Query {
            text: "RETURN 1 AS one".into(),
            limit: None,
            offset: None,
            params: Vec::new(),
            along: None,
        },
    ] {
        for (all, include_named, graph, scope) in [
            (false, true, None, None),
            (true, false, Some("demo".into()), None),
            (true, true, None, Some("saved".into())),
        ] {
            let mut request = Request::op(op.clone(), graph, None, older_cli());
            request.all = all;
            request.include_named = include_named;
            request.scope = scope;
            let (env, _) = raw(&home.socket(), &request);
            assert_eq!(env.errors[0].code(), "usage_invalid", "{}", env.to_json());
        }
    }
    for op in [
        umx_protocol::Op::GraphList {
            all: false,
            graphfile: None,
        },
        umx_protocol::Op::EmbeddingsProbe { embedder: None },
    ] {
        let mut request = Request::op(op, None, None, older_cli());
        request.all = true;
        let (env, _) = raw(&home.socket(), &request);
        assert_eq!(env.errors[0].code(), "usage_invalid", "{}", env.to_json());
    }
}

/// d-21 (the joint ruling): copies of one graph (the same graph id under two aliases) are read once when no
/// graph is named, the primary being the earliest loaded; named copies are unioned with a warning; the primary
/// unloaded with two copies left is copy_ambiguous; a secured graph's copies are only counted.
#[test]
fn copies_of_one_graph_are_read_once_unless_named() {
    let mut home = Home::new("query-copies");
    home.design();
    home.design_copy("design_b");
    let count = "MATCH (a)-[:blocks]->(b {id: $id}) RETURN count(*) AS n";
    let d1 = ["--param", "id=01JDCYPHER00000000000000D1"];
    let (env, exit) = home.query(count, &d1);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(query_rows(&env), [["3"]]);
    let note = issue(&env, Code::CopiesSkipped);
    assert_eq!(
        note.candidates()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["design_b"]
    );

    // Reloading a copy's folder keeps its place in load order: the primary never moves (graph-addressing-expert B1).
    for (folder, alias) in [("design", "design"), ("design_b", "design_b")] {
        let path = home.path.join(folder).display().to_string();
        let (env, exit) = home.umx(&["graph", "load", &path, "--as", alias]);
        assert_eq!(exit, 0, "{}", env.to_json());
        let (env, _) = home.query(count, &d1);
        let note = issue(&env, Code::CopiesSkipped);
        assert_eq!(
            note.candidates()
                .iter()
                .map(|c| c["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["design_b"],
            "after reloading {alias}"
        );
    }

    let (env, _) = home.query(count, &[&d1[..], &["--graph", "design,design_b"]].concat());
    assert_eq!(query_rows(&env), [["6"]]);
    issue(&env, Code::QueryGraphTwice);
    let (env, _) = home.query(
        "MATCH (n {id: $id}) RETURN n ORDER BY n.graph",
        &[&d1[..], &["--graph", "design,design_b"]].concat(),
    );
    let copies: Vec<&str> = env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["n"]["copy"].as_str().unwrap())
        .collect();
    assert_eq!(copies, ["design", "design_b"]);

    // search and walk show each node once with no graph named.
    let (env, exit) = home.umx(&["search", "Launch"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let ids: Vec<&str> = env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    let unique: std::collections::BTreeSet<&str> = ids.iter().copied().collect();
    assert!(!ids.is_empty() && ids.len() == unique.len(), "{ids:?}");
    issue(&env, Code::CopiesSkipped);
    let (env, exit) = home.umx(&["walk", "01JDCYPHER00000000000000D1", "--depth", "2"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    issue(&env, Code::CopiesSkipped);

    // The primary unloaded with one copy left: that copy is read, with no warning.
    home.umx(&["graph", "unload", "design"]);
    let (env, _) = home.query(count, &d1);
    assert_eq!(query_rows(&env), [["3"]]);
    assert!(env.warnings.is_empty(), "{}", env.to_json());
    // Two copies more, then the primary (design_b) unloaded with two left: none is primary.
    home.design_copy("design_c");
    home.design_copy("design_d");
    home.umx(&["graph", "unload", "design_b"]);
    let (env, exit) = home.query(count, &d1);
    assert_eq!(exit, 1, "{}", env.to_json());
    let refusal = issue(&env, Code::CopyAmbiguous);
    // F98: the next makes the first copy primary; naming a copy with --graph (below) also works.
    assert_eq!(refusal.next(), "umx graph primary design_c");
    let (env, _) = home.query(count, &[&d1[..], &["--graph", "design_d"]].concat());
    assert_eq!(query_rows(&env), [["3"]]);

    // A secured graph loaded twice is counted with the secured graphs, never named as a copy.
    let vault = home.path.join("vault").display().to_string();
    let (env, exit) = home.umx(&["graph", "new", &vault, "--alias", "vault", "--secured"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let vault_b = home.path.join("vault_b");
    fs::create_dir_all(&vault_b).unwrap();
    for f in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
        let from = Path::new(&vault).join(f);
        if from.exists() {
            fs::copy(from, vault_b.join(f)).unwrap();
        }
    }
    let (env, exit) = home.umx(&[
        "graph",
        "load",
        vault_b.to_str().unwrap(),
        "--as",
        "vault_b",
    ]);
    assert_eq!(exit, 0, "{}", env.to_json());
    home.umx(&["graph", "unload", "design_c"]);
    let (env, _) = home.query(count, &d1);
    assert_eq!(query_rows(&env), [["3"]]);
    assert!(
        hint_of(&env, Code::SecuredGraphsSkipped)
            .message()
            .contains('2')
    );
    assert!(!env.to_json().contains("vault"), "{}", env.to_json());
}

/// d-21 for single-graph verbs (reviewer R132-1): node list, walk, path and neighbours read one copy with no graph
/// named (the unnamed scope's one open primary, with copies_skipped), and read a named copy alone.
/// Scoped node lists now read copies explicitly; traversal still refuses a comma list until s07b.
#[test]
fn single_graph_verbs_read_one_copy() {
    let mut home = Home::new("one-copy-verbs");
    home.design();
    home.design_copy("design_b");
    let (d1, q1) = ("01JDCYPHER00000000000000D1", "01JDCYPHER00000000000000Q1");
    let verbs: [Vec<&str>; 4] = [
        vec!["node", "list"],
        vec!["walk", d1, "--depth", "1"],
        vec!["path", q1, d1],
        vec!["neighbours", d1],
    ];
    for verb in &verbs {
        let (env, exit) = home.umx(verb);
        assert_eq!(exit, 0, "{verb:?}: {}", env.to_json());
        let note = issue(&env, Code::CopiesSkipped);
        assert_eq!(note.candidates()[0]["id"], "design_b", "{verb:?}");
        let named = [&verb[..], &["--graph", "design_b"]].concat();
        let (env, exit) = home.umx(&named);
        assert_eq!(exit, 0, "{named:?}: {}", env.to_json());
        assert!(env.warnings.is_empty(), "{named:?}: {}", env.to_json());
        let both = [&verb[..], &["--graph", "design,design_b"]].concat();
        let (env, exit) = home.umx(&both);
        if verb == &vec!["node", "list"] {
            assert_eq!(exit, 0, "{}", env.to_json());
            assert_eq!(env.data["total"], 22);
            issue(&env, Code::QueryGraphTwice);
            let rows = env.data["rows"].as_array().unwrap();
            assert_eq!(rows[0]["copy"], "design");
            assert_eq!(rows[11]["copy"], "design_b");
            continue;
        }
        assert_eq!(exit, 2, "{both:?}: {}", env.to_json());
        assert_eq!(env.errors[0].code(), "usage_invalid", "{both:?}");
    }
    let (env, _) = home.umx(&["node", "list"]);
    assert_eq!(env.data["total"], 11);
}

#[test]
fn scopes_cover_req66_lists_query_search_and_restart() {
    let mut home = Home::new("scopes-req66");
    home.start();
    let mut paths = Vec::new();
    for alias in ["g1", "g2"] {
        let path = home.path.join(alias).display().to_string();
        let (env, exit) = home.umx(&["graph", "new", &path, "--alias", alias]);
        assert_eq!(exit, 0, "{}", env.to_json());
        paths.push(path);
        let mut ids = Vec::new();
        for title in ["Approval delays", "Delivery decision"] {
            let (env, exit) = home.umx(&[
                "node", "add", "--graph", alias, "--type", "problem", "--title", title,
            ]);
            assert_eq!(exit, 0, "{}", env.to_json());
            ids.push(env.data["node"]["id"].as_str().unwrap().to_string());
        }
        let (env, exit) = home.umx(&["edge", "add", &ids[0], "blocks", &ids[1], "--graph", alias]);
        assert_eq!(exit, 0, "{}", env.to_json());
        if alias == "g1" {
            let (env, exit) = home.umx(&["node", "list"]);
            assert_eq!(exit, 0);
            assert_eq!(env.data["total"], 2);
            assert!(
                env.data["rows"][0].get("graph").is_none(),
                "single graph shape unchanged"
            );
        }
    }
    let (listed, exit) = home.umx(&["node", "list", "--graph", "g1,g2"]);
    assert_eq!(exit, 0, "{}", listed.to_json());
    assert_eq!(listed.data["total"], 4);
    for (i, alias) in ["g1", "g1", "g2", "g2"].iter().enumerate() {
        let row = &listed.data["rows"][i];
        assert_eq!(row["graph"], *alias);
        assert_eq!(
            row["address"],
            format!("umx:{alias}/{}", row["id"].as_str().unwrap())
        );
    }
    let (saved, exit) = home.umx(&["scope", "save", "both", "--graph", "g1,g2"]);
    assert_eq!(exit, 0, "{}", saved.to_json());
    assert_eq!(saved.data["members"].as_array().unwrap().len(), 2);
    let (scoped, exit) = home.umx(&["node", "list", "--scope", "both"]);
    assert_eq!(exit, 0);
    assert_eq!(listed.data, scoped.data);
    let (page, _) = home.umx(&[
        "node",
        "list",
        "--scope",
        "both",
        "--offset",
        "1",
        "--limit",
        "2",
        "--columns",
        "title",
    ]);
    assert_eq!(page.data["total"], 4);
    assert_eq!(page.data["truncated"], true);
    assert_eq!(page.data["rows"][0]["graph"], "g1");
    assert_eq!(page.data["rows"][1]["graph"], "g2");
    assert!(
        page.data["rows"][1]["address"]
            .as_str()
            .unwrap()
            .starts_with("umx:g2/")
    );
    let (edges, _) = home.umx(&[
        "edge", "list", "--scope", "both", "--offset", "1", "--limit", "1",
    ]);
    assert_eq!(edges.data["total"], 2);
    assert_eq!(edges.data["truncated"], false);
    assert_eq!(edges.data["rows"][0]["graph"], "g2");
    let (query, _) = home.query("MATCH (n) RETURN count(*) AS nodes", &["--scope", "both"]);
    assert_eq!(
        query.data["rows"],
        json!([{"nodes": 4}]),
        "{}",
        query.to_json()
    );
    let (search, exit) = home.umx(&["search", "Approval", "--scope", "both"]);
    assert_eq!(exit, 0, "{}", search.to_json());
    assert_eq!(search.data["rows"].as_array().unwrap().len(), 2);
    assert!(
        search.data["rows"][0]["address"]
            .as_str()
            .unwrap()
            .starts_with("umx:")
    );
    for args in [
        vec!["node", "list", "--scope", "both", "--graph", "g1"],
        vec!["node", "list", "--scope", "g1,g2"],
        vec![
            "node", "add", "--type", "problem", "--title", "No write", "--graph", "g1,g2",
        ],
        vec!["walk", "01JDCYPHER00000000000000D1", "--scope", "both"],
    ] {
        let (env, exit) = home.umx(&args);
        assert_eq!(exit, 2, "{args:?}: {}", env.to_json());
        issue(&env, Code::UsageInvalid);
    }
    assert_eq!(
        home.umx(&["scope", "list"]).0.data["rows"][0]["name"],
        "both"
    );
    assert_eq!(
        home.umx(&["scope", "show", "both"]).0.data["members"][0]["loaded"],
        true
    );
    assert_eq!(home.umx(&["scope", "save", "one", "--graph", "g1"]).1, 0);
    assert_eq!(
        home.umx(&["scope", "save", "one", "--graph", "g2", "--replace"])
            .1,
        0
    );
    assert_eq!(
        home.umx(&["scope", "show", "one"]).0.data["members"][0]["alias"],
        "g2"
    );
    home.umx(&["scope", "delete", "one"]);
    // g2 is unloaded on purpose, so it stays away after the restart (F72 reloads only loaded graphs).
    home.umx(&["graph", "unload", "g2"]);
    home.umx(&["daemon", "stop"]);
    home.daemon.take().unwrap().bounded_wait().unwrap();
    home.start();
    home.umx(&["graph", "load", &paths[0], "--as", "renamed"]);
    let (missing, exit) = home.umx(&["node", "list", "--scope", "both"]);
    assert_eq!(exit, 1);
    let error = issue(&missing, Code::ScopeGraphNotLoaded);
    assert!(error.next().contains(&paths[1]), "{}", error.next());
    assert!(missing.data.is_null());
    home.umx(&["graph", "load", &paths[1]]);
    let (again, exit) = home.umx(&["node", "list", "--scope", "both"]);
    assert_eq!(exit, 0, "{}", again.to_json());
    assert_eq!(again.data["total"], 4);
    assert_eq!(again.data["rows"][0]["graph"], "renamed");
    assert_eq!(
        home.umx(&["scope", "show", "both"]).0.data["members"][0]["alias"],
        "g1"
    );
    assert_eq!(home.umx(&["scope", "delete", "both"]).1, 0);
    issue(&home.umx(&["scope", "show", "both"]).0, Code::ScopeUnknown);
}

#[test]
fn scopes_name_secured_members_and_resolve_only_primary_copies() {
    let mut home = Home::new("scopes-policy-copies");
    home.design();
    home.design_copy("design_b");
    home.design_copy("design_c");
    let path = home.path.join("vault").display().to_string();
    home.umx(&["graph", "new", &path, "--alias", "vault", "--secured"]);
    home.umx(&[
        "node",
        "add",
        "--graph",
        "vault",
        "--type",
        "problem",
        "--title",
        "Hidden vault approval",
    ]);
    let (saved, exit) = home.umx(&["scope", "save", "both", "--graph", "design_b,design,vault"]);
    assert_eq!(exit, 0, "{}", saved.to_json());
    issue(&saved, Code::CopyDropped);
    issue(&saved, Code::ScopeIncludesSecured);
    assert_eq!(saved.data["members"].as_array().unwrap().len(), 2);
    let (nodes, exit) = home.umx(&["node", "list", "--scope", "both"]);
    assert_eq!(exit, 0, "{}", nodes.to_json());
    assert_eq!(nodes.data["total"], 12);
    assert_eq!(nodes.data["rows"][0]["graph"], "design");
    assert_eq!(nodes.data["rows"][11]["graph"], "vault");
    let (unnamed, _) = home.umx(&["search", "approval"]);
    hint_of(&unnamed, Code::SecuredGraphsSkipped);
    issue(&unnamed, Code::CopiesSkipped);
    assert!(
        !unnamed.to_json().contains("vault"),
        "{}",
        unnamed.to_json()
    );
    let (named, _) = home.umx(&["search", "approval", "--scope", "both"]);
    assert!(
        named.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["graph"] == "vault")
    );
    let (copies, exit) = home.umx(&["edge", "list", "--graph", "design,design_b"]);
    assert_eq!(exit, 0);
    issue(&copies, Code::QueryGraphTwice);
    assert!(
        copies.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["copy"] == "design_b")
    );
    home.umx(&["graph", "set", "design", "--policy", "secured"]);
    let (primary, _) = home.umx(&["scope", "save", "primary", "--graph", "design_b"]);
    issue(&primary, Code::ScopeIncludesSecured);
    let (primary, exit) = home.umx(&["node", "list", "--scope", "primary"]);
    assert_eq!(exit, 0, "{}", primary.to_json());
    assert_eq!(primary.data["rows"][0]["graph"], "design");
    home.umx(&["graph", "unload", "design"]);
    let (ambiguous, _) = home.umx(&["node", "list", "--scope", "both"]);
    issue(&ambiguous, Code::CopyAmbiguous);
    assert!(ambiguous.data.is_null());
}

#[test]
fn external_copy_ambiguity_is_not_an_unchecked_write() {
    let mut home = Home::new("external-copy-ambiguity");
    let author = home.graph();
    let a = home.add("problem", "Author");
    let beta_path = home.path.join("beta");
    let beta = home.ok(&[
        "graph",
        "new",
        beta_path.to_str().unwrap(),
        "--alias",
        "beta",
    ]);
    let beta_id = beta.data["graph"]["id"].as_str().unwrap();
    let b = home
        .ok(&[
            "node", "add", "--type", "problem", "--title", "Target", "--graph", "beta",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    home.ok(&[
        "edge",
        "add",
        &a,
        "blocks",
        &format!("umx:beta/{b}"),
        "--graph",
        "demo",
    ]);
    for alias in ["copy_one", "copy_two"] {
        let path = home.path.join(alias);
        fs::create_dir(&path).unwrap();
        for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
            fs::copy(beta_path.join(file), path.join(file)).unwrap();
        }
        home.ok(&["graph", "load", path.to_str().unwrap(), "--as", alias]);
    }
    let missing = format!("umx:{beta_id}/00000000000000000000000000");
    let (control, _) = home.umx(&["edge", "add", &a, "supports", &missing, "--graph", "demo"]);
    issue(&control, Code::NodeNotFound);
    home.ok(&["graph", "unload", "beta"]);
    let before = fs::read(Path::new(&author).join("edges.jsonl")).unwrap();
    let (ambiguous, exit) = home.umx(&["edge", "add", &a, "supports", &missing, "--graph", "demo"]);
    assert_eq!(exit, 1);
    issue(&ambiguous, Code::CopyAmbiguous);
    assert_eq!(
        fs::read(Path::new(&author).join("edges.jsonl")).unwrap(),
        before
    );
    let adjacent = home.ok(&["neighbours", &a, "--graph", "demo"]);
    assert_eq!(adjacent.data["rows"][0]["state"], "copy_ambiguous");
    assert_eq!(
        adjacent.data["rows"][0]["candidates"],
        json!(["copy_one", "copy_two"])
    );
    let listed = home.ok(&["edge", "list", "--from", &a, "--graph", "demo"]);
    assert_eq!(listed.data["rows"][0]["to_state"], "copy_ambiguous");
    assert_eq!(
        listed.data["rows"][0]["to_candidates"],
        json!(["copy_one", "copy_two"])
    );
    let named = home.ok(&[
        "edge",
        "list",
        "--from",
        &format!("umx:demo/{a}"),
        "--graph",
        "demo,copy_one",
    ]);
    assert_eq!(named.data["rows"][0]["to_title"], "Target");
}

#[test]
fn cross_graph_reads_deliver_req21_and_virtual_graph_targets() {
    let mut home = Home::new("cross-graph-reads");
    let g1 = home.graph();
    let a = home.add("problem", "A in graph one");
    let g2 = home.path.join("second");
    let g3 = home.path.join("links");
    home.ok(&["graph", "new", g2.to_str().unwrap(), "--alias", "second"]);
    home.ok(&["graph", "new", g3.to_str().unwrap(), "--alias", "links"]);
    let b = home
        .ok(&[
            "node",
            "add",
            "--type",
            "problem",
            "--title",
            "B in graph two",
            "--graph",
            "second",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let before: Vec<_> = [&PathBuf::from(&g1), &g2]
        .into_iter()
        .map(|dir| {
            ["graph.json", "nodes.jsonl", "edges.jsonl"]
                .map(|file| fs::read(dir.join(file)).unwrap())
        })
        .collect();
    home.ok(&[
        "edge",
        "add",
        &format!("umx:demo/{a}"),
        "blocks",
        &format!("umx:second/{b}"),
        "--graph",
        "links",
    ]);
    let after: Vec<_> = [&PathBuf::from(&g1), &g2]
        .into_iter()
        .map(|dir| {
            ["graph.json", "nodes.jsonl", "edges.jsonl"]
                .map(|file| fs::read(dir.join(file)).unwrap())
        })
        .collect();
    assert_eq!(before, after, "only the author's files may change");
    let walk_args = ["walk", a.as_str(), "--depth", "2"];
    let walk = home.ok(&walk_args);
    assert_eq!(walk.data["rows"][0]["id"], b);
    assert_eq!(walk.data["rows"][0]["graph"], "second");
    assert_eq!(walk.data["rows"][0]["address"], format!("umx:second/{b}"));
    let path = home.ok(&[
        "path",
        &format!("umx:demo/{a}"),
        &format!("umx:second/{b}"),
        "--graph",
        "demo,second,links",
    ]);
    assert_eq!(path.data["rows"][1]["id"], b);
    let neighbours = home.ok(&[
        "neighbours",
        &format!("umx:demo/{a}"),
        "--graph",
        "demo,second,links",
    ]);
    assert_eq!(neighbours.data["rows"][0]["id"], b);
    home.ok(&["scope", "save", "crossing", "--graph", "demo,second,links"]);
    assert_eq!(
        home.ok(&[
            "walk",
            &format!("umx:demo/{a}"),
            "--depth",
            "2",
            "--scope",
            "crossing"
        ])
        .data["rows"][0]["id"],
        b
    );
    home.ok(&["graph", "set", "second", "--policy", "secured"]);
    let hidden = home.ok(&walk_args);
    assert_eq!(hidden.data["rows"][0]["state"], "graph_not_loaded");
    assert!(hidden.data["rows"][0].get("title").is_none());
    home.ok(&["graph", "unload", "second"]);
    let unloaded = home.ok(&walk_args);
    assert_eq!(
        hidden.data.to_string().into_bytes(),
        unloaded.data.to_string().into_bytes()
    );
    assert_eq!(
        hidden.to_json().into_bytes(),
        unloaded.to_json().into_bytes()
    );
    let query = home.ok(&[
        "query",
        &format!("MATCH (a {{id: '{a}'}})-[]->(b) RETURN b"),
    ]);
    assert!(query.data["rows"].as_array().unwrap().is_empty());
    assert!(
        query
            .warnings
            .iter()
            .any(|w| w.code() == "external_edges_skipped")
    );
    home.ok(&["graph", "load", g2.to_str().unwrap()]);
    let graph_node = home.ok(&["node", "get", "umx:second"]);
    assert_eq!(graph_node.data["node"]["type"], "graph");
    assert_eq!(graph_node.data["node"]["address"], "umx:second");
    assert_eq!(graph_node.data["node"]["alias"], "second");
    assert!(graph_node.data["node"].get("path").is_none());
    home.ok(&[
        "edge",
        "add",
        &a,
        "records",
        "umx:second",
        "--graph",
        "demo",
    ]);
    let reserved = home.ok(&[
        "node",
        "add",
        "--type",
        "graph",
        "--title",
        "A stored graph marker",
        "--graph",
        "demo",
    ]);
    assert!(
        reserved
            .warnings
            .iter()
            .any(|w| w.code() == "reserved_type")
    );
}

#[test]
fn all_graph_crossings_keep_visibility_and_owner_boundaries() {
    let mut home = Home::new("all-cross-graph-privacy");
    home.graph();
    for alias in ["second", "far", "links"] {
        let path = home.path.join(alias);
        home.ok(&["graph", "new", path.to_str().unwrap(), "--alias", alias]);
    }
    let add = |alias: &str, title: &str| {
        home.ok(&[
            "node", "add", "--graph", alias, "--type", "problem", "--title", title,
        ])
        .data["node"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let a = add("demo", "Path start");
    let b = add("second", "Zephyrbridge");
    let c = add("far", "Path finish");
    let a_address = format!("umx:demo/{a}");
    let b_address = format!("umx:second/{b}");
    let c_address = format!("umx:far/{c}");
    for (from, relation, to) in [
        (&a_address, "blocks", &b_address),
        (&b_address, "blocks", &c_address),
        (&a_address, "records", &c_address),
    ] {
        home.ok(&["edge", "add", from, relation, to, "--graph", "links"]);
    }
    let path_query =
        format!("MATCH (a {{id: '{a}'}})-[:blocks*1..2]->(n) RETURN n.id AS id ORDER BY id");
    let all = |verb: &str, text: &str| {
        let (env, exit) = home.umx_env(&[verb, text, "--all"], &[("UMX_GRAPH", "second")]);
        assert_eq!(exit, 0, "{}", env.to_json());
        env
    };
    let visible = all("query", &path_query);
    let mut expected = vec![b.as_str(), c.as_str()];
    expected.sort_unstable();
    assert_eq!(
        visible.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        expected
    );
    assert!(
        all("search", "zephyrbridge").data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == b)
    );

    home.ok(&["graph", "set", "second", "--policy", "secured"]);
    let hidden_query = all("query", &path_query);
    let hidden_search = all("search", "zephyrbridge");
    for env in [&hidden_query, &hidden_search] {
        assert!(env.data["rows"].as_array().unwrap().is_empty());
        let aggregate = hint_of(env, Code::SecuredGraphsSkipped);
        assert!(aggregate.candidates().is_empty());
        assert!(!aggregate.message().contains("second"));
    }
    home.ok(&["graph", "unload", "second"]);
    let unloaded_query = all("query", &path_query);
    let unloaded_search = all("search", "zephyrbridge");
    // D53's security-approved aggregate count names nothing and may differ (the project manager).
    // Every data byte and every other diagnostic must remain identical.
    let protected_output = |env: &Envelope| {
        (
            env.data.to_string().into_bytes(),
            env.warnings
                .iter()
                .chain(&env.hints)
                .filter(|w| w.code() != "secured_graphs_skipped")
                .map(|w| json!(w).to_string().into_bytes())
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        protected_output(&hidden_query),
        protected_output(&unloaded_query)
    );
    assert_eq!(
        protected_output(&hidden_search),
        protected_output(&unloaded_search)
    );

    let incoming = format!(
        "MATCH (c {{id: '{c}'}})<-[r:records]-(a) RETURN a.id AS id, r.id AS edge, r.graph AS owner"
    );
    let outgoing = format!(
        "MATCH (a {{id: '{a}'}})-[r:records]->() RETURN a.id AS id, r.id AS edge, r.graph AS owner"
    );
    let owner_visible = all("query", &incoming);
    assert_eq!(owner_visible.data["rows"][0]["id"], a);
    assert_eq!(owner_visible.data["rows"][0]["owner"], "links");
    assert_eq!(
        owner_visible.data["rows"],
        all("query", &outgoing).data["rows"]
    );
    home.ok(&["graph", "set", "links", "--policy", "secured"]);
    assert!(
        all("query", &incoming).data["rows"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let named = home.ok(&["query", &incoming, "--graph", "demo,far,links"]);
    assert_eq!(named.data["rows"], owner_visible.data["rows"]);
    let included = home.ok(&["query", &incoming, "--all", "--include-named"]);
    assert_eq!(included.data["rows"], owner_visible.data["rows"]);
    issue(&included, Code::SecuredGraphsRead);
    assert!(
        all("query", &incoming).data["rows"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn scope_followups_report_primary_hints_moves_policy_and_copies() {
    let mut home = Home::new("scope-followup-warnings");
    let primary_folder = fs::canonicalize(home.design())
        .unwrap()
        .display()
        .to_string();
    home.design_copy("design_b");
    let (primary, exit) = home.umx(&["scope", "save", "primary", "--graph", "design"]);
    assert_eq!(exit, 0, "{}", primary.to_json());
    assert!(primary.warnings.iter().all(|w| w.code() != "copy_dropped"));
    let (saved, exit) = home.umx(&["scope", "save", "via_copy", "--graph", "design_b"]);
    assert_eq!(exit, 0, "{}", saved.to_json());
    issue(&saved, Code::CopyDropped);
    assert_eq!(saved.data["members"][0]["alias"], "design");
    assert_eq!(saved.data["members"][0]["folder"], primary_folder);
    let verbs = [
        vec!["node", "list"],
        vec!["edge", "list"],
        vec!["query", "MATCH (n) RETURN n.id"],
        vec!["search", "approval"],
    ];
    for verb in &verbs {
        let args = [&verb[..], &["--scope", "via_copy"]].concat();
        let (env, exit) = home.umx(&args);
        assert_eq!(exit, 0, "{args:?}: {}", env.to_json());
        assert_eq!(
            issue(&env, Code::CopiesSkipped).candidates()[0]["id"],
            "design_b"
        );
        assert!(
            env.warnings
                .iter()
                .all(|w| !matches!(w.code(), "scope_member_moved" | "scope_includes_secured"))
        );
    }
    home.design_copy("hidden_copy");
    home.umx(&["graph", "set", "hidden_copy", "--policy", "secured"]);
    let (hidden, _) = home.umx(&["node", "list", "--scope", "via_copy"]);
    hint_of(&hidden, Code::SecuredGraphsSkipped);
    assert!(
        !hidden.to_json().contains("hidden_copy"),
        "{}",
        hidden.to_json()
    );
    home.umx(&["graph", "unload", "hidden_copy"]);
    home.umx(&["graph", "unload", "design"]);
    home.umx(&["graph", "set", "design_b", "--policy", "secured"]);
    let actual_folder = fs::canonicalize(home.path.join("design_b"))
        .unwrap()
        .display()
        .to_string();
    for verb in &verbs {
        let args = [&verb[..], &["--scope", "via_copy"]].concat();
        let (env, exit) = home.umx(&args);
        assert_eq!(exit, 0, "{args:?}: {}", env.to_json());
        let moved = issue(&env, Code::ScopeMemberMoved);
        assert!(moved.message().contains("Scope via_copy member design"));
        assert!(
            moved.message().contains(&primary_folder),
            "{}",
            moved.message()
        );
        assert!(
            moved.message().contains(&actual_folder),
            "{}",
            moved.message()
        );
        assert_eq!(moved.next(), "umx scope show via_copy");
        issue(&env, Code::ScopeIncludesSecured);
        assert!(env.warnings.iter().all(|w| w.code() != "copies_skipped"));
    }
    let (env, exit) = home.umx(&["node", "list", "--all"]);
    assert_eq!(exit, 2);
    issue(&env, Code::UsageInvalid);
}

#[test]
fn scope_followups_newer_stores_are_not_read_or_rewritten() {
    let mut home = Home::new("scope-followup-newer");
    home.graph();
    let file = home.path.join("scopes.toml");
    let bytes = b"umx_scopes = 2\nfuture_shape = 'not a v1 scope'\n";
    fs::write(&file, bytes).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    for args in [
        vec!["scope", "list"],
        vec!["scope", "save", "both", "--graph", "demo"],
        vec!["scope", "delete", "both"],
        vec!["node", "list", "--scope", "both"],
        vec!["edge", "list", "--scope", "both"],
        vec!["query", "MATCH (n) RETURN n", "--scope", "both"],
        vec!["search", "approval", "--scope", "both"],
    ] {
        let (env, exit) = home.umx(&args);
        assert_eq!(exit, 1, "{args:?}: {}", env.to_json());
        let newer = issue(&env, Code::ScopeFormatNewer);
        assert!(newer.message().contains("scope format 2"));
        assert_eq!(fs::read(&file).unwrap(), bytes);
    }
    assert_eq!(home.umx(&["node", "list", "--graph", "demo"]).1, 0);
}

#[test]
fn scope_followups_recovery_executes_for_plain_and_space_paths() {
    let mut home = Home::new("scope-followup-recovery");
    home.start();
    let bin = home.path.join("bin");
    fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_umx"), bin.join("umx")).unwrap();
    for (alias, folder) in [("plain", "plain_folder"), ("spaced", "alpha folder")] {
        let path = home.path.join(folder).display().to_string();
        assert_eq!(home.umx(&["graph", "new", &path, "--alias", alias]).1, 0);
        let path = fs::canonicalize(path).unwrap().display().to_string();
        home.umx(&[
            "node", "add", "--graph", alias, "--type", "problem", "--title", "One node",
        ]);
        home.umx(&["scope", "save", "restore", "--graph", alias, "--replace"]);
        home.umx(&["graph", "unload", alias]);
        let (env, _) = home.umx(&["node", "list", "--scope", "restore"]);
        let missing = issue(&env, Code::ScopeGraphNotLoaded);
        assert!(missing.message().contains(&path), "{}", missing.message());
        let expected = if path.contains(' ') {
            format!("umx graph load '{path}'")
        } else {
            format!("umx graph load {path}")
        };
        assert_eq!(missing.next(), expected);
        let mut command = Command::new("/bin/sh");
        sandbox(&mut command, &home.path);
        let out = command
            .args(["-c", missing.next()])
            .env("UMX_HOME", &home.path)
            .env("UMX_ROOTS", &home.path)
            .env_remove("UMX_GRAPH")
            .bounded_output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let (read, exit) = home.umx(&["node", "list", "--scope", "restore"]);
        assert_eq!(exit, 0, "{}", read.to_json());
        assert_eq!(read.data["total"], 1);
    }
}

/// Reviewer R132-1, adapted: with both copies secured, an unscoped single-graph read reads neither (d-20 first) and
/// names no copy as skipped; it is refused as any several loads are (graph_ambiguous, which lists loaded aliases
/// for the --graph next step, as before d-21). The unscoped query is the control: it names neither.
#[test]
fn secured_copies_are_not_read_by_an_unscoped_single_graph_verb() {
    let mut home = Home::new("secured-copies");
    home.design();
    home.design_copy("design_b");
    for alias in ["design", "design_b"] {
        let (env, exit) = home.umx(&["graph", "set", alias, "--policy", "secured"]);
        assert_eq!(exit, 0, "{}", env.to_json());
    }
    let (control, _) = home.query("MATCH (n) RETURN count(*) AS n", &[]);
    assert!(
        !control.to_json().contains("design_b"),
        "{}",
        control.to_json()
    );
    let (env, exit) = home.umx(&["node", "list"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert_eq!(env.errors[0].code(), "graph_ambiguous");
    assert!(env.data.is_null());
    assert!(env.warnings.iter().all(|w| w.code() != "copies_skipped"));
}

/// ws002 F72 (req-15): a read or write naming a graph that unloaded when idle loads it and runs, saying so; a read
/// that names no graph never loads one.
#[test]
fn a_named_graph_that_unloaded_when_idle_loads_on_demand() {
    let mut home = Home::new("f72-idle");
    home.start_with(&["--idle-unload", "1s"]);
    let path = home.path.join("g").display().to_string();
    home.ok(&["graph", "new", &path, "--alias", "demo"]);
    home.ok(&[
        "node", "add", "--graph", "demo", "--type", "problem", "--title", "First",
    ]);
    home.wait_until_idle();
    let (env, _) = home.umx(&["node", "list"]);
    assert_eq!(
        env.errors[0].code(),
        "graph_none_loaded",
        "unnamed: no load"
    );

    let read = home.ok(&["node", "list", "--graph", "demo"]);
    assert_eq!(read.data["total"], 1);
    let found = issue(&read, Code::GraphLoadedOnDemand);
    assert!(
        found.message().contains(&real(&path)),
        "{}",
        found.message()
    );

    // A write loads it on demand and keeps it loaded: a read within the idle time of sending the write finds it
    // loaded. A slower pair (a loaded machine, F101) may see it unload again in between, so it is tried again; the
    // write's last use is no earlier than when it was sent, so a read answered within 1 s of that is in time.
    home.wait_until_idle();
    let mut nodes = 1;
    let mut in_time = false;
    for attempt in 0..10 {
        let sent = Instant::now();
        let title = format!("Second {attempt}");
        let write = home.ok(&[
            "node", "add", "--graph", "demo", "--type", "problem", "--title", &title,
        ]);
        nodes += 1;
        if attempt == 0 {
            issue(&write, Code::GraphLoadedOnDemand);
        }
        let read = home.ok(&["node", "list", "--graph", "demo"]);
        assert_eq!(read.data["total"], nodes);
        if sent.elapsed() < Duration::from_secs(1) {
            assert!(!has(&read, Code::GraphLoadedOnDemand), "loaded now");
            in_time = true;
            break;
        }
    }
    assert!(
        in_time,
        "no write and read fitted in the 1 s idle time in 10 tries"
    );

    // R156-B2: the alias reused for another folder after an explicit unload loads that folder on demand.
    home.ok(&["graph", "unload", "demo"]);
    let new = home.path.join("new").display().to_string();
    home.ok(&["graph", "new", &new, "--alias", "demo"]);
    home.wait_until_idle();
    let read = home.ok(&["node", "list", "--graph", "demo"]);
    assert_eq!(read.data["total"], 0, "the new folder, not the old one");
    let found = issue(&read, Code::GraphLoadedOnDemand);
    assert!(found.message().contains(&real(&new)), "{}", found.message());
}

/// F99: a `--scope` read loads members that unloaded when idle, as `--graph` does, secured ones included (the scope
/// names them); a member unloaded on purpose is refused and loads nothing else for the read.
#[test]
fn a_scope_read_loads_members_that_unloaded_when_idle() {
    let mut home = Home::new("f99-scope");
    home.start_with(&["--idle-unload", "3s"]);
    for (folder, alias, flags) in [
        ("fa", "alpha", &[][..]),
        ("fv", "vault", &["--secured"][..]),
    ] {
        let path = home.path.join(folder).display().to_string();
        let mut args = vec!["graph", "new", path.as_str(), "--alias", alias];
        args.extend_from_slice(flags);
        home.ok(&args);
        home.ok(&[
            "node", "add", "--graph", alias, "--type", "problem", "--title", "First",
        ]);
    }
    home.ok(&["scope", "save", "both", "--graph", "alpha,vault"]);
    home.wait_until_idle();

    let read = home.ok(&["node", "list", "--scope", "both"]);
    assert_eq!(read.data["total"], 2, "{}", read.to_json());
    let loaded = read
        .hints
        .iter()
        .filter(|w| w.code() == "graph_loaded_on_demand")
        .count();
    assert_eq!(loaded, 2, "{}", read.to_json());
    issue(&read, Code::ScopeIncludesSecured);

    home.ok(&["graph", "unload", "vault"]);
    home.wait_until_idle();
    let (env, exit) = home.umx(&["node", "list", "--scope", "both"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::ScopeGraphNotLoaded);
    assert!(
        home.aliases().is_empty(),
        "nothing loaded for a refused read"
    );
}

/// F118: a `--graph a,b` read loads every listed graph that unloaded when idle, as a saved scope does (F99), for node
/// list, search and query: one graph_loaded_on_demand per graph, and the read succeeds. With one of the two idled, only
/// that one loads. A listed graph unloaded on purpose refuses the read, and nothing loads for it.
#[test]
fn a_graph_list_read_loads_every_listed_graph_that_unloaded_when_idle() {
    let mut home = Home::new("f118-list");
    home.start_with(&["--idle-unload", "3s"]);
    let mut paths = Vec::new();
    for (folder, alias) in [("fa", "a2c"), ("fo", "ops")] {
        let path = home.path.join(folder).display().to_string();
        home.ok(&["graph", "new", &path, "--alias", alias]);
        home.ok(&[
            "node",
            "add",
            "--graph",
            alias,
            "--type",
            "problem",
            "--title",
            "Approval delays",
        ]);
        paths.push(path);
    }
    let on_demand = |env: &Envelope| {
        env.hints
            .iter()
            .filter(|w| w.code() == "graph_loaded_on_demand")
            .count()
    };
    let reads: [&[&str]; 3] = [
        &["node", "list", "--graph", "a2c,ops"],
        &["search", "approval", "--graph", "a2c,ops"],
        &[
            "query",
            "MATCH (n) RETURN n.title AS title",
            "--graph",
            "a2c,ops",
        ],
    ];
    for args in reads {
        home.wait_until_idle();
        let (env, exit) = home.umx(args);
        assert_eq!(exit, 0, "{args:?}: {}", env.to_json());
        assert_eq!(on_demand(&env), 2, "{args:?}: {}", env.to_json());
        assert_eq!(
            env.data["rows"].as_array().map(Vec::len),
            Some(2),
            "{args:?}: {}",
            env.to_json()
        );
    }

    // One of the two idled: only that one loads.
    home.wait_until_idle();
    home.ok(&["graph", "load", &paths[0]]);
    let (env, exit) = home.umx(&["node", "list", "--graph", "a2c,ops"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(on_demand(&env), 1, "{}", env.to_json());
    assert!(
        issue(&env, Code::GraphLoadedOnDemand)
            .message()
            .contains("Graph ops ")
    );
    assert_eq!(env.data["total"], 2, "{}", env.to_json());

    // Unloaded on purpose: the whole read is refused before anything loads.
    home.ok(&["graph", "unload", "ops"]);
    home.wait_until_idle();
    let (env, exit) = home.umx(&["node", "list", "--graph", "a2c,ops"]);
    assert_eq!(
        (exit, env.errors[0].code()),
        (1, "graph_not_loaded"),
        "{}",
        env.to_json()
    );
    assert!(
        home.aliases().is_empty(),
        "nothing loaded for a refused read"
    );
}

/// F118 (review N2): a `--graph` list that names a secured graph loads it on demand as `--graph vault` does (F72),
/// beside an open one: both load, the read sees both, and the secured graph is still secured once loaded.
#[test]
fn a_graph_list_read_loads_a_secured_listed_graph_that_unloaded_when_idle() {
    let mut home = Home::new("f118-secured");
    home.start_with(&["--idle-unload", "3s"]);
    for (folder, alias, flags) in [
        ("fa", "alpha", &[][..]),
        ("fv", "vault", &["--secured"][..]),
    ] {
        let path = home.path.join(folder).display().to_string();
        let mut args = vec!["graph", "new", path.as_str(), "--alias", alias];
        args.extend_from_slice(flags);
        home.ok(&args);
        home.ok(&[
            "node", "add", "--graph", alias, "--type", "problem", "--title", "First",
        ]);
    }
    home.wait_until_idle();

    let read = home.ok(&["node", "list", "--graph", "alpha,vault"]);
    assert_eq!(read.data["total"], 2, "{}", read.to_json());
    let loaded: Vec<&str> = read
        .hints
        .iter()
        .filter(|w| w.code() == "graph_loaded_on_demand")
        .map(|w| w.message())
        .collect();
    assert_eq!(loaded.len(), 2, "{}", read.to_json());
    assert!(
        loaded.iter().any(|m| m.contains("Graph vault ")),
        "{loaded:?}"
    );
    let graphs = home.ok(&["graph", "list"]);
    let vault = graphs.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["alias"] == "vault")
        .unwrap_or_else(|| panic!("vault loaded: {}", graphs.to_json()));
    assert_eq!(
        (&vault["effective_policy"], &vault["effective_embedder"]),
        (&json!("secured"), &json!("local")),
        "{}",
        graphs.to_json()
    );
}

/// Security F72 condition 5: graph.json lowered while the daemon was down is caught by the policy record on the
/// start-up reload, as on any load: the graph stays secured and status carries the warning.
#[test]
fn a_reload_after_a_lowered_graph_json_stays_secured() {
    let mut home = Home::new("f72-downgrade");
    let path = home.secured_graph();
    home.stop();
    edit_graph_json(&path, |v| v["embedding"]["policy"] = json!("open"));
    home.start();
    let status = home.ok(&["daemon", "status"]);
    assert_eq!(
        status.data["registry"]["reloaded"][0]["warnings"],
        json!(["policy_downgrade_unconfirmed"]),
        "{}",
        status.to_json()
    );
    assert_eq!(listed(&home, "demo")["effective_policy"], "secured");
}

/// ws002 s16 (req-116's done_when): three graphs, one unloaded when idle and one by hand, all listed by `graph list
/// --all` with state, path, counts and the reload command; `graph load <alias>` loads the unloaded one through the
/// index; a renamed folder shows missing; `graph forget` removes a row; a restart keeps the index; a secured
/// graph's row carries no content (security, plan 098).
#[test]
fn graph_list_all_indexes_every_graph_this_machine_loaded() {
    let mut home = Home::new("s16-index");
    home.start_with(&["--idle-unload", "8s"]);
    let make = |home: &Home, folder: &str, alias: &str, flags: &[&str]| -> String {
        let path = home.path.join(folder).display().to_string();
        let mut args = vec!["graph", "new", path.as_str(), "--alias", alias];
        args.extend_from_slice(flags);
        home.ok(&args);
        home.ok(&[
            "node",
            "add",
            "--graph",
            alias,
            "--type",
            "problem",
            "--title",
            "Vault secret title",
        ]);
        real(&path)
    };
    // Folders are not named like their aliases, so `graph load <alias>` can only find them through the index.
    let alpha = make(
        &home,
        "fa",
        "alpha",
        &["--embedding-model", "text-embedding-3-small@256"],
    );
    home.wait_until_idle();
    let bee = make(&home, "fb", "bee", &[]);
    home.ok(&["graph", "unload", "bee"]);
    let vault = make(&home, "fv", "vault", &["--secured"]);
    home.ok(&["graph", "unload", "vault"]);
    fs::create_dir_all(home.path.join("repo/.git")).unwrap();
    make(&home, "repo/fc", "cee", &[]);

    let all = home.ok(&["graph", "list", "--all"]);
    let row = |alias: &str| indexed(&all, alias);
    let a = row("alpha");
    assert_eq!(
        (
            &a["state"],
            &a["folder"],
            &a["nodes"],
            &a["edges"],
            &a["policy"]
        ),
        (
            &json!("idle"),
            &json!("present"),
            &json!(1),
            &json!(0),
            &json!("open")
        ),
        "{}",
        all.to_json()
    );
    assert_eq!(a["model"], "text-embedding-3-small@256");
    assert_eq!(a["path"], alpha.as_str());
    assert_eq!(a["reload"], format!("umx graph load {alpha} --as alpha"));
    assert!(a["last_loaded"].as_str().unwrap().starts_with("20"), "{a}");
    let b = row("bee");
    assert_eq!((&b["state"], &b["nodes"]), (&json!("unloaded"), &json!(1)));
    let c = row("cee");
    assert_eq!(c["state"], "loaded");
    assert_eq!(
        c["repo"],
        real(&home.path.join("repo").display().to_string())
    );
    let v = row("vault");
    assert_eq!(
        (&v["state"], &v["policy"], &v["path"]),
        (&json!("unloaded"), &json!("secured"), &json!(vault))
    );
    for key in ["nodes", "edges", "model", "last_loaded", "repo"] {
        assert!(v[key].is_null(), "{key}: {v}");
    }
    assert!(!v.to_string().contains("Vault secret"), "{v}");
    // Nothing is stored for it either: its row in the registry file has no counts or model.
    let file = fs::read_to_string(home.path.join("registry/graphs.toml")).unwrap();
    let stored = file
        .split("[[graphs]]")
        .find(|block| block.contains("alias = \"vault\""))
        .unwrap();
    assert!(
        !stored.contains("nodes") && !stored.contains("model"),
        "{stored}"
    );
    assert_eq!(home.aliases(), ["cee"], "plain graph list is unchanged");

    // By alias through the index: the folder `bee` names under the test's cwd holds no graph.
    let loaded = home.ok(&["graph", "load", "bee"]);
    assert_eq!(
        (
            &loaded.data["graph"]["alias"],
            &loaded.data["graph"]["path"]
        ),
        (&json!("bee"), &json!(bee))
    );

    fs::rename(&alpha, format!("{alpha}_moved")).unwrap();
    let all = home.ok(&["graph", "list", "--all"]);
    let a = indexed(&all, "alpha");
    assert_eq!(a["folder"], "missing");
    assert!(
        a["nodes"].is_null(),
        "a folder umx cannot read counts as secured: {a}"
    );

    home.ok(&["node", "list", "--graph", "cee"]);
    let (env, exit) = home.umx(&["graph", "forget", "cee"]);
    assert_eq!((exit, env.errors[0].code()), (1, "graph_forget_loaded"));
    let forgot = home.ok(&["graph", "forget", "alpha"]);
    assert_eq!(forgot.data["rows"], 1);
    let (env, exit) = home.umx(&["graph", "forget", "alpha"]);
    assert_eq!((exit, env.errors[0].code()), (1, "graph_not_known"));

    home.ok(&["graph", "unload", "cee"]);
    home.restart();
    let all = home.ok(&["graph", "list", "--all"]);
    let expect = [
        ("bee", "loaded"),
        ("cee", "unloaded"),
        ("vault", "unloaded"),
    ];
    assert_eq!(
        index_states(&all),
        expect.map(|(a, s)| (a.to_owned(), s.to_owned())),
        "{}",
        all.to_json()
    );
}

/// ws008 E1 with d-20: a secured graph that holds the key is searched only when the read names it.
#[test]
fn a_keyed_read_leaves_an_unnamed_secured_graph_out() {
    let mut home = Home::new("e1-secured");
    home.secured_graph();
    let open = home.path.join("open").display().to_string();
    home.ok(&["graph", "new", &open, "--alias", "open"]);
    for graph in ["demo", "open"] {
        home.ok(&[
            "node",
            "add",
            "--graph",
            graph,
            "--type",
            "person",
            "--title",
            "Jo",
            "--set",
            "entity_key=email:jo@x.com",
        ]);
    }
    let rows = |args: &[&str]| -> Vec<String> {
        home.ok(args).data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["graph"].as_str().unwrap().to_owned())
            .collect()
    };
    let w = "entity_key=email:jo@x.com";
    assert_eq!(
        rows(&["node", "list", "--graph", "open,open", "--where", w]),
        ["open"]
    );
    home.ok(&["scope", "save", "open_only", "--graph", "open"]);
    assert_eq!(
        rows(&["node", "list", "--scope", "open_only", "--where", w]),
        ["open"]
    );
    // Control: named, the secured graph's node is found.
    let mut named = rows(&["node", "list", "--graph", "open,demo", "--where", w]);
    named.sort();
    assert_eq!(named, ["demo", "open"]);
}

/// ws008 E2 (security): a secured node in the middle of a chain, loaded but not named, joins the two open members on
/// either side through the links the open graphs store to it, exactly as when its graph is not loaded at all: its own
/// links (here to C) are never followed and nothing from it is shown.
#[test]
fn a_loaded_unnamed_secured_node_mid_chain_reads_as_if_unloaded() {
    let mut home = Home::new("e2-mid");
    home.secured_graph();
    let s = home.ok(&[
        "node", "add", "--graph", "demo", "--type", "person", "--title", "Jo",
    ]);
    let s = s.data["node"]["address"].as_str().unwrap().to_owned();
    let n = e2_people(&mut home, &["open1", "open2", "open3"], &[]);
    let (a, b, c) = (n[0].clone(), n[1].clone(), n[2].clone());
    home.ok(&["entity", "link", &a, &s]);
    home.ok(&["entity", "link", &s, &b, "--graph", "open2"]);
    home.ok(&["entity", "link", &s, &c, "--graph", "demo"]);
    let loaded = home.ok(&["entity", "show", &a]);
    assert_eq!(
        e2_members(&loaded),
        [a.as_str(), b.as_str()],
        "{}",
        loaded.to_json()
    );
    home.ok(&["graph", "unload", "demo"]);
    let unloaded = home.ok(&["entity", "show", &a]);
    assert_eq!(
        loaded.data,
        unloaded.data,
        "{}\n{}",
        loaded.to_json(),
        unloaded.to_json()
    );
}

/// ws002 s17 end to end (D66, req-128): graph new adds to the repo's Graphfile, scan lists and --add writes what is
/// new, the file is byte-stable, list and load --graphfile read it (a set), and a moved folder is found and followed.
#[test]
fn a_repo_graphfile_is_made_scanned_listed_loaded_and_follows_a_move() {
    let mut home = Home::new("graphfile");
    let repo = fs::canonicalize(home.git_repo()).unwrap();
    let r = repo.to_str().unwrap().to_owned();
    let made = home.ok(&["graph", "new", &in_repo(&repo, "graphs/a"), "--alias", "a"]);
    assert_eq!(
        made.data["graphfile"]["path"],
        "graphs/a",
        "{}",
        made.to_json()
    );
    assert_eq!(
        made.data["graphfile"]["undo"],
        format!("umx graph scan {r} --drop a")
    );
    let quiet = home.ok(&[
        "graph",
        "new",
        &in_repo(&repo, "graphs/b"),
        "--alias",
        "b",
        "--no-graphfile",
    ]);
    assert!(quiet.data["graphfile"].is_null() && quiet.warnings.is_empty());
    let state = |env: &Envelope| -> Vec<(String, String)> {
        env.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["path"].as_str().unwrap().to_owned(),
                    row["state"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    let scanned = home.ok(&["graph", "scan", &r]);
    assert_eq!(
        state(&scanned),
        [
            ("graphs/a".into(), "listed".into()),
            ("graphs/b".into(), "new".into())
        ]
    );
    let added = home.ok(&["graph", "scan", &r, "--add"]);
    assert_eq!(
        (&added.data["added"], &added.data["written"]),
        (&json!(1), &json!(true))
    );
    let bytes = fs::read(repo.join("Graphfile")).unwrap();
    let again = home.ok(&["graph", "scan", &r, "--add"]);
    assert_eq!(again.data["written"], false);
    assert_eq!(
        fs::read(repo.join("Graphfile")).unwrap(),
        bytes,
        "byte-stable"
    );
    let mut with_set = bytes.clone();
    with_set.extend_from_slice(b"\n[sets]\nboth = [\"a\", \"b\"]\n");
    fs::write(repo.join("Graphfile"), &with_set).unwrap();

    home.ok(&["graph", "unload", "a"]);
    home.ok(&["graph", "unload", "b"]);
    let listed = home.ok(&["graph", "list", "--graphfile", &r]);
    assert_eq!(
        state(&listed),
        [
            ("graphs/a".into(), "not_loaded".into()),
            ("graphs/b".into(), "not_loaded".into())
        ]
    );
    assert_eq!(listed.data["sets"]["both"], json!(["a", "b"]));
    let loaded = home.ok(&["graph", "load", "--graphfile", &r, "--set", "both"]);
    let aliases: Vec<&str> = loaded.data["loaded"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["alias"].as_str().unwrap())
        .collect();
    assert_eq!(aliases, ["a", "b"], "{}", loaded.to_json());
    let listed = home.ok(&["graph", "list", "--graphfile", &r]);
    assert_eq!(listed.data["rows"][1]["loaded_as"], "b");

    // b's folder moves: scan finds it at its new path, --add follows it, load --graphfile loads it from there.
    home.ok(&["graph", "unload", "b"]);
    fs::rename(repo.join("graphs/b"), repo.join("graphs/b2")).unwrap();
    let moved = home.ok(&["graph", "scan", &r]);
    assert!(
        state(&moved).contains(&("graphs/b2".into(), "moved".into())),
        "{}",
        moved.to_json()
    );
    home.ok(&["graph", "scan", &r, "--add"]);
    assert!(
        String::from_utf8(fs::read(repo.join("Graphfile")).unwrap())
            .unwrap()
            .contains("path = \"graphs/b2\"")
    );
    let back = home.ok(&["graph", "load", "--graphfile", &r]);
    assert_eq!(back.data["not_loaded"], json!([]), "{}", back.to_json());

    // R289-2: a bare scan from a subfolder scans the whole repo, like the Graphfile it writes; a named folder narrows it.
    fs::create_dir_all(repo.join("docs")).unwrap();
    home.cwd = Some(repo.join("docs"));
    let bare = home.ok(&["graph", "scan"]);
    assert_eq!(
        state(&bare),
        [
            ("graphs/a".into(), "listed".into()),
            ("graphs/b2".into(), "listed".into())
        ],
        "{}",
        bare.to_json()
    );
    let narrowed = home.ok(&["graph", "scan", "."]);
    assert_eq!(state(&narrowed), [], "{}", narrowed.to_json());
    // agents-start-here's two repository lines, exactly as printed, from inside the repo.
    home.ok(&["graph", "unload", "a"]);
    let all = home.ok(&["graph", "load", "--graphfile"]);
    assert_eq!(all.data["not_loaded"], json!([]), "{}", all.to_json());
    assert!(
        all.data["loaded"].to_string().contains("\"a\""),
        "{}",
        all.to_json()
    );
    home.cwd = None;
}

/// F122: the graph that stores a cross-graph edge retypes and deletes it through the CLI, with the far
/// graph unloaded; after a reload of both, the retype stands, the deleted edge is gone, and the far graph's incoming
/// edges show only what is left.
#[test]
fn the_storing_graph_edits_and_deletes_a_cross_graph_edge() {
    let mut home = Home::new("f122");
    home.start();
    let (alpha, beta) = (home.path.join("alpha"), home.path.join("beta"));
    home.ok(&["graph", "new", alpha.to_str().unwrap(), "--alias", "alpha"]);
    home.ok(&["graph", "new", beta.to_str().unwrap(), "--alias", "beta"]);
    let add = |home: &Home, graph: &str, title: &str| {
        home.ok(&[
            "node", "add", "--graph", graph, "--type", "problem", "--title", title,
        ])
        .data["node"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (a, b) = (add(&home, "alpha", "near"), add(&home, "beta", "far"));
    let far = format!("umx:beta/{b}");
    let edges: Vec<String> = (0..2)
        .map(|i| {
            let rel = ["blocks", "raises"][i];
            home.ok(&["edge", "add", &a, rel, &far, "--graph", "alpha"])
                .data["edge"]["id"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    home.ok(&["graph", "unload", "beta"]);
    let edited = home.ok(&[
        "edge",
        "edit",
        &edges[0],
        "--relation",
        "supports",
        "--graph",
        "alpha",
    ]);
    assert_eq!(
        edited.data["edge"]["type"],
        "supports",
        "{}",
        edited.to_json()
    );
    assert!(
        edited.data["sentence"]
            .as_str()
            .unwrap()
            .contains(&format!("/{b}")),
        "the far end is its stored address: {}",
        edited.to_json()
    );
    let gone = home.ok(&["edge", "delete", &edges[1], "--graph", "alpha"]);
    assert_eq!(gone.data["deleted"], true);
    home.ok(&["graph", "unload", "alpha"]);
    home.ok(&["graph", "load", alpha.to_str().unwrap()]);
    home.ok(&["graph", "load", beta.to_str().unwrap()]);
    let into_beta = home.ok(&["edge", "list", "--to", &b, "--graph", "beta"]);
    let kinds: Vec<&str> = into_beta.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["supports"], "{}", into_beta.to_json());
}

/// ws008 TM6 (req-150): `node status --in <anchor> --evidence <pointer>` stores where the move happened, as the
/// anchor's `umx:<graph id>/<node id>`, local or in another graph, and the pointer it cites, on the move; node get
/// shows both. A move without them has neither key. An anchor of a type raised_in does not expect warns; a secured
/// far anchor never says its type.
#[test]
fn a_status_move_names_where_it_happened() {
    let mut home = Home::new("tm6-in");
    home.secured_graph();
    let hidden = home
        .ok(&[
            "node", "add", "--graph", "demo", "--type", "person", "--title", "Ada",
        ])
        .data["node"]["address"]
        .as_str()
        .unwrap()
        .to_owned();
    e2b_graphs(&mut home, &["g1", "g2"]);
    let node = e2b_node(&home, "g1", "risk", "Late", "");
    let segment = e2b_node(&home, "g1", "segment", "Morning", "");
    let meeting = e2b_node(&home, "g2", "meeting", "Kickoff", "");
    let person = e2b_node(&home, "g1", "person", "Bo", "");
    let canonical =
        |graph: &str, address: &str| format!("umx:{}/{}", e2c_gid(&home, graph), e2c_id(address));
    let here = tm6_status(&home, &node, "stalled", &["--in", &segment]);
    assert_eq!(
        here.data["in"],
        json!(canonical("g1", &segment)),
        "{}",
        here.to_json()
    );
    assert!(issues(&here, Code::RelationEndpointUnexpected).is_empty());
    let far = tm6_status(
        &home,
        &node,
        "active",
        &[
            "--in",
            &meeting,
            "--evidence",
            "notes.md#L1-L4",
            "--evidence-kind",
            "room",
        ],
    );
    assert_eq!(far.data["in"], json!(canonical("g2", &meeting)));
    tm6_status(&home, &node, "delivered", &[]);
    let soft = tm6_status(&home, &node, "active", &["--in", &person]);
    issue(&soft, Code::RelationEndpointUnexpected);
    let secured = tm6_status(&home, &node, "stalled", &["--in", &hidden]);
    issue(&secured, Code::LinkIntoSecured);
    assert!(
        issues(&secured, Code::RelationEndpointUnexpected).is_empty(),
        "{}",
        secured.to_json()
    );
    let moves = tm6_moves(&home, &node);
    assert_eq!(moves[0]["in"], json!(canonical("g1", &segment)));
    assert_eq!(moves[1]["in"], json!(canonical("g2", &meeting)));
    assert_eq!(
        (
            &moves[1]["evidence"]["kind"],
            moves[1]["evidence"]["source"]
                .as_str()
                .unwrap()
                .contains("notes.md")
        ),
        (&json!("room"), true),
        "{}",
        moves[1]
    );
    assert!(
        moves[2].get("in").is_none() && moves[2].get("evidence").is_none(),
        "a move without them is stored as before: {}",
        moves[2]
    );
}

/// #TM5 (req-149): `graph spine set` stores a declaration in graph.json and raises the format to the spines format;
/// with none declared the key is absent and the format is not raised. `show` places the graph on it now, `remove`
/// drops it (the format stays), and a set of the same declaration again writes nothing.
#[test]
fn a_declared_spine_round_trips_through_set_show_and_remove() {
    use umx_core::model::FORMAT_SPINES;
    let mut home = Home::new("tm5-round");
    let (path, _) = tm5_day(&mut home);
    let before = tm5_graph_json(&path);
    assert!(before.get("spines").is_none(), "{before}");
    assert!(before["umx"].as_u64().unwrap() < u64::from(FORMAT_SPINES));
    let decl = tm5_decl("day", "segment", true);
    let file = home.ops_file(&decl.to_string());
    let set = home.ok(&["graph", "spine", "set", "demo", "--file", &file]);
    assert_eq!(
        (
            &set.data["spine"],
            &set.data["replaced"],
            &set.data["unchanged"],
            &set.data["format"]
        ),
        (&decl, &json!(false), &json!(false), &json!(FORMAT_SPINES)),
        "{}",
        set.to_json()
    );
    let stored = tm5_graph_json(&path);
    assert_eq!(
        (&stored["umx"], &stored["spines"]),
        (&json!(FORMAT_SPINES), &json!([decl])),
        "{stored}"
    );
    let again = home.ok(&["graph", "spine", "set", "demo", "--file", &file]);
    assert_eq!(again.data["unchanged"], json!(true));
    let shown = home.ok(&["graph", "spine", "show", "demo"]);
    let day = &shown.data["spines"][0];
    assert_eq!(
        (&day["name"], &day["clock"], &day["offered"], &day["decl"]),
        (&json!("day"), &json!("calendar"), &json!(true), &decl),
        "{}",
        shown.to_json()
    );
    assert_eq!(
        day["cover"],
        json!({"total": 8, "by_field": 2, "by_link": 6, "by_evidence": 0, "by_rule": 0, "not_placed": 0})
    );
    let one = home.ok(&["graph", "spine", "show", "demo", "day"]);
    assert_eq!(one.data["spines"], shown.data["spines"]);
    let removed = home.ok(&["graph", "spine", "remove", "demo", "day"]);
    assert_eq!(removed.data["removed"], "day");
    let after = tm5_graph_json(&path);
    assert_eq!(
        (after.get("spines"), &after["umx"]),
        (None, &json!(FORMAT_SPINES)),
        "a write never lowers the format: {after}"
    );
    let none = home.ok(&["graph", "spine", "show", "demo"]);
    assert_eq!(
        (&none.data["spines"], &none.data["next"]),
        (&json!([]), &json!("umx graph spine propose demo"))
    );
}

/// #TM5: `confirm` stores the named candidate's declaration exactly as `propose` lists it (every key and value);
/// a built-in candidate is refused with `set` as its next, and an unknown one lists the candidates.
#[test]
fn confirm_stores_the_proposed_declaration_unchanged() {
    let mut home = Home::new("tm5-confirm");
    let (path, _) = tm5_day(&mut home);
    let proposed = home.ok(&["graph", "spine", "propose", "demo"]);
    let decl = proposed.data["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "type:segment")
        .unwrap()["decl"]
        .clone();
    assert!(decl.is_object(), "{}", proposed.to_json());
    let confirmed = home.ok(&["graph", "spine", "confirm", "demo", "type:segment"]);
    assert_eq!(confirmed.data["spine"], decl);
    let stored = tm5_graph_json(&path)["spines"][0].clone();
    assert_eq!(stored, decl);
    let (env, exit) = home.umx(&["graph", "spine", "confirm", "demo", "recorded"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert_eq!(
        issue(&env, Code::SpineBuiltinCandidate).next(),
        "umx graph spine set demo --file <decl.json>"
    );
    let (env, exit) = home.umx(&["graph", "spine", "confirm", "demo", "type:nothing"]);
    assert_ne!(exit, 0, "{}", env.to_json());
    issue(&env, Code::UsageInvalid);
    assert_eq!(tm5_graph_json(&path)["spines"], json!([decl]));
}

/// #TM5: every name in a declaration is the graph's vocabulary or std (a declared type and its own field count),
/// a graph has one main spine, and a declaration of another shape or with a bad name is refused; a refusal writes
/// nothing.
#[test]
fn a_spine_declaration_is_checked_before_it_is_stored() {
    let mut home = Home::new("tm5-check");
    let path = home.graph();
    let set = |home: &Home, decl: &Value| {
        let file = home.ops_file(&decl.to_string());
        home.umx(&["graph", "spine", "set", "demo", "--file", &file])
    };
    let refused = |home: &Home, decl: Value, code: Code| {
        let (env, exit) = set(home, &decl);
        assert_ne!(exit, 0, "{decl}: {}", env.to_json());
        issue(&env, code);
        env
    };
    let env = refused(
        &home,
        tm5_decl("w", "widget", false),
        Code::SpineNameUnknown,
    );
    assert!(env.to_json().contains("anchors.type"), "{}", env.to_json());
    let mut bad = tm5_decl("w", "segment", false);
    bad["order"] = json!(["starts_on", "finished_on"]);
    refused(&home, bad.clone(), Code::SpineNameUnknown);
    bad["order"] = json!(["starts_on"]);
    bad["place"] = json!(["raised_in", "lands_after"]);
    refused(&home, bad, Code::SpineNameUnknown);
    let mut extra = tm5_decl("w", "segment", false);
    extra["zone"] = json!("x");
    refused(&home, extra, Code::UsageInvalid);
    let mut name = tm5_decl("w", "segment", false);
    name["name"] = json!("Day One");
    refused(&home, name, Code::UsageInvalid);
    let mut title = tm5_decl("w", "segment", false);
    title["title"] = json!(" padded");
    refused(&home, title, Code::UsageInvalid);
    assert!(tm5_graph_json(&path).get("spines").is_none());
    // A declared type, its own date field and a declared relation count as vocabulary.
    home.ok(&["ontology", "add", "field", "sprint", "closes_on:date"]);
    home.ok(&["ontology", "add", "relation", "lands_in"]);
    let own = json!({
        "name": "sprints", "title": "sprint by closes_on", "main": true,
        "anchors": {"type": "sprint"}, "order": ["closes_on"], "place": ["lands_in", "evidence"],
    });
    let (env, exit) = set(&home, &own);
    assert_eq!(exit, 0, "{}", env.to_json());
    let env = refused(
        &home,
        tm5_decl("day", "segment", true),
        Code::SpineMainTaken,
    );
    assert_eq!(
        issue(&env, Code::SpineMainTaken).next(),
        "umx graph spine show demo sprints"
    );
    let (env, exit) = set(&home, &tm5_decl("day", "segment", false));
    assert_eq!(exit, 0, "{}", env.to_json());
    let names: Vec<Value> = tm5_graph_json(&path)["spines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].clone())
        .collect();
    assert_eq!(names, vec![json!("sprints"), json!("day")]);
}

/// #TM5: set, confirm and remove write graph.json through the checked write: when the graph's files changed on disk
/// since the load, each is refused with graph_files_changed and graph.json is left as the checkout made it.
#[test]
fn a_spine_write_is_refused_when_the_graph_changed_on_disk() {
    let mut home = Home::new("tm5-moved");
    let (path, _) = tm5_day(&mut home);
    let file = home.ops_file(&tm5_decl("day", "segment", true).to_string());
    home.ok(&["graph", "spine", "set", "demo", "--file", &file]);
    let mut moved = tm5_graph_json(&path);
    moved["title"] = json!("Checked out");
    checkout(&path, "graph.json", moved.to_string().as_bytes());
    for args in [
        vec!["graph", "spine", "set", "demo", "--file", file.as_str()],
        vec!["graph", "spine", "confirm", "demo", "type:segment"],
        vec!["graph", "spine", "remove", "demo", "day"],
    ] {
        let (env, exit) = home.umx(&args);
        assert_ne!(exit, 0, "{args:?}: {}", env.to_json());
        issue(&env, Code::GraphFilesChanged);
    }
    assert_eq!(tm5_graph_json(&path), moved);
}

/// R366-3: a graph.json whose stored spine umx could not have written (a hand edit: a main spine named
/// `Day One`) is refused at load, with a structured error, and the daemon's writer keeps working.
#[test]
fn a_hand_edited_invalid_stored_spine_is_refused_at_load_and_the_writer_lives() {
    let mut home = Home::new("tm5-stored");
    let (path, _) = tm5_day(&mut home);
    let file = home.ops_file(&tm5_decl("day", "segment", true).to_string());
    home.ok(&["graph", "spine", "set", "demo", "--file", &file]);
    let mut edited = tm5_graph_json(&path);
    edited["spines"][0]["name"] = json!("Day One");
    fs::write(Path::new(&path).join("graph.json"), edited.to_string()).unwrap();
    home.ok(&["graph", "unload", "demo"]);
    let (env, exit) = home.umx(&["graph", "load", &path]);
    assert_ne!(exit, 0, "{}", env.to_json());
    assert!(!env.errors.is_empty(), "{}", env.to_json());
    // The writer was never poisoned: the next write and read both answer.
    let other = home.path.join("other").display().to_string();
    home.ok(&["graph", "new", &other, "--alias", "other"]);
    let second = home.ops_file(&tm5_decl("days", "segment", true).to_string());
    home.ok(&["graph", "spine", "set", "other", "--file", &second]);
    home.ok(&["graph", "list"]);
}

/// Security N1: a declaration has at most 16 order and 16 place entries, and a graph at most 32 spines; past a
/// cap it is refused and nothing is written.
#[test]
fn a_spine_declaration_past_its_caps_is_refused() {
    let mut home = Home::new("tm5-caps");
    let path = home.graph();
    let set = |home: &Home, decl: &Value| {
        let file = home.ops_file(&decl.to_string());
        home.umx(&["graph", "spine", "set", "demo", "--file", &file])
    };
    let mut long = tm5_decl("long", "segment", false);
    long["order"] = json!(vec!["starts_on"; 17]);
    let (env, exit) = set(&home, &long);
    assert_ne!(exit, 0, "{}", env.to_json());
    issue(&env, Code::UsageInvalid);
    let mut wide = tm5_decl("wide", "segment", false);
    wide["place"] = json!(vec!["raised_in"; 17]);
    let (env, exit) = set(&home, &wide);
    assert_ne!(exit, 0, "{}", env.to_json());
    assert!(tm5_graph_json(&path).get("spines").is_none());
    for i in 0..32 {
        let (env, exit) = set(&home, &tm5_decl(&format!("s{i}"), "segment", false));
        assert_eq!(exit, 0, "{i}: {}", env.to_json());
    }
    let (env, exit) = set(&home, &tm5_decl("s32", "segment", false));
    assert_ne!(exit, 0, "{}", env.to_json());
    issue(&env, Code::UsageInvalid);
    assert_eq!(
        tm5_graph_json(&path)["spines"].as_array().unwrap().len(),
        32
    );
}

/// Project manager point 1: propose may offer a candidate built from a type the graph has not declared; confirming it
/// is refused spine_name_unknown, and its next is the exact command that declares it, after which confirm stores it.
#[test]
fn confirming_an_undeclared_candidate_names_the_command_that_declares_it() {
    let mut home = Home::new("tm5-declare");
    home.graph();
    let sprints: Vec<String> = ["Sprint one", "Sprint two"]
        .iter()
        .map(|t| home.add("sprint", t))
        .collect();
    for (i, s) in sprints.iter().enumerate() {
        for p in 0..3 {
            let title = format!("Task {i}{p}");
            home.ok(&[
                "node",
                "add",
                "--type",
                "task",
                "--title",
                &title,
                "--raised-in",
                s,
            ]);
        }
    }
    let proposed = home.ok(&["graph", "spine", "propose", "demo"]);
    assert!(
        proposed.data["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "type:sprint"),
        "{}",
        proposed.to_json()
    );
    let (env, exit) = home.umx(&["graph", "spine", "confirm", "demo", "type:sprint"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let next = issue(&env, Code::SpineNameUnknown).next().to_string();
    assert_eq!(next, "umx ontology add type sprint --graph demo");
    let (_, exit) = home.run_next(&next);
    assert_eq!(exit, 0);
    home.ok(&["graph", "spine", "confirm", "demo", "type:sprint"]);
}

/// Project manager point 4 (B4a): propose, show and guide count the nodes placed only by rule and give the command that
/// lists them; each command runs as printed and lists exactly those ids.
#[test]
fn every_spine_read_names_the_command_that_lists_its_rule_placements() {
    let mut home = Home::new("tm5-by-rule");
    let (_, _) = tm5_day(&mut home);
    home.ok(&["ontology", "add", "field", "segment", "starts_on:date"]);
    let point = home
        .ok(&["node", "list", "--type", "risk", "--limit", "1"])
        .data["rows"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let loose = home.add("problem", "Loose end");
    home.ok(&["edge", "add", &loose, "supports", &point]);
    let listed = |env: &Envelope, key: &str| -> Vec<Value> {
        env.data[key]["ids"].as_array().unwrap().clone()
    };
    let loose_full = home.ok(&["node", "get", &loose]).data["node"]["id"].clone();
    let proposed = home.ok(&["graph", "spine", "propose", "demo"]);
    let hint = &proposed.data["hints"]["by_rule"];
    assert_eq!(
        (&hint["candidate"], &hint["count"], &hint["next"]),
        (
            &json!("type:segment"),
            &json!(1),
            &json!("umx graph spine propose demo --by-rule type:segment")
        ),
        "{}",
        proposed.to_json()
    );
    let (env, exit) = home.run_next(hint["next"].as_str().unwrap());
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(listed(&env, "by_rule"), vec![loose_full.clone()]);
    let guide = home.ok(&["graph", "guide", "demo"]);
    assert_eq!(
        guide.data["spine"]["by_rule_next"],
        json!("umx graph spine propose demo --by-rule type:segment"),
        "{}",
        guide.to_json()
    );
    let file = home.ops_file(&tm5_decl("day", "segment", true).to_string());
    home.ok(&["graph", "spine", "set", "demo", "--file", &file]);
    let shown = home.ok(&["graph", "spine", "show", "demo"]);
    let next = shown.data["spines"][0]["by_rule_next"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(next, "umx graph spine show demo --by-rule day");
    let (env, exit) = home.run_next(&next);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(listed(&env, "by_rule"), vec![loose_full]);
}

/// `node list --columns spine:<name>` gives each node's band along a declared spine, in the spine's order, and how
/// the one placement rule placed it; null where the graph declares no such spine.
#[test]
fn node_list_shows_each_nodes_place_on_a_declared_spine() {
    let mut home = Home::new("spine-column");
    let (_, _) = tm5_day(&mut home);
    home.ok(&["ontology", "add", "field", "segment", "starts_on:date"]);
    let file = home.ops_file(&tm5_decl("day", "segment", true).to_string());
    home.ok(&["graph", "spine", "set", "demo", "--file", &file]);
    let rows = |kind: &str, cols: &str| -> Vec<Value> {
        let env = home.ok(&[
            "node",
            "list",
            "--type",
            kind,
            "--columns",
            cols,
            "--limit",
            "100",
        ]);
        assert!(
            env.data["columns"]
                .as_array()
                .unwrap()
                .contains(&json!("spine:day"))
                || cols.contains("nope")
        );
        env.data["rows"].as_array().unwrap().clone()
    };
    let mut segs: Vec<(String, Value)> = rows("segment", "title,spine:day")
        .iter()
        .map(|r| {
            (
                r["title"].as_str().unwrap().to_owned(),
                r["spine:day"].clone(),
            )
        })
        .collect();
    segs.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        segs,
        vec![
            (
                "Afternoon".to_owned(),
                json!({ "band": 1, "by": "field", "anchor": true })
            ),
            (
                "Morning".to_owned(),
                json!({ "band": 0, "by": "field", "anchor": true })
            ),
        ]
    );
    for r in rows("risk", "title,spine:day") {
        let band = if r["title"].as_str().unwrap().starts_with("Point 0") {
            0
        } else {
            1
        };
        assert_eq!(
            r["spine:day"],
            json!({ "band": band, "by": "link", "anchor": false }),
            "{r}"
        );
    }
    for r in rows("risk", "id,spine:nope") {
        assert_eq!(r["spine:nope"], Value::Null, "{r}");
    }
}

/// `--columns spine:<name>` over several graphs: each row's cell comes from its own graph's spine (null where that
/// graph declares none), and a secured graph's rows, with their cells, show only when it is named.
#[test]
fn node_list_spine_column_over_several_graphs_and_a_secured_one() {
    let mut home = Home::new("spine-column-graphs");
    let (_, _) = tm5_day(&mut home);
    home.ok(&["ontology", "add", "field", "segment", "starts_on:date"]);
    let file = home.ops_file(&tm5_decl("day", "segment", true).to_string());
    home.ok(&["graph", "spine", "set", "demo", "--file", &file]);
    for alias in ["plain", "vault"] {
        let path = home.path.join(alias).display().to_string();
        home.ok(&["graph", "new", &path, "--alias", alias]);
    }
    home.ok(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "Plain risk",
        "--graph",
        "plain",
    ]);
    home.ok(&[
        "node",
        "add",
        "--type",
        "segment",
        "--title",
        "Vault day",
        "--set",
        "starts_on=2026-11-12T09:00:00Z",
        "--graph",
        "vault",
    ]);
    home.ok(&[
        "ontology",
        "add",
        "field",
        "segment",
        "starts_on:date",
        "--graph",
        "vault",
    ]);
    home.ok(&["graph", "spine", "set", "vault", "--file", &file]);
    home.ok(&["graph", "set", "vault", "--policy", "secured"]);
    let cells = |graphs: &[&str]| -> Vec<(String, Value)> {
        let mut args = vec![
            "node",
            "list",
            "--columns",
            "graph,title,spine:day",
            "--limit",
            "100",
        ];
        args.extend_from_slice(graphs);
        let env = home.ok(&args);
        env.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["graph"].as_str().unwrap().to_owned(),
                    r["spine:day"].clone(),
                )
            })
            .collect()
    };
    let both = cells(&["--graph", "demo,plain"]);
    assert!(both.iter().all(|(g, _)| g != "vault"), "{both:?}");
    assert!(
        both.iter().any(|(g, c)| g == "demo" && c["by"].is_string()),
        "{both:?}"
    );
    assert_eq!(
        both.iter()
            .filter(|(g, _)| g == "plain")
            .map(|(_, c)| c.clone())
            .collect::<Vec<_>>(),
        vec![Value::Null]
    );
    let named = cells(&["--graph", "demo,vault"]);
    assert_eq!(
        named
            .iter()
            .filter(|(g, _)| g == "vault")
            .map(|(_, c)| c.clone())
            .collect::<Vec<_>>(),
        vec![json!({ "band": 0, "by": "field", "anchor": true })]
    );
}

/// ws006 S23 (req-56): `umx graph rename` keeps the old alias in graph.json, so `umx:<old>/<id>` resolves as
/// `redirected`, reads and writes naming it follow with one `alias_redirected` hint, an inbound edge and stored
/// evidence from another graph keep working, new evidence through the old alias is stored under the live one,
/// `--graph old,new` is one graph, saved scopes follow, and all of it survives a restart.
#[test]
fn graph_rename_redirects_the_old_alias_everywhere() {
    let mut home = Home::new("s23-rename");
    let (a, x, y) = s23_pair(&mut home);
    home.ok(&["scope", "save", "both", "--graph", "alpha,beta"]);
    let renamed = home.ok(&["graph", "rename", "alpha", "gamma"]);
    assert_eq!(
        (
            &renamed.data["graph"],
            &renamed.data["from"],
            &renamed.data["former_aliases"]
        ),
        (&json!("gamma"), &json!("alpha"), &json!(["alpha"])),
        "{}",
        renamed.to_json()
    );
    let meta = s23_meta(&a);
    assert_eq!(
        (&meta["alias"], &meta["former_aliases"], &meta["umx"]),
        (&json!("gamma"), &json!(["alpha"]), &json!(7)),
        "{meta}"
    );
    for _restart in 0..2 {
        let (env, exit) = home.umx(&["resolve", &format!("umx:alpha/{x}")]);
        assert_eq!(exit, 0, "{}", env.to_json());
        assert_eq!(
            (
                &env.data["status"],
                &env.data["graph"],
                &env.data["redirected_from"],
                &env.data["address"]
            ),
            (
                &json!("redirected"),
                &json!("gamma"),
                &json!("alpha"),
                &json!(format!("umx:gamma/{x}"))
            ),
            "{}",
            env.to_json()
        );
        let read = home.ok(&["node", "get", &format!("umx:alpha/{x}")]);
        assert_eq!(read.data["node"]["title"], "Target");
        let hint = issue(&read, Code::AliasRedirected);
        assert!(
            hint.message().contains("alpha") && hint.message().contains("gamma"),
            "{}",
            hint.message()
        );
        let edges = home.ok(&["edge", "list", "--graph", "beta", "--from", &y]);
        assert_eq!(
            edges.data["rows"][0]["to_title"],
            "Target",
            "{}",
            edges.to_json()
        );
        let checked = home.ok(&["check", "--graph", "gamma,beta"]);
        assert_eq!(checked.data["total"], 0, "{}", checked.to_json());
        let one = home.ok(&["node", "list", "--graph", "alpha,gamma"]);
        assert_eq!(one.data["total"], 1, "{}", one.to_json());
        let scoped = home.ok(&["node", "list", "--scope", "both"]);
        assert!(
            issues(&scoped, Code::ScopeMemberMoved).is_empty(),
            "{}",
            scoped.to_json()
        );
        home.stop();
        home.start();
    }
    // New evidence through the old alias: stored under the live alias, with the hint, never dangling.
    let z = home
        .ok(&[
            "node", "add", "--graph", "beta", "--type", "problem", "--title", "Later",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let added = home.ok(&[
        "node",
        "evidence",
        "--graph",
        "beta",
        &z,
        "add",
        &format!("umx:alpha/{x}#quote=Target"),
    ]);
    issue(&added, Code::AliasRedirected);
    assert!(
        issues(&added, Code::DanglingEvidence).is_empty()
            && issues(&added, Code::EvidenceUnchecked).is_empty()
    );
    let got = home.ok(&["node", "get", "--graph", "beta", &z]);
    assert_eq!(
        got.data["node"]["evidence"][0]["source"],
        json!(format!("umx:gamma/{x}"))
    );
    // The pointer stored before the rename is never rewritten.
    let before = home.ok(&["node", "get", "--graph", "beta", &y]);
    assert_eq!(
        before.data["node"]["evidence"][0]["source"],
        json!(format!("umx:alpha/{x}"))
    );
    let listed = home.ok(&["graph", "list", "--all"]).to_json();
    assert!(
        listed.contains("\"gamma\"") && !listed.contains("\"alpha\""),
        "{listed}"
    );
}

/// ws006 S23: a chain `alpha -> mid -> gamma` resolves both former aliases in one step; renaming back to a former
/// alias is allowed and takes it off the list (graph-addressing-expert (c)); an alias in use is refused; past 16
/// former aliases the oldest is dropped and named.
#[test]
fn a_rename_chain_resolves_and_renaming_back_is_allowed() {
    let mut home = Home::new("s23-chain");
    let (a, x, _) = s23_pair(&mut home);
    home.ok(&["graph", "rename", "alpha", "mid"]);
    home.ok(&["graph", "rename", "mid", "gamma"]);
    for former in ["alpha", "mid"] {
        let env = home.ok(&["resolve", &format!("umx:{former}/{x}")]);
        assert_eq!(
            (&env.data["status"], &env.data["graph"]),
            (&json!("redirected"), &json!("gamma"))
        );
    }
    home.ok(&["graph", "rename", "gamma", "alpha"]);
    assert_eq!(s23_meta(&a)["former_aliases"], json!(["mid", "gamma"]));
    assert_eq!(
        home.ok(&["resolve", &format!("umx:alpha/{x}")]).data["status"],
        "ok"
    );
    assert_eq!(
        home.ok(&["resolve", &format!("umx:gamma/{x}")]).data["status"],
        "redirected"
    );
    let (env, exit) = home.umx(&["graph", "rename", "alpha", "beta"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::AliasInUse);
    let mut live = "alpha".to_string();
    // [mid, gamma] plus alpha and r0..r12 is 16; the 17th (r13) drops the oldest, mid.
    for i in 0..14 {
        let next = format!("r{i}");
        home.ok(&["graph", "rename", &live, &next]);
        live = next;
    }
    let last = home.ok(&["graph", "rename", &live, "omega"]);
    assert_eq!(last.data["dropped"], json!(["mid"]), "{}", last.to_json());
    assert_eq!(s23_meta(&a)["former_aliases"].as_array().unwrap().len(), 16);
    // The live alias wins: `alpha` is a former alias of omega, but once this machine keeps it for another, unloaded
    // graph, it names that graph (here: not loaded, since it was unloaded on purpose), never omega.
    let other = home.path.join("other").display().to_string();
    home.ok(&["graph", "new", &other, "--alias", "alpha"]);
    home.ok(&["graph", "unload", "alpha"]);
    let (read, exit) = home.umx(&["node", "list", "--graph", "alpha"]);
    assert_eq!(exit, 1, "{}", read.to_json());
    assert!(
        issue(&read, Code::GraphNotLoaded).next().contains("/other"),
        "{}",
        read.to_json()
    );
}

/// ws006 S23 (security N2): a former alias held by an open graph and by a secured unnamed one. Evidence through it
/// resolves to the open graph only: no ambiguity, and no hint or reply names the secured graph.
#[test]
fn a_former_alias_shared_with_a_secured_graph_resolves_to_the_open_one() {
    let mut home = Home::new("s23-shared");
    let (_, x, _) = s23_pair(&mut home);
    home.ok(&["graph", "rename", "alpha", "opened"]);
    let c = home.path.join("c").display().to_string();
    home.ok(&["graph", "new", &c, "--alias", "alpha"]);
    home.ok(&["graph", "set", "alpha", "--policy", "secured"]);
    home.ok(&["graph", "rename", "alpha", "hidden"]);
    let n = home
        .ok(&[
            "node", "add", "--graph", "beta", "--type", "problem", "--title", "Later",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let added = home.ok(&[
        "node",
        "evidence",
        "--graph",
        "beta",
        &n,
        "add",
        &format!("umx:alpha/{x}#quote=Target"),
    ]);
    let hint = issue(&added, Code::AliasRedirected);
    assert!(hint.message().contains("opened"), "{}", added.to_json());
    assert!(
        issues(&added, Code::FormerAliasAmbiguous).is_empty()
            && issues(&added, Code::EvidenceUnchecked).is_empty()
            && issues(&added, Code::DanglingEvidence).is_empty()
            && !added.to_json().contains("hidden"),
        "{}",
        added.to_json()
    );
    let got = home.ok(&["node", "get", "--graph", "beta", &n]);
    assert_eq!(
        got.data["node"]["evidence"][0]["source"],
        json!(format!("umx:opened/{x}"))
    );
}

/// ws006 S23: a crash after graph.json but before the registry heals at load, and the Graphfile entry and its set
/// follow a rename.
#[test]
fn a_half_done_rename_heals_and_the_graphfile_follows() {
    let mut home = Home::new("s23-heal");
    let repo = home.git_repo();
    let a = in_repo(&repo, "a");
    home.ok(&["graph", "new", &a, "--alias", "alpha"]);
    let x = home.add("problem", "Kept");
    let gf = repo.join("Graphfile");
    let mut text = fs::read_to_string(&gf).unwrap();
    text.push_str("\n[sets]\nteam = [\"alpha\"]\n");
    fs::write(&gf, &text).unwrap();
    let renamed = home.ok(&["graph", "rename", "alpha", "gamma"]);
    assert_eq!(
        renamed.data["graphfile"],
        "updated",
        "{}",
        renamed.to_json()
    );
    let after = fs::read_to_string(&gf).unwrap();
    assert!(
        after.contains("alias = \"gamma\"")
            && after.contains("team = [\"gamma\"]")
            && !after.contains("alpha"),
        "{after}"
    );
    home.stop();
    let registry = home.path.join("registry/graphs.toml");
    let rows = fs::read_to_string(&registry).unwrap();
    assert!(rows.contains("alias = \"gamma\""), "{rows}");
    fs::write(
        &registry,
        rows.replace("alias = \"gamma\"", "alias = \"alpha\""),
    )
    .unwrap();
    home.start();
    let listed = home.ok(&["graph", "list"]);
    assert_eq!(
        listed.data["rows"][0]["alias"],
        "gamma",
        "{}",
        listed.to_json()
    );
    // Review R6: the start-up reload records the alias the graph loaded as, so the old one redirects again.
    let rows = fs::read_to_string(&registry).unwrap();
    assert!(
        rows.contains("alias = \"gamma\"") && !rows.contains("alias = \"alpha\""),
        "{rows}"
    );
    let resolved = home.ok(&["resolve", &format!("umx:alpha/{x}")]);
    assert_eq!(
        resolved.data["status"],
        "redirected",
        "{}",
        resolved.to_json()
    );
}

/// ws006 S23: `umx graph move` moves the folder in one rename inside its repo; graph.json keeps the folder it left,
/// so the move reply and `umx check` name this graph's `root:repo` pointers into it (a pointer elsewhere, and one
/// relative to the graph folder, are not stale); the registry, scope hints and Graphfile follow; a restart loads it
/// from the new place. A folder that exists, or one in another repo, is refused and nothing moves.
#[test]
fn graph_move_moves_the_folder_and_names_stale_root_repo_pointers() {
    let mut home = Home::new("s23-move");
    let repo = home.git_repo();
    let (a, b) = (in_repo(&repo, "a"), in_repo(&repo, "b"));
    home.ok(&["graph", "new", &a, "--alias", "alpha"]);
    let n = home.add("problem", "Cited");
    for pointer in [
        "root:repo/a/notes.md#L2",
        "root:repo/docs/x.md#L1",
        "sources/t.md#L1",
    ] {
        home.ok(&["node", "evidence", &n, "add", pointer]);
    }
    home.ok(&["scope", "save", "mine", "--graph", "alpha"]);
    let moved = home.ok(&["graph", "move", "alpha", &b]);
    assert_eq!(
        (
            &moved.data["former_paths"],
            &moved.data["stale"],
            &moved.data["stale_total"],
            &moved.data["graphfile"]
        ),
        (
            &json!(["a"]),
            &json!([{"id": n, "place": "evidence:1"}]),
            &json!(1),
            &json!("updated")
        ),
        "{}",
        moved.to_json()
    );
    assert_eq!(moved.data["other_graphs_checked"], false);
    issue(&moved, Code::EvidenceRootStale);
    assert!(!Path::new(&a).exists() && Path::new(&b).join("graph.json").exists());
    assert_eq!(s23_meta(&b)["former_paths"], json!(["a"]));
    let gf = fs::read_to_string(repo.join("Graphfile")).unwrap();
    assert!(
        gf.contains("path = \"b\"") && !gf.contains("path = \"a\""),
        "{gf}"
    );
    for _restart in 0..2 {
        let checked = home.ok(&["check", "--graph", "alpha"]);
        let rows: Vec<(Value, Value)> = checked.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["kind"].clone(), r["subject"].clone()))
            .collect();
        assert_eq!(
            rows,
            vec![(
                json!("evidence_root_stale"),
                json!(format!("{n} evidence:1"))
            )],
            "{}",
            checked.to_json()
        );
        let all = home.ok(&["graph", "list", "--all"]);
        assert_eq!(
            all.data["rows"].as_array().unwrap().len(),
            1,
            "{}",
            all.to_json()
        );
        assert!(all.to_json().contains("/repo/b"), "{}", all.to_json());
        let scoped = home.ok(&["node", "list", "--scope", "mine"]);
        assert!(
            issues(&scoped, Code::ScopeMemberMoved).is_empty(),
            "{}",
            scoped.to_json()
        );
        home.stop();
        home.start();
    }
    let taken = in_repo(&repo, "taken");
    fs::create_dir_all(&taken).unwrap();
    let (env, exit) = home.umx(&["graph", "move", "alpha", &taken]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::GraphMoveRefused);
    let other = home.path.join("other");
    fs::create_dir_all(other.join(".git")).unwrap();
    let (env, _) = home.umx(&["graph", "move", "alpha", other.join("x").to_str().unwrap()]);
    issue(&env, Code::GraphMoveCrossRepo);
    assert!(Path::new(&b).join("graph.json").exists(), "nothing moved");
    assert_eq!(home.ok(&["node", "get", &n]).data["node"]["title"], "Cited");
}

/// ws006 S23 (architect, graph-addressing-expert (g)): a `former_paths` entry naming the graph's own folder (a
/// move cut short before its rename) is not stale; a folder moved by hand loads as a move (one registry row, the
/// stale pointers listed once) unless the old folder still holds the graph (a copy, d-21).
#[test]
fn a_hand_move_heals_on_load_and_a_cut_short_move_is_not_stale() {
    let mut home = Home::new("s23-hand");
    let repo = home.git_repo();
    let (a, b) = (in_repo(&repo, "a"), in_repo(&repo, "b"));
    home.ok(&["graph", "new", &a, "--alias", "alpha"]);
    let n = home.add("problem", "Cited");
    home.ok(&["node", "evidence", &n, "add", "root:repo/a/notes.md#L2"]);
    home.ok(&["graph", "unload", "alpha"]);
    let mut meta = s23_meta(&a);
    meta["former_paths"] = json!(["a"]);
    meta["umx"] = json!(7);
    fs::write(Path::new(&a).join("graph.json"), meta.to_string()).unwrap();
    home.ok(&["graph", "load", &a]);
    assert_eq!(home.ok(&["check", "--graph", "alpha"]).data["total"], 0);
    home.ok(&["graph", "unload", "alpha"]);
    fs::rename(&a, &b).unwrap();
    let loaded = home.ok(&["graph", "load", &b]);
    assert!(
        loaded.data["moved_from"]
            .as_str()
            .is_some_and(|f| f.ends_with("/repo/a")),
        "{}",
        loaded.to_json()
    );
    assert_eq!(
        (&loaded.data["stale_total"], &loaded.data["stale_recorded"]),
        (&json!(1), &json!(false)),
        "{}",
        loaded.to_json()
    );
    assert_eq!(
        home.ok(&["graph", "list", "--all"]).data["rows"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // A copy: the old folder still holds the graph, so loading another folder is no move.
    home.ok(&["graph", "unload", "alpha"]);
    let copy = in_repo(&repo, "c");
    fs::create_dir_all(&copy).unwrap();
    for f in ["graph.json", "nodes.jsonl", "edges.jsonl", ".gitignore"] {
        let _ = fs::copy(Path::new(&b).join(f), Path::new(&copy).join(f));
    }
    home.ok(&["graph", "load", &b]);
    let second = home.ok(&["graph", "load", &copy, "--as", "alpha_copy"]);
    assert!(
        second.data.get("moved_from").is_none(),
        "{}",
        second.to_json()
    );
}

/// ws006 S23 (architect (f)): every verb that takes a graph name reads a former alias as the graph's live alias,
/// with one `alias_redirected` hint: by `--graph`, by address, by edge end and by position.
#[test]
fn every_alias_taking_verb_reads_a_former_alias() {
    let mut home = Home::new("s23-verbs");
    let (_, x, _) = s23_pair(&mut home);
    home.ok(&["graph", "rename", "alpha", "gamma"]);
    let z = home
        .ok(&[
            "node", "add", "--graph", "beta", "--type", "problem", "--title", "Other",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (addr, end) = (format!("umx:alpha/{x}"), format!("umx:alpha/{x}"));
    let walk: Vec<Vec<&str>> = vec![
        vec!["graph", "guide", "alpha"],
        vec!["graph", "set", "alpha", "--title", "Renamed"],
        vec!["graph", "spine", "propose", "alpha"],
        vec!["graph", "spine", "show", "alpha"],
        vec!["graph", "primary", "alpha"],
        vec!["node", "list", "--graph", "alpha"],
        vec![
            "node", "add", "--graph", "alpha", "--type", "problem", "--title", "More",
        ],
        vec!["node", "get", &addr],
        vec!["edge", "list", "--graph", "alpha"],
        vec!["check", "--graph", "alpha"],
        vec!["resolve", &addr],
        vec!["edge", "add", "--graph", "beta", &z, "supports", &end],
        vec!["graph", "unload", "alpha"],
    ];
    for args in walk {
        let env = home.ok(&args);
        let hint = issue(&env, Code::AliasRedirected);
        assert!(
            hint.message().contains("gamma"),
            "{args:?}: {}",
            env.to_json()
        );
    }
    assert!(home.ok(&["graph", "list"]).to_json().contains("\"beta\""));
}

/// ws006 S23 round 2 (review R2-R5): the graph stays loaded through a move, so its copy order and primary mark stand,
/// a write after it lands and a restart reloads it; a move refused because the folder exists changes nothing,
/// graph.json's former paths included, so check still names the pointers into the folder it left.
#[test]
fn a_move_keeps_the_load_and_a_refused_move_keeps_the_history() {
    let mut home = Home::new("s23-keep");
    let repo = home.git_repo();
    let (a, b, c) = (
        in_repo(&repo, "a"),
        in_repo(&repo, "b"),
        in_repo(&repo, "c"),
    );
    home.ok(&["graph", "new", &a, "--alias", "alpha"]);
    let n = home.add("problem", "Cited");
    home.ok(&["node", "evidence", &n, "add", "root:repo/a/notes.md#L2"]);
    fs::create_dir_all(&c).unwrap();
    for f in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
        fs::copy(Path::new(&a).join(f), Path::new(&c).join(f)).unwrap();
    }
    home.ok(&["graph", "load", &c, "--as", "alpha_copy"]);
    let primary = |home: &Home| home.ok(&["graph", "primary", "alpha"]).data["unchanged"].clone();
    assert_eq!(primary(&home), true);
    home.ok(&["graph", "move", "alpha", &b]);
    assert_eq!(primary(&home), true, "the move kept the copy order");
    home.ok(&[
        "node", "add", "--graph", "alpha", "--type", "problem", "--title", "After",
    ]);
    fs::create_dir_all(&a).unwrap();
    let (env, exit) = home.umx(&["graph", "move", "alpha", &a]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::GraphMoveRefused);
    assert_eq!(s23_meta(&b)["former_paths"], json!(["a"]));
    let checked = home.ok(&["check", "--graph", "alpha"]);
    assert_eq!(
        checked.data["rows"][0]["subject"],
        json!(format!("{n} evidence:1")),
        "{}",
        checked.to_json()
    );
    assert_eq!(primary(&home), true);
    home.stop();
    home.start();
    assert_eq!(
        home.ok(&["node", "get", "--graph", "alpha", &n]).data["node"]["title"],
        "Cited"
    );
    assert_eq!(primary(&home), true);
}

/// ws006 S23 round 2 (review R1): a folder graph.json cannot record as a former path (`a:b`, `~archive`) is refused
/// before anything is written: graph.json is byte for byte as it was and the graph stays loaded where it is.
#[test]
fn a_folder_graph_json_cannot_record_is_refused_before_any_write() {
    let mut home = Home::new("s23-odd");
    let repo = home.git_repo();
    for (i, name) in ["a:b", "~archive"].into_iter().enumerate() {
        let from = in_repo(&repo, name);
        let alias = format!("odd{i}");
        home.ok(&["graph", "new", &from, "--alias", &alias]);
        let before = fs::read(Path::new(&from).join("graph.json")).unwrap();
        let to = in_repo(&repo, &format!("moved{i}"));
        let (env, exit) = home.umx(&["graph", "move", &alias, &to]);
        assert_eq!(exit, 1, "{}", env.to_json());
        assert!(
            issue(&env, Code::GraphMoveRefused)
                .message()
                .contains("cannot record"),
            "{}",
            env.to_json()
        );
        assert_eq!(
            fs::read(Path::new(&from).join("graph.json")).unwrap(),
            before
        );
        assert!(!Path::new(&to).exists());
        home.ok(&["node", "list", "--graph", &alias]);
    }
}

/// ws006 S23 round 2 (review R7): a graph loaded `--as` another alias keeps both old names when renamed: graph.json's
/// own (which pointers elsewhere use) and the one it was loaded as.
#[test]
fn renaming_a_graph_loaded_as_another_alias_keeps_both_old_names() {
    let mut home = Home::new("s23-as");
    home.start();
    let a = home.path.join("a").display().to_string();
    home.ok(&["graph", "new", &a, "--alias", "alpha"]);
    let x = home.add("problem", "Kept");
    home.ok(&["graph", "unload", "alpha"]);
    home.ok(&["graph", "load", &a, "--as", "local"]);
    home.ok(&["graph", "rename", "local", "gamma"]);
    assert_eq!(s23_meta(&a)["former_aliases"], json!(["alpha", "local"]));
    for old in ["alpha", "local"] {
        let env = home.ok(&["resolve", &format!("umx:{old}/{x}")]);
        assert_eq!(env.data["status"], "redirected", "{}", env.to_json());
    }
}

/// ws006 S23 round 2 (review R10-R13, R17-R19): the readers that compare graph names read a former alias as its
/// graph: traversal by address with or without `--graph`, `--within`, a mixed old and live address pair, a secured
/// graph's own edges named by its former alias, and `graph load <former>`. One hint per redirect, and its next is
/// the request as given with the live alias (the rest of a `--graph` list and the author graph kept).
#[test]
fn readers_compare_a_former_alias_as_its_graph() {
    let mut home = Home::new("s23-readers");
    let (_, x, y) = s23_pair(&mut home);
    let x2 = home
        .ok(&[
            "node", "add", "--graph", "alpha", "--type", "problem", "--title", "Second",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    home.ok(&["edge", "add", "--graph", "alpha", &x2, "supports", &x]);
    home.ok(&["graph", "rename", "alpha", "gamma"]);
    let addr = format!("umx:alpha/{x}");
    let hinted = |env: &Envelope, next: &str| {
        let hints = hints_of(env, Code::AliasRedirected);
        assert_eq!(hints.len(), 1, "{}", env.to_json());
        assert_eq!(hints[0].next(), next, "{}", env.to_json());
    };
    for args in [
        vec!["neighbours", &addr[..]],
        vec!["neighbours", &addr, "--graph", "gamma"],
        vec!["walk", &addr],
    ] {
        home.ok(&args);
    }
    let within = home.ok(&["node", "list", "--graph", "alpha,gamma", "--within", &addr]);
    hinted(&within, "umx node list --within <node> --graph gamma");
    let mixed = home.ok(&[
        "edge",
        "list",
        "--from",
        &format!("umx:alpha/{x2}"),
        "--to",
        &format!("umx:gamma/{x}"),
    ]);
    assert_eq!(mixed.data["total"], 1, "{}", mixed.to_json());
    let listed = home.ok(&["node", "list", "--graph", "alpha,beta"]);
    hinted(&listed, "umx node list --graph gamma,beta");
    let z = home
        .ok(&[
            "node", "add", "--graph", "beta", "--type", "problem", "--title", "Third",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let added = home.ok(&[
        "edge",
        "add",
        "--graph",
        "beta",
        &z,
        "supports",
        &addr,
        "--evidence",
        &format!("umx:alpha/{x}#quote=Target"),
    ]);
    assert!(
        hints_of(&added, Code::AliasRedirected)[0]
            .next()
            .ends_with("--graph beta"),
        "{}",
        added.to_json()
    );
    assert_eq!(hints_of(&added, Code::AliasRedirected).len(), 1);
    // A batch on the graph takes its former alias in a node reference and a local edge end (review R11).
    let ops = home.ops_file(&format!(
        "{}\n{}\n",
        json!({"op":"node add","ref":"c","type":"problem","title":"Child","parent":addr}),
        json!({"op":"edge add","from":addr,"relation":"supports","to_ref":"c"}),
    ));
    home.ok(&["batch", "--file", &ops, "--graph", "gamma"]);
    // A secured graph named by its former alias reads its own edges as by its live alias, and nothing more.
    home.ok(&["graph", "set", "gamma", "--policy", "secured"]);
    let live = home.ok(&["edge", "list", "--graph", "gamma"]);
    let former = home.ok(&["edge", "list", "--graph", "alpha"]);
    assert_eq!(former.data["rows"], live.data["rows"]);
    assert_eq!(live.data["total"], 3, "{}", live.to_json());
    let _ = y;
    let loaded = home.ok(&["graph", "load", "alpha"]);
    issue(&loaded, Code::AliasRedirected);
    assert_eq!(
        home.ok(&["graph", "list"]).data["rows"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

/// ws006 S23 round 2 (review R15, R16): a former alias two graphs had refuses where it is the selector used, and only
/// there: an explicit `--graph` overrides it as UMX_GRAPH, an edge end through it is `former_alias_ambiguous` (never
/// graph_not_loaded), and an evidence pointer through it says so rather than reading as unchecked.
#[test]
fn a_former_alias_two_graphs_had_refuses_only_where_it_is_used() {
    let mut home = Home::new("s23-shared2");
    home.start();
    let (p, q, r) = (
        home.path.join("p").display().to_string(),
        home.path.join("q").display().to_string(),
        home.path.join("r").display().to_string(),
    );
    home.ok(&["graph", "new", &p, "--alias", "old"]);
    let x = home.add("problem", "In one");
    home.ok(&["graph", "rename", "old", "one"]);
    home.ok(&["graph", "new", &q, "--alias", "old"]);
    home.ok(&["graph", "rename", "old", "two"]);
    home.ok(&["graph", "new", &r, "--alias", "author"]);
    let (env, exit) = home.umx_env(&["node", "list", "--graph", "two"], &[("UMX_GRAPH", "old")]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert!(
        hints_of(&env, Code::AliasRedirected).is_empty(),
        "{}",
        env.to_json()
    );
    let z = home
        .ok(&[
            "node", "add", "--graph", "author", "--type", "problem", "--title", "Here",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (env, exit) = home.umx(&[
        "edge",
        "add",
        "--graph",
        "author",
        &z,
        "supports",
        &format!("umx:old/{x}"),
    ]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::FormerAliasAmbiguous);
    let noted = home.ok(&[
        "node",
        "evidence",
        "--graph",
        "author",
        &z,
        "add",
        &format!("umx:old/{x}#quote=In"),
    ]);
    assert!(
        issue(&noted, Code::DanglingEvidence)
            .message()
            .contains("once had this alias"),
        "{}",
        noted.to_json()
    );
    assert!(issues(&noted, Code::EvidenceUnchecked).is_empty());
}

/// ws006 S23 round 2 (review R20, R21): a hand move is judged by every folder this machine knew the graph at (one that
/// still holds it makes the load a copy), and a row at the new folder for another graph does not stop the heal.
#[test]
fn a_hand_move_is_judged_by_every_copy_and_the_destinations_own_graph() {
    let mut home = Home::new("s23-heal2");
    let repo = home.git_repo();
    let at = |n: &str| in_repo(&repo, n);
    let copy = |from: &str, to: &str| {
        fs::create_dir_all(to).unwrap();
        for f in ["graph.json", "nodes.jsonl", "edges.jsonl", ".gitignore"] {
            let _ = fs::copy(Path::new(from).join(f), Path::new(to).join(f));
        }
    };
    home.ok(&["graph", "new", &at("a"), "--alias", "alpha"]);
    copy(&at("a"), &at("b"));
    home.ok(&["graph", "load", &at("b"), "--as", "beta"]);
    home.ok(&["graph", "unload", "alpha"]);
    home.ok(&["graph", "unload", "beta"]);
    fs::remove_dir_all(at("a")).unwrap();
    copy(&at("b"), &at("c"));
    let loaded = home.ok(&["graph", "load", &at("c"), "--as", "gamma"]);
    assert!(
        loaded.data.get("moved_from").is_none_or(Value::is_null),
        "b still holds the graph: {}",
        loaded.to_json()
    );
    home.ok(&["graph", "new", &at("d"), "--alias", "delta"]);
    home.ok(&["graph", "unload", "delta"]);
    fs::remove_dir_all(at("d")).unwrap();
    home.ok(&["graph", "new", &at("e"), "--alias", "eps"]);
    let n = home
        .ok(&[
            "node", "add", "--graph", "eps", "--type", "problem", "--title", "Cited",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    home.ok(&[
        "node",
        "evidence",
        "--graph",
        "eps",
        &n,
        "add",
        "root:repo/e/x.md#L1",
    ]);
    home.ok(&["graph", "unload", "eps"]);
    fs::rename(at("e"), at("d")).unwrap();
    let healed = home.ok(&["graph", "load", &at("d")]);
    assert!(
        healed.data["moved_from"]
            .as_str()
            .is_some_and(|f| f.ends_with("/repo/e")),
        "{}",
        healed.to_json()
    );
    assert_eq!(healed.data["stale_total"], 1, "{}", healed.to_json());
    let all = home.ok(&["graph", "list", "--all"]);
    assert!(!all.to_json().contains("\"delta\""), "{}", all.to_json());
}

/// ws006 S23 round 2 (review R22, R23): a pointer is matched in the locator normal form (`./`, `//`) and listed once
/// however many of the folders the graph left it is in.
#[test]
fn stale_pointers_are_matched_in_normal_form_and_listed_once() {
    let mut home = Home::new("s23-normal");
    let repo = home.git_repo();
    let at = |n: &str| in_repo(&repo, n);
    home.ok(&["graph", "new", &at("a"), "--alias", "alpha"]);
    let n = home.add("problem", "Cited");
    home.ok(&["node", "evidence", &n, "add", "root:repo/./a//notes.md#L1"]);
    home.ok(&["node", "evidence", &n, "add", "root:repo/a/sub/s.md#L1"]);
    home.ok(&["graph", "move", "alpha", &at("b")]);
    fs::create_dir_all(at("a")).unwrap();
    home.ok(&["graph", "move", "alpha", &at("a/sub")]);
    home.ok(&["graph", "move", "alpha", &at("c")]);
    assert_eq!(
        s23_meta(&at("c"))["former_paths"],
        json!(["a", "b", "a/sub"])
    );
    let checked = home.ok(&["check", "--graph", "alpha"]);
    let subjects: Vec<&Value> = checked.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "evidence_root_stale")
        .map(|r| &r["subject"])
        .collect();
    assert_eq!(
        subjects,
        [
            &json!(format!("{n} evidence:1")),
            &json!(format!("{n} evidence:2"))
        ],
        "{}",
        checked.to_json()
    );
}

/// D83, security A5: confirm-policy by a former alias confirms the graph that name is now, and the warning names its
/// live alias.
#[test]
fn confirming_a_downgrade_by_a_former_alias_names_the_live_alias() {
    let mut home = Home::new("s23-confirm");
    let path = home.secured_graph();
    home.ok(&["graph", "rename", "demo", "gamma"]);
    home.umx(&["graph", "unload", "gamma"]);
    edit_graph_json(&path, |v| v["embedding"]["policy"] = json!("open"));
    home.umx(&["graph", "load", &path]);
    let env = home.ok(&["graph", "confirm-policy", "demo"]);
    assert_eq!(env.data["policy"], "open", "{}", env.to_json());
    issue(&env, Code::AliasRedirected);
    let found = issue(&env, Code::PolicyLowered);
    assert!(
        found.message().starts_with("Graph gamma's"),
        "{}",
        found.message()
    );
}

#[test]
fn a_named_graphs_text_shows_only_when_it_is_named() {
    // PRD 002 search-07 with search-17: an unnamed search leaves a graph read only when named out entirely; naming
    // it, or --all --include-named (with secured_graphs_read), shows its hits with their text.
    let mut home = Home::new("search-text-named");
    home.graph();
    let other = home.path.join("vault").display().to_string();
    home.ok(&["graph", "new", &other, "--alias", "vault"]);
    home.ok(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Vault entry",
        "--note",
        "Escrow terms for the vendor",
        "--graph",
        "vault",
    ]);
    home.ok(&["graph", "set", "vault", "--visibility", "named"]);

    let env = home.ok(&["search", "escrow"]);
    assert!(
        env.to_json().contains("secured_graphs_skipped"),
        "{}",
        env.to_json()
    );
    assert!(!env.to_json().contains("Escrow terms"), "{}", env.to_json());

    let env = home.ok(&["search", "escrow", "--graph", "vault"]);
    assert_eq!(
        hit(&env, "Vault entry")["text"],
        "Escrow terms for the vendor"
    );

    let env = home.ok(&["search", "escrow", "--all", "--include-named"]);
    assert_eq!(
        hit(&env, "Vault entry")["text"],
        "Escrow terms for the vendor"
    );
    assert!(
        env.to_json().contains("secured_graphs_read"),
        "{}",
        env.to_json()
    );
}

#[test]
fn secured_graphs_left_out_are_a_hint_in_every_verb_that_counts_them() {
    // PRD 002 search-13 (d), ruled for every verb that raises it: the count and the runnable next are exactly as
    // before, no secured alias or text is shown, and only the status word changes (warning to ok). Unnamed search and
    // query; and the reads of a saved scope one of whose graphs was secured after it was saved.
    let mut home = Home::new("secured-hint");
    home.graph();
    home.ok(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Ledger hidden",
        "--graph",
        "demo",
    ]);
    let open = home.path.join("open").display().to_string();
    home.ok(&["graph", "new", &open, "--alias", "open"]);
    let id = |env: Envelope| env.data["node"]["id"].as_str().unwrap().to_owned();
    let a = id(home.ok(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Ledger open",
        "--graph",
        "open",
    ]));
    let b = id(home.ok(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Ledger other",
        "--graph",
        "open",
    ]));
    home.ok(&["edge", "add", &a, "supports", &b, "--graph", "open"]);
    home.ok(&["graph", "set", "demo", "--policy", "secured"]);
    // Unnamed search and query here; node list over a scope with a secured copy, search and query with --all, and
    // similarity pairs are checked as hints in their own tests (hint_of; s31_tests for pairs).
    let verbs: [&[&str]; 2] = [
        &["search", "ledger"],
        &["query", "MATCH (n) RETURN n.title AS title"],
    ];
    for args in verbs {
        let (env, exit) = home.umx(args);
        assert_eq!(exit, 0, "{args:?}: {}", env.to_json());
        let hint = hint_of(&env, Code::SecuredGraphsSkipped);
        assert!(
            hint.message().starts_with("1 secured graphs"),
            "{args:?}: {}",
            hint.message()
        );
        assert!(hint.next().starts_with("umx "), "{args:?}: {}", hint.next());
        assert!(
            !env.to_json().contains("Ledger hidden"),
            "{args:?}: {}",
            env.to_json()
        );
        assert!(
            !env.warnings
                .iter()
                .any(|w| w.code() == "secured_graphs_skipped"),
            "{args:?}: {}",
            env.to_json()
        );
    }
}

#[test]
fn a_repeated_graph_or_scope_is_refused_never_read_as_its_last() {
    // F256: `--graph alpha --graph beta` kept only beta, so node find answered ok with no rows for a title in alpha.
    let mut home = Home::new("graph-twice");
    home.start();
    for alias in ["alpha", "beta"] {
        let dir = home.path.join(alias).display().to_string();
        home.ok(&["graph", "new", &dir, "--alias", alias]);
    }
    let added = home.ok(&[
        "node",
        "add",
        "--graph",
        "alpha",
        "--type",
        "problem",
        "--title",
        "Latency budget",
    ]);
    let id = added.data["node"]["id"].as_str().unwrap().to_owned();
    home.ok(&[
        "node",
        "add",
        "--graph",
        "beta",
        "--type",
        "problem",
        "--title",
        "Latency spikes",
    ]);
    let alone = home.ok(&["node", "find", "budget", "--graph", "alpha"]);
    assert_eq!(alone.data["total"], json!(1), "{}", alone.to_json());

    let list = "name every graph in one --graph a,b";
    let one = "use --graph with one name";
    let refused: [(&[&str], &str); 9] = [
        (
            &[
                "node", "find", "budget", "--graph", "alpha", "--graph", "beta",
            ],
            one,
        ),
        (
            &["node", "find", "budget", "--graph=alpha", "--graph=beta"],
            one,
        ),
        (&["node", "find", "budget", "--graph", "alpha,beta"], one),
        (
            &["node", "get", &id, "--graph", "alpha", "--graph", "beta"],
            one,
        ),
        (
            &["node", "list", "--graph", "alpha", "--graph", "beta"],
            list,
        ),
        (
            &["edge", "list", "--graph", "alpha", "--graph", "beta"],
            list,
        ),
        (
            &["neighbours", &id, "--graph", "alpha", "--graph", "beta"],
            list,
        ),
        (
            &[
                "scope", "save", "both", "--graph", "alpha", "--graph", "beta",
            ],
            list,
        ),
        (
            &["node", "list", "--scope", "both", "--scope", "both"],
            "--scope",
        ),
    ];
    for (args, says) in refused {
        let (env, exit) = home.umx(args);
        assert_eq!(
            (exit, env.status),
            (2, Status::Error),
            "{args:?}: {}",
            env.to_json()
        );
        let message = issue(&env, Code::UsageInvalid).message();
        assert!(message.contains(says), "{args:?}: {message}");
    }

    // The refused scope save saved nothing; the list forms read both graphs.
    home.ok(&["scope", "save", "both", "--graph", "alpha,beta"]);
    for args in [
        &["node", "list", "--graph", "alpha,beta"][..],
        &["node", "list", "--scope", "both"],
    ] {
        let env = home.ok(args);
        let graphs: Vec<&str> = env.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["graph"].as_str().unwrap())
            .collect();
        assert_eq!(graphs, ["alpha", "beta"], "{args:?}");
    }
}

#[test]
fn umx_s_reserved_names_are_refused_in_every_graph_with_a_next() {
    // ws008 T2b (req-99, the dogfood report): `std` as a type, relation or field name, the provenance types and the
    // provenance and derived relations, in a std graph and in one without std; each refusal writes nothing.
    let mut home = Home::new("t2b-reserved");
    let plain = std_and_plain(&mut home);
    let demo = home.path.join("g").display().to_string();
    for (alias, path) in [("demo", &demo), ("plain", &plain)] {
        let before = fs::read_to_string(Path::new(path).join("graph.json")).unwrap();
        let g = ["--graph", alias];
        let with = |args: &[&'static str]| -> Vec<&str> { args.iter().copied().chain(g).collect() };
        for args in [
            with(&["ontology", "add", "type", "std"]),
            with(&["ontology", "add", "relation", "std"]),
            with(&["ontology", "add", "field", "problem", "std:string"]),
            with(&["ontology", "add", "field", "std", "note_text:string"]),
            with(&[
                "ontology",
                "add",
                "field",
                "--relation",
                "blocks",
                "std:string",
            ]),
            with(&["node", "add", "--type", "std", "--title", "x"]),
            with(&[
                "node", "add", "--type", "problem", "--title", "x", "--set", "std=1",
            ]),
            with(&[
                "node",
                "add",
                "--type",
                "problem",
                "--title",
                "x",
                "--set",
                "prov_key=x",
            ]),
        ] {
            refused_with(&home, &args, Code::ReservedName);
        }
        for relation in ["authored_by", "edited_by"] {
            refused_with(
                &home,
                &with(&["ontology", "add", "relation", relation]),
                Code::DerivedRelation,
            );
        }
        for relation in ["session_of", "runs_on", "uses_model", "registered_in"] {
            refused_with(
                &home,
                &with(&["ontology", "add", "relation", relation]),
                Code::ReservedRelation,
            );
        }
        refused_with(
            &home,
            &with(&["ontology", "add", "relation", "similar_to"]),
            Code::NameInvalid,
        );
        assert_eq!(
            fs::read_to_string(Path::new(path).join("graph.json")).unwrap(),
            before,
            "{alias}: a refusal writes nothing"
        );
        let (a, b) = (
            home.ok(&with(&["node", "add", "--type", "problem", "--title", "A"])),
            home.ok(&with(&["node", "add", "--type", "problem", "--title", "B"])),
        );
        let (a, b) = (
            a.data["node"]["address"].as_str().unwrap().to_owned(),
            b.data["node"]["address"].as_str().unwrap().to_owned(),
        );
        refused_with(
            &home,
            &["edge", "add", &a, "authored_by", &b],
            Code::DerivedRelation,
        );
        refused_with(&home, &["edge", "add", &a, "std", &b], Code::ReservedName);
        refused_with(
            &home,
            &["edge", "add", &a, "blocks", &b, "--set", "std=1"],
            Code::ReservedName,
        );
    }
    // A provenance type is umx's in a std graph; a graph without std keeps it as its own word.
    for kind in ["session", "agent", "model"] {
        refused_with(
            &home,
            &["ontology", "add", "type", kind, "--graph", "demo"],
            Code::ReservedName,
        );
        let own = home.ok(&["ontology", "add", "type", kind, "--graph", "plain"]);
        assert_eq!(own.data["unchanged"], json!(false), "{kind}");
    }
    // The same in a batch, line by line: one code path.
    for line in [
        json!({"op":"node add","type":"std","title":"x"}),
        json!({"op":"node add","type":"problem","title":"x","sets":[{"field":"std","value":"1"}]}),
    ] {
        let (env, exit) = batch_of(&home, "demo", &[line]);
        assert_eq!(exit, 1, "{}", env.to_json());
        issue(&env, Code::ReservedName);
    }
    let a = home.ok(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Batch end",
        "--graph",
        "demo",
    ]);
    let a = a.data["node"]["id"].as_str().unwrap().to_owned();
    for (relation, code) in [
        ("authored_by", Code::DerivedRelation),
        ("session_of", Code::ReservedRelation),
        ("std", Code::ReservedName),
    ] {
        let (env, exit) = batch_of(
            &home,
            "demo",
            &[json!({"op":"edge add","from":a,"relation":relation,"to":a})],
        );
        assert_eq!(exit, 1, "{relation}: {}", env.to_json());
        issue(&env, code);
    }
}

#[test]
fn an_edge_into_a_secured_graph_never_names_the_far_node_s_type() {
    // ws008 T2b (security N3): edge add's twin of the batch case.
    let mut home = Home::new("t2b-secured-edge");
    home.graph();
    let ada = home.add("person", "Ada");
    let vault = home.path.join("vault").display().to_string();
    home.ok(&["graph", "new", &vault, "--alias", "vault"]);
    let team = home.ok(&[
        "node",
        "add",
        "--type",
        "team",
        "--title",
        "Hidden team",
        "--graph",
        "vault",
    ]);
    let team = team.data["node"]["id"].as_str().unwrap().to_owned();
    home.ok(&["graph", "set", "vault", "--policy", "secured"]);
    let far = format!("umx:vault/{team}");
    let env = home.ok(&["edge", "add", &ada, "works_for", &far, "--graph", "demo"]);
    let codes: Vec<&str> = env.warnings.iter().map(|w| w.code()).collect();
    assert_eq!(codes, ["link_into_secured"], "{}", env.to_json());
    assert!(!env.to_json().contains("team"), "{}", env.to_json());
}
