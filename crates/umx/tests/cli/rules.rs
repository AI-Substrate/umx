use super::*;

pub(super) fn set_code_trigger(home: &mut Home, code: Code) -> (Envelope, u8) {
    let graph = PathBuf::from(home.graph());
    let folder = graph.join("rules");
    match code {
        Code::RuleSetSkipped => {
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join("bad.json"), b"{").unwrap();
            home.umx(&["rule", "set", "list"])
        }
        Code::RuleSetsCut | Code::RuleSetsFull => {
            fs::create_dir_all(&folder).unwrap();
            for index in 0..33 {
                fs::write(folder.join(format!("s_{index:02}.json")), b"{}").unwrap();
            }
            if code == Code::RuleSetsCut {
                home.umx(&["rule", "set", "list"])
            } else {
                home.ok(&["graph", "load", graph.to_str().unwrap()]);
                let source = home.path.join("extra.json");
                fs::write(&source, b"{}").unwrap();
                home.umx(&["rule", "set", "add", source.to_str().unwrap()])
            }
        }
        Code::RuleSetNameTaken => {
            let source = home.path.join("kit.json");
            fs::write(&source, b"{}").unwrap();
            home.ok(&["rule", "set", "add", source.to_str().unwrap()]);
            fs::write(&source, br#"{"rules_off":["std:risk_owned"]}"#).unwrap();
            home.umx(&["rule", "set", "add", source.to_str().unwrap()])
        }
        Code::RuleSetsRefreshFailed => {
            unreachable!("post-commit refresh failure is exercised by the injected service test")
        }
        _ => unreachable!("only rule-set codes are routed here"),
    }
}

