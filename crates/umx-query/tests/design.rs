//! umx-query over the design fixture loaded by umx-graph (workstream 005 s02).
//!
//! Two origins for every supported fixture query (architect, ws005 section 4 step 2): `run` must return the rows
//! LadybugDB 0.22.3 returned (queries.json `expected`), and so must a hand-written oracle: plain Rust over the
//! graph's views, no Cypher. Determinism: the same rows on every run and load order.

#[path = "../../umx-cypher/tests/rows/mod.rs"]
mod rows;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value as J;
use umx_core::codec::{decode_edges, decode_nodes};
use umx_core::problem::{Problem, Slot, ValidName};
use umx_core::{Edge, Id, Node, Ontology};
use umx_graph::{Graph, NodeIx, NodeRef};
use umx_query::{Cell, GraphView, RunOptions, run};

/// The design graph's ontology: the starter one, whose symmetric relations (D49) are the built-ins. Services pass
/// `meta.ontology.symmetric_kinds()` the same way.
fn ontology() -> Ontology {
    Ontology::starter()
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../umx-cypher/fixtures/design")
}

fn records(dir: &Path) -> (Vec<Node>, Vec<Edge>) {
    let nodes = decode_nodes(&fs::read(dir.join("nodes.jsonl")).unwrap()).unwrap();
    let edges = decode_edges(&fs::read(dir.join("edges.jsonl")).unwrap()).unwrap();
    (nodes.records, edges.records)
}

fn design(reverse: bool) -> Graph {
    let (mut nodes, mut edges) = records(&fixture_dir());
    if reverse {
        nodes.reverse();
        edges.reverse();
    }
    Graph::from_records(nodes, edges)
}

fn queries() -> Vec<J> {
    let text = fs::read_to_string(fixture_dir().join("queries.json")).unwrap();
    serde_json::from_str::<J>(&text)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

/// A cell as the conformance checker prints it (tools/lbug-conformance `show`).
fn show(c: &Cell) -> String {
    match c {
        Cell::Null => "null".into(),
        Cell::Bool(b) => b.to_string(),
        Cell::Int(i) => i.to_string(),
        Cell::Float(f) => format!("{f:?}"),
        Cell::Str(s) => s.clone(),
        Cell::List(xs) => format!("[{}]", xs.iter().map(show).collect::<Vec<_>>().join(",")),
        Cell::Map(m) => format!("{m:?}"),
        Cell::Node(n) => n.id.to_text(),
        Cell::Edge(e) => e.id.to_text(),
        Cell::Similar(e) => format!("{}>{}", e.from, e.to),
        Cell::Provenance(e) => format!("{}>{}", e.from, e.to),
    }
}

fn query(g: &Graph, text: &str) -> Result<Vec<Vec<String>>, Problem> {
    let o = ontology();
    let rows = run(
        &GraphView::one("design", g, &o.symmetric_kinds()),
        text,
        &RunOptions::default(),
    )?;
    Ok(rows
        .rows
        .iter()
        .map(|r| r.iter().map(show).collect())
        .collect())
}

#[test]
fn provenance_is_excluded_from_scans_and_indexed_lookups_unless_the_view_includes_it() {
    let mut g = design(false);
    let before = g.node_count() as i64;
    let mut provenance = g.nodes().next().unwrap().to_node();
    provenance.id = Id(u128::MAX - 1);
    provenance.kind = "session".into();
    provenance.fields.insert(
        "prov_key".into(),
        serde_json::json!("session/copilot/h-0123456789abcdef"),
    );
    let id = provenance.id;
    g.insert_node(provenance).unwrap();
    let o = ontology();
    for (include, total, found) in [(false, before, 0), (true, before + 1, 1)] {
        let view = GraphView::one("design", &g, &o.symmetric_kinds()).with_provenance(include);
        for (text, expected) in [
            ("MATCH (n) RETURN count(n)".into(), total),
            (
                format!("MATCH (n) WHERE n.id = '{id}' RETURN count(n)"),
                found,
            ),
        ] {
            let rows = run(&view, &text, &RunOptions::default()).unwrap().rows;
            assert_eq!(rows, [vec![Cell::Int(expected)]]);
        }
    }
}

/// A design node's id from its short name (fixtures/design/README.md).
fn id(short: &str) -> String {
    format!("01JDCYPHER{:0>16}", short.replacen('O', "N", 1))
}

// ---- The oracle: plain Rust over umx-graph's views, one function per fixture query ----------------------------

type Rows = Vec<Vec<String>>;

fn node<'g>(g: &'g Graph, short: &str) -> NodeRef<'g> {
    g.node(id(short).parse::<Id>().unwrap())
        .expect("a design node")
}

fn text_of(n: NodeRef<'_>) -> String {
    n.id().to_text()
}

fn str_field(n: NodeRef<'_>, name: &str) -> Option<String> {
    n.field(name).and_then(|v| v.as_str().map(String::from))
}

