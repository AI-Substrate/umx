//! ws003 s22: a MATCH's WHERE skips the conjuncts its pushdown already proved for the variables it bound, and the
//! answer is unchanged. The oracle is the same predicate after `WITH`, which is never pushed down, so it is evaluated
//! once per row as the query reads. Seeded graphs; single- and multi-variable patterns, two MATCHes sharing a
//! variable, ORs, OPTIONAL MATCH, a variable bound before the MATCH, and pushed predicates that error.

use std::collections::BTreeMap;

use serde_json::json;
use umx_core::{Edge, Id, Node};
use umx_graph::Graph;
use umx_query::{Cell, GraphView, RunOptions, run};

const KINDS: [&str; 5] = ["analyst", "client", "transcript", "room", "document"];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn id(n: u128) -> Id {
    Id((0x0194_0000_0000_u128 << 80) | n)
}

/// `mix` is a string, except on `numeric` nodes, where `toLower(n.mix)` errors.
fn node(r: &mut Rng, n: u128, numeric: bool, status: Option<&str>) -> Node {
    let mix = ["x", "y"][r.below(2)];
    let mut fields = BTreeMap::new();
    fields.insert("mix".into(), if numeric { json!(7) } else { json!(mix) });
    let evidence = (0..r.below(4))
        .map(|_| json!({"source": "s.md", "kind": KINDS[r.below(KINDS.len())]}))
        .collect();
    Node {
        id: id(n),
        kind: ["risk", "problem"][r.below(2)].into(),
        title: format!("t{}", r.below(20)),
        status: status.unwrap_or(["open", "done"][r.below(2)]).into(),
        note: ["", "alpha", "beta", "x"][r.below(4)].into(),
        evidence,
        status_history: vec![],
        created: "2026-11-10T01:00:00Z".into(),
        updated: "2026-11-10T01:00:00Z".into(),
        created_by: "tester".into(),
        locator: None,
        authored_by_session: None,
        edited_by_session: None,
        fields,
    }
}

