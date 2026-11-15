//! Regression test: cross-clause pushdown must NOT be applied inside an `OPTIONAL MATCH`.
//!
//! A single-variable predicate like `WHERE t IS NULL`, written on a non-optional clause, is eligible
//! for pushdown. But if `t` is bound by an `OPTIONAL MATCH`, pushing `t IS NULL` into that optional
//! expansion would prune every real `t` (no node satisfies `IS NULL`), null-filling every row and
//! wrongly keeping them all. Pushdown must stay off during optional expansion; the anti-join must
//! still work.

use std::collections::HashMap;

use umx_cypher::{CypherValue, GraphProvider, execute, parse};

/// Two `Class` nodes — "Root" (no parent) and "Child" (`-[:PARENT]->` Root) — each owning one
/// `Thing` via `HAS_THING`.
struct Graph {
    labels: Vec<Vec<String>>,
    names: Vec<String>,
    forward: HashMap<(usize, String), Vec<usize>>,
}

impl Graph {
    fn new() -> Self {
        // 0 = Root (Class), 1 = Child (Class), 2 = thing_root (Thing), 3 = thing_child (Thing).
        let mut forward: HashMap<(usize, String), Vec<usize>> = HashMap::new();
        forward.insert((1, "PARENT".to_string()), vec![0]); // Child -[:PARENT]-> Root
        forward.insert((0, "HAS_THING".to_string()), vec![2]); // Root  -[:HAS_THING]-> thing_root
        forward.insert((1, "HAS_THING".to_string()), vec![3]); // Child -[:HAS_THING]-> thing_child

        Graph {
            labels: vec![
                vec!["Class".to_string()],
                vec!["Class".to_string()],
                vec!["Thing".to_string()],
                vec!["Thing".to_string()],
            ],
            names: vec![
                "Root".to_string(),
                "Child".to_string(),
                "thing_root".to_string(),
                "thing_child".to_string(),
            ],
            forward,
        }
    }
}

impl Graph {
    fn rel_index(&self, rel_type: &str) -> usize {
        self.relationship_types()
            .iter()
            .position(|r| r == rel_type)
            .expect("a known relationship type")
    }
}

impl GraphProvider for Graph {
    type NodeId = usize;
    type EdgeId = (usize, usize, usize);

    fn scan(&self, labels: &[String]) -> Vec<usize> {
        (0..self.labels.len())
            .filter(|id| labels.is_empty() || labels.iter().any(|l| self.matches_label(*id, l)))
            .collect()
    }

    fn matches_label(&self, node: usize, label: &str) -> bool {
        self.labels[node].iter().any(|l| l == label)
    }

    fn relationship_types(&self) -> Vec<String> {
        vec!["PARENT".to_string(), "HAS_THING".to_string()]
    }

    fn expand(&self, node: usize, rel_type: &str) -> Vec<((usize, usize, usize), usize)> {
        let rel = self.rel_index(rel_type);
        self.forward
            .get(&(node, rel_type.to_string()))
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(pos, to)| ((node, rel, pos), *to))
            .collect()
    }

    fn expand_in(&self, node: usize, rel_type: &str) -> Vec<((usize, usize, usize), usize)> {
        let rel = self.rel_index(rel_type);
        let mut edges: Vec<((usize, usize, usize), usize)> = self
            .forward
            .iter()
            .filter(|((_, r), _)| r == rel_type)
            .flat_map(|((from, _), targets)| {
                targets
                    .iter()
                    .enumerate()
                    .filter(|(_, to)| **to == node)
                    .map(move |(pos, _)| ((*from, rel, pos), *from))
            })
            .collect();
        // HashMap order is random; sort so results are deterministic.
        edges.sort_unstable();
        edges
    }

    fn edge_property(&self, _edge: (usize, usize, usize), _prop: &str) -> CypherValue {
        CypherValue::Null
    }

    fn edge_label(&self, edge: (usize, usize, usize)) -> String {
        self.relationship_types()[edge.1].clone()
    }

    fn edge_id(&self, edge: (usize, usize, usize)) -> String {
        format!("e{}-{}-{}", edge.0, edge.1, edge.2)
    }

    fn property(&self, node: usize, prop: &str) -> CypherValue {
        if prop == "name" {
            CypherValue::Str(self.names[node].clone())
        } else {
            CypherValue::Null
        }
    }

    fn node_id(&self, node: usize) -> String {
        format!("n{node}")
    }

    fn label(&self, node: usize) -> String {
        self.labels[node].first().cloned().unwrap_or_default()
    }

    fn name(&self, node: usize) -> String {
        self.names[node].clone()
    }
}

/// `t IS NULL` (a pushable single-variable predicate on a non-optional clause) references `t`, which
/// is bound by the OPTIONAL MATCH. If it were pushed into the optional expansion it would drop every
/// real `t`, so both classes would survive; correctly, only Root (which has no PARENT) survives.
#[test]
fn is_null_predicate_not_pushed_into_optional_match() {
    let graph = Graph::new();
    let query = "MATCH (c:Class) \
                 OPTIONAL MATCH (c)-[:PARENT]->(t:Class) \
                 MATCH (c)-[:HAS_THING]->(x:Thing) \
                 WHERE t IS NULL \
                 RETURN x.name AS name";
    let result = execute(&graph, &parse(query).unwrap()).unwrap();

    let names: Vec<String> = result
        .rows
        .iter()
        .map(|row| row[0].to_display_string())
        .collect();
    assert_eq!(
        names,
        vec!["thing_root".to_string()],
        "only Root (no PARENT edge) should survive the t-IS-NULL anti-join"
    );
}
