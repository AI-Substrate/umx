//! The cli tests: search, query and the read verbs over several graphs.
//! A module of cli.rs, whose `Home` and helpers it uses, in its own file so no file nears the seed gate's
//! size cap.

use super::*;

#[test]
fn query_verbs_match_the_hand_written_oracle_on_the_fixture() {
    // req-28 done_when: each verb against the fixture graph, compared with expected results worked out by hand
    // from the edge list above (Cypher, req-27, is not built yet, so there is no query to compare with).
    let home = query_fixture("query");
    let run = |args: &[&str]| {
        let (env, exit) = home.umx(args);
        assert_eq!(exit, 0, "{args:?}: {}", env.to_json());
        env
    };
    let ids = |v: &[u8]| v.iter().map(|i| n(*i)).collect::<Vec<_>>();
    let edges = |v: &[u8]| v.iter().map(|i| e(*i)).collect::<Vec<_>>();

    // neighbours: out before in, each in edge id order; similar_to only when named.
    let env = run(&["neighbours", &n(1)]);
    assert_eq!(column(&env, "id"), ids(&[8, 2, 4]));
    assert_eq!(column(&env, "way"), ["out", "in", "in"]);
    assert_eq!(column(&env, "edge"), edges(&[6, 1, 3]));
    assert_eq!(
        column(&env, "relation"),
        ["part_of", "blocks", "criterion_for"]
    );
    let env = run(&["neighbours", &format!("umx:fixture/{}", n(2))]);
    assert_eq!(column(&env, "id"), ids(&[1, 3, 8, 5]));
    assert_eq!(column(&env, "title")[0], "Launch in March");
    let env = run(&["neighbours", &n(2), "--relation", "similar_to"]);
    assert_eq!(column(&env, "id"), ids(&[10]));
    assert_eq!(
        column(&run(&["neighbours", &n(2), "--direction", "in"]), "id"),
        ids(&[5])
    );
    let lower = n(2).to_lowercase();
    let env = run(&[
        "neighbours",
        &lower,
        "--relation",
        "blocks",
        "--direction",
        "out",
    ]);
    assert_eq!(column(&env, "id"), ids(&[1]));
    assert_eq!(
        column(&run(&["neighbours", &n(9)]), "id"),
        Vec::<String>::new()
    );

    // walk: breadth first, each node once at its shortest depth; out by default.
    let env = run(&["walk", &n(6)]);
    assert_eq!(column(&env, "id"), ids(&[5]));
    let env = run(&["walk", &n(6), "--depth", "3"]);
    assert_eq!(column(&env, "id"), ids(&[5, 2, 7, 1, 3, 8]));
    assert_eq!(column(&env, "depth"), ["1", "2", "2", "3", "3", "3"]);
    assert_eq!(column(&env, "from"), ids(&[6, 5, 5, 2, 2, 2]));
    let env = run(&["walk", &n(6), "--depth", "3", "--within", &n(7)]);
    assert_eq!(
        column(&env, "id"),
        ids(&[5, 2, 1, 8]),
        "only nodes under Programme"
    );
    let env = run(&["walk", &n(1), "--depth", "2", "--direction", "in"]);
    assert_eq!(column(&env, "id"), ids(&[2, 4, 5, 3]));
    assert_eq!(column(&env, "from"), ids(&[1, 1, 2, 4]));
    assert_eq!(column(&env, "way"), ["in", "in", "in", "in"]);
    let env = run(&["walk", &n(2), "--direction", "both"]);
    assert_eq!(column(&env, "id"), ids(&[1, 3, 8, 5]));
    let env = run(&["walk", &n(1), "--depth", "6", "--relation", "part_of"]);
    assert_eq!(column(&env, "id"), ids(&[8, 7]));
    // F41: a walk stops once its page is full and one more row is known; then `total` is null, never a guess.
    let env = run(&[
        "walk",
        &n(6),
        "--depth",
        "3",
        "--limit",
        "2",
        "--offset",
        "1",
    ]);
    assert_eq!(column(&env, "id"), ids(&[2, 7]));
    assert_eq!(
        (&env.data["total"], &env.data["truncated"]),
        (&Value::Null, &true.into())
    );
    let env = run(&[
        "walk",
        &n(6),
        "--depth",
        "3",
        "--limit",
        "5",
        "--offset",
        "1",
    ]);
    assert_eq!(
        (&env.data["total"], &env.data["truncated"]),
        (&6.into(), &false.into())
    );
    let text = home.human(&["walk", &n(6), "--depth", "3", "--limit", "2"]);
    assert!(
        text.ends_with("showing 2; more follow (page on with --offset)\n"),
        "{text}"
    );

    // path: shortest, edges named, either way by default.
    let env = run(&["path", &n(6), &n(1)]);
    assert_eq!(column(&env, "id"), ids(&[6, 5, 2, 1]));
    assert_eq!(column(&env, "step"), ["0", "1", "2", "3"]);
    assert_eq!(column(&env, "edge"), ["null".to_string(), e(5), e(4), e(1)]);
    assert_eq!(column(&env, "way"), ["null", "out", "out", "out"]);
    let env = run(&["path", &n(1), &n(6)]);
    assert_eq!(column(&env, "id"), ids(&[1, 2, 5, 6]));
    assert_eq!(column(&env, "way"), ["null", "in", "in", "in"]);
    assert_eq!(
        column(&env, "relation"),
        ["null", "blocks", "answers", "undercuts"]
    );
    let (n1, n6, n9, n10) = (n(1), n(6), n(9), n(10));
    for (args, why) in [
        (
            vec!["path", n1.as_str(), n6.as_str(), "--direction", "out"],
            "no path along edge direction",
        ),
        (vec!["path", n1.as_str(), n9.as_str()], "the island"),
        (
            vec!["path", n1.as_str(), n10.as_str()],
            "only similar_to reaches it",
        ),
        (
            vec!["path", n6.as_str(), n1.as_str(), "--max-depth", "2"],
            "three edges are needed",
        ),
    ] {
        let env = run(&args);
        assert_eq!(
            (env.data["total"].as_u64(), column(&env, "id").len()),
            (Some(0), 0),
            "{why}"
        );
    }
    let env = run(&["path", &n(2), &n(10), "--relation", "similar_to"]);
    assert_eq!(column(&env, "id"), ids(&[2, 10]));
    assert_eq!(column(&run(&["path", &n(1), &n(1)]), "id"), ids(&[1]));

    // --within on node find; node list keeps its own.
    let all = column(&run(&["node", "find", "approval"]), "id");
    assert_eq!(all.len(), 2, "{all:?}");
    let env = run(&["node", "find", "approval", "--within", &n(7)]);
    assert_eq!(column(&env, "id"), ids(&[2]));
    let env = run(&["node", "list", "--within", &n(7)]);
    assert_eq!(column(&env, "id"), ids(&[1, 2, 5, 8]));
}

