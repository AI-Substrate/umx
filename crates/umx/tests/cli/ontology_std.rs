//! The cli tests: the ontology verbs, columns, roles and std.
//! A module of cli.rs, whose `Home` and helpers it uses, in its own file so no file nears the seed gate's
//! size cap.

use super::*;

#[test]
fn declared_values_std_risk_ratings_and_mitigation_are_soft() {
    let mut home = Home::new("risk-values");
    let path = home.graph();
    home.ok(&["rule", "off", "std:risk_owned"]);
    let before = fs::read(Path::new(&path).join("graph.json")).unwrap();
    let std = home.ok(&["ontology", "show", "--std"]);
    let risk_type = std.data["types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "risk")
        .unwrap();
    for name in ["likelihood", "impact"] {
        let field = risk_type["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .unwrap();
        assert_eq!(field["kind"], "string");
        assert_eq!(field["values"], json!(["low", "medium", "high"]));
    }
    let risk = home.ok(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "Launch slips",
        "--set",
        "likelihood=high",
        "--set",
        "impact=low",
    ]);
    assert!(risk.warnings.is_empty(), "{}", risk.to_json());
    let id = risk.data["node"]["id"].as_str().unwrap();
    let action = home.add("action", "Test the launch");
    let (edge, exit) = home.umx(&["edge", "add", &action, "mitigates", id]);
    assert_eq!(exit, 0);
    assert!(edge.warnings.is_empty(), "{}", edge.to_json());
    let (edge, exit) = home.umx(&["edge", "add", id, "mitigates", &action]);
    assert_eq!(exit, 0);
    issue(&edge, Code::RelationEndpointUnexpected);
    assert_eq!(
        fs::read(Path::new(&path).join("graph.json")).unwrap(),
        before
    );
    let guide = home.ok(&["graph", "guide", "demo"]);
    let risk_type = guide.data["types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "risk")
        .unwrap();
    assert!(
        risk_type["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "impact" && f["values"] == json!(["low", "medium", "high"]))
    );
    assert!(
        home.human(&["graph", "guide", "demo"])
            .contains("[low, medium, high]")
    );
}

#[test]
fn declared_values_warning_next_extends_std_and_never_echoes_free_text() {
    let mut home = Home::new("risk-values-next");
    let path = home.graph();
    home.ok(&["rule", "off", "std:risk_owned"]);
    let (risk, exit) = home.umx(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "Launch slips",
        "--set",
        "impact=severe",
    ]);
    assert_eq!(exit, 0);
    assert_eq!(risk.data["node"]["impact"], "severe");
    let warning = issue(&risk, Code::FieldValueOffVocabulary);
    assert_eq!(
        warning.candidates(),
        &[
            json!({"id":"low"}),
            json!({"id":"medium"}),
            json!({"id":"high"})
        ]
    );
    assert_eq!(
        warning.next(),
        "umx ontology add field risk impact:string --graph demo --values severe"
    );
    let (set, exit) = home.run_next(warning.next());
    assert_eq!(exit, 0, "{}", set.to_json());
    assert_eq!(set.data["unchanged"], false);
    let id = risk.data["node"]["id"].as_str().unwrap();
    let (edit, exit) = home.umx(&["node", "edit", id, "--set", "impact=severe"]);
    assert_eq!(exit, 0);
    assert!(edit.warnings.is_empty(), "{}", edit.to_json());
    for text in ["High", "Very High; echo injected"] {
        let (edit, exit) = home.umx(&["node", "edit", id, "--set-str", &format!("impact={text}")]);
        assert_eq!(exit, 0);
        assert_eq!(edit.data["node"]["impact"], text);
        let warning = issue(&edit, Code::FieldValueOffVocabulary);
        assert_eq!(
            warning.next(),
            format!("umx node edit {id} --set impact=<value> --graph demo")
        );
        assert!(!warning.message().contains(text));
        assert!(!warning.next().contains(text));
    }
    let (edit, exit) = home.umx(&["node", "edit", id, "--set-json", "impact=7"]);
    assert_eq!(exit, 0);
    issue(&edit, Code::FieldKindMismatch);
    assert!(
        edit.warnings
            .iter()
            .all(|w| w.code() != "field_value_off_vocabulary")
    );
    let clear = home.ok(&[
        "ontology",
        "set",
        "field",
        "risk",
        "impact",
        "--clear-values",
    ]);
    assert_eq!(clear.data["values"], json!(["low", "medium", "high"]));
    home.ok(&["graph", "unload", "demo"]);
    home.ok(&["graph", "load", &path]);
    let (risk, _) = home.umx(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "A second risk",
        "--set",
        "impact=severe",
    ]);
    issue(&risk, Code::FieldValueOffVocabulary);
}

#[test]
fn declared_values_custom_lists_round_trip_replace_and_clear_without_format_change() {
    let mut home = Home::new("custom-values");
    let path = home.graph();
    let meta = graph_json(&path);
    let format = meta["umx"].clone();
    let add = [
        "ontology",
        "add",
        "field",
        "task",
        "priority:string",
        "--values",
        "low,medium,high",
    ];
    home.ok(&add);
    let before = fs::read(Path::new(&path).join("graph.json")).unwrap();
    assert_eq!(home.ok(&add).data["unchanged"], true);
    assert_eq!(
        fs::read(Path::new(&path).join("graph.json")).unwrap(),
        before
    );
    assert_eq!(
        type_in_file(&path, "task")["fields"][0]["values"],
        json!(["low", "medium", "high"])
    );
    home.ok(&["graph", "unload", "demo"]);
    home.ok(&["graph", "load", &path]);
    let (subset, exit) = home.umx(&[
        "ontology",
        "add",
        "field",
        "task",
        "priority:string",
        "--values",
        "high,low",
    ]);
    assert_eq!(exit, 0);
    assert_eq!(subset.data["unchanged"], true);
    let set = home.ok(&[
        "ontology", "set", "field", "task", "priority", "--values", "high,low",
    ]);
    assert_eq!(set.data["values"], json!(["high", "low"]));
    let (node, _) = home.umx(&[
        "node",
        "add",
        "--type",
        "task",
        "--title",
        "Try a rating",
        "--set",
        "priority=medium",
    ]);
    issue(&node, Code::FieldValueOffVocabulary);
    let clear = home.ok(&[
        "ontology",
        "set",
        "field",
        "task",
        "priority",
        "--clear-values",
    ]);
    assert_eq!(clear.data["values"], json!([]));
    assert!(
        type_in_file(&path, "task")["fields"][0]
            .get("values")
            .is_none()
    );
    let meta = graph_json(&path);
    assert_eq!(meta["umx"], format);
    let (node, _) = home.umx(&[
        "node",
        "add",
        "--type",
        "task",
        "--title",
        "Free rating",
        "--set",
        "priority=medium",
    ]);
    assert!(node.warnings.is_empty(), "{}", node.to_json());
}

#[test]
fn declared_values_invalid_declarations_are_atomic_and_offer_a_label_fix() {
    let mut home = Home::new("invalid-values");
    let path = home.graph();
    let before = fs::read(Path::new(&path).join("graph.json")).unwrap();
    let many = (0..33)
        .map(|i| format!("label_{i}"))
        .collect::<Vec<_>>()
        .join(",");
    for values in ["Low,Very High", "low,low", "", "low,", &many] {
        let (env, exit) = home.umx(&[
            "ontology",
            "add",
            "field",
            "task",
            "priority:string",
            "--values",
            values,
        ]);
        assert_eq!(exit, 2, "{}", env.to_json());
        issue(&env, Code::FieldValuesInvalid);
        assert_eq!(
            fs::read(Path::new(&path).join("graph.json")).unwrap(),
            before
        );
    }
    let (env, _) = home.umx(&[
        "ontology",
        "add",
        "field",
        "task",
        "priority:string",
        "--values",
        "Low,Very High",
    ]);
    let (fixed, exit) = home.run_next(issue(&env, Code::FieldValuesInvalid).next());
    assert_eq!(exit, 0, "{}", fixed.to_json());
    assert_eq!(
        type_in_file(&path, "task")["fields"][0]["values"],
        json!(["low", "very_high"])
    );
    let before = fs::read(Path::new(&path).join("graph.json")).unwrap();
    for spec in ["score:int", "score:bool"] {
        let (env, exit) = home.umx(&[
            "ontology", "add", "field", "task", spec, "--values", "low,high",
        ]);
        assert_eq!(exit, 2);
        issue(&env, Code::FieldValuesInvalid);
        assert_eq!(
            fs::read(Path::new(&path).join("graph.json")).unwrap(),
            before
        );
    }
    let (env, exit) = home.umx(&[
        "ontology",
        "set",
        "field",
        "task",
        "priority",
        "--values",
        "low",
        "--clear-values",
    ]);
    assert_eq!(exit, 2);
    issue(&env, Code::UsageInvalid);
    assert_eq!(
        fs::read(Path::new(&path).join("graph.json")).unwrap(),
        before
    );
}

#[test]
fn declared_values_multi_type_appends_preserve_metadata_and_refuse_overflow_atomically() {
    let mut home = Home::new("append-values");
    let path = home.graph();
    home.ok(&[
        "ontology",
        "add",
        "field",
        "task",
        "priority:string",
        "--values",
        "low",
        "--role",
        "content",
        "--description",
        "Priority",
    ]);
    home.ok(&[
        "ontology",
        "add",
        "field",
        "issue",
        "priority:string",
        "--values",
        "high",
        "--role",
        "content",
    ]);
    home.ok(&[
        "ontology",
        "add",
        "field",
        "task,issue",
        "priority:string",
        "--values",
        "high,match,low",
        "--role",
        "content",
    ]);
    let task = type_in_file(&path, "task");
    let field = &task["fields"][0];
    assert_eq!(field["values"], json!(["low", "high", "match"]));
    assert_eq!(field["role"], "content");
    assert_eq!(field["description"], "Priority");
    assert_eq!(
        type_in_file(&path, "issue")["fields"][0]["values"],
        json!(["high", "match", "low"])
    );
    let (node, exit) = home.umx(&[
        "node",
        "add",
        "--type",
        "task",
        "--title",
        "Urgent task",
        "--set",
        "priority=urgent",
    ]);
    assert_eq!(exit, 0);
    let warning = issue(&node, Code::FieldValueOffVocabulary);
    assert!(warning.next().contains("--role content"));
    let (added, exit) = home.run_next(warning.next());
    assert_eq!(exit, 0, "{}", added.to_json());
    let values = (0..32)
        .map(|i| format!("v_{i}"))
        .collect::<Vec<_>>()
        .join(",");
    home.ok(&[
        "ontology", "set", "field", "task", "priority", "--values", &values, "--role", "data",
    ]);
    assert_eq!(type_in_file(&path, "task")["fields"][0]["role"], "data");
    home.ok(&[
        "ontology", "set", "field", "issue", "priority", "--role", "data",
    ]);
    let before = fs::read(Path::new(&path).join("graph.json")).unwrap();
    let (refused, exit) = home.umx(&[
        "ontology",
        "add",
        "field",
        "issue,task",
        "priority:string",
        "--values",
        "overflow",
    ]);
    assert_eq!(exit, 2);
    let warning = issue(&refused, Code::FieldValuesInvalid);
    assert!(
        warning
            .next()
            .starts_with("umx ontology set field task priority ")
    );
    assert_eq!(
        fs::read(Path::new(&path).join("graph.json")).unwrap(),
        before
    );
    let label = "a".repeat(64);
    home.ok(&[
        "ontology", "set", "field", "task", "priority", "--values", &label,
    ]);
    let before = fs::read(Path::new(&path).join("graph.json")).unwrap();
    let (refused, exit) = home.umx(&[
        "ontology",
        "set",
        "field",
        "task",
        "priority",
        "--values",
        &format!("{label}a"),
    ]);
    assert_eq!(exit, 2);
    issue(&refused, Code::FieldValuesInvalid);
    assert_eq!(
        fs::read(Path::new(&path).join("graph.json")).unwrap(),
        before
    );
}

#[test]
fn declared_values_warning_keeps_an_exact_next_until_the_command_cap() {
    let mut home = Home::new("long-values-next");
    home.graph();
    let (graph, kind, field) = ("g".repeat(64), "t".repeat(64), "f".repeat(64));
    home.ok(&["graph", "rename", "demo", &graph]);
    home.ok(&[
        "ontology",
        "add",
        "field",
        &kind,
        &format!("{field}:string"),
        "--values",
        "low",
        "--graph",
        &graph,
    ]);
    for (length, fits) in [(50, true), (64, false)] {
        let label = "a".repeat(length);
        let (node, exit) = home.umx(&[
            "node",
            "add",
            "--type",
            &kind,
            "--title",
            "Long names",
            "--set",
            &format!("{field}={label}"),
            "--graph",
            &graph,
        ]);
        assert_eq!(exit, 0);
        let next = issue(&node, Code::FieldValueOffVocabulary).next();
        assert_eq!(
            next,
            format!(
                "umx ontology add field {kind} {field}:string --graph {graph} --values {}",
                if fits { &label } else { "<values>" }
            )
        );
        assert!(next.len() <= umx_core::problem::Command::MAX);
        if fits {
            let (declared, exit) = home.run_next(next);
            assert_eq!(exit, 0, "{}", declared.to_json());
        }
    }
}

#[test]
fn declared_values_only_warn_for_fields_the_edit_changes() {
    let mut home = Home::new("changed-values");
    home.graph();
    let node = home.ok(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "Risk",
        "--set",
        "impact=severe",
    ]);
    let id = node.data["node"]["id"].as_str().unwrap();
    for words in [
        vec!["node", "edit", id, "--title", "Renamed risk"],
        vec!["node", "edit", id, "--set", "impact=severe"],
        vec!["node", "edit", id, "--tag", "review"],
    ] {
        let env = home.ok(&words);
        assert!(
            issues(&env, Code::FieldValueOffVocabulary).is_empty(),
            "{}",
            env.to_json()
        );
    }
    let changed = home.ok(&["node", "edit", id, "--set", "impact=extreme"]);
    assert_eq!(issues(&changed, Code::FieldValueOffVocabulary).len(), 1);
    let ops = home.ops_file(&format!(
        "{{\"op\":\"node edit\",\"node\":\"{id}\",\"title\":\"Batch title\"}}\n\
        {{\"op\":\"node edit\",\"node\":\"{id}\",\"sets\":[{{\"field\":\"impact\",\"value\":\"extreme\"}}]}}\n"
    ));
    let env = home.ok(&["batch", "--file", &ops]);
    assert!(
        issues(&env, Code::FieldValueOffVocabulary).is_empty(),
        "{}",
        env.to_json()
    );
}

