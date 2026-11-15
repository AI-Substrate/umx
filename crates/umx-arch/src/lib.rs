//! Architecture rule checker (blueprint 001, c-arch).
//!
//! Rules are data: `rules.toml` carries one row per r-* decision row of the blueprint, and each id
//! has one checker here. [`check`] runs every checker over a workspace; the self-test runs it over
//! the planted fixtures in `fixtures/` and proves each rule fires.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

/// The crate's own directory, so tests find `rules.toml` and `fixtures/` without reading env.
pub const CRATE_DIR: &str = env!("CARGO_MANIFEST_DIR");

const ENV_LINT: &str = "clippy::disallowed_methods";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    pub rule: String,
    pub at: String,
    pub message: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}: {}", self.rule, self.at, self.message)
    }
}

fn violation(rule: &str, at: impl Into<String>, message: impl Into<String>) -> Violation {
    Violation {
        rule: rule.to_string(),
        at: at.into(),
        message: message.into(),
    }
}

// ---------------------------------------------------------------- rules.toml

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    pub blueprint: String,
    #[serde(rename = "rule")]
    pub rules: Vec<Rule>,
    /// Capability library -> its owner crates (r-vendor-crates-owned).
    #[serde(default)]
    pub owned: BTreeMap<String, Vec<String>>,
    /// Allow-lists in the blueprint's order, so the render matches d-4.
    #[serde(rename = "crate")]
    pub crates: Vec<CrateRow>,
    #[serde(default)]
    pub adapters: BTreeMap<String, String>,
    #[serde(default, rename = "port")]
    pub ports: Vec<PortRow>,
    /// Re-exports of a workspace crate or its items that are allowed, by name (r-inward, ap-10).
    #[serde(default, rename = "reexport")]
    pub reexports: Vec<ReexportRow>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    #[serde(default)]
    pub crates: Vec<String>,
    #[serde(rename = "crate")]
    pub krate: Option<String>,
    pub from: Option<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub allow: Vec<String>,
    pub root: Option<String>,
    pub fakes: Option<String>,
    pub feature: Option<String>,
    /// r-command-in-adapters and r-fs-in-adapters: single fns allowed by name, each with its reason.
    #[serde(default)]
    pub except: Vec<Except>,
    /// r-gate-features: the `<crate>/<feature>` list the gate's clippy builds (`umx-arch gate-features` prints it).
    #[serde(default)]
    pub gate: Vec<String>,
    /// r-gate-features: features the gate cannot build, each with its reason.
    #[serde(default)]
    pub gate_excluded: Vec<ExcludedFeature>,
}

/// One fn a rule lets through, by its crate-relative file and its name.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Except {
    pub file: String,
    #[serde(rename = "fn")]
    pub func: String,
    pub reason: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExcludedFeature {
    pub feature: String,
    pub reason: String,
}

impl Rules {
    /// The gate's feature list (r-gate-features), comma-separated for `cargo --features`.
    pub fn gate_features(&self) -> Option<String> {
        self.rules
            .iter()
            .find(|r| r.id == "r-gate-features")
            .map(|r| r.gate.join(","))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrateRow {
    pub name: String,
    pub workspace: Vec<String>,
    pub external: Vec<String>,
    #[serde(default)]
    pub optional: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortRow {
    pub id: String,
    pub module: String,
    /// The port trait under umx-core/src/ports; each trait there has at least one row (GraphFiles has two: the
    /// graph folders and the vector cache).
    #[serde(rename = "trait")]
    pub trait_name: String,
    pub stub: String,
    pub real: RealAdapter,
    pub state: String,
}

/// A port's real adapter: one id for every OS, a table keyed by OS (`std::env::consts::OS`, such as `macos`
/// or `linux`; an OS missing from the table wires none, so describe() must not list the port there, n-13), or a
/// list when config chooses (`real = ["a.x", "b.x"]`): real mode then wires one listed id, or none (the embedder
/// with `embedder.active = none`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RealAdapter {
    One(String),
    ByOs(BTreeMap<String, String>),
    AnyOf(Vec<String>),
}

impl RealAdapter {
    /// The adapter always wired on `os`, if any; `None` for a list, where config chooses.
    pub fn for_os(&self, os: &str) -> Option<&str> {
        match self {
            RealAdapter::One(id) => Some(id),
            RealAdapter::ByOs(table) => table.get(os).map(String::as_str),
            RealAdapter::AnyOf(_) => None,
        }
    }

    /// Every adapter id the row names, whatever the OS: each must run the full contract (r-fake-per-port).
    pub fn all(&self) -> Vec<&str> {
        match self {
            RealAdapter::One(id) => vec![id],
            RealAdapter::ByOs(table) => table.values().map(String::as_str).collect(),
            RealAdapter::AnyOf(ids) => ids.iter().map(String::as_str).collect(),
        }
    }
}

/// One allowed `pub use`: `krate` re-exports `path` (as written after `pub use`, without spaces).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReexportRow {
    #[serde(rename = "crate")]
    pub krate: String,
    pub path: String,
}

/// One row of `describe(mode)`: which adapter a mode wires for a port.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub struct WiringRow {
    pub mode: String,
    pub port: String,
    pub adapter: String,
}

pub fn load_rules(path: &Path) -> Result<Rules, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))
}

impl Rules {
    pub fn ids(&self) -> Vec<String> {
        self.rules.iter().map(|r| r.id.clone()).collect()
    }

    /// Port rows the checkers enforce: everything that has left "planned".
    pub fn active_ports(&self) -> BTreeSet<String> {
        self.ports
            .iter()
            .filter(|p| p.state != "planned")
            .map(|p| p.id.clone())
            .collect()
    }

    /// The allow-list row for a crate: an exact name first, then a wildcard row.
    fn crate_row(&self, name: &str) -> Option<&CrateRow> {
        self.crates
            .iter()
            .find(|c| c.name == name)
            .or_else(|| self.crates.iter().find(|c| matches(&c.name, name)))
    }

    /// The layer table exactly as blueprint 001's d-4 states it ("crate: deps; crate: ...").
    pub fn render(&self) -> String {
        self.crates
            .iter()
            .map(|c| {
                let mut deps = c.workspace.clone();
                deps.extend(
                    c.optional
                        .iter()
                        .map(|(dep, feature)| format!("{dep} (optional, feature {feature})")),
                );
                let deps = if deps.is_empty() {
                    "none".to_string()
                } else {
                    deps.join(", ")
                };
                format!("{}: {deps}", c.name)
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// What umx-arch reads from the blueprint: the r-* ids and d-4's layer table.
#[derive(Debug, Clone)]
pub struct Blueprint {
    pub rule_ids: BTreeSet<String>,
    pub layer_table: Option<String>,
}

const LAYER_TABLE: &str = "Layer table (normal dependencies): ";

pub fn load_blueprint(path: &Path) -> Result<Blueprint, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let doc: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))?;
    let decisions = doc["sections"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["name"] == "decisions"))
        .and_then(|s| s["value"].as_array())
        .ok_or_else(|| format!("{}: no decisions section", path.display()))?;
    let rule_ids = decisions
        .iter()
        .filter_map(|d| d["id"].as_str())
        .filter(|id| id.starts_with("r-"))
        .map(str::to_string)
        .collect();
    let layer_table = decisions
        .iter()
        .find(|d| d["id"] == "d-4")
        .and_then(|d| d["decision"].as_str())
        .and_then(|t| t.split_once(LAYER_TABLE))
        .map(|(_, rest)| rest.split_once(". ").map_or(rest, |(t, _)| t))
        .map(|t| t.trim_end_matches('.').to_string());
    Ok(Blueprint {
        rule_ids,
        layer_table,
    })
}

// ---------------------------------------------------------------- workspace

#[derive(Debug)]
struct Package {
    name: String,
    dir: PathBuf,
    /// The library target's root file, as cargo metadata reports it (`[lib] path` included).
    lib: Option<PathBuf>,
    /// Every shipped target's root file (library and binaries), as cargo metadata reports it.
    roots: Vec<PathBuf>,
    deps: Vec<Dep>,
    features: BTreeMap<String, Vec<String>>,
}

#[derive(Debug)]
struct Dep {
    name: String,
    /// "normal", "build" or "dev".
    kind: String,
    optional: bool,
}

impl Dep {
    /// Normal and build dependencies end up in what ships; dev-dependencies do not.
    fn ships(&self) -> bool {
        self.kind != "dev"
    }
}

fn load_workspace(root: &Path) -> Result<Vec<Package>, String> {
    let out = Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .output()
        .map_err(|e| format!("run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata in {} failed: {}",
            root.display(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("parse cargo metadata: {e}"))?;
    let mut packages = Vec::new();
    for p in meta["packages"].as_array().into_iter().flatten() {
        let manifest = PathBuf::from(p["manifest_path"].as_str().unwrap_or_default());
        let deps = p["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|d| Dep {
                name: d["name"].as_str().unwrap_or_default().to_string(),
                kind: d["kind"].as_str().unwrap_or("normal").to_string(),
                optional: d["optional"].as_bool().unwrap_or(false),
            })
            .collect();
        let features = p["features"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| {
                let items = v
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect();
                (k.clone(), items)
            })
            .collect();
        let lib = p["targets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| {
                t["kind"]
                    .as_array()
                    .is_some_and(|k| k.iter().any(|k| k == "lib" || k == "rlib"))
            })
            .and_then(|t| t["src_path"].as_str())
            .map(PathBuf::from);
        let shipped = [
            "lib",
            "rlib",
            "dylib",
            "cdylib",
            "staticlib",
            "proc-macro",
            "bin",
        ];
        let roots = p["targets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| {
                t["kind"]
                    .as_array()
                    .is_some_and(|k| k.iter().any(|k| shipped.iter().any(|s| k == s)))
            })
            .filter_map(|t| t["src_path"].as_str().map(PathBuf::from))
            .collect();
        packages.push(Package {
            name: p["name"].as_str().unwrap_or_default().to_string(),
            dir: manifest.parent().map(Path::to_path_buf).unwrap_or_default(),
            lib,
            roots,
            deps,
            features,
        });
    }
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(packages)
}

/// `pattern` matches `name` exactly, or by prefix when it ends in `*`.
fn matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

fn matches_any(patterns: &[String], name: &str) -> bool {
    patterns.iter().any(|p| matches(p, name))
}

/// Every .rs file under `dir`, sorted.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// A crate's own source: src/, tests/, benches/, examples/ and build.rs (never fixtures/).
fn crate_sources(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = ["src", "tests", "benches", "examples"]
        .iter()
        .flat_map(|d| rust_files(&dir.join(d)))
        .collect();
    let build = dir.join("build.rs");
    if build.is_file() {
        files.push(build);
    }
    files
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).display().to_string()
}

// ---------------------------------------------------------------- check

/// What one run of the checker looks at.
pub struct Inputs<'a> {
    pub root: &'a Path,
    pub rules: &'a Rules,
    pub blueprint: &'a Blueprint,
    /// Port rows to enforce (the real run passes [`Rules::active_ports`]).
    pub active_ports: &'a BTreeSet<String>,
    /// `describe(mode)` output; the wiring check is skipped when it is absent.
    pub described: Option<&'a [WiringRow]>,
    /// The tree is the whole workspace, so a crate a rule names must exist: a rename then cannot
    /// switch the rule off unseen. The real run sets this; fixtures are partial trees and set it
    /// only to plant that case.
    pub whole_workspace: bool,
}

