//! ws003 s22 and s29: the evidence kinds columns answer exactly what each node's record answers
//! (`Node::evidence_kinds`), through inserts, edits, removals, renumbering, batch commits, rollbacks and drops, and the
//! `--where` filter they drive matches the record-based filter. Three fixtures: a few kinds (the forward column is a
//! mask), around 64 (it changes between a mask and a CSR both ways) and one kind per node (a CSR).

use serde_json::{Value, json};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use umx_core::value::compare_values;
use umx_core::{Id, Node};
use umx_graph::{Graph, NodeFilter, Op, Where};

/// Kinds the writes use: plain, an escaped one, and two date-like texts (`--where` compares dates as dates).
const POOL: [&str; 8] = [
    "analyst",
    "client",
    "room",
    "transcript",
    "document",
    "2026-01-01",
    "2026-01-01T00:00:00Z",
    "tab\t\"q\" é",
];

/// How many kinds the writes draw from.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fx {
    /// [`POOL`] only.
    Few,
    /// 80 more, so a graph of about 90 nodes holds between about 50 and 70.
    Around64,
    /// Each node's own kind (from its number), a pool kind now and then.
    PerNode,
}

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

/// A kind for node `n`'s evidence, drawn as `fx` says.
fn kind(r: &mut Rng, fx: Fx, n: u128) -> String {
    match fx {
        Fx::Around64 if r.below(8) != 0 => format!("p{:02}", r.below(80)),
        Fx::PerNode if r.below(5) != 0 => format!("k{n}-{}", r.below(2)),
        _ => POOL[r.below(POOL.len())].into(),
    }
}

/// One evidence item: mostly a pointer with a kind, sometimes one whose kind is null, a number or a list, one with
/// no kind, a bare string, or a nested `kind` that does not count.
fn item(r: &mut Rng, fx: Fx, n: u128) -> Value {
    match r.below(10) {
        0..=4 => {
            json!({"source": format!("s{}.md", r.below(5)), "kind": kind(r, fx, n)})
        }
        5 => json!({"source": "a.md", "kind": null}),
        6 => json!({"source": "a.md", "kind": 7}),
        7 => json!({"source": "a.md", "kind": ["analyst"], "nested": {"kind": "client"}}),
        8 => json!({"source": "no-kind.md"}),
        _ => json!("loose"),
    }
}

fn evidence(r: &mut Rng, fx: Fx, n: u128) -> Vec<Value> {
    let items = r.below(5);
    (0..items).map(|_| item(r, fx, n)).collect()
}

fn node(r: &mut Rng, fx: Fx, n: u128) -> Node {
    Node {
        id: id(n),
        kind: ["risk", "problem"][r.below(2)].into(),
        title: format!("Node {n}"),
        status: ["open", "done"][r.below(2)].into(),
        note: String::new(),
        evidence: evidence(r, fx, n),
        status_history: vec![],
        created: "2026-11-10T01:00:00Z".into(),
        updated: "2026-11-10T01:00:00Z".into(),
        created_by: "tester".into(),
        locator: None,
        authored_by_session: None,
        edited_by_session: None,
        fields: BTreeMap::new(),
    }
}

/// `filter.rs`'s rule for a list field, on the record: `=` when some kind equals the value, `!=` when none does.
fn holds(n: &Node, w: &Where) -> bool {
    let any = n.evidence_kinds().into_iter().any(|k| {
        let k = Value::String(k.into());
        k == w.value || compare_values(&k, &w.value) == Some(Ordering::Equal)
    });
    match w.op {
        Op::Eq => any,
        Op::Ne => !any,
        _ => false,
    }
}

