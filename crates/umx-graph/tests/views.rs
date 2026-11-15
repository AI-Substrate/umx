//! The view contract every later layout must keep (workshop 008 section 2.8; s02 spec sections 2 and 7; the
//! cypher-expert's point 9): adjacency lists each edge once from each end in edge id order, loads are
//! deterministic, and a view reads the same values as the decoded record.

use serde_json::json;
use umx_core::{Edge, Id, Node};
use umx_graph::{EdgeEnd, Graph, Kind, NodeRef, Step};

fn node(id: u128, kind: &str, title: &str) -> Node {
    let mut n = Node {
        id: Id(id),
        kind: kind.into(),
        title: title.into(),
        status: "open".into(),
        note: format!("note {id}"),
        evidence: vec![json!({"kind": "analyst", "text": "seen"})],
        status_history: vec![json!({"status": "open"})],
        created: "2026-11-10T01:00:00Z".into(),
        updated: "2026-11-10T02:00:00Z".into(),
        created_by: "tester".into(),
        locator: None,
        authored_by_session: None,
        edited_by_session: None,
        fields: Default::default(),
    };
    n.fields.insert("likelihood".into(), json!(id as i64));
    n
}

fn edge(id: u128, from: u128, kind: &str, to: u128) -> Edge {
    let mut e = Edge {
        id: Id(id),
        kind: kind.into(),
        from_graph: None,
        from: Id(from),
        to_graph: None,
        to: Id(to),
        created: "2026-11-10T03:00:00Z".into(),
        created_by: "tester".into(),
        authored_by_session: None,
        edited_by_session: None,
        evidence: Vec::new(),
        fields: Default::default(),
    };
    e.fields.insert("confidence".into(), json!(0.5));
    e
}

fn records() -> (Vec<Node>, Vec<Edge>) {
    (
        vec![
            node(1, "problem", "Approval delays"),
            node(2, "decision", "Launch date"),
            node(3, "risk", "Launch slips"),
        ],
        vec![
            edge(20, 1, "blocks", 2),
            edge(21, 1, "blocks", 2),     // parallel to 20
            edge(22, 2, "relates_to", 2), // self-loop
            edge(23, 3, "part_of", 1),
            edge(24, 2, "similar_to", 1),
            edge(25, 3, "blocks", 2),
        ],
    )
}

fn load(reverse: bool) -> Graph {
    let (mut nodes, mut edges) = records();
    if reverse {
        nodes.reverse();
        edges.reverse();
    }
    Graph::from_records(nodes, edges)
}

type Adjacency<'a> = Vec<(Vec<Step<'a>>, Vec<Step<'a>>)>;

fn adjacency(g: &Graph) -> Adjacency<'_> {
    g.nodes()
        .map(|n| (g.edges_out(n.ix()).collect(), g.edges_in(n.ix()).collect()))
        .collect()
}

fn edge_ids(g: &Graph, steps: &[Step]) -> Vec<Id> {
    steps.iter().map(|s| g.edge_at(s.edge).id()).collect()
}

