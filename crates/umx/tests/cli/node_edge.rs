//! The cli tests: node and edge verbs, dates and anchors.
//! A module of cli.rs, whose `Home` and helpers it uses, in its own file so no file nears the seed gate's
//! size cap.

use super::*;

/// F144 (plan 148): a graph loaded on one branch whose nodes.jsonl a checkout then replaced. Before: the next write
/// rewrote nodes.jsonl from memory, so the checked-out file got every node of the other branch (silent data loss).
/// Now the write is refused with graph_files_changed and the file keeps its bytes; reads answer from memory with
/// stale_files; unload and load clears both; a touch alone refuses too (fail closed).
#[test]
fn a_write_after_a_checkout_changed_the_files_is_refused_and_writes_nothing() {
    let mut home = Home::new("f144");
    let path = home.graph();
    let add = |home: &Home, title: &str| {
        home.umx(&[
            "node", "add", "--graph", "demo", "--type", "finding", "--title", title,
        ])
    };
    // main holds one node; branch e1 adds another. The graph is loaded on e1, then main is checked out under it.
    assert_eq!(add(&home, "Only on main").1, 0);
    let nodes = Path::new(&path).join("nodes.jsonl");
    let main = fs::read(&nodes).unwrap();
    assert_eq!(add(&home, "Only on branch e1").1, 0);
    checkout(&path, "nodes.jsonl", &main);

    let listed = home.ok(&["node", "list", "--graph", "demo"]);
    assert_eq!(listed.data["total"], 2, "the read answers from memory");
    let stale = issue(&listed, Code::StaleFiles);
    assert!(
        stale
            .message()
            .ends_with(": nodes.jsonl. This answer is from the copy loaded before.")
    );
    assert!(
        stale
            .next()
            .starts_with("umx graph unload demo && umx graph load "),
        "{}",
        stale.next()
    );

    let (refused, exit) = add(&home, "Written after the checkout");
    assert_eq!(exit, 1, "{}", refused.to_json());
    let changed = issue(&refused, Code::GraphFilesChanged);
    assert!(
        changed
            .message()
            .contains("changed on disk since it was loaded: nodes.jsonl.")
    );
    assert_eq!(fs::read(&nodes).unwrap(), main, "nothing was written");
    assert!(temp_files(&path).is_empty(), "{:?}", temp_files(&path));
    let listed = home.ok(&["node", "list", "--graph", "demo"]);
    assert_eq!(listed.data["total"], 2, "memory is as it was");

    home.ok(&["graph", "unload", "demo"]);
    home.ok(&["graph", "load", &path, "--as", "demo"]);
    let listed = home.ok(&["node", "list", "--graph", "demo"]);
    assert!(listed.warnings.is_empty(), "{}", listed.to_json());
    assert_eq!(listed.data["total"], 1);
    assert_eq!(listed.data["rows"][0]["title"], "Only on main");
    assert_eq!(add(&home, "Written after the reload").1, 0);

    // A touch changes no byte but moves the file's times: refused all the same (fail closed, graph-storage-expert's
    // ruling).
    let before = fs::read(&nodes).unwrap();
    let long_ago = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    fs::File::options()
        .write(true)
        .open(&nodes)
        .unwrap()
        .set_modified(long_ago)
        .unwrap();
    let (refused, exit) = add(&home, "Written after a touch");
    assert_eq!(exit, 1, "{}", refused.to_json());
    issue(&refused, Code::GraphFilesChanged);
    assert_eq!(fs::read(&nodes).unwrap(), before);
}

/// F220: `umx edge show --human` renders the edge under its sentence, as a node read renders the node: its fields,
/// its evidence, its custom fields and its secret flags (F190), never a bare `: none`.
#[test]
fn edge_show_human_renders_the_edge() {
    let mut home = Home::new("f220-edge-human");
    home.graph();
    let a = home.add("problem", "Approval delays");
    let b = home.add("decision", "Launch date");
    let edge = home.ok(&[
        "edge",
        "add",
        &a,
        "blocks",
        &b,
        "--evidence",
        "docs/plan.md#L4",
        "--set-str",
        "label=weekly sync",
    ]);
    let id = edge.data["edge"]["id"].as_str().unwrap().to_owned();
    let out = home.human(&["edge", "show", &id]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines.first(),
        Some(&"Approval delays blocks Launch date"),
        "{out}"
    );
    for want in [
        format!("id: {id}"),
        "type: blocks".to_string(),
        format!("from: {a}"),
        format!("to: {b}"),
        "label: weekly sync".to_string(),
        "evidence_count: 1".to_string(),
    ] {
        assert!(lines.contains(&want.as_str()), "{want}: {out}");
    }
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("evidence: ") && l.contains("docs/plan.md")),
        "{out}"
    );
    assert!(!lines.contains(&": none"), "{out}");
    // F190's flags render too, as names only.
    let flagged = f190_edge(&home);
    let out = home.human(&["edge", "show", &flagged]);
    let want = format!(
        "secret_suspected: {}",
        json!([
            format!("edge:{flagged}/field 2"),
            format!("edge:{flagged}/evidence:2")
        ])
    );
    assert!(out.lines().any(|l| l == want), "{want}: {out}");
    // R389-1: a custom field named `edge` (a scalar or an object) is one more field of the row; the layout follows
    // the command, so the edge's own fields still show.
    for (value, line) in [
        (r#""custom""#, "edge: custom"),
        (r#"{"note":"nested"}"#, "edge.note: nested"),
    ] {
        let c = home.add("problem", "Gamma");
        let id = home
            .ok(&[
                "edge",
                "add",
                &c,
                "supports",
                &b,
                "--set-json",
                &format!("edge={value}"),
            ])
            .data["edge"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let out = home.human(&["edge", "show", &id]);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.first(), Some(&"Gamma supports Launch date"), "{out}");
        for want in [format!("id: {id}"), "type: supports".to_string()] {
            assert!(lines.contains(&want.as_str()), "{want}: {out}");
        }
        assert!(lines.contains(&line), "{line}: {out}");
    }
}

#[test]
fn the_s05a_verbs_end_to_end() {
    let mut home = Home::new("verbs");
    let path = home.graph();

    // req-07: a new graph loads with the starter vocabulary.
    let (show, _) = home.umx(&["ontology", "show"]);
    assert!(
        show.data["relations"]
            .as_array()
            .unwrap()
            .contains(&"depends_on".into())
    );
    let (loaded, exit) = home.umx(&["graph", "load", &path]);
    assert_eq!(
        (exit, loaded.data["graph"]["alias"].as_str()),
        (0, Some("demo"))
    );

    // req-11: every standard field.
    let (added, _) = home.umx(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Approval delays",
    ]);
    let node = &added.data["node"];
    for key in [
        "id",
        "address",
        "graph",
        "type",
        "title",
        "status",
        "note",
        "evidence",
        "status_history",
        "created",
        "updated",
        "created_by",
    ] {
        assert!(!node[key].is_null(), "{key} missing: {node}");
    }
    let a = node["id"].as_str().unwrap().to_owned();
    let b = home.add("decision", "Launch date");

    // req-59: the sentence is the first line of human output.
    let out = home.human(&["edge", "add", &a, "blocks", &b]);
    assert_eq!(
        out.lines().next(),
        Some("Approval delays blocks Launch date")
    );

    // req-13: listed from both ends, stored once.
    let (from, _) = home.umx(&["edge", "list", "--from", &a]);
    let (to, _) = home.umx(&["edge", "list", "--to", &b]);
    assert_eq!(from.data["rows"][0]["id"], to.data["rows"][0]["id"]);
    assert_eq!(
        umx_core::codec::decode_edges(&fs::read(home.path.join("g/edges.jsonl")).unwrap())
            .unwrap()
            .records
            .iter()
            .filter(|e| !umx_core::ontology::PROVENANCE_RELATIONS.contains(&e.kind.as_str()))
            .count(),
        1
    );

    // req-57: find before add.
    let (found, _) = home.umx(&["node", "find", "aproval", "delays"]);
    assert_eq!(found.data["rows"][0]["id"].as_str(), Some(a.as_str()));

    // req-08: a declared type and field give no warning and a typed value.
    home.umx(&[
        "ontology",
        "add",
        "type",
        "risk",
        "--field",
        "likelihood:number",
    ]);
    let (risk, _) = home.umx(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "Slip",
        "--set",
        "likelihood=3",
    ]);
    assert_eq!(
        (risk.status, risk.data["node"]["likelihood"].as_f64()),
        (Status::Ok, Some(3.0))
    );
    let (listed, _) = home.umx(&[
        "node",
        "list",
        "--where",
        "likelihood>=3",
        "--columns",
        "id,title",
    ]);
    assert_eq!(listed.data["rows"][0]["title"], "Slip");

    // req-01: a fresh daemon over the same files answers the same.
    let (before, _) = home.umx(&["node", "list"]);
    home.umx(&["daemon", "stop"]);
    if let Some(mut d) = home.daemon.take() {
        let _ = d.bounded_wait();
    }
    home.start();
    home.umx(&["graph", "load", &path]);
    let (after, _) = home.umx(&["node", "list"]);
    assert_eq!(before.data, after.data);
}

/// F74: edge list shows an edge's custom fields only when asked. `--columns all` is the usual columns plus every
/// custom field on the page, sorted, null where an edge lacks one; `--columns a,b` is exactly those columns, as node
/// list's; the human table shows them as columns. Without the flag the rows are as before.
#[test]
fn edge_list_shows_custom_fields_when_asked() {
    let mut home = Home::new("edge-columns");
    home.graph();
    let a = home.add("problem", "Approval delays");
    let b = home.add("decision", "Launch date");
    let c = home.add("decision", "Budget");
    let (env, _) = home.umx(&[
        "edge",
        "add",
        &a,
        "blocks",
        &b,
        "--set",
        "confidence=0.8",
        "--set-str",
        "source=standup",
    ]);
    assert_eq!(
        env.data["edge"]["confidence"].as_f64(),
        Some(0.8),
        "{}",
        env.to_json()
    );
    home.umx(&["edge", "add", &a, "blocks", &c]);
    let names = |env: &Envelope| -> Vec<String> {
        let cols = env.data["columns"].as_array().unwrap();
        cols.iter()
            .map(|c| c.as_str().unwrap().to_owned())
            .collect()
    };
    let row = |env: &Envelope, to: &str| -> Value {
        let rows = env.data["rows"].as_array().unwrap();
        rows.iter().find(|r| r["to"] == to).cloned().unwrap()
    };

    let (plain, _) = home.umx(&["edge", "list", "--from", &a]);
    assert!(!names(&plain).contains(&"confidence".to_owned()));
    assert!(row(&plain, &b).get("confidence").is_none());

    let (all, _) = home.umx(&["edge", "list", "--from", &a, "--columns", "all"]);
    let cols = names(&all);
    assert_eq!(cols[..7], names(&plain)[..7], "{cols:?}");
    assert_eq!(cols[cols.len() - 2..], ["confidence", "source"], "{cols:?}");
    assert_eq!(row(&all, &b)["confidence"].as_f64(), Some(0.8));
    assert_eq!(row(&all, &b)["source"], "standup");
    assert_eq!(row(&all, &c).get("confidence"), Some(&Value::Null));
    assert_eq!(row(&all, &c)["sentence"], "Approval delays blocks Budget");

    let (picked, _) = home.umx(&[
        "edge",
        "list",
        "--from",
        &a,
        "--columns",
        "to,confidence,created_by,nope",
    ]);
    assert_eq!(names(&picked), ["to", "confidence", "created_by", "nope"]);
    let picked_b = row(&picked, &b);
    let keys: Vec<&String> = picked_b.as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 4, "{picked_b}");
    assert_eq!(picked_b["confidence"].as_f64(), Some(0.8));
    assert!(picked_b["created_by"].is_string(), "{picked_b}");
    assert!(picked_b["nope"].is_null());
    assert!(row(&picked, &c)["confidence"].is_null());

    let out = home.human(&[
        "edge",
        "list",
        "--from",
        &a,
        "--columns",
        "to_title,confidence",
    ]);
    let header = out
        .lines()
        .find(|l| l.contains("to_title"))
        .unwrap_or_default();
    assert!(header.contains("confidence"), "{out}");
    assert!(out.contains("0.8"), "{out}");
}