#[test]
fn declared_values_on_key_fill_warns_once_in_single_and_batch_writes() {
    let mut home = Home::new("resolved-values");
    home.graph();
    let node = home.ok(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "Risk",
        "--set",
        "entity_key=email:risk@example.invalid",
    ]);
    let id = node.data["node"]["id"].as_str().unwrap();
    let add = [
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "Risk",
        "--set",
        "entity_key=email:risk@example.invalid",
        "--set",
        "impact=severe",
        "--on-key",
        "resolve",
    ];
    let env = home.ok(&add);
    assert_eq!(env.data["node"]["id"], id);
    assert_eq!(issues(&env, Code::FieldValueOffVocabulary).len(), 1);
    assert!(issues(&home.ok(&add), Code::FieldValueOffVocabulary).is_empty());
    let line = json!({"op":"node add","type":"risk","title":"Risk","on_key":"resolve",
        "sets":[{"field":"entity_key","value":"email:risk@example.invalid"},
        {"field":"likelihood","value":"extreme"}]});
    let ops = home.ops_file(&format!("{line}\n{line}\n"));
    let env = home.ok(&["batch", "--file", &ops]);
    assert_eq!(
        issues(&env, Code::FieldValueOffVocabulary).len(),
        1,
        "{}",
        env.to_json()
    );
    let shown = home.ok(&["node", "get", id]);
    assert_eq!(shown.data["node"]["likelihood"], "extreme");
}

#[test]
fn declared_values_std_mutations_store_only_graph_added_labels() {
    let mut home = Home::new("own-values");
    let path = home.graph();
    let before = fs::read(Path::new(&path).join("graph.json")).unwrap();
    let cleared = home.ok(&[
        "ontology",
        "set",
        "field",
        "risk",
        "impact",
        "--clear-values",
    ]);
    assert_eq!(cleared.data["changed"], false);
    assert_eq!(
        fs::read(Path::new(&path).join("graph.json")).unwrap(),
        before
    );
    home.ok(&[
        "ontology",
        "add",
        "field",
        "risk",
        "impact:string",
        "--description",
        "Severity",
    ]);
    assert!(
        type_in_file(&path, "risk")["fields"][0]
            .get("values")
            .is_none()
    );
    home.ok(&[
        "ontology", "set", "field", "risk", "impact", "--role", "content",
    ]);
    assert!(
        type_in_file(&path, "risk")["fields"][0]
            .get("values")
            .is_none()
    );
    home.ok(&[
        "ontology",
        "add",
        "field",
        "risk",
        "impact:string",
        "--role",
        "content",
        "--values",
        "low,severe",
    ]);
    assert_eq!(
        type_in_file(&path, "risk")["fields"][0]["values"],
        json!(["severe"])
    );
    let set = home.ok(&[
        "ontology",
        "set",
        "field",
        "risk",
        "impact",
        "--values",
        "high,urgent,low",
    ]);
    assert_eq!(
        set.data["values"],
        json!(["low", "medium", "high", "urgent"])
    );
    assert_eq!(
        type_in_file(&path, "risk")["fields"][0]["values"],
        json!(["urgent"])
    );
    home.ok(&[
        "ontology",
        "set",
        "field",
        "risk",
        "impact",
        "--clear-values",
    ]);
    assert!(
        type_in_file(&path, "risk")["fields"][0]
            .get("values")
            .is_none()
    );
}

#[test]
fn declared_values_kind_checks_are_placed_and_risk_overrides_are_soft() {
    let mut home = Home::new("placed-values");
    let plain = std_and_plain(&mut home);
    home.ok(&["rule", "off", "std:risk_owned", "--graph", "demo"]);
    for field in ["impact:int", "likelihood:int"] {
        let env = home.ok(&["ontology", "add", "field", "task", field, "--graph", "demo"]);
        assert!(env.warnings.is_empty(), "{}", env.to_json());
        home.ok(&[
            "ontology", "add", "field", "task", field, "--graph", "plain",
        ]);
    }
    home.ok(&[
        "ontology",
        "add",
        "field",
        "risk",
        "impact:int",
        "--graph",
        "plain",
    ]);
    let adopted = home.ok(&["ontology", "adopt-std", "--graph", "plain"]);
    issue(&adopted, Code::StdFieldConflictStored);
    let env = home.ok(&[
        "ontology",
        "add",
        "field",
        "risk",
        "impact:int",
        "--graph",
        "demo",
    ]);
    issue(&env, Code::StdFieldConflictStored);
    let show = home.ok(&["ontology", "show", "--graph", "demo"]);
    assert_eq!(
        named(&named(&show.data["types"], "risk")["fields"], "impact")["kind"],
        "int"
    );
    let node = home.ok(&[
        "node", "add", "--type", "risk", "--title", "Numeric", "--set", "impact=7", "--graph",
        "demo",
    ]);
    assert!(node.warnings.is_empty(), "{}", node.to_json());
    assert_eq!(node.data["node"]["impact"], 7);
    home.ok(&["graph", "unload", "plain"]);
    let loaded = home.ok(&["graph", "load", &plain]);
    let warning = issue(&loaded, Code::StdFieldConflictStored);
    assert!(warning.message().contains("using int"));
    assert_eq!(issues(&loaded, Code::StdFieldConflictStored).len(), 1);
    let show = home.ok(&["ontology", "show", "--graph", "plain"]);
    assert_eq!(
        named(&named(&show.data["types"], "risk")["fields"], "impact")["kind"],
        "int"
    );
}

