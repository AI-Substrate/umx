//! The cli tests: evidence, redaction and check --lint.
//! A module of cli.rs, whose `Home` and helpers it uses, in its own file so no file nears the seed gate's
//! size cap.

use super::*;

#[test]
fn a_redact_scrubs_every_pointer_with_the_source_from_every_file_and_log() {
    // F183 (security's ruling). Synthetic needles: the source and a quote selector.
    const SOURCE: &str = "leak/tok-FAKE7Q2Z9X.md";
    const QUOTE: &str = "FAKEQUOTE77";
    let mut home = Home::new("redact");
    let path = logged_graph(&mut home);
    let (a, b, c) = (
        home.add("problem", "Alpha"),
        home.add("problem", "Bravo"),
        home.add("decision", "Charlie"),
    );
    let (quoted, line) = (format!("{SOURCE}#quote={QUOTE}"), format!("{SOURCE}#L3"));
    home.ok(&["node", "evidence", &a, "add", &quoted]);
    home.ok(&["node", "evidence", &a, "add", &line]);
    home.ok(&["node", "evidence", &a, "add", "docs/ok.md#L1"]);
    home.ok(&["node", "evidence", &b, "add", &line]);
    home.ok(&["node", "evidence", &b, "remove", &line, "--reason", "stale"]);
    home.ok(&["node", "evidence", &c, "add", "docs/ok.md#L2"]);
    let edge = format!("{SOURCE}#L2");
    home.ok(&["edge", "add", &a, "blocks", &c, "--evidence", &edge]);
    // Positive control: the needles are in the graph's files before the redact, and the walk reaches daemon.log.
    assert!(home.path.join("daemon.log").is_file());
    let before = files_holding(&home.path, &[SOURCE, QUOTE]);
    assert!(
        before.iter().any(|f| f.ends_with("nodes.jsonl"))
            && before.iter().any(|f| f.ends_with("edges.jsonl")),
        "{before:?}"
    );

    let (env, exit) = home.umx(&[
        "node",
        "evidence",
        &a,
        "remove",
        &line,
        "--redact",
        "--reason",
        "it held a token",
    ]);
    assert_eq!(exit, 0, "{}", env.to_json());
    // A's two live pointers, B's trail entry; one edge.
    assert_eq!(
        env.data["redacted"],
        json!({ "pointers": 3, "nodes": 2, "edges": 1 })
    );
    assert_eq!(env.data["removed"], Value::Null);
    assert_eq!(
        issue(&env, Code::RedactedInFilesOnly).next(),
        "umx docs get evidence"
    );
    let reply = env.to_json();
    assert!(!reply.contains(SOURCE) && !reply.contains(QUOTE), "{reply}");
    let needles = [SOURCE, QUOTE, "tok-FAKE7Q2Z9X"];
    assert_eq!(files_holding(&home.path, &needles), Vec::<String>::new());

    // A redacted entry keeps kind, when, who and the new reason; no source, selector or hash.
    let trail = home.ok(&["node", "evidence", &a, "list", "--removed"]);
    let rows = trail.data["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row["pointer"], "(redacted)");
        let e = &row["evidence"];
        assert_eq!(
            (&e["redacted"], &e["reason"], &e["kind"]),
            (&json!(true), &json!("it held a token"), &json!("document")),
            "{e}"
        );
        assert!(
            e.get("removed").is_none()
                && e["removed_at"].is_string()
                && e["by_session"].is_string(),
            "{e}"
        );
    }
    let b_trail = home.ok(&["node", "evidence", &b, "list", "--removed"]);
    assert_eq!(b_trail.data["rows"][0]["pointer"], "(redacted)");
    assert_eq!(
        b_trail.data["rows"][0]["evidence"]["reason"],
        "it held a token"
    );
    let live = home.ok(&["node", "evidence", &a, "list"]);
    assert_eq!(live.data["rows"], json!([live.data["rows"][0]]));
    assert_eq!(live.data["rows"][0]["pointer"], "docs/ok.md#L1");
    let c_live = home.ok(&["node", "evidence", &c, "list"]);
    assert_eq!(c_live.data["rows"][0]["pointer"], "docs/ok.md#L2");
    // The files load back as written, and still hold nothing.
    home.ok(&["graph", "load", &path]);
    assert_eq!(files_holding(&home.path, &needles), Vec::<String>::new());
}