/// Violations, plus a notice for every check that did not run, so nothing is skipped silently.
#[derive(Debug, Default)]
pub struct Report {
    pub violations: Vec<Violation>,
    pub notices: Vec<String>,
}

/// Runs every rule's checker. `Err` means a checker could not run, which is never a pass.
pub fn check(inputs: &Inputs) -> Result<Report, String> {
    let rules = inputs.rules;
    let mut report = Report::default();
    let v = &mut report.violations;
    v.extend(check_rule_ids(rules, &inputs.blueprint.rule_ids));
    let ws = if inputs.root.join("Cargo.toml").is_file() {
        load_workspace(inputs.root)?
    } else {
        Vec::new()
    };
    if inputs.whole_workspace {
        v.extend(named_crates_exist(rules, &ws));
    }
    for rule in &rules.rules {
        let found = match rule.id.as_str() {
            "r-core-no-std" => no_std(inputs.root, rule, &ws),
            "r-inward" => {
                let mut found = inward(rules, &ws);
                found.extend(layer_table_matches(rules, inputs.blueprint));
                found.extend(reexports(inputs.root, rules, &ws));
                found
            }
            "r-adapters-isolated" => adapters_isolated(rule, &ws),
            "r-only-compose-wires" => only_compose_wires(rule, &ws),
            "r-vendor-crates-owned" => vendor_owned(rule, rules, &ws),
            "r-cli-is-a-client" => cli_is_a_client(rule, &ws),
            "r-contracts-dev-only" => dev_only(rule, &ws),
            "r-no-fakes-in-release" => no_fakes_in_release(inputs.root, rule, &ws)?,
            "r-fake-per-port" => {
                let mut found = fakes_in(inputs.root, rules, inputs.active_ports, &ws);
                found.extend(port_rows(inputs.root, rules, &ws, inputs.whole_workspace));
                found
            }
            "r-wiring-matches-docs" => match inputs.described {
                Some(d) => check_wiring(rules, inputs.active_ports, d),
                None => {
                    report.notices.push(format!(
                        "{}: runs in umx-compose's describe test, not here",
                        rule.id
                    ));
                    Vec::new()
                }
            },
            "r-env-in-compose" => env_in_compose(inputs.root, rule, &ws),
            "r-identity-client-only" => identity_client_only(inputs.root, rule, &ws),
            "r-command-in-adapters" => command_in_adapters(inputs.root, rule, &ws),
            "r-fs-in-adapters" => fs_in_adapters(inputs.root, rule, &ws),
            "r-no-std-reexport" => no_std_reexport(inputs.root, rule, &ws),
            "r-checked-graph-writes" => checked_graph_writes(inputs.root, rule, &ws),
            "r-gate-features" => gate_features(rule, &ws, inputs.whole_workspace),
            // Checked by check_rule_ids above and by silent_rules in the self-test.
            "r-rules-fire" => Vec::new(),
            other => vec![violation(
                "r-rules-fire",
                "rules.toml",
                format!("{other} has no checker in umx-arch"),
            )],
        };
        v.extend(found);
    }
    for port in rules
        .ports
        .iter()
        .filter(|p| !inputs.active_ports.contains(&p.id))
    {
        report.notices.push(format!(
            "port {}: {} (not enforced yet by r-fake-per-port, r-wiring-matches-docs)",
            port.id, port.state
        ));
    }
    v.sort();
    v.dedup();
    Ok(report)
}

/// rules.toml and the blueprint carry the same r-* ids (d-4).
fn check_rule_ids(rules: &Rules, blueprint: &BTreeSet<String>) -> Vec<Violation> {
    let ours: BTreeSet<String> = rules.ids().into_iter().collect();
    let mut v = Vec::new();
    for id in blueprint.difference(&ours) {
        v.push(violation(
            "r-rules-fire",
            "rules.toml",
            format!("blueprint row {id} has no rules.toml entry"),
        ));
    }
    for id in ours.difference(blueprint) {
        v.push(violation(
            "r-rules-fire",
            "rules.toml",
            format!("{id} is not an r-* row of the blueprint"),
        ));
    }
    v
}

/// d-4's layer table equals the render of rules.toml (r-inward: one policy).
fn layer_table_matches(rules: &Rules, blueprint: &Blueprint) -> Vec<Violation> {
    let render = rules.render();
    match &blueprint.layer_table {
        Some(table) if *table == render => Vec::new(),
        Some(table) => vec![violation(
            "r-inward",
            "blueprint d-4",
            format!(
                "layer table differs from `umx-arch render`:\n  blueprint: {table}\n  render:    {render}"
            ),
        )],
        None => vec![violation(
            "r-inward",
            "blueprint d-4",
            format!("d-4 has no \"{}\"", LAYER_TABLE.trim()),
        )],
    }
}

/// The rules no planted fixture made fire (the self-test's half of r-rules-fire).
pub fn silent_rules(rule_ids: &[String], fired: &BTreeSet<String>) -> Vec<Violation> {
    rule_ids
        .iter()
        .filter(|id| !fired.contains(*id))
        .map(|id| {
            violation(
                "r-rules-fire",
                "fixtures",
                format!("{id} is SILENT: no planted fixture makes it fire"),
            )
        })
        .collect()
}

/// Pure crates are no_std: the library root cargo builds (from metadata, so `[lib] path` cannot hide
/// another file) starts with `#![no_std]`, no source says `extern crate std` however spelled, and no
/// attribute sets a module `path` (which could pull in a file outside the scanned tree).
fn no_std(root: &Path, rule: &Rule, ws: &[Package]) -> Vec<Violation> {
    let mut v = Vec::new();
    for p in ws.iter().filter(|p| rule.crates.contains(&p.name)) {
        let Some(lib) = &p.lib else {
            v.push(violation(&rule.id, &p.name, "no library target"));
            continue;
        };
        let text = fs::read_to_string(lib).unwrap_or_default();
        let first = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("//"));
        if first != Some("#![no_std]") {
            v.push(violation(
                &rule.id,
                rel(root, lib),
                "the library root must start with #![no_std]",
            ));
        }
        let mut files = rust_files(&p.dir.join("src"));
        if let Some(dir) = lib.parent() {
            files.extend(rust_files(dir));
        }
        files.sort();
        files.dedup();
        for f in files {
            let toks = tokens(&fs::read_to_string(&f).unwrap_or_default());
            // `extern crate std`, however spelled: r#std, any spacing or comments, `as` aliases,
            // under any attribute.
            if toks
                .windows(3)
                .any(|w| w[0] == "extern" && w[1] == "crate" && w[2] == "std")
            {
                v.push(violation(
                    &rule.id,
                    rel(root, &f),
                    "`extern crate std` in a no_std crate",
                ));
            }
            if sets_module_path(&toks) {
                v.push(violation(
                    &rule.id,
                    rel(root, &f),
                    "#[path] in a pure crate: modules must live where the checks look",
                ));
            }
        }
    }
    v
}

/// An attribute (`#[...]` or `#![...]`, including inside `cfg_attr`) that holds `path = ...`.
fn sets_module_path(toks: &[String]) -> bool {
    let mut i = 0;
    while i < toks.len() {
        let open = match (toks.get(i + 1), toks.get(i + 2)) {
            (Some(a), _) if toks[i] == "#" && a == "[" => Some(i + 2),
            (Some(a), Some(b)) if toks[i] == "#" && a == "!" && b == "[" => Some(i + 3),
            _ => None,
        };
        let Some(mut j) = open else {
            i += 1;
            continue;
        };
        let mut depth = 1;
        while j < toks.len() && depth > 0 {
            match toks[j].as_str() {
                "[" | "(" => depth += 1,
                "]" | ")" => depth -= 1,
                "path" if toks.get(j + 1).is_some_and(|t| t == "=") => return true,
                _ => {}
            }
            j += 1;
        }
        i = j;
    }
    false
}

/// On a whole workspace, every crate a rule names exists (a pattern ending in `*` matches at least
/// one), so a rename cannot switch a rule off unseen (ap-01). Reported under the rule that names it.
fn named_crates_exist(rules: &Rules, ws: &[Package]) -> Vec<Violation> {
    let exists = |name: &str| ws.iter().any(|p| matches(name, &p.name));
    let mut named: Vec<(&str, &str)> = Vec::new();
    for rule in &rules.rules {
        let one = [&rule.krate, &rule.from, &rule.root, &rule.fakes];
        named.extend(
            one.into_iter()
                .flatten()
                .map(|n| (rule.id.as_str(), n.as_str())),
        );
        for list in [&rule.crates, &rule.deny, &rule.targets, &rule.allow] {
            named.extend(list.iter().map(|n| (rule.id.as_str(), n.as_str())));
        }
    }
    named.extend(rules.crates.iter().map(|c| ("r-inward", c.name.as_str())));
    for c in &rules.crates {
        named.extend(c.workspace.iter().map(|n| ("r-inward", n.as_str())));
        named.extend(c.optional.keys().map(|n| ("r-inward", n.as_str())));
    }
    named.extend(
        rules
            .reexports
            .iter()
            .map(|r| ("r-inward", r.krate.as_str())),
    );
    for owners in rules.owned.values() {
        named.extend(owners.iter().map(|n| ("r-vendor-crates-owned", n.as_str())));
    }
    named.extend(
        rules
            .adapters
            .values()
            .map(|n| ("r-fake-per-port", n.as_str())),
    );
    named.sort();
    named.dedup();
    named
        .into_iter()
        .filter(|(_, name)| !exists(name))
        .map(|(rule, name)| {
            violation(
                rule,
                name,
                format!("{name} is named in rules.toml but is not in the workspace; update rules.toml if it was renamed"),
            )
        })
        .collect()
}

/// Every trait under umx-core/src/ports has a [[port]] row, and every row's trait is there.
/// A partial tree (a fixture) without umx-core or its ports folder is not asked; a whole workspace is.
fn port_rows(root: &Path, rules: &Rules, ws: &[Package], whole_workspace: bool) -> Vec<Violation> {
    const RULE: &str = "r-fake-per-port";
    let dir = ws
        .iter()
        .find(|p| p.name == "umx-core")
        .map(|core| core.dir.join("src/ports"));
    let Some(dir) = dir.filter(|d| d.is_dir()) else {
        return if whole_workspace {
            vec![violation(
                RULE,
                "umx-core/src/ports",
                "no port folder to check the [[port]] rows against",
            )]
        } else {
            Vec::new()
        };
    };
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    for f in rust_files(&dir) {
        let toks = tokens(&fs::read_to_string(&f).unwrap_or_default());
        for w in toks.windows(2).filter(|w| w[0] == "trait") {
            found.insert(w[1].clone(), rel(root, &f));
        }
    }
    let rows: BTreeSet<&str> = rules.ports.iter().map(|p| p.trait_name.as_str()).collect();
    let mut v: Vec<Violation> = found
        .iter()
        .filter(|(t, _)| !rows.contains(t.as_str()))
        .map(|(t, at)| violation(RULE, at, format!("port trait {t} has no [[port]] row")))
        .collect();
    v.extend(
        rules
            .ports
            .iter()
            .filter(|p| !found.contains_key(&p.trait_name))
            .map(|p| {
                violation(
                    RULE,
                    &p.id,
                    format!(
                        "[[port]] {} names trait {}, which is not under {}",
                        p.id,
                        p.trait_name,
                        rel(root, &dir)
                    ),
                )
            }),
    );
    v
}