#[test]
fn declared_values_batch_adds_and_edits_use_the_same_soft_warning() {
    let mut home = Home::new("batch-values");
    home.graph();
    let ops = home.path.join("ops.jsonl");
    fs::write(&ops, concat!(
        "{\"op\":\"node add\",\"ref\":\"r\",\"type\":\"risk\",\"title\":\"Batch risk\",\"sets\":[{\"field\":\"impact\",\"value\":\"severe\"}]}\n",
        "{\"op\":\"node edit\",\"node_ref\":\"r\",\"sets\":[{\"field\":\"likelihood\",\"value\":\"extreme\"}]}\n"
    )).unwrap();
    let (env, exit) = home.umx(&["batch", "--file", ops.to_str().unwrap()]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let warnings = issues(&env, Code::FieldValueOffVocabulary);
    assert!(!warnings.is_empty(), "{}", env.to_json());
    assert!(warnings.iter().any(|w| w.next().contains("impact")));
    assert!(warnings.iter().any(|w| w.next().contains("likelihood")));
    let node = home.ok(&[
        "node",
        "list",
        "--type",
        "risk",
        "--columns",
        "id,impact,likelihood",
    ]);
    assert_eq!(node.data["rows"][0]["impact"], "severe");
    assert_eq!(node.data["rows"][0]["likelihood"], "extreme");
}

/// F96: a type's own columns are node list's default for `--type` it; an explicit --columns wins, a mixed list keeps
/// the usual columns, the key is written only while set, and --reset removes it.
#[test]
fn a_types_columns_are_node_lists_default_for_that_type() {
    let mut home = Home::new("type-columns");
    let path = home.graph();
    home.umx(&["ontology", "add", "field", "problem", "likelihood:int"]);
    home.umx(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Slip",
        "--set",
        "likelihood=3",
    ]);
    let usual = ["id", "type", "title", "status", "updated"];
    assert!(type_in_file(&path, "problem").get("columns").is_none());

    let (set, exit) = home.umx(&[
        "ontology",
        "set",
        "type",
        "problem",
        "--columns",
        "title,likelihood",
    ]);
    assert_eq!(exit, 0, "{}", set.to_json());
    assert_eq!(set.data["unchanged"], false);
    assert_eq!(
        type_in_file(&path, "problem")["columns"],
        json!(["title", "likelihood"])
    );
    let (shown, _) = home.umx(&["ontology", "show"]);
    let types = shown.data["types"].as_array().unwrap();
    let problem = types.iter().find(|t| t["name"] == "problem").unwrap();
    assert_eq!(problem["columns"], json!(["title", "likelihood"]));
    let (again, _) = home.umx(&[
        "ontology",
        "set",
        "type",
        "problem",
        "--columns",
        "title,likelihood",
    ]);
    assert_eq!(again.data["unchanged"], true);

    let (typed, _) = home.umx(&["node", "list", "--type", "problem"]);
    assert_eq!(column_names(&typed), ["id", "title", "likelihood"]);
    assert_eq!(typed.data["rows"][0]["likelihood"], 3);
    let (explicit, _) = home.umx(&[
        "node",
        "list",
        "--type",
        "problem",
        "--columns",
        "id,status",
    ]);
    assert_eq!(column_names(&explicit), ["id", "status"]);
    let (mixed, _) = home.umx(&["node", "list"]);
    assert_eq!(column_names(&mixed), usual);
    let out = home.human(&["node", "list", "--type", "problem"]);
    assert!(
        out.lines()
            .any(|l| l.contains("likelihood") && !l.contains("status")),
        "{out}"
    );

    let (reset, _) = home.umx(&["ontology", "set", "type", "problem", "--reset"]);
    assert_eq!(
        (
            reset.data["unchanged"].as_bool(),
            reset.data["columns"].is_null()
        ),
        (Some(false), true)
    );
    assert!(type_in_file(&path, "problem").get("columns").is_none());
    let (back, _) = home.umx(&["node", "list", "--type", "problem"]);
    assert_eq!(column_names(&back), usual);
}

/// F96: a column that is neither a standard field nor one the type declares is refused, and nothing is written.
#[test]
fn ontology_set_type_refuses_an_undeclared_column() {
    let mut home = Home::new("type-columns-undeclared");
    let path = home.graph();
    let (env, exit) = home.umx(&[
        "ontology",
        "set",
        "type",
        "problem",
        "--columns",
        "title,owner",
    ]);
    assert_eq!((env.status, exit), (Status::Error, 1), "{}", env.to_json());
    assert_eq!(
        issue(&env, Code::FieldMissing).next(),
        "umx ontology add field problem owner:<kind> --graph demo"
    );
    assert!(type_in_file(&path, "problem").get("columns").is_none());
    let (env, _) = home.umx(&["ontology", "set", "type", "nope", "--columns", "title"]);
    issue(&env, Code::TypeUnknown);
}

/// F96: --columns and --reset together are a usage error, refused before anything reaches the daemon.
#[test]
fn ontology_set_type_takes_columns_or_reset_not_both() {
    let mut home = Home::new("type-columns-both");
    let path = home.graph();
    let (env, exit) = home.umx(&[
        "ontology",
        "set",
        "type",
        "problem",
        "--columns",
        "title",
        "--reset",
    ]);
    assert_eq!(exit, 2, "{}", env.to_json());
    assert!(
        issue(&env, Code::UsageInvalid)
            .message()
            .contains("not both"),
        "{}",
        env.to_json()
    );
    assert!(type_in_file(&path, "problem").get("columns").is_none());
}

/// B1: with several graphs loaded, graph_ambiguous's next for `ontology set type` keeps the column list intact
/// (Fill::names, F73), never `title?status`, and runs as printed for one column or several.
#[test]
fn ontology_set_type_next_never_prints_a_mangled_column_list() {
    let mut home = Home::new("type-columns-next");
    home.graph();
    let beta = home.path.join("beta").display().to_string();
    home.ok(&["graph", "new", &beta, "--alias", "beta"]);
    for (given, want) in [
        (
            "title,status",
            "umx ontology set type problem --columns title,status --graph beta",
        ),
        (
            "title",
            "umx ontology set type problem --columns title --graph beta",
        ),
    ] {
        let (amb, exit) = home.umx(&["ontology", "set", "type", "problem", "--columns", given]);
        assert_eq!(
            (exit, amb.errors[0].code()),
            (1, "graph_ambiguous"),
            "{}",
            amb.to_json()
        );
        let next = amb.errors[0].next().to_owned();
        assert_eq!(next, want);
        let (ran, exit) = home.run_next(&next);
        assert_eq!(exit, 0, "{next}: {}", ran.to_json());
    }
}

/// Security N1: a provenance key is never a default column. `ontology set type` refuses one by name (exit 2)
/// and writes nothing.
#[test]
fn ontology_set_type_refuses_a_provenance_column() {
    let mut home = Home::new("type-columns-prov-set");
    let path = home.graph();
    let (env, exit) = home.umx(&[
        "ontology",
        "set",
        "type",
        "problem",
        "--columns",
        "title,prov_key",
    ]);
    assert_eq!(exit, 2, "{}", env.to_json());
    assert!(
        issue(&env, Code::UsageInvalid)
            .message()
            .contains("prov_key"),
        "{}",
        env.to_json()
    );
    assert!(type_in_file(&path, "problem").get("columns").is_none());
}

/// Security N1: a graph.json pulled with a provenance key among a type's columns cannot unhide it: load warns
/// type_column_unknown and node list skips it.
#[test]
fn a_pulled_provenance_column_stays_hidden() {
    let mut home = Home::new("type-columns-prov-read");
    let path = home.graph();
    plant_type_columns(&path, "problem", &["title", "authored_by_session"]);
    let (loaded, _) = home.umx(&["graph", "load", &path]);
    let warned = issue(&loaded, Code::TypeColumnUnknown).message();
    assert!(
        warned.contains("authored_by_session"),
        "{}",
        loaded.to_json()
    );
    let (listed, _) = home.umx(&["node", "list", "--type", "problem"]);
    assert_eq!(column_names(&listed), ["id", "title"]);
}

/// Security N2: a type takes at most 32 default columns; more is refused with the limit, and nothing written.
#[test]
fn ontology_set_type_caps_a_types_columns_at_32() {
    let mut home = Home::new("type-columns-cap");
    let path = home.graph();
    let many: Vec<String> = (0..33).map(|i| format!("c{i}")).collect();
    let (env, exit) = home.umx(&[
        "ontology",
        "set",
        "type",
        "problem",
        "--columns",
        &many.join(","),
    ]);
    assert_eq!(exit, 2, "{}", env.to_json());
    assert!(
        issue(&env, Code::UsageInvalid)
            .message()
            .contains("at most 32"),
        "{}",
        env.to_json()
    );
    assert!(type_in_file(&path, "problem").get("columns").is_none());
}

/// Security: a pulled graph.json listing more than 32 columns for a type shows only the first 32 (after id).
#[test]
fn a_pulled_type_column_list_is_capped_at_32_on_read() {
    let mut home = Home::new("type-columns-cap-read");
    let path = home.graph();
    let fields: Vec<String> = (0..40).map(|i| format!("f{i}")).collect();
    let mut args = vec!["ontology", "add", "type", "wide"];
    let specs: Vec<String> = fields.iter().map(|f| format!("{f}:int")).collect();
    for s in &specs {
        args.extend(["--field", s.as_str()]);
    }
    home.ok(&args);
    let names: Vec<&str> = fields.iter().map(String::as_str).collect();
    plant_type_columns(&path, "wide", &names);
    home.ok(&["graph", "load", &path]);
    let (listed, _) = home.umx(&["node", "list", "--type", "wide"]);
    let mut want = vec!["id".to_owned()];
    want.extend(fields.iter().take(32).cloned());
    assert_eq!(column_names(&listed), want);
}

/// F96: a hand-edited column the type does not declare is never silent: load warns type_column_unknown, and node
/// list skips it.
#[test]
fn a_hand_edited_type_column_warns_on_load_and_is_skipped() {
    let mut home = Home::new("type-columns-dangling");
    let path = home.graph();
    plant_type_columns(&path, "problem", &["title", "gone"]);
    let (loaded, exit) = home.umx(&["graph", "load", &path]);
    assert_eq!(exit, 0, "{}", loaded.to_json());
    assert!(
        issue(&loaded, Code::TypeColumnUnknown)
            .message()
            .contains("gone"),
        "{}",
        loaded.to_json()
    );
    let (listed, _) = home.umx(&["node", "list", "--type", "problem"]);
    assert_eq!(column_names(&listed), ["id", "title"]);
}

