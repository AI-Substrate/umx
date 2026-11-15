//! The design fixture (cypher-expert's design input for workstream 005, section 1.1) through the vendored
//! executor. fixtures/design holds 11 nodes, 19 edges and queries.json; every query marked `conformance`
//! has expected rows measured on LadybugDB 0.22.3 (tools/lbug-conformance runs the same file there).
//!
//! This test is the fork's side of that check, over a plain provider (umx-query's tests run the same file
//! over umx-graph). s01 recorded upstream 0.8.1 getting 13 of 35 wrong; the s02 fixes moved every one out of
//! `KNOWN_WRONG`, and the test fails if one comes back. Also here: the fork's own behaviour (floats, id
//! pushdown, D49 symmetric relations, first-seen order).
//!
//! The loader gives nodes handles in id order, as umx-graph does, so a load order cannot reach the
//! executor; the determinism test proves the executor adds no order of its own.

mod rows;

use std::cell::Cell;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value as J;
use umx_cypher::format::format as render_rows;
use umx_cypher::{
    CypherError, CypherValue, Float, GraphProvider, OutputFormat, Warning, execute, parse,
};

/// Conformance queries whose rows or outcome still differ from LadybugDB 0.22.3 on this fork, with the reason
/// (design input section 2). Sorted by id. s01 recorded 13 on upstream 0.8.1; each fix removes its rows.
const KNOWN_WRONG: &[(&str, &str)] = &[];

struct Node {
    id: String,
    fields: serde_json::Map<String, J>,
}

struct Edge {
    id: String,
    relation: String,
    from: usize,
    to: usize,
    fields: serde_json::Map<String, J>,
}

/// The design graph, handles in id order. `index` turns on an `id` index (`lookup`); `scans` counts the
/// whole-graph scans the executor asked for.
struct Design {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    index: bool,
    scans: Cell<usize>,
    /// conflicts_with is symmetric (D49) when set.
    symmetric: bool,
}

fn fixture_lines(name: &str) -> Vec<J> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/design")
        .join(name);
    fs::read_to_string(path)
        .expect("fixture file")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("fixture json"))
        .collect()
}

fn text(v: &J, key: &str) -> String {
    v[key].as_str().expect(key).to_string()
}

impl Design {
    /// Loads the fixture with its lines in the given order.
    fn load(order: fn(Vec<J>) -> Vec<J>) -> Design {
        let mut nodes: Vec<Node> = order(fixture_lines("nodes.jsonl"))
            .into_iter()
            .map(|n| {
                let mut fields = n.as_object().expect("node object").clone();
                // Derived and read-only in umx (workshop 007 rule 4): the distinct evidence kinds, sorted.
                let kinds: BTreeSet<String> = n["evidence"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e["kind"].as_str().map(String::from))
                    .collect();
                fields.insert("evidence_kinds".into(), J::from(Vec::from_iter(kinds)));
                Node {
                    id: text(&n, "id"),
                    fields,
                }
            })
            .collect();
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        let handle = |id: &str| nodes.iter().position(|n| n.id == id).expect("edge end");
        let mut edges: Vec<Edge> = order(fixture_lines("edges.jsonl"))
            .into_iter()
            .map(|e| Edge {
                id: text(&e, "id"),
                relation: text(&e, "type"),
                from: handle(&text(&e, "from")),
                to: handle(&text(&e, "to")),
                fields: e.as_object().expect("edge object").clone(),
            })
            .collect();
        edges.sort_by(|a, b| a.id.cmp(&b.id));
        Design {
            nodes,
            edges,
            index: false,
            scans: Cell::new(0),
            symmetric: false,
        }
    }
}

fn value(v: &J) -> CypherValue {
    match v {
        J::String(s) => CypherValue::Str(s.clone()),
        J::Bool(b) => CypherValue::Bool(*b),
        J::Array(xs) => CypherValue::List(xs.iter().map(value).collect()),
        J::Number(n) => n.as_i64().map_or_else(
            || CypherValue::Float(Float(n.as_f64().unwrap_or(f64::NAN))),
            CypherValue::Int,
        ),
        J::Null | J::Object(_) => CypherValue::Null,
    }
}

