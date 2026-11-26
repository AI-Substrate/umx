//! The cli tests: provenance, who and resolve.
//! A module of cli.rs, whose `Home` and helpers it uses, in its own file so no file nears the seed gate's
//! size cap.

use super::*;

/// ws007 s04 (req-96): who and node who through the real CLI and daemon, without --provenance, which they refuse.
#[test]
fn who_and_node_who_read_provenance_without_the_flag() {
    let mut home = Home::new("who-verbs");
    home.graph();
    let session = "h-2222222222222222";
    let (added, exit) = raw(
        &home.socket(),
        &provenance_write("pij-who-seat", session, "Who wrote this"),
    );
    assert_eq!(exit, 0, "{}", added.to_json());
    let id = added.data["node"]["id"].as_str().unwrap().to_owned();
    let listed = home.ok(&["who", "pij-who-seat"]);
    assert_eq!(listed.data["key"], "agent/pij/pij-who-seat");
    assert_eq!(listed.data["total"], 1, "{}", listed.to_json());
    let row = &listed.data["rows"][0];
    assert_eq!(row["address"], format!("umx:demo/{id}"));
    assert_eq!(row["how"], json!(["authored", "edited"]));
    assert_eq!(
        row["sessions"],
        json!([format!("session/copilot/{session}")])
    );
    let who = home.ok(&["node", "who", &id]);
    let authored = &who.data["rows"][0];
    assert_eq!(authored["state"], "recorded", "{}", who.to_json());
    assert_eq!(authored["agent"], "agent/pij/pij-who-seat");
    assert_eq!(authored["role"], "coder");
    assert_eq!(authored["harness"], "copilot");
    assert!(authored["model_address"].is_string(), "{}", who.to_json());
    // A new node's author is its last editor too.
    assert_eq!(who.data["rows"][1]["session"], authored["session"]);
    let table = home.human(&["who", "pij-who-seat"]);
    assert!(
        table.contains("Who wrote this") && table.contains("authored"),
        "{table}"
    );
    for args in [
        vec!["who", "pij-who-seat", "--provenance"],
        vec!["node", "who", &id, "--provenance"],
    ] {
        let (refused, exit) = home.umx(&args);
        assert_eq!(exit, 2, "{}", refused.to_json());
        assert_eq!(refused.errors[0].code(), "usage_invalid");
    }
}

#[test]
fn provenance_inline_values_are_usage_errors_before_contacting_the_daemon() {
    let home = Home::new("provenance-inline");
    for flag in [
        "--provenance=false",
        "--provenance=true",
        "--provenance=",
        "--provenance=other",
    ] {
        let (env, exit) = home.umx(&["node", "list", flag]);
        assert_eq!(exit, 2, "{flag}: {}", env.to_json());
        assert_eq!(env.errors[0].code(), "usage_invalid");
        assert!(env.data.is_null());
    }
    for words in [vec!["node", "list"], vec!["node", "list", "--provenance"]] {
        let (env, exit) = home.umx(&words);
        assert_eq!(exit, 3, "{}", env.to_json());
        assert_eq!(env.errors[0].code(), "daemon_not_running");
    }
    assert!(!home.socket().exists());
}

