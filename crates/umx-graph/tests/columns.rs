//! s04 layout checks: the columns answer exactly what the records they came from answer, through every kind of
//! write, and encode gives `umx_core::codec`'s bytes (workshop 008 sections 2.1 to 2.3; s04 notes 3 to 8; the
//! dangling-edge ruling).

mod old_find;

use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use umx_core::codec::{self, GraphFile, KeptLine};
use umx_core::value::compare_values;
use umx_core::{Edge, Id, Node};
use umx_graph::{Dangling, Graph, NodeFilter, NodeRef, Op, Where, parse_where};

fn id(n: u128) -> Id {
    Id((0x0194_0000_0000_u128 << 80) | n)
}

fn text(n: u128) -> String {
    id(n).to_text()
}

fn node(n: u128, kind: &str, title: &str) -> Node {
    Node {
        id: id(n),
        kind: kind.into(),
        title: title.into(),
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

fn edge(n: u128, from: u128, kind: &str, to: u128) -> Edge {
    Edge {
        id: id(n),
        kind: kind.into(),
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

/// Services' load split (graph_store::graph::split_dangling): an edge with an end that is not a node keeps its exact
/// line and stays out of the graph.
fn load(nodes_file: &str, edges_file: &str) -> (Graph, Vec<KeptLine>, Vec<KeptLine>, Vec<Id>) {
    let nodes = codec::decode_nodes(nodes_file.as_bytes()).unwrap();
    let edges = codec::decode_edges(edges_file.as_bytes()).unwrap();
    let ids: BTreeSet<Id> = nodes.records.iter().map(|n| n.id).collect();
    let mut kept_edges = edges.kept.clone();
    let (mut in_graph, mut dangling) = (vec![], vec![]);
    for (e, line) in edges.records.into_iter().zip(edges.record_lines) {
        if ids.contains(&e.from) && ids.contains(&e.to) {
            in_graph.push(e);
        } else {
            dangling.push(e.id);
            let bytes = edges_file
                .lines()
                .nth(line - 1)
                .unwrap()
                .as_bytes()
                .to_vec();
            kept_edges.push(KeptLine {
                id: e.id,
                line: bytes,
            });
        }
    }
    let mut g = Graph::from_records(nodes.records, in_graph);
    g.keep_lines(nodes.kept.clone(), kept_edges.clone())
        .unwrap();
    (g, nodes.kept, kept_edges, dangling)
}

fn records(g: &Graph) -> (Vec<Node>, Vec<Edge>) {
    (
        g.nodes().map(NodeRef::to_node).collect(),
        g.edges().map(|e| e.to_edge()).collect(),
    )
}

/// The s04 note 8 fixture: keys in another order, extra spaces, an unknown key on a node and on an edge, times with
/// fractions and in another zone form, a float written `1.0`, an integer, a repeated id, lines out of id order, and
/// a dangling edge.
#[test]
fn a_hand_edited_graph_loads_and_encodes_in_canonical_form_with_nothing_lost() {
    let (a, b, c, gone) = (text(1), text(2), text(3), text(9));
    let nodes_file = format!(
        concat!(
            r#"{{ "title" : "Bravo",  "id":"{b}", "type":"risk","status":"open","note":"","evidence":[],"status_history":[],"created":"2026-11-10T01:00:00.250Z","updated":"2026-11-10T01:00:00Z","created_by":"hand", "colour":"red", "weight": 1.0, "count": 3 }}"#,
            "\n",
            r#"{{"id":"{a}","type":"problem","title":"Alpha (old)","status":"open","note":"","evidence":[],"status_history":[],"created":"2026-11-10T01:00:00Z","updated":"2026-11-10T01:00:00Z","created_by":"hand"}}"#,
            "\n\n",
            r#"{{"id":"{c}","type":"problem","title":"Charlie","status":"open","note":"line\none","evidence":[{{"kind":"room"}}],"status_history":[],"created":"","updated":"later","created_by":"hand"}}"#,
            "\n",
            r#"{{"id":"{a}","type":"problem","title":"Alpha","status":"done","note":"","evidence":[],"status_history":[],"created":"2026-11-10T01:00:00Z","updated":"2026-11-10T02:00:00Z","created_by":"hand"}}"#,
            "\n"
        ),
        a = a,
        b = b,
        c = c
    );
    let edges_file = format!(
        concat!(
            r#"{{"to":"{gone}","id":"{e2}","type":"blocks","from":"{a}","created":"2026-11-10T03:00:00Z","created_by":"hand"}}"#,
            "\n",
            r#"{{ "id":"{e1}","type":"part_of","from":"{b}","to":"{a}","created":"2026-11-10T03:00:00+00:00","created_by":"hand","via":"merge","confidence":1.0 }}"#,
            "\n",
            r#"{{"id":"{e0}","type":"blocks","from":"{c}","to":"{a}","created":"2026-11-10T03:00:00Z","created_by":"hand"}}"#,
            "\n"
        ),
        a = a,
        b = b,
        c = c,
        gone = gone,
        e0 = text(10),
        e1 = text(11),
        e2 = text(12)
    );
    let (mut g, kept_nodes, kept_edges, dangling) = load(&nodes_file, &edges_file);

    // The warnings' sources: one repeated node id on lines 2 and 5, one dangling edge.
    let decoded = codec::decode_nodes(nodes_file.as_bytes()).unwrap();
    assert_eq!(decoded.repeats.len(), 1);
    assert_eq!(
        (decoded.repeats[0].id, &decoded.repeats[0].lines[..]),
        (id(1), &[2, 5][..])
    );
    assert_eq!(dangling, [id(12)]);
    assert_eq!(g.kept_line_count(), 2);
    assert_eq!(g.repeat_lines(GraphFile::Nodes, id(1)), Some(vec![1, 2]));
    assert_eq!(g.repeat_lines(GraphFile::Nodes, id(2)), None);

    // Canonical form, nothing lost: the bytes codec writes for these records and kept lines.
    let reference = |g: &Graph| {
        let (n, e) = records(g);
        (
            codec::encode_nodes_keeping(&n, &kept_nodes),
            codec::encode_edges_keeping(&e, &kept_edges),
        )
    };
    let (nodes_out, edges_out) = (g.encode_nodes(), g.encode_edges());
    assert_eq!((nodes_out.clone(), edges_out.clone()), reference(&g));
    let nodes_text = String::from_utf8(nodes_out.clone()).unwrap();
    for kept in [
        r#""created":"2026-11-10T01:00:00.250Z""#,
        r#""colour":"red""#,
        r#""weight":1.0"#,
        r#""count":3"#,
        r#""title":"Alpha (old)""#,
        r#""created":"","updated":"later""#,
    ] {
        assert!(nodes_text.contains(kept), "{kept} in {nodes_text}");
    }
    let edges_text = String::from_utf8(edges_out.clone()).unwrap();
    assert!(edges_text.contains(r#""created":"2026-11-10T03:00:00+00:00","created_by":"hand","confidence":1.0,"via":"merge"}"#));
    assert!(
        edges_text.contains(edges_file.lines().next().unwrap()),
        "the dangling line keeps its bytes"
    );
    let mut file_records = codec::decode_nodes(nodes_file.as_bytes()).unwrap().records;
    file_records.sort_by_key(|n| n.id);
    assert_eq!(
        codec::decode_nodes(&nodes_out).unwrap().records,
        file_records
    );

    // A dangling edge is not an edge of the graph.
    assert_eq!(g.edge_count(), 2);
    assert!(g.edge(id(12)).is_none());
    assert!(g.edges().all(|e| e.id() != id(12)));
    assert!(g.list_edges(Some(id(1)), None, None).is_empty());
    assert!(g.list_edges(None, Some(id(9)), None).is_empty());
    assert!(g.find_edge(id(1), "blocks", id(9)).is_none());
    for n in g.nodes() {
        for s in g.edges_out(n.ix()).chain(g.edges_in(n.ix())) {
            assert_ne!(g.edge_at(s.edge).id(), id(12));
            g.node_at(s.other);
        }
    }

    // Kept lines survive the rare path: an id lower than the last rebuilds through the load path.
    g.insert_node(node(0, "risk", "Zero")).unwrap();
    g.insert_edge(edge(5, 0, "blocks", 2));
    assert_eq!((g.encode_nodes(), g.encode_edges()), reference(&g));
    assert_eq!(g.repeat_lines(GraphFile::Nodes, id(1)), Some(vec![2, 3]));
    assert!(g.remove_node(id(0)).is_none(), "an edge still touches it");
    g.remove_edge(id(5));
    assert_eq!(g.remove_node(id(0)).map(|n| n.title), Some("Zero".into()));
    assert_eq!((g.encode_nodes(), g.encode_edges()), (nodes_out, edges_out));
}

/// s05 design notes 2 and 3: the same hand-edited files through the services' load path. Node lines umx did not
/// change keep their bytes (a repeated id's lines grouped before its line in use, the blank line dropped); a changed
/// node becomes canonical with its unknown keys; putting the old record back restores the old bytes.
#[test]
fn untouched_node_lines_keep_their_bytes_through_load_and_writes() {
    let (a, b, c, gone) = (text(1), text(2), text(3), text(9));
    let bravo = format!(
        r#"{{ "title" : "Bravo",  "id":"{b}", "type":"risk","status":"open","note":"","evidence":[],"status_history":[],"created":"2026-11-10T01:00:00.250Z","updated":"2026-11-10T01:00:00Z","created_by":"hand", "colour":"red", "weight": 1.0, "count": 3 }}"#
    );
    let alpha_old = format!(
        r#"{{"id":"{a}","type":"problem","title":"Alpha (old)","status":"open","note":"","evidence":[],"status_history":[],"created":"2026-11-10T01:00:00Z","updated":"2026-11-10T01:00:00Z","created_by":"hand"}}"#
    );
    let charlie = format!(
        r#"{{"id":"{c}","type":"problem","title":"Charlie","status":"open","note":"line\none","evidence":[{{"kind":"room"}}],"status_history":[],"created":"","updated":"later","created_by":"hand"}}"#
    );
    let alpha = format!(
        r#"{{"id":"{a}","type":"problem","title":"Alpha","status":"done","note":"","evidence":[],"status_history":[],"created":"2026-11-10T01:00:00Z","updated":"2026-11-10T02:00:00Z","created_by":"hand"}}"#
    );
    let nodes_file = format!("{bravo}\n{alpha_old}\n\n{charlie}\n{alpha}\n");
    let dangling_line = format!(
        r#"{{"to":"{gone}","id":"{}","type":"blocks","from":"{a}","created":"2026-11-10T03:00:00Z","created_by":"hand"}}"#,
        text(12)
    );
    let edges_file = format!(
        concat!(
            "{d}\n",
            r#"{{ "id":"{e1}","type":"part_of","from":"{b}","to":"{a}","created":"2026-11-10T03:00:00+00:00","created_by":"hand","via":"merge","confidence":1.0 }}"#,
            "\n",
            r#"{{"id":"{e0}","type":"blocks","from":"{c}","to":"{a}","created":"2026-11-10T03:00:00Z","created_by":"hand"}}"#,
            "\n"
        ),
        d = dangling_line,
        a = a,
        b = b,
        c = c,
        e0 = text(10),
        e1 = text(11)
    );
    let (mut g, nodes_load) = Graph::load_nodes(nodes_file.clone().into_bytes()).unwrap();
    let repeats = nodes_load.repeats;
    let loaded = g.load_edges(edges_file.as_bytes()).unwrap();
    let decoded = codec::decode_nodes(nodes_file.as_bytes()).unwrap();
    assert_eq!(repeats, decoded.repeats);
    assert_eq!(repeats[0].lines, [2, 5]);
    assert!(loaded.repeats.is_empty());
    assert_eq!(
        loaded.dangling,
        [Dangling {
            edge: id(12),
            end: "to",
            node: id(9),
            line: 1
        }]
    );
    g.check_rows();

    // Id order, the repeated id's old line first, every line as it was.
    let image = format!("{alpha_old}\n{alpha}\n{bravo}\n{charlie}\n").into_bytes();
    assert_eq!(g.encode_nodes(), image);
    assert_eq!(g.repeat_lines(GraphFile::Nodes, id(1)), Some(vec![1, 2]));
    let mut want = decoded.records.clone();
    want.sort_by_key(|n| n.id);
    assert_eq!(records(&g).0, want);
    // Edges: canonical form, nothing lost, the dangling line exactly.
    let edges_out = g.encode_edges();
    let (_, e) = records(&g);
    assert_eq!(
        edges_out,
        codec::encode_edges_keeping(
            &e,
            &[KeptLine {
                id: id(12),
                line: dangling_line.clone().into_bytes()
            }]
        )
    );

    // Custom fields from the columns.
    let bv = g.node(id(2)).unwrap();
    assert_eq!(
        bv.custom_field_names().collect::<Vec<_>>(),
        ["colour", "count", "weight"]
    );
    assert_eq!(bv.field("weight").unwrap().to_string(), "1.0");
    assert_eq!(g.first_use("count"), Some(json!(3)));
    assert_eq!(g.first_use("nope"), None);
    let by = |expr: &str, value: serde_json::Value| -> Vec<Id> {
        let (field, op, _) = parse_where(expr).unwrap();
        let f = NodeFilter {
            wheres: vec![Where { field, op, value }],
            ..Default::default()
        };
        g.list_nodes(&f).iter().map(|n| n.id()).collect()
    };
    assert_eq!(by("count>=3", json!(3)), [id(2)]);
    assert_eq!(by("colour=red", json!("red")), [id(2)]);
    assert_eq!(by("weight=1", json!(1)), [id(2)]);
    assert!(by("nope=1", json!(1)).is_empty());

    // A changed node becomes canonical, unknown keys kept; the other lines keep their bytes.
    let old_b = bv.to_node();
    let mut new_b = old_b.clone();
    new_b.title = "Bravo two".into();
    assert_eq!(g.insert_node(new_b.clone()), Ok(Some(old_b.clone())));
    let mut canonical = codec::encode_nodes([&new_b]);
    canonical.pop();
    let canonical = String::from_utf8(canonical).unwrap();
    assert!(canonical.contains(r#""colour":"red","count":3,"weight":1.0}"#));
    assert_eq!(
        g.encode_nodes(),
        format!("{alpha_old}\n{alpha}\n{canonical}\n{charlie}\n").into_bytes()
    );
    g.check_rows();
    // Undo: the old record again gives the old bytes back.
    assert_eq!(g.insert_node(old_b.clone()), Ok(Some(new_b)));
    assert_eq!(g.encode_nodes(), image);
    // A record equal to the one held changes nothing.
    assert_eq!(g.insert_node(old_b.clone()), Ok(Some(old_b)));
    assert_eq!(g.encode_nodes(), image);

    // The rare path (an id below the last) derives every row from the image again; nothing else moves.
    g.insert_node(node(0, "risk", "Zero")).unwrap();
    g.insert_edge(edge(5, 0, "blocks", 2));
    g.check_rows();
    let zero = String::from_utf8(codec::encode_nodes([&node(0, "risk", "Zero")])).unwrap();
    assert_eq!(
        g.encode_nodes(),
        format!("{zero}{alpha_old}\n{alpha}\n{bravo}\n{charlie}\n").into_bytes()
    );
    assert_eq!(g.repeat_lines(GraphFile::Nodes, id(1)), Some(vec![2, 3]));
    assert_eq!(g.node_at(g.node_ix(id(0)).unwrap()).title(), "Zero");
    assert_eq!(
        g.edges_out(g.node_ix(id(0)).unwrap())
            .map(|s| g.node_at(s.other).id())
            .collect::<Vec<_>>(),
        [id(2)]
    );
    g.remove_edge(id(5));
    assert!(g.remove_node(id(0)).is_some());
    g.check_rows();
    assert_eq!((g.encode_nodes(), g.encode_edges()), (image, edges_out));
}

/// R1 (req-67 with req-76): every edge line counts for repeated ids, dangling or not; the last line of an id
/// is the one in use and only it is reported dangling or loaded. The oracle is the pre-s05 load: codec's decode, then
/// services' split of the records in use (split_dangling).
#[test]
fn a_repeated_edge_id_with_dangling_copies_loads_as_before() {
    let (a, b, gone) = (text(1), text(2), text(9));
    let nodes_file = String::from_utf8(codec::encode_nodes([
        &node(1, "problem", "A"),
        &node(2, "problem", "B"),
    ]))
    .unwrap();
    let line = |e: u128, from: &str, to: &str, by: &str| {
        format!(
            r#"{{"id":"{}","type":"blocks","from":"{from}","to":"{to}","created":"2026-11-10T03:00:00Z","created_by":"{by}"}}"#,
            text(e)
        )
    };
    let cases = [
        (
            "dangling first, then live",
            vec![line(10, &a, &gone, "x"), line(10, &a, &b, "y")],
        ),
        (
            "both dangling",
            vec![line(10, &a, &gone, "x"), line(10, &gone, &b, "y")],
        ),
        (
            "live first, then dangling",
            vec![line(10, &a, &b, "x"), line(10, &a, &gone, "y")],
        ),
        (
            "a dangling copy after a later live id",
            vec![
                line(10, &a, &gone, "x"),
                line(11, &a, &b, "z"),
                line(10, &a, &b, "y"),
            ],
        ),
        (
            "live, live",
            vec![line(10, &a, &b, "x"), line(10, &b, &a, "y")],
        ),
    ];
    let mut bad = Vec::new();
    for (name, lines) in cases {
        let edges_file = lines.iter().map(|l| format!("{l}\n")).collect::<String>();
        // The pre-s05 load.
        let decoded = codec::decode_edges(edges_file.as_bytes()).unwrap();
        let ids: BTreeSet<Id> = [id(1), id(2)].into();
        let mut kept = decoded.kept.clone();
        let (mut live, mut dangling) = (vec![], vec![]);
        for (e, no) in decoded.records.iter().zip(&decoded.record_lines) {
            let missing: Vec<(&str, Id)> = [("from", e.from), ("to", e.to)]
                .into_iter()
                .filter(|(_, n)| !ids.contains(n))
                .collect();
            if missing.is_empty() {
                live.push(e.clone());
                continue;
            }
            for (end, n) in missing {
                dangling.push(Dangling {
                    edge: e.id,
                    end,
                    node: n,
                    line: *no,
                });
            }
            kept.push(KeptLine {
                id: e.id,
                line: lines[no - 1].clone().into_bytes(),
            });
        }
        let (mut g, _) = Graph::load_nodes(nodes_file.clone().into_bytes()).unwrap();
        let got = g.load_edges(edges_file.as_bytes()).unwrap();
        let mut sorted_live = live.clone();
        sorted_live.sort_by_key(|e| e.id);
        for (what, same) in [
            ("repeats", got.repeats == decoded.repeats),
            ("dangling", got.dangling == dangling),
            ("edges", records(&g).1 == sorted_live),
            (
                "encode",
                g.encode_edges() == codec::encode_edges_keeping(&live, &kept),
            ),
            ("kept lines", g.kept_line_count() == kept.len()),
        ] {
            if !same {
                bad.push(format!("{name}: {what}"));
            }
        }
    }
    assert!(bad.is_empty(), "differs from the pre-s05 load: {bad:#?}");
}

/// R1: a wanted type whose name number does not fit a narrow type column is no node's type, so a type filter
/// with a number `--where` finds nothing (it once wrapped to type 0). Control: the same names once the type column is
/// wide.
#[test]
fn a_type_number_past_the_narrow_column_never_aliases_another_type() {
    // Names: problem 0, open 1, tester 2, then the fields in name order: 65,532 `f` names, likelihood, zz = 65,536.
    let mut a = node(1, "problem", "A");
    for i in 0..65_532 {
        a.fields.insert(format!("f{i:05}"), json!(1));
    }
    a.fields.insert("likelihood".into(), json!(3));
    a.fields.insert("zz".into(), json!(1));
    let filter = |kind: &str| NodeFilter {
        kind: Some(kind.into()),
        wheres: vec![Where {
            field: "likelihood".into(),
            op: Op::Ge,
            value: json!(3),
        }],
        ..Default::default()
    };
    let list =
        |g: &Graph, f: &NodeFilter| -> Vec<Id> { g.list_nodes(f).iter().map(|n| n.id()).collect() };
    let g = Graph::from_records(vec![a.clone()], vec![]);
    let by_type = NodeFilter {
        kind: Some("zz".into()),
        ..Default::default()
    };
    assert!(list(&g, &by_type).is_empty());
    assert!(list(&g, &filter("zz")).is_empty(), "zz is not a type");
    assert_eq!(list(&g, &filter("problem")), [id(1)]);
    // Control: a node of type zz widens the type column; each filter finds its own node.
    let mut b = node(2, "zz", "B");
    b.fields.insert("likelihood".into(), json!(4));
    let g = Graph::from_records(vec![a, b], vec![]);
    assert_eq!(list(&g, &filter("zz")), [id(2)]);
    assert_eq!(list(&g, &filter("problem")), [id(1)]);
    assert_eq!(list(&g, &by_type), [id(2)]);
}

#[test]
fn from_records_keeps_a_dangling_edge_out_as_its_canonical_line() {
    let e = edge(10, 1, "blocks", 9);
    let g = Graph::from_records(
        vec![node(1, "problem", "A")],
        vec![e.clone(), edge(11, 1, "relates_to", 1)],
    );
    assert_eq!((g.edge_count(), g.kept_line_count()), (1, 1));
    assert!(g.edge(id(10)).is_none() && g.kind_of("blocks").is_none());
    let ix = g.node_ix(id(1)).unwrap();
    assert_eq!(g.edges_out(ix).count(), 1);
    assert_eq!(
        g.encode_edges(),
        [
            codec::encode_edges([&e]),
            codec::encode_edges([&edge(11, 1, "relates_to", 1)])
        ]
        .concat()
    );
}

/// R1: a time held as an exception is written as an escaped JSON string, so the file decodes and the records
/// come back, for a node's created and updated and an edge's created. codec is the oracle for the bytes.
#[test]
fn exception_times_with_json_metacharacters_round_trip_through_codec() {
    for odd in [
        "not \"time\"",
        "back\\slash",
        "tab\tnewline\ncontrol\u{1}\u{1f}",
        "2026-11-10T01:00:00Z\"",
        "\u{2028} é",
    ] {
        let mut a = node(1, "problem", "A");
        a.created = odd.into();
        a.updated = format!("{odd}{odd}");
        let b = node(2, "problem", "B");
        let mut e = edge(10, 1, "blocks", 2);
        e.created = odd.into();
        let (nodes_file, edges_file) = (codec::encode_nodes([&a, &b]), codec::encode_edges([&e]));
        let nodes = codec::decode_nodes(&nodes_file).unwrap().records;
        let edges = codec::decode_edges(&edges_file).unwrap().records;
        let g = Graph::from_records(nodes.clone(), edges.clone());
        let (n_out, e_out) = (g.encode_nodes(), g.encode_edges());
        assert_eq!(n_out, nodes_file, "{odd:?}");
        assert_eq!(e_out, edges_file, "{odd:?}");
        assert_eq!(
            codec::decode_nodes(&n_out).unwrap().records,
            nodes,
            "{odd:?}"
        );
        assert_eq!(
            codec::decode_edges(&e_out).unwrap().records,
            edges,
            "{odd:?}"
        );
        let v = g.node(id(1)).unwrap();
        assert_eq!(
            (v.created(), v.updated()),
            (a.created.clone(), a.updated.clone())
        );
        assert_eq!(g.edge(id(10)).unwrap().created(), e.created);
    }
}

#[test]
#[cfg_attr(debug_assertions, should_panic(expected = "dangling"))]
fn insert_edge_refuses_an_end_that_is_not_a_node() {
    let mut g = Graph::from_records(vec![node(1, "problem", "A")], vec![]);
    assert_eq!(g.insert_edge(edge(10, 1, "blocks", 9)), None);
    assert_eq!((g.edge_count(), g.kept_line_count()), (0, 0));
}

#[test]
fn external_ends_are_not_dangling_but_a_missing_local_end_still_is() {
    let mut external = edge(10, 7, "blocks", 8);
    external.from_graph = Some(id(90));
    external.to_graph = Some(id(91));
    let mut mixed = edge(11, 7, "blocks", 8);
    mixed.from_graph = Some(id(90));
    let bytes = codec::encode_edges([&external, &mixed]);
    let (mut g, _) = Graph::load_nodes(vec![]).unwrap();
    let report = g.load_edges(&bytes).unwrap();
    assert_eq!(
        report.dangling,
        [Dangling {
            edge: mixed.id,
            end: "to",
            node: mixed.to,
            line: 2
        }]
    );
    assert_eq!(g.edge_count(), 1);
    assert_eq!(g.edge(external.id).unwrap().to_edge(), external);
    assert_eq!(g.encode_edges(), bytes);
}

#[test]
fn external_mixed_writes_match_an_id_oracle_and_a_fresh_load() {
    let mut nodes: BTreeMap<Id, Node> = [10, 20, 30]
        .into_iter()
        .map(|i| {
            let n = node(i, "problem", "A");
            (n.id, n)
        })
        .collect();
    let mut expected = BTreeMap::new();
    let mut g = Graph::from_records(nodes.values().cloned().collect(), vec![]);
    let mut rng = Rng(67);
    let check = |g: &Graph, expected: &BTreeMap<Id, Edge>| {
        let bytes = codec::encode_edges(expected.values());
        assert_eq!(g.encode_edges(), bytes);
        let (mut fresh, _) = Graph::load_nodes(g.encode_nodes()).unwrap();
        assert!(fresh.load_edges(&bytes).unwrap().dangling.is_empty());
        assert_eq!(
            g.edges().map(|e| e.to_edge()).collect::<Vec<_>>(),
            expected.values().cloned().collect::<Vec<_>>()
        );
        for n in g.nodes() {
            let h = fresh.node_ix(n.id()).unwrap();
            let local = |graph: &Graph, ix| {
                graph
                    .edges_out(ix)
                    .chain(graph.edges_in(ix))
                    .map(|s| (graph.edge_at(s.edge).id(), graph.node_at(s.other).id()))
                    .collect::<Vec<_>>()
            };
            let external = |graph: &Graph, ix| {
                graph
                    .edges_out_external(ix)
                    .chain(graph.edges_in_external(ix))
                    .map(|(e, _, graph_id, node)| (graph.edge_at(e).id(), graph_id, node))
                    .collect::<Vec<_>>()
            };
            assert_eq!(local(g, n.ix()), local(&fresh, h));
            assert_eq!(external(g, n.ix()), external(&fresh, h));
        }
    };
    for turn in 0..180 {
        let n = [10, 20, 30][rng.below(3)];
        let mut e = edge(100 + rng.below(12) as u128, n, "blocks", 20);
        e.from_graph = (turn % 3 == 0).then_some(id(90));
        e.to_graph = (turn % 4 != 0).then_some(id(91));
        e.fields.insert("unknown".into(), json!({"turn": turn}));
        match turn % 6 {
            0 | 1 => {
                expected.insert(e.id, e.clone());
                g.insert_edge(e);
            }
            2 => {
                assert_eq!(g.remove_edge(e.id), expected.remove(&e.id));
            }
            3 => {
                let before = g.encode_edges();
                let mut batch = g.batch();
                batch.insert_edge(e);
                batch.rollback();
                assert_eq!(g.encode_edges(), before);
            }
            4 => {
                expected.insert(e.id, e.clone());
                let mut batch = g.batch();
                batch.insert_edge(e);
                batch.commit();
            }
            _ => {
                // Inserting and removing the smallest and largest ids exercises both renumber paths.
                for id in [5, 40] {
                    let n = node(id, "problem", "renumber");
                    nodes.insert(n.id, n.clone());
                    g.insert_node(n).unwrap();
                    check(&g, &expected);
                }
                for id in [5, 40] {
                    assert_eq!(g.remove_node(self::id(id)), nodes.remove(&self::id(id)));
                }
            }
        }
        check(&g, &expected);
    }
    for id in expected.keys() {
        g.remove_edge(*id);
    }
    assert_eq!(
        g.memory_bytes().external,
        0,
        "external storage is reclaimed after churn"
    );
}

/// s20 (graph-storage condition 2): `reset_edges` after a load, a failed one too, leaves the graph as `load_nodes`
/// left it, and the edges then load again as on a fresh graph. The S5b evidence slot is reset too (S210-3): a reload
/// where the same edge has no evidence must not keep the old list.
#[test]
fn reset_edges_goes_back_to_the_nodes_only_graph() {
    let nodes = codec::encode_nodes([
        &node(10, "problem", "A"),
        &node(20, "problem", "B"),
        &node(30, "idea", "C"),
    ]);
    let mut external = edge(5, 10, "cites", 99);
    external.to_graph = Some(id(90));
    let mut sessioned = edge(6, 10, "fresh_relation", 20);
    sessioned.authored_by_session = Some(id(30));
    sessioned.edited_by_session = Some(id(30));
    sessioned.created_by = "a new author".into();
    sessioned
        .fields
        .insert("x_graph".into(), json!("custom \\ value"));
    sessioned.evidence = vec![json!({ "kind": "document", "source": "docs/a.md" })];
    let dangling = edge(7, 10, "blocks", 77);
    let mut file = codec::encode_edges([&external, &sessioned, &dangling]);
    // A repeat after a later id: the grouping path, an earlier line kept.
    file.extend(codec::encode_edges([&edge(5, 20, "another_relation", 30)]));
    let fresh = || Graph::load_nodes(nodes.clone()).unwrap().0;
    let mut whole = fresh();
    let whole_report = whole.load_edges(&file).unwrap();
    assert!(whole.kept_line_count() > 0 && whole.edge_count() > 0);

    let mut bad = file.clone();
    bad.extend_from_slice(b"{\"id\":\"not an id\"}\n");
    for bytes in [&file, &bad] {
        let mut g = fresh();
        let mark = g.edges_mark();
        let _ = g.load_edges(bytes);
        g.reset_edges(mark);
        assert_eq!(g.memory_bytes(), fresh().memory_bytes());
        assert_eq!((g.edge_count(), g.kept_line_count()), (0, 0));
        assert!(g.encode_edges().is_empty());
        assert_eq!(g.external_end_graphs().count(), 0);
        assert_eq!(g.census().relations, []);
        assert_eq!(g.load_edges(&file).unwrap(), whole_report);
        assert_eq!(g.encode_edges(), whole.encode_edges());
        assert_eq!(g.memory_bytes(), whole.memory_bytes());
        assert_eq!(
            g.kept_edge_lines().collect::<Vec<_>>(),
            whole.kept_edge_lines().collect::<Vec<_>>()
        );
    }
    // The same edge id with no evidence after a reset: nothing of the first load's slot is left.
    let mut bare = sessioned.clone();
    bare.evidence = Vec::new();
    let plain = codec::encode_edges([&bare]);
    let mut g = fresh();
    let mark = g.edges_mark();
    g.load_edges(&file).unwrap();
    assert_eq!(g.edge(id(6)).unwrap().evidence().len(), 1);
    g.reset_edges(mark);
    g.load_edges(&plain).unwrap();
    assert!(g.edge(id(6)).unwrap().evidence().is_empty());
    let mut clean = fresh();
    clean.load_edges(&plain).unwrap();
    assert_eq!(g.encode_edges(), clean.encode_edges());
    assert_eq!(g.memory_bytes(), clean.memory_bytes());
}

/// SplitMix64.
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

const KINDS: &[&str] = &["problem", "risk", "decision", "similar_to"];
const RELATIONS: &[&str] = &["blocks", "part_of", "similar_to", "relates_to"];
const TIMES: &[&str] = &[
    "2026-11-10T01:00:00Z",
    "2026-11-10T01:00:00.5Z",
    "",
    "1970-01-01T00:00:00Z",
    "not \"time\"",
    "back\\slash\u{1}\t\n",
];

fn in_name_order<'a>(names: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    names.collect::<BTreeSet<_>>().into_iter().collect()
}

/// Every read the graph answers, from plain records: the model.
fn check(g: &Graph, nodes: &BTreeMap<Id, Node>, edges: &BTreeMap<Id, Edge>) {
    assert_eq!(g.encode_nodes(), codec::encode_nodes(nodes.values()));
    assert_eq!(g.encode_edges(), codec::encode_edges(edges.values()));
    assert_eq!((g.node_count(), g.edge_count()), (nodes.len(), edges.len()));
    let (n, e) = records(g);
    assert_eq!(n, nodes.values().cloned().collect::<Vec<_>>());
    assert_eq!(e, edges.values().cloned().collect::<Vec<_>>());
    for v in g.nodes() {
        let r = &nodes[&v.id()];
        assert_eq!(
            (v.kind(), v.title(), v.status(), v.created(), v.updated()),
            (
                r.kind.as_str(),
                r.title.as_str(),
                r.status.as_str(),
                r.created.clone(),
                r.updated.clone()
            )
        );
        for f in [
            "title",
            "note",
            "evidence_kinds",
            "created",
            "likelihood",
            "nope",
        ] {
            assert_eq!(v.field(f), r.field(f), "{f}");
        }
        let ix = v.ix();
        let out: Vec<Id> = g.edges_out(ix).map(|s| g.edge_at(s.edge).id()).collect();
        let inn: Vec<Id> = g.edges_in(ix).map(|s| g.edge_at(s.edge).id()).collect();
        assert_eq!(
            out,
            edges
                .values()
                .filter(|e| e.from == r.id)
                .map(|e| e.id)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            inn,
            edges
                .values()
                .filter(|e| e.to == r.id)
                .map(|e| e.id)
                .collect::<Vec<_>>()
        );
        for s in g.edges_out(ix) {
            assert_eq!(g.node_at(s.other).id(), edges[&g.edge_at(s.edge).id()].to);
            assert_eq!(s.kind.as_str(), edges[&g.edge_at(s.edge).id()].kind);
        }
    }
    let kinds: Vec<&str> = g.node_kinds().iter().map(|k| k.as_str()).collect();
    assert_eq!(
        kinds,
        in_name_order(nodes.values().map(|n| n.kind.as_str()))
    );
    let relations: Vec<&str> = g.relation_kinds().iter().map(|k| k.as_str()).collect();
    assert_eq!(
        relations,
        in_name_order(
            edges
                .values()
                .map(|e| e.kind.as_str())
                .filter(|k| *k != "similar_to")
        )
    );
    for name in KINDS.iter().chain(RELATIONS) {
        let in_use =
            nodes.values().any(|n| n.kind == *name) || edges.values().any(|e| e.kind == *name);
        assert_eq!(g.kind_of(name).is_some(), in_use, "{name}");
    }
    g.check_rows();
    for v in g.nodes() {
        let r = &nodes[&v.id()];
        assert_eq!(
            v.custom_field_names().collect::<Vec<_>>(),
            r.fields.keys().map(String::as_str).collect::<Vec<_>>()
        );
    }
    for name in [
        "likelihood",
        "weight",
        "owner",
        "flag",
        "tags",
        "meta",
        "nope",
    ] {
        assert_eq!(
            g.first_use(name),
            nodes.values().find_map(|n| n.fields.get(name).cloned()),
            "{name}"
        );
    }
    for (expr, value) in [
        ("likelihood>=2", json!(2)),
        ("likelihood!=3", json!(3)),
        ("likelihood<2.5", json!(2.5)),
        ("weight=1", json!(1)),
        ("weight=heavy", json!("heavy")),
        ("owner=a\"b", json!("a\"b")),
        ("owner>b", json!("b")),
        ("flag=true", json!(true)),
        ("tags=x", json!("x")),
        ("meta=1", json!(1)),
        ("nope=1", json!(1)),
        ("title>title 3", json!("title 3")),
    ] {
        let (field, op, _) = parse_where(expr).unwrap();
        let w = Where { field, op, value };
        for kind in [None, Some("risk")] {
            let f = NodeFilter {
                kind: kind.map(Into::into),
                wheres: vec![w.clone()],
                ..Default::default()
            };
            let got: Vec<Id> = g.list_nodes(&f).iter().map(|n| n.id()).collect();
            let want: Vec<Id> = nodes
                .values()
                .filter(|n| kind.is_none_or(|k| n.kind == k) && holds(n, &w))
                .map(|n| n.id)
                .collect();
            assert_eq!(got, want, "{expr} {kind:?}");
        }
    }
    let filter = NodeFilter {
        kind: Some("risk".into()),
        status: Some("open".into()),
        ..Default::default()
    };
    let listed: Vec<Id> = g.list_nodes(&filter).iter().map(|n| n.id()).collect();
    let want: Vec<Id> = nodes
        .values()
        .filter(|n| n.kind == "risk" && n.status == "open")
        .map(|n| n.id)
        .collect();
    assert_eq!(listed, want);
    // s06: find reads the normalised titles every write keeps (ws004 s24's rule on top).
    for text in ["title 3 q", "Title xxxxx", "risk"] {
        let rows = nodes
            .values()
            .map(|n| (n.id, n.kind.as_str(), n.title.as_str()));
        assert_eq!(
            old_find::new(g, text, None, usize::MAX),
            old_find::find_rule(rows, text, None, usize::MAX),
            "{text:?}"
        );
    }
}

/// The reference for `--where`: `Node::field` and the operator rules, record by record.
fn holds(n: &Node, w: &Where) -> bool {
    use std::cmp::Ordering;
    let Some(a) = n.field(&w.field) else {
        return false;
    };
    // ws008 E1: on a list, = is "an element equals" and != its complement; ordering never matches.
    if let Some(items) = a.as_array()
        && !w.value.is_array()
    {
        let any = items
            .iter()
            .any(|i| *i == w.value || compare_values(i, &w.value) == Some(Ordering::Equal));
        return match w.op {
            Op::Eq => any,
            Op::Ne => !any,
            _ => false,
        };
    }
    let c = compare_values(&a, &w.value);
    match w.op {
        Op::Eq => a == w.value || c == Some(Ordering::Equal),
        Op::Ne => a != w.value && c != Some(Ordering::Equal),
        Op::Lt => c == Some(Ordering::Less),
        Op::Le => matches!(c, Some(Ordering::Less | Ordering::Equal)),
        Op::Gt => c == Some(Ordering::Greater),
        Op::Ge => matches!(c, Some(Ordering::Greater | Ordering::Equal)),
    }
}

#[test]
fn mixed_writes_read_exactly_as_the_records_they_hold() {
    let mut rng = Rng(7);
    let mut g = Graph::new();
    let (mut nodes, mut edges) = (BTreeMap::new(), BTreeMap::new());
    let mut next = 1000u128;
    for step in 0..600 {
        let ids: Vec<Id> = nodes.keys().copied().collect();
        match rng.below(10) {
            // Mostly appends; sometimes an id below the last (the rebuild path); sometimes a replace.
            0..=3 => {
                let n = match rng.below(6) {
                    0 if !ids.is_empty() => ids[rng.below(ids.len())].0 & 0xffff,
                    1 => {
                        next += 1;
                        u128::from(rng.next() % 999) + 1
                    }
                    _ => {
                        next += 1;
                        next
                    }
                };
                let mut r = node(
                    n,
                    KINDS[rng.below(KINDS.len())],
                    &format!("title {step} \"q\"\n{}", "x".repeat(rng.below(40))),
                );
                r.status = ["open", "done"][rng.below(2)].into();
                r.created = TIMES[rng.below(TIMES.len())].into();
                if rng.below(3) == 0 {
                    r.fields.insert("likelihood".into(), json!(rng.below(5)));
                    r.fields.insert("weight".into(), json!(1.0));
                    r.note = "a note".into();
                    r.evidence = vec![json!({"kind": "room"})];
                }
                // Values of every kind, sometimes several kinds under one name.
                for (name, v) in [
                    ("owner", json!(["a\"b", "b", "é\n"][rng.below(3)])),
                    ("flag", json!(rng.below(2) == 0)),
                    ("tags", json!(["x", "y"])),
                    ("meta", serde_json::Value::Null),
                    ("likelihood", json!(rng.below(5) as f64 + 0.5)),
                    ("weight", json!("heavy")),
                ] {
                    if rng.below(5) == 0 {
                        r.fields.insert(name.into(), v);
                    }
                }
                assert_eq!(g.insert_node(r.clone()), Ok(nodes.insert(r.id, r)));
            }
            4..=6 if !ids.is_empty() => {
                let (from, to) = (ids[rng.below(ids.len())], ids[rng.below(ids.len())]);
                // Mostly a new largest id; sometimes a replace, sometimes an id below the last.
                let n = match rng.below(8) {
                    0 if !edges.is_empty() => {
                        let es: Vec<&Id> = edges.keys().collect();
                        es[rng.below(es.len())].0 & 0xffff
                    }
                    1 => u128::from(rng.next() % 999) + 1,
                    _ => {
                        next += 1;
                        next
                    }
                };
                let mut e = edge(n, 0, RELATIONS[rng.below(RELATIONS.len())], 0);
                (e.from, e.to) = (from, to);
                e.created = TIMES[rng.below(TIMES.len())].into();
                if rng.below(4) == 0 {
                    e.fields.insert("confidence".into(), json!(0.5));
                }
                assert_eq!(g.insert_edge(e.clone()), edges.insert(e.id, e));
            }
            7 | 8 if !edges.is_empty() => {
                let es: Vec<Id> = edges.keys().copied().collect();
                let e = es[rng.below(es.len())];
                assert_eq!(g.remove_edge(e), edges.remove(&e));
            }
            _ if !ids.is_empty() => {
                let n = ids[rng.below(ids.len())];
                let touched = edges.values().any(|e| e.from == n || e.to == n);
                let want = (!touched).then(|| nodes.remove(&n)).flatten();
                assert_eq!(g.remove_node(n), want, "touched {touched}");
            }
            _ => {}
        }
        check(&g, &nodes, &edges);
    }
    assert!(
        nodes.len() > 50 && edges.len() > 20,
        "{} {}",
        nodes.len(),
        edges.len()
    );
    // The load path from the same records gives the same graph.
    let again = Graph::from_records(
        nodes.values().cloned().collect(),
        edges.values().cloned().collect(),
    );
    assert_eq!(
        (again.encode_nodes(), again.encode_edges()),
        (g.encode_nodes(), g.encode_edges())
    );
    // And from the files, as services load them.
    let (mut files, _) = Graph::load_nodes(g.encode_nodes()).unwrap();
    files.load_edges(&g.encode_edges()).unwrap();
    assert_eq!(
        (files.encode_nodes(), files.encode_edges()),
        (g.encode_nodes(), g.encode_edges())
    );
    files.check_rows();
}

/// ws008 TM2: a graph written before month and year dates loads and saves to the same bytes (no stored date is
/// rewritten), and `--where` reads its days and date-times by instant, beside a month and a year added later.
#[test]
fn old_dates_keep_their_bytes_and_compare_by_instant() {
    let dated = |n: u128, value: &str| {
        let mut x = node(n, "action", "dated");
        x.fields.insert("due_on".into(), json!(value));
        x
    };
    let old = [
        dated(1, "2026-10-29"),
        dated(2, "2026-10-29T22:40:00Z"),
        dated(3, "2026-10-30"),
    ];
    let bytes = codec::encode_nodes(&old);
    let (g, _) = Graph::load_nodes(bytes.clone()).unwrap();
    assert_eq!(g.encode_nodes(), bytes);

    let mut both = old.to_vec();
    both.extend([dated(4, "2026-10"), dated(5, "2026")]);
    let bytes = codec::encode_nodes(&both);
    let (g, _) = Graph::load_nodes(bytes.clone()).unwrap();
    assert_eq!(g.encode_nodes(), bytes);
    let by = |expr: &str| -> Vec<Id> {
        let (field, op, value) = parse_where(expr).unwrap();
        let f = NodeFilter {
            wheres: vec![Where {
                field,
                op,
                value: json!(value),
            }],
            ..Default::default()
        };
        let found: Vec<Id> = g.list_nodes(&f).iter().map(|n| n.id()).collect();
        let reference: Vec<Id> = both
            .iter()
            .filter(|n| holds(n, &f.wheres[0]))
            .map(|n| n.id)
            .collect();
        assert_eq!(found, reference, "{expr}");
        found
    };
    assert_eq!(by("due_on<2026-10-30"), [id(1), id(2)]);
    assert_eq!(by("due_on<=2026-10-29"), [id(1), id(2), id(4), id(5)]);
    assert_eq!(by("due_on=2026-10"), [id(1), id(2), id(3), id(4), id(5)]);
    assert_eq!(by("due_on>2026-10-29T22:40:00Z"), [id(3)]);
    assert_eq!(by("due_on<2026-11"), [id(1), id(2), id(3), id(4)]);
}