#[test]
fn ontology_remove_refuses_while_in_use_and_its_next_lists_the_users() {
    // F76.
    let mut home = Home::new("ontology-remove");
    home.graph();
    let a = home.add("problem", "Alpha");
    let (refused, exit) = home.umx(&["ontology", "remove", "type", "problem"]);
    assert_eq!(
        (exit, refused.errors[0].message()),
        (
            1,
            "type problem is used by 1 nodes in demo; nothing changed."
        )
    );
    let (listed, exit) = home.run_next(refused.errors[0].next());
    assert_eq!(exit, 0, "{}", listed.to_json());
    assert_eq!(listed.data["rows"][0]["id"].as_str(), Some(a.as_str()));

    let removed = home.ok(&["ontology", "remove", "type", "capability"]);
    assert_eq!(
        removed.data,
        umx_protocol::json!({ "actor_tier": removed.data["actor_tier"], "removed": "type", "name": "capability", "unchanged": false })
    );
    let again = home.ok(&["ontology", "remove", "type", "capability"]);
    assert_eq!(again.data["unchanged"], true);
    let show = home.ok(&["ontology", "show"]).data.to_string();
    assert!(!show.contains("capability"), "{show}");
    let (builtin, exit) = home.umx(&["ontology", "remove", "status", "open"]);
    assert_eq!((exit, builtin.errors[0].code()), (1, "ontology_builtin"));
}

#[test]
fn ontology_add_field_takes_many_types_or_all_in_one_call() {
    // F71.
    let mut home = Home::new("field-many");
    home.graph();
    // One type: exactly as before, a new type declared too.
    let one = home.ok(&["ontology", "add", "field", "brand_new", "size:int"]);
    assert_eq!(
        one.data,
        umx_protocol::json!({ "actor_tier": one.data["actor_tier"], "added": "field", "name": "size", "unchanged": false })
    );
    // An identical re-declare is ok and changes nothing, so setup scripts can re-run.
    let graph_json = home.path.join("g").join("graph.json");
    let before = fs::read(&graph_json).unwrap();
    for again in [
        &["field", "brand_new", "size:int"][..],
        &["type", "brand_new", "--field", "size:int"],
        &["status", "open"],
        &["relation", "conflicts_with", "--symmetric"],
    ] {
        let mut args = vec!["ontology", "add"];
        args.extend(again);
        assert_eq!(home.ok(&args).data["unchanged"], true, "{again:?}");
    }
    assert_eq!(fs::read(&graph_json).unwrap(), before);
    let (missing, exit) = home.umx(&["ontology", "add", "type", "brand_new", "--field", "w:int"]);
    assert_eq!((exit, missing.errors[0].code()), (1, "field_missing"));
    assert_eq!(
        missing.errors[0].next(),
        "umx ontology add field brand_new w:int --graph demo"
    );
    home.ok(&missing.errors[0]
        .next()
        .split(' ')
        .skip(1)
        .collect::<Vec<_>>());

    let many = home.ok(&[
        "ontology",
        "add",
        "field",
        "problem,actor",
        "option",
        "impact:int",
    ]);
    assert_eq!(
        many.data["types"],
        umx_protocol::json!(["problem", "actor", "option"])
    );
    assert_eq!(many.data["unchanged"], umx_protocol::json!([]));

    let (refused, exit) = home.umx(&[
        "ontology",
        "add",
        "field",
        "actor,nope,decision",
        "impact:string",
    ]);
    assert_eq!(exit, 1, "{}", refused.to_json());
    let said: Vec<(&str, &str)> = refused
        .errors
        .iter()
        .map(|i| (i.code(), i.message()))
        .collect();
    assert_eq!(
        said,
        [
            (
                "field_conflict",
                "Type actor already has field impact as int; nothing changed."
            ),
            (
                "type_unknown",
                "Type nope is not declared in demo; nothing changed."
            ),
        ]
    );
    let show = home.ok(&["ontology", "show"]).data.to_string();
    assert!(!show.contains("nope"), "{show}");

    let all = home.ok(&["ontology", "add", "field", "--all-types", "owner:string"]);
    assert_eq!(
        all.data["types"].as_array().unwrap().len(),
        11,
        "{}",
        all.to_json()
    );

    // graph_ambiguous's next keeps the whole type list.
    let beta = home.path.join("beta").display().to_string();
    home.ok(&["graph", "new", &beta, "--alias", "beta"]);
    for (words, kept) in [
        (
            &["problem,actor", "team:string"][..],
            "problem,actor team:<kind>",
        ),
        (
            &["--all-types", "team:string"][..],
            "--all-types team:<kind>",
        ),
    ] {
        let mut args = vec!["ontology", "add", "field"];
        args.extend(words);
        let (amb, _) = home.umx(&args);
        assert_eq!(
            amb.errors[0].next(),
            format!("umx ontology add field {kept} --graph beta"),
            "{}",
            amb.to_json()
        );
    }
}

#[test]
fn describing_refuses_what_it_cannot_keep_and_changes_nothing() {
    let mut home = Home::new("gdesc");
    let path = home.graph();
    let before = fs::read(format!("{path}/graph.json")).unwrap();
    let unchanged = |what: &str| {
        assert_eq!(
            fs::read(format!("{path}/graph.json")).unwrap(),
            before,
            "{what}"
        );
    };
    let long = "x".repeat(501);
    let (env, exit) = home.umx(&[
        "ontology",
        "add",
        "status",
        "parked",
        "--description",
        &long,
    ]);
    assert_eq!(
        issue(&env, Code::DescriptionTooLong).message(),
        "a description is 501 characters long; the most is 500. Nothing changed."
    );
    assert_eq!(exit, 1);
    let (env, _) = home.umx(&["graph", "set", "demo", "--purpose", &"x".repeat(4097)]);
    assert_eq!(
        issue(&env, Code::DescriptionTooLong).message(),
        "the purpose is 4097 bytes long; the most is 4096. Nothing changed."
    );
    // 500 characters of a two-byte letter is still 500 characters.
    let (env, _) = home.umx(&[
        "graph",
        "set",
        "demo",
        "--description",
        &format!("type:problem={}", "é".repeat(501)),
    ]);
    issue(&env, Code::DescriptionTooLong);
    for bad in [
        "problem=x",
        "kind:problem=x",
        "type:Bad=x",
        "field:risk=x",
        "status:open=two\nlines",
    ] {
        let (env, exit) = home.umx(&["graph", "set", "demo", "--description", bad]);
        assert_eq!((exit, env.errors[0].code()), (2, "usage_invalid"), "{bad}");
    }
    unchanged("refusals");

    // An undeclared type or field: the next declares it with the description, and runs as printed.
    let (env, exit) = home.umx(&[
        "graph",
        "set",
        "demo",
        "--description",
        "field:risk.owner=Who",
    ]);
    assert_eq!(exit, 1);
    let found = issue(&env, Code::DescriptionUndeclared);
    assert_eq!(
        found.next(),
        "umx ontology add field risk owner:<kind> --description <text> --graph demo"
    );
    unchanged("undeclared");
    let line = found
        .next()
        .replace("<kind>", "string")
        .replace("<text>", "'Who watches it'");
    assert_eq!(run_as_printed(&home, &line).1, 0, "{line}");
    assert_eq!(
        graph_json(&path)["ontology"]["types"][10],
        json!({ "name": "risk", "fields": [{ "name": "owner", "kind": "string", "description": "Who watches it" }] })
    );

    // Already declared, with a description: the next describes it instead.
    let (env, exit) = home.umx(&[
        "ontology",
        "add",
        "status",
        "open",
        "--description",
        "Not started",
    ]);
    assert_eq!(exit, 1);
    assert_eq!(
        issue(&env, Code::AlreadyDeclared).next(),
        "umx graph set demo --description status:open=<text>"
    );
    // F71: the same declaration again, with no description or the stored one, is unchanged.
    let env = home.ok(&["ontology", "add", "status", "open"]);
    assert_eq!(env.data["unchanged"], true, "{}", env.to_json());
    home.ok(&[
        "graph",
        "set",
        "demo",
        "--description",
        "status:open=Not started",
    ]);
    let env = home.ok(&[
        "ontology",
        "add",
        "status",
        "open",
        "--description",
        "Not started",
    ]);
    assert_eq!(env.data["unchanged"], true, "{}", env.to_json());

    // A status or relation need not be declared: described, warned.
    let (env, exit) = home.umx(&[
        "graph",
        "set",
        "demo",
        "--description",
        "relation:causes=Makes it happen",
    ]);
    assert_eq!(
        (exit, env.status),
        (0, Status::Warning),
        "{}",
        env.to_json()
    );
    assert_eq!(
        issue(&env, Code::OffVocabularyRelation).next(),
        "umx ontology add relation causes --graph demo"
    );
}

