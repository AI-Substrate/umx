//! ws004 s09 similar_to lists (workshop 008 section 9): kept current one node at a time, they always equal a full
//! rebuild from the same vectors, through every write that moves handles; ties and the floor are exact.

use std::collections::BTreeMap;
use umx_core::similarity::inverse_length;
use umx_core::vectors::TextHash;
use umx_core::{Id, Node};
use umx_graph::{Graph, NodeIx, SimilarEdge};

const SPACE: &str = "m@8";
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

/// Every list and every reverse list, by node id, so graphs with different handles compare.
/// Each node's list and reverse list as (other id, score).
type Lists = BTreeMap<Id, (Vec<(Id, i32)>, Vec<(Id, i32)>)>;

fn lists(g: &Graph) -> Lists {
    let ids = |l: &[SimilarEdge]| {
        l.iter()
            .map(|e| (g.node_at(e.other).id(), e.score))
            .collect::<Vec<_>>()
    };
    g.nodes()
        .map(|n| {
            (
                n.id(),
                (ids(g.similar_out(n.ix())), ids(g.similar_in(n.ix()))),
            )
        })
        .filter(|(_, (o, i))| !o.is_empty() || !i.is_empty())
        .collect()
}

/// What a full rebuild from the graph's current vectors gives.
fn rebuilt(g: &Graph, k: usize, floor: i32) -> Lists {
    let mut fresh = g.clone();
    match fresh.similar_snapshot(SPACE) {
        Some(snap) => assert!(fresh.apply_similar(snap.compute(k, floor))),
        None => fresh.clear_similar(),
    }
    lists(&fresh)
}

#[test]
fn imported_vectors_and_lists_never_make_provenance_similar() {
    let mut provenance = node(3);
    provenance.kind = "model".into();
    provenance
        .fields
        .insert("prov_key".into(), serde_json::json!("model/test"));
    let mut user_agent = node(2);
    user_agent.kind = "agent".into();
    let mut g = Graph::from_records(vec![node(1), user_agent, provenance], vec![]);
    {
        let mut load = g.load_vectors(SPACE, 8);
        for n in 1..=3 {
            assert_eq!(load.put(id(n), None, &[1; 8], H), umx_graph::Put::Stored);
        }
        load.finish();
    }
    let snapshot = g.similar_snapshot(SPACE).unwrap();
    assert!(g.apply_similar(snapshot.compute(3, 0)));
    let expected = lists(&g);
    assert_eq!(expected.len(), 2, "user-made agent remains content");
    assert!(!expected.contains_key(&id(3)));
    assert!(
        expected
            .values()
            .all(|(out, incoming)| { out.iter().chain(incoming).all(|(n, _)| *n != id(3)) })
    );
    {
        let mut load = g.load_similar(SPACE, 3, 0);
        assert!(!load.put(id(3), id(1), 10_000));
        assert!(!load.put(id(1), id(3), 10_000));
        assert!(load.put(id(1), id(2), 10_000));
        assert!(load.put(id(2), id(1), 10_000));
        load.finish();
    }
    assert_eq!(lists(&g), expected);
    let provenance = g.node_ix(id(3)).unwrap();
    g.refresh_similar(provenance);
    g.finish_similar();
    assert_eq!(lists(&g), expected);
    assert_eq!(g.vector(SPACE, provenance, None), Some(&[1; 8][..]));
}