/// The nodes at the other end of `n`'s edges of `rel` (any relation but similar_to when `None`), out then in.
fn steps(
    g: &Graph,
    n: NodeIx,
    rel: Option<&str>,
    out: bool,
    inc: bool,
) -> Vec<(umx_graph::EdgeIx, NodeIx)> {
    let keep = |k: &str| rel.map_or(k != "similar_to", |r| r == k);
    let mut v: Vec<_> = Vec::new();
    if out {
        v.extend(
            g.edges_out(n)
                .filter(|s| keep(s.kind.as_str()))
                .map(|s| (s.edge, s.other)),
        );
    }
    if inc {
        v.extend(
            g.edges_in(n)
                .filter(|s| keep(s.kind.as_str()))
                .map(|s| (s.edge, s.other)),
        );
    }
    v
}

/// Walk endpoints of exactly `len` steps (edges may repeat; LadybugDB's WALK semantics).
fn walks(g: &Graph, from: NodeIx, rels: &[&str], len: usize, out: bool, inc: bool) -> Vec<NodeIx> {
    let mut frontier = vec![from];
    for _ in 0..len {
        frontier = frontier
            .iter()
            .flat_map(|&n| {
                rels.iter()
                    .flat_map(move |r| steps(g, n, Some(r), out, inc))
            })
            .map(|(_, o)| o)
            .collect();
    }
    frontier
}

fn ids_sorted<'g>(it: impl Iterator<Item = NodeRef<'g>>) -> Rows {
    let mut v: Vec<String> = it.map(text_of).collect();
    v.sort();
    v.into_iter().map(|s| vec![s]).collect()
}

fn counted(ends: impl Iterator<Item = String>) -> Rows {
    let mut m: BTreeMap<String, usize> = BTreeMap::new();
    for e in ends {
        *m.entry(e).or_default() += 1;
    }
    m.into_iter().map(|(k, c)| vec![k, c.to_string()]).collect()
}

fn conf(g: &Graph, e: umx_graph::EdgeIx) -> Option<f64> {
    g.edge_at(e).field("confidence").and_then(|v| v.as_f64())
}