/// One leaf of a `use` tree: the full path it names and the name it binds (`None` for `_` or a glob).
#[derive(Debug, PartialEq, Eq)]
struct Leaf {
    path: Vec<String>,
    binds: Option<String>,
    glob: bool,
}

/// The leaves of a use tree (the tokens after `use` up to `;`): groups are expanded, `self` in a group
/// names the group's own path, and `as` renames.
fn use_leaves(toks: &[String]) -> Vec<Leaf> {
    fn tree(toks: &[String], i: &mut usize, prefix: &[String], out: &mut Vec<Leaf>) {
        let mut path = prefix.to_vec();
        let leaf = |path: Vec<String>, alias: Option<String>| {
            let binds = match alias {
                Some(a) if a == "_" => None,
                Some(a) => Some(a),
                None => path.last().cloned(),
            };
            Leaf {
                path,
                binds,
                glob: false,
            }
        };
        loop {
            match toks.get(*i).map(String::as_str) {
                Some(":") => *i += 1,
                Some("{") => {
                    *i += 1;
                    loop {
                        match toks.get(*i).map(String::as_str) {
                            Some("}") => {
                                *i += 1;
                                break;
                            }
                            Some(",") => *i += 1,
                            None => break,
                            _ => tree(toks, i, &path, out),
                        }
                    }
                    return;
                }
                Some("*") => {
                    *i += 1;
                    out.push(Leaf {
                        path,
                        binds: None,
                        glob: true,
                    });
                    return;
                }
                Some("as") => {
                    let alias = toks.get(*i + 1).cloned();
                    *i += 2;
                    out.push(leaf(path, alias));
                    return;
                }
                Some("," | "}") | None => {
                    out.push(leaf(path, None));
                    return;
                }
                // `self` inside a group is the group's path; leading, it is the current module.
                Some("self") if !path.is_empty() => *i += 1,
                Some(seg) => {
                    path.push(seg.to_string());
                    *i += 1;
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        tree(toks, &mut i, &[], &mut out);
        while toks.get(i).is_some_and(|t| t == "," || t == "}") {
            i += 1;
        }
    }
    out
}

/// One `use` or `extern crate`: whether it is `pub`, and its leaves.
type UseItem = (bool, Vec<Leaf>);

/// Every `use` and `extern crate` in some tokens, as (is `pub`, leaves). `pub(crate)` and the like are not `pub`.
fn use_items(toks: &[String]) -> Vec<UseItem> {
    let mut items = Vec::new();
    for i in 0..toks.len() {
        let start = match toks[i].as_str() {
            "use" => i + 1,
            "extern" if toks.get(i + 1).is_some_and(|t| t == "crate") => i + 2,
            _ => continue,
        };
        let public = i > 0 && toks[i - 1] == "pub";
        let end = toks[start..]
            .iter()
            .position(|t| t == ";")
            .map_or(toks.len(), |n| start + n);
        items.push((public, use_leaves(&toks[start..end])));
    }
    items
}

/// A `pub use` (or `pub extern crate`) of a workspace crate or an item from one, or of a whole external
/// crate, fails unless rules.toml lists it as (crate, path): an allowed crate must not launder a crate its
/// importer may not depend on past the allow-lists (ap-10). Use trees are expanded leaf by leaf, and every
/// name a `use` binds (`as` aliases, `self` in groups, items) resolves to its real path, so a later
/// `pub use alias::X` is judged by what it really names. An item of an external crate (`serde_json::Value`)
/// is API, not the crate, and needs no row. Every shipped target root cargo metadata reports is scanned with
/// its folder, and a `#[path]` module fails outright: it could hold an export no scan sees.
fn reexports(root: &Path, rules: &Rules, ws: &[Package]) -> Vec<Violation> {
    let crates: BTreeSet<String> = ws.iter().map(|p| p.name.replace('-', "_")).collect();
    let mut v = Vec::new();
    for p in ws {
        let external: BTreeSet<String> = p
            .deps
            .iter()
            .map(|d| d.name.replace('-', "_"))
            .filter(|d| !crates.contains(d))
            .collect();
        let mut files = rust_files(&p.dir.join("src"));
        for target in &p.roots {
            if let Some(dir) = target.parent() {
                files.extend(rust_files(dir));
            }
        }
        files.sort();
        files.dedup();
        let parsed: Vec<(PathBuf, Vec<String>)> = files
            .into_iter()
            .map(|f| {
                let toks = tokens(&fs::read_to_string(&f).unwrap_or_default());
                (f, toks)
            })
            .collect();
        for (f, toks) in &parsed {
            if sets_module_path(toks) {
                v.push(violation(
                    "r-inward",
                    rel(root, f),
                    "a #[path] module is outside what the re-export check scans; move the module where cargo finds it",
                ));
            }
        }
        // Names each file's `use`s bind, resolved to a path that starts with a crate, to a fixpoint. A path resolves
        // in the module it names (F34): a bare or `self::` path in its own file, `crate::` in the crate root files
        // (the metadata target roots), each `super::` one module up. A `mod` declared in that module shadows a name
        // bound there; mods elsewhere do not.
        type Bound = BTreeMap<String, Vec<String>>;
        let mods = |toks: &[String]| -> BTreeSet<String> {
            toks.windows(2)
                .filter(|w| w[0] == "mod")
                .map(|w| w[1].clone())
                .collect()
        };
        let index: BTreeMap<&Path, usize> = parsed
            .iter()
            .enumerate()
            .map(|(n, (f, _))| (f.as_path(), n))
            .collect();
        let roots: Vec<usize> = p
            .roots
            .iter()
            .filter_map(|r| index.get(r.as_path()).copied())
            .collect();
        // The files of the module that owns folder `d`: a root in `d`, else `d/mod.rs`, else `d.rs`.
        let owner = |d: &Path| -> Vec<usize> {
            let at_root: Vec<usize> = roots
                .iter()
                .copied()
                .filter(|&r| parsed[r].0.parent() == Some(d))
                .collect();
            if !at_root.is_empty() {
                return at_root;
            }
            [d.join("mod.rs"), d.with_extension("rs")]
                .iter()
                .find_map(|f| index.get(f.as_path()).copied())
                .into_iter()
                .collect()
        };
        // The files of the parent module of the module in file `n`; empty when unknown (then nothing resolves).
        let parent = |n: usize| -> Vec<usize> {
            let f = &parsed[n].0;
            let Some(dir) = f.parent() else {
                return Vec::new();
            };
            if roots.contains(&n) {
                return Vec::new();
            }
            if f.file_name().is_some_and(|x| x == "mod.rs") {
                dir.parent().map(owner).unwrap_or_default()
            } else {
                owner(dir)
            }
        };
        let resolve =
            |path: &[String], n: usize, bound: &[Bound], file_mods: &[BTreeSet<String>]| {
                let mut scope = vec![n];
                let mut rest = path;
                while let Some(head) = rest.first() {
                    match head.as_str() {
                        "crate" => scope = roots.clone(),
                        "self" => {}
                        "super" => scope = scope.iter().flat_map(|&m| parent(m)).collect(),
                        _ => break,
                    }
                    rest = &rest[1..];
                }
                let first = rest.first()?;
                if scope.iter().any(|&m| file_mods[m].contains(first)) {
                    return None;
                }
                if crates.contains(first) || external.contains(first) {
                    return Some(rest.to_vec());
                }
                let base = scope.iter().find_map(|&m| bound[m].get(first))?;
                Some(base.iter().chain(&rest[1..]).cloned().collect())
            };
        let files: Vec<(&PathBuf, Vec<UseItem>)> =
            parsed.iter().map(|(f, t)| (f, use_items(t))).collect();
        let file_mods: Vec<BTreeSet<String>> = parsed.iter().map(|(_, t)| mods(t)).collect();
        let mut bound: Vec<Bound> = vec![Bound::new(); files.len()];
        loop {
            let before = bound.clone();
            for (n, (_, items)) in files.iter().enumerate() {
                for leaf in items.iter().flat_map(|(_, leaves)| leaves) {
                    if leaf.glob {
                        continue;
                    }
                    if let (Some(name), Some(path)) =
                        (&leaf.binds, resolve(&leaf.path, n, &bound, &file_mods))
                    {
                        bound[n].insert(name.clone(), path);
                    }
                }
            }
            if bound == before {
                break;
            }
        }
        for (n, (f, items)) in files.iter().enumerate() {
            for (_, leaves) in items.iter().filter(|(public, _)| *public) {
                for leaf in leaves {
                    let Some(path) = resolve(&leaf.path, n, &bound, &file_mods) else {
                        continue;
                    };
                    let workspace = crates.contains(&path[0]);
                    let whole_external = external.contains(&path[0]) && path.len() == 1;
                    if !workspace && !whole_external {
                        continue;
                    }
                    let shown = path.join("::") + if leaf.glob { "::*" } else { "" };
                    let listed = rules
                        .reexports
                        .iter()
                        .any(|r| r.krate == p.name && r.path == shown);
                    if !listed {
                        v.push(violation(
                            "r-inward",
                            rel(root, f),
                            format!(
                                "{} re-exports {shown}; list it under [[reexport]] in rules.toml, or have the importer depend on the crate itself",
                                p.name
                            ),
                        ));
                    }
                }
            }
        }
    }
    v
}

fn inward(rules: &Rules, ws: &[Package]) -> Vec<Violation> {
    let names: BTreeSet<&str> = ws.iter().map(|p| p.name.as_str()).collect();
    let mut v = Vec::new();
    for p in ws {
        let Some(row) = rules.crate_row(&p.name) else {
            v.push(violation(
                "r-inward",
                &p.name,
                "crate has no allow-list row in rules.toml",
            ));
            continue;
        };
        for d in p.deps.iter().filter(|d| d.ships()) {
            if let Some(feature) = row.optional.get(&d.name) {
                let gated = p.features.get(feature).is_some_and(|items| {
                    items
                        .iter()
                        .any(|i| *i == format!("dep:{}", d.name) || *i == d.name)
                });
                if !d.optional || !gated {
                    v.push(violation(
                        "r-inward",
                        &p.name,
                        format!(
                            "{} is allowed only as optional, behind feature `{feature}`",
                            d.name
                        ),
                    ));
                }
                continue;
            }
            let (allowed, kind) = if names.contains(d.name.as_str()) {
                (&row.workspace, "workspace")
            } else {
                (&row.external, "external")
            };
            if !matches_any(allowed, &d.name) {
                v.push(violation(
                    "r-inward",
                    &p.name,
                    format!("{kind} dependency {} is not in its allow-list", d.name),
                ));
            }
        }
    }
    v
}

fn adapters_isolated(rule: &Rule, ws: &[Package]) -> Vec<Violation> {
    let from = rule.from.as_deref().unwrap_or("umx-adapter-*");
    let mut v = Vec::new();
    for p in ws.iter().filter(|p| matches(from, &p.name)) {
        for d in p
            .deps
            .iter()
            .filter(|d| d.ships() && matches_any(&rule.deny, &d.name))
        {
            v.push(violation(
                &rule.id,
                &p.name,
                format!("an adapter must not depend on {}", d.name),
            ));
        }
    }
    v
}

fn only_compose_wires(rule: &Rule, ws: &[Package]) -> Vec<Violation> {
    let mut v = Vec::new();
    for p in ws.iter().filter(|p| !matches_any(&rule.allow, &p.name)) {
        for d in p
            .deps
            .iter()
            .filter(|d| d.ships() && matches_any(&rule.targets, &d.name))
        {
            v.push(violation(
                &rule.id,
                &p.name,
                format!("only {} may depend on {}", rule.allow.join(", "), d.name),
            ));
        }
    }
    v
}

fn vendor_owned(rule: &Rule, rules: &Rules, ws: &[Package]) -> Vec<Violation> {
    let mut v = Vec::new();
    for p in ws {
        for d in p.deps.iter().filter(|d| d.ships()) {
            for (lib, owners) in &rules.owned {
                if matches(lib, &d.name) && !owners.contains(&p.name) {
                    v.push(violation(
                        &rule.id,
                        &p.name,
                        if owners.is_empty() {
                            format!(
                                "{} belongs to an adapter crate that does not exist yet",
                                d.name
                            )
                        } else {
                            format!("{} belongs to {} only", d.name, owners.join(", "))
                        },
                    ));
                }
            }
        }
    }
    v
}

fn cli_is_a_client(rule: &Rule, ws: &[Package]) -> Vec<Violation> {
    let names: BTreeSet<&str> = ws.iter().map(|p| p.name.as_str()).collect();
    let cli = rule.krate.as_deref().unwrap_or("umx-cli");
    let mut v = Vec::new();
    for p in ws.iter().filter(|p| p.name == cli) {
        for d in p.deps.iter().filter(|d| {
            d.ships() && names.contains(d.name.as_str()) && !rule.allow.contains(&d.name)
        }) {
            v.push(violation(
                &rule.id,
                &p.name,
                format!(
                    "the CLI may reach only {}, not {}",
                    rule.allow.join(", "),
                    d.name
                ),
            ));
        }
    }
    v
}

fn dev_only(rule: &Rule, ws: &[Package]) -> Vec<Violation> {
    let mut v = Vec::new();
    for p in ws {
        for d in p
            .deps
            .iter()
            .filter(|d| d.ships() && rule.crates.contains(&d.name))
        {
            v.push(violation(
                &rule.id,
                &p.name,
                format!("{} may only be a dev-dependency (found {})", d.name, d.kind),
            ));
        }
    }
    v
}

/// `cargo tree -p <root> -e normal -i <fakes> --target all` must find nothing (every platform's
/// dependencies, default features included), and with the stub feature it must find the fakes (the
/// positive control: the instrument can see them when they are there). A missing root or fakes crate
/// is reported by [`named_crates_exist`] on a whole workspace.
fn no_fakes_in_release(root: &Path, rule: &Rule, ws: &[Package]) -> Result<Vec<Violation>, String> {
    let bin = rule.root.as_deref().unwrap_or("umx");
    let fakes = rule.fakes.as_deref().unwrap_or("umx-fakes");
    let feature = rule.feature.as_deref().unwrap_or("stub");
    if [bin, fakes]
        .iter()
        .any(|name| !ws.iter().any(|p| p.name == *name))
    {
        return Ok(Vec::new());
    }
    let reaches_fakes = |features: Option<&str>| -> Result<bool, String> {
        let mut cmd = Command::new("cargo");
        cmd.args([
            "tree", "-e", "normal", "--prefix", "none", "--target", "all",
        ])
        .args(["-p", bin, "-i", fakes])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"));
        if let Some(f) = features {
            cmd.args(["--features", f]);
        }
        let out = cmd.output().map_err(|e| format!("run cargo tree: {e}"))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        // cargo tree -i exits 101 with this message when the package is not in the graph.
        if !out.status.success() && !stderr.contains("did not match any packages") {
            return Err(format!("cargo tree in {} failed: {stderr}", root.display()));
        }
        Ok(stdout.lines().any(|l| l.starts_with(&format!("{fakes} "))))
    };
    let mut v = Vec::new();
    if reaches_fakes(None)? {
        v.push(violation(
            &rule.id,
            bin,
            format!("the release build of {bin} contains {fakes}"),
        ));
    }
    if !reaches_fakes(Some(feature))? {
        v.push(violation(
            &rule.id,
            bin,
            format!("positive control failed: {bin} --features {feature} does not reach {fakes}"),
        ));
    }
    Ok(v)
}

/// Every feature a workspace crate declares is either built by the gate's clippy (in `gate`, turned on by one that
/// is, or a crate's `default`) or excluded with a reason; a feature that forwards to an excluded one counts as excluded
/// (umx/cuda -> ... -> umx-adapter-llamacpp/cuda). The gate list must not turn on an excluded feature. On the whole
/// workspace every listed name must exist, so a rename cannot drop a feature out of the gate unseen (ap-11, E-3).
fn gate_features(rule: &Rule, ws: &[Package], whole_workspace: bool) -> Vec<Violation> {
    let id = rule.id.as_str();
    let members: BTreeSet<&str> = ws.iter().map(|p| p.name.as_str()).collect();
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in ws {
        for (feature, items) in &p.features {
            let to = items
                .iter()
                .filter(|i| !i.starts_with("dep:"))
                .filter_map(|i| match i.split_once('/') {
                    Some((dep, f)) => {
                        let dep = dep.trim_end_matches('?');
                        members.contains(dep).then(|| format!("{dep}/{f}"))
                    }
                    None => Some(format!("{}/{i}", p.name)),
                })
                .collect();
            edges.insert(format!("{}/{feature}", p.name), to);
        }
    }
    let reach = |from: &mut dyn Iterator<Item = String>| -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut todo: Vec<String> = from.collect();
        while let Some(f) = todo.pop() {
            if seen.insert(f.clone()) {
                todo.extend(edges.get(&f).into_iter().flatten().cloned());
            }
        }
        seen
    };
    let defaults = ws
        .iter()
        .filter(|p| p.features.contains_key("default"))
        .map(|p| format!("{}/default", p.name));
    let built = reach(&mut rule.gate.iter().cloned().chain(defaults));
    let excluded: BTreeSet<&str> = rule
        .gate_excluded
        .iter()
        .map(|e| e.feature.as_str())
        .collect();
    let mut v = Vec::new();
    for f in edges.keys() {
        let forwards_to_excluded = reach(&mut std::iter::once(f.clone()))
            .iter()
            .any(|r| excluded.contains(r.as_str()));
        if !built.contains(f) && !forwards_to_excluded {
            v.push(violation(
                id,
                f,
                "this feature is neither built by the gate (r-gate-features gate in rules.toml) nor excluded with a \
                 reason (gate_excluded), so no gate builds it",
            ));
        }
    }
    for e in &rule.gate_excluded {
        if built.contains(&e.feature) {
            v.push(violation(
                id,
                &e.feature,
                "the gate's feature list turns on this excluded feature",
            ));
        }
        if e.reason.trim().is_empty() {
            v.push(violation(id, &e.feature, "excluded without a reason"));
        }
    }
    if whole_workspace {
        for name in rule
            .gate
            .iter()
            .map(String::as_str)
            .chain(excluded.iter().copied())
        {
            if !edges.contains_key(name) {
                v.push(violation(
                    id,
                    name,
                    "rules.toml names a feature no workspace crate declares",
                ));
            }
        }
    }
    v
}

fn is_clause(name: &str) -> bool {
    let mut parts = name.splitn(3, '_');
    let (Some(_), Some(clause), Some(what)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    clause.len() > 1
        && clause.starts_with('c')
        && clause[1..].chars().all(|c| c.is_ascii_digit())
        && !what.is_empty()
}

/// Contract clause tests defined in a suite file: `fn <prefix>_c<N>_<what>`.
fn clause_names(src: &str) -> BTreeSet<String> {
    tokens(src)
        .windows(2)
        .filter(|w| w[0] == "fn" && is_clause(&w[1]))
        .map(|w| w[1].clone())
        .collect()
}

/// Names a source runs: called in code (`name(`, not `fn name(`), or listed as identifiers
/// inside a `<port>_contract!(...)` invocation, which expands to one test per listed clause.
/// A clause mentioned only in a comment, a string or an empty same-named test does not count.
fn called_names(src: &str) -> BTreeSet<String> {
    let toks = tokens(src);
    let mut run: BTreeSet<String> = (0..toks.len().saturating_sub(1))
        .filter(|&i| toks[i + 1] == "(" && (i == 0 || toks[i - 1] != "fn"))
        .map(|i| toks[i].clone())
        .collect();
    let mut i = 0;
    while i + 2 < toks.len() {
        let opens = ["(", "[", "{"];
        if toks[i].ends_with("_contract")
            && toks[i + 1] == "!"
            && opens.contains(&toks[i + 2].as_str())
        {
            let mut depth = 0;
            let mut j = i + 2;
            while j < toks.len() {
                match toks[j].as_str() {
                    "(" | "[" | "{" => depth += 1,
                    ")" | "]" | "}" => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    t => {
                        run.insert(t.to_string());
                    }
                }
                j += 1;
            }
            i = j;
        }
        i += 1;
    }
    run
}

/// Rust source as tokens, without comments or literals: identifiers and keywords (a raw `r#x`
/// becomes `x`) and single punctuation characters. Enough to recognise an item however it is
/// spaced, commented or spelled.
fn tokens(src: &str) -> Vec<String> {
    let c: Vec<char> = src.chars().collect();
    let at = |i: usize| c.get(i).copied();
    let ident_start = |ch: Option<char>| ch.is_some_and(|x| x.is_alphabetic() || x == '_');
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '/' && at(i + 1) == Some('/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && at(i + 1) == Some('*') {
            let mut depth = 0;
            while i < c.len() {
                if c[i] == '/' && at(i + 1) == Some('*') {
                    depth += 1;
                    i += 2;
                } else if c[i] == '*' && at(i + 1) == Some('/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if let Some(end) = raw_string_end(&c, i) {
            i = end;
        } else if ch == '"' || (ch == 'b' && at(i + 1) == Some('"')) {
            i += if ch == 'b' { 2 } else { 1 };
            while i < c.len() && c[i] != '"' {
                i += if c[i] == '\\' { 2 } else { 1 };
            }
            i += 1;
        } else if ch == '\'' {
            if at(i + 1) == Some('\\') {
                i += 3;
                while i < c.len() && c[i] != '\'' {
                    i += 1;
                }
                i += 1;
            } else if at(i + 2) == Some('\'') {
                i += 3;
            } else {
                // A lifetime: its name follows as an identifier.
                i += 1;
            }
        } else if ident_start(Some(ch)) {
            if ch == 'r' && at(i + 1) == Some('#') && ident_start(at(i + 2)) {
                i += 2;
            }
            let start = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            out.push(c[start..i].iter().collect());
        } else {
            out.push(ch.to_string());
            i += 1;
        }
    }
    out
}

/// The index just past a raw string (`r"…"`, `r#"…"#`, `br"…"`) starting at `i`, if one does.
fn raw_string_end(c: &[char], i: usize) -> Option<usize> {
    let mut j = i;
    if c.get(j) == Some(&'b') {
        j += 1;
    }
    if c.get(j) != Some(&'r') {
        return None;
    }
    j += 1;
    let hashes = c[j..].iter().take_while(|&&x| x == '#').count();
    j += hashes;
    if c.get(j) != Some(&'"') {
        return None;
    }
    j += 1;
    while j < c.len() {
        if c[j] == '"'
            && c[j + 1..]
                .iter()
                .take(hashes)
                .filter(|&&x| x == '#')
                .count()
                == hashes
        {
            return Some(j + 1 + hashes);
        }
        j += 1;
    }
    Some(c.len())
}

/// Every enforced port row has a contract suite, a fake, and a fake and real adapter that run
/// every clause of it (r-fake-per-port). The real tests and the self-test both call this.
pub fn check_fakes(
    root: &Path,
    rules: &Rules,
    active: &BTreeSet<String>,
) -> Result<Vec<Violation>, String> {
    Ok(fakes_in(root, rules, active, &load_workspace(root)?))
}

fn fakes_in(
    root: &Path,
    rules: &Rules,
    active: &BTreeSet<String>,
    ws: &[Package],
) -> Vec<Violation> {
    const RULE: &str = "r-fake-per-port";
    let dir_of = |name: &str| ws.iter().find(|p| p.name == name).map(|p| p.dir.clone());
    let mut v = Vec::new();
    for port in rules.ports.iter().filter(|p| active.contains(&p.id)) {
        let Some(contracts) = dir_of("umx-contracts") else {
            v.push(violation(RULE, &port.id, "no umx-contracts crate"));
            continue;
        };
        let suite = contracts.join("src").join(format!("{}.rs", port.module));
        let clauses = clause_names(&fs::read_to_string(&suite).unwrap_or_default());
        if clauses.is_empty() {
            v.push(violation(
                RULE,
                rel(root, &suite),
                format!("port {} has no contract clauses", port.id),
            ));
            continue;
        }
        if !port.stub.starts_with("fake.") {
            v.push(violation(
                RULE,
                &port.id,
                format!("stub adapter {} is not a fake", port.stub),
            ));
        }
        for adapter in std::iter::once(port.stub.as_str()).chain(port.real.all()) {
            let prefix = adapter.split('.').next().unwrap_or_default();
            let Some(krate) = rules.adapters.get(prefix) else {
                v.push(violation(
                    RULE,
                    adapter,
                    format!("no [adapters] crate for prefix {prefix}"),
                ));
                continue;
            };
            let Some(dir) = dir_of(krate) else {
                v.push(violation(
                    RULE,
                    adapter,
                    format!("crate {krate} does not exist"),
                ));
                continue;
            };
            let called: BTreeSet<String> = crate_sources(&dir)
                .iter()
                .filter_map(|f| fs::read_to_string(f).ok())
                .flat_map(|src| called_names(&src))
                .collect();
            let missing: Vec<&str> = clauses
                .iter()
                .filter(|c| !called.contains(*c))
                .map(String::as_str)
                .collect();
            if !missing.is_empty() {
                v.push(violation(
                    RULE,
                    adapter,
                    format!("{krate} does not run clauses {}", missing.join(", ")),
                ));
            }
        }
    }
    v
}

/// `describe(mode)` must equal the active rows of the adapter matrix (d-6), the real row taken for the host OS: a
/// port whose table has no entry for it must not be described in real mode. umx-compose's test calls this with its
/// real describe output once ports exist (s05).
pub fn check_wiring(
    rules: &Rules,
    active: &BTreeSet<String>,
    described: &[WiringRow],
) -> Vec<Violation> {
    const RULE: &str = "r-wiring-matches-docs";
    let expected: BTreeSet<WiringRow> = rules
        .ports
        .iter()
        .filter(|p| active.contains(&p.id))
        .flat_map(|p| {
            [
                ("stub", Some(p.stub.as_str())),
                ("real", p.real.for_os(std::env::consts::OS)),
            ]
            .into_iter()
            .filter_map(|(mode, adapter)| {
                Some(WiringRow {
                    mode: mode.to_string(),
                    port: p.id.clone(),
                    adapter: adapter?.to_string(),
                })
            })
        })
        .collect();
    let mut got: BTreeSet<WiringRow> = described.iter().cloned().collect();
    let mut v = Vec::new();
    // A list row: real mode wires at most one adapter, and it must be on the list.
    for p in rules.ports.iter().filter(|p| active.contains(&p.id)) {
        let RealAdapter::AnyOf(ids) = &p.real else {
            continue;
        };
        let wired: Vec<WiringRow> = got
            .iter()
            .filter(|r| r.mode == "real" && r.port == p.id)
            .cloned()
            .collect();
        if wired.len() > 1 {
            v.push(violation(
                RULE,
                format!("real/{}", p.id),
                format!(
                    "describe() wires {} adapters; the matrix allows one",
                    wired.len()
                ),
            ));
        }
        for r in wired {
            got.remove(&r);
            if !ids.contains(&r.adapter) {
                v.push(violation(
                    RULE,
                    format!("real/{}", p.id),
                    format!(
                        "describe() says {}, which is not in the matrix's list",
                        r.adapter
                    ),
                ));
            }
        }
    }
    for r in expected.difference(&got) {
        v.push(violation(
            RULE,
            format!("{}/{}", r.mode, r.port),
            format!("the matrix says {}, describe() does not", r.adapter),
        ));
    }
    for r in got.difference(&expected) {
        v.push(violation(
            RULE,
            format!("{}/{}", r.mode, r.port),
            format!("describe() says {}, the matrix does not", r.adapter),
        ));
    }
    v
}

/// Env is read only in umx-compose (d-9). Clippy bans the calls in the real workspace; this scan
/// also catches env reads and lint suppressions in fixtures, which clippy never sees.
fn env_in_compose(root: &Path, rule: &Rule, ws: &[Package]) -> Vec<Violation> {
    let mut v = Vec::new();
    for p in ws.iter().filter(|p| !rule.allow.contains(&p.name)) {
        for f in crate_sources(&p.dir) {
            let toks = tokens(&fs::read_to_string(&f).unwrap_or_default());
            let path = |w: &[String], a: &str, b: &[&str]| {
                w[0] == a && w[1] == ":" && w[2] == ":" && b.contains(&w[3].as_str())
            };
            if toks
                .windows(4)
                .any(|w| path(w, "env", &["var", "var_os", "vars", "vars_os"]))
            {
                v.push(violation(
                    &rule.id,
                    rel(root, &f),
                    "reads env outside umx-compose",
                ));
            }
            if toks
                .windows(4)
                .any(|w| path(w, "clippy", &["disallowed_methods"]))
            {
                v.push(violation(
                    &rule.id,
                    rel(root, &f),
                    format!("suppresses {ENV_LINT} outside umx-compose"),
                ));
            }
        }
    }
    v
}

fn identity_test_attribute(body: &[String]) -> bool {
    let words: Vec<&str> = body.iter().map(String::as_str).collect();
    if words == ["test"] || words == ["cfg", "(", "test", ")"] {
        return true;
    }
    if !words.starts_with(&["cfg", "(", "all", "("]) {
        return false;
    }
    let mut depth = 1;
    for i in 4..words.len() {
        match words[i] {
            "(" => depth += 1,
            ")" => depth -= 1,
            "test"
                if depth == 1
                    && matches!(words.get(i - 1), Some(&"(" | &","))
                    && matches!(words.get(i + 1), Some(&")" | &",")) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// std::process::Command only in adapter crates and tools (ws009 s2, architect): everywhere else a process is
/// started through a port. Shipped source only (src/): tests/, `#[cfg(test)]` items and build scripts (build-time
/// tooling, such as the commit stamp) are skipped. A fn in `except` is let through by file and name, so a second
/// Command elsewhere in the same crate still fires.
fn command_in_adapters(root: &Path, rule: &Rule, ws: &[Package]) -> Vec<Violation> {
    let mut v = Vec::new();
    for p in ws.iter().filter(|p| !matches_any(&rule.allow, &p.name)) {
        for f in rust_files(&p.dir.join("src")) {
            let toks = without_test_items(&tokens(&fs::read_to_string(&f).unwrap_or_default()));
            let file = f.strip_prefix(&p.dir).unwrap_or(&f).display().to_string();
            for (at, func) in process_commands(&toks) {
                let excepted = rule.except.iter().any(|e| {
                    e.file == format!("{}/{file}", p.name)
                        && Some(e.func.as_str()) == func.as_deref()
                });
                if !excepted {
                    v.push(violation(
                        &rule.id,
                        rel(root, &f),
                        format!(
                            "std::process::Command in {} (token {at}{}); start processes through a port's adapter",
                            p.name,
                            func.map_or(String::new(), |f| format!(", fn {f}"))
                        ),
                    ));
                }
            }
        }
    }
    v
}

/// The filesystem only in adapter crates, the tools and the process shells (F114, architect): everywhere else files
/// are reached through a port. Shipped source only, as [`command_in_adapters`]: tests/, `#[cfg(test)]` items and build
/// scripts are skipped, and a fn in `except` is let through by file and name, so a second use in the same file fires.
fn fs_in_adapters(root: &Path, rule: &Rule, ws: &[Package]) -> Vec<Violation> {
    let mut v = Vec::new();
    for p in ws.iter().filter(|p| !matches_any(&rule.allow, &p.name)) {
        let files: Vec<(PathBuf, Vec<String>)> = rust_files(&p.dir.join("src"))
            .into_iter()
            .map(|f| {
                let toks = without_test_items(&tokens(&fs::read_to_string(&f).unwrap_or_default()));
                (f, toks)
            })
            .collect();
        // Names for std are crate-wide: one file's alias is reached from another as crate::s, super::s or m::s (F119).
        let names = std_names(files.iter().map(|(_, toks)| toks.as_slice()));
        for (f, toks) in files {
            let file = f.strip_prefix(&p.dir).unwrap_or(&f).display().to_string();
            for (at, func, what) in fs_uses(&toks, &names) {
                let excepted = rule.except.iter().any(|e| {
                    e.file == format!("{}/{file}", p.name)
                        && Some(e.func.as_str()) == func.as_deref()
                });
                if !excepted {
                    v.push(violation(
                        &rule.id,
                        rel(root, &f),
                        format!(
                            "{what} in {} (token {at}{}); reach files through a port's adapter",
                            p.name,
                            func.map_or(String::new(), |f| format!(", fn {f}"))
                        ),
                    ));
                }
            }
        }
    }
    v
}

/// No re-export of std or of a std module on the way to fs (F133, architect): a `use` or `extern crate` wider than
/// private (`pub`, `pub(crate)`, `pub(super)`, `pub(in path)`) that brings in std, std::os or std::os::unix
/// themselves, a std path with an fs segment, or a glob of one of these, in every workspace crate. Each path is read
/// through the crate's names for std ([`std_names`]). Shipped source only, as [`fs_in_adapters`].
fn no_std_reexport(root: &Path, rule: &Rule, ws: &[Package]) -> Vec<Violation> {
    let mut v = Vec::new();
    for p in ws {
        let files: Vec<(PathBuf, Vec<String>)> = rust_files(&p.dir.join("src"))
            .into_iter()
            .map(|f| {
                let toks = without_test_items(&tokens(&fs::read_to_string(&f).unwrap_or_default()));
                (f, toks)
            })
            .collect();
        let names = std_names(files.iter().map(|(_, toks)| toks.as_slice()));
        for (f, toks) in &files {
            for (at, what) in std_reexports(toks, &names) {
                v.push(violation(
                    &rule.id,
                    rel(root, f),
                    format!(
                        "{what} in {} (token {at}); keep std and its modules private, re-export only items off the way to fs",
                        p.name
                    ),
                ));
            }
        }
    }
    v
}

/// Whether the `use` or `extern` at token `i` is wider than private: `pub`, or `pub(...)` other than `pub(self)`.
fn wider_than_private(toks: &[String], i: usize) -> bool {
    let at = |j: usize, s: &str| toks.get(j).is_some_and(|t| t == s);
    if i >= 1 && at(i - 1, "pub") {
        return true;
    }
    if i >= 1 && at(i - 1, ")") {
        let Some(open) = (0..i - 1).rev().find(|&j| at(j, "(")) else {
            return false;
        };
        return open >= 1 && at(open - 1, "pub") && !(open + 2 == i - 1 && at(open + 1, "self"));
    }
    false
}

/// Each re-export [`no_std_reexport`] refuses in one file's tokens, with what it is.
fn std_reexports(toks: &[String], names: &[(String, Vec<String>)]) -> Vec<(usize, &'static str)> {
    let at = |j: usize, s: &str| toks.get(j).is_some_and(|t| t == s);
    let mut found = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if t == "extern" && at(i + 1, "crate") && at(i + 2, "std") && wider_than_private(toks, i) {
            found.push((i, "a re-export of std (extern crate)"));
        } else if t == "use" && !(i >= 1 && at(i - 1, ":")) && wider_than_private(toks, i) {
            let refused = use_tree(toks, i + 1).0.iter().any(|leaf| {
                through_roots(names, &leaf.path).iter().any(|path| {
                    std_root(path)
                        .is_some_and(|rest| on_the_way(rest) || rest.iter().any(|s| s == "fs"))
                })
            });
            if refused {
                found.push((i, "a re-export of std or a std module on the way to fs"));
            }
        }
    }
    found
}

/// The unchecked graph-file writes live in umx-core's `setup` module (trait `GraphFilesSetup`), outside `ports` and
/// re-exported nowhere (F144, architect). Naming either is the only way to reach them: an alias, a glob, a fully
/// qualified call or a re-export all name one somewhere. A `setup` path segment counts whatever module it is in.
fn names_setup(toks: &[String]) -> Option<usize> {
    toks.iter()
        .position(|t| t == "GraphFilesSetup" || t == "setup")
}

/// The workspace crates `from` ships with: itself and its normal dependencies, transitively (build and dev
/// dependencies left out).
fn normal_closure<'a>(ws: &'a [Package], from: &[String]) -> BTreeSet<&'a str> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut todo: Vec<&str> = from.iter().map(String::as_str).collect();
    while let Some(name) = todo.pop() {
        let Some(p) = ws.iter().find(|p| p.name == name) else {
            continue;
        };
        if seen.insert(p.name.as_str()) {
            todo.extend(
                p.deps
                    .iter()
                    .filter(|d| d.kind == "normal")
                    .map(|d| d.name.as_str()),
            );
        }
    }
    seen
}

/// F144: umx-services writes graph folders only through `write_set_checked` (architect). No crate in the normal
/// dependency closure of those in `crates` names the setup trait or module, but those in `allow` (umx-core, its
/// home): so none can hand it to the services by a re-export, a glob or an alias chain either (review R4). Shipped
/// source only, as [`command_in_adapters`]: tests/, `#[cfg(test)]` items and build scripts are skipped.
fn checked_graph_writes(root: &Path, rule: &Rule, ws: &[Package]) -> Vec<Violation> {
    let closure = normal_closure(ws, &rule.crates);
    let mut v = Vec::new();
    for p in ws
        .iter()
        .filter(|p| closure.contains(p.name.as_str()) && !matches_any(&rule.allow, &p.name))
    {
        for f in rust_files(&p.dir.join("src")) {
            let toks = without_test_items(&tokens(&fs::read_to_string(&f).unwrap_or_default()));
            if let Some(at) = names_setup(&toks) {
                v.push(violation(
                    &rule.id,
                    rel(root, &f),
                    format!(
                        "{} names the unchecked graph writes (umx_core::setup, token {at}), and umx-services \
                         ships with it; write through write_set_checked with the graph's stamps",
                        p.name
                    ),
                ));
            }
        }
    }
    v
}

/// A local name for std, std::os or std::os::unix, with the std path it stands for.
type StdNames = Vec<(String, Vec<String>)>;

/// std, std::os or std::os::unix itself (the rest of the path after std): a module on the way to fs.
fn on_the_way(rest: &[String]) -> bool {
    rest.len() <= 2 && rest.iter().zip(["os", "unix"]).all(|(a, b)| a == b)
}

/// std::fs or std::os::unix::fs itself (the rest of the path after std): a filesystem module.
fn fs_module(rest: &[String]) -> bool {
    rest == ["fs"] || rest == ["os", "unix", "fs"]
}

/// The local names for the std modules on the way to fs, and for the filesystem modules std::fs and
/// std::os::unix::fs (R302-B1), in one crate's files, with the std path each stands for, from their use trees and
/// `extern crate std as`. A use through one of these names is read as rooted at its path, repeated
/// until nothing new is named, so chains (`use std as s; use s::os as o;`) resolve in any order (F117). The names are
/// the crate's, not one file's: an alias in one file is reached from another as `crate::s`, `super::s` or `m::s`, and
/// a `use crate::*` brings it in bare (F119).
fn std_names<'a>(files: impl IntoIterator<Item = &'a [String]>) -> StdNames {
    let mut leaves: Vec<UseLeaf> = Vec::new();
    let mut names: StdNames = Vec::new();
    for toks in files {
        let at = |i: usize, s: &str| toks.get(i).is_some_and(|t| t == s);
        for (i, t) in toks.iter().enumerate() {
            if t == "use" && !(i >= 1 && at(i - 1, ":")) {
                leaves.extend(use_tree(toks, i + 1).0);
            } else if t == "extern"
                && at(i + 1, "crate")
                && at(i + 2, "std")
                && at(i + 3, "as")
                && let Some(name) = toks.get(i + 4)
            {
                names.push((name.clone(), vec!["std".into()]));
            }
        }
    }
    loop {
        let mut grew = false;
        // A glob names no module itself: it fires at its use instead (R258 B1).
        for leaf in leaves.iter().filter(|l| l.name != "*") {
            // Every meaning a name has in any file is kept: one name may be std::os::unix in one file and std in
            // another (R270-2).
            for path in through_roots(&names, &leaf.path) {
                // A filesystem module's name too, renamed or not, so a re-export through it is read as std::fs
                // (R302-B1: use std::fs as disk; pub use disk::*).
                if std_root(&path).is_some_and(|rest| on_the_way(rest) || fs_module(rest))
                    && !names.iter().any(|(n, to)| *n == leaf.name && *to == path)
                {
                    names.push((leaf.name.clone(), path));
                    grew = true;
                }
            }
        }
        if !grew {
            return names;
        }
    }
}

/// Each filesystem use, with the fn it is in and what it is: a `std::fs` path, `std::os::unix::fs`, a `fs::<fn>(`
/// call, `File::open`, `File::create`, `OpenOptions`, a `use` tree rooted at std that names `fs` anywhere (groups,
/// renames, single items: it fires at the use), and a path that reaches `fs` through a local name for std, std::os or
/// std::os::unix (`use std as s;`, `use std::os::{unix as u}`, `extern crate std as s;`: R251 B1). A use rooted at
/// such a local name counts as rooted at std, through chains of them (`use std as s; use s::{fs as files}`: F117),
/// and so does a path through one after a leading `::` (`::host::fs`: R258 B2) or after crate, self, super or a module
/// (`crate::s::fs`: F119). The names in `roots` are the crate's ([`std_names`]). A glob of std, std::os or
/// std::os::unix fires at its use, since it brings fs into scope under names the scan cannot follow (R258 B1).
fn fs_uses(
    toks: &[String],
    roots: &[(String, Vec<String>)],
) -> Vec<(usize, Option<String>, &'static str)> {
    let at = |i: usize, s: &str| toks.get(i).is_some_and(|t| t == s);
    let sep = |i: usize| at(i, ":") && at(i + 1, ":");
    let ident = |i: usize| {
        toks.get(i)
            .and_then(|t| t.chars().next())
            .is_some_and(|c| c.is_alphabetic() || c == '_')
    };
    let mut found = Vec::new();
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut pending: Option<String> = None;
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        let after_sep = i >= 2 && sep(i - 2);
        let mut next = i + 1;
        let what = match t.as_str() {
            "fn" => {
                pending = toks.get(i + 1).cloned();
                None
            }
            "{" => {
                stack.push(pending.take());
                None
            }
            "}" => {
                stack.pop();
                None
            }
            ";" => {
                pending = None;
                None
            }
            "use" if !after_sep => {
                let (leaves, end) = use_tree(toks, i + 1);
                next = end;
                // The rest after std of every reading of the leaf's path; any one reaching fs fires (R270-2).
                let std_rests = |l: &UseLeaf| -> Vec<Vec<String>> {
                    through_roots(roots, &l.path)
                        .iter()
                        .filter_map(|path| std_root(path).map(<[String]>::to_vec))
                        .collect()
                };
                if leaves.iter().any(|l| {
                    std_rests(l)
                        .iter()
                        .any(|rest| rest.iter().any(|s| s == "fs"))
                }) {
                    Some("a use of std::fs")
                } else {
                    // A glob of std, std::os or std::os::unix brings fs (or the module on the way to it) into
                    // scope under names this scan cannot follow, so the glob itself fires (R258 B1).
                    leaves
                        .iter()
                        .any(|l| l.name == "*" && std_rests(l).iter().any(|rest| on_the_way(rest)))
                        .then_some("a glob of a std module on the way to fs")
                }
            }
            "std" if sep(i + 1) && at(i + 3, "fs") => Some("std::fs"),
            "unix" if sep(i + 1) && at(i + 3, "fs") => Some("std::os::unix::fs"),
            // A local name for std anywhere in a path: at its start, after a leading `::` (`::host::fs`: R258 B2), or
            // after crate, self, super or a module (`crate::s::fs`, `m::s::fs`: F119). The rest of the path is
            // skipped, so it counts once.
            name if sep(i + 1)
                && roots.iter().any(|(r, _)| r == name)
                && path_reaches_fs(toks, i) =>
            {
                next = path_end(toks, i);
                Some("std::fs through a renamed std path")
            }
            // `fs::<fn>(` after `use std::fs;`; a `std::fs::` path is counted at `std`.
            "fs" if !after_sep && sep(i + 1) && ident(i + 3) && at(i + 4, "(") => Some("fs::<fn>("),
            "File" if sep(i + 1) && (at(i + 3, "open") || at(i + 3, "create")) => {
                Some("File::open or File::create")
            }
            "OpenOptions" if !(after_sep && i >= 3 && at(i - 3, "fs")) => Some("OpenOptions"),
            _ => None,
        };
        if let Some(what) = what {
            // A use in a fn's signature (its return type, say) belongs to that fn.
            let func = pending
                .clone()
                .or_else(|| stack.iter().rev().find_map(Clone::clone));
            found.push((i, func, what));
        }
        i = next;
    }
    found
}

/// One name a `use` brings into scope: its full path and the local name (the last segment, the `as` name, or the
/// group's path for `self`). A glob has the name `*`.
struct UseLeaf {
    path: Vec<String>,
    name: String,
}

/// The leaves of the use tree that starts at `from` (after `use`), and the index after its `;`.
fn use_tree(toks: &[String], from: usize) -> (Vec<UseLeaf>, usize) {
    fn tree(toks: &[String], mut i: usize, prefix: &[String], out: &mut Vec<UseLeaf>) -> usize {
        let at = |i: usize, s: &str| toks.get(i).is_some_and(|t| t == s);
        let mut path = prefix.to_vec();
        loop {
            match toks.get(i).map(String::as_str) {
                Some(":") if at(i + 1, ":") => i += 2,
                Some("{") => {
                    i += 1;
                    while i < toks.len() && !at(i, "}") {
                        let before = i;
                        i = tree(toks, i, &path, out);
                        if at(i, ",") {
                            i += 1;
                        } else if i == before {
                            // Not a use tree (malformed source): step over the token.
                            i += 1;
                        }
                    }
                    return i + 1;
                }
                Some("*") => {
                    out.push(UseLeaf {
                        path,
                        name: "*".into(),
                    });
                    return i + 1;
                }
                Some(seg) if seg != "," && seg != "}" && seg != ";" && seg != "as" => {
                    if seg != "self" {
                        path.push(seg.to_string());
                    }
                    i += 1;
                    if at(i, ":") && at(i + 1, ":") {
                        continue;
                    }
                    let name = if at(i, "as") {
                        i += 2;
                        toks[i - 1].clone()
                    } else {
                        path.last().cloned().unwrap_or_default()
                    };
                    out.push(UseLeaf { path, name });
                    return i;
                }
                _ => return i,
            }
        }
    }
    let mut out = Vec::new();
    let mut i = tree(toks, from, &[], &mut out);
    while i < toks.len() && toks[i] != ";" {
        i += 1;
    }
    (out, i + 1)
}

/// The rest of `path` after a leading `std` (an optional `::` before it is skipped by the parser), else None.
fn std_root(path: &[String]) -> Option<&[String]> {
    path.split_first()
        .and_then(|(first, rest)| (first == "std").then_some(rest))
}

/// Every reading of `path` through the local names for std modules. A path rooted at std, core or alloc is never
/// rewritten, so `use std::os::unix as u` stays std::os::unix when another alias is named `os` (R270-1). Any other
/// path is read at each segment that is a name for std in any file, the root (the first segment, or the one after
/// leading `self`, `crate` and `super`; use trees already drop a leading `::`) and every later one, through each std
/// path that name stands for (R270-2). So `crate::s::fs` and `crate::m::s::fs`, where module m holds a private
/// `use std as s` its children can reach, are both std::fs (F119, F133). With no such name, `path` as is.
fn through_roots(roots: &[(String, Vec<String>)], path: &[String]) -> Vec<Vec<String>> {
    let k = path
        .iter()
        .take_while(|seg| matches!(seg.as_str(), "self" | "crate" | "super"))
        .count();
    if matches!(
        path.get(k).map(String::as_str),
        None | Some("std" | "core" | "alloc")
    ) {
        return vec![path.to_vec()];
    }
    let readings: Vec<Vec<String>> = path
        .iter()
        .enumerate()
        .skip(k)
        .flat_map(|(j, seg)| {
            roots
                .iter()
                .filter(move |(name, _)| name == seg)
                .map(move |(_, to)| to.iter().chain(&path[j + 1..]).cloned().collect())
        })
        .collect();
    if readings.is_empty() {
        vec![path.to_vec()]
    } else {
        readings
    }
}

/// The index after the path that starts at token `i` (`name::a::b`).
fn path_end(toks: &[String], mut i: usize) -> usize {
    let at = |i: usize, s: &str| toks.get(i).is_some_and(|t| t == s);
    while at(i + 1, ":") && at(i + 2, ":") {
        i += 3;
    }
    i + 1
}

/// Whether the path that starts at token `i` (`name::a::b`) has an `fs` segment.
fn path_reaches_fs(toks: &[String], mut i: usize) -> bool {
    let at = |i: usize, s: &str| toks.get(i).is_some_and(|t| t == s);
    while at(i + 1, ":") && at(i + 2, ":") {
        i += 3;
        if at(i, "fs") {
            return true;
        }
    }
    false
}

/// The tokens with test-only code removed: every item under `#[cfg(test)]`, the body of every module whose inner
/// attribute is `#![cfg(test)]`, and everything when that inner attribute is at file scope (R182-1: an inline
/// module's inner attribute hides only that module).
fn without_test_items(toks: &[String]) -> Vec<String> {
    let attr = |i: usize, inner: bool| -> Option<usize> {
        if toks.get(i).is_none_or(|t| t != "#")
            || toks.get(i + 1).is_some_and(|t| t == "!") != inner
        {
            return None;
        }
        let open = i + if inner { 2 } else { 1 };
        if toks.get(open).is_none_or(|t| t != "[") {
            return None;
        }
        let mut depth = 1;
        let mut end = open + 1;
        while end < toks.len() {
            match toks[end].as_str() {
                "[" => depth += 1,
                "]" => {
                    depth -= 1;
                    if depth == 0 {
                        return identity_test_attribute(&toks[open + 1..end]).then_some(end + 1);
                    }
                }
                _ => {}
            }
            end += 1;
        }
        None
    };
    // The index of the `}` that closes the `{` at `open` (the last token if it never closes).
    let close = |open: usize| {
        let mut depth = 0;
        for (k, t) in toks.iter().enumerate().skip(open) {
            match t.as_str() {
                "{" => depth += 1,
                "}" => {
                    depth -= 1;
                    if depth == 0 {
                        return k;
                    }
                }
                _ => {}
            }
        }
        toks.len().saturating_sub(1)
    };
    let mut cut: Vec<(usize, usize)> = Vec::new();
    let mut opens: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if let Some(after) = attr(i, true) {
            match opens.last() {
                None => return Vec::new(),
                Some(&open) => cut.push((open, close(open))),
            }
            i = after;
        } else if let Some(after) = attr(i, false) {
            let mut end = after;
            while end < toks.len() && toks[end] != "{" && toks[end] != ";" {
                end += 1;
            }
            if toks.get(end).is_some_and(|t| t == "{") {
                end = close(end);
            }
            cut.push((i, end));
            i = end + 1;
        } else {
            match toks[i].as_str() {
                "{" => opens.push(i),
                "}" => {
                    opens.pop();
                }
                _ => {}
            }
            i += 1;
        }
    }
    toks.iter()
        .enumerate()
        .filter(|(k, _)| !cut.iter().any(|(a, b)| a <= k && k <= b))
        .map(|(_, t)| t.clone())
        .collect()
}

/// Each use of `process::Command` (a path, or `Command` in a `process::{...}` import), with the fn it is in.
fn process_commands(toks: &[String]) -> Vec<(usize, Option<String>)> {
    let mut found = Vec::new();
    // One entry per open brace: the fn it opens, if any.
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut pending: Option<String> = None;
    // Inside `process::{`: the brace depth it opened at.
    let mut group: Option<usize> = None;
    for (i, t) in toks.iter().enumerate() {
        let path_before =
            i >= 3 && toks[i - 3] == "process" && toks[i - 2] == ":" && toks[i - 1] == ":";
        match t.as_str() {
            "fn" => pending = toks.get(i + 1).cloned(),
            "{" => {
                if path_before {
                    group = Some(stack.len());
                }
                stack.push(pending.take());
            }
            "}" => {
                stack.pop();
                if group.is_some_and(|d| stack.len() <= d) {
                    group = None;
                }
            }
            ";" => pending = None,
            "Command" if path_before || group.is_some() => {
                found.push((i, stack.iter().rev().find_map(Clone::clone)));
            }
            _ => {}
        }
    }
    found
}

fn identity_production_tokens(source: &str) -> Vec<String> {
    without_test_items(&tokens(source))
}

fn identity_client_only(root: &Path, rule: &Rule, ws: &[Package]) -> Vec<Violation> {
    let mut found = Vec::new();
    for package in ws.iter().filter(|p| !matches_any(&rule.allow, &p.name)) {
        for path in crate_sources(&package.dir) {
            let source = match fs::read_to_string(&path) {
                Ok(source) => source,
                Err(error) => {
                    found.push(violation(
                        &rule.id,
                        rel(root, &path),
                        format!("could not read identity call sites ({:?})", error.kind()),
                    ));
                    continue;
                }
            };
            let code = identity_production_tokens(&source);
            let identity_trait = code.iter().any(|t| t == "AgentIdentity")
                || code.windows(4).any(|w| w == ["ports", ":", ":", "*"]);
            if (0..code.len().saturating_sub(1)).any(|i| {
                code[i + 1] == "("
                    && (i == 0 || code[i - 1] != "fn")
                    && (code[i] == "client_actor" || (identity_trait && code[i] == "current"))
            }) {
                found.push(violation(
                    &rule.id,
                    rel(root, &path),
                    "identity discovery is client-side only (binary and compose)",
                ));
            }
        }
    }
    found
}

// ---------------------------------------------------------------- fixtures

/// How a fixture's plant is judged: by the umx-arch checker, or by the rule's own enforcer.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Enforcer {
    #[default]
    Checker,
    /// `cargo build`: the compiler enforces no_std.
    Build,
    /// `cargo clippy` with the workspace clippy.toml: clippy enforces the env ban.
    Clippy,
}