/// D56 (req-97): a field's role is stated with --role, shown by ontology show, changed with ontology set field,
/// and written to graph.json under the format that makes an older umx refuse it.
#[test]
fn field_roles_are_set_shown_changed_and_written_with_their_format() {
    let mut home = Home::new("field-roles");
    let path = home.graph();
    home.ok(&[
        "ontology",
        "add",
        "field",
        "doc",
        "file:string",
        "--role",
        "data",
    ]);
    // No role: data (PRD 002 search-04), and the reply's hint says so with the command that makes it content.
    let added = home.ok(&["ontology", "add", "field", "doc", "summary:string"]);
    let hint = issue(&added, Code::FieldDataByDefault);
    assert_eq!(
        hint.next(),
        "umx ontology set field doc summary --role content --graph demo"
    );
    let env = home.ok(&["ontology", "show"]);
    let fields = env.data["types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "doc")
        .unwrap()["fields"]
        .clone();
    assert_eq!(fields[0]["role"], "data");
    assert_eq!(fields[0]["role_default"], false);
    assert_eq!(fields[1]["role"], "data");
    assert_eq!(fields[1]["role_default"], true);
    run_next(&home, hint);
    assert_eq!(roles(&home, "doc")["summary"], "content");
    let file = Path::new(&path).join("graph.json");
    let v: Value = umx_protocol::parse_json(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(v["umx"], 4);
    assert!(
        v["ontology"]["types"]
            .to_string()
            .contains("\"role\":\"data\"")
    );
    let env = home.ok(&[
        "ontology", "set", "field", "doc", "file", "--role", "content",
    ]);
    assert_eq!(
        (
            &env.data["role"],
            &env.data["changed"],
            &env.data["pending"]
        ),
        (&json!("content"), &json!(true), &json!(0))
    );
}

#[test]
fn a_field_in_the_wrong_role_group_is_pointed_out_and_keeps_its_role_until_set() {
    // PRD 002 search-05 done_when: a label field of 3 distinct values declared content and a prose field declared
    // data; `graph guide` gives one hint for each, naming `umx ontology set field ... --role`, and neither role
    // changes until that command runs. The same hints on `embeddings status` and `ontology add field`.
    let mut home = Home::new("role-hints");
    home.graph();
    home.ok(&[
        "ontology",
        "add",
        "field",
        "deal",
        "tier:string",
        "--role",
        "content",
    ]);
    home.ok(&[
        "ontology",
        "add",
        "field",
        "deal",
        "summary:string",
        "--role",
        "data",
    ]);
    let tiers = ["gold", "silver", "bronze"];
    for i in 0..9 {
        let tier = format!("tier={}", tiers[i % 3]);
        let summary = format!(
            "summary=the supplier asked for a longer review window before round {i} of the deal"
        );
        home.ok(&[
            "node",
            "add",
            "--type",
            "deal",
            "--title",
            &format!("Deal {i}"),
            "--set",
            &tier,
            "--set",
            &summary,
        ]);
    }

    let env = home.ok(&["graph", "guide", "demo"]);
    assert_eq!(env.status, Status::Ok, "{}", env.to_json());
    let labels = role_hints(&env, Code::FieldLooksLikeLabels, "tier");
    let prose = role_hints(&env, Code::FieldLooksLikeProse, "summary");
    assert_eq!((labels.len(), prose.len()), (1, 1), "{}", env.to_json());
    assert_eq!(
        labels[0].next(),
        "umx ontology set field deal tier --role data --graph demo"
    );
    assert_eq!(
        prose[0].next(),
        "umx ontology set field deal summary --role content --graph demo"
    );
    assert_eq!(
        labels[0].message(),
        "Field tier of type deal in demo is content, but its 9 values are 3 short labels. As data it would need 9 fewer \
         embedder calls, and words and --where still find it."
    );
    assert_eq!(
        prose[0].message(),
        "Field summary of type deal in demo is data, but 9 of its 9 values read as prose. As content, search by meaning \
         would find them (9 embedder calls)."
    );
    // Counts only: no value of either field is in any hint.
    let hints: String = env
        .hints
        .iter()
        .map(|h| format!("{} {}\n", h.message(), h.next()))
        .collect();
    assert!(
        !hints.contains("silver") && !hints.contains("supplier"),
        "{hints}"
    );

    // Neither role changed by itself.
    let before = roles(&home, "deal");
    assert_eq!(
        (before["tier"].as_str(), before["summary"].as_str()),
        ("content", "data")
    );

    // embeddings status points out the same two fields, and stays ok.
    let status = home.ok(&["embeddings", "status"]);
    assert_eq!(
        role_hints(&status, Code::FieldLooksLikeLabels, "tier").len(),
        1,
        "{}",
        status.to_json()
    );
    assert_eq!(
        role_hints(&status, Code::FieldLooksLikeProse, "summary").len(),
        1,
        "{}",
        status.to_json()
    );
    assert!(status.errors.is_empty());

    // ontology add field judges a field whose nodes already hold values (set before it was declared).
    for i in 0..6 {
        let region = format!("region={}", ["north", "south"][i % 2]);
        home.ok(&[
            "node",
            "add",
            "--type",
            "deal",
            "--title",
            &format!("Region deal {i}"),
            "--set",
            &region,
        ]);
    }
    let added = home.ok(&[
        "ontology",
        "add",
        "field",
        "deal",
        "region:string",
        "--role",
        "content",
    ]);
    let region = role_hints(&added, Code::FieldLooksLikeLabels, "region");
    assert_eq!(region.len(), 1, "{}", added.to_json());
    assert_eq!(
        region[0].next(),
        "umx ontology set field deal region --role data --graph demo"
    );
    // ... and only the field it adds.
    assert!(role_hints(&added, Code::FieldLooksLikeLabels, "tier").is_empty());

    // Running the printed commands changes the roles, and the hints go.
    run_next(&home, labels[0]);
    run_next(&home, prose[0]);
    let after = roles(&home, "deal");
    assert_eq!(
        (after["tier"].as_str(), after["summary"].as_str()),
        ("data", "content")
    );
    let env = home.ok(&["graph", "guide", "demo"]);
    assert!(
        role_hints(&env, Code::FieldLooksLikeLabels, "tier").is_empty(),
        "{}",
        env.to_json()
    );
    assert!(
        role_hints(&env, Code::FieldLooksLikeProse, "summary").is_empty(),
        "{}",
        env.to_json()
    );
}

#[test]
fn graph_new_uses_std_v1_unless_no_std() {
    let mut home = Home::new("std-new");
    let plain = std_and_plain(&mut home);
    let path = home.path.join("g").display().to_string();
    assert_eq!(graph_json(&path)["ontology"]["std"], json!(1));
    assert!(graph_json(&plain)["ontology"].get("std").is_none());
    // --empty drops the starter vocabulary, not std.
    let bare = home.path.join("bare").display().to_string();
    let env = home.ok(&["graph", "new", &bare, "--alias", "bare", "--empty"]);
    assert_eq!(env.data["ontology"]["std"], json!(1), "{}", env.to_json());
    let o = graph_json(&bare)["ontology"].clone();
    assert_eq!((&o["std"], &o["types"]), (&json!(1), &json!([])));
}

#[test]
fn a_graph_without_std_keeps_graph_json_byte_for_byte_and_a_std_one_never_writes_std_entries() {
    let mut home = Home::new("std-bytes");
    let plain = std_and_plain(&mut home);
    let file = Path::new(&plain).join("graph.json");
    let before = fs::read_to_string(&file).unwrap();
    home.ok(&["graph", "load", &plain]);
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        before,
        "a load writes nothing"
    );
    home.ok(&["ontology", "add", "type", "widget", "--graph", "plain"]);
    let o = graph_json(&plain)["ontology"].clone();
    assert!(
        o.get("std").is_none() && !o.to_string().contains("job_title"),
        "{o}"
    );
    home.ok(&["ontology", "remove", "type", "widget", "--graph", "plain"]);
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        before,
        "only the write's own change, undone"
    );
    // A std graph's write keeps `std: 1` and writes only its own declarations.
    let path = home.path.join("g").display().to_string();
    home.ok(&["ontology", "add", "status", "parked", "--graph", "demo"]);
    let o = graph_json(&path)["ontology"].clone();
    assert_eq!(o["std"], json!(1));
    let types: Vec<&str> = o["types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        umx_core::ontology::STARTER_TYPES,
        "no std type written"
    );
    assert!(!strings(&o["statuses"]).contains(&"stalled") && o.get("relation_fields").is_none());
}

#[test]
fn ontology_show_marks_std_entries_and_std_prints_std_v1_alone() {
    let mut home = Home::new("std-show");
    std_and_plain(&mut home);
    let env = home.ok(&["ontology", "show", "--graph", "demo"]);
    let d = &env.data;
    assert_eq!(d["std"], json!(1));
    assert_eq!(d["types"][0]["name"], "person", "std first");
    assert_eq!(named(&d["types"], "person")["std"], json!(true));
    assert_eq!(named(&d["types"], "problem")["std"], json!(false));
    // question is in both the starter vocabulary and std: merged into std's entry, once.
    assert_eq!(named(&d["types"], "question")["std"], json!(true));
    let job = named(&named(&d["types"], "person")["fields"], "job_title");
    assert_eq!(
        (&job["std"], &job["kind"], &job["role"]),
        (&json!(true), &json!("string"), &json!("data"))
    );
    assert_eq!(
        named(&d["relation_fields"]["works_for"], "starts_on")["std"],
        json!(true)
    );
    let std_statuses = strings(&d["std_statuses"]);
    assert!(
        std_statuses.contains(&"stalled") && !std_statuses.contains(&"open"),
        "{std_statuses:?}"
    );
    let std_relations = strings(&d["std_relations"]);
    assert!(std_relations.contains(&"works_for") && std_relations.contains(&"answers"));
    assert!(!std_relations.contains(&"supports"));
    assert_eq!(named(&d["std_any_fields"], "tags")["on"], "node");
    let hint = issue(&env, Code::StdEntriesShown);
    assert_eq!(hint.next(), "umx ontology show --std");
    // The graph's own field on a std type follows std's.
    home.ok(&[
        "ontology",
        "add",
        "field",
        "person",
        "desk:string",
        "--graph",
        "demo",
    ]);
    let env = home.ok(&["ontology", "show", "--graph", "demo"]);
    let fields = &named(&env.data["types"], "person")["fields"];
    let names: Vec<&str> = fields
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["aliases", "job_title", "org", "desk"]);
    assert_eq!(named(fields, "desk")["std"], json!(false));

    // --std: std v1 alone, every entry std.
    let env = home.ok(&["ontology", "show", "--std"]);
    let d = &env.data;
    assert_eq!(d["std"], json!(1));
    let types: Vec<&str> = d["types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    let std_types: Vec<&str> = umx_core::standard::types().map(|t| t.name).collect();
    assert_eq!(types, std_types);
    assert!(
        d["types"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["std"] == json!(true))
    );
    let statuses: Vec<&str> = umx_core::standard::STATUSES
        .iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(
        (strings(&d["statuses"]), strings(&d["std_statuses"])),
        (statuses.clone(), statuses)
    );
    assert!(env.hints.is_empty(), "{}", env.to_json());

    // A graph without std marks nothing, not even a std name it declares.
    let env = home.ok(&["ontology", "show", "--graph", "plain"]);
    let d = &env.data;
    assert_eq!(d["std"], json!(null));
    assert_eq!(named(&d["types"], "question")["std"], json!(false));
    assert!(d.get("std_any_fields").is_none() && d["std_statuses"] == json!([]));
    assert!(env.hints.is_empty(), "{}", env.to_json());
}

