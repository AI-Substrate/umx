//! LadybugDB conformance check: loads a fixture folder (nodes.jsonl, edges.jsonl, queries.json) into an
//! in-memory LadybugDB and runs every query. A query marked `conformance` must return its `expected` rows;
//! aliases run their `alias_of` twin, and a probe with `lbug_error` must still fail with that message.
//! Other probes are recorded only. Writes the full record as JSON and exits 1 on any mismatch.
//!
//! Usage: lbug-conformance <fixture_dir> <out.json>
//!
//! Second origin for umx's own run of the same file (crates/umx-cypher/tests/design.rs and
//! crates/umx-query). No timing, no extensions: the check never runs LOAD, so nothing is fetched at run time.
//!
//! The fixture is a umx graph folder: `evidence_kinds` is derived from `evidence`, as umx does. umx keeps one
//! edge of a reversed pair of a symmetric relation (D49, F48) and LadybugDB would keep both, so a fixture
//! holding such a pair is refused rather than compared (exit 2).

#[path = "../../../crates/umx-cypher/tests/rows/mod.rs"]
mod rows;

use lbug::{Connection, Database, SystemConfig, Value};
use serde_json::{Value as J, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

const TYPES: [&str; 5] = ["problem", "option", "question", "decision", "criterion"];
const RELS: [&str; 7] = [
    "blocks",
    "supports",
    "undercuts",
    "depends_on",
    "answers",
    "part_of",
    "conflicts_with",
];

/// A Cypher literal for a fixture value. The fixture is ours, but quote it properly anyway.
fn lit(v: &J) -> String {
    match v {
        J::Null => "NULL".into(),
        J::String(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
        J::Array(xs) if xs.is_empty() => "CAST([] AS STRING[])".into(),
        J::Array(xs) => format!("[{}]", xs.iter().map(lit).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

/// A cell as text, floats at full precision: `rows` compares them within 1e-9.
fn show(v: &Value) -> String {
    match v {
        Value::Null(_) => "null".into(),
        Value::Double(f) => format!("{f:?}"),
        Value::Float(f) => format!("{f:?}"),
        Value::String(s) => s.clone(),
        Value::List(_, xs) | Value::Array(_, xs) => {
            format!("[{}]", xs.iter().map(show).collect::<Vec<_>>().join(","))
        }
        other => other.to_string(),
    }
}

fn read_lines(dir: &PathBuf, file: &str) -> Vec<J> {
    std::fs::read_to_string(dir.join(file))
        .unwrap_or_else(|e| panic!("{file}: {e}"))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("fixture json"))
        .collect()
}

fn text<'a>(v: &'a J, key: &str) -> &'a str {
    v[key].as_str().unwrap_or_else(|| panic!("missing {key}"))
}

fn first_line(e: impl ToString) -> String {
    e.to_string()
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(200)
        .collect()
}

/// Symmetric in every umx graph (umx-core `SYMMETRIC_RELATIONS`, D49), plus graph.json's `ontology.symmetric`.
const SYMMETRIC: [&str; 3] = ["conflicts_with", "alternative_to", "possibly_same_as"];

/// The first reversed pair of a symmetric relation: `(a r b)` and `(b r a)` both stored.
fn reversed_symmetric_pair(dir: &std::path::Path, edges: &[J]) -> Option<String> {
    let graph: J = std::fs::read_to_string(dir.join("graph.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(J::Null);
    let marked: Vec<&str> = graph["ontology"]["symmetric"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(J::as_str)
        .collect();
    let symmetric = |r: &str| SYMMETRIC.contains(&r) || marked.contains(&r);
    let stored: std::collections::BTreeSet<(&str, &str, &str)> = edges
        .iter()
        .map(|e| (text(e, "type"), text(e, "from"), text(e, "to")))
        .collect();
    stored
        .iter()
        .find(|(r, a, b)| symmetric(r) && a != b && stored.contains(&(*r, *b, *a)))
        .map(|(r, a, b)| format!("{a} {r} {b} and its reverse"))
}

fn load(c: &Connection, nodes: &[J], edges: &[J]) {
    for t in TYPES {
        c.query(&format!(
            "CREATE NODE TABLE {t}(id STRING, title STRING, status STRING, evidence_kinds STRING[], due STRING, \
             likelihood INT64, PRIMARY KEY(id))"
        ))
        .expect("node table");
    }
    let pairs = TYPES
        .iter()
        .flat_map(|a| TYPES.iter().map(move |b| format!("FROM {a} TO {b}")))
        .collect::<Vec<_>>()
        .join(", ");
    for r in RELS {
        c.query(&format!(
            "CREATE REL TABLE {r}({pairs}, id STRING, confidence DOUBLE)"
        ))
        .expect("rel table");
    }
    for n in nodes {
        let t = text(n, "type");
        let kinds: std::collections::BTreeSet<&str> = n["evidence"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["kind"].as_str())
            .collect();
        let kinds = J::from(kinds.into_iter().collect::<Vec<_>>());
        assert!(TYPES.contains(&t), "unknown node type {t}");
        c.query(&format!(
            "CREATE (:{t} {{id: {}, title: {}, status: {}, evidence_kinds: {}, due: {}, likelihood: {}}})",
            lit(&n["id"]),
            lit(&n["title"]),
            lit(&n["status"]),
            lit(&kinds),
            lit(n.get("due").unwrap_or(&J::Null)),
            lit(n.get("likelihood").unwrap_or(&J::Null)),
        ))
        .expect("node");
    }
    let ty: BTreeMap<&str, &str> = nodes
        .iter()
        .map(|n| (text(n, "id"), text(n, "type")))
        .collect();
    for e in edges {
        let (r, s, d) = (text(e, "type"), text(e, "from"), text(e, "to"));
        assert!(RELS.contains(&r), "unknown relation {r}");
        let confidence = e["confidence"]
            .as_f64()
            .map_or("CAST(NULL AS DOUBLE)".into(), |f| format!("{f:?}"));
        c.query(&format!(
            "MATCH (a:{} {{id: {}}}), (b:{} {{id: {}}}) CREATE (a)-[:{r} {{id: {}, confidence: {confidence}}}]->(b)",
            ty[s],
            lit(&e["from"]),
            ty[d],
            lit(&e["to"]),
            lit(&e["id"]),
        ))
        .expect("edge");
    }
}

/// One scalar from a single-row query, or the error's first line.
fn scalar(c: &Connection, q: &str) -> String {
    match c.query(q) {
        Ok(mut r) => r
            .next()
            .and_then(|row| row.first().map(show))
            .unwrap_or_default(),
        Err(e) => format!("error: {}", first_line(e)),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let [_, dir, out] = args.as_slice() else {
        eprintln!("usage: lbug-conformance <fixture_dir> <out.json>");
        return ExitCode::from(2);
    };
    let dir = PathBuf::from(dir);
    let nodes = read_lines(&dir, "nodes.jsonl");
    let edges = read_lines(&dir, "edges.jsonl");
    let queries: J = serde_json::from_str(
        &std::fs::read_to_string(dir.join("queries.json")).expect("queries.json"),
    )
    .expect("queries json");

    if let Some(pair) = reversed_symmetric_pair(&dir, &edges) {
        eprintln!(
            "lbug-conformance: the fixture stores {pair}; umx keeps one edge of such a pair (D49) and LadybugDB \
             both, so the rows cannot be compared. Remove the reversed line."
        );
        return ExitCode::from(2);
    }

    let db = Database::in_memory(SystemConfig::default().max_num_threads(4)).expect("database");
    let c = Connection::new(&db).expect("connection");
    load(&c, &nodes, &edges);

    let mut results = Vec::new();
    let mut mismatches = 0;
    for q in queries.as_array().expect("query list") {
        let (id, cypher) = (text(q, "id"), text(q, "cypher"));
        let lbug_query = q["alias_of"].as_str().unwrap_or(cypher);
        let conformance = q["conformance"] == true;
        let mut rec = json!({"id": id, "conformance": conformance});
        if lbug_query != cypher {
            rec["alias_of"] = json!(lbug_query);
        }
        match c.query(lbug_query) {
            Err(e) => rec["error"] = json!(first_line(e)),
            Ok(res) => {
                let mut rows: Vec<Vec<String>> =
                    res.map(|r| r.iter().map(show).collect()).collect();
                if q["ordered"] != true {
                    rows.sort();
                }
                rec["rows"] = json!(rows);
            }
        }
        let verdict = if conformance {
            let want: Vec<Vec<String>> =
                serde_json::from_value(q["expected"].clone()).expect("expected rows");
            let got: Option<Vec<Vec<String>>> = serde_json::from_value(rec["rows"].clone()).ok();
            let ok = got.is_some_and(|g| rows::rows_match(&g, &want));
            if !ok {
                mismatches += 1;
            }
            rec["agrees"] = json!(ok);
            if ok { "agrees" } else { "DIFFERS" }
        } else if let Some(expected_error) = q["lbug_error"].as_str() {
            let ok = rec["error"]
                .as_str()
                .is_some_and(|e| e.contains(expected_error));
            rec["agrees"] = json!(ok);
            if !ok {
                mismatches += 1;
            }
            if ok { "refused" } else { "DIFFERS" }
        } else {
            "recorded"
        };
        println!(
            "{verdict:9} {id:28} {}",
            rec.get("error").and_then(J::as_str).unwrap_or("")
        );
        results.push(rec);
    }

    let conformance_agree = results
        .iter()
        .filter(|r| r["conformance"] == true && r["agrees"] == true)
        .count();
    let record = json!({
        "engine": format!("lbug {}", scalar(&c, "CALL db_version() RETURN *")),
        "crate": "lbug =0.22.3",
        "recursive_pattern_semantic": scalar(&c, "CALL current_setting('recursive_pattern_semantic') RETURN *"),
        "fixture": dir.display().to_string(),
        "conformance_queries": results.iter().filter(|r| r["conformance"] == true).count(),
        "mismatches": mismatches,
        "results": results,
    });
    std::fs::write(
        out,
        serde_json::to_string_pretty(&record).expect("json") + "\n",
    )
    .expect("write out");
    println!(
        "{} recursive_pattern_semantic={} conformance {}/{} agree",
        record["engine"].as_str().unwrap_or(""),
        record["recursive_pattern_semantic"].as_str().unwrap_or(""),
        conformance_agree,
        record["conformance_queries"]
    );
    if mismatches == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
