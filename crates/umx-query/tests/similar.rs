//! similar_to in Cypher (workstream 005 s05; workshop 008 section 9; cypher-expert design input section 3): a step
//! that names similar_to reads the graph's stored lists, injected here through umx-graph's loader, so no embedder
//! is needed. Kept out of the LadybugDB conformance fixture: LadybugDB has no similar_to.
//!
//! Two origins for the rows: a plain-Rust oracle over `similar_out`, and the lists as written below by hand.

use std::fs;
use std::path::Path;

use umx_core::codec::{decode_edges, decode_nodes};
use umx_core::vectors::TextHash;
use umx_core::{Edge, Id, Ontology};
use umx_graph::Graph;
use umx_query::{Cell, GraphView, Part, RunOptions, check, run};

const SPACE: &str = "fixture-model@4";

/// The injected lists, best first: (node, [(other, score in ten-thousandths)]).
const LISTS: [(&str, &[(&str, i32)]); 6] = [
    ("Q1", &[("Q2", 9100), ("Q3", 8000), ("D1", 7000)]),
    ("Q2", &[("Q1", 9100), ("P1", 8500)]),
    ("Q3", &[("Q1", 8000)]),
    ("D1", &[("D2", 8200), ("D3", 6100)]),
    ("D2", &[("D1", 8200)]),
    ("P1", &[("Q2", 8500)]),
];

/// A design node's id from its short name (umx-cypher/fixtures/design/README.md).
fn id(short: &str) -> String {
    format!("01JDCYPHER{:0>16}", short.replacen('O', "N", 1))
}

fn ix(short: &str) -> Id {
    id(short).parse().unwrap()
}

/// The design graph plus one stored edge record named similar_to (an older graph's, C1 to N1), which a query must
/// never follow. With `lists`, every node but those in `no_vector` gets a whole-node vector, and the lists above
/// are loaded.
fn fixture(lists: bool, no_vector: &[&str]) -> Graph {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../umx-cypher/fixtures/design");
    let nodes = decode_nodes(&fs::read(dir.join("nodes.jsonl")).unwrap())
        .unwrap()
        .records;
    let mut edges = decode_edges(&fs::read(dir.join("edges.jsonl")).unwrap())
        .unwrap()
        .records;
    let mut legacy: Edge = edges[0].clone();
    legacy.id = "01JDCYPHER0000000000000E99".parse().unwrap();
    legacy.kind = "similar_to".into();
    legacy.from = ix("C1");
    legacy.to = ix("N1");
    edges.push(legacy);
    let mut g = Graph::from_records(nodes, edges);
    if lists {
        let all: Vec<Id> = g.nodes().map(|n| n.id()).collect();
        for node in all {
            if !no_vector.iter().any(|s| ix(s) == node) {
                g.set_vector(SPACE, node, None, &[1, 2, 3, 4], TextHash([0; 16]));
            }
        }
        let mut load = g.load_similar(SPACE, 3, 6000);
        for (node, list) in LISTS {
            for (other, score) in list {
                assert!(load.put(ix(node), ix(other), *score));
            }
        }
        load.finish();
    }
    g
}

fn rows_of(g: &Graph, text: &str) -> umx_query::Rows {
    let o = Ontology::starter();
    let s = o.symmetric_kinds();
    run(
        &GraphView::one("design", g, &s),
        text,
        &RunOptions::default(),
    )
    .expect(text)
}

fn show(c: &Cell) -> String {
    match c {
        Cell::Null => "null".into(),
        Cell::Int(i) => i.to_string(),
        Cell::Float(f) => format!("{f:?}"),
        Cell::Str(s) => s.clone(),
        Cell::Node(n) => n.id.to_text(),
        Cell::Edge(e) => e.id.to_text(),
        other => format!("{other:?}"),
    }
}

fn texts(g: &Graph, text: &str) -> Vec<Vec<String>> {
    rows_of(g, text)
        .rows
        .iter()
        .map(|r| r.iter().map(show).collect())
        .collect()
}