#[test]
fn provenance_opt_in_reaches_each_read_and_raw_writes_are_refused() {
    let mut home = Home::new("provenance-reads");
    let path = home.graph();
    let a = home.add("problem", "First content");
    let b = home.add("problem", "Second content");
    let (node, _) = home.umx(&["node", "get", &a]);
    let session = node.data["node"]["authored_by_session"].as_str().unwrap();
    let (shown, exit) = home.umx(&["node", "get", session, "--provenance"]);
    assert_eq!(exit, 0, "{}", shown.to_json());
    let title = shown.data["node"]["title"].as_str().unwrap();
    for words in [
        vec!["node", "list", "--type", "session"],
        vec!["node", "find", title, "--type", "session"],
        vec!["search", session],
        vec!["edge", "list", "--from", &a, "--type", "authored_by"],
        vec!["neighbours", &a, "--relation", "authored_by"],
        vec!["walk", &a, "--relation", "edited_by"],
        vec!["path", &a, &b, "--relation", "authored_by"],
        vec!["query", "MATCH (s:session) RETURN s.id"],
    ] {
        let (hidden, exit) = home.umx(&words);
        assert_eq!(exit, 0, "{}", hidden.to_json());
        assert_eq!(
            hidden.data["rows"],
            json!([]),
            "{words:?}: {}",
            hidden.to_json()
        );
        let mut words = words;
        words.push("--provenance");
        let (shown, exit) = home.umx(&words);
        assert_eq!(exit, 0, "{}", shown.to_json());
        assert!(
            !shown.data["rows"].as_array().unwrap().is_empty(),
            "{words:?}: {}",
            shown.to_json()
        );
        if words[0] == "edge" {
            assert_eq!(shown.data["rows"][0]["id"], Value::Null);
            assert_eq!(shown.data["rows"][0]["derived"], true);
            assert!(shown.data["rows"][0].get("address").is_none());
        }
        if matches!(words[0], "walk" | "neighbours") {
            assert_eq!(shown.data["rows"][0]["edge"], Value::Null);
            assert_eq!(shown.data["rows"][0]["derived"], true);
        }
    }
    for words in [
        vec!["node", "get", session],
        vec!["node", "history", session],
        vec!["resolve", session],
    ] {
        assert_eq!(home.umx(&words).0.errors[0].code(), "node_not_found");
        let mut words = words;
        words.push("--provenance");
        assert_eq!(home.umx(&words).1, 0);
    }
    for words in [vec!["graph", "list"], vec!["graph", "list", "--all"]] {
        let (hidden, _) = home.umx(&words);
        let mut words = words;
        words.push("--provenance");
        let (shown, _) = home.umx(&words);
        assert!(
            shown.data["rows"][0]["nodes"].as_u64().unwrap()
                > hidden.data["rows"][0]["nodes"].as_u64().unwrap()
        );
        assert!(
            shown.data["rows"][0]["edges"].as_u64().unwrap()
                > hidden.data["rows"][0]["edges"].as_u64().unwrap()
        );
    }
    let before = fs::read(Path::new(&path).join("nodes.jsonl")).unwrap();
    for op in [
        umx_protocol::Op::EdgeDelete { edge: a.clone() },
        umx_protocol::Op::GraphUnload {
            alias: "demo".into(),
        },
        umx_protocol::Op::Check {
            rules: false,
            rule: None,
            after: None,
            lint: false,
            sources: false,
            source_budget: None,
            kind: None,
            offset: None,
        },
    ] {
        let mut request = Request::op(op, None, None, older_cli());
        request.include_provenance = true;
        let (env, _) = raw(&home.socket(), &request);
        assert_eq!(env.errors[0].code(), "usage_invalid", "{}", env.to_json());
    }
    assert_eq!(
        fs::read(Path::new(&path).join("nodes.jsonl")).unwrap(),
        before
    );
}