#[test]
fn query_verbs_refuse_what_they_cannot_answer() {
    let home = query_fixture("query-bad");
    let (env, exit) = home.umx(&["walk", &n(1), "--depth", "0"]);
    assert_eq!(exit, 2, "{}", env.to_json());
    assert_eq!(
        issue(&env, Code::UsageInvalid).next(),
        "umx docs get explore"
    );
    let (env, _) = home.umx(&["neighbours", &n(1), "--direction", "sideways"]);
    issue(&env, Code::UsageInvalid);
    let (env, _) = home.umx(&["path", &n(1)]);
    issue(&env, Code::UsageInvalid);
    let (env, exit) = home.umx(&["neighbours", "01QNZZZZZZ"]);
    assert_eq!(exit, 1);
    issue(&env, Code::NodeNotFound);
    let (env, _) = home.umx(&[
        "path",
        &format!("umx:fixture/{}", n(1)),
        &format!("umx:other/{}", n(2)),
    ]);
    issue(&env, Code::GraphNotLoaded);
    let table = home.human(&["neighbours", &n(1)]);
    assert!(
        table.contains("Workstream north") && table.contains("relation"),
        "{table}"
    );
}

/// req-92 done_when, with no embedder configured: `vendor` finds "Pre-approve listed vendors", `vendors` finds
/// "vendor", `approv` finds approve, approval and pre-approve; exact outranks stem outranks prefix; each hit says how
/// its words matched, in JSON and as `vendor ~ vendors (stem)` in human output.
#[test]
fn search_matches_stems_and_prefixes_and_says_how() {
    let mut home = Home::new("stems");
    home.graph();
    let pre = home.add("problem", "Pre-approve listed vendors");
    let vendor = home.add("problem", "Vendor onboarding");
    let approve = home.add("decision", "Approve the budget");
    let approval = home.add("problem", "Approval delay");
    let ship = home.add("problem", "Vendorship terms");
    let ids = |env: &Envelope| -> Vec<String> {
        env.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let found = home.ok(&["search", "vendor"]);
    assert_eq!(ids(&found), [vendor.clone(), pre.clone(), ship.clone()]);
    let forms: Vec<&Value> = found.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| &r["word_terms"][0])
        .collect();
    assert_eq!(
        forms,
        [
            &json!({"query": "vendor", "term": "vendor", "form": "exact"}),
            &json!({"query": "vendor", "term": "vendors", "form": "stem"}),
            &json!({"query": "vendor", "term": "vendorship", "form": "prefix"}),
        ]
    );
    assert_eq!(
        ids(&home.ok(&["search", "vendors"]))[..2],
        [pre.clone(), vendor.clone()]
    );
    let mut approv = ids(&home.ok(&["search", "approv"]));
    approv.sort();
    let mut want = vec![pre.clone(), approve, approval];
    want.sort();
    assert_eq!(approv, want);
    let (human, _) = output_with(&home, &["search", "vendor", "--human"], &[]);
    assert!(human.contains("vendor ~ vendors (stem)"), "{human}");
    assert!(human.contains("vendor ~ vendorship (prefix)"), "{human}");
}

/// req-68: `umx search` finds a node by words in its note, with score and field, and a title match outranks it;
/// a field declared as text is searchable from that write on; several graphs are searched as one.
#[test]
fn search_finds_note_words_ranks_titles_first_and_spans_graphs() {
    let mut home = Home::new("search");
    home.graph();
    let (noted, exit) = home.umx(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Vendor onboarding",
        "--note",
        "Legal sign-off: the approval delay hits every vendor.",
    ]);
    assert_eq!(exit, 0, "{}", noted.to_json());
    let noted = noted.data["node"]["id"].as_str().unwrap().to_owned();
    let titled = home.add("problem", "Approval delay");
    home.add("decision", "Launch date");

    let found = home.ok(&["search", "approval", "delay"]);
    let rows = found.data["rows"].as_array().unwrap();
    let got: Vec<(&str, &str, &str)> = rows
        .iter()
        .map(|r| {
            (
                r["id"].as_str().unwrap(),
                r["word_field"].as_str().unwrap(),
                r["graph"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (titled.as_str(), "title", "demo"),
            (noted.as_str(), "note", "demo")
        ]
    );
    assert!(rows[0]["bm25"].as_f64().unwrap() > rows[1]["bm25"].as_f64().unwrap());

    // A field declared as a string is indexed inside that ontology write.
    home.ok(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Pump",
        "--set",
        "detail=cavitation",
    ]);
    // Undeclared, the field is an exact-match column (req-30): its whole value finds the node, words do not.
    let undeclared = home.ok(&["search", "cavitation"]);
    let row = &undeclared.data["rows"][0];
    assert_eq!(
        (&row["legs"], &row["exact"]),
        (&json!("exact"), &json!("detail"))
    );
    home.ok(&["ontology", "add", "field", "problem", "detail:string"]);
    let declared = home.ok(&["search", "cavitation"]);
    let row = &declared.data["rows"][0];
    assert_eq!(
        (&row["word_field"], &row["exact"]),
        (&json!("detail"), &json!(null))
    );

    // Two graphs: --graph names them; with none given, every loaded graph.
    let ops = home.path.join("ops").display().to_string();
    home.ok(&["graph", "new", &ops, "--alias", "ops"]);
    home.ok(&[
        "node",
        "add",
        "--graph",
        "ops",
        "--type",
        "problem",
        "--title",
        "Approval delay",
    ]);
    for args in [
        &["search", "approval", "delay", "--graph", "demo,ops"][..],
        &["search", "approval", "delay"],
    ] {
        let both = home.ok(args);
        let graphs: Vec<&str> = both.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["graph"].as_str().unwrap())
            .collect();
        assert_eq!(graphs, vec!["demo", "ops", "demo"], "{args:?}");
    }

    let (none, exit) = home.umx(&["search", "?!"]);
    assert_eq!(exit, 2);
    issue(&none, Code::UsageInvalid);

    // R83-3: a --graph list that names no graph is a usage error, with or without --within.
    for args in [
        &["search", "approval", "--graph", ","][..],
        &["search", "approval", "--graph", ",", "--within", &titled],
    ] {
        let (env, exit) = home.umx(args);
        assert_eq!(exit, 2, "{args:?}: {}", env.to_json());
        issue(&env, Code::UsageInvalid);
    }

    // R83-2: in list mode --within must be an address naming one of the listed graphs; the scope is that graph's.
    let root = home.ok(&[
        "node",
        "add",
        "--graph",
        "demo",
        "--type",
        "problem",
        "--title",
        "Scope root",
    ]);
    let root = root.data["node"]["id"].as_str().unwrap().to_owned();
    let child = home.ok(&[
        "node",
        "add",
        "--graph",
        "demo",
        "--type",
        "problem",
        "--title",
        "Scoped approval",
        "--parent",
        &root,
    ]);
    let child = child.data["node"]["id"].as_str().unwrap().to_owned();
    let (wrong, exit) = home.umx(&[
        "search",
        "approval",
        "--graph",
        "demo,ops",
        "--within",
        &format!("umx:never_loaded/{root}"),
    ]);
    assert_eq!(exit, 1, "{}", wrong.to_json());
    issue(&wrong, Code::GraphMismatch);
    let (bare, exit) = home.umx(&[
        "search", "approval", "--graph", "demo,ops", "--within", &root,
    ]);
    assert_eq!(exit, 2, "{}", bare.to_json());
    issue(&bare, Code::UsageInvalid);
    let scoped = home.ok(&[
        "search",
        "approval",
        "--graph",
        "demo,ops",
        "--within",
        &format!("umx:demo/{root}"),
    ]);
    let ids: Vec<&str> = scoped.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![child.as_str()]);
}