#[test]
fn a_redact_is_refused_while_the_text_sits_where_it_cannot_scrub() {
    const SOURCE: &str = "leak/tok-FAKE3K8W.md";
    const QUOTE: &str = "FAKEQUOTE88";
    let mut home = Home::new("redact-block");
    let path = home.graph();
    let a = home.add("problem", "Alpha");
    let quoted = format!("{SOURCE}#quote={QUOTE}");
    home.ok(&["node", "evidence", &a, "add", &quoted]);
    let redact = |home: &Home, reason: &str| {
        home.umx(&[
            "node", "evidence", &a, "remove", &quoted, "--redact", "--reason", reason,
        ])
    };
    let files = || {
        ["nodes.jsonl", "edges.jsonl", "graph.json"]
            .map(|f| fs::read(Path::new(&path).join(f)).unwrap_or_default())
    };

    // The reason may neither hold the pointer (any case) nor look like a secret.
    for reason in ["fakequote88 leaked", "key AKIAQ7Z3X9K2Q7Z3X9K2"] {
        let (env, exit) = redact(&home, reason);
        assert_eq!(exit, 2, "{}", env.to_json());
        issue(&env, Code::UsageInvalid);
    }
    // --redact goes with remove only, and takes no --at: the CLI refuses before any daemon is asked.
    let offline = Home::new("redact-parse");
    for args in [
        vec![
            "node", "evidence", &a, "remove", &quoted, "--redact", "--at", "1", "--reason", "x",
        ],
        vec!["node", "evidence", &a, "add", &quoted, "--redact"],
        vec!["node", "evidence", &a, "list", "--redact"],
    ] {
        let (env, exit) = offline.umx(&args);
        assert_eq!(exit, 2, "{}", env.to_json());
        issue(&env, Code::UsageInvalid);
    }
    // The daemon checks the same, whatever a client sends.
    for (action, at) in [
        (umx_protocol::EvidenceAction::Remove, Some(1)),
        (umx_protocol::EvidenceAction::Add, None),
        (umx_protocol::EvidenceAction::List, None),
    ] {
        let op = umx_protocol::Op::NodeEvidence {
            node: a.clone(),
            action,
            pointer: Some(quoted.clone()),
            evidence_kind: None,
            reason: Some("x".into()),
            at,
            removed: false,
            redact: true,
            all: false,
        };
        let (env, exit) = raw(&home.socket(), &Request::op(op, None, None, older_cli()));
        assert_eq!(exit, 2, "{}", env.to_json());
        issue(&env, Code::UsageInvalid);
    }

    // Every place a redact never rewrites, at once.
    let title = home.add("problem", &format!("see {SOURCE}"));
    let field = home.add("problem", "Field");
    home.ok(&["node", "edit", &field, "--set-str", &format!("ref={QUOTE}")]);
    let other = home.add("decision", "Other");
    home.ok(&[
        "node",
        "evidence",
        &other,
        "add",
        &format!("docs/other.md#quote={QUOTE}"),
    ]);
    let edge = edge_id(&home.ok(&[
        "edge",
        "add",
        &title,
        "blocks",
        &other,
        "--evidence",
        &format!("docs/edge.md#quote={QUOTE}"),
    ]));
    let resource = home.ok(&[
        "node",
        "add",
        "--type",
        "document",
        "--locator",
        &format!("docs/{QUOTE}.md"),
    ]);
    let resource = resource.data["node"]["id"].as_str().unwrap().to_owned();
    let status = home.add("problem", "Status");
    home.ok(&[
        "node",
        "status",
        &status,
        "contested",
        "--reason",
        &format!("see {QUOTE}"),
    ]);
    let trail = home.add("problem", "Trail");
    let gone = format!("docs/gone.md#quote={QUOTE}");
    home.ok(&["node", "evidence", &trail, "add", &gone]);
    home.ok(&[
        "node", "evidence", &trail, "remove", &gone, "--reason", "stale",
    ]);
    home.ok(&["graph", "set", "demo", "--title", &format!("About {QUOTE}")]);
    // A repeated line from a merge: the earlier copy of A's line is kept, pointer and all.
    let nodes = Path::new(&path).join("nodes.jsonl");
    let text = fs::read_to_string(&nodes).unwrap();
    let kept = text
        .lines()
        .find(|l| umx_protocol::parse_json(l).unwrap()["id"] == json!(a))
        .unwrap()
        .replace("Alpha", "Alpha, other branch");
    fs::write(&nodes, format!("{kept}\n{text}")).unwrap();
    home.ok(&["graph", "load", &path]);

    let before = files();
    let (env, exit) = redact(&home, "it held a token");
    assert_eq!(exit, 1, "{}", env.to_json());
    let i = issue(&env, Code::EvidenceRedactBlocked);
    let places: Vec<(String, String)> = i
        .candidates()
        .iter()
        .map(|c| {
            (
                c["id"].as_str().unwrap().to_string(),
                c["title"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    for (id, place) in [
        (title.as_str(), "title"),
        (other.as_str(), "evidence:1"),
        (edge.as_str(), "evidence:1"),
        ("graph.json", "the graph's metadata"),
        (resource.as_str(), "locator"),
        (trail.as_str(), "evidence_removed:1"),
    ] {
        assert!(
            places.iter().any(|(i, p)| i == id && p == place),
            "{id} {place}: {places:?}"
        );
    }
    // A custom field is placed by its ordinal, never named (review R3).
    assert!(
        places
            .iter()
            .any(|(i, p)| i == &field && p.starts_with("field ")),
        "the custom field: {places:?}"
    );
    assert!(!places.iter().any(|(_, p)| p.contains("ref")), "{places:?}");
    assert!(
        places
            .iter()
            .any(|(i, p)| i == &status && p.starts_with("status_history:")),
        "the status reason: {places:?}"
    );
    assert!(
        places.iter().any(|(i, _)| i == "nodes.jsonl"),
        "the kept line: {places:?}"
    );
    let reply = env.to_json();
    assert!(!reply.contains(SOURCE) && !reply.contains(QUOTE), "{reply}");
    assert_eq!(files(), before, "nothing changed");
}

#[test]
fn evidence_kinds_are_computed_from_the_evidence() {
    // E-6: the column used to be faked as [] by the daemon.
    let mut home = Home::new("kinds");
    home.graph();
    let id = home.add("problem", "x");
    for kind in ["room", "analyst", "room"] {
        let (env, exit) = home.umx(&[
            "node",
            "note",
            &id,
            "--append",
            "seen",
            "--evidence",
            &format!("notes.md#L{}", kind.len()),
            "--evidence-kind",
            kind,
        ]);
        assert_eq!(exit, 0, "{}", env.to_json());
    }
    let (env, _) = home.umx(&["node", "get", &id, "--columns", "id,evidence_kinds"]);
    assert_eq!(
        env.data["node"]["evidence_kinds"],
        json!(["analyst", "room"])
    );
    let (env, _) = home.umx(&["node", "list", "--columns", "evidence_kinds"]);
    assert_eq!(
        env.data["rows"][0]["evidence_kinds"],
        json!(["analyst", "room"])
    );
}

#[test]
fn s10_batch_note_evidence_and_status_work_through_the_binary() {
    let mut home = Home::new("s10-note-status");
    let graph = home.graph();
    let existing = home.add("problem", "Existing");
    let source = [
        json!({"op":"node add","ref":"p","type":"problem","title":"Review"}),
        json!({"op":"node note","node_ref":"p","append":"First paragraph","evidence":[
            {"pointer":"notes.md#L1-L2"},
            {"pointer":"analysis.md#quote=An inference","kind":"analyst"}
        ]}),
        json!({"op":"node status","node_ref":"p","status":"agreed","reason":"Reviewed"}),
        json!({"op":"node edit","node":existing,"note":"Replacement"}),
        json!({"op":"node note","node":format!("umx:demo/{existing}"),"append":"Append"}),
    ]
    .into_iter()
    .map(|line| line.to_string())
    .collect::<Vec<_>>()
    .join("\n");
    let file = home.ops_file(&source);
    let before = fs::read(Path::new(&graph).join("nodes.jsonl")).unwrap();
    let (dry, exit) = home.umx(&["batch", "--file", &file, "--dry-run", "--by", "tester"]);
    assert_eq!(exit, 0, "{}", dry.to_json());
    assert!(dry.data["refs"]["p"].is_null());
    assert_eq!(
        fs::read(Path::new(&graph).join("nodes.jsonl")).unwrap(),
        before
    );
    let (done, exit) = home.umx(&["batch", "--file", &file, "--by", "tester"]);
    assert_eq!(exit, 0, "{}", done.to_json());
    assert_eq!(done.data["rows"][1]["op"], "node note");
    assert_eq!(done.data["rows"][2]["op"], "node status");
    let id = done.data["refs"]["p"].as_str().unwrap();
    let (got, exit) = home.umx(&["node", "get", id]);
    assert_eq!(exit, 0, "{}", got.to_json());
    assert_eq!(got.data["node"]["note"], "First paragraph");
    assert_eq!(got.data["node"]["evidence"][0]["kind"], "document");
    assert_eq!(got.data["node"]["evidence"][1]["kind"], "analyst");
    assert_eq!(got.data["node"]["status"], "agreed");
    assert_eq!(got.data["node"]["status_history"][0]["by"], "tester");
    let (got, exit) = home.umx(&["node", "get", &existing]);
    assert_eq!(exit, 0, "{}", got.to_json());
    assert_eq!(got.data["node"]["note"], "Replacement\n\nAppend");
}

/// ws006 S5b (req-124): an edge carries evidence as a node does. edge add stores it, edge list counts it, edge show
/// gives it in full, an existing edge gets new pointers appended (evidence_appended) and a repeat adds nothing; umx:
/// sources get S0's checks (secured reads as unloaded), and a batch edge add does the same.
#[test]
fn edges_carry_evidence_appended_never_dropped_and_checked_as_nodes() {
    let mut home = Home::new("edge-evidence");
    home.start();
    resolve_fixture(&mut home, "rsv");
    resolve_fixture(&mut home, "sec_b");
    home.ok(&["graph", "set", "sec_b", "--policy", "secured"]);
    const B1: &str = "01RSBBBB000000000000000001";
    const B2: &str = "01RSBBBB000000000000000002";
    let add = |extra: &[&str]| {
        let mut args = vec!["edge", "add", "--graph", "rsv", B2, "feeds", A1];
        args.extend_from_slice(extra);
        home.umx(&args)
    };
    let (env, exit) = add(&[
        "--evidence",
        "notes.md#L10-L20",
        "--evidence-kind",
        "document",
    ]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let id = env.data["edge"]["id"].as_str().unwrap().to_owned();
    assert_eq!(env.data["edge"]["evidence"][0]["source"], "notes.md");
    assert_eq!(env.data["edge"]["evidence"][0]["kind"], "document");

    let listed = home.ok(&["edge", "list", "--graph", "rsv", "--from", B2]);
    let row = &listed.data["rows"][0];
    assert_eq!(
        (
            row["evidence_count"].as_u64(),
            row["evidence_kinds"].clone()
        ),
        (Some(1), json!(["document"]))
    );
    assert!(
        listed.data["columns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "evidence_count")
    );
    // An edge without evidence lists zero.
    let plain = home.ok(&["edge", "list", "--graph", "rsv", "--from", B1]);
    assert_eq!(plain.data["rows"][0]["evidence_count"], 0);

    // The same edge with a new pointer: appended, nothing else changed. The same pointer again: edge_exists.
    let (env, exit) = add(&[
        "--evidence",
        "transcripts/016.ndjson#seq=1-5",
        "--evidence-kind",
        "chat",
    ]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(
        issue(&env, Code::EvidenceAppended).next(),
        format!("umx edge show {id}")
    );
    assert!(!has(&env, Code::EdgeExists), "{}", env.to_json());
    let (env, _) = add(&[
        "--evidence",
        "notes.md#L10-L20",
        "--evidence-kind",
        "document",
    ]);
    issue(&env, Code::EdgeExists);
    assert!(!has(&env, Code::EvidenceAppended));

    let shown = home.ok(&["edge", "show", &format!("umx:rsv/{id}")]);
    assert_eq!(shown.data["id"], id.as_str());
    assert_eq!(shown.data["evidence_count"], 2);
    assert_eq!(shown.data["evidence_kinds"], json!(["chat", "document"]));
    let sources: Vec<&str> = shown.data["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["source"].as_str().unwrap())
        .collect();
    assert_eq!(sources, ["notes.md", "transcripts/016.ndjson"]);
    assert_eq!(shown.data["from_title"], "Budget ceiling");
    let (env, exit) = home.umx(&["edge", "show", "--graph", "rsv", "01RSEE00"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::EdgeNotFound);

    // S0's checks on edges: a missing node in a loaded open graph dangles; a secured graph reads as unloaded.
    let (env, exit) = add(&["--evidence", "umx:rsv/01ZZZZZZZZZZZZZZZZZZZZZZZZ#L1"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    issue(&env, Code::DanglingEvidence);
    let (env, _) = add(&["--evidence", "umx:sec_b/01rsaaaa#L1"]);
    issue(&env, Code::EvidenceUnchecked);
    assert!(!has(&env, Code::DanglingEvidence));
    // The reserved names are refused as custom fields.
    let (env, exit) = home.umx(&[
        "edge",
        "add",
        "--graph",
        "rsv",
        B1,
        "supports",
        A1,
        "--set",
        "evidence=x",
    ]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::ReservedName);

    // A batch edge add: new evidence on a new edge, then appended to an existing one.
    let ops = home.ops_file(&format!(
        "{{\"op\":\"edge add\",\"from\":\"{B1}\",\"type\":\"supports\",\"to\":\"{A1}\",\"evidence\":[\"a.md#L1\"],\"evidence_kind\":\"analyst\"}}\n\
{{\"op\":\"edge add\",\"from\":\"{B2}\",\"type\":\"feeds\",\"to\":\"{A1}\",\"evidence\":[\"b.md#L2\",\"umx:rsv/01ZZZZZZZZZZZZZZZZZZZZZZZZ#L1\"]}}\n\
{{\"op\":\"edge add\",\"from\":\"{B1}\",\"type\":\"supports\",\"to\":\"{A1}\",\"evidence\":[\"d.md#L4\"],\"evidence_kind\":\"chat\"}}\n"
    ));
    let (env, exit) = home.umx(&["batch", "--file", &ops, "--graph", "rsv"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    issue(&env, Code::EvidenceAppended);
    issue(&env, Code::DanglingEvidence);
    let shown = home.ok(&["edge", "show", &format!("umx:rsv/{id}")]);
    assert_eq!(shown.data["evidence_count"], 5);
    let new = home.ok(&[
        "edge", "list", "--graph", "rsv", "--from", B1, "--type", "supports",
    ]);
    // The third line re-added the first line's edge (a new id, placed only at prepare): appended, never a panic.
    assert_eq!(
        new.data["rows"][0]["evidence_kinds"],
        json!(["analyst", "chat"])
    );

    // B1: an older edge whose own `evidence` field is prose. It loads with evidence_field_kept; evidence given
    // to it is refused with evidence_field_invalid (exit 1), by the verb and by a batch, and no byte is written.
    // R210-2: edge show keeps showing the prose, under evidence_field_kept; `evidence` stays the (empty) list.
    let edges = home.path.join("rsv").join("edges.jsonl");
    let text = fs::read_to_string(&edges).unwrap();
    let old = "\"created_by\":\"fixture\"}";
    assert_eq!(text.matches(old).count(), 1, "{text}");
    fs::write(
        &edges,
        text.replace(
            old,
            "\"created_by\":\"fixture\",\"evidence\":\"from the call on Tuesday\"}",
        ),
    )
    .unwrap();
    home.ok(&["graph", "unload", "rsv"]);
    let path = home.path.join("rsv");
    let (env, exit) = home.umx(&["graph", "load", path.to_str().unwrap(), "--as", "rsv"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    issue(&env, Code::EvidenceFieldKept);
    let before = fs::read(&edges).unwrap();
    let (env, exit) = home.umx(&[
        "edge",
        "add",
        "--graph",
        "rsv",
        B1,
        "blocks",
        A1,
        "--evidence",
        "c.md#L3",
    ]);
    assert_eq!(exit, 1, "{}", env.to_json());
    // F134: the next moves the field with edge edit.
    assert_eq!(
        issue(&env, Code::EvidenceFieldInvalid).next(),
        "umx edge edit umx:rsv/01RSEE00000000000000000001 --set-json <field>=<value> --unset evidence"
    );
    assert!(env.data.is_null());
    let ops = home.ops_file(&format!(
        "{{\"op\":\"edge add\",\"from\":\"{B1}\",\"type\":\"blocks\",\"to\":\"{A1}\",\"evidence\":[\"c.md#L3\"]}}\n"
    ));
    for dry in [true, false] {
        let mut args = vec!["batch", "--file", &ops, "--graph", "rsv"];
        if dry {
            args.push("--dry-run");
        }
        let (env, exit) = home.umx(&args);
        assert_eq!(exit, 1, "dry run {dry}: {}", env.to_json());
        issue(&env, Code::EvidenceFieldInvalid);
        assert_eq!(fs::read(&edges).unwrap(), before, "nothing written");
    }
    let shown = home.ok(&["edge", "show", "umx:rsv/01RSEE00000000000000000001"]);
    issue(&shown, Code::EvidenceFieldKept);
    assert_eq!(
        shown.data["evidence_field_kept"],
        "from the call on Tuesday"
    );
    assert_eq!(
        (
            shown.data["evidence"].clone(),
            shown.data["evidence_count"].clone()
        ),
        (json!([]), json!(0))
    );
}

#[test]
fn resolve_virtual_graph_targets_and_evidence_sources() {
    let mut home = Home::new("resolve-virtual-graphs");
    let author = home.graph();
    resolve_fixture(&mut home, "rsv");
    let target = home.path.join("rsv");
    let before = ["graph.json", "nodes.jsonl", "edges.jsonl"]
        .map(|file| fs::read(target.join(file)).unwrap());
    let graph_id = "01RG0000000000000000000001";
    for given in [
        "umx:rsv".to_owned(),
        "UMX:rsv".to_owned(),
        format!("umx:rsv/{graph_id}"),
    ] {
        let (env, exit) = home.umx_env(&["resolve", &given], &[("UMX_GRAPH", "demo")]);
        assert_eq!(exit, 0, "{}", env.to_json());
        assert_eq!(env.data["status"], "ok");
        assert_eq!(env.data["address"], "umx:rsv");
        assert_eq!(env.data["graph_id"], graph_id);
        let node = &env.data["node"];
        assert_eq!(node["id"], graph_id);
        assert_eq!(node["type"], "graph");
        assert_eq!(node["address"], "umx:rsv");
        assert_eq!(node["alias"], "rsv");
        assert!(node["title"].is_string());
        assert_eq!(node.as_object().unwrap().len(), 5);
        for fragment in [
            "id",
            "address",
            "type",
            "title",
            "alias",
            "path",
            "status",
            "L1",
            "quote=graph",
        ] {
            let (refused, exit) = home.umx(&["resolve", &format!("{given}#{fragment}")]);
            assert_eq!(exit, 2, "{}", refused.to_json());
            let problem = issue(&refused, Code::UsageInvalid);
            assert_eq!(problem.next(), "umx docs get explore");
            assert!(
                problem
                    .message()
                    .contains("a graph address has no selector or fragment")
            );
        }
    }
    let projected = home.ok(&[
        "node",
        "get",
        "umx:rsv",
        "--columns",
        "id,address,alias,status,graph",
    ]);
    assert_eq!(projected.data["node"]["id"], graph_id);
    assert_eq!(projected.data["node"]["address"], "umx:rsv");
    assert_eq!(projected.data["node"]["alias"], "rsv");
    assert!(projected.data["node"]["status"].is_null());
    assert!(projected.data["node"]["graph"].is_null());

    let added = home.ok(&[
        "node",
        "add",
        "--graph",
        "demo",
        "--type",
        "problem",
        "--title",
        "Cites a virtual graph",
    ]);
    let id = added.data["node"]["id"].as_str().unwrap();
    let author_before = ["graph.json", "nodes.jsonl", "edges.jsonl"]
        .map(|file| fs::read(Path::new(&author).join(file)).unwrap());
    for source in [
        "umx:rsv".to_owned(),
        "UMX:rsv".to_owned(),
        format!("umx:rsv/{graph_id}"),
    ] {
        for fragment in ["title", "L1"] {
            let pointer = format!("{source}#{fragment}");
            let commands = [
                vec![
                    "node",
                    "add",
                    "--graph",
                    "demo",
                    "--type",
                    "problem",
                    "--title",
                    "Refused",
                    "--evidence",
                    &pointer,
                ],
                vec![
                    "node",
                    "note",
                    id,
                    "--graph",
                    "demo",
                    "--append",
                    "Refused",
                    "--evidence",
                    &pointer,
                ],
            ];
            for command in commands {
                let (refused, exit) = home.umx(&command);
                assert_eq!(exit, 1, "{}", refused.to_json());
                let problem = issue(&refused, Code::EvidencePointerInvalid);
                assert_eq!(problem.next(), "umx docs get evidence");
                assert!(
                    problem
                        .message()
                        .contains("a graph address has no selector or fragment")
                );
            }
            let ops = home.ops_file(&format!(
                "{{\"op\":\"node add\",\"type\":\"problem\",\"title\":\"Refused\",\"evidence\":[\"{pointer}\"]}}\n"
            ));
            let (refused, exit) = home.umx(&["batch", "--file", &ops, "--graph", "demo"]);
            assert_eq!(exit, 1, "{}", refused.to_json());
            let problem = issue(&refused, Code::EvidencePointerInvalid);
            assert_eq!(problem.next(), "umx docs get evidence");
            assert!(
                problem
                    .message()
                    .contains("a graph address has no selector or fragment")
            );
        }
    }
    let author_after = ["graph.json", "nodes.jsonl", "edges.jsonl"]
        .map(|file| fs::read(Path::new(&author).join(file)).unwrap());
    assert_eq!(
        author_before, author_after,
        "refused evidence never writes the author"
    );
    let after = ["graph.json", "nodes.jsonl", "edges.jsonl"]
        .map(|file| fs::read(target.join(file)).unwrap());
    assert_eq!(
        before, after,
        "resolve and refused citations never write the target"
    );
    let physical = home.ok(&[
        "node",
        "add",
        "--graph",
        "demo",
        "--type",
        "graph",
        "--title",
        "A stored graph concept",
        "--note",
        "Physical data",
    ]);
    let id = physical.data["node"]["id"].as_str().unwrap();
    home.ok(&["graph", "unload", "demo"]);
    let nodes = Path::new(&author).join("nodes.jsonl");
    let mut rewritten = String::new();
    let mut changed = 0;
    for line in fs::read_to_string(&nodes).unwrap().lines() {
        let mut node: Value = umx_protocol::parse_json(line).unwrap();
        if node["id"] == id {
            node["address"] = json!("umx:demo");
            node["alias"] = json!("demo");
            changed += 1;
        }
        rewritten.push_str(&format!("{node}\n"));
    }
    assert_eq!(changed, 1, "the forged marker must reach the physical row");
    fs::write(&nodes, rewritten).unwrap();
    home.ok(&["graph", "load", &author]);
    let address = format!("umx:demo/{id}");
    let stored = home.ok(&["node", "get", &address]);
    assert_eq!(stored.data["node"]["address"], address);
    assert_eq!(stored.data["node"]["graph"], "demo");
    assert_eq!(stored.data["node"]["note"], "Physical data");
    let resolved = home.ok(&["resolve", &address]);
    assert_eq!(resolved.data["address"], address);
    assert_eq!(resolved.data["node"]["address"], address);
    assert_eq!(resolved.data["node"]["note"], "Physical data");
}

/// ws006 S0: a umx: evidence source is never refused. In a loaded graph with no single node it is kept with
/// dangling_evidence (an ambiguous prefix lists its candidates); in a graph that is not loaded it is kept with
/// evidence_unchecked, and the write never loads that graph; a unique prefix is stored as the full id.
#[test]
fn umx_evidence_is_kept_and_checked_against_loaded_graphs_only() {
    let mut home = Home::new("resolve-evidence");
    home.start();
    resolve_fixture(&mut home, "rsv");
    resolve_fixture(&mut home, "sec_b");
    let add = |home: &Home, pointer: &str| {
        home.umx(&[
            "node",
            "add",
            "--graph",
            "rsv",
            "--type",
            "finding",
            "--title",
            "Cited",
            "--evidence",
            pointer,
        ])
    };
    let source = |env: &Envelope| {
        env.data["node"]["evidence"][0]["source"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    // A tester's repro: the graph is loaded, the id names nothing.
    let (env, exit) = add(
        &home,
        "umx:sec_b/01ZZZZZZZZZZZZZZZZZZZZZZZZ#quote=nothing here",
    );
    assert_eq!(exit, 0, "{}", env.to_json());
    let dangling = issue(&env, Code::DanglingEvidence);
    assert!(
        dangling.message().contains("sec_b"),
        "{}",
        dangling.message()
    );
    assert_eq!(dangling.next(), "umx node find <words> --graph sec_b");
    assert_eq!(source(&env), "umx:sec_b/01ZZZZZZZZZZZZZZZZZZZZZZZZ");
    // An ambiguous prefix: kept, with the candidates.
    let (env, _) = add(&home, "umx:sec_b/01RSBBBB#L1");
    assert_eq!(issue(&env, Code::DanglingEvidence).candidates().len(), 2);
    assert_eq!(source(&env), "umx:sec_b/01RSBBBB");
    // A unique prefix is stored as the full id, alias as written; no warning.
    let (env, _) = add(&home, "umx:sec_b/01rsaaaa#quote=x");
    assert!(
        env.warnings.iter().all(|w| !w.code().contains("evidence")),
        "{}",
        env.to_json()
    );
    assert_eq!(source(&env), format!("umx:sec_b/{A1}"));
    // The scheme folds (UMX:, Umx:) and is stored as umx:, checked the same way.
    let (env, _) = add(&home, "UMX:sec_b/01rsaaaa#quote=x");
    assert!(
        env.warnings.iter().all(|w| !w.code().contains("evidence")),
        "{}",
        env.to_json()
    );
    assert_eq!(source(&env), format!("umx:sec_b/{A1}"));
    let (env, _) = add(&home, "Umx:sec_b/01ZZZZZZZZZZZZZZZZZZZZZZZZ#L1");
    issue(&env, Code::DanglingEvidence);
    assert_eq!(source(&env), "umx:sec_b/01ZZZZZZZZZZZZZZZZZZZZZZZZ");
    // graph-addressing-expert F1 (S2): a ?repo= hint after the selectors is split off before #, the selector is stored,
    // and the hint stays on the source through the prefix expansion.
    let (env, exit) = add(
        &home,
        "umx:sec_b/01rsaaaa#seq=1-2?repo=https://example.invalid/b.git",
    );
    assert_eq!(exit, 0, "{}", env.to_json());
    assert!(
        env.warnings.iter().all(|w| !w.code().contains("evidence")),
        "{}",
        env.to_json()
    );
    assert_eq!(
        source(&env),
        format!("umx:sec_b/{A1}?repo=https://example.invalid/b.git")
    );
    assert_eq!(
        env.data["node"]["evidence"][0]["selector"],
        json!({ "seq": [1, 2] })
    );
    // The planned copy form (req-69) is kept as written and unchecked.
    let (env, exit) = add(&home, "umx:sec_b@main/01RSAAAA#L1");
    assert_eq!(exit, 0, "{}", env.to_json());
    issue(&env, Code::EvidenceUnchecked);
    assert_eq!(source(&env), "umx:sec_b@main/01RSAAAA");
    // node note and batch check too.
    let noted = home.ok(&[
        "node", "add", "--graph", "rsv", "--type", "finding", "--title", "Noted",
    ]);
    let id = noted.data["node"]["id"].as_str().unwrap().to_owned();
    let (env, exit) = home.umx(&[
        "node",
        "note",
        &format!("umx:rsv/{id}"),
        "--append",
        "more",
        "--evidence",
        "umx:rsv/01RSZZZZZZ#L1",
    ]);
    assert_eq!(exit, 0, "{}", env.to_json());
    issue(&env, Code::DanglingEvidence);
    let ops = home.ops_file(
        "{\"op\":\"node add\",\"type\":\"finding\",\"title\":\"B\",\"evidence\":[\"umx:rsv/01RSBBBB#L1\",\"umx:elsewhere/01RSAAAA#L1\"]}\n",
    );
    let (env, exit) = home.umx(&["batch", "--file", &ops, "--graph", "rsv"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    issue(&env, Code::DanglingEvidence);
    issue(&env, Code::EvidenceUnchecked);

    // A graph that is not loaded: unchecked, and the write leaves it unloaded. After a restart sec_b is known to
    // this machine but not loaded unless the daemon brought it back; either way the write loads nothing.
    home.restart();
    resolve_fixture(&mut home, "rsv");
    let aliases = |home: &Home| -> Vec<String> {
        let env = home.ok(&["graph", "list"]);
        env.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["alias"].as_str().unwrap().to_owned())
            .collect()
    };
    let before = aliases(&home);
    let (env, exit) = add(&home, "umx:sec_b/01ZZZZZZZZZZZZZZZZZZZZZZZZ#L1");
    assert_eq!(exit, 0, "{}", env.to_json());
    if before.iter().any(|a| a == "sec_b") {
        issue(&env, Code::DanglingEvidence);
    } else {
        let unchecked = issue(&env, Code::EvidenceUnchecked);
        assert!(
            unchecked
                .next()
                .starts_with("umx graph load <path> --as sec_b"),
            "{}",
            unchecked.next()
        );
    }
    assert_eq!(aliases(&home), before, "evidence never loads a graph");
}

/// F2 (review): the no-load witness. Both graphs unloaded when idle, so F72's registry remembers them as loaded
/// and would load either on demand. The write names rsv, which F72 loads; its evidence cites sec_b, which must stay
/// unloaded: kept with evidence_unchecked, not expanded, and no on-demand load of sec_b. Negative control (a lookup
/// that loads on a miss fails this test): plan 076's no-load negative control (not published).
#[test]
fn evidence_into_a_remembered_graph_never_loads_it() {
    let mut home = Home::new("evidence-no-load");
    home.start_with(&["--idle-unload", "1s"]);
    resolve_fixture(&mut home, "rsv");
    resolve_fixture(&mut home, "sec_b");
    home.wait_until_idle();
    let (env, exit) = home.umx(&[
        "node",
        "add",
        "--graph",
        "rsv",
        "--type",
        "finding",
        "--title",
        "Cites a remembered graph",
        "--evidence",
        "umx:sec_b/01rsaaaa#L1",
    ]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let loaded: Vec<&str> = env
        .hints
        .iter()
        .filter(|w| w.code() == "graph_loaded_on_demand")
        .map(|w| w.message())
        .collect();
    assert_eq!(loaded.len(), 1, "only the named graph loads: {loaded:?}");
    assert!(loaded[0].contains("Graph rsv "), "{loaded:?}");
    issue(&env, Code::EvidenceUnchecked);
    assert!(
        env.warnings.iter().all(|w| w.code() != "dangling_evidence"),
        "{}",
        env.to_json()
    );
    assert_eq!(
        env.data["node"]["evidence"][0]["source"], "umx:sec_b/01rsaaaa",
        "not expanded: sec_b was never read"
    );
    assert!(
        !home.aliases().iter().any(|a| a == "sec_b"),
        "evidence never loads a graph"
    );
}

/// B1 (security, d-20): evidence checks see open graphs only. A loaded secured graph reads exactly as an unloaded
/// one, so a write to an open graph learns nothing from it: the same warnings (no candidates) and the same stored
/// evidence for a full id, a unique prefix, an ambiguous prefix and a missing id, through node add, node note and batch.
#[test]
fn a_secured_graph_answers_evidence_checks_as_if_unloaded() {
    let mut home = Home::new("evidence-secured");
    home.start();
    resolve_fixture(&mut home, "rsv");
    resolve_fixture(&mut home, "sec_b");
    let target = home.ok(&["resolve", "umx:sec_b"]);
    assert_eq!(target.data["node"]["type"], "graph");
    let graph_id = target.data["graph_id"].as_str().unwrap();
    home.ok(&["graph", "set", "sec_b", "--policy", "secured"]);
    let sources = [
        format!("umx:sec_b/{A1}"),
        "umx:sec_b/01rsaaaa".to_owned(),
        "umx:sec_b/01RSBBBB".to_owned(),
        "umx:sec_b/01ZZZZZZZZZZZZZZZZZZZZZZZZ".to_owned(),
        format!("umx:sec_b/{graph_id}"),
    ];
    // Per write: the evidence warnings in full, and the last stored evidence pointer.
    // Each round notes a fresh node: an identical pointer appended twice to one node is stored once.
    let observe = |home: &Home| -> Vec<(Vec<String>, Value)> {
        let mut seen = Vec::new();
        let noted = home.ok(&[
            "node", "add", "--graph", "rsv", "--type", "finding", "--title", "Noted",
        ]);
        let noted = noted.data["node"]["id"].as_str().unwrap().to_owned();
        for (i, source) in sources.iter().enumerate() {
            let pointer = format!("{source}#L1");
            let evidence_warnings = |env: &Envelope| -> Vec<String> {
                env.warnings
                    .iter()
                    .filter(|w| w.code().contains("evidence"))
                    .map(|w| json!(w).to_string())
                    .collect()
            };
            let stored = |home: &Home, id: &str| -> Value {
                let got = home.ok(&["node", "get", &format!("umx:rsv/{id}")]);
                got.data["node"]["evidence"]
                    .as_array()
                    .unwrap()
                    .last()
                    .cloned()
                    .unwrap()
            };
            let title = format!("Probe {i} {}", seen.len());
            let added = home.ok(&[
                "node",
                "add",
                "--graph",
                "rsv",
                "--type",
                "finding",
                "--title",
                &title,
                "--evidence",
                &pointer,
            ]);
            let id = added.data["node"]["id"].as_str().unwrap().to_owned();
            seen.push((evidence_warnings(&added), stored(home, &id)));
            let (note, exit) = home.umx(&[
                "node",
                "note",
                &format!("umx:rsv/{noted}"),
                "--append",
                "more",
                "--evidence",
                &pointer,
            ]);
            assert_eq!(exit, 0, "{}", note.to_json());
            seen.push((evidence_warnings(&note), stored(home, &noted)));
            let ops = home.ops_file(&format!(
                "{{\"op\":\"node add\",\"type\":\"finding\",\"title\":\"Batch {i} {}\",\"evidence\":[\"{pointer}\"]}}\n",
                seen.len()
            ));
            let (batch, exit) = home.umx(&["batch", "--file", &ops, "--graph", "rsv"]);
            assert_eq!(exit, 0, "{}", batch.to_json());
            let id = batch.data["rows"][0]["id"].as_str().unwrap().to_owned();
            seen.push((evidence_warnings(&batch), stored(home, &id)));
        }
        seen
    };
    let secured = observe(&home);
    let bare_refusal = |home: &Home| {
        let (env, exit) = home.umx(&[
            "node",
            "add",
            "--graph",
            "rsv",
            "--type",
            "finding",
            "--title",
            "Refused",
            "--evidence",
            "umx:sec_b#L1",
        ]);
        assert_eq!(exit, 1, "{}", env.to_json());
        let problem = issue(&env, Code::EvidencePointerInvalid);
        assert_eq!(problem.next(), "umx docs get evidence");
        assert!(
            problem
                .message()
                .contains("a graph address has no selector or fragment")
        );
        env.to_json()
    };
    let secured_bare = bare_refusal(&home);
    // Three writes per pointer; each keeps the source as written (no prefix expansion) with one bare warning.
    for ((warnings, stored), source) in secured.iter().zip(sources.iter().flat_map(|p| [p, p, p])) {
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("evidence_unchecked"), "{warnings:?}");
        assert!(
            !warnings[0].contains("\"candidates\""),
            "no candidates: {warnings:?}"
        );
        assert_eq!(stored["source"], *source, "{stored}");
    }
    home.ok(&["graph", "unload", "sec_b"]);
    let unloaded = observe(&home);
    assert_eq!(secured, unloaded, "secured and unloaded must look the same");
    assert_eq!(
        secured_bare,
        bare_refusal(&home),
        "bare graph syntax never reveals loaded state"
    );
}

/// ws006 S1 (req-47): `root:<name>/<path>` evidence against `umx root add`, the built-in root:repo, and the
/// security ruling's containment: a root outside the roots, a path or symlink leading out, a root swapped for a
/// symlink after it was added, and a repo config that tries to add one.
#[test]
fn evidence_cites_source_roots_inside_the_roots_only() {
    let mut home = Home::new("source-roots");
    home.graph();
    let add = |home: &Home, source: &str| {
        home.umx(&[
            "node",
            "add",
            "--type",
            "problem",
            "--title",
            "cited",
            "--evidence",
            source,
        ])
    };
    let cited = "root:acme/transcripts/016.ndjson#seq=1-5";

    let (env, exit) = add(&home, cited);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert_eq!(
        issue(&env, Code::RootUnknown).next(),
        "umx root add acme <folder>"
    );

    let acme = home.path.join("acme");
    fs::create_dir_all(acme.join("transcripts")).unwrap();
    let canonical = acme.canonicalize().unwrap().display().to_string();
    let added = home.ok(&["root", "add", "acme", acme.to_str().unwrap()]);
    assert_eq!(added.data["root"]["canonical"], canonical.as_str());
    assert_eq!(added.data["root"]["folder"], acme.to_str().unwrap());

    // Stored as written: no path expansion, the selector as an object.
    let stored = home.ok(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "cited",
        "--evidence",
        cited,
    ]);
    let evidence = &stored.data["node"]["evidence"][0];
    assert_eq!(evidence["source"], "root:acme/transcripts/016.ndjson");
    assert_eq!(evidence["selector"], json!({ "seq": [1, 5] }));

    let (env, _) = add(&home, "root:acme/../x#L1");
    issue(&env, Code::EvidenceOutsideRoots);
    std::os::unix::fs::symlink("/etc", acme.join("out")).unwrap();
    let (env, _) = add(&home, "root:acme/out/passwd#L1");
    issue(&env, Code::EvidenceOutsideRoots);

    // root:repo needs no config, only a git folder inside the roots.
    let (env, _) = add(&home, "root:repo/docs/x.md#L1");
    issue(&env, Code::RootUnavailable);
    fs::create_dir_all(home.path.join(".git")).unwrap();
    let repo = home.ok(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "in repo",
        "--evidence",
        "root:repo/docs/x.md#L1",
    ]);
    assert_eq!(
        repo.data["node"]["evidence"][0]["source"],
        "root:repo/docs/x.md"
    );

    // A repo config cannot add a root: the machine layer alone names them.
    let repo_config = home.path.join(".umx/config.toml");
    fs::create_dir_all(repo_config.parent().unwrap()).unwrap();
    fs::write(&repo_config, "source_roots.evil = \"/etc\"\n").unwrap();
    fs::set_permissions(&repo_config, fs::Permissions::from_mode(0o600)).unwrap();
    let (env, _) = add(&home, "root:evil/passwd#L1");
    issue(&env, Code::RootUnknown);

    let (env, _) = home.umx(&["root", "add", "etc", "/etc"]);
    issue(&env, Code::PathOutsideRoots);
    let (env, _) = home.umx(&["config", "set", "source_roots.etc", "/etc"]);
    issue(&env, Code::PathOutsideRoots);
    let (env, _) = home.umx(&[
        "root",
        "add",
        "gone",
        home.path.join("gone").to_str().unwrap(),
    ]);
    issue(&env, Code::RootUnavailable);
    for bad in ["repo", "Bad-Name"] {
        let (env, _) = home.umx(&["root", "add", bad, acme.to_str().unwrap()]);
        issue(&env, Code::NameInvalid);
    }

    let list = home.ok(&["root", "list"]);
    assert_eq!(
        list.data["roots"],
        json!([{ "name": "acme", "folder": acme.to_str().unwrap(), "canonical": canonical, "status": "ok" }])
    );

    // Swapped for a symlink that leads out after it was added: refused at the next use, and listed as such.
    fs::rename(&acme, home.path.join("acme_moved")).unwrap();
    std::os::unix::fs::symlink("/etc", &acme).unwrap();
    let (env, _) = add(&home, cited);
    issue(&env, Code::PathOutsideRoots);
    assert_eq!(
        home.ok(&["root", "list"]).data["roots"][0]["status"],
        "outside_roots"
    );
    issue(&home.umx(&["config", "check"]).0, Code::PathOutsideRoots);

    home.ok(&["root", "remove", "acme"]);
    let (env, _) = home.umx(&["root", "remove", "acme"]);
    issue(&env, Code::RootUnknown);
    let (env, _) = add(&home, cited);
    issue(&env, Code::RootUnknown);

    // Review R1: a value umx cannot use is a warning in the list; a machine file it cannot parse or trust is the
    // error from list and from remove, never an empty list or root_unknown, and its bytes are left alone.
    let machine = home.path.join("config.toml");
    home.ok(&[
        "root",
        "add",
        "acme",
        home.path.join("acme_moved").to_str().unwrap(),
    ]);
    let healthy = fs::read(&machine).unwrap();
    fs::write(&machine, [&healthy[..], b"source_roots.bad = 7\n"].concat()).unwrap();
    let (list, _) = home.umx(&["root", "list"]);
    assert_eq!(list.status, Status::Warning, "{}", list.to_json());
    issue(&list, Code::ConfigValueInvalid);
    assert_eq!(list.data["roots"][0]["name"], "acme");
    let mode = fs::metadata(&machine).unwrap().permissions().mode() & 0o777;
    for (bytes, perm, code) in [
        (&b"[[[\n"[..], mode, Code::ConfigFileInvalid),
        (&healthy[..], 0o666, Code::ConfigFileRefused),
    ] {
        fs::write(&machine, bytes).unwrap();
        fs::set_permissions(&machine, fs::Permissions::from_mode(perm)).unwrap();
        for args in [&["root", "list"][..], &["root", "remove", "acme"]] {
            let (env, exit) = home.umx(args);
            assert_eq!(exit, 1, "{}", env.to_json());
            issue(&env, code);
        }
        assert_eq!(fs::read(&machine).unwrap(), bytes);
    }
    fs::set_permissions(&machine, fs::Permissions::from_mode(mode)).unwrap();
    home.ok(&["root", "remove", "acme"]);
}

/// ws008 TM6: a move's evidence pointer is scanned as a node's is: a secret-looking one warns secret_suspected on
/// the move, a read flags it as status_evidence:<i>, and a batch refuses its line naming status_evidence.
#[test]
fn a_status_moves_evidence_is_scanned() {
    let mut home = Home::new("tm6-secret");
    e2b_graphs(&mut home, &["g1"]);
    let node = e2b_node(&home, "g1", "risk", "Late", "");
    let pointer =
        "https://example.test/doc?token=ghp_0123456789abcdefghijklmnopqrstuvwxyzAB#quote=late";
    tm6_status(&home, &node, "active", &[]);
    let moved = tm6_status(&home, &node, "stalled", &["--evidence", pointer]);
    issue(&moved, Code::SecretSuspected);
    let got = home.ok(&["node", "get", &node]);
    assert_eq!(
        got.data["node"]["secret_suspected"],
        json!(["status_evidence:2"]),
        "{}",
        got.to_json()
    );
    let line =
        json!({"op":"node status","node":node,"status":"active","reason":"r","evidence":pointer});
    let ops = home.ops_file(&format!("{line}\n"));
    let (env, exit) = home.umx(&["batch", "--graph", "g1", "--file", &ops]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let refused = issue(&env, Code::BatchLineInvalid);
    assert!(
        refused.message().contains("status_evidence"),
        "{}",
        refused.message()
    );
}

/// R345-1: `--evidence-kind` is checked even with no `--evidence`, on the verb and a batch status line, and
/// refuses as node add refuses the same kind; no move is made.
#[test]
fn a_bad_evidence_kind_refuses_a_status_move_with_no_pointer() {
    let mut home = Home::new("tm6-kind");
    e2b_graphs(&mut home, &["g1"]);
    let node = e2b_node(&home, "g1", "risk", "Late", "");
    let errors = |args: &[&str]| {
        let (env, exit) = home.umx(args);
        assert_ne!(exit, 0, "{args:?}: {}", env.to_json());
        let v: Value = umx_protocol::parse_json(&env.to_json()).unwrap();
        (exit, v["errors"].to_string())
    };
    let by_add = errors(&[
        "node",
        "add",
        "--graph",
        "g1",
        "--type",
        "risk",
        "--title",
        "Other",
        "--evidence-kind",
        "telepathy",
    ]);
    let by_status = errors(&[
        "node",
        "status",
        &node,
        "agreed",
        "--reason",
        "r",
        "--evidence-kind",
        "telepathy",
    ]);
    // The same refusal; only the message's leading verb name differs.
    assert_eq!(
        by_status,
        (
            by_add.0,
            by_add.1.replace("umx node add:", "umx node status:")
        )
    );
    let line = json!({"op":"node status","node":node,"status":"agreed","reason":"r","evidence_kind":"telepathy"});
    let ops = home.ops_file(&format!("{line}\n"));
    let (env, exit) = home.umx(&["batch", "--graph", "g1", "--file", &ops]);
    assert_ne!(exit, 0, "{}", env.to_json());
    let got = home.ok(&["node", "get", &node]);
    assert_eq!(
        (
            &got.data["node"]["status"],
            got.data["node"]["status_history"].as_array().map(Vec::len)
        ),
        (&json!("open"), Some(0)),
        "no move was made: {}",
        got.to_json()
    );
}

/// R345-2: a move's new evidence pointer records the session that added it (`by_session`), on the verb and a
/// batch line, and the verb's reply shows it as stored; a later move or edit by another session leaves it alone.
#[test]
fn a_status_moves_evidence_records_the_adding_session() {
    let mut home = Home::new("tm6-session");
    e2b_graphs(&mut home, &["g1"]);
    let node = e2b_node(&home, "g1", "risk", "Late", "");
    let as_session = |session: &str, args: &[&str]| {
        let (env, exit) = home.umx_env(args, &[("COPILOT_AGENT_SESSION_ID", session)]);
        assert_eq!(exit, 0, "{args:?}: {}", env.to_json());
        env
    };
    let editor = || home.ok(&["node", "get", &node]).data["node"]["edited_by_session"].clone();
    let moved = as_session(
        "SessionA_01",
        &[
            "node",
            "status",
            &node,
            "stalled",
            "--reason",
            "r",
            "--evidence",
            "notes.md#L1",
        ],
    );
    let a = editor();
    assert!(a.is_string(), "{a}");
    assert_eq!(
        moved.data["evidence"]["by_session"],
        a,
        "{}",
        moved.to_json()
    );
    let line = json!({"op":"node status","node":node,"status":"active","reason":"r","evidence":"notes.md#L2"});
    let ops = home.ops_file(&format!("{line}\n"));
    as_session("SessionB_02", &["batch", "--graph", "g1", "--file", &ops]);
    let b = editor();
    assert_ne!(a, b);
    as_session(
        "SessionC_03",
        &["node", "edit", &node, "--set", "colour=red"],
    );
    assert_ne!(editor(), b);
    let moves = tm6_moves(&home, &node);
    assert_eq!(
        (
            &moves[0]["evidence"]["by_session"],
            &moves[1]["evidence"]["by_session"]
        ),
        (&a, &b),
        "{moves:?}"
    );
}

/// F204: `remove --redact` scrubs a source a status move cites, graph-wide, and may be named from a node that
/// holds it only on a move. Each cited move stays, its `evidence` made a redacted entry in place (with the redacting
/// session); other moves keep their bytes, and a node with no trail gains none.
#[test]
fn a_redact_scrubs_a_status_moves_pointer_and_keeps_the_move() {
    const SOURCE: &str = "leak/tok-FAKE5M3V.md";
    let mut home = Home::new("tm6-redact");
    e2b_graphs(&mut home, &["g1"]);
    let a = e2b_node(&home, "g1", "risk", "Late", "");
    let b = e2b_node(&home, "g1", "risk", "Slow", "");
    let segment = e2b_node(&home, "g1", "segment", "Morning", "");
    let cited = format!("{SOURCE}#L5");
    tm6_status(
        &home,
        &a,
        "stalled",
        &["--in", &segment, "--evidence", &cited],
    );
    tm6_status(&home, &b, "active", &[]);
    tm6_status(
        &home,
        &b,
        "stalled",
        &["--evidence", &format!("{SOURCE}#L9")],
    );
    let b_first = tm6_moves(&home, &b)[0].clone();
    assert!(!files_holding(&home.path, &[SOURCE]).is_empty());
    let (env, exit) = home.umx(&[
        "node",
        "evidence",
        &a,
        "remove",
        &cited,
        "--redact",
        "--reason",
        "it held a token",
    ]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(
        env.data["redacted"],
        json!({ "pointers": 2, "nodes": 2, "edges": 0 })
    );
    assert_eq!(files_holding(&home.path, &[SOURCE]), Vec::<String>::new());
    let got = home.ok(&["node", "get", &a]).data["node"].clone();
    let moved = &got["status_history"][0];
    assert_eq!(
        (&moved["to"], &moved["reason"], moved["in"].is_string()),
        (&json!("stalled"), &json!("as said"), true),
        "{moved}"
    );
    let e = &moved["evidence"];
    assert_eq!(
        (&e["kind"], &e["redacted"], &e["reason"]),
        (&json!("document"), &json!(true), &json!("it held a token")),
        "{e}"
    );
    assert!(
        e.get("source").is_none()
            && e["removed_at"].is_string()
            && e["by_session"] == got["edited_by_session"],
        "{e} {got}"
    );
    assert!(got.get("evidence_removed").is_none(), "{got}");
    let b_moves = tm6_moves(&home, &b);
    assert_eq!(b_moves[0], b_first);
    assert_eq!(
        b_moves[1]["evidence"]["redacted"],
        json!(true),
        "{}",
        b_moves[1]
    );
}

/// R345-3: a redact stamps its session only on the status moves' pointers it redacted; an older redacted
/// entry with no session (written before attribution, or imported) keeps its bytes.
#[test]
fn a_redact_stamps_only_the_moves_it_redacts() {
    const SOURCE: &str = "leak/tok-FAKE2R6T.md";
    let mut home = Home::new("tm6-redact-old");
    e2b_graphs(&mut home, &["g1"]);
    let a = e2b_node(&home, "g1", "risk", "Late", "");
    tm6_status(&home, &a, "active", &[]);
    let path = home.path.join("g1").display().to_string();
    let file = Path::new(&path).join("nodes.jsonl");
    let old = json!({"kind":"room","redacted":true,"removed_at":"2026-01-02T03:04:05Z","by":"someone","reason":"imported"});
    let records: Vec<Value> = fs::read_to_string(&file)
        .unwrap()
        .lines()
        .map(|line| {
            let mut n = umx_protocol::parse_json(line).unwrap();
            if n["id"] == json!(e2c_id(&a)) {
                n["status_history"][0]["evidence"] = old.clone();
            }
            n
        })
        .collect();
    fs::write(
        &file,
        records.iter().map(|n| format!("{n}\n")).collect::<String>(),
    )
    .unwrap();
    home.ok(&["graph", "load", &path]);
    assert_eq!(
        tm6_moves(&home, &a)[0]["evidence"],
        old,
        "the old entry is planted"
    );
    let cited = format!("{SOURCE}#L2");
    tm6_status(&home, &a, "stalled", &["--evidence", &cited]);
    let (env, exit) = home.umx(&[
        "node",
        "evidence",
        &a,
        "remove",
        &cited,
        "--redact",
        "--reason",
        "it held a token",
    ]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let got = home.ok(&["node", "get", &a]).data["node"].clone();
    assert_eq!(got["status_history"][0]["evidence"], old, "{got}");
    assert_eq!(
        got["status_history"][1]["evidence"]["by_session"], got["edited_by_session"],
        "{got}"
    );
}

/// F139 (security): a lint term is found in every place a read shows, with the redact's case fold (ß is ss), and
/// each hit names only the node (or edge) and the place, never the term or the text. A term for another graph does
/// not apply, and a graph the check does not read is never scanned (d-20).
#[test]
fn check_lint_names_where_a_term_sits_and_never_the_term() {
    let mut home = Home::new("f139-terms");
    home.graph();
    let other = home.path.join("other").display().to_string();
    home.ok(&["graph", "new", &other, "--alias", "other"]);
    home.ok(&[
        "node",
        "add",
        "--graph",
        "other",
        "--type",
        "problem",
        "--title",
        "Zorblax elsewhere",
    ]);
    let elsewhere_id = home.ok(&["graph", "list"]).data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["alias"] == "other")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    f139_terms(
        &home,
        &json!({"umx_lint_terms": 1, "terms": [
            {"term": "Zorblax"}, {"term": "Strasse"},
            {"term": "Quillon", "graphs": [elsewhere_id]},
        ]})
        .to_string(),
        0o600,
    );
    let titled = f139_add(&home, "The ZORBLAX plan");
    let fielded = f139_add(&home, "Clean");
    home.ok(&[
        "node",
        "edit",
        &fielded,
        "--set",
        "code=zx-zorblax-7",
        "--note",
        "Down the Straße",
    ]);
    let keyed = f139_add(&home, "Keyed");
    home.ok(&["node", "edit", &keyed, "--set", "entity_key=zorblax:1"]);
    let cited = f139_add(&home, "Cited");
    home.ok(&["node", "evidence", &cited, "add", "docs/a.md#quote=zorblax"]);
    home.ok(&[
        "node",
        "status",
        &cited,
        "active",
        "--reason",
        "the zorblax call",
    ]);
    let edge = home.ok(&[
        "edge",
        "add",
        &cited,
        "supports",
        &keyed,
        "--evidence",
        "docs/b.md#quote=Zorblax",
        "--set-str",
        "label=a zorblax tie",
    ]);
    let edge_id = edge.data["edge"]["id"].as_str().unwrap().to_owned();
    // R377-5: a redacted status pointer keeps its redactor's reason, which a read shows.
    let moved = f139_add(&home, "Moved");
    home.ok(&[
        "node",
        "status",
        &moved,
        "active",
        "--reason",
        "plain",
        "--evidence",
        "docs/c.md#L2",
    ]);
    home.ok(&[
        "node",
        "evidence",
        &moved,
        "remove",
        "docs/c.md#L2",
        "--redact",
        "--reason",
        "a zorblax slip",
    ]);
    f139_add(&home, "Quillon here");
    home.ok(&["graph", "set", "demo", "--purpose", "Track zorblax work"]);
    home.ok(&["graph", "set", "demo", "--title", "Zorblax hub"]);
    let (env, exit) = home.umx(&["check", "--graph", "demo", "--lint"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let id = |n: &str| f139_full(&home, n);
    let mut got = f139_rows(&env, "terms");
    got.sort_by_key(|r| r.1.to_string());
    let mut want = vec![
        (id(&titled), json!("field:title")),
        (id(&fielded), json!("field 1")),
        (id(&fielded), json!("field:note")),
        (id(&keyed), json!("key:1")),
        (id(&cited), json!("evidence:1")),
        (id(&cited), json!("status_reason:1")),
        (json!(edge_id), json!(format!("edge:{edge_id}/evidence:1"))),
        (json!(edge_id), json!(format!("edge:{edge_id}/field 1"))),
        (id(&moved), json!("status_evidence:1")),
        (Value::Null, json!("graph:purpose")),
        (Value::Null, json!("graph:title")),
    ];
    want.sort_by_key(|r| r.1.to_string());
    assert_eq!(got, want, "{}", env.to_json());
    let hint = issue(&env, Code::LintTerms);
    assert!(hint.candidates().is_empty());
    assert!(
        hint.message().contains("the first is in graph:purpose"),
        "{}",
        hint.message()
    );
    let reply = env.to_json().to_lowercase();
    for text in ["zorblax", "strasse", "straße", "quillon"] {
        assert!(!reply.contains(text), "{text}: {reply}");
    }
}

/// R377-6 (pm-001, security): a custom field is named by its ordinal among the node's custom fields (`field 2`, as
/// the redact counts), so a field whose name holds a term is found, by its value or its name alone, and neither the
/// name nor the term is in the reply; the hint's first place is the same string.
#[test]
fn check_lint_never_names_a_custom_field_whose_name_holds_a_term() {
    let mut home = Home::new("f139-names");
    home.graph();
    f139_terms(
        &home,
        r#"{"umx_lint_terms":1,"terms":[{"term":"Zorblax"}]}"#,
        0o600,
    );
    let valued = home.add("problem", "Valued");
    home.ok(&[
        "node",
        "edit",
        &valued,
        "--set",
        "alpha=plain",
        "--set",
        "zorblax_ref=zorblax-9",
    ]);
    let named = home.add("problem", "Named");
    home.ok(&["node", "edit", &named, "--set", "zorblax_tag=plain"]);
    let (env, exit) = home.umx(&["check", "--lint", "--kind", "terms"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let mut got = f139_rows(&env, "terms");
    got.sort_by_key(|r| r.1.to_string());
    let mut want = vec![
        (f139_full(&home, &valued), json!("field 2")),
        (f139_full(&home, &named), json!("field 1")),
    ];
    want.sort_by_key(|r| r.1.to_string());
    assert_eq!(got, want, "{}", env.to_json());
    let first = &env.data["lint"]["rows"][0];
    let at = format!(
        "the first is in {} {}",
        first["id"].as_str().unwrap(),
        first["place"].as_str().unwrap()
    );
    let hint = issue(&env, Code::LintTerms);
    assert!(hint.message().contains(&at), "{at}: {}", hint.message());
    let reply = env.to_json().to_lowercase();
    assert!(!reply.contains("zorblax"), "{reply}");
}

/// F214: on a line whose custom fields are not in name order (a hand edit, an import), `check --lint` and the
/// redact name a field by one number: its place among the custom fields as the line holds them.
#[test]
fn lint_and_redact_number_a_custom_field_alike_on_an_unsorted_line() {
    let mut home = Home::new("f214-ordinals");
    let path = home.graph();
    f139_terms(
        &home,
        r#"{"umx_lint_terms":1,"terms":[{"term":"Zorblax"}]}"#,
        0o600,
    );
    let cited = home.add("problem", "Cited");
    home.ok(&["node", "evidence", &cited, "add", "docs/zorblax.md#L3"]);
    let held = home.add("problem", "Held");
    home.ok(&[
        "node",
        "edit",
        &held,
        "--set-str",
        "alpha=plain",
        "--set-str",
        "zeta=see docs/zorblax.md",
    ]);
    let nodes = Path::new(&path).join("nodes.jsonl");
    let text = fs::read_to_string(&nodes).unwrap();
    let sorted = r#""alpha":"plain","zeta":"see docs/zorblax.md""#;
    assert!(text.contains(sorted), "{text}");
    fs::write(
        &nodes,
        text.replace(sorted, r#""zeta":"see docs/zorblax.md","alpha":"plain""#),
    )
    .unwrap();
    home.ok(&["graph", "load", &path]);

    let (env, exit) = home.umx(&["check", "--lint", "--kind", "terms"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let lint: Vec<Value> = f139_rows(&env, "terms")
        .into_iter()
        .filter(|(id, _)| *id == json!(held))
        .map(|(_, place)| place)
        .collect();
    let (env, exit) = home.umx(&[
        "node",
        "evidence",
        &cited,
        "remove",
        "docs/zorblax.md#L3",
        "--redact",
        "--reason",
        "moved",
    ]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let redact: Vec<Value> = issue(&env, Code::EvidenceRedactBlocked)
        .candidates()
        .iter()
        .filter(|c| c["id"] == json!(held))
        .map(|c| c["title"].clone())
        .collect();
    assert_eq!(
        (lint, redact),
        (vec![json!("field 1")], vec![json!("field 1")])
    );
}

/// R377-1, R377-4: the lint reads graph.json as the check read it, not the loaded copy: a hand edit that takes a term
/// out of the purpose and puts one in the title (the graph's own node) is seen before any reload.
#[test]
fn check_lint_reads_graph_json_as_the_file_holds_it() {
    let mut home = Home::new("f139-fresh");
    let path = home.graph();
    f139_terms(
        &home,
        r#"{"umx_lint_terms":1,"terms":[{"term":"Zorblax"}]}"#,
        0o600,
    );
    home.ok(&["graph", "set", "demo", "--purpose", "Track zorblax work"]);
    let file = Path::new(&path).join("graph.json");
    let mut meta: Value = umx_protocol::parse_json(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(meta["ontology"]["purpose"], "Track zorblax work", "{meta}");
    meta["ontology"]["purpose"] = json!("Track plain work");
    meta["title"] = json!("Zorblax hub");
    fs::write(&file, meta.to_string()).unwrap();
    let (env, exit) = home.umx(&["check", "--lint", "--kind", "terms"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(
        f139_rows(&env, "terms"),
        vec![(Value::Null, json!("graph:title"))],
        "{}",
        env.to_json()
    );
}

/// F139: each graph lists at most 100 rows per kind; lint_truncated's next runs as printed and gives the rest.
#[test]
fn check_lint_pages_a_kind_past_one_hundred_rows() {
    let mut home = Home::new("f139-page");
    home.graph();
    let lines: String = (0..101)
        .map(|i| {
            format!(
                "{}\n",
                json!({"op": "node add", "type": "problem", "title": format!("Loose {i}")})
            )
        })
        .collect();
    let ops = home.ops_file(&lines);
    home.ok(&["batch", "--file", &ops, "--reason", "loose"]);
    let (env, _) = home.umx(&["check", "--lint", "--kind", "orphans"]);
    assert_eq!(f139_rows(&env, "orphans").len(), 100);
    let next = issue(&env, Code::LintTruncated).next().to_string();
    assert_eq!(
        next,
        "umx check --graph demo --lint --kind orphans --offset 100"
    );
    let (rest, exit) = home.run_next(&next);
    assert_eq!(exit, 0, "{}", rest.to_json());
    assert_eq!(f139_rows(&rest, "orphans").len(), 1);
    assert!(issues(&rest, Code::LintTruncated).is_empty());
}

/// F139 (security): a terms file others can read, or one that does not parse, skips the term scan with one hint
/// naming the file only.
#[test]
fn an_unusable_lint_terms_file_skips_the_scan_with_one_hint() {
    for (text, mode) in [
        (
            r#"{"umx_lint_terms":1,"terms":[{"term":"Zorblax"}]}"#,
            0o644,
        ),
        ("{", 0o600),
    ] {
        let mut home = Home::new("f139-unusable");
        home.graph();
        f139_terms(&home, text, mode);
        home.add("problem", "Zorblax");
        let (env, exit) = home.umx(&["check", "--lint", "--kind", "terms"]);
        assert_eq!(exit, 0, "{}", env.to_json());
        assert!(
            f139_rows(&env, "terms").is_empty(),
            "{mode:o}: {}",
            env.to_json()
        );
        assert_eq!(
            env.hints
                .iter()
                .filter(|h| h.code() == "lint_terms_unusable")
                .count(),
            1,
            "{mode:o}: {}",
            env.to_json()
        );
        assert_eq!(
            env.data["lint"]["skipped"],
            json!([{"kind": "terms", "why": "the lint terms file cannot be used"}])
        );
    }
}

/// ws006 S23 (graph-addressing-expert): redirect first, then policy. `umx:<former>/<id>` of a loaded secured graph
/// in another graph's evidence reads exactly as an unloaded graph does (evidence_unchecked, no hint).
#[test]
fn a_secured_graphs_former_alias_reads_as_unloaded_in_evidence() {
    let mut home = Home::new("s23-secured");
    let (_, x, _) = s23_pair(&mut home);
    home.ok(&["graph", "set", "alpha", "--policy", "secured"]);
    home.ok(&["graph", "rename", "alpha", "hidden"]);
    let write = |home: &Home, title: &str| {
        let n = home
            .ok(&[
                "node", "add", "--graph", "beta", "--type", "problem", "--title", title,
            ])
            .data["node"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        home.ok(&[
            "node",
            "evidence",
            "--graph",
            "beta",
            &n,
            "add",
            &format!("umx:alpha/{x}#quote=Target"),
        ])
    };
    let loaded = write(&home, "One");
    home.ok(&["graph", "unload", "hidden"]);
    let unloaded = write(&home, "Two");
    let shown = |env: &Envelope| {
        let mut v = env.to_json();
        v = v.replace(
            &env.data["node"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            "<id>",
        );
        (
            issue(env, Code::EvidenceUnchecked).message().to_string(),
            env.hints.len(),
            v.contains("hidden"),
        )
    };
    assert_eq!(shown(&loaded), shown(&unloaded));
    assert!(!shown(&loaded).2, "{}", loaded.to_json());
}