#[test]
fn a_newer_std_loads_with_std_newer_on_std_v1_names() {
    let mut home = Home::new("std-newer");
    let path = home.graph();
    edit_graph_json(&path, |g| g["ontology"]["std"] = json!(2));
    let (env, _) = home.umx(&["graph", "load", &path]);
    let w = issue(&env, Code::StdNewer);
    assert!(
        w.message().contains("std v2") && w.message().contains("std v1"),
        "{}",
        w.message()
    );
    assert_eq!(w.next(), "umx ontology show --std");
    let show = home.ok(&["ontology", "show"]);
    assert_eq!(show.data["std"], json!(2));
    assert_eq!(named(&show.data["types"], "person")["std"], json!(true));
    // A write keeps the file's version.
    home.ok(&["ontology", "add", "status", "parked"]);
    assert_eq!(graph_json(&path)["ontology"]["std"], json!(2));
}

#[test]
fn graph_guide_lists_std_entries_only_when_declared_or_used_and_never_asks_to_describe_them() {
    let mut home = Home::new("std-guide");
    home.graph();
    let env = home.ok(&["graph", "guide", "demo"]);
    let names: Vec<&str> = env.data["types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        !names.contains(&"person") && !names.contains(&"meeting"),
        "{names:?}"
    );
    // question is declared (the starter vocabulary) and std: listed, marked, and describable.
    let question = named(&env.data["types"], "question");
    assert_eq!(question["std"], json!(true));
    assert!(question["next"].is_string());
    issue(&env, Code::StdEntriesShown);
    let missing =
        |env: &Envelope| -> String { issue(env, Code::DescriptionsMissing).message().to_owned() };
    let before = missing(&env);
    home.add("person", "Ada");
    let env = home.ok(&["graph", "guide", "demo"]);
    let person = named(&env.data["types"], "person");
    assert_eq!(
        (&person["std"], &person["declared"], &person["next"]),
        (&json!(true), &json!(true), &json!(null))
    );
    assert_eq!(named(&person["fields"], "job_title")["next"], json!(null));
    assert_eq!(
        missing(&env),
        before,
        "a std entry graph.json does not declare is never counted"
    );
}

#[test]
fn a_role_hint_on_a_std_field_runs_as_printed_and_writes_only_the_graph_s_own_declaration() {
    // req-98, D69: role_hints read the effective ontology, so `ontology set field` must take a std field too.
    let mut home = Home::new("std-role");
    let path = home.graph();
    for (title, job) in [
        (
            "Ada",
            "Leads the quarterly planning of every regional warehouse team",
        ),
        (
            "Bo",
            "Writes the safety rules that each night shift follows on site",
        ),
        (
            "Cy",
            "Keeps the supplier contracts in order and answers their questions",
        ),
    ] {
        let set = format!("job_title={job}");
        home.ok(&[
            "node", "add", "--type", "person", "--title", title, "--set", &set,
        ]);
    }
    let env = home.ok(&["graph", "guide", "demo"]);
    let hint = env
        .hints
        .iter()
        .find(|h| h.code() == "field_looks_like_prose" && h.message().contains("job_title"))
        .unwrap_or_else(|| panic!("no prose hint for job_title: {}", env.to_json()));
    let (done, exit) = run_as_printed(&home, hint.next());
    assert_eq!(exit, 0, "{}", done.to_json());
    let show = home.ok(&["ontology", "show"]);
    let person = named(&show.data["types"], "person");
    assert_eq!(named(&person["fields"], "job_title")["role"], "content");
    assert_eq!(named(&person["fields"], "aliases")["std"], json!(true));
    assert_eq!(
        type_in_file(&path, "person")["fields"],
        json!([{ "name": "job_title", "kind": "string", "role": "content" }]),
        "graph.json holds the graph's own declaration, never std's other entries"
    );
}

#[test]
fn a_std_graph_keeps_std_s_names_and_kinds_and_a_plain_one_takes_them_as_its_own() {
    let mut home = Home::new("t2b-std-names");
    let plain = std_and_plain(&mut home);
    let demo = home.path.join("g").display().to_string();
    let file = |path: &str| fs::read_to_string(Path::new(path).join("graph.json")).unwrap();
    let before = file(&demo);
    // A std type or relation by name alone is std's already: unchanged, nothing written.
    for args in [
        ["ontology", "add", "type", "person"],
        ["ontology", "add", "type", "std:person"],
        ["ontology", "add", "relation", "works_for"],
        ["ontology", "add", "relation", "std:member_of"],
    ] {
        let env = home.ok(&[&args[..], &["--graph", "demo"][..]].concat());
        assert_eq!(
            env.data["unchanged"],
            json!(true),
            "{args:?}: {}",
            env.to_json()
        );
    }
    // A field keeps std's kind where std places it.
    for args in [
        &["ontology", "add", "field", "person", "job_title:int"][..],
        &["ontology", "add", "field", "risk,topic", "due_on:string"][..],
        &[
            "ontology",
            "add",
            "type",
            "person",
            "--field",
            "aliases:string",
        ][..],
        &[
            "ontology",
            "add",
            "field",
            "--relation",
            "works_for",
            "starts_on:string",
        ][..],
    ] {
        let env = refused_with(
            &home,
            &[args, &["--graph", "demo"][..]].concat(),
            Code::StdFieldConflict,
        );
        let found = issue(&env, Code::StdFieldConflict);
        assert_eq!(found.next(), "umx ontology show --std");
    }
    assert_eq!(file(&demo), before, "nothing written");
    // With std's kind it is the graph's own declaration, and `std:` names a std type.
    home.ok(&[
        "ontology",
        "add",
        "field",
        "std:topic",
        "due_on:date",
        "--graph",
        "demo",
    ]);
    let person = home.ok(&[
        "node",
        "add",
        "--type",
        "std:person",
        "--title",
        "Ada",
        "--graph",
        "demo",
    ]);
    assert_eq!(person.data["node"]["type"], json!("person"));
    assert!(!file(&demo).contains("std:"), "names are stored plain");
    // A provenance type on a user node warns, as before; the node is stored.
    let env = home.ok(&[
        "node", "add", "--type", "session", "--title", "x", "--graph", "demo",
    ]);
    issue(&env, Code::ReservedType);
    // The same names in a graph without std are its own: declared and written.
    for args in [
        &["ontology", "add", "type", "person"][..],
        &["ontology", "add", "field", "person", "job_title:int"][..],
        &["ontology", "add", "relation", "works_for"][..],
    ] {
        let env = home.ok(&[args, &["--graph", "plain"][..]].concat());
        assert_eq!(env.data["unchanged"], json!(false), "{args:?}");
    }
    assert!(!file(&plain).contains("relation_endpoints"));
}

#[test]
fn a_custom_field_on_a_std_type_is_written_alone_and_works_as_any_field() {
    // req-100 clauses 1, 2 and 5: graph.json holds only the extension; show lists std's first; the extension is a
    // field to node add and edit, batch, --where, word search, roles and Cypher.
    let mut home = Home::new("t2c-extension");
    std_and_plain(&mut home);
    let demo = home.path.join("g").display().to_string();
    let added = home.ok(&on(
        "demo",
        &["ontology", "add", "field", "person", "shoe_size:int"],
    ));
    assert_eq!(added.data["unchanged"], json!(false));
    assert_eq!(
        type_in_file(&demo, "person"),
        json!({ "name": "person", "fields": [{ "name": "shoe_size", "kind": "int" }] }),
        "only the extension, never std's fields"
    );
    assert_eq!(
        shown_fields(&home, "demo", "person"),
        [
            ("aliases".to_owned(), json!(true)),
            ("job_title".to_owned(), json!(true)),
            ("org".to_owned(), json!(true)),
            ("shoe_size".to_owned(), json!(false)),
        ]
    );
    home.ok(&on(
        "demo",
        &["ontology", "add", "field", "person", "nickname:string"],
    ));
    let ada = home.ok(&on(
        "demo",
        &[
            "node",
            "add",
            "--type",
            "person",
            "--title",
            "Ada",
            "--set",
            "shoe_size=42",
            "--set",
            "nickname=Countess of quartermasters",
        ],
    ));
    assert!(ada.warnings.is_empty(), "{}", ada.to_json());
    let ada = ada.data["node"]["id"].as_str().unwrap().to_owned();
    let edited = home.ok(&on(
        "demo",
        &["node", "edit", &ada, "--set", "shoe_size=43"],
    ));
    assert!(edited.warnings.is_empty(), "{}", edited.to_json());
    let (bad, _) = home.umx(&on(
        "demo",
        &["node", "edit", &ada, "--set", "shoe_size=big"],
    ));
    issue(&bad, Code::FieldKindMismatch);
    home.ok(&on(
        "demo",
        &["node", "edit", &ada, "--set", "shoe_size=43"],
    ));
    let (done, exit) = batch_of(
        &home,
        "demo",
        &[json!({"op":"node add","type":"person","title":"Bo",
            "sets":[{"field":"shoe_size","value":40},{"field":"nickname","value":"Bosun"}]})],
    );
    assert_eq!(exit, 0, "{}", done.to_json());
    assert!(done.warnings.is_empty(), "{}", done.to_json());
    let env = home.ok(&on("demo", &["search", "countess"]));
    assert_eq!(hit(&env, "Ada")["text_field"], "nickname");
    let env = home.ok(&on("demo", &["search", "bosun"]));
    assert_eq!(hit(&env, "Bo")["text_field"], "nickname");
    let env = home.ok(&on(
        "demo",
        &[
            "node",
            "list",
            "--type",
            "person",
            "--where",
            "shoe_size=43",
        ],
    ));
    let ids: Vec<&str> = env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [ada.as_str()]);
    let env = home.ok(&on(
        "demo",
        &[
            "query",
            "MATCH (p:person) WHERE p.shoe_size > 41 RETURN p.title AS t, p.nickname AS n",
        ],
    ));
    assert_eq!(
        env.data["rows"],
        json!([{ "t": "Ada", "n": "Countess of quartermasters" }])
    );
    // A role on the extension: graph.json holds it, and Ada's vector is pending.
    let set = home.ok(&on(
        "demo",
        &[
            "ontology", "set", "field", "person", "nickname", "--role", "content",
        ],
    ));
    assert_eq!(set.data["changed"], json!(true), "{}", set.to_json());
    assert_eq!(
        type_in_file(&demo, "person")["fields"][1],
        json!({ "name": "nickname", "kind": "string", "role": "content" })
    );
    // Removing an unused extension removes only it; std's fields stay.
    home.ok(&on(
        "demo",
        &["ontology", "add", "field", "person", "badge:string"],
    ));
    let removed = home.ok(&on(
        "demo",
        &["ontology", "remove", "field", "person", "badge"],
    ));
    assert_eq!(removed.data["unchanged"], json!(false));
    let names: Vec<String> = shown_fields(&home, "demo", "person")
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(
        names,
        ["aliases", "job_title", "org", "shoe_size", "nickname"]
    );
    // An extension a node holds is in use, as in any graph.
    refused_with(
        &home,
        &on(
            "demo",
            &["ontology", "remove", "field", "person", "shoe_size"],
        ),
        Code::OntologyInUse,
    );
}