/// `fixture.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureSpec {
    #[serde(default)]
    enforcer: Enforcer,
    /// The exact rule ids that must fire; empty for an allowed control.
    expect: Vec<String>,
    /// Build and clippy plants: text the enforcer's failure must contain.
    #[serde(default)]
    must_show: Vec<String>,
    /// Matrix rows to enforce in this fixture.
    #[serde(default)]
    active_ports: Vec<String>,
    /// Overrides of what the blueprint says (default: the real blueprint).
    rule_ids: Option<Vec<String>>,
    layer_table: Option<String>,
    /// Treat the fixture as a whole workspace (see [`Inputs::whole_workspace`]).
    #[serde(default)]
    whole_workspace: bool,
    /// A rules file in the fixture to use instead of the real rules.toml; the blueprint then defaults
    /// to its ids and its render, so only what the plant breaks is reported.
    rules: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Described {
    row: Vec<WiringRow>,
}

/// What a fixture expects and what its enforcer reported.
#[derive(Debug)]
pub struct FixtureOutcome {
    pub name: String,
    pub enforcer: Enforcer,
    pub expected: BTreeSet<String>,
    pub reported: BTreeSet<String>,
    /// Checker violations, or the enforcer's output when it did not match.
    pub detail: Vec<String>,
}

