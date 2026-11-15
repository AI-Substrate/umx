use serde_json::json;
use umx_core::{Edge, Id, Node, codec};
use umx_graph::Graph;

fn node(id: u128, kind: &str, key: Option<&str>) -> Node {
    Node {
        id: Id(id),
        kind: kind.into(),
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
        fields: key
            .map(|k| [("prov_key".into(), json!(k))].into())
            .unwrap_or_default(),
    }
}

fn edge(id: u128, author: Option<Id>) -> Edge {
    Edge {
        id: Id(id),
        kind: "supports".into(),
        from_graph: None,
        from: Id(10),
        to_graph: None,
        to: Id(20),
        created: "now".into(),
        created_by: "t".into(),
        authored_by_session: author,
        edited_by_session: None,
        evidence: Vec::new(),
        fields: [("custom".into(), json!([1, 2]))].into(),
    }
}

fn derived(g: &Graph) -> Vec<(Id, &'static str, Id)> {
    g.provenance_edges()
        .map(|e| (e.from.id(), e.kind, e.to.id()))
        .collect()
}

#[test]
fn provenance_memory_parts_reconcile_after_mutations_and_rollback() {
    let assert_parts = |g: &Graph| {
        let p = g.provenance_stats();
        assert_eq!(
            p.unique_index_bytes
                + p.bitset_bytes
                + p.authored_bytes
                + p.edited_bytes
                + p.edge_authored_bytes
                + p.edge_edited_bytes,
            g.memory_bytes().provenance
        );
    };
    let mut a = node(10, "problem", None);
    a.authored_by_session = Some(Id(30));
    a.edited_by_session = Some(Id(30));
    let mut g = Graph::from_records(
        vec![
            a.clone(),
            node(20, "problem", None),
            node(30, "session", Some("session/copilot/Abc_1234")),
        ],
        vec![edge(100, Some(Id(30)))],
    );
    assert_parts(&g);
    a.edited_by_session = Some(Id(40));
    g.insert_node(a.clone()).unwrap();
    assert_parts(&g);
    assert_eq!(
        (
            g.provenance_stats().authored_ids,
            g.provenance_stats().edited_ids
        ),
        (1, 2)
    );
    {
        let mut b = g.batch();
        a.authored_by_session = Some(Id(50));
        b.insert_node(a).unwrap();
        assert_parts(b.graph());
    }
    assert_parts(&g);
    g.remove_edge(Id(100));
    g.remove_node(Id(30));
    assert_parts(&g);
}
#[test]
fn metadata_never_changes_the_word_corpus_or_content_scores() {
    let mut a = node(10, "problem", None);
    a.title = "Approval delay".into();
    let mut b = node(20, "problem", None);
    b.title = "Budget approval".into();
    let mut user = node(30, "agent", None);
    user.title = "Approval control".into();
    let bare = Graph::from_records(vec![a, b, user], vec![]);
    let query = umx_graph::WordQuery::new("approval delay").unwrap();
    let stats = bare.word_stats(&query);
    let scores = |g: &Graph| {
        g.word_search(&query, &g.word_stats(&query), None, 10)
            .into_iter()
            .map(|(n, score, field)| (n.id(), score.to_bits(), field.to_string()))
            .collect::<Vec<_>>()
    };
    let expected = scores(&bare);
    let mut g = bare.clone();
    let mut metadata = node(1, "session", Some("session/copilot/h-0123456789abcdef"));
    metadata.title = "Approval delay".into();
    g.insert_node(metadata.clone()).unwrap();
    g.insert_node(node(99, "model", Some("model/test")))
        .unwrap();
    assert_eq!(g.provenance_count(), 2);
    assert_eq!(g.word_stats(&query), stats);
    assert_eq!(scores(&g), expected);
    let mut content = metadata.clone();
    content.kind = "problem".into();
    {
        let mut batch = g.batch();
        batch.insert_node(content.clone()).unwrap();
        assert_eq!(batch.graph().word_stats(&query).nodes, stats.nodes + 1);
    }
    assert_eq!(g.word_stats(&query), stats);
    g.insert_node(content).unwrap();
    assert_ne!(g.word_stats(&query), stats);
    g.insert_node(metadata).unwrap();
    assert_eq!(g.word_stats(&query), stats);
    assert_eq!(scores(&g), expected);
}

