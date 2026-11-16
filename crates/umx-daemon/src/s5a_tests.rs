//! ws009 S5a: the change feed on a real daemon. Every write Op gives exactly one change for the graph whose files it
//! wrote and none for any other; a failed write gives none and leaves the revision; a load, reload or unload is an
//! overflow; a write wakes a waiting view; a 10k batch is one overflow and never pushes out an agent's command.

use super::*;
use std::collections::BTreeMap;
use umx_protocol::{BatchLine, BatchOp, EvidenceAction, KeyAction, Op, SettingArg, UiCmd};

fn build() -> Build {
    Build {
        version: "t".into(),
        commit: "t".into(),
        built: "t".into(),
    }
}

fn op(op: Op, graph: Option<&str>) -> Request {
    Request::op(op, graph.map(str::to_owned), None, build())
}

fn wait(epoch: &str, graphs: &[&str], revs: &BTreeMap<String, u64>, timeout_ms: u64) -> Request {
    op(
        Op::UiWait {
            epoch: Some(epoch.to_owned()),
            since: 0,
            timeout_ms,
            graphs: graphs.iter().map(|g| (*g).to_owned()).collect(),
            revs: revs.clone(),
            supervisor: false,
        },
        None,
    )
}

fn ok(env: &Envelope) -> &Value {
    assert!(env.errors.is_empty(), "{}", env.to_json());
    &env.data
}

fn new_graph(d: &Daemon, root: &std::path::Path, alias: &str) {
    ok(&d.handle(&op(
        Op::GraphNew {
            path: root.join(alias).display().to_string(),
            alias: alias.into(),
            title: None,
            empty: false,
            embedding_model: None,
            secured: false,
            no_graphfile: false,
            graphfile: false,
        },
        None,
    )));
}

/// A daemon over real adapters in a fresh folder with graphs `g` and `h`, its epoch, and the revisions a view
/// following both starts from (each from 0: told to load, at the revision of its load).
fn daemon(
    name: &str,
) -> (
    Arc<Daemon>,
    std::path::PathBuf,
    String,
    BTreeMap<String, u64>,
) {
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("umx-s5a-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let config = Config::at(root.join("home"), Some(true)).roots([root.to_str().unwrap()]);
    let d = Daemon::new(&config, build());
    new_graph(&d, &root, "g");
    new_graph(&d, &root, "h");
    let first = d.handle(&op(
        Op::UiWait {
            epoch: None,
            since: 0,
            timeout_ms: 0,
            graphs: vec!["g".into()],
            revs: BTreeMap::new(),
            supervisor: false,
        },
        None,
    ));
    assert_eq!(
        ok(&first)["changes"],
        json!([]),
        "a view that follows no revisions gets no changes"
    );
    let epoch = first.data["epoch"].as_str().unwrap().to_owned();
    let mut revs: BTreeMap<String, u64> = [("g".into(), 0), ("h".into(), 0)].into();
    let started = changes(&d, &epoch, &mut revs);
    assert_eq!(
        started,
        [
            json!({ "graph": "g", "rev": 1, "overflow": true }),
            json!({ "graph": "h", "rev": 1, "overflow": true })
        ],
        "a load is the first step"
    );
    // Revisions given under another epoch were another daemon's: each named graph is reloaded.
    let other = d.handle(&wait("0000000000000000", &["g"], &revs, 0));
    assert_eq!(
        ok(&other)["changes"],
        json!([{ "graph": "g", "rev": 1, "overflow": true }])
    );
    (Arc::new(d), root, epoch, revs)
}

/// The changes a view following `revs` (graphs g and h) gets now; moves `revs` on as the view would.
fn changes(d: &Daemon, epoch: &str, revs: &mut BTreeMap<String, u64>) -> Vec<Value> {
    let graphs: Vec<&str> = revs.keys().map(String::as_str).collect();
    let got = d.handle(&wait(epoch, &graphs, revs, 0));
    let changes = ok(&got)["changes"].as_array().unwrap().clone();
    for c in &changes {
        let rev = c.get("to_rev").or_else(|| c.get("rev")).unwrap();
        revs.insert(c["graph"].as_str().unwrap().into(), rev.as_u64().unwrap());
    }
    changes
}

fn rev(d: &Daemon, alias: &str) -> u64 {
    d.services
        .read()
        .unwrap()
        .graph()
        .loaded(alias)
        .unwrap()
        .feed
        .rev()
}

fn node_add(title: &str) -> Op {
    Op::NodeAdd {
        on_key: None,
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
    }
}

fn batch(lines: &[String], dry_run: bool) -> Op {
    Op::Batch {
        raised_in: None,
        lines: lines
            .iter()
            .enumerate()
            .map(|(i, l)| BatchLine {
                line: i as u32 + 1,
                op: BatchOp::from_line(l).unwrap(),
            })
            .collect(),
        dry_run,
        reason: None,
        echo_sentences: false,
        refs: Default::default(),
        refs_graph: None,
    }
}

fn node_evidence(node: &str, action: EvidenceAction, pointer: Option<String>) -> Op {
    Op::NodeEvidence {
        node: node.into(),
        action,
        pointer,
        evidence_kind: None,
        reason: (action == EvidenceAction::Remove).then(|| "stale".into()),
        at: None,
        removed: false,
        redact: false,
    }
}

/// What an Op writes, decided for every variant (no `_` arm, so a new Op must be decided here): the content files
/// of the request's graph (one change), the loaded set (an overflow for the graph it loads or unloads), graph.json
/// only when the embedder's key changed (at most one change, with no ids), nothing a view reads (no change), or
/// nothing at all (not a write).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Writes {
    Content,
    LoadedSet,
    KeyOnly,
    Elsewhere,
    Read,
}

