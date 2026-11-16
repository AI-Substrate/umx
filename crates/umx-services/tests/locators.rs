//! ws006 S3a (req-77; workshop 010 9a): resource nodes through the services. A graph needs the locator format (5) only once a node
//! has a locator, written in the same set as the first locator line and never lowered; an older umx then refuses it.
//! A graph without resources keeps its graph.json bytes. A duplicate locator after a merge loads, warns
//! `duplicate_locator`, and `umx check` lists it; nothing is erased. A custom field named `locator` from before the
//! name was reserved loads, kept and warned, and no write sets a locator name as a custom field.

use std::sync::Arc;
use umx_core::codec::{self, GraphFile};
use umx_core::model::FORMAT_LOCATORS;
use umx_core::ports::GraphDir;
use umx_core::value::SetValue;
use umx_core::{Id, Locator, Node};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{Ctx, NodeAddRequest, NodeEditRequest, Ports, Services};

struct World {
    files: Arc<FakeGraphFiles>,
    svc: Services,
}

fn world() -> World {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let svc = Services::new(Ports {
        files: files.clone(),
        vector_cache: Arc::new(FakeGraphFiles::new(GraphDir::parse("/cache").unwrap())),
        vector_cache_root: GraphDir::parse("/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new().with_absent_ok(true)),
        sources: Arc::new(umx_fakes::FakeSourceReader::open(
            GraphDir::parse("/").unwrap(),
        )),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    })
    .with_source_roots(Arc::new(Repo));
    World { files, svc }
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

fn dir() -> GraphDir {
    GraphDir::parse("/work/g").unwrap()
}

fn resource(id: u128, title: &str, source: &str) -> Node {
    Node {
        id: Id(id),
        kind: "document".into(),
        title: title.into(),
        status: "open".into(),
        note: String::new(),
        evidence: vec![],
        status_history: vec![],
        created: "2026-11-12T00:00:00Z".into(),
        updated: "2026-11-12T00:00:00Z".into(),
        created_by: "tester".into(),
        locator: Some(Locator::parse("docs/brief.md").unwrap()),
        authored_by_session: None,
        edited_by_session: None,
        fields: Default::default(),
    }
    .with_source(source)
}

trait WithSource {
    fn with_source(self, source: &str) -> Node;
}

impl WithSource for Node {
    fn with_source(mut self, source: &str) -> Node {
        self.locator.as_mut().unwrap().source = source.into();
        self
    }
}

fn add_plain(w: &mut World, title: &str) {
    w.svc
        .node_add(
            &ctx().graph("demo"),
            NodeAddRequest {
                on_key_resolve: false,
                raised_in: None,
                kind: "problem".into(),
                title: title.into(),
                status: None,
                note: None,
                sets: Vec::new(),
                parent: None,
                evidence: Vec::new(),
                evidence_kind: None,
                locator: None,
            },
        )
        .unwrap();
}

fn raw(w: &World, file: GraphFile) -> Vec<u8> {
    w.files.raw_for_tests(&dir(), file).unwrap_or_default()
}

fn umx_of(w: &World) -> u32 {
    codec::decode_graph(&raw(w, GraphFile::Graph)).unwrap().umx
}

/// A demo graph whose nodes.jsonl gained `extra` resource lines while the daemon was away (a merge, a hand edit),
/// loaded again.
fn with_resources(extra: &[Node]) -> (World, Vec<umx_core::Problem>) {
    let mut w = world();
    w.svc
        .graph_new(&ctx(), "/work/g", "demo", None, false, None)
        .unwrap();
    add_plain(&mut w, "Plain one");
    let mut nodes = codec::decode_nodes(&raw(&w, GraphFile::Nodes))
        .unwrap()
        .records;
    nodes.extend_from_slice(extra);
    w.files
        .tamper_for_tests(&dir(), GraphFile::Nodes, &codec::encode_nodes(&nodes));
    let done = w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    (w, done.notes)
}

#[test]
fn a_graph_without_resources_keeps_its_format_and_graph_json_bytes() {
    let mut w = world();
    w.svc
        .graph_new(&ctx(), "/work/g", "demo", None, false, None)
        .unwrap();
    let before = raw(&w, GraphFile::Graph);
    add_plain(&mut w, "Plain one");
    add_plain(&mut w, "Plain two");
    assert_eq!(raw(&w, GraphFile::Graph), before, "graph.json untouched");
    assert!(umx_of(&w) < FORMAT_LOCATORS);
    assert!(
        !String::from_utf8(raw(&w, GraphFile::Nodes))
            .unwrap()
            .contains("locator")
    );
}

#[test]
fn the_first_write_with_a_locator_raises_the_graph_to_the_locator_format_and_never_lowers_it() {
    let (mut w, warnings) = with_resources(&[resource(9_000, "Brief", "docs/brief.md")]);
    assert!(warnings.is_empty(), "{warnings:?}");
    let had = umx_of(&w);
    assert!(had < FORMAT_LOCATORS, "loading writes nothing");
    add_plain(&mut w, "Plain two");
    let graph_json = raw(&w, GraphFile::Graph);
    assert_eq!(
        umx_of(&w),
        FORMAT_LOCATORS,
        "raised in the same set as a write"
    );
    assert!(
        String::from_utf8(raw(&w, GraphFile::Nodes))
            .unwrap()
            .contains("\"locator\":{\"kind\":\"file\",\"format\":\"markdown\",\"source\":\"docs/brief.md\"}")
    );
    // An older umx refuses it as graph_format_newer: umx-core's codec test
    // `the_locator_format_is_written_for_locators_and_refused_by_an_older_reader`.
    assert_eq!(
        codec::decode_graph(&graph_json).unwrap().umx,
        FORMAT_LOCATORS
    );
    // Never lowered, even once no node has a locator.
    let mut nodes = codec::decode_nodes(&raw(&w, GraphFile::Nodes))
        .unwrap()
        .records;
    nodes.retain(|n| n.locator.is_none());
    w.files
        .tamper_for_tests(&dir(), GraphFile::Nodes, &codec::encode_nodes(&nodes));
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    add_plain(&mut w, "Plain three");
    assert_eq!(umx_of(&w), FORMAT_LOCATORS);
}

#[test]
fn a_duplicate_locator_after_a_merge_loads_warns_and_is_checked() {
    let (w, warnings) = with_resources(&[
        resource(9_000, "Brief (ours)", "docs/brief.md"),
        resource(9_001, "Other", "docs/other.md"),
        resource(9_002, "Brief (theirs)", "./docs//brief.md"),
    ]);
    let codes: Vec<(&str, Option<&str>)> = warnings
        .iter()
        .map(|w| (w.code, w.subject.as_deref()))
        .collect();
    let theirs = Id(9_002).to_text();
    assert_eq!(
        codes,
        [
            ("duplicate_locator", Some(theirs.as_str())),
            ("locator_not_normalised", Some(theirs.as_str())),
        ]
    );
    let check = w.svc.check(&ctx().graph("demo"), None).unwrap();
    let rows: Vec<(&str, &str)> = check
        .value
        .page
        .rows
        .iter()
        .map(|r| (r.kind, r.subject.as_str()))
        .collect();
    assert!(
        rows.contains(&("duplicate_locator", theirs.as_str())),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("locator_not_normalised", theirs.as_str())),
        "{rows:?}"
    );
    let nodes = codec::decode_nodes(&raw(&w, GraphFile::Nodes))
        .unwrap()
        .records;
    assert_eq!(
        nodes.iter().filter(|n| n.locator.is_some()).count(),
        3,
        "nothing is erased"
    );
}

/// graph-storage-expert, S241-1: a custom field named `locator` that is not a locator (an older graph's) never
/// fails the load: it loads as that custom field, warns `locator_field_kept`, `umx check` lists it, and other writes
/// keep it as it is.
#[test]
fn a_custom_field_named_locator_loads_kept_warned_and_checked() {
    let mut old = resource(9_000, "Old", "docs/old.md");
    old.locator = None;
    old.fields
        .insert("locator".into(), serde_json::json!("an old custom value"));
    let (mut w, warnings) = with_resources(&[old]);
    let id = Id(9_000).to_text();
    let codes: Vec<(&str, Option<&str>)> = warnings
        .iter()
        .map(|w| (w.code, w.subject.as_deref()))
        .collect();
    assert_eq!(codes, [("locator_field_kept", Some(id.as_str()))]);
    let check = w.svc.check(&ctx().graph("demo"), None).unwrap();
    assert!(
        check
            .value
            .page
            .rows
            .iter()
            .any(|r| r.kind == "locator_field_kept" && r.subject == id),
        "{:?}",
        check.value.page.rows
    );
    add_plain(&mut w, "Plain two");
    let text = String::from_utf8(raw(&w, GraphFile::Nodes)).unwrap();
    assert!(
        text.contains("\"locator\":\"an old custom value\""),
        "{text}"
    );
    assert!(
        umx_of(&w) < FORMAT_LOCATORS,
        "a kept field is not a locator"
    );
}

/// S241-1: no write sets `locator` or a `locator_*` name as a custom field (`locator_field_invalid`), on add or
/// edit, and nothing changes.
#[test]
fn no_write_sets_a_locator_name_as_a_custom_field() {
    let (mut w, _) = with_resources(&[]);
    let id = codec::decode_nodes(&raw(&w, GraphFile::Nodes))
        .unwrap()
        .records
        .into_iter()
        .find(|node| node.kind == "problem" && node.title == "Plain one")
        .expect("seeded content node")
        .id
        .to_text();
    let before = raw(&w, GraphFile::Nodes);
    for name in ["locator", "locator_kind"] {
        let set = || vec![(name.to_string(), SetValue::Str("docs/brief.md".into()))];
        let edit = w.svc.node_edit(
            &ctx().graph("demo"),
            &id,
            NodeEditRequest {
                sets: set(),
                ..Default::default()
            },
        );
        assert_eq!(edit.unwrap_err().code, "locator_field_invalid", "{name}");
        let add = w.svc.node_add(
            &ctx().graph("demo"),
            NodeAddRequest {
                on_key_resolve: false,
                raised_in: None,
                kind: "problem".into(),
                title: "New".into(),
                status: None,
                note: None,
                sets: set(),
                parent: None,
                evidence: Vec::new(),
                evidence_kind: None,
                locator: None,
            },
        );
        assert_eq!(add.unwrap_err().code, "locator_field_invalid", "{name}");
        assert_eq!(raw(&w, GraphFile::Nodes), before, "nothing changed");
    }
}

// ws006 S3b: the resource verbs.

/// The machine's git top for these tests: /work holds every graph.
struct Repo;

impl umx_services::SourceRoots for Repo {
    fn root_folder(&self, _: &str) -> Option<GraphDir> {
        None
    }
    fn repo_top(&self, dir: &GraphDir) -> Option<GraphDir> {
        let top = GraphDir::parse("/work").unwrap();
        dir.is_within(&top).then_some(top)
    }
}

fn repo_world() -> World {
    let mut w = world();
    w.svc
        .graph_new(&ctx(), "/work/g", "demo", None, false, None)
        .unwrap();
    w
}

fn add(
    w: &mut World,
    graph: &str,
    locator: &str,
) -> umx_services::Outcome<umx_services::NodeAdded> {
    w.svc.node_add(
        &ctx().graph(graph),
        NodeAddRequest {
            kind: "document".into(),
            locator: Some(locator.into()),
            ..Default::default()
        },
    )
}

fn edit(
    w: &mut World,
    node: Id,
    req: NodeEditRequest,
) -> umx_services::Outcome<umx_services::NodeEdited> {
    w.svc.node_edit(&ctx().graph("demo"), &node.to_text(), req)
}

/// The warnings' codes, without the vocabulary ones (the second graph has no starter types).
fn codes(warnings: &[umx_core::Problem]) -> Vec<&str> {
    warnings
        .iter()
        .map(|w| w.code)
        .filter(|c| !c.starts_with("off_vocabulary"))
        .collect()
}

/// req-77: node add --locator titles the node from its source and raises the format in the same write; adding the
/// same source again, in any spelling of it (graph-addressing-expert N3: the root:repo/ form too), returns the
/// same node with resource_exists and writes nothing.
#[test]
fn node_add_with_a_locator_titles_it_and_returns_the_one_already_there() {
    let mut w = repo_world();
    let first = add(&mut w, "demo", "docs/brief.md").unwrap();
    assert_eq!(first.value.node.title, "brief.md");
    assert_eq!(
        first.value.node.locator.as_ref().unwrap().source,
        "docs/brief.md"
    );
    assert_eq!(umx_of(&w), FORMAT_LOCATORS);
    let bytes = raw(&w, GraphFile::Nodes);
    for again in ["./docs//brief.md", "root:repo/g/docs/brief.md"] {
        let done = add(&mut w, "demo", again).unwrap();
        assert_eq!(done.value.node.id, first.value.node.id, "{again}");
        assert_eq!(codes(&done.notes), ["resource_exists"], "{again}");
        assert_eq!(raw(&w, GraphFile::Nodes), bytes, "nothing written");
    }
    let folder = add(&mut w, "demo", "docs/").unwrap();
    assert_ne!(
        folder.value.node.id, first.value.node.id,
        "a folder is not a file of one name"
    );
    assert_eq!(folder.value.node.title, "docs");
}

/// Workshop 010 section 3: another loaded graph holding the same thing is named (resource_elsewhere), and the node
/// is still added. Two graphs in one repo meet on a repo path; a URL meets by its normal form.
#[test]
fn another_loaded_graphs_copy_is_named_with_resource_elsewhere() {
    let mut w = repo_world();
    w.svc
        .graph_new(&ctx(), "/work/h", "other", None, false, None)
        .unwrap();
    let brief = add(&mut w, "demo", "docs/brief.md").unwrap().value.node.id;
    let site = add(&mut w, "demo", "https://example.com/a")
        .unwrap()
        .value
        .node
        .id;
    let there = add(&mut w, "other", "root:repo/g/docs/brief.md").unwrap();
    assert_eq!(codes(&there.notes), ["resource_elsewhere"]);
    let elsewhere = |w: &[umx_core::Problem]| {
        w.iter()
            .find(|p| p.code == "resource_elsewhere")
            .and_then(|p| p.subject.clone())
    };
    assert_eq!(elsewhere(&there.notes), Some(brief.to_text()));
    assert_ne!(there.value.node.id, brief, "added all the same");
    let url = add(&mut w, "other", "HTTPS://Example.com:443/a#top").unwrap();
    assert_eq!(elsewhere(&url.notes), Some(site.to_text()));
    let own = add(&mut w, "other", "docs/brief.md").unwrap();
    assert!(
        elsewhere(&own.notes).is_none(),
        "other's own docs/brief.md is another file: {:?}",
        own.notes
    );
}

/// s3 done_when (decided with the graph-addressing-expert): node edit --locator keeps the node's id and edges,
/// refuses a locator another node holds, resets hash and length, warns locator_changed with the counts of edges
/// and evidence pointers that cited the old source; --unset locator makes it plain; the format never lowers.
#[test]
fn node_edit_re_points_and_clears_a_locator_keeping_id_and_edges() {
    let mut w = repo_world();
    let brief = add(&mut w, "demo", "docs/brief.md").unwrap().value.node.id;
    let other = add(&mut w, "demo", "docs/other.md").unwrap().value.node.id;
    let person = w
        .svc
        .node_add(
            &ctx().graph("demo"),
            NodeAddRequest {
                kind: "person".into(),
                title: "Ann".into(),
                evidence: vec!["root:repo/g/docs/brief.md#L1".into()],
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id;
    w.svc
        .edge_add(
            &ctx().graph("demo"),
            &person.to_text(),
            "shared",
            &brief.to_text(),
            &[],
        )
        .unwrap();
    // A hash umx saw on a read (S4), so the edit can be seen to reset it.
    let mut nodes = codec::decode_nodes(&raw(&w, GraphFile::Nodes))
        .unwrap()
        .records;
    let at = nodes.iter().position(|n| n.id == brief).unwrap();
    nodes[at].locator.as_mut().unwrap().hash = Some("sha256:9f2c".into());
    nodes[at].locator.as_mut().unwrap().length = Some(10);
    w.files
        .tamper_for_tests(&dir(), GraphFile::Nodes, &codec::encode_nodes(&nodes));
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();

    let taken = edit(
        &mut w,
        brief,
        NodeEditRequest {
            locator: Some("./docs/other.md".into()),
            ..Default::default()
        },
    );
    let taken = taken.unwrap_err();
    assert_eq!(
        (taken.code, taken.slot("id")),
        ("locator_in_use", Some(other.to_text()))
    );

    let moved = edit(
        &mut w,
        brief,
        NodeEditRequest {
            locator: Some("docs/brief-v2.md".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(moved.value.changed, ["locator"]);
    assert_eq!(moved.value.node.id, brief);
    let l = moved.value.node.locator.as_ref().unwrap();
    assert_eq!(
        (l.source.as_str(), l.hash.as_deref(), l.length),
        ("docs/brief-v2.md", None, None)
    );
    let changed = &moved.notes[0];
    assert_eq!(changed.code, "locator_changed");
    assert_eq!(
        (changed.slot("edges"), changed.slot("evidence")),
        (Some("1".into()), Some("1".into()))
    );

    let cleared = edit(
        &mut w,
        brief,
        NodeEditRequest {
            unsets: vec!["locator".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(cleared.value.changed, ["locator"]);
    assert_eq!(cleared.value.node.locator, None);
    assert_eq!(codes(&cleared.notes), ["locator_changed"]);
    let edges = w
        .svc
        .edge_list(
            &ctx().graph("demo"),
            umx_services::EdgeListRequest {
                to: Some(brief.to_text()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(edges.value.rows.len(), 1, "the edge stays");
    // Never lowered: other.md is still a resource, and with none at all the format stays.
    edit(
        &mut w,
        other,
        NodeEditRequest {
            unsets: vec!["locator".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(umx_of(&w), FORMAT_LOCATORS);
    let again = edit(
        &mut w,
        brief,
        NodeEditRequest {
            unsets: vec!["locator".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(again.value.changed.is_empty(), "a plain node stays plain");
}

/// graph-addressing-expert N1: a node holding a kept custom field named locator cannot take a locator until the
/// field is renamed; --unset locator removes that field.
#[test]
fn a_kept_locator_field_blocks_a_locator_and_unset_clears_it() {
    let mut old = resource(9_000, "Old", "docs/old.md");
    old.locator = None;
    old.fields
        .insert("locator".into(), serde_json::json!("an old custom value"));
    let (mut w, _) = with_resources(&[old]);
    let refused = edit(
        &mut w,
        Id(9_000),
        NodeEditRequest {
            locator: Some("docs/a.md".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(refused.code, "locator_field_invalid");
    let done = edit(
        &mut w,
        Id(9_000),
        NodeEditRequest {
            unsets: vec!["locator".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(done.value.changed, ["locator"]);
    assert!(!done.value.node.has_kept_locator_field());
    assert!(
        !String::from_utf8(raw(&w, GraphFile::Nodes))
            .unwrap()
            .contains("old custom value")
    );
}

/// Locator text is checked as an evidence source is (security S1 and a later review): absolute paths name the root:
/// form, .. and URL credentials are refused, an unknown root says how to add it, and a source that looks like a secret
/// is held (secret_suspected).
#[test]
fn locator_text_is_checked_as_evidence_sources_are() {
    let mut w = repo_world();
    let refused = |w: &mut World, text: &str| add(w, "demo", text).unwrap_err();
    let abs = refused(&mut w, "/work/g/docs/brief.md");
    assert_eq!(abs.code, "locator_absolute");
    assert!(
        abs.slot("fix")
            .unwrap()
            .contains("--locator root:repo/<path>"),
        "{abs:?}"
    );
    let outside = refused(&mut w, "/etc/hosts");
    assert!(
        outside
            .slot("fix")
            .unwrap()
            .contains("--locator root:<name>/<path>"),
        "{outside:?}"
    );
    for text in [
        "../x.md",
        "https://user:pass@example.com/a",
        "docs/a\u{202E}dm.exe",
    ] {
        assert_eq!(refused(&mut w, text).code, "locator_invalid", "{text}");
    }
    assert_eq!(refused(&mut w, "root:acme/a.md").code, "root_unknown");
    let token = format!("docs/ghp_{}.md", "a".repeat(36));
    let held = add(&mut w, "demo", &token).unwrap();
    // The title (the file name) and the locator's source are both held.
    let fields: Vec<_> = held
        .notes
        .iter()
        .filter(|p| p.code.ends_with("secret_suspected"))
        .filter_map(|p| p.slot("field"))
        .collect();
    assert!(fields.contains(&"locator".to_string()), "{:?}", held.notes);
}

/// umx resource show: a resource node with its locator; a plain node is not_a_resource.
#[test]
fn resource_show_prints_a_resource_and_refuses_a_plain_node() {
    let mut w = repo_world();
    let brief = add(&mut w, "demo", "docs/brief.md").unwrap().value.node.id;
    add_plain(&mut w, "Plain");
    let shown = w
        .svc
        .resource_show(&ctx().graph("demo"), &brief.to_text())
        .unwrap();
    assert_eq!(shown.value.node.locator.unwrap().source, "docs/brief.md");
    let plain = codec::decode_nodes(&raw(&w, GraphFile::Nodes))
        .unwrap()
        .records
        .into_iter()
        .find(|n| n.title == "Plain")
        .unwrap();
    let refused = w
        .svc
        .resource_show(&ctx().graph("demo"), &plain.id.to_text())
        .unwrap_err();
    assert_eq!(refused.code, "not_a_resource");
}

/// A graph made with `policy`, in /work (the repo).
fn graph_with(w: &mut World, alias: &str, policy: umx_core::Policy) {
    let embedding = umx_core::GraphEmbedding {
        key: None,
        text_form: 1,
        policy,
        similarity: None,
        extra: Default::default(),
    };
    w.svc
        .graph_new(
            &ctx(),
            &format!("/work/{alias}"),
            alias,
            None,
            false,
            Some(embedding),
        )
        .unwrap();
}

/// Loads a copy of `from` (the same graph id, another folder) as `alias` (d-21).
fn copy_graph(w: &mut World, from: &str, alias: &str) {
    let (src, dst) = (
        GraphDir::parse(&format!("/work/{from}")).unwrap(),
        GraphDir::parse(&format!("/work/{alias}")).unwrap(),
    );
    for file in [
        GraphFile::Graph,
        GraphFile::Nodes,
        GraphFile::Edges,
        GraphFile::GitIgnore,
    ] {
        if let Some(bytes) = w.files.raw_for_tests(&src, file) {
            w.files.tamper_for_tests(&dst, file, &bytes);
        }
    }
    w.svc.graph_load(&ctx(), dst.as_str(), Some(alias)).unwrap();
}

/// d-20 (security and graph-addressing-expert, F1): a secured graph holding the same file is never named, counted
/// or hinted at. The reply is byte-identical to the one with that graph not loaded, on add and on edit. Control: the
/// same graph made open is named.
#[test]
fn resource_elsewhere_never_names_a_secured_graph() {
    let reply = |policy: Option<umx_core::Policy>| {
        let mut w = repo_world();
        graph_with(&mut w, "vault", policy.unwrap_or(umx_core::Policy::SECURED));
        add(&mut w, "vault", "https://example.com/a").unwrap();
        add(&mut w, "vault", "https://example.com/b").unwrap();
        if policy.is_none() {
            w.svc.graph_unload(&ctx(), "vault").unwrap();
        }
        let added = add(&mut w, "demo", "https://example.com/a").unwrap();
        let edited = edit(
            &mut w,
            added.value.node.id,
            NodeEditRequest {
                locator: Some("https://example.com/b".into()),
                ..Default::default()
            },
        )
        .unwrap();
        format!("{added:?}\n{edited:?}")
    };
    let secured = reply(Some(umx_core::Policy::SECURED));
    assert_eq!(secured, reply(None), "a secured graph changes nothing");
    assert!(!secured.contains("vault") && !secured.contains("resource_elsewhere"));
    let open = reply(Some(umx_core::Policy::OPEN));
    assert_eq!(open.matches("resource_elsewhere").count(), 2, "{open}");
}

/// d-21 (F2): another graph is named once, through its primary copy, however many copies are loaded; a copy of
/// the graph being written is not another graph.
#[test]
fn resource_elsewhere_names_each_other_graph_once_and_never_a_copy_of_its_own() {
    let mut w = repo_world();
    graph_with(&mut w, "other", umx_core::Policy::OPEN);
    let there = add(&mut w, "other", "https://example.com/a")
        .unwrap()
        .value
        .node
        .id;
    copy_graph(&mut w, "other", "other_copy");
    add(&mut w, "demo", "https://example.com/b").unwrap();
    copy_graph(&mut w, "g", "demo_copy");
    let added = add(&mut w, "demo", "https://example.com/a").unwrap();
    let named: Vec<_> = added
        .notes
        .iter()
        .filter(|p| p.code == "resource_elsewhere")
        .map(|p| (p.slot("graph"), p.subject.clone()))
        .collect();
    assert_eq!(named, [(Some("other".into()), Some(there.to_text()))]);
    let own = add(&mut w, "demo_copy", "https://example.com/b").unwrap();
    assert_eq!(codes(&own.notes), ["resource_exists"], "demo_copy is demo");
}

/// The edges count `locator_changed` gives for re-pointing `node` to `to`.
fn edges_counted(w: &mut World, node: Id, to: &str) -> Option<String> {
    let done = edit(
        w,
        node,
        NodeEditRequest {
            locator: Some(to.into()),
            ..Default::default()
        },
    )
    .unwrap();
    let changed = done.notes.iter().find(|p| p.code == "locator_changed");
    changed.and_then(|p| p.slot("edges"))
}

/// review R255-2: a self-loop is one edge (it is in both the out and in lists).
#[test]
fn locator_changed_counts_a_self_loop_once() {
    let mut w = repo_world();
    let looped = add(&mut w, "demo", "docs/loop.md").unwrap().value.node.id;
    w.svc
        .edge_add(
            &ctx().graph("demo"),
            &looped.to_text(),
            "relates_to",
            &looped.to_text(),
            &[],
        )
        .unwrap();
    assert_eq!(
        edges_counted(&mut w, looped, "docs/loop2.md"),
        Some("1".into())
    );
}

/// review R255-2: a link this graph stores to a node in another graph is an edge too (external adjacency).
#[test]
fn locator_changed_counts_a_link_to_another_graph_once() {
    let mut w = repo_world();
    graph_with(&mut w, "other", umx_core::Policy::OPEN);
    let far = add(&mut w, "other", "https://example.com/far")
        .unwrap()
        .value
        .node
        .id;
    let linked = add(&mut w, "demo", "docs/link.md").unwrap().value.node.id;
    w.svc
        .edge_add(
            &ctx().graph("demo"),
            &linked.to_text(),
            "relates_to",
            &format!("umx:other/{}", far.to_text()),
            &[],
        )
        .unwrap();
    assert_eq!(
        edges_counted(&mut w, linked, "docs/link2.md"),
        Some("1".into())
    );
}

/// review R255-2: the old locator and the evidence are compared by what they name, both normalised. A loaded,
/// hand-edited `./docs//brief.md` is cited by `docs/brief.md` and by `root:repo/g/docs/brief.md`; both count, and no
/// stored text is rewritten.
#[test]
fn locator_changed_counts_citations_of_an_unnormalised_locator() {
    let (mut w, warnings) = with_resources(&[resource(9_000, "Brief", "./docs//brief.md")]);
    assert_eq!(codes(&warnings), ["locator_not_normalised"]);
    let w = &mut w;
    for pointer in [
        "docs/brief.md#L1",
        "root:repo/g/docs/brief.md#L2",
        "docs/other.md#L3",
    ] {
        w.svc
            .node_add(
                &ctx().graph("demo"),
                NodeAddRequest {
                    kind: "person".into(),
                    title: format!("Cites {pointer}"),
                    evidence: vec![pointer.into()],
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let done = edit(
        w,
        Id(9_000),
        NodeEditRequest {
            locator: Some("docs/brief-v2.md".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let changed = done
        .notes
        .iter()
        .find(|p| p.code == "locator_changed")
        .unwrap();
    assert_eq!(changed.slot("evidence"), Some("2".into()));
    let text = String::from_utf8(raw(w, GraphFile::Nodes)).unwrap();
    assert!(
        text.contains("\"source\":\"docs/brief.md\"") && text.contains("root:repo/g/docs/brief.md")
    );
}
