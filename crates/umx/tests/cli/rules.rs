use super::*;

#[test]
fn unmet_warning_has_context_and_the_list_and_guide_sentence() {
    let mut home = Home::new("rule-checks");
    home.graph();
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
    assert_eq!(list.data["rows"][0]["sentence"], warning.message());
    let guide = home.ok(&["graph", "guide", "demo"]);
    assert_eq!(guide.data["rules"][0]["sentence"], warning.message());
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