/// Runs one fixture through its enforcer with the real rules. A fixture holds `fixture.toml`
/// and, for the checker, an optional `described.toml` standing in for `describe(mode)`.
/// It runs on a copy so cargo never writes into the source tree. `repo_root` supplies clippy.toml.
pub fn run_fixture(
    dir: &Path,
    rules: &Rules,
    blueprint: &Blueprint,
    repo_root: &Path,
) -> Result<FixtureOutcome, String> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let spec_path = dir.join("fixture.toml");
    let spec_text =
        fs::read_to_string(&spec_path).map_err(|e| format!("read {}: {e}", spec_path.display()))?;
    let spec: FixtureSpec =
        toml::from_str(&spec_text).map_err(|e| format!("parse {}: {e}", spec_path.display()))?;
    let local = match &spec.rules {
        Some(file) => Some(load_rules(&dir.join(file))?),
        None => None,
    };
    let (rules, blueprint) = match &local {
        Some(local) => (
            local,
            &Blueprint {
                rule_ids: local.ids().into_iter().collect(),
                layer_table: Some(local.render()),
            },
        ),
        None => (rules, blueprint),
    };
    let work = std::env::temp_dir().join(format!("umx-arch-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    copy_dir(dir, &work)?;
    let result = match spec.enforcer {
        Enforcer::Checker => run_checker(&work, &name, &spec, rules, blueprint),
        Enforcer::Build | Enforcer::Clippy => run_enforcer(&work, &spec, repo_root),
    };
    let _ = fs::remove_dir_all(&work);
    let (reported, detail) = result.map_err(|e| format!("fixture {name}: {e}"))?;
    Ok(FixtureOutcome {
        name,
        enforcer: spec.enforcer,
        expected: spec.expect.into_iter().collect(),
        reported,
        detail,
    })
}

type Judged = (BTreeSet<String>, Vec<String>);

fn run_checker(
    work: &Path,
    name: &str,
    spec: &FixtureSpec,
    rules: &Rules,
    real: &Blueprint,
) -> Result<Judged, String> {
    let blueprint = Blueprint {
        rule_ids: match &spec.rule_ids {
            Some(ids) => ids.iter().cloned().collect(),
            None => real.rule_ids.clone(),
        },
        layer_table: spec
            .layer_table
            .clone()
            .or_else(|| real.layer_table.clone()),
    };
    let described = match fs::read_to_string(work.join("described.toml")) {
        Ok(text) => {
            let d: Described =
                toml::from_str(&text).map_err(|e| format!("{name}/described.toml: {e}"))?;
            Some(d.row)
        }
        Err(_) => None,
    };
    let active: BTreeSet<String> = spec.active_ports.iter().cloned().collect();
    let report = check(&Inputs {
        root: work,
        rules,
        blueprint: &blueprint,
        active_ports: &active,
        described: described.as_deref(),
        whole_workspace: spec.whole_workspace,
    })?;
    Ok((
        report.violations.iter().map(|v| v.rule.clone()).collect(),
        report.violations.iter().map(ToString::to_string).collect(),
    ))
}

/// A plant counts as caught only when the enforcer fails AND its output shows the expected
/// reason; a failure for any other reason, or a failing control, is reported as such.
fn run_enforcer(work: &Path, spec: &FixtureSpec, repo_root: &Path) -> Result<Judged, String> {
    let mut cmd = Command::new("cargo");
    match spec.enforcer {
        Enforcer::Clippy => {
            fs::copy(repo_root.join("clippy.toml"), work.join("clippy.toml"))
                .map_err(|e| format!("copy clippy.toml: {e}"))?;
            cmd.arg("clippy");
        }
        _ => {
            cmd.arg("build");
        }
    }
    cmd.args(["--offline", "--quiet", "--manifest-path"])
        .arg(work.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(work.join("target"));
    if spec.enforcer == Enforcer::Clippy {
        cmd.args(["--", "-D", ENV_LINT]);
    }
    let out = cmd.output().map_err(|e| format!("run cargo: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        return Ok((BTreeSet::new(), Vec::new()));
    }
    let missing: Vec<&String> = spec
        .must_show
        .iter()
        .filter(|m| !text.contains(m.as_str()))
        .collect();
    if !spec.expect.is_empty() && missing.is_empty() {
        return Ok((spec.expect.iter().cloned().collect(), Vec::new()));
    }
    let label = if spec.expect.is_empty() {
        "control failed".to_string()
    } else {
        format!("failed without {missing:?}")
    };
    Ok((
        BTreeSet::from([label]),
        text.lines().map(str::to_string).collect(),
    ))
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("mkdir {}: {e}", to.display()))?;
    for e in fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst).map_err(|e| format!("copy {}: {e}", src.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_items_do_not_mask_shipped_calls_in_neighboring_scopes() {
        let source = r#"
            mod local_tests {
                #![cfg(test)]
                fn hidden(a: &dyn AgentIdentity) {
                    a.current();
                    std::process::Command::new("test");
                }
            }
            #[cfg(all(test, feature = "stub"))]
            fn contract(a: &dyn AgentIdentity) {
                a.current();
                std::process::Command::new("test");
            }
            fn production(a: &dyn AgentIdentity) {
                a.current();
                std::process::Command::new("shipped");
            }
            #[cfg(any(test, feature = "shipping"))]
            fn conditional(a: &dyn AgentIdentity) {
                a.current();
                std::process::Command::new("shipped");
            }
        "#;
        let code = identity_production_tokens(source);
        assert_eq!(code.iter().filter(|t| t.as_str() == "current").count(), 2);
        let calls: Vec<_> = process_commands(&code)
            .into_iter()
            .map(|(_, name)| name.unwrap())
            .collect();
        assert_eq!(calls, ["production", "conditional"]);
    }

    #[test]
    fn clause_names_need_prefix_clause_number_and_name() {
        let src = "pub fn gf_c1_write_then_read() {}\nfn gf_c12_list_sorted(){}\n\
                   fn helper() {}\nfn gf_cx_no() {}\nfn gf_c2_() {}";
        let got: Vec<String> = clause_names(src).into_iter().collect();
        assert_eq!(got, ["gf_c12_list_sorted", "gf_c1_write_then_read"]);
    }

    #[test]
    fn tokens_drop_comments_and_literals_and_unraw_identifiers() {
        let src = "extern /* a */ crate\n r#std as s; // extern crate std\n\
                   let x = \"extern crate std\"; let y = r#\"q\"#; let c = '\\''; fn f<'a>() {}";
        let t = tokens(src);
        assert_eq!(&t[..5], ["extern", "crate", "std", "as", "s"]);
        assert_eq!(t.iter().filter(|x| *x == "std").count(), 1);
        assert!(t.contains(&"a".to_string()), "lifetime name kept");
    }

    #[test]
    fn calls_count_but_definitions_and_comments_do_not() {
        let src = "fn gf_c1_a() {}\n// gf_c2_b()\nfn t() { suite::gf_c3_c(&x); }";
        let got: Vec<String> = called_names(src)
            .into_iter()
            .filter(|n| is_clause(n))
            .collect();
        assert_eq!(got, ["gf_c3_c"]);
    }

    #[test]
    fn a_contract_macro_list_counts_as_run() {
        let src = "umx_contracts::graph_files_contract!(\n    graph_files;\n    gf_c1_a,\n    \
                   // gf_c2_b,\n    gf_c3_c,\n);\nnot_a_macro!(gf_c4_d);\nother_contract(gf_c5_e);";
        let got: Vec<String> = called_names(src)
            .into_iter()
            .filter(|n| is_clause(n))
            .collect();
        assert_eq!(got, ["gf_c1_a", "gf_c3_c"]);
    }

    #[test]
    fn use_trees_expand_to_leaves_with_their_bindings() {
        let leaves = |src: &str| -> Vec<(String, Option<String>, bool)> {
            use_items(&tokens(src))
                .into_iter()
                .flat_map(|(_, l)| l)
                .map(|l| (l.path.join("::"), l.binds, l.glob))
                .collect()
        };
        let got = leaves("use serde_json::{self as x, Value as Y, map::{Map, *}};");
        assert_eq!(
            got,
            [
                ("serde_json".into(), Some("x".into()), false),
                ("serde_json::Value".into(), Some("Y".into()), false),
                ("serde_json::map::Map".into(), Some("Map".into()), false),
                ("serde_json::map".into(), None, true),
            ]
        );
        assert_eq!(
            leaves("pub use ::umx_core as _; extern crate umx_protocol as w;"),
            [
                ("umx_core".into(), None, false),
                ("umx_protocol".into(), Some("w".into()), false),
            ]
        );
        let items = use_items(&tokens("pub use a::b; pub(crate) use c::d; use e;"));
        let public: Vec<bool> = items.iter().map(|(p, _)| *p).collect();
        assert_eq!(public, [true, false, false]);
    }

    #[test]
    fn a_use_rooted_at_an_extern_crate_std_alias_counts_as_rooted_at_std() {
        let what = |src: &str| -> Vec<&str> {
            let toks = tokens(src);
            fs_uses(&toks, &std_names([toks.as_slice()]))
                .into_iter()
                .map(|u| u.2)
                .collect()
        };
        let src =
            "extern crate std as host;\nuse host::{fs as f};\nfn g() { let _ = f::read(\"x\"); }";
        assert_eq!(what(src), ["a use of std::fs"]);
        assert!(
            what("extern crate std as host;\nuse host::{io as f};\nfn g() { f::stdin(); }")
                .is_empty()
        );
        // R258: a glob of a std module on the way to fs fires at the use, through an alias too; others do not.
        let glob = ["a glob of a std module on the way to fs"];
        assert_eq!(
            what("use std as s;\nuse s::*;\nfn g() { let _ = fs::read; }"),
            glob
        );
        assert_eq!(
            what("use std::os::*;\nuse self::unix as u;\nfn g() { u::fs::symlink(\"a\", \"b\"); }"),
            glob
        );
        assert!(
            what("use std::io::*;\nuse std::os::unix::process::*;\nuse std::{collections::*};")
                .is_empty()
        );
        // A local name for std counts anywhere in a path: after a leading `::`, and after self, crate or a module,
        // which may name the same alias (F119, security: self::host is std here).
        let renamed = ["std::fs through a renamed std path"];
        let host = |body: &str| what(&format!("extern crate std as host;\nfn g() {{ {body} }}"));
        assert_eq!(host("return ::host::fs::read(\"x\");"), renamed);
        assert_eq!(host("let _ = (::host::fs::read)(\"x\");"), renamed);
        assert_eq!(host("let _ = a::host::fs::read(\"x\");"), renamed);
        assert_eq!(host("let _ = self::host::fs::read(\"x\");"), renamed);
        assert_eq!(
            host("let _ = crate::host::os::unix::fs::symlink(\"a\", \"b\");"),
            renamed
        );
        assert!(host("let _ = crate::host::io::stderr();").is_empty());
        assert_eq!(what("use ::std::{fs as f};"), ["a use of std::fs"]);
    }

    #[test]
    fn wildcard_matches_by_prefix_only() {
        assert!(matches("umx-adapter-*", "umx-adapter-fs"));
        assert!(!matches("umx-adapter-*", "umx-adapters"));
        assert!(matches("umx-core", "umx-core"));
        assert!(!matches("umx-core", "umx-core2"));
    }
}
