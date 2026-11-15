//! The key indexes (locators, entity keys, provenance keys) built in bulk at load against the same rows inserted
//! one by one, and both against a model made from the records (ws003 s19, graph-storage's differential). Every
//! round, both graphs take the same write (an edit, an add at the end or in the middle, which renumbers, a removal
//! of the last or a middle row, a batch rolled back or committed) and every lookup, holder pair and duplicate list
//! must match the model, with each index's handle order in step with its entries.

use crate::Graph;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use serde_json::{Value, json};
use umx_core::entity::{index_key, stored_keys};
use umx_core::provenance::is_provenance_key;
use umx_core::{Id, Locator, Node, codec};

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

    fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }
}

const KINDS: [&str; 4] = ["person", "team", "agent", "note"];
const LOCATORS: [&str; 6] = [
    "src/a0.rs",
    "src/a1.rs",
    "notes/b.md#L3",
    "notes/b.md#L4",
    "https://docs.example.org/x",
    "src/a0.rs#L9",
];
/// Entity keys as written; one is not in stored form and stores as another's key.
const ENTITY: [&str; 6] = [
    "email:u0@example.org",
    "email:u1@example.org",
    "email:U1@Example.org",
    "domain:host2.example.net",
    "id:k3",
    "email:u4@example.org",
];
const PROV: [&str; 4] = [
    "agent/pij/s0",
    "agent/pij/s1",
    "agent/pij/s2",
    "agent/pij/s3",
];

fn node(rng: &mut Rng, id: u128) -> Node {
    let kind = KINDS[rng.below(KINDS.len())];
    let mut fields = BTreeMap::new();
    if rng.below(2) == 0 {
        let keys: Vec<Value> = (0..1 + rng.below(3))
            .map(|_| json!(ENTITY[rng.below(ENTITY.len())]))
            .collect();
        fields.insert("entity_key".into(), Value::Array(keys));
    }
    if kind == "agent" && rng.below(5) < 3 {
        fields.insert("prov_key".into(), json!(PROV[rng.below(PROV.len())]));
    }
    Node {
        id: Id(id),
        kind: kind.into(),
        title: format!("n{id}"),
        status: "open".into(),
        note: String::new(),
        evidence: vec![],
        status_history: vec![],
        created: "2026-11-12T00:00:00Z".into(),
        updated: "2026-11-12T00:00:00Z".into(),
        created_by: "t".into(),
        locator: (rng.below(5) < 2)
            .then(|| Locator::parse(LOCATORS[rng.below(LOCATORS.len())]).expect("parses")),
        authored_by_session: None,
        edited_by_session: None,
        fields,
    }
}

/// Every answer the three indexes give for the pools, and their duplicate lists.
#[derive(Debug, PartialEq, Eq)]
struct Answers {
    locator: Vec<Option<Id>>,
    entity: Vec<(Option<Id>, bool)>,
    prov: Vec<Option<Id>>,
    duplicate_locators: Vec<(Id, Id)>,
    duplicate_entity_keys: Vec<(Id, Id, String)>,
    duplicate_prov_keys: Vec<(Id, Id)>,
}

/// The pool's entity keys in stored form, distinct.
fn stored_pool() -> BTreeSet<String> {
    ENTITY
        .iter()
        .flat_map(|k| stored_keys(&json!([k])))
        .collect()
}

fn answers(g: &Graph) -> Answers {
    g.locators.check();
    g.entities.check();
    g.provenance.keys.check();
    let mut entity = Vec::new();
    for kind in KINDS {
        for key in stored_pool() {
            let (first, more) = g.entity_holders(kind, &key);
            assert_eq!(
                g.entity_node(kind, &key).map(|n| n.id()),
                first.map(|n| n.id())
            );
            entity.push((first.map(|n| n.id()), more));
        }
    }
    Answers {
        locator: LOCATORS
            .iter()
            .map(|t| {
                g.by_locator(&Locator::parse(t).expect("parses"))
                    .map(|n| n.id())
            })
            .collect(),
        entity,
        prov: PROV
            .iter()
            .map(|k| g.provenance_node(k).map(|n| n.id()))
            .collect(),
        duplicate_locators: g.duplicate_locators().map(|d| (d.node, d.in_use)).collect(),
        duplicate_entity_keys: g
            .duplicate_entity_keys()
            .map(|d| (d.node, d.in_use, d.key))
            .collect(),
        duplicate_prov_keys: g
            .duplicate_prov_keys()
            .map(|d| (d.node, d.in_use))
            .collect(),
    }
}