/// req-65's done_when query: b for every list entry over 0.8, in scan order (a by id), each list best first.
#[test]
fn the_score_filter_returns_the_oracle_rows() {
    let g = fixture(true, &[]);
    let got = texts(
        &g,
        "MATCH (a)-[s:similar_to]->(b) WHERE s.score > 0.8 RETURN b",
    );
    let oracle: Vec<Vec<String>> = g
        .nodes()
        .flat_map(|a| {
            g.similar_out(a.ix())
                .iter()
                .filter(|e| e.score > 8000)
                .map(|e| vec![g.node_at(e.other).id().to_text()])
                .collect::<Vec<_>>()
        })
        .collect();
    let by_hand: Vec<Vec<String>> = ["D2", "D1", "Q2", "Q2", "Q1", "P1"]
        .iter()
        .map(|s| vec![id(s)])
        .collect();
    assert_eq!(oracle, by_hand);
    assert_eq!(got, oracle);
    // 0.8 itself is not over 0.8: Q1 to Q3 and Q3 to Q1 score exactly 8000.
    let at_least = texts(
        &g,
        "MATCH (a)-[s:similar_to]->(b) WHERE s.score >= 0.8 RETURN count(*) AS n",
    );
    assert_eq!(at_least, [["8"]]);
}

#[test]
fn a_similar_to_edge_reads_as_derived() {
    let g = fixture(true, &[]);
    let alias = rows_of(&g, "MATCH ()-[s:similar_to]->() RETURN type(s) AS rel");
    let twin = rows_of(&g, "MATCH ()-[s:similar_to]->() RETURN label(s) AS rel");
    assert!(!alias.rows.is_empty());
    assert_eq!(alias.rows, twin.rows);
    let r = rows_of(
        &g,
        &format!(
            "MATCH (a {{id: '{}'}})-[s:similar_to]->(b) RETURN s, s.score AS score, label(s) AS rel, s.id AS sid, \
             s.address AS address, s.from AS from_id, s.to AS to_id, s.graph AS graph",
            id("Q1")
        ),
    );
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(r.rows.len(), 3);
    let [Cell::Similar(s), score, rel, sid, address, from, to, graph] = &r.rows[0][..] else {
        panic!("{:?}", r.rows);
    };
    assert_eq!(
        (s.graph.as_str(), s.from, s.to, s.score),
        ("design", ix("Q1"), ix("Q2"), 9100)
    );
    assert_eq!(score, &Cell::Float(0.91));
    assert_eq!(rel, &Cell::Str("similar_to".into()));
    assert_eq!((sid, address), (&Cell::Null, &Cell::Null));
    assert_eq!(from, &Cell::Str(id("Q1")));
    assert_eq!(to, &Cell::Str(id("Q2")));
    assert_eq!(graph, &Cell::Str("design".into()));
    // score is a standard field, and similar_to is known even with no lists: no vocabulary warning.
    let bare = fixture(false, &[]);
    let r = rows_of(
        &bare,
        "MATCH (a)-[s:similar_to]->(b) WHERE s.score > 0.5 RETURN b.id AS id",
    );
    assert!(r.rows.is_empty());
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn both_directions_inside_one_graph() {
    let g = fixture(true, &[]);
    // In: every node whose top k holds Q1, in id order, with the score in its own list.
    let holders = texts(
        &g,
        &format!(
            "MATCH (b {{id: '{}'}})<-[s:similar_to]-(a) RETURN a.id AS id, s.score AS score",
            id("Q1")
        ),
    );
    assert_eq!(
        holders,
        [[id("Q2"), "0.91".into()], [id("Q3"), "0.8".into()]]
    );
    // Undirected: the out list, then the in list.
    let both = texts(
        &g,
        &format!(
            "MATCH (a {{id: '{}'}})-[:similar_to]-(b) RETURN b.id AS id",
            id("Q1")
        ),
    );
    let want: Vec<Vec<String>> = ["Q2", "Q3", "D1", "Q2", "Q3"]
        .iter()
        .map(|s| vec![id(s)])
        .collect();
    assert_eq!(both, want);
}

/// Followed only when named: untyped steps, degree and count(r) answer the same with lists as without, and a
/// stored similar_to record is never followed, named or not.
#[test]
fn only_a_named_step_follows_similar_to() {
    let with = fixture(true, &[]);
    let without = fixture(false, &[]);
    for text in [
        "MATCH (n)--() RETURN n.id AS id, count(*) AS degree ORDER BY id",
        "MATCH (n)-[r]-() RETURN n.id AS id, count(r) AS edges ORDER BY id",
        "MATCH (a)-->(b) RETURN a.id AS from_id, b.id AS to_id ORDER BY from_id, to_id",
        "MATCH (a)-[*1..2]-(b) RETURN count(*) AS n",
        "MATCH (a)-[r]->(b) RETURN DISTINCT label(r) AS rel ORDER BY rel",
    ] {
        let (a, b) = (rows_of(&with, text), rows_of(&without, text));
        assert_eq!(a.rows, b.rows, "{text}");
        assert_eq!(a.work, b.work, "{text}");
        assert!(a.similar_missing.is_none(), "{text}");
    }
    let rels = texts(
        &with,
        "MATCH (a)-[r]->(b) RETURN DISTINCT label(r) AS rel ORDER BY rel",
    );
    assert!(!rels.iter().any(|r| r[0] == "similar_to"), "{rels:?}");
    // C1 holds no list; its stored similar_to record to N1 is not read.
    let c1 = texts(
        &with,
        &format!(
            "MATCH (a {{id: '{}'}})-[:similar_to]-(b) RETURN b.id AS id",
            id("C1")
        ),
    );
    assert!(c1.is_empty(), "{c1:?}");
}

/// An OR list reads the lists first, then the stored relation, exactly as each named alone.
#[test]
fn an_or_list_mixes_lists_and_stored_edges() {
    let g = fixture(true, &[]);
    let one = |rel: &str| {
        texts(
            &g,
            &format!(
                "MATCH (a {{id: '{}'}})-[r:{rel}]-(b) RETURN label(r) AS rel, b.id AS id",
                id("D1")
            ),
        )
    };
    let mixed = one("similar_to|blocks");
    let mut want = Vec::new();
    // Undirected: out (lists, then stored), then in (lists, then stored).
    let out = |rel: &str| {
        texts(
            &g,
            &format!(
                "MATCH (a {{id: '{}'}})-[r:{rel}]->(b) RETURN label(r) AS rel, b.id AS id",
                id("D1")
            ),
        )
    };
    let inc = |rel: &str| {
        texts(
            &g,
            &format!(
                "MATCH (a {{id: '{}'}})<-[r:{rel}]-(b) RETURN label(r) AS rel, b.id AS id",
                id("D1")
            ),
        )
    };
    want.extend(out("similar_to"));
    want.extend(out("blocks"));
    want.extend(inc("similar_to"));
    want.extend(inc("blocks"));
    assert!(!out("blocks").is_empty() || !inc("blocks").is_empty());
    assert_eq!(mixed, want);
    assert_eq!(mixed.len(), one("similar_to").len() + one("blocks").len());
}

#[test]
fn variable_length_is_capped_at_two() {
    let g = fixture(true, &[]);
    // A walk: P1 to Q2, then Q2's list (Q1, P1).
    let walk = texts(
        &g,
        &format!(
            "MATCH (a {{id: '{}'}})-[:similar_to*1..2]->(b) RETURN b.id AS id",
            id("P1")
        ),
    );
    let want: Vec<Vec<String>> = ["Q2", "Q1", "P1"].iter().map(|s| vec![id(s)]).collect();
    assert_eq!(walk, want);
    let o = Ontology::starter();
    let s = o.symmetric_kinds();
    for text in [
        "MATCH (a)-[:similar_to*1..3]->(b) RETURN b.id AS id",
        "MATCH (a)-[:similar_to|blocks*1..3]->(b) RETURN b.id AS id",
    ] {
        let e = run(
            &GraphView::one("design", &g, &s),
            text,
            &RunOptions::default(),
        )
        .expect_err(text);
        assert_eq!(e.code, check::BOUND_TOO_LARGE, "{text}");
        assert_eq!(e.slot("max").as_deref(), Some("2"), "{text}");
    }
    // Without a similar_to step the cap is 10.
    rows_of(&g, "MATCH (a)-[:blocks*1..3]->(b) RETURN count(*) AS n");
}

/// The caller warns when a similar_to step read from or reached a node with no vector: its list is empty, so the
/// answer may be missing rows.
#[test]
fn a_node_with_no_vector_is_reported() {
    let g = fixture(true, &["P2"]);
    let bound = |short: &str| {
        rows_of(
            &g,
            &format!(
                "MATCH (a {{id: '{}'}})-[:similar_to]->(b) RETURN b.id AS id",
                id(short)
            ),
        )
        .similar_missing
    };
    assert_eq!(bound("Q1"), Some(vec![false]));
    assert_eq!(bound("P2"), Some(vec![true]));
    // An unbound scan reads every node's list, P2's too.
    let scan = rows_of(&g, "MATCH (a)-[:similar_to]->(b) RETURN count(*) AS n");
    assert_eq!(scan.similar_missing, Some(vec![true]));
    assert_eq!(scan.rows, [[Cell::Int(10)]]);
    let all = fixture(true, &[]);
    assert_eq!(
        rows_of(&all, "MATCH (a)-[:similar_to]->(b) RETURN count(*) AS n").similar_missing,
        Some(vec![false])
    );
    // A graph with no lists: every node lacks a vector in them.
    let bare = fixture(false, &[]);
    assert_eq!(
        rows_of(&bare, "MATCH (a)-[:similar_to]->(b) RETURN count(*) AS n").similar_missing,
        Some(vec![true])
    );
    // Named only inside EXISTS still counts as named.
    let exists = rows_of(
        &all,
        "MATCH (a) WHERE EXISTS { MATCH (a)-[:similar_to]->() } RETURN count(*) AS n",
    );
    assert_eq!(exists.rows, [[Cell::Int(6)]]);
    assert_eq!(exists.similar_missing, Some(vec![false]));
}

/// In a view of two graphs each step stays in its graph, and a returned edge names the graph it came from.
#[test]
fn two_graphs_keep_their_own_lists() {
    let (a, b) = (fixture(true, &[]), fixture(true, &["Q1"]));
    let view = GraphView::new(vec![
        Part {
            alias: "first",
            graph: &a,
            symmetric: Vec::new(),
        },
        Part {
            alias: "second",
            graph: &b,
            symmetric: Vec::new(),
        },
    ]);
    let r = run(
        &view,
        &format!(
            "MATCH (a {{id: '{}'}})-[s:similar_to]->(b) RETURN s",
            id("Q1")
        ),
        &RunOptions::default(),
    )
    .unwrap();
    let graphs: Vec<&str> = r
        .rows
        .iter()
        .map(|row| match &row[0] {
            Cell::Similar(s) => s.graph.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        graphs,
        ["first", "first", "first", "second", "second", "second"]
    );
    assert_eq!(r.similar_missing, Some(vec![false, true]));
}

/// `WHERE s.score > 0.8` prunes the step that binds s: no list entry under the score is held.
#[test]
fn a_score_filter_prunes_the_step() {
    let g = fixture(true, &[]);
    let all = rows_of(&g, "MATCH (a)-[s:similar_to]->(b) RETURN count(*) AS n");
    let some = rows_of(
        &g,
        "MATCH (a)-[s:similar_to]->(b) WHERE s.score > 0.8 RETURN count(*) AS n",
    );
    assert_eq!(
        (&all.rows, &some.rows),
        (&vec![vec![Cell::Int(10)]], &vec![vec![Cell::Int(6)]])
    );
    // Held rows: Q1's step makes three rows unfiltered and one with the filter, so a cap of two stops only the first.
    let o = Ontology::starter();
    let sym = o.symmetric_kinds();
    let view = GraphView::one("design", &g, &sym);
    let capped = RunOptions {
        max_rows: 2,
        ..RunOptions::default()
    };
    let q1 = |filter: &str| {
        format!(
            "MATCH (a {{id: '{}'}})-[s:similar_to]->(b) {filter} RETURN b.id AS id",
            id("Q1")
        )
    };
    assert_eq!(
        run(&view, &q1(""), &capped).unwrap_err().code,
        check::TOO_LARGE
    );
    assert_eq!(
        run(&view, &q1("WHERE s.score > 0.8"), &capped)
            .unwrap()
            .rows,
        [[Cell::Str(id("Q2"))]]
    );
}

/// Regression (pre-existing, found in s05): a WHERE after a WITH pruned an earlier binding of the same name, so
/// `MATCH (n) WITH count(n) AS c MATCH (n:question) WHERE n.status = 'open'` counted 3 nodes, not 11. A WITH ends
/// pushdown; nodes and relationship variables alike.
#[test]
fn a_where_after_with_never_prunes_an_earlier_binding() {
    let g = fixture(true, &[]);
    let count = |text: &str| texts(&g, text);
    assert_eq!(
        count(
            "MATCH (n) WITH count(n) AS c MATCH (n:question) WHERE n.status = 'open' RETURN DISTINCT c AS c"
        ),
        [["11"]]
    );
    assert_eq!(
        count(
            "MATCH ()-[s]->() WITH count(s) AS c MATCH (a)-[s:similar_to]->(b) WHERE s.score > 0.9 RETURN DISTINCT c AS c"
        ),
        count("MATCH ()-[s]->() RETURN count(s) AS c")
    );
}

/// More holders than a u8 counts (reviewer R134-1): 300 nodes whose top k holds one node. The in list is not bounded
/// by k, so every holder comes back, each with its own slot and score.
#[test]
fn three_hundred_holders_of_one_node() {
    let nodes: Vec<umx_core::Node> = (0..301)
        .map(|i| {
            serde_json::from_value(serde_json::json!({
                "id": format!("01JDHAND{i:0>18}"), "type": "problem", "title": format!("p{i}"), "status": "open",
                "note": "", "evidence": [], "status_history": [], "created": "2026-11-10T00:00:00Z",
                "updated": "2026-11-10T00:00:00Z", "created_by": "fixture",
            }))
            .unwrap()
        })
        .collect();
    let ids: Vec<Id> = nodes.iter().map(|n| n.id).collect();
    let mut g = Graph::from_records(nodes, Vec::new());
    for id in &ids {
        g.set_vector(SPACE, *id, None, &[1, 2, 3, 4], TextHash([0; 16]));
    }
    let mut load = g.load_similar(SPACE, 3, 0);
    for (i, holder) in ids[1..].iter().enumerate() {
        // The held node sits at a different place in different lists.
        if i % 2 == 1 {
            assert!(load.put(*holder, ids[(i + 2) % 300 + 1], 9900));
        }
        assert!(load.put(*holder, ids[0], 9000 - i as i32));
    }
    load.finish();
    let r = rows_of(
        &g,
        &format!(
            "MATCH (b {{id: '{}'}})<-[s:similar_to]-(a) RETURN a.id AS id, s.score AS score",
            ids[0]
        ),
    );
    assert_eq!(r.rows.len(), 300);
    let want: Vec<Vec<Cell>> = ids[1..]
        .iter()
        .enumerate()
        .map(|(i, id)| {
            vec![
                Cell::Str(id.to_text()),
                Cell::Float(f64::from(9000 - i as i32) / 10_000.0),
            ]
        })
        .collect();
    assert_eq!(r.rows, want);
    let undirected = rows_of(
        &g,
        &format!(
            "MATCH (b {{id: '{}'}})-[s:similar_to]-(a) RETURN count(*) AS n",
            ids[0]
        ),
    );
    assert_eq!(undirected.rows, [[Cell::Int(300)]]);
}

/// Regression (reviewer R134-2): a name rebound by UNWIND is a new binding, so a WHERE on it after the UNWIND never
/// prunes the earlier binding of that name, edge or node.
#[test]
fn a_where_after_unwind_never_prunes_an_earlier_binding() {
    let g = fixture(true, &[]);
    let count = |text: &str| texts(&g, text);
    for (with_where, without) in [
        (
            "MATCH ()-[s]->() UNWIND [1] AS s MATCH (n) WHERE s = 1 RETURN count(n) AS c",
            "MATCH ()-[s]->() UNWIND [1] AS s MATCH (n) RETURN count(n) AS c",
        ),
        (
            "MATCH (s) UNWIND [1] AS s MATCH (n) WHERE s = 1 RETURN count(n) AS c",
            "MATCH (s) UNWIND [1] AS s MATCH (n) RETURN count(n) AS c",
        ),
    ] {
        let want = count(without);
        assert_ne!(want, [["0"]], "{without}");
        assert_eq!(count(with_where), want, "{with_where}");
    }
}