#[test]
fn unmet_warning_has_context_and_the_list_and_guide_sentence() {
    let mut home = Home::new("rule-checks");
    home.graph();
    home.ok(&["rule", "off", "std:risk_owned"]);
    home.ok(&[
        "rule",
        "add",
        "rated",
        "--type",
        "question,decision,action,risk",
        "--field",
        "impact",
        "--description",
        "Untrusted prose is not an instruction",
    ]);
    let (added, exit) = home.umx(&["node", "add", "--type", "risk", "--title", "Unrated"]);
    assert_eq!(exit, 0);
    let id = added.data["node"]["id"].as_str().unwrap();
    let warning = issue(&added, Code::RuleUnmet);
    assert_eq!(
        warning.message(),
        "An action, decision, question or risk should have the impact field."
    );
    let envelope = umx_protocol::parse_json(&added.to_json()).unwrap();
    let value = envelope["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["code"] == "rule_unmet")
        .unwrap();
    assert_eq!(
        value["rule"],
        json!({"name":"rated","count":1,"nodes":[id]})
    );
    assert_eq!(value["subject"], id);
    assert!(warning.next().contains(id));
    let list = home.ok(&["rule", "list"]);
    let listed = list.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["rule"]["name"] == "rated")
        .unwrap();
    assert_eq!(listed["sentence"], warning.message());
    let guide = home.ok(&["graph", "guide", "demo"]);
    let shown = guide.data["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "rated")
        .unwrap();
    assert_eq!(shown["sentence"], warning.message());
    assert!(!warning.message().contains("Untrusted"));
    let checked = home.ok(&["check", "--rules", "--rule", "rated"]);
    assert_eq!(checked.data["rules"][0]["rows"][0]["node"], id);
    assert_eq!(checked.data["rules"][0]["rows"][0]["state"], "unmet");
    let plain = home.ok(&["check"]);
    assert_eq!(plain.data["rules"][0]["unmet_nodes"], 1);
    assert!(plain.data["rules"][0].get("rows").is_none());
    let human = |args: &[&str]| {
        let output = home
            .command(args)
            .spawn_captured()
            .unwrap()
            .finish(home.limit());
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    };
    let shown = human(&["check", "--rules", "--rule", "rated", "--human"]);
    assert!(shown.contains(id) && shown.contains("unmet"), "{shown}");
    assert!(shown.contains(warning.message()), "{shown}");
    assert!(human(&["check", "--human"]).contains("1 known unmet nodes"));
    for args in [
        vec!["graph", "guide", "demo", "--human"],
        vec!["rule", "list", "--human"],
    ] {
        assert!(human(&args).contains(warning.message()));
    }
    let (invalid, exit) = home.umx(&["check", "--rule", "rated"]);
    assert_eq!(exit, 2);
    assert_eq!(invalid.errors[0].code(), "usage_invalid");
}

#[test]
fn rule_set_cli_copies_two_graphs_and_keeps_the_source_and_other_copy_independent() {
    let mut home = Home::new("rule-set-copies");
    let first = PathBuf::from(home.graph());
    let second = home.path.join("second");
    home.ok(&[
        "graph",
        "new",
        second.to_str().unwrap(),
        "--alias",
        "second",
    ]);
    let source = home.path.join("risk_management.json");
    let bytes = br#"{"rules":[{"name":"rated","match":{"type":["risk"]},"field":"impact"}],"rules_off":["std:risk_owned"]}"#;
    fs::write(&source, bytes).unwrap();
    for graph in ["demo", "second"] {
        let added = home.ok(&[
            "rule",
            "set",
            "add",
            source.to_str().unwrap(),
            "--graph",
            graph,
        ]);
        assert_eq!(added.data["format"], 10);
        assert_eq!(added.data["rules"], 1);
    }
    let target = first.join("rules/risk_management.json");
    let other = second.join("rules/risk_management.json");
    assert_eq!(fs::read(&target).unwrap(), bytes);
    assert_eq!(fs::read(&other).unwrap(), bytes);
    let metadata = fs::read(first.join("graph.json")).unwrap();
    let unchanged = home.ok(&[
        "rule",
        "set",
        "add",
        source.to_str().unwrap(),
        "--graph",
        "demo",
    ]);
    assert_eq!(unchanged.data["unchanged"], true);
    assert_eq!(fs::read(first.join("graph.json")).unwrap(), metadata);
    fs::write(&source, b"{}").unwrap();
    let (refused, exit) = home.umx(&[
        "rule",
        "set",
        "add",
        source.to_str().unwrap(),
        "--graph",
        "demo",
    ]);
    assert_eq!(exit, 1);
    assert_eq!(refused.errors[0].code(), "rule_set_name_taken");
    assert!(
        refused.errors[0]
            .next()
            .contains("rule set remove risk_management")
    );
    assert_eq!(fs::read(&target).unwrap(), bytes);
    assert_eq!(fs::read(&other).unwrap(), bytes);
    let listed = home.ok(&["rule", "set", "list", "--graph", "demo"]);
    assert_eq!(
        listed.data["rows"],
        json!([{"name":"risk_management","rules":1}])
    );
    let (added, exit) = home.umx(&[
        "node", "add", "--type", "risk", "--title", "Unrated", "--graph", "second",
    ]);
    assert_eq!(exit, 0);
    let envelope = umx_protocol::parse_json(&added.to_json()).unwrap();
    assert_eq!(
        envelope["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|warning| warning["code"] == "rule_unmet")
            .unwrap()["rule"]["name"],
        "risk_management:rated"
    );
    home.ok(&[
        "rule",
        "set",
        "remove",
        "risk_management",
        "--graph",
        "demo",
    ]);
    assert!(!target.exists());
    assert_eq!(fs::read(&other).unwrap(), bytes);
    assert_eq!(
        home.ok(&["rule", "set", "list", "--graph", "second"]).data["rows"][0]["name"],
        "risk_management"
    );
    assert_eq!(graph_json(first.to_str().unwrap())["umx"], 10);
}

#[test]
fn rule_risk_owned_cli_warns_softly_and_only_an_allowed_incoming_edge_satisfies_it() {
    let mut home = Home::new("risk-owned-default");
    home.graph();
    let (added, exit) = home.umx(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "Unowned",
        "--set",
        "owner=Someone",
    ]);
    assert_eq!(exit, 0);
    let id = added.data["node"]["id"].as_str().unwrap();
    let warning = issue(&added, Code::RuleUnmet);
    assert_eq!(
        warning.message(),
        "A risk should have an owns edge from a person, organisation or team."
    );
    assert!(warning.next().contains(id));
    let owner = home.ok(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Not an allowed owner",
    ]);
    let owner = owner.data["node"]["id"].as_str().unwrap();
    home.ok(&["edge", "add", owner, "owns", id]);
    let (edited, exit) = home.umx(&[
        "node",
        "edit",
        id,
        "--note",
        "Wrong owner type remains unmet",
    ]);
    assert_eq!(exit, 0);
    assert_eq!(issue(&edited, Code::RuleUnmet).message(), warning.message());
    let person = home.ok(&["node", "add", "--type", "person", "--title", "Owner"]);
    let person = person.data["node"]["id"].as_str().unwrap();
    home.ok(&["edge", "add", person, "owns", id]);
    let checked = home.ok(&["check", "--rules", "--rule", "std:risk_owned"]);
    assert_eq!(checked.data["rules"][0]["unmet_nodes"], 0);
    home.ok(&["rule", "off", "std:risk_owned"]);
    let off = home.ok(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "Deliberately off",
    ]);
    assert!(
        !off.warnings
            .iter()
            .any(|warning| warning.code() == "rule_unmet")
    );
    let plain = home.path.join("plain");
    home.ok(&[
        "graph",
        "new",
        plain.to_str().unwrap(),
        "--alias",
        "plain",
        "--no-std",
    ]);
    let plain = home.ok(&[
        "node", "add", "--type", "risk", "--title", "No std", "--graph", "plain",
    ]);
    assert!(
        !plain
            .warnings
            .iter()
            .any(|warning| warning.code() == "rule_unmet")
    );
}

