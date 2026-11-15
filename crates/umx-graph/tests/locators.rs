//! ws006 S3 (req-77; workshop 010 9a): resource nodes in umx-graph. The locator is kept on its line in the pinned key
//! order, its kind and format in a sparse column, and its key in the one locator index; a duplicate locator after a
//! merge loads and is listed, lookups return the lower id; graphs without resources keep their bytes.

use umx_core::{Format, Id, Locator, LocatorKind, Node, codec};
use umx_graph::{DuplicateLocator, Graph};

fn node(id: u128, title: &str, locator: Option<&str>) -> Node {
    Node {
        id: Id(id),
        kind: "document".into(),
        title: title.into(),
        status: "open".into(),
        note: String::new(),
        evidence: vec![],
        status_history: vec![],
        created: "now".into(),
        updated: "now".into(),
        created_by: "t".into(),
        locator: locator.map(|l| Locator::parse(l).unwrap()),
        authored_by_session: None,
        edited_by_session: None,
        fields: Default::default(),
    }
}

fn loaded(nodes: Vec<Node>) -> Graph {
    let bytes = codec::encode_nodes(&nodes);
    let (g, _) = Graph::load_nodes(bytes.clone()).unwrap();
    assert_eq!(g.encode_nodes(), bytes, "a load keeps every byte");
    g.check_rows();
    g
}

#[test]
fn resource_rows_keep_their_locator_kind_format_and_bytes() {
    let mut full = node(4, "Call", Some("t/016.ndjson"));
    full.locator.as_mut().unwrap().hash = Some("sha256:9f2c".into());
    full.locator.as_mut().unwrap().length = Some(1843302);
    full.locator.as_mut().unwrap().seq_max = Some(3511);
    full.authored_by_session = Some(Id(90));
    full.edited_by_session = Some(Id(91));
    let g = loaded(vec![
        node(1, "Plain", None),
        node(2, "Brief", Some("docs/brief.md")),
        node(3, "Specs", Some("specs/")),
        full.clone(),
        node(5, "Site", Some("https://Example.com/a")),
    ]);
    let at = |id| g.node_ix(Id(id)).unwrap();
    assert_eq!(g.locator_kind(at(1)), None);
    assert_eq!(
        [2, 3, 4, 5].map(|id| g.locator_kind(at(id))),
        [
            Some((LocatorKind::File, Some(Format::Markdown))),
            Some((LocatorKind::Folder, None)),
            Some((LocatorKind::File, Some(Format::TranscriptNdjson))),
            Some((LocatorKind::Url, Some(Format::Other))),
        ]
    );
    assert!(g.has_locators());
    assert_eq!(
        g.resources().map(|n| n.id()).collect::<Vec<_>>(),
        [Id(2), Id(3), Id(4), Id(5)]
    );
    let find = |text: &str| g.by_locator(&Locator::parse(text).unwrap()).map(|n| n.id());
    assert_eq!(find("./docs//brief.md"), Some(Id(2)));
    assert_eq!(find("specs/"), Some(Id(3)));
    assert_eq!(find("specs"), None, "a file is not the folder of one name");
    assert_eq!(find("https://example.com:443/a#x"), Some(Id(5)));
    assert_eq!(g.duplicate_locators().count(), 0);
    let line = String::from_utf8(g.encode_nodes()).unwrap();
    assert!(
        line.contains(&format!(
            "\"created_by\":\"t\",\"locator\":{{\"kind\":\"file\",\"format\":\"transcript_ndjson\",\"source\":\"t/016.ndjson\",\"hash\":\"sha256:9f2c\",\"length\":1843302,\"seq_max\":3511}},\"authored_by_session\":\"{}\",\"edited_by_session\":\"{}\"}}",
            Id(90),
            Id(91)
        )),
        "{line}"
    );
}

#[test]
fn a_graph_without_resources_has_none_and_keeps_its_bytes() {
    for bytes in [
        include_bytes!("../../umx-cypher/fixtures/design/nodes.jsonl").as_slice(),
        include_bytes!("../../umx/tests/fixtures/query-graph/nodes.jsonl").as_slice(),
    ] {
        let (g, _) = Graph::load_nodes(bytes.to_vec()).unwrap();
        assert!(!g.has_locators());
        assert_eq!(g.encode_nodes(), bytes);
        assert!(!String::from_utf8_lossy(&g.encode_nodes()).contains("\"locator\""));
    }
}