#[test]
fn several_types_take_a_field_with_std_s_types_counted_as_declared() {
    // req-100, t2b-1's lesson: `type_unknown` on a std type had the no-op `ontology add type` as its next. The docs
    // example `risk,issue,task` and usage 103's `risk,issue` run on a new std graph.
    let mut home = Home::new("t2c-many");
    let path = home.graph();
    let (env, exit) = home.umx(&["ontology", "add", "field", "risk,issue,task", "impact:int"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let unknown: Vec<(&str, &str)> = env.errors.iter().map(|e| (e.code(), e.message())).collect();
    assert_eq!(unknown.len(), 2, "risk is std's: {unknown:?}");
    assert!(
        unknown
            .iter()
            .all(|(code, message)| *code == "type_unknown" && !message.contains("risk")),
        "{unknown:?}"
    );
    for e in &env.errors {
        let (next, exit) = run_as_printed(&home, e.next());
        assert_eq!(exit, 0, "{}", next.to_json());
        assert_eq!(next.data["unchanged"], json!(false), "{}", next.to_json());
    }
    let env = home.ok(&["ontology", "add", "field", "risk,issue,task", "impact:int"]);
    assert_eq!(env.data["types"], json!(["risk", "issue", "task"]));
    assert_eq!(
        type_in_file(&path, "risk"),
        json!({ "name": "risk", "fields": [{ "name": "impact", "kind": "int" }] })
    );
    let env = home.ok(&["ontology", "add", "field", "risk,issue", "impact:int"]);
    assert_eq!(env.data["unchanged"], json!(["risk", "issue"]));
    // --all-types means graph.json's types, never std's.
    home.ok(&["ontology", "add", "field", "--all-types", "owner:string"]);
    let meta = graph_json(&path);
    assert!(
        !meta["ontology"]["types"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "team"),
        "{meta}"
    );
}

#[test]
fn std_s_own_fields_are_never_removed_and_the_graph_s_role_for_one_is() {
    // req-100 clause 5: remove field takes out only what the graph added.
    let mut home = Home::new("t2c-remove");
    let plain = std_and_plain(&mut home);
    let demo = home.path.join("g").display().to_string();
    let file = |path: &str| fs::read_to_string(Path::new(path).join("graph.json")).unwrap();
    let before = file(&demo);
    for args in [
        &["ontology", "remove", "field", "person", "aliases"][..],
        &["ontology", "remove", "field", "person", "tags"][..],
        &[
            "ontology",
            "remove",
            "field",
            "--relation",
            "works_for",
            "starts_on",
        ][..],
    ] {
        let env = refused_with(
            &home,
            &[args, &["--graph", "demo"][..]].concat(),
            Code::ReservedName,
        );
        assert!(
            issue(&env, Code::ReservedName)
                .message()
                .contains("std field"),
            "{}",
            env.to_json()
        );
    }
    assert_eq!(file(&demo), before, "nothing written");
    // The graph's own role for a std field goes, even while nodes hold the field: std's declaration backs them.
    home.ok(&[
        "node",
        "add",
        "--type",
        "person",
        "--title",
        "Ada",
        "--set",
        "job_title=Quartermaster",
        "--graph",
        "demo",
    ]);
    home.ok(&[
        "ontology",
        "set",
        "field",
        "person",
        "job_title",
        "--role",
        "content",
        "--graph",
        "demo",
    ]);
    assert!(file(&demo).contains("job_title"));
    let env = home.ok(&[
        "ontology",
        "remove",
        "field",
        "person",
        "job_title",
        "--graph",
        "demo",
    ]);
    assert_eq!(env.data["unchanged"], json!(false), "{}", env.to_json());
    assert!(!file(&demo).contains("job_title"));
    // req-97: the field is data again, so Ada's embedded text changed and her vector is pending, as after
    // `ontology set field person job_title --role data`.
    let moved = issue(&env, Code::RoleChanged);
    assert!(
        moved.message().contains("is now data: 1 nodes"),
        "{}",
        env.to_json()
    );
    let show = home.ok(&["ontology", "show", "--graph", "demo"]);
    let job = named(&named(&show.data["types"], "person")["fields"], "job_title");
    assert_eq!((&job["role"], &job["std"]), (&json!("data"), &json!(true)));
    // A graph without std has no std fields: nothing declared, nothing to refuse.
    let before = file(&plain);
    let env = home.ok(&[
        "ontology", "remove", "field", "person", "aliases", "--graph", "plain",
    ]);
    assert_eq!(env.data["unchanged"], json!(true));
    assert_eq!(file(&plain), before);
}

#[test]
fn a_description_for_a_std_field_is_the_graph_s_own_declaration_of_it() {
    // req-100 review F2: std's entry has no description, so a described std field is written as the graph's own
    // (std's kind), never already_declared with a next that loops on description_undeclared.
    let mut home = Home::new("t2c-describe");
    let path = home.graph();
    let env = home.ok(&[
        "ontology",
        "add",
        "field",
        "person",
        "org:string",
        "--description",
        "The employer as the source names it",
    ]);
    assert_eq!(env.data["unchanged"], json!(false), "{}", env.to_json());
    let printed: Vec<String> = env
        .hints
        .iter()
        .chain(&env.warnings)
        .map(|i| i.next().to_owned())
        .collect();
    assert_eq!(
        type_in_file(&path, "person")["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == "org")
            .unwrap()["description"],
        json!("The employer as the source names it")
    );
    let show = home.ok(&["ontology", "show"]);
    let org = named(&named(&show.data["types"], "person")["fields"], "org");
    assert_eq!(
        (&org["kind"], &org["description"]),
        (
            &json!("string"),
            &json!("The employer as the source names it")
        )
    );
    // The same again is unchanged; another description is already_declared, and its next now runs.
    let again = home.ok(&[
        "ontology",
        "add",
        "field",
        "person",
        "org:string",
        "--description",
        "The employer as the source names it",
    ]);
    assert_eq!(again.data["unchanged"], json!(true));
    let (env, exit) = home.umx(&[
        "ontology",
        "add",
        "field",
        "person",
        "org:string",
        "--description",
        "Who pays them",
    ]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let next = issue(&env, Code::AlreadyDeclared)
        .next()
        .replace("<text>", "\"Who pays them\"");
    let (done, exit) = run_as_printed(&home, &next);
    assert_eq!(exit, 0, "{next}: {}", done.to_json());
    // Several types at once do the same: the described std field is written on each.
    let env = home.ok(&[
        "ontology",
        "add",
        "field",
        "person,organisation",
        "aliases:list",
        "--description",
        "Other names it goes by",
    ]);
    assert_eq!(
        env.data["types"],
        json!(["person", "organisation"]),
        "{}",
        env.to_json()
    );
    // The first add's printed nexts run as printed.
    assert!(!printed.is_empty());
    for next in &printed {
        let (done, exit) = run_as_printed(&home, next);
        assert_eq!(exit, 0, "{next}: {}", done.to_json());
    }
}

#[test]
fn a_stored_any_node_std_field_on_a_custom_type_is_in_use_while_a_node_holds_it() {
    // req-100 review F3: std allows starts_on on any node, but places it only on the dated types; a widget's own
    // starts_on, written by an older umx, is the graph's field and keeps the in-use rule.
    let mut home = Home::new("t2c-any-node");
    let path = home.graph();
    edit_graph_json(&path, |g| {
        g["ontology"]["types"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "name": "widget", "fields": [{ "name": "starts_on", "kind": "date" }] }));
    });
    home.ok(&["graph", "load", &path]);
    home.ok(&[
        "node",
        "add",
        "--type",
        "widget",
        "--title",
        "Gear",
        "--set",
        "starts_on=2026-01",
    ]);
    let before = fs::read_to_string(Path::new(&path).join("graph.json")).unwrap();
    refused_with(
        &home,
        &["ontology", "remove", "field", "widget", "starts_on"],
        Code::OntologyInUse,
    );
    assert_eq!(
        fs::read_to_string(Path::new(&path).join("graph.json")).unwrap(),
        before
    );
}

#[test]
fn std_relations_expect_end_types_softly_and_a_graph_widens_them() {
    let mut home = Home::new("t2b-ends");
    std_and_plain(&mut home);
    let demo = home.path.join("g").display().to_string();
    let add = |g: &str, kind: &str, title: &str| -> String {
        let env = home.ok(&[
            "node", "add", "--type", kind, "--title", title, "--graph", g,
        ]);
        env.data["node"]["address"].as_str().unwrap().to_owned()
    };
    let (ada, bo, team, acme) = (
        add("demo", "person", "Ada"),
        add("demo", "person", "Bo"),
        add("demo", "team", "Platform"),
        add("demo", "organisation", "Acme"),
    );
    // Outside std's ends: stored, with the warning naming the expected types.
    let env = home.ok(&["edge", "add", &ada, "works_for", &team]);
    let w = issue(&env, Code::RelationEndpointUnexpected);
    assert!(
        w.message().contains("team") && w.message().contains("organisation"),
        "{}",
        w.message()
    );
    assert!(
        env.data["edge"]["id"].is_string(),
        "stored: {}",
        env.to_json()
    );
    let quiet = home.ok(&["edge", "add", &ada, "works_for", &acme]);
    assert!(quiet.warnings.is_empty(), "{}", quiet.to_json());
    let (meeting, place, question) = (
        add("demo", "meeting", "Kickoff"),
        add("demo", "place", "Room 4"),
        add("demo", "question", "Which vendor?"),
    );
    for (from, relation, to) in [
        (&meeting, "held_at", &place),
        (&question, "raised_by", &team),
        (&ada, "member_of", &team),
        (&team, "member_of", &acme),
        (&ada, "owns", &question),
    ] {
        let env = home.ok(&["edge", "add", from, relation, to]);
        assert!(env.warnings.is_empty(), "{relation}: {}", env.to_json());
    }
    let env = home.ok(&["edge", "add", &ada, "raised_by", &acme]);
    issue(&env, Code::RelationEndpointUnexpected);
    // A graph widens them, stored in graph.json; then the same call is quiet.
    let widened = home.ok(&[
        "ontology",
        "add",
        "relation",
        "std:works_for",
        "--from",
        "std:person",
        "--to",
        "team",
        "--graph",
        "demo",
    ]);
    assert_eq!(
        widened.data["unchanged"],
        json!(false),
        "{}",
        widened.to_json()
    );
    assert_eq!(
        graph_json(&demo)["ontology"]["relation_endpoints"],
        json!({ "works_for": { "from": ["person"], "to": ["team"] } })
    );
    let again = home.ok(&[
        "ontology",
        "add",
        "relation",
        "works_for",
        "--to",
        "team",
        "--graph",
        "demo",
    ]);
    assert_eq!(again.data["unchanged"], json!(true));
    let quiet = home.ok(&["edge", "add", &bo, "std:works_for", &team]);
    assert!(quiet.warnings.is_empty(), "{}", quiet.to_json());
    assert_eq!(quiet.data["edge"]["type"], json!("works_for"));
    // Widening adds to std's ends, never replaces them: a person to an organisation stays quiet.
    let cy = add("demo", "person", "Cy");
    let still = home.ok(&["edge", "add", &cy, "works_for", &acme]);
    assert!(still.warnings.is_empty(), "{}", still.to_json());
    // A symmetric relation has no direction, so it takes no end types: with --symmetric, after it, or before it.
    let usage = |args: &[&str]| {
        let (env, exit) = home.umx(args);
        assert_eq!(exit, 2, "{args:?}: {}", env.to_json());
        issue(&env, Code::UsageInvalid);
    };
    let g = ["--graph", "demo"];
    usage(
        &[
            &[
                "ontology",
                "add",
                "relation",
                "rivals",
                "--symmetric",
                "--from",
                "team",
            ][..],
            &g,
        ]
        .concat(),
    );
    home.ok(&[
        &["ontology", "add", "relation", "rivals", "--symmetric"][..],
        &g,
    ]
    .concat());
    usage(
        &[
            &["ontology", "add", "relation", "rivals", "--from", "team"][..],
            &g,
        ]
        .concat(),
    );
    usage(
        &[
            &[
                "ontology",
                "add",
                "relation",
                "conflicts_with",
                "--to",
                "team",
            ][..],
            &g,
        ]
        .concat(),
    );
    home.ok(&[
        &[
            "ontology",
            "add",
            "relation",
            "pairs_with",
            "--from",
            "team",
        ][..],
        &g,
    ]
    .concat());
    usage(
        &[
            &["ontology", "add", "relation", "pairs_with", "--symmetric"][..],
            &g,
        ]
        .concat(),
    );
    assert_eq!(
        graph_json(&demo)["ontology"]["relation_endpoints"],
        json!({ "works_for": { "from": ["person"], "to": ["team"] }, "pairs_with": { "from": ["team"] } })
    );
    // A graph without std expects nothing of works_for.
    let (p, t) = (add("plain", "person", "Cy"), add("plain", "team", "Ops"));
    let env = home.ok(&["edge", "add", &p, "works_for", &t]);
    assert!(
        !env.to_json().contains("relation_endpoint_unexpected"),
        "{}",
        env.to_json()
    );
    // A batch warns as edge add does (one code path).
    let (env, exit) = batch_of(
        &home,
        "demo",
        &[json!({"op":"edge add","from":ada,"relation":"member_of","to":question})],
    );
    assert_eq!(exit, 0, "{}", env.to_json());
    issue(&env, Code::RelationEndpointUnexpected);
}

#[test]
fn an_older_umx_s_stored_authored_by_edge_loads_kept_and_warned() {
    let mut home = Home::new("t2b-stored");
    let path = home.graph();
    let (a, b) = (home.add("problem", "First"), home.add("problem", "Second"));
    let id = edge_id(&home.umx(&["edge", "add", &a, "blocks", &b]).0);
    home.ok(&["graph", "unload", "demo"]);
    let file = Path::new(&path).join("edges.jsonl");
    let text = fs::read_to_string(&file)
        .unwrap()
        .replace("\"blocks\"", "\"authored_by\"");
    fs::write(&file, &text).unwrap();
    let env = home.ok(&["graph", "load", &path]);
    let w = issue(&env, Code::ReservedRelationStored);
    assert_eq!(w.next(), format!("umx edge show {id}"));
    assert_eq!(fs::read_to_string(&file).unwrap(), text, "nothing dropped");
    let shown = home.ok(&["edge", "show", &id]);
    assert_eq!(
        shown.data["type"],
        json!("authored_by"),
        "{}",
        shown.to_json()
    );
}

#[test]
fn std_names_are_plain_in_cypher_and_derived_authorship_reads_as_before_in_a_std_graph() {
    // ws008 T2b (req-99): std's qualifier is refused in Cypher; the derived authored_by rules (ws007) hold in a std
    // graph.
    let mut home = Home::new("t2b-cypher");
    home.graph();
    let (ada, team) = (home.add("person", "Ada"), home.add("team", "Platform"));
    home.ok(&["edge", "add", &ada, "member_of", &team]);
    for (text, plain) in [
        ("MATCH (n:std:person) RETURN n", "person"),
        ("MATCH (n:`std:person`) RETURN n", "person"),
        ("MATCH (a)-[:std:member_of]->(b) RETURN a", "member_of"),
    ] {
        let (env, exit) = home.query(text, &[]);
        assert_eq!(exit, 1, "{text}: {}", env.to_json());
        let refused = issue(&env, Code::QueryStdQualifier);
        assert_eq!(
            refused.message(),
            format!("std names are plain in Cypher: write :{plain}."),
            "{text}"
        );
        assert_eq!(refused.next(), "umx docs get query");
    }
    let env = home.ok(&[
        "query",
        "MATCH (p:person)-[:member_of]->(t:team) RETURN p.id AS id",
    ]);
    assert_eq!(
        env.data["rows"],
        json!([{ "id": ada }]),
        "{}",
        env.to_json()
    );
    let authored = home.ok(&[
        "query",
        "MATCH (n)-[r:authored_by]->(s:session) RETURN n.id AS node, r AS edge",
        "--provenance",
    ]);
    let rows = authored.data["rows"].as_array().unwrap();
    assert!(
        rows.iter().any(|row| row["node"] == json!(ada)),
        "{}",
        authored.to_json()
    );
    for row in rows {
        assert_eq!(row["edge"]["derived"], json!(true), "{row}");
        assert_eq!(row["edge"]["type"], json!("authored_by"), "{row}");
        assert!(row["edge"].get("id").is_none_or(Value::is_null), "{row}");
    }
    let untyped = home.ok(&[
        "query",
        "MATCH (n)-[r]->(s:session) RETURN count(r) AS total",
        "--provenance",
    ]);
    assert_eq!(untyped.data["rows"], json!([{ "total": 0 }]));
    let (env, exit) = home.query("MATCH (n)-[:authored_by*1..2]->(s) RETURN n.id AS id", &[]);
    assert_eq!(exit, 1);
    issue(&env, Code::QueryFeatureNotSupported);
}

#[test]
fn ontology_set_takes_a_std_qualified_type() {
    // ws008 T2b: `std:` on ontology set type and ontology set field, stored plain.
    let mut home = Home::new("t2b-set-std");
    let path = home.graph();
    // person is std's, not in graph.json: the list (a std field included) is the graph's own declaration.
    let columns = home.ok(&[
        "ontology",
        "set",
        "type",
        "std:person",
        "--columns",
        "title,job_title",
    ]);
    assert_eq!(
        columns.data["type"],
        json!("person"),
        "{}",
        columns.to_json()
    );
    home.add("person", "Ada");
    let listed = home.ok(&["node", "list", "--type", "person"]);
    assert_eq!(
        listed.data["columns"],
        json!(["id", "title", "job_title"]),
        "{}",
        listed.to_json()
    );
    home.ok(&["graph", "unload", "demo"]);
    let loaded = home.ok(&["graph", "load", &path]);
    assert!(loaded.warnings.is_empty(), "{}", loaded.to_json());
    let role = home.ok(&[
        "ontology",
        "set",
        "field",
        "std:person",
        "job_title",
        "--role",
        "content",
    ]);
    assert_eq!(role.data["type"], json!("person"), "{}", role.to_json());
    assert_eq!(
        type_in_file(&path, "person"),
        json!({ "name": "person", "fields": [{ "name": "job_title", "kind": "string", "role": "content" }],
            "columns": ["title", "job_title"] })
    );
    assert!(
        !fs::read_to_string(Path::new(&path).join("graph.json"))
            .unwrap()
            .contains("std:")
    );
    // A column is known as node list and the load rule know it: tags is not a field std places on team, and a
    // provenance type is umx's, never the graph's own declaration.
    let before = fs::read_to_string(Path::new(&path).join("graph.json")).unwrap();
    refused_with(
        &home,
        &["ontology", "set", "type", "team", "--columns", "title,tags"],
        Code::FieldMissing,
    );
    refused_with(
        &home,
        &["ontology", "set", "type", "session", "--columns", "title"],
        Code::ReservedName,
    );
    assert_eq!(
        fs::read_to_string(Path::new(&path).join("graph.json")).unwrap(),
        before,
        "nothing written"
    );
}