#[test]
fn filtered_resolution_ignores_hidden_ambiguity_and_subtrees_do_not_cross_hidden_nodes() {
    let g = Graph::from_records(
        vec![
            node(1, "session", Some("session/copilot/Abc_1234")),
            node(2, "problem", None),
        ],
        vec![],
    );
    let prefix = &Id(2).to_text()[..6];
    assert!(matches!(g.lookup(prefix), umx_graph::Lookup::Ambiguous(_)));
    assert!(matches!(
        g.lookup_where(prefix, |ix| !g.is_provenance(ix)),
        umx_graph::Lookup::Found(Id(2))
    ));
    let umx_graph::Lookup::NotFound(near) =
        g.lookup_where(&Id(1).to_text(), |ix| !g.is_provenance(ix))
    else {
        panic!("hidden ids cannot resolve");
    };
    assert_eq!(near, [Id(2)]);

    let mut links = Vec::new();
    for (id, from, to) in [(100, 20, 10), (101, 30, 20), (102, 40, 10)] {
        let mut e = edge(id, None);
        e.kind = "part_of".into();
        e.from = Id(from);
        e.to = Id(to);
        links.push(e);
    }
    let g = Graph::from_records(
        vec![
            node(10, "problem", None),
            node(20, "session", Some("session/copilot/Abc_1234")),
            node(30, "problem", None),
            node(40, "agent", None),
        ],
        links,
    );
    assert_eq!(g.within(Id(10)).len(), 3);
    assert_eq!(
        g.within_where(Id(10), |ix| !g.is_provenance(ix))
            .iter()
            .collect::<Vec<_>>(),
        [Id(40)]
    );
}

fn fresh(g: &Graph) {
    let (mut loaded, _) = Graph::load_nodes(g.encode_nodes()).unwrap();
    loaded.load_edges(&g.encode_edges()).unwrap();
    for n in g.nodes() {
        assert_eq!(
            g.is_provenance(n.ix()),
            loaded.is_provenance(loaded.node_ix(n.id()).unwrap())
        );
        assert_eq!(n.to_node(), loaded.node(n.id()).unwrap().to_node());
        assert_eq!(n.authored_by_session(), n.to_node().authored_by_session);
        assert_eq!(n.edited_by_session(), n.to_node().edited_by_session);
        if let Some(key) = n
            .custom_field("prov_key")
            .and_then(|v| v.as_str().map(str::to_owned))
        {
            assert_eq!(
                g.provenance_node(&key).map(|n| n.id()),
                loaded.provenance_node(&key).map(|n| n.id())
            );
        }
    }
    assert_eq!(derived(g), derived(&loaded));
    assert_eq!(
        g.duplicate_prov_keys().collect::<Vec<_>>(),
        loaded.duplicate_prov_keys().collect::<Vec<_>>()
    );
    for e in g.edges() {
        assert_eq!(e.authored_by_session(), e.to_edge().authored_by_session);
        assert_eq!(e.to_edge(), loaded.edge(e.id()).unwrap().to_edge());
    }
}