/// ws006 S0 (req-15 in part): one fixture per status of `umx resolve`. Read-only: nothing loads (on-demand load of a
/// registered graph comes through F72; graphs that are not registered keep graph_not_loaded).
#[test]
fn resolve_gives_each_status() {
    let mut home = Home::new("resolve-statuses");
    home.start();
    resolve_fixture(&mut home, "rsv");

    // ok: an 8-character prefix gives the full id, as an address or a bare id in the only graph.
    // The scheme is case-insensitive (UMX:, Umx:); alias and id are not folded.
    for given in [
        "umx:rsv/01RSAAAA",
        "01rsaaaa",
        "UMX:rsv/01RSAAAA",
        "Umx:rsv/01rsaaaa",
    ] {
        let env = home.ok(&["resolve", given]);
        assert_eq!(env.data["status"], "ok", "{given}");
        assert_eq!(env.data["address"], format!("umx:rsv/{A1}"));
        assert_eq!(
            (env.data["kind"].as_str(), env.data["node"]["id"].as_str()),
            (Some("node"), Some(A1))
        );
        assert_eq!(env.data["graph_id"], "01RG0000000000000000000001");
    }
    // #<field> gives its value; a selector comes back unread.
    let env = home.ok(&["resolve", &format!("umx:rsv/{A1}#problem")]);
    assert_eq!(
        (env.data["field"].as_str(), env.data["value"].as_str()),
        (Some("problem"), Some("Approval lag"))
    );
    let env = home.ok(&["resolve", &format!("umx:rsv/{A1}#title")]);
    assert_eq!(env.data["value"], "Launch in March");
    let env = home.ok(&["resolve", &format!("umx:rsv/{A1}#seq=1-2")]);
    assert_eq!(
        (env.data["selector"].as_str(), env.data["read"].as_bool()),
        (Some("seq=1-2"), Some(false))
    );
    assert!(env.data["value"].is_null());
    // A full edge id resolves to the edge.
    let env = home.ok(&["resolve", "umx:rsv/01RSEE00000000000000000001#type"]);
    assert_eq!(
        (env.data["kind"].as_str(), env.data["value"].as_str()),
        (Some("edge"), Some("blocks"))
    );
    assert_eq!(env.data["edge"]["from"], "01RSBBBB000000000000000001");
    // ?repo= never changes what the address names.
    let env = home.ok(&[
        "resolve",
        &format!("umx:rsv/{A1}?repo=https://example.invalid/x.git"),
    ]);
    assert_eq!(
        (env.data["node"]["id"].as_str(), env.data["repo"].as_str()),
        (Some(A1), Some("https://example.invalid/x.git"))
    );

    // ambiguous: a prefix two nodes share, with both as candidates.
    let (env, exit) = home.umx(&["resolve", "umx:rsv/01RSBBBB"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let ids: Vec<&str> = issue(&env, Code::IdAmbiguous)
        .candidates()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["01RSBBBB000000000000000001", "01RSBBBB000000000000000002"]
    );
    // node_not_found and field_not_found.
    let (env, exit) = home.umx(&["resolve", "umx:rsv/01RSZZZZZZZZZZZZZZZZZZZZZZ"]);
    assert_eq!(exit, 1);
    issue(&env, Code::NodeNotFound);
    let (env, exit) = home.umx(&["resolve", &format!("umx:rsv/{A1}#nope")]);
    assert_eq!(exit, 1);
    let missing = issue(&env, Code::FieldNotFound);
    assert_eq!(missing.next(), format!("umx node get umx:rsv/{A1}"));
    let (env, exit) = home.umx(&["resolve", "umx:rsv/01RSAAAA#Bad"]);
    assert_eq!(exit, 2, "{}", env.to_json());
    // graph-addressing-expert F1: one grammar. Resolve refuses what evidence refuses, and returns the canonical
    // selector.
    for bad in ["#t=1-2", "#seq=3-1", "#Problem"] {
        let (env, exit) = home.umx(&["resolve", &format!("umx:rsv/{A1}{bad}")]);
        assert_eq!(exit, 2, "{bad}: {}", env.to_json());
        issue(&env, Code::UsageInvalid);
    }
    let env = home.ok(&["resolve", &format!("umx:rsv/{A1}#quote=we ship&L7-L7")]);
    assert_eq!(env.data["selector"], "L7&quote=we%20ship");
    // The copy form is planned (req-69): refused, naming the copy's own alias, never graph_not_loaded.
    let (env, exit) = home.umx(&["resolve", &format!("umx:rsv@main/{A1}")]);
    assert_eq!(exit, 2, "{}", env.to_json());
    let refused = issue(&env, Code::UsageInvalid);
    assert!(
        refused.message().contains("own alias"),
        "{}",
        refused.message()
    );

    // graph_not_loaded: an unregistered alias. A plain repo hint is named and next loads the graph once cloned; a
    // hint that could add a git option is never shown.
    let (env, exit) = home.umx(&["resolve", "umx:nowhere/01RSAAAA"]);
    assert_eq!(exit, 1);
    assert_eq!(issue(&env, Code::GraphNotLoaded).next(), "umx graph list");
    let (env, _) = home.umx(&[
        "resolve",
        "umx:nowhere/01RSAAAA?repo=git@github.com:org/x.git",
    ]);
    let hinted = issue(&env, Code::GraphNotLoaded);
    assert!(
        hinted
            .message()
            .contains("git clone git@github.com:org/x.git"),
        "{}",
        hinted.message()
    );
    assert_eq!(hinted.next(), "umx graph load <path> --as nowhere");
    let (env, _) = home.umx(&[
        "resolve",
        "umx:nowhere/01RSAAAA?repo=https://x/y --upload-pack=touch",
    ]);
    let hostile = issue(&env, Code::GraphNotLoaded);
    assert!(
        !hostile.message().contains("upload") && !hostile.next().contains("upload"),
        "{}",
        env.to_json()
    );
    let listed = home.ok(&["graph", "list"]);
    assert_eq!(
        listed.data["rows"].as_array().unwrap().len(),
        1,
        "resolve loaded nothing"
    );

    // copy_ambiguous: copies with none primary and no graph named; an address names its copy and is never ambiguous.
    resolve_fixture(&mut home, "rsv_b");
    resolve_fixture(&mut home, "rsv_c");
    home.ok(&["graph", "unload", "rsv"]);
    let (env, exit) = home.umx(&["resolve", "01RSAAAA"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::CopyAmbiguous);
    let env = home.ok(&["resolve", "umx:rsv_c/01RSAAAA"]);
    assert_eq!(
        (env.data["graph"].as_str(), env.data["address"].as_str()),
        (Some("rsv_c"), Some(&*format!("umx:rsv_c/{A1}")))
    );
}

/// F185: `resource show` reads like `node get`. A provenance record the caller's scope hides is not found, with the
/// same error byte for byte, so the verb never says a hidden id exists (it used to answer `not_a_resource`).
#[test]
fn resource_show_never_finds_a_hidden_provenance_record() {
    let mut home = Home::new("resource-show-hidden");
    home.graph();
    let a = home.add("problem", "First content");
    let node = home.ok(&["node", "get", &a]);
    let session = node.data["node"]["authored_by_session"]
        .as_str()
        .unwrap()
        .to_owned();
    let (shown, shown_exit) = home.umx(&["resource", "show", &session]);
    let (got, got_exit) = home.umx(&["node", "get", &session]);
    assert_eq!((shown_exit, got_exit), (1, 1), "{}", shown.to_json());
    assert_eq!(
        shown.errors[0].code(),
        "node_not_found",
        "{}",
        shown.to_json()
    );
    assert_eq!(
        json!(shown.errors).to_string(),
        json!(got.errors).to_string()
    );
}

/// F216: the verbs that resolve a node with the write resolver (`resolve_node`) answer a hidden provenance record's
/// id exactly as `node get` does, `node_not_found` with the same error bytes, so none of them says a hidden id exists:
/// node edit (`node_in`), node add --parent, embeddings allow (`allow_secret`), edge add at either end, by address in
/// its own graph and in another loaded graph (`write_end`), and ui focus (`ui_target`).
#[test]
fn write_resolver_verbs_never_find_a_hidden_provenance_record() {
    let mut home = Home::new("write-resolver-hidden");
    home.graph();
    let a = home.add("problem", "First content");
    let node = home.ok(&["node", "get", &a]);
    let session = node.data["node"]["authored_by_session"]
        .as_str()
        .unwrap()
        .to_owned();
    let address = format!("umx:demo/{session}");
    let mut differ = Vec::new();
    for (text, words) in [
        (
            &session,
            vec!["node", "edit", &session, "--title", "Renamed"],
        ),
        (
            &session,
            vec![
                "node", "add", "--type", "problem", "--title", "Child", "--parent", &session,
            ],
        ),
        (
            &session,
            vec!["embeddings", "allow", "--node", &session, "--field", "note"],
        ),
        (&session, vec!["edge", "add", &a, "supports", &session]),
        (&session, vec!["edge", "add", &session, "supports", &a]),
        (&address, vec!["edge", "add", &a, "supports", &address]),
        (&session, vec!["ui", "focus", &session]),
    ] {
        let (got, got_exit) = home.umx(&["node", "get", text]);
        let (env, exit) = home.umx(&words);
        let answer = (exit, json!(env.errors).to_string());
        if got_exit != 1 || answer != (got_exit, json!(got.errors).to_string()) {
            differ.push(format!("{words:?}: {}", env.to_json()));
        }
    }
    assert!(differ.is_empty(), "{differ:#?}");
    // A second loaded graph: an edge end that names a node in it (`write_end`'s other-graph arm).
    let other_path = home.path.join("other").display().to_string();
    home.ok(&["graph", "new", &other_path, "--alias", "other"]);
    let b = home.ok(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Other content",
        "--graph",
        "other",
    ]);
    let b = b.data["node"]["id"].as_str().unwrap().to_owned();
    let theirs = home.ok(&["node", "get", &format!("umx:other/{b}")]);
    let theirs = format!(
        "umx:other/{}",
        theirs.data["node"]["authored_by_session"].as_str().unwrap()
    );
    let (got, _) = home.umx(&["node", "get", &theirs]);
    let (env, exit) = home.umx(&["edge", "add", &a, "supports", &theirs, "--graph", "demo"]);
    assert_eq!(got.errors[0].code(), "node_not_found", "{}", got.to_json());
    if (exit, json!(env.errors).to_string()) != (1, json!(got.errors).to_string()) {
        differ.push(format!("edge add to {theirs}: {}", env.to_json()));
    }
    assert!(differ.is_empty(), "{differ:#?}");
    let (got, _) = home.umx(&["node", "get", &session, "--graph", "demo"]);
    assert_eq!(got.errors[0].code(), "node_not_found", "{}", got.to_json());
}
