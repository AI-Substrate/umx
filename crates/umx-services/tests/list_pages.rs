//! s16 (plan 166, review A1 and A3): edge list and node list take the page before they build rows.
//!
//! Seeded differentials over graphs with local, external, broken, secured-owner and provenance edges, and nodes of
//! every line shape (codec-only lines, escaped strings, list, object and null customs, a raw session id):
//! - every edge list page equals the same slice of the whole list, with the same total, truncation, graphs and
//!   warnings, and the whole list counts exactly the rows it shows;
//! - every node list page equals the same slice of the whole list, and each listed node reads every column (standard,
//!   locator, custom, absent) and every custom name exactly as its decoded node does.

use std::sync::Arc;
use umx_core::codec::{self, GraphFile};
use umx_core::ports::{GraphDir, GraphFiles};
use umx_core::value::SetValue;
use umx_core::{Edge, Id};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{
    Ctx, EdgeListRequest, ListedNode, NodeAddRequest, NodeListRequest, Ports, Scope, Services,
};

struct World {
    files: Arc<FakeGraphFiles>,
    svc: Services,
}

fn world() -> World {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let ports = Ports {
        sources: Arc::new(umx_fakes::FakeSourceReader::open(
            GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: Arc::new(FakeGraphFiles::new(GraphDir::parse("/cache").unwrap())),
        vector_cache_root: GraphDir::parse("/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new().with_absent_ok(true)),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    };
    World {
        files,
        svc: Services::new(ports),
    }
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

fn dir(path: &str) -> GraphDir {
    GraphDir::parse(path).unwrap()
}

/// xorshift64*: the same graph for the same seed on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const KINDS: [&str; 3] = ["problem", "option", "question"];
const STATUSES: [Option<&str>; 3] = [None, Some("open"), Some("agreed")];
const RELATIONS: [&str; 5] = [
    "supports",
    "blocks",
    "depends_on",
    "conflicts_with",
    "answers",
];

struct Seeded {
    w: World,
    demo: Vec<Id>,
    beta: Vec<Id>,
    parents: Vec<Id>,
}

fn node(w: &mut World, graph: &str, req: NodeAddRequest) -> Id {
    w.svc
        .node_add(&ctx().graph(graph), req)
        .unwrap()
        .value
        .node
        .id
}

fn link(w: &mut World, owner: &str, from: String, relation: &str, to: String) -> bool {
    w.svc
        .edge_add(&ctx().graph(owner), &from, relation, &to, &[])
        .is_ok()
}

fn edges_of(w: &World, alias: &str) -> Vec<Edge> {
    w.svc
        .loaded_graph(alias)
        .unwrap()
        .edges()
        .map(|edge| edge.to_edge())
        .collect()
}

fn session_of(w: &World, alias: &str, node: Id) -> Id {
    w.svc
        .node_get(&ctx().graph(alias), &node.to_text())
        .unwrap()
        .value
        .1
        .authored_by_session
        .unwrap()
}

fn seeded(seed: u64) -> Seeded {
    let mut w = world();
    let mut r = Rng(seed);
    let ids: Vec<Id> = [
        ("/work/g", "demo"),
        ("/work/beta", "beta"),
        ("/work/gamma", "gamma"),
    ]
    .into_iter()
    .map(|(path, alias)| {
        w.svc
            .graph_new(&ctx(), path, alias, None, false, None)
            .unwrap()
            .value
            .graph
            .id
    })
    .collect();
    let beta_id = ids[1];
    let parents: Vec<Id> = (0..3)
        .map(|i| {
            node(
                &mut w,
                "demo",
                NodeAddRequest {
                    kind: "problem".into(),
                    title: format!("Parent area {i}"),
                    ..Default::default()
                },
            )
        })
        .collect();
    let mut demo = parents.clone();
    for i in 0..30 {
        let mut sets = vec![("cost".to_string(), SetValue::Raw(r.below(6).to_string()))];
        if r.below(2) == 0 {
            sets.push((
                "why".into(),
                SetValue::Str(format!("Synthetic \"reason\" {i}\\ with\nbreak")),
            ));
        }
        if r.below(3) == 0 {
            sets.push((
                "tags".into(),
                SetValue::Json(serde_json::json!(["a", i, {"k": null}])),
            ));
        }
        if r.below(4) == 0 {
            sets.push(("flag".into(), SetValue::Raw("true".into())));
        }
        if r.below(5) == 0 {
            sets.push(("gone".into(), SetValue::Json(serde_json::Value::Null)));
        }
        let parent = (r.below(2) == 0).then(|| format!("umx:demo/{}", r.pick(&parents)));
        demo.push(node(
            &mut w,
            "demo",
            NodeAddRequest {
                kind: r.pick(&KINDS).to_string(),
                title: format!("Synthetic node {i} of seed {seed}"),
                status: r.pick(&STATUSES).map(String::from),
                note: (r.below(3) == 0).then(|| format!("A synthetic note {i}")),
                sets,
                parent,
                locator: (i % 11 == 0).then(|| format!("https://example.invalid/doc-{i}")),
                ..Default::default()
            },
        ));
    }
    let beta: Vec<Id> = (0..8)
        .map(|i| {
            node(
                &mut w,
                "beta",
                NodeAddRequest {
                    kind: "option".into(),
                    title: format!("Far synthetic {i}"),
                    ..Default::default()
                },
            )
        })
        .collect();
    let gamma: Vec<Id> = (0..3)
        .map(|i| {
            node(
                &mut w,
                "gamma",
                NodeAddRequest {
                    kind: "question".into(),
                    title: format!("Closed synthetic {i}"),
                    ..Default::default()
                },
            )
        })
        .collect();
    let mut added = 0;
    for _ in 0..45 {
        let (a, b) = (*r.pick(&demo), *r.pick(&demo));
        if a != b {
            added += usize::from(link(
                &mut w,
                "demo",
                a.to_text(),
                r.pick(&RELATIONS),
                b.to_text(),
            ));
        }
    }
    assert!(added > 30, "seed {seed}: only {added} local edges");
    for _ in 0..8 {
        let (a, b) = (*r.pick(&demo), *r.pick(&beta));
        let relation = *r.pick(&RELATIONS);
        link(
            &mut w,
            "demo",
            a.to_text(),
            relation,
            format!("umx:beta/{b}"),
        );
    }
    for _ in 0..4 {
        let (b, a) = (*r.pick(&beta), *r.pick(&demo));
        let relation = *r.pick(&RELATIONS);
        link(
            &mut w,
            "beta",
            b.to_text(),
            relation,
            format!("umx:demo/{a}"),
        );
    }
    for &g in &gamma {
        let a = *r.pick(&demo);
        link(
            &mut w,
            "demo",
            a.to_text(),
            "blocks",
            format!("umx:gamma/{g}"),
        );
        link(
            &mut w,
            "gamma",
            g.to_text(),
            "answers",
            format!("umx:demo/{a}"),
        );
    }
    // A demo edge whose beta end is then removed: edge_broken on read.
    let gone = beta[7];
    assert!(link(
        &mut w,
        "demo",
        demo[5].to_text(),
        "supports",
        format!("umx:beta/{gone}")
    ));
    // Stored edges onto session nodes, local and foreign: hidden unless the read includes provenance.
    let (local_session, beta_session) = (
        session_of(&w, "demo", demo[0]),
        session_of(&w, "beta", beta[0]),
    );
    let mut edges = edges_of(&w, "demo");
    let template = edges[0].clone();
    for (n, (to_graph, to)) in [
        (None, local_session),
        (Some(beta_id), beta_session),
        (None, local_session),
    ]
    .into_iter()
    .enumerate()
    {
        edges.push(Edge {
            id: Id(9000 + n as u128),
            kind: "raises".into(),
            from_graph: None,
            from: demo[3 + n],
            to_graph,
            to,
            ..template.clone()
        });
    }
    w.files.tamper_for_tests(
        &dir("/work/g"),
        GraphFile::Edges,
        &codec::encode_edges(&edges),
    );
    // Node lines in other shapes: keys out of the writer's order (left to the codec), escapes, a nested object, an
    // empty list and an invalid session id kept raw.
    let nodes = w
        .files
        .read(&dir("/work/g"), GraphFile::Nodes)
        .unwrap()
        .unwrap();
    let text = std::str::from_utf8(&nodes).unwrap();
    let mut out = String::new();
    for (i, line) in text.lines().enumerate() {
        let mut v: serde_json::Map<String, serde_json::Value> = serde_json::from_str(line).unwrap();
        let ours = v
            .get("id")
            .and_then(|id| serde_json::from_value::<Id>(id.clone()).ok())
            .is_some_and(|id| demo.contains(&id));
        if ours && i % 4 == 0 {
            v.insert(
                "esc".into(),
                serde_json::json!("tab\tquote\" back\\ \u{e9}"),
            );
            v.insert(
                "deep".into(),
                serde_json::json!({"a": [1, {"b": null}], "z": "y"}),
            );
            v.insert("none".into(), serde_json::json!([]));
            if i % 8 == 0 {
                v.insert(
                    "authored_by_session".into(),
                    serde_json::json!("not-a-session"),
                );
            }
            out.push_str(&serde_json::to_string(&v).unwrap());
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    w.files
        .tamper_for_tests(&dir("/work/g"), GraphFile::Nodes, out.as_bytes());
    let beta_nodes = w
        .files
        .read(&dir("/work/beta"), GraphFile::Nodes)
        .unwrap()
        .unwrap();
    let kept: String = std::str::from_utf8(&beta_nodes)
        .unwrap()
        .lines()
        .filter(|line| !line.contains(&gone.to_text()))
        .map(|line| format!("{line}\n"))
        .collect();
    w.files
        .tamper_for_tests(&dir("/work/beta"), GraphFile::Nodes, kept.as_bytes());
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    w.svc.graph_load(&ctx(), "/work/beta", None).unwrap();
    let given = [("policy".to_string(), "secured".to_string())];
    w.svc.graph_set(&ctx(), "gamma", &given).unwrap();
    Seeded {
        w,
        demo,
        beta: beta[..7].to_vec(),
        parents,
    }
}

fn contexts() -> Vec<Ctx> {
    let scope = Scope::Graphs(vec!["demo".into(), "beta".into()]);
    vec![
        ctx().graph("demo"),
        ctx().graph("demo").with_provenance(true),
        ctx().graph("beta"),
        ctx().graph("beta").with_provenance(true),
        ctx().graph("demo,beta"),
        ctx().graph("demo,beta").with_provenance(true),
        ctx().graph("demo,gamma"),
        ctx().scope(scope.clone()),
        ctx().scope(scope).with_provenance(true),
    ]
}

/// (limit, offset) pairs around the edges of a list of `total` rows.
fn windows(total: usize) -> Vec<(usize, usize)> {
    vec![
        (0, 0),
        (1, 0),
        (1, 1),
        (3, 2),
        (7, 5),
        (100, 0),
        (5, total.saturating_sub(2)),
        (5, total.saturating_sub(1)),
        (5, total),
        (5, total + 3),
        (usize::MAX, 3),
        (total, 0),
    ]
}

fn broken(warnings: &[umx_core::Problem]) -> usize {
    warnings.iter().filter(|p| p.code == "edge_broken").count()
}

fn others(warnings: &[umx_core::Problem]) -> Vec<&str> {
    warnings
        .iter()
        .filter(|p| p.code != "edge_broken")
        .map(|p| p.code)
        .collect()
}

fn edge_pages_match(w: &World, c: &Ctx, req: &EdgeListRequest) -> usize {
    let with = |limit, offset| {
        w.svc.edge_list(
            c,
            EdgeListRequest {
                limit,
                offset,
                ..req.clone()
            },
        )
    };
    let whole = match with(usize::MAX, 0) {
        Ok(whole) => whole,
        Err(p) => {
            for (limit, offset) in windows(0) {
                assert_eq!(with(limit, offset).unwrap_err(), p, "{c:?} {req:?}");
            }
            return 0;
        }
    };
    let total = whole.value.total.unwrap();
    assert_eq!(
        total,
        whole.value.rows.len(),
        "{c:?} {req:?}: counts only rows it shows"
    );
    for (limit, offset) in windows(total) {
        let done = with(limit, offset).unwrap();
        let page = &done.value;
        let at = offset.min(total);
        let end = total.min(offset.saturating_add(limit));
        let want = &whole.value.rows[at..end.max(at)];
        let at = format!("{c:?} {req:?} limit {limit} offset {offset}");
        assert_eq!(page.rows, want, "{at}");
        assert_eq!(
            (&page.alias, page.total, page.truncated),
            (
                &whole.value.alias,
                Some(total),
                offset.saturating_add(want.len()) < total
            ),
            "{at}"
        );
        if let Some(graphs) = &whole.value.graphs {
            let paged = page.graphs.as_ref().unwrap();
            assert_eq!(
                paged.rows,
                graphs.rows[offset.min(total)..end.max(offset.min(total))],
                "{at}"
            );
            assert_eq!(paged.copies, graphs.copies, "{at}");
        }
        let states = want
            .iter()
            .flat_map(|row| [row.from_state, row.to_state])
            .filter(|state| *state == Some("edge_broken"))
            .count();
        assert_eq!(broken(&done.notes), states, "{at}");
        assert_eq!(others(&done.notes), others(&whole.notes), "{at}");
    }
    total
}

#[test]
fn every_edge_list_page_is_the_same_slice_of_the_whole_list() {
    for seed in [7, 1066, 0x5eed_5eed] {
        let s = seeded(seed);
        let mut r = Rng(seed ^ 0xa5a5);
        let mut reqs = vec![EdgeListRequest::default()];
        for kind in [
            "supports",
            "blocks",
            "part_of",
            "raises",
            "authored_by",
            "answers",
        ] {
            reqs.push(EdgeListRequest {
                kind: Some(kind.into()),
                ..Default::default()
            });
        }
        for _ in 0..4 {
            let a = format!("umx:demo/{}", r.pick(&s.demo));
            let b = format!("umx:beta/{}", r.pick(&s.beta));
            reqs.push(EdgeListRequest {
                from: Some(a.clone()),
                ..Default::default()
            });
            reqs.push(EdgeListRequest {
                to: Some(a.clone()),
                ..Default::default()
            });
            reqs.push(EdgeListRequest {
                from: Some(a.clone()),
                kind: Some(r.pick(&RELATIONS).to_string()),
                ..Default::default()
            });
            reqs.push(EdgeListRequest {
                from: Some(b.clone()),
                ..Default::default()
            });
            reqs.push(EdgeListRequest {
                to: Some(b),
                ..Default::default()
            });
        }
        let parent = format!("umx:demo/{}", s.parents[0]);
        reqs.push(EdgeListRequest {
            to: Some(parent),
            kind: Some("part_of".into()),
            ..Default::default()
        });
        let mut seen = std::collections::BTreeMap::new();
        for c in contexts() {
            for req in &reqs {
                let total = edge_pages_match(&s.w, &c, req);
                *seen
                    .entry(format!("{:?}", c.include_provenance))
                    .or_insert(0) += total;
            }
        }
        // The seeded graph exercises the filter: some edges are hidden unless the read includes provenance, and some
        // rows carry external states.
        let demo = |c: &Ctx| {
            s.w.svc
                .edge_list(
                    c,
                    EdgeListRequest {
                        limit: usize::MAX,
                        ..Default::default()
                    },
                )
                .unwrap()
                .value
        };
        let raises = |c: &Ctx| {
            demo(c)
                .rows
                .iter()
                .filter(|row| row.edge.kind == "raises")
                .count()
        };
        // Local session ends show with provenance; the foreign one only when its graph is named too.
        assert_eq!(
            [
                raises(&ctx().graph("demo")),
                raises(&ctx().graph("demo").with_provenance(true)),
                raises(&ctx().graph("demo,beta").with_provenance(true)),
            ],
            [0, 2, 3],
            "seed {seed}: the session edges"
        );
        let (plain, shown) = (
            demo(&ctx().graph("demo")),
            demo(&ctx().graph("demo").with_provenance(true)),
        );
        assert!(
            shown.rows.iter().any(|row| row.edge.id.is_none()),
            "seed {seed}: derived rows"
        );
        for state in ["edge_broken", "graph_not_loaded"] {
            assert!(
                plain.rows.iter().any(|row| row.to_state == Some(state)),
                "seed {seed}: a {state} row"
            );
        }
        assert!(seen.values().all(|total| *total > 0));
    }
}

/// The names a node column can ask for: every standard name, every locator part, the graph's custom names, the
/// daemon's own columns and a name no node holds.
fn names(w: &World) -> Vec<String> {
    let mut names: Vec<String> = [
        "id",
        "type",
        "title",
        "status",
        "note",
        "evidence",
        "evidence_kinds",
        "status_history",
        "created",
        "updated",
        "created_by",
        "authored_by_session",
        "edited_by_session",
        "copy",
        "graph",
        "address",
        "absent_field",
        "cost",
        "why",
        "tags",
        "flag",
        "gone",
        "esc",
        "deep",
        "none",
    ]
    .map(String::from)
    .to_vec();
    names.extend(
        umx_core::locator::LOCATOR_NAMES
            .iter()
            .map(|n| n.to_string()),
    );
    for alias in ["demo", "beta"] {
        for n in w.svc.loaded_graph(alias).unwrap().nodes() {
            names.extend(n.custom_field_names().map(String::from));
        }
    }
    names.sort();
    names.dedup();
    names
}

fn same_as_decoded(n: ListedNode<'_>, names: &[String], at: &str) {
    let node = n.to_node();
    assert_eq!(n.id(), node.id, "{at}");
    for name in names {
        assert_eq!(
            n.field(name),
            node.field(name),
            "{at} field {name} of {}",
            node.id
        );
    }
    assert!(
        n.custom_field_names()
            .eq(node.fields.keys().map(String::as_str)),
        "{at} custom names of {}",
        node.id
    );
}

fn node_pages_match(w: &World, c: &Ctx, req: &NodeListRequest, names: &[String]) -> usize {
    let with = |limit, offset| {
        w.svc.node_list(
            c,
            NodeListRequest {
                limit,
                offset,
                ..req.clone()
            },
        )
    };
    let whole = match with(usize::MAX, 0) {
        Ok(whole) => whole,
        Err(p) => {
            for (limit, offset) in windows(0) {
                assert_eq!(with(limit, offset).unwrap_err(), p, "{c:?} {req:?}");
            }
            return 0;
        }
    };
    let total = whole.value.total.unwrap();
    assert_eq!(total, whole.value.rows.len(), "{c:?} {req:?}");
    for n in &whole.value.rows {
        same_as_decoded(*n, names, &format!("{c:?} {req:?}"));
    }
    let ids: Vec<Id> = whole.value.rows.iter().map(|n| n.id()).collect();
    for (limit, offset) in windows(total) {
        let done = with(limit, offset).unwrap();
        let page = &done.value;
        let at = offset.min(total);
        let end = total.min(offset.saturating_add(limit)).max(at);
        let where_ = format!("{c:?} {req:?} limit {limit} offset {offset}");
        assert_eq!(
            page.rows.iter().map(|n| n.id()).collect::<Vec<_>>(),
            ids[at..end],
            "{where_}"
        );
        assert_eq!(
            (page.total, page.truncated),
            (Some(total), offset.saturating_add(end - at) < total),
            "{where_}"
        );
        assert_eq!(
            done.notes.iter().map(|p| p.code).collect::<Vec<_>>(),
            whole.notes.iter().map(|p| p.code).collect::<Vec<_>>(),
            "{where_}"
        );
    }
    total
}

#[test]
fn every_node_list_page_reads_each_column_as_its_decoded_node() {
    for seed in [7, 1066, 0x5eed_5eed] {
        let s = seeded(seed);
        let names = names(&s.w);
        let mut reqs = vec![NodeListRequest::default()];
        for kind in KINDS {
            reqs.push(NodeListRequest {
                kind: Some(kind.into()),
                ..Default::default()
            });
        }
        reqs.push(NodeListRequest {
            status: Some("open".into()),
            ..Default::default()
        });
        reqs.push(NodeListRequest {
            kind: Some("problem".into()),
            wheres: vec!["cost>=3".into()],
            ..Default::default()
        });
        for &parent in &s.parents {
            reqs.push(NodeListRequest {
                within: Some(format!("umx:demo/{parent}")),
                ..Default::default()
            });
        }
        let mut listed = 0;
        for c in contexts() {
            for req in &reqs {
                listed += node_pages_match(&s.w, &c, req, &names);
            }
        }
        assert!(listed > 0);
        // The odd line shapes are listed: a raw session id read back as a custom field, escapes and a nested object.
        let all =
            s.w.svc
                .node_list(
                    &ctx().graph("demo"),
                    NodeListRequest {
                        limit: usize::MAX,
                        ..Default::default()
                    },
                )
                .unwrap()
                .value;
        assert!(all.rows.iter().any(|n| n.field("authored_by_session")
            == Some(serde_json::json!("not-a-session"))));
        assert!(all.rows.iter().any(|n| n.field("deep").is_some()));
        assert!(all.rows.iter().any(|n| n.field("locator_kind").is_some()));
    }
}