#[test]
fn external_ends_are_explicit_and_never_appear_as_local_handles() {
    let local = edge(20, 1, "blocks", 2);
    let mut outgoing = edge(21, 1, "blocks", 2);
    outgoing.to_graph = Some(Id(90));
    let mut incoming = edge(22, 1, "blocks", 2);
    incoming.from_graph = Some(Id(91));
    let mut link = edge(23, 1, "blocks", 2);
    link.from_graph = Some(Id(90));
    link.to_graph = Some(Id(91));
    let edges = [local, outgoing, incoming, link];
    let bytes = umx_core::codec::encode_edges(&edges);
    let (mut g, _) = Graph::load_nodes(umx_core::codec::encode_nodes([
        &node(1, "problem", "A"),
        &node(2, "problem", "B"),
    ]))
    .unwrap();
    assert!(g.load_edges(&bytes).unwrap().dangling.is_empty());
    assert_eq!(g.edge_count(), 4);
    assert_eq!(g.encode_edges(), bytes);
    for (record, view) in edges.iter().zip(g.edges()) {
        assert_eq!(view.to_edge(), *record);
        assert_eq!(
            (view.from_graph(), view.to_graph()),
            (record.from_graph, record.to_graph)
        );
        assert_eq!(
            (view.from_ix().is_none(), view.to_ix().is_none()),
            (record.from_graph.is_some(), record.to_graph.is_some())
        );
    }
    let (a, b) = (g.node_ix(Id(1)).unwrap(), g.node_ix(Id(2)).unwrap());
    assert_eq!(
        g.edges_out(a)
            .map(|s| g.edge_at(s.edge).id())
            .collect::<Vec<_>>(),
        [Id(20)]
    );
    assert_eq!(
        g.edges_in(b)
            .map(|s| g.edge_at(s.edge).id())
            .collect::<Vec<_>>(),
        [Id(20)]
    );
    assert_eq!(g.node_at(g.edges_out(a).next().unwrap().other).id(), Id(2));
    assert_eq!(g.list_edges(None, None, None).len(), 1);
    let out: Vec<_> = g
        .edges_out_external(a)
        .map(|(e, k, graph, id)| (g.edge_at(e).id(), k.as_str(), graph, id))
        .collect();
    let inc: Vec<_> = g
        .edges_in_external(b)
        .map(|(e, k, graph, id)| (g.edge_at(e).id(), k.as_str(), graph, id))
        .collect();
    assert_eq!(out, [(Id(21), "blocks", Id(90), Id(2))]);
    assert_eq!(inc, [(Id(22), "blocks", Id(91), Id(1))]);
    assert_eq!(
        g.edge_ends(g.edge(Id(21)).unwrap().ix()),
        (EdgeEnd::Local(a), EdgeEnd::External(Id(90), Id(2)))
    );
    assert_eq!(
        g.edge_ends(g.edge(Id(23)).unwrap().ix()),
        (
            EdgeEnd::External(Id(90), Id(1)),
            EdgeEnd::External(Id(91), Id(2))
        )
    );
    assert!(g.remove_node(Id(1)).is_none());
    assert!(g.remove_node(Id(2)).is_none());
    g.remove_edge(Id(20));
    assert!(g.find_edge(Id(1), "blocks", Id(2)).is_none());
    assert!(
        g.remove_node(Id(1)).is_none(),
        "external outgoing edge still touches it"
    );
    assert!(
        g.remove_node(Id(2)).is_none(),
        "external incoming edge still touches it"
    );
    g.remove_edge(Id(21));
    g.remove_edge(Id(22));
    assert!(g.remove_node(Id(1)).is_some());
    assert!(g.remove_node(Id(2)).is_some());
    assert_eq!(g.edge_count(), 1, "a link graph needs no local nodes");
    assert_eq!(g.encode_edges(), umx_core::codec::encode_edges([&edges[3]]));
}

#[test]
fn every_edge_shows_once_from_each_end_in_edge_id_order() {
    let g = load(false);
    for e in g.edges() {
        let out: Vec<Step> = g
            .edges_out(e.from_ix().unwrap())
            .filter(|s| s.edge == e.ix())
            .collect();
        let inn: Vec<Step> = g
            .edges_in(e.to_ix().unwrap())
            .filter(|s| s.edge == e.ix())
            .collect();
        assert_eq!(out.len(), 1, "{:?} out", e.id());
        assert_eq!(inn.len(), 1, "{:?} in", e.id());
        assert_eq!(Some(out[0].other), e.to_ix());
        assert_eq!(Some(inn[0].other), e.from_ix());
        assert_eq!(out[0].kind.as_str(), e.kind());
        assert_eq!(Some(out[0].kind), g.kind_of(e.kind()));
        assert_eq!(g.edge_at(out[0].edge).id(), e.id());
    }
    let adj = adjacency(&g);
    let outs: usize = adj.iter().map(|(out, _)| out.len()).sum();
    let ins: usize = adj.iter().map(|(_, inn)| inn.len()).sum();
    assert_eq!((outs, ins), (g.edge_count(), g.edge_count()));
    for (out, inn) in &adj {
        for steps in [out, inn] {
            let ids = edge_ids(&g, steps);
            assert!(ids.windows(2).all(|w| w[0] < w[1]), "{ids:?}");
        }
    }
    let two = g.node_ix(Id(2)).unwrap();
    let out: Vec<Step> = g.edges_out(two).collect();
    let inn: Vec<Step> = g.edges_in(two).collect();
    assert_eq!(edge_ids(&g, &out), [Id(22), Id(24)]);
    assert_eq!(edge_ids(&g, &inn), [Id(20), Id(21), Id(22), Id(25)]);
}