/// Holders per key in key byte order, each list in id order: the first is in use, the rest are duplicates of it.
type Holders = BTreeMap<Vec<u8>, Vec<Id>>;

fn duplicates(holders: &Holders) -> Vec<(Id, Id)> {
    holders
        .values()
        .flat_map(|ids| ids[1..].iter().map(|&l| (l, ids[0])))
        .collect()
}

/// The answers from the records alone: the lowest id holding a key is in use.
fn model(records: &BTreeMap<Id, Node>) -> Answers {
    let (mut loc, mut ent, mut prov) = (Holders::new(), Holders::new(), Holders::new());
    for (id, n) in records {
        if let Some(l) = &n.locator {
            loc.entry(l.key()).or_default().push(*id);
        }
        if let Some(v) = n.fields.get("entity_key") {
            let keys: BTreeSet<String> = stored_keys(v).into_iter().collect();
            for k in keys {
                ent.entry(index_key(&n.kind, &k)).or_default().push(*id);
            }
        }
        let p = n.fields.get("prov_key").and_then(Value::as_str);
        if is_provenance_key(&n.kind, p) {
            prov.entry(p.expect("a key").as_bytes().to_vec())
                .or_default()
                .push(*id);
        }
    }
    let first = |h: &Holders, key: &[u8]| h.get(key).map(|ids| ids[0]);
    let mut entity = Vec::new();
    for kind in KINDS {
        for key in stored_pool() {
            let ids = ent.get(&index_key(kind, &key));
            entity.push((ids.map(|i| i[0]), ids.is_some_and(|i| i.len() > 1)));
        }
    }
    let duplicate_entity_keys = ent
        .iter()
        .flat_map(|(key, ids)| {
            let text = String::from_utf8_lossy(
                &key[key.iter().position(|b| *b == 0).expect("kind\\0key") + 1..],
            )
            .into_owned();
            ids[1..].iter().map(move |&l| (l, ids[0], text.clone()))
        })
        .collect();
    Answers {
        locator: LOCATORS
            .iter()
            .map(|t| first(&loc, &Locator::parse(t).expect("parses").key()))
            .collect(),
        entity,
        prov: PROV.iter().map(|k| first(&prov, k.as_bytes())).collect(),
        duplicate_locators: duplicates(&loc),
        duplicate_entity_keys,
        duplicate_prov_keys: duplicates(&prov),
    }
}

fn bytes(records: &BTreeMap<Id, Node>) -> Vec<u8> {
    codec::encode_nodes(records.values())
}

/// Both graphs and a fresh load of the records (sorted file, then shuffled with one repeated line, which takes the
/// unsorted load path) answer as the model does.
fn agree(rng: &mut Rng, bulk: &Graph, rows: &Graph, records: &BTreeMap<Id, Node>, at: &str) {
    let want = model(records);
    assert_eq!(answers(bulk), want, "bulk-loaded graph, {at}");
    assert_eq!(answers(rows), want, "row-by-row graph, {at}");
    bulk.check_rows();
    rows.check_rows();
    assert_eq!(bulk.encode_nodes(), bytes(records), "{at}");
    assert_eq!(rows.encode_nodes(), bytes(records), "{at}");
    let (fresh, load) = Graph::load_nodes(bytes(records)).expect("loads");
    assert_eq!(answers(&fresh), want, "fresh load, {at}");
    assert_eq!(
        load.duplicate_prov_keys,
        fresh.duplicate_prov_keys().collect::<Vec<_>>()
    );
    if records.len() > 1 {
        let mut lines: Vec<&[u8]> = Vec::new();
        let all = bytes(records);
        lines.extend(all.split_inclusive(|b| *b == b'\n'));
        for i in (1..lines.len()).rev() {
            lines.swap(i, rng.below(i + 1));
        }
        // An earlier line of a repeated id: an older form of one record, kept and never indexed. The repeat takes
        // the load's unsorted path, which lays the image out and derives the rows again.
        let (&id, _) = records
            .iter()
            .nth(rng.below(records.len()))
            .expect("a record");
        let mut older = node(rng, id.0);
        older.locator = Some(Locator::parse(LOCATORS[0]).expect("parses"));
        let older = codec::encode_nodes([&older]);
        let mut file = older.clone();
        file.extend(lines.concat());
        let (shuffled, load) = Graph::load_nodes(file).expect("loads");
        assert_eq!(load.repeats.len(), 1, "{at}");
        assert_eq!(answers(&shuffled), want, "shuffled load, {at}");
    }
}