/// Every node's kinds and `field` against its record, every kind's holders, and the filter against the record
/// filter, crossed with type and status.
fn verify_reads(g: &Graph) {
    let records: Vec<Node> = g.nodes().map(|n| n.to_node()).collect();
    for (n, record) in g.nodes().zip(&records) {
        let want = record.evidence_kinds();
        assert_eq!(n.evidence_kinds().collect::<Vec<_>>(), want, "{}", n.id());
        assert_eq!(
            n.field("evidence_kinds"),
            record.field("evidence_kinds"),
            "{}",
            n.id()
        );
    }
    for kind in POOL.iter().copied().chain([
        "unknown",
        "2026-01-01T00:00:00.000Z",
        "p00",
        "p37",
        "k8-0",
        "k12-1",
    ]) {
        let held: Vec<Id> = g.nodes_with_evidence_kind(kind).map(|n| n.id()).collect();
        let want: Vec<Id> = records
            .iter()
            .filter(|r| r.evidence_kinds().contains(&kind))
            .map(|r| r.id)
            .collect();
        assert_eq!(held, want, "holders of {kind:?}");
        for op in [Op::Eq, Op::Ne] {
            for (type_, status) in [
                (None, None),
                (Some("risk"), None),
                (None, Some("open")),
                (Some("problem"), Some("done")),
            ] {
                let w = Where {
                    field: "evidence_kinds".into(),
                    op,
                    value: json!(kind),
                };
                let filter = NodeFilter {
                    tags: Default::default(),
                    kind: type_.map(Into::into),
                    status: status.map(Into::into),
                    wheres: vec![w.clone()],
                    within: None,
                };
                let got: Vec<Id> = g.list_nodes(&filter).iter().map(|n| n.id()).collect();
                let want: Vec<Id> = records
                    .iter()
                    .filter(|r| {
                        type_.is_none_or(|t| r.kind == t)
                            && status.is_none_or(|s| r.status == s)
                            && holds(r, &w)
                    })
                    .map(|r| r.id)
                    .collect();
                assert_eq!(got, want, "{kind:?} {op:?} {type_:?} {status:?}");
            }
        }
    }
}

fn verify(g: &Graph) {
    verify_reads(g);
    // The whole column equals one built fresh from the image.
    g.check_rows();
}

/// An id no node holds: above the last (`end`) or in a gap below it, so handles renumber.
fn free_id(g: &Graph, r: &mut Rng, end: bool) -> u128 {
    let last = g.nodes().last().map_or(0, |n| n.id().0 & 0xFFFF_FFFF);
    if end || last < 4 {
        return last + 1 + r.below(3) as u128;
    }
    loop {
        let n = 1 + r.below(last as usize) as u128;
        if g.node(id(n)).is_none() {
            return n;
        }
    }
}

fn edited(g: &Graph, r: &mut Rng, fx: Fx, keep_kinds: bool) -> Option<Node> {
    let ids: Vec<Id> = g.nodes().map(|n| n.id()).collect();
    let target = ids.get(r.below(ids.len().max(1)))?;
    let mut n = g.node(*target)?.to_node();
    if keep_kinds {
        n.title.push('!');
        n.evidence.reverse();
    } else {
        n.evidence = evidence(r, fx, n.id.0 & 0xFFFF_FFFF);
    }
    Some(n)
}

#[test]
fn the_column_equals_the_records_through_writes_batches_and_rollbacks() {
    for (fx, nodes, layouts) in [
        (Fx::Few, 30, &["mask"][..]),
        (Fx::Around64, 90, &["mask", "narrow"][..]),
        (Fx::PerNode, 160, &["narrow"][..]),
    ] {
        // The forward column's form after every round, and how often it changed form.
        let mut seen = BTreeMap::<&str, usize>::new();
        let mut switches = 0;
        for seed in 1..=4u64 {
            let mut r = Rng(seed);
            let nodes: Vec<Node> = (1..=nodes).map(|i| node(&mut r, fx, i * 4)).collect();
            let mut g = Graph::from_records(nodes, vec![]);
            verify(&g);
            for _ in 0..120 {
                let was = g.evidence_kinds_layout();
                round(&mut g, &mut r, fx);
                verify(&g);
                *seen.entry(g.evidence_kinds_layout()).or_default() += 1;
                switches += usize::from(was != g.evidence_kinds_layout());
            }
        }
        let held: Vec<&str> = seen.keys().copied().collect();
        assert_eq!(held, layouts, "{fx:?}: forms {seen:?}");
        assert!(
            fx != Fx::Around64 || switches >= 4,
            "{fx:?}: {switches} changes of form"
        );
    }
}

