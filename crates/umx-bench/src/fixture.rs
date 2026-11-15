//! Seeded synthetic concept graphs (no client data). Same seed and size give byte-identical files. The plain
//! fixture is unchanged since 2026-11-09. The rich one (copied from spikes/002-storage) adds what real concept
//! graphs carry: a note of about 280 characters on every node, evidence pointers, status history, per-record
//! times and several authors. All of its text comes from the word lists below.

use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use umx_core::codec;
use umx_core::ontology::{PART_OF, STARTER_RELATIONS, STARTER_STATUSES, STARTER_TYPES};
use umx_core::{Edge, GraphMeta, Id, Node, Ontology};

/// 2026-11-09T00:00:00Z: the time bits of every fixture id, and every record's timestamps.
const EPOCH_MS: u64 = 1_794_182_400_000;
const STAMP: &str = "2026-11-09T00:00:00Z";

const SUBJECTS: &[&str] = &[
    "Approval",
    "Vendor",
    "Launch",
    "Budget",
    "Hiring",
    "Pricing",
    "Onboarding",
    "Support",
    "Billing",
    "Security",
    "Compliance",
    "Migration",
    "Forecast",
    "Inventory",
    "Shipping",
    "Training",
    "Reporting",
    "Licensing",
    "Audit",
    "Roadmap",
];
const ISSUES: &[&str] = &[
    "delays",
    "costs",
    "backlog",
    "risk",
    "gaps",
    "rework",
    "handoffs",
    "scope",
    "quality",
    "capacity",
    "churn",
    "drift",
    "bottleneck",
    "overlap",
    "uncertainty",
];
const PLACES: &[&str] = &[
    "in procurement",
    "for the pilot",
    "across regions",
    "in the second quarter",
    "for enterprise accounts",
    "during rollout",
    "in the field team",
    "for partners",
    "at month end",
    "in the new market",
];

const AUTHORS: &[&str] = &[
    "agent-ingest",
    "agent-review",
    "user-a",
    "agent-live",
    "umx-bench",
];

/// SplitMix64: small, fast and the same everywhere.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a>(&mut self, items: &'a [&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

fn id(n: u64) -> Id {
    Id(((EPOCH_MS as u128) << 80) | n as u128)
}

/// The three graph files of a fixture.
pub struct Fixture {
    pub graph: Vec<u8>,
    pub nodes: Vec<u8>,
    pub edges: Vec<u8>,
    pub node_count: usize,
    pub edge_count: usize,
}

fn sentence(rng: &mut Rng) -> String {
    format!(
        "{} {} {} was raised again and the group tied it to {} {}.",
        rng.pick(SUBJECTS),
        rng.pick(ISSUES),
        rng.pick(PLACES),
        rng.pick(SUBJECTS).to_lowercase(),
        rng.pick(ISSUES)
    )
}

