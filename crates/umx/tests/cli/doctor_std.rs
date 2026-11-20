//! ws008 t5: `umx doctor` hints `doctor_graph_not_std` for each loaded graph made without std. A module of the cli
//! tests (it uses their `Home` and helpers), kept in its own file.

use super::*;

/// The subjects of doctor's rows for `check`, in order.
fn subjects<'a>(env: &'a Envelope, check: &str) -> Vec<&'a str> {
    env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["check"] == check)
        .map(|r| r["subject"].as_str().unwrap())
        .collect()
}

#[test]
fn doctor_hints_adopt_std_for_a_graph_made_without_std() {
    let (env, exit) = trigger(Code::DoctorGraphNotStd);
    // A hint: the verdict and exit code stay as the other rows make them, and it is never a warning.
    assert_eq!(
        (exit, env.data["verdict"].as_str()),
        (0, Some("ok")),
        "{}",
        env.to_json()
    );
    assert!(
        env.warnings
            .iter()
            .all(|w| w.code() != "doctor_graph_not_std"),
        "{}",
        env.to_json()
    );
    let found = issue(&env, Code::DoctorGraphNotStd);
    assert!(
        found.message().contains("Graph demo"),
        "{}",
        found.message()
    );
    assert_eq!(
        found.next(),
        "umx ontology adopt-std --graph demo --dry-run"
    );
    let row = env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["check"] == "graph std")
        .expect("a graph std row");
    assert_eq!(
        (row["subject"].as_str(), row["outcome"].as_str()),
        (Some("demo"), Some("info")),
        "{}",
        env.to_json()
    );
}

#[test]
fn doctor_gives_a_std_graph_no_adopt_std_hint() {
    let mut home = Home::new("doctor_std_graph");
    home.graph();
    let (env, exit) = home.umx(&["doctor"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert!(
        env.hints
            .iter()
            .chain(&env.warnings)
            .all(|i| i.code() != "doctor_graph_not_std"),
        "{}",
        env.to_json()
    );
    let ok: Vec<&Value> = env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["check"] == "graph std")
        .collect();
    assert_eq!(ok.len(), 1, "{}", env.to_json());
    assert_eq!(ok[0]["outcome"], "ok", "{}", env.to_json());
}

/// The hint names only graphs `graph list` gave doctor, the same ones its folder rows name; a secured graph made
/// without std is asked about by name, as its check is, and no other graph appears.
#[test]
fn doctor_s_std_hint_names_only_the_graphs_it_already_lists() {
    let mut home = Home::new("doctor_std_secured");
    home.graph();
    let vault = home.path.join("vault").display().to_string();
    home.ok(&["graph", "new", &vault, "--alias", "vault", "--no-std"]);
    home.ok(&["graph", "set", "vault", "--policy", "secured"]);
    let (env, exit) = home.umx(&["doctor"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let listed: Vec<String> = home.ok(&["graph", "list"]).data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["alias"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        subjects(&env, "graph std"),
        subjects(&env, "graph folder"),
        "{}",
        env.to_json()
    );
    assert_eq!(subjects(&env, "graph std"), listed, "{}", env.to_json());
    let hinted: Vec<&str> = env
        .hints
        .iter()
        .filter(|h| h.code() == "doctor_graph_not_std")
        .map(Issue::next)
        .collect();
    assert_eq!(
        hinted,
        ["umx ontology adopt-std --graph vault --dry-run"],
        "{}",
        env.to_json()
    );
}