#[test]
fn duplicates_are_preserved_case_is_exact_and_user_reserved_types_stay_content() {
    let records = [
        node(1, "agent", Some("agent/pij/pij-seat")),
        node(2, "agent", Some("agent/pij/pij-seat")),
        node(3, "session", Some("session/copilot/Abc_1234")),
        node(4, "session", Some("session/copilot/abc_1234")),
        node(5, "agent", None),
        node(6, "problem", Some("agent/pij/pij-content")),
        node(7, "agent", Some("../bad")),
        node(8, "model", Some("agent/pij/pij-wrong-kind")),
    ];
    let bytes = codec::encode_nodes(&records);
    let (g, load) = Graph::load_nodes(bytes.clone()).unwrap();
    assert_eq!(g.encode_nodes(), bytes);
    assert_eq!(g.node_count(), records.len());
    assert_eq!(g.provenance_node("agent/pij/pij-seat").unwrap().id(), Id(1));
    assert_eq!(
        g.provenance_node("session/copilot/Abc_1234").unwrap().id(),
        Id(3)
    );
    assert_eq!(
        g.provenance_node("session/copilot/abc_1234").unwrap().id(),
        Id(4)
    );
    let duplicates = g.duplicate_prov_keys().collect::<Vec<_>>();
    assert_eq!(load.duplicate_prov_keys, duplicates);
    assert_eq!(duplicates.len(), 1);
    assert_eq!(
        (
            duplicates[0].node,
            duplicates[0].in_use,
            duplicates[0].code()
        ),
        (Id(2), Id(1), "duplicate_prov_key")
    );
    for n in g.nodes() {
        assert_eq!(g.is_provenance(n.ix()), n.id().0 <= 4);
    }
    assert!(g.provenance_node("agent/pij/pij-wrong-kind").is_none());
    // Every holder of a key, lowest first (plan 188): the duplicate too; exact case; nothing for a bad key.
    let holders = |key: &str| {
        g.provenance_holders(key)
            .map(|n| n.id())
            .collect::<Vec<_>>()
    };
    assert_eq!(holders("agent/pij/pij-seat"), [Id(1), Id(2)]);
    assert_eq!(holders("session/copilot/abc_1234"), [Id(4)]);
    assert!(holders("agent/pij/pij-content").is_empty());
    assert!(holders("../bad").is_empty());
    fresh(&g);
}

#[test]
fn derived_views_resolve_only_local_sessions_and_never_erase_dangling_keys() {
    let mut content = node(10, "problem", None);
    content.authored_by_session = Some(Id(0));
    content.edited_by_session = Some(Id(1));
    let mut dangling = node(20, "problem", None);
    dangling.authored_by_session = Some(Id(99));
    dangling.edited_by_session = Some(Id(2));
    let nodes = [
        node(0, "session", Some("session/copilot/Abc_1234")),
        node(1, "session", Some("session/copilot/Def_5678")),
        node(2, "session", None),
        content,
        dangling,
    ];
    let mut g = Graph::from_records(nodes.to_vec(), vec![edge(30, Some(Id(0)))]);
    assert_eq!(
        derived(&g),
        [(Id(10), "authored_by", Id(0)), (Id(10), "edited_by", Id(1))]
    );
    assert_eq!(g.node(Id(20)).unwrap().authored_by_session(), Some(Id(99)));
    assert_eq!(g.edge(Id(30)).unwrap().authored_by_session(), Some(Id(0)));
    assert_eq!(
        g.encode_edges(),
        codec::encode_edges([&edge(30, Some(Id(0)))])
    );
    assert_eq!(g.edge_count(), 1);
    assert!(g.memory_bytes().provenance >= g.node_count() * 8 + g.edge_count() * 4);
    let memory = g.memory_bytes();
    assert_eq!(
        memory.total(),
        memory.hot()
            + memory.field_columns
            + memory.image
            + memory.cold
            + memory.word_index
            + memory.vectors
            + memory.similarity
            + memory.external
            + memory.provenance
    );
    fresh(&g);
    g.remove_node(Id(0)).unwrap();
    assert_eq!(g.node(Id(10)).unwrap().authored_by_session(), Some(Id(0)));
    assert_eq!(derived(&g), [(Id(10), "edited_by", Id(1))]);
    fresh(&g);
}