/// `nodes` nodes with about `edges_per_node` edges each: a part_of tree (every node but the first has a parent
/// among earlier nodes) plus random starter relations, no self-loops or duplicate triples. `rich` adds notes,
/// evidence, status history, per-record times and authors; the plain draws and bytes are unchanged.
pub fn generate(seed: u64, nodes: usize, edges_per_node: usize, rich: bool) -> Fixture {
    let mut rng = Rng(seed);
    let mut counter = 0u64;
    let mut next_id = || {
        counter += 1;
        id(counter)
    };
    // Rich records are one second apart, so times do not all intern to one value.
    let stamp = |i: usize| {
        if rich {
            umx_core::time::iso_from_ms(EPOCH_MS + i as u64 * 1000)
        } else {
            STAMP.to_owned()
        }
    };
    let meta = GraphMeta {
        spines: Vec::new(),
        former_aliases: Vec::new(),
        former_paths: Vec::new(),
        umx: 1,
        id: next_id(),
        alias: "bench".into(),
        title: format!("Synthetic bench graph (seed {seed}, {nodes} nodes)"),
        created: stamp(0),
        embedding: None,
        ontology: Ontology::starter(),
    };
    let mut records = Vec::with_capacity(nodes);
    for i in 0..nodes {
        let mut fields = BTreeMap::new();
        if i % 3 == 0 {
            fields.insert("likelihood".to_owned(), json!(1 + rng.below(5)));
        }
        if i % 5 == 0 {
            fields.insert("owner".to_owned(), json!(format!("team_{}", rng.below(12))));
        }
        let title = format!(
            "{} {} {}",
            rng.pick(SUBJECTS),
            rng.pick(ISSUES),
            rng.pick(PLACES)
        );
        let (mut note, mut evidence, mut history) = (String::new(), Vec::new(), Vec::new());
        if rich {
            for _ in 0..2 + rng.below(3) {
                note.push_str(&sentence(&mut rng));
                note.push(' ');
            }
            for _ in 0..rng.below(4) {
                let from = 100 + rng.below(9000);
                evidence.push(json!({
                    "source": format!("transcripts/call-{:03}.ndjson", rng.below(40)),
                    "seq": [from, from + 1 + rng.below(40)],
                    "kind": "room",
                    "exact": false,
                    "hash": format!("sha256:{:016x}{:016x}{:016x}{:016x}", rng.next(), rng.next(), rng.next(), rng.next()),
                }));
            }
            for _ in 0..rng.below(3) {
                history.push(json!({
                    "from": rng.pick(STARTER_STATUSES),
                    "to": rng.pick(STARTER_STATUSES),
                    "at": stamp(i),
                    "by": rng.pick(AUTHORS),
                    "reason": sentence(&mut rng),
                    "label": "resolved",
                }));
            }
        } else if i % 4 == 0 {
            note = format!("Synthetic note {i}.");
        }
        records.push(Node {
            id: next_id(),
            kind: rng.pick(STARTER_TYPES).to_owned(),
            title,
            status: rng.pick(STARTER_STATUSES).to_owned(),
            note,
            evidence,
            status_history: history,
            created: stamp(i),
            updated: stamp(i),
            created_by: if rich {
                rng.pick(AUTHORS).to_owned()
            } else {
                "umx-bench".into()
            },
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields,
        });
    }
    let mut edges = Vec::new();
    let mut seen = BTreeSet::new();
    let others: Vec<&str> = STARTER_RELATIONS
        .iter()
        .copied()
        .filter(|r| *r != PART_OF)
        .collect();
    for i in 1..nodes {
        let parent = rng.below(i);
        let mut add = |from: usize,
                       rel: &str,
                       to: usize,
                       edges: &mut Vec<Edge>,
                       next_id: &mut dyn FnMut() -> Id| {
            if from != to && seen.insert((from, rel.to_owned(), to)) {
                edges.push(Edge {
                    id: next_id(),
                    kind: rel.to_owned(),
                    from_graph: None,
                    from: records[from].id,
                    to_graph: None,
                    to: records[to].id,
                    created: stamp(i),
                    created_by: "umx-bench".into(),
                    authored_by_session: None,
                    edited_by_session: None,
                    evidence: Vec::new(),
                    fields: BTreeMap::new(),
                });
            }
        };
        add(i, PART_OF, parent, &mut edges, &mut next_id);
        for _ in 1..edges_per_node {
            let (to, rel) = (rng.below(nodes), others[rng.below(others.len())]);
            add(i, rel, to, &mut edges, &mut next_id);
        }
    }
    Fixture {
        graph: codec::encode_graph(&meta),
        nodes: codec::encode_nodes(&records),
        edges: codec::encode_edges(&edges),
        node_count: records.len(),
        edge_count: edges.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_bytes_and_a_byte_identical_round_trip() {
        let a = generate(7, 300, 3, false);
        let b = generate(7, 300, 3, false);
        assert_eq!(
            (a.graph, a.nodes.clone(), a.edges.clone()),
            (b.graph, b.nodes, b.edges)
        );
        assert_ne!(generate(8, 300, 3, false).nodes, a.nodes);
        assert_eq!(
            codec::encode_nodes(&codec::decode_nodes(&a.nodes).unwrap().records),
            a.nodes
        );
        assert_eq!(
            codec::encode_edges(&codec::decode_edges(&a.edges).unwrap().records),
            a.edges
        );
        assert_eq!(a.node_count, 300);
        assert!(
            a.edge_count > 600 && a.edge_count <= 897,
            "{}",
            a.edge_count
        );
    }

    /// FNV-1a over the three files: a stable digest that does not depend on the std hasher.
    fn digest(fx: &Fixture) -> u64 {
        [&fx.graph, &fx.nodes, &fx.edges]
            .iter()
            .flat_map(|b| b.iter())
            .fold(0xcbf2_9ce4_8422_2325, |h, b| {
                (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
            })
    }

    #[test]
    fn the_plain_fixture_keeps_its_bytes() {
        // Pinned before the rich fixture existed (s003 s01): earlier result files stay comparable.
        assert_eq!(
            digest(&generate(1, 1000, 3, false)),
            5_799_141_795_078_924_001
        );
    }

    #[test]
    fn the_rich_fixture_carries_notes_evidence_and_history_and_round_trips() {
        let fx = generate(1, 1000, 3, true);
        assert!(fx.nodes == generate(1, 1000, 3, true).nodes);
        let nodes = codec::decode_nodes(&fx.nodes).unwrap().records;
        assert_eq!(codec::encode_nodes(&nodes), fx.nodes);
        assert!(nodes.iter().all(|n| n.note.len() > 100));
        let mean_note = nodes.iter().map(|n| n.note.len()).sum::<usize>() / nodes.len();
        assert!((230..=330).contains(&mean_note), "{mean_note}");
        assert!(nodes.iter().any(|n| n.evidence.len() == 3));
        assert!(nodes.iter().any(|n| n.status_history.len() == 2));
        assert_ne!(nodes[1].created, nodes[2].created);
        let hash = nodes.iter().flat_map(|n| &n.evidence).next().unwrap()["hash"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(hash.starts_with("sha256:") && hash.len() == 71, "{hash}");
    }
}