#[allow(clippy::too_many_lines)]
fn oracle(g: &Graph, query_id: &str) -> Option<Rows> {
    let kind = |k: &'static str| move |n: &NodeRef<'_>| n.kind() == k;
    let tid = |ix: NodeIx| text_of(g.node_at(ix));
    Some(match query_id {
        "z04-type-fn" => g
            .edges()
            .filter(|e| e.kind() == "answers")
            .map(|e| vec![e.kind().into()])
            .collect(),
        "m01-label" => ids_sorted(g.nodes().filter(kind("question"))),
        "m02-label-or" => ids_sorted(
            g.nodes()
                .filter(|n| matches!(n.kind(), "question" | "criterion")),
        ),
        "m03-prop-map" => vec![vec![node(g, "D1").title().into()]],
        "m04-out" | "m05-in" => {
            let mut v: Vec<String> = steps(g, node(g, "D1").ix(), Some("blocks"), false, true)
                .into_iter()
                .map(|(_, o)| tid(o))
                .collect();
            v.sort();
            v.into_iter().map(|s| vec![s]).collect()
        }
        "m06-undirected" => steps(g, node(g, "D2").ix(), Some("conflicts_with"), true, true)
            .into_iter()
            .map(|(_, o)| vec![tid(o)])
            .collect(),
        "m07-rel-types-or" => {
            let mut v: Vec<(String, String, f64)> = Vec::new();
            for r in ["supports", "undercuts"] {
                for (e, o) in steps(g, node(g, "O1").ix(), Some(r), true, false) {
                    v.push((r.into(), tid(o), conf(g, e)?));
                }
            }
            v.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
            v.into_iter()
                .map(|(r, i, c)| vec![r, i, format!("{c:?}")])
                .collect()
        }
        "m08-untyped-parallel" => counted(
            steps(g, node(g, "O1").ix(), None, true, true)
                .into_iter()
                .map(|(_, o)| tid(o)),
        ),
        "m09-self-loop-undirected" => counted(
            steps(g, node(g, "C1").ix(), Some("undercuts"), true, true)
                .into_iter()
                .map(|(_, o)| tid(o)),
        ),
        "m10-relvar" => steps(g, node(g, "O2").ix(), Some("undercuts"), true, false)
            .into_iter()
            .map(|(e, o)| {
                let edge = g.edge_at(e);
                vec![
                    edge.id().to_text(),
                    edge.kind().into(),
                    format!("{:?}", conf(g, e).unwrap()),
                    tid(o),
                ]
            })
            .collect(),
        "m11-degree" => {
            let mut v: Vec<(usize, String)> = g
                .nodes()
                .map(|n| (steps(g, n.ix(), None, true, true).len(), text_of(n)))
                .filter(|(d, _)| *d > 0)
                .collect();
            v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            v.into_iter()
                .take(3)
                .map(|(d, i)| vec![i, d.to_string()])
                .collect()
        }
        "m12-varlen-cycle" => counted(
            (1..=4)
                .flat_map(|len| walks(g, node(g, "D1").ix(), &["depends_on"], len, true, false))
                .map(tid),
        ),
        "m13-exactly-two" => {
            let set: BTreeSet<String> = walks(g, node(g, "Q1").ix(), &["blocks"], 2, true, false)
                .into_iter()
                .map(tid)
                .collect();
            set.into_iter().map(|s| vec![s]).collect()
        }
        "m14-two-patterns" => {
            let mut v = Vec::new();
            for e in g.edges().filter(|e| e.kind() == "blocks") {
                let d = g.node_at(e.to_ix().unwrap());
                if d.kind() != "decision" {
                    continue;
                }
                for (_, x) in steps(g, d.ix(), Some("depends_on"), true, false) {
                    v.push(vec![e.from().to_text(), text_of(d), tid(x)]);
                }
            }
            v.sort_by(|a, b| (&a[0], &a[1]).cmp(&(&b[0], &b[1])));
            v
        }
        "m15-rel-uniqueness" => walks(g, node(g, "D1").ix(), &["conflicts_with"], 2, true, true)
            .into_iter()
            .map(|n| vec![tid(n)])
            .collect(),
        "m16-zero-lower" => {
            let start = node(g, "Q1").ix();
            let mut v: Vec<String> = std::iter::once(start)
                .chain(walks(g, start, &["blocks"], 1, true, false))
                .map(tid)
                .collect();
            v.sort();
            v.into_iter().map(|s| vec![s]).collect()
        }
        "z15-walk-undirected" => counted(
            walks(
                g,
                node(g, "D3").ix(),
                &["conflicts_with", "depends_on"],
                2,
                true,
                true,
            )
            .into_iter()
            .map(tid),
        ),
        "w01-bool" => ids_sorted(g.nodes().filter(|n| {
            n.field("likelihood")
                .and_then(|v| v.as_i64())
                .is_some_and(|l| l >= 3)
                || (n.status() == "contested" && !n.title().starts_with("Vendor"))
        })),
        "w02-in-neq" => ids_sorted(g.nodes().filter(|n| {
            n.kind() == "question"
                && matches!(n.status(), "open" | "proposed")
                && text_of(*n) != id("Q3")
        })),
        "w03-is-null" => ids_sorted(
            g.nodes()
                .filter(|n| n.kind() == "decision" && n.field("due").is_none()),
        ),
        "w04-strings" => ids_sorted(
            g.nodes()
                .filter(|n| n.title().contains("Launch") && n.title().ends_with("May")),
        ),
        "w05-float-edge" => {
            let mut v: Vec<Vec<String>> = g
                .edges()
                .filter(|e| e.kind() == "supports" && conf(g, e.ix()).is_some_and(|c| c > 0.5))
                .map(|e| {
                    vec![
                        e.from().to_text(),
                        e.to().to_text(),
                        format!("{:?}", conf(g, e.ix()).unwrap()),
                    ]
                })
                .collect();
            v.sort();
            v
        }
        "w06-list-in" => ids_sorted(g.nodes().filter(|n| {
            let kinds = n.field("evidence_kinds").unwrap();
            let kinds = kinds.as_array().unwrap();
            kinds.iter().any(|k| k == "analyst") && kinds.len() == 1
        })),
        "w08-not-exists" => ids_sorted(g.nodes().filter(|n| {
            n.kind() == "question"
                && matches!(n.status(), "open" | "proposed")
                && steps(g, n.ix(), Some("answers"), false, true).is_empty()
        })),
        "w09-exists-where" => ids_sorted(g.nodes().filter(|n| {
            n.kind() == "decision"
                && steps(g, n.ix(), Some("blocks"), false, true)
                    .iter()
                    .any(|(_, q)| {
                        let q = g.node_at(*q);
                        q.kind() == "question" && q.status() == "open"
                    })
        })),
        "o01-optional" => {
            let mut v = Vec::new();
            for q in g.nodes().filter(kind("question")) {
                let by = steps(g, q.ix(), Some("answers"), false, true);
                if by.is_empty() {
                    v.push(vec![text_of(q), "null".into()]);
                }
                for (_, d) in by {
                    v.push(vec![text_of(q), tid(d)]);
                }
            }
            v.sort_by(|a, b| a[0].cmp(&b[0]));
            v
        }
        "a01-agg" => g
            .nodes()
            .filter(kind("decision"))
            .filter_map(|d| {
                let mut srcs: Vec<String> = steps(g, d.ix(), Some("blocks"), false, true)
                    .into_iter()
                    .map(|(_, b)| tid(b))
                    .collect();
                srcs.sort();
                Some(vec![
                    text_of(d),
                    srcs.len().to_string(),
                    srcs.first()?.clone(),
                    srcs.last()?.clone(),
                ])
            })
            .collect(),
        "a02-agg-float" => {
            let edges: Vec<_> = g
                .edges()
                .filter(|e| matches!(e.kind(), "supports" | "undercuts"))
                .collect();
            let sources: BTreeSet<Id> = edges.iter().map(|e| e.from()).collect();
            let total: f64 = edges.iter().filter_map(|e| conf(g, e.ix())).sum();
            #[allow(clippy::cast_precision_loss)]
            let mean = total / edges.len() as f64;
            vec![vec![
                sources.len().to_string(),
                edges.len().to_string(),
                format!("{total:?}"),
                format!("{mean:?}"),
            ]]
        }
        "a03-collect-sorted" => {
            let mut srcs: Vec<String> = steps(g, node(g, "D1").ix(), Some("blocks"), false, true)
                .into_iter()
                .map(|(_, b)| tid(b))
                .collect();
            srcs.sort();
            vec![vec![format!("[{}]", srcs.join(","))]]
        }
        "x01-with-filter" => counted(
            g.edges()
                .filter(|e| e.kind() == "blocks")
                .map(|e| e.to().to_text()),
        )
        .into_iter()
        .filter(|r| r[1].parse::<usize>().unwrap() >= 2)
        .collect(),
        "r01-distinct-skip-limit" => {
            let set: BTreeSet<String> = g
                .edges()
                .filter(|e| e.kind() == "blocks")
                .map(|e| e.from().to_text())
                .collect();
            set.into_iter()
                .rev()
                .skip(1)
                .take(2)
                .map(|s| vec![s])
                .collect()
        }
        "u01-unwind" => ["D1", "Q9"]
            .iter()
            .map(|s| {
                let title = id(s)
                    .parse::<Id>()
                    .ok()
                    .and_then(|i| g.node(i))
                    .map(|n| n.title().to_string());
                vec![id(s), title.unwrap_or_else(|| "null".into())]
            })
            .collect(),
        "c01-case" => g
            .nodes()
            .filter(kind("decision"))
            .map(|d| {
                vec![
                    text_of(d),
                    if d.status() == "agreed" {
                        "settled"
                    } else {
                        "open"
                    }
                    .into(),
                ]
            })
            .collect(),
        "f01-functions" => {
            let p = node(g, "P1");
            vec![vec![
                p.title().to_lowercase(),
                p.status().to_uppercase(),
                str_field(p, "due").unwrap_or_else(|| "none".into()),
                p.kind().into(),
            ]]
        }
        "k01-comment" => ids_sorted(g.nodes().filter(kind("criterion"))),
        // 3.0 IN [3]: true for every criterion.
        "n01-in-list-numeric" => ids_sorted(g.nodes().filter(kind("criterion"))),
        // [3, 3.0] holds one distinct value.
        "n02-count-distinct-numeric" | "n03-with-distinct-numeric" => {
            let values: BTreeSet<u64> = [3.0_f64, 3.0].iter().map(|f| f.to_bits()).collect();
            vec![vec![values.len().to_string()]]
        }
        // 3 and 3.0 are one group of two.
        "n04-group-numeric-count" => vec![vec!["2".into()]],
        // [3.0] is in [[3]]: once per criterion, and once more per extra input row when not hoisted.
        "n05-nested-in-hoisted" => vec![vec![
            g.nodes().filter(kind("criterion")).count().to_string(),
        ]],
        "n06-nested-in-not-hoisted" => {
            vec![vec![
                (2 * g.nodes().filter(kind("criterion")).count()).to_string(),
            ]]
        }
        // [3, 4.0] = [3.0, 4] element by element.
        "n07-list-equality" => {
            let (ints, floats) = ([3_i64, 4], [3.0_f64, 4.0]);
            #[allow(clippy::cast_precision_loss)]
            let equal = ints.iter().zip(floats).all(|(i, f)| *i as f64 == f);
            vec![vec![usize::from(equal).to_string()]]
        }
        // An undercuts edge whose ends are one node: the self-loop.
        "p01-map-reads-path-var" | "p02-map-anonymous" => {
            let mut v: Vec<String> = g
                .edges()
                .filter(|e| e.kind() == "undercuts" && e.from() == e.to())
                .map(|e| e.from().to_text())
                .collect();
            v.sort();
            v.into_iter().map(|s| vec![s]).collect()
        }
        // The date rows (D71 step 10): each dated node's `due`, through umx_core::time with no Cypher.
        "d01-date" | "d02-date-part" | "d03-date-trunc" | "d04-date-less" | "d05-date-equal" => {
            use umx_core::time::{Unit, date_part, date_span, date_text, date_trunc};
            let mut v: Vec<(String, umx_core::time::DateSpan)> = g
                .nodes()
                .filter_map(|n| Some((text_of(n), date_span(&str_field(n, "due")?)?)))
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            let unit = |u: &str| Unit::parse(u).unwrap();
            v.into_iter()
                .filter(|(_, s)| match query_id {
                    "d04-date-less" => s.relation(&date_span("2026-12-02").unwrap()).is_lt(),
                    "d05-date-equal" => s.relation(&date_span("2026-12-01").unwrap()).is_eq(),
                    _ => true,
                })
                .map(|(id, s)| match query_id {
                    "d01-date" => vec![id, date_text(s).unwrap()],
                    "d02-date-part" => ["year", "quarter", "month", "week", "day", "dayofweek"]
                        .map(|u| date_part(unit(u), s).unwrap().to_string())
                        .to_vec(),
                    "d03-date-trunc" => ["year", "quarter", "month", "week", "day"]
                        .map(|u| date_trunc(unit(u), s).unwrap())
                        .to_vec(),
                    _ => vec![id],
                })
                .collect()
        }
        // Dated nodes first by their day, then the rest (null sorts last), each by id.
        "d06-date-order" => {
            let mut v: Vec<(Option<String>, String)> = g
                .nodes()
                .map(|n| {
                    let day = str_field(n, "due")
                        .and_then(|d| umx_core::time::date_span(&d))
                        .and_then(umx_core::time::date_text);
                    (day, text_of(n))
                })
                .collect();
            v.sort_by(|a, b| (a.0.is_none(), &a.0, &a.1).cmp(&(b.0.is_none(), &b.0, &b.1)));
            v.into_iter()
                .map(|(d, id)| vec![id, d.unwrap_or_else(|| "null".into())])
                .collect()
        }
        "d07-date-null" => vec![vec!["null".into()]],
        _ => return None,
    })
}

