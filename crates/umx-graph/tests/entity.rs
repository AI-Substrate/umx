//! ws008 E1: the entity key index follows every row change (insert, replace, remove, a load, a rebuild), and
//! `--where` on a list field matches an element.

use serde_json::json;
use umx_core::{Id, Node, Value};
use umx_graph::{Graph, NodeFilter, Op, Where};

fn node(id: u128, kind: &str, keys: Option<Value>) -> Node {
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
        fields: keys
            .map(|k| [("entity_key".into(), k)].into())
            .unwrap_or_default(),
    }
}

fn dups(g: &Graph) -> Vec<(u128, u128, String)> {
    g.duplicate_entity_keys()
        .map(|d| (d.node.0, d.in_use.0, d.key))
        .collect()
}

fn found(g: &Graph, kind: &str, key: &str) -> Option<u128> {
    g.entity_node(kind, key).map(|n| n.id().0)
}

#[test]
fn the_index_follows_every_row_change_and_matches_only_within_a_type() {
    let mut g = Graph::new();
    g.insert_node(node(10, "person", Some(json!(["email:a@x.com", "upn:a"]))))
        .unwrap();
    g.insert_node(node(20, "organisation", Some(json!(["email:a@x.com"]))))
        .unwrap();
    g.insert_node(node(30, "person", Some(json!(["email:A@X.com"]))))
        .unwrap();
    assert_eq!(found(&g, "person", "email:a@x.com"), Some(10));
    assert_eq!(found(&g, "organisation", "email:a@x.com"), Some(20));
    assert_eq!(found(&g, "person", "upn:a"), Some(10));
    // A hand-written key is indexed in stored form; another type never counts as a repeat.
    assert_eq!(dups(&g), [(30, 10, "email:a@x.com".to_string())]);

    // Replace: the old keys leave the index.
    g.insert_node(node(10, "person", Some(json!(["upn:a"]))))
        .unwrap();
    assert_eq!(found(&g, "person", "email:a@x.com"), Some(30));
    assert!(dups(&g).is_empty());

    // A load and a rebuild give the same index.
    let (loaded, _) = Graph::load_nodes(g.encode_nodes()).unwrap();
    assert_eq!(found(&loaded, "person", "email:a@x.com"), Some(30));
    assert_eq!(found(&loaded, "person", "upn:a"), Some(10));

    // Remove: a lower node going renumbers the rest, and the index still names the right ids.
    g.insert_node(node(5, "person", Some(json!(["upn:b"]))))
        .unwrap();
    g.remove_node(Id(10));
    assert_eq!(found(&g, "person", "upn:a"), None);
    assert_eq!(found(&g, "person", "upn:b"), Some(5));
    assert_eq!(found(&g, "person", "email:a@x.com"), Some(30));
    g.remove_node(Id(30));
    assert_eq!(found(&g, "person", "email:a@x.com"), None);
}

#[test]
fn where_on_a_list_matches_an_element_and_not_equal_is_its_complement() {
    let mut g = Graph::new();
    g.insert_node(node(1, "person", Some(json!(["email:a@x.com", "upn:a"]))))
        .unwrap();
    g.insert_node(node(2, "person", Some(json!(["upn:b"]))))
        .unwrap();
    g.insert_node(node(3, "person", None)).unwrap();
    let ids = |op: Op, value: &str| -> Vec<u128> {
        let filter = NodeFilter {
            kind: None,
            status: None,
            wheres: vec![Where {
                field: "entity_key".into(),
                op,
                value: json!(value),
            }],
            within: None,
        };
        g.list_nodes(&filter).iter().map(|n| n.id().0).collect()
    };
    assert_eq!(ids(Op::Eq, "upn:a"), [1]);
    assert_eq!(ids(Op::Eq, "upn:b"), [2]);
    assert_eq!(
        ids(Op::Ne, "upn:a"),
        [2],
        "no element equal; a node without the field never matches"
    );
    assert!(ids(Op::Gt, "a").is_empty(), "ordering never matches a list");
}
