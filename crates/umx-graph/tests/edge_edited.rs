//! Edge edited_by_session (ws007 P2, the project manager B, graph-storage-expert's spec): a typed key in a fixed slot
//! after authored_by_session, left out when absent, sparse in memory by edge id, carried by every edge write path.

use serde_json::json;
use umx_core::{Edge, Id, Node, codec};
use umx_graph::Graph;

fn node(id: u128) -> Node {
    Node {
        id: Id(id),
        kind: "problem".into(),
        title: "title".into(),
        status: "open".into(),
        note: String::new(),
        evidence: vec![],
        status_history: vec![],
        created: "now".into(),
        updated: "now".into(),
        created_by: "t".into(),
        locator: None,
        authored_by_session: None,
        edited_by_session: None,
        fields: Default::default(),
    }
}

fn edge(id: u128, edited: Option<Id>) -> Edge {
    Edge {
        id: Id(id),
        kind: "supports".into(),
        from_graph: None,
        from: Id(10),
        to_graph: None,
        to: Id(20),
        created: "now".into(),
        created_by: "t".into(),
        authored_by_session: Some(Id(30)),
        edited_by_session: edited,
        evidence: Vec::new(),
        fields: [("zeta".into(), json!(1))].into(),
    }
}

fn graph(edges: Vec<Edge>) -> Graph {
    Graph::from_records(vec![node(10), node(20)], edges)
}

fn reload(g: &Graph) -> Graph {
    let (mut fresh, _) = Graph::load_nodes(g.encode_nodes()).unwrap();
    fresh.load_edges(&g.encode_edges()).unwrap();
    fresh
}

#[test]
fn the_slot_sits_after_authored_and_round_trips_with_and_without_it() {
    let plain = graph(vec![edge(100, None)]);
    let bytes = plain.encode_edges();
    assert!(!String::from_utf8_lossy(&bytes).contains("edited_by_session"));
    assert_eq!(reload(&plain).encode_edges(), bytes);

    let g = graph(vec![edge(100, Some(Id(31)))]);
    let text = String::from_utf8(g.encode_edges()).unwrap();
    assert!(
        text.contains(&format!(
            "\"created_by\":\"t\",\"authored_by_session\":\"{}\",\"edited_by_session\":\"{}\",\"zeta\":1",
            Id(30),
            Id(31)
        )),
        "{text}"
    );
    // The scanner and the codec agree, and an encode of either gives the same bytes.
    assert_eq!(
        codec::decode_edges(text.as_bytes()).unwrap().records,
        [edge(100, Some(Id(31)))]
    );
    let again = reload(&g);
    assert_eq!(
        again.edge(Id(100)).unwrap().edited_by_session(),
        Some(Id(31))
    );
    assert_eq!(again.encode_edges(), text.as_bytes());
    assert_eq!(
        again.edge(Id(100)).unwrap().field("edited_by_session"),
        Some(json!(Id(31).to_text()))
    );
}

#[test]
fn a_value_that_is_not_an_id_stays_a_raw_field_and_is_never_typed() {
    let g = graph(vec![edge(100, None)]);
    let text = String::from_utf8(g.encode_edges()).unwrap();
    let bad = text.replace(
        "\"zeta\":1",
        "\"edited_by_session\":\"not-an-id\",\"zeta\":1",
    );
    let (mut h, _) = Graph::load_nodes(g.encode_nodes()).unwrap();
    h.load_edges(bad.as_bytes()).unwrap();
    let e = h.edge(Id(100)).unwrap();
    assert_eq!(e.edited_by_session(), None);
    assert_eq!(e.field("edited_by_session"), Some(json!("not-an-id")));
    assert_eq!(e.to_edge().fields["edited_by_session"], json!("not-an-id"));
    // Kept as written: an untouched graph writes it back.
    assert_eq!(String::from_utf8(h.encode_edges()).unwrap(), bad);
}

#[test]
fn an_edit_sets_it_and_delete_or_rollback_restore_it_exactly() {
    let mut g = graph(vec![edge(100, None), edge(101, Some(Id(31)))]);
    let before = g.encode_edges();
    // In place (set_edge): an edit stamps it; another edit replaces it.
    g.insert_edge(edge(100, Some(Id(32))));
    assert_eq!(g.edge(Id(100)).unwrap().edited_by_session(), Some(Id(32)));
    g.insert_edge(edge(100, Some(Id(33))));
    assert_eq!(g.edge(Id(100)).unwrap().edited_by_session(), Some(Id(33)));
    // Removal drops it; re-insert by record brings it back.
    let gone = g.remove_edge(Id(101)).unwrap();
    assert_eq!(gone.edited_by_session, Some(Id(31)));
    assert_eq!(g.provenance_stats().edge_edited_edges, 1);
    g.insert_edge(gone);
    assert_eq!(g.edge(Id(101)).unwrap().edited_by_session(), Some(Id(31)));

    for prepared in [false, true] {
        let mut g = graph(vec![edge(100, None), edge(101, Some(Id(31)))]);
        let mut b = g.batch();
        b.insert_edge(edge(100, Some(Id(34))));
        b.remove_edge(Id(101)).unwrap();
        // An out-of-order (lower) id takes the merge path.
        let mut low = edge(50, Some(Id(35)));
        low.to = Id(10);
        low.from = Id(20);
        b.insert_edge(low);
        if prepared {
            b.prepare().unwrap();
        }
        b.rollback();
        g.check_rows();
        assert_eq!(g.encode_edges(), before, "prepared {prepared}");
        assert_eq!(g.edge(Id(101)).unwrap().edited_by_session(), Some(Id(31)));
        assert_eq!(g.provenance_stats().edge_edited_edges, 1);
    }
}

#[test]
fn a_batch_writes_the_same_bytes_as_the_same_writes_one_by_one() {
    let mut one = graph(vec![edge(100, None), edge(101, Some(Id(31)))]);
    let mut batched = graph(vec![edge(100, None), edge(101, Some(Id(31)))]);
    let mut low = edge(50, Some(Id(36)));
    low.from = Id(20);
    low.to = Id(10);
    let ops = [edge(100, Some(Id(34))), low, edge(102, None)];
    for e in ops.clone() {
        one.insert_edge(e);
    }
    one.remove_edge(Id(101));
    let mut b = batched.batch();
    for e in ops {
        b.insert_edge(e);
    }
    b.remove_edge(Id(101)).unwrap();
    b.commit();
    assert_eq!(batched.encode_edges(), one.encode_edges());
    assert_eq!(
        batched.edge(Id(50)).unwrap().edited_by_session(),
        Some(Id(36))
    );
}

#[test]
fn only_edited_edges_pay_and_the_parts_add_up() {
    let none = graph((100..200).map(|i| edge(i, None)).collect());
    let s = none.provenance_stats();
    assert_eq!(
        (s.edge_edited_bytes, s.edge_edited_ids, s.edge_edited_edges),
        (0, 0, 0)
    );
    let some = graph(
        (100..200)
            .map(|i| edge(i, (i % 10 == 0).then_some(Id(40 + i % 3))))
            .collect(),
    );
    let s = some.provenance_stats();
    assert_eq!(s.edge_edited_edges, 10);
    assert!(s.edge_edited_ids <= 3);
    assert!(s.edge_edited_bytes > 0);
    assert_eq!(
        s.unique_index_bytes
            + s.bitset_bytes
            + s.authored_bytes
            + s.edited_bytes
            + s.edge_authored_bytes
            + s.edge_edited_bytes,
        some.memory_bytes().provenance
    );
}