impl GraphProvider for Design {
    type NodeId = usize;
    type EdgeId = usize;

    fn scan(&self, labels: &[String]) -> Vec<usize> {
        self.scans.set(self.scans.get() + 1);
        (0..self.nodes.len())
            .filter(|&h| labels.is_empty() || labels.iter().any(|l| self.matches_label(h, l)))
            .collect()
    }

    fn matches_label(&self, node: usize, label: &str) -> bool {
        self.nodes[node].fields["type"] == label
    }

    fn relationship_types(&self) -> Vec<String> {
        let set: BTreeSet<&str> = self.edges.iter().map(|e| e.relation.as_str()).collect();
        set.into_iter().map(String::from).collect()
    }

    fn expand(&self, node: usize, rel_type: &str) -> Vec<(usize, usize)> {
        self.edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.from == node && e.relation == rel_type)
            .map(|(i, e)| (i, e.to))
            .collect()
    }

    fn expand_in(&self, node: usize, rel_type: &str) -> Vec<(usize, usize)> {
        self.edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.to == node && e.relation == rel_type)
            .map(|(i, e)| (i, e.from))
            .collect()
    }

    fn edge_property(&self, edge: usize, prop: &str) -> CypherValue {
        self.edges[edge]
            .fields
            .get(prop)
            .map_or(CypherValue::Null, value)
    }

    fn edge_label(&self, edge: usize) -> String {
        self.edges[edge].relation.clone()
    }

    fn edge_id(&self, edge: usize) -> String {
        self.edges[edge].id.clone()
    }

    fn is_symmetric(&self, rel_type: &str) -> bool {
        self.symmetric && rel_type == "conflicts_with"
    }

    fn indexed(&self, prop: &str) -> bool {
        self.index && prop == "id"
    }

    fn lookup(&self, prop: &str, value: &CypherValue) -> Option<Vec<usize>> {
        if !self.indexed(prop) {
            return None;
        }
        Some(match value {
            CypherValue::Str(id) => self
                .nodes
                .binary_search_by(|n| n.id.as_str().cmp(id))
                .ok()
                .into_iter()
                .collect(),
            _ => Vec::new(),
        })
    }

    fn property(&self, node: usize, prop: &str) -> CypherValue {
        self.nodes[node]
            .fields
            .get(prop)
            .map_or(CypherValue::Null, value)
    }

    fn node_id(&self, node: usize) -> String {
        self.nodes[node].id.clone()
    }

    fn label(&self, node: usize) -> String {
        text(&J::Object(self.nodes[node].fields.clone()), "type")
    }

    fn name(&self, node: usize) -> String {
        text(&J::Object(self.nodes[node].fields.clone()), "title")
    }
}

/// A cell as LadybugDB's checker prints it (tools/lbug-conformance `show`).
fn show(v: &CypherValue) -> String {
    match v {
        CypherValue::Null => "null".into(),
        CypherValue::Bool(b) => b.to_string(),
        CypherValue::Int(i) => i.to_string(),
        CypherValue::Float(f) => format!("{:?}", f.0),
        CypherValue::Str(s) => s.clone(),
        CypherValue::List(xs) => format!("[{}]", xs.iter().map(show).collect::<Vec<_>>().join(",")),
        other => format!("{other:?}"),
    }
}

/// Runs one query: Ok(rows as strings) or Err(the executor's message).
fn run(graph: &Design, query: &str) -> Result<Vec<Vec<String>>, String> {
    let parsed = parse(query).map_err(|e| e.to_string())?;
    let result = execute(graph, &parsed).map_err(|e| e.to_string())?;
    Ok(result
        .rows
        .iter()
        .map(|r| r.iter().map(show).collect())
        .collect())
}

fn queries() -> Vec<J> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/design/queries.json");
    let all: J =
        serde_json::from_str(&fs::read_to_string(path).expect("queries.json")).expect("json");
    all.as_array().expect("query list").clone()
}