#[test]
fn membership_flips_invalidate_snapshots_but_preserve_vectors_and_rollback_lists() {
    let mut content = node(1);
    content
        .fields
        .insert("prov_key".into(), serde_json::json!("agent/pij/pij-seat"));
    let mut provenance = content.clone();
    provenance.kind = "agent".into();
    let mut g = Graph::from_records(vec![content.clone(), node(2), node(3)], vec![]);
    for n in 1..=3 {
        g.set_vector(SPACE, id(n), None, &[1; 8], H);
    }
    let snapshot = g.similar_snapshot(SPACE).unwrap();
    assert!(g.apply_similar(snapshot.compute(3, 0)));
    let before = lists(&g);
    let stale = g.similar_snapshot(SPACE).unwrap().compute(3, 0);
    let changed = {
        let mut b = g.batch();
        b.insert_node(provenance.clone()).unwrap();
        b.commit().provenance_changed
    };
    assert_eq!(changed, [id(1)]);
    assert!(
        !g.apply_similar(stale),
        "membership alone invalidates a snapshot"
    );
    let handles: Vec<_> = changed.iter().map(|n| g.node_ix(*n).unwrap()).collect();
    g.refresh_similar_batch(&handles);
    g.finish_similar();
    let after = lists(&g);
    assert_ne!(after, before);
    assert!(!after.contains_key(&id(1)));
    assert!(
        after
            .values()
            .all(|(out, incoming)| { out.iter().chain(incoming).all(|(n, _)| *n != id(1)) })
    );
    assert_eq!(
        g.vector(SPACE, g.node_ix(id(1)).unwrap(), None),
        Some(&[1; 8][..])
    );
    let changed = {
        let mut b = g.batch();
        b.insert_node(content).unwrap();
        b.commit().provenance_changed
    };
    let handles: Vec<_> = changed.iter().map(|n| g.node_ix(*n).unwrap()).collect();
    g.refresh_similar_batch(&handles);
    g.finish_similar();
    assert_eq!(
        lists(&g),
        before,
        "no re-embed when it becomes content again"
    );
    let generation = g.vectors_generation();
    let stale = g.similar_snapshot(SPACE).unwrap().compute(3, 0);
    {
        let mut b = g.batch();
        b.insert_node(provenance).unwrap();
    }
    assert_eq!(lists(&g), before, "rollback never changes cached lists");
    assert!(g.vectors_generation() > generation);
    assert!(!g.apply_similar(stale));
}

/// A seeded vector with few distinct values, so ties at the k-th place happen.
fn vector(rng: &mut impl FnMut(u64) -> u64) -> Vec<i8> {
    (0..8).map(|_| rng(5) as i8 - 2).collect()
}

#[test]
fn lists_kept_current_always_equal_a_full_rebuild() {
    let mut state = 5u64;
    let mut rnd = |n: u64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) % n
    };
    let (k, floor) = (3, 1000);
    let mut nodes: Vec<u128> = (1..=24).map(|n| n * 2).collect();
    let mut g = Graph::from_records(nodes.iter().map(|&n| node(n)).collect(), vec![]);
    g.load_similar(SPACE, k, floor).finish();
    let mut most = 0;
    for step in 0..600u64 {
        let pick = |rnd: &mut dyn FnMut(u64) -> u64, nodes: &[u128]| {
            nodes[rnd(nodes.len() as u64) as usize]
        };
        match rnd(9) {
            0..=3 => {
                // A whole-node vector arrives or changes.
                let n = pick(&mut rnd, &nodes);
                let v = vector(&mut rnd);
                g.set_vector(SPACE, id(n), None, &v, H);
                let ix = g.node_ix(id(n)).unwrap();
                g.refresh_similar(ix);
                g.finish_similar();
            }
            4 => {
                // It goes (pending again).
                let n = pick(&mut rnd, &nodes);
                g.remove_vector(SPACE, id(n), None);
                let ix = g.node_ix(id(n)).unwrap();
                g.refresh_similar(ix);
                g.finish_similar();
            }
            5 => {
                // A new node below the last id: every later handle moves.
                if let Some(n) =
                    (1..*nodes.last().unwrap()).find(|c| !nodes.contains(c) && rnd(3) == 0)
                {
                    g.insert_node(node(n)).unwrap();
                    nodes.push(n);
                    nodes.sort();
                }
            }
            7 => {
                // A batch: several vectors arrive, change or go, then one refresh recomputes each list once.
                let mut batch = Vec::new();
                for _ in 0..2 + rnd(4) {
                    let n = pick(&mut rnd, &nodes);
                    if rnd(4) == 0 {
                        g.remove_vector(SPACE, id(n), None);
                    } else {
                        let v = vector(&mut rnd);
                        g.set_vector(SPACE, id(n), None, &v, H);
                    }
                    batch.push(g.node_ix(id(n)).unwrap());
                }
                g.refresh_similar_batch(&batch);
                g.finish_similar();
            }
            6 if nodes.len() > 6 => {
                // A node goes, vector and all: lists that held it are stale until recomputed.
                let n = pick(&mut rnd, &nodes);
                g.remove_node(id(n)).unwrap();
                nodes.retain(|&x| x != n);
                g.refresh_stale_similar();
                g.finish_similar();
            }
            _ => {
                let n = nodes.last().unwrap() + 1;
                g.insert_node(node(n)).unwrap();
                nodes.push(n);
            }
        }
        assert_eq!(g.similar_stale().count(), 0, "step {step}");
        let now = lists(&g);
        assert_eq!(now, rebuilt(&g, k, floor), "after step {step}");
        most = most.max(now.values().map(|(o, _)| o.len()).sum::<usize>());
    }
    assert!(most > 30, "the run held a fair number of edges: {most}");
}