/// A random id between the lowest and the highest that no record holds, if any.
fn middle_id(rng: &mut Rng, records: &BTreeMap<Id, Node>) -> Option<u128> {
    let (lo, hi) = (records.keys().next()?.0, records.keys().last()?.0);
    (0..8)
        .map(|_| lo + 1 + rng.below((hi - lo).max(1) as usize) as u128)
        .find(|&id| id < hi && !records.contains_key(&Id(id)))
}

/// A write to both graphs and the model: an edit, an add at the end or in the middle, or a removal.
fn write(rng: &mut Rng, g: &mut [&mut Graph; 2], records: &mut BTreeMap<Id, Node>) -> &'static str {
    let top = records.keys().last().map_or(0, |i| i.0);
    let pick = |rng: &mut Rng, records: &BTreeMap<Id, Node>| {
        records.keys().nth(rng.below(records.len().max(1))).copied()
    };
    match rng.below(5) {
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
            "edit keeping keys"
        }
        3 => {
            let Some(&last) = records.keys().last() else {
                return "nothing";
            };
            for g in g.iter_mut() {
                assert!(g.remove_node(last).is_some());
            }
            records.remove(&last);
            "remove the last"
        }
        _ => {
            let Some(e) = pick(rng, records) else {
                return "nothing";
            };
            for g in g.iter_mut() {
                assert!(g.remove_node(e).is_some());
            }
            records.remove(&e);
            "remove one"
        }
    }
}

/// A batch of adds (end and middle) and edits on both graphs, checked after every write, then rolled back,
/// dropped or committed.
fn batch(rng: &mut Rng, g: &mut [&mut Graph; 2], records: &mut BTreeMap<Id, Node>) -> &'static str {
    let mut after = records.clone();
    let mut nodes = Vec::new();
    for _ in 0..1 + rng.below(6) {
        let top = after.keys().last().map_or(0, |i| i.0);
        let id = match (rng.below(3), middle_id(rng, &after)) {
            (0, Some(m)) => m,
            (1, _) if !after.is_empty() => after.keys().nth(rng.below(after.len())).expect("one").0,
            _ => top + 4,
        };
        let n = node(rng, id);
        after.insert(n.id, n.clone());
        nodes.push((n, model(&after)));
    }
    let end = rng.below(3);
    for g in g.iter_mut() {
        let mut b = g.batch();
        for (n, want) in &nodes {
            b.insert_node(n.clone()).expect("fits");
            assert_eq!(&answers(b.graph()), want, "inside a batch");
        }
        match end {
            0 => b.rollback(),
            1 => drop(b),
            _ => {
                b.commit();
            }
        }
    }
    if end == 2 {
        *records = after;
        "batch committed"
    } else {
        "batch rolled back"
    }
}

#[test]
fn bulk_built_indexes_answer_as_rows_inserted_one_by_one_through_every_write() {
    let mut seen = BTreeSet::new();
    for seed in 0..6 {
        let mut rng = Rng(seed);
        let mut records = BTreeMap::new();
        for i in 1..=30 + rng.below(30) as u128 {
            let n = node(&mut rng, i * 4);
            records.insert(n.id, n);
        }
        let (mut bulk, load) = Graph::load_nodes(bytes(&records)).expect("loads");
        assert!(
            load.unsorted.is_none(),
            "the sorted file takes the bulk pass"
        );
        bulk.load_edges(&[]).expect("no edges");
        let mut rows = Graph::new();
        for n in records.values() {
            rows.insert_node(n.clone()).expect("fits");
        }
        agree(&mut rng, &bulk, &rows, &records, "after load");
        let want = model(&records);
        assert!(
            !want.duplicate_locators.is_empty() && !want.duplicate_entity_keys.is_empty(),
            "the pools make duplicates at load"
        );
        for round in 0..120 {
            let what = if rng.one_in(4) {
                batch(&mut rng, &mut [&mut bulk, &mut rows], &mut records)
            } else {
                write(&mut rng, &mut [&mut bulk, &mut rows], &mut records)
            };
            seen.insert(what);
            agree(
                &mut rng,
                &bulk,
                &rows,
                &records,
                &format!("seed {seed} round {round}: {what}"),
            );
        }
    }
    seen.remove("nothing");
    assert_eq!(seen.len(), 8, "every kind of write ran: {seen:?}");
}