#[test]
fn the_fixture_is_the_design_graph() {
    let g = Design::load(|v| v);
    assert_eq!((g.nodes.len(), g.edges.len()), (11, 19));
    let q = queries();
    assert_eq!(q.len(), 73);
    assert_eq!(q.iter().filter(|q| q["conformance"] == true).count(), 53);
    for q in q.iter().filter(|q| q["conformance"] == true) {
        assert!(q["expected"].is_array(), "{} has no expected rows", q["id"]);
    }
}

/// The baseline: the fork answers every conformance query except `KNOWN_WRONG` as LadybugDB does. A query that
/// calls a provider's own function (`provider_functions`: the date functions, VENDORED row 28) is umx-query's to
/// answer; this test graph has none.
#[test]
fn the_fork_baseline_on_the_design_fixture() {
    let graph = Design::load(|v| v);
    let mut wrong = Vec::new();
    let mut report = Vec::new();
    for q in queries() {
        if q["conformance"] != true
            || q.get("umx_from").is_some()
            || q["provider_functions"] == true
        {
            continue;
        }
        let id = q["id"].as_str().expect("id");
        let expected: Vec<Vec<String>> =
            serde_json::from_value(q["expected"].clone()).expect("rows");
        let outcome = run(&graph, q["cypher"].as_str().expect("cypher")).map(|mut rows| {
            if q["ordered"] != true {
                rows.sort();
            }
            rows
        });
        let line = match &outcome {
            Ok(rows) if rows::rows_match(rows, &expected) => format!("ok     {id}"),
            Ok(rows) => format!("wrong  {id}: got {rows:?}, LadybugDB {expected:?}"),
            Err(e) => format!("error  {id}: {e}"),
        };
        if !line.starts_with("ok") {
            wrong.push(id.to_string());
        }
        report.push(line);
    }
    println!("{}", report.join("\n"));
    wrong.sort();
    let known: Vec<String> = KNOWN_WRONG
        .iter()
        .map(|(id, _)| (*id).to_string())
        .collect();
    assert_eq!(
        wrong, known,
        "the baseline moved: update KNOWN_WRONG (and VENDORED.md)"
    );
}

/// Drops every `ORDER BY ...` up to the next SKIP or LIMIT, or the end.
fn without_order_by(query: &str) -> String {
    let mut out = query.to_string();
    while let Some(start) = out.find(" ORDER BY ") {
        let rest = &out[start + 1..];
        let end = [" SKIP ", " LIMIT ", " RETURN "]
            .iter()
            .filter_map(|k| rest.find(k))
            .min()
            .map_or(out.len(), |e| start + 1 + e);
        out.replace_range(start..end, "");
    }
    out
}

