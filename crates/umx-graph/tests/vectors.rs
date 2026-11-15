//! s13 vector blocks (workshop 008 section 2.5; ws004 s07 acceptance sections 1 and 4): what the blocks hold after a
//! load and after every kind of write, against a plain map of what was put.

use std::collections::BTreeMap;
use umx_core::similarity::inverse_length;
use umx_core::vectors::TextHash;
use umx_core::{Edge, Id, Node};
use umx_graph::{Graph, NodeIx, Put};

/// Every vector here comes from the same text; the hash only rides along.
const H: TextHash = TextHash([7; 16]);

fn id(n: u128) -> Id {
    Id((0x0194_0000_0000_u128 << 80) | n)
}

fn node(n: u128) -> Node {
    Node {
        id: id(n),
        kind: "problem".into(),
        title: format!("Node {n}"),
        status: "open".into(),
        note: String::new(),
        evidence: vec![],
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

fn edge(n: u128, from: u128, to: u128) -> Edge {
    Edge {
        id: id(n),
        kind: "causes".into(),
        from_graph: None,
        from: id(from),
        to_graph: None,
        to: id(to),
        created: "2026-11-10T03:00:00Z".into(),
        created_by: "tester".into(),
        authored_by_session: None,
        edited_by_session: None,
        evidence: Vec::new(),
        fields: BTreeMap::new(),
    }
}

fn v(seed: u64, dims: usize) -> Vec<i8> {
    (0..dims as u64)
        .map(|i| (seed.wrapping_mul(31).wrapping_add(i * 7) % 255) as i64 as i8)
        .collect()
}

type Key = (String, Id, Option<String>);

/// Every vector the graph answers, by (space, node id, field), plus the block invariants.
fn held(g: &Graph) -> BTreeMap<Key, Vec<i8>> {
    let mut out = BTreeMap::new();
    let spaces: Vec<String> = g.vector_spaces().map(str::to_owned).collect();
    assert!(
        spaces.windows(2).all(|w| w[0] < w[1]),
        "spaces in name order"
    );
    for space in spaces {
        let mut any = false;
        if let Some(b) = g.whole_block(&space) {
            assert!(b.rows() <= g.node_bound(), "no row past the last node");
            assert_eq!(b.values().len(), b.rows() * b.dims());
            for row in 0..b.rows() {
                let values = &b.values()[row * b.dims()..(row + 1) * b.dims()];
                assert_eq!(
                    b.hash(row),
                    b.has(row).then_some(H),
                    "a row keeps its vector's text hash"
                );
                if !b.has(row) {
                    assert!(values.iter().all(|&x| x == 0), "an empty row is zeroed");
                    assert_eq!(b.inverse_lengths()[row], 0.0);
                    continue;
                }
                any = true;
                let ix = b.node(row as u32);
                assert_eq!(b.inverse_lengths()[row], inverse_length(values));
                assert_eq!(g.vector(&space, ix, None), Some(values));
                out.insert((space.clone(), g.node_at(ix).id(), None), values.to_vec());
            }
        }
        let mut names = vec![];
        for f in g.field_blocks(&space) {
            names.push(f.field().to_owned());
            assert!(f.rows() > 0, "an empty field block is dropped");
            let nodes: Vec<NodeIx> = (0..f.rows() as u32).map(|r| f.node(r)).collect();
            assert!(
                nodes.windows(2).all(|w| w[0] < w[1]),
                "rows sorted by handle, no repeats"
            );
            for (row, ix) in nodes.into_iter().enumerate() {
                any = true;
                let values = &f.values()[row * f.dims()..(row + 1) * f.dims()];
                assert_eq!(f.inverse_lengths()[row], inverse_length(values));
                assert_eq!(g.vector(&space, ix, Some(f.field())), Some(values));
                out.insert(
                    (
                        space.clone(),
                        g.node_at(ix).id(),
                        Some(f.field().to_owned()),
                    ),
                    values.to_vec(),
                );
            }
        }
        assert!(
            names.windows(2).all(|w| w[0] < w[1]),
            "fields in name order"
        );
        assert!(any, "a space with no vector is dropped");
    }
    out
}

#[test]
fn a_load_sorts_rows_keeps_the_first_repeat_and_refuses_orphans_and_wrong_lengths() {
    let mut g = Graph::from_records((1..=6).map(node).collect(), vec![edge(100, 1, 2)]);
    let nodes_before = g.encode_nodes();
    let edges_before = g.encode_edges();
    let mut load = g.load_vectors("model@4", 4);
    // Shard order is not handle order.
    for n in [5u128, 2, 6, 1, 3] {
        assert_eq!(
            load.put(id(n), Some("title"), &v(n as u64, 4), H),
            Put::Stored
        );
        assert_eq!(load.put(id(n), None, &v(n as u64 + 50, 4), H), Put::Stored);
    }
    assert_eq!(
        load.put(id(2), Some("title"), &v(99, 4), H),
        Put::Stored,
        "a repeat"
    );
    assert_eq!(load.put(id(5), None, &v(98, 4), H), Put::Stored, "a repeat");
    assert_eq!(load.put(id(4), Some("note"), &v(4, 4), H), Put::Stored);
    assert_eq!(load.put(id(77), Some("title"), &v(1, 4), H), Put::Orphan);
    assert_eq!(load.put(id(4), Some("title"), &v(1, 3), H), Put::WrongDims);
    assert_eq!(load.finish(), 11);

    let got = held(&g);
    let key = |n: u128, f: Option<&str>| ("model@4".to_owned(), id(n), f.map(str::to_owned));
    assert_eq!(got.len(), 11);
    assert_eq!(
        got[&key(2, Some("title"))],
        v(2, 4),
        "the first of a repeat is kept"
    );
    assert_eq!(got[&key(5, None)], v(55, 4));
    assert_eq!(got[&key(4, Some("note"))], v(4, 4));
    assert!(
        !got.contains_key(&key(4, None)),
        "node 4 has no whole-node vector"
    );
    let whole = g.whole_block("model@4").unwrap();
    assert_eq!((whole.rows(), whole.count()), (6, 5));
    assert_eq!(
        g.field_blocks("model@4")
            .map(|f| (f.field().to_owned(), f.rows()))
            .collect::<Vec<_>>(),
        [("note".to_owned(), 1), ("title".to_owned(), 5)]
    );
    // No vector reaches the graph files.
    assert_eq!(g.encode_nodes(), nodes_before);
    assert_eq!(g.encode_edges(), edges_before);

    // Loading the space again replaces it; a load with nothing in it leaves no space.
    let mut load = g.load_vectors("model@4", 4);
    load.put(id(1), None, &v(7, 4), H);
    drop(load);
    assert_eq!(held(&g).len(), 1, "dropping a load finishes it");
    let mut load = g.load_vectors("model@4", 4);
    load.put(id(1), Some("title"), &v(7, 4), H);
    load.abandon();
    assert_eq!(
        held(&g).len(),
        0,
        "an abandoned load leaves the space empty"
    );
    assert_eq!(g.load_vectors("model@4", 4).finish(), 0);
    assert_eq!(g.vector_spaces().count(), 0);
    assert_eq!(g.memory_bytes().vectors, 0);
}

/// A seeded run of every write that touches vectors or renumbers handles, checked against a plain map after each.
#[test]
fn every_write_keeps_the_blocks_equal_to_what_was_put() {
    let mut state = 11u64;
    let mut rnd = |n: u64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) % n
    };
    let spaces = [("a@4", 4usize), ("b@2", 2)];
    let fields = [None, Some("note"), Some("title"), Some("x")];
    // Even ids leave room for a smaller new id, which renumbers every later handle.
    let mut nodes: Vec<u128> = (1..=20).map(|n| n * 2).collect();
    let mut g = Graph::from_records(nodes.iter().map(|&n| node(n)).collect(), vec![]);
    let mut model: BTreeMap<Key, Vec<i8>> = BTreeMap::new();
    let mut next_edge = 1_000_000u128;
    let mut most = 0;
    for step in 0..1500u64 {
        let pick = |rnd: &mut dyn FnMut(u64) -> u64, nodes: &[u128]| {
            nodes[rnd(nodes.len() as u64) as usize]
        };
        match rnd(10) {
            0..=3 => {
                let (space, dims) = spaces[rnd(2) as usize];
                let field = fields[rnd(4) as usize];
                let n = pick(&mut rnd, &nodes);
                let values = v(step, dims);
                assert_eq!(g.set_vector(space, id(n), field, &values, H), Put::Stored);
                model.insert((space.into(), id(n), field.map(Into::into)), values);
                if model.keys().any(|k| k.0 == space) {
                    assert_eq!(
                        g.set_vector(space, id(n), field, &v(step, dims + 1), H),
                        Put::WrongDims
                    );
                }
                assert_eq!(
                    g.set_vector(space, id(1 << 40), field, &v(step, dims), H),
                    Put::Orphan
                );
            }
            4 => {
                let (space, _) = spaces[rnd(2) as usize];
                let field = fields[rnd(4) as usize];
                let n = pick(&mut rnd, &nodes);
                let had = model
                    .remove(&(space.into(), id(n), field.map(Into::into)))
                    .is_some();
                assert_eq!(g.remove_vector(space, id(n), field), had);
            }
            5 => {
                // A new node: usually the largest id (appended), sometimes a gap below (renumbers).
                let n = if rnd(3) == 0 {
                    (1..*nodes.last().unwrap()).find(|c| !nodes.contains(c) && rnd(4) == 0)
                } else {
                    Some(nodes.last().unwrap() + 1 + u128::from(rnd(3)))
                };
                if let Some(n) = n {
                    assert!(g.insert_node(node(n)).unwrap().is_none());
                    nodes.push(n);
                    nodes.sort();
                }
            }
            6 if nodes.len() > 3 => {
                // Remove the last node, or one in the middle (renumbers).
                let n = if rnd(2) == 0 {
                    *nodes.last().unwrap()
                } else {
                    pick(&mut rnd, &nodes)
                };
                assert!(
                    g.remove_node(id(n)).is_some(),
                    "no edges touch any node here"
                );
                nodes.retain(|&x| x != n);
                model.retain(|k, _| k.1 != id(n));
            }
            7 => {
                // An edge with a small id rebuilds through the load path with the same node handles; drop it again.
                next_edge -= 1;
                let (a, b) = (pick(&mut rnd, &nodes), pick(&mut rnd, &nodes));
                g.insert_edge(edge(1 << 41, a, b));
                g.insert_edge(edge(next_edge, a, b));
                g.remove_edge(id(next_edge));
                g.remove_edge(id(1 << 41));
            }
            8 => {
                // Replaying a space's vectors through a load, shuffled, with a stale repeat put after.
                let (space, dims) = spaces[rnd(2) as usize];
                let mut lines: Vec<(Key, Vec<i8>)> = model
                    .iter()
                    .filter(|(k, _)| k.0 == space)
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                for i in (1..lines.len()).rev() {
                    lines.swap(i, rnd(i as u64 + 1) as usize);
                }
                let mut load = g.load_vectors(space, dims);
                for (k, values) in &lines {
                    assert_eq!(load.put(k.1, k.2.as_deref(), values, H), Put::Stored);
                }
                if let Some((k, _)) = lines.first() {
                    load.put(k.1, k.2.as_deref(), &v(9999, dims), H);
                }
                assert_eq!(load.finish(), lines.len());
            }
            _ => {
                // A node edit that keeps its id: its vectors stay (stale until re-embedded).
                let n = pick(&mut rnd, &nodes);
                let mut changed = node(n);
                changed.title = format!("Edited at {step}");
                assert!(g.insert_node(changed).unwrap().is_some());
            }
        }
        assert_eq!(held(&g), model, "after step {step}");
        most = most.max(model.len());
    }
    assert!(
        most > 40,
        "the run held a fair number of vectors at once: {most}"
    );
}