/// SEC-5 (blueprint d-20): a search that names no graph covers only open graphs and counts the secured ones it left
/// out, never naming them; naming a secured graph searches it, and its hits say so.
#[test]
fn an_unnamed_search_skips_secured_graphs_and_names_none() {
    let mut home = Home::new("search-secured");
    home.graph();
    let open = home.add("problem", "Approval delay");
    let sec = home.path.join("sec").display().to_string();
    home.ok(&["graph", "new", &sec, "--alias", "vault", "--secured"]);
    let secret = home.ok(&[
        "node",
        "add",
        "--graph",
        "vault",
        "--type",
        "problem",
        "--title",
        "Approval delay",
    ]);
    let secret = secret.data["node"]["id"].as_str().unwrap().to_owned();

    let (env, exit) = home.umx(&["search", "approval", "delay"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let rows = env.data["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{}", env.to_json());
    assert_eq!(
        (rows[0]["id"].as_str(), rows[0]["policy"].as_str()),
        (Some(open.as_str()), Some("open"))
    );
    let note = hint_of(&env, Code::SecuredGraphsSkipped);
    let text = env.to_json();
    assert!(!text.contains("vault") && !text.contains(&secret), "{text}");
    assert!(
        note.message().starts_with("1 secured graphs"),
        "{}",
        note.message()
    );

    let named = home.ok(&["search", "approval", "delay", "--graph", "vault"]);
    let rows = named.data["rows"].as_array().unwrap();
    assert_eq!(
        (
            rows[0]["id"].as_str(),
            rows[0]["graph"].as_str(),
            rows[0]["policy"].as_str()
        ),
        (Some(secret.as_str()), Some("vault"), Some("secured"))
    );
    assert!(
        !named
            .hints
            .iter()
            .chain(&named.warnings)
            .any(|w| w.code() == "secured_graphs_skipped")
    );
    let both = home.ok(&["search", "approval", "delay", "--graph", "demo,vault"]);
    assert_eq!(both.data["rows"].as_array().unwrap().len(), 2);
}

#[test]
fn all_graph_read_flags_work_on_search_and_query_and_reject_bad_combinations() {
    let mut home = Home::new("all-graph-reads");
    home.secured_graph();
    home.add("problem", "Quarterly named note");
    let open = home.path.join("open").display().to_string();
    home.ok(&["graph", "new", &open, "--alias", "open"]);
    home.ok(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Quarterly open note",
        "--graph",
        "open",
    ]);
    for (verb, text) in [
        ("search", "quarterly"),
        (
            "query",
            "MATCH (n) RETURN n.graph AS graph, n.title AS title",
        ),
    ] {
        let (plain, exit) = home.umx(&[verb, text]);
        assert_eq!(exit, 0, "{}", plain.to_json());
        let (all, exit) = home.umx_with(&[verb, text, "--all"], &[("UMX_GRAPH", "demo")]);
        assert_eq!(exit, 0, "{}", all.to_json());
        assert_eq!(all, plain);
        assert!(
            all.data["rows"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["graph"] == "open")
        );
        let skipped = hint_of(&all, Code::SecuredGraphsSkipped);
        for value in [skipped.message(), skipped.next()] {
            assert!(
                !value.contains("include-named") && !value.contains("--all"),
                "{value}"
            );
        }
        assert!(skipped.next().contains("--graph"));
        let (named, exit) = home.umx(&[verb, text, "--all", "--include-named"]);
        assert_eq!(exit, 0, "{}", named.to_json());
        assert!(
            named.data["rows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["graph"] == "demo")
        );
        let read = issue(&named, Code::SecuredGraphsRead);
        assert_eq!(read.candidates()[0]["id"], "demo");
        assert!(read.message().contains("now in this output"));
        for flags in [
            vec!["--include-named"],
            vec!["--all", "--graph", "open"],
            vec!["--all", "--scope", "saved"],
        ] {
            let mut args = vec![verb, text];
            args.extend(flags);
            let (env, exit) = home.umx(&args);
            assert_eq!(exit, 2, "{args:?}: {}", env.to_json());
            assert_eq!(env.errors[0].code(), "usage_invalid");
        }
    }
}

/// Every query of the design fixture through the CLI and a private daemon: the supported ones give their expected
/// rows as JSON, the refused ones their code with a runnable next, the soft-vocabulary ones their warning.
#[test]
fn umx_query_answers_the_design_fixture_end_to_end() {
    let mut home = Home::new("query-design");
    home.design();
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../umx-cypher/fixtures/design/queries.json");
    let queries = umx_protocol::parse_json(&fs::read_to_string(path).unwrap()).unwrap();
    let (mut answered, mut refused) = (0, 0);
    for q in queries.as_array().unwrap() {
        let id = q["id"].as_str().unwrap();
        if q.get("umx_from").is_some() {
            continue; // a later step's query (w07, s06)
        }
        let (env, exit) = home.query(q["cypher"].as_str().unwrap(), &[]);
        if let Some(code) = q["umx_error"].as_str() {
            assert_eq!(
                (env.status, exit),
                (Status::Error, 1),
                "{id}: {}",
                env.to_json()
            );
            assert_eq!(env.errors[0].code(), code, "{id}");
            assert!(env.errors[0].next().starts_with("umx "), "{id}");
            refused += 1;
            continue;
        }
        assert_eq!(exit, 0, "{id}: {}", env.to_json());
        if let Some(code) = q["umx_warning"].as_str() {
            issue(&env, Code::from_code(code).unwrap());
        }
        if let Some(expected) = q["expected"].as_array() {
            let mut want: Vec<Vec<String>> = expected
                .iter()
                .map(|r| r.as_array().unwrap().iter().map(query_cell).collect())
                .collect();
            let mut got = query_rows(&env);
            if q["ordered"] != true {
                want.sort();
                got.sort();
            }
            assert!(
                rows::rows_match(&got, &want),
                "{id}: got {got:?}, want {want:?}"
            );
            assert_eq!(env.data["total"], json!(want.len()), "{id}");
            answered += 1;
        }
    }
    assert!(
        answered >= 44 && refused >= 10,
        "{answered} answered, {refused} refused"
    );

    // A whole node and a whole edge come back in umx's shapes.
    let q2 = "01JDCYPHER00000000000000Q2";
    let (env, _) = home.query(
        &format!("MATCH (n {{id: '{q2}'}})<-[r:answers]-(d) RETURN n, r"),
        &[],
    );
    let row = &env.data["rows"][0];
    let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(
        keys(&row["n"]),
        ["address", "graph", "id", "status", "title", "type"]
    );
    assert_eq!(row["n"]["address"], format!("umx:design/{q2}"));
    assert_eq!(
        keys(&row["r"]),
        ["address", "from", "graph", "id", "to", "type"]
    );
    assert_eq!(
        (&row["r"]["type"], &row["r"]["to"]),
        (&json!("answers"), &json!(q2))
    );
}

#[test]
fn umx_query_unused_warning_envelopes_keep_empty_rows_and_ontology_next() {
    for code in [
        Code::QueryLabelUnused,
        Code::QueryRelationUnused,
        Code::QueryPropertyUnused,
    ] {
        let (env, exit) = trigger(code);
        assert_eq!(
            (env.status, exit),
            (Status::Warning, 0),
            "{}",
            env.to_json()
        );
        assert_eq!(env.data["rows"], json!([]));
        let warning = issue(&env, code);
        assert_eq!(warning.next(), "umx ontology show");
        assert!(!warning.message().contains('?'), "{}", env.to_json());
    }
}

/// `--param` values are bound as literals: `{id: $id}` costs what the literal does (the id index, no scan), far
/// less than reading every node.
#[test]
fn umx_query_params_push_down_like_literals() {
    let mut home = Home::new("query-param");
    home.design();
    let d1 = "01JDCYPHER00000000000000D1";
    let work = |text: &str, flags: &[&str]| {
        let (env, exit) = home.query(text, flags);
        assert_eq!(exit, 0, "{}", env.to_json());
        assert_eq!(env.data["rows"][0]["t"], "Launch in March");
        env.data["work"].as_u64().unwrap()
    };
    let param = work(
        "MATCH (n {id: $id}) RETURN n.title AS t",
        &["--param", &format!("id={d1}")],
    );
    let literal = work(
        &format!("MATCH (n {{id: '{d1}'}}) RETURN n.title AS t"),
        &[],
    );
    let scan = work(
        "MATCH (n) WHERE n.title = $t RETURN n.title AS t",
        &["--param", "t=Launch in March"],
    );
    assert_eq!(param, literal);
    assert!(param < scan, "{param} vs a scan's {scan}");
    // A value typed as --set types it: a whole number compares as a number.
    let (env, _) = home.query(
        "MATCH (n) WHERE n.likelihood >= $least RETURN n.id AS id ORDER BY id",
        &["--param", "least=3"],
    );
    assert_eq!(query_rows(&env), [["01JDCYPHER00000000000000P1"]]);
    let (env, exit) = home.query(
        "MATCH (n {id: $id}) RETURN n.id AS id",
        &["--param", "id=a", "--param", "id=b"],
    );
    assert_eq!((exit, env.errors[0].code()), (2, "usage_invalid"));
}

/// similar_to end to end (ws005 s05): committed vectors, `umx similarity rebuild` (no embedder needed), then a query
/// naming similar_to. JSON: the derived edge with null id and address, and the lists' model, k and floor. Human: the
/// edge as `<from> -[similar_to <score>]-> <to>`, never its nulls.
#[test]
fn umx_query_reads_similar_to_lists_in_json_and_human_output() {
    let mut home = Home::new("query-similar");
    home.start();
    let path = home.path.join("g");
    let p = path.display().to_string();
    home.ok(&[
        "graph",
        "new",
        &p,
        "--alias",
        "demo",
        "--embedding-model",
        "m@4",
    ]);
    let a = home.add("problem", "Slow builds");
    let b = home.add("problem", "Slow test runs");
    let c = home.add("problem", "Billing errors");
    // Whole-node vectors, as an embedder would have committed them: a and b point the same way, c elsewhere.
    let space = path.join("embeddings/m@4");
    fs::create_dir_all(&space).unwrap();
    let mut shards: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (id, values) in [(&a, "ZAAAAA=="), (&b, "WigAAA=="), (&c, "AABkAA==")] {
        let tail = id[id.len() - 2..].to_lowercase();
        let line = format!("{id}\t@node.v1\t{}\t{values}", "0".repeat(32));
        shards.entry(tail).or_default().push(line);
    }
    for (tail, mut lines) in shards {
        lines.sort();
        fs::write(space.join(format!("{tail}.tsv")), lines.join("\n") + "\n").unwrap();
    }
    // m@4 is not calibrated (no floor): the graph's own floor keeps c's 0.0 matches out.
    let file = path.join("graph.json");
    let mut v: Value = umx_protocol::parse_json(&fs::read_to_string(&file).unwrap()).unwrap();
    v["embedding"]["similarity"] = json!({ "floor": 0.5 });
    fs::write(&file, v.to_string()).unwrap();
    home.ok(&["graph", "unload", "demo"]);
    home.ok(&["graph", "load", &p]);
    home.ok(&["similarity", "rebuild", "--graph", "demo"]);

    let text = "MATCH (x)-[s:similar_to]->(y) RETURN x.id AS from_id, s, s.score AS score ORDER BY from_id";
    let env = home.ok(&["query", text, "--graph", "demo"]);
    let rows = env.data["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{}", env.to_json());
    let first = &rows[0]["s"];
    let (low, high) = if a < b { (&a, &b) } else { (&b, &a) };
    assert_eq!(rows[0]["from_id"], low.as_str());
    assert_eq!(
        (
            &first["id"],
            &first["address"],
            &first["derived"],
            &first["type"]
        ),
        (
            &json!(null),
            &json!(null),
            &json!(true),
            &json!("similar_to")
        )
    );
    assert_eq!(
        (first["from"].as_str(), first["to"].as_str()),
        (Some(low.as_str()), Some(high.as_str()))
    );
    assert_eq!(first["score"], rows[0]["score"]);
    assert!(first["score"].as_f64().unwrap() > 0.9, "{first}");
    assert_eq!(env.data["similarity"][0]["graph"], "demo");
    assert_eq!(env.data["similarity"][0]["model"], "m@4");
    assert_eq!(env.data["similarity"][0]["floor_calibrated"], false);
    assert_eq!(env.data["similarity"][0]["floor_source"], "graph");
    assert!(env.data["similarity"][0]["k"].as_u64().is_some());
    assert!(env.warnings.is_empty(), "{}", env.to_json());
    // Not named: no similarity section, and c (no list entries) is in no row either way.
    let plain = home.ok(&[
        "query",
        "MATCH (n) RETURN count(*) AS nodes",
        "--graph",
        "demo",
    ]);
    assert!(
        plain.data.get("similarity").is_none(),
        "{}",
        plain.to_json()
    );

    let shown = home.human(&["query", text, "--graph", "demo"]);
    let score = first["score"].to_string();
    assert!(
        shown.contains(&format!("{low} -[similar_to {score}]-> {high}")),
        "{shown}"
    );
    assert!(
        !shown.contains("null") && !shown.contains("address"),
        "{shown}"
    );
}

/// d-20 end to end: an unnamed query reads the open graph and only counts the secured one, naming it nowhere in
/// the envelope; naming the secured graph reads it; a comma list reads both.
#[test]
fn umx_query_reads_secured_graphs_only_when_named() {
    let mut home = Home::new("query-d20");
    home.start();
    for (alias, secured) in [("notes", false), ("vault", true)] {
        let path = home.path.join(alias).display().to_string();
        let mut args = vec!["graph", "new", path.as_str(), "--alias", alias];
        if secured {
            args.push("--secured");
        }
        let (env, exit) = home.umx(&args);
        assert_eq!(exit, 0, "{}", env.to_json());
        let title = format!("{alias} thing");
        let (env, exit) = home.umx(&[
            "node", "add", "--graph", alias, "--type", "problem", "--title", &title,
        ]);
        assert_eq!(exit, 0, "{}", env.to_json());
    }
    let text = "MATCH (n) RETURN n.graph AS g, n.title AS t ORDER BY g";
    let (env, _) = home.query(text, &[]);
    assert_eq!(query_rows(&env), [["notes", "notes thing"]]);
    assert!(
        hint_of(&env, Code::SecuredGraphsSkipped)
            .message()
            .contains('1')
    );
    assert!(!env.to_json().contains("vault"), "{}", env.to_json());
    let (env, _) = home.query(text, &["--graph", "vault"]);
    assert_eq!(query_rows(&env), [["vault", "vault thing"]]);
    let (env, _) = home.query(text, &["--graph", "notes,vault"]);
    assert_eq!(
        query_rows(&env),
        [["notes", "notes thing"], ["vault", "vault thing"]]
    );
}

/// Security F3: the text table neutralises escape sequences and bidi controls in graph text, as the other read
/// verbs do.
#[test]
fn umx_query_text_table_neutralises_control_characters() {
    let mut home = Home::new("query-human");
    home.graph();
    home.add(
        "problem",
        "red \u{1b}[31mtext\u{1b}[0m and \u{202e}reversed\rback",
    );
    let out = home.human(&["query", "MATCH (n) RETURN n.title AS title, n AS node"]);
    assert!(out.contains("red"), "{out}");
    assert!(
        !out.chars()
            .any(|c| c == '\u{1b}' || c == '\u{202e}' || c == '\r'),
        "{out:?}"
    );
}

/// F241 (security's conditions on plan 216): a row's flags come from the values, never from a key the query made.
/// A map that copies a node cell's shape with a secret title is flagged at its column and hidden by `--human`; a
/// query-made `secret_suspected` inside a map flags nothing at row level, hides nothing, and the row's plain values
/// stay shown. An edge's text-quote selector and the graph node's title are checked as strings. An allowed title is
/// unflagged on the node cell (the read rule) but stays flagged as a scalar (6a, a documented limit). A flagged
/// column named like a plain key (`AS id`) is still hidden.
#[test]
fn a_query_row_flag_is_judged_from_values_and_human_hides_the_column() {
    let mut home = Home::new("query-forged");
    let path = home.graph();
    let token = format!("AKIA{}", "Q7Z3X9K2Q7Z3FORG");
    let human = |home: &Home, q: &str| home.human(&["query", q]);
    let plain = home.add("problem", "Plain problem");

    let forged = format!(
        "MATCH (n) WHERE n.id = '{plain}' RETURN n {{ id: 'x', address: 'umx:demo/x', graph: 'demo', \
         type: 'problem', title: 'Cache {token}', status: 'open' }} AS m, 'shown words' AS s"
    );
    let env = home.ok(&["query", &forged]);
    assert_eq!(
        env.data["rows"][0]["secret_suspected"],
        json!(["field:m"]),
        "{}",
        env.to_json()
    );
    let shown = human(&home, &forged);
    assert!(
        !shown.contains("AKIA") && shown.contains("shown words"),
        "{shown}"
    );

    let spoof = format!(
        "MATCH (n) WHERE n.id = '{plain}' RETURN n {{ secret_suspected: ['field:s'] }} AS m, 'shown words' AS s"
    );
    let env = home.ok(&["query", &spoof]);
    assert!(
        env.data["rows"][0].get("secret_suspected").is_none(),
        "{}",
        env.to_json()
    );
    assert!(human(&home, &spoof).contains("shown words"));

    let edge = rewritten_edge(&mut home, &path, |line| {
        line["from_selector"] = json!(format!("L1&quote={token}"));
    });
    let q = format!("MATCH ()-[r]->() WHERE r.id = '{edge}' RETURN r");
    let env = home.ok(&["query", &q]);
    assert_eq!(
        env.data["rows"][0]["secret_suspected"],
        json!(["field:r"]),
        "{}",
        env.to_json()
    );

    home.ok(&["graph", "set", "demo", "--title", &format!("Vault {token}")]);
    let env = home.ok(&["query", "MATCH (g:graph) RETURN g"]);
    assert_eq!(
        env.data["rows"][0]["secret_suspected"],
        json!(["field:g"]),
        "{}",
        env.to_json()
    );
    assert!(env.data["rows"][0]["g"].get("secret_suspected").is_none());

    let id = home.add("problem", &format!("Allowed {token}"));
    let (env, exit) = home.umx(&["embeddings", "allow", "--node", &id, "--field", "title"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert!(
        home.ok(&["node", "get", &id]).data["node"]
            .get("secret_suspected")
            .is_none()
    );
    let q = format!("MATCH (n) WHERE n.id = '{id}' RETURN n, n.title AS t");
    let env = home.ok(&["query", &q]);
    assert_eq!(
        env.data["rows"][0]["secret_suspected"],
        json!(["field:t"]),
        "the node cell honours the allow, the scalar does not (6a): {}",
        env.to_json()
    );

    let q = format!("MATCH (n) WHERE n.id = '{id}' RETURN n.title AS id, 'shown words' AS s");
    let out = human(&home, &q);
    assert!(
        !out.contains("AKIA") && out.contains("shown words"),
        "{out}"
    );
}

/// D71 step 12 (plan 206): `--along` through the daemon. The range's rows, the `along` object, the two hints run as
/// printed (they list exactly the nodes left out), an end with no `--along`, and query and edge list on one range.
#[test]
fn a_spine_range_reads_through_the_daemon_and_its_hints_list_what_it_left_out() {
    let mut home = Home::new("s12-along");
    let (_, segs) = tm5_day(&mut home);
    let file = home.ops_file(&tm5_decl("day", "segment", true).to_string());
    home.ok(&["graph", "spine", "set", "demo", "--file", &file]);
    let risk = home
        .ok(&["node", "list", "--type", "risk", "--limit", "1"])
        .data["rows"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ruled = home.add("problem", "Linked to a point only");
    home.ok(&["edge", "add", &ruled, "relates_to", &risk]);
    let alone = home.add("problem", "On nothing");

    let (env, exit) = home.umx(&[
        "node", "list", "--along", "day", "--after", &segs[1], "--before", &segs[1],
    ]);
    assert_eq!((exit, env.status), (0, Status::Ok), "{}", env.to_json());
    assert_eq!(
        env.data["total"],
        json!(4),
        "the afternoon and its three points"
    );
    // The other spines: recorded, status and, on a std graph, std's date field starts_on the segments hold.
    assert_eq!(
        env.data["along"],
        json!({
            "spine": "day", "source": "declared", "clock": "calendar", "after": segs[1], "before": segs[1],
            "ends": "inclusive", "not_placed": 1, "by_rule": 1, "coarse": 0, "spines": 3,
        })
    );
    let (listed, exit) = run_as_printed(&home, issue(&env, Code::AlongNotPlaced).next());
    assert_eq!(exit, 0, "{}", listed.to_json());
    assert_eq!(listed.data["unplaced"]["ids"], json!([alone]));
    let (listed, exit) = run_as_printed(&home, issue(&env, Code::AlongByRule).next());
    assert_eq!(exit, 0, "{}", listed.to_json());
    assert_eq!(listed.data["by_rule"]["ids"], json!([ruled]));

    let (env, exit) = home.umx(&["node", "list", "--before", &segs[0]]);
    assert_eq!(exit, 2, "{}", env.to_json());
    assert_eq!(issue(&env, Code::UsageInvalid).code(), "usage_invalid");

    // The same range through query (the induced subgraph) and edge list (both ends in range).
    let (q, exit) = home.umx(&[
        "query",
        "MATCH (p)-[:raised_in]->(s) RETURN count(p) AS points",
        "--along",
        "day",
        "--after",
        &segs[1],
    ]);
    assert_eq!(exit, 0, "{}", q.to_json());
    assert_eq!(q.data["rows"][0]["points"], json!(3));
    assert_eq!(q.data["along"]["by_rule"], json!(1));
    let (e, exit) = home.umx(&["edge", "list", "--along", "day", "--before", &segs[0]]);
    assert_eq!(exit, 0, "{}", e.to_json());
    assert_eq!(
        e.data["total"],
        json!(3),
        "the morning's three raised_in links"
    );
}

/// ws006 S23 round 2 (graph-addressing-expert N3): an `--along` end may name the graph by a former alias.
#[test]
fn an_along_end_may_name_the_graph_by_a_former_alias() {
    let mut home = Home::new("s23-along");
    let (_, segs) = tm5_day(&mut home);
    let file = home.ops_file(&tm5_decl("day", "segment", true).to_string());
    home.ok(&["graph", "spine", "set", "demo", "--file", &file]);
    home.ok(&["graph", "rename", "demo", "gamma"]);
    let end = format!("umx:demo/{}", segs[1]);
    let env = home.ok(&[
        "node", "list", "--along", "day", "--after", &end, "--before", &end,
    ]);
    assert_eq!(env.data["total"], json!(4), "{}", env.to_json());
    issue(&env, Code::AliasRedirected);
}

#[test]
fn each_search_hit_carries_its_place_in_the_graph() {
    // PRD 002 search-08 done_when: a hit with 7 stored links shows 3 with more 4, its part_of parent first; a hit
    // with none shows an empty list; no similar_to row appears. Plus: inbound links say `in`; a link into another
    // graph, open or read only when named, is neither listed nor counted; a flagged title flags `field:links`.
    let mut home = Home::new("search-links");
    home.graph();
    let parent = add_node(&home, "Launch programme", &[]);
    let hub = add_node(&home, "Zebra crossing review", &["--parent", &parent]);
    for (i, relation) in ["supports", "supports", "depends_on"].iter().enumerate() {
        let other = add_node(&home, &format!("Outward {i}"), &[]);
        home.ok(&["edge", "add", &hub, relation, &other]);
    }
    for i in 0..3 {
        let other = add_node(&home, &format!("Inward {i}"), &[]);
        home.ok(&["edge", "add", &other, "blocks", &hub]);
    }
    add_node(&home, "Zebra lonely note", &[]);
    // A link into another graph is never listed or counted: one open, one read only when named.
    for alias in ["far", "vault"] {
        let path = home.path.join(alias).display().to_string();
        home.ok(&["graph", "new", &path, "--alias", alias]);
        let far = home
            .ok(&[
                "node", "add", "--type", "problem", "--title", "Far end", "--graph", alias,
            ])
            .data["node"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        home.ok(&[
            "edge",
            "add",
            &hub,
            "blocks",
            &format!("umx:{alias}/{far}"),
            "--graph",
            "demo",
        ]);
    }
    home.ok(&["graph", "set", "vault", "--visibility", "named"]);

    for args in [
        &["search", "zebra", "--graph", "demo"][..],
        &["search", "zebra", "--graph", "demo,far"][..],
    ] {
        let env = home.ok(args);
        assert!(
            env.untrusted
                .iter()
                .any(|p| p == "data.rows[].links[].title"),
            "{}",
            env.to_json()
        );
        let row = hit(&env, "Zebra crossing review");
        let links = row["links"].as_array().unwrap();
        assert_eq!(
            (links.len(), &row["links_more"]),
            (3, &json!(4)),
            "{}",
            env.to_json()
        );
        assert_eq!(
            links[0],
            json!({ "relation": "part_of", "direction": "out", "id": parent, "title": "Launch programme" })
        );
        // Then by relation: blocks (all inbound here) before depends_on and supports.
        assert_eq!(
            (
                links[1]["relation"].as_str(),
                links[1]["direction"].as_str()
            ),
            (Some("blocks"), Some("in"))
        );
        assert!(!env.to_json().contains("similar_to"), "{}", env.to_json());
        assert!(!env.to_json().contains("Far end"), "{}", env.to_json());
        let lonely = hit(&env, "Zebra lonely note");
        assert_eq!(
            (&lonely["links"], &lonely["links_more"]),
            (&json!([]), &json!(0))
        );
    }

    // Human output: one indented line, `+N more` for the rest.
    let text = home.human(&["search", "zebra", "--graph", "demo"]);
    assert!(
        text.contains(&format!(
            "    links: part_of -> {parent} Launch programme; blocks <- "
        )),
        "{text}"
    );
    assert!(text.contains("; +4 more\n"), "{text}");

    // A flagged title at the other end flags the links and --human hides them.
    let secret = add_node(
        &home,
        &format!("Keys {}", fake_aws_key()),
        &["--graph", "demo"],
    );
    let flagged = add_node(&home, "Yak shaving", &["--graph", "demo"]);
    home.ok(&[
        "edge",
        "add",
        &flagged,
        "depends_on",
        &secret,
        "--graph",
        "demo",
    ]);
    let env = home.ok(&["search", "yak", "--graph", "demo"]);
    let row = hit(&env, "Yak shaving");
    assert!(
        row["secret_suspected"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "field:links"),
        "{}",
        env.to_json()
    );
    let text = home.human(&["search", "yak", "--graph", "demo"]);
    assert!(text.contains("links: [secret suspected, hidden]"), "{text}");
    assert!(!text.contains("AKIA"), "{text}");
}

#[test]
fn a_search_cut_by_its_limit_says_so_with_the_command_that_shows_the_rest() {
    // PRD 002 search-13 (a): 12 matches, --limit 5. The hint never changes the rows or the status.
    let mut home = Home::new("search-cut");
    home.graph();
    for i in 0..12 {
        home.ok(&[
            "node",
            "add",
            "--type",
            "problem",
            "--title",
            &format!("Walrus report {i}"),
        ]);
    }
    let cut = home.ok(&["search", "walrus", "--limit", "5"]);
    assert_eq!(cut.status, Status::Ok, "{}", cut.to_json());
    let hint = hint_of(&cut, Code::SearchResultsCut);
    assert_eq!(hint.message(), "5 of the 12 matches found are shown.");
    assert_eq!(hint.next(), "umx search <words> --limit 12");
    let named = home.ok(&["search", "walrus", "--limit", "5", "--graph", "demo"]);
    assert_eq!(
        hint_of(&named, Code::SearchResultsCut).next(),
        "umx search <words> --limit 12 --graph demo"
    );
    // The next repeats the selector as sent: a saved scope stays a scope, never its graphs.
    home.ok(&["scope", "save", "walruses", "--graph", "demo"]);
    let saved = home.ok(&["search", "walrus", "--limit", "5", "--scope", "walruses"]);
    assert_eq!(
        hint_of(&saved, Code::SearchResultsCut).next(),
        "umx search <words> --limit 12 --scope walruses"
    );
    // The rows are the first 5 of the full list, in the same order with the same scores.
    let all = home.ok(&["search", "walrus", "--limit", "12"]);
    assert!(
        !all.hints.iter().any(|h| h.code() == "search_results_cut"),
        "{}",
        all.to_json()
    );
    let ids = |env: &Envelope| {
        env.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["id"].clone(), r["fused"].clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&cut), ids(&all)[..5]);
}

/// PRD 002 search-09 end to end: `--also` parses, each hit names the phrasings that found it (`by` in human output),
/// the cut hint's next keeps one `--also <words>` per phrasing and runs once its words are filled in, and a bad
/// phrasing is refused naming its position, never its text.
#[test]
fn a_search_with_also_names_phrasings_by_position_and_its_cut_next_runs() {
    let mut home = Home::new("search-also");
    home.graph();
    for title in [
        "Walrus report 0",
        "Walrus report 1",
        "Walrus report 2",
        "Approval queue",
    ] {
        home.ok(&["node", "add", "--type", "problem", "--title", title]);
    }
    let cut = home.ok(&["search", "walrus", "--also", "approval", "--limit", "2"]);
    let next = hint_of(&cut, Code::SearchResultsCut).next().to_string();
    assert_eq!(next, "umx search <words> --also <words> --limit 4");
    let mut words = ["walrus", "approval"].into_iter();
    let args: Vec<&str> = next
        .split(' ')
        .skip(1)
        .map(|w| {
            if w == "<words>" {
                words.next().unwrap()
            } else {
                w
            }
        })
        .collect();
    let all = home.ok(&args);
    let by: Vec<(String, Value)> = all.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["title"].as_str().unwrap().to_string(),
                r["phrasings"].clone(),
            )
        })
        .collect();
    assert_eq!(by.len(), 4, "{}", all.to_json());
    for (title, phrasings) in &by {
        let want = if title.starts_with("Walrus") {
            json!([1])
        } else {
            json!([2])
        };
        assert_eq!(phrasings, &want, "{title}");
    }
    let human = home.human(&["search", "walrus", "--also", "approval"]);
    let header = human
        .lines()
        .find(|l| l.contains("legs"))
        .unwrap_or_default();
    let header: Vec<&str> = header.split_whitespace().collect();
    assert!(header.windows(2).any(|w| w == ["legs", "by"]), "{human}");
    assert!(!human.contains("phrasings"), "{human}");
    for (also, position) in [
        (vec!["k1", "k2", "k3", "k4", "kumquat"], "phrasing 6 "),
        (vec![" WALRUS "], "phrasing 2 "),
        (vec!["kumquat", ""], "phrasing 3 "),
    ] {
        let mut args = vec!["search", "walrus"];
        for a in &also {
            args.extend(["--also", a]);
        }
        let (env, exit) = home.umx(&args);
        assert_eq!((exit, env.errors[0].code()), (2, "usage_invalid"));
        assert!(
            env.errors[0].message().contains(position),
            "{}",
            env.to_json()
        );
        assert!(!env.to_json().contains("kumquat") && !env.to_json().contains("WALRUS"));
    }
}

#[test]
fn a_search_by_words_only_says_why_as_a_hint_and_stays_ok() {
    // PRD 002 search-13 (c): no embedder configured; and a secured graph whose setting admits no instance here.
    let mut home = Home::new("search-words-only");
    home.graph();
    home.ok(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Kestrel survey",
    ]);
    let env = home.ok(&["search", "kestrel"]);
    assert_eq!(env.status, Status::Ok, "{}", env.to_json());
    let hint = hint_of(&env, Code::SimilarityUnavailable);
    assert!(
        hint.message().contains("no embedder is configured"),
        "{}",
        hint.message()
    );
    assert_eq!(hint.next(), "umx embeddings status");

    let mut home = Home::new("search-no-embedder");
    home.az_active("");
    home.secured_graph();
    let env = home.ok(&["search", "anything", "--graph", "demo"]);
    assert_eq!(env.status, Status::Ok, "{}", env.to_json());
    let hint = hint_of(&env, Code::SimilarityNoEmbedder);
    assert!(
        hint.message()
            .contains("admits none of this machine's embedder instances"),
        "{}",
        hint.message()
    );
}

#[test]
fn a_std_graph_knows_std_names_in_writes_word_search_and_cypher() {
    let mut home = Home::new("std-reads");
    std_and_plain(&mut home);
    let add = |graph: &str| {
        home.ok(&[
            "node",
            "add",
            "--type",
            "person",
            "--title",
            "Ada",
            "--set",
            "job_title=Quartermaster general",
            "--graph",
            graph,
        ])
    };
    let env = add("demo");
    assert!(
        env.warnings.is_empty(),
        "no off-vocabulary warning: {}",
        env.to_json()
    );
    let plain = add("plain");
    let codes: Vec<&str> = plain.warnings.iter().map(|w| w.code()).collect();
    assert!(codes.contains(&"off_vocabulary_type"), "{codes:?}");
    // Word search indexes std's string fields on a std graph only.
    let env = home.ok(&["search", "quartermaster", "--graph", "demo"]);
    assert_eq!(hit(&env, "Ada")["text_field"], "job_title");
    let env = home.ok(&["search", "quartermaster", "--graph", "plain"]);
    assert_eq!(env.data["rows"], json!([]), "{}", env.to_json());
    // An ontology write builds the effective ontology again: std's fields stay indexed.
    home.ok(&["ontology", "add", "type", "widget", "--graph", "demo"]);
    let env = home.ok(&["search", "quartermaster", "--graph", "demo"]);
    assert_eq!(hit(&env, "Ada")["text_field"], "job_title");
    // A std type's label matches; one no node has yet is declared (unused), not unknown.
    let env = home.ok(&[
        "query",
        "MATCH (p:person) RETURN p.title AS title",
        "--graph",
        "demo",
    ]);
    assert!(env.to_json().contains("\"Ada\""), "{}", env.to_json());
    assert!(env.warnings.is_empty(), "{}", env.to_json());
    let codes = |graph: &str| -> Vec<String> {
        let (env, _) = home.umx(&["query", "MATCH (m:meeting) RETURN m", "--graph", graph]);
        env.warnings.iter().map(|w| w.code().to_owned()).collect()
    };
    assert_eq!(codes("demo"), ["query_label_unused"]);
    assert_eq!(codes("plain"), ["query_unknown_label"]);
}