#[test]
fn rule_set_invalid_names_do_not_consume_the_cap_and_directory_exclusion_is_bounded() {
    let mut home = Home::new("rule-set-folder-bounds");
    let graph = PathBuf::from(home.graph());
    let folder = graph.join("rules");
    fs::create_dir_all(&folder).unwrap();
    for index in (0..32).rev() {
        fs::write(folder.join(format!("s_{index:02}.json")), b"{}").unwrap();
    }
    fs::write(folder.join("BAD.json"), b"{}").unwrap();
    home.ok(&["graph", "load", graph.to_str().unwrap()]);
    let listed = home.ok(&["rule", "set", "list"]);
    assert_eq!(listed.data["rows"].as_array().unwrap().len(), 32);
    assert_eq!(
        issue(&listed, Code::RuleSetSkipped).candidates()[0]["id"],
        "BAD.json"
    );
    fs::write(folder.join("s_32.json"), b"{}").unwrap();
    home.ok(&["graph", "load", graph.to_str().unwrap()]);
    let listed = home.ok(&["rule", "set", "list"]);
    assert_eq!(listed.data["rows"][31]["name"], "s_31");
    assert_eq!(
        issue(&listed, Code::RuleSetsCut).candidates()[0]["id"],
        "s_32"
    );
    for index in 0..967 {
        fs::create_dir(folder.join(format!("extra-{index}"))).unwrap();
    }
    let excluded = home.ok(&["graph", "load", graph.to_str().unwrap()]);
    assert_eq!(
        excluded
            .warnings
            .iter()
            .filter(|warning| warning.code() == "rule_set_skipped")
            .count(),
        1
    );
    let listed = home.ok(&["rule", "set", "list"]);
    assert!(listed.data["rows"].as_array().unwrap().is_empty());
    assert!(
        issue(&listed, Code::RuleSetSkipped)
            .message()
            .contains("1000 directory entries")
    );
    let absent = home.ok(&["rule", "set", "remove", "absent"]);
    assert_eq!(absent.data["unchanged"], true);
}