/// A node layout: for some nodes, whether `mix` is numeric and a fixed status.
type Layout = fn(u128) -> Option<(bool, &'static str)>;

fn graph(seed: u64, layout: Layout) -> Graph {
    let mut r = Rng(seed);
    let nodes: Vec<Node> = (1..=60u128)
        .map(|n| match layout(n) {
            Some((numeric, status)) => node(&mut r, n, numeric, Some(status)),
            None => node(&mut r, n, false, None),
        })
        .collect();
    let edges = (0..150u128)
        .map(|e| {
            let mut fields = BTreeMap::new();
            fields.insert("weight".into(), json!(r.below(4)));
            Edge {
                id: id(1_000 + e),
                kind: ["supports", "blocks"][r.below(2)].into(),
                from_graph: None,
                from: id(1 + r.below(60) as u128),
                to_graph: None,
                to: id(1 + r.below(60) as u128),
                created: "2026-11-10T01:00:00Z".into(),
                created_by: "tester".into(),
                authored_by_session: None,
                edited_by_session: None,
                evidence: Vec::new(),
                fields,
            }
        })
        .collect();
    Graph::from_records(nodes, edges)
}

type Outcome = Result<Vec<Vec<Cell>>, &'static str>;

fn outcome(g: &Graph, text: &str) -> (Outcome, u64) {
    match run(&GraphView::one("t", g, &[]), text, &RunOptions::default()) {
        Ok(rows) => (Ok(rows.rows), rows.work),
        Err(p) => (Err(p.code), 0),
    }
}

/// (query, the same query with each MATCH's WHERE moved after a WITH of its variables).
const PAIRS: [(&str, &str); 10] = [
    (
        "MATCH (n) WHERE size(n.evidence_kinds) > 0 AND NOT 'client' IN n.evidence_kinds AND NOT 'transcript' IN \
         n.evidence_kinds RETURN n.id AS id ORDER BY id",
        "MATCH (n) WITH n WHERE size(n.evidence_kinds) > 0 AND NOT 'client' IN n.evidence_kinds AND NOT \
         'transcript' IN n.evidence_kinds RETURN n.id AS id ORDER BY id",
    ),
    (
        "MATCH (n) WHERE 'analyst' IN n.evidence_kinds AND size(n.evidence_kinds) = 1 RETURN n.id AS id, \
         n.evidence_kinds AS k ORDER BY id",
        "MATCH (n) WITH n WHERE 'analyst' IN n.evidence_kinds AND size(n.evidence_kinds) = 1 RETURN n.id AS id, \
         n.evidence_kinds AS k ORDER BY id",
    ),
    (
        "MATCH (n:problem {status: 'open'}) WHERE n.note = 'alpha' AND n.title <> 't3' RETURN n.id AS id, n.note AS \
         note",
        "MATCH (n:problem {status: 'open'}) WITH n WHERE n.note = 'alpha' AND n.title <> 't3' RETURN n.id AS id, \
         n.note AS note",
    ),
    (
        "MATCH (a)-[r:supports]->(b) WHERE a.status = 'open' AND r.weight > 1 AND a.title < b.title AND 'analyst' IN \
         b.evidence_kinds RETURN a.id AS src, b.id AS dst, r.weight AS w",
        "MATCH (a)-[r:supports]->(b) WITH a, r, b WHERE a.status = 'open' AND r.weight > 1 AND a.title < b.title \
         AND 'analyst' IN b.evidence_kinds RETURN a.id AS src, b.id AS dst, r.weight AS w",
    ),
    (
        "MATCH (a:risk) WHERE a.status = 'open' MATCH (a)-[r]->(b) WHERE a.note <> 'x' AND b.status = 'done' AND \
         a.title <> b.title RETURN a.id AS src, b.id AS dst ORDER BY src, dst",
        "MATCH (a:risk) WITH a WHERE a.status = 'open' MATCH (a)-[r]->(b) WITH a, r, b WHERE a.note <> 'x' AND \
         b.status = 'done' AND a.title <> b.title RETURN a.id AS src, b.id AS dst ORDER BY src, dst",
    ),
    (
        "MATCH (a)-[r]->(b) WHERE a.status = 'open' OR b.status = 'open' RETURN a.id AS src, b.id AS dst",
        "MATCH (a)-[r]->(b) WITH a, r, b WHERE a.status = 'open' OR b.status = 'open' RETURN a.id AS src, b.id AS dst",
    ),
    (
        "MATCH (n) WHERE n.status = 'open' OR 'client' IN n.evidence_kinds RETURN n.id AS id",
        "MATCH (n) WITH n WHERE n.status = 'open' OR 'client' IN n.evidence_kinds RETURN n.id AS id",
    ),
    (
        "MATCH (a) WHERE a.status = 'open' OPTIONAL MATCH (a)-[r]->(b) WHERE b.status = 'done' RETURN a.id AS src, \
         b.id AS dst ORDER BY src, dst",
        "MATCH (a) WITH a WHERE a.status = 'open' OPTIONAL MATCH (a)-[r]->(b) WHERE b.status = 'done' RETURN a.id \
         AS src, b.id AS dst ORDER BY src, dst",
    ),
    // `a` is bound before the second MATCH and the WITH stops the pushdown: its conjunct must still run.
    (
        "MATCH (a) WITH a MATCH (a)-[r]->(b) WHERE a.status = 'open' AND b.status = 'done' RETURN a.id AS src, b.id \
         AS dst ORDER BY src, dst",
        "MATCH (a) WITH a MATCH (a)-[r]->(b) WITH a, r, b WHERE a.status = 'open' AND b.status = 'done' RETURN \
         a.id AS src, b.id AS dst ORDER BY src, dst",
    ),
    // `toLower` errors on a numeric `mix`: a pushed evaluation errs, so the whole WHERE runs again. The pushdown
    // prunes a `done` node with a numeric `mix` before the WHERE sees it; the oracle gets the same by testing the
    // status first.
    (
        "MATCH (n) WHERE toLower(n.mix) = 'x' AND n.status = 'open' RETURN n.id AS id",
        "MATCH (n) WITH n WHERE n.status = 'open' AND toLower(n.mix) = 'x' RETURN n.id AS id",
    ),
];

#[test]
fn skipping_proven_conjuncts_answers_as_the_where_after_with() {
    // No numeric mix; numeric mix only on `done` nodes (the pushed error is swallowed, the node pruned by the next
    // conjunct); numeric mix on open nodes too (the query errors).
    let layouts: [Layout; 3] = [
        |_| None,
        |n| (n % 7 == 0).then_some((true, "done")),
        |n| (n % 7 == 0).then_some((true, if n % 2 == 0 { "open" } else { "done" })),
    ];
    for seed in 1..=5 {
        for (l, layout) in layouts.iter().enumerate() {
            let g = graph(seed, *layout);
            for (query, oracle) in PAIRS {
                let (got, _) = outcome(&g, query);
                let (want, _) = outcome(&g, oracle);
                assert_eq!(got, want, "seed {seed} layout {l}: {query}");
            }
            // Only the erroring query on the layout with an open numeric `mix` may fail.
            for (i, (query, _)) in PAIRS.iter().enumerate() {
                let failed = outcome(&g, query).0.is_err();
                assert_eq!(failed, i == 9 && l == 2, "seed {seed} layout {l}: {query}");
            }
        }
    }
}

#[test]
fn proven_conjuncts_are_not_charged_twice() {
    // Two graphs that differ only in every seventh node's `mix`: a string, or a number that makes the pushed
    // `toLower` err. Those nodes are `done`, so the next conjunct prunes them either way and both answer the same
    // rows. With the error the whole WHERE runs again for every survivor; without it the proven conjuncts do not.
    let q = PAIRS[9].0;
    let clean = graph(3, |n| (n % 7 == 0).then_some((false, "done")));
    let erred = graph(3, |n| (n % 7 == 0).then_some((true, "done")));
    let (rows_clean, work_clean) = outcome(&clean, q);
    let (rows_erred, work_erred) = outcome(&erred, q);
    assert_eq!(rows_clean, rows_erred);
    let survivors = rows_clean.unwrap().len() as u64;
    assert!(survivors > 0);
    // Each survivor's second evaluation reads `mix` and `status`, a unit each.
    assert!(
        work_erred >= work_clean + 2 * survivors,
        "clean {work_clean}, erred {work_erred}, survivors {survivors}"
    );
}