#[test]
fn two_loads_of_the_same_records_read_the_same() {
    let (a, b) = (load(false), load(true));
    assert_eq!(adjacency(&a), adjacency(&b));
    let ids = |g: &Graph| g.nodes().map(NodeRef::id).collect::<Vec<_>>();
    assert_eq!(ids(&a), [Id(1), Id(2), Id(3)]);
    assert_eq!(ids(&a), ids(&b));
    assert_eq!(a.encode_nodes(), b.encode_nodes());
    assert_eq!(a.encode_edges(), b.encode_edges());
}

#[test]
fn a_view_reads_what_the_record_holds() {
    let g = load(false);
    let standard = [
        "id",
        "type",
        "title",
        "status",
        "note",
        "evidence",
        "status_history",
        "created",
        "updated",
        "created_by",
    ];
    for n in g.nodes() {
        let rec = n.to_node();
        for name in standard.iter().chain(&["likelihood", "missing"]) {
            assert_eq!(n.field(name), rec.field(name), "{name}");
        }
        assert!(standard.iter().all(|name| n.field(name).is_some()));
        assert_eq!(
            (n.id(), n.kind(), n.title(), n.status(), n.created_by()),
            (
                rec.id,
                rec.kind.as_str(),
                rec.title.as_str(),
                rec.status.as_str(),
                rec.created_by.as_str()
            )
        );
        assert_eq!(
            (n.created(), n.updated()),
            (rec.created.clone(), rec.updated.clone())
        );
        assert_eq!(n.custom_field("title"), None);
        assert_eq!(
            n.custom_field("likelihood"),
            rec.fields.get("likelihood").cloned()
        );
        assert!(n.custom_field_names().eq(["likelihood"]));
        assert_eq!(g.node_at(n.ix()).id(), n.id());
    }
    let e = g.edge(Id(23)).unwrap();
    let rec = e.to_edge();
    assert_eq!((e.from(), e.to(), e.kind()), (Id(3), Id(1), "part_of"));
    assert_eq!(e.field("from"), Some(json!(Id(3).to_text())));
    assert_eq!(e.field("type"), Some(json!("part_of")));
    assert_eq!(e.field("confidence"), rec.fields.get("confidence").cloned());
    assert_eq!(e.field("missing"), None);
    assert!(g.node(Id(9)).is_none() && g.node_ix(Id(9)).is_none());
}

#[test]
fn kinds_in_use_in_name_order_without_similar_to() {
    let g = load(false);
    let names = |ks: Vec<Kind>| ks.iter().map(|k| k.as_str().to_owned()).collect::<Vec<_>>();
    assert_eq!(
        names(g.relation_kinds()),
        ["blocks", "part_of", "relates_to"]
    );
    assert_eq!(names(g.node_kinds()), ["decision", "problem", "risk"]);
    let k = g.kind_of("similar_to").unwrap();
    assert_eq!(g.kind_name(k), "similar_to");
    assert!(g.kind_of("nope").is_none());
    let within = g.within(Id(1));
    assert_eq!(within.iter().collect::<Vec<_>>(), [Id(3)]);
    assert!(within.contains(Id(3)) && !within.contains(Id(2)) && within.len() == 1);
}