fn writes(op: &Op) -> Writes {
    use Writes::*;
    match op {
        Op::NodeAdd { .. }
        | Op::EdgeAdd { .. }
        | Op::OntologyAddType { .. }
        | Op::OntologyAddStatus { .. }
        | Op::OntologyAddRelation { .. }
        | Op::OntologyAddField { .. }
        | Op::OntologyAddRelationField { .. }
        | Op::OntologySetField { .. }
        | Op::OntologySetType { .. }
        | Op::OntologyRemoveType { .. }
        | Op::OntologyRemoveStatus { .. }
        | Op::OntologyRemoveRelation { .. }
        | Op::OntologyRemoveField { .. }
        | Op::OntologyRemoveRelationField { .. }
        | Op::EntityKey { .. }
        | Op::NodeEdit { .. }
        | Op::NodeNote { .. }
        | Op::NodeEvidence {
            action: EvidenceAction::Add | EvidenceAction::Remove,
            ..
        }
        | Op::NodeStatus { .. }
        | Op::GraphSet { .. }
        | Op::Batch { dry_run: false, .. }
        | Op::EdgeDelete { .. }
        | Op::EdgeEdit { .. }
        | Op::EntityLink { dry_run: false, .. }
        | Op::EntityUnlink { .. }
        | Op::EntityDeny { .. }
        | Op::EntityUndeny { .. }
        | Op::GraphSpineSet { .. }
        | Op::GraphSpineConfirm { .. }
        | Op::GraphSpineRemove { .. } => Content,
        Op::GraphNew { .. }
        | Op::GraphLoad { .. }
        | Op::GraphUnload { .. }
        | Op::GraphRename { .. }
        | Op::GraphMove { .. } => LoadedSet,
        Op::EmbeddingsReembed { dry_run: false, .. } => KeyOnly,
        // Config, registry, scopes, roots, this machine's records, vectors and similar_to lists: no graph content.
        Op::ConfigSet { .. }
        | Op::ConfigUnset { .. }
        | Op::EmbeddingsRepair
        | Op::GraphConfirmPolicy { .. }
        | Op::SimilarityRebuild
        | Op::Batch { dry_run: true, .. }
        | Op::EntityLink { dry_run: true, .. }
        | Op::EmbeddingsAllow { .. }
        | Op::GraphForget { .. }
        | Op::GraphPrimary { .. }
        // The repo Graphfile only (ws002 s17): never a graph's content.
        | Op::GraphScan { add: true, .. }
        | Op::GraphScan { rewrite: true, .. }
        | Op::GraphScan { drop: Some(_), .. }
        | Op::ScopeSave { .. }
        | Op::ScopeDelete { .. }
        | Op::ConfigImport { dry_run: false, .. }
        | Op::RootAdd { .. }
        | Op::RootRemove { .. } => Elsewhere,
        Op::GraphList { .. }
        | Op::NodeGet { .. }
        | Op::NodeList { .. }
        | Op::NodeFind { .. }
        | Op::EdgeList { .. }
        | Op::OntologyShow
        | Op::NodeHistory { .. }
        | Op::Check { .. }
        | Op::ConfigShow { .. }
        | Op::ConfigCheck { .. }
        | Op::EmbeddingsReembed { dry_run: true, .. }
        | Op::Neighbours { .. }
        | Op::Walk { .. }
        | Op::Path { .. }
        | Op::Search { .. }
        | Op::EmbeddingsProbe { .. }
        | Op::Query { .. }
        | Op::ScopeList
        | Op::ScopeShow { .. }
        | Op::ConfigImport { dry_run: true, .. }
        | Op::EmbeddingsStatus
        | Op::RootList
        | Op::Resolve { .. }
        | Op::GraphGuide { .. }
        | Op::GraphSpinePropose { .. }
        | Op::GraphScan { .. }
        | Op::UiCommand { .. }
        | Op::UiWait { .. }
        | Op::UiSelect { .. }
        | Op::UiSelection
        | Op::SimilarityPairs { .. }
        | Op::EntityShow { .. }
        | Op::Who { .. }
        | Op::NodeWho { .. }
        | Op::EdgeShow { .. }
        | Op::ResourceShow { .. }
        | Op::GraphSpineShow { .. }
        | Op::NodeEvidence {
            action: EvidenceAction::List,
            ..
        } => Read,
    }
}

fn ids(v: &[&str]) -> Value {
    json!(v)
}