/// One write: an insert at the end or in the middle, an edit, a removal, or a batch.
fn round(g: &mut Graph, r: &mut Rng, fx: Fx) {
    match r.below(7) {
        0 | 1 => {
            let end = r.below(2) == 0;
            let n = free_id(g, r, end);
            let n = node(r, fx, n);
            g.insert_node(n).unwrap();
        }
        2 | 3 => {
            let keep = r.below(3) == 0;
            if let Some(n) = edited(g, r, fx, keep) {
                g.insert_node(n).unwrap();
            }
        }
        4 => {
            let ids: Vec<Id> = g.nodes().map(|n| n.id()).collect();
            if !ids.is_empty() {
                // The last row half the time (pop), else one in the middle (renumbering).
                let at = if r.below(2) == 0 {
                    ids.len() - 1
                } else {
                    r.below(ids.len())
                };
                g.remove_node(ids[at]).unwrap();
            }
        }
        _ => batch_round(g, r, fx),
    }
}

/// A batch of inserts (end and middle), edits and placeholder fills, then commit, rollback, prepare and rollback,
/// or drop. Reads inside the batch answer from the column too.
fn batch_round(g: &mut Graph, r: &mut Rng, fx: Fx) {
    let before = (g.encode_nodes(), kinds(g));
    let outcome = r.below(4);
    let mut b = g.batch();
    if r.below(3) == 0 {
        let base = free_id(b.graph(), r, true);
        let ids: Vec<Id> = (0..3).map(|i| id(base + i * 2)).collect();
        b.insert_placeholders(ids.clone(), "2026-11-12T00:00:00Z")
            .unwrap();
        for (i, at) in ids.iter().enumerate() {
            let mut n = node(r, fx, base + i as u128 * 2);
            n.id = *at;
            b.fill(*at, n).unwrap();
        }
    }
    for _ in 0..1 + r.below(6) {
        let n = match r.below(3) {
            0 => {
                let end = r.below(2) == 0;
                let at = free_id(b.graph(), r, end);
                node(r, fx, at)
            }
            k => match edited(b.graph(), r, fx, k == 2) {
                Some(n) => n,
                None => continue,
            },
        };
        b.insert_node(n).unwrap();
        verify_reads(b.graph());
    }
    match outcome {
        0 => {
            b.commit();
            return;
        }
        1 => b.rollback(),
        2 => {
            b.prepare().unwrap();
            b.rollback();
        }
        _ => drop(b),
    }
    assert_eq!((g.encode_nodes(), kinds(g)), before, "rolled back");
}

fn kinds(g: &Graph) -> Vec<(Id, Vec<String>)> {
    g.nodes()
        .map(|n| (n.id(), n.evidence_kinds().map(String::from).collect()))
        .collect()
}

#[test]
fn hand_edited_lines_read_as_the_codec_reads_them() {
    let t = |n: u128| id(n).to_text();
    let line = |n: u128, evidence: &str, extra: &str| {
        format!(
            "{{\"id\":\"{}\",\"type\":\"risk\",\"title\":\"t\",\"status\":\"open\",\"note\":\"\",\"evidence\":{evidence},\
             \"status_history\":[],\"created\":\"2026-11-10T01:00:00Z\",\"updated\":\"2026-11-10T01:00:00Z\",\
             \"created_by\":\"t\"{extra}}}\n",
            t(n)
        )
    };
    // 101 levels: past the scanner's depth, within serde's, so the line goes to the codec.
    let deep = format!(",\"deep\":{}{}", "[".repeat(101), "]".repeat(101));
    let file = [
        // A repeated kind key in one item: serde's map keeps the last.
        line(1, r#"[{"kind":"room","kind":"analyst"}]"#, ""),
        line(2, r#"[{"kind":"room","kind":5}]"#, ""),
        line(
            3,
            r#"[ { "kind" : "a\u00e9\"b" } , "x" , 3 , {"k\u0069nd":"escaped key"} ]"#,
            "",
        ),
        line(
            4,
            r#"[{"kind":"room"},{"kind":"room"},{"kind":"client"}]"#,
            &deep,
        ),
        line(5, r#"[]"#, &deep),
    ]
    .concat();
    let (g, _) = Graph::load_nodes(file.into_bytes()).unwrap();
    verify(&g);
    let got = kinds(&g);
    assert_eq!(got[0].1, ["analyst"]);
    assert!(got[1].1.is_empty());
    assert_eq!(got[2].1, ["a\u{e9}\"b", "escaped key"]);
    assert_eq!(got[3].1, ["client", "room"]);
    assert!(got[4].1.is_empty());
}