#[test]
fn a_list_ranks_by_score_then_lower_id_and_keeps_scores_at_the_floor() {
    let mut g = Graph::from_records((1..=5).map(node).collect(), vec![]);
    // Node 1 points along x; 2 and 3 are equal (a tie); 4 is at 45 degrees; 5 is opposite.
    let vs: [[i8; 8]; 5] = [
        [10, 0, 0, 0, 0, 0, 0, 0],
        [10, 0, 0, 0, 0, 0, 0, 0],
        [10, 0, 0, 0, 0, 0, 0, 0],
        [10, 10, 0, 0, 0, 0, 0, 0],
        [-10, 0, 0, 0, 0, 0, 0, 0],
    ];
    for (i, v) in vs.iter().enumerate() {
        g.set_vector(SPACE, id(i as u128 + 1), None, v, H);
    }
    // 1/sqrt(2) rounds to 0.7071: a floor of exactly 0.7071 keeps node 4.
    let d = umx_core::similarity::dot(&vs[3], &vs[0]);
    let at_floor =
        umx_core::similar::score_units(d, inverse_length(&vs[3]), inverse_length(&vs[0]));
    assert_eq!(at_floor, 7071);
    let snap = g.similar_snapshot(SPACE).unwrap();
    assert!(g.apply_similar(snap.compute(2, 7071)));
    let out = |g: &Graph, n: u128| {
        g.similar_out(g.node_ix(id(n)).unwrap())
            .iter()
            .map(|e| (g.node_at(e.other).id(), e.score))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        out(&g, 1),
        [(id(2), 10_000), (id(3), 10_000)],
        "a tie at the k-th place goes to the lower id"
    );
    assert_eq!(out(&g, 4), [(id(1), 7071), (id(2), 7071)]);
    assert!(out(&g, 5).is_empty(), "nothing at or above the floor");
    let snap = g.similar_snapshot(SPACE).unwrap();
    assert!(g.apply_similar(snap.compute(2, 7072)));
    assert!(out(&g, 4).is_empty(), "just below the floor is out");
    let into_1: Vec<NodeIx> = g
        .similar_in(g.node_ix(id(1)).unwrap())
        .iter()
        .map(|e| e.other)
        .collect();
    assert_eq!(
        into_1,
        [g.node_ix(id(2)).unwrap(), g.node_ix(id(3)).unwrap()]
    );
}

#[test]
fn stored_lines_load_sorted_by_score_and_a_moved_snapshot_is_not_applied() {
    let mut g = Graph::from_records((1..=4).map(node).collect(), vec![]);
    let mut load = g.load_similar(SPACE, 2, 0);
    // File order is (node, other), not score.
    assert!(load.put(id(1), id(2), 5000));
    assert!(load.put(id(1), id(3), 9000));
    assert!(
        !load.put(id(1), id(4), 1000),
        "k = 2: a third line is a stale file's"
    );
    assert!(!load.put(id(1), id(1), 9999), "never itself");
    assert!(!load.put(id(1), id(77), 9999), "not a node");
    assert!(load.put(id(4), id(1), 6000));
    load.finish();
    let out1: Vec<i32> = g
        .similar_out(g.node_ix(id(1)).unwrap())
        .iter()
        .map(|e| e.score)
        .collect();
    assert_eq!(out1, [9000, 5000]);
    let in1: Vec<(NodeIx, i32)> = g
        .similar_in(g.node_ix(id(1)).unwrap())
        .iter()
        .map(|e| (e.other, e.score))
        .collect();
    assert_eq!(in1, [(g.node_ix(id(4)).unwrap(), 6000)]);
    assert!(g.memory_bytes().similarity > 0);

    g.set_vector(SPACE, id(1), None, &[1; 8], H);
    let snap = g.similar_snapshot(SPACE).unwrap();
    g.set_vector(SPACE, id(2), None, &[2; 8], H);
    assert!(
        !g.apply_similar(snap.compute(2, 0)),
        "the blocks moved while it computed"
    );
    assert_eq!(
        g.similar_out(g.node_ix(id(1)).unwrap()).len(),
        2,
        "nothing changed"
    );
}