/// Queries with no ORDER BY give byte-identical output across runs and load orders: no row order, DISTINCT,
/// grouping or collect may depend on hash order.
#[test]
fn rows_without_order_by_are_the_same_on_every_run_and_load_order() {
    let mut texts: Vec<String> = queries()
        .iter()
        .map(|q| without_order_by(q["cypher"].as_str().expect("cypher")))
        .collect();
    texts.extend(
        [
            "MATCH (n) RETURN n.id AS id",
            "MATCH (n) RETURN DISTINCT n.type AS t",
            "MATCH (n)-->(m) RETURN n.id AS a, m.id AS b",
            "MATCH (n)--(m) RETURN DISTINCT m.id AS id",
            "MATCH (a)-[:blocks|depends_on*1..3]-(b) RETURN DISTINCT b.id AS id",
            "MATCH (n) RETURN n.type AS t, count(*) AS n, collect(n.id) AS ids",
            "MATCH (n) WHERE n.status IN ['open', 'agreed', 'proposed'] RETURN n.id AS id, n.status AS s",
            "MATCH (n) UNWIND n.evidence_kinds AS k RETURN DISTINCT k",
            "MATCH (n) RETURN n",
        ]
        .map(String::from),
    );
    let render = |graph: &Design| -> String {
        texts
            .iter()
            .map(|t| match parse(t).and_then(|q| execute(graph, &q)) {
                Ok(r) => render_rows(&r, OutputFormat::Json),
                Err(e) => e.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let orders: [fn(Vec<J>) -> Vec<J>; 3] = [
        |v| v,
        |mut v| {
            v.reverse();
            v
        },
        |mut v| {
            v.rotate_left(5);
            v
        },
    ];
    let first = render(&Design::load(orders[0]));
    assert!(first.contains(&id("Q3")), "the instrument sees rows");
    for order in orders {
        for _ in 0..2 {
            assert_eq!(render(&Design::load(order)), first);
        }
    }
}

/// A design node's id from its short name (fixtures/design/README.md): `id("D1")`.
fn id(short: &str) -> String {
    let short = short.replacen('O', "N", 1);
    format!("01JDCYPHER{short:0>16}")
}

/// One-row queries over the design graph, as cells.
fn cells(query: &str) -> Vec<String> {
    let graph = Design::load(|v| v);
    let rows = run(&graph, query).unwrap_or_else(|e| panic!("{query}: {e}"));
    assert_eq!(rows.len(), 1, "{query}");
    rows[0].clone()
}

/// R-Q6: float literals, numeric comparison across integer and float, and float aggregates.
#[test]
fn floats_are_values_and_compare_as_numbers() {
    assert_eq!(
        cells("UNWIND [1.5, -2.5, 1e3, 2.5E-1] AS x RETURN collect(x) AS xs"),
        ["[1.5,-2.5,1000.0,0.25]"]
    );
    // `1..2` is still a range of two integers, not the float `1.`: Q1 blocks P1, D1 and D2, and P1 blocks D1.
    assert_eq!(
        cells(&format!(
            "MATCH (a {{id: '{}'}})-[:blocks*1..2]->(t) RETURN count(*) AS n",
            id("Q1")
        )),
        ["4"]
    );
    assert_eq!(
        cells("UNWIND [3] AS x RETURN x = 3.0 AS eq, x < 3.5 AS lt, 3.0 IN [3] AS within"),
        ["true", "true", "true"]
    );
    // avg is a float even over integers (LadybugDB's DOUBLE); sum stays an integer over integers.
    assert_eq!(
        cells("UNWIND [1, 2] AS x RETURN avg(x) AS a, sum(x) AS s"),
        ["1.5", "3"]
    );
}

/// Id pushdown: the index never changes a row, and an id in a node's map or in WHERE replaces the scan.
#[test]
fn an_id_index_replaces_the_scan_and_changes_no_row() {
    let plain = Design::load(|v| v);
    let mut indexed = Design::load(|v| v);
    indexed.index = true;
    for q in queries() {
        let text = q["cypher"].as_str().expect("cypher");
        assert_eq!(run(&indexed, text), run(&plain, text), "{text}");
    }
    for text in [
        &format!("MATCH (n {{id: '{}'}}) RETURN n.title AS t", id("D1")),
        &format!("MATCH (n) WHERE n.id = '{}' RETURN n.title AS t", id("D1")),
        &format!(
            "MATCH (a)-[:blocks]->(b {{id: '{}'}}) RETURN a.id AS id",
            id("D1")
        ),
    ] {
        indexed.scans.set(0);
        assert!(!run(&indexed, text).expect("runs").is_empty(), "{text}");
        assert_eq!(indexed.scans.get(), 0, "{text} scanned");
    }
}

/// `$name` parameters (ws005 s04): bound to literals before the query runs, so a parameter answers as the same
/// literal written in the text would, including the id index (no scan); one left unbound is named.
#[test]
fn parameters_bind_as_literals_and_use_the_index() {
    use std::collections::BTreeMap;
    use umx_cypher::ast::Literal;
    let mut indexed = Design::load(|v| v);
    indexed.index = true;
    use umx_cypher::ast::Expr;
    let params = BTreeMap::from([
        ("id".to_string(), Expr::Literal(Literal::Str(id("D1")))),
        ("least".to_string(), Expr::Literal(Literal::Int(3))),
    ]);
    for (with_params, literal) in [
        (
            "MATCH (n {id: $id}) RETURN n.title AS t".to_string(),
            format!("MATCH (n {{id: '{}'}}) RETURN n.title AS t", id("D1")),
        ),
        (
            "MATCH (a)-[:blocks]->(b) WHERE b.id = $id RETURN a.id AS id ORDER BY id".to_string(),
            format!(
                "MATCH (a)-[:blocks]->(b) WHERE b.id = '{}' RETURN a.id AS id ORDER BY id",
                id("D1")
            ),
        ),
        (
            "MATCH (b)-[:blocks]->(d) WITH d, count(*) AS n WHERE n >= $least RETURN d.id AS id"
                .to_string(),
            "MATCH (b)-[:blocks]->(d) WITH d, count(*) AS n WHERE n >= 3 RETURN d.id AS id"
                .to_string(),
        ),
    ] {
        let mut query = parse(&with_params).expect("parses");
        query.bind_params(&params).expect("bound");
        assert_eq!(query, parse(&literal).unwrap(), "{with_params}");
        indexed.scans.set(0);
        let rows = execute(&indexed, &query).expect("runs");
        assert!(!rows.rows.is_empty(), "{with_params}");
    }
    let mut query = parse("MATCH (n {id: $id}) RETURN n.title AS t").unwrap();
    query.bind_params(&params).unwrap();
    indexed.scans.set(0);
    execute(&indexed, &query).unwrap();
    assert_eq!(indexed.scans.get(), 0, "a bound $id scanned");

    let mut query = parse("MATCH (n) WHERE n.id = $nope RETURN n.id AS id").unwrap();
    assert_eq!(query.bind_params(&params), Err("nope".to_string()));
    assert!(parse("RETURN $ AS x").is_err());
}

/// D49: a directed step naming a symmetric relation is refused (R1); an untyped directed step that crosses one
/// runs and warns (R2); an undirected step is plain.
#[test]
fn symmetric_relations_have_no_direction() {
    let mut graph = Design::load(|v| v);
    graph.symmetric = true;
    let exec = |q: &str| execute(&graph, &parse(q).expect("parses"));
    for q in [
        "MATCH (a:decision)-[:conflicts_with]->(b) RETURN a.id AS a_id",
        "MATCH (a)<-[:conflicts_with]-(b) RETURN a.id AS a_id",
        "MATCH (a)-[:blocks|conflicts_with*1..3]->(b) RETURN b.id AS id",
        "MATCH (d:decision) WHERE EXISTS { MATCH (d)-[:conflicts_with]->() } RETURN d.id AS id",
    ] {
        assert_eq!(
            exec(q),
            Err(CypherError::SymmetricDirection {
                relation: "conflicts_with".into()
            }),
            "{q}"
        );
    }
    let undirected = exec(&format!(
        "MATCH (a {{id: '{}'}})-[:conflicts_with]-(b) RETURN b.id AS id",
        id("D2")
    ))
    .expect("runs");
    assert!(undirected.warnings.is_empty());
    let untyped = exec(&format!(
        "MATCH (a {{id: '{}'}})-->(b) RETURN b.id AS id",
        id("D1")
    ))
    .expect("runs");
    assert_eq!(
        untyped.warnings,
        [Warning::SymmetricInDirectedStep {
            relations: vec!["conflicts_with".into()]
        }]
    );
    let none = exec(&format!(
        "MATCH (a {{id: '{}'}})-->(b) RETURN b.id AS id",
        id("Q1")
    ))
    .expect("runs");
    assert!(none.warnings.is_empty(), "Q1 crosses no symmetric edge");
    graph.symmetric = false;
    assert!(
        execute(
            &graph,
            &parse("MATCH (a)-[:conflicts_with]->(b) RETURN a.id AS a").unwrap()
        )
        .is_ok()
    );
}

/// DISTINCT, grouping and collect keep first-seen order (handle order here), never value order: the BTree
/// sets behind them only answer membership. First-seen order differs from value order on these rows.
#[test]
fn distinct_and_collect_keep_first_seen_order() {
    let graph = Design::load(|v| v);
    let col = |q: &str| -> Vec<String> {
        run(&graph, q)
            .expect("runs")
            .into_iter()
            .map(|r| r[0].clone())
            .collect()
    };
    assert_eq!(
        col("MATCH (n) RETURN DISTINCT n.status AS s"),
        [
            "open",
            "agreed",
            "contested",
            "proposed",
            "ruled_out",
            "answered"
        ]
    );
    assert_eq!(
        col("MATCH (n) RETURN n.status AS s, count(*) AS c"),
        [
            "open",
            "agreed",
            "contested",
            "proposed",
            "ruled_out",
            "answered"
        ]
    );
    assert_eq!(
        cells("MATCH (n:decision) RETURN collect(n.status) AS s"),
        ["[agreed,contested,agreed]"]
    );
    assert_eq!(
        cells("MATCH (n:question) RETURN collect(DISTINCT n.status) AS s"),
        ["[open,answered,proposed]"]
    );
}

/// B1: an integral float and its integer are one value in a hoisted IN list, DISTINCT, grouping and
/// collect/count DISTINCT, as LadybugDB answers.
#[test]
fn an_integral_float_and_its_integer_are_one_value() {
    assert_eq!(
        cells(
            "UNWIND [3] AS x WITH collect(x) AS xs MATCH (n:criterion) WHERE 3.0 IN xs RETURN n.id AS id"
        ),
        [id("C1")]
    );
    assert_eq!(
        cells(
            "UNWIND [3.0] AS x WITH collect(x) AS xs MATCH (n:criterion) WHERE 3 IN xs RETURN n.id AS id"
        ),
        [id("C1")]
    );
    assert_eq!(cells("UNWIND [3, 3.0] AS x RETURN DISTINCT x AS x"), ["3"]);
    assert_eq!(
        cells("UNWIND [3, 3.0] AS x RETURN x AS x, count(*) AS c"),
        ["3", "2"]
    );
    assert_eq!(
        cells(
            "UNWIND [3, 3.0, [1, 1.0]] AS x RETURN collect(DISTINCT x) AS xs, count(DISTINCT x) AS n"
        ),
        ["[3,[1,1.0]]", "2"]
    );
    // Not integral: two values.
    assert_eq!(
        cells("UNWIND [3, 3.5] AS x RETURN count(DISTINCT x) AS n"),
        ["2"]
    );
    // The IN hoist on (one input row) and off (two input rows) answers the same (reviewer R105-1).
    let graph = Design::load(|v| v);
    for (list, probe, found) in [
        ("3", "3.0", 1),
        ("3.0", "3", 1),
        ("3", "3", 1),
        ("3.5", "3", 0),
    ] {
        let on = format!(
            "UNWIND [{list}] AS x WITH collect(x) AS xs MATCH (n:criterion) WHERE {probe} IN xs RETURN count(*) AS c"
        );
        let off = format!(
            "UNWIND [{list}] AS x WITH collect(x) AS xs UNWIND [1, 2] AS k MATCH (n:criterion) WHERE {probe} IN xs \
             RETURN count(*) AS c"
        );
        assert_eq!(
            run(&graph, &on).expect("runs"),
            [[found.to_string()]],
            "{on}"
        );
        assert_eq!(
            run(&graph, &off).expect("runs"),
            [[(2 * found).to_string()]],
            "{off}"
        );
    }
}

/// B2: a map entry that reads a variable bound in the same pattern is checked on the finished path, so it
/// answers as the WHERE form does; one bound by an earlier clause or UNWIND still uses the index.
#[test]
fn a_map_entry_reading_a_path_variable_is_a_filter() {
    let mut graph = Design::load(|v| v);
    graph.index = true;
    for q in [
        "MATCH (a)-[:undercuts]->(b {id: a.id}) RETURN a.id AS id",
        "MATCH (a)-[:undercuts]->({id: a.id}) RETURN a.id AS id",
        "MATCH (b {id: a.id})<-[:undercuts]-(a) RETURN a.id AS id",
        "MATCH (a)-[:undercuts]->(b) WHERE b.id = a.id RETURN a.id AS id",
    ] {
        assert_eq!(run(&graph, q).expect("runs"), [[id("C1")]], "{q}");
    }
    // The hidden variable of an anonymous node does not leak into RETURN *.
    let star = execute(
        &graph,
        &parse("MATCH (a)-[:undercuts]->({id: a.id}) RETURN *").unwrap(),
    )
    .unwrap();
    assert_eq!(star.columns, ["a"]);
    for q in [
        format!(
            "UNWIND ['{}'] AS x MATCH (n {{id: x}}) RETURN n.title AS t",
            id("D1")
        ),
        format!(
            "MATCH (c {{id: '{}'}}) MATCH (x {{id: c.id}}) RETURN x.id AS id",
            id("C1")
        ),
    ] {
        graph.scans.set(0);
        assert_eq!(run(&graph, &q).expect("runs").len(), 1, "{q}");
        assert_eq!(graph.scans.get(), 0, "{q} scanned");
    }
}

/// The daemon serves each connection on a std::thread::spawn thread: 2 MiB of stack.
const DAEMON_STACK: usize = 2 << 20;

/// Each recursive form of the grammar, nested `k` levels, as a whole query.
fn nested(form: &str, k: usize) -> String {
    let r = |s: &str| s.repeat(k);
    let expr = match form {
        "parens" => format!("{}true{}", r("("), r(")")),
        // An odd number of NOTs flips the literal; pick it so the WHERE holds.
        "not" => format!(
            "{}{}",
            r("NOT "),
            if k.is_multiple_of(2) { "true" } else { "false" }
        ),
        "and" => format!("{}true", r("true AND ")),
        "or" => format!("{}true", r("false OR ")),
        "list" => format!("{}1{} IS NOT NULL", r("["), r("]")),
        "case" => format!("{}true{}", r("CASE WHEN true THEN "), r(" END")),
        "exists" => format!("{}true{}", r("EXISTS { MATCH (m) WHERE "), r(" }")),
        "function" => format!("{}'a'{} = 'a'", r("toLower("), r(")")),
        "map" => format!("{}1{} IS NOT NULL", r("n {k: "), r("}")),
        _ => unreachable!(),
    };
    format!("MATCH (n:criterion) WHERE {expr} RETURN n.id AS id")
}

const FORMS: [&str; 9] = [
    "parens", "not", "and", "or", "list", "case", "exists", "function", "map",
];

/// Security's F1: past MAX_DEPTH every recursive form is a syntax error ("nesting too deep") before
/// anything recurses deeply; at the deepest accepted depth, parsing, running and dropping the tree fit the
/// daemon's 2 MiB stack. 100,000 levels used to abort the process with a stack overflow.
#[test]
fn nesting_is_capped_and_the_deepest_query_fits_the_daemon_stack() {
    std::thread::Builder::new()
        .stack_size(DAEMON_STACK)
        .spawn(|| {
            let graph = Design::load(|v| v);
            for form in FORMS {
                match parse(&nested(form, 100_000)) {
                    Err(CypherError::Syntax { message, .. }) => {
                        assert_eq!(message, umx_cypher::NESTING_TOO_DEEP, "{form}")
                    }
                    other => panic!("{form}: {other:?}"),
                }
                let deepest = (1..=umx_cypher::MAX_DEPTH)
                    .take_while(|&k| parse(&nested(form, k)).is_ok())
                    .last()
                    .unwrap_or_else(|| panic!("{form}: even depth 1 fails"));
                assert!(
                    deepest * 2 >= umx_cypher::MAX_DEPTH / 2,
                    "{form}: only {deepest} levels"
                );
                let query = parse(&nested(form, deepest)).expect("parses");
                let rows = execute(&graph, &query).unwrap_or_else(|e| panic!("{form}: {e}"));
                assert_eq!(rows.rows.len(), 1, "{form} at depth {deepest}");
                drop(query);
            }
        })
        .expect("thread")
        .join()
        .expect("no stack overflow");
}

/// Reviewer R115-1: a parenthesised chain wrapped in a longer chain, repeated, used to build an AST far
/// deeper than MAX_DEPTH (the chain loops forgot the left operand's height), which aborted a 2 MiB stack.
/// The bound is now on the built tree: the reviewer's query is refused, and every mixed shape that parses
/// also runs and drops on the daemon's stack.
#[test]
fn chains_around_parenthesised_chains_are_bounded_by_tree_height() {
    let mixed = |levels: usize, chain: usize, op: &str| {
        let mut expr = "true".to_string();
        for level in (0..levels).rev() {
            expr = format!(
                "({expr}{})",
                format!(" {op} false").repeat(chain.saturating_sub(level))
            );
        }
        format!("MATCH (n) WHERE NOT ({expr}) RETURN n.id AS id")
    };
    std::thread::Builder::new()
        .stack_size(DAEMON_STACK)
        .spawn(move || {
            match parse(&mixed(63, 62, "OR")) {
                Err(CypherError::Syntax { message, .. }) => {
                    assert_eq!(message, umx_cypher::NESTING_TOO_DEEP)
                }
                other => panic!("the reviewer's query: {other:?}"),
            }
            let graph = Design::load(|v| v);
            let mut ran = 0;
            for op in ["OR", "AND"] {
                for levels in [2, 8, 16, 32, 63] {
                    for chain in [4, 16, 40, 62] {
                        let Ok(query) = parse(&mixed(levels, chain, op)) else {
                            continue;
                        };
                        let rows = execute(&graph, &query).expect("runs");
                        // The OR shape is true and the AND shape false, so NOT keeps none or all.
                        let want = if op == "OR" { 0 } else { 11 };
                        assert_eq!(rows.rows.len(), want, "{op} {levels} {chain}");
                        ran += 1;
                    }
                }
            }
            assert!(ran > 4, "only {ran} shapes parsed");
        })
        .expect("thread")
        .join()
        .expect("no stack overflow");
}

/// Security's F2: the work budget is charged inside the walk and expand loops, so an exploding query
/// stops part way with TooLarge (no rows), well inside the budget's time, instead of building every target.
#[test]
fn an_exploding_walk_or_product_stops_inside_the_budget() {
    use std::time::Instant;
    use umx_cypher::{Limit, Limits, execute_limited};
    let graph = Design::load(|v| v);
    let run_with = |q: &str, limits: &Limits| execute_limited(&graph, &parse(q).unwrap(), limits);

    // Untyped *1..10 both ways on 19 edges: about 10^9 walks. It never finished before the budget.
    let started = Instant::now();
    let walk = run_with(
        "MATCH (a)-[*1..10]-(b) RETURN count(*) AS n",
        &Limits::default(),
    );
    assert!(
        matches!(walk, Err(CypherError::TooLarge { used, .. }) if used <= umx_cypher::DEFAULT_MAX_WORK + 64),
        "{walk:?}"
    );
    assert!(
        started.elapsed().as_secs() < 30,
        "stopped in {:?}",
        started.elapsed()
    );

    // Seven nodes, no relation: 11^7 = 19M rows; the row cap stops it at 1M held.
    let product = run_with(
        "MATCH (a), (b), (c), (d), (e), (f), (g) RETURN count(*) AS n",
        &Limits::default(),
    );
    assert!(
        matches!(
            product,
            Err(CypherError::TooLarge {
                cause: Limit::Rows,
                ..
            })
        ),
        "{product:?}"
    );

    // A small budget stops mid-walk, just past the limit (steps are charged a node's list at a time).
    let small = Limits {
        max_work: 1_000,
        ..Limits::default()
    };
    let stopped = run_with("MATCH (a)-[*1..10]-(b) RETURN count(*) AS n", &small);
    assert!(
        matches!(stopped, Err(CypherError::TooLarge { cause: Limit::Work, used }) if (1_001..1_020).contains(&used)),
        "{stopped:?}"
    );
    // The same query under the cap runs and reports its work.
    let fine = run_with(
        "MATCH (a)-[*1..2]-(b) RETURN count(*) AS n",
        &Limits::default(),
    )
    .unwrap();
    assert!(fine.work > 0 && fine.work < 10_000, "{}", fine.work);

    // The cancel hook (a wall-clock backstop the service builds) stops the query with Cancelled.
    let asked = Cell::new(0);
    let cancel = || {
        asked.set(asked.get() + 1);
        true
    };
    let hooked = Limits {
        cancel: Some(&cancel),
        ..Limits::default()
    };
    assert!(matches!(
        run_with("MATCH (a)-[*1..10]-(b) RETURN count(*) AS n", &hooked),
        Err(CypherError::Cancelled { used }) if used >= umx_cypher::CANCEL_EVERY
    ));
    assert_eq!(asked.get(), 1);
}