/// Workshop 010 9a: both branches of a merge added the same source. Both load, every line stays, the lower id is
/// in use; a hand-edited (unnormalised) source is keyed as it normalises and listed.
#[test]
fn a_duplicate_locator_after_a_merge_loads_and_is_listed() {
    let mut theirs = node(7, "Brief (theirs)", Some("docs/brief.md"));
    theirs.locator.as_mut().unwrap().source = "./docs//brief.md".into();
    let g = loaded(vec![
        node(3, "Brief (ours)", Some("docs/brief.md")),
        node(5, "Other", Some("docs/other.md")),
        theirs,
    ]);
    assert_eq!(
        g.duplicate_locators().collect::<Vec<_>>(),
        [DuplicateLocator {
            node: Id(7),
            in_use: Id(3),
        }]
    );
    assert_eq!(g.unnormalised_locators().collect::<Vec<_>>(), [Id(7)]);
    assert_eq!(
        g.by_locator(&Locator::parse("docs/brief.md").unwrap())
            .map(|n| n.id()),
        Some(Id(3))
    );
    assert_eq!(g.node_count(), 3, "nothing is erased");
}

/// Writes keep the index true: a replace re-keys the row, a removal drops it, and a handle shift (an insert before
/// it) moves it; check_rows re-derives every row and compares.
#[test]
fn writes_keep_the_locator_index_true() {
    let mut g = loaded(vec![
        node(10, "A", Some("a.md")),
        node(20, "B", None),
        node(30, "C", Some("c.md")),
    ]);
    let find = |g: &Graph, text: &str| g.by_locator(&Locator::parse(text).unwrap()).map(|n| n.id());
    g.insert_node(node(20, "B", Some("b.md"))).unwrap();
    assert_eq!(find(&g, "b.md"), Some(Id(20)));
    g.insert_node(node(10, "A", Some("a2.md"))).unwrap();
    assert_eq!((find(&g, "a.md"), find(&g, "a2.md")), (None, Some(Id(10))));
    g.insert_node(node(5, "Before", Some("z.md"))).unwrap();
    assert_eq!(
        (find(&g, "z.md"), find(&g, "c.md")),
        (Some(Id(5)), Some(Id(30))),
        "handles moved"
    );
    g.remove_node(Id(30));
    assert_eq!(find(&g, "c.md"), None);
    g.insert_node(node(20, "B", None)).unwrap();
    assert_eq!(find(&g, "b.md"), None, "a plain node again");
    g.insert_node(node(40, "D", Some("a2.md"))).unwrap();
    assert_eq!(
        g.duplicate_locators().collect::<Vec<_>>(),
        [DuplicateLocator {
            node: Id(40),
            in_use: Id(10),
        }]
    );
    g.check_rows();
    let (again, _) = Graph::load_nodes(g.encode_nodes()).unwrap();
    assert_eq!(
        again.duplicate_locators().collect::<Vec<_>>(),
        g.duplicate_locators().collect::<Vec<_>>()
    );
    assert_eq!(
        g.node(Id(10)).unwrap().to_node().locator,
        Some(Locator::parse("a2.md").unwrap())
    );
}

/// graph-storage-expert, S241-1: a custom field named `locator` that is not a locator (an older graph's, or a
/// newer umx's kind) loads as that custom field: listed, not a resource, not indexed, and every byte kept. A newer
/// umx's extra locator key loads on a real resource and is kept.
#[test]
fn a_kept_locator_field_loads_as_a_custom_field_and_is_listed() {
    let mut old = node(2, "Old", None);
    old.fields
        .insert("locator".into(), serde_json::json!("an old custom value"));
    let mut newer_kind = node(3, "Newer", None);
    newer_kind.fields.insert(
        "locator".into(),
        serde_json::json!({ "kind": "passage", "of": "x", "selector": "seq=1" }),
    );
    let mut newer_key = node(4, "Brief", Some("docs/brief.md"));
    newer_key
        .locator
        .as_mut()
        .unwrap()
        .extra
        .insert("later".into(), serde_json::json!(1));
    let g = loaded(vec![node(1, "Plain", None), old, newer_kind, newer_key]);
    assert_eq!(g.kept_locator_fields(), [Id(2), Id(3)]);
    assert_eq!(g.resources().map(|n| n.id()).collect::<Vec<_>>(), [Id(4)]);
    let at = |id| g.node_ix(Id(id)).unwrap();
    assert_eq!(g.locator_kind(at(2)), None);
    assert_eq!(
        g.node(Id(2)).unwrap().custom_field("locator"),
        Some(serde_json::json!("an old custom value"))
    );
    let line = String::from_utf8(g.encode_nodes()).unwrap();
    assert!(
        line.contains("\"source\":\"docs/brief.md\",\"later\":1}"),
        "{line}"
    );
}