/// F137: `--columns all` on node list and node get is the default columns plus every custom field on the page (the
/// node, for get), sorted, null where a node lacks one, as edge list's F74. A page with no custom fields gives exactly
/// the defaults; `all` overrides a type's own columns (F96); `all` beside other names is refused as usage.
#[test]
fn node_list_and_get_columns_all_show_every_custom_field() {
    let mut home = Home::new("node-columns-all");
    home.graph();
    let a = home
        .ok(&[
            "node",
            "add",
            "--type",
            "problem",
            "--title",
            "Approval delays",
            "--set-str",
            "owner=ana",
            "--set",
            "likelihood=3",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let b = home
        .ok(&[
            "node",
            "add",
            "--type",
            "problem",
            "--title",
            "Launch slips",
            "--set-str",
            "zone=west",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let names = |env: &Envelope| -> Vec<String> {
        env.data["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_owned())
            .collect()
    };
    let row = |env: &Envelope, id: &str| -> Value {
        let rows = env.data["rows"].as_array().unwrap();
        rows.iter().find(|r| r["id"] == id).cloned().unwrap()
    };
    let defaults = ["id", "type", "title", "status", "updated"];

    let plain = home.ok(&["node", "list"]);
    assert_eq!(names(&plain), defaults);

    let all = home.ok(&["node", "list", "--columns", "all"]);
    assert_eq!(
        names(&all),
        [&defaults[..], &["likelihood", "owner", "zone"]].concat(),
        "{}",
        all.to_json()
    );
    assert_eq!(row(&all, &a)["owner"], "ana");
    assert_eq!(row(&all, &a)["likelihood"], 3);
    assert_eq!(row(&all, &a)["zone"], Value::Null);
    assert_eq!(row(&all, &b)["zone"], "west");
    assert_eq!(row(&all, &b)["owner"], Value::Null);
    assert!(row(&all, &a).get("all").is_none(), "{}", all.to_json());

    // A page whose nodes have no custom fields: exactly the defaults.
    let bare = home.add("decision", "Budget");
    let only = home.ok(&["node", "list", "--type", "decision", "--columns", "all"]);
    assert_eq!(names(&only), defaults, "{}", only.to_json());
    assert_eq!(row(&only, &bare)["title"], "Budget");

    // A type's own default columns (F96) give way to all.
    home.ok(&["ontology", "set", "type", "problem", "--columns", "title"]);
    let typed = home.ok(&["node", "list", "--type", "problem"]);
    assert_eq!(names(&typed), ["id", "title"], "{}", typed.to_json());
    let typed_all = home.ok(&["node", "list", "--type", "problem", "--columns", "all"]);
    assert_eq!(names(&typed_all), names(&all), "{}", typed_all.to_json());

    // node get: the defaults plus the node's own custom fields, sorted.
    let got = home.ok(&["node", "get", &a, "--columns", "all"]);
    let keys: Vec<&String> = got.data["node"].as_object().unwrap().keys().collect();
    let mut want: Vec<&str> = [&defaults[..], &["likelihood", "owner"]].concat();
    want.sort_unstable();
    let mut have: Vec<&str> = keys.iter().map(|k| k.as_str()).collect();
    have.sort_unstable();
    assert_eq!(have, want, "{}", got.to_json());
    assert_eq!(got.data["node"]["owner"], "ana");
    let got_bare = home.ok(&["node", "get", &bare, "--columns", "all"]);
    assert_eq!(
        got_bare.data["node"].as_object().unwrap().len(),
        defaults.len(),
        "{}",
        got_bare.to_json()
    );

    // all beside other names is refused as usage, never a column named all.
    for args in [
        &["node", "list", "--columns", "all,owner"][..],
        &["node", "get", &a, "--columns", "title,all"][..],
    ] {
        let (env, exit) = home.umx(args);
        assert_eq!(exit, 2, "{args:?}: {}", env.to_json());
        assert!(
            env.to_json().contains("--columns all takes no other names"),
            "{args:?}: {}",
            env.to_json()
        );
    }
}

/// F137: a custom field named like a column umx computes never becomes a column under `--columns all`; the computed
/// column is authoritative, as F74's B1. umx refuses `copy` and the reserved names on write, so the forged field is
/// planted in nodes.jsonl (a hand edit, or a graph written before the name was reserved) and the graph loaded again.
#[test]
fn node_list_columns_all_never_shows_a_custom_field_named_like_a_computed_column() {
    let mut home = Home::new("node-columns-computed");
    let path = home.graph();
    let id = home
        .ok(&[
            "node",
            "add",
            "--type",
            "problem",
            "--title",
            "Forged",
            "--set-str",
            "owner=ana",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    home.ok(&["graph", "unload", "demo"]);
    let file = Path::new(&path).join("nodes.jsonl");
    let text = fs::read_to_string(&file).unwrap();
    let forged: Vec<String> = text
        .lines()
        .map(|line| {
            let mut v: Value = umx_protocol::parse_json(line).unwrap();
            if v["id"] == id.as_str() {
                v["copy"] = json!("forged-copy");
            }
            v.to_string()
        })
        .collect();
    fs::write(&file, forged.join("\n") + "\n").unwrap();
    home.ok(&["graph", "load", &path]);

    let all = home.ok(&["node", "list", "--columns", "all"]);
    let cols: Vec<&str> = all.data["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert!(cols.contains(&"owner"), "{cols:?}");
    assert!(!cols.contains(&"copy"), "{cols:?}");
    assert!(!all.to_json().contains("forged-copy"), "{}", all.to_json());
    let got = home.ok(&["node", "get", &id, "--columns", "all"]);
    assert!(got.data["node"].get("copy").is_none(), "{}", got.to_json());
    assert_eq!(got.data["node"]["owner"], "ana");
    // The instrument can see a positive: the forged field is in the node as stored.
    let raw = home.ok(&["node", "get", &id]);
    assert_eq!(raw.data["node"]["copy"], "forged-copy", "{}", raw.to_json());
}

/// B1: a custom field named like a computed edge list column never stands in for it. A local edge forges
/// `from_state` and `to_title`; an edge to a graph that is then unloaded forges them too. Explicit columns and
/// `all` show the computed values: the real title of a visible end, null for a state or a hidden end's title.
#[test]
fn edge_list_columns_never_fill_a_computed_column_from_a_custom_field() {
    let mut home = Home::new("edge-columns-forged");
    home.graph();
    let a = home.add("problem", "Approval delays");
    let b = home.add("decision", "Launch date");
    let forge = [
        "--set-str",
        "from_state=graph_not_loaded",
        "--set-str",
        "to_title=Forged",
    ];
    let mut args = vec!["edge", "add", a.as_str(), "blocks", b.as_str()];
    args.extend(forge);
    home.ok(&args);
    let beta = home.path.join("beta");
    home.ok(&["graph", "new", beta.to_str().unwrap(), "--alias", "beta"]);
    let far = home
        .ok(&[
            "node", "add", "--type", "problem", "--title", "Far end", "--graph", "beta",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let address = format!("umx:beta/{far}");
    let mut args = vec![
        "edge",
        "add",
        a.as_str(),
        "blocks",
        address.as_str(),
        "--graph",
        "demo",
    ];
    args.extend(forge);
    home.ok(&args);
    home.ok(&["graph", "unload", "beta"]);
    let row = |env: &Envelope, to: &str| -> Value {
        let rows = env.data["rows"].as_array().unwrap();
        rows.iter()
            .find(|r| r["to"] == to)
            .cloned()
            .unwrap_or_else(|| panic!("{}", env.to_json()))
    };
    let shown =
        |env: &Envelope, to: &str, col: &str| row(env, to).get(col).cloned().unwrap_or(Value::Null);

    let (picked, _) = home.umx(&[
        "edge",
        "list",
        "--from",
        &a,
        "--graph",
        "demo",
        "--columns",
        "to,to_title,to_state,from_state",
    ]);
    assert_eq!(
        shown(&picked, &b, "to_title"),
        "Launch date",
        "{}",
        picked.to_json()
    );
    assert_eq!(
        shown(&picked, &b, "from_state"),
        Value::Null,
        "{}",
        picked.to_json()
    );
    assert_eq!(
        shown(&picked, &far, "to_state"),
        "graph_not_loaded",
        "{}",
        picked.to_json()
    );
    assert_eq!(
        shown(&picked, &far, "to_title"),
        Value::Null,
        "{}",
        picked.to_json()
    );
    assert_eq!(
        shown(&picked, &far, "from_state"),
        Value::Null,
        "{}",
        picked.to_json()
    );

    let (all, _) = home.umx(&[
        "edge",
        "list",
        "--from",
        &a,
        "--graph",
        "demo",
        "--columns",
        "all",
    ]);
    let cols: Vec<&str> = all.data["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    for col in ["to_title", "from_state"] {
        assert_eq!(
            cols.iter().filter(|c| **c == col).count(),
            1,
            "{col} in {cols:?}"
        );
    }
    assert_eq!(
        shown(&all, &b, "to_title"),
        "Launch date",
        "{}",
        all.to_json()
    );
    assert_eq!(
        shown(&all, &b, "from_state"),
        Value::Null,
        "{}",
        all.to_json()
    );
    assert_eq!(
        shown(&all, &far, "to_title"),
        Value::Null,
        "{}",
        all.to_json()
    );
    assert_eq!(
        shown(&all, &far, "to_state"),
        "graph_not_loaded",
        "{}",
        all.to_json()
    );
    assert_eq!(
        shown(&all, &far, "from_state"),
        Value::Null,
        "{}",
        all.to_json()
    );
}

#[test]
fn the_s05b_verbs_end_to_end() {
    let mut home = Home::new("s05b");
    let path = home.graph();
    let id = home.add("problem", "Approval delays");

    let (edited, exit) = home.umx(&[
        "node",
        "edit",
        &id,
        "--title",
        "Approval backlog",
        "--set",
        "owner=finance",
    ]);
    assert_eq!(exit, 0, "{}", edited.to_json());
    assert_eq!(edited.data["changed"], json!(["title", "owner"]));
    let (same, _) = home.umx(&["node", "edit", &id, "--title", "Approval backlog"]);
    assert_eq!(same.data["changed"], json!([]));

    // req-58: note and evidence in one write.
    let (noted, exit) = home.umx(&[
        "node",
        "note",
        &id,
        "--append",
        "Raised on the call",
        "--evidence",
        "calls/3-nov.vtt#seq=120-140",
        "--evidence-kind",
        "room",
    ]);
    assert_eq!(exit, 0, "{}", noted.to_json());
    assert_eq!(noted.data["evidence_added"], 1);
    assert_eq!(noted.data["node"]["evidence"][0]["fidelity"], "lossy");

    // node add takes the same evidence flags, in its one write; a refused pointer adds nothing.
    let add_with = |pointer: &str| {
        home.umx(&[
            "node",
            "add",
            "--type",
            "problem",
            "--title",
            "Vendor backlog",
            "--evidence",
            pointer,
            "--evidence-kind",
            "room",
        ])
    };
    let (added, exit) = add_with("calls/3-nov.vtt#seq=1-2");
    assert_eq!(exit, 0, "{}", added.to_json());
    assert_eq!(added.data["node"]["evidence"][0]["fidelity"], "lossy");
    let (refused, exit) = add_with("../secret.md#L1");
    assert_eq!(
        (exit, refused.errors[0].code()),
        (1, "evidence_outside_roots"),
        "{}",
        refused.to_json()
    );
    let (listed, _) = home.umx(&["node", "list", "--type", "problem"]);
    let vendor = listed.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["title"] == "Vendor backlog")
        .count();
    assert_eq!(vendor, 1, "{}", listed.to_json());

    // req-12: a move without a reason warns, and its next records a move with one.
    let (moved, exit) = home.umx(&["node", "status", &id, "agreed"]);
    assert_eq!((exit, moved.data["label"].as_str()), (0, Some("resolved")));
    let next = issue(&moved, Code::ReasonMissing).next().to_owned();
    assert_eq!(
        next,
        format!("umx node status umx:demo/{id} agreed --reason <why>")
    );
    let (again, _) = home.run_next(&next.replace("<why>", "signed_off"));
    assert_eq!(again.data["reason"], "signed_off");
    let (contested, _) = home.umx(&["node", "status", &id, "contested", "--reason", "pilot"]);
    assert_eq!(contested.data["label"], "in_contention");

    let (history, _) = home.umx(&["node", "history", &id]);
    let kinds: Vec<&str> = history.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["created", "status", "status", "status"]);
    assert_eq!(history.data["rows"][3]["to"], "contested");
    let (newest, _) = home.umx(&["node", "history", &id, "--limit", "1"]);
    assert_eq!(
        (
            newest.data["rows"][0]["to"].as_str(),
            newest.data["truncated"].as_bool()
        ),
        (Some("contested"), Some(true))
    );

    // Unload drops only memory: a fresh load sees every change, history included.
    let (unloaded, exit) = home.umx(&["graph", "unload", "demo"]);
    assert_eq!((exit, unloaded.data["unloaded"].as_bool()), (0, Some(true)));
    let (listed, _) = home.umx(&["graph", "list"]);
    assert_eq!(listed.data["total"], 0);
    home.umx(&["graph", "load", &path]);
    let (reloaded, _) = home.umx(&["node", "history", &id]);
    assert_eq!(reloaded.data, history.data);
}

#[test]
fn edge_add_existing_fields_refuse_without_values_in_single_and_atomic_batch_replies() {
    let mut home = Home::new("edge-existing-fields");
    let path = PathBuf::from(home.graph());
    let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
    let (first, exit) = home.umx(&[
        "edge",
        "add",
        &a,
        "blocks",
        &b,
        "--set-str",
        "why=stored_private_marker",
    ]);
    assert_eq!(exit, 0, "{}", first.to_json());
    let id = edge_id(&first);
    let files = || {
        (
            fs::read(path.join("nodes.jsonl")).unwrap(),
            fs::read(path.join("edges.jsonl")).unwrap(),
        )
    };
    let before = files();
    let modified = fs::metadata(path.join("edges.jsonl"))
        .unwrap()
        .modified()
        .unwrap();
    let (again, exit) = home.umx(&[
        "edge",
        "add",
        &a,
        "blocks",
        &b,
        "--set-str",
        "why=stored_private_marker",
    ]);
    assert_eq!(exit, 0, "{}", again.to_json());
    assert_eq!(edge_id(&again), id);
    assert!(
        again
            .warnings
            .iter()
            .any(|issue| issue.code() == "edge_exists")
    );
    assert_eq!(files(), before);
    assert_eq!(
        fs::metadata(path.join("edges.jsonl"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );

    let (refused, exit) = home.umx(&[
        "edge",
        "add",
        &a,
        "blocks",
        &b,
        "--set-str",
        "why=requested_private_marker",
        "--set-str",
        "missing=missing_private_marker",
    ]);
    assert_eq!(exit, 1, "{}", refused.to_json());
    let issue = &refused.errors[0];
    assert_eq!(issue.code(), "edge_exists_differs");
    assert!(issue.message().contains(&id));
    assert!(issue.message().contains("missing why"));
    // plan 205: the in-place edit, naming the first differing field (missing, then why); never a value.
    assert_eq!(
        issue.next(),
        format!("umx edge edit {id} --set missing=<value>")
    );
    assert!(!refused.to_json().contains("private_marker"));
    assert_eq!(files(), before);
    let file = home.ops_file(&format!(
        "{}\n{}\n",
        json!({"op":"node edit","node":a,"title":"Earlier edit must roll back"}),
        json!({"op":"edge add","from":a,"relation":"blocks","to":b,"sets":[
            {"field":"why","as":"str","value":"requested_private_marker"},
            {"field":"missing","as":"str","value":"missing_private_marker"}
        ]}),
    ));
    for dry_run in [true, false] {
        let mut args = vec!["batch", "--file", &file];
        if dry_run {
            args.push("--dry-run");
        }
        let (failed, exit) = home.umx(&args);
        assert_eq!(exit, 1, "{}", failed.to_json());
        assert_eq!(failed.errors[0].code(), "batch_failed");
        assert_eq!(failed.errors[1].code(), "edge_exists_differs");
        assert_eq!(failed.errors[1].line(), Some(2));
        assert_eq!(failed.errors[1].message(), issue.message());
        assert_eq!(failed.errors[1].next(), issue.next());
        assert!(!failed.to_json().contains("private_marker"));
        assert_eq!(files(), before);
    }
}

#[test]
fn edge_fields_have_a_vocabulary_like_node_fields() {
    // F92 (a tester): an undeclared edge field warns, and its next declares it on the relation.
    let mut home = Home::new("edge-fields");
    home.graph();
    let (a, b, c) = (
        home.add("problem", "One"),
        home.add("decision", "Two"),
        home.add("option", "Three"),
    );
    let (warned, exit) = home.umx(&["edge", "add", &a, "supports", &b, "--set", "confidence=0.7"]);
    assert_eq!(exit, 0, "{}", warned.to_json());
    let said: Vec<(&str, &str)> = warned
        .warnings
        .iter()
        .map(|i| (i.code(), i.message()))
        .collect();
    assert_eq!(
        said,
        [(
            "off_vocabulary_edge_field",
            "Field confidence is not declared on relation supports."
        )]
    );
    let next = warned.warnings[0].next();
    assert_eq!(
        next,
        "umx ontology add field --relation supports confidence:float --graph demo"
    );
    for unchanged in [false, true] {
        let (declared, exit) = home.run_next(next);
        assert_eq!(exit, 0, "{}", declared.to_json());
        assert_eq!(
            declared.data,
            umx_protocol::json!({
                "actor_tier": declared.data["actor_tier"],
                "added": "relation field", "relation": "supports", "name": "confidence", "unchanged": unchanged,
            })
        );
    }
    let show = home.ok(&["ontology", "show"]);
    // std's own relation fields (works_for and member_of spans) come with a std graph.
    assert_eq!(
        show.data["relation_fields"]["supports"],
        umx_protocol::json!([{ "name": "confidence", "kind": "float", "std": false }])
    );
    let quiet = home.ok(&["edge", "add", &b, "supports", &c, "--set", "confidence=0.9"]);
    assert!(quiet.warnings.is_empty(), "{}", quiet.to_json());
    let (refused, exit) = home.umx(&[
        "ontology",
        "add",
        "field",
        "--relation",
        "supports",
        "confidence:int",
    ]);
    assert_eq!(
        (exit, refused.errors[0].message()),
        (
            1,
            "Relation supports already has field confidence as float; nothing changed."
        )
    );
}

/// ws006 S1b (req-126): `umx node evidence` end to end. Both argument orders, list positions and canonical text,
/// remove refused without --reason, the trail, already_removed (exit 0), evidence_ambiguous and --at, a batch with
/// one --reason, and nodes.jsonl untouched by reads and repeats.
#[test]
fn node_evidence_lists_adds_and_removes_with_a_trail() {
    let mut home = Home::new("evidence-maint");
    let path = home.graph();
    let id = home.add("problem", "Cited");
    let nodes = Path::new(&path).join("nodes.jsonl");
    let bytes = || fs::read(&nodes).unwrap();
    for p in ["calls/3.ndjson#seq=1-5", "docs/a.md#quote=we ship"] {
        let env = home.ok(&["node", "evidence", &id, "add", p]);
        assert_eq!(env.data["added"], true, "{}", env.to_json());
    }
    let before = bytes();
    assert!(!String::from_utf8_lossy(&before).contains("evidence_removed"));
    let env = home.ok(&["node", "evidence", "list", &id]);
    let rows: Vec<(u64, &str)> = env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["at"].as_u64().unwrap(), r["pointer"].as_str().unwrap()))
        .collect();
    assert_eq!(
        rows,
        vec![
            (1, "calls/3.ndjson#seq=1-5"),
            (2, "docs/a.md#quote=we%20ship")
        ]
    );
    assert_eq!(
        home.ok(&["node", "evidence", &id, "add", "docs/a.md#quote=we ship"])
            .data["added"],
        false
    );
    assert_eq!(bytes(), before, "a list and a repeated add write nothing");

    let (env, exit) = home.umx(&[
        "node",
        "evidence",
        &id,
        "remove",
        "docs/a.md#quote=we%20ship",
    ]);
    assert_eq!(exit, 2, "{}", env.to_json());
    issue(&env, Code::UsageInvalid);
    assert_eq!(bytes(), before);

    let env = home.ok(&[
        "node",
        "evidence",
        &id,
        "remove",
        "docs/a.md#quote=we%20ship",
        "--reason",
        "stale",
    ]);
    assert_eq!(env.data["removed"]["at"], 2);
    let entry = &env.data["removed"]["evidence"];
    assert_eq!(
        (&entry["removed"]["source"], &entry["reason"]),
        (&json!("docs/a.md"), &json!("stale"))
    );
    assert!(
        entry["by_session"].is_string(),
        "the removing session: {entry}"
    );
    let trail = home.ok(&["node", "evidence", &id, "list", "--removed"]);
    assert_eq!(
        trail.data["rows"][0]["pointer"],
        "docs/a.md#quote=we%20ship"
    );
    assert_eq!(trail.data["rows"][0]["evidence"]["reason"], "stale");
    let after = bytes();
    let (env, exit) = home.umx(&[
        "node",
        "evidence",
        "remove",
        &id,
        "docs/a.md#quote=we ship",
        "--reason",
        "stale",
    ]);
    assert_eq!(exit, 0, "{}", env.to_json());
    issue(&env, Code::AlreadyRemoved);
    assert_eq!(bytes(), after, "already_removed writes nothing");

    // Two that differ only by kind: ambiguous until --at names one.
    home.ok(&[
        "node",
        "evidence",
        &id,
        "add",
        "calls/3.ndjson#seq=1-5",
        "--evidence-kind",
        "room",
    ]);
    let (env, exit) = home.umx(&[
        "node",
        "evidence",
        &id,
        "remove",
        "calls/3.ndjson#seq=1-5",
        "--reason",
        "dup",
    ]);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert_eq!(issue(&env, Code::EvidenceAmbiguous).candidates().len(), 2);
    let env = home.ok(&[
        "node",
        "evidence",
        &id,
        "remove",
        "calls/3.ndjson#seq=1-5",
        "--reason",
        "dup",
        "--at",
        "2",
    ]);
    assert_eq!(env.data["removed"]["evidence"]["removed"]["kind"], "room");

    // A batch with one --reason; a rerun is only warnings.
    let ops = home.ops_file(&format!(
        "{{\"op\":\"evidence remove\",\"node\":\"{id}\",\"pointer\":\"calls/3.ndjson#seq=1-5\"}}\n\
         {{\"op\":\"evidence remove\",\"node\":\"{id}\",\"pointer\":\"gone.md#L1\",\"missing\":\"skip\"}}\n"
    ));
    let (env, exit) = home.umx(&["batch", "--file", &ops]);
    assert_eq!(exit, 1, "no reason anywhere: {}", env.to_json());
    assert_eq!(
        issue(&env, Code::BatchFailed).next(),
        "umx batch --file <file> --dry-run"
    );
    // R236-N1: a failed batch that had --reason names a dry run that keeps it, as a placeholder only.
    let strict = home.ops_file(&format!(
        "{{\"op\":\"evidence remove\",\"node\":\"{id}\",\"pointer\":\"never.md#L1\"}}\n"
    ));
    let (env, exit) = home.umx(&["batch", "--file", &strict, "--reason", "secret-ish words"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert_eq!(
        issue(&env, Code::BatchFailed).next(),
        "umx batch --file <file> --dry-run --reason <reason>"
    );
    assert!(!env.to_json().contains("secret-ish"), "{}", env.to_json());
    let env = home.ok(&["batch", "--file", &ops, "--reason", "question lines"]);
    issue(&env, Code::EvidenceMissingSkipped);
    let env = home.ok(&["node", "evidence", &id, "list", "--removed"]);
    assert_eq!(env.data["rows"].as_array().unwrap().len(), 3);
    assert_eq!(env.data["rows"][2]["evidence"]["reason"], "question lines");
    assert!(
        home.ok(&["node", "evidence", &id, "list"]).data["rows"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let (env, exit) = home.umx(&["batch", "--file", &ops, "--reason", "question lines"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    issue(&env, Code::AlreadyRemoved);
}

#[test]
fn a_hand_written_dangling_edge_is_kept_reported_and_left_out() {
    // req-76 through the binary: load warns, check lists it, a node add elsewhere leaves the line byte-identical,
    // and edge list does not show it.
    let mut home = Home::new("dangling");
    let path = home.graph();
    let a = home.add("problem", "One");
    let missing = "01ZZZZZZZZZZZZZZZZZZZZZZZZ";
    let edge_id = "01AAAAAAAAAAAAAAAAAAAAAAAA";
    let line = format!(
        "{{\"id\":\"{edge_id}\",\"type\":\"blocks\",\"from\":\"{a}\",\"to\":\"{missing}\",\"created\":\"2026-11-10T00:00:00Z\",\"created_by\":\"hand\"}}\n"
    );
    let edges = Path::new(&path).join("edges.jsonl");
    fs::write(&edges, &line).unwrap();

    let (env, exit) = home.umx(&["graph", "load", &path]);
    assert_eq!(exit, 0);
    let warned = issue(&env, Code::DanglingEdge);
    assert!(warned.message().contains(edge_id) && warned.message().contains(missing));
    assert!(
        warned.message().contains("its to end"),
        "{}",
        warned.message()
    );
    assert_eq!(
        (
            env.data["graph"]["edges"].as_u64(),
            env.data["graph"]["kept_lines"].as_u64()
        ),
        (Some(0), Some(1))
    );

    let (env, _) = home.umx(&["check", "--graph", "demo"]);
    assert_eq!(env.data["rows"][0]["kind"], "dangling_edge");
    assert_eq!(env.data["rows"][0]["subject"], edge_id);

    home.add("decision", "Elsewhere");
    let (env, _) = home.umx(&["edge", "add", &a, "blocks", &a]);
    assert_eq!(env.errors.len(), 0, "{}", env.to_json());
    let text = fs::read_to_string(&edges).unwrap();
    assert!(
        text.starts_with(&line),
        "the hand-written line is untouched: {text}"
    );
    let (env, _) = home.umx(&["edge", "list"]);
    assert_eq!(env.data["total"], 1, "only the new edge: {}", env.to_json());
}

/// F75: fix a wrong edge with edge edit and edge delete instead of editing edges.jsonl by hand.
#[test]
fn a_wrong_edge_is_retyped_reversed_or_deleted_keeping_its_id() {
    let mut home = Home::new("edge-fix");
    home.env.push(("USER".into(), "edge-login".into()));
    home.env.push(("LOGNAME".into(), "edge-login".into()));
    let path = home.graph();
    let edges_file = Path::new(&path).join("edges.jsonl");
    let lines = || fs::read_to_string(&edges_file).unwrap();
    let (a, b, c) = (
        home.add("problem", "Alpha"),
        home.add("decision", "Bravo"),
        home.add("option", "Charlie"),
    );
    let added = home.umx(&[
        "edge",
        "add",
        &a,
        "blocks",
        &b,
        "--set",
        "colour=red",
        "--set",
        "zone=north",
    ]);
    let e1 = edge_id(&added.0);
    let created = added.0.data["edge"]["created"].clone();
    let created_by = added.0.data["edge"]["created_by"].clone();

    // Reverse: same id, the ends swap, the edit is stamped with who and when.
    let (env, exit) = home.umx(&["edge", "edit", &e1, "--reverse", "--by", "fixer"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(edge_id(&env), e1);
    assert_eq!(env.data["changed"], json!(["direction"]));
    let edge = &env.data["edge"];
    assert_eq!(
        (edge["from"].as_str(), edge["to"].as_str()),
        (Some(&*b), Some(&*a))
    );
    assert_eq!(edge["updated_by"], "edge-login");
    assert_eq!(env.data["sentence"], "Bravo blocks Alpha");
    // The stamp sits among the custom fields in name order: no fixed key place, so an older umx writes the same bytes
    // (graph-storage-expert, F75).
    let updated = edge["updated"].clone();
    let authored = edge["authored_by_session"].clone();
    let edited = edge["edited_by_session"].clone();
    let all = lines();
    let line = all
        .lines()
        .find(|line| umx_protocol::parse_json(line).unwrap()["id"] == e1)
        .unwrap();
    assert_eq!(
        line,
        format!(
            "{{\"id\":\"{e1}\",\"type\":\"blocks\",\"from\":\"{b}\",\"to\":\"{a}\",\"created\":{created},\"created_by\":{created_by},\"authored_by_session\":{authored},\"edited_by_session\":{edited},\"colour\":\"red\",\"updated\":{updated},\"updated_by\":\"edge-login\",\"zone\":\"north\"}}"
        )
    );

    // Retype, then retype and reverse together.
    let (env, _) = home.umx(&["edge", "edit", &e1, "--relation", "supports"]);
    assert_eq!(env.data["changed"], json!(["type"]));
    let (env, _) = home.umx(&["edge", "edit", &e1, "--relation", "answers", "--reverse"]);
    assert_eq!(env.data["changed"], json!(["type", "direction"]));
    assert_eq!(env.data["sentence"], "Alpha answers Bravo");
    // Nothing to change: nothing written.
    let before = lines();
    let (env, exit) = home.umx(&["edge", "edit", &e1, "--relation", "answers"]);
    assert_eq!((exit, env.data["changed"].clone()), (0, json!([])));
    assert_eq!(lines(), before);

    // An edit that would duplicate another edge is refused, naming it; nothing changes.
    let e2 = edge_id(&home.umx(&["edge", "add", &a, "blocks", &c]).0);
    let e3 = edge_id(&home.umx(&["edge", "add", &a, "supports", &c]).0);
    let before = lines();
    let (env, exit) = home.umx(&["edge", "edit", &e3, "--relation", "blocks"]);
    assert_eq!(exit, 1);
    assert!(
        issue(&env, Code::EdgeWouldDuplicate)
            .message()
            .contains(&e2)
    );
    assert_eq!(lines(), before);
    // Reversed onto a symmetric relation's other direction is the same edge too (D49).
    let e4 = edge_id(&home.umx(&["edge", "add", &c, "conflicts_with", &b]).0);
    let e5 = edge_id(&home.umx(&["edge", "add", &b, "blocks", &c]).0);
    let (env, exit) = home.umx(&["edge", "edit", &e5, "--relation", "conflicts_with"]);
    assert_eq!(exit, 1);
    assert!(
        issue(&env, Code::EdgeWouldDuplicate)
            .message()
            .contains(&e4)
    );

    // A retype to a symmetric relation stores the lower id as from (D49); ids are time ordered, so a < c.
    let e6 = edge_id(&home.umx(&["edge", "add", &c, "depends_on", &a]).0);
    let (env, _) = home.umx(&["edge", "edit", &e6, "--relation", "alternative_to"]);
    assert_eq!(env.data["changed"], json!(["type", "direction"]));
    assert_eq!(
        (
            env.data["edge"]["from"].as_str(),
            env.data["edge"]["to"].as_str()
        ),
        (Some(&*a), Some(&*c))
    );

    // The stamp's names are reserved for umx.
    let (env, exit) = home.umx(&["edge", "add", &b, "blocks", &a, "--set", "updated=now"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert_eq!(env.errors[0].code(), "reserved_name");

    // Delete: the reply carries the edge; the id is gone after, and a second delete says so.
    let (env, exit) = home.umx(&["edge", "delete", &format!("umx:demo/{e3}")]);
    assert_eq!((exit, env.data["deleted"].clone()), (0, json!(true)));
    assert_eq!(edge_id(&env), e3);
    assert!(!lines().contains(&e3));
    let (env, exit) = home.umx(&["edge", "delete", &e3]);
    assert_eq!(exit, 1);
    assert_eq!(
        issue(&env, Code::EdgeNotFound).next(),
        "umx edge list --graph demo"
    );

    // The same fixes as one batch: all or nothing.
    let before = lines();
    let ops = format!(
        "{{\"op\":\"edge edit\",\"edge\":\"{e1}\",\"relation\":\"blocks\"}}\n\
         {{\"op\":\"edge delete\",\"edge\":\"{e2}\"}}\n\
         {{\"op\":\"edge add\",\"from\":\"{a}\",\"relation\":\"blocks\",\"to\":\"{c}\"}}\n\
         {{\"op\":\"edge edit\",\"edge\":\"{e5}\",\"reverse\":true}}\n"
    );
    let bad = home.ops_file(&format!(
        "{ops}{{\"op\":\"edge delete\",\"edge\":\"{e2}\"}}\n"
    ));
    let (env, exit) = home.umx(&["batch", "--file", &bad]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let errors: Vec<(&str, Option<u32>)> =
        env.errors.iter().map(|i| (i.code(), i.line())).collect();
    assert_eq!(
        errors,
        [("batch_failed", None), ("edge_not_found", Some(5))]
    );
    assert_eq!(lines(), before);
    let done = big(&home, &["batch", "--file", &home.ops_file(&ops)]);
    let rows: Vec<Value> = done.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| json!([r["op"], r["changed"]]))
        .collect();
    assert_eq!(
        rows,
        [
            json!(["edge edit", ["type"]]),
            json!(["edge delete", []]),
            json!(["edge add", []]),
            json!(["edge edit", ["direction"]]),
        ]
    );
    // Line 3 adds a blocks c after line 2 deleted the old one: no edge_exists, a new id. Line 1's retype moves e1's
    // colour and zone to blocks, which declares neither (F92).
    let warned: Vec<(&str, Option<u32>)> =
        done.warnings.iter().map(|i| (i.code(), i.line())).collect();
    assert_eq!(
        warned,
        [
            ("off_vocabulary_edge_field", Some(1)),
            ("off_vocabulary_edge_field", Some(1))
        ],
        "{}",
        done.to_json()
    );
    let listed = big(&home, &["edge", "list", "--type", "blocks"]);
    let blocks: Vec<(&str, &str)> = listed.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["from"].as_str().unwrap(), r["to"].as_str().unwrap()))
        .collect();
    assert_eq!(blocks, [(&*a, &*b), (&*c, &*b), (&*a, &*c)]);
    assert!(!lines().contains(&e2));
    // umx check lists an edge's own updated field, which edge edit will not overwrite.
    assert!(
        big(&home, &["check"]).data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["kind"] != "edge_reserved_field_in_use")
    );
    let id = rewritten_edge(&mut home, &path, |line| line["updated_by"] = json!(7));
    let rows = big(&home, &["check"]).data["rows"].clone();
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|r| r["kind"] == "edge_reserved_field_in_use" && r["subject"] == json!(id)),
        "{rows}"
    );
}

#[test]
fn derived_retypes_and_deletes_of_repeated_ids_are_refused_and_change_nothing() {
    let mut home = Home::new("edge-fix-r196");
    let path = home.graph();
    let edges_file = Path::new(&path).join("edges.jsonl");
    let lines = || fs::read_to_string(&edges_file).unwrap();
    let listed = |home: &Home| big(home, &["edge", "list"]).data["rows"].clone();
    let refused =
        |env: &Envelope, code: &str| env.errors.iter().map(|i| i.code()).any(|c| c == code);

    // B1: no edge is retyped into a relation umx derives, directly or in a batch; nothing changes on disk or in
    // the daemon.
    let (a, b) = (home.add("problem", "Alpha"), home.add("decision", "Bravo"));
    let e = edge_id(&home.umx(&["edge", "add", &a, "blocks", &b]).0);
    let (disk, memory) = (lines(), listed(&home));
    for derived in ["authored_by", "edited_by", "similar_to"] {
        let code = if derived == "similar_to" {
            "name_invalid"
        } else {
            "derived_relation"
        };
        let (env, exit) = home.umx(&["edge", "edit", &e, "--relation", derived]);
        assert_eq!(exit, 1, "{}", env.to_json());
        assert!(refused(&env, code), "{}", env.to_json());
        let ops = home.ops_file(&format!(
            "{{\"op\":\"edge edit\",\"edge\":\"{e}\",\"relation\":\"{derived}\"}}\n"
        ));
        let (env, exit) = home.umx(&["batch", "--file", &ops]);
        assert_eq!(exit, 1, "{}", env.to_json());
        assert!(refused(&env, code), "{}", env.to_json());
        let (env, exit) = home.umx(&["edge", "add", &a, derived, &b]);
        assert_eq!(exit, 1, "{}", env.to_json());
        assert!(refused(&env, code), "{}", env.to_json());
    }
    assert_eq!((lines(), listed(&home)), (disk, memory));

    // B2: an id on two lines of edges.jsonl, loaded through the real file loader: the earlier line is kept.
    let r = repeated_edge(&mut home, &path);
    assert_eq!(lines().matches(&r).count(), 2, "the kept-line precondition");
    let check = big(&home, &["check"]).data["rows"].clone();
    assert!(
        check
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["kind"] == "duplicate_id" && row["subject"] == json!(r)),
        "{check}"
    );
    // A delete would bring the kept line back on the next load: refused, directly and in a batch; nothing changes.
    let (disk, memory) = (lines(), listed(&home));
    let (env, exit) = home.umx(&["edge", "delete", &r]);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert_eq!(
        issue(&env, Code::EdgeRepeated).next(),
        "umx check --graph demo"
    );
    let ops = home.ops_file(&format!("{{\"op\":\"edge delete\",\"edge\":\"{r}\"}}\n"));
    let (env, exit) = home.umx(&["batch", "--file", &ops]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let errors: Vec<(&str, Option<u32>)> =
        env.errors.iter().map(|i| (i.code(), i.line())).collect();
    assert_eq!(errors, [("batch_failed", None), ("edge_repeated", Some(1))]);
    assert_eq!((lines(), listed(&home)), (disk, memory.clone()));
    let (_, exit) = home.umx(&["graph", "load", &path]);
    assert_eq!(exit, 0);
    assert_eq!(
        listed(&home),
        memory,
        "the edge in use is still the later line"
    );

    // An edit is safe: the edited line stays the one in use after a reload, and it says the id is repeated.
    let (env, exit) = home.umx(&["edge", "edit", &r, "--relation", "depends_on"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert!(
        env.warnings.iter().any(|w| w.code() == "duplicate_id"),
        "{}",
        env.to_json()
    );
    home.umx(&["graph", "load", &path]);
    let kinds = |home: &Home| -> Vec<Value> {
        listed(home)
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["id"] == json!(r))
            .map(|row| row["type"].clone())
            .collect()
    };
    assert_eq!(kinds(&home), [json!("depends_on")]);

    // Once one line is left for the id, the delete goes through and the id is gone after a reload.
    let text: String = lines()
        .lines()
        .filter(|l| !(l.contains(&r) && l.contains("\"supports\"")))
        .map(|l| format!("{l}\n"))
        .collect();
    fs::write(&edges_file, text).unwrap();
    home.umx(&["graph", "load", &path]);
    let (env, exit) = home.umx(&["edge", "delete", &r]);
    assert_eq!(exit, 0, "{}", env.to_json());
    home.umx(&["graph", "load", &path]);
    assert!(kinds(&home).is_empty() && !lines().contains(&r));
}

/// ws006 S3b (req-77's done_when, less S4's resource check): node add --locator titles the node from the file and
/// returns the same id with resource_exists when run again; an absolute path is refused with the root: form in
/// next; resource show prints the locator; edge add works like any edge; node edit re-points and --unset clears;
/// Cypher answers n.locator_kind and reads n.locator as null with query_unknown_property.
#[test]
fn resource_nodes_through_the_cli() {
    let mut home = Home::new("resource-nodes");
    home.graph();
    let add = |h: &Home, locator: &str| {
        h.umx(&["node", "add", "--type", "document", "--locator", locator])
    };
    let (first, exit) = add(&home, "docs/brief.md");
    assert_eq!(exit, 0, "{}", first.to_json());
    let brief = first.data["node"]["id"].as_str().unwrap().to_owned();
    assert_eq!(first.data["node"]["title"], "brief.md");
    assert_eq!(
        first.data["node"]["locator"],
        json!({ "kind": "file", "format": "markdown", "source": "docs/brief.md" })
    );
    let (again, exit) = add(&home, "docs/brief.md");
    assert_eq!(exit, 0, "{}", again.to_json());
    assert_eq!(again.data["node"]["id"], json!(brief));
    assert_eq!(again.warnings[0].code(), "resource_exists");
    assert!(
        again.warnings[0]
            .next()
            .starts_with("umx resource show umx:demo/"),
        "{}",
        again.to_json()
    );

    let (abs, exit) = add(&home, "/etc/hosts");
    assert_eq!((abs.errors[0].code(), exit), ("locator_absolute", 1));
    assert_eq!(
        abs.errors[0].next(),
        "umx node add --type <type> --locator root:<name>/<path>"
    );

    let shown = home.ok(&["resource", "show", &brief]);
    assert_eq!(shown.data["resource"]["locator"]["source"], "docs/brief.md");
    let person = home.add("person", "Ann");
    home.ok(&["edge", "add", &person, "shared", &brief]);

    let rows = |text: &str| {
        let env = home.ok(&["query", text, "--graph", "demo"]);
        (
            env.data["rows"].clone(),
            env.warnings
                .iter()
                .map(|w| w.code().to_owned())
                .collect::<Vec<_>>(),
        )
    };
    let (found, warnings) = rows(
        "MATCH (n) WHERE n.locator_kind = 'file' RETURN n.id AS id, n.locator_format AS format",
    );
    assert_eq!(found, json!([{ "id": brief, "format": "markdown" }]));
    assert!(warnings.is_empty(), "{warnings:?}");
    let (whole, warnings) = rows("MATCH (n) WHERE n.locator_kind = 'file' RETURN n.locator AS l");
    assert_eq!(whole, json!([{ "l": null }]));
    assert_eq!(warnings, ["query_unknown_property"]);
    let listed = home.ok(&[
        "node",
        "list",
        "--where",
        "locator_kind=file",
        "--columns",
        "id,locator_source",
    ]);
    assert_eq!(
        listed.data["rows"],
        json!([{ "id": brief, "locator_source": "docs/brief.md" }])
    );

    let moved = home.ok(&["node", "edit", &brief, "--locator", "docs/brief-v2.md"]);
    assert_eq!(moved.data["node"]["id"], json!(brief));
    assert_eq!(moved.data["changed"], json!(["locator"]));
    assert_eq!(moved.warnings[0].code(), "locator_changed");
    assert!(
        moved.warnings[0].message().contains("1 edges touch it"),
        "{}",
        moved.to_json()
    );
    let cleared = home.ok(&["node", "edit", &brief, "--unset", "locator"]);
    assert_eq!(cleared.data["node"].get("locator"), None);
    let (plain, exit) = home.umx(&["resource", "show", &brief]);
    assert_eq!((plain.errors[0].code(), exit), ("not_a_resource", 1));
    let edges = home.ok(&["edge", "list", "--to", &brief]);
    assert_eq!(
        edges.data["rows"].as_array().unwrap().len(),
        1,
        "the edge stays"
    );
}

/// F211: a date field takes dates and date-times before 1970 like later ones: stored in universal time, read back,
/// and compared across 1970 by `--where`. Outside years 0001 to 9999 a value is not a date (`field_kind_mismatch`).
#[test]
fn a_date_field_holds_dates_before_1970() {
    let mut home = Home::new("f211-pre-1970");
    home.graph();
    home.ok(&["ontology", "add", "field", "problem", "seen_on:date"]);
    let add = |home: &Home, title: &str, seen: &str| {
        let env = home.ok(&[
            "node",
            "add",
            "--type",
            "problem",
            "--title",
            title,
            "--set",
            &format!("seen_on={seen}"),
        ]);
        let id = env.data["node"]["id"].as_str().unwrap().to_owned();
        (id, env)
    };
    let (moon, env) = add(&home, "Moon landing", "1969-07-20T16:17:40-04:00");
    assert!(
        !has(&env, Code::FieldKindMismatch) && !has(&env, Code::TimeNoOffset),
        "{}",
        env.to_json()
    );
    assert_eq!(env.data["node"]["seen_on"], json!("1969-07-20T20:17:40Z"));
    let (hastings, _) = add(&home, "Hastings", "1066-10-14");
    let (later, _) = add(&home, "Later", "1971-01");
    assert_eq!(
        home.ok(&["node", "get", &moon]).data["node"]["seen_on"],
        json!("1969-07-20T20:17:40Z")
    );
    let ids = |w: &str| -> Vec<String> {
        let mut v: Vec<String> = home.ok(&["node", "list", "--where", w]).data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .collect();
        v.sort();
        v
    };
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    assert_eq!(
        ids("seen_on<1970"),
        sorted(vec![moon.clone(), hastings.clone()])
    );
    assert_eq!(
        ids("seen_on>=1969-07-20"),
        sorted(vec![moon.clone(), later.clone()])
    );
    assert_eq!(ids("seen_on=1969-07"), vec![moon.clone()]);
    // Outside years 0001 to 9999: stored as given, not a date.
    let (_, env) = add(&home, "Year zero", "0000-01-01");
    assert!(has(&env, Code::FieldKindMismatch), "{}", env.to_json());
    assert_eq!(env.data["node"]["seen_on"], json!("0000-01-01"));
}

/// D82 (req-158): a `--where` value compares as written. One matching pair of quotes around it is dropped, so a quoted
/// date finds what the unquoted one does; `created` and `updated` compare as dates (UTC spans), as a declared date
/// field does; an ordering compare with a value that is not a date on a date field is refused with `where_invalid`,
/// whose reason shows a date, while `=` and `!=` still compare it as text.
#[test]
fn a_where_value_compares_as_written() {
    let mut home = Home::new("d82-where-as-written");
    let path = home.graph();
    let old = rewritten_node(&mut home, &path, |line| {
        line["created"] = json!("2024-11-11T08:00:00Z");
        line["updated"] = json!("2024-11-12T08:00:00Z");
    });
    let new = home.add("problem", "Fresh");
    let ids = |w: &str| -> Vec<String> {
        let mut v: Vec<String> = home.ok(&["node", "list", "--where", w]).data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .collect();
        v.sort();
        v
    };
    let mut both = vec![old.clone(), new.clone()];
    both.sort();
    for w in [
        "updated>=2025-01-01",
        "updated>='2025-01-01'",
        "updated>=\"2025-01-01\"",
        "updated >= '2025-01-01'",
        "updated>=2025",
        "updated>2024-11-12",
    ] {
        assert_eq!(ids(w), vec![new.clone()], "{w}");
    }
    for w in [
        "updated=2024-11-12",
        "updated='2024-11-12'",
        "updated<2024-11-13",
        "created=\"2024-11-11T08:00:00Z\"",
        "created<'2025'",
    ] {
        assert_eq!(ids(w), vec![old.clone()], "{w}");
    }
    assert_eq!(ids("updated>=2024-11-12"), both);
    // One pair only, and only a matching one: these stay text, so they are not dates.
    for w in ["updated>='2025-01-01\"", "updated>=''2025-01-01''"] {
        let (env, exit) = home.umx(&["node", "list", "--where", w]);
        assert_eq!(exit, 1, "{w}: {}", env.to_json());
        assert_eq!(env.errors[0].code(), "where_invalid", "{}", env.to_json());
    }
    for w in ["updated>=next-week", "created<'last week'", "updated<="] {
        let (env, exit) = home.umx(&["node", "list", "--where", w]);
        assert_eq!(exit, 1, "{w}: {}", env.to_json());
        let e = &env.errors[0];
        assert_eq!(e.code(), "where_invalid", "{}", env.to_json());
        assert!(
            e.message()
                .contains("wants a date such as 2026-01-15 or 2026-01-15T09:00:00Z."),
            "{}",
            env.to_json()
        );
        assert_eq!(e.next(), "umx docs get node", "{}", env.to_json());
    }
    // = and != still compare a non-date as text: no refusal, nothing equal.
    assert!(ids("updated=next-week").is_empty());
    assert_eq!(ids("updated!=next-week"), both);
    // A declared date field takes a quoted date too.
    home.ok(&["ontology", "add", "field", "problem", "due_on:date"]);
    let due = |title: &str, on: &str| {
        home.ok(&[
            "node",
            "add",
            "--type",
            "problem",
            "--title",
            title,
            "--set",
            &format!("due_on={on}"),
        ])
        .data["node"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (feb, dec) = (due("February", "2025-02-14"), due("December", "2024-12-01"));
    assert_eq!(ids("due_on>='2025-02'"), vec![feb.clone()]);
    assert_eq!(ids("due_on<\"2025-02\""), vec![dec.clone()]);
    let (env, exit) = home.umx(&["node", "list", "--where", "due_on>=soon"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert_eq!(env.errors[0].code(), "where_invalid", "{}", env.to_json());
}

/// F211, pm-003: a graph that already stores year 0000 in a date field (a date before F211 set years 0001 to 9999)
/// still loads with no refusal and keeps the value as written. A compare reads it as text, which sorts before every
/// date, so `<1970` still finds it and `>=0001` does not, as when it was a date; `=` matches it exactly. `umx check`
/// names the node and field (`date_not_a_date`, with a `node edit --set` fix).
#[test]
fn a_stored_year_0000_loads_compares_as_text_and_check_names_it() {
    let mut home = Home::new("f211-year-zero");
    let path = home.graph();
    home.ok(&["ontology", "add", "field", "problem", "seen_on:date"]);
    let dated = |home: &Home, title: &str, seen: &str| {
        home.ok(&[
            "node",
            "add",
            "--type",
            "problem",
            "--title",
            title,
            "--set",
            &format!("seen_on={seen}"),
        ])
        .data["node"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (zero, month, instant) = (
        dated(&home, "Year zero", "0000"),
        dated(&home, "Month zero", "0000-01"),
        dated(&home, "Instant zero", "0000-06-01T00:00:00Z"),
    );
    let later = dated(&home, "Later", "1971-01");
    // Unload and load again, as a graph written before F211 would be.
    home.ok(&["graph", "unload", "demo"]);
    let loaded = home.ok(&["graph", "load", &path]);
    assert!(loaded.errors.is_empty(), "{}", loaded.to_json());
    assert_eq!(
        home.ok(&["node", "get", &zero]).data["node"]["seen_on"],
        json!("0000")
    );
    let ids = |w: &str| -> Vec<String> {
        let mut v: Vec<String> = home.ok(&["node", "list", "--where", w]).data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .collect();
        v.sort();
        v
    };
    let mut before = vec![zero.clone(), month.clone(), instant.clone()];
    before.sort();
    assert_eq!(ids("seen_on<1970"), before);
    assert_eq!(ids("seen_on>=0001"), vec![later.clone()]);
    assert_eq!(ids("seen_on=0000"), vec![zero.clone()]);
    let checked = home.ok(&["check"]);
    for id in [&zero, &month, &instant] {
        let named = issues(&checked, Code::DateNotADate)
            .into_iter()
            .chain(
                checked
                    .warnings
                    .iter()
                    .filter(|w| w.code() == "date_not_a_date"),
            )
            .any(|w| w.message().contains(id.as_str()) && w.message().contains("seen_on"));
        assert!(named, "{id}: {}", checked.to_json());
    }
    assert!(
        checked
            .warnings
            .iter()
            .all(|w| w.code() != "date_not_a_date" || !w.message().contains(later.as_str())),
        "{}",
        checked.to_json()
    );
}

/// F182 (field report): a retype to `same_as` (symmetric) of an edge with an end in another graph keeps D49's rule, the
/// lower (graph id, node id) end stored as `from`, and says so: the hint `edge_direction_normalised` names the
/// relation and the ends as stored, by the verb and by a batch line, and the reply's `changed` holds `direction`. A
/// retype that keeps the stored order says nothing. The edge's carried `why`, which `same_as` does not declare, warns
/// `off_vocabulary_edge_field` naming it (the report's "names no field": the reporter's wrapper had dropped the message).
#[test]
fn a_retype_that_swaps_the_stored_ends_says_so() {
    let mut home = Home::new("f182-retype");
    e2b_graphs(&mut home, &["g1", "g2"]);
    let rows = home.ok(&["graph", "list"]).data["rows"].clone();
    let id_of = |alias: &str| {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|r| r["alias"] == alias)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    // The edges are stored in the graph with the higher id, so their far end (in the other graph) is the lower one.
    let (low, high) = if id_of("g1") < id_of("g2") {
        ("g1", "g2")
    } else {
        ("g2", "g1")
    };
    let far = e2b_node(&home, low, "problem", "Alpha", "");
    let far_id = far.rsplit('/').next().unwrap().to_owned();
    let edge_from = |home: &Home, title: &str| {
        let near = e2b_node(home, high, "problem", title, "");
        let id = edge_id(&home.ok(&[
            "edge",
            "add",
            &near,
            "maps_to",
            &far,
            "--graph",
            high,
            "--set",
            "why=mapped",
        ]));
        (near.rsplit('/').next().unwrap().to_owned(), id)
    };
    let check = |env: &Envelope, edge: &str, near: &str| {
        let hint = issue(env, Code::EdgeDirectionNormalised);
        assert!(
            env.hints.iter().any(|h| std::ptr::eq(h, hint)),
            "{}",
            env.to_json()
        );
        let msg = hint.message();
        assert!(
            msg.contains("same_as")
                && msg.contains(edge)
                && msg.contains(&format!("from {far_id}, to {near}")),
            "{msg}"
        );
        assert!(
            issue(env, Code::OffVocabularyEdgeField)
                .message()
                .contains("why"),
            "{}",
            env.to_json()
        );
    };

    let (near, id) = edge_from(&home, "Beta");
    let edited = home.ok(&[
        "edge",
        "edit",
        &id,
        "--relation",
        "same_as",
        "--graph",
        high,
    ]);
    assert_eq!(edited.data["changed"], json!(["type", "direction"]));
    assert_eq!(
        (
            edited.data["edge"]["from"].as_str(),
            edited.data["edge"]["to"].as_str()
        ),
        (Some(far_id.as_str()), Some(near.as_str()))
    );
    check(&edited, &id, &near);

    let (near, id) = edge_from(&home, "Gamma");
    let file = home.path.join("retype.jsonl");
    fs::write(
        &file,
        format!("{{\"op\":\"edge edit\",\"edge\":\"{id}\",\"relation\":\"same_as\"}}\n"),
    )
    .unwrap();
    let batch = home.ok(&[
        "batch",
        "--file",
        &file.display().to_string(),
        "--graph",
        high,
    ]);
    assert_eq!(
        batch.data["rows"][0]["changed"],
        json!(["type", "direction"])
    );
    check(&batch, &id, &near);

    // Stored in the lower graph, from its own node to the far one: already lower end first, so no hint.
    let near = e2b_node(&home, low, "problem", "Delta", "");
    let other = e2b_node(&home, high, "problem", "Epsilon", "");
    let id = edge_id(&home.ok(&["edge", "add", &near, "maps_to", &other, "--graph", low]));
    let kept = home.ok(&["edge", "edit", &id, "--relation", "same_as", "--graph", low]);
    assert_eq!(kept.data["changed"], json!(["type"]));
    assert!(
        kept.hints
            .iter()
            .all(|h| h.code() != "edge_direction_normalised"),
        "{}",
        kept.to_json()
    );
}

/// F134, F161: `edge edit --set` / `--unset` change an edge's custom fields in place, keeping its id, with
/// `edge add --set`'s checks, by the verb and by a batch line. A field set to the value it holds changes nothing.
/// Reserved names are refused, except `--unset evidence` on an older edge whose own `evidence` is not a list, which
/// is what `evidence_field_invalid`'s next now spells.
#[test]
fn edge_edit_sets_and_unsets_custom_fields_keeping_the_id() {
    let mut home = Home::new("f134-edge-set");
    let path = home.graph();
    let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
    let id = edge_id(&home.ok(&[
        "edge",
        "add",
        &a,
        "supports",
        &b,
        "--set",
        "confidence=0.8",
        "--set",
        "why=first",
    ]));

    // The verb: one field set (an undeclared one warns, naming it), one removed; stamps; the id kept.
    let edited = home.ok(&[
        "edge",
        "edit",
        &id,
        "--set",
        "valid_to=2026-11-30",
        "--set-str",
        "why=Named in the kickoff",
        "--unset",
        "confidence",
    ]);
    assert_eq!(
        edited.data["changed"],
        json!(["valid_to", "why", "confidence"])
    );
    let e = &edited.data["edge"];
    assert_eq!(e["id"], json!(id));
    assert_eq!(
        (&e["valid_to"], &e["why"]),
        (&json!("2026-11-30"), &json!("Named in the kickoff"))
    );
    assert!(e.get("confidence").is_none(), "{e}");
    assert!(
        e["updated"].is_string()
            && e["updated_by"].is_string()
            && e["edited_by_session"].is_string(),
        "{e}"
    );
    assert!(
        issues(&edited, Code::OffVocabularyEdgeField)
            .iter()
            .any(|w| w.message().contains("valid_to")),
        "{}",
        edited.to_json()
    );
    // The same value again changes nothing and writes nothing.
    let again = home.ok(&["edge", "edit", &id, "--set", "valid_to=2026-11-30"]);
    assert_eq!(again.data["changed"], json!([]));
    assert_eq!(again.data["edge"]["updated"], e["updated"]);

    // Reserved names are refused, nothing changed; a time with no offset and a secret-looking value warn.
    for (flag, value) in [
        ("--set", "type=supports"),
        ("--set", "created=2026-01-01"),
        ("--unset", "from"),
    ] {
        let (env, exit) = home.umx(&["edge", "edit", &id, flag, value]);
        assert_eq!(
            (exit, env.errors[0].code()),
            (1, "reserved_name"),
            "{}",
            env.to_json()
        );
    }
    home.ok(&[
        "ontology",
        "add",
        "field",
        "--relation",
        "supports",
        "seen_at:date",
    ]);
    let timed = home.ok(&["edge", "edit", &id, "--set", "seen_at=2026-11-30T09:40"]);
    assert!(has(&timed, Code::TimeNoOffset), "{}", timed.to_json());
    let key = fake_aws_key();
    let secret = home.ok(&["edge", "edit", &id, "--set-str", &format!("token={key}")]);
    assert!(has(&secret, Code::SecretSuspected), "{}", secret.to_json());

    // A batch edge edit line takes sets and unsets.
    let file = home.path.join("set.jsonl");
    fs::write(
        &file,
        format!(
            "{{\"op\":\"edge edit\",\"edge\":\"{id}\",\"sets\":[{{\"field\":\"valid_to\",\"value\":\"2026-12-31\"}}],\"unsets\":[\"token\",\"seen_at\"]}}\n"
        ),
    )
    .unwrap();
    let batch = home.ok(&["batch", "--file", &file.display().to_string()]);
    assert_eq!(
        batch.data["rows"][0]["changed"],
        json!(["valid_to", "token", "seen_at"])
    );
    let shown = home.ok(&["edge", "show", &id]);
    assert_eq!(shown.data["valid_to"], json!("2026-12-31"));
    assert!(shown.data.get("token").is_none(), "{}", shown.to_json());

    // An older edge's own `evidence` that is not a list: evidence is refused with a next that moves it, and that
    // next works.
    let old = rewritten_edge(&mut home, &path, |line| {
        line["evidence"] = json!("from the kickoff")
    });
    let (refused, exit) = home.umx(&["edge", "edit", &old, "--set", "evidence=x"]);
    assert_eq!(
        (exit, refused.errors[0].code()),
        (1, "reserved_name"),
        "{}",
        refused.to_json()
    );
    let line = home.ok(&["edge", "show", &old]);
    let (from, to) = (
        line.data["from"].as_str().unwrap().to_owned(),
        line.data["to"].as_str().unwrap().to_owned(),
    );
    let (env, exit) = home.umx(&[
        "edge",
        "add",
        &from,
        "blocks",
        &to,
        "--evidence",
        "docs/a.md#L1",
    ]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let next = issue(&env, Code::EvidenceFieldInvalid).next().to_owned();
    assert!(
        next.starts_with("umx edge edit ") && next.contains("--unset evidence"),
        "{next}"
    );
    let moved = home.ok(&[
        "edge",
        "edit",
        &old,
        "--set-str",
        "why=from the kickoff",
        "--unset",
        "evidence",
    ]);
    assert_eq!(moved.data["changed"], json!(["why", "evidence"]));
    home.ok(&[
        "edge",
        "add",
        &from,
        "blocks",
        &to,
        "--evidence",
        "docs/a.md#L1",
    ]);
}

/// F190: edge show and edge list name what umx's own secret check flags in an edge's evidence and fields, as the
/// lint names those places (`edge:<id>/evidence:<i>` and `edge:<id>/field <n>`, both from 1, never the
/// field's name), left out when nothing is flagged. The value is still shown (the positive control); no flag carries
/// it; an unreturned field, or evidence on a list row, is never flagged; an asked-for column holds `[]`.
#[test]
fn edge_reads_name_secret_shaped_evidence_and_fields_never_the_values() {
    let mut home = Home::new("edge-secret-flags");
    home.graph();
    let id = f190_edge(&home);
    let clean = {
        let (a, b) = (
            home.add("problem", "Plain A"),
            home.add("problem", "Plain B"),
        );
        home.ok(&[
            "edge",
            "add",
            &a,
            "supports",
            &b,
            "--set-str",
            "alpha=plain",
        ])
        .data["edge"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let field2 = format!("edge:{id}/field 2");
    let evidence2 = format!("edge:{id}/evidence:2");

    let shown = home.ok(&["edge", "show", &id]);
    let edge = &shown.data;
    assert_eq!(
        edge["secret_suspected"],
        json!([field2, evidence2]),
        "{}",
        shown.to_json()
    );
    assert!(
        edge["zeta"].as_str().unwrap().contains("AKIA"),
        "the value is still shown"
    );
    assert!(
        edge["evidence"][1]["source"]
            .as_str()
            .unwrap()
            .contains("ghp_")
    );
    assert!(
        home.ok(&["edge", "show", &clean])
            .data
            .get("secret_suspected")
            .is_none()
    );
    assert_eq!(
        f190_lint_places(&home, &id),
        vec![json!(evidence2), json!(field2)],
        "the lint names the same places"
    );

    let rows = |env: &Envelope| env.data["rows"].as_array().unwrap().clone();
    let row = |env: &Envelope, at: &str| rows(env).into_iter().find(|r| r["id"] == at).unwrap();
    let has_column = |env: &Envelope| {
        env.data["columns"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| *c == "secret_suspected")
            .count()
    };
    let plain = home.ok(&["edge", "list"]);
    assert_eq!(has_column(&plain), 0, "{}", plain.to_json());
    assert!(
        rows(&plain)
            .iter()
            .all(|r| r.get("secret_suspected").is_none())
    );
    let all = home.ok(&["edge", "list", "--columns", "all"]);
    assert_eq!(
        row(&all, &id)["secret_suspected"],
        json!([field2]),
        "{}",
        all.to_json()
    );
    assert!(row(&all, &clean).get("secret_suspected").is_none());
    assert_eq!(has_column(&all), 1);
    let alpha = home.ok(&["edge", "list", "--columns", "id,alpha,evidence"]);
    assert_eq!(
        has_column(&alpha),
        0,
        "the unreturned zeta is not flagged: {}",
        alpha.to_json()
    );
    assert!(row(&alpha, &id).get("secret_suspected").is_none());
    let zeta = home.ok(&["edge", "list", "--columns", "id,zeta"]);
    assert_eq!(row(&zeta, &id)["secret_suspected"], json!([field2]));
    let asked = home.ok(&["edge", "list", "--columns", "id,secret_suspected"]);
    assert_eq!(has_column(&asked), 1);
    assert!(
        rows(&asked)
            .iter()
            .all(|r| r["secret_suspected"] == json!([])),
        "{}",
        asked.to_json()
    );

    for env in [&shown, &all, &zeta] {
        let mut names = Vec::new();
        secret_flag_texts(&env.data, &mut names);
        assert!(!names.is_empty());
        assert!(
            names
                .iter()
                .all(|f| !f.contains("AKIA") && !f.contains("ghp_") && !f.contains("zeta")),
            "{names:?}"
        );
        let issues = format!("{:?}{:?}", env.warnings, env.errors);
        assert!(
            !issues.contains("AKIA") && !issues.contains("ghp_"),
            "{issues}"
        );
    }
    // The human table carries the column and the names; the value is hidden (F224).
    let human = home.human(&["edge", "list", "--columns", "id,zeta"]);
    assert!(
        human.contains("secret_suspected") && human.contains("field 2"),
        "{human}"
    );
    assert!(
        !human.contains("AKIA") && human.contains("[secret suspected, hidden]"),
        "{human}"
    );
}

/// Review on #385: edge show shows a computed `sentence`, `from_title` and the rest over a stored custom field of the
/// same name (a hand edit), so a secret-shaped stored one is not shown and never flagged; the fields it does show keep
/// their stored numbers (zeta stays the fourth custom field), and evidence is still flagged.
#[test]
fn edge_show_never_flags_a_stored_field_a_computed_one_hides() {
    let mut home = Home::new("edge-flags-hidden");
    let path = home.graph();
    let id = f190_edge(&home);
    let planted = format!("uses AKIA{}", "Q7Z3X9K2Q7Z3PLNT");
    let edges = Path::new(&path).join("edges.jsonl");
    let text: String = fs::read_to_string(&edges)
        .unwrap()
        .lines()
        .map(|l| {
            let mut v: Value = umx_protocol::parse_json(l).unwrap();
            if v["id"] == id {
                v["sentence"] = json!(planted);
                v["from_title"] = json!(planted);
            }
            format!("{v}\n")
        })
        .collect();
    fs::write(&edges, text).unwrap();
    home.ok(&["graph", "unload", "demo"]);
    home.ok(&["graph", "load", &path]);

    let shown = home.ok(&["edge", "show", &id]);
    assert!(
        !shown.data.to_string().contains("PLNT"),
        "positive control: hidden: {}",
        shown.data
    );
    assert_eq!(
        shown.data["secret_suspected"],
        json!([
            format!("edge:{id}/field 4"),
            format!("edge:{id}/evidence:2")
        ]),
        "{}",
        shown.to_json()
    );
}

/// F222: an older edge's `evidence` field that is not a list is shown under `evidence_field_kept`, over a stored custom
/// field of that name (a hand edit): a secret-shaped stored one is not shown, so never flagged, and the fields shown
/// keep their stored numbers (zeta stays the third custom field).
#[test]
fn edge_show_never_flags_a_stored_field_the_kept_evidence_replaces() {
    let mut home = Home::new("edge-flags-kept");
    let path = home.graph();
    let id = f190_edge(&home);
    let planted = format!("uses AKIA{}", "Q7Z3X9K2Q7Z3PLNT");
    let edges = Path::new(&path).join("edges.jsonl");
    let text: String = fs::read_to_string(&edges)
        .unwrap()
        .lines()
        .map(|l| {
            let mut v: Value = umx_protocol::parse_json(l).unwrap();
            if v["id"] == id {
                v["evidence"] = json!("from the call on Tuesday");
                v["evidence_field_kept"] = json!(planted);
            }
            format!("{v}\n")
        })
        .collect();
    fs::write(&edges, text).unwrap();
    home.ok(&["graph", "unload", "demo"]);
    let (env, exit) = home.umx(&["graph", "load", &path]);
    assert_eq!(exit, 0, "{}", env.to_json());

    let shown = home.ok(&["edge", "show", &id]);
    assert_eq!(
        shown.data["evidence_field_kept"], "from the call on Tuesday",
        "positive control: the kept value replaces the stored field"
    );
    assert!(!shown.data.to_string().contains("PLNT"), "{}", shown.data);
    assert_eq!(
        shown.data["secret_suspected"],
        json!([format!("edge:{id}/field 3")]),
        "{}",
        shown.to_json()
    );
}

/// F223: a secret in an older edge's prose `evidence` field is flagged where edge show shows it,
/// `field:evidence_field_kept`, not under the stored `evidence` key, which holds the evidence list in the reply.
#[test]
fn edge_show_names_a_secret_in_the_kept_evidence_where_it_shows() {
    let mut home = Home::new("edge-flags-kept-prose");
    let path = home.graph();
    let id = f190_edge(&home);
    let planted = format!("from the call, key AKIA{}", "Q7Z3X9K2Q7Z3PLNT");
    let edges = Path::new(&path).join("edges.jsonl");
    let text: String = fs::read_to_string(&edges)
        .unwrap()
        .lines()
        .map(|l| {
            let mut v: Value = umx_protocol::parse_json(l).unwrap();
            if v["id"] == id {
                v["evidence"] = json!(planted);
            }
            format!("{v}\n")
        })
        .collect();
    fs::write(&edges, text).unwrap();
    home.ok(&["graph", "unload", "demo"]);
    let (env, exit) = home.umx(&["graph", "load", &path]);
    assert_eq!(exit, 0, "{}", env.to_json());

    let shown = home.ok(&["edge", "show", &id]);
    assert_eq!(
        shown.data["evidence_field_kept"],
        json!(planted),
        "positive control: the prose is shown there"
    );
    assert_eq!(shown.data["evidence"], json!([]));
    assert_eq!(
        shown.data["secret_suspected"],
        json!([
            format!("edge:{id}/field:evidence_field_kept"),
            format!("edge:{id}/field 2")
        ]),
        "{}",
        shown.to_json()
    );
}

/// ws008 TM3 (req-147, D71 R4): a date-time written with a clock and no offset warns time_no_offset (stored as
/// universal time), on node add, node edit, a batch's node lines and an edge's date field; written with its offset,
/// or as a plain date, it does not.
#[test]
fn a_date_time_without_an_offset_warns_on_every_write() {
    let mut home = Home::new("tm3-offset");
    e2b_graphs(&mut home, &["g1"]);
    home.ok(&[
        "ontology",
        "add",
        "type",
        "meeting",
        "--field",
        "starts_on:date",
        "--graph",
        "g1",
    ]);
    let add = |when: &str| {
        let set = format!("starts_on={when}");
        home.ok(&[
            "node", "add", "--graph", "g1", "--type", "meeting", "--title", "Kickoff", "--set",
            &set,
        ])
    };
    let bare = add("2026-10-30T09:40");
    let i = issue(&bare, Code::TimeNoOffset);
    assert!(i.message().contains("starts_on"), "{}", i.message());
    assert_eq!(i.next(), "umx docs get time");
    assert_eq!(bare.data["node"]["starts_on"], "2026-10-30T09:40:00Z");
    // R332-1: padding never changes the answer; the value is stored trimmed either way.
    let padded = add(" 2026-10-30T09:40:00");
    assert_eq!(
        issues(&padded, Code::TimeNoOffset).len(),
        1,
        "{}",
        padded.to_json()
    );
    for given in [
        "2026-10-30T09:40:00+11:00",
        "2026-10-30T09:40:00Z",
        "2026-10-30",
        "2026-10-30T09:40:00Z ",
        " 2026-10-30T09:40:00-05:00 ",
    ] {
        let env = add(given);
        assert!(
            issues(&env, Code::TimeNoOffset).is_empty(),
            "{given}: {}",
            env.to_json()
        );
    }
    let node = bare.data["node"]["address"].as_str().unwrap().to_owned();
    let edited = home.ok(&["node", "edit", &node, "--set", "starts_on=2026-10-30 10:00"]);
    issue(&edited, Code::TimeNoOffset);
    let batch = tm3_batch(
        &home,
        "g1",
        &[
            json!({"op":"node add","type":"meeting","title":"Retro","sets":[{"field":"starts_on","value":"2026-10-31T09:00"}]}),
            json!({"op":"node edit","node":node,"sets":[{"field":"starts_on","value":"2026-10-30T09:40:00+11:00"}]}),
            json!({"op":"node edit","node":node,"sets":[{"field":"starts_on","value":"2026-10-30T09:40:00Z "}]}),
            json!({"op":"node add","type":"meeting","title":"Padded","sets":[{"field":"starts_on","value":" 2026-11-02T09:00"}]}),
        ],
    );
    let lines: Vec<Option<u32>> = issues(&batch, Code::TimeNoOffset)
        .iter()
        .map(|i| i.line())
        .collect();
    assert_eq!(lines, [Some(1), Some(4)], "{}", batch.to_json());
    let a = e2b_node(&home, "g1", "person", "Ada", "");
    let b = e2b_node(&home, "g1", "organisation", "Acme", "");
    home.ok(&[
        "ontology",
        "add",
        "field",
        "--relation",
        "works_for",
        "starts_on:date",
        "--graph",
        "g1",
    ]);
    let edge = home.ok(&[
        "edge",
        "add",
        &a,
        "works_for",
        &b,
        "--set",
        "starts_on=2026-01-05T09:00",
    ]);
    issue(&edge, Code::TimeNoOffset);
}

/// ws008 TM3: a text field that holds a time warns time_as_text, by its value (a date or a clock time) or by its
/// name (ends in _at or _on); its next declares the field kind date, or for a field declared as text, points at the
/// time topic. A declared date field, and text that is no time, do not warn.
#[test]
fn time_held_as_text_warns_by_value_or_name() {
    let mut home = Home::new("tm3-text");
    e2b_graphs(&mut home, &["g1"]);
    let add = |sets: &[&str]| {
        let mut args = vec![
            "node", "add", "--graph", "g1", "--type", "risk", "--title", "Late",
        ];
        for s in sets {
            args.extend(["--set", s]);
        }
        home.ok(&args)
    };
    for set in ["due=2026-10-30", "met=9:40 am", "signed_on=yesterday"] {
        let env = add(&[set]);
        let warned = issues(&env, Code::TimeAsText);
        assert_eq!(warned.len(), 1, "--set {set}: {}", env.to_json());
        let i = warned[0];
        let field = set.split('=').next().unwrap();
        assert_eq!(
            i.next(),
            format!("umx ontology add field risk {field}:date --graph g1"),
            "{set}"
        );
    }
    let quiet = add(&["owner=Ada", "ratio=3:2"]);
    assert!(
        issues(&quiet, Code::TimeAsText).is_empty(),
        "{}",
        quiet.to_json()
    );
    home.ok(&[
        "ontology",
        "add",
        "field",
        "risk",
        "review_on:date",
        "--graph",
        "g1",
    ]);
    let dated = add(&["review_on=2026-11-01"]);
    assert!(
        issues(&dated, Code::TimeAsText).is_empty(),
        "{}",
        dated.to_json()
    );
    home.ok(&[
        "ontology",
        "add",
        "field",
        "risk",
        "said_at:string",
        "--graph",
        "g1",
    ]);
    let declared = add(&["said_at=standup"]);
    assert_eq!(
        issue(&declared, Code::TimeAsText).next(),
        "umx docs get time"
    );
    let node = quiet.data["node"]["address"].as_str().unwrap().to_owned();
    issue(
        &home.ok(&["node", "edit", &node, "--set", "closed=2026-11-02"]),
        Code::TimeAsText,
    );
    let batch = tm3_batch(
        &home,
        "g1",
        &[
            json!({"op":"node edit","node":node,"title":"Later"}),
            json!({"op":"node add","type":"risk","title":"Other","sets":[{"field":"due","value":"2026-12"}]}),
        ],
    );
    let warned = issues(&batch, Code::TimeAsText);
    assert_eq!(warned.len(), 1, "{}", batch.to_json());
    assert_eq!(warned[0].line(), Some(2));
    let a = e2b_node(&home, "g1", "person", "Ada", "");
    let b = e2b_node(&home, "g1", "organisation", "Acme", "");
    let edge = home.ok(&[
        "edge",
        "add",
        &a,
        "works_for",
        &b,
        "--set",
        "joined_on=2024-03",
    ]);
    assert_eq!(
        issue(&edge, Code::TimeAsText).next(),
        "umx ontology add field --relation works_for joined_on:date --graph g1"
    );
}

/// ws008 TM3 (D71): a meeting, event, segment or transcript written with no starts_on warns anchor_undated, on add
/// (its next names `<node>`), in a batch, and on an edit that retypes a node to one or removes its starts_on (its next
/// names the node); a dated one, another type, or an edit that leaves the date alone does not.
#[test]
fn an_undated_anchor_warns_when_it_is_added_or_made() {
    let mut home = Home::new("tm3-anchor");
    e2b_graphs(&mut home, &["g1"]);
    let add = |kind: &str, sets: &[&str]| {
        let mut args = vec![
            "node", "add", "--graph", "g1", "--type", kind, "--title", "Day one",
        ];
        for s in sets {
            args.extend(["--set", s]);
        }
        home.ok(&args)
    };
    for kind in umx_core::standard::ANCHOR_TYPES {
        let env = add(kind, &[]);
        let i = issue(&env, Code::AnchorUndated);
        assert!(i.message().contains(kind), "{}", i.message());
        assert_eq!(
            i.next(),
            "umx node edit <node> --graph g1 --set starts_on=<date-time>"
        );
    }
    let dated = add("meeting", &["starts_on=2026-10-30T09:40:00+11:00"]);
    assert!(
        issues(&dated, Code::AnchorUndated).is_empty(),
        "{}",
        dated.to_json()
    );
    let other = add("risk", &[]);
    assert!(issues(&other, Code::AnchorUndated).is_empty());
    let node = other.data["node"]["address"].as_str().unwrap().to_owned();
    let retyped = home.ok(&["node", "edit", &node, "--type", "event"]);
    assert_eq!(
        issue(&retyped, Code::AnchorUndated).next(),
        format!("umx node edit {node} --set starts_on=<date-time>")
    );
    let titled = home.ok(&["node", "edit", &node, "--title", "Day two"]);
    assert!(
        issues(&titled, Code::AnchorUndated).is_empty(),
        "{}",
        titled.to_json()
    );
    let meeting = dated.data["node"]["address"].as_str().unwrap().to_owned();
    issue(
        &home.ok(&["node", "edit", &meeting, "--unset", "starts_on"]),
        Code::AnchorUndated,
    );
    let batch = tm3_batch(
        &home,
        "g1",
        &[
            json!({"op":"node add","type":"risk","title":"r"}),
            json!({"op":"node add","type":"segment","title":"Morning"}),
        ],
    );
    let warned = issues(&batch, Code::AnchorUndated);
    assert_eq!(warned.len(), 1, "{}", batch.to_json());
    assert_eq!(warned[0].line(), Some(2));
}

/// ws008 TM4 (req-148): `node add --raised-in <anchor>` adds the node and a raised_in edge to the anchor in one write,
/// to a node of its own graph or of another (an address); the reply names the edge. A refusal of the node or of the
/// anchor writes nothing. An anchor of a type raised_in does not expect warns relation_endpoint_unexpected and is
/// stored; a far anchor in a secured graph never says its type.
#[test]
fn a_node_raised_in_an_anchor_gets_its_edge_in_the_same_write() {
    let mut home = Home::new("tm4-add");
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
    let segment = e2b_node(&home, "g1", "segment", "Morning", "");
    let meeting = e2b_node(&home, "g2", "meeting", "Kickoff", "");
    let add = |anchor: &str| {
        home.ok(&[
            "node",
            "add",
            "--graph",
            "g1",
            "--type",
            "risk",
            "--title",
            "Late",
            "--raised-in",
            anchor,
        ])
    };
    let here = add(&segment);
    let edge = &here.data["raised_in_edge"];
    assert!(
        edge["id"].is_string()
            && edge["sentence"]
                .as_str()
                .is_some_and(|s| s == "Late raised in Morning"),
        "{}",
        here.to_json()
    );
    assert!(issues(&here, Code::RelationEndpointUnexpected).is_empty());
    let node = e2c_id(here.data["node"]["address"].as_str().unwrap());
    assert_eq!(tm4_raised(&home, "g1"), [(node.clone(), e2c_id(&segment))]);
    let far = add(&meeting);
    assert!(
        issues(&far, Code::RelationEndpointUnexpected).is_empty(),
        "{}",
        far.to_json()
    );
    let far_node = e2c_id(far.data["node"]["address"].as_str().unwrap());
    assert!(tm4_raised(&home, "g1").contains(&(far_node, e2c_id(&meeting))));
    let person = e2b_node(&home, "g1", "person", "Bo", "");
    let soft = add(&person);
    let i = issue(&soft, Code::RelationEndpointUnexpected);
    assert!(
        i.message().contains("person") && i.message().contains("segment"),
        "{}",
        i.message()
    );
    assert_eq!(tm4_raised(&home, "g1").len(), 3, "stored all the same");
    let secured = add(&hidden);
    issue(&secured, Code::LinkIntoSecured);
    assert!(
        issues(&secured, Code::RelationEndpointUnexpected).is_empty(),
        "a secured far anchor's type is never read: {}",
        secured.to_json()
    );
    // Atomic: a refused node, or a refused anchor, leaves neither the node nor the edge.
    let before = (
        home.ok(&["node", "list", "--graph", "g1"]).data["rows"].clone(),
        tm4_raised(&home, "g1"),
    );
    let (env, exit) = home.umx(&[
        "node",
        "add",
        "--graph",
        "g1",
        "--type",
        "risk",
        "--title",
        "x",
        "--set",
        "title=y",
        "--raised-in",
        &segment,
    ]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let (env, exit) = home.umx(&[
        "node",
        "add",
        "--graph",
        "g1",
        "--type",
        "risk",
        "--title",
        "x",
        "--raised-in",
        "01JC8XA4H7B3KT9M2NZ5QW6RDE",
    ]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let after = (
        home.ok(&["node", "list", "--graph", "g1"]).data["rows"].clone(),
        tm4_raised(&home, "g1"),
    );
    assert_eq!(before, after);
}

/// ws008 TM4: an anchor that does not resolve is refused byte for byte as edge add refuses that end: a node the graph
/// does not hold, a node another loaded graph does not hold, and a graph that is not loaded; for node add and for a
/// batch, where it refuses the whole batch before any line runs.
#[test]
fn an_unresolvable_anchor_is_refused_as_edge_add_refuses_it() {
    let mut home = Home::new("tm4-refuse");
    e2b_graphs(&mut home, &["g1", "g2"]);
    let from = e2b_node(&home, "g1", "risk", "Late", "");
    let ghost = "01JC8XA4H7B3KT9M2NZ5QW6RDE";
    let errors = |args: &[&str]| {
        let (env, exit) = home.umx(args);
        assert_eq!(exit, 1, "{args:?}: {}", env.to_json());
        let v: Value = umx_protocol::parse_json(&env.to_json()).unwrap();
        v["errors"].to_string()
    };
    let g2_missing = format!("umx:g2/{ghost}");
    let unloaded = format!("umx:g3/{ghost}");
    let ops = home.ops_file("{\"op\":\"node add\",\"type\":\"risk\",\"title\":\"x\"}\n");
    for anchor in [ghost, g2_missing.as_str(), unloaded.as_str()] {
        let by_edge = errors(&["edge", "add", &from, "raised_in", anchor, "--graph", "g1"]);
        let by_node = errors(&[
            "node",
            "add",
            "--graph",
            "g1",
            "--type",
            "risk",
            "--title",
            "x",
            "--raised-in",
            anchor,
        ]);
        assert_eq!(by_node, by_edge, "{anchor}");
        let by_batch = errors(&[
            "batch",
            "--graph",
            "g1",
            "--file",
            &ops,
            "--raised-in",
            anchor,
        ]);
        assert_eq!(by_batch, by_edge, "{anchor}: the batch is refused up front");
    }
    assert_eq!(
        home.ok(&["node", "list", "--graph", "g1"]).data["rows"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// R341-2: the batch's anchor is resolved before any line is checked, so a missing anchor is refused with edge
/// add's own error even when a line would also fail its id preflight.
#[test]
fn a_missing_anchor_beats_a_line_preflight_error() {
    let mut home = Home::new("tm4-order");
    e2b_graphs(&mut home, &["g1"]);
    let taken = e2b_node(&home, "g1", "risk", "Old", "");
    let ghost = "01JC8XA4H7B3KT9M2NZ5QW6RDE";
    let errors = |args: &[&str]| {
        let (env, exit) = home.umx(args);
        assert_eq!(exit, 1, "{args:?}: {}", env.to_json());
        let v: Value = umx_protocol::parse_json(&env.to_json()).unwrap();
        v["errors"].to_string()
    };
    let by_edge = errors(&["edge", "add", &taken, "raised_in", ghost, "--graph", "g1"]);
    let line = json!({"op":"node add","id":e2c_id(&taken),"type":"risk","title":"Clash"});
    let ops = home.ops_file(&format!("{line}\n"));
    let by_batch = errors(&[
        "batch",
        "--graph",
        "g1",
        "--file",
        &ops,
        "--raised-in",
        ghost,
    ]);
    assert_eq!(by_batch, by_edge);
}

/// ws008 TM6: an `--in` anchor that does not resolve is refused byte for byte as edge add refuses that end, and the
/// move is not made.
#[test]
fn a_status_anchor_that_does_not_resolve_is_refused_as_edge_add_refuses_it() {
    let mut home = Home::new("tm6-refuse");
    e2b_graphs(&mut home, &["g1", "g2"]);
    let node = e2b_node(&home, "g1", "risk", "Late", "");
    let ghost = "01JC8XA4H7B3KT9M2NZ5QW6RDE";
    let errors = |args: &[&str]| {
        let (env, exit) = home.umx(args);
        assert_eq!(exit, 1, "{args:?}: {}", env.to_json());
        let v: Value = umx_protocol::parse_json(&env.to_json()).unwrap();
        v["errors"].to_string()
    };
    let g2_missing = format!("umx:g2/{ghost}");
    let unloaded = format!("umx:g3/{ghost}");
    for anchor in [ghost, g2_missing.as_str(), unloaded.as_str()] {
        let by_edge = errors(&["edge", "add", &node, "raised_in", anchor, "--graph", "g1"]);
        let by_status = errors(&[
            "node", "status", &node, "stalled", "--reason", "r", "--in", anchor,
        ]);
        assert_eq!(by_status, by_edge, "{anchor}");
    }
    assert!(tm6_moves(&home, &node).is_empty(), "no move was made");
}
