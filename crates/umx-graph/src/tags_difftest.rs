//! The tag index (ws008 t2b) built in the load pass, built row by row, and built at once by turning std on, each
//! against a model made from the records (graph-storage's differential, as `keys_difftest`). Every round all three
//! graphs take the same write (an edit, an add at the end or in the middle, which renumbers, a removal, a batch
//! rolled back or committed, and now and then std turned off and on again), and every filter and each row's tags must
//! match the model.

use crate::{Graph, NodeFilter, TagFilter};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use serde_json::{Value, json};
use umx_core::ontology::TextFields;
use umx_core::tags::{Pattern, stored_tags};
use umx_core::{Id, Node, codec};

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

/// Tags as written: some not in stored form (`B` stores as `b`), some unusable (kept, never indexed).
fn pool() -> [Value; 8] {
    [
        json!("a"),
        json!("b"),
        json!("B"),
        json!("origin:x"),
        json!("origin:y"),
        json!("phase:one"),
        json!("bad tag"),
        json!(3),
    ]
}

/// The filters every round asks: each pattern alone, as a complement, and two groups together.
fn filters() -> Vec<TagFilter> {
    let p = |t: &str| Pattern::parse(t).expect("a pattern");
    let mut out = Vec::new();
    for t in ["a", "b", "origin:x", "origin:*", "phase:*", "none_such"] {
        out.push(TagFilter {
            all: vec![vec![p(t)]],
            none: vec![],
        });
        out.push(TagFilter {
            all: vec![],
            none: vec![p(t)],
        });
    }
    out.push(TagFilter {
        all: vec![vec![p("a"), p("origin:y")], vec![p("b")]],
        none: vec![p("phase:one")],
    });
    out
}