// ---- Tests --------------------------------------------------------------------------------------------------

/// Every supported fixture query (all `conformance` ones runnable before s06): `run` over umx-graph returns
/// LadybugDB 0.22.3's rows, and so does the oracle.
#[test]
fn every_fixture_query_matches_lbug_and_the_oracle() {
    let g = design(false);
    let mut checked = 0;
    let mut failures = Vec::new();
    for q in queries() {
        if q["conformance"] != true || q.get("umx_from").is_some() {
            continue;
        }
        let qid = q["id"].as_str().unwrap();
        let want: Rows = serde_json::from_value(q["expected"].clone()).unwrap();
        match query(&g, q["cypher"].as_str().unwrap()) {
            Ok(got) if rows::rows_match(&got, &want) => {
                if let Some(twin) = q["alias_of"].as_str() {
                    let twin_rows = query(&g, twin).unwrap();
                    assert!(
                        rows::rows_match(&got, &twin_rows),
                        "{qid}: alias differs from twin"
                    );
                }
            }
            other => failures.push(format!("umx-query {qid}: {other:?}, LadybugDB {want:?}")),
        }
        match oracle(&g, qid) {
            Some(got) if rows::rows_match(&got, &want) => {}
            other => failures.push(format!("oracle {qid}: {other:?}, LadybugDB {want:?}")),
        }
        checked += 1;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(
        checked, 52,
        "52 conformance queries run before s06 (w07 waits)"
    );
}

#[test]
fn relationship_type_alias_preserves_bindings_and_nulls_without_accepting_nodes() {
    let g = design(false);
    for text in [
        "MATCH ()-[r:answers]->() RETURN type(r) AS t",
        "MATCH ()-[r:answers]->() RETURN TYPE(r) AS t",
        "MATCH ()-[r:answers]->() WITH r AS type RETURN type(type) AS t",
        "MATCH ()-[r:answers]->() WITH * RETURN type(r) AS t",
        "MATCH ()-[r:answers]->() WITH r AS rel ORDER BY type(rel) LIMIT 1 RETURN type(rel) AS t",
        "MATCH ()-[r:answers]->() RETURN r AS rel ORDER BY type(rel)",
        "MATCH (n:question) OPTIONAL MATCH (n)-[r:nope]->() RETURN type(r) AS t",
        "WITH 1 AS one RETURN type(null) AS t",
        "MATCH (n:question) WHERE EXISTS { MATCH (n)<-[r:answers]-() WHERE type(r) = 'answers' } RETURN n.id AS id",
    ] {
        let twin = text.replace("type(", "label(").replace("TYPE(", "label(");
        assert_eq!(
            query(&g, text).unwrap(),
            query(&g, &twin).unwrap(),
            "{text}"
        );
    }
    assert!(
        !query(&g, "MATCH (type:question) RETURN type.id AS id")
            .unwrap()
            .is_empty()
    );
    for (text, hint) in [
        (
            "MATCH (n) RETURN type(n)",
            "use label(n) for the type of a node",
        ),
        (
            "MATCH ()-[r:answers]->(n) WITH n AS r RETURN type(r)",
            "use label(n) for the type of a node",
        ),
        (
            "MATCH ()-[r:answers]->() UNWIND [1] AS r RETURN type(r)",
            "use type(r) with one single-hop relationship variable",
        ),
        (
            "MATCH (n) WHERE EXISTS { MATCH (n)-[r]->() } RETURN type(r)",
            "use type(r) with one single-hop relationship variable",
        ),
        (
            "MATCH ()-[r:answers]->() WITH r.id AS rid ORDER BY type(rid) LIMIT 1 RETURN rid",
            "use type(r) with one single-hop relationship variable",
        ),
        (
            "MATCH (n) RETURN n AS a ORDER BY type(a)",
            "use label(n) for the type of a node",
        ),
        (
            "MATCH (n) RETURN type('x')",
            "use type(r) with one single-hop relationship variable",
        ),
        (
            "MATCH ()-[r:blocks*1..2]->() RETURN type(r)",
            "use label(r) for the relation of an edge per hop",
        ),
    ] {
        let error = query(&g, text).unwrap_err();
        assert_eq!(error.code, "query_unknown_function", "{text}");
        assert_eq!(error.slot("hint").as_deref(), Some(hint), "{text}");
    }
}

/// Rows without ORDER BY are the same on every run and load order: handle order, never hash order.
#[test]
fn rows_are_the_same_on_every_run_and_load_order() {
    let render = |g: &Graph| -> String {
        queries()
            .iter()
            .map(|q| {
                let text = q["cypher"].as_str().unwrap();
                let text = match text.find(" ORDER BY ") {
                    Some(i) if !text[i..].contains(" SKIP ") && !text[i..].contains(" LIMIT ") => {
                        &text[..i]
                    }
                    _ => text,
                };
                let o = ontology();
                format!(
                    "{:?}",
                    run(
                        &GraphView::one("design", g, &o.symmetric_kinds()),
                        text,
                        &RunOptions::default()
                    )
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let first = render(&design(false));
    assert!(first.contains(&id("Q3")), "the instrument sees rows");
    for reverse in [false, true, false, true] {
        assert_eq!(render(&design(reverse)), first);
    }
}

/// The design fixture holds no reversed pair of a symmetric relation, so umx (which keeps one edge of such a
/// pair, F48) and LadybugDB load the same edges (the project manager's F2).
#[test]
fn the_design_fixture_has_no_reversed_symmetric_pair() {
    let (_, edges) = records(&fixture_dir());
    let stored: BTreeSet<(&str, Id, Id)> = edges
        .iter()
        .map(|e| (e.kind.as_str(), e.from, e.to))
        .collect();
    for (r, a, b) in &stored {
        assert!(
            !(ontology().is_symmetric(r) && a != b && stored.contains(&(r, *b, *a))),
            "{a:?} {r} {b:?} is stored both ways"
        );
    }
}

/// D49 over umx-graph, umx-only: a lone reversed conflicts_with edge (higher id as `from`, as an older graph may
/// hold) is found by an undirected match from either end; a directed step naming it is refused; an untyped
/// directed step that crosses it warns.
#[test]
fn a_lone_reversed_symmetric_edge() {
    let a = "01JDCYPHER000000000000000A";
    let b = "01JDCYPHER000000000000000B";
    let node_line = |id: &str, title: &str| {
        format!(
            r#"{{"id":"{id}","type":"decision","title":"{title}","status":"open","note":"","evidence":[],"status_history":[],"created":"2026-11-10T00:00:00Z","updated":"2026-11-10T00:00:00Z","created_by":"fixture"}}"#
        )
    };
    let nodes =
        decode_nodes(format!("{}\n{}\n", node_line(a, "A"), node_line(b, "B")).as_bytes()).unwrap();
    let edge = format!(
        r#"{{"id":"01JDCYPHER0000000000000E01","type":"conflicts_with","from":"{b}","to":"{a}","created":"2026-11-10T00:00:00Z","created_by":"fixture"}}"#
    );
    let edges = decode_edges(format!("{edge}\n").as_bytes()).unwrap();
    let g = Graph::from_records(nodes.records, edges.records);
    let o = ontology();
    let view = GraphView::one("design", &g, &o.symmetric_kinds());
    let opts = RunOptions::default();

    for (from, other) in [(a, b), (b, a)] {
        let rows = query(
            &g,
            &format!("MATCH (x {{id: '{from}'}})-[:conflicts_with]-(y) RETURN y.id AS id"),
        )
        .unwrap();
        assert_eq!(rows, [[other.to_string()]], "undirected from {from}");
    }
    assert_eq!(
        run(
            &view,
            "MATCH (x)-[:conflicts_with]->(y) RETURN x.id AS id",
            &opts
        ),
        Err(Problem::new("query_symmetric_direction").with(
            "relation",
            Slot::Name(ValidName::new("conflicts_with").unwrap())
        ))
    );
    let rows = run(
        &view,
        "MATCH (x)-->(y) RETURN x.id AS x_id, y.id AS y_id",
        &opts,
    )
    .unwrap();
    assert_eq!(
        rows.rows,
        [[Cell::Str(b.into()), Cell::Str(a.into())]],
        "stored direction only"
    );
    assert_eq!(
        rows.warnings,
        [Problem::new("query_symmetric_in_directed_step").with(
            "relation",
            Slot::Name(ValidName::new("conflicts_with").unwrap())
        )]
    );
    // A graph with no symmetric relation declared: the directed step runs, no warning.
    let plain = GraphView::one("design", &g, &[]);
    assert!(
        run(
            &plain,
            "MATCH (x)-[:conflicts_with]->(y) RETURN x.id AS id",
            &opts
        )
        .is_ok()
    );
    assert!(
        run(&plain, "MATCH (x)-->(y) RETURN x.id AS id", &opts)
            .unwrap()
            .warnings
            .is_empty()
    );
}

/// A whole node or edge comes back in umx's shape (cypher-expert section 1.2), and its `address` and `graph` read
/// as fields too (cypher-expert F3).
#[test]
fn whole_nodes_and_edges_have_umx_shapes() {
    let g = design(false);
    let o = ontology();
    let view = GraphView::one("design", &g, &o.symmetric_kinds());
    let text = format!(
        "MATCH (a {{id: '{}'}})-[r:undercuts]->(b) RETURN a, r, a.address AS address, a.graph AS graph, \
         r.address AS edge_address, r.graph AS edge_graph",
        id("O2")
    );
    let rows = run(&view, &text, &RunOptions::default()).unwrap();
    assert!(rows.warnings.is_empty(), "{:?}", rows.warnings);
    let [
        Cell::Node(n),
        Cell::Edge(e),
        address,
        graph,
        edge_address,
        edge_graph,
    ] = &rows.rows[0][..]
    else {
        panic!("{:?}", rows.rows);
    };
    let o2 = format!("umx:design/{}", id("O2"));
    assert_eq!(
        (n.id.to_text(), n.address.as_str(), n.graph.as_str()),
        (id("O2"), o2.as_str(), "design")
    );
    assert_eq!(
        (n.kind.as_str(), n.status.as_str()),
        ("option", "ruled_out")
    );
    assert_eq!(e.id.to_text(), "01JDCYPHER0000000000000E11");
    assert_eq!((e.kind.as_str(), e.graph.as_str()), ("undercuts", "design"));
    assert_eq!((e.from.to_text(), e.to.to_text()), (id("O2"), id("D3")));
    assert_eq!(e.address, "umx:design/01JDCYPHER0000000000000E11");
    assert_eq!(*address, Cell::Str(o2));
    assert_eq!(*graph, Cell::Str("design".into()));
    assert_eq!(*edge_address, Cell::Str(e.address.clone()));
    assert_eq!(*edge_graph, Cell::Str("design".into()));
}

/// Several graphs in one view: rows come graph by graph (in the order given), the same id in two graphs is two
/// nodes, a step stays inside its graph, and the id index looks in every graph.
#[test]
fn a_view_over_two_graphs_keeps_them_apart() {
    use umx_query::Part;
    let a = design(false);
    let b = design(false);
    let o = ontology();
    let part = |alias, graph| Part {
        alias,
        graph,
        symmetric: o.symmetric_kinds().iter().map(|s| s.to_string()).collect(),
    };
    let view = GraphView::new(vec![part("one", &a), part("two", &b)]);
    let rows = |text: &str| -> Vec<Vec<String>> {
        run(&view, text, &RunOptions::default())
            .unwrap_or_else(|p| panic!("{text}: {p:?}"))
            .rows
            .iter()
            .map(|r| r.iter().map(show).collect())
            .collect()
    };
    assert_eq!(
        rows("MATCH (n:criterion) RETURN n.graph AS g, n.id AS id"),
        [["one", &id("C1")], ["two", &id("C1")]]
    );
    assert_eq!(
        rows("MATCH (n) RETURN count(DISTINCT n) AS nodes"),
        [["22"]]
    );
    // Each graph's D1 has three blockers in its own graph: six rows, never nine or twelve.
    let text = format!(
        "MATCH (a)-[:blocks]->(d {{id: '{}'}}) RETURN a.graph AS g, d.graph AS h, count(*) AS n ORDER BY g",
        id("D1")
    );
    assert_eq!(rows(&text), [["one", "one", "3"], ["two", "two", "3"]]);
}

/// D49 per edge (reviewer R127-1): an untyped directed step warns only when the crossed edge's own graph declares
/// its relation symmetric. A typed directed step on a relation symmetric in any graph of the view is still refused.
#[test]
fn the_untyped_directed_warning_reads_the_crossed_edges_own_graph() {
    use umx_query::Part;
    let directed = design(false);
    let empty = Graph::from_records(Vec::new(), Vec::new());
    let part = |alias, graph, symmetric: &[&str]| Part {
        alias,
        graph,
        symmetric: symmetric.iter().map(|s| s.to_string()).collect(),
    };
    // conflicts_with is plain in "directed" (which holds D1's conflicts_with edges) and symmetric in "other".
    let view = GraphView::new(vec![
        part("directed", &directed, &[]),
        part("other", &empty, &["conflicts_with"]),
    ]);
    let text = format!("MATCH (a {{id: '{}'}})-->(b) RETURN b.id AS id", id("D1"));
    let rows = run(&view, &text, &RunOptions::default()).unwrap();
    assert!(!rows.rows.is_empty());
    assert!(rows.warnings.is_empty(), "{:?}", rows.warnings);
    let p = run(
        &view,
        "MATCH (a)-[:conflicts_with]->(b) RETURN a.id AS id",
        &RunOptions::default(),
    )
    .unwrap_err();
    assert_eq!(p.code, umx_query::check::SYMMETRIC_DIRECTION);
    // Symmetric where the edge is: it warns.
    let view = GraphView::new(vec![part("sym", &directed, &["conflicts_with"])]);
    let rows = run(&view, &text, &RunOptions::default()).unwrap();
    assert_eq!(
        rows.warnings.iter().map(|w| w.code).collect::<Vec<_>>(),
        [umx_query::check::SYMMETRIC_IN_DIRECTED_STEP]
    );
}

/// Parameters (reviewer R127-2, cypher-expert F1): an unaliased column reading `$x` is named `$x`, never after
/// its value, so equal values are not equal columns; a list parameter is a list.
#[test]
fn parameter_columns_keep_their_names_and_lists_bind() {
    use umx_query::Param;
    let g = design(false);
    let o = ontology();
    let view = GraphView::one("design", &g, &o.symmetric_kinds());
    let options = RunOptions {
        params: BTreeMap::from([
            ("left".to_string(), Param::Int(3)),
            ("right".to_string(), Param::Int(3)),
            (
                "ids".to_string(),
                Param::List(vec![Param::Str(id("Q1")), Param::Str(id("Q3"))]),
            ),
        ]),
        ..Default::default()
    };
    let rows = run(&view, "UNWIND [1] AS x RETURN $left, $right", &options);
    assert_eq!(rows.unwrap().columns, ["$left", "$right"]);
    let rows = run(
        &view,
        "MATCH (n) WHERE n.id IN $ids RETURN n.id AS id ORDER BY id",
        &options,
    )
    .unwrap();
    assert_eq!(rows.rows.len(), 2);
}

/// The symmetric set comes from the caller (D49 R3): `Ontology::symmetric_kinds()` (F48). Until merged
/// the callers held a local list; this test, which made the switch, now keeps such a list from coming back.
#[test]
fn the_local_symmetric_list_goes_when_the_ontology_has_one() {
    let core = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../umx-core/src/ontology.rs"),
    )
    .unwrap();
    let marker = ["const LOCAL_", "SYMMETRIC"].concat();
    for file in ["tests/design.rs", "../umx-bench/src/cypher.rs"] {
        let text = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(file)).unwrap();
        assert!(text.contains("GraphView::one"), "{file} is still a caller");
        assert!(
            !(core.contains("pub fn symmetric_kinds") && text.contains(&marker)),
            "umx-core has Ontology::symmetric_kinds(): pass Ontology::starter().symmetric_kinds() in {file} and \
             delete the local list"
        );
    }
}

/// F241 (plan 216), umx-only (LadybugDB has no such rule): a column the query returns named `secret_suspected`, the
/// key umx's secret flags take on each row, is refused before any graph work, matched exactly on the final columns:
/// an alias, a bare variable from WITH or UNWIND, and `RETURN *`. Other names, and the name used only inside the
/// query, run; so does another case (`Secret_Suspected`).
#[test]
fn a_column_named_like_the_secret_flags_key_is_refused() {
    let g = design(false);
    for text in [
        "MATCH (n) RETURN n.id AS secret_suspected",
        "WITH 1 AS secret_suspected RETURN secret_suspected",
        "UNWIND [1] AS secret_suspected RETURN secret_suspected",
        "MATCH (secret_suspected) RETURN *",
        "MATCH (n) WITH n AS secret_suspected RETURN *",
    ] {
        let refused = query(&g, text).expect_err(text);
        assert_eq!(refused.code, "query_column_reserved", "{text}");
    }
    for text in [
        "MATCH (n) RETURN n.id AS secret_suspected_",
        "MATCH (n) RETURN n.id AS Secret_Suspected",
        "MATCH (secret_suspected) RETURN secret_suspected.id AS id",
        "MATCH (n) WITH n AS secret_suspected RETURN secret_suspected.id AS id",
        "MATCH (secret_suspected) WITH secret_suspected.id AS id RETURN *",
    ] {
        assert!(query(&g, text).is_ok(), "{text}: {:?}", query(&g, text));
    }
}
