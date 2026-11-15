//! ws006 S5b (req-124): the edge `evidence` slot, per graph-storage-expert's layout. It sits after
//! authored_by_session and before custom fields, is left out when empty, is cold and sparse in memory, and every edge
//! path carries it. A value that is not an evidence list stays a raw custom field.

use serde_json::{Value, json};
use umx_core::{Edge, Id, Node};
use umx_graph::Graph;

fn node(id: u128) -> Node {
    Node {
        id: Id(id),
        kind: "problem".into(),
        title: format!("n{id}"),
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

fn pointer(source: &str) -> Value {
    json!({ "source": source, "selector": { "lines": [10, 20] }, "kind": "document", "fidelity": "exact", "hash": null })
}

fn edge(id: u128, evidence: Vec<Value>) -> Edge {
    Edge {
        id: Id(id),
        kind: "feeds".into(),
        from_graph: None,
        from: Id(1),
        to_graph: None,
        to: Id(2),
        created: "now".into(),
        created_by: "t".into(),
        authored_by_session: Some(Id(9)),
        edited_by_session: None,
        evidence,
        fields: [("zz".into(), json!(1)), ("aa".into(), json!("x"))].into(),
    }
}

fn graph(edges: Vec<Edge>) -> Graph {
    Graph::from_records(vec![node(1), node(2), node(9)], edges)
}

fn reload(g: &Graph) -> Graph {
    let (mut loaded, _) = Graph::load_nodes(g.encode_nodes()).unwrap();
    loaded.load_edges(&g.encode_edges()).unwrap();
    loaded
}

fn line(g: &Graph) -> String {
    String::from_utf8(g.encode_edges()).unwrap()
}

#[test]
fn the_slot_round_trips_byte_for_byte_with_and_without_evidence_in_its_fixed_place() {
    let with = graph(vec![edge(30, vec![pointer("root:repo/src/x.rs")])]);
    let without = graph(vec![edge(31, Vec::new())]);
    for g in [&with, &without] {
        let again = reload(g);
        assert_eq!(again.encode_edges(), g.encode_edges());
        for e in g.edges() {
            assert_eq!(again.edge(e.id()).unwrap().to_edge(), e.to_edge());
        }
    }
    let expected = format!(
        "{{\"id\":\"{}\",\"type\":\"feeds\",\"from\":\"{}\",\"to\":\"{}\",\"created\":\"now\",\"created_by\":\"t\",\
\"authored_by_session\":\"{}\",\"evidence\":[{{\"fidelity\":\"exact\",\"hash\":null,\"kind\":\"document\",\
\"selector\":{{\"lines\":[10,20]}},\"source\":\"root:repo/src/x.rs\"}}],\"aa\":\"x\",\"zz\":1}}\n",
        Id(30),
        Id(1),
        Id(2),
        Id(9)
    );
    assert_eq!(line(&with), expected);
    assert!(!line(&without).contains("evidence"));
    // The serde path (umx-core's codec, used for lines the scanner leaves to it) writes the same bytes.
    assert_eq!(
        umx_core::codec::encode_edges(&[with.edge(Id(30)).unwrap().to_edge()]),
        expected.as_bytes()
    );
    assert_eq!(with.edge(Id(30)).unwrap().evidence().len(), 1);
}

#[test]
fn an_old_shape_line_is_read_into_the_slot_and_an_invalid_value_stays_a_raw_field() {
    // An older umx wrote the slot as a custom field, in name order among the others.
    let p = pointer("notes.md");
    let old = format!(
        "{{\"id\":\"{}\",\"type\":\"feeds\",\"from\":\"{}\",\"to\":\"{}\",\"created\":\"now\",\"created_by\":\"t\",\
\"aa\":\"x\",\"evidence\":[{p}],\"zz\":1}}\n",
        Id(30),
        Id(1),
        Id(2)
    );
    let (mut g, _) = Graph::load_nodes(graph(vec![]).encode_nodes()).unwrap();
    g.load_edges(old.as_bytes()).unwrap();
    let e = g.edge(Id(30)).unwrap().to_edge();
    assert_eq!(e.evidence, vec![p.clone()]);
    assert!(!e.fields.contains_key("evidence"));
    assert_eq!(g.edges_with_invalid_evidence().count(), 0);
    assert_eq!(reload(&g).edge(Id(30)).unwrap().to_edge(), e);

    // Prose under `evidence`, an empty list, or a list that is not pointers: kept as written, never read as evidence.
    for value in [
        json!("from the call on Tuesday"),
        json!([]),
        json!([1, 2]),
        json!([{ "note": "x" }]),
    ] {
        let raw = format!(
            "{{\"id\":\"{}\",\"type\":\"feeds\",\"from\":\"{}\",\"to\":\"{}\",\"created\":\"now\",\"created_by\":\"t\",\
\"evidence\":{value}}}\n",
            Id(30),
            Id(1),
            Id(2)
        );
        let (mut g, _) = Graph::load_nodes(graph(vec![]).encode_nodes()).unwrap();
        g.load_edges(raw.as_bytes()).unwrap();
        let e = g.edge(Id(30)).unwrap().to_edge();
        assert!(e.evidence.is_empty(), "{value}");
        assert_eq!(e.fields.get("evidence"), Some(&value));
        assert_eq!(
            g.edges_with_invalid_evidence().collect::<Vec<_>>(),
            [Id(30)]
        );
        assert_eq!(g.encode_edges(), raw.as_bytes(), "{value}");
        let decoded = umx_core::codec::decode_edges(raw.as_bytes()).unwrap();
        assert_eq!(decoded.records, [e]);
    }
}

#[test]
fn an_update_keeps_the_slot_and_delete_or_rollback_restore_it() {
    let ev = vec![pointer("root:repo/src/x.rs"), pointer("notes.md")];
    let mut g = graph(vec![edge(30, ev.clone())]);
    let before = g.encode_edges();

    // An update that changes something else keeps the evidence it carries.
    let mut changed = g.edge(Id(30)).unwrap().to_edge();
    changed.fields.insert("weight".into(), json!(3));
    g.insert_edge(changed);
    assert_eq!(g.edge(Id(30)).unwrap().evidence(), ev);
    assert_eq!(reload(&g).edge(Id(30)).unwrap().evidence(), ev);

    // Removal gives the whole record back, evidence included; putting it back restores the bytes.
    let mut g = graph(vec![edge(30, ev.clone())]);
    let removed = g.remove_edge(Id(30)).unwrap();
    assert_eq!(removed.evidence, ev);
    assert!(!line(&g).contains("evidence"));
    g.insert_edge(removed);
    assert_eq!(g.encode_edges(), before);

    // A batch that drops or replaces the evidence, rolled back before or after prepare, leaves it as it was.
    for prepare in [false, true] {
        let mut g = graph(vec![edge(30, ev.clone())]);
        {
            let mut b = g.batch();
            let mut e = b.graph().edge(Id(30)).unwrap().to_edge();
            e.evidence = vec![pointer("other.md")];
            b.insert_edge(e);
            let mut e = edge(31, Vec::new());
            e.evidence = vec![pointer("new.md")];
            b.insert_edge(e);
            if prepare {
                b.prepare().unwrap();
            }
            b.rollback();
        }
        assert_eq!(g.encode_edges(), before, "prepare: {prepare}");
        assert_eq!(g.edge(Id(30)).unwrap().evidence(), ev);
    }
}

#[test]
fn a_batch_gives_the_same_bytes_as_one_write_at_a_time() {
    let edges = [
        edge(30, vec![pointer("a.md")]),
        edge(31, Vec::new()),
        edge(32, vec![pointer("b.md"), pointer("c.md")]),
    ];
    let mut one = graph(vec![]);
    for e in edges.clone() {
        one.insert_edge(e);
    }
    let mut many = graph(vec![]);
    {
        let mut b = many.batch();
        for e in edges {
            b.insert_edge(e);
        }
        b.commit();
    }
    assert_eq!(many.encode_edges(), one.encode_edges());
    assert_eq!(reload(&many).encode_edges(), one.encode_edges());
}

#[test]
fn only_edges_with_evidence_pay_for_it_and_it_is_counted() {
    let without = graph(vec![edge(30, Vec::new()), edge(31, Vec::new())]);
    let with = graph(vec![
        edge(30, Vec::new()),
        edge(31, vec![pointer("root:repo/src/x.rs")]),
    ]);
    let slot = umx_graph_slot_bytes(&with) - umx_graph_slot_bytes(&without);
    assert!(
        slot >= line(&with).len() - line(&without).len(),
        "the slot's bytes are counted: {slot}"
    );
}

fn umx_graph_slot_bytes(g: &Graph) -> usize {
    g.memory_bytes().cold
}

#[test]
fn a_repeated_evidence_key_is_decided_by_the_codec_whatever_its_values() {
    let list = pointer("a.md");
    let other = pointer("b.md");
    for (first, second) in [
        (list.clone(), other.clone()),
        (list.clone(), json!("prose")),
        (json!("prose"), list.clone()),
        (json!("prose"), json!("more")),
    ] {
        let line = format!(
            "{{\"id\":\"{}\",\"type\":\"feeds\",\"from\":\"{}\",\"to\":\"{}\",\"created\":\"now\",\"created_by\":\"t\",\
\"evidence\":[{first}],\"evidence\":[{second}]}}\n",
            Id(30),
            Id(1),
            Id(2)
        )
        .replace("[\"prose\"]", "\"prose\"")
        .replace("[\"more\"]", "\"more\"");
        let (mut g, _) = Graph::load_nodes(graph(vec![]).encode_nodes()).unwrap();
        let loaded = g.load_edges(line.as_bytes());
        let decoded = umx_core::codec::decode_edges(line.as_bytes());
        match (&loaded, &decoded) {
            (Ok(_), Ok(d)) => assert_eq!(g.edge(Id(30)).unwrap().to_edge(), d.records[0], "{line}"),
            (Err(a), Err(b)) => assert_eq!(a, b, "{line}"),
            _ => panic!("the scanner and the codec disagree on {line}: {loaded:?} vs {decoded:?}"),
        }
        // Either way the line is refused, never silently resolved to one of the two values.
        assert!(decoded.is_err(), "{line}: {decoded:?}");
    }
}

/// ws007 P2's typed `edited_by_session` and the evidence slot on one line: written editor first, then evidence, and
/// read back by a load and by the codec alike (the scanner's fast path is checked in umx-graph's difftest).
#[test]
fn an_edited_edge_with_evidence_round_trips_through_scanner_and_codec() {
    let mut e = edge(30, vec![pointer("root:repo/src/x.rs")]);
    e.edited_by_session = Some(Id(9));
    let g = graph(vec![e.clone()]);
    let text = line(&g);
    let editor = text
        .find("\"edited_by_session\"")
        .expect("the editor is written");
    let evidence = text.find("\"evidence\"").expect("the evidence is written");
    assert!(editor < evidence, "{text}");
    let again = reload(&g);
    assert_eq!(again.edge(Id(30)).unwrap().to_edge(), e);
    assert_eq!(again.encode_edges(), g.encode_edges());
    let decoded = umx_core::codec::decode_edges(text.as_bytes()).unwrap();
    assert_eq!(decoded.records[0], e);
}

#[test]
fn a_batch_delete_and_edit_carry_the_slot_and_rollback_restores_it() {
    let ev = vec![pointer("root:repo/src/x.rs")];
    for prepare in [false, true] {
        let mut g = graph(vec![edge(30, ev.clone()), edge(31, Vec::new())]);
        let before = g.encode_edges();
        {
            let mut b = g.batch();
            // An edit (F75's path: the record as the batch holds it, changed, put back) keeps the slot.
            let mut e = b.edge_record(Id(30)).unwrap();
            e.kind = "supports".into();
            b.insert_edge(e);
            assert_eq!(b.edge_record(Id(30)).unwrap().evidence, ev);
            // A delete gives the record back with its evidence; the batch no longer shows the edge.
            assert_eq!(b.remove_edge(Id(30)).unwrap().evidence, ev);
            assert!(b.edge_record(Id(30)).is_none());
            if prepare {
                b.prepare().unwrap();
            }
            b.rollback();
        }
        assert_eq!(g.encode_edges(), before, "prepare: {prepare}");
        assert_eq!(g.edge(Id(30)).unwrap().evidence(), ev);
    }
}