#[test]
fn every_write_op_gives_one_change_for_the_graph_it_wrote_and_none_for_another() {
    let (d, root, epoch, mut revs) = daemon("table");
    let first = ok(&d.handle(&op(node_add("Approval delays"), Some("g"))))["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let other = ok(&d.handle(&op(node_add("Budget cuts"), Some("g"))))["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let c = changes(&d, &epoch, &mut revs);
    assert_eq!(c.len(), 1, "two writes, one entry: {c:?}");
    let mut two = vec![first.as_str(), other.as_str()];
    two.sort();
    assert_eq!(c[0]["nodes"]["added"], ids(&two));
    assert_eq!((&c[0]["from_rev"], &c[0]["to_rev"]), (&json!(1), &json!(3)));
    let h_node = ok(&d.handle(&op(node_add("Elsewhere"), Some("h"))))["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    changes(&d, &epoch, &mut revs);
    let set = |name: &str, value: &str| SettingArg {
        name: name.into(),
        value: value.into(),
    };
    // (request, the change g's view expects for a Content write)
    let mut table: Vec<(Request, Option<Value>)> = vec![
        (op(node_add("Slow sign-off"), Some("g")), None),
        (
            op(
                Op::NodeEdit {
                    node: first.clone(),
                    title: Some("Approval delays (Q3)".into()),
                    kind: None,
                    note: None,
                    sets: Vec::new(),
                    unsets: Vec::new(),
                    locator: None,
                },
                Some("g"),
            ),
            Some(json!({ "nodes": { "edited": [first] } })),
        ),
        (
            op(
                Op::NodeNote {
                    node: first.clone(),
                    append: "seen again".into(),
                    evidence: Vec::new(),
                    evidence_kind: None,
                },
                Some("g"),
            ),
            Some(json!({ "nodes": { "edited": [first] } })),
        ),
        (
            op(
                node_evidence(&first, EvidenceAction::Add, Some("docs/a.md#L1".into())),
                Some("g"),
            ),
            Some(json!({ "nodes": { "edited": [first] } })),
        ),
        (
            op(node_evidence(&first, EvidenceAction::List, None), Some("g")),
            None,
        ),
        (
            op(
                node_evidence(&first, EvidenceAction::Remove, Some("docs/a.md#L1".into())),
                Some("g"),
            ),
            Some(json!({ "nodes": { "edited": [first] } })),
        ),
        (
            op(
                Op::NodeStatus {
                    evidence: None,
                    evidence_kind: None,
                    place: None,
                    node: first.clone(),
                    status: "done".into(),
                    reason: Some("fixed".into()),
                    label: None,
                },
                Some("g"),
            ),
            Some(json!({ "nodes": { "edited": [first] } })),
        ),
        (
            op(
                Op::EdgeAdd {
                    from: first.clone(),
                    relation: "blocks".into(),
                    to: other.clone(),
                    sets: Vec::new(),
                    evidence: Vec::new(),
                    evidence_kind: None,
                },
                Some("g"),
            ),
            None,
        ),
        (
            op(
                Op::OntologyAddType {
                    name: "risk_item".into(),
                    fields: Vec::new(),
                    description: None,
                },
                Some("g"),
            ),
            Some(json!({})),
        ),
        (
            op(
                Op::OntologyAddStatus {
                    name: "parked".into(),
                    description: None,
                },
                Some("g"),
            ),
            Some(json!({})),
        ),
        (
            op(
                Op::OntologyAddRelation {
                    name: "relates".into(),
                    symmetric: false,
                    description: None,
                },
                Some("g"),
            ),
            Some(json!({})),
        ),
        (
            op(
                Op::OntologyAddField {
                    types: vec!["risk_item".into()],
                    field: "owner:string".into(),
                    all_types: false,
                    description: None,
                    role: None,
                },
                Some("g"),
            ),
            Some(json!({})),
        ),
        (
            op(
                Op::OntologyAddRelationField {
                    relation: "relates".into(),
                    field: "weight:int".into(),
                },
                Some("g"),
            ),
            Some(json!({})),
        ),
        (
            op(
                Op::OntologySetField {
                    type_name: "risk_item".into(),
                    field: "owner".into(),
                    role: "data".into(),
                },
                Some("g"),
            ),
            Some(json!({})),
        ),
        (
            op(
                Op::OntologySetType {
                    name: "risk_item".into(),
                    columns: Some(vec!["owner".into()]),
                },
                Some("g"),
            ),
            Some(json!({})),
        ),
        (
            op(
                Op::OntologyRemoveRelationField {
                    relation: "relates".into(),
                    field: "weight".into(),
                },
                Some("g"),
            ),
            Some(json!({})),
        ),
        (
            op(
                Op::OntologySetType {
                    name: "risk_item".into(),
                    columns: None,
                },
                Some("g"),
            ),
            Some(json!({})),
        ),
        (
            op(
                Op::OntologyRemoveField {
                    type_name: "risk_item".into(),
                    field: "owner".into(),
                },
                Some("g"),
            ),
            Some(json!({})),
        ),
        (
            op(
                Op::OntologyRemoveType {
                    name: "risk_item".into(),
                },
                Some("g"),
            ),
            Some(json!({})),
        ),
        (
            op(
                Op::OntologyRemoveStatus {
                    name: "parked".into(),
                },
                Some("g"),
            ),
            Some(json!({})),
        ),
        (
            op(
                Op::OntologyRemoveRelation {
                    name: "relates".into(),
                },
                Some("g"),
            ),
            Some(json!({})),
        ),
        (
            op(
                Op::EntityKey {
                    node: first.clone(),
                    action: KeyAction::Add,
                    key: "email:sam@example.com".into(),
                },
                Some("g"),
            ),
            Some(json!({ "nodes": { "edited": [first] } })),
        ),
        (
            op(
                Op::GraphSet {
                    alias: "g".into(),
                    settings: vec![set("title", "Launch planning")],
                },
                Some("g"),
            ),
            Some(json!({})),
        ),
        (
            op(
                batch(
                    &[
                        r#"{"op":"node add","ref":"q","type":"problem","title":"Batch one"}"#
                            .into(),
                        format!(
                            r#"{{"op":"node edit","node":"{other}","title":"Budget cuts (Q3)"}}"#
                        ),
                    ],
                    false,
                ),
                Some("g"),
            ),
            None,
        ),
        (
            op(
                batch(
                    &[r#"{"op":"node add","type":"problem","title":"Dry"}"#.into()],
                    true,
                ),
                Some("g"),
            ),
            None,
        ),
        (
            op(
                Op::GraphLoad {
                    path: root.join("g").display().to_string(),
                    as_alias: Some("g".into()),
                    by_alias: None,
                    graphfile: false,
                    set: None,
                },
                None,
            ),
            None,
        ),
        (
            op(
                Op::ConfigSet {
                    key: "daemon.idle_unload".into(),
                    value: "0".into(),
                    repo: false,
                    cwd: None,
                },
                None,
            ),
            None,
        ),
        (
            op(
                Op::ConfigUnset {
                    key: "daemon.idle_unload".into(),
                    repo: false,
                    cwd: None,
                },
                None,
            ),
            None,
        ),
        (op(Op::EmbeddingsRepair, Some("g")), None),
        (
            op(
                Op::EmbeddingsReembed {
                    dry_run: false,
                    model: None,
                },
                Some("g"),
            ),
            None,
        ),
        (
            op(
                Op::GraphConfirmPolicy {
                    alias: "g".into(),
                    typed: Some("g".into()),
                },
                None,
            ),
            None,
        ),
        (op(Op::SimilarityRebuild, Some("g")), None),
        (
            op(
                Op::EmbeddingsAllow {
                    node: first.clone(),
                    field: "title".into(),
                },
                Some("g"),
            ),
            None,
        ),
        (
            op(
                Op::ScopeSave {
                    name: "both".into(),
                    replace: false,
                },
                Some("g,h"),
            ),
            None,
        ),
        (
            op(
                Op::ScopeDelete {
                    name: "both".into(),
                },
                None,
            ),
            None,
        ),
        (
            op(
                Op::ConfigImport {
                    from: root.join("absent.toml").display().to_string(),
                    name: None,
                    model: None,
                    dims: None,
                    force: false,
                    dry_run: false,
                },
                None,
            ),
            None,
        ),
        (
            op(
                Op::RootAdd {
                    name: "docs".into(),
                    folder: root.display().to_string(),
                },
                None,
            ),
            None,
        ),
        (
            op(
                Op::RootRemove {
                    name: "docs".into(),
                },
                None,
            ),
            None,
        ),
        (op(Op::GraphForget { alias: "x".into() }, None), None),
        // F98: g is the only copy, so this is unchanged and writes nothing a view reads.
        (op(Op::GraphPrimary { alias: "g".into() }, None), None),
        // ws002 s17: the table's folder is in no git repository, so this refuses; it never touches a graph.
        (
            op(
                Op::GraphScan {
                    path: std::env::temp_dir().display().to_string(),
                    add: true,
                    rewrite: false,
                    drop: None,
                    depth: None,
                    whole_repo: false,
                },
                None,
            ),
            None,
        ),
    ];
    // Edge edit and delete need the edge the table adds: run them after it.
    let run = |request: &Request, expect: Option<Value>, revs: &mut BTreeMap<String, u64>| {
        let o = request.op.as_ref().unwrap();
        let class = writes(o);
        assert_eq!(o.writes(), class != Writes::Read, "{}", o.command());
        let before = (rev(&d, "g"), rev(&d, "h"));
        let env = d.handle(request);
        let c = changes(&d, &epoch, revs);
        eprintln!(
            "table: {:<22} {:?} -> {} {} | changes {}",
            o.command(),
            class,
            if env.errors.is_empty() { "ok" } else { "error" },
            env.errors.first().map_or("", |e| e.code()),
            json!(c)
        );
        let mine: Vec<&Value> = c.iter().filter(|c| c["graph"] == "g").collect();
        assert!(
            c.iter().all(|c| c["graph"] == "g"),
            "{}: a change for a graph it did not write: {c:?}",
            o.command()
        );
        match class {
            Writes::Content if env.errors.is_empty() => {
                assert_eq!(mine.len(), 1, "{}: {c:?}", o.command());
                assert_eq!(rev(&d, "g"), before.0 + 1, "{}", o.command());
                if let Some(expect) = expect {
                    for (kind, lists) in expect.as_object().unwrap() {
                        for (list, want) in lists.as_object().unwrap() {
                            assert_eq!(&mine[0][kind][list], want, "{} {kind}.{list}", o.command());
                        }
                    }
                    if expect == json!({}) {
                        assert_eq!(
                            (&mine[0]["nodes"], &mine[0]["edges"]),
                            (
                                &json!({"added": [], "edited": [], "removed": []}),
                                &json!({"added": [], "edited": [], "removed": []})
                            ),
                            "{}",
                            o.command()
                        );
                    }
                }
                assert!(mine[0].get("overflow").is_none(), "{}", o.command());
            }
            Writes::LoadedSet => {
                assert!(env.errors.is_empty(), "{}", env.to_json());
                assert_eq!(mine.len(), 1, "{}: {c:?}", o.command());
                assert_eq!(mine[0]["overflow"], true, "{}", o.command());
            }
            Writes::KeyOnly if !mine.is_empty() => {
                let none = json!({"added": [], "edited": [], "removed": []});
                assert_eq!(mine.len(), 1, "{}: {c:?}", o.command());
                assert_eq!((&mine[0]["nodes"], &mine[0]["edges"]), (&none, &none));
            }
            _ => {
                assert!(mine.is_empty(), "{}: {}: {c:?}", o.command(), env.to_json());
                assert_eq!((rev(&d, "g"), rev(&d, "h")), before, "{}", o.command());
            }
        }
        assert_eq!(rev(&d, "h"), before.1, "{}: h never moves", o.command());
        (o.command(), env, c)
    };
    // Op variants run, by discriminant (the five ontology adds share one label).
    let mut seen = std::collections::HashSet::new();
    for (request, expect) in table.drain(..) {
        let (command, env, c) = run(&request, expect, &mut revs);
        if matches!(writes(request.op.as_ref().unwrap()), Writes::Content) {
            assert!(env.errors.is_empty(), "{}", env.to_json());
        }
        if command == "node add" {
            let id = env.data["node"]["id"].as_str().unwrap();
            assert_eq!(c[0]["nodes"]["added"], ids(&[id]));
        }
        if command == "batch" && !request_dry(&request) {
            let added = env.data["rows"][0]["id"].as_str().unwrap();
            assert_eq!(c[0]["nodes"]["added"], ids(&[added]));
            assert_eq!(c[0]["nodes"]["edited"], ids(&[other.as_str()]));
        }
        seen.insert(std::mem::discriminant(request.op.as_ref().unwrap()));
    }
    let edge = ok(&d.handle(&op(
        Op::EdgeList {
            from: Some(first.clone()),
            to: None,
            kind: Some("blocks".into()),
            columns: None,
            limit: None,
            offset: None,
            along: None,
        },
        Some("g"),
    )))["rows"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for (request, expect) in [
        (
            op(
                Op::EdgeEdit {
                    edge: edge.clone(),
                    relation: None,
                    reverse: true,
                    sets: Vec::new(),
                    unsets: Vec::new(),
                },
                Some("g"),
            ),
            json!({ "edges": { "edited": [edge] } }),
        ),
        (
            op(Op::EdgeDelete { edge: edge.clone() }, Some("g")),
            json!({ "edges": { "removed": [edge] } }),
        ),
    ] {
        run(&request, Some(expect), &mut revs);
        seen.insert(std::mem::discriminant(request.op.as_ref().unwrap()));
    }
    // ws008 E2: a same_as link is one edge added to the graph that stores it, and unlink removes it.
    let link = op(
        Op::EntityLink {
            a: first.clone(),
            b: other.clone(),
            dry_run: false,
        },
        Some("g"),
    );
    let (_, linked, c) = run(&link, None, &mut revs);
    let id = ok(&linked)["edge"]["id"].as_str().unwrap().to_owned();
    assert_eq!(c[0]["edges"]["added"], ids(&[id.as_str()]));
    seen.insert(std::mem::discriminant(link.op.as_ref().unwrap()));
    let unlink = op(
        Op::EntityUnlink {
            a: first.clone(),
            b: other.clone(),
        },
        Some("g"),
    );
    run(
        &unlink,
        Some(json!({ "edges": { "removed": [id] } })),
        &mut revs,
    );
    seen.insert(std::mem::discriminant(unlink.op.as_ref().unwrap()));
    // ws008 E2c: a denial is one edge added, and undeny removes it.
    let deny = op(
        Op::EntityDeny {
            a: first.clone(),
            b: other.clone(),
            reason: Some("two different approvals".into()),
        },
        Some("g"),
    );
    let (_, denied, c) = run(&deny, None, &mut revs);
    let id = ok(&denied)["edge"]["id"].as_str().unwrap().to_owned();
    assert_eq!(c[0]["edges"]["added"], ids(&[id.as_str()]));
    seen.insert(std::mem::discriminant(deny.op.as_ref().unwrap()));
    let undeny = op(
        Op::EntityUndeny {
            a: first.clone(),
            b: other.clone(),
        },
        Some("g"),
    );
    run(
        &undeny,
        Some(json!({ "edges": { "removed": [id] } })),
        &mut revs,
    );
    seen.insert(std::mem::discriminant(undeny.op.as_ref().unwrap()));
    // Marking a relation symmetric when the graph holds it both ways drops one edge: the change says which (plan 129).
    let rivals: Vec<String> = [(&first, &other), (&other, &first)]
        .into_iter()
        .map(|(from, to)| {
            let added = d.handle(&op(
                Op::EdgeAdd {
                    from: from.clone(),
                    relation: "rivals".into(),
                    to: to.clone(),
                    sets: Vec::new(),
                    evidence: Vec::new(),
                    evidence_kind: None,
                },
                Some("g"),
            ));
            ok(&added)["edge"]["id"].as_str().unwrap().to_owned()
        })
        .collect();
    changes(&d, &epoch, &mut revs);
    let (_, _, c) = run(
        &op(
            Op::OntologyAddRelation {
                name: "rivals".into(),
                symmetric: true,
                description: None,
            },
            Some("g"),
        ),
        None,
        &mut revs,
    );
    let kept = ok(&d.handle(&op(
        Op::EdgeList {
            from: None,
            to: None,
            kind: Some("rivals".into()),
            columns: None,
            limit: None,
            offset: None,
            along: None,
        },
        Some("g"),
    )))["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(kept.len(), 1, "{kept:?}");
    let dropped: Vec<&str> = rivals
        .iter()
        .map(String::as_str)
        .filter(|id| *id != kept[0])
        .collect();
    assert_eq!(c[0]["edges"]["removed"], ids(&dropped), "{c:?}");
    assert_eq!(
        (&c[0]["edges"]["added"], &c[0]["edges"]["edited"]),
        (&json!([]), &json!([]))
    );
    // Unload and new last. The view still names g after its unload and gets its overflow once (R252-1).
    let unload = op(Op::GraphUnload { alias: "g".into() }, None);
    assert_eq!(writes(unload.op.as_ref().unwrap()), Writes::LoadedSet);
    let g_rev = revs["g"];
    ok(&d.handle(&unload));
    assert_eq!(
        changes(&d, &epoch, &mut revs),
        [json!({ "graph": "g", "rev": g_rev + 1, "overflow": true })]
    );
    assert!(changes(&d, &epoch, &mut revs).is_empty());
    revs.remove("g");
    seen.insert(std::mem::discriminant(unload.op.as_ref().unwrap()));
    let made = op(
        Op::GraphNew {
            path: root.join("n").display().to_string(),
            alias: "n".into(),
            title: None,
            empty: false,
            embedding_model: None,
            secured: false,
            no_graphfile: false,
            graphfile: false,
        },
        None,
    );
    let env = d.handle(&made);
    assert!(
        changes(&d, &epoch, &mut revs).is_empty(),
        "a new graph n is not h"
    );
    assert!(ok(&env).is_object());
    let mut n: BTreeMap<String, u64> = [("n".into(), 0)].into();
    assert_eq!(
        changes(&d, &epoch, &mut n),
        [json!({ "graph": "n", "rev": 1, "overflow": true })],
        "a new graph's load is its first step"
    );
    seen.insert(std::mem::discriminant(made.op.as_ref().unwrap()));
    assert_eq!(
        rev(&d, "h"),
        1,
        "h only had its load and its one node: {h_node}"
    );
    assert_eq!(seen.len(), 45, "every Op variant with writes()");
    let _ = fs::remove_dir_all(root);
}

fn request_dry(r: &Request) -> bool {
    matches!(r.op, Some(Op::Batch { dry_run: true, .. }))
}

#[test]
fn a_cross_graph_edge_is_a_change_only_for_the_graph_it_is_written_in() {
    let (d, root, epoch, mut revs) = daemon("cross");
    let target = ok(&d.handle(&op(node_add("In g"), Some("g"))))["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let from = ok(&d.handle(&op(node_add("In h"), Some("h"))))["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    changes(&d, &epoch, &mut revs);
    let g_rev = revs["g"];
    let added = d.handle(&op(
        Op::EdgeAdd {
            from: format!("umx:h/{from}"),
            relation: "blocks".into(),
            to: format!("umx:g/{target}"),
            sets: Vec::new(),
            evidence: Vec::new(),
            evidence_kind: None,
        },
        Some("h"),
    ));
    let edge = ok(&added)["edge"]["id"].as_str().unwrap().to_owned();
    // A view of g alone (security T1, T3): nothing, and g's revision has not moved.
    let g_only: BTreeMap<String, u64> = [("g".into(), g_rev)].into();
    let seen = d.handle(&wait(&epoch, &["g"], &g_only, 0));
    assert_eq!(ok(&seen)["changes"], json!([]));
    assert!(!seen.to_json().contains(&edge));
    let c = changes(&d, &epoch, &mut revs);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(
        (&c[0]["graph"], &c[0]["edges"]["added"]),
        (&json!("h"), &json!([edge]))
    );
    assert_eq!(revs["g"], g_rev);
    // Ids and revisions only: no text, no note, no writer (security T2).
    let keys: Vec<&String> = c[0].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["edges", "from_rev", "graph", "nodes", "to_rev"]);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_failed_write_gives_no_change_and_keeps_the_revision() {
    use std::os::unix::fs::PermissionsExt;
    let (d, root, epoch, mut revs) = daemon("failed");
    let folder = root.join("g");
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o555)).unwrap();
    let before = rev(&d, "g");
    let failed = d.handle(&op(node_add("Never written"), Some("g")));
    let refused = d.handle(&op(
        batch(
            &[r#"{"op":"node add","type":"problem","title":"Never"}"#.into()],
            false,
        ),
        Some("g"),
    ));
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!failed.errors.is_empty() && !refused.errors.is_empty());
    assert_eq!(rev(&d, "g"), before);
    assert!(changes(&d, &epoch, &mut revs).is_empty());
    let bad = d.handle(&op(
        batch(
            &[
                r#"{"op":"node add","type":"problem","title":"Rolled back"}"#.into(),
                r#"{"op":"edge delete","edge":"01ZZZZZZZZZZZZZZZZZZZZZZZZ"}"#.into(),
            ],
            false,
        ),
        Some("g"),
    ));
    assert!(!bad.errors.is_empty(), "{}", bad.to_json());
    assert_eq!(rev(&d, "g"), before, "a rolled-back batch");
    assert!(changes(&d, &epoch, &mut revs).is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_write_wakes_a_waiting_view_and_an_unload_tells_it_to_reload() {
    let (d, root, epoch, revs) = daemon("wake");
    let g_only: BTreeMap<String, u64> = [("g".into(), revs["g"])].into();
    std::thread::scope(|s| {
        let waiting = s.spawn(|| {
            let started = Instant::now();
            (
                d.handle(&wait(&epoch, &["g"], &g_only, 20_000)),
                started.elapsed(),
            )
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(!waiting.is_finished());
        let id = ok(&d.handle(&op(node_add("Late"), Some("g"))))["node"]["id"].clone();
        let (got, took) = waiting.join().unwrap();
        assert!(took < Duration::from_secs(2), "woke after {took:?}");
        assert_eq!(ok(&got)["changes"][0]["nodes"]["added"], json!([id]));
        assert_eq!(got.data["events"], json!([]));
    });
    let now: BTreeMap<String, u64> = [("g".into(), revs["g"] + 1)].into();
    std::thread::scope(|s| {
        let waiting = s.spawn(|| d.handle(&wait(&epoch, &["g"], &now, 20_000)));
        std::thread::sleep(Duration::from_millis(300));
        ok(&d.handle(&op(Op::GraphUnload { alias: "g".into() }, None)));
        let got = waiting.join().unwrap();
        assert_eq!(
            ok(&got)["changes"],
            json!([{ "graph": "g", "rev": revs["g"] + 2, "overflow": true }])
        );
    });
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_10k_batch_is_one_overflow_and_the_agents_commands_around_it_still_arrive() {
    let (d, root, epoch, mut revs) = daemon("burst");
    let clear = || {
        op(
            Op::UiCommand {
                command: UiCmd::Clear { note: None },
            },
            Some("g"),
        )
    };
    ok(&d.handle(&clear()));
    let lines: Vec<String> = (0..10_000)
        .map(|i| format!(r#"{{"op":"node add","type":"problem","title":"Item {i}"}}"#))
        .collect();
    ok(&d.handle(&op(batch(&lines, false), Some("g"))));
    ok(&d.handle(&clear()));
    let got = d.handle(&wait(&epoch, &["g", "h"], &revs, 0));
    let data = ok(&got);
    let seqs: Vec<&Value> = data["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| &e["seq"])
        .collect();
    assert_eq!(seqs, [&json!(1), &json!(2)], "both commands, in order");
    assert_eq!(
        data["changes"],
        json!([{ "graph": "g", "rev": revs["g"] + 1, "overflow": true }]),
        "one entry for g, none for h"
    );
    assert!(
        changes(&d, &epoch, &mut revs)
            .iter()
            .all(|c| c["overflow"] == true)
    );
    assert!(changes(&d, &epoch, &mut revs).is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn provenance_nodes_are_left_out_of_a_change() {
    let (d, root, epoch, mut revs) = daemon("prov");
    let agent = "01JC8XA4H7B3KT9M2NZ5QW6RDE";
    let line = format!(
        r#"{{"id":"{agent}","type":"agent","title":"Writer","status":"open","note":"","evidence":[],"status_history":[],"created":"2026-11-12T00:00:00Z","updated":"2026-11-12T00:00:00Z","created_by":"hand","prov_key":"agent/pij/pij-feed-writer","role":"before"}}"#
    );
    fs::write(root.join("g").join("nodes.jsonl"), format!("{line}\n")).unwrap();
    ok(&d.handle(&op(
        Op::GraphLoad {
            path: root.join("g").display().to_string(),
            as_alias: Some("g".into()),
            by_alias: None,
            graphfile: false,
            set: None,
        },
        None,
    )));
    assert_eq!(changes(&d, &epoch, &mut revs)[0]["overflow"], true);
    let mut request = op(
        batch(
            &[r#"{"op":"node add","type":"problem","title":"Shown"}"#.into()],
            false,
        ),
        Some("g"),
    );
    request.actor = Some(umx_protocol::ActorClaim {
        tier: "pij".into(),
        harness: Some("copilot".into()),
        session: Some("h-0123456789abcdef".into()),
        seat: Some("pij-feed-writer".into()),
        role: Some("coder".into()),
        ..Default::default()
    });
    let edited = d.handle(&request);
    let shown = ok(&edited)["rows"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        d.services
            .read()
            .unwrap()
            .graph()
            .loaded("g")
            .unwrap()
            .graph
            .node(agent.parse().unwrap())
            .unwrap()
            .field("role"),
        Some(json!("coder")),
        "the claim updates the existing provenance node"
    );
    let c = changes(&d, &epoch, &mut revs);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(c[0]["nodes"]["added"], json!([shown]));
    assert_eq!(
        c[0]["nodes"]["edited"],
        json!([]),
        "the agent node is provenance"
    );
    assert_eq!(c[0]["edges"]["added"], json!([]));
    assert_eq!(c[0]["edges"]["edited"], json!([]));
    assert!(!json!(c).to_string().contains(agent));
    let _ = fs::remove_dir_all(root);
}

/// Architect: a graph unloaded between two polls is overflow for the view that named it, beside the other
/// graph's changes; the wait is never refused for it and never loads it back (F72), even though the registry still
/// lists it as loaded (it unloaded when idle).
#[test]
fn a_graph_unloaded_between_polls_is_overflow_beside_the_others_and_stays_unloaded() {
    let (d, root, epoch, mut revs) = daemon("unloaded");
    assert!(changes(&d, &epoch, &mut revs).is_empty());
    {
        let mut s = d.services.write().unwrap();
        s.graph()
            .loaded("h")
            .unwrap()
            .last_used
            .store(u64::MAX / 2, std::sync::atomic::Ordering::Relaxed);
        s.set_idle_unload_ms(1);
        let until = Instant::now() + Duration::from_secs(20);
        while s.idle_wait_ms().is_some_and(|ms| ms > 0) {
            assert!(Instant::now() < until, "idle unload never fell due");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(s.unload_idle(), ["g"]);
        s.set_idle_unload_ms(0);
        assert!(
            s.graph().remembered_loaded("g").is_some(),
            "F72 would load g"
        );
        // As the writer's own idle unload does (Writing's drop).
        let fed = s.take_changes();
        d.ui.lock().unwrap().record(fed);
        d.ui_wake.notify_all();
    }
    let added = ok(&d.handle(&op(node_add("Still here"), Some("h"))))["node"]["id"].clone();
    let c = changes(&d, &epoch, &mut revs);
    assert_eq!(
        c,
        [
            json!({ "graph": "g", "rev": 2, "overflow": true }),
            json!({ "graph": "h", "from_rev": 1, "to_rev": 2,
                "nodes": { "added": [added], "edited": [], "removed": [] },
                "edges": { "added": [], "edited": [], "removed": [] } }),
        ]
    );
    assert!(changes(&d, &epoch, &mut revs).is_empty(), "overflow once");
    assert!(d.services.read().unwrap().graph().loaded("g").is_none());
    // A graph never loaded this epoch: overflow at 0 for a view that follows it, never an error.
    let never: BTreeMap<String, u64> = [("never".into(), 7)].into();
    let got = d.handle(&wait(&epoch, &["never"], &never, 0));
    assert_eq!(
        ok(&got)["changes"],
        json!([{ "graph": "never", "rev": 0, "overflow": true }])
    );
    // The wait is exempt from F72 even if a graph_not_loaded reached it.
    let missed = verbs::issue(&umx_services::problems::graph_not_loaded("g"), "ui wait");
    let reply = d.on_demand(
        &wait(&epoch, &["g"], &revs, 0),
        Envelope::fail("ui wait", missed),
    );
    assert_eq!(reply.errors[0].code(), "graph_not_loaded");
    assert!(
        d.services.read().unwrap().graph().loaded("g").is_none(),
        "never loaded back"
    );
    let _ = fs::remove_dir_all(root);
}

/// R252-1 check 1: an explicit unload with no wait outstanding, then a write to the other graph: one successful
/// reply with both. Check 4: alias and count refusals stand; a view that gives no revs still gets no changes.
#[test]
fn an_unload_with_no_wait_outstanding_and_the_refusals_that_stand() {
    let (d, root, epoch, mut revs) = daemon("gap");
    assert!(changes(&d, &epoch, &mut revs).is_empty());
    ok(&d.handle(&op(Op::GraphUnload { alias: "g".into() }, None)));
    let added = ok(&d.handle(&op(node_add("After"), Some("h"))))["node"]["id"].clone();
    let c = changes(&d, &epoch, &mut revs);
    assert_eq!(c.len(), 2, "{c:?}");
    assert_eq!(c[0], json!({ "graph": "g", "rev": 2, "overflow": true }));
    assert_eq!(c[1]["nodes"]["added"], json!([added]));
    assert!(d.services.read().unwrap().graph().loaded("g").is_none());
    let none = d.handle(&wait(&epoch, &["g", "h"], &BTreeMap::new(), 0));
    assert_eq!(ok(&none)["changes"], json!([]), "no revs, no changes");
    let refused = |graphs: Vec<String>, revs: BTreeMap<String, u64>| {
        let names: Vec<&str> = graphs.iter().map(String::as_str).collect();
        let env = d.handle(&wait(&epoch, &names, &revs, 0));
        env.errors.first().map(|e| e.code().to_owned())
    };
    assert_eq!(
        refused(vec!["Not-An-Alias".into()], BTreeMap::new()).as_deref(),
        Some("usage_invalid")
    );
    let many: Vec<String> = (0..65).map(|i| format!("g{i}")).collect();
    assert_eq!(
        refused(many.clone(), BTreeMap::new()).as_deref(),
        Some("usage_invalid")
    );
    let many_revs: BTreeMap<String, u64> = many.iter().map(|g| (g.clone(), 0)).collect();
    assert_eq!(
        refused(vec!["h".into()], many_revs).as_deref(),
        Some("usage_invalid")
    );
    let _ = fs::remove_dir_all(root);
}

/// Architect on d12cc07: once a view is at an unloaded graph's overflow revision it hears nothing more, but loading
/// the graph again moves the revision on: a poll gets a fresh overflow, and a waiter already sleeping wakes for it.
#[test]
fn loading_a_graph_again_after_its_overflow_is_a_fresh_overflow_that_wakes_a_waiter() {
    let (d, root, epoch, mut revs) = daemon("reload");
    ok(&d.handle(&op(Op::GraphUnload { alias: "g".into() }, None)));
    assert_eq!(
        changes(&d, &epoch, &mut revs),
        [json!({ "graph": "g", "rev": 2, "overflow": true })]
    );
    assert!(
        changes(&d, &epoch, &mut revs).is_empty(),
        "at its overflow rev"
    );
    let load = || {
        op(
            Op::GraphLoad {
                path: root.join("g").display().to_string(),
                as_alias: Some("g".into()),
                by_alias: None,
                graphfile: false,
                set: None,
            },
            None,
        )
    };
    ok(&d.handle(&load()));
    assert_eq!(
        changes(&d, &epoch, &mut revs),
        [json!({ "graph": "g", "rev": 3, "overflow": true })],
        "a poll after the load"
    );
    ok(&d.handle(&op(Op::GraphUnload { alias: "g".into() }, None)));
    assert_eq!(changes(&d, &epoch, &mut revs)[0]["rev"], 4);
    let at: BTreeMap<String, u64> = [("g".into(), revs["g"])].into();
    std::thread::scope(|s| {
        let waiting = s.spawn(|| {
            let started = Instant::now();
            (
                d.handle(&wait(&epoch, &["g"], &at, 20_000)),
                started.elapsed(),
            )
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(!waiting.is_finished(), "nothing new while g stays unloaded");
        ok(&d.handle(&load()));
        let (got, took) = waiting.join().unwrap();
        assert!(took < Duration::from_secs(2), "woke after {took:?}");
        assert_eq!(
            ok(&got)["changes"],
            json!([{ "graph": "g", "rev": 5, "overflow": true }])
        );
    });
    let _ = fs::remove_dir_all(root);
}