fn node(rng: &mut Rng, id: u128) -> Node {
    let mut fields = BTreeMap::new();
    match rng.below(4) {
        0 => {}
        1 => {
            fields.insert("tags".into(), json!("a"));
        }
        _ => {
            let pool = pool();
            let tags: Vec<Value> = (0..1 + rng.below(4))
                .map(|_| pool[rng.below(pool.len())].clone())
                .collect();
            fields.insert("tags".into(), Value::Array(tags));
        }
    }
    Node {
        id: Id(id),
        kind: "risk".into(),
        title: format!("n{id}"),
        status: "open".into(),
        note: String::new(),
        evidence: vec![],
        status_history: vec![],
        created: "2026-11-20T00:00:00Z".into(),
        updated: "2026-11-20T00:00:00Z".into(),
        created_by: "t".into(),
        locator: None,
        authored_by_session: None,
        edited_by_session: None,
        fields,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Answers {
    lists: Vec<Vec<Id>>,
    rows: Vec<(Id, Vec<String>)>,
}

fn answers(g: &Graph) -> Answers {
    Answers {
        lists: filters()
            .into_iter()
            .map(|tags| {
                g.list_nodes(&NodeFilter {
                    tags,
                    ..NodeFilter::default()
                })
                .iter()
                .map(|n| n.id())
                .collect()
            })
            .collect(),
        rows: g
            .nodes()
            .map(|n| (n.id(), g.tags_of(n).into_iter().map(String::from).collect()))
            .collect(),
    }
}

/// The answers from the records alone.
fn model(records: &BTreeMap<Id, Node>) -> Answers {
    let tags_of = |n: &Node| n.fields.get("tags").map(stored_tags).unwrap_or_default();
    let has = |n: &Node, g: &[Pattern]| tags_of(n).iter().any(|t| g.iter().any(|p| p.matches(t)));
    Answers {
        lists: filters()
            .iter()
            .map(|f| {
                records
                    .values()
                    .filter(|n| f.all.iter().all(|g| has(n, g)) && !has(n, &f.none))
                    .map(|n| n.id)
                    .collect()
            })
            .collect(),
        rows: records.values().map(|n| (n.id, tags_of(n))).collect(),
    }
}

fn bytes(records: &BTreeMap<Id, Node>) -> Vec<u8> {
    codec::encode_nodes(records.values())
}

/// A load that knows the graph is std. Id 1 is the graph's own id, which no record has.
fn std_load(bytes: Vec<u8>) -> Graph {
    let (mut g, _) =
        Graph::load_nodes_indexing_for_graph(bytes, TextFields::default(), Id(1), true)
            .expect("loads");
    g.load_edges(&[]).expect("no edges");
    g
}

fn agree(rng: &mut Rng, graphs: &[&mut Graph; 3], records: &BTreeMap<Id, Node>, at: &str) {
    let want = model(records);
    for (g, name) in graphs.iter().zip(["bulk", "row by row", "flipped"]) {
        assert_eq!(answers(g), want, "{name} graph, {at}");
        g.check_rows();
    }
    assert_eq!(answers(&std_load(bytes(records))), want, "fresh load, {at}");
    if records.len() > 1 {
        // Shuffled lines take the load's unsorted path, which lays the image out and derives every row again.
        let all = bytes(records);
        let mut lines: Vec<&[u8]> = all.split_inclusive(|b| *b == b'\n').collect();
        for i in (1..lines.len()).rev() {
            lines.swap(i, rng.below(i + 1));
        }
        assert_eq!(
            answers(&std_load(lines.concat())),
            want,
            "shuffled load, {at}"
        );
    }
}

fn middle_id(rng: &mut Rng, records: &BTreeMap<Id, Node>) -> Option<u128> {
    let (lo, hi) = (records.keys().next()?.0, records.keys().last()?.0);
    (0..8)
        .map(|_| lo + 1 + rng.below((hi - lo).max(1) as usize) as u128)
        .find(|&id| id < hi && !records.contains_key(&Id(id)))
}

fn write(rng: &mut Rng, g: &mut [&mut Graph; 3], records: &mut BTreeMap<Id, Node>) -> &'static str {
    let top = records.keys().last().map_or(4, |i| i.0);
    let pick = |rng: &mut Rng, records: &BTreeMap<Id, Node>| {
        records.keys().nth(rng.below(records.len().max(1))).copied()
    };
    match rng.below(6) {
        0 | 1 => {
            let (id, what) = match (rng.below(3), middle_id(rng, records), pick(rng, records)) {
                (0, Some(m), _) => (m, "add in the middle"),
                (1, _, Some(e)) => (e.0, "edit"),
                _ => (top + 4, "add at the end"),
            };
            let n = node(rng, id);
            for g in g.iter_mut() {
                g.insert_node(n.clone()).expect("fits");
            }
            records.insert(n.id, n);
            what
        }
        2 => {
            let Some(e) = pick(rng, records) else {
                return "nothing";
            };
            let mut n = records[&e].clone();
            n.title.push('.');
            for g in g.iter_mut() {
                g.insert_node(n.clone()).expect("fits");
            }
            records.insert(e, n);
            "edit keeping tags"
        }
        3 => {
            let Some(e) = pick(rng, records) else {
                return "nothing";
            };
            for g in g.iter_mut() {
                assert!(g.remove_node(e).is_some());
            }
            records.remove(&e);
            "remove one"
        }
        4 => {
            // The flipped graph leaves std and joins it again: its index is built anew from its rows.
            g[2].set_std_tags(false);
            assert_eq!(g[2].memory_bytes().tags, 0);
            g[2].set_std_tags(true);
            "std off and on"
        }
        _ => batch(rng, g, records),
    }
}

/// A batch of adds (end and middle) and edits on every graph, then rolled back or committed.
fn batch(rng: &mut Rng, g: &mut [&mut Graph; 3], records: &mut BTreeMap<Id, Node>) -> &'static str {
    let mut after = records.clone();
    let mut nodes = Vec::new();
    for _ in 0..1 + rng.below(6) {
        let top = after.keys().last().map_or(4, |i| i.0);
        let id = match (rng.below(3), middle_id(rng, &after)) {
            (0, Some(m)) => m,
            (1, _) if !after.is_empty() => after.keys().nth(rng.below(after.len())).expect("one").0,
            _ => top + 4,
        };
        let n = node(rng, id);
        after.insert(n.id, n.clone());
        nodes.push((n, model(&after)));
    }
    let commit = rng.below(2) == 0;
    for g in g.iter_mut() {
        let mut b = g.batch();
        for (n, want) in &nodes {
            b.insert_node(n.clone()).expect("fits");
            assert_eq!(&answers(b.graph()), want, "inside a batch");
        }
        if commit {
            b.commit();
        } else {
            b.rollback();
        }
    }
    if commit {
        *records = after;
        "batch committed"
    } else {
        "batch rolled back"
    }
}

#[test]
fn the_tag_index_answers_as_the_records_through_every_write_and_flip() {
    let mut seen = BTreeSet::new();
    for seed in 0..5 {
        let mut rng = Rng(seed);
        let mut records = BTreeMap::new();
        for i in 2..=30 + rng.below(30) as u128 {
            let n = node(&mut rng, i * 4);
            records.insert(n.id, n);
        }
        let mut bulk = std_load(bytes(&records));
        let mut rows = Graph::new();
        rows.set_std_tags(true);
        for n in records.values() {
            rows.insert_node(n.clone()).expect("fits");
        }
        let (mut flipped, _) = Graph::load_nodes(bytes(&records)).expect("loads");
        flipped.load_edges(&[]).expect("no edges");
        assert!(!flipped.std_tags());
        flipped.set_std_tags(true);
        let mut graphs = [&mut bulk, &mut rows, &mut flipped];
        agree(&mut rng, &graphs, &records, "after load");
        for round in 0..100 {
            let what = write(&mut rng, &mut graphs, &mut records);
            seen.insert(what);
            agree(
                &mut rng,
                &graphs,
                &records,
                &format!("seed {seed} round {round}: {what}"),
            );
        }
    }
    seen.remove("nothing");
    assert_eq!(seen.len(), 8, "every kind of write ran: {seen:?}");
}