#[test]
fn edits_removals_out_of_order_inserts_and_rollback_keep_every_index_current() {
    let mut content = node(10, "problem", None);
    content.authored_by_session = Some(Id(20));
    let mut g = Graph::from_records(
        vec![
            content,
            node(20, "session", Some("session/copilot/Abc_1234")),
        ],
        vec![],
    );
    let key = "session/copilot/Abc_1234";
    let mut session = g.node(Id(20)).unwrap().to_node();
    session.kind = "problem".into();
    g.insert_node(session.clone()).unwrap();
    assert!(!g.is_provenance(g.node_ix(Id(20)).unwrap()));
    assert!(g.provenance_node(key).is_none());
    assert!(derived(&g).is_empty());
    fresh(&g);
    session.kind = "session".into();
    g.insert_node(session.clone()).unwrap();
    assert!(g.is_provenance(g.node_ix(Id(20)).unwrap()));
    assert_eq!(derived(&g), [(Id(10), "authored_by", Id(20))]);
    session.fields.insert("prov_key".into(), json!("bad"));
    g.insert_node(session).unwrap();
    assert!(!g.is_provenance(g.node_ix(Id(20)).unwrap()));
    g.insert_node(node(20, "session", Some(key))).unwrap();
    g.insert_node(node(5, "session", Some(key))).unwrap();
    assert_eq!(g.provenance_node(key).unwrap().id(), Id(5));
    assert_eq!(g.duplicate_prov_keys().next().unwrap().node, Id(20));
    fresh(&g);
    g.remove_node(Id(5)).unwrap();
    assert_eq!(g.provenance_node(key).unwrap().id(), Id(20));
    fresh(&g);
    let bytes = g.encode_nodes();
    {
        let mut b = g.batch();
        b.insert_node(node(1, "session", Some(key))).unwrap();
        let mut content = b.graph().node(Id(10)).unwrap().to_node();
        content.edited_by_session = Some(Id(1));
        b.insert_node(content).unwrap();
        b.insert_edge(edge(35, Some(Id(1))));
        assert_eq!(b.graph().provenance_node(key).unwrap().id(), Id(1));
    }
    assert_eq!(g.encode_nodes(), bytes);
    assert_eq!(g.edge_count(), 0);
    fresh(&g);
    {
        let mut b = g.batch();
        b.insert_edge(edge(40, Some(Id(20))));
        b.insert_edge(edge(30, None));
        b.commit();
    }
    assert_eq!(g.edge(Id(40)).unwrap().authored_by_session(), Some(Id(20)));
    assert_eq!(g.edge(Id(30)).unwrap().authored_by_session(), None);
    fresh(&g);
    g.remove_edge(Id(40));
    g.insert_edge(edge(30, Some(Id(999))));
    fresh(&g);
    g.remove_edge(Id(30));
    g.remove_node(Id(20)).unwrap();
    assert!(g.provenance_node(key).is_none());
    fresh(&g);
}

#[test]
fn inserting_a_lower_id_rebinds_unique_keys_to_the_new_handles() {
    let key = "session/copilot/Abc_1234";
    let mut g = Graph::from_records(
        vec![node(10, "problem", None), node(20, "session", Some(key))],
        vec![],
    );
    g.insert_node(node(5, "problem", None)).unwrap();
    assert_eq!(g.provenance_node(key).unwrap().id(), Id(20));
    assert_eq!(g.duplicate_prov_keys().count(), 0);
    assert!(g.is_provenance(g.node_ix(Id(20)).unwrap()));
    assert!(!g.is_provenance(g.node_ix(Id(10)).unwrap()));
    fresh(&g);
}

#[test]
fn old_bytes_and_noncanonical_untouched_lines_stay_byte_identical() {
    let old = format!(
        "{{ \"id\":\"{}\", \"type\":\"problem\", \"title\":\"old\", \"status\":\"open\", \"note\":\"\", \"evidence\":[], \"status_history\":[], \"created\":\"\", \"updated\":\"\", \"created_by\":\"old\", \"unknown\":{{\"keep\":true}} }}\n",
        Id(1)
    );
    let (g, _) = Graph::load_nodes(old.as_bytes().to_vec()).unwrap();
    assert_eq!(g.encode_nodes(), old.as_bytes());
    assert_eq!(g.node(Id(1)).unwrap().authored_by_session(), None);
    let with_keys = old.replace(
        "\"unknown\":",
        &format!(
            "\"authored_by_session\":\"{}\", \"edited_by_session\":\"{}\", \"unknown\":",
            Id(3),
            Id(4)
        ),
    );
    let (mut g, _) = Graph::load_nodes(with_keys.as_bytes().to_vec()).unwrap();
    assert_eq!(g.encode_nodes(), with_keys.as_bytes());
    assert!(
        !g.node(Id(1))
            .unwrap()
            .custom_field_names()
            .any(|k| k.ends_with("_by_session"))
    );
    g.insert_node(g.node(Id(1)).unwrap().to_node()).unwrap();
    assert_eq!(g.encode_nodes(), with_keys.as_bytes());
    fresh(&g);
}

