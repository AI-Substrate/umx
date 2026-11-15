//! The rule checks over the real workspace, and the self-test that proves every rule fires.
//! Run `cargo test -p umx-arch -- --nocapture` to see the per-rule table.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use umx_arch::{
    Blueprint, CRATE_DIR, Inputs, Rules, check, load_blueprint, load_rules, run_fixture,
    silent_rules,
};

fn rules() -> Rules {
    load_rules(&Path::new(CRATE_DIR).join("rules.toml")).expect("rules.toml loads")
}

fn repo_root() -> PathBuf {
    Path::new(CRATE_DIR)
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn real_blueprint(rules: &Rules) -> Blueprint {
    load_blueprint(&repo_root().join(&rules.blueprint)).expect("blueprint loads")
}

#[test]
fn workspace_obeys_every_rule() {
    let rules = rules();
    let blueprint = real_blueprint(&rules);
    let active = rules.active_ports();
    let report = check(&Inputs {
        root: &repo_root(),
        rules: &rules,
        blueprint: &blueprint,
        active_ports: &active,
        described: None,
        whole_workspace: true,
    })
    .expect("checker runs");
    for n in &report.notices {
        println!("inactive {n}");
    }
    for v in &report.violations {
        println!("VIOLATION {v}");
    }
    assert!(
        report.violations.is_empty(),
        "{} violation(s)",
        report.violations.len()
    );
}

#[test]
fn selftest_every_rule_fires_on_a_planted_fixture() {
    let rules = rules();
    let blueprint = real_blueprint(&rules);
    let mut dirs: Vec<PathBuf> = fs::read_dir(Path::new(CRATE_DIR).join("fixtures"))
        .expect("fixtures dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    let mut fired: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut allowed_samples = 0;
    for dir in &dirs {
        let outcome = run_fixture(dir, &rules, &blueprint, &repo_root()).expect("fixture runs");
        let ok = outcome.reported == outcome.expected;
        if !ok {
            failures.push(outcome.name.clone());
        }
        println!(
            "{:8} {:30} {:8} expect {:?} reported {:?}",
            if ok { "ok" } else { "MISMATCH" },
            outcome.name,
            format!("{:?}", outcome.enforcer).to_lowercase(),
            outcome.expected,
            outcome.reported
        );
        if !ok {
            for line in &outcome.detail {
                println!("         {line}");
            }
        }
        if outcome.expected.is_empty() {
            allowed_samples += 1;
        }
        for id in &outcome.reported {
            fired
                .entry(id.clone())
                .or_default()
                .push(outcome.name.clone());
        }
    }

    println!();
    for id in rules.ids() {
        match fired.get(&id) {
            Some(by) => println!("fired    {id:24} by {}", by.join(", ")),
            None => println!("SILENT   {id}"),
        }
    }
    let fired_ids: BTreeSet<String> = fired.into_keys().collect();
    let silent = silent_rules(&rules.ids(), &fired_ids);
    assert!(failures.is_empty(), "fixtures did not match: {failures:?}");
    assert!(
        allowed_samples > 0,
        "no allowed sample (a fixture expecting nothing)"
    );
    assert!(silent.is_empty(), "silent rules: {silent:?}");
}

/// The self-test's own fence: a rule with no planted fixture is reported under r-rules-fire.
#[test]
fn a_rule_without_a_plant_is_reported_silent() {
    let rules = rules();
    let fired: BTreeSet<String> = rules.ids().into_iter().collect();
    let mut ids = rules.ids();
    ids.push("r-planted-silent".to_string());
    let silent = silent_rules(&ids, &fired);
    assert_eq!(silent.len(), 1);
    assert_eq!(silent[0].rule, "r-rules-fire");
    assert!(silent[0].message.contains("r-planted-silent"));
}