#[test]
fn word_index_excludes_provenance_and_reserved_keys_but_not_user_reserved_types() {
    let mut provenance = node(1, "agent", Some("agent/pij/pij-calm-otter"));
    provenance.title = "hiddenmarker".into();
    provenance.note = "hiddennote".into();
    let mut content = node(2, "agent", None);
    content.title = "visiblemarker".into();
    content
        .fields
        .insert("prov_key".into(), json!("unindexedmarker"));
    content
        .fields
        .insert("authored_by_session".into(), json!("unindexedmarker"));
    let fields = umx_core::ontology::TextFields::strings(
        umx_core::ontology::PROVENANCE_KEYS.iter().copied(),
    );
    let mut g = Graph::from_records_indexing(vec![provenance, content], vec![], fields);
    let hits = |g: &Graph, text: &str| {
        let q = umx_graph::WordQuery::new(text).unwrap();
        g.word_search(&q, &g.word_stats(&q), None, 10)
            .into_iter()
            .map(|(n, _, _)| n.id())
            .collect::<Vec<_>>()
    };
    assert_eq!(hits(&g, "visiblemarker"), [Id(2)]);
    for text in ["hiddenmarker", "hiddennote", "unindexedmarker"] {
        assert!(hits(&g, text).is_empty());
    }
    let mut changed = g.node(Id(1)).unwrap().to_node();
    changed.fields.remove("prov_key");
    g.insert_node(changed).unwrap();
    assert_eq!(hits(&g, "hiddenmarker"), [Id(1)]);
    fresh(&g);
}

#[test]
fn malformed_provenance_loads_is_kept_warned_and_edit_stamping_removes_only_bad_editor() {
    let node_bytes: &[u8] = include_bytes!("fixtures/provenance-invalid/nodes.jsonl");
    let edge_bytes: &[u8] = include_bytes!("fixtures/provenance-invalid/edges.jsonl");
    let (mut g, nodes) = Graph::load_nodes(node_bytes.to_vec()).unwrap();
    let edges = g.load_edges(edge_bytes).unwrap();
    assert_eq!(
        nodes
            .invalid_provenance
            .iter()
            .map(|i| (i.line, i.key))
            .collect::<Vec<_>>(),
        [
            (1, "prov_key"),
            (1, "authored_by_session"),
            (1, "edited_by_session")
        ]
    );
    assert_eq!(
        edges
            .invalid_provenance
            .iter()
            .map(|i| (i.line, i.key))
            .collect::<Vec<_>>(),
        [(1, "authored_by_session"), (1, "edited_by_session")]
    );
    assert_eq!(
        g.nodes().map(|n| n.to_node()).collect::<Vec<_>>(),
        codec::decode_nodes(node_bytes).unwrap().records
    );
    assert_eq!(g.encode_nodes(), node_bytes);
    assert_eq!(g.edge(Id(3)).unwrap().authored_by_session(), None);
    assert_eq!(
        g.edge(Id(3)).unwrap().field("authored_by_session"),
        Some(json!(7))
    );
    let canonical_edges = g.encode_edges();
    assert_eq!(
        canonical_edges,
        codec::encode_edges(&codec::decode_edges(edge_bytes).unwrap().records)
    );
    g.insert_node(node(10, "problem", None)).unwrap();
    assert!(g.encode_nodes().starts_with(node_bytes));
    assert_eq!(g.encode_edges(), canonical_edges);
    let (mut loaded, _) = Graph::load_nodes(g.encode_nodes()).unwrap();
    assert_eq!(
        loaded
            .load_edges(&canonical_edges)
            .unwrap()
            .invalid_provenance
            .len(),
        2
    );
    assert_eq!(loaded.encode_edges(), canonical_edges);
    let mut changed = loaded.node(Id(1)).unwrap().to_node();
    changed.record_edit(Id(42));
    loaded.insert_node(changed).unwrap();
    let changed = loaded.node(Id(1)).unwrap().to_node();
    assert_eq!(changed.authored_by_session, None);
    assert_eq!(changed.edited_by_session, Some(Id(42)));
    assert_eq!(changed.fields["authored_by_session"], json!(null));
    assert!(!changed.fields.contains_key("edited_by_session"));
    let encoded = String::from_utf8(loaded.encode_nodes()).unwrap();
    assert_eq!(encoded.matches("\"edited_by_session\":").count(), 1);
    assert_eq!(encoded.matches("\"authored_by_session\":").count(), 1);
    fresh(&loaded);
}
